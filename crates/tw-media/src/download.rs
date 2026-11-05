//! Resumable HTTPS download into `<dest>.part`, renamed only when complete.

use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tw_core::{ErrorCode, TwError, TwResult};

pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    dest.with_file_name(name)
}

/// Downloads `url` to `dest`. An existing `.part` file is resumed with an HTTP
/// Range request; if the server ignores Range, the download restarts.
pub async fn download(http: &reqwest::Client, url: &str, dest: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> TwResult<()> {
    let part = part_path(dest);
    let have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
    let mut req = http.get(url).timeout(std::time::Duration::from_secs(6 * 3600));
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| TwError::new(ErrorCode::Io, format!("download: {e}")))?;
    let status = resp.status();
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && have > 0 {
        // The part file is already complete (or larger than the resource): restart.
        let _ = tokio::fs::remove_file(&part).await;
        return Box::pin(download(http, url, dest, progress)).await;
    }
    if !status.is_success() {
        return Err(TwError::backend(format!("download failed: HTTP {status}"))
            .with_hint("Microsoft download links expire after 24 hours; run the command again for a fresh link."));
    }
    let total = resp.content_length().map(|len| if resumed { len + have } else { len });
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&part)
        .await
        .map_err(|e| TwError::io(part.display(), e))?;
    let mut done = if resumed { have } else { 0 };
    progress(done, total);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            TwError::new(ErrorCode::Io, format!("download interrupted: {e}")).with_hint("Run the same command again to resume.")
        })?;
        file.write_all(&chunk).await.map_err(|e| TwError::io(part.display(), e))?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush().await.map_err(|e| TwError::io(part.display(), e))?;
    file.sync_all().await.map_err(|e| TwError::io(part.display(), e))?;
    drop(file);
    if let Some(t) = total {
        if done != t {
            return Err(TwError::new(ErrorCode::Io, format!("download incomplete: {done} of {t} bytes"))
                .with_hint("Run the same command again to resume."));
        }
    }
    tokio::fs::rename(&part, dest).await.map_err(|e| TwError::io(dest.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Minimal HTTP/1.1 server serving `body`, honouring `Range: bytes=N-` when `ranges`.
    async fn serve(body: Vec<u8>, ranges: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let body = body.clone();
                tokio::spawn(async move {
                    let mut req = vec![0u8; 4096];
                    let n = sock.read(&mut req).await.unwrap();
                    let req = String::from_utf8_lossy(&req[..n]).to_ascii_lowercase();
                    let start = req
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|r| r.trim_end_matches('-').trim().trim_end_matches('-').parse::<usize>().ok());
                    let (status, slice) = match start {
                        Some(s) if ranges => ("206 Partial Content", &body[s..]),
                        _ => ("200 OK", &body[..]),
                    };
                    let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len());
                    sock.write_all(head.as_bytes()).await.unwrap();
                    sock.write_all(slice).await.unwrap();
                });
            }
        });
        format!("http://{addr}/win.iso")
    }

    #[tokio::test]
    async fn fresh_and_resumed_downloads() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let url = serve(body.clone(), true).await;
        let dir = tempfile::tempdir().unwrap();
        let http = reqwest::Client::new();

        let dest = dir.path().join("a.iso");
        download(&http, &url, &dest, |_, _| {}).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part_path(&dest).exists());

        // Simulate an interrupted download: first 50 000 bytes already present.
        let dest = dir.path().join("b.iso");
        std::fs::write(part_path(&dest), &body[..50_000]).unwrap();
        let mut first = None;
        download(&http, &url, &dest, |done, total| {
            first.get_or_insert((done, total));
        })
        .await
        .unwrap();
        assert_eq!(first, Some((50_000, Some(200_000))), "resumed from the part file");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    #[tokio::test]
    async fn server_without_range_restarts_cleanly() {
        let body: Vec<u8> = vec![7u8; 10_000];
        let url = serve(body.clone(), false).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("c.iso");
        std::fs::write(part_path(&dest), b"garbage-prefix").unwrap();
        download(&reqwest::Client::new(), &url, &dest, |_, _| {}).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }
}
