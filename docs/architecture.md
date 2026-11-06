# Architecture

## Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `tw-core` | VM names; `machine.toml` model and validation (Windows 11 requirements); host and architecture policy; store (locks, atomic saves, permissions); errors and exit codes; coordinate conversion; the `VirtualizationBackend`, `ComputerControl` and `InteractiveConsole` traits; **`VmService`** | none |
| `tw-input` | Terminal input parser (ported from upstream); key chords to QEMU qcodes and Windows VK codes; US-ASCII key mapping | none |
| `tw-protocol` | Guest protocol v1: framing, mutually authenticated handshake, HMAC session, replay protection | none |
| `tw-qemu` | QEMU detection and accelerator probing, firmware descriptors, command-line construction, QMP client, `QemuBackend`, Plane H console | core, input |
| `tw-renderer` | Layout, terminal reply filter, Kitty encoder, terminal capability decision | none |
| `tw-media` | Microsoft download page and API client, resumable download, SHA-256 verification, ISO cache, Downloads watcher | core |
| `tw-mcp` | MCP stdio server (`Target` trait, implemented for `VmService`) | core |
| `tw-cli` | The `terminal-windows` binary: commands, guided `up` flow with settings questionnaire, terminal viewer, native window viewer (macOS and Windows), image and firmware management | all of the above |
| `tw-agent` (`guest/windows-agent`) | Windows guest agent | core, input, protocol |

The original prompt's `tw-transport` and `tw-security` gained too little as separate crates and were folded into `tw-protocol` and `tw-core::secure`. `tw-hyperv` will be added when its code exists (no empty crates).

## One service layer

```text
tw-cli --+
tw-mcp --+--> VmService --> Arc<dyn VirtualizationBackend> --> QemuBackend (later HyperVBackend)
viewer --+        |
                  +--> Store
```

The lifecycle rules (refuse deleting a running VM, run preflight before creating a directory, keep partial artifacts, locking) exist in exactly one place, `VmService`.

## Runtime layout (QEMU, macOS)

```text
<data>/machines/dev/     machine.toml, disk.qcow2, efivars.fd, firmware.json, tpm/, agent.key, runtime.log, swtpm.log
<data>/firmware/         managed UEFI firmware and its descriptor (terminal-windows firmware install)
<data>/images/           verified Windows ISOs and their records
<data>/TerminalWindows.app/  macOS only: bundle (Info.plist, icon, binary copy) the native viewer window runs from, so the Dock shows a proper name and icon
<runtime>/  (0700)       qmp.sock, qmpv.sock, vnc.sock, agent.sock, swtpm-ctrl.sock, qemu.pid
```

`<runtime>` is `<vm>/run` by default and falls back to `$TMPDIR/tw-<hash>` when the path would exceed the 104-byte Unix socket limit.

`qmp.sock` serves `action` and MCP; `qmpv.sock` is a second monitor reserved for the interactive viewer, because a QMP socket accepts one client at a time.

`start`:
1. Load the config, take the lock, check that the VM is stopped.
2. Check the recorded firmware (`firmware.json`; located again by file name after a package upgrade) and the attached media.
3. Start `swtpm socket --tpm2 --terminate` detached and wait for its control socket.
4. Start QEMU with `-daemonize`, so initialisation errors are collected synchronously into `runtime.log`.
5. Return the state from QMP `query-status`.

Windows hosts have no `-daemonize`, so QEMU is started detached and QMP (a named pipe) is polled.

## Control planes

| | Plane H (implemented) | Plane G (protocol and agent logic implemented; host connection not) |
|---|---|---|
| Path | QMP `screendump`, `input-send-event` | virtio-serial plus tw-protocol |
| Reach | firmware, setup, sign-in and UAC included | the signed-in user's desktop |
| Text | US-layout ASCII only (physical keys) | Unicode |
| Coordinates | framebuffer pixels to 0..32767 | physical pixels to 0..65535 |
| Trust | ground truth | guest-reported |

A future `HybridControl` will route text and UI-tree requests to Plane G and everything else to Plane H when an agent is connected (P4).

## Extension points

- New backend: implement `VirtualizationBackend` (`preflight`, `create`, `start`, `stop`, `pause`, `resume`, `status`, `console`, `interactive_console`, `capabilities`).
- New renderer transport: add a module to `tw-renderer` and a decision to `detect::decide`.
