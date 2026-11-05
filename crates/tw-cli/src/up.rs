//! `terminal-windows up` (also what a bare `terminal-windows` does in a terminal):
//! check prerequisites, get firmware and a verified ISO, create the VM, boot it
//! and open the viewer. Every step is skipped when it is already done, so running
//! it again simply reconnects, like terminal-mac's guided flow.

use super::{confirm, firmware_cmd, media, FirmwareCommand, Output};
use serde_json::json;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use tw_core::backend::{StartOptions, VirtualizationBackend, VmState};
use tw_core::host::{Arch, HostInfo};
use tw_core::model::{BackendKind, NetworkMode, NewMachine};
use tw_core::service::VmService;
use tw_core::store::Store;
use tw_core::{ErrorCode, TwError, TwResult, VmName};
use tw_media::cache;
use tw_qemu::{detect, firmware};

#[derive(Debug, Default, Clone)]
pub struct UpOptions {
    pub name: Option<VmName>,
    pub iso: Option<PathBuf>,
    pub language: Option<String>,
    pub no_view: bool,
    pub detach: bool,
    pub plan: bool,
    pub yes: bool,
    pub cpus: Option<u32>,
    pub memory_gib: Option<u64>,
    pub disk_gib: Option<u64>,
}

/// Language for the Windows image, from the system locale (Microsoft's spelling).
pub fn language_for_locale(locale: &str) -> &'static str {
    let l = locale.to_ascii_lowercase();
    let prefix = l.split(['_', '-', '.']).next().unwrap_or("");
    match prefix {
        "ko" => "Korean",
        "ja" => "Japanese",
        "de" => "German",
        "fr" => "French",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" if l.starts_with("pt_br") || l.starts_with("pt-br") => "Brazilian Portuguese",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "nl" => "Dutch",
        "pl" => "Polish",
        "tr" => "Turkish",
        "sv" => "Swedish",
        "zh" if l.contains("tw") || l.contains("hant") || l.contains("hk") => "Chinese Traditional",
        "zh" => "Chinese Simplified",
        _ => "English",
    }
}

fn system_locale() -> String {
    for var in ["TW_LANGUAGE_LOCALE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() && v != "C" && v != "POSIX" {
                return v;
            }
        }
    }
    if cfg!(target_os = "macos") {
        if let Ok(out) = std::process::Command::new("defaults").args(["read", "-g", "AppleLocale"]).output() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    String::new()
}

/// The VM an unnamed command refers to: "default" if it exists, else the only VM, else "default".
pub fn resolve_default_name(store: &Store, requested: Option<VmName>) -> TwResult<VmName> {
    if let Some(name) = requested {
        return Ok(name);
    }
    let default = VmName::parse("default")?;
    if store.exists(&default) {
        return Ok(default);
    }
    let existing: Vec<VmName> = store.list()?.into_iter().filter_map(|r| r.ok()).map(|c| c.name).collect();
    Ok(if existing.len() == 1 { existing[0].clone() } else { default })
}

fn has_secure_firmware(store: &Store, install: Option<&detect::QemuInstall>, arch: Arch) -> bool {
    let dirs = firmware::descriptor_dirs(install.and_then(|i| i.prefix.as_deref()), Some(&store.root().join("firmware")));
    firmware::discover(&dirs, arch).iter().any(|f| f.secure_boot)
}

fn brew_available() -> bool {
    std::process::Command::new("brew").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn step(n: usize, total: usize, text: &str) {
    eprintln!("[{n}/{total}] {text}");
}

pub async fn run(
    opts: UpOptions,
    store: &Store,
    service: &VmService,
    backend: &Arc<dyn VirtualizationBackend>,
    host: HostInfo,
    out: &Output,
) -> TwResult<()> {
    let arch = host.native_guest_arch()?;
    let name = resolve_default_name(store, opts.name.clone())?;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    // What is missing right now (no side effects).
    let mut install = detect::find_qemu(&host, arch).await.ok();
    let swtpm = detect::find_swtpm(&host);
    let need_qemu = install.is_none();
    let need_swtpm = swtpm.is_none() && host.os != tw_core::host::HostOs::Windows;
    let need_firmware = host.arch == Some(Arch::Aarch64) && !has_secure_firmware(store, install.as_ref(), arch);
    let exists = store.exists(&name);
    let language = opts.language.clone().unwrap_or_else(|| language_for_locale(&system_locale()).to_string());
    let need_image = !exists && opts.iso.is_none() && cache::find(&cache::images_dir(store.root()), arch, None).is_none();
    let state = if exists { Some(service.status(&name).await?.state) } else { None };
    let need_start = state != Some(VmState::Running) && state != Some(VmState::Paused);
    let mut open_viewer = !opts.no_view && !opts.detach && interactive;
    if open_viewer {
        if let Some(terminal) = super::view::known_unsupported_terminal() {
            // Still do the whole setup; only the final viewer step is skipped.
            eprintln!("note: {terminal} cannot show the viewer, so setup will stop before opening it.");
            open_viewer = false;
        }
    }

    let mut steps: Vec<(&str, String)> = Vec::new();
    if need_qemu || need_swtpm {
        steps.push(("install-prerequisites", "install QEMU and swtpm with Homebrew".into()));
    }
    if need_firmware {
        steps.push(("install-firmware", "install the Secure Boot UEFI firmware (release download, SHA-256 verified)".into()));
    }
    if need_image {
        steps.push(("get-windows-image", format!("download Windows 11 ({language}) from Microsoft and verify its SHA-256")));
    }
    if !exists {
        steps.push(("create-vm", format!("create VM '{name}'")));
    }
    if need_start {
        steps.push(("start-vm", format!("boot VM '{name}'")));
    }
    if open_viewer {
        steps.push(("open-viewer", "open the terminal viewer (Ctrl+] leaves it, the VM keeps running)".into()));
    }

    if opts.plan {
        let list: Vec<_> = steps.iter().map(|(id, text)| json!({ "step": id, "description": text })).collect();
        out.emit(json!({ "vm": name, "steps": list }), |_| {
            if steps.is_empty() {
                format!("Nothing to do: VM '{name}' is already running.")
            } else {
                steps.iter().enumerate().map(|(i, (_, t))| format!("{}. {t}", i + 1)).collect::<Vec<_>>().join("\n")
            }
        });
        return Ok(());
    }

    let total = steps.len().max(1);
    let mut n = 0;
    let mut next = |text: &str| {
        n += 1;
        step(n, total, text);
    };

    // 1. Prerequisites.
    if need_qemu || need_swtpm {
        let mut formulae = Vec::new();
        if need_qemu {
            formulae.push("qemu");
        }
        if need_swtpm {
            formulae.push("swtpm");
        }
        next(&format!("Installing {} with Homebrew", formulae.join(" and ")));
        let command = format!("brew install {}", formulae.join(" "));
        if host.os != tw_core::host::HostOs::Macos || !brew_available() {
            return Err(TwError::new(ErrorCode::MissingDependency, format!("missing: {}", formulae.join(", ")))
                .with_hint(format!("Install them with your package manager (macOS with Homebrew: {command})")));
        }
        if !opts.yes && !(interactive && confirm(&format!("Run `{command}`?"))?) {
            return Err(TwError::new(ErrorCode::MissingDependency, format!("missing: {}", formulae.join(", ")))
                .with_hint(format!("Run `{command}` yourself, or pass --yes to let terminal-windows do it")));
        }
        let status = std::process::Command::new("brew").arg("install").args(&formulae).status().map_err(|e| TwError::io("brew", e))?;
        if !status.success() {
            return Err(TwError::new(ErrorCode::MissingDependency, format!("`{command}` failed")));
        }
        install = detect::find_qemu(&host, arch).await.ok();
        if install.is_none() {
            return Err(TwError::new(ErrorCode::MissingDependency, "QEMU is still not found after installing it"));
        }
    }

    // 2. Firmware.
    if need_firmware {
        next("Installing the Secure Boot UEFI firmware");
        firmware_cmd::run(FirmwareCommand::Install { from: None }, store, host, out).await?;
        if !has_secure_firmware(store, install.as_ref(), arch) {
            return Err(TwError::backend("the firmware was installed but is not usable").with_hint("Run: terminal-windows firmware list"));
        }
    }

    // 3. Windows image and VM.
    let mut created = false;
    if !exists {
        let iso = match opts.iso.clone() {
            Some(p) => std::path::absolute(&p).unwrap_or(p),
            None => {
                let dir = cache::images_dir(store.root());
                if let Some(c) = cache::find(&dir, arch, None) {
                    c.path
                } else {
                    next(&format!("Getting Windows 11 ({language}) from Microsoft"));
                    media::download_image(store, arch, &language, false, 180).await?.0.path
                }
            }
        };
        next(&format!("Creating VM '{name}'"));
        let request = NewMachine {
            name: name.clone(),
            backend: BackendKind::Qemu,
            arch,
            cpus: opts.cpus,
            memory_mib: opts.memory_gib.map(|g| g.saturating_mul(1024)),
            disk_gib: opts.disk_gib,
            display: None,
            network: NetworkMode::Nat,
            install_iso: Some(iso),
            drivers_iso: None,
        };
        let host_cpus = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(2);
        service.create(request, host_cpus, super::now_rfc3339()).await?;
        created = true;
    }

    // 4. Boot.
    if need_start {
        next(&format!("Booting VM '{name}'"));
        service.start(&name, &StartOptions::default()).await?;
        if created {
            // A new VM boots the Windows ISO, which shows "Press any key to boot from CD or DVD"
            // for about three seconds. Only a brand-new VM gets this: pressing a key on a VM that
            // has already been installed would restart Setup instead of booting Windows.
            eprintln!("      waiting for the \"boot from CD\" prompt ...");
            match press_key_at_boot_prompt(service, &name).await {
                Ok(true) => eprintln!("      pressed a key; Windows Setup is starting"),
                Ok(false) => eprintln!("      no boot prompt appeared; open the viewer and press a key if the VM shows \"Press any key\""),
                Err(e) => eprintln!("      warning: could not watch for the boot prompt: {}", e.message),
            }
        }
    }

    // 5. View.
    let status = backend.status(&service.load(&name)?, &store.paths(&name)).await?;
    if open_viewer {
        next("Opening the viewer");
        return match super::view::run(service, &name).await {
            // The VM is up; a terminal that cannot draw the viewer is not a failure of `up`.
            Err(e) if e.code == ErrorCode::Unsupported => {
                eprintln!("VM '{name}' is {:?}.\n{e}", status.state);
                Ok(())
            }
            other => other,
        };
    }
    out.emit(json!({ "vm": name, "state": status.state, "next": format!("terminal-windows view {name}") }), |_| {
        let mut text = format!("VM '{name}' is {:?}. Watch and control it with: terminal-windows view {name}", status.state);
        if let Some(terminal) = super::view::known_unsupported_terminal() {
            text.push_str(&format!("\n{}", super::view::unsupported_terminal_error(terminal)));
        }
        text
    });
    Ok(())
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
async fn press_key_at_boot_prompt(service: &VmService, name: &VmName) -> TwResult<bool> {
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
    fn locale_to_microsoft_language() {
        assert_eq!(language_for_locale("ko_KR.UTF-8"), "Korean");
        assert_eq!(language_for_locale("ja_JP"), "Japanese");
        assert_eq!(language_for_locale("pt_BR.UTF-8"), "Brazilian Portuguese");
        assert_eq!(language_for_locale("pt_PT"), "Portuguese");
        assert_eq!(language_for_locale("zh_TW"), "Chinese Traditional");
        assert_eq!(language_for_locale("zh_CN"), "Chinese Simplified");
        assert_eq!(language_for_locale("en_US.UTF-8"), "English");
        assert_eq!(language_for_locale(""), "English");
        assert_eq!(language_for_locale("xx_YY"), "English");
    }

    #[test]
    fn every_mapped_language_is_one_microsoft_publishes() {
        // The spellings used in the table must exist in Microsoft's published list (measured 2026-11-05).
        let published = [
            "Korean",
            "Japanese",
            "German",
            "French",
            "Spanish",
            "Italian",
            "Brazilian Portuguese",
            "Portuguese",
            "Russian",
            "Dutch",
            "Polish",
            "Turkish",
            "Swedish",
            "Chinese Traditional",
            "Chinese Simplified",
            "English",
        ];
        for locale in ["ko", "ja", "de", "fr", "es", "it", "pt_BR", "pt", "ru", "nl", "pl", "tr", "sv", "zh_TW", "zh_CN", "en"] {
            assert!(published.contains(&language_for_locale(locale)), "{locale}");
        }
    }

    #[test]
    fn default_name_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        let make = |n: &str| {
            let name = VmName::parse(n).unwrap();
            store.create_dir(&name).unwrap();
            let cfg = tw_core::model::MachineConfig::new_windows11(
                NewMachine {
                    name: name.clone(),
                    backend: BackendKind::Qemu,
                    arch: Arch::Aarch64,
                    cpus: None,
                    memory_mib: None,
                    disk_gib: None,
                    display: None,
                    network: NetworkMode::Nat,
                    install_iso: None,
                    drivers_iso: None,
                },
                8,
                "t".into(),
            )
            .unwrap();
            store.save(&cfg).unwrap();
        };
        assert_eq!(resolve_default_name(&store, None).unwrap().as_str(), "default", "no VMs");
        make("win11");
        assert_eq!(resolve_default_name(&store, None).unwrap().as_str(), "win11", "the only VM");
        make("other");
        assert_eq!(resolve_default_name(&store, None).unwrap().as_str(), "default", "ambiguous: a new default");
        make("default");
        assert_eq!(resolve_default_name(&store, None).unwrap().as_str(), "default");
        assert_eq!(resolve_default_name(&store, Some(VmName::parse("other").unwrap())).unwrap().as_str(), "other", "explicit wins");
    }
}
