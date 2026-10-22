//! Small text helpers shared by the tools: size caps, CSV, number and reference formatting.

use hh_core::model::{a1, Rect};

/// Hard cap of one tool result (~3k tokens).
pub const MAX_CHARS: usize = 12_000;

/// Cuts `text` to at most `limit` characters at a line break and appends the contract's
/// `… cut: <what>; ask for <how>` line.
pub fn cut(text: String, limit: usize, what: impl FnOnce(usize) -> String, how: &str) -> String {
    if text.len() <= limit {
        return text;
    }
    let reserve = 200;
    let mut end = limit.saturating_sub(reserve).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let keep = &text[..end];
    let keep = match keep.rfind('\n') {
        Some(nl) if nl > end / 2 => &keep[..nl],
        _ => keep,
    };
    let omitted = text[keep.len()..].lines().filter(|l| !l.trim().is_empty()).count();
    format!("{keep}\n… cut: {}; ask for {how}", what(omitted))
}

/// Shortens `s` to `max` characters (with `…`), newlines and tabs as spaces.
pub fn short(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let flat = flat.trim();
    if flat.chars().count() <= max {
        return flat.to_string();
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Number text: integers without `.0`, otherwise at most 12 significant digits (no float noise
/// such as 0.30000000000000004).
pub fn num(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let rounded: f64 = format!("{v:.11e}").parse().unwrap_or(v);
    hh_core::normalize::fmt_num(rounded)
}

/// `1,234` style thousands separators for counts.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// One CSV field (RFC 4180 quoting).
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn csv_row<S: AsRef<str>>(fields: &[S]) -> String {
    fields.iter().map(|f| csv_field(f.as_ref())).collect::<Vec<_>>().join(",")
}

/// `Sheet1!B3` or `'My Sheet'!B3`.
pub fn sheet_ref(sheet: &str, cell: &str) -> String {
    let plain = sheet.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && sheet.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if plain {
        format!("{sheet}!{cell}")
    } else {
        format!("'{}'!{cell}", sheet.replace('\'', "''"))
    }
}

pub fn range_a1(r: Rect) -> String {
    if r.top == r.bottom && r.left == r.right {
        a1(r.top, r.left)
    } else {
        format!("{}:{}", a1(r.top, r.left), a1(r.bottom, r.right))
    }
}

/// Column letters of a 0-based column.
pub fn letter(col: u32) -> String {
    a1(0, col).trim_end_matches('1').to_string()
}

/// SQL identifier as the catalog shows it: bare when it is a plain identifier, else `"quoted"`.
pub fn ident(name: &str) -> String {
    let plain = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain && !is_keyword(name) { name.to_string() } else { quote_ident(name) }
}

/// SQLite keywords (lowercase); a column with such a name must be quoted.
const KEYWORDS: &[&str] = &[
    "abort", "action", "add", "after", "all", "alter", "always", "analyze", "and", "as", "asc", "attach",
    "autoincrement", "before", "begin", "between", "by", "cascade", "case", "cast", "check", "collate", "column",
    "commit", "conflict", "constraint", "create", "cross", "current", "current_date", "current_time",
    "current_timestamp", "database", "default", "deferrable", "deferred", "delete", "desc", "detach", "distinct", "do",
    "drop", "each", "else", "end", "escape", "except", "exclude", "exclusive", "exists", "explain", "fail", "filter",
    "first", "following", "for", "foreign", "from", "full", "generated", "glob", "group", "groups", "having", "if",
    "ignore", "immediate", "in", "index", "indexed", "initially", "inner", "insert", "instead", "intersect", "into",
    "is", "isnull", "join", "key", "last", "left", "like", "limit", "match", "materialized", "natural", "no", "not",
    "nothing", "notnull", "null", "nulls", "of", "offset", "on", "or", "order", "others", "outer", "over", "partition",
    "plan", "pragma", "preceding", "primary", "query", "raise", "range", "recursive", "references", "regexp",
    "reindex", "release", "rename", "replace", "restrict", "returning", "right", "rollback", "row", "rows",
    "savepoint", "select", "set", "table", "temp", "temporary", "then", "ties", "to", "transaction", "trigger",
    "unbounded", "union", "unique", "update", "using", "vacuum", "values", "view", "virtual", "when", "where",
    "window", "with", "without",
];

pub fn is_keyword(name: &str) -> bool {
    KEYWORDS.contains(&name.to_ascii_lowercase().as_str())
}

/// Always-quoted SQL identifier.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Parses `B3`, `$B$3`, `B3:F12` (optionally `Sheet!` prefixed; the prefix is returned).
pub fn parse_range(s: &str) -> Option<(Option<String>, Rect)> {
    let s = s.trim();
    let (sheet, rng) = match s.rfind('!') {
        Some(i) => {
            let sh = s[..i].trim();
            let sh = sh.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')).map(|x| x.replace("''", "'")).unwrap_or(sh.to_string());
            (Some(sh), &s[i + 1..])
        }
        None => (None, s),
    };
    let rng = rng.replace('$', "");
    let mut parts = rng.split(':');
    let a = parse_cell(parts.next()?)?;
    let b = match parts.next() {
        Some(p) => parse_cell(p)?,
        None => a,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((
        sheet,
        Rect { top: a.0.min(b.0), left: a.1.min(b.1), bottom: a.0.max(b.0), right: a.1.max(b.1) },
    ))
}

/// `B3` → (row 2, col 1), 0-based.
pub fn parse_cell(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = s.split_at(split);
    if letters.is_empty() || letters.len() > 3 || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut col: u32 = 0;
    for c in letters.chars() {
        col = col * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    let row: u32 = digits.parse().ok()?;
    if row == 0 || row > 1_048_576 || col > 16_384 {
        return None;
    }
    Some((row - 1, col - 1))
}

/// Similarity of two names in 0..=1 for "did you mean" hints: case, spaces, punctuation and soft
/// hyphens ignored; a containment counts high.
pub fn similarity(a: &str, b: &str) -> f64 {
    let norm = |s: &str| -> Vec<char> { s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect() };
    let (x, y) = (norm(a), norm(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    if x == y {
        return 1.0;
    }
    let contains = |h: &[char], n: &[char]| n.len() >= 3 && h.windows(n.len()).any(|w| w == n);
    if contains(&x, &y) || contains(&y, &x) {
        return 0.85;
    }
    let d = levenshtein(&x, &y);
    1.0 - d as f64 / x.len().max(y.len()) as f64
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The `n` names most similar to `wanted` (similarity ≥ 0.5), best first.
pub fn closest<'a>(wanted: &str, names: impl IntoIterator<Item = &'a str>, n: usize) -> Vec<&'a str> {
    let mut scored: Vec<(f64, &str)> =
        names.into_iter().map(|c| (similarity(wanted, c), c)).filter(|(s, _)| *s >= 0.5).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out: Vec<&str> = Vec::new();
    for (_, c) in scored {
        if !out.contains(&c) {
            out.push(c);
        }
        if out.len() == n {
            break;
        }
    }
    out
}

/// B161: the names most similar to `wanted` for a "did you mean", best first, compared with case,
/// accents, spaces, punctuation and the catalog's grouping braces ignored; when one is clearly the
/// closest (the same name that way, or well ahead of the next), only that one.
pub fn clearly_closest<'a>(wanted: &str, names: impl IntoIterator<Item = &'a str>, n: usize) -> Vec<&'a str> {
    let w = hh_core::values::norm_key(wanted);
    let mut scored: Vec<(f64, &str)> = Vec::new();
    for c in names {
        let s = similarity(&w, &hh_core::values::norm_key(c));
        if s >= 0.5 && !scored.iter().any(|x| x.1 == c) {
            scored.push((s, c));
        }
    }
    // Stable: equal scores keep the names' order.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    if let [(a, best), (b, _), ..] = scored.as_slice()
        && ((*a >= 1.0 && *b < 1.0) || a - b >= 0.15)
    {
        return vec![best];
    }
    scored.into_iter().take(n).map(|x| x.1).collect()
}

/// Years 1950–2049 that stand alone in `s` (not part of a longer number).
pub fn years_in(s: &str) -> Vec<i32> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= b.len() {
        if b[i..i + 4].iter().all(u8::is_ascii_digit) {
            let before = i > 0 && (b[i - 1].is_ascii_digit() || b[i - 1] == b'.' || b[i - 1] == b',');
            let after = i + 4 < b.len()
                && (b[i + 4].is_ascii_digit()
                    || ((b[i + 4] == b'.' || b[i + 4] == b',') && b.get(i + 5).is_some_and(u8::is_ascii_digit)));
            if !before && !after {
                let y: i32 = s[i..i + 4].parse().unwrap_or(0);
                if (1950..=2049).contains(&y) {
                    out.push(y);
                }
            }
            // skip the whole digit run
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_adds_line() {
        let text = (0..2000).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let c = cut(text, 1000, |n| format!("{n} lines"), "a narrower range");
        assert!(c.len() <= 1000, "{}", c.len());
        assert!(c.lines().last().unwrap().starts_with("… cut: "));
        assert!(c.ends_with("ask for a narrower range"));
    }

    #[test]
    fn numbers() {
        assert_eq!(num(3.0), "3");
        assert_eq!(num(0.1 + 0.2), "0.3");
        assert_eq!(num(-1234.5), "-1234.5");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn ranges() {
        let (s, r) = parse_range("'My Sheet'!$B$3:A1").unwrap();
        assert_eq!(s.as_deref(), Some("My Sheet"));
        assert_eq!((r.top, r.left, r.bottom, r.right), (0, 0, 2, 1));
        assert!(parse_range("B0").is_none());
        assert_eq!(sheet_ref("Tabelle1", "B3"), "Tabelle1!B3");
        assert_eq!(sheet_ref("My Sheet", "B3"), "'My Sheet'!B3");
        assert_eq!(ident("group"), "\"group\"");
        assert_eq!(ident("Region"), "Region");
        assert_eq!(ident("2011"), "\"2011\"");
    }

    #[test]
    fn similar_names() {
        let names = ["De-zember / 20211)", "Oktober / 2022", "Region"];
        assert_eq!(closest("Dezember / 2021", names, 2)[0], "De-zember / 20211)");
        assert!(closest("xyz", names, 2).is_empty());
    }

    #[test]
    fn clearly_closest_first_and_alone() {
        let cols = [
            "RESTITUIÇÃO DO IMPOSTO / Comunidades Religiosas - DL 20/90 de 13/01",
            "RESTITUIÇÃO DO IMPOSTO / Instituições Particulares de Solidariedade Social - DL n.º 84/2017, de 21/07",
            "RESTITUIÇÃO DO IMPOSTO / Forças Armadas e de Segurança - DL n.º 84/2017, de 21/07",
            "RESTITUIÇÃO DO IMPOSTO / Associações e Corpos Bombeiros - DL n.º 84/2017, de 21/07",
        ];
        // Braces, accents and case aside it is the third column: only that one.
        let w = "restituicao do imposto / {Forcas Armadas e de Seguranca - DL n.º 84/2017, de 21/07}";
        assert_eq!(clearly_closest(w, cols, 4), [cols[2]]);
        // No clear winner: the closest first, several.
        let names = ["Umsatz 2023", "Umsatz 2024", "Region"];
        let c = clearly_closest("Umsatz 2025", names, 3);
        assert_eq!(c, ["Umsatz 2023", "Umsatz 2024"]);
    }

    #[test]
    fn years() {
        assert_eq!(years_in("Bericht 2017, Werte 2011-2025"), vec![2017, 2011, 2025]);
        assert!(years_in("12345 1,2017 3.2020").is_empty());
        assert_eq!(years_in("20211)"), Vec::<i32>::new());
    }
}
