//! Recomputes a total from the cached values of its source cells and compares.

use rustc_hash::FxHashSet;

use crate::grid::Grid;
use crate::model::{Rect, Value};
use crate::totals::detect::{Evidence, TotalCandidate, TotalOrigin};
use crate::totals::formula::{parse, FormulaKind, RefKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckReason {
    /// Equal within floating-point tolerance.
    Match,
    /// Typed total equals the sum rounded to its own number of decimals.
    Rounded,
    /// The total disagrees with its source (for formulas: a stale cached result).
    Mismatch,
    /// The total adds other totals on top of their data; `expected` excludes them.
    DoubleCounted,
    /// Typed value equals the average of the source, not the sum — probably not a sum total.
    MatchesAverage,
    /// The total cell has no cached value.
    NoCachedValue,
    /// The total cell holds text, a boolean or an error.
    NotNumeric,
    /// A source cell holds an error value.
    ErrorInSource,
    /// A source formula has no cached value.
    MissingSourceValue,
    /// Nothing numeric in the source and the total is not zero.
    NoSource,
    /// Typed total that disagrees with its inferred source, but neither it nor its row / column
    /// siblings confirmed that source arithmetically — the range guess may be wrong.
    Unconfirmed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TotalCheck {
    /// Recomputed sum of the source (without double-counted totals), if computable.
    pub expected: Option<f64>,
    /// The total cell's cached / typed value.
    pub actual: Option<f64>,
    /// `Some(true)` verified, `Some(false)` wrong, `None` unknown.
    pub ok: Option<bool>,
    /// `actual - expected`.
    pub diff: Option<f64>,
    pub reason: CheckReason,
    /// Numeric source cells that were added.
    pub summed: usize,
    /// Source cells holding numbers stored as text — Excel's SUM skips them, so they are noted, not summed
    /// (except as direct `+` operands, which Excel converts, and in typed totals, which were added
    /// up by hand).
    pub numbers_as_text: Vec<(u32, u32)>,
}

/// Parses text that is plainly a number ("1,234.50", " 12 ", "(500)", "-3.5", "12%").
/// Locale-aware parsing belongs to the values workstream; this only flags obvious cases.
pub fn numeric_text(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || t.len() > 40 {
        return None;
    }
    let (t, neg) = match t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        Some(x) => (x, true),
        None => (t, false),
    };
    let (t, pct) = match t.strip_suffix('%') {
        Some(x) => (x.trim_end(), true),
        None => (t, false),
    };
    if !t.bytes().any(|b| b.is_ascii_digit()) || !t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b',' | b'.' | b'-' | b'+')) {
        return None;
    }
    let v: f64 = t.replace(',', "").parse().ok()?;
    let v = if pct { v / 100.0 } else { v };
    Some(if neg { -v } else { v })
}

/// Float equality for totals. `loose` additionally accepts a typed value rounded to its own
/// decimals (0–2): 1235 for 1234.6.
pub fn close(actual: f64, expected: f64, loose: bool) -> bool {
    let d = (actual - expected).abs();
    if d <= 1e-9 + 1e-9 * actual.abs().max(expected.abs()) {
        return true;
    }
    loose && rounded_match(actual, expected)
}

fn rounded_match(actual: f64, expected: f64) -> bool {
    let d = (actual - expected).abs();
    for dec in 0..=2 {
        let scale = 10f64.powi(dec);
        let scaled = actual * scale;
        if (scaled - scaled.round()).abs() <= 1e-6 * scaled.abs().max(1.0) {
            return d <= 0.5 / scale + 1e-9 * expected.abs().max(1.0);
        }
    }
    false
}

fn clamp(r: &Rect, used: &Rect) -> Option<Rect> {
    let c = Rect {
        top: r.top.max(used.top),
        left: r.left.max(used.left),
        bottom: r.bottom.min(used.bottom),
        right: r.right.min(used.right),
    };
    (c.top <= c.bottom && c.left <= c.right).then_some(c)
}

pub fn verify(grid: &Grid, cand: &TotalCandidate) -> TotalCheck {
    let mut check = TotalCheck {
        expected: None,
        actual: None,
        ok: None,
        diff: None,
        reason: CheckReason::Match,
        summed: 0,
        numbers_as_text: Vec::new(),
    };
    let cell = grid.cell(cand.row, cand.col);
    check.actual = match cell.and_then(|c| c.value.as_ref()) {
        Some(Value::Number(v) | Value::DateTime(v)) => Some(*v),
        Some(_) => {
            check.reason = CheckReason::NotNumeric;
            return check;
        }
        None => {
            check.reason = CheckReason::NoCachedValue;
            None
        }
    };
    // Direct `+` operands (single cells of an addition chain) convert numeric text like Excel does.
    // The parse the dependency index already holds, if any (B132).
    let parsed_here;
    let parsed = match (cell.and_then(|c| c.formula.as_deref()), grid.parsed_formula(cand.row, cand.col)) {
        (Some(_), Some(p)) => Some(p),
        (Some(f), None) => {
            parsed_here = parse(f);
            Some(&parsed_here)
        }
        (None, _) => None,
    };
    let coerce: Vec<(u32, u32)> = parsed
        .filter(|p| matches!(p.kind, FormulaKind::AdditionChain | FormulaKind::Sum | FormulaKind::Subtotal))
        .map(|p| p.sum_refs.iter().filter(|r| r.kind == RefKind::Cell).map(|r| (r.rect.top, r.rect.left)).collect())
        .unwrap_or_default();

    let typed = cand.origin == TotalOrigin::Typed;
    let Some(used) = grid.used_range() else {
        check.reason = CheckReason::NoSource;
        return check;
    };
    let exclude: FxHashSet<(u32, u32)> = cand.exclude.iter().copied().collect();
    let double_counted: FxHashSet<(u32, u32)> = cand.double_counted.iter().copied().collect();
    let coerce: FxHashSet<(u32, u32)> = coerce.into_iter().collect();
    let mut seen_text: FxHashSet<(u32, u32)> = FxHashSet::default();
    let mut sum = 0.0;
    let mut values: Vec<f64> = Vec::new();
    let mut missing = false;
    let mut error = false;
    let mut double = 0.0;
    for r in &cand.ranges {
        let Some(r) = clamp(r, &used) else { continue };
        // Only positions holding a cell matter; visit them row-major like a scan of the area.
        for i in grid.cells_in(r) {
            let c = &grid.sheet.cells[i as usize];
            let pos = (c.row, c.col);
            if pos == (cand.row, cand.col) || exclude.contains(&pos) {
                continue;
            }
            let v = match &c.value {
                Some(Value::Number(v) | Value::DateTime(v)) => Some(*v),
                Some(Value::Text(s)) => match numeric_text(s) {
                    Some(v) => {
                        if seen_text.insert(pos) {
                            check.numbers_as_text.push(pos);
                        }
                        (typed || coerce.contains(&pos)).then_some(v)
                    }
                    None => None,
                },
                Some(Value::Error(_)) => {
                    error = true;
                    None
                }
                Some(Value::Bool(_)) => None,
                None => {
                    missing |= c.formula.is_some();
                    None
                }
            };
            if let Some(v) = v {
                sum += v;
                values.push(v);
                if double_counted.contains(&pos) {
                    double += v;
                }
            }
        }
    }
    check.summed = values.len();
    if error {
        check.reason = CheckReason::ErrorInSource;
        return check;
    }
    if missing {
        check.reason = CheckReason::MissingSourceValue;
        return check;
    }
    let expected = sum - double;
    check.expected = Some(expected);
    let Some(actual) = check.actual else { return check };
    check.diff = Some(actual - expected);
    if close(actual, expected, false) {
        check.ok = Some(true);
        check.reason = CheckReason::Match;
    } else if double != 0.0 && close(actual, sum, typed) {
        check.ok = Some(false);
        check.reason = CheckReason::DoubleCounted;
    } else if typed && close(actual, expected, true) {
        check.ok = Some(true);
        check.reason = CheckReason::Rounded;
    } else if values.is_empty() {
        check.reason = CheckReason::NoSource;
    } else if typed && values.len() > 1 && close(actual, expected / values.len() as f64, true) {
        check.reason = CheckReason::MatchesAverage;
    } else if typed
        && !cand.evidence.iter().any(|e| matches!(e, Evidence::ArithmeticMatch | Evidence::SiblingsMatch))
    {
        check.reason = CheckReason::Unconfirmed;
    } else {
        check.ok = Some(false);
        check.reason = CheckReason::Mismatch;
    }
    check
}

#[cfg(test)]
mod tests {
    use super::{close, numeric_text};

    #[test]
    fn numeric_text_cases() {
        assert_eq!(numeric_text(" 12 "), Some(12.0));
        assert_eq!(numeric_text("1,234.5"), Some(1234.5));
        assert_eq!(numeric_text("(500)"), Some(-500.0));
        assert_eq!(numeric_text("12%"), Some(0.12));
        assert_eq!(numeric_text("Beograd"), None);
        assert_eq!(numeric_text("2019-01-01"), None);
        assert_eq!(numeric_text("-"), None);
        assert_eq!(numeric_text("Q1"), None);
    }

    #[test]
    fn tolerance() {
        assert!(close(0.3, 0.1 + 0.2, false));
        assert!(close(1e12, 1e12 + 1e-4, false));
        assert!(!close(100.0, 101.0, false));
        assert!(close(1235.0, 1234.6, true));
        assert!(!close(1235.0, 1234.4, true));
        assert!(close(12.35, 12.346, true));
        assert!(!close(1240.0, 1234.6, true));
    }
}
