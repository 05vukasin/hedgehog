//! Column typing: decide one dtype per column from all its values, then convert every value.
//!
//! A column is numeric / date / bool when at least 90 % of its non-missing values read as such
//! (native values or text parsed with the column's locale hints). Values that do not fit are kept
//! as text and listed in a warning — nothing is silently nulled. Integer-like text with leading
//! zeros (`00123`) keeps the column as text (IDs).

use std::collections::HashMap;

use super::{Dtype, Out, Transforms, T};
use crate::model::{a1, Value};
use crate::values::date::is_plausible_serial;
use crate::values::{
    missing_token, parse_bool, parse_date, parse_number, serial_to_ymd, ColumnHints, DateOrder, DecimalSep, MissingKind,
    NegativeStyle, ParsedDate, ParsedNumber,
};
use crate::values::text::clean;

/// Excel's cell text limit.
pub(super) const MAX_TEXT: usize = 32_767;
const TYPE_SHARE: f64 = 0.9;
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

pub(super) struct Typed {
    pub dtype: Dtype,
    pub unit: Option<String>,
    pub currency: Option<String>,
    pub decimal: Option<DecimalSep>,
    pub date_order: Option<DateOrder>,
    pub ambiguous: usize,
    pub values: Vec<Out>,
    pub warnings: Vec<String>,
}

/// Shortest decimal form, integers without `.0`.
pub(super) fn fmt_num(v: f64) -> String {
    let mut s = String::new();
    super::json::num(v, &mut s);
    s
}

/// `2024-03-31` or `2024-03-31 10:30:00` for an Excel serial.
pub(super) fn iso(serial: f64) -> String {
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
}

/// Types one column. `rows[i]` is the source row of `slots[i]`, `col` the source column (both
/// 0-based, for warnings). `first` are the hints for the typing pass (explicit locale or none);
/// `refine` is asked for the final hints of a numeric / date column, given a sample of its text
/// values and whether any value was ambiguous under `first` — so locale inference only runs on
/// columns where it can change something.
pub(super) fn type_column(
    name: &str,
    col: u32,
    slots: &[Slot],
    rows: &[u32],
    first: ColumnHints,
    refine: &mut dyn FnMut(&[&str], bool) -> ColumnHints,
    tr: &mut Transforms,
) -> Typed {
    // Pass 0: missing values (cheap) and the number of real values.
    let missing: Vec<Option<bool>> = slots
        .iter()
        .map(|s| match s {
            Slot::Val(Value::Text(t)) => missing_token(t).map(|k| k == MissingKind::Error),
            Slot::Val(Value::Error(_)) => Some(true),
            _ => None,
        })
        .collect();
    let n = slots.iter().zip(&missing).filter(|(s, m)| matches!(s, Slot::Val(_)) && m.is_none()).count();
    // A kind is out once its misses exceed what the 90 % rule allows.
    let allowed = n - (TYPE_SHARE * n as f64).ceil() as usize;

    // Pass 1: parse text with `first`, giving up on a kind as soon as it cannot win.
    let nh = first.number_hint();
    let (mut num_ok, mut lead0, mut date_ok, mut bool_ok) = (0usize, 0usize, 0usize, 0usize);
    let (mut num_miss, mut date_miss, mut bool_miss) = (0usize, 0usize, 0usize);
    let mut ambiguous_seen = false;
    let mut cands: Vec<Cand> = vec![Cand::default(); slots.len()];
    for (i, s) in slots.iter().enumerate() {
        let Slot::Val(v) = s else { continue };
        if missing[i].is_some() {
            continue;
        }
        let (mut is_num, mut is_date, mut is_bool) = (false, false, false);
        match v {
            Value::Number(_) => is_num = true,
            Value::DateTime(_) => is_date = true,
            Value::Bool(_) => is_bool = true,
            Value::Text(t) => {
                let c = &mut cands[i];
                if num_miss <= allowed && lead0 == 0 {
                    c.num = parse_number(t, nh);
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
            }
            Value::Error(_) => {}
        }
        if is_num { num_ok += 1 } else { num_miss += 1 }
        if is_date { date_ok += 1 } else { date_miss += 1 }
        if is_bool { bool_ok += 1 } else { bool_miss += 1 }
    }
    let share = |k: usize| n > 0 && k as f64 >= TYPE_SHARE * n as f64;
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
    } else if share(num_ok) && lead0 == 0 {
        Kind::Num
    } else {
        Kind::Text
    };

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
            hints = refine(&sample, ambiguous_seen);
            let reparse = ambiguous_seen || hints.currency_context != first.currency_context;
            if reparse && hints != first {
                let nh = hints.number_hint();
                for (i, s) in slots.iter().enumerate() {
                    if let (Slot::Val(Value::Text(t)), None) = (s, missing[i]) {
                        if kind == Kind::Num {
                            cands[i].num = parse_number(t, nh);
                        } else {
                            cands[i].date = parse_date(t, hints.date_order);
                        }
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
        for c in &cands {
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
    let mut has_time = false;
    let mut all_int = true;
    let mut values = Vec::with_capacity(slots.len());
    for (i, (s, c)) in slots.iter().zip(&cands).enumerate() {
        let c = (missing[i], c);
        let out = match s {
            Slot::Empty => Out::Null,
            Slot::NoCache => {
                tr.add(T::FormulasWithoutValue, 1);
                Out::Null
            }
            Slot::Val(v) => {
                if let Some(is_err) = c.0 {
                    tr.add(if is_err { T::ErrorsToNull } else { T::MissingToNull }, 1);
                    values.push(Out::Null);
                    continue;
                }
                let mut fallback = |tr: &mut Transforms| {
                    unparsed.push(a1(rows[i], col));
                    tr.add(T::UnparsedKeptAsText, 1);
                    Out::Text(fit(display(v), tr))
                };
                match (&kind, v) {
                    (Kind::Num, Value::Number(x)) => Out::Num(*x),
                    (Kind::Num, Value::Text(_)) => match &c.1.num {
                        Some(p) if !p.leading_zero => {
                            tr.add(T::NumbersFromText, 1);
                            tr.add(T::PercentFromText, p.percent as usize);
                            tr.add(T::CurrencyStripped, p.currency.is_some() as usize);
                            tr.add(T::UnitStripped, p.unit.is_some() as usize);
                            tr.add(T::ScaleApplied, (p.scale != 1.0) as usize);
                            tr.add(
                                T::AccountingNegatives,
                                matches!(p.negative, Some(NegativeStyle::Parens | NegativeStyle::TrailingMinus)) as usize,
                            );
                            ambiguous += p.ambiguous as usize;
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
                        Out::Text(fit(cl.into_owned(), tr))
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
    if ambiguous > 0 {
        warnings.push(format!(
            "column '{name}': {ambiguous} value(s) read with a guessed decimal separator or day/month order (pass a locale to override)"
        ));
    }
    Typed { dtype, unit, currency, decimal: hints.decimal, date_order: hints.date_order, ambiguous, values, warnings }
}
