# terminal-mac upstream analysis

- Subject: `govijr/terminal-mac` @ `ac787d5` (v0.2.0 build 6), Swift 6, about 3,500 lines
- License: MIT (Copyright (c) 2026 Govijr). Copyright and permission notices must be kept when adapting; recorded in [PROVENANCE.md](PROVENANCE.md).
- Method: the whole source was read and judged, not just the README.

## 1. Module responsibilities and dependencies

```text
TerminalMac (executable, many macOS-only frameworks)
 +- CLI.swift             command dispatch, process split (__run), host mailbox calls
 +- VirtualMachine.swift  VZ configuration/installation/runtime, host control mailbox loop, NSWindow
 +- Viewer.swift          terminal viewer state machine, input queue (64), palette, chrome UI
 +- DesktopRenderer.swift frame decode/compose/encode (Kitty shared memory, Kitty PNG, iTerm2)
 +- AgentServer.swift     MCP stdio server (8 tools)
 +- Guest.swift           guest helper: mailbox polling, CGEvent input, permission UI
 +- GuestCapture.swift    ScreenCaptureKit stream, lz4 BGRA encoding
 +- GuestBootstrap.swift  in-guest self install/update/login item registration
 +- FirstRun.swift / RestoreDownload.swift   the `up` flow, IPSW download
 +- Entry.swift
TerminalUI       TerminalWriter (serial output queue), TerminalMetrics (layout, reply filter), Chrome
TerminalMacCore  Models (argument parser, Machine, Request/Reply, Store), Mailbox, TerminalInput, Setup (FileLock, readiness, plan)
CPhoneShim       C wrapper for shm_open/shm_unlink (variadic functions cannot be called from Swift)
```

Dependency direction: `TerminalMac -> TerminalUI, TerminalMacCore, CPhoneShim`. Core and UI are independent of each other.

## 2. Platform-independent algorithms (worth porting)

| Item | Location | Verdict |
|---|---|---|
| VM name rule `^[a-zA-Z0-9][a-zA-Z0-9_-]{0,47}$` | Models.swift | ported (plus Windows reserved names) |
| Terminal input parser (UTF-8 fragments, CSI, SGR mouse, Ctrl+] to quit) | TerminalInput.swift | ported and extended |
| Cell to pixel layout and cell-centre reverse mapping | TerminalMetrics.swift | ported |
| Terminal reply filter (CSI 14t/16t, Kitty APC replies) | TerminalMetrics.swift | ported and extended (DA1) |
| Render queue that keeps only the latest frame (backpressure) | DesktopRenderer.swift | idea ported (viewer) |
| Kitty double buffering (image ids 2/3) and z-index | DesktopRenderer.swift | ported |
| Send frames only on change; stop capture 15 s after the viewer leaves | GuestCapture.swift | idea ported (viewer sends only changed frames) |
| Keep partial artifacts, never overwrite | VirtualMachine.swift | principle ported |

## 3. macOS-specific dependencies (must be rewritten)

Virtualization.framework (VZ*), AppKit/NSWindow, ScreenCaptureKit, CGEvent and AX permissions, ServiceManagement, IPSW restore images, the `hw.model == VirtualMac*` guest guard, POSIX shared memory (`t=s`), Darwin `flock` and `openat`.

## 4. Protocol format

- Request: `request-<UUID>.json` = `{id, action, arguments: {String:String}, deadline: Date}`
- Response: `reply-<UUID>.json` = `{id, ok, values: {String:String}, error}`
- Frames: `image-<UUID>.bin` (lz4 BGRA) or `.png`; metadata as strings in `values`
- Polling: host 2 ms to 25 ms backoff; guest 3 ms (active) or 40 ms (idle)

**Assessment:** the file-boundary defences (O_NOFOLLOW, nlink == 1, size limits, atomic rename) are excellent. But (1) every value is a string map, so type validation is scattered over the consumers; (2) the absolute deadline is fragile against guest clock drift; (3) there is no version field; (4) there is no authentication (access to the shared folder is the authority); (5) **virtio-fs on QEMU needs the Linux-only virtiofsd**, so it cannot be reused on Windows or macOS hosts. Verdict: **redesign** ([protocol.md](protocol.md)).

## 5. Terminal graphics

Three transports: Kitty shared memory (raw RGBA, bypasses the pty), Kitty PNG chunks, iTerm2 OSC 1337. Shared-memory support is measured with a 1x1 probe image (`a=q,t=s` answered with `OK`). The image sits below cell backgrounds (z=-1073741825) so overlay UI stays visible. Output goes through one serial queue so escape sequences never interleave; `writeAndWait` provides backpressure.

Implication for Windows: Windows Terminal lacks Kitty support (it supports Sixel) and there is no POSIX shm, so a temporary-file or PNG-chunk path and a Sixel path are needed.

## 6. Input handling

The viewer enables SGR mouse (1002/1006) and the alternate screen (1049) and sends single, double and triple clicks as a `count`. The input queue holds at most 64 items. Consecutive text is merged into one `type`. The Cmd key is swallowed by the terminal, so Ctrl+G opens a palette of macOS shortcuts. On Windows the palette needs the Win key, Ctrl+Alt+Del and Alt+Tab (Ctrl+Alt+Del is only possible through Plane H).

## 7. MCP tools

`screenshot, click (button, clicks 1-3), move, drag (toX, toY), scroll (dy, dx), type, key, status`, each with a `screenshot: bool` option (capture after a 400 ms wait). The protocol version is echoed from the client (no negotiation) and argument validation is partial (`additionalProperties:false` is declared but not enforced server-side). Changes here: strict server-side validation, version negotiation, `structuredContent`, and lifecycle tools (list/start/stop).

## 8. VM lifecycle

`start` re-executes itself as `__run NAME`; that process owns the VZ VM and `runtime.lock`. Lifecycle commands travel through the `host-control` mailbox. With QEMU the QEMU process is the VM and QMP is the control channel, so no resident helper process is needed, which simplifies the design.

## 9. Guest to host communication

The guest helper sees only the shared folders `control` (rw) and `tools` (ro). Updates replace the helper when a new build appears in `tools`. On Windows the tools are provided as a per-VM ISO (CD), and auto-update is deferred to P8.

## 10. Security assumptions

No host input or screen APIs, no network listener, guest-supplied content is never executed, NAT is stated not to be isolation, guest permissions (TCC) are granted by the user. All kept. Windows adds explicit handling of the secure desktop, UIPI and Session 0 boundaries.

## 11. Existing tests

14 XCTest cases: setup plan, readiness, locking (including symlink refusal), name validation, argument parser, mailbox boundaries (traversal, symlinks, hard links, size, non-regular files), discarding expired and mismatched requests, round trip, timeout cleanup, two input parser tests. Upstream itself states that live guest verification is incomplete (docs/VALIDATION.md); that honesty principle is adopted as-is in compatibility.md.

## 12. Rewrite versus port summary

| Area | Verdict |
|---|---|
| VM engine (VZ to QEMU/Hyper-V) | rewrite |
| Guest transport (virtio-fs mailbox to virtio-serial/hvsocket with an authenticated protocol) | rewrite |
| Guest helper (ScreenCaptureKit/CGEvent to DXGI/SendInput/UIA) | rewrite |
| Primary screen and input path | **new: hypervisor console (QMP)**, a layer upstream does not have |
| Terminal input parser, layout, reply filter | port |
| Renderer transport policy (latest frame only, double buffering) | idea ported |
| MCP server | rewrite (same tool set plus extensions) |
| CLI argument parser | replaced by clap |
