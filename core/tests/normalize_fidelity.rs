//! Cell fidelity: every source value reaches the normalized output unchanged (or a documented
//! normalisation of it). Each test rebuilds, in a small workbook, a pattern the cell-fidelity
//! gate (eval/fidelity/check.py) found in the public corpora: a day of a month read as a month of
//! another year, a time of day written as a fraction, a weekday-formatted date left as a serial,
//! a row label overwritten by the one above, a group row's other cells dropped, a header row
//! that went to Notes.

use std::path::PathBuf;

use hh_core::normalize::{iso, normalize_workbook, Normalized, Options, Out, Table};
use hh_core::{open, Value};
use rust_xlsxwriter::{Format, Workbook, Worksheet};

fn build(name: &str, fill: impl Fn(&mut Worksheet)) -> Normalized {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_fidelity_{name}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Sheet1").unwrap();
    fill(ws);
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    normalize_workbook(&book, &Options { keep_totals: true, ..Options::default() })
}

fn names(t: &Table) -> Vec<&str> {
    t.columns.iter().map(|c| c.name.as_str()).collect()
}

fn col<'a>(t: &'a Table, name: &str) -> &'a [Out] {
    &t.columns.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no {name} in {:?}", names(t))).values
}

fn shown(v: &Out) -> String {
    match v {
        Out::Null => String::new(),
        Out::Num(x) => hh_core::normalize::fmt_num(*x),
        Out::Date(x) => iso(*x),
        Out::Bool(b) => b.to_string(),
        Out::Text(s) => s.clone(),
    }
}

fn texts(t: &Table, name: &str) -> Vec<String> {
    col(t, name).iter().map(shown).collect()
}

#[test]
fn a_day_of_a_month_is_not_a_month_of_another_year() {
    // "March 25" / "March 26" (MiMoTable 1257(4).xlsx, care dates) were read as March 2025 and
    // March 2026: 36 distinct days collapsed into two invented months.
    let n = build("noyear", |ws| {
        ws.write_string(0, 0, "Dog").unwrap();
        ws.write_string(0, 1, "Care Date").unwrap();
        for (r, d) in ["March 25", "March 26", "March 26", "March 25", "March 27"].iter().enumerate() {
            ws.write_string(r as u32 + 1, 0, format!("Dog {r}")).unwrap();
            ws.write_string(r as u32 + 1, 1, *d).unwrap();
        }
    });
    let t = &n.tables[0];
    assert_eq!(texts(t, "Care Date"), ["March 25", "March 26", "March 26", "March 25", "March 27"]);
}

#[test]
fn a_time_of_day_is_a_clock_time_not_a_fraction() {
    // h:mm cells (MiMoTable 3333(80).xlsx entry times) came out as 0.395833333333333.
    let n = build("times", |ws| {
        let hm = Format::new().set_num_format("h:mm");
        ws.write_string(0, 0, "Visitor").unwrap();
        ws.write_string(0, 1, "Entry Time").unwrap();
        for (r, x) in [0.395_833_333_333_333_3, 0.4375, 0.770_833_333_333_333_4].iter().enumerate() {
            ws.write_string(r as u32 + 1, 0, format!("V{r}")).unwrap();
            ws.write_number_with_format(r as u32 + 1, 1, *x, &hm).unwrap();
        }
    });
    let t = &n.tables[0];
    assert_eq!(texts(t, "Entry Time"), ["09:30:00", "10:30:00", "18:30:00"]);
}

#[test]
fn a_weekday_format_shows_a_date() {
    // `[$-804]aaa;@` (a Chinese weekday name) was not a date format: the cells stayed serials
    // (44044 …) in MiMoTable's summer study plan.
    let path: PathBuf = std::env::temp_dir().join(format!("hh_fidelity_aaa_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let aaa = Format::new().set_num_format("[$-804]aaa;@");
    ws.write_number_with_format(0, 0, 44044.0, &aaa).unwrap();
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    assert_eq!(book.sheets[0].cells[0].value, Some(Value::DateTime(44044.0)));
}

/// Interest rates by item: a label merged over its rows in A, sub-items in B (BankDeposit-
/// InterestRates.xlsx, Sheet2), with a group row ("III. Discount") whose rate is a sentence.
fn rates(ws: &mut Worksheet) {
    let rows: [(&str, &str, Option<f64>); 8] = [
        ("I. Short-term Loans", "Within Six Months (inclusive)", Some(5.6)),
        ("", "Six Months to One Year (inclusive)", Some(6.0)),
        ("II. Medium and Long-term Loans", "One to Three Years (inclusive)", Some(6.15)),
        ("", "Three to Five Years (inclusive)", Some(6.4)),
        ("", "More than Five Years", Some(6.55)),
        ("III. Discount", "", None),
        ("IV. Housing Provident Fund Loans", "Within Five Years (inclusive)", Some(4.0)),
        ("", "More than Five Years", Some(4.5)),
    ];
    ws.write_string(0, 0, "Item").unwrap();
    ws.write_string(0, 2, "Annual Interest Rate (%)").unwrap();
    ws.merge_range(0, 0, 0, 1, "Item", &Format::new()).unwrap();
    for (k, (a, b, x)) in rows.iter().enumerate() {
        let r = k as u32 + 1;
        if !a.is_empty() {
            ws.write_string(r, 0, *a).unwrap();
        }
        if !b.is_empty() {
            ws.write_string(r, 1, *b).unwrap();
        }
        match x {
            Some(x) => ws.write_number(r, 2, *x).unwrap(),
            None => ws.write_string(r, 2, "Determined by adding points to the rediscount rate").unwrap(),
        };
    }
    let f = Format::new();
    ws.merge_range(1, 0, 2, 0, "I. Short-term Loans", &f).unwrap();
    ws.merge_range(3, 0, 5, 0, "II. Medium and Long-term Loans", &f).unwrap();
    ws.merge_range(7, 0, 8, 0, "IV. Housing Provident Fund Loans", &f).unwrap();
}

#[test]
fn every_row_keeps_its_own_label() {
    let n = build("rates", rates);
    for t in &n.tables {
        let Some(c) = t.columns.iter().find(|c| c.source_col == Some(0)) else { continue };
        for (i, v) in c.values.iter().enumerate() {
            let row = t.src_rows[i];
            let want = match row {
                1 | 2 => "I. Short-term Loans",
                3..=5 => "II. Medium and Long-term Loans",
                7 | 8 => "IV. Housing Provident Fund Loans",
                _ => continue,
            };
            assert_eq!(shown(v), want, "row {} of {:?}", row + 1, names(t));
        }
    }
}

#[test]
fn nothing_of_a_group_row_is_dropped() {
    // A group row's label goes to the group column; the rest of the row ("Determined by adding
    // points …" in the rate column) went nowhere.
    let n = build("rates2", rates);
    let in_tables = n.tables.iter().any(|t| {
        t.columns.iter().any(|c| c.values.iter().any(|v| shown(v).starts_with("Determined by adding points")))
    });
    let in_notes = n.notes.iter().any(|x| x.text.starts_with("Determined by adding points"));
    assert!(in_tables || in_notes, "{:?}", n.notes);
}

#[test]
fn a_header_row_under_a_key_value_line_names_the_columns() {
    // ProjectStatisticsTable1.xlsx: "Project Name: | Number of Clients: | 7" above the header
    // row; the header went to Notes as a repeated header and the columns were col_B … col_N, so
    // an agent looked for the contract number in col_B (the row number).
    let n = build("kvline", |ws| {
        let date = Format::new().set_num_format("mm-dd-yy");
        ws.write_string(0, 1, "Project Statistics Table").unwrap();
        ws.write_string(2, 2, "Project Name:").unwrap();
        ws.write_string(2, 6, "Number of Clients:").unwrap();
        ws.write_number(2, 7, 7.0).unwrap();
        let head = ["No.", "Contract Number", "Client Name", "Client Unit", "Contact", "Project Name", "Quantity", "Start Date", "Duration", "Status"];
        for (c, h) in head.iter().enumerate() {
            ws.write_string(3, c as u16 + 1, *h).unwrap();
        }
        for r in 0..7u32 {
            let row = 4 + r;
            ws.write_number(row, 1, (r + 1) as f64).unwrap();
            ws.write_string(row, 2, format!("DH10293{}", 31 + r)).unwrap();
            ws.write_string(row, 3, format!("DaoKe{}", r + 1)).unwrap();
            ws.write_string(row, 4, format!("Unit {r}")).unwrap();
            ws.write_string(row, 5, format!("180xxxx102{}", r + 1)).unwrap();
            ws.write_string(row, 6, format!("Product {r}")).unwrap();
            ws.write_string(row, 7, format!("{} sets", 10 * (r + 1))).unwrap();
            ws.write_number_with_format(row, 8, 44378.0 + 3.0 * r as f64, &date).unwrap();
            ws.write_number(row, 9, (40 + 7 * r) as f64).unwrap();
            ws.write_string(row, 10, "Completed").unwrap();
        }
    });
    let t = n.tables.iter().find(|t| t.columns.iter().any(|c| c.values.iter().any(|v| shown(v) == "DH1029331"))).unwrap();
    assert_eq!(texts(t, "Contract Number")[..2], ["DH1029331", "DH1029332"], "{:?}", names(t));
    assert_eq!(texts(t, "Client Unit")[..2], ["Unit 0", "Unit 1"]);
    assert_eq!(t.header_rows, 1);
    assert!(!t.warnings.iter().any(|w| w.starts_with("no header row")), "{:?}", t.warnings);
}

#[test]
fn values_with_unit_letters_are_not_scaled() {
    // "1.0MM" (a thickness in millimetres) became 1000000 and "26th" became 26000.
    let n = build("units", |ws| {
        ws.write_string(0, 0, "Material").unwrap();
        ws.write_string(0, 1, "Thickness").unwrap();
        ws.write_string(0, 2, "Day").unwrap();
        for (r, (th, d)) in [("1.0MM", "14th"), ("3.0MM", "26th"), ("3.0MM", "29th")].iter().enumerate() {
            ws.write_string(r as u32 + 1, 0, format!("Plate {r}")).unwrap();
            ws.write_string(r as u32 + 1, 1, *th).unwrap();
            ws.write_string(r as u32 + 1, 2, *d).unwrap();
        }
    });
    let t = &n.tables[0];
    assert_eq!(texts(t, "Thickness"), ["1.0MM", "3.0MM", "3.0MM"]);
    assert_eq!(texts(t, "Day"), ["14th", "26th", "29th"]);
}

#[test]
fn a_header_row_under_a_block_of_key_value_lines_names_the_columns() {
    // Enron "Area Managers.xls" (TableSense): a deal form of `Key: | value` lines above the
    // monthly table; the table started at the form, got no header, and its header row went to
    // Notes as a repeated header (columns col_A … col_G).
    let n = build("kvblock", |ws| {
        let date = Format::new().set_num_format("yyyy-mm-dd");
        ws.write_string(2, 0, "Counterparty Name:").unwrap();
        ws.write_string(2, 1, "The Oakridge Co-op").unwrap();
        ws.write_string(2, 5, "Deal Date:").unwrap();
        ws.write_number_with_format(2, 6, 37034.0, &date).unwrap();
        ws.write_string(3, 0, "Originator:").unwrap();
        ws.write_string(3, 1, "Greg Frers").unwrap();
        ws.write_string(3, 5, "Deal ID #:").unwrap();
        ws.write_string(5, 0, "Deal Start Date:").unwrap();
        ws.write_number_with_format(5, 1, 37104.0, &date).unwrap();
        ws.write_string(5, 5, "Unit of Measure:").unwrap();
        ws.write_string(5, 6, "GJ").unwrap();
        ws.write_string(6, 0, "Deal End Date:").unwrap();
        ws.write_number_with_format(6, 1, 37468.0, &date).unwrap();
        ws.write_string(6, 5, "Volume (Monthly/Daily):").unwrap();
        ws.write_string(6, 6, "Monthly").unwrap();
        ws.write_string(7, 0, "Term Duration (Months):").unwrap();
        ws.write_string(8, 5, "Price:").unwrap();
        ws.write_string(9, 0, "Fuel Rate %:").unwrap();
        ws.write_number(9, 1, 0.01561).unwrap();
        ws.write_string(9, 5, "Buy/Sell:").unwrap();
        ws.write_string(9, 6, "Buy").unwrap();
        for (c, h) in ["Month", "Rate 11 Volume", "Rate 13 Volume", "Combined Volume", "Fuel", "Total"].iter().enumerate() {
            ws.write_string(12, c as u16 + 1, *h).unwrap();
        }
        let months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        for (k, m) in months.iter().enumerate() {
            let row = 14 + k as u32;
            let v = 600.0 + 10.5 * k as f64;
            ws.write_string(row, 1, *m).unwrap();
            ws.write_number(row, 2, v).unwrap();
            ws.write_number(row, 4, v).unwrap();
            ws.write_number(row, 5, (v * 0.0156 * 100.0).round() / 100.0).unwrap();
            ws.write_number(row, 6, v + 10.0).unwrap();
        }
    });
    let t = n.tables.iter().find(|t| t.columns.iter().any(|c| c.values.iter().any(|v| shown(v) == "February"))).unwrap();
    assert_eq!(texts(t, "Month")[..2], ["January", "February"], "{:?}", names(t));
    assert_eq!(texts(t, "Rate 11 Volume")[0], "600");
    assert!(!n.notes.iter().any(|x| x.kind == "repeated_header" && x.text == "Month"), "{:?}", n.notes);
}

#[test]
fn a_header_over_a_blank_row_and_a_code_row_is_one_header() {
    // ONS a01 labour market tables: a "Date of publication: | … | Inquiries: | …" block, then
    // "People | Men | Women", a blank row, "Level | Rate per thousand" and the dataset codes over
    // the monthly rows. The table got no header and the label rows went to Notes.
    let n = build("ons", |ws| {
        ws.write_string(0, 0, "Table 10: Redundancies levels and rates for those aged 16 and over").unwrap();
        ws.write_string(0, 6, "Source: Labour Force Survey").unwrap();
        ws.write_string(1, 0, "Date of publication:").unwrap();
        ws.write_string(1, 1, "15 September 2026").unwrap();
        ws.write_string(1, 5, "Date of next publication:").unwrap();
        ws.write_string(1, 6, "20 October 2026").unwrap();
        ws.write_string(2, 0, "Inquiries: Email:").unwrap();
        ws.write_string(2, 1, "labour.market@ons.gov.uk").unwrap();
        ws.write_string(2, 5, "Telephone:").unwrap();
        ws.write_string(2, 6, "01633 455400").unwrap();
        ws.write_string(3, 0, "United Kingdom (thousands)").unwrap();
        for (c, h) in [(1u16, "People"), (3, "Men"), (5, "Women")] {
            ws.write_string(4, c, h).unwrap();
        }
        for c in 0..3u16 {
            ws.write_string(6, 1 + 2 * c, "Level").unwrap();
            ws.write_string(6, 2 + 2 * c, "Rate per thousand").unwrap();
        }
        ws.write_string(7, 0, "Dataset identifier code").unwrap();
        for (c, code) in ["BEAO", "BEIR", "BEIU", "BEIX", "BEJA", "BEJD"].iter().enumerate() {
            ws.write_string(7, c as u16 + 1, *code).unwrap();
        }
        let months = ["Jan-Mar", "Feb-Apr", "Mar-May", "Apr-Jun", "May-Jul", "Jun-Aug", "Jul-Sep", "Aug-Oct", "Sep-Nov", "Oct-Dec"];
        for y in 0..3u32 {
            for (m, p) in months.iter().enumerate() {
                let row = 9 + y * 10 + m as u32;
                ws.write_string(row, 0, format!("{p} {}", 2001 + y)).unwrap();
                for c in 0..6u16 {
                    let v = 100.0 + (row * 7 + c as u32 * 13) as f64 % 50.0;
                    ws.write_number(row, c + 1, if c % 2 == 0 { v } else { v / 10.0 }).unwrap();
                }
            }
        }
    });
    let t = n.tables.iter().find(|t| t.columns.iter().any(|c| c.values.iter().any(|v| shown(v) == "Feb-Apr 2001"))).unwrap();
    assert!(names(t).iter().any(|c| c.contains("Women") && c.contains("BEJD")), "{:?}", names(t));
    assert!(!n.notes.iter().any(|x| x.kind == "repeated_header"), "{:?}", n.notes);
}
