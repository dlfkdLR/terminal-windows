# Compatibility matrix

**Only measured results are recorded.** Compiling is never listed as platform support. Every cell is one of `✅ date/environment`, `❌ reason`, `⏳ pending`, `⛔ unsupported (intentional)`.

| Stage | macOS Apple Silicon (Win11 ARM64) | macOS Intel (Win11 x64) | Windows x64 + QEMU/WHPX | Windows x64 + Hyper-V |
|---|---|---|---|---|
| 1. Builds | ✅ 2026-11-05, macOS 26 arm64; also CI on macos-14 | 🟡 release binary is cross-compiled in CI; never run on Intel hardware | ✅ CI on windows-latest (build, clippy, tests) | ⏳ backend not implemented |
| 2. CLI runs | ✅ 2026-11-05 (`doctor`, `ls`, `status`, `create`, `mcp`, ...) | ⏳ | 🟡 tests run in CI; CLI not run by hand | ⏳ |
| 3. VM boots | ✅ 2026-11-05 firmware and Windows Setup (see measured records) | ⏳ | ⏳ no swtpm for Windows, so Win11 is expected to be impossible | ⏳ |
| 4. Windows installs | 🟡 2026-11-05 Windows 11 26H2 ARM64 (Korean) Setup passes the requirement checks (Secure Boot, TPM) and reaches the license screen. File copy, OOBE and sign-in ⏳ | ⏳ | ⏳ | ⏳ |
| 5. Agent connects | ⏳ | ⏳ | ⏳ | ⏳ |
| 6. Terminal viewer | 🟡 2026-11-05 verified in a pseudo-terminal: capability probe, frame delivery (1,440,000 bytes), mouse, Ctrl+G, Ctrl+] exit, terminal restore. Display in a real Ghostty/Kitty window ⏳ | ⏳ | ⏳ | ⏳ |
| 7. MCP control | 🟡 2026-11-05 stdio calls against a real VM (UEFI) succeeded. Claude Code / Codex clients ⏳ | ⏳ | ⏳ | ⏳ |
| 8. E2E | ⏳ | ⏳ | ⏳ | ⏳ |

## Measured record: first boot on Apple Silicon (2026-11-05)

Environment: MacBook Pro (Apple Silicon), macOS 26, QEMU 11.1.3 (Homebrew), swtpm 0.10.2, Homebrew edk2-aarch64 (no Secure Boot). No Windows ISO yet, so the first boot used an empty 80 GiB qcow2 disk without media.

| Item | Result |
|---|---|
| `doctor` HVF probe | ✅ supported (actually started) |
| `create` (qcow2, efivars, TPM state directory, agent key) | ✅ |
| `start`: HVF + `virt` + ramfb + NVMe + qemu-xhci + swtpm TPM + virtio-serial + NAT + QMP + VNC (unix) | ✅ `running` (after fixes 1 and 2 below) |
| UEFI screen capture (`action screenshot`, QMP screendump PNG) | ✅ 800×600, EDK II shell, NVMe `BLK0` recognised |
| Keyboard input (`action type` / `key`, QMP qcodes) | ✅ a 47-character mixed-case/symbol string typed exactly (30 ms event gap) |
| `pause`, refusing actions while paused, `resume` | ✅ (exit code 6 on refusal) |
| MCP `initialize` (2025-11-25), `key` plus screenshot, `status` | ✅ against a real VM |
| `stop` (ACPI) | ✅ request delivered; the UEFI shell has no OS to handle it, so it keeps running (expected) |
| `stop --force` and automatic swtpm exit (`--terminate`) | ✅ no stray processes |
| Restart, TPM state files kept | ✅ |
| `delete` refused while running; delete after stop | ✅ |
| Simulated QEMU upgrade (recorded Cellar path gone) and firmware lookup again | ✅ |
| Mouse (usb-tablet absolute coordinates) | ✅ buttons clicked accurately in Windows Setup (cursor at the given coordinates) |

Problems found by measuring, and fixed:
1. **`tpm-tis-device` with HVF fails with `HV_BAD_ARGUMENT`.** It works under TCG. The cause is presumed to be the PPI RAM region (not 16 KiB page aligned); `ppi=off` fixes it. Windows 11 Setup then passed its TPM check without PPI.
2. `-mon` is deprecated in QEMU 11, so `-object monitor-qmp` is used (older QEMU keeps `-mon`).
3. The QMP socket serves one client at a time, so checking status while holding a console connection timed out. The order was fixed and the trait documented.
4. Sending key events in bursts overflows the USB HID queue (16 entries) and loses input (8 ms batches: lost; 15 ms: scrambled; 30/50 ms: exact). Default 30 ms, tunable with `TW_CONSOLE_EVENT_DELAY_MS`.
5. Firmware descriptors from Homebrew contain versioned paths, so after an upgrade the firmware is looked up again by file name.

Note: typing during EDK II's 5-second `startup.nsh` countdown right after boot is consumed as "any key" (a test timing issue, not a code defect).

## Measured record: guided `up` flow (2026-11-05, Apple Silicon)

| Item | Result |
|---|---|
| `up --plan` on a fresh data directory and on a machine with a running VM | ✅ correct steps / "nothing to do" |
| `up --iso ... --no-view --yes` in a fresh data directory: firmware installed from the v0.1.1 release (SHA-256 verified), VM created, booted | ✅ Windows Setup screen reached with no manual key press |
| "Press any key to boot from CD" handling | The prompt is on screen only about 3 s (6.5 s to 9.7 s after start). A first version pressed keys on a fixed schedule and missed it, ending in the UEFI shell. Now detected from raw frames (about 600 lit pixels within the top rows) and a key is pressed only while it is shown ✅ |
| Bare `terminal-windows` in a pseudo-terminal with an existing running VM | ✅ opens the viewer, Ctrl+] exits, VM keeps running |
| Bare `terminal-windows` without a terminal | ✅ prints help |
| `view` / bare `terminal-windows` in a terminal without Kitty graphics (real report: 0.1.2 printed the probe codes as text and failed) | Fixed in 0.1.3: Apple Terminal and the VS Code terminal are recognised from `TERM_PROGRAM` and no probe is sent; `up` still sets up and boots, exits 0 and names an installed supported terminal. ✅ verified in a pseudo-terminal with `TERM_PROGRAM=Apple_Terminal`; other unknown terminals still rely on the live probe and now get the same guidance on failure |
| Microsoft download step inside `up` | ⏳ not run (needs the browser click); the same code path was measured with `image download` |
| Homebrew installation step inside `up` | ⏳ not run (QEMU and swtpm were already installed) |

## Measured record: Secure Boot firmware (2026-11-05)

Windows 11 Setup refused to continue with "This PC must support Secure Boot". Cause and fix: [firmware/README.md](../firmware/README.md).

| Firmware | Under HVF | Windows Setup requirements |
|---|---|---|
| Homebrew QEMU 11.1.3 default | ✅ boots | ❌ no Secure Boot |
| Debian AAVMF 2025.02 / 2026.08 (both secboot and no-secboot) | ❌ hangs at "Start boot option" (one vCPU at 100 %); fine under TCG | - |
| UTM 5.0.8 secure | ✅ firmware runs, ❌ no ramfb, so with virtio-gpu the Windows screen freezes | - |
| **Own build** (edk2 from QEMU 11.1.3 sources + `SECURE_BOOT_ENABLE`) | ✅ Windows Setup boots | ✅ passes (with TPM `ppi=off`) |
| The same build compiled by GitHub Actions (macos-14), shipped in release v0.1.1 and installed with `firmware install` | ✅ Windows Setup boots (bytes differ from the local build, behaviour is the same) | confirmed with the local build |

Environment change: during this work `brew install lld` upgraded Homebrew's LLVM to 23, which broke Homebrew's Rust (1.96); `brew reinstall rust` repaired it and moved Rust to 1.99.1. All tests, clippy and fmt were re-checked on 1.99.1.

## Measured record: Windows ISO download (2026-11-05)

| Item | Result |
|---|---|
| Parsing the official page (ARM64 `3816`, x64 `3813`, version 26H2, SHA-256 for 38 languages) | ✅ |
| SKU API (language list) | ✅ |
| Download link API | ❌ rejected by Microsoft's Sentinel (`ErrorSettings.SentinelReject`); not worked around, so the browser fallback is used |
| Browser fallback: instructions, Downloads watcher, SHA-256 verification, move into the cache | ✅ completed with a real 7.90 GiB Korean ARM64 ISO; the official hash matched |
| SHA-256 speed | debug build 15 MiB/s, then 186 MiB/s (about 12x) with `profile.dev.package.sha2 opt-level=3`; release 266 MiB/s |
| `image import` of a forged file | ✅ rejected |
| Resumable (Range) download, watcher, cache | ✅ automated tests against a local server |

## Automated tests (2026-11-05)

`cargo test --workspace`: 121 passed, 0 failed (Apple Silicon, Rust 1.99.1). CI runs the tests on macOS (arm64), Linux and Windows, and compiles the guest agent for Windows x64 and ARM64. `cargo clippy --workspace --all-targets -D warnings` and `cargo fmt --check` pass.
These tests cover protocol, command-line construction, mock QMP and mock backends. **They are not evidence that a VM was booted or Windows was controlled.**

The guest agent's `cfg(windows)` code (`win32.rs`) now compiles and lints in CI for both Windows targets, but has **never been run inside a Windows VM**.

## Assumptions still to be confirmed (update when measured)

| Assumption | Status |
|---|---|
| Homebrew `edk2-aarch64` firmware has the Secure Boot feature flag | ✅ it does not (only `verbose-static`); Windows 11 refuses to install with it, so `create` now fails early |
| HVF with `q35,smm=on` (Secure Boot firmware on Intel Macs) works | ⏳ |
| `-cpu max` and `kernel-irqchip=off` are sensible defaults for WHPX | ⏳ |
| Windows ARM64 Setup recognises a `usb-storage` CD | ✅ |
| QEMU `screendump format=png` (7.1+) | ✅ PNG works on QEMU 11.1.3 (PPM fallback is not measured) |
| `-chardev pipe` server and QMP on QEMU for Windows | ⏳ |
| virtio-serial throughput | ⏳ benchmark planned |
| Windows Setup passes the TPM check with `ppi=off` | ✅ (requirement screen passed) |
| Windows 11 networking without a virtio driver on ARM64 | ⏳ to be tested in OOBE |

## Intentionally unsupported

- x64 Windows on Apple Silicon (TCG emulation): refused
- ARM64 Windows on x64 hosts: refused
- Inline graphics inside tmux or screen: native-window fallback
