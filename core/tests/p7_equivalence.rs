//! P7 equivalence on real files: the streamed model input (`learned::equivalence`) against the
//! materialized feature matrix, sheet by sheet — every segment feature bit for bit, every role,
//! every class score of the lockstep tree walk against the plain walk, and the final layout —
//! with the rows split over 1 and 4 threads.
//!
//! `HH_P7_FILES=<file with one path per line> cargo test --release -p hh_core --test p7_equivalence -- --ignored --nocapture`
//! (default: `core/benches/enron50.txt`, relative to `$HEDGEHOG_DATA`). Sheets over the model's
//! default cell limit are skipped, as `normalize` skips the model there.

use std::path::PathBuf;

use hh_core::eval::{bench, data_root};
use hh_core::grid::Grid;

#[test]
#[ignore]
fn streamed_model_input_matches_the_materialized_one() {
    let list = std::env::var_os("HH_P7_FILES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches").join("enron50.txt"));
    let files = bench::resolve_inputs(&list).unwrap();
    let model = hh_core::learned::embedded().expect("embedded model");
    let limit = hh_core::normalize::DEFAULT_MODEL_MAX_CELLS;
    let root = data_root();
    let (mut sheets, mut segments, mut skipped, mut unreadable) = (0usize, 0usize, 0usize, 0usize);
    let mut bad = Vec::new();
    for path in &files {
        let Ok(wb) = hh_core::open(path) else {
            unreadable += 1;
            continue;
        };
        for sheet in &wb.sheets {
            if sheet.cells.is_empty() || sheet.cells.len() > limit {
                skipped += 1;
                continue;
            }
            let grid = Grid::new(sheet);
            for threads in [1, 4] {
                match hh_core::learned::equivalence(&grid, model, threads) {
                    Ok(n) => segments += n,
                    Err(e) => bad.push(format!(
                        "{} / {} ({threads} threads): {e}",
                        path.strip_prefix(&root).unwrap_or(path).display(),
                        sheet.name
                    )),
                }
            }
            sheets += 1;
        }
    }
    eprintln!(
        "p7 equivalence: {} files ({unreadable} unreadable), {sheets} sheets, {segments} segment checks, {skipped} sheets skipped, {} differences",
        files.len(),
        bad.len()
    );
    assert!(bad.is_empty(), "{} differences:\n{}", bad.len(), bad.join("\n"));
}
