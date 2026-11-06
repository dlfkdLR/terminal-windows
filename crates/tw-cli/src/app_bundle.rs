//! A tiny `.app` bundle for the native viewer window (macOS).
//!
//! A bare executable that opens a window gets macOS's generic black "exec" Dock icon and no
//! proper name. Running the same executable from inside an `.app` bundle with an `Info.plist`
//! and an icon gives it a real name and icon. The bundle lives in the data directory and is
//! refreshed whenever the installed binary changes.

use std::path::{Path, PathBuf};
use tw_core::{secure, TwError, TwResult};

const ICON: &[u8] = include_bytes!("../assets/AppIcon.icns");
const EXECUTABLE: &str = "terminal-windows";

pub fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>terminal-windows</string>
	<key>CFBundleDisplayName</key><string>terminal-windows</string>
	<key>CFBundleIdentifier</key><string>dev.terminal-windows.viewer</string>
	<key>CFBundleExecutable</key><string>{EXECUTABLE}</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>{version}</string>
	<key>CFBundleVersion</key><string>{version}</string>
	<key>LSMinimumSystemVersion</key><string>11.0</string>
	<key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
"#
    )
}

fn write_if_changed(path: &Path, data: &[u8]) -> TwResult<()> {
    if std::fs::read(path).is_ok_and(|existing| existing == data) {
        return Ok(());
    }
    secure::write_private_atomic(path, data)
}

/// Creates or refreshes `<root>/TerminalWindows.app` and returns the path of the executable
/// inside it. `exe` is the currently running terminal-windows binary.
pub fn ensure(root: &Path, exe: &Path, version: &str) -> TwResult<PathBuf> {
    let app = root.join("TerminalWindows.app");
    let contents = app.join("Contents");
    for dir in [contents.join("MacOS"), contents.join("Resources")] {
        std::fs::create_dir_all(&dir).map_err(|e| TwError::io(dir.display(), e))?;
    }
    write_if_changed(&contents.join("Info.plist"), info_plist(version).as_bytes())?;
    write_if_changed(&contents.join("Resources").join("AppIcon.icns"), ICON)?;
    let target = contents.join("MacOS").join(EXECUTABLE);
    let binary = std::fs::read(exe).map_err(|e| TwError::io(exe.display(), e))?;
    if std::fs::read(&target).map(|existing| existing != binary).unwrap_or(true) {
        // Write next to the target and rename, so a running viewer keeps its old file.
        let staging = contents.join("MacOS").join(format!(".{EXECUTABLE}.new"));
        std::fs::write(&staging, &binary).map_err(|e| TwError::io(staging.display(), e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).map_err(|e| TwError::io(staging.display(), e))?;
        }
        std::fs::rename(&staging, &target).map_err(|e| TwError::io(target.display(), e))?;
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_complete_bundle_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("terminal-windows-src");
        std::fs::write(&exe, b"binary-v1").unwrap();
        let root = tmp.path().join("data");
        let inner = ensure(&root, &exe, "0.1.7").unwrap();
        assert_eq!(inner, root.join("TerminalWindows.app/Contents/MacOS/terminal-windows"));
        assert_eq!(std::fs::read(&inner).unwrap(), b"binary-v1");
        let plist = std::fs::read_to_string(root.join("TerminalWindows.app/Contents/Info.plist")).unwrap();
        for key in ["CFBundleName", "CFBundleIdentifier", "CFBundleExecutable", "CFBundleIconFile", "CFBundlePackageType"] {
            assert!(plist.contains(key), "{key}");
        }
        assert!(plist.contains("<string>0.1.7</string>"));
        assert!(root.join("TerminalWindows.app/Contents/Resources/AppIcon.icns").metadata().unwrap().len() > 1000);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&inner).unwrap().permissions().mode() & 0o111, 0o111, "executable");
        }
        assert_eq!(ensure(&root, &exe, "0.1.7").unwrap(), inner, "second call changes nothing");
    }

    #[test]
    fn a_new_binary_replaces_the_old_one() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("src");
        std::fs::write(&exe, b"old").unwrap();
        let inner = ensure(tmp.path(), &exe, "0.1.7").unwrap();
        std::fs::write(&exe, b"new binary").unwrap();
        ensure(tmp.path(), &exe, "0.1.8").unwrap();
        assert_eq!(std::fs::read(&inner).unwrap(), b"new binary");
        assert!(std::fs::read_to_string(tmp.path().join("TerminalWindows.app/Contents/Info.plist")).unwrap().contains("0.1.8"));
        assert!(!tmp.path().join("TerminalWindows.app/Contents/MacOS/.terminal-windows.new").exists(), "no staging file left");
    }

    #[test]
    fn a_missing_source_binary_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(ensure(tmp.path(), &tmp.path().join("nope"), "0.1.7").is_err());
    }
}
