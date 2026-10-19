//! Tiny JSON writer (no serde dependency in core).

use std::fmt::Write;

pub fn string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Number rounded to 4 decimals; NaN/inf → `null`.
pub fn num(out: &mut String, x: f64) {
    if x.is_finite() {
        let r = (x * 10_000.0).round() / 10_000.0;
        let _ = write!(out, "{r}");
    } else {
        out.push_str("null");
    }
}

pub fn opt_num(out: &mut String, x: Option<f64>) {
    match x {
        Some(v) => num(out, v),
        None => out.push_str("null"),
    }
}

/// Writes `"key":` (with a leading comma unless `first`).
pub fn key(out: &mut String, k: &str, first: bool) {
    if !first {
        out.push(',');
    }
    string(out, k);
    out.push(':');
}
