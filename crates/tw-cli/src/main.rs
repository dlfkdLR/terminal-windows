//! terminal-windows CLI. All lifecycle work goes through `tw_core::service::VmService`.

use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tw_core::backend::{StartOptions, StopMode, VirtualizationBackend};
use tw_core::control::MouseButton;
use tw_core::host::{Arch, HostInfo};
use tw_core::model::{BackendKind, DisplayConfig, NetworkMode, NewMachine};
use tw_core::service::VmService;
use tw_core::store::Store;
use tw_core::{ErrorCode, TwError, TwResult, VmName};

mod firmware_cmd;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod geometry;
mod media;
mod prompt;
mod up;
mod view;
mod window;
mod window_ctl;

#[derive(Parser, Debug)]
#[command(name = "terminal-windows", version, about = "Run and control Windows virtual machines from a terminal")]
struct Cli {
    /// Print machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    json: bool,
    /// Log verbosity for diagnostics on stderr (error, warn, info, debug, trace).
    #[arg(long, global = true, default_value = "warn")]
    log: String,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Check the host: QEMU, accelerator, UEFI firmware, TPM, terminal.
    Doctor,
    /// Installation media helpers.
    Image {
        #[command(subcommand)]
        command: ImageCommand,
    },
    /// Create a Windows 11 VM from legally obtained installation media.
    Create {
        name: VmName,
        /// Windows installation ISO (x64 on Intel/AMD hosts, ARM64 on Apple Silicon).
        /// Defaults to the newest verified image in the cache (`terminal-windows image download`).
        #[arg(long)]
        iso: Option<PathBuf>,
        /// Optional driver ISO (e.g. virtio-win, needed for networking on ARM64).
        #[arg(long)]
        drivers_iso: Option<PathBuf>,
        /// Guest architecture. Defaults to the host's; cross-architecture emulation is refused.
        #[arg(long, value_parser = parse_arch)]
        arch: Option<Arch>,
        #[arg(long)]
        cpus: Option<u32>,
        /// Memory in GiB.
        #[arg(long)]
        memory: Option<u64>,
        /// Disk size in GiB (sparse qcow2).
        #[arg(long)]
        disk: Option<u64>,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long)]
        height: Option<u32>,
        /// No guest network device.
        #[arg(long)]
        offline: bool,
    },
    /// Boot a VM (headless; view it with `view` or control it with `action`).
    Start {
        #[arg(default_value = "default")]
        name: VmName,
    },
    /// Request a graceful shutdown, or power off with --force.
    Stop {
        #[arg(default_value = "default")]
        name: VmName,
        /// Immediate power-off. Unsaved guest work can be lost.
        #[arg(long)]
        force: bool,
    },
    Pause {
        #[arg(default_value = "default")]
        name: VmName,
    },
    Resume {
        #[arg(default_value = "default")]
        name: VmName,
    },
    Status {
        #[arg(default_value = "default")]
        name: VmName,
    },
    /// List VMs.
    #[command(alias = "list")]
    Ls,
    /// View and control the desktop. Inside the terminal with Kitty graphics (Ghostty, Kitty, WezTerm);
    /// any other terminal gets the native window instead.
    View {
        #[arg(default_value = "default")]
        name: VmName,
        /// Always use the native window, even in a terminal that can show graphics.
        #[arg(long)]
        window: bool,
    },
    /// Open the native viewer window (macOS and Windows). Works in any terminal.
    Show {
        #[arg(default_value = "default")]
        name: VmName,
    },
    /// Close the native viewer window. The VM keeps running.
    Hide {
        #[arg(default_value = "default")]
        name: VmName,
    },
    /// Delete a stopped VM and all its data.
    Delete {
        name: VmName,
        /// Do not ask for confirmation.
        #[arg(long)]
        force: bool,
    },
    /// Print the end of the hypervisor log.
    Logs {
        #[arg(default_value = "default")]
        name: VmName,
        #[arg(long, default_value_t = 100)]
        lines: usize,
    },
    /// Show a VM's configuration, or change it while stopped.
    Config {
        #[arg(default_value = "default")]
        name: VmName,
        /// Detach the installation and driver ISOs (after Windows is installed).
        #[arg(long)]
        detach_media: bool,
    },
    /// Secure Boot capable UEFI firmware (required by Windows 11 on AArch64).
    Firmware {
        #[command(subcommand)]
        command: FirmwareCommand,
    },
    /// Internal: the native viewer window process started by `show`.
    #[command(hide = true)]
    Window { name: VmName },
    /// Run the MCP server on stdio for agents (Claude Code, Codex, Cursor, ...).
    Mcp {
        #[arg(default_value = "default")]
        name: VmName,
        /// Print a JSON snippet for MCP client configuration instead of serving.
        #[arg(long)]
        config: bool,
    },
    /// Guided setup: check prerequisites, get firmware and Windows, create, boot and view.
    /// Running `terminal-windows` with no command in a terminal does the same.
    Up {
        /// VM name. Defaults to "default", or to the only VM if there is just one.
        name: Option<VmName>,
        /// Use this Windows ISO instead of downloading one from Microsoft.
        #[arg(long)]
        iso: Option<PathBuf>,
        /// Windows language to download (Microsoft's names, e.g. Korean). Default: from the system locale.
        #[arg(long)]
        language: Option<String>,
        /// Create and boot the VM, then print its state instead of opening the viewer.
        #[arg(long)]
        no_view: bool,
        /// Same as --no-view.
        #[arg(long)]
        detach: bool,
        /// Show the steps that would be taken, without changing anything.
        #[arg(long)]
        plan: bool,
        /// Do not ask before installing prerequisites with Homebrew.
        #[arg(long, short = 'y')]
        yes: bool,
        #[arg(long)]
        cpus: Option<u32>,
        /// Memory in GiB for a new VM.
        #[arg(long)]
        memory: Option<u64>,
        /// Disk size in GiB for a new VM.
        #[arg(long)]
        disk: Option<u64>,
        /// Ask for the settings of a new VM (CPUs, memory, disk, network).
        #[arg(long, conflicts_with = "auto")]
        custom: bool,
        /// Use the automatic settings without asking.
        #[arg(long)]
        auto: bool,
    },
    /// Computer-use actions on a running VM.
    Action {
        name: VmName,
        #[command(subcommand)]
        action: Action,
    },
}

#[derive(Subcommand, Debug)]
enum FirmwareCommand {
    /// Install the firmware published with the latest release (SHA-256 verified).
    Install {
        /// Install a local build (a directory with code.fd and vars.fd, see firmware/build.sh).
        #[arg(long)]
        from: Option<PathBuf>,
    },
    /// List the UEFI firmware QEMU can use for this host.
    List,
}

#[derive(Subcommand, Debug)]
enum ImageCommand {
    /// List cached, verified Windows images.
    List,
    /// Show the current official release and its published SHA-256 (no download).
    Info {
        #[arg(long, value_parser = parse_arch)]
        arch: Option<Arch>,
        #[arg(long, default_value = "English")]
        language: String,
    },
    /// Download Windows 11 from Microsoft and verify it against the published SHA-256.
    Download {
        #[arg(long, value_parser = parse_arch)]
        arch: Option<Arch>,
        /// Language as listed by Microsoft, e.g. English, Korean, Japanese.
        #[arg(long, default_value = "English")]
        language: String,
        /// Do not open the browser if Microsoft refuses an automated download.
        #[arg(long)]
        no_browser: bool,
        /// How long to wait for a browser download to finish.
        #[arg(long, default_value_t = 180)]
        wait_minutes: u64,
    },
    /// Verify an ISO you downloaded from Microsoft and add it to the cache.
    Import {
        path: PathBuf,
        #[arg(long, value_parser = parse_arch)]
        arch: Option<Arch>,
        /// Move the file into the cache instead of copying it.
        #[arg(long = "move")]
        move_file: bool,
    },
}

#[derive(Subcommand, Debug)]
enum Action {
    /// Save a PNG of the guest screen and print its path.
    Screenshot {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Click {
        x: u32,
        y: u32,
        #[arg(long, default_value = "left", value_parser = parse_button)]
        button: MouseButton,
        #[arg(long, default_value_t = 1)]
        clicks: u8,
    },
    Move {
        x: u32,
        y: u32,
    },
    Drag {
        x: u32,
        y: u32,
        to_x: u32,
        to_y: u32,
    },
    /// Scroll. 120 units = one wheel notch; negative dy scrolls content down.
    Scroll {
        #[arg(long, allow_negative_numbers = true, default_value_t = 0)]
        dy: i32,
        #[arg(long, allow_negative_numbers = true, default_value_t = 0)]
        dx: i32,
        /// Pointer position; defaults to the screen centre.
        #[arg(long)]
        x: Option<u32>,
        #[arg(long)]
        y: Option<u32>,
    },
    Type {
        text: String,
    },
    /// Press a key chord, e.g. ctrl+shift+esc, win+r, return.
    Key {
        chord: String,
    },
    Status,
}

fn parse_arch(s: &str) -> Result<Arch, String> {
    match s.to_ascii_lowercase().as_str() {
        "x86_64" | "x64" | "amd64" => Ok(Arch::X86_64),
        "aarch64" | "arm64" => Ok(Arch::Aarch64),
        _ => Err("expected x86_64 (x64) or aarch64 (arm64)".into()),
    }
}

fn parse_button(s: &str) -> Result<MouseButton, String> {
    match s {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        _ => Err("expected left, right or middle".into()),
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

struct Output {
    json: bool,
}

impl Output {
    fn emit(&self, value: Value, human: impl FnOnce(&Value) -> String) {
        if self.json {
            println!("{}", serde_json::to_string_pretty(&json!({ "ok": true, "result": value })).unwrap_or_default());
        } else {
            println!("{}", human(&value));
        }
    }
}

fn kv(value: &Value) -> String {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{k}: {}", v.as_str().map(String::from).unwrap_or_else(|| v.to_string())))
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

fn confirm(prompt: &str) -> TwResult<bool> {
    if !std::io::stdin().is_terminal() {
        return Err(
            TwError::invalid("refusing to delete without confirmation in a non-interactive session").with_hint("Pass --force to confirm.")
        );
    }
    eprint!("{prompt} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(|e| TwError::io("stdin", e))?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

async fn run(cli: Cli, out: &Output) -> TwResult<()> {
    let host = HostInfo::current();
    let store = Store::from_env()?;
    let backend: Arc<dyn VirtualizationBackend> = Arc::new(tw_qemu::QemuBackend::new(host, store.root().join("firmware")));
    let service = VmService::new(store.clone(), backend.clone());

    let command = match cli.command {
        Some(command) => command,
        // `terminal-windows` alone: guided setup in a terminal, help otherwise (like terminal-mac).
        None if std::io::stdin().is_terminal() => Command::Up {
            name: None,
            iso: None,
            language: None,
            no_view: false,
            detach: false,
            plan: false,
            yes: false,
            cpus: None,
            memory: None,
            disk: None,
            custom: false,
            auto: false,
        },
        None => {
            use clap::CommandFactory;
            let _ = Cli::command().print_help();
            return Ok(());
        }
    };
    let mut command = command;
    // Commands that take a VM name default to "default", or to the only VM that exists.
    if let Command::Start { name }
    | Command::Stop { name, .. }
    | Command::Pause { name }
    | Command::Resume { name }
    | Command::Status { name }
    | Command::View { name, .. }
    | Command::Show { name }
    | Command::Hide { name }
    | Command::Window { name }
    | Command::Logs { name, .. }
    | Command::Config { name, .. }
    | Command::Mcp { name, .. } = &mut command
    {
        if name.as_str() == "default" {
            *name = up::resolve_default_name(&store, None)?;
        }
    }

    match command {
        Command::Doctor => {
            let caps = backend.capabilities().await;
            let qemu = match host.arch {
                Some(a) => tw_qemu::detect::find_qemu(&host, a).await.map(|q| json!(q)).unwrap_or_else(|e| json!({ "error": e })),
                None => json!({ "error": "unsupported host architecture" }),
            };
            // vars_os: a non-UTF-8 variable must not crash diagnostics.
            let env: std::collections::HashMap<String, String> =
                std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
            let value = json!({
                "version": env!("CARGO_PKG_VERSION"),
                "host": host,
                "native_guest_arch": host.native_guest_arch().ok(),
                "data_dir": store.root(),
                "qemu": qemu,
                "swtpm": tw_qemu::detect::find_swtpm(&host),
                "capabilities": caps,
                "terminal": {
                    "emulator_hint": tw_renderer::detect::emulator_hint(&env),
                    "inside_multiplexer": tw_renderer::detect::inside_multiplexer(&env),
                    "note": "inline graphics are decided by a live query when `view` starts, not by environment variables"
                }
            });
            out.emit(value, |v| serde_json::to_string_pretty(v).unwrap_or_default());
        }
        Command::Firmware { command } => firmware_cmd::run(command, &store, host, out).await?,
        Command::Image { command } => media::run(command, &store, host, out).await?,
        Command::Create { name, iso, drivers_iso, arch, cpus, memory, disk, width, height, offline } => {
            let arch = match arch {
                Some(a) => a,
                None => host.native_guest_arch()?,
            };
            host.check_guest_arch(arch)?;
            let display = match (width, height) {
                (Some(width), Some(height)) => Some(DisplayConfig { width, height }),
                (None, None) => None,
                _ => return Err(TwError::invalid("--width and --height must be given together")),
            };
            let abs = |p: PathBuf| std::path::absolute(&p).unwrap_or(p);
            let request = NewMachine {
                name,
                backend: BackendKind::Qemu,
                arch,
                cpus,
                memory_mib: memory.map(|g| g.saturating_mul(1024)),
                disk_gib: disk,
                display,
                network: if offline { NetworkMode::Offline } else { NetworkMode::Nat },
                install_iso: Some(match iso {
                    Some(p) => abs(p),
                    None => {
                        let cached = tw_media::cache::find(&tw_media::cache::images_dir(store.root()), arch, None).ok_or_else(|| {
                            TwError::not_found(format!("no verified Windows 11 {} image in the cache", arch.windows_name()))
                                .with_hint("terminal-windows image download   (or pass --iso /path/to/Win11.iso)")
                        })?;
                        eprintln!(
                            "Using cached image {} ({}, {})",
                            cached.path.display(),
                            cached.record.language,
                            cached.record.version.as_deref().unwrap_or("?")
                        );
                        cached.path
                    }
                }),
                drivers_iso: drivers_iso.map(abs),
            };
            let host_cpus = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(2);
            let config = service.create(request, host_cpus, now_rfc3339()).await?;
            out.emit(serde_json::to_value(&config).unwrap_or_default(), |_| {
                format!(
                    "Created VM '{}' ({} CPUs, {} MiB, {} GiB disk). Start it with: terminal-windows start {}",
                    config.name, config.cpus, config.memory_mib, config.disk_gib, config.name
                )
            });
        }
        Command::Start { name } => {
            let status = service.start(&name, &StartOptions::default()).await?;
            out.emit(serde_json::to_value(&status).unwrap_or_default(), kv);
        }
        Command::Stop { name, force } => {
            let status = service.stop(&name, if force { StopMode::Force } else { StopMode::Graceful }).await?;
            out.emit(serde_json::to_value(&status).unwrap_or_default(), kv);
        }
        Command::Pause { name } => out.emit(serde_json::to_value(service.pause(&name).await?).unwrap_or_default(), kv),
        Command::Resume { name } => out.emit(serde_json::to_value(service.resume(&name).await?).unwrap_or_default(), kv),
        Command::Status { name } => out.emit(serde_json::to_value(service.status(&name).await?).unwrap_or_default(), kv),
        Command::Ls => {
            let mut rows = Vec::new();
            for entry in store.list()? {
                match entry {
                    Ok(config) => {
                        let status = backend.status(&config, &store.paths(&config.name)).await;
                        rows.push(json!({ "name": config.name, "arch": config.arch, "state": status.as_ref().map(|s| json!(s.state)).unwrap_or(Value::Null),
                                         "error": status.err(), "cpus": config.cpus, "memory_mib": config.memory_mib }));
                    }
                    Err((name, e)) => rows.push(json!({ "name": name, "error": e })),
                }
            }
            out.emit(json!(rows), |v| {
                let rows = v.as_array().cloned().unwrap_or_default();
                if rows.is_empty() {
                    return "No VMs. Create one with: terminal-windows create NAME --iso /path/to/Win11.iso".into();
                }
                rows.iter()
                    .map(|r| {
                        format!(
                            "{:<20} {:<10} {:<8} {}",
                            r["name"].as_str().unwrap_or("?"),
                            r["state"].as_str().unwrap_or("error"),
                            r["arch"].as_str().unwrap_or(""),
                            r["error"]["message"].as_str().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        Command::View { name, window } => {
            if window {
                view::open_window(&service, &name, out).await?
            } else {
                view::run(&service, &name).await?
            }
        }
        Command::Show { name } => view::open_window(&service, &name, out).await?,
        Command::Hide { name } => {
            let closed = window_ctl::hide(&service, &name)?;
            out.emit(json!({ "name": name, "closed": closed }), |_| {
                if closed {
                    format!("Closed the viewer window of '{name}'.")
                } else {
                    format!("No viewer window is open for '{name}'.")
                }
            });
        }
        Command::Window { name } => window_ctl::run_window(&service, &name).await?,
        Command::Up { name, iso, language, no_view, detach, plan, yes, cpus, memory, disk, custom, auto } => {
            let options =
                up::UpOptions { name, iso, language, no_view, detach, plan, yes, cpus, memory_gib: memory, disk_gib: disk, custom, auto };
            up::run(options, &store, &service, &backend, host, out).await?
        }
        Command::Delete { name, force } => {
            if !store.paths(&name).dir.exists() {
                return Err(TwError::not_found(format!("VM '{name}' does not exist")));
            }
            if !force && !confirm(&format!("Delete VM '{name}' and its disk permanently?"))? {
                return Err(TwError::new(ErrorCode::InvalidInput, "deletion cancelled"));
            }
            service.delete(&name).await?;
            out.emit(json!({ "name": name, "deleted": true }), |_| format!("Deleted VM '{name}'."));
        }
        Command::Logs { name, lines } => {
            store.load(&name)?;
            let log = store.paths(&name).log;
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let tail: Vec<&str> = text.lines().rev().take(lines).collect::<Vec<_>>().into_iter().rev().collect();
            out.emit(json!({ "path": log, "lines": tail }), |v| {
                v["lines"].as_array().map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default()
            });
        }
        Command::Config { name, detach_media } => {
            let mut config = store.load(&name)?;
            if detach_media {
                let _lock = store.lock(&name)?;
                if backend.status(&config, &store.paths(&name)).await?.state != tw_core::backend::VmState::Stopped {
                    return Err(TwError::state("stop the VM before changing its configuration"));
                }
                config.install_iso = None;
                config.drivers_iso = None;
                store.save(&config)?;
            }
            out.emit(serde_json::to_value(&config).unwrap_or_default(), |_| config.to_toml().unwrap_or_default());
        }
        Command::Mcp { name, config } => {
            if config {
                let exe = std::env::current_exe().map_err(|e| TwError::io("current executable", e))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({ "mcpServers": { "terminal-windows": { "command": exe, "args": ["mcp", name.as_str()] } } })
                    )
                    .unwrap_or_default()
                );
                return Ok(());
            }
            let server = tw_mcp::Server::new(service, name, env!("CARGO_PKG_VERSION"));
            tw_mcp::serve(&server, tokio::io::BufReader::new(tokio::io::stdin()), tokio::io::stdout())
                .await
                .map_err(|e| TwError::io("MCP stdio", e))?;
        }
        Command::Action { name, action } => {
            // QEMU's QMP socket serves one client at a time, so read the VM
            // status before the console connection is opened and held.
            let vm_status = if matches!(action, Action::Status) { Some(service.status(&name).await?) } else { None };
            let control = service.console(&name).await?;
            let value = match action {
                Action::Screenshot { output } => {
                    let shot = control.screenshot().await?;
                    let path = match output {
                        Some(p) => p,
                        None => {
                            let dir = store.root().join("screenshots");
                            tw_core::secure::create_private_dir(&dir)?;
                            dir.join(format!("{name}-{}.png", time::OffsetDateTime::now_utc().unix_timestamp_nanos()))
                        }
                    };
                    std::fs::write(&path, &shot.png).map_err(|e| TwError::io(path.display(), e))?;
                    json!({ "path": path, "width": shot.width, "height": shot.height, "source": shot.source })
                }
                Action::Click { x, y, button, clicks } => {
                    control.click(x, y, button, clicks).await?;
                    json!({ "done": "click", "x": x, "y": y })
                }
                Action::Move { x, y } => {
                    control.move_pointer(x, y).await?;
                    json!({ "done": "move", "x": x, "y": y })
                }
                Action::Drag { x, y, to_x, to_y } => {
                    control.drag((x, y), (to_x, to_y)).await?;
                    json!({ "done": "drag" })
                }
                Action::Scroll { dy, dx, x, y } => {
                    let (w, h) = control.display_size().await?;
                    control.scroll(x.unwrap_or(w / 2), y.unwrap_or(h / 2), dx, dy).await?;
                    json!({ "done": "scroll", "dx": dx, "dy": dy })
                }
                Action::Type { text } => {
                    control.type_text(&text).await?;
                    json!({ "done": "type", "characters": text.chars().count() })
                }
                Action::Key { chord } => {
                    control.key(&chord).await?;
                    json!({ "done": "key", "chord": chord })
                }
                Action::Status => {
                    let (w, h) = control.display_size().await?;
                    json!({ "vm": vm_status, "plane": control.plane(), "display": { "width": w, "height": h },
                            "guest_agent": "not connected (Plane G is not implemented on the host yet)" })
                }
            };
            out.emit(value, kv);
        }
    }
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let filter = tracing_subscriber::EnvFilter::try_new(&cli.log).unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
    let out = Output { json: cli.json };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start async runtime: {e}");
            std::process::exit(70);
        }
    };
    if let Err(err) = runtime.block_on(run(cli, &out)) {
        if out.json {
            println!("{}", serde_json::to_string_pretty(&json!({ "ok": false, "error": err })).unwrap_or_default());
        } else {
            eprintln!("error: {err}");
        }
        std::process::exit(err.exit_code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_spec_commands() {
        for args in [
            vec!["terminal-windows"],
            vec!["terminal-windows", "up"],
            vec!["terminal-windows", "up", "--plan", "--yes", "--language", "Korean", "work"],
            vec!["terminal-windows", "up", "--custom"],
            vec!["terminal-windows", "up", "--auto", "--memory", "12", "--disk", "120", "--cpus", "6"],
            vec!["terminal-windows", "start"],
            vec!["terminal-windows", "view"],
            vec!["terminal-windows", "doctor"],
            vec!["terminal-windows", "image", "list"],
            vec!["terminal-windows", "firmware", "install", "--from", "/tmp/fw"],
            vec!["terminal-windows", "firmware", "list"],
            vec!["terminal-windows", "image", "download", "--language", "Korean"],
            vec!["terminal-windows", "image", "import", "/tmp/Win11.iso", "--move"],
            vec!["terminal-windows", "create", "dev"],
            vec!["terminal-windows", "create", "dev", "--iso", "./windows.iso"],
            vec!["terminal-windows", "stop", "dev", "--force"],
            vec!["terminal-windows", "--json", "ls"],
            vec!["terminal-windows", "mcp", "dev"],
            vec!["terminal-windows", "action", "dev", "screenshot"],
            vec!["terminal-windows", "action", "dev", "click", "400", "300"],
            vec!["terminal-windows", "action", "dev", "drag", "100", "100", "500", "500"],
            vec!["terminal-windows", "action", "dev", "scroll", "--dy", "-300"],
            vec!["terminal-windows", "action", "dev", "type", "Hello Windows"],
            vec!["terminal-windows", "action", "dev", "key", "ctrl+shift+esc"],
            vec!["terminal-windows", "delete", "dev", "--force"],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
    }

    #[test]
    fn custom_and_auto_conflict() {
        assert!(Cli::try_parse_from(["terminal-windows", "up", "--custom", "--auto"]).is_err());
    }

    #[test]
    fn rejects_bad_names_and_arch() {
        assert!(Cli::try_parse_from(["terminal-windows", "start", "../etc"]).is_err());
        assert!(Cli::try_parse_from(["terminal-windows", "create", "dev", "--iso", "x.iso", "--arch", "riscv"]).is_err());
        assert_eq!(parse_arch("ARM64").unwrap(), Arch::Aarch64);
    }
}
