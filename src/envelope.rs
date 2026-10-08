//! Uniform JSON envelope used by every subcommand.
//!
//! Success: `{"ok":true,"data":...,"hint":null}`
//! Failure: `{"ok":false,"error":{"code":...,"message":...},"hint":"..."}`

use serde_json::{json, Value};
use std::fmt;

/// Successful command output: payload plus an optional next-step hint.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub data: Value,
    pub hint: Option<String>,
}

impl Output {
    pub fn new(data: Value) -> Self {
        Self { data, hint: None }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// A failure that always carries a machine-readable code and a next-step hint.
#[derive(Debug, Clone, PartialEq)]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub hint: String,
}

impl AppError {
    pub fn new(code: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.into(),
        }
    }

    /// Wrap an I/O error with context about what was being done.
    pub fn io(what: &str, err: std::io::Error) -> Self {
        let hint = match err.kind() {
            std::io::ErrorKind::NotFound => {
                "Check the path. Relative paths are resolved from the current directory."
            }
            std::io::ErrorKind::PermissionDenied => {
                "Permission denied. Choose a path you can write to, e.g. under your home directory."
            }
            _ => "Check the path and try again.",
        };
        Self::new("io_error", format!("{what}: {err}"), hint)
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

pub type CmdResult = Result<Output, AppError>;

/// Build the envelope JSON value for a command result.
pub fn envelope(res: &CmdResult) -> Value {
    match res {
        Ok(out) => json!({ "ok": true, "data": out.data, "hint": out.hint }),
        Err(e) => json!({
            "ok": false,
            "error": { "code": e.code, "message": e.message },
            "hint": e.hint,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_envelope_has_fixed_key_order() {
        let res: CmdResult = Ok(Output::new(json!({"x": 1})));
        let s = serde_json::to_string(&envelope(&res)).unwrap();
        assert_eq!(s, r#"{"ok":true,"data":{"x":1},"hint":null}"#);
    }

    #[test]
    fn error_envelope_always_has_hint() {
        let res: CmdResult = Err(AppError::new("bad", "boom", "try again"));
        let s = serde_json::to_string(&envelope(&res)).unwrap();
        assert_eq!(
            s,
            r#"{"ok":false,"error":{"code":"bad","message":"boom"},"hint":"try again"}"#
        );
    }
}
