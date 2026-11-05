# Host-to-guest protocol v1

Implementation: `crates/tw-protocol`. Guest-side use: `guest/windows-agent`.

## Transport

| Backend | Host endpoint | Guest device |
|---|---|---|
| QEMU (macOS/Linux hosts) | Unix socket `<runtime>/agent.sock` (directory 0700) | virtio-serial port `\\.\Global\com.terminal-windows.agent.0` |
| QEMU (Windows host) | named pipe `\\.\pipe\tw-<tag>-agent` | same |
| Hyper-V (planned) | hvsocket (service GUID) | AF_HYPERV socket |

None of these has a TCP/IP listener. The protocol only assumes an ordered byte stream.

## Frame

```text
u32 BE  len        (= 8 + 32 + payload length)
u64 BE  seq        (0 for the handshake, then 1, 2, ... per direction)
[32]    mac        HMAC-SHA256(session_key, dir || seq || payload); all zero during the handshake
[...]   payload    UTF-8 JSON (Message)
```

- `dir`: host to guest `0x01`, guest to host `0x02` (prevents reflection).
- Size limits (checked before allocating): 128 KiB for frames the guest receives, 16 MiB for frames the host receives, 16 KiB for handshake messages.
- MAC is verified before the sequence number. On failure the connection is discarded and re-established (recovery belongs to the layer above).

## Handshake (mutual authentication)

`K` is the per-VM 256-bit key (`<vm>/agent.key`, 0600; delivered to the guest through the tools ISO).

```text
H -> G  hello      { protocol: "terminal-windows", versions: [1], nonce: Nh }
G -> H  hello_ack  { version: 1, nonce: Ng, proof: HMAC(K, "tw-v1-ack"  || Nh || Ng), agent: {version, os, arch, capabilities} }
H -> G  ready      { proof: HMAC(K, "tw-v1-ready" || Ng || Nh) }
session_key = HMAC(K, "tw-v1-session" || Nh || Ng)
```

- With no common version the guest sends `goodbye` and exits; the host reports `VersionMismatch`.
- Each direction's proof is compared in constant time (`hmac::verify_slice`).
- A timeout covers the whole handshake.

## Messages

```jsonc
{"type":"request","id":7,"timeout_ms":5000,"action":"click","x":10,"y":20,"button":"right","count":2}
{"type":"response","id":7,"status":"ok","result":{"kind":"done"}}
{"type":"response","id":8,"status":"error","code":"secure_desktop","message":"..."}
{"type":"event","event":"session_changed","session":"locked"}
{"type":"goodbye","reason":"..."}
```

Actions: `status, screenshot{max_width,max_height}, move, click{button,count}, drag{to_x,to_y}, scroll{dx,dy}, type{text}, key{chord}, ui_tree{max_depth,max_nodes}, shutdown`. Unknown actions (for example `exec`) are rejected at deserialisation: **the protocol contains no action that runs arbitrary commands.**

Error codes: `invalid_argument, unsupported, secure_desktop, elevated_target, no_interactive_session, timeout, internal`.

## Timeouts

A **relative `timeout_ms`** is used instead of an absolute time (upstream's absolute deadline breaks when the guest clock drifts). The host fails a request if no response arrives within `timeout + 250 ms`. v1 allows one request in flight per connection.

## Coordinates

All coordinates are **physical pixels** of the guest's primary monitor (top-left origin). The agent is Per-Monitor-V2 DPI aware and converts to `SendInput` absolute coordinates (0-65535) with `tw_core::coords::normalize`.

## Security limits (stated honestly)

A virtio-serial port can be opened by any process in the guest, so malware that steals the key could impersonate the agent. Values reported through Plane G are therefore marked `guest-reported`, and the ground truth for the screen is the hypervisor capture (Plane H). The purpose of the authentication is to prevent confusion between VMs, impersonation by other local host processes, and replay, reordering and reflection.

## Tests

`cargo test -p tw-protocol`: round trip, wrong key (fails on both sides), unanswered handshake timeout, version mismatch, forged MAC, replay, reflection, oversized frame (rejected before allocation), unknown action rejected.
