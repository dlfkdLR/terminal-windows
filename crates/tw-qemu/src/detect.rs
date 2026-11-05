//! Discovery of QEMU, qemu-img and swtpm, and accelerator probing.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tw_core::host::{Arch, HostInfo, HostOs};
use tw_core::{ErrorCode, TwError, TwResult};

/// Overrides the directory searched first for QEMU binaries.
pub const QEMU_DIR_ENV: &str = "TW_QEMU_DIR";
/// Overrides the swtpm binary.
pub const SWTPM_ENV: &str = "TW_SWTPM";

#[derive(Debug, Clone, serde::Serialize)]
pub struct QemuInstall {
    pub system: PathBuf,
    pub img: PathBuf,
    pub version: Option<(u32, u32, u32)>,
    pub prefix: Option<PathBuf>,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn candidate_dirs(host: &HostInfo) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os(QEMU_DIR_ENV) {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    match host.os {
        HostOs::Macos => {
            dirs.push(PathBuf::from("/opt/homebrew/bin"));
            dirs.push(PathBuf::from("/usr/local/bin"));
            dirs.push(PathBuf::from("/opt/local/bin"));
        }
        HostOs::Windows => {
            for var in ["ProgramFiles", "ProgramW6432"] {
                if let Some(pf) = std::env::var_os(var) {
                    dirs.push(PathBuf::from(pf).join("qemu"));
                }
            }
        }
        _ => {}
    }
    dirs
}

pub fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let file = exe(name);
    dirs.iter().map(|d| d.join(&file)).find(|p| p.is_file())
}

pub fn system_binary_name(arch: Arch) -> &'static str {
    match arch {
        Arch::X86_64 => "qemu-system-x86_64",
        Arch::Aarch64 => "qemu-system-aarch64",
    }
}

/// Parses `QEMU emulator version 9.2.0 (…)`.
pub fn parse_version(output: &str) -> Option<(u32, u32, u32)> {
    let rest = output.split("version ").nth(1)?;
    let token = rest.split(|c: char| !(c.is_ascii_digit() || c == '.')).next()?;
    let mut parts = token.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

pub async fn find_qemu(host: &HostInfo, arch: Arch) -> TwResult<QemuInstall> {
    let dirs = candidate_dirs(host);
    let system = find_in(&dirs, system_binary_name(arch)).ok_or_else(|| {
        TwError::new(ErrorCode::MissingDependency, format!("{} was not found", system_binary_name(arch))).with_hint(match host.os {
            HostOs::Macos => "Install QEMU: brew install qemu   (or set TW_QEMU_DIR)",
            HostOs::Windows => "Install QEMU for Windows from https://qemu.weilnetz.de/ or set TW_QEMU_DIR",
            _ => "Install QEMU with your package manager or set TW_QEMU_DIR",
        })
    })?;
    let img = system
        .parent()
        .map(|d| d.join(exe("qemu-img")))
        .filter(|p| p.is_file())
        .or_else(|| find_in(&dirs, "qemu-img"))
        .ok_or_else(|| TwError::new(ErrorCode::MissingDependency, "qemu-img was not found next to QEMU"))?;
    let version = run_capture(&system, &["--version"], Duration::from_secs(10)).await.ok().and_then(|o| parse_version(&o));
    let prefix = system.parent().and_then(|bin| if bin.ends_with("bin") { bin.parent() } else { Some(bin) }).map(Path::to_path_buf);
    Ok(QemuInstall { system, img, version, prefix })
}

pub fn find_swtpm(host: &HostInfo) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SWTPM_ENV) {
        return Some(PathBuf::from(path)).filter(|p| p.is_file());
    }
    find_in(&candidate_dirs(host), "swtpm")
}

/// Accelerator name QEMU uses for hardware virtualization on this host OS.
pub fn native_accelerator(host: &HostInfo) -> Option<&'static str> {
    match host.os {
        HostOs::Macos => Some("hvf"),
        HostOs::Windows => Some("whpx"),
        HostOs::Linux => Some("kvm"),
        HostOs::Other => None,
    }
}

/// Accelerators compiled into the QEMU binary (`-accel help`). Compiled-in
/// does not mean usable; see [`probe_accelerator`].
pub async fn compiled_accelerators(system: &Path) -> TwResult<Vec<String>> {
    let out = run_capture(system, &["-accel", "help"], Duration::from_secs(10)).await?;
    Ok(out.lines().skip(1).map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

/// Actually starts a paused, device-less machine with the accelerator. If QEMU
/// is still alive after a moment the accelerator initialised; it is then killed.
pub async fn probe_accelerator(system: &Path, accel: &str) -> TwResult<()> {
    let mut child = tokio::process::Command::new(system)
        .args(["-accel", accel, "-machine", "none", "-nodefaults", "-display", "none", "-S", "-monitor", "none", "-serial", "none"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| TwError::io(system.display(), e))?;
    match tokio::time::timeout(Duration::from_millis(1500), child.wait()).await {
        Err(_) => {
            let _ = child.kill().await;
            Ok(())
        }
        Ok(status) => {
            let mut stderr = String::new();
            if let Some(mut e) = child.stderr.take() {
                use tokio::io::AsyncReadExt;
                let _ = e.read_to_string(&mut stderr).await;
            }
            Err(TwError::new(
                ErrorCode::Incompatible,
                format!(
                    "accelerator '{accel}' failed to initialise ({}): {}",
                    status.map(|s| s.to_string()).unwrap_or_default(),
                    stderr.trim()
                ),
            )
            .with_hint(match accel {
                "whpx" => "Enable the 'Windows Hypervisor Platform' Windows feature and reboot.",
                "hvf" => "Hypervisor.framework is unavailable (nested virtualization or a sandbox can cause this).",
                _ => "Hardware virtualization is unavailable on this host.",
            }))
        }
    }
}

pub async fn run_capture(program: &Path, args: &[&str], timeout: Duration) -> TwResult<String> {
    let fut = tokio::process::Command::new(program).args(args).stdin(Stdio::null()).kill_on_drop(true).output();
    let out = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| TwError::new(ErrorCode::Timeout, format!("{} timed out", program.display())))?
        .map_err(|e| TwError::io(program.display(), e))?;
    if !out.status.success() {
        return Err(TwError::backend(format!(
            "{} {} failed: {}",
            program.display(),
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version("QEMU emulator version 9.2.0 (v9.2.0)\nCopyright"), Some((9, 2, 0)));
        assert_eq!(parse_version("QEMU emulator version 10.1.50"), Some((10, 1, 50)));
        assert_eq!(parse_version("QEMU emulator version 8.2"), Some((8, 2, 0)));
        assert_eq!(parse_version("garbage"), None);
    }

    #[test]
    fn finds_binaries_in_given_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join(exe("qemu-system-aarch64"));
        std::fs::write(&bin, b"").unwrap();
        assert_eq!(find_in(&[PathBuf::from("/nonexistent"), tmp.path().to_path_buf()], "qemu-system-aarch64"), Some(bin));
        assert_eq!(find_in(&[tmp.path().to_path_buf()], "swtpm"), None);
    }

    #[test]
    fn accelerator_per_os() {
        assert_eq!(native_accelerator(&HostInfo { os: HostOs::Macos, arch: Some(Arch::Aarch64) }), Some("hvf"));
        assert_eq!(native_accelerator(&HostInfo { os: HostOs::Windows, arch: Some(Arch::X86_64) }), Some("whpx"));
    }
}
