//! Virtualization backend abstraction and the capability model.

use crate::control::ComputerControl;
use crate::error::TwResult;
use crate::host::Arch;
use crate::model::{BackendKind, MachineConfig};
use crate::store::MachinePaths;
use async_trait::async_trait;
use serde::Serialize;

/// Whether a backend implements a capability. `Unverified` means code exists
/// but has not been validated on this host; it must never be shown as supported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Support {
    Supported,
    Unverified { note: String },
    Unsupported { reason: String },
}

impl Support {
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Support::Unsupported { reason: reason.into() }
    }
    pub fn unverified(note: impl Into<String>) -> Self {
        Support::Unverified { note: note.into() }
    }
    pub fn is_supported(&self) -> bool {
        matches!(self, Support::Supported)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    pub backend: BackendKind,
    pub guest_arch: Vec<Arch>,
    pub hardware_acceleration: Support,
    pub uefi: Support,
    pub tpm: Support,
    pub graphics: Support,
    pub headless: Support,
    pub networking: Support,
    pub guest_agent_transport: Support,
    pub snapshots: Support,
    pub shared_folders: Support,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VmState {
    Stopped,
    Starting,
    Running,
    Paused,
    ShuttingDown,
    /// The hypervisor process is alive but in an unexpected state.
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct VmStatus {
    pub name: String,
    pub state: VmState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopMode {
    /// ACPI power button; the guest decides when to power off.
    Graceful,
    /// Immediate power-off. Unsaved guest work can be lost.
    Force,
}

#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// Boot from the installation media even if the disk is bootable.
    pub boot_installer: bool,
}

#[async_trait]
pub trait VirtualizationBackend: Send + Sync {
    fn kind(&self) -> BackendKind;
    /// Capabilities on this host, after probing installed components.
    async fn capabilities(&self) -> Capabilities;
    /// Checks host prerequisites for `config` (binaries, accelerator, firmware,
    /// TPM) without touching the disk, so `create` fails before leaving data behind.
    async fn preflight(&self, config: &MachineConfig) -> TwResult<()>;
    /// Provisions backend artifacts (disk, firmware vars, TPM state) in an
    /// already-created, empty machine directory.
    async fn create(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<()>;
    async fn start(&self, config: &MachineConfig, paths: &MachinePaths, options: &StartOptions) -> TwResult<VmStatus>;
    async fn stop(&self, config: &MachineConfig, paths: &MachinePaths, mode: StopMode) -> TwResult<VmStatus>;
    async fn pause(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<VmStatus>;
    async fn resume(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<VmStatus>;
    async fn status(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<VmStatus>;
    /// Hypervisor-plane control of a running VM.
    async fn console(&self, config: &MachineConfig, paths: &MachinePaths) -> TwResult<Box<dyn ComputerControl>>;
}
