//! Training data for the learned cell-role model: one CSV row per labelled cell with
//! every core feature ([`crate::context`], [`crate::values`], [`crate::totals`]), the rule layout's
//! role for the cell and its neighbours (`l_*`), and the ground-truth role.
//!
//! Data cells are most of every sheet; only a deterministic fraction of them is written, with
//! `weight = 1 / fraction` so weighted metrics stay unbiased. Columns: `source, file, sheet, row,
//! col, split, label, weight`, then the features in a fixed order (see the header line).

use std::io::Write;

use crate::analysis::Role;
use crate::features::Features;
use crate::grid::Grid;

use super::gt::{Corpus, GtFile};
use super::split::{fnv1a, Split};

/// What to export.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// `deco`, `synthetic`, … (written to the `source` column).
    pub source: String,
    pub split: Split,
    /// Share of Data cells kept (others always kept).
    pub data_keep: f32,
    pub limit: Option<usize>,
    pub progress: bool,
}

/// Counts of what was written.
#[derive(Debug, Clone, Default)]
pub struct ExportStats {
    pub files: usize,
    pub failed: usize,
    pub sheets: usize,
    pub rows: usize,
    pub per_role: [usize; Role::ALL.len()],
}

/// Role as a small integer, in [`Role::ALL`] order (the `label` column and the `l_*` features;
/// -1 = no cell).
pub fn role_id(r: Role) -> i32 {
    Role::ALL.iter().position(|&x| x == r).map_or(-1, |i| i as i32)
}

/// Every feature column for one sheet, in a fixed order: core features, then layout features.
pub fn sheet_features(grid: &Grid) -> Features {
    sheet_features_with(grid, &crate::layout::analyze_for_model(grid))
}

/// [`sheet_features`] with the rule layout already computed.
pub fn sheet_features_with(grid: &Grid, lay: &crate::analysis::SheetLayout) -> Features {
    let n = grid.sheet.cells.len();
    let mut f = Features::new(n);
    crate::context::compute(grid, &mut f);
    crate::values::features::compute(grid, &mut f);
    let cands = crate::totals::features::compute_returning(grid, &mut f);
    finish(grid, lay, &cands, &mut f, true);
    f
}

/// Sheets with at least this many cells compute their feature groups on parallel threads.
pub(crate) const PARALLEL_MIN_CELLS: usize = 20_000;

/// The rule layout and [`sheet_features`] of a sheet, the independent parts (layout, context,
/// values, totals) on parallel threads for larger sheets. Same result as the sequential path.
/// Without `styles`, the formatting features ([`STYLE_FEATURES`]) are left out (a model that does
/// not read them need not pay their memory).
pub fn layout_and_features(grid: &Grid, styles: bool) -> (crate::analysis::SheetLayout, Features) {
    let n = grid.sheet.cells.len();
    if n < PARALLEL_MIN_CELLS {
        let lay = crate::layout::analyze_for_model(grid);
        let mut f = Features::new(n);
        crate::context::compute(grid, &mut f);
        crate::values::features::compute(grid, &mut f);
        let cands = crate::totals::features::compute_returning(grid, &mut f);
        finish(grid, &lay, &cands, &mut f, styles);
        return (lay, f);
    }
    let (lay, mut f, values, (totals, cands)) = std::thread::scope(|s| {
        let lay = s.spawn(|| crate::layout::analyze_for_model(grid));
        let values = s.spawn(|| {
            let mut f = Features::new(n);
            crate::values::features::compute(grid, &mut f);
            f
        });
        let totals = s.spawn(|| {
            let mut f = Features::new(n);
            let c = crate::totals::features::compute_returning(grid, &mut f);
            (f, c)
        });
        let mut ctx = Features::new(n);
        crate::context::compute(grid, &mut ctx);
        (join(lay.join()), ctx, join(values.join()), join(totals.join()))
    });
    f.append(values);
    f.append(totals);
    finish(grid, &lay, &cands, &mut f, styles);
    (lay, f)
}

/// A scoped thread's result, re-raising its panic.
fn join<T>(r: std::thread::Result<T>) -> T {
    r.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// The `l_*` (rule layout), `x_*` (detected totals) and, with `styles`, `s_*` (formatting)
/// columns, after the core features.
fn finish(
    grid: &Grid,
    lay: &crate::analysis::SheetLayout,
    cands: &[crate::totals::TotalCandidate],
    f: &mut Features,
    styles: bool,
) {
    let n = grid.sheet.cells.len();
    let lx = LayoutTotals::new(grid, lay, cands);
    let mut cols: Vec<Vec<f32>> = (0..LAYOUT_TOTALS.len()).map(|_| Vec::with_capacity(n)).collect();
    for i in 0..n {
        for (col, v) in cols.iter_mut().zip(lx.cell(i)) {
            col.push(v);
        }
    }
    drop(lx);
    for (name, col) in LAYOUT_TOTALS.iter().zip(cols) {
        f.add(name, col);
    }
    if styles {
        add_style_features(grid, f);
    }
}

/// The `l_*` (rule layout) and `x_*` (detected totals) features, in column order.
pub const LAYOUT_TOTALS: [&str; 11] = [
    "l_role",
    "l_role_up",
    "l_role_down",
    "l_role_left",
    "l_role_right",
    "l_in_table",
    "x_total_cand",
    "x_total_level",
    "x_row_vtotals",
    "x_row_vtotal_frac",
    "x_col_htotal_frac",
];

/// The [`LAYOUT_TOTALS`] features one cell at a time (what [`finish`] adds, cell by cell).
///
/// `x_*`: detected totals around the cell. DECO labels a whole total row (its label and every
/// number), so whether the row holds detected totals matters as much as the cell itself.
pub(crate) struct LayoutTotals<'g> {
    grid: &'g Grid<'g>,
    lay: &'g crate::analysis::SheetLayout,
    in_table: Vec<bool>,
    /// Candidate cells: (`x_total_cand`, `x_total_level`); the last candidate on a cell decides.
    cand: rustc_hash::FxHashMap<usize, (f32, f32)>,
    /// Rows with vertical total candidates: (count, count / numbers in the row).
    row_v: rustc_hash::FxHashMap<u32, (u32, f32)>,
    /// Columns with horizontal total candidates: count / numbers in the column.
    col_h: rustc_hash::FxHashMap<u32, f32>,
}

impl<'g> LayoutTotals<'g> {
    pub(crate) fn new(
        grid: &'g Grid<'g>,
        lay: &'g crate::analysis::SheetLayout,
        cands: &[crate::totals::TotalCandidate],
    ) -> LayoutTotals<'g> {
        use crate::totals::{Direction, TotalLevel};
        let mut cand = rustc_hash::FxHashMap::default();
        let mut row_v: rustc_hash::FxHashMap<u32, u32> = Default::default();
        let mut col_h: rustc_hash::FxHashMap<u32, u32> = Default::default();
        for c in cands {
            let Some(i) = grid.cell_index(c.row, c.col) else { continue };
            let vertical = c.direction == Direction::Vertical;
            let level = match c.level {
                TotalLevel::Total => 1.0,
                TotalLevel::Subtotal => 2.0,
                TotalLevel::GrandTotal => 3.0,
            };
            cand.insert(i, (if vertical { 1.0 } else { 2.0 }, level));
            if vertical {
                *row_v.entry(c.row).or_default() += 1;
            } else {
                *col_h.entry(c.col).or_default() += 1;
            }
        }
        let cells = &grid.sheet.cells;
        let numeric = |idx: &[u32]| {
            idx.iter().filter(|&&i| matches!(cells[i as usize].value, Some(crate::model::Value::Number(_)))).count().max(1)
        };
        // A row or column without candidates has 0 / numbers = +0.0.
        let row_v = row_v.into_iter().map(|(r, v)| (r, (v, v as f32 / numeric(grid.row_cells(r)) as f32))).collect();
        let col_h = col_h.into_iter().map(|(c, h)| (c, h as f32 / numeric(grid.col_cells(c)) as f32)).collect();
        LayoutTotals { grid, lay, in_table: in_tables(&grid.sheet.cells, &lay.tables), cand, row_v, col_h }
    }

    /// The [`LAYOUT_TOTALS`] of cell `i` (index into `sheet.cells`), in order.
    pub(crate) fn cell(&self, i: usize) -> [f32; LAYOUT_TOTALS.len()] {
        let grid = self.grid;
        let cells = &grid.sheet.cells;
        let roles = &self.lay.roles;
        let role_at = |r: i64, c: i64| -> f32 {
            if r < 0 || c < 0 {
                return -1.0;
            }
            grid.cell_index(r as u32, c as u32).map_or(-1.0, |i| role_id(roles[i]) as f32)
        };
        let c = &cells[i];
        let (r, k) = (c.row as i64, c.col as i64);
        let (cand, level) = self.cand.get(&i).copied().unwrap_or((0.0, 0.0));
        let (v, rf) = self.row_v.get(&c.row).copied().unwrap_or((0, 0.0));
        let cf = self.col_h.get(&c.col).copied().unwrap_or(0.0);
        [
            role_id(roles[i]) as f32,
            role_at(r - 1, k),
            role_at(r + 1, k),
            role_at(r, k - 1),
            role_at(r, k + 1),
            self.in_table[i] as u8 as f32,
            cand,
            level,
            v as f32,
            rf,
            cf,
        ]
    }
}

/// Per cell: whether any table's rectangle contains it (`l_in_table`). A sweep over the rows with
/// the tables that span the current row, instead of testing every table for every cell:
/// O(cells × overlapping tables + tables · log) rather than O(cells × tables).
pub(crate) fn in_tables(cells: &[crate::model::Cell], tables: &[crate::analysis::DetectedTable]) -> Vec<bool> {
    let inside = |r: &crate::model::Rect, c: &crate::model::Cell| r.left <= c.col && c.col <= r.right;
    let mut rects: Vec<crate::model::Rect> = tables.iter().map(|t| t.rect).collect();
    rects.sort_by_key(|r| r.top);
    let mut order: Vec<u32> = Vec::new();
    if !cells.windows(2).all(|w| w[0].row <= w[1].row) {
        order = (0..cells.len() as u32).collect();
        order.sort_by_key(|&i| cells[i as usize].row);
    }
    let mut out = vec![false; cells.len()];
    let mut active: Vec<crate::model::Rect> = Vec::new();
    let (mut next, mut row) = (0usize, None::<u32>);
    for k in 0..cells.len() {
        let i = if order.is_empty() { k } else { order[k] as usize };
        let c = &cells[i];
        if row != Some(c.row) {
            row = Some(c.row);
            // Rows only grow: a table that ended above this row is done for good.
            active.retain(|r| r.bottom >= c.row);
            while next < rects.len() && rects[next].top <= c.row {
                if rects[next].bottom >= c.row {
                    active.push(rects[next]);
                }
                next += 1;
            }
        }
        out[i] = active.iter().any(|r| inside(r, c));
    }
    out
}

/// The formatting features (`s_*`), in column order.
pub const STYLE_FEATURES: [&str; 14] = [
    "s_known",
    "s_bold",
    "s_italic",
    "s_underline",
    "s_color",
    "s_font_rel",
    "s_fill",
    "s_border_top",
    "s_border_bottom",
    "s_border_lr",
    "s_indent",
    "s_align",
    "s_wrap",
    "s_numfmt",
];

/// Positions in [`STYLE_FEATURES`] of the on/off features given as the cell's value minus the
/// sheet's mean (bold, italic, underline, colour, fill, borders, wrap).
const RELATIVE: [usize; 9] = [1, 2, 3, 4, 6, 7, 8, 9, 12];

/// `s_*`: the cell's formatting ([`crate::model::Format`]). `s_known` is 0 and every other
/// feature 0 when the file format carries no styles hedgehog reads (xls, xlsb, ods, csv).
/// `s_font_rel` is the font size over the sheet's most common size; `s_align` 0 general, 1 left,
/// 2 center, 3 right, 4 other; `s_numfmt` the [`crate::model::NumFmt`] in declaration order
/// (0 general … 8 other); `s_border_lr` counts the left and right borders.
fn add_style_features(grid: &Grid, f: &mut Features) {
    use crate::model::{Align, NumFmt};
    let sheet = grid.sheet;
    let n = sheet.cells.len();
    let mut cols: Vec<Vec<f32>> = (0..STYLE_FEATURES.len()).map(|_| vec![0f32; n]).collect();
    if sheet.formats.len() == n {
        let mut sizes = [0u32; 512];
        for fm in &sheet.formats {
            if let Some(s) = fm.unpack() {
                sizes[s.size_half_pt as usize] += 1;
            }
        }
        let mode = (1..sizes.len()).max_by_key(|&k| (sizes[k], std::cmp::Reverse(k))).filter(|&k| sizes[k] > 0);
        for (i, fm) in sheet.formats.iter().enumerate() {
            let Some(s) = fm.unpack() else { continue };
            let align = match s.align {
                Align::General => 0.0,
                Align::Left => 1.0,
                Align::Center => 2.0,
                Align::Right => 3.0,
                Align::Other => 4.0,
            };
            let numfmt = match s.num_fmt {
                NumFmt::General => 0.0,
                NumFmt::Integer => 1.0,
                NumFmt::Decimal => 2.0,
                NumFmt::Thousands => 3.0,
                NumFmt::Percent => 4.0,
                NumFmt::Currency => 5.0,
                NumFmt::Date => 6.0,
                NumFmt::Text => 7.0,
                NumFmt::Other => 8.0,
            };
            let font_rel = match mode {
                Some(m) if s.size_half_pt > 0 => s.size_half_pt as f32 / m as f32,
                _ => 1.0,
            };
            let row = [
                1.0,
                s.bold as u8 as f32,
                s.italic as u8 as f32,
                s.underline as u8 as f32,
                s.color as u8 as f32,
                font_rel,
                s.fill as u8 as f32,
                s.border_top as u8 as f32,
                s.border_bottom as u8 as f32,
                (s.border_left as u8 + s.border_right as u8) as f32,
                s.indent as f32,
                align,
                s.wrap as u8 as f32,
                numfmt,
            ];
            for (c, v) in cols.iter_mut().zip(row) {
                c[i] = v;
            }
        }
        // On/off formatting counts relative to the sheet: bold in a sheet where everything is
        // bold, or borders on every cell, says nothing about the cell.
        for k in RELATIVE {
            let mean = cols[k].iter().sum::<f32>() / n.max(1) as f32;
            cols[k].iter_mut().for_each(|x| *x -= mean);
        }
    }
    for (name, col) in STYLE_FEATURES.iter().zip(cols) {
        f.add(name, col);
    }
}

fn quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Writes the CSV header (when `header`) and one row per kept labelled cell of every file in the
/// split. Files that fail to open are counted and skipped.
pub fn export(corpus: &Corpus, opts: &ExportOptions, out: &mut impl Write, header: bool) -> std::io::Result<ExportStats> {
    let mut stats = ExportStats::default();
    let mut names: Option<Vec<&'static str>> = None;
    let keep_below = (opts.data_keep.clamp(0.0, 1.0) as f64 * u64::MAX as f64) as u64;
    let weight = if opts.data_keep > 0.0 { 1.0 / opts.data_keep.min(1.0) } else { 0.0 };
    let files: Vec<&GtFile> =
        corpus.files.iter().filter(|f| opts.split.contains(&f.id)).take(opts.limit.unwrap_or(usize::MAX)).collect();
    let mut wrote_header = !header;
    for (k, file) in files.iter().enumerate() {
        if opts.progress && k % 50 == 0 {
            eprintln!("  {}/{} files, {} rows", k, files.len(), stats.rows);
        }
        let wb = std::iter::once(&file.path).chain(&file.fallback_paths).find_map(|p| crate::reader::open_with(p, crate::reader::ReadOptions { formats: true, ..Default::default() }).ok());
        let Some(wb) = wb else {
            stats.failed += 1;
            continue;
        };
        stats.files += 1;
        let split = super::split::split_of(&file.id).as_str();
        for gs in &file.sheets {
            let Some((si, _)) = super::run::find_sheet(&wb, &gs.name, gs.index) else { continue };
            let sheet = &wb.sheets[si];
            let grid = Grid::new(sheet);
            let f = sheet_features(&grid);
            let cols: Vec<&[f32]> = (0..f.names().len()).map(|i| f.column(i)).collect();
            if !wrote_header {
                let mut line = String::from("source,file,sheet,row,col,split,label,weight");
                for n in f.names() {
                    line.push(',');
                    line.push_str(n);
                }
                writeln!(out, "{line}")?;
                wrote_header = true;
            }
            match &names {
                None => names = Some(f.names().to_vec()),
                Some(n) => assert_eq!(n.as_slice(), f.names(), "feature columns differ between sheets"),
            }
            stats.sheets += 1;
            let prefix = format!("{},{},{}", quote(&opts.source), quote(&file.id), quote(&gs.name));
            let mut line = String::new();
            for gc in &gs.cells {
                let idx = grid.cell_index(gc.row, gc.col).or_else(|| {
                    let m = grid.merge_at(gc.row, gc.col)?;
                    grid.cell_index(m.top, m.left)
                });
                let Some(i) = idx else { continue };
                let w = if gc.role == Role::Data {
                    let h = fnv1a(&format!("{}\u{1f}{}\u{1f}{}\u{1f}{}", file.id, gs.name, gc.row, gc.col));
                    if h > keep_below {
                        continue;
                    }
                    weight
                } else {
                    1.0
                };
                line.clear();
                use std::fmt::Write as _;
                let _ = write!(line, "{prefix},{},{},{split},{},{w}", gc.row, gc.col, role_id(gc.role));
                for c in &cols {
                    let _ = write!(line, ",{}", c[i]);
                }
                writeln!(out, "{line}")?;
                stats.rows += 1;
                stats.per_role[role_id(gc.role) as usize] += 1;
            }
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DetectedTable;
    use crate::model::{Cell, Rect, Value};

    #[test]
    fn in_tables_matches_testing_every_table() {
        let t = |top, left, bottom, right| DetectedTable {
            rect: Rect { top, left, bottom, right },
            header_top: top,
            header_rows: 1,
            confidence: 1.0,
        };
        let tables = vec![t(5, 0, 9, 3), t(0, 2, 20, 2), t(12, 1, 14, 6), t(30, 0, 30, 0), t(8, 5, 12, 9)];
        let mut cells = Vec::new();
        for r in 0..35u32 {
            for c in 0..11u32 {
                if (r * 7 + c * 3) % 4 != 0 {
                    cells.push(Cell { row: r, col: c, value: Some(Value::Number(1.0)), formula: None });
                }
            }
        }
        let brute = |cells: &[Cell]| -> Vec<bool> {
            cells
                .iter()
                .map(|c| {
                    tables.iter().any(|t| {
                        t.rect.top <= c.row && c.row <= t.rect.bottom && t.rect.left <= c.col && c.col <= t.rect.right
                    })
                })
                .collect()
        };
        assert_eq!(in_tables(&cells, &tables), brute(&cells));
        cells.reverse(); // not sorted by row
        assert_eq!(in_tables(&cells, &tables), brute(&cells));
        assert!(in_tables(&cells, &[]).iter().all(|&x| !x));
    }
}
