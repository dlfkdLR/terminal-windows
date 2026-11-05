//! On-disk layout of machines.
//!
//! ```text
//! <root>/machines/<name>/machine.toml   validated configuration
//!                        disk.qcow2     virtual disk (QEMU)
//!                        efivars.fd     per-VM UEFI variable store
//!                        tpm/           swtpm state
//!                        agent.key      per-VM guest-agent secret (0600)
//!                        runtime.log    hypervisor stdout/stderr
//!                        .lock          exclusive lock for mutating operations
//! <runtime>/                            Unix sockets (short path, 0700)
//! ```
//!
//! `TERMINAL_WINDOWS_HOME` overrides `<root>`.

use crate::error::{ErrorCode, TwError, TwResult};
use crate::model::MachineConfig;
use crate::name::VmName;
use crate::secure;
use std::fs;
use std::path::{Path, PathBuf};

pub const HOME_ENV: &str = "TERMINAL_WINDOWS_HOME";
/// macOS limits `sun_path` to 104 bytes (Linux: 108). Keep a margin.
const MAX_SOCKET_PATH: usize = 100;
/// Longest socket file name we create inside a runtime directory.
const LONGEST_SOCKET_NAME: &str = "swtpm-ctrl.sock";

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// Paths of one machine.
#[derive(Debug, Clone)]
pub struct MachinePaths {
    pub dir: PathBuf,
    pub config: PathBuf,
    pub disk: PathBuf,
    pub efivars: PathBuf,
    pub tpm_dir: PathBuf,
    pub agent_key: PathBuf,
    pub log: PathBuf,
    pub lock: PathBuf,
    pub runtime: PathBuf,
}

impl MachinePaths {
    pub fn qmp_socket(&self) -> PathBuf {
        self.runtime.join("qmp.sock")
    }
    /// Second QMP monitor reserved for the interactive viewer, so `action`/MCP
    /// (which use `qmp.sock`) keep working while a viewer is open.
    pub fn qmp_view_socket(&self) -> PathBuf {
        self.runtime.join("qmpv.sock")
    }
    pub fn vnc_socket(&self) -> PathBuf {
        self.runtime.join("vnc.sock")
    }
    pub fn agent_socket(&self) -> PathBuf {
        self.runtime.join("agent.sock")
    }
    pub fn swtpm_socket(&self) -> PathBuf {
        self.runtime.join("swtpm-ctrl.sock")
    }
    /// The answer-file CD; only exists while a virgin disk is being installed.
    pub fn unattend_iso(&self) -> PathBuf {
        self.dir.join("unattend.iso")
    }
    pub fn pid_file(&self) -> PathBuf {
        self.runtime.join("qemu.pid")
    }
    pub fn swtpm_pid_file(&self) -> PathBuf {
        self.runtime.join("swtpm.pid")
    }
}

/// Exclusive advisory lock held for the lifetime of the value.
#[derive(Debug)]
pub struct MachineLock {
    _file: fs::File,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolves the root from `TERMINAL_WINDOWS_HOME` or the platform data directory
    /// (`~/Library/Application Support/terminal-windows`, `%LOCALAPPDATA%\terminal-windows`).
    pub fn from_env() -> TwResult<Self> {
        if let Some(home) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
            let path = PathBuf::from(home);
            if !path.is_absolute() {
                return Err(TwError::invalid(format!("{HOME_ENV} must be an absolute path")));
            }
            return Ok(Self::new(path));
        }
        let base = dirs::data_local_dir().ok_or_else(|| TwError::new(ErrorCode::Io, "cannot determine the per-user data directory"))?;
        Ok(Self::new(base.join("terminal-windows")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn machines_dir(&self) -> PathBuf {
        self.root.join("machines")
    }

    pub fn paths(&self, name: &VmName) -> MachinePaths {
        let dir = self.machines_dir().join(name.as_str());
        let local_runtime = dir.join("run");
        let runtime = if local_runtime.join(LONGEST_SOCKET_NAME).as_os_str().len() <= MAX_SOCKET_PATH {
            local_runtime
        } else {
            short_runtime_dir(&dir)
        };
        MachinePaths {
            config: dir.join("machine.toml"),
            disk: dir.join("disk.qcow2"),
            efivars: dir.join("efivars.fd"),
            tpm_dir: dir.join("tpm"),
            agent_key: dir.join("agent.key"),
            log: dir.join("runtime.log"),
            lock: dir.join(".lock"),
            runtime,
            dir,
        }
    }

    pub fn exists(&self, name: &VmName) -> bool {
        self.paths(name).config.is_file()
    }

    pub fn load(&self, name: &VmName) -> TwResult<MachineConfig> {
        let paths = self.paths(name);
        let text = match fs::read_to_string(&paths.config) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(TwError::not_found(format!("VM '{name}' does not exist"))
                    .with_hint(format!("terminal-windows create {name} --iso /path/to/Win11.iso")))
            }
            Err(e) => return Err(TwError::io(paths.config.display(), e)),
        };
        let config = MachineConfig::from_toml(&text)?;
        if &config.name != name {
            return Err(TwError::invalid(format!(
                "{} declares name '{}' but lives in directory '{name}'",
                paths.config.display(),
                config.name
            )));
        }
        Ok(config)
    }

    /// Creates the machine directory. Fails if it already exists, so partial
    /// or foreign data is never overwritten.
    pub fn create_dir(&self, name: &VmName) -> TwResult<MachinePaths> {
        let paths = self.paths(name);
        secure::create_private_dir(&self.machines_dir())?;
        match fs::create_dir(&paths.dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(TwError::new(ErrorCode::AlreadyExists, format!("VM directory already exists: {}", paths.dir.display()))
                    .with_hint("Choose another name, or delete the existing VM first. Existing data is never overwritten."))
            }
            Err(e) => return Err(TwError::io(paths.dir.display(), e)),
        }
        secure::create_private_dir(&paths.dir)?;
        Ok(paths)
    }

    pub fn save(&self, config: &MachineConfig) -> TwResult<()> {
        config.validate()?;
        let paths = self.paths(&config.name);
        secure::write_private_atomic(&paths.config, config.to_toml()?.as_bytes())
    }

    pub fn list(&self) -> TwResult<Vec<Result<MachineConfig, (String, TwError)>>> {
        let dir = self.machines_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(TwError::io(dir.display(), e)),
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        Ok(names
            .into_iter()
            .filter_map(|raw| {
                let name = VmName::parse(&raw).ok()?;
                if !self.paths(&name).config.exists() {
                    return None;
                }
                Some(self.load(&name).map_err(|e| (raw, e)))
            })
            .collect())
    }

    /// Takes the per-machine lock without blocking.
    pub fn lock(&self, name: &VmName) -> TwResult<MachineLock> {
        let paths = self.paths(name);
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&paths.lock).map_err(|e| TwError::io(paths.lock.display(), e))?;
        match file.try_lock() {
            Ok(()) => Ok(MachineLock { _file: file }),
            Err(fs::TryLockError::WouldBlock) => {
                Err(TwError::new(ErrorCode::Busy, format!("another terminal-windows operation is using VM '{name}'"))
                    .with_hint("Wait for it to finish, then retry."))
            }
            Err(fs::TryLockError::Error(e)) => Err(TwError::io(paths.lock.display(), e)),
        }
    }

    /// Prepares the runtime (socket) directory.
    pub fn prepare_runtime(&self, paths: &MachinePaths) -> TwResult<()> {
        secure::create_private_dir(&paths.runtime)?;
        secure::verify_private_dir(&paths.runtime)
    }

    /// Deletes a machine directory. The config is renamed first so a crash
    /// mid-way leaves a directory that is clearly marked as being deleted
    /// rather than a half-deleted VM that still looks valid.
    pub fn delete(&self, name: &VmName, _lock: &MachineLock) -> TwResult<()> {
        let paths = self.paths(name);
        if paths.config.exists() {
            fs::rename(&paths.config, paths.dir.join("machine.toml.deleting")).map_err(|e| TwError::io(paths.config.display(), e))?;
        }
        if paths.runtime.starts_with(&paths.dir) {
            // Removed with the directory.
        } else if paths.runtime.exists() {
            fs::remove_dir_all(&paths.runtime).map_err(|e| TwError::io(paths.runtime.display(), e))?;
        }
        fs::remove_dir_all(&paths.dir).map_err(|e| TwError::io(paths.dir.display(), e))
    }
}

/// A short, per-machine socket directory under the system temp directory,
/// named by a hash of the machine directory so it is stable across runs.
fn short_runtime_dir(machine_dir: &Path) -> PathBuf {
    // FNV-1a: stable, dependency-free. Not a security boundary; the directory
    // itself is verified to be private before use.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in machine_dir.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    std::env::temp_dir().join(format!("tw-{:012x}", hash & 0xffff_ffff_ffff))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Arch;
    use crate::model::{BackendKind, NetworkMode, NewMachine};

    fn sample(name: &str) -> MachineConfig {
        MachineConfig::new_windows11(
            NewMachine {
                name: VmName::parse(name).unwrap(),
                backend: BackendKind::Qemu,
                arch: Arch::Aarch64,
                cpus: None,
                memory_mib: None,
                disk_gib: None,
                display: None,
                network: NetworkMode::Offline,
                install_iso: None,
                drivers_iso: None,
                unattended: false,
                windows_language: None,
            },
            8,
            "t".into(),
        )
        .unwrap()
    }

    #[test]
    fn create_save_load_list_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        assert!(store.list().unwrap().is_empty());
        let config = sample("dev");
        store.create_dir(&config.name).unwrap();
        store.save(&config).unwrap();
        assert_eq!(store.load(&config.name).unwrap(), config);
        assert_eq!(store.list().unwrap().len(), 1);

        let again = store.create_dir(&config.name).unwrap_err();
        assert_eq!(again.code, ErrorCode::AlreadyExists);

        let lock = store.lock(&config.name).unwrap();
        store.delete(&config.name, &lock).unwrap();
        assert!(!store.exists(&config.name));
        assert_eq!(store.load(&config.name).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn lock_is_exclusive() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        let name = VmName::parse("dev").unwrap();
        store.create_dir(&name).unwrap();
        let held = store.lock(&name).unwrap();
        assert_eq!(store.lock(&name).unwrap_err().code, ErrorCode::Busy);
        drop(held);
        assert!(store.lock(&name).is_ok());
    }

    #[test]
    fn config_name_must_match_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        let config = sample("dev");
        store.create_dir(&config.name).unwrap();
        store.save(&config).unwrap();
        let other = VmName::parse("other").unwrap();
        store.create_dir(&other).unwrap();
        fs::copy(store.paths(&config.name).config, store.paths(&other).config).unwrap();
        assert!(store.load(&other).unwrap_err().message.contains("declares name"));
    }

    #[test]
    fn socket_paths_fit_platform_limits() {
        let long_root = PathBuf::from("/Users/someone/Library/Application Support/terminal-windows-with-a-long-suffix");
        let store = Store::new(long_root);
        let name = VmName::parse(&"n".repeat(48)).unwrap();
        let paths = store.paths(&name);
        for socket in [paths.qmp_socket(), paths.qmp_view_socket(), paths.vnc_socket(), paths.agent_socket(), paths.swtpm_socket()] {
            assert!(socket.as_os_str().len() <= 104, "{} is too long", socket.display());
        }
        assert_eq!(store.paths(&name).runtime, paths.runtime, "runtime dir is stable");
    }
}
