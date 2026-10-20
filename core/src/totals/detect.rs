//! Finds total, subtotal and grand-total cells: aggregate formulas over the cells above / to the
//! left, and typed-in numbers in rows (or columns) labelled with a total keyword.

use std::collections::{HashMap, HashSet};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::grid::Grid;
use crate::model::{Cell, Rect, Value};
use crate::totals::deps::DependencyIndex;
use crate::totals::formula::FormulaKind;
use crate::totals::keywords::{KeywordLevel, total_keyword};
use crate::totals::verify::{close, numeric_text, verify};
use crate::values::missing_token;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TotalOrigin {
    /// The cell holds an aggregate formula.
    Formula,
    /// A typed-in number in a row / column labelled as total.
    Typed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TotalLevel {
    Total,
    /// Total of one group inside a table, followed by more groups or a grand total.
    Subtotal,
    /// Total of totals (or of everything above, excluding subtotals).
    GrandTotal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Column total: adds cells above, in the same column.
    Vertical,
    /// Row total: adds cells to the left, in the same row.
    Horizontal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    /// The cell's formula is an aggregate of this kind.
    Formula(FormulaKind),
    /// The row (vertical) or column header (horizontal) carries a total keyword.
    Keyword(KeywordLevel),
    /// Its source contains other totals that it adds once more (double counting).
    ContainsTotals,
    /// It adds up other totals (or everything except the subtotals) — a grand total.
    SumsTotals,
    /// The typed value equals the sum of the inferred source (used to choose the source).
    ArithmeticMatch,
    /// "X Total" whose group starts at the row (row, col) labelled "X".
    GroupLabel(u32, u32),
    /// Typed total without an arithmetic match, but most other totals in its row / column match,
    /// so the inferred source is trusted and a mismatch is reported as wrong.
    SiblingsMatch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TotalCandidate {
    pub row: u32,
    pub col: u32,
    pub origin: TotalOrigin,
    pub level: TotalLevel,
    pub direction: Direction,
    /// Source ranges on the same sheet, in the order the total adds them.
    pub ranges: Vec<Rect>,
    /// Cells inside `ranges` the total does not add (nested SUBTOTALs, subtotal rows of a typed
    /// grand total).
    pub exclude: Vec<(u32, u32)>,
    /// Other totals inside `ranges` that this total adds again on top of their data.
    pub double_counted: Vec<(u32, u32)>,
    /// Label cell: the keyword cell, or the nearest text to the left (vertical) / above (horizontal).
    pub label: Option<(u32, u32)>,
    pub evidence: Vec<Evidence>,
}

impl TotalCandidate {
    fn new(row: u32, col: u32, origin: TotalOrigin, direction: Direction) -> Self {
        TotalCandidate {
            row,
            col,
            origin,
            level: TotalLevel::Total,
            direction,
            ranges: Vec::new(),
            exclude: Vec::new(),
            double_counted: Vec::new(),
            label: None,
            evidence: Vec::new(),
        }
    }

    /// True if (row, col) is one of the cells this total adds.
    pub fn covers(&self, row: u32, col: u32) -> bool {
        self.ranges.iter().any(|r| r.top <= row && row <= r.bottom && r.left <= col && col <= r.right)
            && !self.exclude.contains(&(row, col))
    }
}

/// One column's cells in row order, reduced to what the typed-total walks read, stored densely
/// (the cells themselves are row-major, so stepping down a column through them misses the cache).
struct ColumnView {
    rows: Vec<u32>,
    /// [`ColumnView::NUMBER`] | [`ColumnView::FORMULA`] | [`ColumnView::LABEL`] |
    /// [`ColumnView::NUMBER_TEXT`].
    flags: Vec<u8>,
    /// The number, or the number stored as text (0 when there is none).
    values: Vec<f64>,
}

impl ColumnView {
    const NUMBER: u8 = 1;
    const FORMULA: u8 = 2;
    /// Text that is not a number nor a missing token: a header, it ends a walk up.
    const LABEL: u8 = 4;
    /// A number stored as text ("1,200.50"): a typed total was added up by hand, so it counts.
    const NUMBER_TEXT: u8 = 8;

    fn new(grid: &Grid, col: u32) -> Self {
        let line = grid.col_cells(col);
        let mut v = ColumnView {
            rows: Vec::with_capacity(line.len()),
            flags: Vec::with_capacity(line.len()),
            values: Vec::with_capacity(line.len()),
        };
        for &i in line {
            let c = &grid.sheet.cells[i as usize];
            let mut num = number(Some(c));
            let mut f = 0;
            if num.is_some() {
                f |= Self::NUMBER;
            } else if let Some(v) = text(Some(c)).and_then(numeric_text) {
                num = Some(v);
                f |= Self::NUMBER_TEXT;
            }
            if c.formula.is_some() {
                f |= Self::FORMULA;
            }
            if text(Some(c)).is_some_and(header_text) {
                f |= Self::LABEL;
            }
            v.rows.push(c.row);
            v.flags.push(f);
            v.values.push(num.unwrap_or(0.0));
        }
        v
    }

    /// Index of the first cell at or below row `r`.
    fn below(&self, r: u32) -> usize {
        self.rows.partition_point(|&x| x < r)
    }
}

/// Set of row numbers as a bitset: membership in O(1) without hashing (hot in the walks below).
struct RowSet(Vec<u64>);

impl RowSet {
    fn new(rows: impl Iterator<Item = u32>) -> Self {
        let mut bits: Vec<u64> = Vec::new();
        for r in rows {
            let w = (r / 64) as usize;
            if w >= bits.len() {
                bits.resize(w + 1, 0);
            }
            bits[w] |= 1 << (r % 64);
        }
        RowSet(bits)
    }

    #[inline]
    fn contains(&self, r: &u32) -> bool {
        self.0.get((*r / 64) as usize).is_some_and(|w| w & (1 << (*r % 64)) != 0)
    }
}

fn text(cell: Option<&Cell>) -> Option<&str> {
    match cell?.value.as_ref()? {
        Value::Text(s) => Some(s),
        _ => None,
    }
}

/// Text that can head a column of numbers: not a number stored as text and not a missing-value
/// token ("n/a", "-", "#N/A"), which stand in for blank data cells.
fn header_text(s: &str) -> bool {
    numeric_text(s).is_none() && missing_token(s).is_none()
}

fn number(cell: Option<&Cell>) -> Option<f64> {
    match cell?.value.as_ref()? {
        Value::Number(v) | Value::DateTime(v) => Some(*v),
        _ => None,
    }
}

/// Row → (label column, level) for rows whose leftmost keyword text cell names a total and that
/// hold at least one number or formula right of it (a header row "Name | Q1 | Total" does not).
pub fn keyword_rows(grid: &Grid) -> HashMap<u32, (u32, KeywordLevel)> {
    keyword_rows_with(grid, &keyword_cells(grid))
}

/// Text cells naming a total, by index into `sheet.cells` (ascending).
fn keyword_cells(grid: &Grid) -> Vec<(usize, KeywordLevel)> {
    // Registers repeat the same texts on many rows: classify each distinct text once.
    let mut seen: FxHashMap<&str, Option<KeywordLevel>> = FxHashMap::default();
    grid.sheet
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match &c.value {
            Some(Value::Text(s)) => (*seen.entry(s.as_str()).or_insert_with(|| total_keyword(s))).map(|lvl| (i, lvl)),
            _ => None,
        })
        .collect()
}

fn keyword_rows_with(grid: &Grid, kw_cells: &[(usize, KeywordLevel)]) -> HashMap<u32, (u32, KeywordLevel)> {
    let kw_of: FxHashMap<usize, KeywordLevel> = kw_cells.iter().copied().collect();
    let mut out = HashMap::new();
    let mut label: Option<(u32, u32, KeywordLevel)> = None; // (row, col, level) of the current row
    for (i, c) in grid.sheet.cells.iter().enumerate() {
        if label.is_some_and(|(r, _, _)| r != c.row) {
            label = None;
        }
        if out.contains_key(&c.row) {
            continue;
        }
        match (&label, &c.value) {
            (None, Some(Value::Text(_))) => {
                if let Some(&level) = kw_of.get(&i) {
                    label = Some((c.row, c.col, level));
                }
            }
            (Some((r, lc, lvl)), v) if (c.formula.is_some() || matches!(v, Some(Value::Number(_)))) => {
                out.insert(*r, (*lc, *lvl));
            }
            _ => {}
        }
    }
    drop_repeated_labels(grid, &mut out);
    out
}

/// Shortest run of adjacent identical total labels read as a category value.
const MIN_REPEATED_LABEL_RUN: usize = 3;

/// Long-format registers repeat a measure name containing a total word on consecutive rows
/// ("Totaal van aantal eenheden" on 25k rows of a 163k-row table): a value of a category column,
/// not total rows. Real total rows are separated by data, or differ in wording.
fn drop_repeated_labels(grid: &Grid, rows: &mut HashMap<u32, (u32, KeywordLevel)>) {
    let label = |r: u32, lc: u32| text(grid.cell(r, lc)).map(|t| t.trim().to_lowercase());
    let mut sorted: Vec<(u32, u32)> = rows.iter().map(|(&r, &(lc, _))| (r, lc)).collect();
    sorted.sort_unstable();
    // Runs of consecutive rows with the same label column and text; a pair ("Total" over a
    // nested "Total") is still a report, a run of three or more is a column of values.
    let mut drop = Vec::new();
    let mut start = 0;
    while start < sorted.len() {
        let (r0, lc) = sorted[start];
        let text0 = label(r0, lc);
        let mut end = start + 1;
        while end < sorted.len()
            && sorted[end] == (sorted[end - 1].0 + 1, lc)
            && label(sorted[end].0, lc) == text0
        {
            end += 1;
        }
        if end - start >= MIN_REPEATED_LABEL_RUN {
            drop.extend(sorted[start..end].iter().map(|x| x.0));
        }
        start = end;
    }
    for r in drop {
        rows.remove(&r);
    }
}

fn is_label(cell: &Cell) -> bool {
    text(Some(cell)).is_some_and(|s| numeric_text(s).is_none())
}

/// Nearest label (non-numeric text) cell left of / above a position, memoized per row / column:
/// for each line, the coordinate of the last label at or before every cell, so a lookup is one
/// binary search instead of a walk over the whole line.
struct Labels<'g> {
    grid: &'g Grid<'g>,
    rows: FxHashMap<u32, Vec<u32>>,
    cols: FxHashMap<u32, Vec<u32>>,
}

impl<'g> Labels<'g> {
    fn new(grid: &'g Grid<'g>) -> Self {
        Labels { grid, rows: FxHashMap::default(), cols: FxHashMap::default() }
    }

    fn nearest(grid: &Grid, line: &[u32], at: impl Fn(&Cell) -> u32, memo: &[u32], before: u32) -> Option<u32> {
        let cells = &grid.sheet.cells;
        let end = line.partition_point(|&i| at(&cells[i as usize]) < before);
        let found = *memo.get(end.checked_sub(1)?)?;
        (found != u32::MAX).then_some(found)
    }

    fn prefix(grid: &Grid, line: &[u32], at: impl Fn(&Cell) -> u32) -> Vec<u32> {
        let mut last = u32::MAX;
        line.iter()
            .map(|&i| {
                let c = &grid.sheet.cells[i as usize];
                if is_label(c) {
                    last = at(c);
                }
                last
            })
            .collect()
    }

    /// Nearest text cell left of (row, col) in the same row.
    fn left(&mut self, row: u32, col: u32) -> Option<u32> {
        let grid = self.grid;
        let line = grid.row_cells(row);
        let memo = self.rows.entry(row).or_insert_with(|| Self::prefix(grid, line, |c| c.col));
        Self::nearest(grid, line, |c| c.col, memo, col)
    }

    /// Nearest text cell above (row, col) in the same column.
    fn above(&mut self, row: u32, col: u32) -> Option<u32> {
        let grid = self.grid;
        let line = grid.col_cells(col);
        let memo = self.cols.entry(col).or_insert_with(|| Self::prefix(grid, line, |c| c.row));
        Self::nearest(grid, line, |c| c.row, memo, row)
    }
}

pub fn detect(grid: &Grid) -> Vec<TotalCandidate> {
    detect_with(grid, &DependencyIndex::new(grid))
}

/// [`detect`] reusing an already built dependency index.
pub fn detect_with(grid: &Grid, deps: &DependencyIndex) -> Vec<TotalCandidate> {
    let sheet = grid.sheet;
    let kw_cells = keyword_cells(grid);
    let kw_rows = keyword_rows_with(grid, &kw_cells);
    let mut labels = Labels::new(grid);
    let mut out: Vec<TotalCandidate> = Vec::new();

    // Cells holding SUBTOTAL formulas, column → sorted rows (SUBTOTAL skips them).
    let mut subtotal_cells: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
    for g in deps.formulas() {
        if g.parsed.functions.iter().any(|n| n == "SUBTOTAL") {
            subtotal_cells.entry(g.col).or_default().push(g.row);
        }
    }
    for v in subtotal_cells.values_mut() {
        v.sort_unstable();
    }

    // (a) Aggregate formulas over cells above / to the left.
    for f in deps.formulas() {
        let p = &f.parsed;
        if !p.kind.is_sum() || p.has_error || p.sum_refs.is_empty() {
            continue;
        }
        if p.sum_refs.iter().any(|r| !r.is_local(&sheet.name)) {
            continue;
        }
        let vertical =
            p.sum_refs.iter().all(|r| r.rect.left == f.col && r.rect.right == f.col && r.rect.bottom < f.row);
        let horizontal = !vertical
            && p.sum_refs.iter().all(|r| r.rect.top == f.row && r.rect.bottom == f.row && r.rect.right < f.col);
        if !vertical && !horizontal {
            continue;
        }
        let dir = if vertical { Direction::Vertical } else { Direction::Horizontal };
        let mut c = TotalCandidate::new(f.row, f.col, TotalOrigin::Formula, dir);
        c.evidence.push(Evidence::Formula(p.kind));
        c.ranges = p.sum_refs.iter().map(|r| r.rect).collect();
        let keyword = if vertical {
            kw_rows.get(&f.row).filter(|(lc, _)| *lc < f.col).map(|&(lc, lvl)| ((f.row, lc), lvl))
        } else {
            labels
                .above(f.row, f.col)
                .and_then(|r| total_keyword(text(grid.cell(r, f.col))?).map(|lvl| ((r, f.col), lvl)))
        };
        let area: u64 = p.sum_refs.iter().map(|r| r.area()).sum();
        if area < 2 && keyword.is_none() {
            continue; // `=SUM(C5)` is a link, not a total
        }
        match keyword {
            Some((cell, lvl)) => {
                c.label = Some(cell);
                c.evidence.push(Evidence::Keyword(lvl));
                c.level = level_of(lvl);
            }
            None => {
                c.label = if vertical {
                    labels.left(f.row, f.col).map(|lc| (f.row, lc))
                } else {
                    labels.above(f.row, f.col).map(|r| (r, f.col))
                };
            }
        }
        if p.kind == FormulaKind::Subtotal {
            // SUBTOTAL skips cells that themselves hold SUBTOTAL formulas.
            for r in &c.ranges {
                let from = c.exclude.len();
                for col in r.left..=r.right {
                    let Some(rows) = subtotal_cells.get(&col) else {
                        continue;
                    };
                    let a = rows.partition_point(|&x| x < r.top);
                    let b = rows.partition_point(|&x| x <= r.bottom);
                    c.exclude.extend(rows[a..b].iter().map(|&row| (row, col)));
                }
                c.exclude[from..].sort_unstable(); // row-major, as the cells are listed
            }
        }
        out.push(c);
    }

    let formula_total: FxHashSet<(u32, u32)> = out.iter().map(|c| (c.row, c.col)).collect();
    let rows_with_cells = RowSet::new(sheet.cells.iter().map(|c| c.row));
    let kw_set = RowSet::new(kw_rows.keys().copied());
    let mut columns: FxHashMap<u32, ColumnView> = FxHashMap::default();

    // (b) Typed numbers in keyword rows → column totals over the cells above.
    let mut kw_sorted: Vec<_> = kw_rows.iter().map(|(&r, &v)| (r, v)).collect();
    kw_sorted.sort_unstable_by_key(|x| x.0);
    for &(row, (lc, lvl)) in &kw_sorted {
        let cols: Vec<u32> = grid
            .row_cells(row)
            .iter()
            .map(|&i| &sheet.cells[i as usize])
            .filter(|c| c.col > lc && c.formula.is_none())
            .filter(|c| matches!(c.value, Some(Value::Number(_))))
            .map(|c| c.col)
            .collect();
        let group = if cols.is_empty() { None } else { group_start(grid, &kw_rows, &rows_with_cells, row, lc) };
        for col in cols {
            let value = number(grid.cell(row, col)).unwrap_or(0.0);
            if let Some(c) = typed_vertical(
                grid,
                &kw_rows,
                &formula_total,
                &rows_with_cells,
                &kw_set,
                columns.entry(col).or_insert_with(|| ColumnView::new(grid, col)),
                group,
                row,
                col,
                lc,
                lvl,
                value,
            ) {
                out.push(c);
            }
        }
    }

    // (b') Typed numbers under a keyword column header → row totals over the cells to the left.
    let taken: HashSet<(u32, u32)> = out.iter().map(|c| (c.row, c.col)).collect();
    for &(i, lvl) in &kw_cells {
        let h = &sheet.cells[i];
        if kw_rows.get(&h.row).is_some_and(|&(lc, _)| lc != h.col) {
            continue; // a second keyword in a total row ("Total" … "Total %") is not a header
        }
        out.extend(typed_horizontal(grid, &taken, h.row, h.col, lvl));
    }

    drop_unconfirmed_inside_formulas(&mut out);
    resolve_nesting(grid, &mut out);
    mark_sibling_support(grid, &mut out);
    out.sort_by_key(|c| (c.row, c.col));
    out
}

fn level_of(k: KeywordLevel) -> TotalLevel {
    match k {
        KeywordLevel::Total => TotalLevel::Total,
        KeywordLevel::Subtotal => TotalLevel::Subtotal,
        KeywordLevel::GrandTotal => TotalLevel::GrandTotal,
    }
}

/// Group name announced by a total label: "Condor - Energy Total" → "condor - energy",
/// "Total East" / "Ukupno Vojvodina" → "east" / "vojvodina" (pivot-table and report style).
fn group_name(label: &str) -> Option<String> {
    let l = label.trim().trim_end_matches(':').trim().to_lowercase();
    const WORDS: [&str; 6] = ["total", "subtotal", "ukupno", "svega", "укупно", "свега"];
    for w in WORDS {
        if let Some(rest) = l.strip_suffix(w).filter(|r| r.ends_with(' ')) {
            return Some(rest.trim().to_string()).filter(|s| !s.is_empty());
        }
        if let Some(rest) = l.strip_prefix(w).filter(|r| r.starts_with(' ')) {
            return Some(rest.trim().to_string()).filter(|s| !s.is_empty());
        }
    }
    None
}

/// `t.trim().to_lowercase() == lower`, without allocating for ASCII text.
fn same_label(t: &str, lower: &str) -> bool {
    let t = t.trim();
    if t.is_ascii() { t.eq_ignore_ascii_case(lower) } else { t.to_lowercase() == lower }
}

/// First row of the group that the "X Total" label at (row, label_col) closes (see [`typed_group`]).
fn group_start(
    grid: &Grid,
    kw_rows: &HashMap<u32, (u32, KeywordLevel)>,
    rows_with_cells: &RowSet,
    row: u32,
    label_col: u32,
) -> Option<u32> {
    let name = group_name(text(grid.cell(row, label_col))?)?;
    let mut start = None;
    let mut r = row;
    while r > 0 && row - r < 10_000 {
        r -= 1;
        if !rows_with_cells.contains(&r) {
            break;
        }
        if let Some(&(lc, _)) = kw_rows.get(&r) {
            if lc <= label_col {
                break; // a sibling or a higher-level total: the group would have started below it
            }
            continue;
        }
        if text(grid.cell(r, label_col)).is_some_and(|t| same_label(t, &name)) {
            // Tabular pivots repeat the group label on every row: take the whole run.
            // Deeper totals interrupt the run without ending the group.
            let mut s = r;
            let mut k = r;
            while k > 0 {
                k -= 1;
                if kw_rows.get(&k).is_some_and(|&(lc, _)| lc > label_col) {
                    continue;
                }
                if !text(grid.cell(k, label_col)).is_some_and(|t| same_label(t, &name)) {
                    break;
                }
                s = k;
            }
            start = Some(s);
            break;
        }
    }
    start
}

/// "X Total" whose group starts at the row labelled "X" in the same label column (Excel pivot
/// tables and outline reports): the source is that block, minus the deeper totals inside it.
/// `start` is [`group_start`] for the row (the same for every column of it).
#[allow(clippy::too_many_arguments)]
fn typed_group(
    grid: &Grid,
    kw_rows: &HashMap<u32, (u32, KeywordLevel)>,
    start: Option<u32>,
    row: u32,
    col: u32,
    label_col: u32,
    lvl: KeywordLevel,
    value: f64,
) -> Option<TotalCandidate> {
    let start = start?;
    let mut exclude = Vec::new();
    let mut sum = 0.0;
    let mut n = 0;
    for r in start..row {
        let cell = grid.cell(r, col);
        if kw_rows.contains_key(&r) {
            if cell.is_some() {
                exclude.push((r, col));
            }
            continue;
        }
        if let Some(v) = number(cell).or_else(|| text(cell).and_then(numeric_text)) {
            sum += v;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    let mut c = TotalCandidate::new(row, col, TotalOrigin::Typed, Direction::Vertical);
    c.label = Some((row, label_col));
    c.evidence.push(Evidence::Keyword(lvl));
    c.evidence.push(Evidence::GroupLabel(start, label_col));
    c.level = level_of(lvl);
    c.ranges = vec![Rect { top: start, left: col, bottom: row - 1, right: col }];
    c.exclude = exclude;
    if close(value, sum, true) {
        c.evidence.push(Evidence::ArithmeticMatch);
    }
    Some(c)
}

/// A typed total whose walk up passes more earlier totals than this is not taken (bounds the work
/// on sheets where a keyword repeats on thousands of rows).
const MAX_EARLIER_TOTALS: usize = 2000;

#[allow(clippy::too_many_arguments)]
fn typed_vertical(
    grid: &Grid,
    kw_rows: &HashMap<u32, (u32, KeywordLevel)>,
    formula_total: &FxHashSet<(u32, u32)>,
    rows_with_cells: &RowSet,
    kw_set: &RowSet,
    view: &ColumnView,
    group: Option<u32>,
    row: u32,
    col: u32,
    label_col: u32,
    lvl: KeywordLevel,
    value: f64,
) -> Option<TotalCandidate> {
    if let Some(c) = typed_group(grid, kw_rows, group, row, col, label_col, lvl, value) {
        return Some(c);
    }
    // Walks and sums step along the column's cells instead of probing the cell index per row.
    let check_formulas = !formula_total.is_empty();

    // Walk up: data rows until a header text, an empty row or the top; remember earlier totals.
    let mut top = row;
    let mut first_total: Option<u32> = None;
    let mut totals: Vec<u32> = Vec::new();
    let mut passed = 0usize;
    let mut r = row;
    let mut k = view.below(row);
    while r > 0 {
        r -= 1;
        if !rows_with_cells.contains(&r) {
            break;
        }
        let cell = if k > 0 && view.rows[k - 1] == r {
            k -= 1;
            Some(view.flags[k])
        } else {
            None
        };
        if kw_set.contains(&r) || (check_formulas && formula_total.contains(&(r, col))) {
            passed += 1;
            if passed > MAX_EARLIER_TOTALS {
                return None; // not a report's grand total: keyword rows all the way up
            }
            if cell.is_some() {
                totals.push(r);
            }
            first_total.get_or_insert(r);
            top = r;
            continue;
        }
        if cell.is_some_and(|f| f & ColumnView::LABEL != 0) {
            break;
        }
        top = r;
    }
    let end = view.below(row);
    // First row at or after `from` (and before `row`) holding one of `flags`, else `row`.
    let first_with = |from: u32, flags: u8| {
        (view.below(from)..end).find(|&k| view.flags[k] & flags != 0).map_or(row, |k| view.rows[k])
    };
    let first_value = |from: u32| first_with(from, ColumnView::NUMBER | ColumnView::FORMULA);
    let first_value_or_text =
        |from: u32| first_with(from, ColumnView::NUMBER | ColumnView::NUMBER_TEXT | ColumnView::FORMULA);
    // Leading blanks and text (a year header "2023" stored as text) are not part of the source,
    // unless a leading number stored as text is what makes the total add up (checked below).
    let walk_top = top;
    top = first_value(top);
    if top == row {
        return None;
    }
    let skip_set: FxHashSet<u32> = totals.iter().copied().collect();
    let sum_rows = |from: u32, skip: &FxHashSet<u32>| -> (f64, usize) {
        let mut s = 0.0;
        let mut n = 0;
        for k in view.below(from)..end {
            if view.flags[k] & (ColumnView::NUMBER | ColumnView::NUMBER_TEXT) == 0
                || (!skip.is_empty() && skip.contains(&view.rows[k]))
            {
                continue;
            }
            s += view.values[k];
            n += 1;
        }
        (s, n)
    };
    // Section since the previous total, and everything above except the earlier totals.
    let section_from = first_total.map_or(walk_top, |t| t + 1);
    let mut section_top = first_value(section_from);
    let (mut s_section, mut n_section) =
        if section_top < row { sum_rows(section_top, &FxHashSet::default()) } else { (0.0, 0) };
    let (mut s_all, mut n_all) = sum_rows(top, &skip_set);
    let loose_section = first_value_or_text(section_from);
    if loose_section < section_top && !close(value, s_section, true) {
        let (s, n) = sum_rows(loose_section, &FxHashSet::default());
        if close(value, s, true) {
            (section_top, s_section, n_section) = (loose_section, s, n);
        }
    }
    let loose_top = first_value_or_text(walk_top);
    if loose_top < top && !close(value, s_all, true) {
        let (s, n) = sum_rows(loose_top, &skip_set);
        if close(value, s, true) {
            (top, s_all, n_all) = (loose_top, s, n);
        }
    }
    if n_all == 0 {
        return None;
    }
    let mut c = TotalCandidate::new(row, col, TotalOrigin::Typed, Direction::Vertical);
    c.label = Some((row, label_col));
    c.evidence.push(Evidence::Keyword(lvl));
    c.level = level_of(lvl);
    let section = |c: &mut TotalCandidate| {
        c.ranges = vec![Rect { top: section_top, left: col, bottom: row - 1, right: col }];
    };
    // Everything above; earlier totals excluded (a grand total when there are any).
    let whole = |c: &mut TotalCandidate| {
        c.ranges = vec![Rect { top, left: col, bottom: row - 1, right: col }];
        if !totals.is_empty() {
            c.exclude = totals.iter().map(|&r| (r, col)).collect();
            c.level = TotalLevel::GrandTotal;
            c.evidence.push(Evidence::SumsTotals);
        }
    };
    let has_section = n_section > 0;
    if has_section && close(value, s_section, true) {
        section(&mut c);
        c.evidence.push(Evidence::ArithmeticMatch);
    } else if close(value, s_all, true) {
        whole(&mut c);
        c.evidence.push(Evidence::ArithmeticMatch);
    } else if has_section && (totals.is_empty() || lvl != KeywordLevel::GrandTotal) {
        section(&mut c);
    } else if lvl == KeywordLevel::GrandTotal || totals.is_empty() {
        whole(&mut c);
    } else {
        // Directly under other totals with no data in between (a collapsed list of "X Total"
        // rows): its source is not on the sheet.
        return None;
    }
    Some(c)
}

/// Typed vertical totals whose source was not confirmed by arithmetic get
/// [`Evidence::SiblingsMatch`] when most other totals in the same row or column check out —
/// then a mismatch is a real error rather than a wrongly inferred source.
fn mark_sibling_support(grid: &Grid, cands: &mut [TotalCandidate]) {
    // A zero that "matches" a sum of zeros proves nothing about the inferred ranges.
    let matched: Vec<bool> = cands
        .iter()
        .map(|c| {
            number(grid.cell(c.row, c.col)).is_some_and(|v| v != 0.0)
                && match c.origin {
                    TotalOrigin::Typed => c.evidence.contains(&Evidence::ArithmeticMatch),
                    TotalOrigin::Formula => verify(grid, c).ok == Some(true),
                }
        })
        .collect();
    let mut by_row: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut by_col: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        if c.direction == Direction::Vertical {
            by_row.entry(c.row).or_default().push(i);
            by_col.entry(c.col).or_default().push(i);
        }
    }
    let supported: Vec<bool> = (0..cands.len())
        .map(|i| {
            let c = &cands[i];
            if c.origin != TotalOrigin::Typed || c.direction != Direction::Vertical || matched[i] {
                return false;
            }
            let count = |list: &[usize]| {
                let others = list.iter().filter(|&&j| j != i);
                let (mut n, mut ok) = (0, 0);
                for &j in others {
                    n += 1;
                    ok += matched[j] as usize;
                }
                (n, ok)
            };
            let (n_row, ok_row) = count(&by_row[&c.row]);
            let (n_col, ok_col) = count(&by_col[&c.col]);
            // A column whose totals mostly disagree is aggregated some other way (pivot
            // averages, counts, ratios) — not evidence of a wrong number.
            if n_col >= 2 && 2 * ok_col < n_col {
                return false;
            }
            let (n, ok) = (n_row + n_col, ok_row + ok_col);
            ok >= 1 && 2 * ok >= n
        })
        .collect();
    for (c, s) in cands.iter_mut().zip(supported) {
        if s {
            c.evidence.push(Evidence::SiblingsMatch);
        }
    }
}

fn typed_horizontal(
    grid: &Grid,
    taken: &HashSet<(u32, u32)>,
    hrow: u32,
    hcol: u32,
    lvl: KeywordLevel,
) -> Vec<TotalCandidate> {
    // Typed numbers directly below the header.
    let mut rows = Vec::new();
    let mut r = hrow + 1;
    while let Some(cell) = grid.cell(r, hcol) {
        if cell.formula.is_some() || !matches!(cell.value, Some(Value::Number(_))) {
            break;
        }
        if !taken.contains(&(r, hcol)) {
            rows.push(r);
        }
        r += 1;
    }
    if rows.is_empty() || hcol == 0 {
        return Vec::new();
    }
    // Per row: the numbers to the left, nearest first, up to a text cell or two blanks.
    let lefts: Vec<Vec<(u32, f64)>> = rows
        .iter()
        .map(|&r| {
            let mut v = Vec::new();
            let mut blanks = 0;
            let mut c = hcol;
            while c > 0 && blanks < 2 {
                c -= 1;
                match grid.cell(r, c) {
                    None => blanks += 1,
                    Some(cell) => {
                        blanks = 0;
                        match &cell.value {
                            Some(Value::Number(x)) => v.push((c, *x)),
                            Some(Value::Text(s)) if header_text(s) => break,
                            _ => {}
                        }
                    }
                }
            }
            v
        })
        .collect();
    // The number of addends k must match arithmetically in at least half of the rows.
    let mut votes: HashMap<usize, usize> = HashMap::new();
    for (i, &r) in rows.iter().enumerate() {
        let value = number(grid.cell(r, hcol)).unwrap_or(0.0);
        let mut s = 0.0;
        for (k, &(_, x)) in lefts[i].iter().enumerate() {
            s += x;
            if k >= 1 && value != 0.0 && close(value, s, true) {
                *votes.entry(k + 1).or_default() += 1;
            }
        }
    }
    let Some((&k, &n)) = votes.iter().max_by_key(|(k, n)| (**n, **k)) else {
        return Vec::new();
    };
    if n < 2.max(rows.len().div_ceil(2)) {
        return Vec::new();
    }
    rows.iter()
        .zip(&lefts)
        .filter(|(_, l)| !l.is_empty())
        .map(|(&r, l)| {
            let left = l[(k - 1).min(l.len() - 1)].0;
            let mut c = TotalCandidate::new(r, hcol, TotalOrigin::Typed, Direction::Horizontal);
            c.ranges = vec![Rect { top: r, left, bottom: r, right: hcol - 1 }];
            c.label = Some((hrow, hcol));
            c.level = level_of(lvl);
            c.evidence.push(Evidence::Keyword(lvl));
            let value = number(grid.cell(r, hcol)).unwrap_or(0.0);
            let sum: f64 = l.iter().take(k).map(|x| x.1).sum();
            c.evidence.push(if close(value, sum, true) { Evidence::ArithmeticMatch } else { Evidence::SiblingsMatch });
            c
        })
        .collect()
}

/// A typed "total" that a formula total adds as an ordinary value, and whose own source did not
/// add up, is a data cell next to a stray keyword ("TOTAL:" one row above the real SUM).
fn drop_unconfirmed_inside_formulas(cands: &mut Vec<TotalCandidate>) {
    // Vertical formula totals add cells of their own column, horizontal ones of their own row.
    let mut by_line: FxHashMap<(bool, u32), Vec<usize>> = FxHashMap::default();
    for (i, f) in cands.iter().enumerate().filter(|(_, c)| c.origin == TotalOrigin::Formula) {
        let line = if f.direction == Direction::Vertical { f.col } else { f.row };
        by_line.entry((f.direction == Direction::Vertical, line)).or_default().push(i);
    }
    let keep: Vec<bool> = cands
        .iter()
        .map(|c| {
            if c.origin == TotalOrigin::Formula || c.evidence.contains(&Evidence::ArithmeticMatch) {
                return true;
            }
            let vertical = c.direction == Direction::Vertical;
            let line = if vertical { c.col } else { c.row };
            !by_line.get(&(vertical, line)).is_some_and(|fs| fs.iter().any(|&j| cands[j].covers(c.row, c.col)))
        })
        .collect();
    let mut k = keep.into_iter();
    cands.retain(|_| k.next().unwrap_or(true));
}

/// Totals inside other totals' sources (same direction): an outer total that adds only totals is a
/// grand total over subtotals; one that adds inner totals *and* the data they summarise
/// double-counts. Inner totals become subtotals.
fn resolve_nesting(grid: &Grid, cands: &mut [TotalCandidate]) {
    let pos: HashMap<(u32, u32), usize> = cands.iter().enumerate().map(|(i, c)| ((c.row, c.col), i)).collect();
    let mut inner_all: Vec<usize> = Vec::new();
    let mut updates: Vec<(usize, bool, bool, Vec<usize>)> = Vec::new();
    for i in 0..cands.len() {
        let c = &cands[i];
        let mut inside: Vec<usize> = Vec::new();
        let mut inside_set: FxHashSet<usize> = FxHashSet::default();
        let excluded: FxHashSet<(u32, u32)> = c.exclude.iter().copied().collect();
        let mut data_cells = 0usize;
        let area: u64 = c.ranges.iter().map(|r| (r.bottom - r.top + 1) as u64 * (r.right - r.left + 1) as u64).sum();
        if area > 200_000 {
            continue;
        }
        for r in &c.ranges {
            for row in r.top..=r.bottom {
                for col in r.left..=r.right {
                    if (row, col) == (c.row, c.col) || excluded.contains(&(row, col)) {
                        continue;
                    }
                    match pos.get(&(row, col)) {
                        Some(&j) if cands[j].direction == c.direction => {
                            if inside_set.insert(j) {
                                inside.push(j);
                            }
                        }
                        _ => data_cells += number(grid.cell(row, col)).is_some() as usize,
                    }
                }
            }
        }
        let excluded_totals: Vec<usize> = c.exclude.iter().filter_map(|p| pos.get(p).copied()).collect();
        let within = |inner: &TotalCandidate| {
            inner.ranges.iter().all(|ir| {
                c.ranges
                    .iter()
                    .any(|r| r.top <= ir.top && ir.bottom <= r.bottom && r.left <= ir.left && ir.right <= r.right)
            })
        };
        let nested: Vec<usize> = inside.iter().copied().filter(|&j| within(&cands[j])).collect();
        let sums_totals = !inside.is_empty() && data_cells == 0;
        inner_all.extend(excluded_totals.iter().copied());
        let grand = !excluded_totals.is_empty() || sums_totals || !nested.is_empty();
        if sums_totals {
            inner_all.extend(inside.iter().copied());
        } else if !nested.is_empty() {
            inner_all.extend(nested.iter().copied());
        }
        updates.push((i, grand, sums_totals, if sums_totals { Vec::new() } else { nested }));
    }
    let inner_set: HashSet<usize> = inner_all.iter().copied().collect();
    for (i, grand, sums_totals, nested) in updates {
        let mut cells: Vec<(u32, u32)> = nested.iter().map(|&j| (cands[j].row, cands[j].col)).collect();
        cells.sort_unstable();
        let c = &mut cands[i];
        if sums_totals && !c.evidence.contains(&Evidence::SumsTotals) {
            c.evidence.push(Evidence::SumsTotals);
        }
        if !cells.is_empty() {
            c.evidence.push(Evidence::ContainsTotals);
            c.double_counted = cells;
        }
        // A total that is itself inside a bigger one stays a (sub)total.
        if grand && c.level == TotalLevel::Total && !inner_set.contains(&i) {
            c.level = TotalLevel::GrandTotal;
        }
    }
    for i in inner_all {
        if cands[i].level == TotalLevel::Total {
            cands[i].level = TotalLevel::Subtotal;
        }
    }
}
