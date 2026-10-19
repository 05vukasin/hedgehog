//! Fixtures are generated with rust_xlsxwriter at test time, so no spreadsheets live in git.

use std::path::PathBuf;

use hh_core::{open, Rect, Value};
use rust_xlsxwriter::{Format, Workbook};

fn fixture(name: &str, build: impl FnOnce(&mut Workbook)) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hh_test_{}_{name}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    build(&mut wb);
    wb.save(&path).unwrap();
    path
}

#[test]
fn reads_values_formulas_and_merges() {
    let path = fixture("basic", |wb| {
        let ws = wb.add_worksheet().set_name("Report").unwrap();
        ws.merge_range(1, 1, 1, 3, "Q3 sales", &Format::new()).unwrap();
        ws.write_string(3, 1, "City").unwrap();
        ws.write_string(3, 2, "Budget").unwrap();
        ws.write_string(4, 1, "Beograd").unwrap();
        ws.write_number(4, 2, 1200.5).unwrap();
        ws.write_string(5, 1, "Niš").unwrap();
        ws.write_number(5, 2, 800.0).unwrap();
        ws.write_string(6, 1, "Ukupno").unwrap();
        ws.write_formula(6, 2, "=SUM(C5:C6)").unwrap();
    });
    let wb = open(&path).unwrap();
    std::fs::remove_file(&path).ok();

    let sheet = &wb.sheets[0];
    assert_eq!(sheet.name, "Report");
    assert_eq!(sheet.merges, vec![Rect { top: 1, left: 1, bottom: 1, right: 3 }]);
    assert_eq!(sheet.used_range(), Some(Rect { top: 1, left: 1, bottom: 6, right: 3 }));

    let at = |r, c| sheet.cells.iter().find(|x| x.row == r && x.col == c).unwrap();
    assert_eq!(at(4, 1).value, Some(Value::Text("Beograd".into())));
    assert_eq!(at(4, 2).value, Some(Value::Number(1200.5)));
    assert_eq!(at(6, 2).formula.as_deref(), Some("SUM(C5:C6)"));
    assert_eq!(sheet.formula_count(), 1);
}

#[test]
fn empty_sheet_has_no_range() {
    let path = fixture("empty", |wb| {
        wb.add_worksheet();
    });
    let wb = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(wb.sheets[0].used_range(), None);
}

#[test]
fn missing_file_is_an_error() {
    assert!(open("/nonexistent/file.xlsx").is_err());
}
