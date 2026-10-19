//! `values::compute` on real xlsx files built at test time (no spreadsheets in git).

use std::path::PathBuf;

use hh_core::values::{self, DecimalSep, DateOrder};
use hh_core::{open, Features, Grid, Sheet};
use rust_xlsxwriter::Workbook;

fn fixture(name: &str, build: impl FnOnce(&mut Workbook)) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hh_values_{}_{name}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    build(&mut wb);
    wb.save(&path).unwrap();
    path
}

struct View<'a> {
    sheet: &'a Sheet,
    f: Features,
}

impl View<'_> {
    fn at(&self, r: u32, c: u32, name: &str) -> f32 {
        let i = self.sheet.cells.iter().position(|x| x.row == r && x.col == c).unwrap_or_else(|| panic!("no cell {r},{c}"));
        self.f.value(name, i).unwrap()
    }
}

#[test]
fn serbian_report_sheet() {
    let path = fixture("sr", |wb| {
        let ws = wb.add_worksheet().set_name("Izveštaj").unwrap();
        ws.write_string(0, 0, "Pregled prihoda po opštinama (u hiljadama dinara)").unwrap();
        for (c, h) in ["Opština", "Iznos", "Datum", "Udeo", "Napomena"].iter().enumerate() {
            ws.write_string(2, c as u16, *h).unwrap();
        }
        let rows = [
            ("Београд", "1.200", "12.03.2024.", "12,5%", "-"),
            ("  Novi Sad ", "950", "1. april 2024.", "8%", "n/a"),
            ("NIŠ", "12.500,75", "Q2 2024", "3,25 %", "x"),
        ];
        for (i, (a, b, c, d, e)) in rows.iter().enumerate() {
            let r = 3 + i as u32;
            ws.write_string(r, 0, *a).unwrap();
            ws.write_string(r, 1, *b).unwrap();
            ws.write_string(r, 2, *c).unwrap();
            ws.write_string(r, 3, *d).unwrap();
            ws.write_string(r, 4, *e).unwrap();
        }
        ws.write_string(6, 0, "Укупно:").unwrap();
        ws.write_formula(6, 1, "=SUM(B4:B6)").unwrap();
        ws.write_number(6, 3, 2024).unwrap();
        ws.write_string(8, 0, "Izvor: Ministarstvo finansija").unwrap();
        ws.write_string(9, 0, "1. Porez na dohodak").unwrap();
    });
    let wb = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let sheet = &wb.sheets[0];
    let grid = Grid::new(sheet);
    let mut f = Features::new(sheet.cells.len());
    values::compute(&grid, &mut f);
    assert_eq!(f.names().len(), values::FEATURES.len());
    let v = View { sheet, f };

    // Column B is decided as comma-decimal by "12.500,75", so "1.200" is 1200.
    let hints = values::sheet_column_hints(&grid);
    assert_eq!(hints[&1].decimal, Some(DecimalSep::Comma));
    assert_eq!(v.at(3, 1, "v_numeric_text"), 1.0);
    assert_eq!(v.at(3, 1, "v_is_integer"), 1.0);
    assert!((v.at(3, 1, "v_log_abs") - 1201f32.log10()).abs() < 1e-4);
    assert_eq!(v.at(5, 1, "v_is_integer"), 0.0);

    // Dates as text, in three forms.
    for r in 3..=5 {
        assert_eq!(v.at(r, 2, "v_date_text"), 1.0, "row {r}");
        assert_eq!(v.at(r, 2, "v_numeric_text"), 0.0, "row {r}");
    }
    // Percent text.
    for r in 3..=5 {
        assert_eq!(v.at(r, 3, "v_is_percent"), 1.0, "row {r}");
    }
    // Missing tokens.
    for r in 3..=5 {
        assert_eq!(v.at(r, 4, "v_missing_token"), 1.0, "row {r}");
    }
    // Text variants.
    assert_eq!(v.at(3, 0, "v_has_cyrillic"), 1.0);
    assert_eq!(v.at(4, 0, "v_is_padded"), 1.0);
    assert_eq!(v.at(5, 0, "v_upper_ratio"), 1.0);
    // Total row: Cyrillic keyword + formula.
    assert_eq!(v.at(6, 0, "v_kw_total"), 1.0);
    assert_eq!(v.at(6, 0, "v_ends_with_colon"), 1.0);
    assert_eq!(v.at(6, 1, "v_is_formula"), 1.0);
    assert_eq!(v.at(6, 3, "v_is_year"), 1.0);
    // Title, note and enumerated label.
    assert_eq!(v.at(0, 0, "v_kw_total"), 0.0);
    assert!(v.at(0, 0, "v_word_count") >= 6.0);
    assert_eq!(v.at(8, 0, "v_kw_note"), 1.0);
    assert_eq!(v.at(9, 0, "v_starts_with_enumerator"), 1.0);
    assert_eq!(v.at(2, 0, "v_is_text"), 1.0);
    assert_eq!(v.at(2, 0, "v_numeric_text"), 0.0);
}

#[test]
fn english_sheet_with_native_types() {
    let path = fixture("en", |wb| {
        let ws = wb.add_worksheet();
        ws.write_string(0, 0, "Date").unwrap();
        ws.write_string(0, 1, "Amount").unwrap();
        ws.write_string(0, 2, "Flag").unwrap();
        let dates = ["03/04/2024", "25/04/2024", "13/05/2024"];
        let amounts = ["$1,200.50", "(500)", "1,200"];
        for i in 0..3u32 {
            ws.write_string(1 + i, 0, dates[i as usize]).unwrap();
            ws.write_string(1 + i, 1, amounts[i as usize]).unwrap();
        }
        ws.write_boolean(1, 2, true).unwrap();
        ws.write_string(2, 2, "Yes").unwrap();
        ws.write_number(3, 2, 0.5).unwrap();
        ws.write_formula(4, 1, "=1/0").unwrap();
        ws.write_string(5, 0, "Grand Total").unwrap();
        ws.write_string(5, 1, "#N/A").unwrap();
    });
    let wb = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let sheet = &wb.sheets[0];
    let grid = Grid::new(sheet);
    let hints = values::sheet_column_hints(&grid);
    // Two of three dates have day > 12 → whole column is day/month.
    assert_eq!(hints[&0].date_order, Some(DateOrder::DayMonth));
    assert_eq!(hints[&1].decimal, Some(DecimalSep::Dot));
    let mut f = Features::new(sheet.cells.len());
    values::compute(&grid, &mut f);
    let v = View { sheet, f };
    assert_eq!(v.at(1, 0, "v_date_text"), 1.0);
    assert_eq!(v.at(1, 1, "v_has_currency"), 1.0);
    assert_eq!(v.at(2, 1, "v_is_negative"), 1.0);
    assert_eq!(v.at(3, 1, "v_is_integer"), 1.0);
    assert_eq!(v.at(1, 2, "v_is_bool"), 1.0);
    assert_eq!(v.at(2, 2, "v_bool_text"), 1.0);
    assert_eq!(v.at(3, 2, "v_is_number"), 1.0);
    assert_eq!(v.at(3, 2, "v_is_integer"), 0.0);
    assert_eq!(v.at(5, 0, "v_kw_total"), 1.0);
    assert_eq!(v.at(5, 1, "v_is_error"), 1.0);
    assert_eq!(v.at(5, 1, "v_missing_token"), 1.0);
    assert_eq!(v.at(4, 1, "v_is_formula"), 1.0);
}
