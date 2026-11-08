# terminal-windows

Create, boot, view and control a real Windows virtual machine from a terminal. It is designed so that MCP agents (Codex, Claude Code, Cursor, ...) can do GUI work in an **isolated Windows guest instead of on your own desktop**. It is based on the structure and ideas of [govijr/terminal-mac](https://github.com/govijr/terminal-mac).

> **Status: early development.** On Apple Silicon (QEMU + HVF) the VM boots, Windows 11 Setup runs through its hardware requirement checks (TPM, Secure Boot), and the VM can be viewed and driven from the terminal. A complete Windows installation, the in-guest agent and real-terminal verification of `view` are still pending. See [docs/compatibility.md](docs/compatibility.md) for exactly what has been verified, and on what.

## Design highlights

- **QEMU** based (macOS: HVF, Windows: WHPX); a **Hyper-V** backend is planned for Windows hosts.
- **Two control planes:** the hypervisor console (QMP) drives setup, sign-in and UAC screens; an in-guest agent adds Unicode input and UI Automation.
- No network listeners, no shared folders or clipboard sharing, no bypass of Windows requirements.

Plan and analysis of the original specification: [docs/PLAN.md](docs/PLAN.md)

## Requirements

- QEMU 7.1+ and swtpm (macOS: `brew install qemu swtpm`)
- A Windows 11 installation ISO. `terminal-windows image download` fetches it from Microsoft and verifies it (see below). Apple Silicon needs the **ARM64** ISO.
- To build from source: Rust 1.89+

## Install

```sh
curl -fsSL https://github.com/dlfkdLR/terminal-windows/releases/latest/download/get.sh | bash
```

This downloads the latest release, verifies it against `SHA256SUMS` and installs a single binary to `~/.local/bin/terminal-windows` (no administrator rights, no shell configuration changes). On Windows (experimental): `irm https://github.com/dlfkdLR/terminal-windows/releases/latest/download/install.ps1 | iex`.

### Apple Silicon: Secure Boot firmware

Windows 11 requires Secure Boot capable firmware, and the firmware that ships with Homebrew's QEMU has none. Install ours once (downloaded from the release and SHA-256 verified):

```sh
terminal-windows firmware install
terminal-windows firmware list      # shows whether each firmware supports Secure Boot
```

Background and how to build it yourself: [firmware/README.md](firmware/README.md)

## Quick start

```sh
terminal-windows        # same as `terminal-windows up`
```

In a terminal, running `terminal-windows` with no command does everything that is still missing, then opens the viewer, like terminal-mac:

1. installs QEMU and swtpm with Homebrew (asks first; `--yes` skips the question),
2. installs the Secure Boot firmware (Apple Silicon),
3. downloads Windows 11 from Microsoft in your system language and verifies its SHA-256 (the final download click happens in your browser, see below),
4. asks how to create the VM (see below), then creates and boots it, pressing the "boot from CD" key at the right moment, and **installs Windows automatically** (see below),
5. opens the viewer: inside the terminal where that works, otherwise in a native window (Ctrl+] or closing the window leaves it; the VM keeps running).

### Automatic Windows installation

Like terminal-mac, which installs macOS through Apple's API and leaves only the account setup to you, terminal-windows lets Windows Setup run by itself with an answer file: language (taken from the ISO), wiping the VM's own empty disk, partitions, and the **Windows 11 Pro** edition. Two things stay yours on purpose:

- **the license agreement page**: you click "Agree" in the viewer, once;
- **the account setup (OOBE)** after the installation: name, account, privacy choices.

No product key is entered (Windows installs unactivated, the same as choosing "I don't have a product key") and no requirement is bypassed. The answer file is only attached to a VM whose disk is brand new and that has no Windows yet, so it can never wipe a disk that holds data or an installed Windows. `terminal-windows up --manual-install` (or leaving `--unattended` off in `create`) lets you click through Setup yourself instead. An ISO whose language is not known (a renamed file you imported) keeps the language page interactive.

### Automatic or custom VM

When a new VM is about to be created in a terminal, you choose:

1. **Automatic (recommended):** half of your CPUs (2 to 8), 8 GiB of RAM (4 GiB on computers with less than 16 GiB), an 80 GiB disk, NAT network.
2. **Custom:** you set the name, CPUs, memory, disk size and network (NAT or offline). The limits are shown and enforced: at least 2 CPUs, 4 GiB RAM and 64 GiB disk (Windows 11's minimums), memory at most your RAM minus 4 GiB, disk at most 2048 GiB. The disk is sparse: it only takes the space the guest actually uses.

`--custom` always asks, `--auto` never asks, and `--cpus`, `--memory` (GiB) and `--disk` (GiB) set values directly (no questions). Without a terminal (scripts) the automatic settings are used.

Run it again later and it just reconnects. `terminal-windows up --plan` shows the steps without doing anything; `--no-view` boots without opening the viewer; `--iso PATH`, `--language`, `--cpus`, `--memory`, `--disk` customise a new VM. Commands that take a VM name (`start`, `view`, `status`, ...) default to `default`, or to the only VM if there is just one.

## Build and use

```sh
cargo build --release
B=target.nosync/release/terminal-windows   # .cargo/config.toml keeps build output out of synced folders

$B doctor                                   # probes QEMU, accelerator, firmware, TPM
$B image download --language Korean         # download from Microsoft + verify the official SHA-256
$B firmware install                         # (Apple Silicon, once) Secure Boot firmware
$B create dev                               # uses the verified ISO from the cache (or pass --iso)
$B start dev
$B view dev                                 # view and control in the terminal (Ghostty/Kitty/WezTerm); Ctrl+] exits
$B action dev screenshot                    # prints the PNG path
$B action dev click 400 300
$B action dev key ctrl+shift+esc
$B action dev type "Hello Windows"          # US-ASCII only without the guest agent
$B stop dev                                 # ACPI shutdown request (--force: immediate power off)
$B delete dev                               # asks for confirmation (--force skips it)
```

### Which terminal?

`terminal-windows view` works in **any** terminal, like terminal-mac:

- **Ghostty, Kitty, WezTerm** (Kitty graphics): the desktop is drawn inside the terminal.
- **Any other terminal** (Apple Terminal, VS Code or Cursor terminal, tmux, ...): a native window opens instead (macOS and Windows). `terminal-windows show` opens it directly, `terminal-windows hide` closes it, and `view --window` forces it. Closing the window never stops the VM.

The native window forwards the mouse and the keyboard as a physical keyboard, so the guest's own layout decides what is typed: Command is the Windows key, and on a 101-key layout the Right Option key is the Hangul toggle of the Korean IME (keyboards with dedicated keys send those too).

### Viewer keys

Ctrl+G opens a palette of Windows shortcuts (Start, Ctrl+Alt+Del, Alt+Tab, Run, Hangul/English toggle, ...). Ctrl+] leaves the viewer; the VM keeps running.

### Getting the Windows ISO

Just as terminal-mac downloads macOS images from Apple, `image download` reads the current release and the per-language SHA-256 values from **Microsoft's official download page** and requests a download link from the same API the page uses.

- If a link is granted: resumable download, SHA-256 verification, then cached.
- If Microsoft refuses the automated request (its bot protection, "Sentinel"; it always refused when measured on 2026-11-05), terminal-windows **does not work around it**. It opens the official page in your browser, explains the clicks, watches your Downloads folder, verifies the SHA-256 of the finished file and moves it into the cache.
- Already have an ISO? `image import PATH [--move]` (it must match an official hash to be cached).
- `image info` shows the current version and hash without downloading; `image list` shows the cache.

ISOs are never bundled or redistributed, and unverified files are never put in the cache (you can still use one explicitly with `create --iso`).

Every command supports `--json` and fails with stable error codes and exit codes (`crates/tw-core/src/error.rs`).

## Connect an agent (MCP)

```sh
claude mcp add terminal-windows -- /absolute/path/to/terminal-windows mcp dev
terminal-windows mcp dev --config     # JSON snippet for other MCP clients
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.terminal-windows]
command = "/absolute/path/to/terminal-windows"
args = ["mcp", "dev"]
```

Tools: `screenshot, click, move, drag, scroll, type, key, status, list_vms, start_vm, stop_vm`. Coordinates are screenshot pixels with the origin at the top left. Agents should work observe, then act, then verify (the `screenshot: true` option helps).

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Documentation: [architecture](docs/architecture.md) · [protocol](docs/protocol.md) · [security](docs/security.md) · [compatibility](docs/compatibility.md) · [upstream analysis](docs/upstream-analysis.md) · [code provenance](docs/PROVENANCE.md)

## License

MIT. Parts adapted from upstream terminal-mac (MIT, Copyright (c) 2026 Govijr) are listed in [docs/PROVENANCE.md](docs/PROVENANCE.md) and noted in the header of each affected file.

## Commands

| Command | What it does |
|---|---|
| `terminal-windows` / `up` | Guided setup: firmware, Windows ISO, VM creation, boot, view (`--auto`, `--custom`) |
| `view`, `show`, `hide` | Desktop in a Kitty-graphics terminal (Ghostty, Kitty, WezTerm) or the native window; `hide` leaves the VM running |
| `ls`, `status`, `start`, `stop [--force]`, `pause`, `resume`, `logs`, `config`, `delete --force` | VM lifecycle |
| `create <name> --unattended [--cpus --memory --disk --offline --iso --drivers-iso]` | Create a VM without the guided flow |
| `image download`, `firmware install`, `doctor` | Verified Windows ISO, Secure Boot firmware, host check |
| `action <name> screenshot / click / type / key / scroll / drag / move / status` | Computer-use actions on a running VM |
| `mcp` | MCP server on stdio for agents |

Add `--json` to any command for machine-readable output. To update, run the install command again.

### Network on ARM64

Windows 11 ARM64 has no inbox driver for any NIC QEMU can emulate. The VM uses virtio-net and the Microsoft-signed NetKVM driver from virtio-win (BSD-3-Clause, bundled unmodified, see `docs/PROVENANCE.md`) is placed on the setup-media CD in `$WinPEDriver$`. A VM whose Windows installation was already started with an older version does not get the driver retroactively: create a new VM, or install it from OOBE's "Install driver" picker (the CD stays attached while the installer ISO is attached).
