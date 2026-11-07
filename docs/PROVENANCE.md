# Code provenance

Upstream: [govijr/terminal-mac](https://github.com/govijr/terminal-mac) @ `ac787d577de58a3804d32c34b73af453e83f77d8`, MIT License, Copyright (c) 2026 Govijr. Original license text: [UPSTREAM-LICENSE-terminal-mac.txt](UPSTREAM-LICENSE-terminal-mac.txt).

No Swift source file was copied verbatim. The table lists items whose **algorithm or behaviour was adapted to Rust**; the header comment of each file also names the source and copyright.

| terminal-windows file | Upstream source | Kind | Changes |
|---|---|---|---|
| `crates/tw-input/src/terminal.rs` | `Sources/TerminalMacCore/TerminalInput.swift` | adapted | Added SS3 keys, CSI modifiers (`ESC[1;5A`) and `ESC[n~` function keys. Both upstream tests ported |
| `crates/tw-renderer/src/layout.rs` | `Sources/TerminalUI/TerminalMetrics.swift` (`ImageLayout`) | adapted | Same logic; guards against zero sizes |
| `crates/tw-renderer/src/metrics.rs` | same file (`TerminalMetricFilter`) | adapted | Added DA1 (Sixel detection); iTerm2 `ReportCellSize` and OSC 11 left out |
| `crates/tw-renderer/src/kitty.rs` | `Sources/TerminalMac/DesktopRenderer.swift` | idea borrowed | Double-buffered image ids (2/3), z-index, 4096-byte chunks. The shared-memory path was not ported; a temporary-file transport was added |
| `crates/tw-core/src/name.rs` | `Models.swift` `validateName` | adapted | Rejects Windows reserved device names (CON, NUL, ...) |
| `crates/tw-mcp/src/server.rs` | `Sources/TerminalMac/AgentServer.swift` | idea borrowed | Some tool names and wording, the observe, act, verify guidance. Strict schema validation, version negotiation and structuredContent are new |
| `crates/tw-core/src/store.rs` | `Models.swift` `Store`, `Setup.swift` `FileLock` | idea borrowed | Atomic saves, exclusive locks, keep partial artifacts instead of deleting |
| `crates/tw-cli/src/view.rs` | `Sources/TerminalMac/Viewer.swift` | idea borrowed | Alternate screen with SGR mouse, latest-frame-only rendering, Ctrl+G palette, Ctrl+] to leave. Frames come from the hypervisor console, not a guest helper |
| `firmware/build.sh` | none | new | Builds edk2 from QEMU's sources; independent of upstream |

All other code (QEMU backend, QMP, firmware descriptors, protocol v1, guest agent, media download, CLI) is newly written.

## Bundled third-party binaries

| Files | Source | License | Changes |
|---|---|---|---|
| `crates/tw-core/assets/netkvm-arm64/` (`netkvm.inf`, `netkvm.sys`, `netkvm.cat`, `netkvmp.exe`) | [virtio-win 0.1.302](https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/archive-virtio/virtio-win-0.1.302-1/virtio-win-0.1.302.iso), `NetKVM/w11/ARM64` (Red Hat / Fedora Project) | BSD-3-Clause, text in `LICENSE-virtio-win.txt`; also copied onto the answer-file CD as `LICENSE.TXT` | none (byte-identical; SHA-256 in `SOURCE.txt`) |

Purpose: the Windows 11 ARM64 install image has no inbox driver for any NIC QEMU can emulate, so OOBE stops at "Connect to a network". The Microsoft-signed NetKVM driver is placed in `$WinPEDriver$` on the answer-file CD (`tw_core::drivers`), which Windows Setup injects. Nothing is downloaded or modified at run time.
