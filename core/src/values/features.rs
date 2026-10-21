//! Per-cell content features (`v_*` columns), one row per entry of `sheet.cells`.
//!
//! Column-level context is inferred first (decimal convention and day/month order per sheet
//! column, falling back to the sheet-wide decimal vote), then each cell is described on its own.
//! All features are 0/1 flags unless noted. See [`FEATURES`] for the one-line documentation.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::date::{parse_date, parse_time, serial_to_ymd, DateOrder, DateOrderVotes};
use super::locale::Locale;
use super::keywords::{is_note, starts_with_enumerator, total_score};
use super::missing::{is_excel_error, missing_token, parse_bool};
use super::currency::mentions_currency;
use super::number::{parse_number, DecimalSep, DecimalVotes, NativeNumbers, NumberHint};
use super::text::{has_cyrillic, is_padded, trim};
use crate::features::Features;
use crate::grid::Grid;
use crate::model::{Cell, Value};

/// Name and one-line meaning of every feature, in the order they are added.
pub const FEATURES: [(&str, &str); 34] = [
    ("v_is_number", "stored value is a number"),
    ("v_is_text", "stored value is text"),
    ("v_is_date", "stored value is an Excel date/time"),
    ("v_is_bool", "stored value is a boolean"),
    ("v_is_error", "stored value is an Excel error, or text spelling one (`#N/A`)"),
    ("v_is_formula", "cell has a formula"),
    ("v_no_value", "formula cell without a cached value"),
    ("v_numeric_text", "text that parses as a number (locale, currency, units, %, scale handled)"),
    ("v_date_text", "text that parses as a date, month or quarter"),
    ("v_bool_text", "text that is a boolean code (yes/no, da/ne, ✓)"),
    ("v_period_label", "text that is a month or quarter name without a year (`Jan`, `Q3`, `III kvartal`)"),
    ("v_missing_token", "text missing-value token (`-`, `n/a`, `nema`, `..`, `x`) or an error value"),
    ("v_has_currency", "numeric text carrying a currency symbol/code (`1.200 RSD`, `€ 15`)"),
    ("v_has_unit", "numeric text carrying a unit (`15 kg`, `3 kom`, `120 m²`)"),
    ("v_is_percent", "numeric text with a `%` sign"),
    ("v_has_scale", "numeric text with a scale word (`5k`, `1,2 mil.`)"),
    ("v_is_negative", "numeric value is negative (any style: `-5`, `(500)`, `500-`)"),
    ("v_has_footnote", "numeric text with a footnote marker (`1.234*`, `567 (a)`)"),
    ("v_leading_zero", "integer text with leading zeros (`00123`) — likely an ID"),
    ("v_is_integer", "numeric value (number or numeric text) is a whole number"),
    ("v_is_year", "whole number 1900–2100 written as a number or plain text (`2024`, `2024.`)"),
    ("v_log_abs", "log10(1 + |value|) for numeric values, else 0"),
    ("v_len", "characters in the displayed value (trimmed text; numbers in shortest form)"),
    ("v_word_count", "number of whitespace-separated words"),
    ("v_digit_ratio", "share of characters that are digits"),
    ("v_alpha_ratio", "share of characters that are letters"),
    ("v_upper_ratio", "share of letters that are uppercase"),
    ("v_has_cyrillic", "text contains Cyrillic letters"),
    ("v_is_padded", "text has leading/trailing whitespace, NBSP or invisible characters"),
    ("v_ends_with_colon", "text ends with `:` (label / group header)"),
    ("v_kw_total", "1 if text starts with a total keyword (total/ukupno/svega/укупно…), 0.5 if one appears later"),
    ("v_kw_note", "text starts like a note/source line (`Note:`, `Izvor:`, `Napomena`, `*`, `¹`)"),
    ("v_starts_with_enumerator", "text starts with an enumerator (`1.`, `1.2`, `a)`, `(3)`, `IV.`, `-`, `•`) then text"),
    ("v_time_text", "text that is a clock time alone (`6:00`, `6:00 PM`, `18:00:00`, `18h30`)"),
];

const N: usize = FEATURES.len();

/// Per-column context used to resolve ambiguous values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ColumnHints {
    pub decimal: Option<DecimalSep>,
    pub date_order: Option<DateOrder>,
    /// The column holds currency amounts or mentions a currency (see [`NumberHint::currency_context`]).
    pub currency_context: bool,
}

impl ColumnHints {
    pub fn number_hint(&self) -> NumberHint {
        NumberHint::with_decimal(self.decimal).with_currency_context(self.currency_context)
    }
}

/// Column evidence for word-like currency codes: a cell already read with a currency, or text
/// mentioning one (`Amount (USD)`, `$`, `Currency`).
fn currency_evidence(s: &str) -> bool {
    parse_number(s, NumberHint::default()).is_some_and(|p| p.currency.is_some()) || mentions_currency(s)
}

/// Infers decimal convention and day/month order from a column's text values.
pub fn infer_column<'a>(values: impl IntoIterator<Item = &'a str>) -> ColumnHints {
    infer_column_with(values, &NativeNumbers::default())
}

/// [`infer_column`] that also weighs the column's numbers stored as numbers (see [`NativeNumbers`]).
pub fn infer_column_with<'a>(values: impl IntoIterator<Item = &'a str>, native: &NativeNumbers) -> ColumnHints {
    let mut d = DecimalVotes::default();
    let mut o = DateOrderVotes::default();
    let mut cur = false;
    for s in values {
        d.add(s);
        o.add(s);
        cur = cur || currency_evidence(s);
    }
    ColumnHints { decimal: d.decision_with(native), date_order: o.decision(), currency_context: cur }
}

/// Infers [`ColumnHints`] for every column of the sheet, from the data only.
///
/// Per column: majority of unambiguous values. Columns without evidence fall back to the
/// sheet-wide vote, then to cross-evidence (a comma-decimal sheet writes dates day-first; a
/// month-first sheet uses the decimal point). With no evidence at all the hint stays `None` and
/// the parsers apply their documented tie-breaks, flagging the value `ambiguous`.
pub fn sheet_column_hints(grid: &Grid) -> HashMap<u32, ColumnHints> {
    sheet_column_hints_with(grid, None)
}

/// Like [`sheet_column_hints`], but an explicit `locale` overrides inference for every column.
pub fn sheet_column_hints_with(grid: &Grid, locale: Option<Locale>) -> HashMap<u32, ColumnHints> {
    let mut votes: HashMap<u32, (DecimalVotes, DateOrderVotes)> = HashMap::new();
    let mut currency_cols: HashMap<u32, bool> = HashMap::new();
    let mut natives: HashMap<u32, NativeNumbers> = HashMap::new();
    // Each text's votes and currency evidence depend only on the text: computed once per text.
    let mut memo: rustc_hash::FxHashMap<&str, (DecimalVotes, DateOrderVotes, bool)> = rustc_hash::FxHashMap::default();
    for c in &grid.sheet.cells {
        if let Some(Value::Number(x)) = &c.value {
            if locale.is_none() {
                natives.entry(c.col).or_default().add(*x);
            }
            continue;
        }
        if let Some(Value::Text(s)) = &c.value {
            let (d, o, cur_ev) = *memo.entry(s.as_str()).or_insert_with(|| {
                let (mut d, mut o) = (DecimalVotes::default(), DateOrderVotes::default());
                if locale.is_none() {
                    d.add(s);
                    o.add(s);
                }
                (d, o, currency_evidence(s))
            });
            let e = votes.entry(c.col).or_default();
            e.0.merge(&d);
            e.1.merge(&o);
            let cur = currency_cols.entry(c.col).or_default();
            *cur |= cur_ev;
        }
    }
    let cur = |col: &u32| currency_cols.get(col).copied().unwrap_or(false);
    if let Some(l) = locale {
        return votes.into_keys().map(|col| (col, ColumnHints { currency_context: cur(&col), ..l.hints() })).collect();
    }
    let mut sheet_d = DecimalVotes::default();
    let mut sheet_o = DateOrderVotes::default();
    for (d, o) in votes.values() {
        sheet_d.merge(d);
        sheet_o.merge(o);
    }
    let sheet_order = sheet_o.decision();
    let sheet_decimal = sheet_d.decision().or(match sheet_order {
        Some(DateOrder::MonthDay) => Some(DecimalSep::Dot),
        _ => None,
    });
    let sheet_order = sheet_order.or(match sheet_decimal {
        Some(DecimalSep::Comma) => Some(DateOrder::DayMonth),
        _ => None,
    });
    votes
        .into_iter()
        .map(|(col, (d, o))| {
            let currency_context = cur(&col);
            let native = natives.get(&col).copied().unwrap_or_default();
            let decimal = d.decision_with(&native).or(sheet_decimal);
            (col, ColumnHints { decimal, date_order: o.decision().or(sheet_order), currency_context })
        })
        .collect()
}

/// Adds every `v_*` column (see [`FEATURES`]) to `out`, one value per entry of `sheet.cells`,
/// inferring each column's conventions from the data.
pub fn compute(grid: &Grid, out: &mut Features) {
    compute_with(grid, out, None);
}

/// [`compute`] with an optional explicit locale for the whole sheet (overrides inference).
pub fn compute_with(grid: &Grid, out: &mut Features, locale: Option<Locale>) {
    let cells = &grid.sheet.cells;
    let hints = sheet_column_hints_with(grid, locale);
    let mut cols: Vec<Vec<f32>> = (0..N).map(|_| Vec::with_capacity(cells.len())).collect();
    // Registers repeat the same values on many rows: a text's or number's features depend only on
    // the value and its column's hints (and the formula flag, set per cell), so each pair is
    // computed once.
    let mut memo: rustc_hash::FxHashMap<(&str, ColumnHints), [f32; N]> = rustc_hash::FxHashMap::default();
    let mut num_memo: rustc_hash::FxHashMap<(u64, ColumnHints), [f32; N]> = rustc_hash::FxHashMap::default();
    for c in cells {
        let h = hints.get(&c.col).copied().unwrap_or_else(|| locale.map(Locale::hints).unwrap_or_default());
        let row = match &c.value {
            Some(Value::Text(t)) => {
                let mut r = *memo.entry((t.as_str(), h)).or_insert_with(|| cell_features(c, h));
                r[5] = if c.formula.is_some() { 1.0 } else { 0.0 };
                r
            }
            Some(Value::Number(x)) => {
                let mut r = *num_memo.entry((x.to_bits(), h)).or_insert_with(|| cell_features(c, h));
                r[5] = if c.formula.is_some() { 1.0 } else { 0.0 };
                r
            }
            _ => cell_features(c, h),
        };
        for (col, v) in cols.iter_mut().zip(row) {
            col.push(v);
        }
    }
    for ((name, _), col) in FEATURES.iter().zip(cols) {
        out.add(name, col);
    }
}

/// Small stack buffer for formatting numbers without allocating.
struct StackStr {
    buf: [u8; 48],
    len: usize,
}

impl StackStr {
    fn new() -> Self {
        StackStr { buf: [0; 48], len: 0 }
    }
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl std::fmt::Write for StackStr {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let b = s.as_bytes();
        if self.len + b.len() > self.buf.len() {
            return Err(std::fmt::Error);
        }
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
        Ok(())
    }
}

fn format_number(x: f64, out: &mut StackStr) {
    let a = x.abs();
    let _ = if a != 0.0 && !(1e-6..1e15).contains(&a) { write!(out, "{x:e}") } else { write!(out, "{x}") };
}

fn is_year(x: f64) -> bool {
    x.fract() == 0.0 && (1900.0..=2100.0).contains(&x)
}

/// The `v_*` feature values of one cell, in [`FEATURES`] order.
pub fn cell_features(cell: &Cell, hints: ColumnHints) -> [f32; N] {
    let mut f = [0f32; N];
    let flag = |b: bool| if b { 1.0 } else { 0.0 };
    f[5] = flag(cell.formula.is_some());
    let mut buf = StackStr::new();
    let display: &str;
    let mut numeric: Option<f64> = None;
    match &cell.value {
        None => {
            f[6] = 1.0;
            display = "";
        }
        Some(Value::Number(x)) => {
            f[0] = 1.0;
            numeric = Some(*x);
            f[20] = flag(is_year(*x));
            format_number(*x, &mut buf);
            display = buf.as_str();
        }
        Some(Value::DateTime(x)) => {
            f[2] = 1.0;
            match serial_to_ymd(*x) {
                Some((y, m, d)) => {
                    let _ = write!(buf, "{y:04}-{m:02}-{d:02}");
                }
                None => format_number(*x, &mut buf),
            }
            display = buf.as_str();
        }
        Some(Value::Bool(b)) => {
            f[3] = 1.0;
            display = if *b { "TRUE" } else { "FALSE" };
        }
        Some(Value::Error(e)) => {
            f[4] = 1.0;
            f[11] = 1.0;
            display = e.as_str();
        }
        Some(Value::Text(s)) => {
            f[1] = 1.0;
            let t = trim(s);
            display = t;
            f[27] = flag(has_cyrillic(t));
            f[28] = flag(is_padded(s));
            f[29] = flag(t.ends_with(':'));
            if let Some(k) = missing_token(t) {
                f[11] = 1.0;
                f[4] = flag(k == super::missing::MissingKind::Error || is_excel_error(t));
            } else if let Some(p) = parse_number(t, hints.number_hint()) {
                f[7] = 1.0;
                f[12] = flag(p.currency.is_some());
                f[13] = flag(p.unit.is_some());
                f[14] = flag(p.percent);
                f[15] = flag(p.scale != 1.0);
                f[17] = flag(p.footnote);
                f[18] = flag(p.leading_zero);
                let v = p.normalized();
                numeric = Some(v);
                let plain_year = t.len() <= 5 && t.as_bytes()[..4.min(t.len())].iter().all(u8::is_ascii_digit);
                f[20] = flag(plain_year && t.len() >= 4 && (t.len() == 4 || t.ends_with('.')) && is_year(v));
            } else {
                if t.len() == 5 && t.ends_with('.') && t[..4].bytes().all(|b| b.is_ascii_digit()) {
                    // "2024." — Serbian ordinal year; not a number to the parser.
                    if let Ok(y) = t[..4].parse::<f64>() {
                        f[20] = flag(is_year(y));
                        f[19] = f[20];
                    }
                }
                if parse_date(t, hints.date_order).is_some() {
                    f[8] = 1.0;
                } else if super::date::is_period_label(t) {
                    f[10] = 1.0;
                } else if parse_time(t).is_some() {
                    f[33] = 1.0;
                }
                f[9] = flag(parse_bool(t).is_some());
                f[30] = total_score(t);
                f[31] = flag(is_note(t));
                f[32] = flag(starts_with_enumerator(t));
            }
        }
    }
    if let Some(x) = numeric {
        f[16] = flag(x < 0.0);
        f[19] = flag(x.is_finite() && x.fract() == 0.0);
        f[21] = (1.0 + x.abs()).log10() as f32;
    }
    // Character statistics of the displayed value.
    let (mut len, mut digits, mut alpha, mut upper, mut words) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut in_word = false;
    for ch in display.chars() {
        len += 1;
        if ch.is_whitespace() {
            in_word = false;
            continue;
        }
        if !in_word {
            words += 1;
            in_word = true;
        }
        if ch.is_ascii_digit() {
            digits += 1;
        } else if ch.is_alphabetic() {
            alpha += 1;
            if ch.is_uppercase() {
                upper += 1;
            }
        }
    }
    f[22] = len as f32;
    f[23] = words as f32;
    if len > 0 {
        f[24] = digits as f32 / len as f32;
        f[25] = alpha as f32 / len as f32;
    }
    if alpha > 0 {
        f[26] = upper as f32 / alpha as f32;
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Sheet;

    fn text(row: u32, col: u32, s: &str) -> Cell {
        Cell { row, col, value: Some(Value::Text(s.into())), formula: None }
    }
    fn get(f: &[f32; N], name: &str) -> f32 {
        f[FEATURES.iter().position(|(n, _)| *n == name).unwrap()]
    }

    #[test]
    fn names_unique_and_prefixed() {
        let mut names: Vec<&str> = FEATURES.iter().map(|(n, _)| *n).collect();
        assert!(names.iter().all(|n| n.starts_with("v_")));
        names.sort();
        names.dedup();
        assert_eq!(names.len(), N);
        assert!(FEATURES.iter().all(|(_, d)| !d.is_empty()));
    }

    #[test]
    fn number_cell() {
        let c = Cell { row: 0, col: 0, value: Some(Value::Number(-2024.0)), formula: Some("A1".into()) };
        let f = cell_features(&c, ColumnHints::default());
        assert_eq!(get(&f, "v_is_number"), 1.0);
        assert_eq!(get(&f, "v_is_formula"), 1.0);
        assert_eq!(get(&f, "v_is_negative"), 1.0);
        assert_eq!(get(&f, "v_is_integer"), 1.0);
        assert_eq!(get(&f, "v_is_year"), 0.0);
        assert_eq!(get(&f, "v_len"), 5.0);
        assert_eq!(get(&f, "v_digit_ratio"), 0.8);
        let c = Cell { row: 0, col: 0, value: Some(Value::Number(2024.0)), formula: None };
        assert_eq!(get(&cell_features(&c, ColumnHints::default()), "v_is_year"), 1.0);
        let c = Cell { row: 0, col: 0, value: Some(Value::Number(1e300)), formula: None };
        assert!(get(&cell_features(&c, ColumnHints::default()), "v_len") > 0.0);
    }

    #[test]
    fn other_value_kinds() {
        let h = ColumnHints::default();
        let f = cell_features(&Cell { row: 0, col: 0, value: None, formula: Some("SUM(A1)".into()) }, h);
        assert_eq!(get(&f, "v_no_value"), 1.0);
        let f = cell_features(&Cell { row: 0, col: 0, value: Some(Value::DateTime(45292.0)), formula: None }, h);
        assert_eq!((get(&f, "v_is_date"), get(&f, "v_len")), (1.0, 10.0));
        let f = cell_features(&Cell { row: 0, col: 0, value: Some(Value::Bool(true)), formula: None }, h);
        assert_eq!((get(&f, "v_is_bool"), get(&f, "v_upper_ratio")), (1.0, 1.0));
        let f = cell_features(&Cell { row: 0, col: 0, value: Some(Value::Error("#DIV/0!".into())), formula: None }, h);
        assert_eq!((get(&f, "v_is_error"), get(&f, "v_missing_token")), (1.0, 1.0));
    }

    #[test]
    fn text_cells() {
        let h = ColumnHints::default();
        let f = cell_features(&text(0, 0, " (1.200,50 RSD) "), h);
        for n in ["v_is_text", "v_numeric_text", "v_has_currency", "v_is_negative", "v_is_padded"] {
            assert_eq!(get(&f, n), 1.0, "{n}");
        }
        assert_eq!(get(&f, "v_is_integer"), 0.0);
        let f = cell_features(&text(0, 0, "12%"), h);
        assert_eq!(get(&f, "v_is_percent"), 1.0);
        let f = cell_features(&text(0, 0, "2024"), h);
        assert_eq!((get(&f, "v_is_year"), get(&f, "v_numeric_text")), (1.0, 1.0));
        let f = cell_features(&text(0, 0, "2024."), h);
        assert_eq!(get(&f, "v_is_year"), 1.0);
        let f = cell_features(&text(0, 0, "12.03.2024."), h);
        assert_eq!((get(&f, "v_date_text"), get(&f, "v_numeric_text")), (1.0, 0.0));
        let f = cell_features(&text(0, 0, "n/a"), h);
        assert_eq!((get(&f, "v_missing_token"), get(&f, "v_is_error")), (1.0, 0.0));
        let f = cell_features(&text(0, 0, "#N/A"), h);
        assert_eq!((get(&f, "v_missing_token"), get(&f, "v_is_error")), (1.0, 1.0));
        let f = cell_features(&text(0, 0, "УКУПНО:"), h);
        assert_eq!((get(&f, "v_kw_total"), get(&f, "v_has_cyrillic"), get(&f, "v_ends_with_colon")), (1.0, 1.0, 1.0));
        assert_eq!(get(&f, "v_upper_ratio"), 1.0);
        let f = cell_features(&text(0, 0, "Izvor: RZS"), h);
        assert_eq!(get(&f, "v_kw_note"), 1.0);
        let f = cell_features(&text(0, 0, "1. Prihodi od poreza"), h);
        assert_eq!((get(&f, "v_starts_with_enumerator"), get(&f, "v_word_count")), (1.0, 4.0));
        let f = cell_features(&text(0, 0, "Q3"), h);
        assert_eq!(get(&f, "v_period_label"), 1.0);
        let f = cell_features(&text(0, 0, "Da"), h);
        assert_eq!(get(&f, "v_bool_text"), 1.0);
        let f = cell_features(&text(0, 0, "00123"), h);
        assert_eq!(get(&f, "v_leading_zero"), 1.0);
    }

    #[test]
    fn column_hints_resolve_ambiguity() {
        let sheet = Sheet {
            name: "s".into(),
            cells: vec![text(0, 0, "1.200"), text(0, 1, "1.200"), text(1, 0, "3,5"), text(1, 1, "3.25")],
            merges: vec![],
            formats: vec![],
        };
        let grid = Grid::new(&sheet);
        let hints = sheet_column_hints(&grid);
        assert_eq!(hints[&0].decimal, Some(DecimalSep::Comma));
        assert_eq!(hints[&1].decimal, Some(DecimalSep::Dot));
        let mut out = Features::new(sheet.cells.len());
        compute(&grid, &mut out);
        assert_eq!(out.names().len(), N);
        // "1.200" in column 0 is 1200, in column 1 it is 1.2.
        let l = out.get("v_log_abs").unwrap();
        assert!((l[0] - 1201f32.log10()).abs() < 1e-4);
        assert!((l[1] - 2.2f32.log10()).abs() < 1e-4);
        assert_eq!(out.get("v_is_integer").unwrap()[..2], [1.0, 0.0]);
    }

    #[test]
    fn infer_column_combines() {
        let h = infer_column(["1.200,50", "25/12/2024"]);
        assert_eq!(h, ColumnHints { decimal: Some(DecimalSep::Comma), date_order: Some(DateOrder::DayMonth), currency_context: false });
        assert!(infer_column(["ALL 5", "Amount (USD)"]).currency_context);
        assert!(!infer_column(["ALL 5", "TOP 10"]).currency_context);
    }
}
