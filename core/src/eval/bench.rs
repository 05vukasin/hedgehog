//! Performance harness shared by `hedgehog bench`, `hedgehog golden`, the criterion benches in
//! `core/benches/` and the ignored golden test: runs every stage of the pipeline on a workbook,
//! times each stage, and (optionally) hashes every output so optimizations can be checked to
//! change nothing.
//!
//! Stages, in order: `open` (reader), `grid` ([`Grid::new`]), `context` ([`crate::context::compute`]),
//! `values` ([`crate::values::compute`]), `totals` ([`crate::totals::compute`], the `t_*` features),
//! `detect_verify` ([`crate::totals::detect`] + [`crate::totals::verify`] of every candidate) and
//! `baseline` ([`crate::eval::baseline::analyze`]).

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::features::Features;
use crate::grid::Grid;
use crate::model::{Rect, Sheet, Value, Workbook};

pub const STAGES: [&str; 7] = ["open", "grid", "context", "values", "totals", "detect_verify", "baseline"];

/// Seconds spent per stage (index = position in [`STAGES`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct StageTimes(pub [f64; STAGES.len()]);

impl StageTimes {
    pub fn add(&mut self, o: &StageTimes) {
        for (a, b) in self.0.iter_mut().zip(o.0) {
            *a += b;
        }
    }

    pub fn total(&self) -> f64 {
        self.0.iter().sum()
    }
}

/// FNV-1a, 64 bit: tiny, deterministic across runs, platforms and Rust versions.
#[derive(Debug, Clone, Copy)]
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub fn u64(&mut self, x: u64) {
        self.bytes(&x.to_le_bytes());
    }

    pub fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }
}

/// Result of running the whole pipeline on one file.
#[derive(Debug, Clone, Default)]
pub struct FileRun {
    pub sheets: usize,
    pub cells: usize,
    pub merges: usize,
    pub formulas: usize,
    pub times: StageTimes,
    /// Hash of every output (when requested), else 0.
    pub hash: u64,
}

/// Opens `path` and runs every stage on every sheet.
pub fn run_file(path: &Path, golden: bool) -> Result<FileRun, crate::reader::Error> {
    let t = Instant::now();
    let wb = crate::reader::open(path)?;
    let open = t.elapsed().as_secs_f64();
    let mut run = run_workbook(&wb, golden);
    run.times.0[0] = open;
    Ok(run)
}

/// Runs every stage after `open` on every sheet of `wb`.
pub fn run_workbook(wb: &Workbook, golden: bool) -> FileRun {
    let mut run = FileRun { sheets: wb.sheets.len(), ..Default::default() };
    let mut h = Fnv::default();
    for sheet in &wb.sheets {
        run.cells += sheet.cells.len();
        run.merges += sheet.merges.len();
        run.formulas += sheet.formula_count();
        let (t, sh) = run_sheet(sheet, golden);
        run.times.add(&t);
        if golden {
            h.u64(sh);
        }
    }
    run.hash = if golden { h.0 } else { 0 };
    run
}

/// Runs every stage after `open` on one sheet; returns stage times and the output hash (0 unless
/// `golden`).
pub fn run_sheet(sheet: &Sheet, golden: bool) -> (StageTimes, u64) {
    let mut t = StageTimes::default();
    let mut h = Fnv::default();
    if golden {
        hash_sheet(&mut h, sheet);
    }

    let s = Instant::now();
    let grid = Grid::new(sheet);
    t.0[1] = s.elapsed().as_secs_f64();

    let n = sheet.cells.len();
    let mut f = Features::new(n);
    let s = Instant::now();
    crate::context::compute(&grid, &mut f);
    t.0[2] = s.elapsed().as_secs_f64();

    let s = Instant::now();
    crate::values::compute(&grid, &mut f);
    t.0[3] = s.elapsed().as_secs_f64();

    let s = Instant::now();
    crate::totals::compute(&grid, &mut f);
    t.0[4] = s.elapsed().as_secs_f64();

    let s = Instant::now();
    let cands = crate::totals::detect(&grid);
    let checks: Vec<_> = cands.iter().map(|c| crate::totals::verify(&grid, c)).collect();
    t.0[5] = s.elapsed().as_secs_f64();

    let s = Instant::now();
    let layout = crate::eval::baseline::analyze(&grid);
    t.0[6] = s.elapsed().as_secs_f64();

    if golden {
        for name in f.names() {
            h.str(name);
            for v in f.get(name).unwrap_or(&[]) {
                h.u64(v.to_bits() as u64);
            }
        }
        h.str(&format!("{cands:?}"));
        h.str(&format!("{checks:?}"));
        h.str(&format!("{layout:?}"));
    }
    (t, if golden { h.0 } else { 0 })
}

fn hash_sheet(h: &mut Fnv, sheet: &Sheet) {
    h.str(&sheet.name);
    h.u64(sheet.cells.len() as u64);
    for c in &sheet.cells {
        h.u64(((c.row as u64) << 32) | c.col as u64);
        match &c.value {
            None => h.u64(0),
            Some(Value::Bool(b)) => h.u64(1 + *b as u64 * 16),
            Some(Value::Number(x)) => {
                h.u64(2);
                h.u64(x.to_bits());
            }
            Some(Value::DateTime(x)) => {
                h.u64(3);
                h.u64(x.to_bits());
            }
            Some(Value::Text(s)) => {
                h.u64(4);
                h.str(s);
            }
            Some(Value::Error(s)) => {
                h.u64(5);
                h.str(s);
            }
        }
        match &c.formula {
            None => h.u64(0),
            Some(f) => h.str(f),
        }
    }
    h.u64(sheet.merges.len() as u64);
    for m in &sheet.merges {
        h.u64(((m.top as u64) << 32) | m.left as u64);
        h.u64(((m.bottom as u64) << 32) | m.right as u64);
    }
}

/// Peak resident set size of this process in bytes (`VmHWM`), Linux only.
pub fn peak_rss() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Spreadsheet files to benchmark: every `.xlsx/.xlsm/.xlsb/.xls/.ods` in a directory (sorted),
/// or the lines of a list file (blank lines and `#` comments skipped; relative paths are resolved
/// against [`crate::eval::data_root`]).
pub fn resolve_inputs(arg: &Path) -> std::io::Result<Vec<PathBuf>> {
    if arg.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(arg)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                    matches!(x.to_ascii_lowercase().as_str(), "xlsx" | "xlsm" | "xlsb" | "xls" | "ods")
                })
            })
            .collect();
        v.sort();
        return Ok(v);
    }
    let root = crate::eval::data_root();
    Ok(std::fs::read_to_string(arg)?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let p = PathBuf::from(l);
            if p.is_absolute() { p } else { root.join(p) }
        })
        .collect())
}

/// Generated sheets for benchmarks and worst-case checks. All deterministic.
pub mod synth {
    use super::*;
    use crate::model::Cell;

    fn rng(mut seed: u64) -> impl FnMut() -> u32 {
        move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as u32
        }
    }

    fn sheet(name: &str, mut cells: Vec<Cell>, merges: Vec<Rect>) -> Sheet {
        cells.sort_by_key(|c| (c.row, c.col));
        Sheet { name: name.into(), cells, merges }
    }

    fn num(row: u32, col: u32, v: f64) -> Cell {
        Cell { row, col, value: Some(Value::Number(v)), formula: None }
    }

    fn text(row: u32, col: u32, s: String) -> Cell {
        Cell { row, col, value: Some(Value::Text(s)), formula: None }
    }

    /// ~`n` cells in stacked / side-by-side tables (merged titles, headers, label column,
    /// numeric body with holes, SUM total rows).
    pub fn dense(n: usize) -> Sheet {
        let mut r = rng(42);
        let mut cells = Vec::with_capacity(n + 1000);
        let mut merges = Vec::new();
        let rows_per_table = (n / 40).max(4) as u32; // 4 tables × ~10 cells per row
        for band in 0..2u32 {
            let top = band * (rows_per_table + 50);
            for t in 0..2u32 {
                let left = t * 12;
                merges.push(Rect { top, left, bottom: top, right: left + 9 });
                cells.push(text(top, left, format!("Table {band}{t}")));
                for c in 0..10 {
                    cells.push(text(top + 2, left + c, format!("Q{c} 2024")));
                }
                for i in 0..rows_per_table {
                    let row = top + 3 + i;
                    cells.push(text(row, left, format!("item {}", r() % 300)));
                    cells.push(num(row, left + 1, i as f64));
                    for c in 2..10 {
                        if r() % 50 != 0 {
                            cells.push(num(row, left + c, (r() % 10_000) as f64 / 7.0));
                        }
                    }
                }
                let row = top + 3 + rows_per_table;
                cells.push(text(row, left, "Total".into()));
                for c in 2..10 {
                    let col = crate::model::a1(0, left + c);
                    let col = col.trim_end_matches('1');
                    cells.push(Cell {
                        row,
                        col: left + c,
                        value: Some(Value::Number(1.0)),
                        formula: Some(format!("SUM({col}{}:{col}{})", top + 4, row)),
                    });
                }
            }
        }
        sheet("dense", cells, merges)
    }

    /// `n` cells scattered over the whole 1,048,576 × 16,384 sheet (used range = everything).
    pub fn sparse_huge(n: usize) -> Sheet {
        let mut r = rng(7);
        let mut cells: Vec<Cell> = (0..n)
            .map(|i| {
                let (row, col) = (r() % 1_048_576, r() % 16_384);
                if i % 3 == 0 { text(row, col, format!("t{}", r() % 100)) } else { num(row, col, i as f64) }
            })
            .collect();
        cells.push(num(0, 0, 1.0));
        cells.push(num(1_048_575, 16_383, 2.0));
        cells.sort_by_key(|c| (c.row, c.col));
        cells.dedup_by_key(|c| (c.row, c.col));
        sheet("sparse_huge", cells, Vec::new())
    }

    /// 1M rows in one column plus a total; tests row-count scaling.
    pub fn tall(rows: u32) -> Sheet {
        let mut cells: Vec<Cell> = (0..rows).map(|r| num(r, 0, r as f64)).collect();
        cells.push(Cell { row: rows, col: 0, value: Some(Value::Number(0.0)), formula: Some("SUM(A:A)".into()) });
        sheet("tall", cells, Vec::new())
    }

    /// One row across all 16,384 columns.
    pub fn wide() -> Sheet {
        let cells = (0..16_384).map(|c| num(0, c, c as f64)).collect();
        sheet("wide", cells, Vec::new())
    }

    /// A grid of `n` small merged headers (2×3 each) above numbers.
    pub fn many_merges(n: usize) -> Sheet {
        let mut cells = Vec::new();
        let mut merges = Vec::new();
        let per_row = 50u32;
        for i in 0..n as u32 {
            let (band, k) = (i / per_row, i % per_row);
            let (top, left) = (band * 4, k * 3);
            merges.push(Rect { top, left, bottom: top + 1, right: left + 2 });
            cells.push(text(top, left, format!("H{i}")));
            for c in 0..3 {
                cells.push(num(top + 2, left + c, (i * 3 + c) as f64));
            }
        }
        sheet("many_merges", cells, merges)
    }

    /// `n` formula cells: SUMs over columns, `+` chains, whole-column refs, links.
    pub fn formula_heavy(n: usize) -> Sheet {
        let mut cells = Vec::new();
        let cols = 20u32;
        let rows = (n as u32 / cols).max(2);
        for r in 0..rows {
            for c in 0..cols {
                cells.push(num(r, c, (r * cols + c) as f64));
            }
            // row total: SUM of the row, and an addition chain
            cells.push(Cell { row: r, col: cols, value: Some(Value::Number(0.0)), formula: Some(format!("SUM(A{0}:T{0})", r + 1)) });
            cells.push(Cell { row: r, col: cols + 1, value: Some(Value::Number(0.0)), formula: Some(format!("A{0}+B{0}+C{0}", r + 1)) });
        }
        for c in 0..cols {
            let col = crate::model::a1(0, c);
            let col = col.trim_end_matches('1');
            cells.push(Cell { row: rows, col: c, value: Some(Value::Number(0.0)), formula: Some(format!("SUM({col}1:{col}{rows})")) });
            cells.push(Cell { row: rows + 1, col: c, value: Some(Value::Number(0.0)), formula: Some(format!("SUM({col}:{col})")) });
        }
        // make the body formulas too: every other cell links to its left neighbour
        for cell in cells.iter_mut().filter(|c| c.row < rows && c.col > 0 && c.col < cols && (c.row + c.col) % 2 == 0) {
            let left = crate::model::a1(cell.row, cell.col - 1);
            cell.formula = Some(format!("{left}*1"));
        }
        sheet("formula_heavy", cells, Vec::new())
    }

    /// A few cells plus one merge covering the whole sheet (A1:XFD1048576) and a whole-column merge.
    pub fn giant_merge() -> Sheet {
        let cells = vec![text(0, 0, "title".into()), num(5, 5, 1.0), num(6, 5, 2.0)];
        let merges = vec![
            Rect { top: 0, left: 0, bottom: 1_048_575, right: 16_383 },
            Rect { top: 0, left: 3, bottom: 1_048_575, right: 3 },
        ];
        sheet("giant_merge", cells, merges)
    }

    /// Every generated case with a short name.
    pub fn all() -> Vec<(&'static str, Sheet)> {
        vec![
            ("dense_100k", dense(100_000)),
            ("sparse_huge_20k", sparse_huge(20_000)),
            ("many_merges_5k", many_merges(5_000)),
            ("formula_heavy_50k", formula_heavy(50_000)),
            ("tall_1m", tall(1_000_000)),
            ("wide_16k", wide()),
            ("giant_merge", giant_merge()),
        ]
    }
}
