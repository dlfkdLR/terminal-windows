//! Waits for a browser download to finish in a directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tw_core::{ErrorCode, TwError, TwResult};

fn iso_files(dir: &Path) -> HashMap<PathBuf, (u64, SystemTime)> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("iso")))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (e.path(), (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH))))
        })
        .collect()
}

/// Returns the first `.iso` in `dir` that is new or changed relative to the
/// state at call time, once its size has stayed the same for `settle`.
/// Browsers write to `.crdownload`/`.download`/`.part` names and rename at the
/// end, so a completed `.iso` normally appears atomically.
pub async fn wait_for_new_iso(dir: &Path, timeout: Duration, settle: Duration, poll: Duration) -> TwResult<PathBuf> {
    let before = iso_files(dir);
    let deadline = tokio::time::Instant::now() + timeout;
    let mut candidate: Option<(PathBuf, u64, tokio::time::Instant)> = None;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(TwError::new(ErrorCode::Timeout, format!("no new ISO appeared in {}", dir.display()))
                .with_hint("When the download finishes, run: terminal-windows image import /path/to/Win11.iso"));
        }
        let now = iso_files(dir);
        let fresh = now.iter().filter(|(p, meta)| before.get(*p) != Some(meta)).max_by_key(|(_, (_, t))| *t);
        match (fresh, &candidate) {
            (Some((p, (len, _))), Some((cp, clen, since))) if p == cp && len == clen => {
                if since.elapsed() >= settle && *len > 0 {
                    return Ok(p.clone());
                }
            }
            (Some((p, (len, _))), _) => candidate = Some((p.clone(), *len, tokio::time::Instant::now())),
            (None, _) => candidate = None,
        }
        tokio::time::sleep(poll).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detects_new_complete_iso_and_ignores_existing_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.iso"), b"old").unwrap();
        let d = dir.path().to_path_buf();
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            std::fs::write(d.join("Win11_26H2_Korean_Arm64.iso.crdownload"), b"partial").unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            std::fs::rename(d.join("Win11_26H2_Korean_Arm64.iso.crdownload"), d.join("Win11_26H2_Korean_Arm64.iso")).unwrap();
        });
        let found =
            wait_for_new_iso(dir.path(), Duration::from_secs(5), Duration::from_millis(100), Duration::from_millis(20)).await.unwrap();
        writer.await.unwrap();
        assert_eq!(found.file_name().unwrap(), "Win11_26H2_Korean_Arm64.iso");
    }

    #[tokio::test]
    async fn times_out_with_import_hint() {
        let dir = tempfile::tempdir().unwrap();
        let err = wait_for_new_iso(dir.path(), Duration::from_millis(60), Duration::from_millis(10), Duration::from_millis(10))
            .await
            .unwrap_err();
        assert!(err.hint.unwrap().contains("image import"));
    }
}
