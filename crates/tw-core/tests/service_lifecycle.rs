//! VmService lifecycle against a mock backend (no hypervisor required).

use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tw_core::backend::*;
use tw_core::control::ComputerControl;
use tw_core::host::Arch;
use tw_core::model::{BackendKind, MachineConfig, NetworkMode, NewMachine};
use tw_core::service::VmService;
use tw_core::store::{MachinePaths, Store};
use tw_core::{ErrorCode, TwError, TwResult, VmName};

#[derive(Default)]
struct Mock {
    state: Mutex<Option<VmState>>,
    fail_preflight: bool,
    fail_create: bool,
}

fn status(c: &MachineConfig, s: VmState) -> VmStatus {
    VmStatus { name: c.name.to_string(), state: s, pid: None, detail: None }
}

#[async_trait]
impl VirtualizationBackend for Mock {
    fn kind(&self) -> BackendKind {
        BackendKind::Qemu
    }
    async fn capabilities(&self) -> Capabilities {
        unimplemented!()
    }
    async fn preflight(&self, _: &MachineConfig) -> TwResult<()> {
        if self.fail_preflight {
            return Err(TwError::new(ErrorCode::MissingDependency, "no qemu"));
        }
        Ok(())
    }
    async fn create(&self, _: &MachineConfig, p: &MachinePaths) -> TwResult<()> {
        std::fs::write(&p.disk, b"disk").unwrap();
        if self.fail_create {
            return Err(TwError::backend("qemu-img failed"));
        }
        Ok(())
    }
    async fn start(&self, c: &MachineConfig, _: &MachinePaths, _: &StartOptions) -> TwResult<VmStatus> {
        *self.state.lock().unwrap() = Some(VmState::Running);
        Ok(status(c, VmState::Running))
    }
    async fn stop(&self, c: &MachineConfig, _: &MachinePaths, _: StopMode) -> TwResult<VmStatus> {
        *self.state.lock().unwrap() = Some(VmState::Stopped);
        Ok(status(c, VmState::Stopped))
    }
    async fn pause(&self, c: &MachineConfig, _: &MachinePaths) -> TwResult<VmStatus> {
        *self.state.lock().unwrap() = Some(VmState::Paused);
        Ok(status(c, VmState::Paused))
    }
    async fn resume(&self, c: &MachineConfig, p: &MachinePaths) -> TwResult<VmStatus> {
        self.start(c, p, &StartOptions::default()).await
    }
    async fn status(&self, c: &MachineConfig, _: &MachinePaths) -> TwResult<VmStatus> {
        Ok(status(c, self.state.lock().unwrap().unwrap_or(VmState::Stopped)))
    }
    async fn console(&self, _: &MachineConfig, _: &MachinePaths) -> TwResult<Box<dyn ComputerControl>> {
        Err(TwError::unsupported("mock has no console"))
    }
}

fn request(dir: &std::path::Path) -> NewMachine {
    let iso = dir.join("win.iso");
    std::fs::write(&iso, b"iso").unwrap();
    NewMachine {
        name: VmName::parse("dev").unwrap(),
        backend: BackendKind::Qemu,
        arch: Arch::Aarch64,
        cpus: None,
        memory_mib: None,
        disk_gib: None,
        display: None,
        network: NetworkMode::Nat,
        install_iso: Some(iso),
        drivers_iso: None,
    }
}

#[tokio::test]
async fn full_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = VmService::new(Store::new(tmp.path().join("home")), Arc::new(Mock::default()));
    let name = VmName::parse("dev").unwrap();
    svc.create(request(tmp.path()), 8, "t".into()).await.unwrap();
    assert_eq!(svc.create(request(tmp.path()), 8, "t".into()).await.unwrap_err().code, ErrorCode::AlreadyExists);
    assert_eq!(svc.status(&name).await.unwrap().state, VmState::Stopped);
    assert!(svc.console(&name).await.err().unwrap().hint.unwrap().contains("start dev"));
    assert_eq!(svc.start(&name, &StartOptions::default()).await.unwrap().state, VmState::Running);
    assert_eq!(svc.delete(&name).await.unwrap_err().code, ErrorCode::InvalidState, "running VM is never deleted");
    assert_eq!(svc.pause(&name).await.unwrap().state, VmState::Paused);
    assert_eq!(svc.console(&name).await.err().unwrap().code, ErrorCode::InvalidState);
    svc.stop(&name, StopMode::Graceful).await.unwrap();
    assert_eq!(svc.list().await.unwrap().len(), 1);
    svc.delete(&name).await.unwrap();
    assert!(svc.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn failed_preflight_leaves_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let svc = VmService::new(Store::new(home.clone()), Arc::new(Mock { fail_preflight: true, ..Default::default() }));
    assert_eq!(svc.create(request(tmp.path()), 8, "t".into()).await.unwrap_err().code, ErrorCode::MissingDependency);
    assert!(!home.join("machines/dev").exists());
}

#[tokio::test]
async fn failed_create_keeps_partial_data_for_diagnosis() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let svc = VmService::new(Store::new(home.clone()), Arc::new(Mock { fail_create: true, ..Default::default() }));
    let err = svc.create(request(tmp.path()), 8, "t".into()).await.unwrap_err();
    assert!(err.hint.unwrap().contains("kept at"));
    assert!(home.join("machines/dev/disk.qcow2").exists(), "never silently deleted");
    // And it can be removed explicitly even without machine.toml.
    svc.delete(&VmName::parse("dev").unwrap()).await.unwrap();
    assert!(!home.join("machines/dev").exists());
}

#[tokio::test]
async fn missing_iso_is_rejected_before_anything_happens() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = VmService::new(Store::new(tmp.path().join("home")), Arc::new(Mock::default()));
    let mut req = request(tmp.path());
    req.install_iso = Some(tmp.path().join("missing.iso"));
    assert_eq!(svc.create(req, 8, "t".into()).await.unwrap_err().code, ErrorCode::NotFound);
}
