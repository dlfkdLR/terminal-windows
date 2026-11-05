//! QEMU virtualization backend.
//!
//! * [`detect`] — find QEMU, qemu-img and swtpm; probe accelerators by actually starting them.
//! * [`firmware`] — select UEFI firmware from QEMU's firmware descriptor JSON files.
//! * [`cmdline`] — pure functions that build QEMU/swtpm/qemu-img argument vectors.
//! * [`qmp`] — asynchronous QEMU Machine Protocol client.
//! * [`backend`] — [`tw_core::backend::VirtualizationBackend`] implementation.
//! * [`console`] — Plane H computer control over QMP (screendump + virtual tablet/keyboard).

pub mod backend;
pub mod cmdline;
pub mod console;
pub mod detect;
pub mod firmware;
pub mod image;
pub mod qmp;

pub use backend::QemuBackend;
