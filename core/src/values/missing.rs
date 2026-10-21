//! Missing-value tokens, Excel error strings and boolean codes (irregularities V8, V10, S9).

use super::text::{norm_key, trim};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MissingKind {
    /// Empty or only whitespace / invisible characters.
    Blank,
    /// Punctuation placeholder: `-`, `–`, `—`, `/`, `..`, `...`, `…`, `:`, `x`, `?`, `*`, `_`
    /// (`:` and `: c` / `:z` are Eurostat's "not available" with a flag).
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

/// Eurostat / statistical-office flag letters: b break in series, c confidential, d definition
/// differs, e estimated, f forecast, n not significant, p provisional, r revised, s estimate,
/// u low reliability, z not applicable.
pub(crate) const FLAG_LETTERS: &[u8] = b"bcdefnprsuz";

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
    if matches!(first, '-' | '–' | '—' | '−' | '/' | '.' | '…' | ':' | '?' | '*' | '_' | '\\') && chars.all(|c| c == first) {
        return Some(MissingKind::Placeholder);
    }
    // Eurostat: ":" with flag letters ("not available, confidential" `: c`, `:z`, `: bu`).
    if let Some(flags) = t.strip_prefix(':') {
        let flags = trim(flags);
        if (1..=3).contains(&flags.len()) && flags.bytes().all(|b| FLAG_LETTERS.contains(&b)) {
            return Some(MissingKind::Placeholder);
        }
    }
    if matches!(t, "-/-" | "--/--" | "(-)" | "( - )" | "n.p.") {
        return Some(MissingKind::Placeholder);
    }
    let key = norm_key(t);
    match key.as_str() {
        "x" | "xx" | "xxx" => Some(MissingKind::Placeholder),
        "n/a" | "n.a." | "n. a." | "n.a" | "na" | "#na" | "n/d" | "n.d." | "nd" | "null" | "nil" | "none" | "nan"
        | "missing" | "unknown" | "not available" | "not applicable" | "no data" | "tbd" | "nema" | "nema podataka"
        | "bez podataka" | "nepoznato" | "nije poznato" | "nije dostupno" | "nedostupno" | "nema podatka" | "ø"
        // de: keine Angabe, nicht verfügbar / bekannt; nl: niet beschikbaar; fr: non disponible /
        // communiqué; es: sin datos, no disponible; it: non disponibile; pl: brak danych.
        | "k.a." | "k. a." | "k.a" | "keine angabe" | "keine angaben" | "n.v." | "n.b." | "nicht verfugbar"
        | "niet beschikbaar" | "n.c." | "non disponible" | "non communique" | "s.d." | "sin datos" | "no disponible"
        | "non disponibile" | "b.d." | "brak danych" => {
            Some(MissingKind::NotAvailable)
        }
        _ => None,
    }
}

/// Why a cell in a numeric column holds text instead of a number, for the few kinds of text
/// that statistical tables put there on purpose. Such cells stay text (nothing is guessed), but
/// they do not count against the column being numeric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericAnnotation {
    /// Suppressed or confidential value: `c`, `[c]`, `(D)`, `confidential`, `suppressed`.
    Suppressed,
    /// A bound instead of a value: `<5`, `> 1000`, `≤ 0.5`, `(Z)` (less than half the unit).
    Censored,
    /// A pointer to a note: `see note`, `see footnote`.
    SeeNote,
}

/// Recognizes [`NumericAnnotation`] text (`None` for anything else, including missing tokens).
pub fn numeric_annotation(s: &str) -> Option<NumericAnnotation> {
    let t = trim(s);
    if t.is_empty() || t.len() > 24 {
        return None;
    }
    // US Census `(Z)`: less than half the unit shown.
    if t == "(Z)" {
        return Some(NumericAnnotation::Censored);
    }
    if let Some(rest) = t.strip_prefix(['<', '>', '≤', '≥', '‹', '›']) {
        let rest = rest.strip_prefix('=').unwrap_or(rest);
        let rest = trim(rest);
        if rest.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == ',')
            && super::number::parse_number(rest, super::number::NumberHint::default()).is_some()
        {
            return Some(NumericAnnotation::Censored);
        }
        return None;
    }
    if matches!(t, "c" | "C" | "[c]" | "[C]" | "(c)" | "(C)" | "(D)" | "(S)" | "[low]" | "[u]" | "F") {
        return Some(NumericAnnotation::Suppressed);
    }
    match norm_key(t).as_str() {
        "confidential" | "conf." | "suppressed" | "withheld" | "redacted" | "geheim" | "vertraulich" | "confidentiel"
        | "secret" | "secret statistique" | "riservato" | "confidencial" | "poverljivo" | "tajno" => {
            Some(NumericAnnotation::Suppressed)
        }
        "see note" | "see notes" | "see footnote" | "see footnotes" | "see comment" | "see text" | "siehe fussnote"
        | "siehe anmerkung" | "voir note" | "ver nota" | "vidi napomenu" => Some(NumericAnnotation::SeeNote),
        _ => None,
    }
}

/// [`missing_token`] plus the bracketed codes of statistical tables: `(X)` / `[x]` not applicable,
/// `(NA)` / `[NA]` not available, `[z]` not applicable (US Census, ONS). Used when typing table
/// columns; the cell features (and so the trained row model) keep seeing these as text.
pub fn table_missing_token(s: &str) -> Option<MissingKind> {
    missing_token(s).or_else(|| {
        matches!(trim(s), "(X)" | "(x)" | "[x]" | "[X]" | "(NA)" | "[NA]" | "[z]" | "[Z]").then_some(MissingKind::Placeholder)
    })
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
            (":", Placeholder),
            (": c", Placeholder),
            (":z", Placeholder),
            ("k.A.", NotAvailable),
            ("k. A.", NotAvailable),
            ("n.v.", NotAvailable),
            ("b.d.", NotAvailable),
            ("Keine Angabe", NotAvailable),
            ("#N/A", Error),
            ("#REF!", Error),
            ("#div/0!", Error),
            ("#VALUE!", Error),
            ("#NAME?", Error),
        ];
        for (s, k) in cases {
            assert_eq!(missing_token(s), Some(k), "{s:?}");
        }
        for s in ["0", "a", "-5", "Beograd", "n", "#hashtag", "x1", "- a", "nema kraja", "1/2", ": note", "c", "<5"] {
            assert_eq!(missing_token(s), None, "{s:?}");
        }
        assert_eq!(MissingKind::Error.as_str(), "error");
    }

    #[test]
    fn table_missing_tokens() {
        for s in ["(X)", "[x]", "(NA)", "[z]", "-", ":"] {
            assert_eq!(table_missing_token(s), Some(MissingKind::Placeholder), "{s:?}");
        }
        for s in ["(X)", "[x]", "(NA)", "(Z)"] {
            assert_eq!(missing_token(s), None, "{s:?}");
        }
        assert_eq!(table_missing_token("(Z)"), None);
        assert_eq!(numeric_annotation("(Z)"), Some(NumericAnnotation::Censored));
    }

    #[test]
    fn numeric_annotations() {
        use NumericAnnotation::*;
        for (s, k) in [
            ("<5", Censored),
            ("> 1000", Censored),
            ("<=0,5", Censored),
            ("≤ 10", Censored),
            ("c", Suppressed),
            ("[c]", Suppressed),
            ("(D)", Suppressed),
            ("Confidential", Suppressed),
            ("suppressed", Suppressed),
            ("see note", SeeNote),
            ("See footnote", SeeNote),
        ] {
            assert_eq!(numeric_annotation(s), Some(k), "{s:?}");
        }
        for s in ["", "5", "<", "<abc", "< 5 years", "cc", "C++", "approx.", "-", "n/a", "confidentially speaking"] {
            assert_eq!(numeric_annotation(s), None, "{s:?}");
        }
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
