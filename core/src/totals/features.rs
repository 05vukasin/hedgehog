//! Per-cell `t_*` feature columns.

use crate::features::Features;
use crate::grid::Grid;
use crate::totals::deps::DependencyIndex;
use crate::totals::detect::detect_rows;
use crate::totals::verify::verify;

/// Names of the columns [`compute`] adds, in order.
pub const NAMES: [&str; 9] = [
    "t_is_formula",
    "t_is_aggregate_formula",
    "t_refs_above",
    "t_refs_left",
    "t_ref_count",
    "t_is_referenced",
    "t_referenced_by_aggregate",
    "t_in_keyword_total_row",
    "t_total_verified",
];

/// Adds the `t_*` columns (one value per `grid.sheet.cells` entry):
/// * `t_is_formula`, `t_is_aggregate_formula` (SUM/SUBTOTAL/AGGREGATE/`+` chain/AVERAGE/COUNT/MIN/MAX) — 1/0
/// * `t_refs_above` / `t_refs_left` — 1 if the formula has same-sheet references and all of them lie
///   above in the same column / left in the same row
/// * `t_ref_count` — number of references in the formula
/// * `t_is_referenced`, `t_referenced_by_aggregate` — 1 if a formula on this sheet (an aggregate) reads the cell
/// * `t_in_keyword_total_row` — 1 if the row is labelled with a total keyword
/// * `t_total_verified` — 1 total verified, 0 total wrong, -1 not a total or unknown
pub fn compute(grid: &Grid, out: &mut Features) {
    compute_returning(grid, out);
}

/// [`compute`], also returning the detected total candidates (so callers need not detect again).
pub fn compute_returning(grid: &Grid, out: &mut Features) -> Vec<crate::totals::TotalCandidate> {
    let n = grid.sheet.cells.len();
    let (t, cands) = Totals::new(grid);
    let mut cols: Vec<Vec<f32>> = vec![Vec::with_capacity(n); NAMES.len()];
    for i in 0..n {
        for (col, v) in cols.iter_mut().zip(t.cell(i)) {
            col.push(v);
        }
    }
    for (name, col) in NAMES.iter().zip(cols) {
        out.add(name, col);
    }
    cands
}

/// The `t_*` features one cell at a time: the dependency index, keyword rows and verified
/// candidates of the sheet (first pass); [`compute_returning`] is exactly this, cell by cell.
pub struct Totals<'g> {
    grid: &'g Grid<'g>,
    deps: &'g DependencyIndex,
    keyword_rows: std::collections::HashMap<u32, (u32, crate::totals::KeywordLevel)>,
    /// `t_total_verified` of the candidate cells (the last candidate on a cell decides).
    verified: rustc_hash::FxHashMap<usize, f32>,
}

impl<'g> Totals<'g> {
    /// First pass; also returns the detected total candidates.
    pub fn new(grid: &'g Grid<'g>) -> (Totals<'g>, Vec<crate::totals::TotalCandidate>) {
        let deps = grid.deps();
        // As the learned row model was trained: every total keyword counts, incidental or not.
        let crate::totals::Detection { candidates: cands, keyword_rows, .. } = detect_rows(grid, deps, false);
        let mut verified = rustc_hash::FxHashMap::default();
        for cand in &cands {
            if let Some(i) = grid.cell_index(cand.row, cand.col) {
                let v = match verify(grid, cand).ok {
                    Some(true) => 1.0,
                    Some(false) => 0.0,
                    None => -1.0,
                };
                verified.insert(i, v);
            }
        }
        (Totals { grid, deps, keyword_rows, verified }, cands)
    }

    /// The [`NAMES`] of cell `i` (index into `sheet.cells`), in order.
    pub fn cell(&self, i: usize) -> [f32; NAMES.len()] {
        let sheet = self.grid.sheet;
        let c = &sheet.cells[i];
        let mut f = [0f32; NAMES.len()];
        if let Some(p) = self.deps.formula(c.row, c.col) {
            f[0] = 1.0;
            f[1] = p.kind.is_aggregate() as u8 as f32;
            let local: Vec<_> = p.refs.iter().filter(|r| r.is_local(&sheet.name)).collect();
            if !local.is_empty() {
                f[2] = local.iter().all(|r| r.rect.left == c.col && r.rect.right == c.col && r.rect.bottom < c.row)
                    as u8 as f32;
                f[3] = local.iter().all(|r| r.rect.top == c.row && r.rect.bottom == c.row && r.rect.right < c.col)
                    as u8 as f32;
            }
            f[4] = p.refs.len() as f32;
        }
        f[5] = self.deps.is_referenced(i) as u8 as f32;
        f[6] = self.deps.referenced_by_aggregate(i) as u8 as f32;
        f[7] = self.keyword_rows.contains_key(&c.row) as u8 as f32;
        f[8] = self.verified.get(&i).copied().unwrap_or(-1.0);
        f
    }
}
