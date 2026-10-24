//! Spatial / structural context of every cell — the `c_*` feature columns.
//!
//! Owned by the `context` workstream — see PLAN.md. Everything here is computed from coarse cell
//! types ([`Kind`]), positions and merged areas; no text parsing.
//!
//! "Block" below means the cell's 8-connected [`Component`] (see [`components`]). Row and column
//! aggregates are taken *within the block* (the cells of that row/column that belong to the same
//! component), so side-by-side tables don't pollute each other.
//!
//! Feature columns (one value per entry of `sheet.cells`; booleans are 0/1):
//!
//! | name | meaning |
//! |---|---|
//! | `c_kind` | own [`Kind`] code |
//! | `c_has_formula` | cell has a formula |
//! | `c_n_ul` `c_n_u` `c_n_ur` `c_n_l` `c_n_r` `c_n_dl` `c_n_d` `c_n_dr` | [`Kind`] code of the 8 neighbours (u=up, d=down, l=left, r=right) |
//! | `c_same_up` `c_same_down` `c_same_left` `c_same_right` | neighbour is a cell of the same kind |
//! | `c_empty_neighbours` | 0..8, neighbours that are neither a cell nor covered by a merge |
//! | `c_in_merge` `c_merge_width` `c_merge_height` `c_merge_top_left` | merged-area membership, size (0 if none), cell is its anchor |
//! | `c_row_filled` | cells in the whole sheet row |
//! | `c_row_cells` | cells in the row within the block |
//! | `c_row_fill_ratio` | `c_row_cells` / block width |
//! | `c_row_text_ratio` `c_row_num_ratio` `c_row_date_ratio` `c_row_formula_ratio` | shares of the block row's cells (num = number only) |
//! | `c_row_single` | the block row has exactly one cell |
//! | `c_row_first` | cell is the leftmost cell of its block row |
//! | `c_row_text_next_num` | block row is all text and the next block row has a number/date |
//! | `c_col_cells` | cells in the column within the block |
//! | `c_col_consistency` | share of the block column's majority kind |
//! | `c_col_type_match` | own kind equals the block column's majority kind |
//! | `c_col_num_ratio` | share of number/date cells in the block column |
//! | `c_col_distinct_ratio` | distinct values / cells in the block column |
//! | `c_col_monotonic` | share of increasing steps between consecutive number/date cells of the block column (0 if < 3 such cells) |
//! | `c_col_num_below` `c_col_text_above` | share of number/date cells below / text cells above the cell in its block column |
//! | `c_comp_cells` `c_comp_rows` `c_comp_cols` | block size: cells, height, width |
//! | `c_comp_row_off` `c_comp_col_off` | offset from the block's top / left |
//! | `c_comp_row_rel` `c_comp_col_rel` | same, scaled to 0..1 |
//! | `c_comp_first_row` `c_comp_last_row` `c_comp_first_col` `c_comp_last_col` | cell sits on the block's edge |
//! | `c_gap_above` `c_gap_below` | rows to the nearest sheet row with nothing in it (1 = adjacent) |
//! | `c_sheet_components` | number of blocks on the sheet |
//! | `c_rows_from_top` `c_cols_from_left` | offset from the sheet's used range |
//! | `c_text_over_numbers` | text cell × `c_col_num_below` |
//! | `c_label_col` | cell is in the block's leftmost text-majority column and a number-majority column lies to its right |
//! | `c_row_label` | `c_label_col` and the cell is text (row-label candidate) |

mod components;
mod kind;

pub use components::{components, label, Component, Labeling};
pub use kind::{kind_of, Kind};

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::features::Features;
use crate::grid::Grid;
use crate::model::Value;
use components::{offset, NEIGHBOURS};
use kind::numeric_value;

/// Names of all columns [`compute`] adds, in order.
pub const FEATURES: &[&str] = &[
    "c_kind", "c_has_formula",
    "c_n_ul", "c_n_u", "c_n_ur", "c_n_l", "c_n_r", "c_n_dl", "c_n_d", "c_n_dr",
    "c_same_up", "c_same_down", "c_same_left", "c_same_right", "c_empty_neighbours",
    "c_in_merge", "c_merge_width", "c_merge_height", "c_merge_top_left",
    "c_row_filled", "c_row_cells", "c_row_fill_ratio", "c_row_text_ratio", "c_row_num_ratio",
    "c_row_date_ratio", "c_row_formula_ratio", "c_row_single", "c_row_first", "c_row_text_next_num",
    "c_col_cells", "c_col_consistency", "c_col_type_match", "c_col_num_ratio", "c_col_distinct_ratio",
    "c_col_monotonic", "c_col_num_below", "c_col_text_above",
    "c_comp_cells", "c_comp_rows", "c_comp_cols", "c_comp_row_off", "c_comp_col_off",
    "c_comp_row_rel", "c_comp_col_rel", "c_comp_first_row", "c_comp_last_row",
    "c_comp_first_col", "c_comp_last_col", "c_gap_above", "c_gap_below", "c_sheet_components",
    "c_rows_from_top", "c_cols_from_left",
    "c_text_over_numbers", "c_label_col", "c_row_label",
];

#[derive(Default, Clone, Copy)]
struct RowAgg {
    n: u32,
    text: u32,
    num: u32,
    date: u32,
    formula: u32,
    numeric: u32,
    min_col: u32,
}

/// One sheet row: its cell count and, for row-major sheets, its (block, row) slots `a..b`.
struct SheetRow {
    row: u32,
    cells: u32,
    slots: (u32, u32),
}

#[derive(Default, Clone, Copy)]
struct ColAgg {
    n: u32,
    kinds: [u32; Kind::COUNT],
    numeric: u32,
    distinct: u32,
    pairs: u32,
    inc: u32,
    last: Option<f64>,
}

impl ColAgg {
    fn majority(&self) -> (usize, u32) {
        let mut best = (0, 0);
        for (k, &c) in self.kinds.iter().enumerate() {
            if c > best.1 {
                best = (k, c);
            }
        }
        best
    }
}

/// Adds all `c_*` columns (see the module docs) to `out`, which must have one row per sheet cell.
/// O(cells + merged-area perimeters).
pub fn compute(grid: &Grid, out: &mut Features) {
    let n = grid.sheet.cells.len();
    let ctx = Context::new(grid);
    let mut cur = ctx.cursors(&[0]).pop().expect("one cursor");
    let mut data: Vec<Vec<f32>> = FEATURES.iter().map(|_| Vec::with_capacity(n)).collect();
    let mut row = [0f32; N];
    for i in 0..n {
        ctx.cell(&mut cur, i, &mut row);
        for (col, &v) in data.iter_mut().zip(&row) {
            col.push(v);
        }
    }
    for (name, values) in FEATURES.iter().zip(data) {
        out.add(name, values);
    }
}

/// Number of `c_*` features.
pub const N: usize = 56;
const _: () = assert!(FEATURES.len() == N);

/// The sheet-wide state behind the `c_*` features (first pass: kinds, blocks, row and column
/// aggregates), so a caller can produce the features of one cell at a time instead of holding
/// every column ([`compute`] is exactly this, cell by cell).
pub struct Context<'g> {
    grid: &'g Grid<'g>,
    kinds: Vec<Kind>,
    comp_of: Vec<u32>,
    /// Per block: bounding box and cell count.
    comps: Vec<(crate::model::Rect, u32)>,
    sheet_row_slot: Vec<u32>,
    sheet_rows: Vec<SheetRow>,
    row_slot: Vec<u32>,
    row_aggs: Vec<RowAgg>,
    next_numeric: Vec<bool>,
    col_slot: Vec<u32>,
    col_aggs: Vec<ColAgg>,
    label_col: Vec<Option<u32>>,
    runs: Vec<(u32, u32)>,
    used_top: u32,
    used_left: u32,
}

/// Where a walk over the cells with [`Context::cell`] stands: the next cell, and per (block,
/// column) how many cells, number/date cells and text cells it has passed.
pub struct Cursor {
    next: usize,
    seen: Vec<[u32; 3]>,
}

impl<'g> Context<'g> {
    /// First pass over the sheet. O(cells + merged-area perimeters).
    pub fn new(grid: &'g Grid<'g>) -> Context<'g> {
        let sheet = grid.sheet;
        let cells = &sheet.cells;
        let n = cells.len();
        let kinds: Vec<Kind> = cells.iter().map(kind_of).collect();
        let lab = label(grid);
        let comp_of = lab.cell_component;
        let comps: Vec<(crate::model::Rect, u32)> =
            lab.components.iter().map(|c| (c.rect, c.cells.len() as u32)).collect();
        drop(lab.components);

        // ---- aggregates (first pass) ----
        // Every cell gets a slot for its sheet row, its (block, row) and its (block, column), so the
        // per-cell pass below indexes vectors instead of hashing.
        let mut sheet_row_slot: Vec<u32> = Vec::with_capacity(n);
        let mut sheet_rows: Vec<SheetRow> = Vec::new();
        let mut row_slot: Vec<u32> = Vec::with_capacity(n);
        let mut row_keys: Vec<(u32, u32)> = Vec::new(); // (block, row)
        let mut row_aggs: Vec<RowAgg> = Vec::new();
        // Row-major sheets (what the reader produces) need no maps: a row's cells are contiguous and
        // its (block, row) slots are the few created since the row started. Otherwise: hash maps.
        let row_major = cells.windows(2).all(|w| (w[0].row, w[0].col) < (w[1].row, w[1].col));
        let mut sheet_row_index: HashMap<u32, u32> = HashMap::default();
        let mut row_index: HashMap<(u32, u32), u32> = HashMap::default();
        let mut last_sheet_row: Option<(u32, u32)> = None;
        let mut last_row: Option<((u32, u32), u32)> = None;
        for (i, cell) in cells.iter().enumerate() {
            let (k, comp) = (kinds[i], comp_of[i]);
            let sr = match last_sheet_row {
                Some((r, slot)) if r == cell.row => slot,
                _ => {
                    let mut new_row = || {
                        let first = row_aggs.len() as u32;
                        sheet_rows.push(SheetRow { row: cell.row, cells: 0, slots: (first, first) });
                        (sheet_rows.len() - 1) as u32
                    };
                    let slot = if row_major { new_row() } else { *sheet_row_index.entry(cell.row).or_insert_with(new_row) };
                    last_sheet_row = Some((cell.row, slot));
                    slot
                }
            };
            sheet_rows[sr as usize].cells += 1;
            sheet_row_slot.push(sr);

            let key = (comp, cell.row);
            let slot = match last_row {
                Some((kk, slot)) if kk == key => slot,
                _ => {
                    let found = if row_major {
                        let (a, b) = sheet_rows[sr as usize].slots;
                        (a..b).find(|&s| row_keys[s as usize].0 == comp)
                    } else {
                        row_index.get(&key).copied()
                    };
                    let slot = found.unwrap_or_else(|| {
                        row_keys.push(key);
                        row_aggs.push(RowAgg { min_col: u32::MAX, ..Default::default() });
                        let s = (row_aggs.len() - 1) as u32;
                        if row_major {
                            sheet_rows[sr as usize].slots.1 += 1;
                        } else {
                            row_index.insert(key, s);
                        }
                        s
                    });
                    last_row = Some((key, slot));
                    slot
                }
            };
            row_slot.push(slot);
            let ra = &mut row_aggs[slot as usize];
            ra.n += 1;
            ra.text += (k == Kind::Text) as u32;
            ra.num += (k == Kind::Number) as u32;
            ra.date += (k == Kind::Date) as u32;
            ra.formula += cell.formula.is_some() as u32;
            ra.numeric += k.is_numeric() as u32;
            ra.min_col = ra.min_col.min(cell.col);
        }
        drop(sheet_row_index);
        // Whether the block's next row has a number/date, per (block, row) slot.
        let next_numeric: Vec<bool> = if row_major {
            let mut v = vec![false; row_aggs.len()];
            for w in sheet_rows.windows(2) {
                if w[0].row.checked_add(1) != Some(w[1].row) {
                    continue;
                }
                for s in w[0].slots.0..w[0].slots.1 {
                    let comp = row_keys[s as usize].0;
                    v[s as usize] = (w[1].slots.0..w[1].slots.1)
                        .find(|&t| row_keys[t as usize].0 == comp)
                        .is_some_and(|t| row_aggs[t as usize].numeric > 0);
                }
            }
            v
        } else {
            row_keys
                .iter()
                .map(|&(comp, r)| {
                    r.checked_add(1)
                        .and_then(|nr| row_index.get(&(comp, nr)))
                        .is_some_and(|&s| row_aggs[s as usize].numeric > 0)
                })
                .collect()
        };
        drop((row_index, row_keys));

        // Columns: visit cells column by column (stable, so each column keeps sheet order — top to
        // bottom for a row-major sheet), probing the map only when the block changes.
        let by_col = stable_order_by_col(cells);
        let mut col_slot: Vec<u32> = vec![0; n];
        let mut col_keys: Vec<(u32, u32)> = Vec::new(); // (block, col)
        let mut col_aggs: Vec<ColAgg> = Vec::new();
        let mut col_index: HashMap<(u32, u32), u32> = HashMap::default();
        let mut seen_values: HashSet<(u32, ValueKey)> = HashSet::default();
        let mut last_col: Option<((u32, u32), u32)> = None;
        for &i in &by_col {
            let i = i as usize;
            let cell = &cells[i];
            let (k, comp) = (kinds[i], comp_of[i]);
            // Column order is grouped: earlier columns' distinct values are no longer needed.
            // Keep the component in the key because components may interleave within a column.
            if last_col.is_some_and(|((_, col), _)| col != cell.col) {
                seen_values.clear();
            }
            let key = (comp, cell.col);
            let slot = match last_col {
                Some((kk, slot)) if kk == key => slot,
                _ => {
                    let slot = *col_index.entry(key).or_insert_with(|| {
                        col_keys.push(key);
                        col_aggs.push(ColAgg::default());
                        (col_aggs.len() - 1) as u32
                    });
                    last_col = Some((key, slot));
                    slot
                }
            };
            col_slot[i] = slot;
            let ca = &mut col_aggs[slot as usize];
            ca.n += 1;
            ca.kinds[k as usize] += 1;
            ca.numeric += k.is_numeric() as u32;
            if seen_values.insert((comp, value_key(cell.value.as_ref(), cell.formula.as_deref()))) {
                ca.distinct += 1;
            }
            if let Some(v) = numeric_value(cell) {
                if let Some(last) = ca.last {
                    ca.pairs += 1;
                    ca.inc += (v > last) as u32;
                }
                ca.last = Some(v);
            }
        }
        drop((seen_values, col_index, by_col));

        // Per block: leftmost text-majority column, rightmost number-majority column.
        let mut text_col = vec![u32::MAX; comps.len()];
        let mut num_col = vec![None::<u32>; comps.len()];
        for (&(comp, col), ca) in col_keys.iter().zip(&col_aggs) {
            let c = comp as usize;
            if ca.kinds[Kind::Text as usize] * 2 > ca.n {
                text_col[c] = text_col[c].min(col);
            }
            if ca.numeric * 2 > ca.n {
                num_col[c] = Some(num_col[c].map_or(col, |x| x.max(col)));
            }
        }
        let label_col: Vec<Option<u32>> = (0..comps.len())
            .map(|c| match num_col[c] {
                Some(nc) if text_col[c] != u32::MAX && nc > text_col[c] => Some(text_col[c]),
                _ => None,
            })
            .collect();

        // Runs of consecutive sheet rows that contain anything (cells or merged areas).
        let mut intervals: Vec<(u32, u32)> = sheet_rows.iter().map(|r| (r.row, r.row)).collect();
        intervals.extend(sheet.merges.iter().map(|m| (m.top, m.bottom)));
        intervals.sort_unstable();
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (a, b) in intervals {
            match runs.last_mut() {
                Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
                _ => runs.push((a, b)),
            }
        }

        let used = grid.used_range();
        let (used_top, used_left) = used.map_or((0, 0), |u| (u.top, u.left));
        Context {
            grid,
            kinds,
            comp_of,
            comps,
            sheet_row_slot,
            sheet_rows,
            row_slot,
            row_aggs,
            next_numeric,
            col_slot,
            col_aggs,
            label_col,
            runs,
            used_top,
            used_left,
        }
    }

    /// One [`Cursor`] per entry of `starts` (ascending cell indices), standing at that cell as if
    /// every cell before it had been passed to [`Context::cell`]. O(cells + starts × columns).
    pub fn cursors(&self, starts: &[usize]) -> Vec<Cursor> {
        let mut seen = vec![[0u32; 3]; self.col_aggs.len()];
        let mut out = Vec::with_capacity(starts.len());
        let mut i = 0;
        for &start in starts {
            assert!(i <= start, "cursor starts must ascend");
            while i < start {
                let k = self.kinds[i];
                let s = &mut seen[self.col_slot[i] as usize];
                s[0] += 1;
                s[1] += k.is_numeric() as u32;
                s[2] += (k == Kind::Text) as u32;
                i += 1;
            }
            out.push(Cursor { next: start, seen: seen.clone() });
        }
        out
    }

    /// The [`FEATURES`] of cell `i` (index into `sheet.cells`), in order. A cursor's cells must come
    /// in sheet order, each once (the counts above / below a cell in its column are running
    /// counters); [`compute`] walks one cursor over every cell.
    pub fn cell(&self, cur: &mut Cursor, i: usize, out: &mut [f32; N]) {
        assert_eq!(i, cur.next, "context features must be produced in sheet order");
        cur.next += 1;
        let grid = self.grid;
        let cell = &grid.sheet.cells[i];
        let kinds = &self.kinds;
        let kind_at = |r: u32, c: u32| -> Kind {
            match grid.cell_index(r, c) {
                Some(i) => kinds[i],
                None if grid.merge_at(r, c).is_some() => Kind::MergedEmpty,
                None => Kind::Empty,
            }
        };
        let mut f = Out { out, next: 0 };
        let (r, c, k) = (cell.row, cell.col, kinds[i]);
        let comp_id = self.comp_of[i];
        let (comp_rect, comp_cells) = self.comps[comp_id as usize];

        f.push("c_kind", k.code());
        f.push("c_has_formula", b(cell.formula.is_some()));

        // Neighbourhood.
        let mut nk = [Kind::Empty; 8];
        for (slot, (dr, dc)) in NEIGHBOURS.iter().enumerate() {
            nk[slot] = offset(r, c, *dr, *dc).map_or(Kind::Empty, |(nr, nc)| kind_at(nr, nc));
        }
        for (slot, name) in ["c_n_ul", "c_n_u", "c_n_ur", "c_n_l", "c_n_r", "c_n_dl", "c_n_d", "c_n_dr"]
            .into_iter()
            .enumerate()
        {
            f.push(name, nk[slot].code());
        }
        let same = |x: Kind| b(x == k && x != Kind::Empty && x != Kind::MergedEmpty);
        f.push("c_same_up", same(nk[1]));
        f.push("c_same_down", same(nk[6]));
        f.push("c_same_left", same(nk[3]));
        f.push("c_same_right", same(nk[4]));
        f.push("c_empty_neighbours", nk.iter().filter(|&&x| x == Kind::Empty).count() as f32);

        let merge = grid.merge_at(r, c);
        f.push("c_in_merge", b(merge.is_some()));
        f.push("c_merge_width", merge.map_or(0.0, |m| (m.right - m.left + 1) as f32));
        f.push("c_merge_height", merge.map_or(0.0, |m| (m.bottom - m.top + 1) as f32));
        f.push("c_merge_top_left", b(merge.is_some_and(|m| m.top == r && m.left == c)));

        // Row aggregates.
        let width = comp_rect.right - comp_rect.left + 1;
        let height = comp_rect.bottom - comp_rect.top + 1;
        let ra = self.row_aggs[self.row_slot[i] as usize];
        let rn = ra.n as f32;
        f.push("c_row_filled", self.sheet_rows[self.sheet_row_slot[i] as usize].cells as f32);
        f.push("c_row_cells", rn);
        f.push("c_row_fill_ratio", rn / width as f32);
        f.push("c_row_text_ratio", ra.text as f32 / rn);
        f.push("c_row_num_ratio", ra.num as f32 / rn);
        f.push("c_row_date_ratio", ra.date as f32 / rn);
        f.push("c_row_formula_ratio", ra.formula as f32 / rn);
        f.push("c_row_single", b(ra.n == 1));
        f.push("c_row_first", b(ra.min_col == c));
        f.push("c_row_text_next_num", b(ra.text == ra.n && self.next_numeric[self.row_slot[i] as usize]));

        // Column aggregates.
        let ca = &self.col_aggs[self.col_slot[i] as usize];
        let seen = &mut cur.seen[self.col_slot[i] as usize];
        let cn = ca.n as f32;
        let (maj_kind, maj_count) = ca.majority();
        f.push("c_col_cells", cn);
        f.push("c_col_consistency", maj_count as f32 / cn);
        f.push("c_col_type_match", b(maj_kind == k as usize));
        f.push("c_col_num_ratio", ca.numeric as f32 / cn);
        f.push("c_col_distinct_ratio", ca.distinct as f32 / cn);
        f.push("c_col_monotonic", if ca.pairs >= 2 { ca.inc as f32 / ca.pairs as f32 } else { 0.0 });
        let below_n = ca.n - seen[0] - 1;
        let below_num = ca.numeric - seen[1] - k.is_numeric() as u32;
        let num_below = if below_n > 0 { below_num as f32 / below_n as f32 } else { 0.0 };
        f.push("c_col_num_below", num_below);
        f.push("c_col_text_above", if seen[0] > 0 { seen[2] as f32 / seen[0] as f32 } else { 0.0 });
        seen[0] += 1;
        seen[1] += k.is_numeric() as u32;
        seen[2] += (k == Kind::Text) as u32;

        // Block / position.
        let row_off = r - comp_rect.top;
        let col_off = c - comp_rect.left;
        f.push("c_comp_cells", comp_cells as f32);
        f.push("c_comp_rows", height as f32);
        f.push("c_comp_cols", width as f32);
        f.push("c_comp_row_off", row_off as f32);
        f.push("c_comp_col_off", col_off as f32);
        f.push("c_comp_row_rel", rel(row_off, height));
        f.push("c_comp_col_rel", rel(col_off, width));
        f.push("c_comp_first_row", b(r == comp_rect.top));
        f.push("c_comp_last_row", b(r == comp_rect.bottom));
        f.push("c_comp_first_col", b(c == comp_rect.left));
        f.push("c_comp_last_col", b(c == comp_rect.right));
        let (run_top, run_bottom) = self.runs[self.runs.partition_point(|run| run.1 < r)];
        f.push("c_gap_above", (r - run_top + 1) as f32);
        f.push("c_gap_below", (run_bottom - r + 1) as f32);
        f.push("c_sheet_components", self.comps.len() as f32);
        f.push("c_rows_from_top", r.saturating_sub(self.used_top) as f32);
        f.push("c_cols_from_left", c.saturating_sub(self.used_left) as f32);

        // Header-likeness helpers.
        f.push("c_text_over_numbers", if k == Kind::Text { num_below } else { 0.0 });
        let is_label_col = self.label_col[comp_id as usize] == Some(c);
        f.push("c_label_col", b(is_label_col));
        f.push("c_row_label", b(is_label_col && k == Kind::Text));
        f.end();
    }
}

/// Cell indices grouped by column (ascending), each column in sheet order.
fn stable_order_by_col(cells: &[crate::model::Cell]) -> Vec<u32> {
    let n = cells.len();
    let max_col = cells.iter().map(|c| c.col).max().unwrap_or(0) as usize;
    if max_col > 4 * n + 16_384 {
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.sort_by_key(|&i| cells[i as usize].col); // stable
        return order;
    }
    // Counting sort: O(cells + columns).
    let mut start = vec![0u32; max_col + 2];
    for c in cells {
        start[c.col as usize + 1] += 1;
    }
    for j in 1..start.len() {
        start[j] += start[j - 1];
    }
    let mut order = vec![0u32; n];
    for (i, c) in cells.iter().enumerate() {
        let s = &mut start[c.col as usize];
        order[*s as usize] = i as u32;
        *s += 1;
    }
    order
}

fn b(x: bool) -> f32 {
    x as u8 as f32
}

/// Offset scaled to 0..1 over a span (0 for a span of 1).
fn rel(off: u32, span: u32) -> f32 {
    if span > 1 { off as f32 / (span - 1) as f32 } else { 0.0 }
}

/// What makes two cells "the same value" for `c_col_distinct_ratio` (text compared trimmed;
/// a formula without cached value by its formula).
#[derive(PartialEq, Eq, Hash)]
enum ValueKey<'a> {
    Number(u64),
    Text(&'a str),
    Date(u64),
    Bool(bool),
    Error(&'a str),
    Formula(Option<&'a str>),
}

fn value_key<'a>(v: Option<&'a Value>, formula: Option<&'a str>) -> ValueKey<'a> {
    match v {
        Some(Value::Number(x)) => ValueKey::Number(x.to_bits()),
        Some(Value::DateTime(x)) => ValueKey::Date(x.to_bits()),
        Some(Value::Text(s)) => ValueKey::Text(s.trim()),
        Some(Value::Bool(x)) => ValueKey::Bool(*x),
        Some(Value::Error(s)) => ValueKey::Error(s),
        None => ValueKey::Formula(formula),
    }
}

/// Writes one cell's [`FEATURES`] in order (the names check the order in debug builds).
struct Out<'a> {
    out: &'a mut [f32; N],
    next: usize,
}

impl Out<'_> {
    #[inline]
    fn push(&mut self, name: &'static str, v: f32) {
        debug_assert_eq!(FEATURES[self.next], name, "feature push order differs from FEATURES");
        self.out[self.next] = v;
        self.next += 1;
    }

    #[inline]
    fn end(&self) {
        assert_eq!(self.next, N, "a cell pushed the wrong number of features");
    }
}
