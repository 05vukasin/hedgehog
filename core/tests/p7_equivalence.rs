//! P7 equivalence on real files: the streamed model input (`learned::equivalence`) against the
//! materialized feature matrix, sheet by sheet — every segment feature bit for bit, every role,
//! every class score of the lockstep tree walk against the plain walk, and the final layout —
//! with the rows walked as 1 and as 4 chunks.
//!
//! `HH_P7_FILES=<file with one path per line> cargo test --release -p hh_core --test p7_equivalence -- --ignored --nocapture`
//! (default: `core/benches/enron50.txt`, relative to `$HEDGEHOG_DATA`). Sheets over the model's
//! default cell limit (or `HH_P7_MAX_CELLS`) are skipped. `HH_P7_SKIP=<n>` resumes after the
//! first `n` files. One progress line per file goes to stderr.

use std::path::PathBuf;

use hh_core::eval::{bench, data_root};
use hh_core::grid::Grid;
use hh_core::{Cell, Rect, Sheet, Value};

#[test]
fn streamed_dense_and_split_rows_match_materialized_features() {
    let mut cells = Vec::new();
    for row in [0, 1, 2, 4, 5, 6, 9] {
        // Short gaps stay in one segment; wide gaps split the row into separate segments.
        let cols: &[u32] = if row == 4 { &[0, 1, 2, 20, 21, 22] } else { &[0, 1, 2, 3, 4, 5] };
        for (k, &col) in cols.iter().enumerate() {
            let value = match (row, k) {
                (0, _) => Some(Value::Text(format!("Header {k}"))),
                (_, 0) => Some(Value::Text(format!("Item {row}"))),
                (_, 1) => Some(Value::Number(-0.0)),
                (_, 2) => Some(Value::Text("1,234.50".into())),
                (_, 3) => Some(Value::DateTime(45000.0 + row as f64)),
                (_, 4) => None,
                _ => Some(Value::Number(row as f64 / 7.0)),
            };
            cells.push(Cell { row, col, value, formula: (k == 4).then(|| "SUM(B2:C2)".into()) });
        }
    }
    let sheet = Sheet {
        name: "segments".into(), cells,
        merges: vec![Rect { top: 11, left: 0, bottom: 11, right: 5 }],
        formats: vec![],
    };
    let model = hh_core::learned::embedded().expect("embedded model");
    let segments = hh_core::learned::equivalence(&Grid::new(&sheet), model, &[1, 2, 4]).unwrap();
    assert!(segments > 7, "wide row must produce multiple segments");
    let empty = Sheet { name: "empty".into(), cells: vec![], merges: sheet.merges, formats: vec![] };
    assert_eq!(hh_core::learned::equivalence(&Grid::new(&empty), model, &[1, 4]).unwrap(), 0);
}

#[test]
#[ignore]
fn streamed_model_input_matches_the_materialized_one() {
    let env = |k: &str| std::env::var(k).ok();
    let list = env("HH_P7_FILES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches").join("enron50.txt"));
    let files = bench::resolve_inputs(&list).unwrap();
    let model = hh_core::learned::embedded().expect("embedded model");
    let limit = env("HH_P7_MAX_CELLS").and_then(|v| v.parse().ok()).unwrap_or(hh_core::normalize::DEFAULT_MODEL_MAX_CELLS);
    let skip: usize = env("HH_P7_SKIP").and_then(|v| v.parse().ok()).unwrap_or(0);
    let root = data_root();
    let (mut sheets, mut segments, mut skipped, mut unreadable) = (0usize, 0usize, 0usize, 0usize);
    let mut bad = Vec::new();
    for (i, path) in files.iter().enumerate().skip(skip) {
        let name = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        let Ok(wb) = hh_core::open(path) else {
            unreadable += 1;
            eprintln!("[{i}] {name}: unreadable");
            continue;
        };
        let before = (sheets, segments, bad.len());
        for sheet in &wb.sheets {
            if sheet.cells.is_empty() || sheet.cells.len() > limit {
                skipped += 1;
                continue;
            }
            let grid = Grid::new(sheet);
            match hh_core::learned::equivalence(&grid, model, &[1, 4]) {
                Ok(n) => segments += n,
                Err(e) => {
                    eprintln!("    DIFF {name} / {}: {e}", sheet.name);
                    bad.push(format!("{name} / {}: {e}", sheet.name));
                }
            }
            sheets += 1;
        }
        eprintln!(
            "[{i}] {name}: {} sheets, {} segments, {} differences",
            sheets - before.0,
            segments - before.1,
            bad.len() - before.2
        );
    }
    eprintln!(
        "p7 equivalence: {} files ({unreadable} unreadable), {sheets} sheets, {segments} segments, {skipped} sheets skipped, {} differences",
        files.len() - skip.min(files.len()),
        bad.len()
    );
    assert!(bad.is_empty(), "{} differences:\n{}", bad.len(), bad.join("\n"));
}
