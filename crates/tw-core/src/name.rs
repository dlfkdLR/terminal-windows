//! VM names. Ported from upstream `validateName` (terminal-mac Models.swift):
//! 1–48 ASCII letters, digits, `_` or `-`, starting with a letter or digit.
//! Additionally rejects Windows reserved device names, because the name is
//! used as a directory name on Windows hosts.

use crate::error::{TwError, TwResult};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VmName(String);

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4",
    "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

impl VmName {
    pub fn parse(raw: &str) -> TwResult<Self> {
        let bytes = raw.as_bytes();
        let valid_chars = !bytes.is_empty()
            && bytes.len() <= 48
            && bytes[0].is_ascii_alphanumeric()
            && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-');
        if !valid_chars {
            return Err(TwError::invalid(format!(
                "invalid VM name {raw:?}: use 1-48 letters, digits, '_' or '-', starting with a letter or digit"
            )));
        }
        if RESERVED.contains(&raw.to_ascii_lowercase().as_str()) {
            return Err(TwError::invalid(format!("invalid VM name {raw:?}: reserved on Windows")));
        }
        Ok(Self(raw.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for VmName {
    type Error = TwError;
    fn try_from(value: String) -> TwResult<Self> {
        Self::parse(&value)
    }
}
impl From<VmName> for String {
    fn from(value: VmName) -> Self {
        value.0
    }
}
impl fmt::Display for VmName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::str::FromStr for VmName {
    type Err = TwError;
    fn from_str(s: &str) -> TwResult<Self> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_names() {
        for ok in ["dev", "Win11", "a", "build_01", "x-y", &"a".repeat(48)] {
            assert!(VmName::parse(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn rejects_names_that_could_escape_the_store_or_break_windows() {
        for bad in ["", "-dev", "_dev", "../x", "a/b", "a\\b", "dev.", "with space", "한글", "nul", "COM1", &"a".repeat(49)] {
            assert!(VmName::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn serde_validates() {
        let parsed: Result<VmName, _> = serde_json::from_str("\"../etc\"");
        assert!(parsed.is_err());
    }
}
