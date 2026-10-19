//! Corpus check on real Enron spreadsheets (untrusted; read only through `hh_core::open`).
//! Ignored by default; run with
//! `cargo test --release -p hh_core --test context_corpus -- --ignored --nocapture`
//! Env: `HEDGEHOG_DATA` (default `../testdata` next to the repo), `HH_CORPUS_N` (default 500).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::Instant;

use hh_core::context;
use hh_core::{open, Features, Grid};

fn data_dir() -> PathBuf {
    match std::env::var("HEDGEHOG_DATA") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata"),
    }
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore]
fn enron_sample() {
    let dir = data_dir().join("irregular/enron");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "xlsx"))
        .collect();
    files.sort();
    // Deterministic shuffle.
    let mut seed: u64 = 7;
    for i in (1..files.len()).rev() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        files.swap(i, (seed >> 33) as usize % (i + 1));
    }
    let want: usize = std::env::var("HH_CORPUS_N").ok().and_then(|s| s.parse().ok()).unwrap_or(500);
    files.truncate(want);

    let (mut opened, mut open_errors, mut panics) = (0, 0, Vec::new());
    let mut file_ms: Vec<f64> = Vec::new();
    let mut comps_per_sheet: Vec<f64> = Vec::new();
    let (mut sheets, mut nonempty_sheets, mut multi_comp) = (0usize, 0usize, 0usize);
    let (mut total_cells, mut total_comps, mut singleton_comps, mut comps_ge4) = (0usize, 0usize, 0usize, 0usize);
    let (mut block_rows, mut single_block_rows) = (0usize, 0usize);
    let (mut merged_cells, mut text_over_num, mut row_label) = (0usize, 0usize, 0usize);
    let mut slowest = (0.0f64, String::new());

    for path in &files {
        let wb = match open(path) {
            Ok(wb) => wb,
            Err(_) => {
                open_errors += 1;
                continue;
            }
        };
        opened += 1;
        let t = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut per_sheet = Vec::new();
            for sheet in &wb.sheets {
                let grid = Grid::new(sheet);
                let mut f = Features::new(sheet.cells.len());
                context::compute(&grid, &mut f);
                let comps = context::components(&grid);
                per_sheet.push((sheet.cells.len(), f, comps));
            }
            per_sheet
        }));
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let per_sheet = match result {
            Ok(x) => x,
            Err(_) => {
                panics.push(path.display().to_string());
                continue;
            }
        };
        file_ms.push(ms);
        if ms > slowest.0 {
            let cells: usize = wb.sheets.iter().map(|s| s.cells.len()).sum();
            slowest = (ms, format!("{} ({cells} cells)", path.file_name().unwrap().to_string_lossy()));
        }
        for (n, f, comps) in per_sheet {
            sheets += 1;
            if n == 0 {
                continue;
            }
            nonempty_sheets += 1;
            total_cells += n;
            comps_per_sheet.push(comps.len() as f64);
            multi_comp += (comps.len() > 1) as usize;
            total_comps += comps.len();
            singleton_comps += comps.iter().filter(|c| c.cells.len() == 1).count();
            comps_ge4 += comps.iter().filter(|c| c.cells.len() >= 4).count();
            let first = f.get("c_row_first").unwrap();
            let single = f.get("c_row_single").unwrap();
            for i in 0..n {
                if first[i] == 1.0 {
                    block_rows += 1;
                    single_block_rows += (single[i] == 1.0) as usize;
                }
            }
            merged_cells += f.get("c_in_merge").unwrap().iter().filter(|&&v| v == 1.0).count();
            text_over_num += f.get("c_text_over_numbers").unwrap().iter().filter(|&&v| v >= 0.8).count();
            row_label += f.get("c_row_label").unwrap().iter().filter(|&&v| v == 1.0).count();
        }
    }

    file_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    comps_per_sheet.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let share = |a: usize, b: usize| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
    println!("== context corpus check: {} files sampled from {}", files.len(), dir.display());
    println!("opened {opened}, open errors {open_errors}, panics {}", panics.len());
    println!(
        "compute time per file (all sheets): mean {:.2} ms, p50 {:.2}, p95 {:.2}, p99 {:.2}, max {:.1} ms [{}]",
        mean(&file_ms),
        pct(&file_ms, 0.5),
        pct(&file_ms, 0.95),
        pct(&file_ms, 0.99),
        slowest.0,
        slowest.1
    );
    println!("sheets {sheets}, non-empty {nonempty_sheets}, cells {total_cells}");
    println!(
        "components per non-empty sheet: mean {:.1}, p50 {}, p90 {}, max {}; sheets with >1 component {:.1}%",
        mean(&comps_per_sheet),
        pct(&comps_per_sheet, 0.5),
        pct(&comps_per_sheet, 0.9),
        comps_per_sheet.last().copied().unwrap_or(0.0),
        share(multi_comp, nonempty_sheets)
    );
    println!(
        "components: {total_comps}, singletons {:.1}%, with >=4 cells {:.1}% (mean {:.1} per sheet)",
        share(singleton_comps, total_comps),
        share(comps_ge4, total_comps),
        comps_ge4 as f64 / nonempty_sheets.max(1) as f64
    );
    println!(
        "block rows: {block_rows}, single-cell {:.1}%; cells in merges {:.2}%; text-over-numbers cells {:.2}%; row-label cells {:.2}%",
        share(single_block_rows, block_rows),
        share(merged_cells, total_cells),
        share(text_over_num, total_cells),
        share(row_label, total_cells)
    );
    assert!(panics.is_empty(), "panicked on: {panics:?}");
}
