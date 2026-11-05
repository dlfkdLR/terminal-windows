//! Private file and directory helpers.
//!
//! On Unix, directories are created 0700 and files 0600. On Windows the store
//! lives under the per-user `%LOCALAPPDATA%`, whose inherited ACL already
//! restricts access to the user, SYSTEM and Administrators.

use crate::error::{TwError, TwResult};
use rand::RngCore;
use std::fs;
use std::io::Write;
use std::path::Path;

pub fn create_private_dir(path: &Path) -> TwResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(path).map_err(|e| TwError::io(path.display(), e))?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| TwError::io(path.display(), e))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path).map_err(|e| TwError::io(path.display(), e))?;
    Ok(())
}

/// Ensures `path` is a real directory (not a symlink) that we can use privately.
pub fn verify_private_dir(path: &Path) -> TwResult<()> {
    let meta = fs::symlink_metadata(path).map_err(|e| TwError::io(path.display(), e))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(TwError::new(crate::ErrorCode::Io, format!("{} is not a private directory", path.display())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(TwError::new(crate::ErrorCode::Io, format!("{} is accessible by other users; expected mode 0700", path.display())));
        }
    }
    Ok(())
}

/// Writes a file atomically (temp file + rename) with private permissions.
pub fn write_private_atomic(path: &Path, data: &[u8]) -> TwResult<()> {
    let dir = path.parent().ok_or_else(|| TwError::invalid("path has no parent"))?;
    let mut suffix = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut suffix);
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"), hex::encode(suffix)));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(TwError::io(path.display(), e));
    }
    Ok(())
}

/// A fresh 256-bit secret, hex encoded.
pub fn generate_key_hex() -> String {
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    hex::encode(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_content_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("k");
        write_private_atomic(&file, b"one").unwrap();
        write_private_atomic(&file, b"two").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "no temp files left behind");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_or_shared_dirs_are_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        create_private_dir(&real).unwrap();
        verify_private_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(verify_private_dir(&link).is_err());
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(verify_private_dir(&real).is_err());
    }

    #[test]
    fn keys_are_random_and_256_bit() {
        let a = generate_key_hex();
        assert_eq!(a.len(), 64);
        assert_ne!(a, generate_key_hex());
    }
}
