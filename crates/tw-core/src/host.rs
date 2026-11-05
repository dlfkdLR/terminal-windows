//! Host detection and the host→guest architecture policy.

use crate::error::{ErrorCode, TwError, TwResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostOs {
    Macos,
    Windows,
    Linux,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
    /// Name used by Windows media and documentation.
    pub fn windows_name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x64",
            Arch::Aarch64 => "ARM64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HostInfo {
    pub os: HostOs,
    pub arch: Option<Arch>,
}

impl HostInfo {
    pub fn current() -> Self {
        let os = match std::env::consts::OS {
            "macos" => HostOs::Macos,
            "windows" => HostOs::Windows,
            "linux" => HostOs::Linux,
            _ => HostOs::Other,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Some(Arch::X86_64),
            "aarch64" => Some(Arch::Aarch64),
            _ => None,
        };
        Self { os, arch }
    }

    /// The only guest architecture that runs with hardware acceleration on this
    /// host. terminal-windows never falls back to cross-architecture emulation.
    pub fn native_guest_arch(&self) -> TwResult<Arch> {
        self.arch
            .ok_or_else(|| TwError::new(ErrorCode::Incompatible, format!("unsupported host CPU architecture: {}", std::env::consts::ARCH)))
    }

    /// Rejects a requested guest architecture that would require emulation.
    pub fn check_guest_arch(&self, guest: Arch) -> TwResult<()> {
        let native = self.native_guest_arch()?;
        if native == guest {
            return Ok(());
        }
        let hint = match (native, guest) {
            (Arch::Aarch64, Arch::X86_64) => {
                "Apple Silicon runs Windows 11 ARM64 with hardware acceleration; use an ARM64 ISO (--arch aarch64). x64 emulation is intentionally not offered because it is too slow to be usable."
            }
            _ => "Use installation media that matches the host CPU architecture.",
        };
        Err(TwError::new(
            ErrorCode::Incompatible,
            format!("a {} guest cannot be hardware-accelerated on a {} host", guest.windows_name(), native.windows_name()),
        )
        .with_hint(hint))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x64_guest_on_arm_host_is_rejected_with_hint() {
        let host = HostInfo { os: HostOs::Macos, arch: Some(Arch::Aarch64) };
        let err = host.check_guest_arch(Arch::X86_64).unwrap_err();
        assert_eq!(err.code, ErrorCode::Incompatible);
        assert!(err.hint.unwrap().contains("ARM64"));
        assert!(host.check_guest_arch(Arch::Aarch64).is_ok());
    }

    #[test]
    fn arm_guest_on_x64_host_is_rejected() {
        let host = HostInfo { os: HostOs::Windows, arch: Some(Arch::X86_64) };
        assert!(host.check_guest_arch(Arch::Aarch64).is_err());
        assert!(host.check_guest_arch(Arch::X86_64).is_ok());
    }

    #[test]
    fn unknown_host_arch_is_incompatible() {
        let host = HostInfo { os: HostOs::Other, arch: None };
        assert_eq!(host.native_guest_arch().unwrap_err().code, ErrorCode::Incompatible);
    }
}
