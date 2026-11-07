//! Drivers shipped inside the binary so a fresh Windows 11 ARM64 install can see a network adapter.
//!
//! The ARM64 `install.wim` has no inbox driver for any NIC that QEMU can emulate on `virt`
//! (measured 2026-11-05/07: virtio-net-pci found nothing, usb-net binds the generic `usbser`
//! COM-port driver because QEMU's RNDIS interface descriptor has no MS-OS compatible ID).
//! OOBE then stops at "Connect to a network" (error 0xe0000100). The Red Hat/virtio-win NetKVM
//! driver is Microsoft-signed and BSD-3-Clause licensed, so it is carried unmodified.
//! Provenance: `assets/netkvm-arm64/SOURCE.txt`, `docs/PROVENANCE.md`.

/// Folder Windows Setup scans at the root of every drive (including the answer-file CD) and
/// adds to both WinPE and the installed image. Upper-case because the ISO is ISO 9660.
pub const SETUP_DRIVER_DIR: &str = "$WINPEDRIVER$";

const NETKVM_INF: &[u8] = include_bytes!("../assets/netkvm-arm64/netkvm.inf");
const NETKVM_SYS: &[u8] = include_bytes!("../assets/netkvm-arm64/netkvm.sys");
const NETKVM_CAT: &[u8] = include_bytes!("../assets/netkvm-arm64/netkvm.cat");
const NETKVMP_EXE: &[u8] = include_bytes!("../assets/netkvm-arm64/netkvmp.exe");
const LICENSE: &[u8] = include_bytes!("../assets/netkvm-arm64/LICENSE-virtio-win.txt");

/// ISO paths (one directory level) and contents of the NetKVM ARM64 driver package.
pub fn netkvm_arm64_files() -> Vec<(String, &'static [u8])> {
    let d = SETUP_DRIVER_DIR;
    vec![
        (format!("{d}/NETKVM.INF"), NETKVM_INF),
        (format!("{d}/NETKVM.SYS"), NETKVM_SYS),
        (format!("{d}/NETKVM.CAT"), NETKVM_CAT),
        (format!("{d}/NETKVMP.EXE"), NETKVMP_EXE),
        (format!("{d}/LICENSE.TXT"), LICENSE),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_is_the_expected_arm64_driver() {
        let files = netkvm_arm64_files();
        assert_eq!(files.len(), 5);
        let inf = String::from_utf8_lossy(NETKVM_INF);
        assert!(inf.contains("NTARM64"), "INF targets ARM64");
        assert!(inf.contains("VEN_1AF4&DEV_1000"), "INF matches the virtio-net PCI id");
        assert!(NETKVM_SYS.starts_with(b"MZ"), "the .sys is a PE file");
        assert!(String::from_utf8_lossy(LICENSE).contains("Redistribution"), "license text travels with the binary");
    }
}
