//! Streaming SHA-256 of large files.

use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::AsyncReadExt;
use tw_core::{TwError, TwResult};

/// Hashes `path`, calling `progress(done, total)` at most every 256 MiB.
pub async fn sha256_file(path: &Path, mut progress: impl FnMut(u64, u64)) -> TwResult<String> {
    let mut file = tokio::fs::File::open(path).await.map_err(|e| TwError::io(path.display(), e))?;
    let total = file.metadata().await.map_err(|e| TwError::io(path.display(), e))?.len();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let (mut done, mut next_report) = (0u64, 0u64);
    loop {
        let n = file.read(&mut buf).await.map_err(|e| TwError::io(path.display(), e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        if done >= next_report {
            progress(done, total);
            next_report = done + 256 * 1024 * 1024;
        }
    }
    progress(done, total);
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("abc");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(sha256_file(&f, |_, _| {}).await.unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
