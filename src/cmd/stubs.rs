//! Subcommands planned for later versions. They parse their arguments and
//! return `not_implemented` with a workable fallback in the hint.

use crate::envelope::{AppError, CmdResult};

pub fn not_implemented(name: &str) -> CmdResult {
    let hint = match name {
        "quote" => "Fallback: `agentbox fetch \"https://stooq.com/q/l/?s=aapl.us&f=sd2t2ohlcv&h&e=csv\"` (replace aapl.us with the ticker).",
        "market" => "Fallback: `agentbox fetch \"https://gamma-api.polymarket.com/events?closed=false&limit=10\"` then `agentbox read doc:N --grep KEYWORD`.",
        _ => "See `agentbox --help` for implemented subcommands.",
    };
    Err(AppError::new(
        "not_implemented",
        format!(
            "`{name}` is planned but not implemented in v{}",
            env!("CARGO_PKG_VERSION")
        ),
        hint,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stub_has_specific_hint() {
        for name in ["quote", "market"] {
            let e = not_implemented(name).unwrap_err();
            assert_eq!(e.code, "not_implemented");
            assert!(e.hint.starts_with("Fallback:"), "{name}");
        }
    }
}
