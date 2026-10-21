//! Row labels shared by side-by-side and stacked grids (the Enron Henry Hub report): a summary
//! table names the rows once (flow, pipeline); a daily grid beside it and one per month below it
//! repeat those rows without labels. The grids get copies of the summary's label columns; a grid
//! with another row layout does not. A first data row of zeros under a header of dates is data,
//! not a repeat of that header.

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, ColRole, Normalized, Options, Out, Table};
use hh_core::open;
use rust_xlsxwriter::{Format, Formula, Workbook, Worksheet};

const PIPES: [&str; 4] = ["ACADIAN", "BRIDGELINE", "MAINLINE", "NGPL"];

/// Value of pipeline `p` (0-based; receipts first, then deliveries) on day `d` of grid `g`; the
/// first pipeline flows nothing (a row of zeros right under the header).
fn flow(g: u32, p: usize, d: u32) -> f64 {
    if p.is_multiple_of(PIPES.len()) {
        return 0.0;
    }
    let v = (1000 * (p as u32 + 1) + 37 * d + 501 * g) as f64;
    if p >= PIPES.len() { -v } else { v }
}

/// A grid of `days` date columns at (`top`, `left`): a header of dates, then receipts, a total
/// row, deliveries and a total row, the same rows as the summary.
fn grid(ws: &mut Worksheet, g: u32, top: u32, left: u16, days: u32, date: &Format) {
    for d in 0..days {
        let col = left + d as u16;
        ws.write_number_with_format(top, col, 37257.0 + 31.0 * g as f64 + d as f64, date).unwrap();
        let mut row = top + 1;
        for block in 0..2 {
            let first = row;
            for k in 0..PIPES.len() {
                ws.write_number(row, col, flow(g, block * PIPES.len() + k, d)).unwrap();
                row += 1;
            }
            let sum: f64 = (0..PIPES.len()).map(|k| flow(g, block * PIPES.len() + k, d)).sum();
            let letter = rust_xlsxwriter::utility::column_number_to_name(col);
            let f = format!("=SUM({letter}{}:{letter}{})", first + 1, row);
            ws.write_formula(row, col, Formula::new(f).set_result(sum.to_string())).unwrap();
            row += 1;
        }
    }
}

fn fixture() -> Normalized {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_row_labels_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Flows").unwrap();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    // Summary A1:D11: flow (once per block), pipeline, capacity, average; a total after each block.
    for (c, h) in ["Flow", "Pipeline", "Capacity", "Average"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    let mut row = 1;
    for (block, name) in ["Receipts", "Deliveries"].iter().enumerate() {
        ws.write_string(row, 0, *name).unwrap();
        for (k, p) in PIPES.iter().enumerate() {
            ws.write_string(row, 1, *p).unwrap();
            ws.write_number(row, 2, 50000.0 * (k + 1) as f64).unwrap();
            ws.write_number(row, 3, flow(0, block * PIPES.len() + k, 0)).unwrap();
            row += 1;
        }
        ws.write_string(row, 1, "Total").unwrap();
        ws.write_number(row, 2, 500000.0).unwrap();
        ws.write_number(row, 3, (0..PIPES.len()).map(|k| flow(0, block * PIPES.len() + k, 0)).sum::<f64>()).unwrap();
        row += 1;
    }
    // Beside it (G..K, same rows) and below it (C.., three blank rows apart): daily grids.
    grid(ws, 0, 0, 6, 5, &date);
    grid(ws, 1, 14, 2, 5, &date);
    grid(ws, 2, 28, 2, 5, &date);
    // A grid with fewer rows (no deliveries block) keeps its plain numbers.
    for d in 0..5u16 {
        ws.write_number_with_format(42, 2 + d, 37400.0 + d as f64, &date).unwrap();
        for k in 0..6u32 {
            ws.write_number(43 + k, 2 + d, (100 * k + d as u32 + 1) as f64).unwrap();
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    normalize_workbook(&book, &Options { rules_only: true, ..Options::default() })
}

fn names(t: &Table) -> Vec<&str> {
    t.columns.iter().map(|c| c.name.as_str()).collect()
}

fn text(t: &Table, col: &str) -> Vec<Option<String>> {
    let c = t.columns.iter().find(|c| c.name == col).unwrap_or_else(|| panic!("no {col} in {:?}", names(t)));
    c.values
        .iter()
        .map(|v| match v {
            Out::Text(s) => Some(s.clone()),
            Out::Null => None,
            other => panic!("{col}: {other:?}"),
        })
        .collect()
}

#[test]
fn grids_beside_and_below_get_the_summary_row_labels() {
    let n = fixture();
    let ranges: Vec<String> = n.tables.iter().map(|t| format!("{:?}", t.rect)).collect();
    assert_eq!(n.tables.len(), 5, "{ranges:?}");
    let summary = &n.tables[0];
    assert_eq!(&names(summary)[..2], ["Flow", "Pipeline"]);
    let pipes: Vec<Option<String>> = PIPES.iter().chain(&PIPES).map(|p| Some(p.to_string())).collect();
    let flows: Vec<Option<String>> =
        ["Receipts"; 4].iter().chain(&["Deliveries"; 4]).map(|f| Some(f.to_string())).collect();
    assert_eq!(text(summary, "Pipeline"), pipes);
    assert_eq!(text(summary, "Flow"), flows);
    // The side-by-side grid, then the two monthly grids below: every data row is there (the zeros
    // under the date header too) and carries its pipeline and flow.
    for (k, g) in [(1, 0u32), (2, 1), (3, 2)] {
        let t = &n.tables[k];
        assert_eq!(&names(t)[..2], ["Flow", "Pipeline"], "T{}", t.n);
        assert_eq!(t.src_rows.len(), 8, "T{} rows {:?}", t.n, t.src_rows);
        assert_eq!(text(t, "Pipeline"), pipes, "T{}", t.n);
        assert_eq!(text(t, "Flow"), flows, "T{}", t.n);
        let copied = t.columns.iter().take(2).all(|c| c.source_col.is_none());
        assert!(copied, "T{}: copied columns have no source column", t.n);
        assert!(t.warnings.iter().any(|w| w.contains("copied from T1")), "T{}: {:?}", t.n, t.warnings);
        // The values line up: NGPL deliveries on the second day.
        let day2 = &t.columns[3];
        assert_eq!(day2.values[7], Out::Num(flow(g, 7, 1)), "T{}", t.n);
    }
    // Other row layout: no labels.
    let other = &n.tables[4];
    assert!(other.columns.iter().all(|c| c.source_col.is_some() || c.role != ColRole::Data), "{:?}", names(other));
    assert!(other.warnings.iter().all(|w| !w.contains("copied from")));
    // No data row ended up in Notes as a "repeated header".
    assert!(n.notes.iter().all(|x| x.kind != "repeated_header"), "{:?}", n.notes);
}
