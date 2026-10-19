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

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

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

#[derive(Default, Clone, Copy)]
struct ColAgg {
    n: u32,
    kinds: [u32; Kind::COUNT],
    numeric: u32,
    distinct: u32,
    pairs: u32,
    inc: u32,
    last: Option<f64>,
    // running counters for the second pass
    seen_n: u32,
    seen_numeric: u32,
    seen_text: u32,
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
    let sheet = grid.sheet;
    let cells = &sheet.cells;
    let n = cells.len();
    let kinds: Vec<Kind> = cells.iter().map(kind_of).collect();
    let lab = label(grid);
    let comp_of = &lab.cell_component;
    let comps = &lab.components;

    let kind_at = |r: u32, c: u32| -> Kind {
        match grid.cell_index(r, c) {
            Some(i) => kinds[i],
            None if grid.merge_at(r, c).is_some() => Kind::MergedEmpty,
            None => Kind::Empty,
        }
    };

    // ---- aggregates (first pass) ----
    let mut sheet_rows: HashMap<u32, u32> = HashMap::new();
    let mut rows: HashMap<(u32, u32), RowAgg> = HashMap::new();
    let mut cols: HashMap<(u32, u32), ColAgg> = HashMap::new();
    let mut seen_values: HashSet<(u32, u32, u64)> = HashSet::new();
    for (i, cell) in cells.iter().enumerate() {
        let (k, comp) = (kinds[i], comp_of[i]);
        *sheet_rows.entry(cell.row).or_default() += 1;

        let ra = rows.entry((comp, cell.row)).or_insert(RowAgg { min_col: u32::MAX, ..Default::default() });
        ra.n += 1;
        ra.text += (k == Kind::Text) as u32;
        ra.num += (k == Kind::Number) as u32;
        ra.date += (k == Kind::Date) as u32;
        ra.formula += cell.formula.is_some() as u32;
        ra.numeric += k.is_numeric() as u32;
        ra.min_col = ra.min_col.min(cell.col);

        let ca = cols.entry((comp, cell.col)).or_default();
        ca.n += 1;
        ca.kinds[k as usize] += 1;
        ca.numeric += k.is_numeric() as u32;
        if seen_values.insert((comp, cell.col, value_hash(cell.value.as_ref(), cell.formula.as_deref()))) {
            ca.distinct += 1;
        }
        // Cells are row-major, so a column's cells arrive top to bottom.
        if let Some(v) = numeric_value(cell) {
            if let Some(last) = ca.last {
                ca.pairs += 1;
                ca.inc += (v > last) as u32;
            }
            ca.last = Some(v);
        }
    }
    drop(seen_values);

    // Per block: leftmost text-majority column, rightmost number-majority column.
    let mut text_col = vec![u32::MAX; comps.len()];
    let mut num_col = vec![None::<u32>; comps.len()];
    for (&(comp, col), ca) in &cols {
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
    let mut intervals: Vec<(u32, u32)> = sheet_rows.keys().map(|&r| (r, r)).collect();
    intervals.extend(sheet.merges.iter().map(|m| (m.top, m.bottom)));
    intervals.sort_unstable();
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for (a, b) in intervals {
        match runs.last_mut() {
            Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
            _ => runs.push((a, b)),
        }
    }
    let run_of = |r: u32| -> (u32, u32) {
        let i = runs.partition_point(|run| run.1 < r);
        runs[i]
    };

    let used = grid.used_range();
    let (used_top, used_left) = used.map_or((0, 0), |u| (u.top, u.left));

    // ---- per-cell features ----
    let mut f = Cols::new(n);
    let n_comps = comps.len() as f32;
    for (i, cell) in cells.iter().enumerate() {
        let (r, c, k) = (cell.row, cell.col, kinds[i]);
        let comp_id = comp_of[i];
        let comp = &comps[comp_id as usize];

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
        let width = comp.rect.right - comp.rect.left + 1;
        let height = comp.rect.bottom - comp.rect.top + 1;
        let ra = rows[&(comp_id, r)];
        let rn = ra.n as f32;
        f.push("c_row_filled", sheet_rows[&r] as f32);
        f.push("c_row_cells", rn);
        f.push("c_row_fill_ratio", rn / width as f32);
        f.push("c_row_text_ratio", ra.text as f32 / rn);
        f.push("c_row_num_ratio", ra.num as f32 / rn);
        f.push("c_row_date_ratio", ra.date as f32 / rn);
        f.push("c_row_formula_ratio", ra.formula as f32 / rn);
        f.push("c_row_single", b(ra.n == 1));
        f.push("c_row_first", b(ra.min_col == c));
        let next_numeric = r.checked_add(1).and_then(|nr| rows.get(&(comp_id, nr))).is_some_and(|x| x.numeric > 0);
        f.push("c_row_text_next_num", b(ra.text == ra.n && next_numeric));

        // Column aggregates.
        let ca = cols.get_mut(&(comp_id, c)).unwrap();
        let cn = ca.n as f32;
        let (maj_kind, maj_count) = ca.majority();
        f.push("c_col_cells", cn);
        f.push("c_col_consistency", maj_count as f32 / cn);
        f.push("c_col_type_match", b(maj_kind == k as usize));
        f.push("c_col_num_ratio", ca.numeric as f32 / cn);
        f.push("c_col_distinct_ratio", ca.distinct as f32 / cn);
        f.push("c_col_monotonic", if ca.pairs >= 2 { ca.inc as f32 / ca.pairs as f32 } else { 0.0 });
        let below_n = ca.n - ca.seen_n - 1;
        let below_num = ca.numeric - ca.seen_numeric - k.is_numeric() as u32;
        let num_below = if below_n > 0 { below_num as f32 / below_n as f32 } else { 0.0 };
        f.push("c_col_num_below", num_below);
        f.push("c_col_text_above", if ca.seen_n > 0 { ca.seen_text as f32 / ca.seen_n as f32 } else { 0.0 });
        ca.seen_n += 1;
        ca.seen_numeric += k.is_numeric() as u32;
        ca.seen_text += (k == Kind::Text) as u32;

        // Block / position.
        let row_off = r - comp.rect.top;
        let col_off = c - comp.rect.left;
        f.push("c_comp_cells", comp.cells.len() as f32);
        f.push("c_comp_rows", height as f32);
        f.push("c_comp_cols", width as f32);
        f.push("c_comp_row_off", row_off as f32);
        f.push("c_comp_col_off", col_off as f32);
        f.push("c_comp_row_rel", rel(row_off, height));
        f.push("c_comp_col_rel", rel(col_off, width));
        f.push("c_comp_first_row", b(r == comp.rect.top));
        f.push("c_comp_last_row", b(r == comp.rect.bottom));
        f.push("c_comp_first_col", b(c == comp.rect.left));
        f.push("c_comp_last_col", b(c == comp.rect.right));
        let (run_top, run_bottom) = run_of(r);
        f.push("c_gap_above", (r - run_top + 1) as f32);
        f.push("c_gap_below", (run_bottom - r + 1) as f32);
        f.push("c_sheet_components", n_comps);
        f.push("c_rows_from_top", r.saturating_sub(used_top) as f32);
        f.push("c_cols_from_left", c.saturating_sub(used_left) as f32);

        // Header-likeness helpers.
        f.push("c_text_over_numbers", if k == Kind::Text { num_below } else { 0.0 });
        let is_label_col = label_col[comp_id as usize] == Some(c);
        f.push("c_label_col", b(is_label_col));
        f.push("c_row_label", b(is_label_col && k == Kind::Text));
        f.end_row();
    }
    f.finish(out);
}

fn b(x: bool) -> f32 {
    x as u8 as f32
}

/// Offset scaled to 0..1 over a span (0 for a span of 1).
fn rel(off: u32, span: u32) -> f32 {
    if span > 1 { off as f32 / (span - 1) as f32 } else { 0.0 }
}

fn value_hash(v: Option<&Value>, formula: Option<&str>) -> u64 {
    let mut h = DefaultHasher::new();
    match v {
        Some(Value::Number(x)) => (1u8, x.to_bits()).hash(&mut h),
        Some(Value::DateTime(x)) => (3u8, x.to_bits()).hash(&mut h),
        Some(Value::Text(s)) => (2u8, s.trim()).hash(&mut h),
        Some(Value::Bool(x)) => (4u8, x).hash(&mut h),
        Some(Value::Error(s)) => (5u8, s).hash(&mut h),
        None => (6u8, formula).hash(&mut h),
    }
    h.finish()
}

/// Column-wise builder: every cell pushes the [`FEATURES`] in order, then calls `end_row`.
struct Cols {
    data: Vec<Vec<f32>>,
    next: usize,
}

impl Cols {
    fn new(rows: usize) -> Self {
        Cols { data: FEATURES.iter().map(|_| Vec::with_capacity(rows)).collect(), next: 0 }
    }

    #[inline]
    fn push(&mut self, name: &'static str, v: f32) {
        debug_assert_eq!(FEATURES[self.next], name, "feature push order differs from FEATURES");
        self.data[self.next].push(v);
        self.next += 1;
    }

    #[inline]
    fn end_row(&mut self) {
        assert_eq!(self.next, FEATURES.len(), "a cell pushed the wrong number of features");
        self.next = 0;
    }

    fn finish(self, out: &mut Features) {
        for (name, values) in FEATURES.iter().zip(self.data) {
            out.add(name, values);
        }
    }
}
