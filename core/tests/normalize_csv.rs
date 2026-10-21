//! Normalized output of a messy CSV written at test time: title line, header, European numbers
//! and dates, a total row and a note below. Re-opens the output and checks tables and manifest.

use std::path::PathBuf;

use hh_core::normalize::{normalize_file, Options};
use hh_core::{open, Sheet, Value, Workbook};

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hh_norm_csv_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const MESSY: &str = "\u{FEFF}Prodaja po gradovima 2024;;;\r
;;;\r
Grad;Iznos;Datum;Kod\r
Beograd;1.234,56;31.12.2024.;007\r
Niš;2.000,00;15.01.2025.;012\r
\"Novi Sad; centar\";987,10;01.02.2025.;101\r
Kragujevac;250;03.02.2025.;044\r
Ukupno;4.471,66;;\r
;;;\r
Izvor: interna evidencija, iznosi u RSD;;;\r
";

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
fn messy_csv_round_trip() {
    let dir = tmp_dir("messy");
    let src = dir.join("prodaja.csv");
    std::fs::write(&src, MESSY).unwrap();

    // The reader: one sheet named after the stem, plain numbers typed, the rest left as text.
    let wb = open(&src).unwrap();
    assert_eq!(names(&wb), ["prodaja"]);
    let s = &wb.sheets[0];
    assert_eq!(row_vals(s, 3), [text("Beograd"), text("1.234,56"), text("31.12.2024."), text("007")]);
    assert_eq!(row_vals(s, 5)[..2], [text("Novi Sad; centar"), text("987,10")]);
    assert_eq!(row_vals(s, 6)[..2], [text("Kragujevac"), num(250.0)]);

    let out = normalize_file(&src, dir.join("out"), &Options::default()).unwrap();
    assert_eq!(out.xlsx_path, dir.join("out/prodaja.normalized.xlsx"));
    let wb = open(&out.xlsx_path).unwrap();
    assert_eq!(names(&wb), ["README", "T1_prodaja_po_gradovima_2024", "Totals", "Notes"]);

    // European numbers and dates parsed, leading-zero codes kept as text, total row removed.
    let t1 = sheet(&wb, "T1_prodaja_po_gradovima_2024");
    assert_eq!(row_vals(t1, 0), [text("Grad"), text("Iznos"), text("Datum"), text("Kod"), text("_src_row")]);
    assert_eq!(row_count(t1), 5, "4 data rows, total removed");
    assert_eq!(row_vals(t1, 1), [text("Beograd"), num(1234.56), date(2024, 12, 31), text("007"), num(4.0)]);
    assert_eq!(row_vals(t1, 3), [text("Novi Sad; centar"), num(987.1), date(2025, 2, 1), text("101"), num(6.0)]);
    assert_eq!(row_vals(t1, 4), [text("Kragujevac"), num(250.0), date(2025, 2, 3), text("044"), num(7.0)]);

    // The total row checks out against the data above it.
    // Title, the removed total and the source note are all kept in Notes. The total holds
    // text (`4.471,66`), so it is not checked against the sum: the Totals check reads stored
    // numbers only, as for numbers stored as text in a workbook.
    let notes = sheet(&wb, "Notes");
    let kinds: Vec<Vec<Option<Value>>> = (1..row_count(notes)).map(|r| row_vals(notes, r)[..5].to_vec()).collect();
    assert_eq!(
        kinds,
        [
            [text("prodaja"), text("A1"), text("title"), text("T1"), text("Prodaja po gradovima 2024")],
            [text("prodaja"), text("B8"), text("total_row"), text("T1"), text("4.471,66")],
            [text("prodaja"), text("A10"), text("note"), text("T1"), text("Izvor: interna evidencija, iznosi u RSD")],
        ]
    );
    assert_eq!(row_count(sheet(&wb, "Totals")), 1);
    assert_eq!(out.cells.total, out.cells.in_tables + out.cells.in_totals + out.cells.in_notes);

    let m = &out.manifest_json;
    for needle in [
        "\"title\": \"Prodaja po gradovima 2024\"",
        "\"header_top\": 3",
        "\"decimal\": \",\"",
        "\"dtype\": \"date\"",
        "\"total_rows_removed\": 1",
        "\"numbers_from_text\": 3",
        "\"cells\": 24",
        "pd.read_excel(\\\"prodaja.normalized.xlsx\\\", sheet_name=\\\"T1_prodaja_po_gradovima_2024\\\")",
    ] {
        assert!(m.contains(needle), "manifest lacks {needle}\n{m}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn windows_1252_tsv() {
    let dir = tmp_dir("tsv");
    let src = dir.join("ventes.tsv");
    let data: &[u8] = b"Produit\tPrix\tPart\r\nCaf\xe9\t3,50\t12%\r\nCr\xe8me br\xfbl\xe9e\t4,20\t7,5%\r\nTh\xe9\t2,10\t5%\r\n";
    std::fs::write(&src, data).unwrap();
    let out = normalize_file(&src, &dir, &Options::default()).unwrap();
    let wb = open(&out.xlsx_path).unwrap();
    let t = &wb.sheets[1];
    assert_eq!(row_vals(t, 0), [text("Produit"), text("Prix"), text("Part"), text("_src_row")]);
    assert_eq!(row_vals(t, 2), [text("Crème brûlée"), num(4.2), num(0.075), num(3.0)]);
    std::fs::remove_dir_all(&dir).ok();
}

/// Statistical-office conventions in one CSV: percentages with up to three decimals next to plain
/// decimals (`40.805` is 40.805, B40), integers with an EU thousands dot next to typed integers
/// (`4.405` is 4405), footnote marks on numbers, `:` for "not available", and censored /
/// suppressed cells that stay text without turning a short numeric column into text (B34).
#[test]
fn statistical_markers_csv() {
    let dir = tmp_dir("stat");
    let src = dir.join("stat.csv");
    let data = "Name,Gap,Staff,Revised,Small\n\
                A,12.3,3178,123¹,12\n\
                B,40.805,4.405,45 p,<5\n\
                C,8.4,2411,20560.32e,15\n\
                D,-3.25,512,12 (r),c\n\
                E,20.1,1.250,7,:\n\
                F,15.125,980,8,9\n";
    std::fs::write(&src, data).unwrap();
    let out = normalize_file(&src, &dir, &Options::default()).unwrap();
    let wb = open(&out.xlsx_path).unwrap();
    let t = &wb.sheets[1];
    assert_eq!(row_vals(t, 2), [text("B"), num(40.805), num(4405.0), num(45.0), text("<5"), num(3.0)]);
    assert_eq!(row_vals(t, 3)[..5], [text("C"), num(8.4), num(2411.0), num(20560.32), num(15.0)]);
    assert_eq!(row_vals(t, 4)[3..5], [num(12.0), text("c")]);
    assert_eq!(row_vals(t, 5)[1..5], [num(20.1), num(1250.0), num(7.0), None]);
    let m = &out.manifest_json;
    let compact: String = m.split_whitespace().collect();
    for needle in [
        "\"footnote_marks_stripped\":4",
        "{\"mark\":\"¹\",\"count\":1,\"cells\":[\"D2\"]}",
        "{\"mark\":\"e\",\"count\":1,\"cells\":[\"D4\"]}",
        "\"missing_to_null\":1",
        "censored,suppressedorsee-notevalue(s)keptastext:E3'<5',E5'c'",
    ] {
        assert!(compact.contains(needle), "manifest lacks {needle}\n{m}");
    }
    let small = compact.split("\"name\":\"Small\"").nth(1).unwrap();
    assert!(small.split("\"name\"").next().unwrap().contains("\"dtype\":\"integer\""), "{m}");
    std::fs::remove_dir_all(&dir).ok();
}
