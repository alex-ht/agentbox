//! Uniform JSON envelope used by every subcommand.
//!
//! Success: `{"ok":true,"data":...,"hint":null}`
//! Failure: `{"ok":false,"error":{"code":...,"message":...},"hint":"..."}`

use serde_json::{json, Map, Value};
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
    /// Extra keys merged into `error` after `code` and `message`.
    /// Boxed so `AppError` stays small enough for `Result`.
    pub details: Option<Box<Value>>,
}

impl AppError {
    pub fn new(code: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.into(),
            details: None,
        }
    }

    /// Attach extra error fields (for example the known official URL when a fetch is blocked).
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(Box::new(details));
        self
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
        Err(e) => {
            let mut error = Map::new();
            error.insert("code".into(), json!(e.code));
            error.insert("message".into(), json!(e.message));
            if let Some(extra) = e.details.as_deref().and_then(Value::as_object) {
                for (k, v) in extra {
                    if k != "code" && k != "message" {
                        error.insert(k.clone(), v.clone());
                    }
                }
            }
            json!({
                "ok": false,
                "error": Value::Object(error),
                "hint": e.hint,
            })
        }
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

    #[test]
    fn error_details_follow_code_and_message() {
        let res: CmdResult = Err(AppError::new("fetch_blocked", "nope", "retry")
            .with_details(json!({"id": "org:nhi", "official_url": "https://www.nhi.gov.tw"})));
        let s = serde_json::to_string(&envelope(&res)).unwrap();
        assert_eq!(
            s,
            r#"{"ok":false,"error":{"code":"fetch_blocked","message":"nope","id":"org:nhi","official_url":"https://www.nhi.gov.tw"},"hint":"retry"}"#
        );
    }
}
