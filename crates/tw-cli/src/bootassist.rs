//! Pressing "any key" for the Windows installer's "boot from CD or DVD" prompt.
//!
//! The Windows ISO's boot manager waits about three seconds for a key and otherwise falls
//! through to the next boot device. terminal-mac installs macOS through an API and never needs
//! this; for Windows, `start` and `up` do it themselves whenever it is needed, so a VM whose
//! installation was interrupted never ends up in the UEFI shell.

use tw_core::service::VmService;
use tw_core::{TwResult, VmName};

/// The description Windows gives its UEFI boot entry.
const BOOT_ENTRY: &str = "Windows Boot Manager";

/// Whether `haystack` contains `text` encoded as UTF-16LE (how UEFI stores boot entry names).
pub fn contains_utf16(haystack: &[u8], text: &str) -> bool {
    let needle: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle.as_slice())
}

/// Whether the firmware's variable store holds the "Windows Boot Manager" boot entry, which
/// Windows Setup registers once it has written its boot files. Unlike the disk size, this is
/// not fooled by an installation that copied most files and was then interrupted.
pub fn windows_registered(efivars: &std::path::Path) -> bool {
    std::fs::read(efivars).is_ok_and(|bytes| contains_utf16(&bytes, BOOT_ENTRY))
}

/// True when the VM still has its installer attached and Windows has not registered itself,
/// which is the only case in which a key press at the CD prompt is right. Pressing it on an
/// installed VM would restart Setup instead of booting Windows.
pub fn installer_needed(install_iso_attached: bool, windows_registered: bool) -> bool {
    install_iso_attached && !windows_registered
}

/// Waits for the prompt and presses a key if this VM needs its installer.
pub async fn assist(service: &VmService, name: &VmName) {
    let Ok(config) = service.load(name) else { return };
    if !installer_needed(config.install_iso.is_some(), windows_registered(&service.store().paths(name).efivars)) {
        return;
    }
    eprintln!("      waiting for the \"boot from CD\" prompt ...");
    match press_key_at_boot_prompt(service, name).await {
        Ok(true) => eprintln!("      pressed a key; Windows Setup is starting"),
        Ok(false) => eprintln!("      no boot prompt appeared; open the viewer and press a key if the VM shows \"Press any key\""),
        Err(e) => eprintln!("      warning: could not watch for the boot prompt: {}", e.message),
    }
}

/// Whether `frame` is the Windows boot manager's "Press any key to boot from CD or DVD" screen:
/// a few hundred lit pixels, all inside the top rows (measured 2026-11-05: about 600 lit
/// pixels within rows 6-20 of 800x600; the firmware logo, Setup and the UEFI shell all differ).
pub fn is_boot_prompt(frame: &tw_core::control::RawFrame) -> bool {
    let width = frame.width as usize;
    let (mut lit, mut max_row) = (0usize, 0usize);
    for (row, pixels) in frame.rgb.chunks_exact(width * 3).enumerate() {
        let row_lit = pixels.as_chunks::<3>().0.iter().filter(|p| p.iter().any(|c| *c > 24)).count();
        if row_lit > 0 {
            lit += row_lit;
            max_row = row;
            if row >= 40 || lit > 4000 {
                return false;
            }
        }
    }
    (50..=4000).contains(&lit) && max_row < 40
}

/// Presses a key while the boot prompt is on screen. Returns whether a key was pressed.
pub async fn press_key_at_boot_prompt(service: &VmService, name: &VmName) -> TwResult<bool> {
    let console = service.interactive_console(name).await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    let mut pressed = false;
    let mut quiet_frames = 0;
    while tokio::time::Instant::now() < deadline {
        let frame = console.frame().await?;
        if is_boot_prompt(&frame) {
            console.key("space").await?;
            pressed = true;
            quiet_frames = 0;
        } else if pressed {
            quiet_frames += 1;
            if quiet_frames >= 3 {
                return Ok(true);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    Ok(pressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(lit: &[(usize, usize)]) -> tw_core::control::RawFrame {
        let (width, height) = (800usize, 600usize);
        let mut rgb = vec![0u8; width * height * 3];
        for (x, y) in lit {
            let i = (y * width + x) * 3;
            rgb[i..i + 3].copy_from_slice(&[255, 255, 255]);
        }
        tw_core::control::RawFrame { width: width as u32, height: height as u32, rgb }
    }

    #[test]
    fn boot_prompt_detection() {
        // About 600 lit pixels in rows 6-20, like the real "Press any key" screen.
        let text: Vec<(usize, usize)> = (0..600).map(|i| (1 + i % 360, 6 + (i / 360) * 7 + i % 7)).collect();
        assert!(is_boot_prompt(&frame(&text)));
        assert!(!is_boot_prompt(&frame(&[])), "black screen");
        let logo: Vec<(usize, usize)> = (0..11_685).map(|i| (303 + i % 192, 271 + i / 192)).collect();
        assert!(!is_boot_prompt(&frame(&logo)), "firmware logo");
        let shell: Vec<(usize, usize)> = (0..14_793).map(|i| (i % 700, 8 + i / 700 * 12)).collect();
        assert!(!is_boot_prompt(&frame(&shell)), "text-filled screen");
        let tiny: Vec<(usize, usize)> = (0..10).map(|i| (i, 5)).collect();
        assert!(!is_boot_prompt(&frame(&tiny)), "a stray pixel row is not a prompt");
    }

    #[test]
    fn installer_is_only_needed_for_a_vm_without_windows() {
        assert!(installer_needed(true, false), "installer attached, Windows not registered");
        assert!(!installer_needed(true, true), "Windows is installed: boot the disk");
        assert!(!installer_needed(false, false), "no installer attached");
    }

    #[test]
    fn finds_utf16_boot_entry_names() {
        let mut store = vec![0xffu8; 1000];
        assert!(!contains_utf16(&store, BOOT_ENTRY));
        let entry: Vec<u8> = "Windows Boot Manager".encode_utf16().flat_map(u16::to_le_bytes).collect();
        store[300..300 + entry.len()].copy_from_slice(&entry);
        assert!(contains_utf16(&store, BOOT_ENTRY));
        assert!(!contains_utf16(b"Windows Boot Manager", BOOT_ENTRY), "plain ASCII is not a UEFI name");
        assert!(!contains_utf16(&store, ""));
    }

    #[test]
    fn missing_variable_store_means_not_registered() {
        assert!(!windows_registered(std::path::Path::new("/nonexistent/efivars.fd")));
    }
}
