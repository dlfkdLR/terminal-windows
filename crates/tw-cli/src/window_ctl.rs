//! `show` / `hide` and the hidden `window` command: the native viewer runs as its own
//! detached process, so it outlives the terminal command and closing it never touches the VM.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tw_core::backend::VmState;
use tw_core::service::VmService;
use tw_core::{ErrorCode, TwError, TwResult, VmName};

fn pid_file(service: &VmService, name: &VmName) -> PathBuf {
    service.store().paths(name).runtime.join("viewer.pid")
}

fn log_file(service: &VmService, name: &VmName) -> PathBuf {
    service.store().paths(name).dir.join("viewer.log")
}

fn read_pid(path: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// True only if `pid` is a running terminal-windows process (a recycled PID must never be killed).
fn is_our_viewer(pid: u32) -> bool {
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains("terminal-windows"))
    } else {
        Command::new("ps").args(["-p", &pid.to_string(), "-o", "command="]).output().is_ok_and(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            text.contains("terminal-windows") && text.split_whitespace().any(|w| w == "window")
        })
    }
}

fn kill(pid: u32) -> bool {
    if cfg!(windows) {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    } else {
        Command::new("kill").arg(pid.to_string()).status().is_ok_and(|s| s.success())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Shown {
    pub pid: u32,
    pub already_open: bool,
}

/// Opens the native window for a running VM (or reports the one that is already open).
pub async fn show(service: &VmService, name: &VmName) -> TwResult<Shown> {
    if cfg!(not(any(target_os = "macos", target_os = "windows"))) {
        return Err(TwError::unsupported("the native window viewer is only available on macOS and Windows"));
    }
    let state = service.status(name).await?.state;
    if !matches!(state, VmState::Running | VmState::Paused) {
        return Err(TwError::state(format!("VM '{name}' is {state:?}")).with_hint(format!("terminal-windows start {name}")));
    }
    let pid_path = pid_file(service, name);
    if let Some(pid) = read_pid(&pid_path).filter(|p| is_our_viewer(*p)) {
        return Ok(Shown { pid, already_open: true });
    }
    let exe = std::env::current_exe().map_err(|e| TwError::io("current executable", e))?;
    // On macOS the window runs from a small .app bundle: a bare executable would get the generic
    // black "exec" Dock icon and no proper name.
    #[cfg(target_os = "macos")]
    let exe = match crate::app_bundle::ensure(service.store().root(), &exe, env!("CARGO_PKG_VERSION")) {
        Ok(inner) => inner,
        Err(e) => {
            tracing::warn!("cannot create the app bundle ({e}); the window will have a generic icon");
            exe
        }
    };
    let log_path = log_file(service, name);
    let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path).map_err(|e| TwError::io(log_path.display(), e))?;
    let mut command = Command::new(exe);
    command.args(["window", name.as_str()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0); // Ctrl+C in the terminal must not close the window
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    let mut child = command.spawn().map_err(|e| TwError::io("start the window process", e))?;
    let pid = child.id();
    // A window that dies immediately (no display, console unreachable) must be reported now.
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(Some(status)) = child.try_wait() {
            let tail = std::fs::read_to_string(&log_path).unwrap_or_default();
            let tail: Vec<&str> = tail.lines().rev().take(3).collect();
            return Err(TwError::new(
                ErrorCode::GuestUnavailable,
                format!("the viewer window exited right away ({status}): {}", tail.into_iter().rev().collect::<Vec<_>>().join(" ")),
            )
            .with_hint(format!("Log: {}", log_path.display())));
        }
    }
    tw_core::secure::write_private_atomic(&pid_path, pid.to_string().as_bytes())?;
    Ok(Shown { pid, already_open: false })
}

/// Closes the native window if one is open. Returns whether one was closed.
pub fn hide(service: &VmService, name: &VmName) -> TwResult<bool> {
    let pid_path = pid_file(service, name);
    let Some(pid) = read_pid(&pid_path) else { return Ok(false) };
    let closed = is_our_viewer(pid) && kill(pid);
    let _ = std::fs::remove_file(&pid_path);
    Ok(closed)
}

/// Body of the hidden `window` command (runs in the detached process).
pub async fn run_window(service: &VmService, name: &VmName) -> TwResult<()> {
    let result = crate::window::run(service, name).await;
    let _ = std::fs::remove_file(pid_file(service, name));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_processes_are_never_ours() {
        assert!(!is_our_viewer(std::process::id()) || cfg!(test) && false, "the test runner is not a viewer window");
        assert!(!is_our_viewer(999_999_999));
    }
}
