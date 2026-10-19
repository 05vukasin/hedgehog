//! Minimal ordered JSON value + pretty printer. Keys keep insertion order, so the manifest is
//! byte-for-byte deterministic and diffs stay meaningful (no serde dependency needed).

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj() -> Self {
        Json::Obj(Vec::new())
    }

    /// Adds a key to an object (builder style). Panics on non-objects (programming error).
    pub fn with(mut self, key: &str, value: impl Into<Json>) -> Self {
        self.push(key, value);
        self
    }

    pub fn push(&mut self, key: &str, value: impl Into<Json>) {
        match self {
            Json::Obj(v) => v.push((key.to_string(), value.into())),
            _ => panic!("Json::push on a non-object"),
        }
    }

    pub fn pretty(&self) -> String {
        let mut out = String::new();
        write(self, 0, &mut out);
        out.push('\n');
        out
    }
}

impl From<bool> for Json {
    fn from(v: bool) -> Self {
        Json::Bool(v)
    }
}
impl From<i64> for Json {
    fn from(v: i64) -> Self {
        Json::Int(v)
    }
}
impl From<u32> for Json {
    fn from(v: u32) -> Self {
        Json::Int(v as i64)
    }
}
impl From<usize> for Json {
    fn from(v: usize) -> Self {
        Json::Int(v as i64)
    }
}
impl From<f64> for Json {
    fn from(v: f64) -> Self {
        Json::Num(v)
    }
}
impl From<f32> for Json {
    fn from(v: f32) -> Self {
        // Round to 3 decimals so f32 noise (0.8500000238) does not leak into the manifest.
        Json::Num((v as f64 * 1000.0).round() / 1000.0)
    }
}
impl From<&str> for Json {
    fn from(v: &str) -> Self {
        Json::Str(v.to_string())
    }
}
impl From<String> for Json {
    fn from(v: String) -> Self {
        Json::Str(v)
    }
}
impl From<&String> for Json {
    fn from(v: &String) -> Self {
        Json::Str(v.clone())
    }
}
impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Self {
        v.map_or(Json::Null, Into::into)
    }
}
impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(v: Vec<T>) -> Self {
        Json::Arr(v.into_iter().map(Into::into).collect())
    }
}

pub fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
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

/// Shortest round-trip form; integers without `.0`; non-finite → `null`.
pub fn num(v: f64, out: &mut String) {
    if !v.is_finite() {
        out.push_str("null");
    } else if v == v.trunc() && v.abs() < 1e15 {
        let _ = write!(out, "{}", v as i64);
    } else {
        let _ = write!(out, "{v}");
    }
}

fn indent(n: usize, out: &mut String) {
    for _ in 0..n {
        out.push_str("  ");
    }
}

/// Short scalar arrays are printed on one line to keep the manifest readable.
fn inline(v: &[Json]) -> bool {
    v.len() <= 8 && v.iter().all(|x| matches!(x, Json::Null | Json::Bool(_) | Json::Int(_) | Json::Num(_)) || matches!(x, Json::Str(s) if s.len() <= 40))
}

fn write(v: &Json, level: usize, out: &mut String) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Json::Num(f) => num(*f, out),
        Json::Str(s) => escape(s, out),
        Json::Arr(a) if a.is_empty() => out.push_str("[]"),
        Json::Arr(a) if inline(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(x, level, out);
            }
            out.push(']');
        }
        Json::Arr(a) => {
            out.push_str("[\n");
            for (i, x) in a.iter().enumerate() {
                indent(level + 1, out);
                write(x, level + 1, out);
                if i + 1 < a.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(level, out);
            out.push(']');
        }
        Json::Obj(o) if o.is_empty() => out.push_str("{}"),
        Json::Obj(o) => {
            out.push_str("{\n");
            for (i, (k, x)) in o.iter().enumerate() {
                indent(level + 1, out);
                escape(k, out);
                out.push_str(": ");
                write(x, level + 1, out);
                if i + 1 < o.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(level, out);
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_ordered_and_escaped() {
        let j = Json::obj().with("b", 1u32).with("a", "x\"y\n").with("n", Json::Null).with("f", 0.5).with("l", vec![1u32, 2]);
        assert_eq!(j.pretty(), "{\n  \"b\": 1,\n  \"a\": \"x\\\"y\\n\",\n  \"n\": null,\n  \"f\": 0.5,\n  \"l\": [1, 2]\n}\n");
        let mut s = String::new();
        num(f64::NAN, &mut s);
        num(3.0, &mut s);
        assert_eq!(s, "null3");
    }
}
