//! Bridges a blocking, full-duplex byte device (the virtio-serial port opened
//! as a file) to an async stream. A blocking read must never hold up writes,
//! so reading and writing run on two dedicated threads over cloned handles.

use std::io::{Read, Write};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

/// Returns the async end. The threads exit when the device or the stream closes.
pub fn bridge<R, W>(mut reader: R, mut writer: W) -> DuplexStream
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let (ours, theirs) = tokio::io::duplex(256 * 1024);
    let (mut rd, mut wr) = tokio::io::split(ours);
    let handle = tokio::runtime::Handle::current();
    let h2 = handle.clone();
    // Device → async.
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if handle.block_on(wr.write_all(&buf[..n])).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = handle.block_on(wr.shutdown());
    });
    // Async → device.
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match h2.block_on(rd.read(&mut buf)) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if writer.write_all(&buf[..n]).and_then(|_| writer.flush()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    theirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct ChanReader(mpsc::Receiver<Vec<u8>>, Vec<u8>);
    impl Read for ChanReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(v) => self.1 = v,
                    Err(_) => return Ok(0),
                }
            }
            let n = out.len().min(self.1.len());
            out[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }
    struct ChanWriter(mpsc::Sender<Vec<u8>>);
    impl Write for ChanWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.send(b.to_vec()).map_err(|_| std::io::ErrorKind::BrokenPipe)?;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn writes_are_not_blocked_by_a_pending_read() {
        let (to_dev_tx, to_dev_rx) = mpsc::channel();
        let (from_dev_tx, from_dev_rx) = mpsc::channel::<Vec<u8>>();
        let mut stream = bridge(ChanReader(from_dev_rx, Vec::new()), ChanWriter(to_dev_tx));
        // The reader thread is blocked (nothing from the device yet); writing must still work.
        stream.write_all(b"hello").await.unwrap();
        let got = tokio::task::spawn_blocking(move || to_dev_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap()).await.unwrap();
        assert_eq!(got, b"hello");
        from_dev_tx.send(b"world".to_vec()).unwrap();
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"world");
    }
}
