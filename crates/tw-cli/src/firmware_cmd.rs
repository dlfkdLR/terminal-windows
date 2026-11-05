//! `terminal-windows firmware …`: Secure Boot capable UEFI firmware for AArch64 guests.
//!
//! Windows 11 Setup refuses to run without Secure Boot support, and the firmware
//! bundled with Homebrew's QEMU has none. The firmware built by `firmware/build.sh`
//! (edk2 + `SECURE_BOOT_ENABLE`) is published with each release together with
//! its SHA-256 and installed here.

use super::{FirmwareCommand, Output};
use serde_json::json;
use std::io::Read;
use std::path::PathBuf;
use tw_core::host::{Arch, HostInfo};
use tw_core::store::Store;
use tw_core::{secure, ErrorCode, TwError, TwResult};
use tw_qemu::firmware::{self, MANAGED_AARCH64_SECBOOT};

const RELEASE_BASE: &str = "https://github.com/dlfkdLR/terminal-windows/releases/latest/download";
const FLASH_SIZE: u64 = 64 * 1024 * 1024;

/// Decompresses a gzip stream, refusing anything larger than a flash image.
fn gunzip_limited(data: &[u8]) -> TwResult<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(FLASH_SIZE + 1)
        .read_to_end(&mut out)
        .map_err(|e| TwError::invalid(format!("corrupt firmware archive: {e}")))?;
    Ok(out)
}

fn check_flash(name: &str, data: &[u8]) -> TwResult<()> {
    if data.len() as u64 != FLASH_SIZE {
        return Err(TwError::invalid(format!("{name} is {} bytes; UEFI flash images must be exactly 64 MiB", data.len())));
    }
    Ok(())
}

fn install_files(store: &Store, code: &[u8], vars: &[u8]) -> TwResult<PathBuf> {
    check_flash("code.fd", code)?;
    check_flash("vars.fd", vars)?;
    let root = store.root().join("firmware");
    secure::create_private_dir(&root)?;
    let dir = root.join(MANAGED_AARCH64_SECBOOT);
    secure::create_private_dir(&dir)?;
    let (code_path, vars_path) = (dir.join("code.fd"), dir.join("vars.fd"));
    secure::write_private_atomic(&code_path, code)?;
    secure::write_private_atomic(&vars_path, vars)?;
    firmware::write_managed_descriptor(&root, MANAGED_AARCH64_SECBOOT, Arch::Aarch64, &code_path, &vars_path)
        .map_err(|e| TwError::io(root.display(), e))?;
    Ok(dir)
}

async fn fetch(http: &reqwest::Client, url: &str) -> TwResult<Vec<u8>> {
    let resp = http.get(url).send().await.map_err(|e| TwError::new(ErrorCode::Io, format!("GET {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(TwError::backend(format!("GET {url}: HTTP {}", resp.status()))
            .with_hint("Has a release with firmware been published yet? Build it yourself with firmware/build.sh and use --from DIR."));
    }
    resp.bytes().await.map(|b| b.to_vec()).map_err(|e| TwError::new(ErrorCode::Io, format!("GET {url}: {e}")))
}

fn expected_sha(sums: &str, file: &str) -> TwResult<String> {
    sums.lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .find(|(_, name)| name.trim().trim_start_matches('*') == file)
        .map(|(h, _)| h.to_ascii_lowercase())
        .ok_or_else(|| TwError::backend(format!("{file} is not listed in SHA256SUMS")))
}

pub async fn run(command: FirmwareCommand, store: &Store, host: HostInfo, out: &Output) -> TwResult<()> {
    match command {
        FirmwareCommand::Install { from } => {
            if host.arch != Some(Arch::Aarch64) {
                return Err(TwError::unsupported("managed firmware is only needed on AArch64 hosts")
                    .with_hint("On x86-64 hosts QEMU ships Secure Boot capable OVMF firmware."));
            }
            let (code, vars, source) = match from {
                Some(dir) => {
                    let read = |n: &str| std::fs::read(dir.join(n)).map_err(|e| TwError::io(dir.join(n).display(), e));
                    (read("code.fd")?, read("vars.fd")?, format!("local build in {}", dir.display()))
                }
                None => {
                    let http = tw_media::download::http_client()?;
                    let sums = String::from_utf8_lossy(&fetch(&http, &format!("{RELEASE_BASE}/SHA256SUMS")).await?).into_owned();
                    let mut files = Vec::new();
                    for name in ["code", "vars"] {
                        let asset = format!("{MANAGED_AARCH64_SECBOOT}-{name}.fd.gz");
                        eprintln!("Downloading {asset} ...");
                        let gz = fetch(&http, &format!("{RELEASE_BASE}/{asset}")).await?;
                        let want = expected_sha(&sums, &asset)?;
                        let got = tw_media::verify::sha256_bytes(&gz);
                        if want != got {
                            return Err(TwError::backend(format!("SHA-256 mismatch for {asset} (expected {want}, got {got})")));
                        }
                        files.push(gunzip_limited(&gz)?);
                    }
                    let vars = files.pop().unwrap_or_default();
                    let code = files.pop().unwrap_or_default();
                    (code, vars, format!("release {RELEASE_BASE}"))
                }
            };
            let dir = install_files(store, &code, &vars)?;
            out.emit(json!({ "installed": dir, "source": source }), |_| {
                format!("Installed Secure Boot capable UEFI firmware ({source}) to {}\nNew VMs use it automatically.", dir.display())
            });
        }
        FirmwareCommand::List => {
            let arch = host.native_guest_arch()?;
            let install = tw_qemu::detect::find_qemu(&host, arch).await.ok();
            let dirs = firmware::descriptor_dirs(install.as_ref().and_then(|i| i.prefix.as_deref()), Some(&store.root().join("firmware")));
            let found = firmware::discover(&dirs, arch);
            out.emit(json!({ "arch": arch, "firmware": found }), |_| {
                if found.is_empty() {
                    return "No UEFI firmware found. Install QEMU (brew install qemu) and run: terminal-windows firmware install".into();
                }
                found
                    .iter()
                    .map(|f| {
                        format!(
                            "{}  [{}] {}",
                            f.code.display(),
                            if f.secure_boot { "secure boot" } else { "NO secure boot: unusable for Windows 11" },
                            f.description
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;

    #[test]
    fn gunzip_round_trip_and_bomb_guard() {
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(&[7u8; 1000]).unwrap();
        assert_eq!(gunzip_limited(&enc.finish().unwrap()).unwrap(), vec![7u8; 1000]);

        let mut big = GzEncoder::new(Vec::new(), Compression::fast());
        big.write_all(&vec![0u8; (FLASH_SIZE + 4096) as usize]).unwrap();
        let out = gunzip_limited(&big.finish().unwrap()).unwrap();
        assert!(out.len() as u64 > FLASH_SIZE, "oversized output is returned truncated, then rejected by check_flash");
        assert!(check_flash("code.fd", &out).is_err());
        assert!(gunzip_limited(b"not gzip").is_err());
    }

    #[test]
    fn sums_lookup() {
        let sums = "AA11  edk2-aarch64-secboot-code.fd.gz\nbb22 *other.tar.gz\n";
        assert_eq!(expected_sha(sums, "edk2-aarch64-secboot-code.fd.gz").unwrap(), "aa11");
        assert_eq!(expected_sha(sums, "other.tar.gz").unwrap(), "bb22");
        assert!(expected_sha(sums, "missing").is_err());
    }

    #[test]
    fn install_writes_descriptor_that_discovery_finds() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        let flash = vec![0u8; FLASH_SIZE as usize];
        assert!(install_files(&store, &flash[..10], &flash).is_err(), "wrong size rejected");
        install_files(&store, &flash, &flash).unwrap();
        let found = firmware::discover(&[tmp.path().join("firmware")], Arch::Aarch64);
        assert_eq!(found.len(), 1);
        assert!(found[0].secure_boot && !found[0].enrolled_keys);
        assert!(found[0].code.ends_with("code.fd"));
    }
}
