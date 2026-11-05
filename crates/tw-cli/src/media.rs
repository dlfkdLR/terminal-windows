//! `terminal-windows image …`: official Windows media, verified before use.

use super::{now_rfc3339, ImageCommand, Output};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tw_core::host::{Arch, HostInfo};
use tw_core::store::Store;
use tw_core::{ErrorCode, TwError, TwResult};
use tw_media::cache::{self, CachedImage, ImageRecord, Transfer};
use tw_media::microsoft::{self, Client, LinkOutcome, PageInfo};

/// Prints progress to stderr at most once per second.
struct Progress {
    label: &'static str,
    last: Option<Instant>,
}

impl Progress {
    fn new(label: &'static str) -> Self {
        Self { label, last: None }
    }
    fn update(&mut self, done: u64, total: Option<u64>) {
        let finished = total.is_some_and(|t| done >= t);
        if !finished && self.last.is_some_and(|l| l.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.last = Some(Instant::now());
        let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
        match total {
            Some(t) if t > 0 => {
                eprint!("\r{}: {:5.1}%  {:.2} / {:.2} GiB   ", self.label, done as f64 * 100.0 / t as f64, gib(done), gib(t))
            }
            _ => eprint!("\r{}: {:.2} GiB   ", self.label, gib(done)),
        }
        if finished {
            eprintln!();
        }
    }
}

fn arch_or_host(arch: Option<Arch>, host: HostInfo) -> TwResult<Arch> {
    let arch = match arch {
        Some(a) => a,
        None => host.native_guest_arch()?,
    };
    if host.check_guest_arch(arch).is_err() {
        eprintln!("note: a {} image cannot run with acceleration on this host; downloading anyway as requested.", arch.windows_name());
    }
    Ok(arch)
}

/// Hashes `path` and requires it to match a hash Microsoft publishes for `arch`.
async fn verify_official(client: &Client, page: &PageInfo, path: &Path) -> TwResult<(String, String)> {
    let mut progress = Progress::new("Verifying SHA-256");
    let sha = tw_media::verify::sha256_file(path, |d, t| progress.update(d, Some(t))).await?;
    if let Some(h) = page.match_hash(&sha) {
        return Ok((sha, h.language.clone()));
    }
    let other = match page.arch {
        Arch::Aarch64 => Arch::X86_64,
        Arch::X86_64 => Arch::Aarch64,
    };
    if let Ok(other_page) = client.page(other).await {
        if other_page.match_hash(&sha).is_some() {
            return Err(TwError::new(
                ErrorCode::Incompatible,
                format!("{} is the official {} image, not {}", path.display(), other.windows_name(), page.arch.windows_name()),
            )
            .with_hint("Pass --arch to match the image, or download the image for this host."));
        }
    }
    Err(TwError::invalid(format!("SHA-256 {sha} of {} does not match any hash Microsoft publishes for the current Windows 11 {} release", path.display(), page.arch.windows_name()))
        .with_hint("The file may be incomplete, modified, or an older release. Unverified media can still be used explicitly with `create --iso PATH`."))
}

fn record(page: &PageInfo, language: &str, sha: String, source: &str) -> ImageRecord {
    ImageRecord {
        file: cache::file_name(page.version.as_deref(), language, page.arch),
        arch: page.arch,
        language: language.to_string(),
        version: page.version.clone(),
        sha256: sha,
        verified_against: page.url.clone(),
        source: source.to_string(),
        added_at: now_rfc3339(),
    }
}

fn open_browser(url: &str) -> bool {
    // The URL is a fixed Microsoft address, never user or network input.
    let status = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else if cfg!(windows) {
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).status()
    } else {
        std::process::Command::new("xdg-open").arg(url).status()
    };
    status.is_ok_and(|s| s.success())
}

fn image_json(c: &CachedImage) -> Value {
    json!({ "path": c.path, "arch": c.record.arch, "language": c.record.language, "version": c.record.version,
            "sha256": c.record.sha256, "source": c.record.source, "verified_against": c.record.verified_against })
}

pub async fn run(command: ImageCommand, store: &Store, host: HostInfo, out: &Output) -> TwResult<()> {
    let dir = cache::images_dir(store.root());
    match command {
        ImageCommand::List => {
            let images: Vec<Value> = cache::list(&dir).iter().map(image_json).collect();
            out.emit(json!({ "directory": dir, "images": images }), |v| {
                let rows = v["images"].as_array().cloned().unwrap_or_default();
                if rows.is_empty() {
                    return format!("No verified images in {}.\nDownload one with: terminal-windows image download", dir.display());
                }
                rows.iter()
                    .map(|r| {
                        format!(
                            "{}  [{} {} {}, verified]",
                            r["path"].as_str().unwrap_or(""),
                            r["version"].as_str().unwrap_or("?"),
                            r["language"].as_str().unwrap_or(""),
                            r["arch"].as_str().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        ImageCommand::Info { arch, language } => {
            let arch = arch_or_host(arch, host)?;
            let page = Client::new()?.page(arch).await?;
            let hash = page.hash_for(&language).map(|h| h.sha256.clone());
            let languages: Vec<&str> = page.hashes.iter().map(|h| h.language.as_str()).collect();
            out.emit(
                json!({ "edition": page.edition_name, "edition_id": page.edition_id, "version": page.version, "arch": arch,
                        "language": language, "sha256": hash, "page": page.url, "languages": languages }),
                |v| {
                    format!(
                        "{} ({})\nversion: {}\n{} SHA-256: {}\nofficial page: {}",
                        v["edition"].as_str().unwrap_or(""),
                        arch.windows_name(),
                        v["version"].as_str().unwrap_or("?"),
                        language,
                        v["sha256"].as_str().unwrap_or("(language not found)"),
                        v["page"].as_str().unwrap_or("")
                    )
                },
            );
        }
        ImageCommand::Import { path, arch, move_file } => {
            let arch = arch_or_host(arch, host)?;
            let client = Client::new()?;
            let page = client.page(arch).await?;
            let (sha, language) = verify_official(&client, &page, &path).await?;
            let transfer = if move_file { Transfer::Move } else { Transfer::Copy };
            let cached = cache::register(&dir, &path, record(&page, &language, sha, "import"), transfer)?;
            out.emit(image_json(&cached), |_| {
                format!("Verified against Microsoft's published SHA-256 and cached:\n{}", cached.path.display())
            });
        }
        ImageCommand::Download { arch, language, no_browser, wait_minutes } => {
            let arch = arch_or_host(arch, host)?;
            let client = Client::new()?;
            let page = client.page(arch).await?;
            let canonical = page
                .hashes
                .iter()
                .find(|h| microsoft::same_language(&h.language, &language))
                .map(|h| h.language.clone())
                .ok_or_else(|| {
                    TwError::invalid(format!("Microsoft publishes no {language:?} image"))
                        .with_hint(format!("Available: {}", page.hashes.iter().map(|h| h.language.as_str()).collect::<Vec<_>>().join(", ")))
                })?;
            if let Some(c) = cache::find(&dir, arch, Some(&canonical)).filter(|c| c.record.version == page.version) {
                out.emit(image_json(&c), |_| format!("Already cached and verified: {}", c.path.display()));
                return Ok(());
            }
            eprintln!("{} — Windows 11 {} {} ({canonical})", page.edition_name, page.version.as_deref().unwrap_or(""), arch.windows_name());

            let session = microsoft::session_id();
            let skus = client.skus(&page, &session).await?;
            let sku = skus
                .iter()
                .find(|s| microsoft::same_language(&s.language, &canonical))
                .ok_or_else(|| TwError::backend(format!("no download SKU for {canonical}")))?;
            let cached = match client.link(&page, sku, &session).await? {
                LinkOutcome::Granted(url) => {
                    tw_core::secure::create_private_dir(&dir)?;
                    let dest = dir.join(cache::file_name(page.version.as_deref(), &canonical, arch));
                    let mut progress = Progress::new("Downloading");
                    tw_media::download::download(client.http(), &url, &dest, |d, t| progress.update(d, t)).await?;
                    match verify_official(&client, &page, &dest).await {
                        Ok((sha, lang)) => cache::register(&dir, &dest, record(&page, &lang, sha, "microsoft-download"), Transfer::Move)?,
                        Err(e) => {
                            let _ = std::fs::remove_file(&dest);
                            return Err(e);
                        }
                    }
                }
                LinkOutcome::Rejected(reason) => {
                    eprintln!("Microsoft declined the automated download request: {reason}");
                    eprintln!("terminal-windows does not work around Microsoft's bot protection. Finishing in your browser instead:");
                    let downloads =
                        dirs::download_dir().ok_or_else(|| TwError::new(ErrorCode::Io, "cannot determine the Downloads folder"))?;
                    if no_browser || !open_browser(&page.url) {
                        eprintln!("  Open: {}", page.url);
                    } else {
                        eprintln!("  Opened {}", page.url);
                    }
                    eprintln!("  1. Select \"{}\" and click Download Now", page.edition_name);
                    eprintln!("  2. Choose the language \"{canonical}\" and click Confirm");
                    eprintln!("  3. Click the 64-bit Download button and save to {}", downloads.display());
                    eprintln!("Waiting for the download to finish (up to {wait_minutes} minutes; Ctrl+C to stop)...");
                    let found: PathBuf = tw_media::watch::wait_for_new_iso(
                        &downloads,
                        Duration::from_secs(wait_minutes * 60),
                        Duration::from_secs(5),
                        Duration::from_secs(2),
                    )
                    .await?;
                    eprintln!("Found {}", found.display());
                    let (sha, lang) = verify_official(&client, &page, &found).await?;
                    cache::register(&dir, &found, record(&page, &lang, sha, "browser-download"), Transfer::Move)?
                }
            };
            out.emit(image_json(&cached), |_| {
                format!(
                    "Verified against Microsoft's published SHA-256 and cached:\n{}\nCreate a VM with: terminal-windows create NAME",
                    cached.path.display()
                )
            });
        }
    }
    Ok(())
}
