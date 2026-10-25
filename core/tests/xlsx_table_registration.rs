//! Public writer API behavior, including the existing state left by failed registrations.
use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};

use rust_xlsxwriter::{Table, Workbook, Worksheet, XlsxError};

fn overlap(error: XlsxError) -> (String, String) {
    match error {
        XlsxError::TableRangeOverlaps(current, previous) => (current, previous),
        other => panic!("expected table overlap, got {other}"),
    }
}

#[test]
fn table_registration_keeps_disjoint_tables_and_first_row_major_collision() {
    let mut ws = Worksheet::new();
    ws.add_table(2, 2, 4, 3, &Table::new()).unwrap();
    ws.add_table(6, 0, 8, 1, &Table::new()).unwrap();
    // Although both previous tables overlap, the first collision is in row 2.
    let err = ws.add_table(0, 0, 7, 4, &Table::new()).map(|_| ()).unwrap_err();
    assert_eq!(overlap(err), ("(0, 0, 7, 4) / A1:E8".into(), "(2, 2, 4, 3) / C3:D5".into()));
    // The rejected table inserted a prefix tagged with the next (absent) table index.
    // Continuing into that prefix preserves the original missing-range panic.
    assert!(catch_unwind(AssertUnwindSafe(|| {
        let _ = ws.add_table(0, 0, 1, 1, &Table::new());
    })).is_err());
}

#[test]
fn first_table_autofilter_failure_keeps_its_occupancy_state() {
    let mut ws = Worksheet::new();
    ws.autofilter(0, 0, 3, 1).unwrap();
    let err = ws.add_table(0, 0, 3, 1, &Table::new()).map(|_| ()).unwrap_err();
    assert!(matches!(err, XlsxError::AutofilterRangeOverlaps(_, _)));
    assert!(catch_unwind(AssertUnwindSafe(|| {
        let _ = ws.add_table(0, 0, 2, 1, &Table::new());
    })).is_err());
}

#[test]
fn early_registration_errors_do_not_claim_cells_or_lose_prior_tables() {
    let mut ws = Worksheet::new();
    assert!(matches!(ws.add_table(5, 0, 4, 1, &Table::new()), Err(XlsxError::RowColumnOrderError)));
    ws.add_table(0, 0, 2, 1, &Table::new()).unwrap();
    assert!(matches!(ws.add_table(5, 0, 4, 1, &Table::new()), Err(XlsxError::RowColumnOrderError)));
    ws.add_table(3, 0, 5, 1, &Table::new()).unwrap();
    let err = ws.add_table(1, 0, 4, 1, &Table::new()).map(|_| ()).unwrap_err();
    assert_eq!(overlap(err).1, "(0, 0, 2, 1) / A1:B3");
}

#[test]
fn single_low_memory_table_serializes_cells_and_registration_metadata() {
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet_with_low_memory();
    ws.add_table(0, 0, 3, 1, &Table::new().set_name("Values")).unwrap();
    for row in 1..=3 {
        ws.write_number(row, 0, row as f64).unwrap();
        ws.write_string(row, 1, format!("row {row}")).unwrap();
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(wb.save_to_buffer().unwrap())).unwrap();
    let mut table = String::new();
    zip.by_name("xl/tables/table1.xml").unwrap().read_to_string(&mut table).unwrap();
    assert!(table.contains("name=\"Values\"") && table.contains("ref=\"A1:B4\""));
    let mut sheet = String::new();
    zip.by_name("xl/worksheets/sheet1.xml").unwrap().read_to_string(&mut sheet).unwrap();
    assert!(sheet.contains("<tableParts count=\"1\">") && sheet.contains("r=\"A4\""));
    let mut strings = String::new();
    zip.by_name("xl/sharedStrings.xml").unwrap().read_to_string(&mut strings).unwrap();
    assert!(strings.contains("row 3"));
}
