//! [`VirtualizationBackend`] implementation on QEMU.

use crate::cmdline::{self, Endpoint, LaunchPlan, VncEndpoint};
use crate::detect;
use crate::firmware::{self, Firmware};
use async_trait::async_trait;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tw_core::backend::{Capabilities, StartOptions, StopMode, Support, VirtualizationBackend, VmState, VmStatus};
use tw_core::control::{ComputerControl, InteractiveConsole};
use tw_core::host::{Arch, HostInfo, HostOs};
use tw_core::model::{BackendKind, MachineConfig};
use tw_core::secure;
use tw_core::store::MachinePaths;
use tw_core::{ErrorCode, TwError, TwResult};

const QMP_TIMEOUT: Duration = Duration::from_secs(5);
const FIRMWARE_FILE: &str = "firmware.json";

pub struct QemuBackend {
    host: HostInfo,
    /// `<data>/firmware`: firmware installed by `terminal-windows firmware install`.
    firmware_dir: PathBuf,
}

impl QemuBackend {
    pub fn new(host: HostInfo, firmware_dir: PathBuf) -> Self {
        Self { host, firmware_dir }
    }

    fn accelerator(&self) -> TwResult<&'static str> {
        detect::native_accelerator(&self.host)
            .ok_or_else(|| TwError::new(ErrorCode::Incompatible, "no hardware accelerator is known for this host OS"))
    }

    fn endpoints(&self, paths: &MachinePaths) -> (Endpoint, Endpoint, VncEndpoint) {
        if self.host.os == HostOs::Windows {
            let tag = pipe_tag(&paths.dir);
            (
                Endpoint::NamedPipe(format!("tw-{tag}-qmp")),
                Endpoint::NamedPipe(format!("tw-{tag}-agent")),
                // Display number derived from the tag, password-protected; see docs/security.md.
                VncEndpoint::LoopbackWithPassword { display: 100 + (u16::from_str_radix(&tag[..3], 16).unwrap_or(0) % 400) },
            )
        } else {
            (Endpoint::Unix(paths.qmp_socket()), Endpoint::Unix(paths.agent_socket()), VncEndpoint::Unix(paths.vnc_socket()))
        }
    }

    /// Loads the firmware chosen at creation. Package managers embed versioned
    /// paths (Homebrew: `Cellar/qemu/<version>/…`), so after a QEMU upgrade the
    /// recorded file disappears; the same firmware build is then located again
    /// by file name among the current descriptors and the record is updated.
    fn load_firmware(&self, paths: &MachinePaths, install: &detect::QemuInstall, arch: Arch) -> TwResult<Firmware> {
        let file = paths.dir.join(FIRMWARE_FILE);
        let text = std::fs::read_to_string(&file).map_err(|e| TwError::io(file.display(), e))?;
        let fw: Firmware = serde_json::from_str(&text).map_err(|e| TwError::invalid(format!("{}: {e}", file.display())))?;
        if fw.code.is_file() {
            return Ok(fw);
        }
        let current = firmware::discover(&firmware::descriptor_dirs(install.prefix.as_deref(), Some(&self.firmware_dir)), arch);
        match relocate(&fw, &current) {
            Some(moved) => {
                tracing::info!(from = %fw.code.display(), to = %moved.code.display(), "firmware moved (QEMU upgraded?); using the new location");
                secure::write_private_atomic(&file, serde_json::to_string_pretty(&moved).unwrap_or_default().as_bytes())?;
                Ok(moved)
            }
            None => {
                Err(TwError::new(ErrorCode::MissingDependency, format!("firmware {} recorded at creation is missing", fw.code.display()))
                    .with_hint("Reinstall QEMU. The VM's UEFI variables were created for that firmware build."))
            }
        }
    }

    fn select_firmware(&self, install: &detect::QemuInstall, arch: Arch) -> TwResult<Firmware> {
        let found = firmware::discover(&firmware::descriptor_dirs(install.prefix.as_deref(), Some(&self.firmware_dir)), arch);
        let fw = found.into_iter().next().ok_or_else(|| {
            TwError::new(ErrorCode::MissingDependency, format!("no UEFI firmware descriptor for {} was found", arch.as_str()))
                .with_hint("Install a QEMU build that ships edk2 firmware (Homebrew and the official Windows installer do).")
        })?;
        if !fw.secure_boot {
            // Windows 11 Setup stops with "PC must support Secure Boot" (measured 2026-11-05
            // with Homebrew's firmware). Refuse early instead of failing hours into a setup.
            return Err(TwError::new(
                ErrorCode::MissingDependency,
                format!("the available UEFI firmware ({}) does not support Secure Boot, which Windows 11 requires", fw.code.display()),
            )
            .with_hint("Run: terminal-windows firmware install   (Secure Boot capable firmware built for QEMU's virt machine)"));
        }
        Ok(fw)
    }

    #[cfg(unix)]
    async fn connect(&self, paths: &MachinePaths) -> TwResult<Option<crate::qmp::Qmp<tokio::net::UnixStream>>> {
        let sock = paths.qmp_socket();
        if !sock.exists() {
            return Ok(None);
        }
        match crate::qmp::connect_unix(&sock, QMP_TIMEOUT).await {
            Ok(q) => Ok(Some(q)),
            Err(e) if e.code == ErrorCode::Io => Ok(None), // stale socket: nothing listening
            Err(e) => Err(e),
        }
    }

    #[cfg(windows)]
    async fn connect(&self, paths: &MachinePaths) -> TwResult<Option<crate::qmp::Qmp<tokio::net::windows::named_pipe::NamedPipeClient>>> {
        let (Endpoint::NamedPipe(name), _, _) = self.endpoints(paths) else { unreachable!() };
        match crate::qmp::connect_pipe(&name, QMP_TIMEOUT).await {
            Ok(q) => Ok(Some(q)),
            Err(e) if e.code == ErrorCode::Io => Ok(None),
            Err(e) => Err(e),
        }
    }

    #[cfg(unix)]
    async fn connect_view(&self, paths: &MachinePaths) -> TwResult<Option<crate::qmp::Qmp<tokio::net::UnixStream>>> {
        let sock = paths.qmp_view_socket();
        if !sock.exists() {
            return Ok(None);
        }
        match crate::qmp::connect_unix(&sock, QMP_TIMEOUT).await {
            Ok(q) => Ok(Some(q)),
            Err(e) if e.code == ErrorCode::Io => Ok(None),
            Err(e) => Err(e),
        }
    }

    #[cfg(windows)]
    async fn connect_view(
        &self,
        paths: &MachinePaths,
    ) -> TwResult<Option<crate::qmp::Qmp<tokio::net::windows::named_pipe::NamedPipeClient>>> {
        let name = format!("tw-{}-qmpv", pipe_tag(&paths.dir));
        match crate::qmp::connect_pipe(&name, QMP_TIMEOUT).await {
            Ok(q) => Ok(Some(q)),
            Err(e) if e.code == ErrorCode::Io => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn require_qmp(
        &self,
        config: &MachineConfig,
        paths: &MachinePaths,
    ) -> TwResult<impl std::ops::DerefMut<Target = crate::qmp::Qmp<impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static>>>
    {
        match self.connect(paths).await? {
            Some(q) => Ok(Box::new(q)),
            None => {
                Err(TwError::state(format!("VM '{}' is not running", config.name))
                    .with_hint(format!("terminal-windows start {}", config.name)))
            }
        }
    }
}

/// Finds the same firmware build (same code/vars file names and features) at a new location.
fn relocate(old: &Firmware, current: &[Firmware]) -> Option<Firmware> {
    current
        .iter()
        .find(|f| {
            f.code.file_name() == old.code.file_name()
                && f.vars_template.file_name() == old.vars_template.file_name()
                && f.secure_boot == old.secure_boot
                && f.requires_smm == old.requires_smm
        })
        .cloned()
}

/// Short stable tag for per-VM pipe names on Windows (pipes are a global namespace).
fn pipe_tag(dir: &Path) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in dir.to_string_lossy().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:012x}", h & 0xffff_ffff_ffff)
}

fn map_state(status: &str) -> VmState {
    match status {
        "running" => VmState::Running,
        "paused" | "suspended" | "save-vm" | "restore-vm" => VmState::Paused,
        "shutdown" => VmState::ShuttingDown,
        "prelaunch" | "inmigrate" | "finish-migrate" | "postmigrate" => VmState::Starting,
        _ => VmState::Error,
    }
}

fn read_pid(paths: &MachinePaths) -> Option<u32> {
    std::fs::read_to_string(paths.pid_file()).ok()?.trim().parse().ok()
}

fn append_log(paths: &MachinePaths, text: &str) {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut f) = options.open(&paths.log) {
        let _ = writeln!(f, "{text}");
    }
}

fn detach(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

async fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    path.exists()
}

#[async_trait]
impl VirtualizationBackend for QemuBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Qemu
    }

    async fn capabilities(&self) -> Capabilities {
        let arch = self.host.arch;
        let install = match arch {
            Some(a) => detect::find_qemu(&self.host, a).await.ok(),
            None => None,
        };
        let accel = match (&install, detect::native_accelerator(&self.host)) {
            (Some(i), Some(name)) => match detect::probe_accelerator(&i.system, name).await {
                Ok(()) => Support::Supported,
                Err(e) => Support::unsupported(e.to_string()),
            },
            (None, _) => Support::unsupported("QEMU is not installed"),
            (_, None) => Support::unsupported("no accelerator for this host OS"),
        };
        let uefi = match (&install, arch) {
            (Some(i), Some(a)) => {
                let fws = firmware::discover(&firmware::descriptor_dirs(i.prefix.as_deref(), Some(&self.firmware_dir)), a);
                match fws.first() {
                    Some(f) if f.secure_boot => Support::unverified(format!("Secure Boot capable firmware found: {}", f.code.display())),
                    Some(f) => Support::unverified(format!("UEFI firmware without Secure Boot: {}", f.code.display())),
                    None => Support::unsupported("no UEFI firmware descriptor found"),
                }
            }
            _ => Support::unsupported("QEMU is not installed"),
        };
        let tpm = match detect::find_swtpm(&self.host) {
            Some(p) => Support::unverified(format!("swtpm found at {}", p.display())),
            None if self.host.os == HostOs::Windows => {
                Support::unsupported("swtpm is not available for Windows hosts; use the Hyper-V backend for Windows 11 (planned)")
            }
            None => Support::unsupported("swtpm not found (brew install swtpm)"),
        };
        let pending = "implemented; not yet validated on this host (see docs/compatibility.md)";
        Capabilities {
            backend: BackendKind::Qemu,
            guest_arch: arch.into_iter().collect(),
            hardware_acceleration: accel,
            uefi,
            tpm,
            graphics: Support::unverified(pending),
            headless: Support::unverified(pending),
            networking: Support::unverified(pending),
            guest_agent_transport: Support::unverified("virtio-serial; the Windows agent has not been validated yet"),
            snapshots: Support::unsupported("not implemented yet (planned: qcow2 internal snapshots)"),
            shared_folders: Support::unsupported("disabled by design; see docs/security.md"),
        }
    }

    async fn preflight(&self, config: &MachineConfig) -> TwResult<()> {
        self.host.check_guest_arch(config.arch)?;
        let install = detect::find_qemu(&self.host, config.arch).await?;
        detect::probe_accelerator(&install.system, self.accelerator()?).await?;
        if config.tpm && detect::find_swtpm(&self.host).is_none() {
            return Err(TwError::new(ErrorCode::MissingDependency, "Windows 11 needs a TPM 2.0, but swtpm was not found").with_hint(
                if self.host.os == HostOs::Windows {
                    "swtpm has no official Windows build. Windows 11 guests on Windows hosts are planned through the Hyper-V backend."
                } else {
                    "brew install swtpm   (or set TW_SWTPM)"
                },
            ));
        }
        self.select_firmware(&install, config.arch)?;
        Ok(())
    }

    async fn create(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<()> {
        let install = detect::find_qemu(&self.host, config.arch).await?;
        let fw = self.select_firmware(&install, config.arch)?;
        let vars = std::fs::read(&fw.vars_template).map_err(|e| TwError::io(fw.vars_template.display(), e))?;
        secure::write_private_atomic(&paths.efivars, &vars)?;
        secure::write_private_atomic(&paths.dir.join(FIRMWARE_FILE), serde_json::to_string_pretty(&fw).unwrap_or_default().as_bytes())?;
        secure::create_private_dir(&paths.tpm_dir)?;
        secure::write_private_atomic(&paths.agent_key, secure::generate_key_hex().as_bytes())?;
        let args = cmdline::qemu_img_create_args(&paths.disk, config.disk_gib);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        detect::run_capture(&install.img, &refs, Duration::from_secs(60)).await?;
        Ok(())
    }

    async fn start(&self, config: &MachineConfig, paths: &MachinePaths, _options: &StartOptions) -> TwResult<VmStatus> {
        self.host.check_guest_arch(config.arch)?;
        let install = detect::find_qemu(&self.host, config.arch).await?;
        let accel = self.accelerator()?;
        let fw = self.load_firmware(paths, &install, config.arch)?;
        for iso in [&config.install_iso, &config.drivers_iso].into_iter().flatten() {
            if !iso.is_file() {
                return Err(TwError::not_found(format!("attached media is missing: {}", iso.display()))
                    .with_hint(format!("Restore the file or detach it: terminal-windows config {} --detach-media", config.name)));
            }
        }
        append_log(paths, &format!("=== start {} (QEMU {:?}, accel {accel})", config.name, install.version));

        let tpm_socket = if config.tpm {
            let swtpm = detect::find_swtpm(&self.host).ok_or_else(|| TwError::new(ErrorCode::MissingDependency, "swtpm was not found"))?;
            let sock = paths.swtpm_socket();
            let _ = std::fs::remove_file(&sock);
            let args = cmdline::swtpm_args(&paths.tpm_dir, &sock, &paths.dir.join("swtpm.log"))?;
            let mut cmd = tokio::process::Command::new(&swtpm);
            cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            detach(&mut cmd);
            cmd.spawn().map_err(|e| TwError::io(swtpm.display(), e))?;
            if !wait_for_path(&sock, Duration::from_secs(5)).await {
                return Err(TwError::backend("swtpm did not create its control socket")
                    .with_hint(format!("See {}", paths.dir.join("swtpm.log").display())));
            }
            Some(sock)
        } else {
            None
        };

        let (qmp, agent, vnc) = self.endpoints(paths);
        let qmp_view = Some(if self.host.os == HostOs::Windows {
            Endpoint::NamedPipe(format!("tw-{}-qmpv", pipe_tag(&paths.dir)))
        } else {
            Endpoint::Unix(paths.qmp_view_socket())
        });
        let daemonize = cfg!(unix);
        let plan = LaunchPlan {
            config,
            accelerator: accel,
            firmware: &fw,
            disk: &paths.disk,
            efivars: &paths.efivars,
            pid_file: &paths.pid_file(),
            qmp,
            qmp_view,
            agent,
            vnc,
            tpm_socket,
            daemonize,
            qemu_version: install.version,
        };
        let args = cmdline::qemu_args(&plan)?;
        append_log(paths, &format!("{} {}", install.system.display(), args.join(" ")));
        let mut cmd = tokio::process::Command::new(&install.system);
        cmd.args(&args).stdin(Stdio::null());
        detach(&mut cmd);
        if daemonize {
            // With -daemonize the foreground process exits only after the VM
            // initialised, so configuration errors are reported synchronously.
            let out = tokio::time::timeout(Duration::from_secs(60), cmd.output())
                .await
                .map_err(|_| TwError::new(ErrorCode::Timeout, "QEMU did not finish starting within 60 s"))?
                .map_err(|e| TwError::io(install.system.display(), e))?;
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.trim().is_empty() {
                append_log(paths, stderr.trim());
            }
            if !out.status.success() {
                return Err(TwError::backend(format!("QEMU failed to start: {}", stderr.trim()))
                    .with_hint(format!("Full log: {}", paths.log.display())));
            }
        } else {
            let log =
                std::fs::OpenOptions::new().create(true).append(true).open(&paths.log).map_err(|e| TwError::io(paths.log.display(), e))?;
            let log2 = log.try_clone().map_err(|e| TwError::io(paths.log.display(), e))?;
            cmd.stdout(log).stderr(log2);
            cmd.spawn().map_err(|e| TwError::io(install.system.display(), e))?;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let status = self.status(config, paths).await?;
            if status.state != VmState::Stopped || tokio::time::Instant::now() >= deadline {
                if status.state == VmState::Stopped {
                    return Err(TwError::backend("QEMU started but its monitor never became reachable")
                        .with_hint(format!("See {}", paths.log.display())));
                }
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn stop(&self, config: &MachineConfig, paths: &MachinePaths, mode: StopMode) -> TwResult<VmStatus> {
        let mut qmp = self.require_qmp(config, paths).await?;
        let (state, detail) = match mode {
            StopMode::Graceful => {
                qmp.execute("system_powerdown", None).await?;
                (VmState::ShuttingDown, "ACPI power button sent; Windows decides when to power off")
            }
            StopMode::Force => {
                // The connection closes as QEMU exits; that is the expected outcome.
                let _ = qmp.execute("quit", None).await;
                (VmState::Stopped, "powered off immediately")
            }
        };
        Ok(VmStatus { name: config.name.to_string(), state, pid: read_pid(paths), detail: Some(detail.into()) })
    }

    async fn pause(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<VmStatus> {
        self.require_qmp(config, paths).await?.execute("stop", None).await?;
        self.status(config, paths).await
    }

    async fn resume(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<VmStatus> {
        self.require_qmp(config, paths).await?.execute("cont", None).await?;
        self.status(config, paths).await
    }

    async fn status(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<VmStatus> {
        let name = config.name.to_string();
        let Some(mut qmp) = self.connect(paths).await? else {
            return Ok(VmStatus { name, state: VmState::Stopped, pid: None, detail: None });
        };
        let (status, _) = qmp.query_status().await?;
        Ok(VmStatus { name, state: map_state(&status), pid: read_pid(paths), detail: Some(format!("qemu status: {status}")) })
    }

    async fn interactive_console(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<Box<dyn InteractiveConsole>> {
        match self.connect_view(paths).await? {
            Some(qmp) => Ok(Box::new(crate::console::QmpConsole::new(qmp, paths.runtime.clone()))),
            None => Err(TwError::state(format!("VM '{}' has no viewer monitor socket", config.name)).with_hint(format!(
                "It was started by an older terminal-windows. Restart it once: terminal-windows stop {0} && terminal-windows start {0}",
                config.name
            ))),
        }
    }

    async fn console(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<Box<dyn ComputerControl>> {
        match self.connect(paths).await? {
            Some(qmp) => Ok(Box::new(crate::console::QmpConsole::new(qmp, paths.runtime.clone()))),
            None => Err(TwError::state(format!("VM '{}' is not running", config.name))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_states_map_to_vm_states() {
        assert_eq!(map_state("running"), VmState::Running);
        assert_eq!(map_state("paused"), VmState::Paused);
        assert_eq!(map_state("prelaunch"), VmState::Starting);
        assert_eq!(map_state("guest-panicked"), VmState::Error);
        assert_eq!(map_state("shutdown"), VmState::ShuttingDown);
    }

    #[test]
    fn firmware_is_relocated_after_upgrade_by_file_name() {
        let fw = |code: &str, sb: bool| Firmware {
            description: String::new(),
            code: code.into(),
            vars_template: code.replace("code", "vars").into(),
            format: "raw".into(),
            secure_boot: sb,
            enrolled_keys: false,
            requires_smm: false,
            source: "/d.json".into(),
        };
        let old = fw("/opt/homebrew/Cellar/qemu/11.1.3/share/qemu/edk2-aarch64-code.fd", false);
        let new = fw("/opt/homebrew/Cellar/qemu/11.2.0/share/qemu/edk2-aarch64-code.fd", false);
        let other = fw("/x/edk2-aarch64-secure-code.fd", true);
        assert_eq!(relocate(&old, &[other.clone(), new.clone()]), Some(new));
        assert_eq!(relocate(&old, &[other]), None);
    }

    #[test]
    fn pipe_tags_are_stable_and_distinct() {
        assert_eq!(pipe_tag(Path::new("/a/dev")), pipe_tag(Path::new("/a/dev")));
        assert_ne!(pipe_tag(Path::new("/a/dev")), pipe_tag(Path::new("/b/dev")));
        assert_eq!(pipe_tag(Path::new("/x")).len(), 12);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stopped_when_no_monitor_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tw_core::store::Store::new(tmp.path().to_path_buf());
        let name = tw_core::VmName::parse("dev").unwrap();
        let paths = store.paths(&name);
        let backend = QemuBackend::new(HostInfo::current(), PathBuf::from("/nonexistent"));
        assert!(backend.connect(&paths).await.unwrap().is_none());
    }
}
