//! Cheap per-cell signals used by the layout rules: coarse type, keyword classes, `SUM(` shape.
//! Deliberately self-contained; the values/context workstreams will provide richer versions.

use crate::model::{Cell, Value};

use super::params::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    /// Number, date, numeric-looking text, or a formula without cached value.
    Num,
    /// Missing tokens ("-", "n/a"), booleans, errors: filled but type-neutral.
    Missing,
    /// Empty merged area (no value at its anchor).
    Blank,
}

/// How a formula aggregates other cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agg {
    None,
    /// Sum/plus of ≥2 cells above in the same column (total row).
    Vertical,
    /// Sum/plus of ≥2 cells to the left in the same row (total column).
    Horizontal,
}

const MISSING_TOKENS: &[&str] = &[
    "-", "--", "---", "–", "—", "n/a", "na", "n.a.", "#n/a", "x", ".", "..", "...", "?", "null", "none", "nema", "нема",
    // NCES and other US tables: not applicable, rounds to zero, reporting standards not met.
    "†", "#", "‡",
    // US Census: not applicable, not available.
    "(x)", "(na)",
];

const TOTAL_WORDS: &[&str] = &[
    "total", "totals", "subtotal", "subtotals", "sum", "ukupno", "ukupna", "ukupni", "ukupan", "sveukupno", "svega",
    "zbir", "zbirno", "укупно", "укупна", "укупни", "укупан", "свеукупно", "свега", "збир", "збирно", "average",
    "avg", "mean", "maximum", "minimum", "prosek", "prosečno", "просек", "просечно",
];

const AGG_FUNCS: &[&str] = &["SUM(", "SUBTOTAL(", "AVERAGE(", "MAX(", "MIN(", "COUNT(", "COUNTA(", "MEDIAN("];

const NOTE_WORDS: &[&str] = &[
    "note", "notes", "source", "sources", "napomena", "napomene", "izvor", "izvori", "напомена", "напомене", "извор",
    "извори", "nb", "footnote", "footnotes", "comment", "comments", "remark", "remarks", "prepared", "pripremio",
    "pripremila", "припремио", "disclaimer",
];

/// Note words in more languages (de, fr, es, it, pt, nl, pl, cs, da, sv, fi; B38), counted only
/// with a colon after them.
const NOTE_WORDS_COLON: &[&str] = &[
    "quelle", "quellen", "anmerkung", "anmerkungen", "hinweis", "hinweise", "fussnote", "fußnote", "fussnoten",
    "fußnoten", "remarque", "remarques", "nota", "notas", "fuente", "fuentes", "fonte", "fontes", "opmerking",
    "opmerkingen", "bron", "uwaga", "uwagi", "źródło", "poznámka", "poznámky", "zdroj", "kilde", "källa", "lähde",
];

pub(crate) fn kind_of(cell: &Cell) -> Kind {
    match &cell.value {
        Some(Value::Number(_)) | Some(Value::DateTime(_)) => Kind::Num,
        Some(Value::Text(s)) => text_kind(s),
        Some(Value::Bool(_)) | Some(Value::Error(_)) => Kind::Missing,
        None if cell.formula.is_some() => Kind::Num,
        None => Kind::Blank,
    }
}

fn text_kind(s: &str) -> Kind {
    let t = s.trim();
    if t.is_empty() {
        return Kind::Blank;
    }
    if t.len() <= 5 && MISSING_TOKENS.iter().any(|m| t.eq_ignore_ascii_case(m)) {
        return Kind::Missing;
    }
    // Eurostat's `:` (not available) with or without flag letters (`: m`, `:c`, `: bu`, B54).
    if t.starts_with(':') && crate::values::missing::missing_token(t).is_some() {
        return Kind::Missing;
    }
    if looks_numeric(t) {
        return Kind::Num;
    }
    // `20221)`, `2022 1)`: a year with a note mark in a header stays a label.
    let unbalanced = t.matches(')').count() > t.matches('(').count();
    if !unbalanced && t.len() <= 40 && !is_period_code(t) && t.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '(' | '+' | '.')) {
        // A number with a footnote mark or flag (`28472.72*`, `1737³`, `6077.61 (p)`, `20560.32e`)
        // is a number, not a label (B33: such rows started new tables).
        use crate::values::number::{NumberHint, parse_number};
        if parse_number(t, NumberHint::default()).is_some_and(|p| p.footnote && p.unit.is_none() && p.currency.is_none())
        {
            return Kind::Num;
        }
    }
    // Censored and suppressed values (`<5`, `> 1000`, `confidential`, `[c]`, `see note`) stand
    // in for a number: type-neutral, like a missing token.
    use crate::values::missing::{NumericAnnotation, numeric_annotation};
    let annot = numeric_annotation(t);
    if annot == Some(NumericAnnotation::Censored) && t != "(Z)"
        || (annot == Some(NumericAnnotation::Suppressed) && t.chars().filter(|c| c.is_alphabetic()).count() >= 6)
    {
        return Kind::Missing;
    }
    Kind::Text
}

/// A unit or scale label as written in a unit row under a header (B45): `Mrd. EUR`, `%`, `% 4)`,
/// `2020 = 100`, `1 000 Personen`, `in 1000`, `Tsd. EUR`, `EUR`, `kg`, `Anzahl`. Short, and every
/// word a unit, scale, currency, number, footnote mark or one of a few count words.
pub(crate) fn is_unit_text(t: &str) -> bool {
    use crate::values::number::{scale, unit};
    let t = t.trim();
    if t.is_empty() || t.chars().count() > 24 {
        return false;
    }
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    // Index base: "2020 = 100", "2015=100".
    if let Some(i) = compact.find("=100")
        && i == 4
        && compact[..4].bytes().all(|b| b.is_ascii_digit())
    {
        return true;
    }
    const WORDS: [&str; 24] = [
        "in", "je", "per", "pro", "personen", "persons", "anzahl", "number", "stunden", "hours", "jahre", "years",
        "prozent", "percent", "punkte", "points", "einwohner", "inhabitants", "tonnes", "tonnen", "t", "ha", "km",
        "euro",
    ];
    let mut any_unit = false;
    for w in t.split(|c: char| c.is_whitespace() || c == '/') {
        let w = w.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | ','));
        if w.is_empty() {
            continue;
        }
        let lw = w.to_lowercase();
        if w.contains('%') || w.contains('‰') || scale(w).is_some() || unit(w).is_some() || crate::values::currency::currency(w).is_some()
        {
            any_unit = true;
        } else if WORDS.contains(&lw.as_str()) {
            any_unit |= !matches!(lw.as_str(), "in" | "je" | "per" | "pro");
        } else if w.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            // numbers ("1 000"), footnote numbers
        } else {
            return false;
        }
    }
    any_unit
}

/// A ruling line drawn with text: a run of at least 3 of one of `- = _ ~ – —`, or 4 of `* # +`
/// (`***` and `...` are missing-value marks), or `|`
/// alone). Only a whole row of such cells (or a `|` cell) is a ruling line ([`super::blocks`]).
pub(crate) fn is_rule_text(t: &str) -> bool {
    let t = t.trim();
    if t == "|" || t == "||" || t == "¦" {
        return true;
    }
    let mut it = t.chars();
    let Some(first) = it.next() else { return false };
    let min = if matches!(first, '-' | '=' | '_' | '~' | '–' | '—') { 3 } else { 4 };
    matches!(first, '-' | '=' | '_' | '~' | '*' | '#' | '+' | '–' | '—')
        && t.chars().count() >= min
        && t.chars().all(|c| c == first || (c == ' ' && first != ' '))
}

/// "1,200.50", "(500)", "12%", "$ 3.4", "1.200,50 RSD"-ish, "01/02/2001", "-7".
fn looks_numeric(t: &str) -> bool {
    if t.len() > 40 {
        return false;
    }
    let mut digits = 0usize;
    let mut other = 0usize;
    for ch in t.chars() {
        match ch {
            '0'..='9' => digits += 1,
            ',' | '.' | ' ' | '\u{a0}' | '\'' | '(' | ')' | '-' | '+' | '/' | '%' | '$' | '€' | '£' => {}
            _ => other += 1,
        }
    }
    digits > 0 && other == 0
}

/// Integer year or a date value — numbers that commonly label columns ("2023", Jan-01).
pub(crate) fn is_label_number(cell: &Cell) -> bool {
    match &cell.value {
        Some(Value::DateTime(_)) => true,
        Some(Value::Number(n)) => n.fract() == 0.0 && (YEAR_MIN..=YEAR_MAX).contains(n),
        Some(Value::Text(s)) => {
            let t = s.trim();
            let year = |n: f64| n.fract() == 0.0 && (YEAR_MIN..=YEAR_MAX).contains(&n);
            (t.len() == 4 && t.parse::<f64>().is_ok_and(year))
                || is_text_period(t)
                || is_split_year(t)
                // `2022¹`, `2023*`: a year with a footnote mark (counted as a number by `kind_of`).
                || (t.len() <= 12
                    && kind_of(cell) == Kind::Num
                    && t.as_bytes()[..4.min(t.len())].iter().all(u8::is_ascii_digit)
                    && crate::values::number::parse_number(t, Default::default())
                        .is_some_and(|p| p.footnote && p.thousands_sep.is_none() && year(p.value)))
        }
        _ => false,
    }
}

/// A period or date written as text with a year: `2010-01`, `01/2010`, `2010-01-31`,
/// `31.01.2010`, `1/31/2010` (B39: a header row of `YYYY-MM` periods). Not `2010.01` or `01.2010`,
/// which read as decimals.
pub(crate) fn is_text_period(t: &str) -> bool {
    if is_period_code(t) {
        return true;
    }
    if t.len() < 6 || t.len() > 10 || !t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'/' | b'.')) {
        return false;
    }
    let sep = match t.bytes().find(|b| !b.is_ascii_digit()) {
        Some(s) => s,
        None => return false,
    };
    let parts: Vec<&str> = t.split(sep as char).collect();
    if parts.iter().any(|p| p.is_empty()) || t.bytes().any(|b| !b.is_ascii_digit() && b != sep) {
        return false;
    }
    let year = |p: &str| p.len() == 4 && p.parse::<f64>().is_ok_and(|n| (YEAR_MIN..=YEAR_MAX).contains(&n));
    let small = |p: &str, max: u32| p.len() <= 2 && p.parse::<u32>().is_ok_and(|n| (1..=max).contains(&n));
    match parts[..] {
        // `2010-01`, `01/2010`; not `2011/12`, a split year (fiscal, marketing).
        [a, b] if sep != b'.' => (sep == b'-' && year(a) && small(b, 12)) || (small(a, 12) && year(b)),
        [a, b, c] => (year(a) && small(b, 12) && small(c, 31)) || (small(a, 31) && small(b, 31) && year(c)),
        _ => false,
    }
}

/// A split year (school, fiscal or marketing year: `1990-91`, `2011/12`, the second year one after
/// the first) or a range of years (`2004-2008`). A column label, not a number (B55).
pub(crate) fn is_split_year(t: &str) -> bool {
    let b = t.as_bytes();
    if !(7..=9).contains(&b.len()) || !b[..4].iter().all(u8::is_ascii_digit) || !matches!(b[4], b'-' | b'/') {
        return false;
    }
    let rest = &t[5..];
    let Ok(y) = t[..4].parse::<u32>() else { return false };
    if !(YEAR_MIN as u32..=YEAR_MAX as u32).contains(&y) || !rest.bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    match rest.len() {
        2 => rest.parse::<u32>().is_ok_and(|n| n == (y + 1) % 100),
        // a range of years (`2004-2008`)
        4 => rest.parse::<u32>().is_ok_and(|n| n > y && n <= y + 100),
        _ => false,
    }
}

/// A statistics-database period code: a year, a period letter and its number (`2016K3`,
/// `2016Q4`, `2014M01`, `2020H1`; PX-Web and SDMX exports, B53). Not a number with a scale.
pub(crate) fn is_period_code(t: &str) -> bool {
    let b = t.as_bytes();
    if !t.is_ascii() || !(6..=7).contains(&b.len()) || !b[..4].iter().all(u8::is_ascii_digit) {
        return false;
    }
    let year = t[..4].parse::<f64>().is_ok_and(|n| (YEAR_MIN..=YEAR_MAX).contains(&n));
    let n = &t[5..];
    let max = match b[4].to_ascii_uppercase() {
        b'Q' | b'K' => 4,
        b'M' => 12,
        b'H' | b'S' => 2,
        _ => return false,
    };
    year && n.bytes().all(|c| c.is_ascii_digit()) && n.parse::<u32>().is_ok_and(|v| (1..=max).contains(&v))
}

pub(crate) fn text_of(cell: &Cell) -> Option<&str> {
    match &cell.value {
        Some(Value::Text(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

/// "Total", "Grand total", "Ukupno Vojvodina", "Subtotal:", "УКУПНО", "Σ".
pub(crate) fn is_total_label(s: &str) -> bool {
    let t = s.trim();
    if t.starts_with('Σ') || t.starts_with('∑') {
        return true;
    }
    // The words of `tokens(t)`, compared without allocating for ASCII words (the total words are
    // lower case, so an ASCII word matches one exactly when it equals it ignoring ASCII case).
    let is_total_word = |w: &str| {
        if w.is_ascii() {
            TOTAL_WORDS.iter().any(|tw| tw.eq_ignore_ascii_case(w))
        } else {
            TOTAL_WORDS.contains(&w.to_lowercase().as_str())
        }
    };
    let mut words = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty());
    let Some(first) = words.next() else { return false };
    if is_total_word(first) {
        return true;
    }
    let mut n = 1;
    let mut any = false;
    for w in words {
        n += 1;
        if n > TOTAL_LABEL_MAX_TOKENS {
            return false;
        }
        any = any || is_total_word(w);
    }
    any
}

/// "Source: …", "Napomena", "* preliminary", "(1) …", "a) …", "Извор: РЗС".
/// A symbol legend under a table: a missing-value mark, then what it means (`---Not available.`,
/// `#Rounds to zero.`, `!Interpret data with caution.`, `: not available`). A single dash
/// (`-Counterparty`) is a list item and `... aber` a continued sentence, not a legend.
fn is_legend_line(t: &str) -> bool {
    let mark: String = t.chars().take_while(|c| matches!(c, '-' | '–' | '—' | ':' | '#' | '!')).collect();
    let rest = t[mark.len()..].trim_start();
    let n = mark.chars().count();
    let first = mark.chars().next();
    let ok_mark = (n >= 3 && first.is_some_and(|f| f != ':' && mark.chars().all(|c| c == f)))
        || (n == 1 && matches!(first, Some('#' | '!')))
        || (n == 1 && first == Some(':') && t[1..].starts_with(' '));
    ok_mark && rest.chars().next().is_some_and(char::is_alphabetic) && rest.split_whitespace().count() >= 2
}

pub(crate) fn is_note_label(s: &str) -> bool {
    let t = s.trim();
    let Some(first) = t.chars().next() else { return false };
    if matches!(first, '*' | '†' | '‡' | '¹' | '²' | '³' | '⁽') {
        return true;
    }
    if is_legend_line(t) {
        return true;
    }
    // "(1) text", "1) text", "a) text"
    let head: String = t.chars().take(5).collect();
    if let Some(close) = head.find(')') {
        let inner = head[..close].trim_start_matches('(');
        if (1..=2).contains(&inner.chars().count())
            && inner.chars().all(|c| c.is_ascii_alphanumeric())
            && t.chars().count() > close + 2
        {
            return true;
        }
    }
    let Some(first) = tokens(t).into_iter().next() else { return false };
    if NOTE_WORDS.contains(&first.as_str()) {
        return true;
    }
    // Other languages' note words only as a label with a colon ("Quelle:", "Remarque :").
    NOTE_WORDS_COLON.contains(&first.as_str())
        && t.chars().skip(first.chars().count()).find(|c| !c.is_whitespace()) == Some(':')
}

/// `SUM(`/`AVERAGE(`/`MAX(`… over at least two same-sheet cells, anywhere (loose summary cells).
pub(crate) fn is_aggregate_formula(formula: &str) -> bool {
    let f = formula.to_ascii_uppercase();
    if f.contains('!') || !AGG_FUNCS.iter().any(|a| f.contains(a)) {
        return false;
    }
    let (refs, _) = parse_refs(f.as_bytes());
    refs.iter().map(|r| (r.r2 - r.r1 + 1) as u64 * (r.c2 - r.c1 + 1) as u64).sum::<u64>() >= 2
}

#[derive(Debug, Clone, Copy)]
struct Ref {
    r1: u32,
    c1: u32,
    r2: u32,
    c2: u32,
}

/// Classifies `SUM(C5:C9)`, `SUBTOTAL(9,C5:C9)`, `C5+C10+C15` at (row, col). Cross-sheet refs → None.
pub(crate) fn formula_agg(formula: &str, row: u32, col: u32) -> Agg {
    let f = formula.to_ascii_uppercase();
    if f.contains('!') {
        return Agg::None;
    }
    let is_sum = AGG_FUNCS.iter().any(|a| f.contains(a));
    let (refs, rest_is_plus) = parse_refs(f.as_bytes());
    if refs.is_empty() || !(is_sum || rest_is_plus) {
        return Agg::None;
    }
    let cells: u64 = refs.iter().map(|r| (r.r2 - r.r1 + 1) as u64 * (r.c2 - r.c1 + 1) as u64).sum();
    if cells < 2 {
        return Agg::None;
    }
    if refs.iter().all(|r| r.c1 == col && r.c2 == col && r.r2 < row) {
        Agg::Vertical
    } else if (is_sum || refs.len() >= 3) && refs.iter().all(|r| r.r1 == row && r.r2 == row && r.c2 < col) {
        Agg::Horizontal
    } else {
        Agg::None
    }
}

/// Extracts A1 refs/ranges; second value: the formula is only refs joined by `+` (and parens).
fn parse_refs(b: &[u8]) -> (Vec<Ref>, bool) {
    let mut refs = Vec::new();
    let mut only_plus = true;
    let mut i = 0;
    while i < b.len() {
        let prev_ident = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'.');
        if !prev_ident && let Some((r, c, n)) = cell_ref(&b[i..]) {
            let mut j = i + n;
            let mut rf = Ref { r1: r, c1: c, r2: r, c2: c };
            if j < b.len() && b[j] == b':' && let Some((r2, c2, n2)) = cell_ref(&b[j + 1..]) {
                rf = Ref { r1: r.min(r2), c1: c.min(c2), r2: r.max(r2), c2: c.max(c2) };
                j += 1 + n2;
            }
            // Function names like LOG10( are not refs.
            if j < b.len() && (b[j] == b'(' || b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                only_plus = false;
                i = j;
                continue;
            }
            refs.push(rf);
            i = j;
            continue;
        }
        if !matches!(b[i], b'+' | b'(' | b')' | b' ') {
            only_plus = false;
        }
        i += 1;
    }
    let plus = only_plus && refs.len() >= 2;
    (refs, plus)
}

/// `$A$1`-style reference at the start of `b` → (row, col, bytes consumed), 0-based.
fn cell_ref(b: &[u8]) -> Option<(u32, u32, usize)> {
    let mut i = 0;
    if b.get(i) == Some(&b'$') {
        i += 1;
    }
    let ls = i;
    while i < b.len() && b[i].is_ascii_uppercase() && i - ls < 3 {
        i += 1;
    }
    if i == ls {
        return None;
    }
    let mut col: u32 = 0;
    for &ch in &b[ls..i] {
        col = col * 26 + (ch - b'A' + 1) as u32;
    }
    if b.get(i) == Some(&b'$') {
        i += 1;
    }
    let ds = i;
    while i < b.len() && b[i].is_ascii_digit() && i - ds < 7 {
        i += 1;
    }
    if i == ds {
        return None;
    }
    let row: u32 = std::str::from_utf8(&b[ds..i]).ok()?.parse().ok()?;
    if row == 0 || col == 0 {
        return None;
    }
    Some((row - 1, col - 1, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_and_notes() {
        for s in ["Total", "Grand Total", "Ukupno Vojvodina", "SUBTOTAL:", "УКУПНО", "Σ", "Svega", "Total revenue"] {
            assert!(is_total_label(s), "{s}");
        }
        for s in ["Totally unrelated long label with many many words total", "Region", "Beograd"] {
            assert!(!is_total_label(s), "{s}");
        }
        for s in ["Source: RZS", "Napomena: preliminarno", "* preliminary", "(1) estimated", "a) excl. VAT", "Извор: РЗС"] {
            assert!(is_note_label(s), "{s}");
        }
        for s in ["Beograd", "Q1", "(500)"] {
            assert!(!is_note_label(s), "{s}");
        }
    }

    #[test]
    fn formula_shapes() {
        assert_eq!(formula_agg("SUM(C5:C9)", 9, 2), Agg::Vertical);
        assert_eq!(formula_agg("SUBTOTAL(9,$C$5:$C$9)", 9, 2), Agg::Vertical);
        assert_eq!(formula_agg("C5+C10+C15", 20, 2), Agg::Vertical);
        assert_eq!(formula_agg("SUM(B5:D5)", 4, 4), Agg::Horizontal);
        assert_eq!(formula_agg("C4+D5", 4, 2), Agg::None);
        assert_eq!(formula_agg("C5*1.1", 9, 2), Agg::None);
        assert_eq!(formula_agg("SUM(Sheet2!C5:C9)", 9, 2), Agg::None);
        assert_eq!(formula_agg("SUM(C5:C9)", 3, 2), Agg::None);
        assert_eq!(formula_agg("LOG10(C5)+C6", 9, 2), Agg::None);
    }

    #[test]
    fn periods_rules_units_and_marked_numbers() {
        for s in ["2010-01", "01/2010", "2010-01-31", "31.01.2010", "1/31/2010"] {
            assert!(is_text_period(s), "{s}");
        }
        for s in ["2010.01", "01.2010", "2011/12", "2010-13", "1234-01", "12-34", "2010"] {
            assert!(!is_text_period(s), "{s}");
        }
        for s in ["-----", "=====", "_____", "~~~~~", "*****", "|", "---------------"] {
            assert!(is_rule_text(s), "{s}");
        }
        for s in ["-", "--", "Total", "=-=", "1---"] {
            assert!(!is_rule_text(s), "{s}");
        }
        for s in ["Mrd. EUR", "%", "% 4)", "2020 = 100", "1 000 Personen", "Tsd. EUR", "in 1000 EUR", "EUR"] {
            assert!(is_unit_text(s), "{s}");
        }
        for s in ["Region", "in", "Ursprungswerte", "2020", "Bruttoinlandsprodukt"] {
            assert!(!is_unit_text(s), "{s}");
        }
        for s in ["28472.72*", "1737³", "6077.61 (p)", "20560.32e", "7 p"] {
            assert_eq!(text_kind(s), Kind::Num, "{s}");
        }
        for s in ["<5", "> 1000", "confidential", "suppressed"] {
            assert_eq!(text_kind(s), Kind::Missing, "{s}");
        }
        for s in ["C", "F", "Q1 2003", "Riverbend 2", "See Notes", "[c]"] {
            assert_eq!(text_kind(s), Kind::Text, "{s}");
        }
    }

    #[test]
    fn period_codes_split_years_legends_and_marks() {
        // PX-Web / SDMX period codes are labels, not numbers with a scale (B53).
        for s in ["2016K3", "2016Q4", "2014M01", "2014M12", "2020H1"] {
            assert!(is_period_code(s) && is_text_period(s), "{s}");
            assert_eq!(text_kind(s), Kind::Text, "{s}");
        }
        for s in ["2016K5", "2014M13", "2016K", "16K3", "2016X1", "1500K3"] {
            assert!(!is_period_code(s), "{s}");
        }
        for s in ["1990-91", "2011/12", "1999-00", "2004-2005", "2004-2018"] {
            assert!(is_split_year(s), "{s}");
        }
        for s in ["1990-93", "2004-2003", "2011", "1990-9", "12-34"] {
            assert!(!is_split_year(s), "{s}");
        }
        // Symbol legends under a table are notes; list items and continued sentences are not.
        for s in ["---Not available.", "# Rounds to zero.", "!Interpret data with caution.", ": not available"] {
            assert!(is_note_label(s), "{s}");
        }
        for s in ["-Counterparty ($MM)", "... aber der Betreiber", "#1 Plant", "---"] {
            assert!(!is_note_label(s), "{s}");
        }
        // Missing-value marks: Eurostat `:` with flags, NCES and Census marks.
        for s in [": m", ":", ": c", "†", "‡", "#", "(X)", "(NA)"] {
            assert_eq!(text_kind(s), Kind::Missing, "{s}");
        }
    }

    #[test]
    fn numeric_text() {
        assert_eq!(text_kind("1,200.50"), Kind::Num);
        assert_eq!(text_kind("(500)"), Kind::Num);
        assert_eq!(text_kind("n/a"), Kind::Missing);
        assert_eq!(text_kind("Q1 2003"), Kind::Text);
    }
    #[test]
    fn compound_sd_is_measurement_layout_evidence_only() {
        for value in ["40.3 (SD 12.9", "39.7 (SD 12.6)", "55 (SD 13", "-2 (SD 0)"] {
            assert_eq!(text_kind(value), Kind::Num, "{value}");
        }
        for value in ["Mean (SD)", "Number (SD reporting)", "40.3 (SD pending", "40.3 (SD -1)",
            "40.3 (SD 12.9) years", "NaN (SD 2)", "40.3 (SD inf)", "40.3 (SD 12.9))",
            "40.3 (SD 12.9) other", "40.3 SD 12.9", "SD 12.9"] {
            assert_eq!(text_kind(value), Kind::Text, "{value}");
        }
    }

}
