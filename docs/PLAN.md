# terminal-windows: refined plan (v2)

- Written: 2026-11-05
- Upstream reference: `govijr/terminal-mac` @ `ac787d5` (v0.2.0 build 6, MIT)
- Scope: a review of the original specification ("Terminal-Windows: Full Implementation Prompt"), revised and extended so that **Windows guest and Windows host development can start directly from it**

This document replaces the original prompt as the **top-level plan**. Requirements of the original that are kept are followed as written; where this document differs, this document wins. Detailed designs live in:

| Document | Contents |
|---|---|
| [upstream-analysis.md](upstream-analysis.md) | Analysis of the real terminal-mac code; port/rewrite decisions |
| [architecture.md](architecture.md) | Crate layout, runtime composition, data flow |
| [protocol.md](protocol.md) | Host-to-guest protocol v1 |
| [security.md](security.md) | Threat model and default security policy |
| [compatibility.md](compatibility.md) | Per-platform verification matrix (measured results only) |
| [PROVENANCE.md](PROVENANCE.md) | Record of code copied, adapted or newly written relative to upstream |
| [../firmware/README.md](../firmware/README.md) | The Secure Boot UEFI firmware: why it exists and how it is built |

---

## 1. One-paragraph summary

> terminal-mac is "Apple Virtualization + macOS guest + virtio-fs mailbox + in-guest helper". Moving that structure to Windows as-is breaks on the **transport, the TPM, the screen/input path and terminal graphics on Windows hosts**. The v2 plan separates a **hypervisor control plane (QMP/VNC)** from a **guest-agent control plane (virtio-serial)**. The hypervisor plane lets the setup screens, sign-in and UAC be seen and driven without any agent installed; the agent concentrates on what only code inside the guest can do (Unicode input, UI Automation).

---

## 2. Problems found in the original specification

Severity: 🔴 cannot work unless the design changes · 🟠 large schedule/quality risk · 🟡 ambiguous or missing

### 2.1 🔴 A shared-folder mailbox does not work on macOS or Windows hosts

Upstream drops request/response JSON files into Apple VZ's virtio-fs automount (`macOSGuestAutomountTag`). Using virtio-fs with QEMU needs `virtiofsd` on the host, and **virtiofsd is Linux-only** (it depends on vhost-user and Linux kernel features). So none of the three target hosts (Windows x64, macOS Intel, macOS Apple Silicon) has this path. On the guest side Windows would also need a virtio-fs driver plus WinFsp.

**Change:** the guest agent transport is **virtio-serial (virtserialport)**. The host side is a QEMU chardev (Unix socket, or a Windows named pipe); the guest side is the device `\\.\Global\com.terminal-windows.agent.0`. There is no network listener and the host endpoint is protected by file permissions (0600) or a DACL. The Hyper-V backend will use **hvsocket (AF_HYPERV)**. A second-choice fallback would be SLIRP `guestfwd` (with `restrict=on` so it coexists with offline mode). The shared-folder mailbox is not adopted.

### 2.2 🔴 There is effectively no vTPM (swtpm) for QEMU on Windows hosts

Installing Windows 11 needs a TPM 2.0. QEMU's TPM depends on an external `swtpm` process (`-tpmdev emulator`), and swtpm exists mainly for Linux and macOS (Homebrew); **there is no official Windows build.** The original prompt makes "boot Windows 11 with WHPX" the primary goal, but that combination will probably stall at the TPM. QEMU with WHPX also has many reports of performance and stability problems with Windows 11 guests.

**Change:**
- On Windows hosts the **supported path is the Hyper-V backend** (Generation 2 VM, built-in vTPM, Secure Boot, hvsocket), promoted from "optional, Phase 8" to **Phase 3b**.
- QEMU with WHPX on Windows hosts is marked `experimental`, and `doctor` actually checks whether swtpm is usable. Without a TPM, creating a Windows 11 VM is **refused** (no bypass flag, honouring the original's "do not bypass eligibility checks").
- Windows Home hosts (no Hyper-V) are stated as unsupported until QEMU, WHPX and swtpm are verified.

### 2.3 🔴 A guest agent alone cannot handle setup, sign-in or UAC screens

Desktop Duplication / Windows Graphics Capture and `SendInput` only work on an **ordinary desktop of a user session**. In these situations they cannot work, in principle:

| Situation | Agent capture | Agent input |
|---|---|---|
| Windows Setup / OOBE (agent not installed) | ✗ | ✗ |
| Sign-in / lock screen (Winlogon desktop) | ✗ | ✗ |
| UAC consent prompt (secure desktop) | ✗ | ✗ |
| Elevated windows (UIPI) | ○ | ✗ (medium integrity cannot reach high integrity) |
| Running as a Session 0 service | ✗ | ✗ |

**Change:** introduce **two control planes**.

- **Plane H (hypervisor):** screen from QEMU QMP `screendump` (or the VNC framebuffer); input through QMP `input-send-event` (absolute-coordinate usb-tablet plus qcode keys). This sits where a physical keyboard and monitor would, so firmware setup, installation, sign-in and UAC are all visible. It is not a bypass of a security boundary; it is the VM owner's "physical console".
- **Plane G (guest agent):** Unicode text input, UI Automation tree, focused application, DPI/resolution, readiness, graceful shutdown requests.
- The ground truth for screenshots is Plane H. Data reported by Plane G is marked `source: guest-reported` (malware inside the guest could impersonate the agent).
- MCP policy: when the guest is on the secure desktop or lock screen, agent tools **refuse to act and tell the agent to hand over to the user** by default. Plane H is used only if the VM owner explicitly opts in with `--allow-console-input` (planned).

### 2.4 🔴 QEMU cannot show or hide its native window at runtime (resolved with our own window)

Upstream's `show`/`hide` hide a VZ window with `orderOut`. QEMU's `-display cocoa/gtk/sdl` window cannot be hidden and re-shown at runtime.

**Change:** the VM **always runs headless** (`-display none`) and the screen is exported only through QMP (and VNC on a local Unix socket). `show` is redefined as "start a native viewer process" and `hide` as "stop it". The process is our own window (winit and softbuffer, macOS and Windows) reading frames over a second QMP monitor, which is also the automatic fallback of `view` in terminals without graphics. VM lifetime and window lifetime become independent, which is actually more robust.

### 2.5 🟠 Terminals on Windows hosts do not support the Kitty graphics protocol

Windows Terminal does not support the Kitty Graphics Protocol; it supports **Sixel** (1.22+). Kitty and Ghostty have no native Windows builds. WezTerm runs on Windows and supports parts of the Kitty and iTerm2 image protocols. Upstream's fast path (`t=s`, POSIX shared memory) does not exist on Windows.

**Change:** the renderer is pluggable: `kitty-file` (raw RGB via a temporary file; Kitty, Ghostty), `kitty-direct` (PNG chunks through the pty), `sixel` (Windows Terminal; low frame rate) and a `native-window` fallback. Support is decided **by measuring at the emulator level** (a Kitty `a=q` query, the Sixel bit `4` in the DA1 reply), never from environment variables alone.

### 2.6 🟠 The coordinate system was undefined (Windows DPI scaling)

Windows is often at 125% or 150% scaling. Screenshot pixels, logical coordinates, `SendInput` absolute coordinates (0-65535 normalised over the virtual desktop) and QEMU usb-tablet coordinates (0-32767) are all different. Upstream used macOS "points", which has no direct Windows counterpart.

**Change:** the **public coordinate system is the physical pixels of the primary display (the pixels of the screenshot image, origin at the top left)**. The agent is built with a Per-Monitor-V2 DPI-aware manifest. When a screenshot is returned downscaled, `scale` metadata accompanies it and the server converts back (the model never converts by hand). The conversions are unit-tested in `tw-core::coords`.

### 2.7 🟠 How the guest agent runs was ambiguous

"Do not install it as an invisible service" conflicts with "connect automatically after boot". A Windows service runs in Session 0 and cannot capture or drive the user's desktop.

**Change:** the agent runs as a **Scheduled Task at user logon (user session, interactive)** with an **always-visible tray icon**, and a Start-menu entry to stop or uninstall it. Driving elevated windows would need a signed `uiAccess=true` build (installed under Program Files), which is to be evaluated as a separate option; by default the agent runs at medium integrity and returns an `elevated_target` error when UIPI blocks input.

### 2.8 🟠 Windows 11 ARM64 on Apple Silicon has large driver and display constraints

- Windows ARM64 effectively has no virtio-gpu display driver, so **ramfb (fixed resolution)** is used. "Dynamic resolution adaptation" cannot be met on ARM64 (the resolution is fixed by the firmware GOP).
- The disk is **NVMe**, which has an inbox driver (no extra driver needed during setup).
- The network card is a **USB network device (RNDIS)**, not virtio-net: Windows ARM64 has no inbox virtio-net driver, which left OOBE stuck on "Connect to a network" (error 0xe0000100). The Windows 11 26H2 ARM64 image ships `netrndis.inf` (checked inside `install.wim`), so nothing has to be downloaded. `--drivers-iso` remains available for other drivers.
- Microsoft's officially supported Windows 11 virtualization on Apple Silicon is Parallels and VMware Fusion. The QEMU path works technically but is **not a Microsoft-supported configuration**, and the documentation says so.
- x64 Windows on Apple Silicon (TCG emulation) is **refused**, keeping the original requirement.

### 2.9 🟠 The Intel Mac + HVF + Secure Boot firmware combination is unverified

Secure Boot capable OVMF (`*-secure-code.fd`) usually requires SMM (`-machine q35,smm=on`), and whether HVF supports SMM has to be measured per QEMU version. Windows 11's requirement is "Secure Boot **capable** UEFI".

**Change:** firmware is not chosen from hard-coded paths. The **firmware descriptor JSON files** that QEMU installs (`share/qemu/firmware/*.json`) are parsed and firmware is selected by `architecture`, `interface-types` and `features` (secure-boot, enrolled-keys, requires-smm). This is implemented in `tw-qemu::firmware`. The Secure Boot path on Intel Macs is recorded as `pending` in compatibility.md.

### 2.10 🟠 Protocol deadlines must not use absolute times

Upstream's `Request.deadline` is an **absolute host time** that the guest checks with `deadline > Date()`. Guest clocks drift badly right after boot, after a pause and after a snapshot restore, so requests would expire immediately or stay valid forever.

**Change:** protocol v1 uses a **relative timeout (`timeout_ms`), deadline management on the host, and monotonically increasing sequence numbers** (protocol.md).

### 2.11 🟠 The effective reach of "replay protection and authentication" was overstated

A virtio-serial port can be opened by **any process inside the guest**. When the guest is untrusted (original section 10), "guest authentication" does not protect the host. Real protection comes from (1) access control on the host endpoint, (2) strict validation of guest input, and (3) not exposing dangerous host features to the guest.

**Change:** keep the per-VM HMAC session but state its purpose exactly: preventing confusion between VMs, preventing impersonation by other local processes on the host (key file is 0600), and preventing replay and reordering. The documentation says "guest authentication is not guest trust" (security.md).

### 2.12 🟡 Problems with scope, order and deliverables

| Problem | Change |
|---|---|
| 11 crates conflicts with "no empty modules" (for example a premature `tw-hyperv` would be an empty shell) | Add a crate only when real code exists. Currently 8 crates plus the guest agent |
| `tw-transport` and `tw-security` gain little as separate crates | Transport and session security live in `tw-protocol`; file permissions and key handling in `tw-core::secure` |
| The synchronous trait sketch (`fn start(...)`) does not fit the Tokio requirement | Object-safe `async_trait` traits plus `Support::{Supported, Unsupported, Unverified}` |
| E2E (hours of installation, genuine ISO needed) as a single-session completion condition | Per-phase **exit criteria** and an "evidence" format (section 7) |
| MCP tools lack double click, right click, wait, UI tree, region zoom | `click{button,count}`, `wait_for_change`, `ui_tree` (Plane G), `screenshot{region,max_width}` |
| MCP protocol version unspecified | Negotiate against the supported list (`2025-11-25`, `2025-06-18`, `2025-03-26`) and use `structuredContent` |
| `delete` only mentions safety; deleting while running or after partial failure is not handled | Refuse while running; take the lock; rename `machine.toml` to `.deleting` first, then delete |
| No position on unattended installation (autounattend) | The documented `autounattend.xml` (local account, agent install through FirstLogonCommands) is offered only as an **explicit opt-in**. Keys that bypass TPM, Secure Boot or network requirements (LabConfig, BypassNRO, ...) are never included |
| Guest keyboard layout not considered | Plane H text input only guarantees US-layout ASCII (qcodes are physical keys). Other characters need Plane G Unicode input and fail with an explicit error |

---

## 3. Product definition (v2)

### 3.1 Goals
1. One terminal command (`terminal-windows`, or `terminal-windows up`) creates, boots and shows a Windows VM.
2. From installation to sign-in, the terminal and agents can see and drive the screen **without an agent** (Plane H).
3. After sign-in, the agent adds Unicode input, the UI tree and similar features (Plane G).
4. The CLI, MCP server and viewer share **one `VmService`**.

### 3.2 Non-goals (explicit)
- Not a hardened malware sandbox.
- No bypass of Windows licensing, activation or hardware requirements. No bundled ISO.
- No dependency on cloud services or paid APIs.
- No emulation of x64 guests on Apple Silicon.

### 3.3 Support levels
| Level | Meaning |
|---|---|
| `supported` | A combination for which all 8 stages of compatibility.md were measured and passed |
| `experimental` | Builds and the CLI work; booting a VM or later stages are partly unverified |
| `pending` | A code path exists but there is no environment to measure it |
| `unsupported` | Intentionally refused (the error message says why) |

---

## 4. Architecture overview

```text
                +------------- terminal-windows (host) --------------+
 CLI ---+       |                                                     |
 MCP ---+--> VmService --> VirtualizationBackend (QEMU | Hyper-V)     |
 View --+       |   |                                                 |
                |   +--> ComputerControl -- Plane H: QMP -------------+--> QEMU --> virtual monitor/keyboard/tablet
                |   |                    +- Plane G: tw-protocol -----+--> virtio-serial / hvsocket
                |   +--> Store (machine.toml, locks, keys, logs)      |            |
                +-----------------------------------------------------+            v
                                                          Windows guest: tw-agent (user session, tray icon)
                                                          Desktop Duplication, SendInput, UI Automation
```

| Host | Backend | Acceleration | Guest | Firmware / TPM | Plane G transport | Target level |
|---|---|---|---|---|---|---|
| macOS Apple Silicon | QEMU `virt` | HVF | Win11 ARM64 | edk2-aarch64 with Secure Boot (built here) + swtpm (`tpm-tis-device`) | virtio-serial (Unix socket) | supported (first priority) |
| macOS Intel | QEMU `q35` | HVF | Win11 x64 | OVMF + swtpm (`tpm-tis`) | virtio-serial | experimental, then supported |
| Windows x64 (Pro and up) | Hyper-V Gen2 | Hyper-V | Win11 x64 | built-in UEFI / vTPM | hvsocket | supported (Phase 3b) |
| Windows x64 | QEMU `q35` | WHPX | Win11 x64 | OVMF + swtpm (if available) | virtio-serial (named pipe) | experimental |

Why **Apple Silicon** is the first development host: it is the current development machine, QEMU, HVF, swtpm and edk2 are all available through Homebrew, and the Plane H/G design can be measured there fastest. The same code serves the QEMU path on Windows hosts (the differences are Unix socket versus named pipe, and the accelerator name).

Details: [architecture.md](architecture.md)

---

## 5. Windows guest agent: design points

- **Language and build:** Rust, `windows-sys`, targets `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`. Manifest: PerMonitorV2 DPI aware, `asInvoker`.
- **Execution:** a Scheduled Task at user logon, tray icon always visible, single-instance mutex, log at `%LOCALAPPDATA%\terminal-windows-agent\agent.log`.
- **Installation:** a per-VM "tools ISO" (agent exe, VM key, install script) attached as a CD. The user runs `install.cmd` themselves, or an opt-in autounattend runs it through FirstLogonCommands.
- **Capability mapping:**

| Capability | API | Notes |
|---|---|---|
| Screen capture | Desktop Duplication (`IDXGIOutputDuplication`) | Dirty-rect based; off by default (Plane H comes first) |
| Mouse | `SendInput` with `MOUSEEVENTF_ABSOLUTE` | Physical pixels normalised to 0-65535 (`tw_core::coords`) |
| Key chords | `SendInput` virtual keys | `win`, `ctrl`, `alt`, `shift` |
| Unicode input | `SendInput` `KEYEVENTF_UNICODE` | UTF-16 surrogate pairs handled; no clipboard |
| Focus / UI tree | UI Automation (`IUIAutomation`) | Depth and node count limited |
| Display | `GetSystemMetrics`, `GetDpiForSystem` | Reports resolution and scale |
| Session state | `OpenInputDesktop` failure or name check | Reports `secure_desktop`, `locked`, `no_session` |

---

## 6. Default security posture (details: security.md)

- No sharing of the host desktop, clipboard, home directory or credentials. Shared folders are off at the feature level.
- No network listeners: QMP, VNC and the agent channel are all **Unix sockets (0600, directory 0700)** or **named pipes with a current-user DACL**. Where TCP is unavoidable: `127.0.0.1` plus a VNC password, and a warning that other local users can reach the port.
- Network modes: `nat` (default, explicitly not isolation) and `offline` (no NIC).
- Guest strings are length-limited and control characters escaped before logging. Keys and tokens are never logged.
- The host **never runs a shell command built from guest data**. QEMU and swtpm arguments come only from host-side configuration.

---

## 7. Phased development plan (revised)

Each phase must meet its **exit criteria** and record the evidence (command, output, date, host specification) in compatibility.md before the next one starts.

| Phase | Content | Exit criteria | Status |
|---|---|---|---|
| P0 | Upstream analysis, licensing | upstream-analysis.md, PROVENANCE.md | ✅ Done |
| P1 | Rust foundation: config model, store, errors/exit codes, backend trait, CLI, logging | `cargo test` passes; `doctor`, `create`, `ls`, `config` really work | ✅ Done |
| P2a | QEMU detection, accelerator, firmware descriptors, command line, QMP client | Command-line unit tests plus mock QMP integration tests | ✅ Done |
| P2b | Real boot (Apple Silicon) | `start`, a UEFI screenshot via QMP, `pause/resume/stop` measured | ✅ 2026-11-05 (compatibility.md) |
| P2c | Plane H control | Boot the ISO and drive the setup screens with `action screenshot/click/key` | ✅ 2026-11-05 Windows 11 Setup driven by mouse and keyboard up to the license screen |
| P3a | Protocol v1 and transport | Handshake, HMAC, replay protection and size limits tested (done); virtio-serial measured | 🟡 Protocol done, measurement pending |
| P3b | Hyper-V backend (Windows host) | Create a Gen2 + vTPM VM, connect over hvsocket | ⏳ Needs a Windows machine |
| P4 | Windows agent | After sign-in, `action type` of a Unicode sentence appears in Notepad | 🟡 Pure logic done; Win32 code compiles in CI (Windows x64 and ARM64); not run in a VM |
| P5 | Terminal viewer | Verified in Kitty, Ghostty, WezTerm and Windows Terminal (Sixel), including resize and recovery | 🟡 In-terminal viewer verified in a pseudo-terminal; native window viewer (macOS, Windows) implemented and observed on macOS; real Ghostty/Kitty/WezTerm and Sixel pending |
| P6 | MCP | An observe, act, verify loop measured with Claude Code or Codex | 🟡 Server and schemas done; driven against a real VM over stdio; real clients pending |
| P7 | Cross-platform validation | The 8-stage matrix in compatibility.md | ⏳ |
| P8 | Snapshots (qcow2 internal / Hyper-V checkpoints), templates, updates, release automation | Only after P2c, P4 and P6 | ⛔ (release automation already exists) |

### Order of work for the Windows host version (practical)
1. Prepare a Windows 11 Pro x64 machine: enable Hyper-V and the Windows Hypervisor Platform; install Rust (MSVC) and QEMU for Windows.
2. Confirm `cargo test --workspace` and `cargo build -p tw-agent` pass (the same as CI; both already pass there).
3. P3b: add a `tw-hyperv` crate and implement `VirtualizationBackend` (call the PowerShell `Hyper-V` module with **fixed argument arrays**, or WMI `root\virtualization\v2`). Start screenshots with `Msvm_VirtualSystemManagementService.GetVirtualSystemThumbnailImage` and measure performance.
4. Agent: validate the `cfg(windows)` code of `guest/windows-agent` in a real VM, then add an hvsocket listener.
5. Viewer: confirm the DA1 Sixel bit in Windows Terminal and implement the Sixel renderer; measure `kitty-direct` in WezTerm.

---

## 8. Risk register

| ID | Risk | Impact | Response |
|---|---|---|---|
| R1 | No swtpm for Windows | Win11 impossible on the QEMU path of Windows hosts | Hyper-V path first; QEMU stays experimental |
| R2 | QEMU with WHPX performance and stability | Perceived quality on Windows hosts | Benchmark, then choose the default backend |
| R3 | ARM64 ramfb has a fixed resolution | No dynamic resolution | Choose the resolution at creation; document it |
| R4 | Microsoft narrowing local-account OOBE paths | Unattended installation fails | Unattended install is opt-in; the manual path always remains |
| R5 | virtio-serial throughput | Limits Plane G frame streaming | Frames use Plane H; Plane G stays control-only |
| R6 | Windows Terminal Sixel performance | Low frame rate | Send only on change, downscale, native-window fallback |
| R7 | AF_UNIX support in QEMU for Windows | Different socket paths | Named-pipe chardev, or localhost plus password as fallback; decide after measurement |
| R8 | Malware in the guest impersonating the agent | Wrong observations | Ground truth is Plane H; guest-reported data is labelled |
| R9 | Upstream firmware builds (Debian, UTM) do not fit HVF + ramfb | No Secure Boot firmware for Apple Silicon | Own build of QEMU's edk2 with Secure Boot, published with releases ([../firmware/README.md](../firmware/README.md)) |

---

## 9. Current state

Implemented and verified by tests, versus not yet measured:

**Implemented, with automated tests passing (Apple Silicon, macOS 26; CI on macOS, Linux and Windows):**
- `tw-core`: VM name validation, `machine.toml` model and Windows 11 requirement validation, host/guest architecture policy, store (atomic writes, exclusive locks, 0700/0600 permissions), error and exit codes, coordinate conversion, backend and control traits, per-VM keys, `VmService`.
- `tw-qemu`: binary, accelerator and swtpm detection; firmware descriptor parsing and selection (including managed firmware); QEMU, swtpm and qemu-img command construction (x86_64 and aarch64); async QMP client; backend lifecycle; Plane H control (screendump, absolute-coordinate mouse, qcode keys, US-ASCII text).
- `tw-protocol`: v1 framing, handshake, HMAC-SHA256 session, replay protection, size limits.
- `tw-input`: upstream terminal input parser ported, chord parser, qcode and Windows VK mapping.
- `tw-renderer`: upstream layout and metric filter ported, Kitty encoder (temporary-file and PNG transports), terminal detection.
- `tw-media`: Microsoft download page and API client, resumable download, SHA-256 verification, ISO cache, Downloads watcher.
- `tw-mcp`: MCP stdio server (initialisation, version negotiation, tool schemas, tool execution).
- `tw-cli`: `up`, `doctor`, `image`, `firmware`, `create`, `start`, `stop`, `pause`, `resume`, `status`, `ls`, `view`, `delete`, `logs`, `config`, `action ...`, `mcp`. `show` and `hide` open and close the native window viewer. `up` (also a bare `terminal-windows` in a terminal) runs the guided flow.
- `guest/windows-agent`: request dispatcher, coordinate normalisation, UTF-16 chunking and other platform-independent logic with tests; the Win32 implementation compiles in CI for x64 and ARM64.

**Measured (2026-11-05):** Apple Silicon with QEMU 11.1.3 and HVF: firmware boot, QMP screenshots, keyboard and mouse input, pause/resume, forced stop, restart, delete, MCP calls against a real VM, the Windows 11 ARM64 Setup passing its Secure Boot and TPM requirement checks, and an official ISO downloaded and verified. Details in compatibility.md.

**Not yet measured:** a complete Windows installation (file copy, OOBE, sign-in), agent connection, `view` in a real Ghostty/Kitty window, MCP clients (Claude Code or Codex) driving the VM, the Intel Mac and Windows host paths.
