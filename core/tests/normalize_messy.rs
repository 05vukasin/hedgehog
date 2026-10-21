//! Normalized output on messy fixtures generated at test time: normalize → re-open the output
//! with `hh_core::open` (calamine) and check headers, types, values, `_src_row`, Totals, Notes
//! and the manifest.

use std::path::{Path, PathBuf};

use hh_core::normalize::{normalize_file, Options};
use hh_core::values::Locale;
use hh_core::{open, Sheet, Value, Workbook};
use rust_xlsxwriter::{Format, Formula, Workbook as XWorkbook};

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hh_norm_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Sales sheet: title, 2-row merged header, group headers, subtotals + grand total, US numbers as
/// text, text dates, percentages, missing tokens, a note below.
fn build_messy(path: &Path) {
    let mut wb = XWorkbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Sales").unwrap();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    ws.write_string(0, 0, "Sales report 2024").unwrap();
    ws.merge_range(2, 0, 3, 0, "Product", &Format::new()).unwrap();
    ws.merge_range(2, 1, 2, 2, "Revenue", &Format::new()).unwrap();
    ws.write_string(3, 1, "Q1").unwrap();
    ws.write_string(3, 2, "Q2").unwrap();
    ws.merge_range(2, 3, 3, 3, "Date", &Format::new()).unwrap();
    ws.merge_range(2, 4, 3, 4, "Share", &Format::new()).unwrap();
    ws.write_string(4, 0, "Region: North").unwrap();
    ws.write_string(5, 0, "  Apple ").unwrap();
    ws.write_string(5, 1, "1,200.50").unwrap();
    ws.write_string(5, 2, "1,000.00").unwrap();
    ws.write_string(5, 3, "03/15/2024").unwrap();
    ws.write_string(5, 4, "12%").unwrap();
    ws.write_string(6, 0, "Pear").unwrap();
    ws.write_number(6, 1, 300.0).unwrap();
    ws.write_number(6, 2, 200.0).unwrap();
    ws.write_number_with_format(6, 3, 45371.0, &date).unwrap(); // 2024-03-20
    ws.write_string(6, 4, "n/a").unwrap();
    ws.write_string(7, 0, "Subtotal North").unwrap();
    ws.write_number(7, 1, 1500.5).unwrap();
    ws.write_number(7, 2, 1200.0).unwrap();
    ws.write_string(8, 0, "Region: South").unwrap();
    ws.write_string(9, 0, "Banana").unwrap();
    ws.write_number(9, 1, 50.0).unwrap();
    ws.write_number(9, 2, 60.0).unwrap();
    ws.write_string(9, 3, "04/01/2024").unwrap();
    ws.write_string(9, 4, "5%").unwrap();
    ws.write_string(10, 0, "Kiwi").unwrap();
    ws.write_string(10, 1, "-").unwrap();
    ws.write_number(10, 2, 70.0).unwrap();
    ws.write_string(10, 3, "04/02/2024").unwrap();
    ws.write_string(10, 4, "7.5%").unwrap();
    ws.write_string(11, 0, "Subtotal South").unwrap();
    ws.write_number(11, 1, 50.0).unwrap();
    ws.write_number(11, 2, 130.0).unwrap();
    ws.write_string(12, 0, "Grand total").unwrap();
    ws.write_number(12, 1, 1550.5).unwrap();
    ws.write_number(12, 2, 1999.0).unwrap(); // wrong on purpose (should be 1330)
    ws.write_string(14, 0, "Source: internal ERP export, amounts in USD").unwrap();

    // Two stacked tables, European numbers, a Serbian Cyrillic header, text dates.
    let ws = wb.add_worksheet();
    ws.set_name("Prodaja").unwrap();
    for (c, h) in ["Град", "Износ", "Датум"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    let rows = [
        ("Београд", "1.234,56", "31.12.2024."),
        ("Ниш", "2.000,00", "15.01.2025."),
        ("Нови Сад", "987,10", "01.02.2025."),
        ("Крагујевац", "#N/A", "03.02.2025."),
    ];
    for (i, (a, b, c)) in rows.iter().enumerate() {
        let r = 1 + i as u32;
        ws.write_string(r, 0, *a).unwrap();
        ws.write_string(r, 1, *b).unwrap();
        ws.write_string(r, 2, *c).unwrap();
    }
    for (c, h) in ["Code", "Qty", "Active", "Code"].iter().enumerate() {
        ws.write_string(10, c as u16, *h).unwrap();
    }
    for i in 0..4u32 {
        ws.write_string(11 + i, 0, format!("00{}", i + 1)).unwrap();
        ws.write_number(11 + i, 1, (i * 10) as f64).unwrap();
        ws.write_string(11 + i, 2, if i % 2 == 0 { "yes" } else { "no" }).unwrap();
        ws.write_string(11 + i, 3, format!("X{i}")).unwrap();
    }
    ws.write_string(20, 6, "stray").unwrap();
    wb.save(path).unwrap();
}

fn sheet<'a>(wb: &'a Workbook, name: &str) -> &'a Sheet {
    wb.sheets.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no sheet {name}: {:?}", names(wb)))
}

fn names(wb: &Workbook) -> Vec<String> {
    wb.sheets.iter().map(|s| s.name.clone()).collect()
}

fn get(s: &Sheet, row: u32, col: u32) -> Option<&Value> {
    s.cells.iter().find(|c| c.row == row && c.col == col).and_then(|c| c.value.as_ref())
}

fn row_vals(s: &Sheet, row: u32) -> Vec<Option<Value>> {
    let w = s.cells.iter().map(|c| c.col).max().unwrap_or(0);
    (0..=w).map(|c| get(s, row, c).cloned()).collect()
}

fn text(s: &str) -> Option<Value> {
    Some(Value::Text(s.to_string()))
}

fn num(v: f64) -> Option<Value> {
    Some(Value::Number(v))
}

fn date(y: i32, m: u8, d: u8) -> Option<Value> {
    Some(Value::DateTime(hh_core::values::ymd_to_serial(y, m, d)))
}

fn row_count(s: &Sheet) -> u32 {
    s.cells.iter().map(|c| c.row + 1).max().unwrap_or(0)
}

#[test]
fn messy_workbook_round_trip() {
    let dir = tmp_dir("messy");
    let src = dir.join("messy.xlsx");
    build_messy(&src);
    let before = std::fs::read(&src).unwrap();
    let out = normalize_file(&src, dir.join("out"), &Options::default()).unwrap();
    assert_eq!(std::fs::read(&src).unwrap(), before, "source must not change");
    assert_eq!(out.xlsx_path, dir.join("out/messy.normalized.xlsx"));
    assert_eq!(out.manifest_path, dir.join("out/messy.manifest.json"));
    assert!(out.csv_paths.is_empty());
    let wb = open(&out.xlsx_path).unwrap();
    assert_eq!(names(&wb), ["README", "T1_sales_report_2024", "T2_prodaja", "T3_prodaja", "Totals", "Notes"]);

    // T1: title above, 2-row merged header, groups → leading column, totals removed.
    let t1 = sheet(&wb, "T1_sales_report_2024");
    assert_eq!(
        row_vals(t1, 0),
        [text("Region"), text("Product"), text("Revenue / Q1"), text("Revenue / Q2"), text("Date"), text("Share"), text("_src_row")]
    );
    assert_eq!(row_count(t1), 5, "4 data rows, subtotals and grand total removed");
    assert_eq!(
        row_vals(t1, 1),
        [text("North"), text("Apple"), num(1200.5), num(1000.0), date(2024, 3, 15), num(0.12), num(6.0)]
    );
    assert_eq!(
        row_vals(t1, 2),
        [text("North"), text("Pear"), num(300.0), num(200.0), date(2024, 3, 20), None, num(7.0)]
    );
    assert_eq!(
        row_vals(t1, 4),
        [text("South"), text("Kiwi"), None, num(70.0), date(2024, 4, 2), num(0.075), num(11.0)]
    );

    // T2: European numbers as text, Serbian dates, Cyrillic header, #N/A → empty.
    let t2 = sheet(&wb, "T2_prodaja");
    assert_eq!(row_vals(t2, 0), [text("Град"), text("Износ"), text("Датум"), text("_src_row")]);
    assert_eq!(row_vals(t2, 1), [text("Београд"), num(1234.56), date(2024, 12, 31), num(2.0)]);
    assert_eq!(row_vals(t2, 3), [text("Нови Сад"), num(987.1), date(2025, 2, 1), num(4.0)]);
    assert_eq!(row_vals(t2, 4), [text("Крагујевац"), None, date(2025, 2, 3), num(5.0)]);

    // T3: leading-zero codes stay text, yes/no → bool, duplicate header renamed.
    let t3 = sheet(&wb, "T3_prodaja");
    assert_eq!(row_vals(t3, 0), [text("Code"), text("Qty"), text("Active"), text("Code (2)"), text("_src_row")]);
    assert_eq!(row_vals(t3, 2), [text("002"), num(10.0), Some(Value::Bool(false)), text("X1"), num(13.0)]);

    // Totals: subtotals + grand total, the wrong grand total flagged.
    let tot = sheet(&wb, "Totals");
    assert_eq!(get(tot, 0, 2), text("cell").as_ref());
    let rows: Vec<Vec<Option<Value>>> = (1..row_count(tot)).map(|r| row_vals(tot, r)).collect();
    assert_eq!(rows.len(), 6);
    let c13 = rows.iter().find(|r| r[2] == text("C13")).expect("grand total C13 listed");
    assert_eq!(c13[3], text("Grand total"));
    assert_eq!(c13[8], num(1999.0));
    assert_eq!(c13[10], text("mismatch"));
    assert!(rows.iter().all(|r| r[0] == text("T1") && r[14] == Some(Value::Bool(false))));
    let c12 = rows.iter().find(|r| r[2] == text("C12")).unwrap();
    assert_eq!((c12[5].clone(), c12[10].clone()), (text("subtotal"), text("ok")));

    // Notes: title, footnote and a stray cell, nothing dropped.
    let notes = sheet(&wb, "Notes");
    assert_eq!(row_vals(notes, 1), [text("Sales"), text("A1"), text("title"), text("T1"), text("Sales report 2024"), None]);
    assert_eq!(row_vals(notes, 2)[1..4], [text("A15"), text("note"), text("T1")]);
    assert_eq!(row_vals(notes, 3)[..5], [text("Prodaja"), text("G21"), text("other"), None, text("stray")]);
    assert_eq!(out.cells.total, out.cells.in_tables + out.cells.in_totals + out.cells.in_notes);
    assert_eq!(out.cells.in_notes, 3);

    // Manifest.
    let m = std::fs::read_to_string(&out.manifest_path).unwrap();
    assert_eq!(m, out.manifest_json);
    for needle in [
        "\"schema_version\": 2",
        "\"sha256\": \"",
        "\"header_top\": 3",
        "\"header_rows\": 2",
        "\"title\": \"Sales report 2024\"",
        "\"group_column\": \"Region\"",
        "\"name\": \"Revenue / Q1\"",
        "\"text\": \"Revenue\",\n              \"cell\": \"B3\"",
        "\"dtype\": \"percent\"",
        "\"dtype\": \"date\"",
        "\"dtype\": \"bool\"",
        "\"decimal\": \",\"",
        "\"date_order\": \"mdy\"",
        "\"total_rows_removed\": 3",
        "\"group_header_rows_to_column\": 2",
        "\"numbers_from_text\": 5",
        "\"errors_to_null\": 1",
        "\"duplicate_headers_renamed\": 1",
        "\"mismatch\": 1",
        "\"unassigned_cells\": 1",
        "pd.read_excel(\\\"messy.normalized.xlsx\\\", sheet_name=\\\"T1_sales_report_2024\\\")",
    ] {
        assert!(m.contains(needle), "manifest lacks {needle}\n{m}");
    }

    // Deterministic: a second run gives the same manifest bytes.
    let again = normalize_file(&src, dir.join("out2"), &Options::default()).unwrap();
    assert_eq!(again.manifest_json.replace("out2", "out"), out.manifest_json);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn keep_totals_csv_and_locale() {
    let dir = tmp_dir("keep");
    let src = dir.join("messy.xlsx");
    build_messy(&src);
    let opts = Options { keep_totals: true, csv: true, locale: Some(Locale::parse("de-DE").unwrap()), ..Options::default() };
    let out = normalize_file(&src, &dir, &opts).unwrap();
    let wb = open(&out.xlsx_path).unwrap();
    let t1 = sheet(&wb, "T1_sales_report_2024");
    assert_eq!(row_vals(t1, 0).last().unwrap(), &text("_row_type"));
    assert_eq!(row_count(t1), 8, "4 data + 2 subtotals + grand total");
    let types: Vec<Option<Value>> = (1..8).map(|r| row_vals(t1, r)[7].clone()).collect();
    assert_eq!(
        types,
        [text("data"), text("data"), text("subtotal"), text("data"), text("data"), text("subtotal"), text("total")]
    );
    assert_eq!(row_vals(t1, 7)[6], num(13.0), "_src_row of the grand total");
    // A subtotal belongs to its group, the grand total to none.
    assert_eq!(get(t1, 0, 0), text("Region").as_ref());
    let groups: Vec<Option<Value>> = (1..8).map(|r| row_vals(t1, r)[0].clone()).collect();
    assert_eq!(
        groups,
        [text("North"), text("North"), text("North"), text("South"), text("South"), text("South"), None]
    );
    // Unambiguous US text numbers are still read correctly; the locale only settles ambiguous ones.
    assert_eq!(get(t1, 1, 2), num(1200.5).as_ref());
    assert!(out.manifest_json.contains("\"locale\": \"de\""));
    // European numbers read correctly under de-DE.
    assert_eq!(get(sheet(&wb, "T2_prodaja"), 1, 1), num(1234.56).as_ref());
    // Totals kept in data are marked as such.
    let tot = sheet(&wb, "Totals");
    assert!((1..row_count(tot)).all(|r| get(tot, r, 14) == Some(&Value::Bool(true))));

    assert_eq!(out.csv_paths.len(), 3);
    let csv = std::fs::read_to_string(dir.join("messy.T2_prodaja.csv")).unwrap();
    assert_eq!(csv.lines().next().unwrap(), "Град,Износ,Датум,_src_row,_row_type");
    assert_eq!(csv.lines().nth(1).unwrap(), "Београд,1234.56,2024-12-31,2,data");
    assert_eq!(csv.lines().nth(4).unwrap(), "Крагујевац,,2025-02-03,5,data");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn forward_filled_header_formulas_and_blank_rows() {
    let dir = tmp_dir("ffill");
    let src = dir.join("plan.xlsx");
    let mut wb = XWorkbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Plan").unwrap();
    // Parent labels written once (not merged) over two children each.
    ws.write_string(0, 0, "Item").unwrap();
    ws.write_string(0, 1, "2023").unwrap();
    ws.write_string(0, 3, "2024").unwrap();
    for (c, h) in ["Plan", "Actual", "Plan", "Actual"].iter().enumerate() {
        ws.write_string(1, 1 + c as u16, *h).unwrap();
    }
    for i in 0..6u32 {
        let r = 2 + i + (i >= 3) as u32; // blank spacer row after 3 items
        ws.write_string(r, 0, format!("Item {i}")).unwrap();
        for c in 1..=4u16 {
            ws.write_number(r, c, (i * 10 + c as u32) as f64).unwrap();
        }
    }
    ws.write_string(9, 0, "Total").unwrap();
    ws.write_formula(9, 1, Formula::new("=SUM(B3:B9)").set_result("156")).unwrap();
    wb.save(&src).unwrap();

    let out = normalize_file(&src, &dir, &Options::default()).unwrap();
    let book = open(&out.xlsx_path).unwrap();
    let t = &book.sheets[1];
    assert_eq!(
        row_vals(t, 0),
        [text("Item"), text("2023 / Plan"), text("2023 / Actual"), text("2024 / Plan"), text("2024 / Actual"), text("_src_row")]
    );
    assert_eq!(row_count(t), 7);
    assert_eq!(row_vals(t, 4), [text("Item 3"), num(31.0), num(32.0), num(33.0), num(34.0), num(7.0)]);
    let m = &out.manifest_json;
    assert!(m.contains("\"header_cells_forward_filled\": 2"), "{m}");
    assert!(m.contains("\"blank_rows_removed\": 1"), "{m}");
    assert!(m.contains("\"cell\": \"B10\""), "formula total listed: {m}");
    let tot = sheet(&book, "Totals");
    assert_eq!(get(tot, 1, 13), text("=SUM(B3:B9)").as_ref());
    assert_eq!(get(tot, 1, 10), text("ok").as_ref());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn empty_and_tableless_workbooks() {
    let dir = tmp_dir("empty");
    let src = dir.join("loose.xlsx");
    let mut wb = XWorkbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "Just a note").unwrap();
    ws.write_number(3, 2, 42.0).unwrap();
    wb.add_worksheet();
    wb.save(&src).unwrap();
    let out = normalize_file(&src, &dir, &Options::default()).unwrap();
    assert_eq!(out.tables, 0);
    let book = open(&out.xlsx_path).unwrap();
    assert_eq!(names(&book), ["README", "Totals", "Notes"]);
    let notes = sheet(&book, "Notes");
    assert_eq!(row_count(notes), 3);
    assert_eq!(out.cells.in_notes, 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unparsed_values_are_kept_and_reported() {
    let dir = tmp_dir("unparsed");
    let src = dir.join("u.xlsx");
    let mut wb = XWorkbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "Budget execution").unwrap(); // title directly above the header
    ws.write_string(1, 0, "Name").unwrap();
    ws.write_string(1, 1, "Amount").unwrap();
    ws.write_string(1, 2, "Ratio").unwrap();
    for i in 0..12u32 {
        ws.write_string(2 + i, 0, format!("Row {i}")).unwrap();
        if i == 4 {
            ws.write_string(2 + i, 1, "approx. five").unwrap();
        } else {
            ws.write_string(2 + i, 1, format!("{},5", 100 + i)).unwrap();
        }
        ws.write_string(2 + i, 2, "1.200").unwrap();
    }
    wb.save(&src).unwrap();
    let out = normalize_file(&src, &dir, &Options::default()).unwrap();
    let book = open(&out.xlsx_path).unwrap();
    let t = &book.sheets[1];
    assert_eq!(get(t, 1, 1), num(100.5).as_ref());
    assert_eq!(get(t, 5, 1), text("approx. five").as_ref());
    let m = &out.manifest_json;
    assert!(m.contains("1 value(s) are not numbers and were kept as text: B7"), "{m}");
    assert!(m.contains("\"unparsed_kept_as_text\": 1"), "{m}");
    // The title is not part of the table range (the model path trims it); it names the table.
    assert!(m.contains("\"range\": \"A2:C14\""), "{m}");
    assert!(m.contains("\"header_top\": 2"), "{m}");
    assert!(m.contains("\"title\": \"Budget execution\""), "{m}");
    assert_eq!(names(&book)[1], "T1_budget_execution");
    // "1.200" in a comma-decimal sheet: thousands separator.
    assert_eq!(get(t, 1, 2), num(1200.0).as_ref());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn group_label_on_the_first_data_row_opens_a_group() {
    // Month groups: most labels have a row of their own, "Sep" sits on its group's first data row
    // after a blank row. It must start group "Sep", not stay a value under group "Aug".
    let dir = tmp_dir("inline_group");
    let src = dir.join("months.xlsx");
    let mut wb = XWorkbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Months").unwrap();
    for (c, h) in ["Item", "Amount", "Fee"].iter().enumerate() {
        ws.write_string(0, 1 + c as u16, *h).unwrap();
    }
    let item = |ws: &mut rust_xlsxwriter::Worksheet, r: u32, name: &str, v: f64| {
        ws.write_string(r, 1, name).unwrap();
        ws.write_number(r, 2, v).unwrap();
        ws.write_number(r, 3, v / 10.0).unwrap();
    };
    ws.write_string(1, 0, "Jul").unwrap();
    item(ws, 2, "Gas", 100.0);
    item(ws, 3, "Power", 200.0);
    item(ws, 4, "Services", 300.0);
    ws.write_string(5, 0, "Aug").unwrap();
    item(ws, 6, "Gas", 110.0);
    item(ws, 7, "Power", 210.0);
    item(ws, 8, "Services", 310.0);
    ws.write_string(10, 0, "Sep").unwrap();
    item(ws, 10, "Gas", 120.0);
    item(ws, 11, "Power", 220.0);
    item(ws, 12, "Services", 320.0);
    ws.write_string(14, 0, "Oct").unwrap();
    item(ws, 15, "Gas", 130.0);
    wb.save(&src).unwrap();
    let opts = Options { rules_only: true, ..Options::default() };
    let out = normalize_file(&src, &dir, &opts).unwrap();
    let book = open(&out.xlsx_path).unwrap();
    let t = &book.sheets[1];
    assert_eq!(row_vals(t, 0), [text("group"), text("Item"), text("Amount"), text("Fee"), text("_src_row")]);
    let groups: Vec<Option<Value>> = (1..row_count(t)).map(|r| get(t, r, 0).cloned()).collect();
    let expect = ["Jul", "Jul", "Jul", "Aug", "Aug", "Aug", "Sep", "Sep", "Sep", "Oct"];
    assert_eq!(groups, expect.iter().map(|g| text(g)).collect::<Vec<_>>());
    assert_eq!(row_vals(t, 7), [text("Sep"), text("Gas"), num(120.0), num(12.0), num(11.0)]);
    std::fs::remove_dir_all(&dir).ok();
}
