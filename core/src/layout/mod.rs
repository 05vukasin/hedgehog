//! Owned by the `layout` workstream — see PLAN.md.
//!
//! Rule-based table detection and cell roles (M2):
//! 1. blocks: 8-connected components of filled cells + merged areas, merged across 1–2 blank
//!    spacer rows / 1 spacer column when the pieces look like one table ([`blocks`]);
//! 2. per region: title rows above, note rows below, header depth (multi-row, merged), splits of
//!    stacked tables at header-like rows, totals (keywords / `SUM(` above), group-header rows ([`tables`]);
//! 3. filled cells outside tables become Title / Note / Other by position and wording.
//!
//! All thresholds live in [`params`]. Cost is O(cells + merge perimeters), never O(used range area).

mod blocks;
mod changepoint;
mod cells;
pub mod params;
mod tables;

use crate::analysis::{Role, SheetLayout};
use crate::grid::Grid;
use crate::model::Sheet;

pub(crate) use cells::{is_note_label, is_rule_text, is_unit_text};
use cells::{is_aggregate_formula, is_total_label, text_of, Kind};
use params::{LOOSE_AGGREGATES_AS_TOTAL, LOOSE_MAX_ROWS, NOTE_MAX_GAP, NOTE_MIN_LEN, TITLE_MAX_GAP};
use tables::Out;

/// Sheets with at least this many cells detect total rows and blocks on two threads.
const PARALLEL_MIN_CELLS: usize = 100_000;

/// Detects tables and assigns one [`Role`] per entry of `grid.sheet.cells` (same order).
pub fn analyze(grid: &Grid) -> SheetLayout {
    analyze_inner(grid, params::COLUMN_NUMBER_ROWS, true)
}

/// [`analyze`] as the learned row model's features see it: without the column-numbering rule
/// (the model was trained on layouts without it, and a numbering row's role feeds its neighbours'
/// features). The model path applies the rule itself afterwards (`learned::tables`). Likewise
/// without the register rule for incidental total words ([`crate::totals::incidental_keyword`]),
/// which the model path applies to its output (`learned::analyze_with`).
pub fn analyze_for_model(grid: &Grid) -> SheetLayout {
    analyze_inner(grid, false, false)
}

fn analyze_inner(grid: &Grid, column_numbers: bool, registers: bool) -> SheetLayout {
    let sheet = grid.sheet;
    let trace = crate::normalize::trace_ms;
    let totals = || {
        let t = std::time::Instant::now();
        let rows = if registers { crate::totals::labelled_rows(grid) } else { Vec::new() };
        trace("layout.totals", t.elapsed());
        rows
    };
    let blocks = || {
        let t = std::time::Instant::now();
        let regions = blocks::regions(grid);
        trace("layout.blocks", t.elapsed());
        regions
    };
    // The total rows and the blocks are independent reads of the grid: on a large sheet they
    // run side by side (B120).
    let (labelled_totals, regions) = if registers && sheet.cells.len() >= PARALLEL_MIN_CELLS {
        std::thread::scope(|s| {
            let h = s.spawn(totals);
            let regions = blocks();
            (h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)), regions)
        })
    } else {
        (totals(), blocks())
    };
    let mut out = Out {
        roles: vec![Role::Other; sheet.cells.len()],
        tables: Vec::new(),
        loose: Vec::new(),
        column_numbers,
        registers,
        labelled_totals,
    };
    let t = std::time::Instant::now();
    let (sigs, search) = changepoint::time_spent();
    for region in regions {
        tables::analyze_region(sheet, region, &mut out);
    }
    let (sigs2, search2) = changepoint::time_spent();
    let (sigs, search) = (sigs2.saturating_sub(sigs), search2.saturating_sub(search));
    trace("layout.regions", t.elapsed().saturating_sub(sigs + search));
    trace("layout.cp_signatures", sigs);
    trace("layout.cp_search", search);
    let t = std::time::Instant::now();
    classify_loose(sheet, &mut out);
    out.tables.sort_by_key(|t| (t.rect.top, t.rect.left));
    trace("layout.loose", t.elapsed());
    SheetLayout { tables: out.tables, roles: out.roles }
}

/// Used by the model path (`learned::tables`) so its tables never overlap: where two rectangles do
/// (a vertical merge hanging below a table's last row into the next one, a narrow table beside a
/// wide one), one of them gives up the shared part — whichever cut (an edge of either rectangle moved to just outside the other) keeps
/// both headers and loses the fewest positions. `tables` is sorted by (top, left) and stays so.
pub(crate) fn separate_tables(tables: &mut [crate::analysis::DetectedTable]) {
    use crate::model::Rect;
    let area = |r: Rect| (r.bottom - r.top + 1) as u64 * (r.right - r.left + 1) as u64;
    for j in 1..tables.len() {
        for i in 0..j {
            let (a, b) = (tables[i].rect, tables[j].rect);
            if !(a.top <= b.bottom && b.top <= a.bottom && a.left <= b.right && b.left <= a.right) {
                continue;
            }
            // (which table, its new rectangle), for every cut that leaves it non-empty.
            let mut cuts: Vec<(usize, Rect)> = Vec::new();
            for (k, x, y) in [(i, a, b), (j, b, a)] {
                if y.top > x.top {
                    cuts.push((k, Rect { bottom: y.top - 1, ..x }));
                }
                if y.bottom < x.bottom {
                    cuts.push((k, Rect { top: y.bottom + 1, ..x }));
                }
                if y.left > x.left {
                    cuts.push((k, Rect { right: y.left - 1, ..x }));
                }
                if y.right < x.right {
                    cuts.push((k, Rect { left: y.right + 1, ..x }));
                }
            }
            // A cut that takes a table's header away from it comes last.
            let lost = |&(k, r): &(usize, Rect)| {
                let t = &tables[k];
                let headless = t.header_rows > 0 && !(r.top <= t.header_top && t.header_top <= r.bottom);
                (headless, area(t.rect) - area(r))
            };
            let Some((k, r)) = cuts.iter().min_by_key(|c| lost(c)).copied() else { continue };
            let t = &mut tables[k];
            t.rect = r;
            t.header_top = t.header_top.clamp(r.top, r.bottom);
            t.header_rows = t.header_rows.min(r.bottom + 1 - t.header_top);
        }
    }
    tables.sort_by_key(|t| (t.rect.top, t.rect.left));
}

/// Column number of a cell: a whole number `n`, alone or as `(n)`, `[n]`, `n)` or `n.`.
fn column_number(v: &crate::model::Value) -> Option<i64> {
    use crate::model::Value;
    match v {
        Value::Number(x) if x.fract() == 0.0 && *x >= 0.0 && *x < 1000.0 => Some(*x as i64),
        Value::Text(t) => {
            let t = t.trim();
            let t = t.strip_prefix(['(', '[']).unwrap_or(t);
            let t = t.strip_suffix([')', ']', '.']).unwrap_or(t);
            (!t.is_empty() && t.len() <= 3 && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse().ok()).flatten()
        }
        _ => None,
    }
}

/// A column-numbering row (Destatis, Eurostat: `1 2 3 4 5` under the header, so notes can say
/// "Sp. 3–4"), given its filled cells' values from left to right: at least three column numbers,
/// counting up by one from at least 1.
pub fn column_numbering<'a>(values: impl Iterator<Item = &'a crate::model::Value>) -> bool {
    let mut prev: Option<i64> = None;
    let mut n = 0;
    for v in values {
        let Some(k) = column_number(v) else { return false };
        if prev.map_or(k < 1, |p| k != p + 1) {
            return false;
        }
        prev = Some(k);
        n += 1;
    }
    n >= 3
}

/// A header of period numbers: a text label, then a column numbering (`Year | 1 | 2 | 3 | …`, the
/// years of a project model, B46). Given the row's filled cells' values from left to right.
pub fn labelled_numbering<'a>(values: impl Iterator<Item = &'a crate::model::Value>) -> bool {
    let mut it = values.peekable();
    matches!(it.next(), Some(crate::model::Value::Text(t)) if !t.trim().is_empty() && column_number(&crate::model::Value::Text(t.clone())).is_none())
        && column_numbering(it)
}

/// Text near the top of a table → Title; marked or below a table → Note; the rest → Other.
fn classify_loose(sheet: &Sheet, out: &mut Out) {
    let first_top = out.tables.iter().map(|t| t.rect.top).min();
    for region in std::mem::take(&mut out.loose) {
        let r = region.rect;
        let small = region.rows.len() <= LOOSE_MAX_ROWS;
        let mut above = small && first_top.is_some_and(|t| r.bottom < t);
        let mut below = false;
        for t in &out.tables {
            let t = t.rect;
            let cols = r.left <= t.right.saturating_add(1) && r.right.saturating_add(1) >= t.left;
            if !cols || !small {
                continue;
            }
            if r.bottom < t.top && t.top - r.bottom - 1 <= TITLE_MAX_GAP {
                above = true;
            }
            if r.top > t.bottom && r.top - t.bottom - 1 <= NOTE_MAX_GAP {
                below = true;
            }
        }
        for row in &region.rows {
            for e in &row.entries {
                let Some(i) = e.cell else { continue };
                let cell = &sheet.cells[i];
                out.roles[i] = match e.kind {
                    _ if LOOSE_AGGREGATES_AS_TOTAL && cell.formula.as_deref().is_some_and(is_aggregate_formula) => {
                        Role::Total
                    }
                    Kind::Text if LOOSE_AGGREGATES_AS_TOTAL && is_total_label(text_of(cell).unwrap_or("")) => Role::Total,
                    Kind::Text => {
                        let t = text_of(cell).unwrap_or("");
                        if is_note_label(t) || (!above && t.trim().chars().count() >= NOTE_MIN_LEN) {
                            Role::Note
                        } else if above {
                            Role::Title
                        } else if below {
                            Role::Note
                        } else {
                            Role::Other
                        }
                    }
                    _ => Role::Other,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DetectedTable;
    use crate::model::{Rect, Value};

    #[test]
    fn column_numbering_counts_up_by_one() {
        let n = |xs: &[&str]| {
            let v: Vec<Value> = xs
                .iter()
                .map(|x| x.parse::<f64>().map_or_else(|_| Value::Text(x.to_string()), Value::Number))
                .collect();
            column_numbering(v.iter())
        };
        assert!(n(&["1", "2", "3", "4"]));
        assert!(n(&["(1)", "(2)", "(3)"]));
        assert!(n(&["3", "4", "5"]));
        assert!(!n(&["1", "2"]), "too short");
        assert!(!n(&["1", "3", "4"]), "gap");
        assert!(!n(&["0", "1", "2"]), "starts at 0");
        assert!(!n(&["1.5", "2.5", "3.5"]));
        assert!(!n(&["Year", "1", "2", "3"]));
        assert!(!n(&["2019", "2020", "2021"]), "years are a header of their own, not numbers");
    }

    fn t(top: u32, left: u32, bottom: u32, right: u32) -> DetectedTable {
        DetectedTable { rect: Rect { top, left, bottom, right }, header_top: top, header_rows: 1, confidence: 1.0 }
    }

    #[test]
    fn overlapping_tables_are_separated() {
        // A table hanging over the next one (Finanzstatistik T2 A17:IB43 over T3 A31:HO43).
        let mut v = vec![t(16, 0, 42, 235), t(30, 0, 42, 222)];
        separate_tables(&mut v);
        assert_eq!(v[0].rect, Rect { top: 16, left: 0, bottom: 29, right: 235 });
        assert_eq!(v[1].rect, Rect { top: 30, left: 0, bottom: 42, right: 222 });
        // A narrow table sharing one column with a wide one gives up that column.
        let mut v = vec![t(7, 2, 108, 10), t(21, 0, 60, 2)];
        separate_tables(&mut v);
        assert_eq!(v[0].rect, Rect { top: 7, left: 2, bottom: 108, right: 10 });
        assert_eq!(v[1].rect, Rect { top: 21, left: 0, bottom: 60, right: 1 });
    }
}
