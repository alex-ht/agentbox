//! `calc <expr>`: floating-point arithmetic with a small, predictable grammar.
//!
//! Grammar (precedence low -> high):
//!   expr   := term (('+' | '-') term)*
//!   term   := unary (('*' | '/' | '%') unary)*
//!   unary  := ('-' | '+') unary | power
//!   power  := primary (('^' | '**') unary)?        right-associative
//!   primary:= number | name | name '(' args ')' | '(' expr ')'
//!
//! Hand-written instead of `evalexpr` so that `7/2` is 3.5 (no integer
//! division surprises) and errors can carry precise hints.

use crate::envelope::{AppError, CmdResult, Output};
use serde_json::json;

pub fn run(expr: &str) -> CmdResult {
    let value = eval(expr)?;
    Ok(Output::new(json!({
        "expr": expr,
        "result": number_json(value),
        "text": format_number(value),
    })))
}

pub fn eval(expr: &str) -> Result<f64, AppError> {
    let tokens = tokenize(expr)?;
    if tokens.is_empty() {
        return Err(err(
            "empty expression",
            "Example: `agentbox calc \"(120 - 95) / 95 * 100\"`.",
        ));
    }
    let mut p = Parser { tokens, pos: 0 };
    let v = p.expr()?;
    if p.tokens.get(p.pos) == Some(&Tok::Comma) {
        return Err(err(
            "unexpected `,`",
            "Remove thousands separators (write 1234, not 1,234); commas only separate function arguments.",
        ));
    }
    if p.pos < p.tokens.len() {
        return Err(err(
            format!("unexpected `{}`", p.tokens[p.pos]),
            "Check for a missing operator or unbalanced parentheses.",
        ));
    }
    if !v.is_finite() {
        return Err(err(
            "result is not a finite number",
            "Check for division by zero or overflow.",
        ));
    }
    Ok(v)
}

fn err(msg: impl Into<String>, hint: &str) -> AppError {
    AppError::new("calc_error", msg, hint)
}

/// Integers stay integers in JSON (when exactly representable); others are floats.
fn number_json(v: f64) -> serde_json::Value {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

pub fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        format!("{}", v as i64)
    } else {
        // Round away float noise like 0.1+0.2 = 0.30000000000000004.
        let s = format!("{:.12}", v);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

impl std::fmt::Display for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tok::Num(n) => write!(f, "{n}"),
            Tok::Ident(s) => write!(f, "{s}"),
            Tok::Op(c) => write!(f, "{c}"),
            Tok::LParen => write!(f, "("),
            Tok::RParen => write!(f, ")"),
            Tok::Comma => write!(f, ","),
        }
    }
}

fn tokenize(s: &str) -> Result<Vec<Tok>, AppError> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_')
                {
                    i += 1;
                }
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let mut j = i + 1;
                    if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                        j += 1;
                    }
                    if j < chars.len() && chars[j].is_ascii_digit() {
                        i = j;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                let text: String = chars[start..i].iter().filter(|&&c| c != '_').collect();
                let n = text.parse::<f64>().map_err(|_| {
                    err(
                        format!("bad number `{text}`"),
                        "Write numbers like 1234.5 or 1.2e6 (no thousands separators).",
                    )
                })?;
                out.push(Tok::Num(n));
            }
            'a'..='z' | 'A'..='Z' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Ident(
                    chars[start..i]
                        .iter()
                        .collect::<String>()
                        .to_ascii_lowercase(),
                ));
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                out.push(Tok::Op('^'));
                i += 2;
            }
            '+' | '-' | '*' | '/' | '%' | '^' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            '×' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '$' | '€' | '£' | '¥' => i += 1, // tolerate currency symbols
            other => {
                return Err(err(
                    format!("unexpected character `{other}`"),
                    "Use numbers, + - * / % ^, parentheses and functions like sqrt(), round(x, 2).",
                ))
            }
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expr(&mut self) -> Result<f64, AppError> {
        let mut v = self.term()?;
        while let Some(Tok::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, AppError> {
        let mut v = self.unary()?;
        while let Some(Tok::Op(op @ ('*' | '/' | '%'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.unary()?;
            if (op == '/' || op == '%') && r == 0.0 {
                return Err(err("division by zero", "Check the divisor."));
            }
            v = match op {
                '*' => v * r,
                '/' => v / r,
                _ => v % r,
            };
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<f64, AppError> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Ok(-self.unary()?)
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<f64, AppError> {
        let base = self.primary()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            let exp = self.unary()?;
            return Ok(base.powf(exp));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<f64, AppError> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(n),
            Some(Tok::LParen) => {
                let v = self.expr()?;
                match self.next() {
                    Some(Tok::RParen) => Ok(v),
                    _ => Err(err("missing `)`", "Balance the parentheses.")),
                }
            }
            Some(Tok::Ident(name)) => {
                if let Some(Tok::LParen) = self.peek() {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if let Some(Tok::RParen) = self.peek() {
                        self.pos += 1;
                    } else {
                        loop {
                            args.push(self.expr()?);
                            match self.next() {
                                Some(Tok::Comma) => continue,
                                Some(Tok::RParen) => break,
                                _ => {
                                    return Err(err(
                                        format!("bad arguments to {name}()"),
                                        "Separate arguments with commas and close with `)`.",
                                    ))
                                }
                            }
                        }
                    }
                    call(&name, &args)
                } else {
                    match name.as_str() {
                        "pi" => Ok(std::f64::consts::PI),
                        "e" => Ok(std::f64::consts::E),
                        _ => Err(err(
                            format!("unknown name `{name}`"),
                            "Known constants: pi, e. Functions need parentheses, e.g. sqrt(2).",
                        )),
                    }
                }
            }
            Some(t) => Err(err(
                format!("unexpected `{t}`"),
                "Check the operators around this token.",
            )),
            None => Err(err(
                "expression ends too early",
                "Complete the expression after the last operator.",
            )),
        }
    }
}

const FUNCS: &str =
    "sqrt abs round floor ceil ln log log2 log10 exp sin cos tan pow min max sum avg";

fn call(name: &str, a: &[f64]) -> Result<f64, AppError> {
    let need = |n: usize| -> Result<(), AppError> {
        if a.len() == n {
            Ok(())
        } else {
            Err(err(
                format!("{name}() takes {n} argument(s), got {}", a.len()),
                "Fix the number of arguments.",
            ))
        }
    };
    let v = match name {
        "sqrt" | "abs" | "floor" | "ceil" | "ln" | "log2" | "log10" | "exp" | "sin" | "cos"
        | "tan" => {
            need(1)?;
            let x = a[0];
            match name {
                "sqrt" => x.sqrt(),
                "abs" => x.abs(),
                "floor" => x.floor(),
                "ceil" => x.ceil(),
                "ln" => x.ln(),
                "log2" => x.log2(),
                "log10" => x.log10(),
                "exp" => x.exp(),
                "sin" => x.sin(),
                "cos" => x.cos(),
                _ => x.tan(),
            }
        }
        "log" => match a.len() {
            1 => a[0].log10(),
            2 => a[0].log(a[1]),
            _ => {
                return Err(err(
                    "log() takes 1 or 2 arguments",
                    "log(x) is base 10; log(x, base) for other bases.",
                ))
            }
        },
        "round" => match a.len() {
            1 => a[0].round(),
            2 => {
                let f = 10f64.powi(a[1] as i32);
                (a[0] * f).round() / f
            }
            _ => {
                return Err(err(
                    "round() takes 1 or 2 arguments",
                    "round(x) or round(x, digits).",
                ))
            }
        },
        "pow" => {
            need(2)?;
            a[0].powf(a[1])
        }
        "min" | "max" | "sum" | "avg" => {
            if a.is_empty() {
                return Err(err(
                    format!("{name}() needs at least one argument"),
                    "Example: max(3, 7, 5).",
                ));
            }
            match name {
                "min" => a.iter().copied().fold(f64::INFINITY, f64::min),
                "max" => a.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                "sum" => a.iter().sum(),
                _ => a.iter().sum::<f64>() / a.len() as f64,
            }
        }
        _ => {
            return Err(err(
                format!("unknown function `{name}`"),
                &format!("Available: {FUNCS}."),
            ))
        }
    };
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        eval(s).unwrap()
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(ev("1 + 2 * 3"), 7.0);
        assert_eq!(ev("(1 + 2) * 3"), 9.0);
        assert_eq!(ev("2 ^ 3 ^ 2"), 512.0);
        assert_eq!(ev("2 ** 10"), 1024.0);
        assert_eq!(ev("-2 ^ 2"), -4.0);
        assert_eq!(ev("10 - 4 - 3"), 3.0);
        assert_eq!(ev("7 / 2"), 3.5);
        assert_eq!(ev("7 % 4"), 3.0);
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(ev("sqrt(16) + abs(-2)"), 6.0);
        assert_eq!(ev("round(2.71828, 2)"), 2.72);
        assert_eq!(ev("max(1, 9, 4) - min(5, 2)"), 7.0);
        assert_eq!(ev("avg(2, 4, 6)"), 4.0);
        assert_eq!(ev("log(1000)"), 3.0);
        assert!((ev("pi") - std::f64::consts::PI).abs() < 1e-12);
        assert_eq!(ev("1.5e3 + 1_000"), 2500.0);
        assert_eq!(ev("$120 × 3"), 360.0);
    }

    #[test]
    fn errors_have_hints() {
        for bad in ["1 / 0", "2 +", "(1 + 2", "foo(1)", "1,234 * 2", "3 # 4", ""] {
            let e = eval(bad).unwrap_err();
            assert_eq!(e.code, "calc_error", "{bad}");
            assert!(!e.hint.is_empty(), "{bad}");
        }
    }

    #[test]
    fn formats_results() {
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(42.0), "42");
        let out = run("(120 - 95) / 95 * 100").unwrap();
        assert_eq!(out.data["text"], "26.315789473684");
        let out = run("6 * 7").unwrap();
        assert_eq!(out.data["result"], 42);
    }
}
