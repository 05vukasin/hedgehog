//! Agent-facing manifest fields: time coverage, per-table totals with total rows, cleaned header
//! names, navigation links that are not titles, and totals over non-additive columns.

use std::path::PathBuf;

use hh_core::normalize::{Options, normalize_file};
use rust_xlsxwriter::Workbook as XWorkbook;

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hh_agent_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Compact form of the pretty manifest (no newlines / indentation), for substring checks.
fn compact(s: &str) -> String {
    s.lines().map(str::trim).collect::<Vec<_>>().join("")
}

#[test]
fn agent_fields_in_the_manifest() {
    let dir = tmp_dir("fields");
    let src = dir.join("einnahmen.xlsx");
    let mut wb = XWorkbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Tab1").unwrap();
    ws.write_string(0, 0, "zur Inhaltsübersicht").unwrap();
    ws.write_string(1, 0, "Tabelle 1: Einnahmen nach Region 2022 bis 2023")
        .unwrap();
    for (c, h) in [
        "Region",
        "20221)",
        "2023",
        "Kalender-\nfaktoren",
        "Anteil in %",
    ]
    .iter()
    .enumerate()
    {
        ws.write_string(3, c as u16, *h).unwrap();
    }
    let rows = [
        ("Nord", 100.0, 110.0, 1.5, 10.0),
        ("Süd", 200.0, 210.0, 2.5, 20.0),
        ("Ost", 300.0, 310.0, 3.5, 30.0),
        ("West", 400.0, 410.0, 4.5, 40.0),
    ];
    for (i, (a, b, c, d, e)) in rows.iter().enumerate() {
        let r = 4 + i as u32;
        ws.write_string(r, 0, *a).unwrap();
        ws.write_number(r, 1, *b).unwrap();
        ws.write_number(r, 2, *c).unwrap();
        ws.write_number(r, 3, *d).unwrap();
        ws.write_number(r, 4, *e).unwrap();
    }
    ws.write_string(8, 0, "Insgesamt").unwrap();
    ws.write_number(8, 1, 1000.0).unwrap();
    ws.write_number(8, 2, 1040.0).unwrap();
    ws.write_number(8, 3, 12.0).unwrap();
    ws.write_number(8, 4, 24.0).unwrap(); // a weighted average share, not a sum
    ws.write_string(10, 0, "1) vorläufig").unwrap();
    wb.save(&src).unwrap();

    let opts = Options {
        rules_only: true,
        ..Options::default()
    };
    let out = normalize_file(&src, &dir, &opts).unwrap();
    let m = compact(&out.manifest_json);

    // Navigation link skipped: the next line is the title; the link stays in Notes as "other".
    assert!(
        m.contains(
            r#""title": "Tabelle 1: Einnahmen nach Region 2022 bis 2023","title_cell": "A2""#
        ),
        "{m}"
    );
    // Header names cleaned; the original cells untouched.
    assert!(m.contains(r#""name": "2022""#), "{m}");
    assert!(
        m.contains(r#""original_header": [{"text": "20221)","cell": "B4"}]"#),
        "{m}"
    );
    assert!(m.contains(r#""name": "Kalenderfaktoren""#), "{m}");
    assert!(m.contains(r#""headers_cleaned": 2"#), "{m}");
    // Time coverage from year headers, per table and for the workbook.
    assert!(
        m.contains(r#""time_coverage": {"from": "2022","to": "2023","granularity": "year","source": "header","columns": ["2022", "2023"]}"#),
        "{m}"
    );
    assert!(m.contains(r#""tables": ["T1"]}"#), "{m}");
    // Per-table totals with the total row's label and source row.
    assert!(
        m.contains(r#""rows": [{"row": 9,"label": "Insgesamt","level": "total","kept_in_data": "#),
        "{m}"
    );
    // The share column's "total" is an average: unknown / not_additive, not a mismatch.
    assert!(m.contains(r#""cell": "E9","row": 9"#), "{m}");
    assert!(
        m.contains(r#""status": "unknown","reason": "not_additive""#),
        "{m}"
    );
    assert!(m.contains(r#""mismatch": 0"#), "{m}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn time_coverage_from_date_and_year_columns() {
    let dir = tmp_dir("dates");
    let src = dir.join("monthly.xlsx");
    let mut wb = XWorkbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Monthly").unwrap();
    for (c, h) in ["Monat", "Wert"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    for i in 0..14u32 {
        let (y, mo) = (2023 + (i / 12), 1 + i % 12);
        ws.write_string(1 + i, 0, format!("{y}-{mo:02}")).unwrap();
        ws.write_number(1 + i, 1, 10.0 + i as f64).unwrap();
    }
    let ws = wb.add_worksheet();
    ws.set_name("Annual").unwrap();
    for (c, h) in ["Jahr", "Einwohner"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    for i in 0..5u32 {
        ws.write_number(1 + i, 0, 1990.0 + i as f64).unwrap();
        ws.write_number(1 + i, 1, 5000.0 + i as f64).unwrap();
    }
    wb.save(&src).unwrap();
    let opts = Options {
        rules_only: true,
        ..Options::default()
    };
    let out = normalize_file(&src, &dir, &opts).unwrap();
    let m = compact(&out.manifest_json);
    assert!(m.contains(r#""from": "2023-01","to": "2024-02","granularity": "month","source": "column","columns": ["Monat"]"#), "{m}");
    assert!(m.contains(r#""from": "1990","to": "1994","granularity": "year","source": "column","columns": ["Jahr"]"#), "{m}");
    // Workbook: the whole span at the finest granularity.
    assert!(m.contains(r#""time_coverage": {"from": "1990","to": "2024-02","granularity": "month","source": "column""#), "{m}");
    std::fs::remove_dir_all(&dir).ok();
}
