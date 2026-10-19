//! Per-cell `t_*` feature columns.

use crate::features::Features;
use crate::grid::Grid;
use crate::totals::deps::DependencyIndex;
use crate::totals::detect::{detect_with, keyword_rows};
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
    let sheet = grid.sheet;
    let n = sheet.cells.len();
    let deps = DependencyIndex::new(grid);
    let kw = keyword_rows(grid);
    let mut cols: Vec<Vec<f32>> = vec![vec![0.0; n]; NAMES.len()];
    for (i, c) in sheet.cells.iter().enumerate() {
        if let Some(p) = deps.formula(c.row, c.col) {
            cols[0][i] = 1.0;
            cols[1][i] = p.kind.is_aggregate() as u8 as f32;
            let local: Vec<_> = p.refs.iter().filter(|r| r.is_local(&sheet.name)).collect();
            if !local.is_empty() {
                cols[2][i] = local.iter().all(|r| r.rect.left == c.col && r.rect.right == c.col && r.rect.bottom < c.row)
                    as u8 as f32;
                cols[3][i] = local.iter().all(|r| r.rect.top == c.row && r.rect.bottom == c.row && r.rect.right < c.col)
                    as u8 as f32;
            }
            cols[4][i] = p.refs.len() as f32;
        }
        cols[5][i] = deps.is_referenced(i) as u8 as f32;
        cols[6][i] = deps.referenced_by_aggregate(i) as u8 as f32;
        cols[7][i] = kw.contains_key(&c.row) as u8 as f32;
        cols[8][i] = -1.0;
    }
    for cand in detect_with(grid, &deps) {
        if let Some(i) = grid.cell_index(cand.row, cand.col) {
            cols[8][i] = match verify(grid, &cand).ok {
                Some(true) => 1.0,
                Some(false) => 0.0,
                None => -1.0,
            };
        }
    }
    for (name, col) in NAMES.iter().zip(cols) {
        out.add(name, col);
    }
}
