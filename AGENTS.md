# AGENTS.md: instructions for coding agents working on terminal-windows

Rules for coding agents (Claude Code, Codex, ...) working in this repository. The plan in `docs/PLAN.md` takes precedence.

## Before you work
- Check the current phase and its exit criteria in the phase table of `docs/PLAN.md` (section 7). Do not start P8 before P2c, P4 and P6 are done.
- After changes always run: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## Must-follow rules
- **Never claim support without measurement.** `docs/compatibility.md` records only results that were actually run (date, host, command). Compiling is not support.
- Use `Support::Supported` only for capabilities verified by a real probe; everything else is `Unverified` or `Unsupported`.
- No fake tests, no tests that always pass, no empty placeholder crates.
- Lifecycle logic lives only in `tw_core::service::VmService`. Do not duplicate it in the CLI or MCP server.
- External process arguments are built only by the pure functions in `tw-qemu::cmdline`. No shell, and never build commands from guest-supplied data.
- No unbounded queues or buffers (state the limit). Do not add network listeners.
- No hard-coded personal paths. Override with `TERMINAL_WINDOWS_HOME`, `TW_QEMU_DIR`, `TW_SWTPM`.
- No code that bypasses Windows requirements (TPM, Secure Boot, RAM, activation, OOBE).
- Do not commit VM disks, ISOs, keys or large binaries. Push to a remote only with explicit permission.
- When adapting upstream code, record the source in the file header and in `docs/PROVENANCE.md`.
- All Markdown, code comments, CLI messages and release text are written in English.

## Commits
Small units with the prefixes `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `build:`, `ci:`.

## Notes about the test machine
- The repository may live in a synced folder such as iCloud, so build output goes to `target.nosync/` (`.cargo/config.toml`).
- The `cfg(windows)` code of the guest agent only compiles where a Windows target exists (the CI `windows-agent` job).
