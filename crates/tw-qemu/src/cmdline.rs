//! QEMU, swtpm and qemu-img argument construction. Pure functions: no I/O,
//! no shell. Every value comes from validated configuration and host-chosen
//! paths, never from guest-supplied data.

use crate::firmware::Firmware;
use std::path::{Path, PathBuf};
use tw_core::host::Arch;
use tw_core::model::{MachineConfig, NetworkMode};
use tw_core::{TwError, TwResult};

/// Where a QEMU character device or monitor listens. Unix sockets are used on
/// macOS/Linux; Windows uses named pipes. Neither is reachable over the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Unix(PathBuf),
    /// Pipe name without the `\\.\pipe\` prefix.
    NamedPipe(String),
}

impl Endpoint {
    fn chardev(&self, id: &str, server: bool) -> String {
        match self {
            Endpoint::Unix(path) => {
                let mut s = format!("socket,id={id},path={}", escape(path));
                if server {
                    s.push_str(",server=on,wait=off");
                }
                s
            }
            Endpoint::NamedPipe(name) => format!("pipe,id={id},path={}", escape_str(name)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VncEndpoint {
    Unix(PathBuf),
    /// Loopback TCP with QEMU's VNC password authentication enabled; the
    /// password is set over QMP after start. Used only where Unix sockets are unavailable.
    LoopbackWithPassword {
        display: u16,
    },
    Disabled,
}

#[derive(Debug, Clone)]
pub struct LaunchPlan<'a> {
    pub config: &'a MachineConfig,
    pub accelerator: &'a str,
    pub firmware: &'a Firmware,
    pub disk: &'a Path,
    pub efivars: &'a Path,
    pub pid_file: &'a Path,
    pub qmp: Endpoint,
    pub agent: Endpoint,
    pub vnc: VncEndpoint,
    pub tpm_socket: Option<PathBuf>,
    pub daemonize: bool,
    /// Detected QEMU version; selects between old and new option spellings.
    pub qemu_version: Option<(u32, u32, u32)>,
}

/// QEMU option values use `,` as a separator; a literal comma is written `,,`.
fn escape(path: &Path) -> String {
    escape_str(&path.to_string_lossy())
}
fn escape_str(s: &str) -> String {
    s.replace(',', ",,")
}

pub const AGENT_PORT_NAME: &str = "com.terminal-windows.agent.0";

pub fn qemu_args(plan: &LaunchPlan<'_>) -> TwResult<Vec<String>> {
    let c = plan.config;
    if plan.accelerator == "tcg" {
        return Err(TwError::unsupported("refusing to run Windows under TCG emulation; hardware acceleration is required"));
    }
    if c.tpm && plan.tpm_socket.is_none() {
        return Err(TwError::invalid("TPM is enabled but no swtpm socket was provided"));
    }
    let mut a: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
    let name = format!("guest=tw-{},debug-threads=on", c.name);
    push(&["-name", &name, "-nodefaults", "-no-user-config", "-msg", "timestamp=on"]);

    // Machine, accelerator, CPU.
    let (machine, cpu) = match c.arch {
        Arch::Aarch64 => ("virt,highmem=on".to_string(), "host"),
        Arch::X86_64 => {
            let mut m = "q35".to_string();
            if plan.firmware.requires_smm {
                m.push_str(",smm=on");
            }
            // WHPX does not expose `-cpu host`; `max` passes through what it can.
            (m, if plan.accelerator == "whpx" { "max" } else { "host" })
        }
    };
    let accel = if plan.accelerator == "whpx" {
        // Unverified default: the userspace irqchip is reported to be more
        // reliable for Windows guests on WHPX. Track in docs/compatibility.md.
        "whpx,kernel-irqchip=off".to_string()
    } else {
        plan.accelerator.to_string()
    };
    let smp = c.cpus.to_string();
    let mem = format!("{}M", c.memory_mib);
    let uuid = c.smbios_uuid.clone();
    push(&["-machine", &machine, "-accel", &accel, "-cpu", cpu, "-smp", &smp, "-m", &mem, "-uuid", &uuid]);
    push(&["-rtc", "base=localtime"]);

    // UEFI: read-only code + per-VM variable store.
    if plan.firmware.requires_smm {
        push(&["-global", "driver=cfi.pflash01,property=secure,value=on"]);
    }
    let fmt = escape_str(&plan.firmware.format);
    let code = format!("if=pflash,unit=0,format={fmt},readonly=on,file={}", escape(&plan.firmware.code));
    let vars = format!("if=pflash,unit=1,format={fmt},file={}", escape(plan.efivars));
    push(&["-drive", &code, "-drive", &vars]);

    // Display. Windows ARM64 has no virtio-gpu display driver, so ramfb is used
    // (fixed resolution chosen in firmware). x86 uses the standard VGA with EDID.
    match c.arch {
        Arch::Aarch64 => push(&["-device", "ramfb"]),
        Arch::X86_64 => {
            let vga = format!("VGA,edid=on,xres={},yres={}", c.display.width, c.display.height);
            push(&["-device", &vga]);
        }
    }

    // Input: absolute tablet so the hypervisor console can place the pointer exactly.
    push(&["-device", "qemu-xhci,id=xhci", "-device", "usb-kbd,bus=xhci.0", "-device", "usb-tablet,bus=xhci.0"]);

    // System disk on NVMe: Windows has an inbox NVMe driver on x64 and ARM64.
    let disk = format!("if=none,id=disk0,format=qcow2,cache=writeback,discard=unmap,file={}", escape(plan.disk));
    push(&["-drive", &disk, "-device", "nvme,drive=disk0,serial=tw-disk0,bootindex=1"]);

    // Installation and driver media.
    let mut cds: Vec<(&Path, &str)> = Vec::new();
    if let Some(iso) = &c.install_iso {
        cds.push((iso, "0"));
    }
    if let Some(iso) = &c.drivers_iso {
        cds.push((iso, ""));
    }
    for (i, (iso, bootindex)) in cds.iter().enumerate() {
        let drive = format!("if=none,id=cd{i},media=cdrom,readonly=on,file={}", escape(iso));
        let boot = if bootindex.is_empty() { String::new() } else { format!(",bootindex={bootindex}") };
        let device = match c.arch {
            Arch::X86_64 => format!("ide-cd,drive=cd{i},bus=ide.{i}{boot}"),
            Arch::Aarch64 => format!("usb-storage,drive=cd{i},bus=xhci.0,removable=on{boot}"),
        };
        push(&["-drive", &drive, "-device", &device]);
    }

    // TPM 2.0 through swtpm.
    if let Some(sock) = &plan.tpm_socket {
        let chardev = format!("socket,id=chrtpm,path={}", escape(sock));
        let device = match c.arch {
            Arch::X86_64 => "tpm-tis,tpmdev=tpm0",
            // ppi=off: the Physical Presence Interface RAM region is not 16 KiB
            // page aligned and HVF on Apple Silicon rejects mapping it
            // (HV_BAD_ARGUMENT, measured with QEMU 11.1.3). The TPM itself is unaffected.
            Arch::Aarch64 => "tpm-tis-device,tpmdev=tpm0,ppi=off",
        };
        push(&["-chardev", &chardev, "-tpmdev", "emulator,id=tpm0,chardev=chrtpm", "-device", device]);
    }

    // Guest agent channel (Plane G): virtio-serial, no network involved.
    let agent = plan.agent.chardev("agent0", true);
    let port = format!("virtserialport,bus=vser0.0,chardev=agent0,name={AGENT_PORT_NAME}");
    push(&["-device", "virtio-serial-pci,id=vser0", "-chardev", &agent, "-device", &port]);

    // Network.
    match c.network {
        NetworkMode::Nat => {
            let nic = match c.arch {
                Arch::X86_64 => "e1000e,netdev=net0",
                // Requires the virtio-win ARM64 driver (attach with --drivers-iso).
                Arch::Aarch64 => "virtio-net-pci,netdev=net0",
            };
            push(&["-netdev", "user,id=net0", "-device", nic]);
        }
        NetworkMode::Offline => {}
    }

    // Control (Plane H) and display export. Always headless: QEMU cannot hide
    // or re-show its own window at runtime, so viewers attach separately.
    let qmp = plan.qmp.chardev("qmp0", true);
    push(&["-chardev", &qmp]);
    // QEMU 11 deprecates `-mon` in favour of `-object monitor-qmp`.
    if plan.qemu_version.is_some_and(|v| v >= (11, 0, 0)) {
        push(&["-object", "monitor-qmp,id=mon0,chardev=qmp0"]);
    } else {
        push(&["-mon", "chardev=qmp0,mode=control"]);
    }
    match &plan.vnc {
        VncEndpoint::Unix(path) => {
            let v = format!("unix:{}", escape(path));
            push(&["-vnc", &v]);
        }
        VncEndpoint::LoopbackWithPassword { display } => {
            let v = format!("127.0.0.1:{display},password=on");
            push(&["-vnc", &v]);
        }
        VncEndpoint::Disabled => {}
    }
    push(&["-display", "none"]);
    let pid = escape(plan.pid_file);
    push(&["-pidfile", &pid]);
    if plan.daemonize {
        push(&["-daemonize"]);
    }
    Ok(a)
}

/// swtpm arguments. swtpm splits option values on commas and has no escape,
/// so paths containing commas are rejected.
pub fn swtpm_args(state_dir: &Path, ctrl_socket: &Path, log: &Path) -> TwResult<Vec<String>> {
    for p in [state_dir, ctrl_socket, log] {
        if p.to_string_lossy().contains(',') {
            return Err(TwError::invalid(format!("swtpm cannot use paths containing commas: {}", p.display())));
        }
    }
    Ok(vec![
        "socket".into(),
        "--tpm2".into(),
        "--tpmstate".into(),
        format!("dir={}", state_dir.display()),
        "--ctrl".into(),
        format!("type=unixio,path={}", ctrl_socket.display()),
        "--log".into(),
        format!("file={},level=1", log.display()),
        // Exit when QEMU disconnects, so no swtpm process outlives its VM.
        "--terminate".into(),
    ])
}

pub fn qemu_img_create_args(disk: &Path, size_gib: u64) -> Vec<String> {
    vec!["create".into(), "-q".into(), "-f".into(), "qcow2".into(), disk.to_string_lossy().into_owned(), format!("{size_gib}G")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tw_core::model::{BackendKind, NewMachine};
    use tw_core::VmName;

    fn config(arch: Arch, network: NetworkMode, iso: bool) -> MachineConfig {
        MachineConfig::new_windows11(
            NewMachine {
                name: VmName::parse("dev").unwrap(),
                backend: BackendKind::Qemu,
                arch,
                cpus: Some(4),
                memory_mib: Some(8192),
                disk_gib: Some(80),
                display: None,
                network,
                install_iso: iso.then(|| PathBuf::from("/media/Win11,ARM64.iso")),
                drivers_iso: iso.then(|| PathBuf::from("/media/virtio-win.iso")),
            },
            8,
            "t".into(),
        )
        .unwrap()
    }

    fn firmware(smm: bool) -> Firmware {
        Firmware {
            description: "t".into(),
            code: "/fw/code.fd".into(),
            vars_template: "/fw/vars.fd".into(),
            format: "raw".into(),
            secure_boot: true,
            enrolled_keys: true,
            requires_smm: smm,
            source: "/fw/50.json".into(),
        }
    }

    fn plan<'a>(c: &'a MachineConfig, fw: &'a Firmware, accel: &'a str) -> LaunchPlan<'a> {
        LaunchPlan {
            config: c,
            accelerator: accel,
            firmware: fw,
            disk: Path::new("/vm/dev/disk.qcow2"),
            efivars: Path::new("/vm/dev/efivars.fd"),
            pid_file: Path::new("/run/tw/qemu.pid"),
            qmp: Endpoint::Unix("/run/tw/qmp.sock".into()),
            agent: Endpoint::Unix("/run/tw/agent.sock".into()),
            vnc: VncEndpoint::Unix("/run/tw/vnc.sock".into()),
            tpm_socket: Some("/run/tw/swtpm-ctrl.sock".into()),
            daemonize: true,
            qemu_version: Some((11, 1, 3)),
        }
    }

    fn joined(args: &[String]) -> String {
        args.join(" ")
    }

    fn pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn aarch64_hvf_windows11() {
        let c = config(Arch::Aarch64, NetworkMode::Nat, true);
        let fw = firmware(false);
        let args = qemu_args(&plan(&c, &fw, "hvf")).unwrap();
        assert!(pair(&args, "-machine", "virt,highmem=on"));
        assert!(pair(&args, "-accel", "hvf"));
        assert!(pair(&args, "-cpu", "host"));
        assert!(pair(&args, "-device", "ramfb"));
        assert!(pair(&args, "-device", "tpm-tis-device,tpmdev=tpm0,ppi=off"));
        assert!(pair(&args, "-object", "monitor-qmp,id=mon0,chardev=qmp0"));
        assert!(pair(&args, "-device", "nvme,drive=disk0,serial=tw-disk0,bootindex=1"));
        assert!(pair(&args, "-device", "usb-storage,drive=cd0,bus=xhci.0,removable=on,bootindex=0"));
        assert!(pair(&args, "-device", "usb-storage,drive=cd1,bus=xhci.0,removable=on"));
        assert!(pair(&args, "-device", "virtio-net-pci,netdev=net0"));
        assert!(pair(&args, "-uuid", &c.smbios_uuid));
        // Comma in the ISO path is escaped for QEMU's option parser.
        assert!(joined(&args).contains("file=/media/Win11,,ARM64.iso"));
        assert!(pair(&args, "-display", "none"));
        assert!(args.contains(&"-daemonize".to_string()));
        assert!(!joined(&args).contains("smm=on"));
    }

    #[test]
    fn x86_64_with_smm_firmware_and_offline_network() {
        let c = config(Arch::X86_64, NetworkMode::Offline, false);
        let fw = firmware(true);
        let args = qemu_args(&plan(&c, &fw, "hvf")).unwrap();
        assert!(pair(&args, "-machine", "q35,smm=on"));
        assert!(pair(&args, "-global", "driver=cfi.pflash01,property=secure,value=on"));
        assert!(pair(&args, "-device", "VGA,edid=on,xres=1920,yres=1080"));
        assert!(pair(&args, "-device", "tpm-tis,tpmdev=tpm0"));
        assert!(!joined(&args).contains("-netdev"), "offline mode has no NIC");
        assert!(!joined(&args).contains("media=cdrom"));
    }

    #[test]
    fn whpx_named_pipes_and_password_vnc() {
        let c = config(Arch::X86_64, NetworkMode::Nat, false);
        let fw = firmware(false);
        let mut p = plan(&c, &fw, "whpx");
        p.qmp = Endpoint::NamedPipe("tw-dev-qmp".into());
        p.agent = Endpoint::NamedPipe("tw-dev-agent".into());
        p.vnc = VncEndpoint::LoopbackWithPassword { display: 17 };
        p.daemonize = false;
        p.qemu_version = Some((9, 2, 0));
        let args = qemu_args(&p).unwrap();
        assert!(pair(&args, "-mon", "chardev=qmp0,mode=control"), "older QEMU keeps -mon");
        assert!(pair(&args, "-accel", "whpx,kernel-irqchip=off"));
        assert!(pair(&args, "-cpu", "max"));
        assert!(pair(&args, "-chardev", "pipe,id=qmp0,path=tw-dev-qmp"));
        assert!(pair(&args, "-vnc", "127.0.0.1:17,password=on"));
        assert!(pair(&args, "-device", "e1000e,netdev=net0"));
        assert!(!args.contains(&"-daemonize".to_string()));
    }

    #[test]
    fn agent_channel_is_virtio_serial_without_network() {
        let c = config(Arch::Aarch64, NetworkMode::Offline, false);
        let fw = firmware(false);
        let args = qemu_args(&plan(&c, &fw, "hvf")).unwrap();
        assert!(pair(&args, "-chardev", "socket,id=agent0,path=/run/tw/agent.sock,server=on,wait=off"));
        assert!(pair(&args, "-device", &format!("virtserialport,bus=vser0.0,chardev=agent0,name={AGENT_PORT_NAME}")));
        assert!(!joined(&args).contains("tcp:"));
    }

    #[test]
    fn refuses_tcg_and_missing_tpm() {
        let c = config(Arch::X86_64, NetworkMode::Nat, false);
        let fw = firmware(false);
        assert!(qemu_args(&plan(&c, &fw, "tcg")).is_err());
        let mut p = plan(&c, &fw, "hvf");
        p.tpm_socket = None;
        assert!(qemu_args(&p).is_err());
    }

    #[test]
    fn swtpm_and_qemu_img() {
        let args = swtpm_args(Path::new("/vm/tpm"), Path::new("/run/s.sock"), Path::new("/vm/swtpm.log")).unwrap();
        assert_eq!(args[0], "socket");
        assert!(args.contains(&"--tpm2".to_string()) && args.contains(&"--terminate".to_string()));
        assert!(swtpm_args(Path::new("/a,b"), Path::new("/s"), Path::new("/l")).is_err());
        assert_eq!(qemu_img_create_args(Path::new("/vm/d.qcow2"), 80).last().unwrap(), "80G");
    }
}
