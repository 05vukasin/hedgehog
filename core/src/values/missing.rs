//! Missing-value tokens, Excel error strings and boolean codes (irregularities V8, V10, S9).

use super::text::{norm_key, trim};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MissingKind {
    /// Empty or only whitespace / invisible characters.
    Blank,
    /// Punctuation placeholder: `-`, `–`, `—`, `/`, `..`, `...`, `x`, `?`, `*`, `_`.
    Placeholder,
    /// A word meaning "not available": `n/a`, `NA`, `null`, `none`, `nema`, `nepoznato`, `nan`.
    NotAvailable,
    /// An Excel error value written as text or stored as an error: `#N/A`, `#REF!`, `#DIV/0!`.
    Error,
}

impl MissingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MissingKind::Blank => "blank",
            MissingKind::Placeholder => "placeholder",
            MissingKind::NotAvailable => "not_available",
            MissingKind::Error => "error",
        }
    }
}

/// Excel error literals (case-insensitive).
pub fn is_excel_error(s: &str) -> bool {
    const ERRORS: [&str; 11] = [
        "#n/a", "#ref!", "#div/0!", "#value!", "#name?", "#num!", "#null!", "#spill!", "#calc!", "#getting_data", "#n/a!",
    ];
    let t = trim(s);
    t.starts_with('#') && t.len() <= 14 && ERRORS.iter().any(|e| t.eq_ignore_ascii_case(e))
}

/// Classifies text that stands for a missing value; `None` for real content.
pub fn missing_token(s: &str) -> Option<MissingKind> {
    let t = trim(s);
    if t.is_empty() {
        return Some(MissingKind::Blank);
    }
    if t.len() > 24 {
        return None;
    }
    if is_excel_error(t) {
        return Some(MissingKind::Error);
    }
    // Pure punctuation placeholders: runs of one dash kind, slashes, dots, ?, *, _.
    let mut chars = t.chars().filter(|c| !c.is_whitespace());
    let first = chars.next()?;
    if matches!(first, '-' | '–' | '—' | '−' | '/' | '.' | '…' | '?' | '*' | '_' | '\\') && chars.all(|c| c == first) {
        return Some(MissingKind::Placeholder);
    }
    if matches!(t, "-/-" | "--/--" | "(-)" | "( - )" | "n.p.") {
        return Some(MissingKind::Placeholder);
    }
    let key = norm_key(t);
    match key.as_str() {
        "x" | "xx" | "xxx" => Some(MissingKind::Placeholder),
        "n/a" | "n.a." | "n. a." | "n.a" | "na" | "#na" | "n/d" | "n.d." | "nd" | "null" | "nil" | "none" | "nan"
        | "missing" | "unknown" | "not available" | "not applicable" | "no data" | "tbd" | "nema" | "nema podataka"
        | "bez podataka" | "nepoznato" | "nije poznato" | "nije dostupno" | "nedostupno" | "nema podatka" | "ø" => {
            Some(MissingKind::NotAvailable)
        }
        _ => None,
    }
}

/// Boolean codes: yes/no, true/false, y/n, da/ne (Latin & Cyrillic), ✓/✗.
/// `1`/`0` and `x` are deliberately excluded — decide those per column.
pub fn parse_bool(s: &str) -> Option<bool> {
    let t = trim(s);
    if t.len() > 8 {
        return None;
    }
    match t {
        "✓" | "✔" | "☑" => return Some(true),
        "✗" | "✘" | "☐" => return Some(false),
        _ => {}
    }
    match norm_key(t).as_str() {
        "yes" | "y" | "true" | "da" | "d" | "tacno" | "istina" | "ja" | "oui" | "si" => Some(true),
        "no" | "n" | "false" | "ne" | "netacno" | "nein" | "non" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tokens() {
        use MissingKind::*;
        let cases = [
            ("", Blank),
            ("  \u{A0}\u{200B}", Blank),
            ("-", Placeholder),
            ("--", Placeholder),
            (" – ", Placeholder),
            ("—", Placeholder),
            ("/", Placeholder),
            ("..", Placeholder),
            ("...", Placeholder),
            ("…", Placeholder),
            ("?", Placeholder),
            ("x", Placeholder),
            ("X", Placeholder),
            ("-/-", Placeholder),
            ("n/a", NotAvailable),
            ("N/A", NotAvailable),
            ("NA", NotAvailable),
            ("n.a.", NotAvailable),
            ("null", NotAvailable),
            ("NULL", NotAvailable),
            ("None", NotAvailable),
            ("nema", NotAvailable),
            ("НЕМА", NotAvailable),
            ("Nema podataka", NotAvailable),
            ("NaN", NotAvailable),
            ("#N/A", Error),
            ("#REF!", Error),
            ("#div/0!", Error),
            ("#VALUE!", Error),
            ("#NAME?", Error),
        ];
        for (s, k) in cases {
            assert_eq!(missing_token(s), Some(k), "{s:?}");
        }
        for s in ["0", "a", "-5", "Beograd", "n", "#hashtag", "x1", "- a", "nema kraja", "1/2"] {
            assert_eq!(missing_token(s), None, "{s:?}");
        }
        assert_eq!(MissingKind::Error.as_str(), "error");
    }

    #[test]
    fn excel_errors() {
        assert!(is_excel_error("#N/A"));
        assert!(is_excel_error(" #NUM! "));
        assert!(!is_excel_error("#"));
        assert!(!is_excel_error("#1"));
    }

    #[test]
    fn bools() {
        for s in ["Yes", "Y", "TRUE", "da", "DA", "Да", "✓"] {
            assert_eq!(parse_bool(s), Some(true), "{s:?}");
        }
        for s in ["no", "N", "False", "ne", "Не", "✗"] {
            assert_eq!(parse_bool(s), Some(false), "{s:?}");
        }
        for s in ["1", "0", "x", "maybe", ""] {
            assert_eq!(parse_bool(s), None, "{s:?}");
        }
    }
}
