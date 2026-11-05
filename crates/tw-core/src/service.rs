//! `VmService`: the single entry point used by the CLI, the MCP server and the
//! viewer, so lifecycle logic is implemented exactly once.

use crate::backend::{StartOptions, StopMode, VirtualizationBackend, VmState, VmStatus};
use crate::control::{ComputerControl, InteractiveConsole};
use crate::error::{ErrorCode, TwError, TwResult};
use crate::model::{MachineConfig, NewMachine};
use crate::name::VmName;
use crate::store::Store;
use std::sync::Arc;

#[derive(Clone)]
pub struct VmService {
    store: Store,
    backend: Arc<dyn VirtualizationBackend>,
}

impl VmService {
    pub fn new(store: Store, backend: Arc<dyn VirtualizationBackend>) -> Self {
        Self { store, backend }
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn backend(&self) -> &Arc<dyn VirtualizationBackend> {
        &self.backend
    }

    /// Creates a VM. On failure after the directory was created, the partial
    /// directory is kept (never silently deleted) and reported for diagnosis.
    pub async fn create(&self, request: NewMachine, host_cpus: u32, created_at: String) -> TwResult<MachineConfig> {
        if request.backend != self.backend.kind() {
            return Err(TwError::invalid("requested backend does not match the active backend"));
        }
        for iso in [&request.install_iso, &request.drivers_iso].into_iter().flatten() {
            if !iso.is_file() {
                return Err(TwError::not_found(format!("installation media not found: {}", iso.display())));
            }
        }
        let config = MachineConfig::new_windows11(request, host_cpus, created_at)?;
        if self.store.exists(&config.name) || self.store.paths(&config.name).dir.exists() {
            return Err(TwError::new(ErrorCode::AlreadyExists, format!("VM '{}' already exists", config.name)));
        }
        self.backend.preflight(&config).await?;
        let paths = self.store.create_dir(&config.name)?;
        let _lock = self.store.lock(&config.name)?;
        let result = async {
            self.backend.create(&config, &paths).await?;
            self.store.save(&config)
        }
        .await;
        result.map_err(|e| {
            let hint = format!(
                "The partially created VM was kept at {} for diagnosis. Remove it with `terminal-windows delete {} --force` before retrying.",
                paths.dir.display(),
                config.name
            );
            TwError { hint: Some(e.hint.clone().map(|h| format!("{h}\n{hint}")).unwrap_or(hint)), ..e }
        })?;
        Ok(config)
    }

    pub fn load(&self, name: &VmName) -> TwResult<MachineConfig> {
        self.store.load(name)
    }

    pub async fn list(&self) -> TwResult<Vec<(MachineConfig, VmStatus)>> {
        let mut out = Vec::new();
        for entry in self.store.list()? {
            let Ok(config) = entry else { continue };
            let paths = self.store.paths(&config.name);
            let status = self.backend.status(&config, &paths).await?;
            out.push((config, status));
        }
        Ok(out)
    }

    pub async fn status(&self, name: &VmName) -> TwResult<VmStatus> {
        let config = self.store.load(name)?;
        self.backend.status(&config, &self.store.paths(name)).await
    }

    pub async fn start(&self, name: &VmName, options: &StartOptions) -> TwResult<VmStatus> {
        let config = self.store.load(name)?;
        let _lock = self.store.lock(name)?;
        let paths = self.store.paths(name);
        let current = self.backend.status(&config, &paths).await?;
        if current.state != VmState::Stopped {
            return Ok(current);
        }
        self.store.prepare_runtime(&paths)?;
        self.backend.start(&config, &paths, options).await
    }

    pub async fn stop(&self, name: &VmName, mode: StopMode) -> TwResult<VmStatus> {
        let config = self.store.load(name)?;
        let paths = self.store.paths(name);
        self.backend.stop(&config, &paths, mode).await
    }

    pub async fn pause(&self, name: &VmName) -> TwResult<VmStatus> {
        let config = self.store.load(name)?;
        self.backend.pause(&config, &self.store.paths(name)).await
    }

    pub async fn resume(&self, name: &VmName) -> TwResult<VmStatus> {
        let config = self.store.load(name)?;
        self.backend.resume(&config, &self.store.paths(name)).await
    }

    /// Deletes a stopped VM. Refuses while it runs.
    pub async fn delete(&self, name: &VmName) -> TwResult<()> {
        let paths = self.store.paths(name);
        if !paths.dir.exists() {
            return Err(TwError::not_found(format!("VM '{name}' does not exist")));
        }
        let lock = self.store.lock(name)?;
        if let Ok(config) = self.store.load(name) {
            let status = self.backend.status(&config, &paths).await?;
            if status.state != VmState::Stopped {
                return Err(TwError::state(format!("VM '{name}' is {:?}; stop it before deleting", status.state))
                    .with_hint(format!("terminal-windows stop {name}")));
            }
        }
        self.store.delete(name, &lock)
    }

    /// Console for the interactive viewer. Requires a running or paused VM.
    pub async fn interactive_console(&self, name: &VmName) -> TwResult<Box<dyn InteractiveConsole>> {
        let config = self.store.load(name)?;
        let paths = self.store.paths(name);
        match self.backend.status(&config, &paths).await?.state {
            VmState::Stopped => Err(TwError::state(format!("VM '{name}' is stopped")).with_hint(format!("terminal-windows start {name}"))),
            _ => self.backend.interactive_console(&config, &paths).await,
        }
    }

    /// Hypervisor-plane control. Requires a running VM.
    pub async fn console(&self, name: &VmName) -> TwResult<Box<dyn ComputerControl>> {
        let config = self.store.load(name)?;
        let paths = self.store.paths(name);
        let status = self.backend.status(&config, &paths).await?;
        match status.state {
            VmState::Running => self.backend.console(&config, &paths).await,
            VmState::Paused => Err(TwError::state(format!("VM '{name}' is paused")).with_hint(format!("terminal-windows resume {name}"))),
            VmState::Stopped => Err(TwError::new(ErrorCode::InvalidState, format!("VM '{name}' is stopped"))
                .with_hint(format!("terminal-windows start {name}"))),
            other => Err(TwError::state(format!("VM '{name}' is {other:?}"))),
        }
    }
}
