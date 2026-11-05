//! Verified ISO cache: `<data>/images/<file>.iso` plus a `<file>.iso.json` record.
//! Only images whose SHA-256 matched Microsoft's published value are recorded.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tw_core::host::Arch;
use tw_core::{secure, TwError, TwResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRecord {
    pub file: String,
    pub arch: Arch,
    pub language: String,
    pub version: Option<String>,
    pub sha256: String,
    /// Page whose published hash matched.
    pub verified_against: String,
    /// "microsoft-download", "browser-download" or "import".
    pub source: String,
    pub added_at: String,
}

#[derive(Debug, Clone)]
pub struct CachedImage {
    pub path: PathBuf,
    pub record: ImageRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Move,
    Copy,
}

pub fn images_dir(root: &Path) -> PathBuf {
    root.join("images")
}

/// `Win11_26H2_Korean_Arm64.iso` — mirrors Microsoft's own naming.
pub fn file_name(version: Option<&str>, language: &str, arch: Arch) -> String {
    let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>();
    let arch = match arch {
        Arch::Aarch64 => "Arm64",
        Arch::X86_64 => "x64",
    };
    match version {
        Some(v) => format!("Win11_{}_{}_{arch}.iso", clean(v), clean(language)),
        None => format!("Win11_{}_{arch}.iso", clean(language)),
    }
}

fn record_path(iso: &Path) -> PathBuf {
    let mut name = iso.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".json");
    iso.with_file_name(name)
}

/// Places a verified ISO into the cache and writes its record.
pub fn register(dir: &Path, source: &Path, record: ImageRecord, transfer: Transfer) -> TwResult<CachedImage> {
    secure::create_private_dir(dir)?;
    let dest = dir.join(&record.file);
    if dest != source {
        if dest.exists() {
            return Err(TwError::new(tw_core::ErrorCode::AlreadyExists, format!("{} is already cached", dest.display())));
        }
        match transfer {
            Transfer::Move => {
                if std::fs::rename(source, &dest).is_err() {
                    // Different volume: copy, then remove the original.
                    std::fs::copy(source, &dest).map_err(|e| TwError::io(dest.display(), e))?;
                    std::fs::remove_file(source).map_err(|e| TwError::io(source.display(), e))?;
                }
            }
            Transfer::Copy => {
                std::fs::copy(source, &dest).map_err(|e| TwError::io(dest.display(), e))?;
            }
        }
    }
    let json = serde_json::to_vec_pretty(&record).map_err(|e| TwError::backend(e.to_string()))?;
    secure::write_private_atomic(&record_path(&dest), &json)?;
    Ok(CachedImage { path: dest, record })
}

/// Cached images with a valid record whose file still exists, newest first.
pub fn list(dir: &Path) -> Vec<CachedImage> {
    let mut out: Vec<CachedImage> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".iso.json"))
        .filter_map(|p| {
            let record: ImageRecord = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            let iso = p.with_file_name(&record.file);
            iso.is_file().then_some(CachedImage { path: iso, record })
        })
        .collect();
    out.sort_by(|a, b| b.record.added_at.cmp(&a.record.added_at));
    out
}

pub fn find(dir: &Path, arch: Arch, language: Option<&str>) -> Option<CachedImage> {
    list(dir).into_iter().find(|c| c.record.arch == arch && language.is_none_or(|l| crate::microsoft::same_language(&c.record.language, l)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(file: &str, arch: Arch, lang: &str, at: &str) -> ImageRecord {
        ImageRecord {
            file: file.into(),
            arch,
            language: lang.into(),
            version: Some("26H2".into()),
            sha256: "00".repeat(32),
            verified_against: "https://www.microsoft.com/".into(),
            source: "import".into(),
            added_at: at.into(),
        }
    }

    #[test]
    fn naming() {
        assert_eq!(file_name(Some("26H2"), "Korean", Arch::Aarch64), "Win11_26H2_Korean_Arm64.iso");
        assert_eq!(file_name(None, "English (United Kingdom)", Arch::X86_64), "Win11_English__United_Kingdom__x64.iso");
    }

    #[test]
    fn register_list_find() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = images_dir(tmp.path());
        let src = tmp.path().join("download.iso");
        std::fs::write(&src, b"iso").unwrap();
        let c =
            register(&dir, &src, record("Win11_26H2_Korean_Arm64.iso", Arch::Aarch64, "Korean", "2026-11-05T01"), Transfer::Copy).unwrap();
        assert!(src.exists(), "copy keeps the original");
        assert!(c.path.is_file());

        let src2 = tmp.path().join("dl2.iso");
        std::fs::write(&src2, b"iso2").unwrap();
        register(&dir, &src2, record("Win11_26H2_English_Arm64.iso", Arch::Aarch64, "English", "2026-11-05T02"), Transfer::Move).unwrap();
        assert!(!src2.exists(), "move removes the original");

        assert_eq!(list(&dir).len(), 2);
        assert_eq!(find(&dir, Arch::Aarch64, None).unwrap().record.language, "English", "newest first");
        assert_eq!(find(&dir, Arch::Aarch64, Some("korean")).unwrap().record.language, "Korean");
        assert!(find(&dir, Arch::X86_64, None).is_none());

        let dup = tmp.path().join("dup.iso");
        std::fs::write(&dup, b"x").unwrap();
        assert!(register(&dir, &dup, record("Win11_26H2_Korean_Arm64.iso", Arch::Aarch64, "Korean", "t"), Transfer::Copy).is_err());

        std::fs::remove_file(&c.path).unwrap();
        assert_eq!(list(&dir).len(), 1, "records without their ISO are ignored");
    }
}
