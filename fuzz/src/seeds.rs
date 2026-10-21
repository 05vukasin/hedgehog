//! Generated seed workbooks (`hh_fuzz seeds <dir>`): small xlsx files with the features the
//! readers handle specially — shared and inline strings, dates, number formats, merges, formulas,
//! several sheets, total rows — plus CSV variants. Real files from `$HEDGEHOG_DATA` are added next
//! to these by `fuzz/run.sh`.

use std::path::Path;

use rust_xlsxwriter::{Format, FormatAlign, Formula, Workbook};

pub fn write(dir: &Path) -> Result<(), String> {
    let e = |e: rust_xlsxwriter::XlsxError| e.to_string();
    std::fs::create_dir_all(dir.join("xlsx")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("csv")).map_err(|e| e.to_string())?;

    // 1: a titled table with a header, merges, dates, number formats, a total row with formulas.
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet().set_name("Budget").map_err(e)?;
    let bold = Format::new().set_bold().set_align(FormatAlign::Center);
    let date = Format::new().set_num_format("dd.mm.yyyy");
    let money = Format::new().set_num_format("#,##0.00 \"€\"");
    ws.merge_range(0, 0, 0, 3, "Budget 2023 (in EUR)", &bold).map_err(e)?;
    for (c, h) in ["City", "Date", "Amount", "Share"].iter().enumerate() {
        ws.write_string_with_format(2, c as u16, *h, &bold).map_err(e)?;
    }
    for r in 0..6u32 {
        ws.write_string(3 + r, 0, ["Beograd", "Niš", "Novi Sad", "Zürich", "Wien", "Paris"][r as usize]).map_err(e)?;
        ws.write_number_with_format(3 + r, 1, 44_927.0 + r as f64 * 31.0, &date).map_err(e)?;
        ws.write_number_with_format(3 + r, 2, 1000.5 * (r + 1) as f64, &money).map_err(e)?;
        ws.write_formula(3 + r, 3, Formula::new(format!("C{}/C10", 4 + r))).map_err(e)?;
    }
    ws.write_string_with_format(9, 0, "Total", &bold).map_err(e)?;
    ws.write_formula(9, 2, Formula::new("SUM(C4:C9)").set_result("21010.5")).map_err(e)?;
    let ws2 = wb.add_worksheet().set_name("Notes & more").map_err(e)?;
    ws2.write_string(0, 0, "Source: test").map_err(e)?;
    ws2.write_number(5, 5, 1.0).map_err(e)?;
    ws2.write_boolean(6, 5, true).map_err(e)?;
    wb.save(dir.join("xlsx/gen_budget.xlsx")).map_err(e)?;

    // 2: a stacked sheet with two tables and a group column.
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let mut r = 0u32;
    for (title, n) in [("Table A", 4u32), ("Table B", 3)] {
        ws.write_string(r, 0, title).map_err(e)?;
        ws.write_string(r + 1, 0, "Region").map_err(e)?;
        ws.write_string(r + 1, 1, "Q1").map_err(e)?;
        ws.write_string(r + 1, 2, "Q2").map_err(e)?;
        for i in 0..n {
            ws.write_string(r + 2 + i, 0, format!("R{i}")).map_err(e)?;
            ws.write_number(r + 2 + i, 1, (i * 7) as f64).map_err(e)?;
            ws.write_number(r + 2 + i, 2, (i * 11) as f64 + 0.25).map_err(e)?;
        }
        ws.write_string(r + 2 + n, 0, "Ukupno").map_err(e)?;
        r += n + 5;
    }
    wb.save(dir.join("xlsx/gen_stacked.xlsx")).map_err(e)?;

    // 3: an empty workbook and a single-cell one.
    let mut wb = Workbook::new();
    wb.add_worksheet();
    wb.save(dir.join("xlsx/gen_empty.xlsx")).map_err(e)?;
    let mut wb = Workbook::new();
    wb.add_worksheet().write_string(0, 0, "x").map_err(e)?;
    wb.save(dir.join("xlsx/gen_one.xlsx")).map_err(e)?;

    let csvs: &[(&str, &str)] = &[
        ("comma.csv", "City,Amount,Date\nBeograd,1200.5,2023-01-31\nNiš,\"1,000\",2023-02-28\nTotal,2200.5,\n"),
        ("semicolon.csv", "sep=;\nGrad;Iznos;Datum\nBeograd;1.200,50;31.12.2023\nNiš;-0,5;01.01.2024\nUkupno;1.200,00;\n"),
        ("tab.tsv", "a\tb\tc\n1\t2\t3\n\"x\ty\"\t\t\n"),
        ("quoted.csv", "\u{feff}\"h1\",\"h \"\"2\"\"\"\n\"multi\nline\",2\n"),
        ("ragged.csv", "a,b\n1\n1,2,3,4\n\n,,\n"),
    ];
    for (name, body) in csvs {
        std::fs::write(dir.join("csv").join(name.replace(".tsv", ".csv")), body).map_err(|e| e.to_string())?;
    }
    // UTF-16 LE with BOM (Excel's "Unicode text").
    let mut u16le = vec![0xff, 0xfe];
    for u in "a\tb\r\n1\t2\r\n".encode_utf16() {
        u16le.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(dir.join("csv/utf16.csv"), u16le).map_err(|e| e.to_string())?;
    Ok(())
}
