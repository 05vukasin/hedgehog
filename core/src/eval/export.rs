//! Training data for the learned cell-role model (PLAN.md M6): one CSV row per labelled cell with
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
    finish(grid, lay, &cands, &mut f);
    f
}

/// Sheets with at least this many cells compute their feature groups on parallel threads.
const PARALLEL_MIN_CELLS: usize = 20_000;

/// The rule layout and [`sheet_features`] of a sheet, the independent parts (layout, context,
/// values, totals) on parallel threads for larger sheets. Same result as the sequential path.
pub fn layout_and_features(grid: &Grid) -> (crate::analysis::SheetLayout, Features) {
    let n = grid.sheet.cells.len();
    if n < PARALLEL_MIN_CELLS {
        let lay = crate::layout::analyze_for_model(grid);
        let f = sheet_features_with(grid, &lay);
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
    finish(grid, &lay, &cands, &mut f);
    (lay, f)
}

/// A scoped thread's result, re-raising its panic.
fn join<T>(r: std::thread::Result<T>) -> T {
    r.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// The `l_*` (rule layout) and `x_*` (detected totals) columns, after the core features.
fn finish(grid: &Grid, lay: &crate::analysis::SheetLayout, cands: &[crate::totals::TotalCandidate], f: &mut Features) {
    let cells = &grid.sheet.cells;
    let role_at = |r: i64, c: i64| -> f32 {
        if r < 0 || c < 0 {
            return -1.0;
        }
        grid.cell_index(r as u32, c as u32).map_or(-1.0, |i| role_id(lay.roles[i]) as f32)
    };
    let mut cols: [Vec<f32>; 6] = Default::default();
    for (i, c) in cells.iter().enumerate() {
        let (r, k) = (c.row as i64, c.col as i64);
        cols[0].push(role_id(lay.roles[i]) as f32);
        cols[1].push(role_at(r - 1, k));
        cols[2].push(role_at(r + 1, k));
        cols[3].push(role_at(r, k - 1));
        cols[4].push(role_at(r, k + 1));
        let in_table = lay.tables.iter().any(|t| {
            t.rect.top <= c.row && c.row <= t.rect.bottom && t.rect.left <= c.col && c.col <= t.rect.right
        });
        cols[5].push(in_table as u8 as f32);
    }
    let [a, b, d, e, g, h] = cols;
    f.add("l_role", a);
    f.add("l_role_up", b);
    f.add("l_role_down", d);
    f.add("l_role_left", e);
    f.add("l_role_right", g);
    f.add("l_in_table", h);
    add_total_context(grid, cands, f);
}

/// `x_*`: detected totals around the cell. DECO labels a whole total row (its label and every
/// number), so whether the row holds detected totals matters as much as the cell itself.
fn add_total_context(grid: &Grid, cands: &[crate::totals::TotalCandidate], f: &mut Features) {
    use crate::totals::{Direction, TotalLevel};
    use rustc_hash::FxHashMap;
    let cells = &grid.sheet.cells;
    let n = cells.len();
    let mut cand = vec![0f32; n];
    let mut level = vec![0f32; n];
    let mut row_v: FxHashMap<u32, u32> = FxHashMap::default();
    let mut col_h: FxHashMap<u32, u32> = FxHashMap::default();
    for c in cands {
        let Some(i) = grid.cell_index(c.row, c.col) else { continue };
        let vertical = c.direction == Direction::Vertical;
        cand[i] = if vertical { 1.0 } else { 2.0 };
        level[i] = match c.level {
            TotalLevel::Total => 1.0,
            TotalLevel::Subtotal => 2.0,
            TotalLevel::GrandTotal => 3.0,
        };
        if vertical {
            *row_v.entry(c.row).or_default() += 1;
        } else {
            *col_h.entry(c.col).or_default() += 1;
        }
    }
    let numeric = |idx: &[u32]| {
        idx.iter().filter(|&&i| matches!(cells[i as usize].value, Some(crate::model::Value::Number(_)))).count().max(1)
    };
    let mut row_frac_memo: FxHashMap<u32, f32> = FxHashMap::default();
    let mut col_frac_memo: FxHashMap<u32, f32> = FxHashMap::default();
    let (mut rv, mut rf, mut cf) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for c in cells {
        let v = row_v.get(&c.row).copied().unwrap_or(0);
        rv.push(v as f32);
        rf.push(*row_frac_memo.entry(c.row).or_insert_with(|| v as f32 / numeric(grid.row_cells(c.row)) as f32));
        let h = col_h.get(&c.col).copied().unwrap_or(0);
        cf.push(*col_frac_memo.entry(c.col).or_insert_with(|| h as f32 / numeric(grid.col_cells(c.col)) as f32));
    }
    f.add("x_total_cand", cand);
    f.add("x_total_level", level);
    f.add("x_row_vtotals", rv);
    f.add("x_row_vtotal_frac", rf);
    f.add("x_col_htotal_frac", cf);
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
        let wb = std::iter::once(&file.path).chain(&file.fallback_paths).find_map(|p| crate::reader::open(p).ok());
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
