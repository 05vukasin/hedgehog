//! Pathological inputs must stay cheap: no structure may grow with the area of the used range or
//! of a merged area, and nothing may be quadratic in rows. Sizes are kept small enough for debug
//! builds; `hedgehog bench --synthetic` runs the full-size versions.

use std::time::{Duration, Instant};

use hh_core::eval::bench::{run_sheet, synth};

fn limit() -> Duration {
    Duration::from_secs(if cfg!(debug_assertions) { 60 } else { 5 })
}

#[test]
fn synthetic_worst_cases_finish() {
    for (name, sheet) in [
        ("giant_merge", synth::giant_merge()),
        ("giant_merge_sparse", synth::giant_merge_sparse()),
        ("sparse_huge", synth::sparse_huge(2_000)),
        ("wide", synth::wide()),
        ("tall", synth::tall(50_000)),
        ("many_merges", synth::many_merges(2_000)),
        ("formula_heavy", synth::formula_heavy(5_000)),
    ] {
        let t = Instant::now();
        run_sheet(&sheet, true);
        assert!(t.elapsed() < limit(), "{name} took {:?}", t.elapsed());
    }
}

/// A sheet using A1 and XFD1048576 once read as two dense rows × cols grids (17G entries).
#[test]
fn reader_handles_opposite_corners() {
    let path = std::env::temp_dir().join(format!("hh_test_{}_corners.xlsx", std::process::id()));
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "top left").unwrap();
    ws.write_number(1_048_575, 16_383, 1.0).unwrap();
    ws.write_formula(1_048_575, 16_382, "=SUM(A:A)").unwrap();
    wb.save(&path).unwrap();
    let t = Instant::now();
    let book = hh_core::open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert!(t.elapsed() < limit());
    let s = &book.sheets[0];
    assert_eq!(s.cells.len(), 3);
    assert_eq!((s.cells[2].row, s.cells[2].col), (1_048_575, 16_383));
    assert_eq!(s.cells[1].formula.as_deref(), Some("SUM(A:A)"));
    let t = Instant::now();
    run_sheet(s, true);
    assert!(t.elapsed() < limit());
}
