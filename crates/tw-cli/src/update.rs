//! `terminal-windows update`: asks GitHub for the latest release and, for an installed copy, runs
//! that release's own `get.sh` (which downloads the binary and verifies its SHA-256 against the
//! release's `SHA256SUMS`). Nothing is executed unless a newer release exists.

use std::path::{Path, PathBuf};
use tw_core::{ErrorCode, TwError, TwResult};

pub const REPO: &str = "dlfkdLR/terminal-windows";

/// `v0.1.9` / `0.1.9` -> `(0, 1, 9)`. Anything else (pre-release suffixes included) is rejected.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let v = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    parts.next().is_none().then_some(v)
}

/// The `tag_name` of GitHub's "latest release" JSON, if it is a plain version.
pub fn latest_tag(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    parse_version(tag).map(|_| tag.to_string())
}

/// The install prefix when `exe` is `<prefix>/bin/terminal-windows`; None for a source build.
pub fn install_prefix(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    (bin.file_name()? == "bin" && exe.file_stem()? == "terminal-windows").then(|| bin.parent().map(Path::to_path_buf))?
}

pub struct Status {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
}

pub async fn check() -> TwResult<Status> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let http = tw_media::download::http_client()?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = http
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| TwError::new(ErrorCode::Backend, format!("cannot query the latest release: {e}")))?
        .text()
        .await
        .map_err(|e| TwError::new(ErrorCode::Backend, format!("cannot read the latest release: {e}")))?;
    let tag = latest_tag(&body).ok_or_else(|| TwError::backend("the latest release has no plain version tag"))?;
    let update_available = parse_version(&tag) > parse_version(&current);
    Ok(Status { current, latest: tag.trim_start_matches('v').to_string(), update_available })
}

/// Downloads the release's installer and runs it for this installation's prefix.
#[cfg(unix)]
pub async fn install(tag: &str) -> TwResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let exe = std::env::current_exe().map_err(|e| TwError::io("current executable", e))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let prefix = install_prefix(&exe).ok_or_else(|| {
        TwError::new(ErrorCode::InvalidInput, format!("{} is not an installed copy (<prefix>/bin/terminal-windows)", exe.display()))
            .with_hint("Built from source? Pull and rebuild instead.")
    })?;
    let http = tw_media::download::http_client()?;
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/get.sh");
    let script = http
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| TwError::backend(format!("cannot download the installer {url}: {e}")))?
        .bytes()
        .await
        .map_err(|e| TwError::backend(format!("cannot read the installer: {e}")))?;
    if script.len() > 256 * 1024 || !script.starts_with(b"#!/usr/bin/env bash") {
        return Err(TwError::backend("the downloaded installer does not look like get.sh; refusing to run it"));
    }
    let dir = std::env::temp_dir().join(format!("tw-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| TwError::io(dir.display(), e))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| TwError::io(dir.display(), e))?;
    let file = dir.join("get.sh");
    std::fs::write(&file, &script).map_err(|e| TwError::io(file.display(), e))?;
    let status = std::process::Command::new("bash")
        .arg(&file)
        .env("TW_VERSION", tag)
        .env("TW_PREFIX", &prefix)
        .env("TW_REPO", REPO)
        .status()
        .map_err(|e| TwError::io("bash", e));
    let _ = std::fs::remove_dir_all(&dir);
    if !status?.success() {
        return Err(TwError::backend("the installer failed (see its output above); nothing was changed"));
    }
    Ok(())
}

#[cfg(not(unix))]
pub async fn install(tag: &str) -> TwResult<()> {
    Err(TwError::new(ErrorCode::Unsupported, format!("automatic update is not available on Windows yet ({tag})"))
        .with_hint(format!("Run install.ps1 from https://github.com/{REPO}/releases/latest")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.1.9"), Some((0, 1, 9)));
        assert!(parse_version("v0.1.10") > parse_version("0.1.9"), "10 is newer than 9");
        for bad in ["", "v1", "1.2", "1.2.3.4", "1.2.x", "v1.2.3-rc1", "latest"] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn latest_release_json() {
        assert_eq!(latest_tag(r#"{"tag_name":"v0.1.10","name":"x"}"#).as_deref(), Some("v0.1.10"));
        assert_eq!(latest_tag(r#"{"tag_name":"nightly"}"#), None);
        assert_eq!(latest_tag("{}"), None);
        assert_eq!(latest_tag("not json"), None);
    }

    #[test]
    fn only_installed_copies_update_themselves() {
        assert_eq!(install_prefix(Path::new("/home/u/.local/bin/terminal-windows")), Some(PathBuf::from("/home/u/.local")));
        assert_eq!(install_prefix(Path::new("/repo/target/debug/terminal-windows")), None, "source build");
        assert_eq!(install_prefix(Path::new("/usr/bin/other")), None);
    }
}
