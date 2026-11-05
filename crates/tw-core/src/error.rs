//! Structured errors with stable machine-readable codes and process exit codes.

use serde::Serialize;
use std::fmt;

/// Stable error codes. They appear in `--json` output and MCP results, so
/// renaming one is a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    NotFound,
    AlreadyExists,
    Busy,
    InvalidState,
    Unsupported,
    MissingDependency,
    Incompatible,
    Timeout,
    Backend,
    Protocol,
    GuestUnavailable,
    Io,
    Internal,
}

impl ErrorCode {
    /// Process exit code. 0 is success and 1 is reserved for unexpected panics.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::InvalidInput => 2,
            ErrorCode::NotFound => 3,
            ErrorCode::AlreadyExists => 4,
            ErrorCode::Busy => 5,
            ErrorCode::InvalidState => 6,
            ErrorCode::Unsupported | ErrorCode::Incompatible => 7,
            ErrorCode::MissingDependency => 8,
            ErrorCode::Timeout => 9,
            ErrorCode::GuestUnavailable => 10,
            ErrorCode::Backend | ErrorCode::Protocol => 11,
            ErrorCode::Io => 12,
            ErrorCode::Internal => 70,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidInput => "invalid_input",
            ErrorCode::NotFound => "not_found",
            ErrorCode::AlreadyExists => "already_exists",
            ErrorCode::Busy => "busy",
            ErrorCode::InvalidState => "invalid_state",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::MissingDependency => "missing_dependency",
            ErrorCode::Incompatible => "incompatible",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Backend => "backend",
            ErrorCode::Protocol => "protocol",
            ErrorCode::GuestUnavailable => "guest_unavailable",
            ErrorCode::Io => "io",
            ErrorCode::Internal => "internal",
        }
    }
}

/// An error with a code, a human message and an optional actionable hint.
#[derive(Debug, Clone, Serialize)]
pub struct TwError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

pub type TwResult<T> = Result<T, TwError>;

impl TwError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None }
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }
    pub fn state(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidState, message)
    }
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }
    pub fn backend(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Backend, message)
    }
    pub fn io(context: impl fmt::Display, err: std::io::Error) -> Self {
        Self::new(ErrorCode::Io, format!("{context}: {err}"))
    }
    pub fn exit_code(&self) -> i32 {
        self.code.exit_code()
    }
}

impl fmt::Display for TwError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "\nhint: {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TwError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_distinct_for_user_facing_classes() {
        let codes = [
            ErrorCode::InvalidInput,
            ErrorCode::NotFound,
            ErrorCode::AlreadyExists,
            ErrorCode::Busy,
            ErrorCode::InvalidState,
            ErrorCode::Unsupported,
            ErrorCode::MissingDependency,
            ErrorCode::Timeout,
            ErrorCode::GuestUnavailable,
            ErrorCode::Backend,
            ErrorCode::Io,
            ErrorCode::Internal,
        ];
        let mut seen = std::collections::HashSet::new();
        for code in codes {
            assert!(code.exit_code() > 1);
            assert!(seen.insert(code.exit_code()), "duplicate exit code for {code:?}");
        }
    }

    #[test]
    fn json_shape_is_stable() {
        let err = TwError::not_found("VM 'dev' does not exist").with_hint("terminal-windows create dev --iso ...");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "not_found");
        assert_eq!(json["hint"], "terminal-windows create dev --iso ...");
        assert_eq!(err.code.as_str(), "not_found");
    }
}
