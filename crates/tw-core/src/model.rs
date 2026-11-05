//! Persistent machine configuration (`machine.toml`) and its validation.

use crate::error::{TwError, TwResult};
use crate::host::Arch;
use crate::name::VmName;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    Qemu,
    HyperV,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkMode {
    /// User-mode NAT. The guest can reach the internet and possibly host/LAN
    /// services. This is NOT network isolation.
    #[default]
    Nat,
    /// No guest network device at all.
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GuestOs {
    Windows11,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub width: u32,
    pub height: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self { width: 1920, height: 1080 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineConfig {
    pub schema_version: u32,
    pub name: VmName,
    pub backend: BackendKind,
    pub guest_os: GuestOs,
    pub arch: Arch,
    pub cpus: u32,
    pub memory_mib: u64,
    pub disk_gib: u64,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub network: NetworkMode,
    /// UEFI with Secure Boot capable firmware. Required for Windows 11.
    pub uefi: bool,
    pub secure_boot: bool,
    /// Virtual TPM 2.0. Required for Windows 11.
    pub tpm: bool,
    /// Installation media attached as a CD-ROM until installation completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_iso: Option<PathBuf>,
    /// Optional driver media (for example virtio-win) attached as a second CD-ROM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drivers_iso: Option<PathBuf>,
    /// Install Windows automatically (answer file for partitions, edition and language). The
    /// license page and account setup stay with the user.
    #[serde(default)]
    pub unattended: bool,
    /// Microsoft's language name of the installation media (for example "Korean"), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_language: Option<String>,
    /// RFC 3339 creation time.
    pub created_at: String,
    /// Stable SMBIOS system UUID. Without it every QEMU VM reports the same
    /// all-zero UUID, which confuses Windows licensing and management tools.
    pub smbios_uuid: String,
}

/// Windows 11 minimum system requirements as published by Microsoft.
pub mod win11 {
    pub const MIN_CPUS: u32 = 2;
    pub const MIN_MEMORY_MIB: u64 = 4096;
    pub const MIN_DISK_GIB: u64 = 64;
}

/// RFC 4122 version 4 UUID from the OS random number generator.
pub fn random_uuid() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

pub const MAX_CPUS: u32 = 256;
pub const MAX_MEMORY_MIB: u64 = 1024 * 1024;
pub const MAX_DISK_GIB: u64 = 16 * 1024;

#[derive(Debug, Clone)]
pub struct NewMachine {
    pub name: VmName,
    pub backend: BackendKind,
    pub arch: Arch,
    pub cpus: Option<u32>,
    pub memory_mib: Option<u64>,
    pub disk_gib: Option<u64>,
    pub display: Option<DisplayConfig>,
    pub network: NetworkMode,
    pub install_iso: Option<PathBuf>,
    pub drivers_iso: Option<PathBuf>,
    pub unattended: bool,
    pub windows_language: Option<String>,
}

impl MachineConfig {
    /// Builds a validated Windows 11 configuration with defaults sized from the host.
    pub fn new_windows11(req: NewMachine, host_cpus: u32, created_at: String) -> TwResult<Self> {
        let default_cpus = (host_cpus / 2).clamp(win11::MIN_CPUS, 8);
        let config = Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            name: req.name,
            backend: req.backend,
            guest_os: GuestOs::Windows11,
            arch: req.arch,
            cpus: req.cpus.unwrap_or(default_cpus),
            memory_mib: req.memory_mib.unwrap_or(8192),
            disk_gib: req.disk_gib.unwrap_or(80),
            display: req.display.unwrap_or_default(),
            network: req.network,
            uefi: true,
            secure_boot: true,
            tpm: true,
            install_iso: req.install_iso,
            drivers_iso: req.drivers_iso,
            unattended: req.unattended,
            windows_language: req.windows_language,
            created_at,
            smbios_uuid: random_uuid(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Validates ranges and Windows 11 requirements. Requirements are enforced,
    /// never bypassed.
    pub fn validate(&self) -> TwResult<()> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(TwError::invalid(format!(
                "unsupported machine.toml schema_version {} (this build understands {})",
                self.schema_version, CONFIG_SCHEMA_VERSION
            )));
        }
        if !(1..=MAX_CPUS).contains(&self.cpus) {
            return Err(TwError::invalid(format!("cpus must be between 1 and {MAX_CPUS}")));
        }
        if !(256..=MAX_MEMORY_MIB).contains(&self.memory_mib) {
            return Err(TwError::invalid("memory is out of range"));
        }
        if !(1..=MAX_DISK_GIB).contains(&self.disk_gib) {
            return Err(TwError::invalid("disk size is out of range"));
        }
        if !(640..=7680).contains(&self.display.width) || !(480..=4320).contains(&self.display.height) {
            return Err(TwError::invalid("display must be between 640x480 and 7680x4320"));
        }
        if !is_uuid(&self.smbios_uuid) {
            return Err(TwError::invalid("smbios_uuid must be a UUID"));
        }
        if !self.display.width.is_multiple_of(8) {
            return Err(TwError::invalid("display width must be a multiple of 8"));
        }
        match self.guest_os {
            GuestOs::Windows11 => {
                let mut missing = Vec::new();
                if self.cpus < win11::MIN_CPUS {
                    missing.push(format!("at least {} CPUs", win11::MIN_CPUS));
                }
                if self.memory_mib < win11::MIN_MEMORY_MIB {
                    missing.push("at least 4 GiB of memory".to_string());
                }
                if self.disk_gib < win11::MIN_DISK_GIB {
                    missing.push(format!("at least {} GiB of disk", win11::MIN_DISK_GIB));
                }
                if !self.uefi || !self.secure_boot {
                    missing.push("UEFI firmware with Secure Boot capability".to_string());
                }
                if !self.tpm {
                    missing.push("TPM 2.0".to_string());
                }
                if !missing.is_empty() {
                    return Err(TwError::invalid(format!(
                        "Windows 11 requires {}. terminal-windows does not bypass Windows hardware requirements.",
                        missing.join(", ")
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn to_toml(&self) -> TwResult<String> {
        toml::to_string_pretty(self).map_err(|e| TwError::new(crate::ErrorCode::Internal, format!("serialize machine.toml: {e}")))
    }

    pub fn from_toml(text: &str) -> TwResult<Self> {
        let config: Self = toml::from_str(text).map_err(|e| TwError::invalid(format!("invalid machine.toml: {e}")))?;
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> NewMachine {
        NewMachine {
            name: VmName::parse("dev").unwrap(),
            backend: BackendKind::Qemu,
            arch: Arch::Aarch64,
            cpus: None,
            memory_mib: None,
            disk_gib: None,
            display: None,
            network: NetworkMode::Nat,
            install_iso: Some(PathBuf::from("/media/Win11_ARM64.iso")),
            drivers_iso: None,
            unattended: false,
            windows_language: None,
        }
    }

    #[test]
    fn defaults_meet_windows11_requirements_and_round_trip() {
        let config = MachineConfig::new_windows11(request(), 10, "2026-11-05T00:00:00Z".into()).unwrap();
        assert_eq!(config.cpus, 5);
        assert!(config.tpm && config.uefi && config.secure_boot);
        let text = config.to_toml().unwrap();
        assert_eq!(MachineConfig::from_toml(&text).unwrap(), config);
    }

    #[test]
    fn small_host_still_gets_minimum_cpus() {
        let config = MachineConfig::new_windows11(request(), 2, "t".into()).unwrap();
        assert_eq!(config.cpus, 2);
    }

    #[test]
    fn requirements_are_enforced_not_bypassed() {
        let mut req = request();
        req.memory_mib = Some(2048);
        req.disk_gib = Some(32);
        let err = MachineConfig::new_windows11(req, 8, "t".into()).unwrap_err();
        assert!(err.message.contains("4 GiB"));
        assert!(err.message.contains("64 GiB"));

        let mut config = MachineConfig::new_windows11(request(), 8, "t".into()).unwrap();
        config.tpm = false;
        assert!(config.validate().unwrap_err().message.contains("TPM 2.0"));
    }

    #[test]
    fn edited_toml_is_revalidated() {
        let config = MachineConfig::new_windows11(request(), 8, "t".into()).unwrap();
        let tampered = config.to_toml().unwrap().replace("tpm = true", "tpm = false");
        assert!(MachineConfig::from_toml(&tampered).is_err());
        let bad_name = config.to_toml().unwrap().replace("name = \"dev\"", "name = \"../x\"");
        assert!(MachineConfig::from_toml(&bad_name).is_err());
    }

    #[test]
    fn uuids_are_v4_and_unique() {
        let a = random_uuid();
        assert!(is_uuid(&a));
        assert_eq!(&a[14..15], "4");
        assert_ne!(a, random_uuid());
        assert!(!is_uuid("not-a-uuid"));
    }

    #[test]
    fn unknown_schema_version_is_rejected() {
        let config = MachineConfig::new_windows11(request(), 8, "t".into()).unwrap();
        let future = config.to_toml().unwrap().replace("schema_version = 1", "schema_version = 99");
        assert!(MachineConfig::from_toml(&future).unwrap_err().message.contains("schema_version"));
    }
}
