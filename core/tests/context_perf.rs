//! Timing of `context::compute` on a generated 100k-cell sheet.
//! Run in release for the real number:
//! `cargo test --release -p hh_core --test context_perf -- --nocapture`

use std::time::Instant;

use hh_core::context;
use hh_core::{Cell, Features, Grid, Rect, Sheet, Value};

/// ~100k cells: 4 tables side by side / stacked, each with a merged title, a header row,
/// text label column, numeric body, sparse holes and a total row.
fn big_sheet() -> Sheet {
    let mut cells = Vec::new();
    let mut merges = Vec::new();
    let mut seed: u64 = 42;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as u32
    };
    // Two bands of tables, each band two tables wide: 4 × ~2550 rows × ~10 cols ≈ 100k.
    for band in 0..2u32 {
        let top = band * 2600;
        for t in 0..2u32 {
            let left = t * 12;
            merges.push(Rect { top, left, bottom: top, right: left + 9 });
            cells.push((top, left, Some(Value::Text(format!("Table {band}{t}"))), None));
            for c in 0..10 {
                cells.push((top + 2, left + c, Some(Value::Text(format!("H{c}"))), None));
            }
            for r in 0..2550 {
                let row = top + 3 + r;
                cells.push((row, left, Some(Value::Text(format!("item {}", rnd() % 300))), None));
                cells.push((row, left + 1, Some(Value::Number(r as f64)), None));
                for c in 2..10 {
                    if rnd() % 50 != 0 {
                        cells.push((row, left + c, Some(Value::Number((rnd() % 10_000) as f64 / 7.0)), None));
                    }
                }
            }
            let row = top + 3 + 2550;
            cells.push((row, left, Some(Value::Text("Total".into())), None));
            for c in 2..10 {
                cells.push((row, left + c, Some(Value::Number(1.0)), Some("SUM(A1:A9)".to_string())));
            }
        }
    }
    cells.sort_by_key(|c| (c.0, c.1));
    let cells = cells.into_iter().map(|(row, col, value, formula)| Cell { row, col, value, formula }).collect();
    Sheet { name: "big".into(), cells, merges }
}

#[test]
fn hundred_k_cells_fast() {
    let sheet = big_sheet();
    let n = sheet.cells.len();
    assert!(n >= 100_000, "only {n} cells");

    let t0 = Instant::now();
    let grid = Grid::new(&sheet);
    let t_grid = t0.elapsed();
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut f = Features::new(n);
        let t = Instant::now();
        context::compute(&grid, &mut f);
        best = best.min(t.elapsed().as_secs_f64());
        assert_eq!(f.value("c_sheet_components", 0), Some(8.0)); // 4 tables + 4 titles
    }
    let comps = context::components(&grid);
    assert_eq!(comps.len(), 8);
    println!(
        "context perf: {n} cells, Grid::new {:.1} ms, compute (best of 3) {:.1} ms, {} features",
        t_grid.as_secs_f64() * 1e3,
        best * 1e3,
        context::FEATURES.len()
    );
    let limit = if cfg!(debug_assertions) { 15.0 } else { 1.0 };
    assert!(best < limit, "compute took {best:.3}s");
}
