//! Whether Windows is installed on a VM, as far as the host can tell without the guest.
//!
//! When Windows Setup has written its boot files it registers a "Windows Boot Manager" boot entry
//! in the firmware's variable store (`efivars.fd`). That is a better signal than the disk size,
//! which is fooled by an installation that copied most files and was then interrupted.

use std::path::Path;

use crate::host::Arch;
use crate::model::NetworkMode;

/// The description Windows gives its UEFI boot entry.
const BOOT_ENTRY: &str = "Windows Boot Manager";

/// A disk file smaller than this has never held a Windows installation (a few hundred KB for a
/// fresh qcow2; a partitioned, formatted disk is a few hundred MB).
pub const VIRGIN_DISK_MAX_BYTES: u64 = 1 << 30;

/// Whether `haystack` contains `text` encoded as UTF-16LE (how UEFI stores boot entry names).
pub fn contains_utf16(haystack: &[u8], text: &str) -> bool {
    let needle: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle.as_slice())
}

/// Whether the variable store holds the Windows boot entry.
pub fn windows_registered(efivars: &Path) -> bool {
    std::fs::read(efivars).is_ok_and(|bytes| contains_utf16(&bytes, BOOT_ENTRY))
}

/// True when the VM still has its installer attached and Windows has not registered itself: the
/// only case in which a key press at the CD prompt is right. Pressing it on an installed VM would
/// restart Setup instead of booting Windows.
pub fn installer_needed(install_iso_attached: bool, windows_registered: bool) -> bool {
    install_iso_attached && !windows_registered
}

/// Whether the disk image has (practically) never been written to.
pub fn disk_is_virgin(disk: &Path) -> bool {
    std::fs::metadata(disk).map(|m| m.len() < VIRGIN_DISK_MAX_BYTES).unwrap_or(true)
}

/// Whether the wipe-capable answer file may be attached: the installer is attached, Windows is
/// not registered **and the disk is virgin**. A disk with data on it (even a half-finished
/// installation) is never wiped automatically; there Setup stays interactive.
pub fn unattended_allowed(install_iso_attached: bool, efivars: &Path, disk: &Path) -> bool {
    installer_needed(install_iso_attached, windows_registered(efivars)) && disk_is_virgin(disk)
}

/// What goes on the setup-media CD of a VM start.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct SetupMedia {
    /// The wipe-capable answer file (`unattended_allowed` and the VM was created with `--unattended`).
    pub answer_file: bool,
    /// The NetKVM driver folder for Windows Setup (`$WinPEDriver$`).
    pub drivers: bool,
}

/// Decides the setup-media contents. The drivers need no answer file and are harmless, so they
/// stay while the installer is attached (also lets OOBE's driver picker find them); the answer
/// file follows the strict `unattended_allowed` rule.
pub fn setup_media(
    unattended: bool,
    install_iso_attached: bool,
    efivars: &Path,
    disk: &Path,
    arch: Arch,
    network: NetworkMode,
) -> SetupMedia {
    SetupMedia {
        answer_file: unattended && unattended_allowed(install_iso_attached, efivars, disk),
        drivers: install_iso_attached && arch == Arch::Aarch64 && network == NetworkMode::Nat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_boot_entry_names() {
        let mut store = vec![0xffu8; 1000];
        assert!(!contains_utf16(&store, BOOT_ENTRY));
        let entry: Vec<u8> = BOOT_ENTRY.encode_utf16().flat_map(u16::to_le_bytes).collect();
        store[300..300 + entry.len()].copy_from_slice(&entry);
        assert!(contains_utf16(&store, BOOT_ENTRY));
        assert!(!contains_utf16(BOOT_ENTRY.as_bytes(), BOOT_ENTRY), "plain ASCII is not a UEFI name");
        assert!(!contains_utf16(&store, ""));
    }

    #[test]
    fn installer_rule() {
        assert!(installer_needed(true, false));
        assert!(!installer_needed(true, true), "Windows is installed: boot the disk");
        assert!(!installer_needed(false, false), "no installer attached");
    }

    #[test]
    fn unattended_never_touches_disks_with_data_or_installed_windows() {
        let tmp = tempfile::tempdir().unwrap();
        let (vars, disk) = (tmp.path().join("efivars.fd"), tmp.path().join("disk.qcow2"));
        std::fs::write(&vars, vec![0xffu8; 4096]).unwrap();
        let sparse = std::fs::File::create(&disk).unwrap();
        sparse.set_len(300 * 1024).unwrap();
        assert!(unattended_allowed(true, &vars, &disk), "virgin disk, no Windows, installer attached");
        assert!(!unattended_allowed(false, &vars, &disk), "no installer attached");

        let mut registered = vec![0xffu8; 4096];
        let entry: Vec<u8> = BOOT_ENTRY.encode_utf16().flat_map(u16::to_le_bytes).collect();
        registered[100..100 + entry.len()].copy_from_slice(&entry);
        std::fs::write(&vars, registered).unwrap();
        assert!(!unattended_allowed(true, &vars, &disk), "Windows registered");

        std::fs::write(&vars, vec![0xffu8; 4096]).unwrap();
        std::fs::File::create(&disk).unwrap().set_len(7 << 30).unwrap();
        assert!(!unattended_allowed(true, &vars, &disk), "7 GB of data on the disk: a half-finished install is never wiped");
        assert!(!windows_registered(&tmp.path().join("missing")));
    }

    #[test]
    fn setup_media_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let (vars, disk) = (tmp.path().join("efivars.fd"), tmp.path().join("disk.qcow2"));
        std::fs::write(&vars, vec![0xffu8; 4096]).unwrap();
        std::fs::File::create(&disk).unwrap().set_len(300 * 1024).unwrap();
        let m = |un, att, arch, net| setup_media(un, att, &vars, &disk, arch, net);
        let (a, x, nat, off) = (Arch::Aarch64, Arch::X86_64, NetworkMode::Nat, NetworkMode::Offline);
        assert_eq!(m(true, true, a, nat), SetupMedia { answer_file: true, drivers: true });
        assert_eq!(
            m(false, true, a, nat),
            SetupMedia { answer_file: false, drivers: true },
            "manual install still gets drivers, never an answer file"
        );
        assert_eq!(m(true, true, x, nat), SetupMedia { answer_file: true, drivers: false }, "x64 has an inbox NIC driver");
        assert_eq!(m(true, true, a, off), SetupMedia { answer_file: true, drivers: false }, "no NIC, no driver");
        assert_eq!(m(true, false, a, nat), SetupMedia { answer_file: false, drivers: false }, "installer detached");
        // A disk with data is never wiped automatically, but the drivers stay.
        std::fs::File::create(&disk).unwrap().set_len(5 << 30).unwrap();
        assert_eq!(m(true, true, a, nat), SetupMedia { answer_file: false, drivers: true });
    }
}
