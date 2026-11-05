# Security model

terminal-windows is **not a hardened malware sandbox.** It will not be described as one without dedicated security testing.

## Trust boundaries

| Party | Trust | Basis |
|---|---|---|
| Host user and terminal-windows processes | trusted | the VM owner |
| QEMU and swtpm processes | trusted (same user) | started only with arguments the host generates |
| The guest OS and everything in it | **untrusted** | including the agent |
| MCP client (the agent) | partially trusted | can drive the guest only, no access to the host desktop |

## Defaults

- Host desktop input and capture APIs are not used. All input goes through the VM's virtual devices (Plane H) or the in-guest agent (Plane G).
- No shared folders (disabled at the feature level), no clipboard sharing, no credential forwarding, no home directory exposure.
- No network listeners: QMP, VNC and the agent channel use Unix sockets (directory 0700; owner, mode and symlink checks) or Windows named pipes. Only VNC on Windows hosts is an exception that uses `127.0.0.1` with a QEMU VNC password; it is documented that **other local users on the same host can see the port.**
- Storage: directories 0700; `machine.toml`, `agent.key` and `efivars.fd` 0600; atomic writes. On Windows the inherited ACL of `%LOCALAPPDATA%`.
- The viewer's frame files (raw guest screen contents) are written 0600 and removed when the viewer exits.
- The native window runs as its own process; `hide` only terminates a process whose command line is `terminal-windows window ...` (a recycled PID is never killed). Its log (`viewer.log`) stays empty unless `TW_WINDOW_DEBUG` is set, which logs forwarded input events, including keys.
- No host command is executed from guest data. QEMU, swtpm and qemu-img arguments come only from validated configuration and host-chosen paths (`tw-qemu::cmdline`; no shell; QEMU option commas escaped; swtpm rejects paths containing commas).
- Resource bounds: frame size, 32 KiB of text, scroll within ±10000, 1 MiB per QMP line, 64 buffered QMP events, 4 MiB per MCP message, 64 queued viewer inputs.
- Logs never contain guest strings, keys or passwords.
- Downloaded media and firmware are verified against SHA-256 values published by their source (Microsoft's page for ISOs; `SHA256SUMS` of the release for firmware) before they are used.

## Network modes

| Mode | Meaning |
|---|---|
| `nat` (default) | QEMU user-mode NAT. The guest can reach the internet and **may reach host and LAN services. This is not isolation.** |
| `offline` (`--offline`) | no guest NIC |

## Relation to Windows security boundaries

- The UAC secure desktop, the lock screen and Ctrl+Alt+Del cannot be driven by the guest agent (impossible, and no bypass is attempted).
- The hypervisor console (Plane H) is equivalent to the VM's physical keyboard and monitor. That is normal for the VM owner, but **an agent (MCP) operating sign-in, UAC or credential dialogs is risky**, so the MCP instructions tell agents to hand those screens to the user. Automatic refusal based on the `secure_desktop` state is planned for P6; it is not implemented yet because Plane H alone cannot tell which screen is showing.
- The agent is a user-session process with an always-visible tray icon, not a service. The user can inspect, stop and uninstall it (P4).
- No feature bypasses Windows activation or the TPM, Secure Boot, RAM or network-at-OOBE requirements. The Secure Boot firmware shipped here makes the VM genuinely Secure Boot capable; it does not fake the check.

## Threats and responses

| Threat | Response |
|---|---|
| Guest injects host commands | No execute action in the protocol; unknown actions rejected at deserialisation |
| Guest exhausts memory with huge frames | Length checked before allocation |
| Another local user connects to QMP | 0700 runtime directory with owner and mode verification |
| Symlink swap of the runtime directory | `symlink_metadata` check; refused if group or others have access |
| Replay, reflection, reordering | Direction-bound HMAC and strict sequence numbers |
| Malware in the guest impersonating the agent | Cannot be fully prevented. Ground truth is Plane H; reported values are labelled |
| Path escape through VM names | `VmName` validation (`..`, `/`, `\` and reserved device names rejected), directory name must match the config name |
| Other local users reading viewer frame files | Files created 0600, deleted on exit |
| Tampered ISO or firmware download | SHA-256 verification against the publisher's values |
