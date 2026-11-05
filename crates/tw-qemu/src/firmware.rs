//! UEFI firmware selection from QEMU firmware descriptors
//! (`share/qemu/firmware/*.json`, format: QEMU `docs/interop/firmware.json`).
//!
//! Using descriptors instead of hard-coded file names keeps the backend
//! working across Homebrew, the Windows installer and Linux distributions, and
//! lets us know whether a firmware build is Secure Boot capable or needs SMM.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use tw_core::host::Arch;

#[derive(Debug, Deserialize)]
struct Descriptor {
    #[serde(default)]
    description: String,
    #[serde(rename = "interface-types")]
    interface_types: Vec<String>,
    mapping: Mapping,
    targets: Vec<Target>,
    #[serde(default)]
    features: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Mapping {
    device: String,
    executable: Option<FlashFile>,
    #[serde(rename = "nvram-template")]
    nvram_template: Option<FlashFile>,
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FlashFile {
    filename: String,
    #[serde(default = "raw")]
    format: String,
}

fn raw() -> String {
    "raw".into()
}

#[derive(Debug, Deserialize)]
struct Target {
    architecture: String,
}

/// A usable split-flash UEFI firmware (read-only code + per-VM variable store).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Firmware {
    pub description: String,
    pub code: PathBuf,
    pub vars_template: PathBuf,
    pub format: String,
    pub secure_boot: bool,
    pub enrolled_keys: bool,
    pub requires_smm: bool,
    pub source: PathBuf,
}

fn resolve(file: &str, descriptor: &Path) -> PathBuf {
    let path = PathBuf::from(file);
    if path.is_absolute() {
        return path;
    }
    // Relative names: try the directories above the descriptor directory
    // (`share/qemu` and `share`), since installers differ.
    let firmware_dir = descriptor.parent();
    let candidates: Vec<PathBuf> =
        firmware_dir.into_iter().flat_map(|d| d.ancestors().skip(1).take(2)).map(|base| base.join(&path)).collect();
    candidates.iter().find(|c| c.is_file()).cloned().or_else(|| candidates.into_iter().next()).unwrap_or(path)
}

/// Parses one descriptor. Returns `None` for descriptors that do not describe
/// split-flash UEFI for `arch`, or whose files are missing.
pub fn parse_descriptor(json: &str, source: &Path, arch: Arch) -> Option<Firmware> {
    let d: Descriptor = match serde_json::from_str(json) {
        Ok(d) => d,
        Err(e) => {
            tracing::debug!(source = %source.display(), error = %e, "ignoring unreadable firmware descriptor");
            return None;
        }
    };
    if !d.interface_types.iter().any(|t| t == "uefi") || d.mapping.device != "flash" {
        return None;
    }
    if d.mapping.mode.as_deref().is_some_and(|m| m != "split") {
        return None;
    }
    if !d.targets.iter().any(|t| t.architecture == arch.as_str()) {
        return None;
    }
    let code = d.mapping.executable?;
    let vars = d.mapping.nvram_template?;
    let has = |f: &str| d.features.iter().any(|x| x == f);
    // Confidential-computing-only builds cannot boot an ordinary guest.
    if has("amd-sev-snp") || has("intel-tdx") {
        return None;
    }
    let fw = Firmware {
        description: d.description,
        code: resolve(&code.filename, source),
        vars_template: resolve(&vars.filename, source),
        format: code.format,
        secure_boot: has("secure-boot"),
        enrolled_keys: has("enrolled-keys"),
        requires_smm: has("requires-smm"),
        source: source.to_path_buf(),
    };
    if !fw.code.is_file() || !fw.vars_template.is_file() {
        return None;
    }
    Some(fw)
}

/// Standard descriptor directories for a QEMU installed under `prefix`.
pub fn descriptor_dirs(prefix: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(prefix) = prefix {
        dirs.push(prefix.join("share/qemu/firmware"));
        // QEMU for Windows installs to <prefix>\share\firmware.
        dirs.push(prefix.join("share/firmware"));
    }
    for fixed in ["/etc/qemu/firmware", "/usr/share/qemu/firmware", "/opt/homebrew/share/qemu/firmware", "/usr/local/share/qemu/firmware"] {
        dirs.push(PathBuf::from(fixed));
    }
    dirs.dedup();
    dirs
}

/// All usable firmware for `arch`, best first: Secure Boot capable with
/// Microsoft keys enrolled, then Secure Boot capable, then plain UEFI.
/// Descriptor file names sort by priority prefix (`50-…`, `60-…`) as QEMU specifies.
pub fn discover(dirs: &[PathBuf], arch: Arch) -> Vec<Firmware> {
    let mut files: Vec<PathBuf> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_os_string()));
    let mut found: Vec<Firmware> =
        files.iter().filter_map(|p| std::fs::read_to_string(p).ok().and_then(|json| parse_descriptor(&json, p, arch))).collect();
    found.dedup_by(|a, b| a.code == b.code && a.vars_template == b.vars_template);
    // Stable sort keeps descriptor priority inside each class.
    found.sort_by_key(|f| match (f.secure_boot, f.enrolled_keys) {
        (true, true) => 0,
        (true, false) => 1,
        _ => 2,
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn descriptor(code: &str, vars: &str, arch: &str, features: &str) -> String {
        format!(
            r#"{{"description":"test {arch}","interface-types":["uefi"],
              "mapping":{{"device":"flash","executable":{{"filename":"{code}","format":"raw"}},
              "nvram-template":{{"filename":"{vars}","format":"raw"}}}},
              "targets":[{{"architecture":"{arch}","machines":["virt-*","pc-q35-*"]}}],
              "features":[{features}],"tags":[]}}"#
        )
    }

    #[test]
    fn prefers_secure_boot_with_enrolled_keys_and_resolves_relative_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let share = tmp.path().join("share");
        let fwdir = share.join("qemu/firmware");
        std::fs::create_dir_all(&fwdir).unwrap();
        for f in ["code.fd", "vars.fd", "sb-code.fd", "sb-vars.fd", "arm-code.fd", "arm-vars.fd"] {
            std::fs::write(share.join("qemu").join(f), b"x").unwrap();
        }
        let q = share.join("qemu");
        let p = |f: &str| q.join(f).display().to_string();
        write(&fwdir, "60-plain.json", &descriptor(&p("code.fd"), &p("vars.fd"), "x86_64", r#""acpi-s3""#));
        write(
            &fwdir,
            "50-sb.json",
            &descriptor(&p("sb-code.fd"), &p("sb-vars.fd"), "x86_64", r#""requires-smm","secure-boot","enrolled-keys""#),
        );
        write(&fwdir, "60-arm.json", &descriptor("qemu/arm-code.fd", "qemu/arm-vars.fd", "aarch64", ""));
        write(&fwdir, "70-broken.json", "{not json");
        write(&fwdir, "80-missing.json", &descriptor("/nonexistent/a.fd", "/nonexistent/b.fd", "x86_64", ""));

        let x86 = discover(std::slice::from_ref(&fwdir), Arch::X86_64);
        assert_eq!(x86.len(), 2);
        assert!(x86[0].secure_boot && x86[0].enrolled_keys && x86[0].requires_smm);
        assert!(!x86[1].secure_boot);

        let arm = discover(&[fwdir], Arch::Aarch64);
        assert_eq!(arm.len(), 1);
        assert_eq!(arm[0].code, share.join("qemu/arm-code.fd"));
    }

    #[test]
    fn non_uefi_and_memory_mapped_firmware_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let bios = r#"{"interface-types":["bios"],"mapping":{"device":"memory"},"targets":[{"architecture":"x86_64"}]}"#;
        assert!(parse_descriptor(bios, &tmp.path().join("a.json"), Arch::X86_64).is_none());
    }
}
