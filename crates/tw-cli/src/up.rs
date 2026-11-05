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

/// What the host offers, used for defaults and limits.
#[derive(Debug, Clone, Copy)]
pub struct HostResources {
    pub cpus: u32,
    pub memory_gib: Option<u64>,
}

/// Resources and network mode of a new VM.
#[derive(Debug, Clone)]
pub struct VmSettings {
    pub name: VmName,
    pub cpus: u32,
    pub memory_gib: u64,
    pub disk_gib: u64,
    pub network: NetworkMode,
}

impl VmSettings {
    pub fn summary(&self) -> String {
        let net = match self.network {
            NetworkMode::Nat => "NAT network",
            NetworkMode::Offline => "no network",
        };
        format!("{} CPUs, {} GiB RAM, {} GiB disk (grows as used), {net}", self.cpus, self.memory_gib, self.disk_gib)
    }
}

/// Inclusive limits for each setting on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub cpus: (u32, u32),
    pub memory_gib: (u64, u64),
    pub disk_gib: (u64, u64),
}

pub const MIN_CPUS: u32 = tw_core::model::win11::MIN_CPUS;
pub const MIN_MEMORY_GIB: u64 = tw_core::model::win11::MIN_MEMORY_MIB / 1024;
pub const MIN_DISK_GIB: u64 = tw_core::model::win11::MIN_DISK_GIB;
/// Memory kept for the host when the host's RAM is known.
const HOST_RESERVE_GIB: u64 = 4;

pub fn limits(res: &HostResources) -> TwResult<Limits> {
    let max_memory = match res.memory_gib {
        Some(total) if total >= MIN_MEMORY_GIB + HOST_RESERVE_GIB => (total - HOST_RESERVE_GIB).min(128),
        Some(total) => {
            return Err(TwError::new(
                ErrorCode::Incompatible,
                format!("this computer has {total} GiB of RAM; Windows 11 needs {MIN_MEMORY_GIB} GiB for the VM plus memory for the host"),
            ))
        }
        None => 64,
    };
    Ok(Limits { cpus: (MIN_CPUS, res.cpus.max(MIN_CPUS)), memory_gib: (MIN_MEMORY_GIB, max_memory), disk_gib: (MIN_DISK_GIB, 2048) })
}

/// The automatic choice: half of the CPUs (2-8), 8 GiB of RAM on machines with 16 GiB or more
/// (otherwise the 4 GiB minimum), an 80 GiB sparse disk. Explicit command-line values win.
pub fn auto_settings(res: &HostResources, name: VmName, opts: &UpOptions) -> TwResult<VmSettings> {
    let lim = limits(res)?;
    let cpus = opts.cpus.unwrap_or_else(|| (res.cpus / 2).clamp(MIN_CPUS, 8)).clamp(lim.cpus.0, lim.cpus.1.max(opts.cpus.unwrap_or(0)));
    let memory = opts.memory_gib.unwrap_or(match res.memory_gib {
        Some(total) if total < 16 => MIN_MEMORY_GIB,
        _ => 8,
    });
    let memory_gib = memory.min(lim.memory_gib.1.max(opts.memory_gib.unwrap_or(0)));
    Ok(VmSettings { name, cpus, memory_gib, disk_gib: opts.disk_gib.unwrap_or(80), network: NetworkMode::Nat })
}

/// Best-effort host RAM in GiB.
pub fn host_memory_gib() -> Option<u64> {
    let bytes: Option<u64> = if cfg!(target_os = "macos") {
        let out = std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    } else if cfg!(target_os = "linux") {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = meminfo.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
        Some(kb * 1024)
    } else {
        None
    };
    bytes.map(|b| b >> 30)
}

pub fn host_resources() -> HostResources {
    HostResources { cpus: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(2), memory_gib: host_memory_gib() }
}

/// The "custom" questionnaire. `taken` says whether a VM name is already used.
pub fn ask_settings<R: std::io::BufRead, W: std::io::Write>(
    p: &mut super::prompt::Prompter<R, W>,
    requested_name: Option<&VmName>,
    res: &HostResources,
    defaults: &VmSettings,
    taken: &dyn Fn(&VmName) -> bool,
) -> TwResult<VmSettings> {
    let io = |e: std::io::Error| TwError::new(ErrorCode::InvalidInput, e.to_string());
    let lim = limits(res)?;
    let ram = res.memory_gib.map(|m| format!("{m} GiB RAM")).unwrap_or_else(|| "RAM unknown".into());
    p.say(&format!(
        "This computer: {} CPUs, {ram}. Windows 11 needs at least {MIN_CPUS} CPUs, {MIN_MEMORY_GIB} GiB RAM and {MIN_DISK_GIB} GiB disk.",
        res.cpus
    ))
    .map_err(io)?;
    let name = match requested_name {
        Some(n) => n.clone(),
        None => {
            let text = p
                .text("VM name", defaults.name.as_str(), |v| {
                    let parsed = VmName::parse(v).map_err(|e| e.message)?;
                    if taken(&parsed) {
                        Err(format!("a VM named '{v}' already exists"))
                    } else {
                        Ok(())
                    }
                })
                .map_err(io)?;
            VmName::parse(&text)?
        }
    };
    let cpus = p.number("CPUs", u64::from(defaults.cpus), u64::from(lim.cpus.0), u64::from(lim.cpus.1), "CPUs").map_err(io)? as u32;
    let memory_gib = p.number("Memory", defaults.memory_gib, lim.memory_gib.0, lim.memory_gib.1, "GiB").map_err(io)?;
    let disk_gib = p
        .number("Disk size (sparse: only space actually used is taken)", defaults.disk_gib, lim.disk_gib.0, lim.disk_gib.1, "GiB")
        .map_err(io)?;
    let network = match p
        .choose("Network", &["NAT: the VM uses this computer's internet connection (recommended)", "Offline: no network device"], 0)
        .map_err(io)?
    {
        1 => NetworkMode::Offline,
        _ => NetworkMode::Nat,
    };
    Ok(VmSettings { name, cpus, memory_gib, disk_gib, network })
}

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
    /// Always ask for the settings of a new VM.
    pub custom: bool,
    /// Never ask: use the automatic settings.
    pub auto: bool,
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
    let mut name = resolve_default_name(store, opts.name.clone())?;
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
    let open_viewer = !opts.no_view && !opts.detach && interactive;

    // Settings of a new VM, decided before any long step so the user can walk away afterwards.
    let resources = host_resources();
    let mut settings: Option<VmSettings> = None;
    if !exists {
        let auto = auto_settings(&resources, name.clone(), &opts)?;
        let explicit = opts.cpus.is_some() || opts.memory_gib.is_some() || opts.disk_gib.is_some();
        let may_ask = interactive && !opts.plan && !opts.auto && !explicit;
        settings = Some(if may_ask {
            let mut prompter = super::prompt::Prompter::new(std::io::stdin().lock(), std::io::stderr());
            let io = |e: std::io::Error| TwError::new(ErrorCode::InvalidInput, e.to_string());
            let custom = opts.custom || {
                prompter.say(&format!("No VM named '{name}' yet.")).map_err(io)?;
                let options =
                    [format!("Automatic (recommended): {}", auto.summary()), "Custom: choose CPUs, memory, disk and network".to_string()];
                let refs: Vec<&str> = options.iter().map(String::as_str).collect();
                prompter.choose("How should the VM be created?", &refs, 0).map_err(io)? == 1
            };
            if custom {
                let chosen = ask_settings(&mut prompter, opts.name.as_ref(), &resources, &auto, &|n| store.exists(n))?;
                name = chosen.name.clone();
                chosen
            } else {
                auto
            }
        } else {
            auto
        });
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
        let detail = settings.as_ref().map(|s| format!(": {}", s.summary())).unwrap_or_default();
        steps.push(("create-vm", format!("create VM '{name}'{detail}")));
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
        let chosen = settings.clone().ok_or_else(|| TwError::new(ErrorCode::Internal, "settings were not decided"))?;
        next(&format!("Creating VM '{name}': {}", chosen.summary()));
        let request = NewMachine {
            name: name.clone(),
            backend: BackendKind::Qemu,
            arch,
            cpus: Some(chosen.cpus),
            memory_mib: Some(chosen.memory_gib.saturating_mul(1024)),
            disk_gib: Some(chosen.disk_gib),
            display: None,
            network: chosen.network,
            install_iso: Some(iso),
            drivers_iso: None,
        };
        service.create(request, resources.cpus, super::now_rfc3339()).await?;
    }

    // 4. Boot.
    if need_start {
        next(&format!("Booting VM '{name}'"));
        service.start(&name, &StartOptions::default()).await?;
        super::bootassist::assist(service, &name).await;
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
        format!("VM '{name}' is {:?}. Watch and control it with: terminal-windows view {name}", status.state)
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(cpus: u32, mem: Option<u64>) -> HostResources {
        HostResources { cpus, memory_gib: mem }
    }

    fn opts() -> UpOptions {
        UpOptions::default()
    }

    #[test]
    fn automatic_settings_scale_with_the_host() {
        let name = || VmName::parse("default").unwrap();
        let s = auto_settings(&res(10, Some(32)), name(), &opts()).unwrap();
        assert_eq!((s.cpus, s.memory_gib, s.disk_gib), (5, 8, 80));
        let s = auto_settings(&res(16, Some(64)), name(), &opts()).unwrap();
        assert_eq!(s.cpus, 8, "capped at 8");
        let s = auto_settings(&res(4, Some(8)), name(), &opts()).unwrap();
        assert_eq!((s.cpus, s.memory_gib), (2, 4), "a small host gets the Windows 11 minimum");
        let s = auto_settings(&res(8, None), name(), &opts()).unwrap();
        assert_eq!(s.memory_gib, 8, "unknown RAM keeps the default");
        let s = auto_settings(&res(8, Some(32)), name(), &UpOptions { memory_gib: Some(12), disk_gib: Some(120), cpus: Some(6), ..opts() })
            .unwrap();
        assert_eq!((s.cpus, s.memory_gib, s.disk_gib), (6, 12, 120), "explicit values win");
        assert!(auto_settings(&res(8, Some(6)), name(), &opts()).is_err(), "6 GiB host cannot run Windows 11 plus itself");
    }

    #[test]
    fn limits_leave_memory_for_the_host() {
        let l = limits(&res(10, Some(16))).unwrap();
        assert_eq!(l, Limits { cpus: (2, 10), memory_gib: (4, 12), disk_gib: (64, 2048) });
        assert_eq!(limits(&res(1, Some(64))).unwrap().cpus, (2, 2), "never below the Windows 11 minimum");
        assert_eq!(limits(&res(8, Some(512))).unwrap().memory_gib.1, 128);
    }

    #[test]
    fn custom_questionnaire_collects_every_setting() {
        let r = res(10, Some(32));
        let defaults = auto_settings(&r, VmName::parse("default").unwrap(), &opts()).unwrap();
        // name (taken first, then fine), cpus, memory, disk, network = offline
        let script = "win11\nwork\n6\n12 GiB\n200\n2\n";
        let mut p = super::super::prompt::Prompter::new(script.as_bytes(), Vec::new());
        let taken = |n: &VmName| n.as_str() == "win11";
        let got = ask_settings(&mut p, None, &r, &defaults, &taken).unwrap();
        assert_eq!(got.name.as_str(), "work");
        assert_eq!((got.cpus, got.memory_gib, got.disk_gib), (6, 12, 200));
        assert_eq!(got.network, NetworkMode::Offline);
        assert!(got.summary().contains("no network"));
    }

    #[test]
    fn custom_questionnaire_accepts_defaults_and_a_given_name() {
        let r = res(10, Some(32));
        let given = VmName::parse("lab").unwrap();
        let defaults = auto_settings(&r, given.clone(), &opts()).unwrap();
        let mut p = super::super::prompt::Prompter::new("\n\n\n\n".as_bytes(), Vec::new());
        let got = ask_settings(&mut p, Some(&given), &r, &defaults, &|_| false).unwrap();
        assert_eq!((got.name.as_str(), got.cpus, got.memory_gib, got.disk_gib, got.network), ("lab", 5, 8, 80, NetworkMode::Nat));
    }

    #[test]
    fn custom_questionnaire_rejects_values_outside_the_limits() {
        let r = res(10, Some(16));
        let defaults = auto_settings(&r, VmName::parse("default").unwrap(), &opts()).unwrap();
        // cpus: default; memory 2 (below 4), 64 (above 12), then 6; disk 10 (below 64), then 100; network: default
        let mut p = super::super::prompt::Prompter::new("\n2\n64\n6\n10\n100\n\n".as_bytes(), Vec::new());
        let got = ask_settings(&mut p, Some(&VmName::parse("d").unwrap()), &r, &defaults, &|_| false).unwrap();
        assert_eq!((got.memory_gib, got.disk_gib), (6, 100));
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
