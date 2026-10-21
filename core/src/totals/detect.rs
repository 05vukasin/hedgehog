//! Finds total, subtotal and grand-total cells: aggregate formulas over the cells above / to the
//! left, and typed-in numbers in rows (or columns) labelled with a total keyword.

use std::collections::{HashMap, HashSet};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::grid::Grid;
use crate::model::{Cell, Rect, Value};
use crate::totals::deps::DependencyIndex;
use crate::totals::formula::FormulaKind;
use crate::totals::keywords::{
    KeywordLevel, NonSumKind, NonSumLabel, REGISTER_MIN_ROWS, ambiguous_keyword, incidental_keyword, keyword_at_edge,
    nonsum_keyword, total_keyword, total_keyword_base,
};
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
    /// The source is the rows below the total (a subtotal above its group, a grand total at the top).
    SourceBelow,
    /// No total word: the row's numbers equal the sum of the rows above them in several columns
    /// ("France métropolitaine" under the départements).
    Unlabelled,
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
/// token ("n/a", "-", "#N/A"), which stand in for blank data cells, and not a ruling line
/// (`=====`, `~~~~~` between the data and its total, B38).
fn header_text(s: &str) -> bool {
    numeric_text(s).is_none() && missing_token(s).is_none() && !crate::layout::is_rule_text(s)
}

fn number(cell: Option<&Cell>) -> Option<f64> {
    match cell?.value.as_ref()? {
        Value::Number(v) | Value::DateTime(v) => Some(*v),
        _ => None,
    }
}

/// Row → (label column, level) for rows whose leftmost keyword text cell names a total and that
/// hold at least one number or formula right of it (a header row "Name | Q1 | Total" does not).
/// Rows whose keyword is only an incidental word of a free-text cell in a register
/// ([`incidental_keyword`]) are left out; [`detect_rows`] adds those whose numbers add up.
pub fn keyword_rows(grid: &Grid) -> HashMap<u32, (u32, KeywordLevel)> {
    keyword_rows_with(grid, &keyword_cells(grid, true), true, false).0
}

/// Rows whose total word is incidental ([`incidental_keyword`]: free text in a register), sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IncidentalRows {
    /// Their numbers do not add up: ordinary records, whatever their text says.
    pub records: Vec<u32>,
    /// They add up or close a group: total rows after all.
    pub totals: Vec<u32>,
}

/// The rows of `grid` whose total word is incidental, split by whether arithmetic (or the group
/// they close) confirms them as totals. Cheap when there are none (one pass over the text cells).
pub fn incidental_rows(grid: &Grid) -> IncidentalRows {
    let (_, weak, _) = keyword_rows_with(grid, &keyword_cells(grid, true), true, false);
    if weak.is_empty() {
        return IncidentalRows::default();
    }
    let d = grid.totals.get_or_init(|| detect_rows(grid, &DependencyIndex::new(grid), true));
    IncidentalRows { records: d.incidental_rows.clone(), totals: d.confirmed_rows.clone() }
}

/// The rows the learned row model's post-step turns back into data: the register rule it was
/// built with (B1) — a total word of the row model's list in a longer text right of other text,
/// a recurring label a category by its share alone, a total only when one of its typed numbers is
/// the sum of at least two numbers above it or it closes its group. The other weak rows (B32
/// descriptions starting with a total word, names such as "Somme") are left to the normalized
/// output's row overrides, so the model's roles stay as trained.
pub fn model_records(grid: &Grid) -> Vec<u32> {
    let kw_cells = keyword_cells(grid, false);
    let (strong, weak, _) = keyword_rows_with(grid, &kw_cells, true, true);
    if weak.is_empty() {
        return Vec::new();
    }
    let d = grid.totals.get_or_init(|| detect_rows(grid, &DependencyIndex::new(grid), true));
    let formula_total: FxHashSet<(u32, u32)> = d
        .candidates
        .iter()
        .filter(|c| c.origin == TotalOrigin::Formula && !c.evidence.contains(&Evidence::SourceBelow))
        .map(|c| (c.row, c.col))
        .collect();
    let rows_with_cells = RowSet::new(grid.sheet.cells.iter().map(|c| c.row));
    let none = RowSet::new(std::iter::empty());
    let budget = std::cell::Cell::new(0);
    let strong_set = RowSet::new(strong.keys().copied());
    let mut all_rows = strong.clone();
    all_rows.extend(weak.iter().map(|(&r, &v)| (r, v)));
    let all_set = RowSet::new(all_rows.keys().copied());
    let mut columns: FxHashMap<u32, ColumnView> = FxHashMap::default();
    let mut weak_sorted: Vec<_> = weak.iter().map(|(&r, &v)| (r, v)).collect();
    weak_sorted.sort_unstable_by_key(|x| x.0);
    let mut records = Vec::new();
    for &(row, (lc, lvl)) in &weak_sorted {
        let cols: Vec<u32> = grid
            .row_cells(row)
            .iter()
            .map(|&i| &grid.sheet.cells[i as usize])
            .filter(|c| c.col > lc && c.formula.is_none() && matches!(c.value, Some(Value::Number(_))))
            .map(|c| c.col)
            .collect();
        let mut adds_up = |rows: &KeywordRows, set: &RowSet| {
            let w = Walk {
                grid,
                kw_rows: rows,
                formula_total: &formula_total,
                rows_with_cells: &rows_with_cells,
                kw_set: set,
                skip: &none,
                more: false,
                budget: &budget,
                spacers: false,
            };
            let group = if cols.is_empty() { None } else { group_start(grid, rows, &rows_with_cells, row, lc, false) };
            cols.iter().any(|&col| {
                let value = number(grid.cell(row, col)).unwrap_or(0.0);
                let view = columns.entry(col).or_insert_with(|| ColumnView::new(grid, col));
                value != 0.0
                    && typed_vertical(&w, view, group, row, col, lc, lvl, value).is_some_and(|c| {
                        c.evidence.contains(&Evidence::ArithmeticMatch) && source_numbers(view, &c) >= 2
                    })
            })
        };
        let confirmed = closes_group_with(grid, row, lc, false) || adds_up(&all_rows, &all_set) || adds_up(&strong, &strong_set);
        if !confirmed {
            records.push(row);
        }
    }
    records
}

/// Rows labelled as totals by a total word, sorted: [`keyword_rows`] plus the rows whose
/// incidental total word is confirmed ([`IncidentalRows::totals`]). Empty on sheets shorter than
/// a register, where no total word is incidental.
pub fn labelled_rows(grid: &Grid) -> Vec<u32> {
    if sheet_rows(grid) < REGISTER_MIN_ROWS {
        return Vec::new();
    }
    let (strong, weak, b1) = keyword_rows_with(grid, &keyword_cells(grid, true), true, false);
    // The rule layout (and through it the learned model) keeps the register rule it was built with
    // (B1, see [`model_records`]); the other weak rows are left to the normalized output.
    let mut rows: Vec<u32> = if b1.is_empty() {
        strong.into_keys().chain(weak.into_keys()).collect()
    } else {
        let d = grid.totals.get_or_init(|| detect_rows(grid, &DependencyIndex::new(grid), true));
        let mut v: Vec<u32> = d.keyword_rows.keys().copied().collect();
        v.extend(weak.into_keys().filter(|r| !b1.contains(r) && !d.keyword_rows.contains_key(r)));
        v
    };
    rows.sort_unstable();
    rows
}

/// Rows from the sheet's first to its last cell (cells are row-major).
fn sheet_rows(grid: &Grid) -> u32 {
    match (grid.sheet.cells.first(), grid.sheet.cells.last()) {
        (Some(a), Some(b)) => b.row - a.row + 1,
        _ => 0,
    }
}

/// Text cells naming a total, by index into `sheet.cells` (ascending).
fn keyword_cells(grid: &Grid, extended: bool) -> Vec<(usize, KeywordLevel)> {
    let kw = if extended { total_keyword } else { total_keyword_base };
    // Registers repeat the same texts on many rows: classify each distinct text once.
    let mut seen: FxHashMap<&str, Option<KeywordLevel>> = FxHashMap::default();
    grid.sheet
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match &c.value {
            Some(Value::Text(s)) => (*seen.entry(s.as_str()).or_insert_with(|| kw(s))).map(|lvl| (i, lvl)),
            _ => None,
        })
        .collect()
}

type KeywordRows = HashMap<u32, (u32, KeywordLevel)>;

/// A total label that recurs as a category of its column (long-format statistics: "Summe der
/// bezogenen Rohstoffe" once per industry) is the column's label for its sum rows, not free text:
/// at least this many times, and either in at least 1 % of the column's cells or at the end of a
/// block ([`ends_block`]).
const MIN_CATEGORY_REPEATS: usize = 3;

/// The label text cells left of `col` in `row` (not numbers stored as text), as (column, text).
fn left_texts<'g>(grid: &Grid<'g>, row: u32, col: u32) -> Vec<(u32, &'g str)> {
    let cells = &grid.sheet.cells;
    grid.row_cells(row)
        .iter()
        .map(|&i| &cells[i as usize])
        .take_while(|c| c.col < col)
        .filter_map(|c| match &c.value {
            Some(Value::Text(t)) if header_text(t) => Some((c.col, t.as_str())),
            _ => None,
        })
        .collect()
}

/// Row `row` closes a block of long-format data: the text cells left of `col` on the row above (the
/// keys, "Personalausgaben | Dienstbezüge") hold the same text on this row and change on the row
/// below, as on the last row of each "Summe aller Produkte" block. Free text never repeats its keys
/// like this.
fn ends_block(grid: &Grid, row: u32, col: u32) -> bool {
    if row == 0 {
        return false;
    }
    let keys = left_texts(grid, row - 1, col);
    let here = left_texts(grid, row, col);
    let same = |line: &[(u32, &str)]| keys.iter().all(|k| line.contains(k));
    !keys.is_empty() && same(&here) && !same(&left_texts(grid, row + 1, col))
}

/// The text cells left of `col` on `row` (its keys) are all on the row above or the row below too:
/// a category value inside a block of long-format data ("Material- und Wareneingang | Industrie 3
/// | Summe der bezogenen Rohstoffe"), not a record with its own id ("K-24-00758 | Total station
/// survey equipment"). True when the row has no keys.
fn shares_keys(grid: &Grid, row: u32, col: u32) -> bool {
    let keys = left_texts(grid, row, col);
    let all_in = |r: u32| {
        let line = left_texts(grid, r, col);
        keys.iter().all(|k| line.contains(k))
    };
    keys.is_empty() || (row > 0 && all_in(row - 1)) || all_in(row + 1)
}

/// Keyword rows (see [`keyword_rows`]), split into rows labelled by their keyword cell and rows
/// whose keyword is incidental ([`incidental_keyword`]; only with `registers`): those need
/// arithmetic to be totals. A keyword text that starts or ends with the total word and recurs as
/// a category of its column ([`MIN_CATEGORY_REPEATS`]) is a label, not incidental.
fn keyword_rows_with(
    grid: &Grid,
    kw_cells: &[(usize, KeywordLevel)],
    registers: bool,
    legacy: bool,
) -> (KeywordRows, KeywordRows, FxHashSet<u32>) {
    let kw_of: FxHashMap<usize, KeywordLevel> = kw_cells.iter().copied().collect();
    let n_rows = if registers { sheet_rows(grid) } else { 0 };
    let cells = &grid.sheet.cells;
    let mut repeats: FxHashMap<(u32, &str), usize> = FxHashMap::default();
    if n_rows >= REGISTER_MIN_ROWS {
        for &(i, _) in kw_cells {
            if let Some(Value::Text(t)) = &cells[i].value {
                *repeats.entry((cells[i].col, t.as_str())).or_default() += 1;
            }
        }
    }
    // The register rule as the learned row model's post-step knows it (B1: a longer text right of
    // other text, the row model's keywords, a recurring label a category by its share alone).
    let b1_category = |row: u32, col: u32, t: &str| {
        let n = repeats.get(&(col, t)).copied().unwrap_or(0);
        n >= MIN_CATEGORY_REPEATS && keyword_at_edge(t) && (n * 100 >= grid.col_cells(col).len() || ends_block(grid, row, col))
    };
    let mut b1 = FxHashSet::default();
    let category = |row: u32, col: u32, t: &str| {
        let n = repeats.get(&(col, t)).copied().unwrap_or(0);
        n >= MIN_CATEGORY_REPEATS
            && keyword_at_edge(t)
            && ((n * 100 >= grid.col_cells(col).len() && shares_keys(grid, row, col)) || ends_block(grid, row, col))
    };
    // A word that is also a name ("Somme") alone as the row's label under a table's numbers is its
    // total label; the same word with numbers on the row below sits inside a list (a département).
    let data_below = |row: u32| {
        grid.row_cells(row + 1).iter().any(|&j| matches!(cells[j as usize].value, Some(Value::Number(_))))
    };
    // A label text right of the keyword cell on its row (a register's other columns).
    let text_right = |row: u32, col: u32| {
        grid.row_cells(row).iter().map(|&j| &cells[j as usize]).any(|c| {
            c.col > col && matches!(&c.value, Some(Value::Text(t)) if header_text(t) && !t.trim().is_empty())
        })
    };
    let mut out = HashMap::new();
    let mut weak = HashMap::new();
    let mut label: Option<(u32, u32, KeywordLevel, bool)> = None; // (row, col, level, incidental)
    let mut cur_row = None;
    let mut text_left = false; // a label text seen on the current row before `label`
    let mut skipped = false; // a repeated key with a total word passed over on the current row
    for (i, c) in grid.sheet.cells.iter().enumerate() {
        if cur_row != Some(c.row) {
            cur_row = Some(c.row);
            text_left = false;
            skipped = false;
            label = None;
        }
        if out.contains_key(&c.row) || weak.contains_key(&c.row) {
            continue;
        }
        match (&label, &c.value) {
            (None, Some(Value::Text(s))) => {
                // A key in a run of at least three rows ("Deutschland Insgesamt" on every row of a
                // long table) is a category, not this row's label: look further right.
                let repeated = || {
                    let same = |d: i64| {
                        let r = c.row as i64 + d;
                        r >= 0 && text(grid.cell(r as u32, c.col)).is_some_and(|t| t.trim() == s.trim())
                    };
                    (same(-1) && (same(1) || same(-2))) || (same(1) && same(2))
                };
                let kw = kw_of.get(&i).filter(|_| legacy || !(registers && repeated()));
                skipped |= kw.is_none() && kw_of.contains_key(&i);
                if let Some(&level) = kw {
                    // A free text in a register, or a word that is also a name or a code ("Somme",
                    // "SOM", "SUMA Kommunikation"): a total only if its numbers add up.
                    let right = n_rows >= REGISTER_MIN_ROWS && text_right(c.row, c.col);
                    let inc = if legacy {
                        incidental_keyword(s, text_left, false, n_rows) && !b1_category(c.row, c.col, s)
                    } else {
                        // B50: a short name right of the record's own keys ("GROWTH WIRELESS LLC |
                        // TOTAL WIRELESS", a shop's trade name) is a value, not the row's label:
                        // a total label right of text sits under keys shared with its neighbours.
                        let own_keys = || n_rows >= REGISTER_MIN_ROWS && text_left && !shares_keys(grid, c.row, c.col);
                        let weak = registers
                            && (incidental_keyword(s, text_left, right, n_rows)
                                || ambiguous_keyword(s).is_some_and(|name| name || text_left || data_below(c.row))
                                || own_keys());
                        weak && !category(c.row, c.col, s)
                    };
                    if registers
                        && !skipped
                        && total_keyword_base(s).is_some()
                        && incidental_keyword(s, text_left, false, n_rows)
                        && !b1_category(c.row, c.col, s)
                    {
                        b1.insert(c.row);
                    }
                    label = Some((c.row, c.col, level, inc));
                } else if header_text(s) {
                    text_left = true;
                }
            }
            (Some((r, lc, lvl, inc)), v) if (c.formula.is_some() || matches!(v, Some(Value::Number(_)))) => {
                if *inc { &mut weak } else { &mut out }.insert(*r, (*lc, *lvl));
            }
            _ => {}
        }
    }
    drop_repeated_labels(grid, &mut out);
    drop_repeated_labels(grid, &mut weak);
    b1.retain(|r| weak.contains_key(r));
    (out, weak, b1)
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
    grid.totals.get_or_init(|| detect_rows(grid, &DependencyIndex::new(grid), true)).candidates.clone()
}

/// Everything [`detect`] finds on the sheet (cached on the grid): candidates, keyword rows,
/// "of which" and average rows, known aggregates.
pub fn detection<'a>(grid: &'a Grid) -> &'a Detection {
    grid.totals.get_or_init(|| detect_rows(grid, &DependencyIndex::new(grid), true))
}

/// [`detect`] reusing an already built dependency index.
pub fn detect_with(grid: &Grid, deps: &DependencyIndex) -> Vec<TotalCandidate> {
    grid.totals.get_or_init(|| detect_rows(grid, deps, true)).candidates.clone()
}

/// What [`detect_rows`] finds.
#[derive(Debug, Clone, Default)]
pub struct Detection {
    pub candidates: Vec<TotalCandidate>,
    /// Row → (label column, level) of the rows labelled as totals by a keyword.
    pub keyword_rows: HashMap<u32, (u32, KeywordLevel)>,
    /// Rows with an incidental total word that arithmetic did not confirm (sorted).
    pub incidental_rows: Vec<u32>,
    /// Rows with an incidental total word that adds up or closes a group: total rows (sorted).
    pub confirmed_rows: Vec<u32>,
    /// "Of which" and average rows (sorted by row): neither data nor sums.
    pub nonsum: Vec<NonSumRow>,
    /// Rows of known aggregates without a checkable sum (World Bank region and income-group codes:
    /// `WLD`, `EUU`, `HIC`, …), as (row, label column), sorted.
    pub aggregates: Vec<(u32, u32)>,
}

/// A row that is neither data nor a sum of other rows ([`NonSumKind`]): an "of which" part of the
/// row above it, or an average of the rows above it.
#[derive(Debug, Clone, PartialEq)]
pub struct NonSumRow {
    pub row: u32,
    pub label_col: u32,
    pub kind: NonSumKind,
    /// One check per number right of the label.
    pub cells: Vec<NonSumCheck>,
}

/// One number of a [`NonSumRow`], checked against the rows above.
#[derive(Debug, Clone, PartialEq)]
pub struct NonSumCheck {
    pub col: u32,
    pub value: f64,
    /// Average: the mean (the median for a "Median" row) of the numbers above it in its section;
    /// of which: the number of the row it is a part of.
    pub expected: Option<f64>,
    /// The cells `expected` comes from.
    pub source: Option<Rect>,
    /// `Some(true)`: the mean matches / the part is not larger than the whole; `Some(false)`: the
    /// part is larger than the whole; `None`: nothing to compare with, or an average that does not
    /// match (a weighted or rounded one).
    pub ok: Option<bool>,
}

/// [`detect_with`], also returning the keyword rows. With `registers`, a total word that is
/// incidental ([`incidental_keyword`]: free text in a register) makes its row a total row only if
/// one of the row's typed numbers is the sum of at least two numbers above it or the row closes a
/// group; without, every keyword counts (the behaviour the learned row model's features were
/// trained on). Totals are cached on the grid only for `registers` (see [`detect`]).
pub fn detect_rows(grid: &Grid, deps: &DependencyIndex, registers: bool) -> Detection {
    let sheet = grid.sheet;
    let kw_cells = keyword_cells(grid, registers);
    let kw_fn = if registers { total_keyword } else { total_keyword_base };
    let (mut kw_rows, weak_rows, _) = keyword_rows_with(grid, &kw_cells, registers, false);
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
        let above =
            p.sum_refs.iter().all(|r| r.rect.left == f.col && r.rect.right == f.col && r.rect.bottom < f.row);
        // A subtotal above its group, a grand total at the top (normalized output only).
        let below = registers
            && !above
            && p.sum_refs.iter().all(|r| r.rect.left == f.col && r.rect.right == f.col && r.rect.top > f.row);
        let vertical = above || below;
        let horizontal = !vertical
            && p.sum_refs.iter().all(|r| r.rect.top == f.row && r.rect.bottom == f.row && r.rect.right < f.col);
        if !vertical && !horizontal {
            continue;
        }
        let dir = if vertical { Direction::Vertical } else { Direction::Horizontal };
        let mut c = TotalCandidate::new(f.row, f.col, TotalOrigin::Formula, dir);
        c.evidence.push(Evidence::Formula(p.kind));
        if below {
            c.evidence.push(Evidence::SourceBelow);
        }
        c.ranges = p.sum_refs.iter().map(|r| r.rect).collect();
        let keyword = if vertical {
            kw_rows.get(&f.row).filter(|(lc, _)| *lc < f.col).map(|&(lc, lvl)| ((f.row, lc), lvl))
        } else {
            labels
                .above(f.row, f.col)
                .and_then(|r| kw_fn(text(grid.cell(r, f.col))?).map(|lvl| ((r, f.col), lvl)))
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
    let mut columns: FxHashMap<u32, ColumnView> = FxHashMap::default();
    // Typed numbers right of a keyword row's label.
    let typed_cols = |row: u32, lc: u32| -> Vec<u32> {
        grid.row_cells(row)
            .iter()
            .map(|&i| &sheet.cells[i as usize])
            .filter(|c| c.col > lc && c.formula.is_none())
            .filter(|c| matches!(c.value, Some(Value::Number(_))))
            .map(|c| c.col)
            .collect()
    };
    // "Of which" and average rows (normalized output only): out of every sum.
    let nonsum = if registers {
        nonsum_rows(grid, &kw_rows, &formula_total, &rows_with_cells, &mut columns)
    } else {
        Vec::new()
    };
    let skip = RowSet::new(nonsum.iter().map(|n| n.row));
    let budget = std::cell::Cell::new(WALK_BELOW_BUDGET);

    // (b0) A row whose total word is incidental (free text in a register) is a total row only if
    // one of its typed numbers is the sum of at least two numbers above it (sections bounded by
    // every keyword row, as for any total, or by the other total rows only), or if it closes a
    // group: "X Total" under rows whose label column is "X" or empty since the row labelled "X".
    let mut incidental = Vec::new();
    let mut confirmed = Vec::new();
    if !weak_rows.is_empty() {
        let strong_rows = kw_rows.clone();
        let strong_set = RowSet::new(strong_rows.keys().copied());
        let mut all_rows = kw_rows.clone();
        all_rows.extend(weak_rows.iter().map(|(&r, &v)| (r, v)));
        let all_set = RowSet::new(all_rows.keys().copied());
        let mut weak_sorted: Vec<_> = weak_rows.iter().map(|(&r, &v)| (r, v)).collect();
        weak_sorted.sort_unstable_by_key(|x| x.0);
        for &(row, (lc, lvl)) in &weak_sorted {
            let cols = typed_cols(row, lc);
            let adds_up = |rows: &KeywordRows, set: &RowSet, columns: &mut FxHashMap<u32, ColumnView>| {
                let w = Walk {
                    grid,
                    kw_rows: rows,
                    formula_total: &formula_total,
                    rows_with_cells: &rows_with_cells,
                    kw_set: set,
                    skip: &skip,
                    more: false, // a weak word needs its rows above to add up
                    budget: &budget,
                    spacers: false,
                };
                let group =
                    if cols.is_empty() { None } else { group_start(grid, rows, &rows_with_cells, row, lc, registers) };
                // More than half of the row's numbers must add up: one number matching the sum of
                // two above it is a coincidence in a register ("Ukupno: 3 komada stolica | 265"
                // under 109 and 156).
                let (mut numbers, mut matches) = (0, 0);
                for &col in &cols {
                    let value = number(grid.cell(row, col)).unwrap_or(0.0);
                    if value == 0.0 {
                        continue;
                    }
                    numbers += 1;
                    let view = columns.entry(col).or_insert_with(|| ColumnView::new(grid, col));
                    matches += typed_vertical(&w, view, group, row, col, lc, lvl, value).is_some_and(|c| {
                        c.evidence.contains(&Evidence::ArithmeticMatch) && source_numbers(view, &c) >= 2
                    }) as usize;
                }
                (matches, numbers)
            };
            let (m1, n1) = adds_up(&all_rows, &all_set, &mut columns);
            let (m2, n2) = if m1 > 0 && 2 * m1 > n1 { (m1, n1) } else { adds_up(&strong_rows, &strong_set, &mut columns) };
            if closes_group(grid, row, lc) || (m1 > 0 && 2 * m1 > n1) || (m2 > 0 && 2 * m2 > n2) {
                confirmed.push(row);
                kw_rows.insert(row, (lc, lvl));
            } else {
                incidental.push(row);
            }
        }
    }
    let kw_set = RowSet::new(kw_rows.keys().copied());

    // (b) Typed numbers in keyword rows → column totals over the cells above (or below).
    let mut kw_sorted: Vec<_> = kw_rows.iter().map(|(&r, &v)| (r, v)).collect();
    kw_sorted.sort_unstable_by_key(|x| x.0);
    {
        let w = Walk {
            grid,
            kw_rows: &kw_rows,
            formula_total: &formula_total,
            rows_with_cells: &rows_with_cells,
            kw_set: &kw_set,
            skip: &skip,
            more: registers,
            budget: &budget,
            spacers: false,
        };
        for &(row, (lc, lvl)) in &kw_sorted {
            let cols = typed_cols(row, lc);
            let group =
                if cols.is_empty() { None } else { group_start(grid, &kw_rows, &rows_with_cells, row, lc, registers) };
            for col in cols {
                let value = number(grid.cell(row, col)).unwrap_or(0.0);
                let view = columns.entry(col).or_insert_with(|| ColumnView::new(grid, col));
                if let Some(c) = typed_vertical(&w, view, group, row, col, lc, lvl, value) {
                    out.push(c);
                }
            }
        }
    }

    // (b'') Rows without a total word that add up (normalized output only).
    if registers {
        let found = unlabelled_totals(grid, &kw_rows, &skip, &formula_total, &mut labels);
        out.extend(found);
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
    let aggregates = if registers { aggregate_rows(grid) } else { Vec::new() };
    Detection {
        candidates: out,
        keyword_rows: kw_rows,
        incidental_rows: incidental,
        confirmed_rows: confirmed,
        nonsum,
        aggregates,
    }
}


/// Rows a group closed by an "X Total" label may span ([`closes_group`]).
const MAX_GROUP_ROWS: u32 = 10_000;

/// The "X Total" label at (row, label_col) closes a group: going up its column, the first
/// non-empty cell is "X" (pivot tables and outline reports: the group label, then blanks or the
/// label repeated). Other columns are not looked at, so a block beside another one (whose own
/// totals sit on other rows) still closes its groups.
fn closes_group(grid: &Grid, row: u32, label_col: u32) -> bool {
    closes_group_with(grid, row, label_col, true)
}

/// [`closes_group`] with or without the group names of the normalized output ([`group_name`]).
fn closes_group_with(grid: &Grid, row: u32, label_col: u32, more: bool) -> bool {
    let Some(name) = text(grid.cell(row, label_col)).and_then(|t| group_name(t, more)) else {
        return false;
    };
    for r in (row.saturating_sub(MAX_GROUP_ROWS)..row).rev() {
        match grid.cell(r, label_col) {
            None => continue,
            Some(c) if c.value.is_none() => continue,
            Some(c) => return text(Some(c)).is_some_and(|t| same_label(t, &name)),
        }
    }
    false
}

/// Numbers (typed or stored as text) a vertical candidate in `view`'s column adds.
fn source_numbers(view: &ColumnView, c: &TotalCandidate) -> usize {
    c.ranges
        .iter()
        .map(|r| {
            (view.below(r.top)..view.below(r.bottom + 1))
                .filter(|&k| view.flags[k] & (ColumnView::NUMBER | ColumnView::NUMBER_TEXT) != 0)
                .filter(|&k| !c.exclude.contains(&(view.rows[k], c.col)))
                .count()
        })
        .sum()
}

fn level_of(k: KeywordLevel) -> TotalLevel {
    match k {
        KeywordLevel::Total => TotalLevel::Total,
        KeywordLevel::Subtotal => TotalLevel::Subtotal,
        KeywordLevel::GrandTotal => TotalLevel::GrandTotal,
    }
}

/// Group name announced by a total label: "Condor - Energy Total" → "condor - energy",
/// "Total East" / "Ukupno Vojvodina" → "east" / "vojvodina" (pivot-table and report style). With
/// `more` (normalized output) also "Nord gesamt", "Razem Belgia", "North - total" → "north".
fn group_name(label: &str, more: bool) -> Option<String> {
    let l = label.trim().trim_end_matches(':').trim().to_lowercase();
    const WORDS: [&str; 6] = ["total", "subtotal", "ukupno", "svega", "укупно", "свега"];
    const MORE: [&str; 12] = [
        "gesamt", "insgesamt", "summe", "sum", "razem", "celkem", "yhteensä", "összesen", "i alt", "totalt", "всего",
        "итого",
    ];
    let words = WORDS.iter().chain(if more { &MORE[..] } else { &[] });
    let clean = |s: &str| {
        let s = if more { s.trim().trim_matches(['-', '–', ':', ',']).trim() } else { s.trim() };
        Some(s.to_string()).filter(|s| !s.is_empty())
    };
    for w in words {
        if let Some(rest) = l.strip_suffix(w).filter(|r| r.ends_with(' ')) {
            return clean(rest);
        }
        if let Some(rest) = l.strip_prefix(w).filter(|r| r.starts_with(' ')) {
            return clean(rest);
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
    more: bool,
) -> Option<u32> {
    let name = group_name(text(grid.cell(row, label_col))?, more)?;
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

/// What the typed-total walks read besides the column itself.
#[derive(Clone, Copy)]
struct Walk<'a, 'g> {
    grid: &'a Grid<'g>,
    kw_rows: &'a KeywordRows,
    formula_total: &'a FxHashSet<(u32, u32)>,
    rows_with_cells: &'a RowSet,
    kw_set: &'a RowSet,
    /// Rows that are neither data nor totals ("of which", averages): left out of every sum.
    skip: &'a RowSet,
    /// Normalized output: look below a total whose rows above do not add up, and the other
    /// additions over the keywords the row model was trained on.
    more: bool,
    /// Rows the walks below may still step over on this sheet.
    budget: &'a std::cell::Cell<usize>,
    /// Step over single blank rows (spacers inside a table); only kept when the sum then matches.
    spacers: bool,
}

/// "X Total" whose group starts at the row labelled "X" in the same label column (Excel pivot
/// tables and outline reports): the source is that block, minus the deeper totals inside it.
/// `start` is [`group_start`] for the row (the same for every column of it).
#[allow(clippy::too_many_arguments)]
fn typed_group(
    w: &Walk,
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
        let cell = w.grid.cell(r, col);
        if w.kw_rows.contains_key(&r) || w.skip.contains(&r) {
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

/// Rows the walks below a total may step over per sheet, all totals together.
const WALK_BELOW_BUDGET: usize = 4_000_000;

/// A typed number in a keyword row: a total over the rows above it (its section since the previous
/// total, or everything above without the earlier totals), else, in the normalized output, over
/// the rows below it when they add up ([`typed_below`]: a subtotal above its group, a grand total at
/// the top).
#[allow(clippy::too_many_arguments)]
fn typed_vertical(
    w: &Walk,
    view: &ColumnView,
    group: Option<u32>,
    row: u32,
    col: u32,
    label_col: u32,
    lvl: KeywordLevel,
    value: f64,
) -> Option<TotalCandidate> {
    let matched = |c: &TotalCandidate| c.evidence.contains(&Evidence::ArithmeticMatch);
    let up = typed_above(w, view, group, row, col, label_col, lvl, value);
    if !w.more || value == 0.0 || up.as_ref().is_some_and(matched) {
        return up;
    }
    if let Some(c) = typed_below(w, view, row, col, label_col, lvl, value) {
        return Some(c);
    }
    // Blank spacer rows inside the table ("a blank row every 5 rows"): only a sum that matches.
    let ws = Walk { spacers: true, ..*w };
    typed_above(&ws, view, group, row, col, label_col, lvl, value)
        .filter(matched)
        .or_else(|| typed_below(&ws, view, row, col, label_col, lvl, value))
        .or(up)
}

#[allow(clippy::too_many_arguments)]
fn typed_above(
    w: &Walk,
    view: &ColumnView,
    group: Option<u32>,
    row: u32,
    col: u32,
    label_col: u32,
    lvl: KeywordLevel,
    value: f64,
) -> Option<TotalCandidate> {
    if let Some(c) = typed_group(w, group, row, col, label_col, lvl, value) {
        return Some(c);
    }
    // Walks and sums step along the column's cells instead of probing the cell index per row.
    let check_formulas = !w.formula_total.is_empty();

    // Walk up: data rows until a header text, an empty row or the top; remember earlier totals.
    let mut top = row;
    let mut first_total: Option<u32> = None;
    let mut totals: Vec<u32> = Vec::new();
    let mut skipped: Vec<u32> = Vec::new();
    let mut passed = 0usize;
    let mut r = row;
    let mut k = view.below(row);
    let mut blank = false;
    while r > 0 {
        r -= 1;
        if !w.rows_with_cells.contains(&r) {
            if w.spacers && !blank {
                blank = true;
                continue;
            }
            break;
        }
        blank = false;
        let cell = if k > 0 && view.rows[k - 1] == r {
            k -= 1;
            Some(view.flags[k])
        } else {
            None
        };
        if w.skip.contains(&r) {
            // "of which" / average rows: not data, not a total; the walk goes on above them.
            if cell.is_some() {
                skipped.push(r);
            }
            continue;
        }
        if w.kw_set.contains(&r) || (check_formulas && w.formula_total.contains(&(r, col))) {
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
        (view.below(from)..end)
            .find(|&k| view.flags[k] & flags != 0 && !w.skip.contains(&view.rows[k]))
            .map_or(row, |k| view.rows[k])
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
                || w.skip.contains(&view.rows[k])
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
    if !skipped.is_empty() {
        let from = c.ranges[0].top;
        c.exclude.extend(skipped.iter().filter(|&&r| r >= from).map(|&r| (r, col)));
        c.exclude.sort_unstable();
    }
    Some(c)
}

/// A total above its source (normalized output): a subtotal at the top of its group ("Total
/// Belgium" over the Belgian rows), a grand total at the top of the table ("Insgesamt" over
/// "Österreich gesamt", its Länder and "Ausland"). The rows below, down to the next total (its
/// section), or a run of them from the row below skipping the totals inside (the first one that
/// adds up), must add up to `value`: there is no default source below.
fn typed_below(
    w: &Walk,
    view: &ColumnView,
    row: u32,
    col: u32,
    label_col: u32,
    lvl: KeywordLevel,
    value: f64,
) -> Option<TotalCandidate> {
    let check_formulas = !w.formula_total.is_empty();
    let mut k = view.below(row + 1);
    let mut r = row;
    let (mut s_section, mut n_section, mut section_end) = (0.0, 0usize, None::<u32>);
    let (mut s_all, mut n_all) = (0.0, 0usize);
    let mut skipped: Vec<u32> = Vec::new();
    let mut totals: Vec<u32> = Vec::new();
    let mut matched: Option<u32> = None;
    let mut last_value = row;
    let mut blank = false;
    loop {
        r += 1;
        let left = w.budget.get();
        if left == 0 || totals.len() > MAX_EARLIER_TOTALS {
            break;
        }
        if !w.rows_with_cells.contains(&r) {
            if w.spacers && !blank {
                blank = true;
                continue;
            }
            break;
        }
        blank = false;
        w.budget.set(left - 1);
        let flags = if k < view.rows.len() && view.rows[k] == r {
            k += 1;
            Some(view.flags[k - 1])
        } else {
            None
        };
        if w.skip.contains(&r) {
            if flags.is_some() {
                skipped.push(r);
            }
            continue;
        }
        if w.kw_set.contains(&r) || (check_formulas && w.formula_total.contains(&(r, col))) {
            if flags.is_some() {
                totals.push(r);
            }
            section_end.get_or_insert(r - 1);
            continue;
        }
        let Some(f) = flags else { continue };
        if f & ColumnView::LABEL != 0 {
            break;
        }
        if f & (ColumnView::NUMBER | ColumnView::NUMBER_TEXT) == 0 {
            continue;
        }
        let v = view.values[k - 1];
        last_value = r;
        if section_end.is_none() {
            s_section += v;
            n_section += 1;
        }
        s_all += v;
        n_all += 1;
        if matched.is_none() && n_all >= 2 && close(value, s_all, true) {
            matched = Some(r);
        }
    }
    let section_bottom = section_end.unwrap_or(last_value);
    let mut c = TotalCandidate::new(row, col, TotalOrigin::Typed, Direction::Vertical);
    c.label = Some((row, label_col));
    c.evidence.push(Evidence::Keyword(lvl));
    c.level = level_of(lvl);
    let bottom = if n_section >= 2 && section_bottom > row && close(value, s_section, true) {
        section_bottom
    } else {
        let m = matched?;
        let inner: Vec<(u32, u32)> = totals.iter().filter(|&&t| t < m).map(|&t| (t, col)).collect();
        if !inner.is_empty() {
            c.exclude = inner;
            c.level = TotalLevel::GrandTotal;
            c.evidence.push(Evidence::SumsTotals);
        }
        m
    };
    c.ranges = vec![Rect { top: row + 1, left: col, bottom, right: col }];
    c.exclude.extend(skipped.iter().filter(|&&s| s <= bottom).map(|&s| (s, col)));
    c.exclude.sort_unstable();
    c.evidence.push(Evidence::ArithmeticMatch);
    c.evidence.push(Evidence::SourceBelow);
    Some(c)
}

/// A label of a [`NonSumKind`] row found by [`nonsum_rows`], before its numbers are checked.
struct NonSumLabelAt {
    row: u32,
    col: u32,
    kind: NonSumKind,
    plain: NonSumLabel,
    median: bool,
}

/// Labels a row's first two label texts may carry to make it an "of which" or average row.
const NONSUM_LABEL_TEXTS: usize = 2;

/// Blank rows an "of which" row may sit below the row it is a part of.
const MAX_SPACER_ROWS: usize = 2;

/// "Of which" and average rows (normalized output; see [`NonSumRow`]). A row is one when its
/// label (one of its first two label texts, [`nonsum_keyword`]) says so and it holds a number right
/// of the label, and:
/// - of which: a row directly above it (past other "of which" rows) holds a number in the same
///   column: the whole it is part of; the part should not be larger;
/// - average: in some column its number is the mean (median) of the numbers above it in its section
///   (since the previous total or average row, past one directly above it: "Total" then "Average"),
///   or the label is the plain sign or word ("Ø 2012", "Average") over at least two numbers and,
///   for the word, at the end of its block (a category "Media" between data rows is data).
fn nonsum_rows(
    grid: &Grid,
    kw_rows: &KeywordRows,
    formula_total: &FxHashSet<(u32, u32)>,
    rows_with_cells: &RowSet,
    columns: &mut FxHashMap<u32, ColumnView>,
) -> Vec<NonSumRow> {
    let cells = &grid.sheet.cells;
    let mut seen: FxHashMap<&str, Option<(NonSumKind, NonSumLabel)>> = FxHashMap::default();
    let mut found: Vec<NonSumLabelAt> = Vec::new();
    let mut cur_row = None;
    let mut texts = 0usize;
    for c in cells {
        if cur_row != Some(c.row) {
            cur_row = Some(c.row);
            texts = 0;
        }
        let Some(Value::Text(s)) = &c.value else { continue };
        if !header_text(s) || s.trim().is_empty() || texts >= NONSUM_LABEL_TEXTS {
            continue;
        }
        texts += 1;
        if found.last().is_some_and(|f| f.row == c.row) || kw_rows.contains_key(&c.row) {
            continue;
        }
        if let Some((kind, plain)) = *seen.entry(s.as_str()).or_insert_with(|| nonsum_keyword(s)) {
            let median = kind == NonSumKind::Average && crate::values::norm_key(s).starts_with("median");
            found.push(NonSumLabelAt { row: c.row, col: c.col, kind, plain, median });
        }
    }
    if found.is_empty() {
        return Vec::new();
    }
    let of_which = RowSet::new(found.iter().filter(|f| f.kind == NonSumKind::OfWhich).map(|f| f.row));
    let averages = RowSet::new(found.iter().filter(|f| f.kind == NonSumKind::Average).map(|f| f.row));
    let labelled = RowSet::new(found.iter().map(|f| f.row));
    let mut out = Vec::new();
    for f in &found {
        let nums: Vec<(u32, f64)> = grid
            .row_cells(f.row)
            .iter()
            .map(|&i| &cells[i as usize])
            .filter(|c| c.col > f.col)
            .filter_map(|c| match c.value {
                Some(Value::Number(v)) => Some((c.col, v)),
                _ => None,
            })
            .collect();
        if nums.is_empty() {
            continue;
        }
        let mut checks = Vec::with_capacity(nums.len());
        let mut keep = false;
        for (col, value) in nums {
            let view = columns.entry(col).or_insert_with(|| ColumnView::new(grid, col));
            let mut check = NonSumCheck { col, value, expected: None, source: None, ok: None };
            let mut k = view.below(f.row);
            let mut r = f.row;
            match f.kind {
                NonSumKind::OfWhich => {
                    // The nearest row above that is not itself a part (past a spacer row or two).
                    let mut blanks = 0;
                    while r > 0 {
                        r -= 1;
                        if !rows_with_cells.contains(&r) {
                            blanks += 1;
                            if blanks > MAX_SPACER_ROWS {
                                break;
                            }
                            continue;
                        }
                        if of_which.contains(&r) {
                            continue;
                        }
                        let rule = text(grid.cell(r, col))
                            .is_some_and(|t| !t.trim().is_empty() && t.trim().chars().all(|c| "-=_~*–—.".contains(c)));
                        if rule {
                            // a separator line ("-----") between the whole and its part
                            blanks += 1;
                            if blanks > MAX_SPACER_ROWS {
                                break;
                            }
                            continue;
                        }
                        if let Some(parent) = number(grid.cell(r, col)) {
                            check.expected = Some(parent);
                            check.source = Some(Rect { top: r, left: col, bottom: r, right: col });
                            let within = value.abs() <= parent.abs() * (1.0 + 1e-9) + 1e-9
                                && (value == 0.0 || parent == 0.0 || (value > 0.0) == (parent > 0.0));
                            check.ok = Some(within);
                            keep = true;
                        }
                        break;
                    }
                }
                NonSumKind::Average => {
                    // The section above (since the previous total or average), and everything above
                    // without the totals and averages (an overall average under group averages).
                    let mut section: Vec<f64> = Vec::new();
                    let mut all: Vec<f64> = Vec::new();
                    let (mut section_top, mut all_top) = (f.row, f.row);
                    let mut section_done = false;
                    let mut blank = false;
                    while r > 0 {
                        r -= 1;
                        if !rows_with_cells.contains(&r) {
                            if !blank {
                                blank = true; // a spacer row inside the table
                                continue;
                            }
                            break;
                        }
                        blank = false;
                        let flags = if k > 0 && view.rows[k - 1] == r {
                            k -= 1;
                            Some(view.flags[k])
                        } else {
                            None
                        };
                        if kw_rows.contains_key(&r) || formula_total.contains(&(r, col)) || averages.contains(&r) {
                            // "Total" then "Average": average the rows above the total.
                            section_done |= !section.is_empty();
                            continue;
                        }
                        if of_which.contains(&r) {
                            continue;
                        }
                        let Some(fl) = flags else { continue };
                        if fl & ColumnView::LABEL != 0 {
                            break;
                        }
                        if fl & (ColumnView::NUMBER | ColumnView::NUMBER_TEXT) != 0 {
                            let v = view.values[k];
                            if !section_done {
                                section.push(v);
                                section_top = r;
                            }
                            all.push(v);
                            all_top = r;
                        }
                    }
                    // A "Median" row may hold the mean (and the other way round is not checked).
                    let fits = |values: &mut Vec<f64>| -> Option<(f64, bool)> {
                        if values.len() < 2 {
                            return None;
                        }
                        let mean = values.iter().sum::<f64>() / values.len() as f64;
                        if f.median {
                            values.sort_by(|a, b| a.total_cmp(b));
                            let m = values.len() / 2;
                            let median = if values.len() % 2 == 1 { values[m] } else { (values[m - 1] + values[m]) / 2.0 };
                            if close(value, median, true) {
                                return Some((median, true));
                            }
                        }
                        Some((mean, close(value, mean, true)))
                    };
                    let s = fits(&mut section);
                    let a = if all.len() > section.len() { fits(&mut all) } else { None };
                    let (expected, top, ok) = match (s, a) {
                        (Some((e, true)), _) => (Some(e), section_top, true),
                        (_, Some((e, true))) => (Some(e), all_top, true),
                        (Some((e, false)), _) => (Some(e), section_top, false),
                        (None, Some((e, false))) => (Some(e), all_top, false),
                        (None, None) => (None, f.row, false),
                    };
                    if let Some(e) = expected {
                        check.expected = Some(e);
                        check.source = Some(Rect { top, left: col, bottom: f.row - 1, right: col });
                        if ok {
                            check.ok = Some(true);
                            keep = true;
                        }
                    }
                }
            }
            checks.push(check);
        }
        // In a register a name starting with an average word, with more text beside it ("AVG
        // VITRAIS, UNIPESSOAL LDA | NIF | …"), is a record whose number met a mean by chance.
        if f.kind == NonSumKind::Average && f.plain == NonSumLabel::Phrase && sheet_rows(grid) >= REGISTER_MIN_ROWS {
            let more_text = grid.row_cells(f.row).iter().map(|&i| &cells[i as usize]).any(|c| {
                c.col != f.col && matches!(&c.value, Some(Value::Text(t)) if header_text(t) && !t.trim().is_empty())
            });
            if more_text {
                continue;
            }
        }
        if f.kind == NonSumKind::Average && !keep {
            // Unchecked: only a plain label over at least two numbers; a word at the end of its block.
            let block_end = !rows_with_cells.contains(&(f.row + 1))
                || kw_rows.contains_key(&(f.row + 1))
                || labelled.contains(&(f.row + 1));
            keep = checks.iter().any(|c| c.expected.is_some())
                && match f.plain {
                    NonSumLabel::Sign => true,
                    NonSumLabel::Bare => block_end,
                    NonSumLabel::Phrase => false,
                };
        }
        if keep {
            out.push(NonSumRow { row: f.row, label_col: f.col, kind: f.kind, cells: checks });
        }
    }
    out
}

/// Fewest numbers a section needs before a row without a total word may be taken for its sum
/// ([`unlabelled_totals`]).
const UNLABELLED_MIN_ADDENDS: usize = 3;

/// Fewest numbers above a single-column total without a total word.
const UNLABELLED_MIN_ADDENDS_ONE_COLUMN: usize = 5;

/// Running sums of one column for [`unlabelled_totals`]: (sum, non-zero numbers, first row).
#[derive(Default)]
struct Run {
    /// Since the previous total row (or the top of the block).
    section: (f64, usize, u32),
    /// Since the top of the block, without the total rows.
    all: (f64, usize, u32),
    /// Total rows inside `all`.
    totals: Vec<u32>,
}

/// Rows without a total word whose numbers are the sum of the rows above them (normalized output):
/// "France métropolitaine" under the départements, "DOM" under the overseas ones, "France
/// métropolitaine et DOM" over both. In at least two columns (and in at least two thirds of the
/// row's numbers, one of them not a small whole number) a number equals the sum of the at least
/// [`UNLABELLED_MIN_ADDENDS`] non-zero numbers above it,
/// since the previous total row (or everything above but the totals); a table with one number
/// column needs [`UNLABELLED_MIN_ADDENDS_ONE_COLUMN`]. A blank row ends a block; a label in a
/// column, or a row of years, starts that column again.
fn unlabelled_totals(
    grid: &Grid,
    kw_rows: &KeywordRows,
    skip: &RowSet,
    formula_total: &FxHashSet<(u32, u32)>,
    labels: &mut Labels,
) -> Vec<TotalCandidate> {
    let cells = &grid.sheet.cells;
    let formula_rows: FxHashSet<u32> = formula_total.iter().map(|&(r, _)| r).collect();
    let mut runs: FxHashMap<u32, Run> = FxHashMap::default();
    let mut out = Vec::new();
    let mut prev: Option<u32> = None;
    let mut i = 0;
    while i < cells.len() {
        let row = cells[i].row;
        let mut j = i;
        while j < cells.len() && cells[j].row == row {
            j += 1;
        }
        let line = &cells[i..j];
        i = j;
        if prev.is_some_and(|p| row > p + 1) {
            runs.clear();
        }
        prev = Some(row);
        let nums: Vec<(u32, f64)> = line
            .iter()
            .filter_map(|c| match c.value {
                Some(Value::Number(v)) => Some((c.col, v)),
                _ => None,
            })
            .collect();
        for c in line {
            if matches!(&c.value, Some(Value::Text(t)) if header_text(t) && !t.trim().is_empty()) {
                runs.remove(&c.col);
            }
        }
        if skip.contains(&row) || nums.is_empty() {
            continue;
        }
        let years = nums.len() >= 2 && nums.iter().all(|&(_, v)| v.fract() == 0.0 && (1800.0..=2100.0).contains(&v));
        if years {
            for (col, _) in &nums {
                runs.remove(col);
            }
            continue;
        }
        let total_row = kw_rows.contains_key(&row) || formula_rows.contains(&row);
        let mut matched: Vec<(u32, bool)> = Vec::new(); // (column, over everything above)
        let mut addends = 0;
        if !total_row {
            for &(col, v) in &nums {
                let Some(run) = runs.get(&col) else { continue };
                if v == 0.0 || line.iter().any(|c| c.col == col && c.formula.is_some()) {
                    continue;
                }
                if run.section.1 >= UNLABELLED_MIN_ADDENDS && close(v, run.section.0, true) {
                    matched.push((col, false));
                    addends = addends.max(run.section.1);
                } else if !run.totals.is_empty()
                    && run.all.1 >= UNLABELLED_MIN_ADDENDS
                    && close(v, run.all.0, true)
                {
                    matched.push((col, true));
                    addends = addends.max(run.all.1);
                }
            }
        }
        // Small whole numbers add up by chance ("2, 0, 0" over "3"): at least one match must be a
        // number of three digits or a fraction.
        let distinctive = matched
            .iter()
            .filter(|&&(col, _)| nums.iter().any(|&(c, v)| c == col && (v.abs() >= 100.0 || v.fract() != 0.0)))
            .count();
        let found = distinctive >= 1
            && (matched.len() >= 2 && 3 * matched.len() >= 2 * nums.len()
                || nums.len() == 1 && matched.len() == 1 && addends >= UNLABELLED_MIN_ADDENDS_ONE_COLUMN);
        if found {
            let label = labels.left(row, nums[0].0).map(|lc| (row, lc));
            for &(col, whole) in &matched {
                let run = &runs[&col];
                let mut c = TotalCandidate::new(row, col, TotalOrigin::Typed, Direction::Vertical);
                c.label = label;
                if whole {
                    c.ranges = vec![Rect { top: run.all.2, left: col, bottom: row - 1, right: col }];
                    c.exclude = run.totals.iter().map(|&t| (t, col)).collect();
                    c.level = TotalLevel::GrandTotal;
                    c.evidence.push(Evidence::SumsTotals);
                } else {
                    c.ranges = vec![Rect { top: run.section.2, left: col, bottom: row - 1, right: col }];
                }
                c.evidence.push(Evidence::ArithmeticMatch);
                c.evidence.push(Evidence::Unlabelled);
                out.push(c);
            }
        }
        for &(col, v) in &nums {
            let run = runs.entry(col).or_insert_with(|| Run {
                section: (0.0, 0, row),
                all: (0.0, 0, row),
                totals: Vec::new(),
            });
            if found || total_row {
                run.section = (0.0, 0, row + 1);
                run.totals.push(row);
            } else {
                // Zeros are no addends: a column of zeros and one small value adds up to it.
                let n = (v != 0.0) as usize;
                run.section.0 += v;
                run.section.1 += n;
                run.all.0 += v;
                run.all.1 += n;
            }
        }
    }
    out
}

/// World Bank aggregate codes (regions, income groups, lending groups, demographic groups, `WLD`):
/// rows of a country list that are no country.
const WB_AGGREGATES: &[&str] = &[
    "AFE", "AFW", "ARB", "CEB", "CSS", "EAP", "EAR", "EAS", "ECA", "ECS", "EMU", "EUU", "FCS", "HIC", "HPC", "IBD",
    "IBT", "IDA", "IDB", "IDX", "LAC", "LCN", "LDC", "LIC", "LMC", "LMY", "LTE", "MEA", "MIC", "MNA", "NAC", "OED",
    "OSS", "PRE", "PSS", "PST", "SAS", "SSA", "SSF", "SST", "TEA", "TEC", "TLA", "TMN", "TSA", "TSS", "UMC", "WLD",
];

/// A column of country codes needs this many codes, and this many distinct aggregate codes among
/// them, before its aggregate rows are taken for aggregates.
const COUNTRY_CODES_MIN: usize = 30;
const AGGREGATE_CODES_MIN: usize = 5;

/// Rows of known aggregates in a country list (normalized output): a column of three-letter codes
/// ([`COUNTRY_CODES_MIN`]) with World Bank aggregate codes among them ([`AGGREGATE_CODES_MIN`]
/// distinct ones): "World | WLD", "European Union | EUU", "High income | HIC". Their values are
/// weighted or partial, so they are no checkable sums; they are marked so a sum over the
/// countries does not count them again. Only rows holding a number. (row, label column), sorted.
fn aggregate_rows(grid: &Grid) -> Vec<(u32, u32)> {
    let cells = &grid.sheet.cells;
    let code = |s: &str| s.len() == 3 && s.bytes().all(|b| b.is_ascii_uppercase());
    let mut per_col: FxHashMap<u32, (usize, FxHashSet<&str>, Vec<u32>)> = FxHashMap::default();
    for c in cells {
        if let Some(Value::Text(t)) = &c.value {
            let t = t.trim();
            if code(t) {
                let e = per_col.entry(c.col).or_default();
                e.0 += 1;
                if WB_AGGREGATES.contains(&t) {
                    e.1.insert(t);
                    e.2.push(c.row);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (col, (n, aggs, rows)) in per_col {
        if n < COUNTRY_CODES_MIN || aggs.len() < AGGREGATE_CODES_MIN {
            continue;
        }
        for row in rows {
            let line = grid.row_cells(row).iter().map(|&i| &cells[i as usize]);
            let mut label = None;
            let mut has_number = false;
            for c in line {
                match &c.value {
                    Some(Value::Number(_)) => has_number = true,
                    Some(Value::Text(t)) if label.is_none() && header_text(t) && c.col != col => label = Some(c.col),
                    _ => {}
                }
            }
            if has_number {
                out.push((row, label.unwrap_or(col)));
            }
        }
    }
    out.sort_unstable();
    out.dedup_by_key(|x| x.0);
    out
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
            // A sibling that adds the rows below it says nothing about a source above.
            let below = |k: usize| cands[k].evidence.contains(&Evidence::SourceBelow);
            let count = |list: &[usize]| {
                let others = list.iter().filter(|&&j| j != i);
                let (mut n, mut ok) = (0, 0);
                for &j in others {
                    n += 1;
                    ok += (matched[j] && below(j) == below(i)) as usize;
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
