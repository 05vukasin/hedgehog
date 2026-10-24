//! Column typing: decide one dtype per column from all its values, then convert every value.
//!
//! A column is numeric / date / bool when at least 90 % of its non-missing values read as such
//! (native values or text parsed with the column's locale hints). Values that do not fit are kept
//! as text and listed in a warning — nothing is silently nulled. Integer-like text with leading
//! zeros (`00123`) keeps the column as text (IDs).
//!
//! Text that statistical tables put in numeric columns on purpose — censored values (`<5`,
//! `> 1000`), suppression marks (`c`, `[c]`, `confidential`) and `see note` — stays text but does
//! not count against the numeric share, as long as the numbers outnumber it. Numbers with footnote
//! marks (`123¹`, `12 (r)`, `7 p`, `20560.32e`) are read as numbers; the marks are kept per column
//! (`Column::footnote_marks`, the manifest's `footnote_marks`). Text labels lose their footnote
//! marks the same way (`Delta²` → `Delta`, B142) when few of the column's labels carry one; marks
//! that are part of a name or unit stay (`Fläche km²`, `CO₂`, `Area 51`).

use std::collections::{BTreeMap, HashMap};

use super::{Dtype, Out, Transforms, T};
use crate::model::{a1, Value};
use crate::values::date::is_plausible_serial;
use crate::values::{
    numeric_annotation, parse_bool, table_missing_token, parse_date, parse_number, serial_to_ymd, ColumnHints, DateOrder,
    DecimalSep, MissingKind, NativeNumbers, NegativeStyle, ParsedDate, ParsedNumber,
};
use crate::values::text::{clean, may_end_in_footnote, split_label_footnote};

/// Excel's cell text limit.
pub(super) const MAX_TEXT: usize = 32_767;
const TYPE_SHARE: f64 = 0.9;
/// A numeric column may hold up to this share of a few distinct words (`approx.`, `pending`).
const WORDS_NUM_SHARE: f64 = 0.75;
/// Distinct words allowed by [`WORDS_NUM_SHARE`].
const MAX_DISTINCT_WORDS: usize = 3;
const MAX_LISTED: usize = 10;
/// Text values used to infer a column's decimal separator / day-month order.
const INFER_SAMPLE: usize = 2000;

/// One source value of a column.
#[derive(Debug, Clone, Copy)]
pub(super) enum Slot<'a> {
    Empty,
    /// Formula cell without a cached value.
    NoCache,
    Val(&'a Value),
}

/// `x` / `X` (one letter), `✓`, `✔`: a mark that flags a row.
pub(crate) fn is_flag_mark(t: &str) -> bool {
    matches!(t.trim(), "x" | "X" | "✓" | "✔")
}

/// Every filled cell of the column is a flag mark (at least two), and some cells are blank.
fn flag_column(slots: &[Slot]) -> bool {
    let (mut marks, mut blanks) = (0usize, 0usize);
    for s in slots {
        match s {
            Slot::Val(Value::Text(t)) if is_flag_mark(t) => marks += 1,
            Slot::Val(Value::Text(t)) if t.trim().is_empty() => blanks += 1,
            // a formula without a saved value shows nothing either
            Slot::Empty | Slot::NoCache => blanks += 1,
            _ => return false,
        }
    }
    marks >= 2 && blanks >= 1
}

pub(super) struct Typed {
    pub dtype: Dtype,
    pub unit: Option<String>,
    pub currency: Option<String>,
    pub decimal: Option<DecimalSep>,
    pub date_order: Option<DateOrder>,
    pub ambiguous: usize,
    pub values: Vec<Out>,
    /// Footnote marks stripped from numbers: mark → source cells (0-based row, col).
    pub footnote_marks: Vec<(String, Vec<(u32, u32)>)>,
    pub warnings: Vec<String>,
}

/// Shortest decimal form, integers without `.0`.
pub fn fmt_num(v: f64) -> String {
    let mut s = String::new();
    super::json::num(v, &mut s);
    s
}

/// `2024-03-31` or `2024-03-31 10:30:00` for an Excel serial; a time of day without a date (a
/// serial in [0, 1), from an `h:mm` cell) is `10:30:00`, not the fraction `0.4375`.
pub fn iso(serial: f64) -> String {
    if (0.0..1.0).contains(&serial) {
        let secs = ((serial * 86400.0).round() as u32).min(86399);
        return format!("{:02}:{:02}:{:02}", secs / 3600 % 24, secs / 60 % 60, secs % 60);
    }
    let Some((y, m, d)) = serial_to_ymd(serial) else { return fmt_num(serial) };
    let frac = serial - serial.floor();
    if frac.abs() < 1e-9 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        let secs = (frac * 86400.0).round() as u32;
        format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", secs / 3600 % 24, secs / 60 % 60, secs % 60)
    }
}

/// Display text of a source value (headers, notes, text columns).
pub(super) fn display(v: &Value) -> String {
    match v {
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Number(n) => fmt_num(*n),
        Value::DateTime(d) => iso(*d),
        Value::Text(s) => clean(s).into_owned(),
        Value::Error(e) => e.clone(),
    }
}

/// Truncates to Excel's cell limit, counting it.
pub(super) fn fit(mut s: String, tr: &mut Transforms) -> String {
    if s.len() > MAX_TEXT && s.chars().count() > MAX_TEXT {
        s = s.chars().take(MAX_TEXT).collect();
        tr.add(T::TextTruncated, 1);
    }
    s
}

fn most_common(counts: &HashMap<&'static str, usize>) -> Option<(&'static str, usize)> {
    let mut v: Vec<_> = counts.iter().map(|(k, n)| (*k, *n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.first().copied()
}

fn list(addrs: &[String]) -> String {
    let mut s = addrs.iter().take(MAX_LISTED).cloned().collect::<Vec<_>>().join(", ");
    if addrs.len() > MAX_LISTED {
        s.push_str(&format!(" … (+{})", addrs.len() - MAX_LISTED));
    }
    s
}

/// Parsed readings of one text value (each `None` when not tried or not parseable).
#[derive(Default, Clone, Copy)]
struct Cand {
    num: Option<ParsedNumber>,
    date: Option<ParsedDate>,
    boolean: Option<bool>,
    /// Censored / suppressed / see-note text: not a number, but no evidence against one.
    annotation: bool,
    /// `num` read from an accounting negative missing its closing parenthesis (`(11,134`).
    lone_paren: bool,
}

impl Cand {
    /// Anything read (a value without readings is the same as `Cand::default()`).
    fn has_reading(&self) -> bool {
        self.num.is_some() || self.date.is_some() || self.boolean.is_some() || self.annotation
    }
}

/// A number, or an accounting negative written without its closing parenthesis (`(11,134`, as
/// StatCan tables have it): `(` then a number and no `)` reads as that number in parentheses.
/// The second value says the parenthesis was missing (the column lists those cells in a warning).
fn parse_num_lenient(t: &str, nh: crate::values::NumberHint) -> (Option<ParsedNumber>, bool) {
    if let Some(p) = parse_number(t, nh) {
        return (Some(p), false);
    }
    let s = t.trim();
    let lone = s.strip_prefix('(').is_some_and(|r| r.trim_start().starts_with(|c: char| c.is_ascii_digit())) && !s.contains(')');
    if !lone {
        return (None, false);
    }
    match parse_number(&format!("{s})"), nh) {
        Some(p) if p.negative == Some(NegativeStyle::Parens) && !p.leading_zero => (Some(p), true),
        _ => (None, false),
    }
}

/// A column header that names a date: `Date`, `Care Date`, `Datum`, `Lieferdatum`, `Fecha de
/// pago`, `Data`, `Дата`, `日期`, … (the last part of a ` / ` path).
fn date_header(name: &str) -> bool {
    let last = name.rsplit(" / ").next().unwrap_or(name).to_lowercase();
    if ["日期", "日付", "날짜"].iter().any(|w| last.contains(w)) {
        return true;
    }
    last.split(|c: char| !c.is_alphanumeric()).any(|w| {
        matches!(w, "date" | "dates" | "datum" | "fecha" | "data" | "dato" | "dátum" | "datė" | "tarih" | "päivämäärä" | "дата" | "датум")
            || (w.len() > 6 && w.ends_with("datum"))
    })
}

/// Every value of the column is a number in the range of everyday Excel dates (1954-2064), at
/// least two of them, not all round thousands.
fn serial_values(slots: &[Slot], missing: &[Option<bool>]) -> bool {
    let mut xs: Vec<f64> = Vec::new();
    for (s, m) in slots.iter().zip(missing) {
        match (s, m) {
            (Slot::Val(Value::Number(x)), None) => xs.push(*x),
            (Slot::Val(_), None) => return false,
            _ => {}
        }
    }
    xs.len() >= 2 && xs.iter().all(|&x| is_plausible_serial(x)) && crate::values::date::looks_like_serial_column(&xs)
}

/// `(4)`: a number of one to three digits in parentheses and nothing else, read as negative.
fn is_bare_paren(t: &str, p: &ParsedNumber) -> bool {
    let s = t.trim();
    p.negative == Some(NegativeStyle::Parens)
        && s.strip_prefix('(').and_then(|r| r.strip_suffix(')')).is_some_and(|d| (1..=3).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit()))
}

/// Types one column. `rows[i]` is the source row of `slots[i]`, `col` the source column (both
/// 0-based, for warnings). `first` are the hints for the typing pass (explicit locale or none);
/// `refine` is asked for the final hints of a numeric / date column, given a sample of its text
/// values, whether any value was ambiguous under `first` and the column's numbers stored as
/// numbers — so locale inference only runs on columns where it can change something.
pub(super) fn type_column(
    name: &str,
    col: u32,
    slots: &[Slot],
    rows: &[u32],
    first: ColumnHints,
    refine: &mut dyn FnMut(&[&str], bool, &NativeNumbers) -> ColumnHints,
    tr: &mut Transforms,
) -> Typed {
    type_column_untimed(name, col, slots, rows, first, refine, tr)
}

/// Adds the time of a table's column typing (for the trace).
pub(super) fn add_typing_time(d: std::time::Duration) {
    TYPING.with(|s| s.set(s.get() + d));
}

thread_local! {
    /// Time this thread spent typing columns ([`add_typing_time`]) (see [`take_typing_time`]).
    static TYPING: std::cell::Cell<std::time::Duration> = const { std::cell::Cell::new(std::time::Duration::ZERO) };
}

/// Time spent typing columns on this thread since the last call (for the trace).
pub(super) fn take_typing_time() -> std::time::Duration {
    TYPING.with(|s| s.replace(std::time::Duration::ZERO))
}

fn type_column_untimed(
    name: &str,
    col: u32,
    slots: &[Slot],
    rows: &[u32],
    first: ColumnHints,
    refine: &mut dyn FnMut(&[&str], bool, &NativeNumbers) -> ColumnHints,
    tr: &mut Transforms,
) -> Typed {
    // Pass 0: missing values (cheap) and the number of real values.
    let mut missing: Vec<Option<bool>> = slots
        .iter()
        .map(|s| match s {
            Slot::Val(Value::Text(t)) => table_missing_token(t).map(|k| k == MissingKind::Error),
            Slot::Val(Value::Error(_)) => Some(true),
            _ => None,
        })
        .collect();
    // A column of `X` marks and blanks (Enron "DUMP": X on some rows) is a flag: the mark is a
    // value, kept as written, so a flagged row stays apart from an unflagged one.
    if flag_column(slots) {
        for (s, m) in slots.iter().zip(missing.iter_mut()) {
            if matches!(s, Slot::Val(Value::Text(t)) if is_flag_mark(t)) {
                *m = None;
            }
        }
    }
    let n = slots.iter().zip(&missing).filter(|(s, m)| matches!(s, Slot::Val(_)) && m.is_none()).count();
    // A kind is out once its misses exceed what the 90 % rule allows.
    let allowed = n - (TYPE_SHARE * n as f64).ceil() as usize;
    let allowed_num = n - (WORDS_NUM_SHARE * n as f64).ceil() as usize;

    // Pass 1: parse text with `first`, giving up on a kind as soon as it cannot win.
    let nh = first.number_hint();
    let (mut num_ok, mut lead0, mut date_ok, mut bool_ok) = (0usize, 0usize, 0usize, 0usize);
    let (mut num_miss, mut date_miss, mut bool_miss) = (0usize, 0usize, 0usize);
    let mut annotations = 0usize;
    // Text misses of the numeric reading while they are all short words without digits.
    let mut words: Vec<&str> = Vec::new();
    let mut words_only = true;
    let mut native = NativeNumbers::default();
    let mut ambiguous_seen = false;
    // Readings of the text values that are not missing, by slot index in slot order; a text that
    // reads as nothing is left out (most texts of a text column: little memory per column, B134).
    let mut cands: Vec<(u32, Cand)> = Vec::new();
    // Item numbers in parentheses (`(4)`: an outline number or a footnote reference) read as
    // accounting negatives, by index into `cands`; and whether the column shows that it writes
    // amounts that way (another negative, a thousands separator).
    let mut bare_parens: Vec<usize> = Vec::new();
    let mut accounting = false;
    for (i, s) in slots.iter().enumerate() {
        let Slot::Val(v) = s else { continue };
        if missing[i].is_some() {
            continue;
        }
        let (mut is_num, mut is_date, mut is_bool) = (false, false, false);
        let mut annotation = false;
        match v {
            Value::Number(x) => {
                is_num = true;
                accounting |= *x < 0.0;
                native.add(*x);
            }
            Value::DateTime(_) => is_date = true,
            Value::Bool(_) => is_bool = true,
            Value::Text(t) => {
                let mut cand = Cand::default();
                let c = &mut cand;
                if num_miss <= allowed_num && lead0 == 0 {
                    (c.num, c.lone_paren) = parse_num_lenient(t, nh);
                    if let Some(p) = &c.num {
                        if p.leading_zero {
                            lead0 += 1;
                        } else {
                            is_num = true;
                            ambiguous_seen |= p.ambiguous;
                        }
                    }
                }
                if date_miss <= allowed {
                    c.date = parse_date(t, first.date_order);
                    if let Some(d) = &c.date {
                        is_date = true;
                        ambiguous_seen |= d.ambiguous;
                    }
                }
                if bool_miss <= allowed {
                    c.boolean = parse_bool(t);
                    is_bool = c.boolean.is_some();
                }
                if !is_num && c.num.is_none() && lead0 == 0 && num_miss <= allowed_num {
                    c.annotation = numeric_annotation(t).is_some();
                    annotation = c.annotation;
                    if !annotation && words_only {
                        let w = t.trim();
                        if w.chars().count() > 24 || w.chars().any(|ch| ch.is_ascii_digit()) {
                            words_only = false;
                        } else if !words.contains(&w) {
                            words.push(w);
                            words_only = words.len() <= MAX_DISTINCT_WORDS;
                        }
                    }
                }
                if let Some(p) = &cand.num {
                    if is_bare_paren(t, p) {
                        bare_parens.push(cands.len());
                    } else {
                        accounting |= p.negative.is_some() || p.thousands_sep.is_some();
                    }
                }
                if cand.has_reading() {
                    cands.push((i as u32, cand));
                }
            }
            Value::Error(_) => {}
        }
        if is_num {
            num_ok += 1
        } else if annotation {
            annotations += 1
        } else {
            num_miss += 1;
            words_only &= matches!(v, Value::Text(_));
        }
        if is_date { date_ok += 1 } else { date_miss += 1 }
        if is_bool { bool_ok += 1 } else { bool_miss += 1 }
    }
    // Without such evidence `(4)` is no amount: it stays text (and counts against a numeric
    // column like any other text).
    if !accounting {
        for &k in &bare_parens {
            if cands[k].1.num.take().is_some() {
                num_ok -= 1;
                num_miss += 1;
                words_only = false;
            }
        }
    }
    let share = |k: usize| n > 0 && k as f64 >= TYPE_SHARE * n as f64;
    // Censored / suppressed cells neither count for nor against a numeric column (when there are
    // more numbers than them, or nothing else).
    // A few distinct words among many numbers (one `approx.` in a short column) keep it numeric too.
    let num_share = num_ok > 0 && (num_ok > annotations || num_miss == 0) && {
        let hard = (n - annotations) as f64;
        let words_ok = words_only && num_ok >= 3 * num_miss && num_ok as f64 >= WORDS_NUM_SHARE * hard;
        num_ok as f64 >= TYPE_SHARE * hard || words_ok
    };
    #[derive(PartialEq)]
    enum Kind {
        Empty,
        Bool,
        Date,
        Num,
        Text,
    }
    let kind = if n == 0 {
        Kind::Empty
    } else if share(bool_ok) {
        Kind::Bool
    } else if share(date_ok) && date_ok >= num_ok {
        Kind::Date
    } else if num_share && lead0 == 0 {
        Kind::Num
    } else {
        Kind::Text
    };

    // Excel dates stored without a date format (plain serials 44301, 44302, …): a date column
    // only when its header says date and every value is such a serial.
    let kind = if kind == Kind::Num && date_header(name) && serial_values(slots, &missing) { Kind::Date } else { kind };

    // Pass 2 (numeric / date columns): final hints; re-parse only if they can change a value.
    let mut hints = first;
    if kind == Kind::Num || kind == Kind::Date {
        let sample: Vec<&str> = slots
            .iter()
            .zip(&missing)
            .filter_map(|(s, m)| match (s, m) {
                (Slot::Val(Value::Text(t)), None) => Some(t.as_str()),
                _ => None,
            })
            .take(INFER_SAMPLE)
            .collect();
        if !sample.is_empty() {
            hints = refine(&sample, ambiguous_seen, &native);
            let reparse = ambiguous_seen || hints.currency_context != first.currency_context;
            if reparse && hints != first {
                let nh = hints.number_hint();
                let mut old = std::mem::take(&mut cands).into_iter().peekable();
                for (i, s) in slots.iter().enumerate() {
                    let (Slot::Val(Value::Text(t)), None) = (s, missing[i]) else { continue };
                    let mut c = match old.next_if(|x| x.0 == i as u32) {
                        Some((_, c)) => c,
                        None => Cand::default(),
                    };
                    if kind == Kind::Num {
                        (c.num, c.lone_paren) = parse_num_lenient(t, nh);
                        if !accounting && c.num.as_ref().is_some_and(|p| is_bare_paren(t, p)) {
                            c.num = None;
                        }
                    } else {
                        c.date = parse_date(t, hints.date_order);
                    }
                    if c.has_reading() {
                        cands.push((i as u32, c));
                    }
                }
            }
        }
    }

    // Numeric subtype from the text values.
    let (mut pct, mut cur_n, mut unit_n) = (0usize, 0usize, 0usize);
    let mut curs: HashMap<&'static str, usize> = HashMap::new();
    let mut units: HashMap<&'static str, usize> = HashMap::new();
    if kind == Kind::Num {
        for (_, c) in &cands {
            if let Some(p) = &c.num {
                pct += p.percent as usize;
                if let Some(cc) = p.currency {
                    cur_n += 1;
                    *curs.entry(cc).or_default() += 1;
                }
                if let Some(u) = p.unit {
                    unit_n += 1;
                    *units.entry(u).or_default() += 1;
                }
            }
        }
    }

    let mut warnings = Vec::new();
    let mut ambiguous = 0usize;
    let mut unparsed: Vec<String> = Vec::new();
    let mut annotated: Vec<String> = Vec::new();
    let mut lone_parens: Vec<String> = Vec::new();
    let mut marks: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    let mut has_time = false;
    let mut all_int = true;
    let mut values = Vec::with_capacity(slots.len());
    // Text labels with footnote marks (B142): stripped when they are the exception in the column
    // (at most half of its texts), so a column of names that all end the same way stays.
    let strip_labels = kind == Kind::Text && {
        let (mut texts, mut marked) = (0usize, 0usize);
        for (s, m) in slots.iter().zip(&missing) {
            if let (Slot::Val(Value::Text(t)), None) = (s, m) {
                texts += 1;
                marked += (may_end_in_footnote(t) && split_label_footnote(&clean(t)).is_some()) as usize;
            }
        }
        marked > 0 && 2 * marked <= texts
    };
    // `None` / `Unknown` in a column of text labels is a category (`Number of recommendations met:
    // None | One | Two`, StatCan 2736), not a missing value: kept as written. Only in a text column
    // with two or more other distinct texts.
    let category_words = kind == Kind::Text && {
        let mut seen: Vec<&str> = Vec::new();
        for (s, m) in slots.iter().zip(&missing) {
            if let (Slot::Val(Value::Text(t)), None) = (s, m)
                && !seen.contains(&t.trim())
            {
                seen.push(t.trim());
                if seen.len() >= 2 {
                    break;
                }
            }
        }
        seen.len() >= 2
    };
    // `N.B.` (New Brunswick) and `N.V.` among labels are names too; the German "nicht bekannt" /
    // "nicht verfügbar" marks are missing values only in a column of numbers.
    let is_category_word =
        |v: &Value| matches!(v, Value::Text(t) if matches!(crate::values::norm_key(t).as_str(), "none" | "unknown" | "n.b." | "n.v."));
    // The previous slot's value: a label filled down from it (same cell) records its mark once.
    let mut last_val: Option<&Value> = None;
    let no_reading = Cand::default();
    let mut next_cand = cands.iter().peekable();
    for (i, s) in slots.iter().enumerate() {
        let c = match next_cand.next_if(|x| x.0 == i as u32) {
            Some((_, c)) => c,
            None => &no_reading,
        };
        let c = (missing[i], c);
        let this = match s {
            Slot::Val(v) => Some(*v),
            _ => None,
        };
        let prev = std::mem::replace(&mut last_val, this);
        let out = match s {
            Slot::Empty => Out::Null,
            Slot::NoCache => {
                tr.add(T::FormulasWithoutValue, 1);
                Out::Null
            }
            Slot::Val(v) => {
                if c.0 == Some(false) && category_words && is_category_word(v) {
                    values.push(Out::Text(fit(display(v).trim().to_string(), tr)));
                    continue;
                }
                if let Some(is_err) = c.0 {
                    tr.add(if is_err { T::ErrorsToNull } else { T::MissingToNull }, 1);
                    values.push(Out::Null);
                    continue;
                }
                let annotation = c.1.annotation && kind == Kind::Num;
                let mut fallback = |tr: &mut Transforms| {
                    if annotation {
                        annotated.push(format!("{} '{}'", a1(rows[i], col), display(v)));
                    } else {
                        unparsed.push(a1(rows[i], col));
                    }
                    tr.add(T::UnparsedKeptAsText, 1);
                    Out::Text(fit(display(v), tr))
                };
                match (&kind, v) {
                    (Kind::Num, Value::Number(x)) => Out::Num(*x),
                    (Kind::Num, Value::Text(t)) => match &c.1.num {
                        Some(p) if !p.leading_zero => {
                            tr.add(T::NumbersFromText, 1);
                            if let Some(m) = p.footnote_mark(t) {
                                tr.add(T::FootnoteMarksStripped, 1);
                                marks.entry(m.to_string()).or_default().push((rows[i], col));
                            }
                            tr.add(T::PercentFromText, p.percent as usize);
                            tr.add(T::CurrencyStripped, p.currency.is_some() as usize);
                            tr.add(T::UnitStripped, p.unit.is_some() as usize);
                            tr.add(T::ScaleApplied, (p.scale != 1.0) as usize);
                            tr.add(
                                T::AccountingNegatives,
                                matches!(p.negative, Some(NegativeStyle::Parens | NegativeStyle::TrailingMinus)) as usize,
                            );
                            ambiguous += p.ambiguous as usize;
                            if c.1.lone_paren {
                                lone_parens.push(format!("{} '{}'", a1(rows[i], col), display(v)));
                            }
                            Out::Num(p.normalized())
                        }
                        _ => fallback(tr),
                    },
                    (Kind::Date, Value::DateTime(x)) => Out::Date(*x),
                    (Kind::Date, Value::Text(_)) => match &c.1.date {
                        Some(d) if d.serial() >= 1.0 => {
                            tr.add(T::DatesFromText, 1);
                            ambiguous += d.ambiguous as usize;
                            Out::Date(d.serial())
                        }
                        _ => fallback(tr),
                    },
                    (Kind::Date, Value::Number(x)) if is_plausible_serial(*x) => {
                        tr.add(T::DatesFromSerialNumbers, 1);
                        Out::Date(*x)
                    }
                    (Kind::Bool, Value::Bool(b)) => Out::Bool(*b),
                    (Kind::Bool, Value::Text(_)) => match c.1.boolean {
                        Some(b) => {
                            tr.add(T::BoolsFromText, 1);
                            Out::Bool(b)
                        }
                        None => fallback(tr),
                    },
                    (Kind::Text | Kind::Empty, Value::Text(t)) => {
                        let cl = clean(t);
                        if cl.len() != t.len() {
                            tr.add(T::WhitespaceTrimmed, 1);
                        }
                        let split = if strip_labels && may_end_in_footnote(&cl) { split_label_footnote(&cl) } else { None };
                        match split {
                            Some((body, m)) => {
                                if !prev.is_some_and(|p| std::ptr::eq(p, *v)) {
                                    tr.add(T::FootnoteMarksStripped, 1);
                                    marks.entry(m.to_string()).or_default().push((rows[i], col));
                                }
                                Out::Text(fit(body.to_string(), tr))
                            }
                            None => Out::Text(fit(cl.into_owned(), tr)),
                        }
                    }
                    (Kind::Text | Kind::Empty, _) => {
                        tr.add(T::ValuesToText, 1);
                        Out::Text(display(v))
                    }
                    _ => fallback(tr),
                }
            }
        };
        match &out {
            Out::Num(x) => all_int &= x.fract() == 0.0 && x.abs() < 9.0e15,
            Out::Date(x) => has_time |= x.fract().abs() > 1e-9,
            _ => {}
        }
        values.push(out);
    }

    let dtype = match kind {
        Kind::Empty => Dtype::Empty,
        Kind::Bool => Dtype::Bool,
        Kind::Text => Dtype::Text,
        Kind::Date if has_time => Dtype::Datetime,
        Kind::Date => Dtype::Date,
        Kind::Num if pct * 2 > num_ok => Dtype::Percent,
        Kind::Num if cur_n * 2 > num_ok => Dtype::Currency,
        Kind::Num if all_int => Dtype::Integer,
        Kind::Num => Dtype::Number,
    };
    let currency = if cur_n * 2 > num_ok && kind == Kind::Num { most_common(&curs).map(|c| c.0.to_string()) } else { None };
    if curs.len() > 1 {
        let mut codes: Vec<_> = curs.keys().copied().collect();
        codes.sort_unstable();
        warnings.push(format!("column '{name}': mixed currencies {}", codes.join(", ")));
    }
    let unit = if unit_n * 2 > num_ok && kind == Kind::Num { most_common(&units).map(|u| u.0.to_string()) } else { None };
    if !unparsed.is_empty() {
        let what = match dtype {
            Dtype::Date | Dtype::Datetime => "dates",
            Dtype::Bool => "booleans",
            _ => "numbers",
        };
        warnings.push(format!(
            "column '{name}' ({}): {} value(s) are not {what} and were kept as text: {}",
            dtype.as_str(),
            unparsed.len(),
            list(&unparsed)
        ));
    }
    if !annotated.is_empty() {
        warnings.push(format!(
            "column '{name}' ({}): {} censored, suppressed or see-note value(s) kept as text: {}",
            dtype.as_str(),
            annotated.len(),
            list(&annotated)
        ));
    }
    if !lone_parens.is_empty() {
        warnings.push(format!(
            "column '{name}' ({}): {} value(s) with an opening parenthesis but no closing one read as negative: {}",
            dtype.as_str(),
            lone_parens.len(),
            list(&lone_parens)
        ));
    }
    if ambiguous > 0 {
        warnings.push(format!(
            "column '{name}': {ambiguous} value(s) read with a guessed decimal separator or day/month order (pass a locale to override)"
        ));
    }
    let footnote_marks = marks.into_iter().collect();
    Typed {
        dtype,
        unit,
        currency,
        decimal: hints.decimal,
        date_order: hints.date_order,
        ambiguous,
        values,
        footnote_marks,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistical_non_significance_keeps_native_numbers_and_literal_marker() {
        let run = |values: &[Value]| {
            let slots: Vec<_> = values.iter().map(Slot::Val).collect();
            let rows: Vec<_> = (0..values.len() as u32).collect();
            let mut tr = Transforms::default();
            type_column("Correlation", 0, &slots, &rows, ColumnHints::default(), &mut |_, _, _| ColumnHints::default(), &mut tr)
        };
        let text = |s: &str| Value::Text(s.into());
        let typed = run(&[Value::Number(0.71), Value::Number(-0.26), text("N.S.")]);
        assert_eq!(typed.dtype, Dtype::Number);
        assert_eq!(typed.values, vec![Out::Num(0.71), Out::Num(-0.26), Out::Text("N.S.".into())]);
        assert!(typed.warnings.iter().any(|s| s.contains("non-significant")));
        for values in [vec![text("N.S.")], vec![text("N.S."), text("Alice"), text("Bob")],
            vec![text("001"), Value::Number(0.71), text("N.S.")],
            vec![Value::Number(0.71), Value::Number(-0.26), text("N.S")]] {
            let typed = run(&values);
            assert_eq!(typed.dtype, Dtype::Text, "{values:?}");
            assert_eq!(typed.values[0], Out::Text(display(&values[0])));
            assert!(!typed.warnings.iter().any(|s| s.contains("non-significant")));
        }
    }
}
