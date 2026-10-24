//! Numbers in a header row: a column numbering (`1 2 3 4` under `Jahr | BIP | BNE | …`, so notes can
//! say "Sp. 3") does not name the columns; numbers under one label over all of them are that label's
//! values (`Number of years since landing` over `1 … 6`, HiTab 1220) and do.

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, Normalized, Options, Table};
use hh_core::open;
use rust_xlsxwriter::Workbook;

fn normalize(name: &str, build: impl FnOnce(&mut rust_xlsxwriter::Worksheet)) -> Normalized {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_hnum_{name}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    build(wb.add_worksheet());
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    normalize_workbook(&book, &Options { keep_totals: true, ..Options::default() })
}

fn names(t: &Table) -> Vec<&str> {
    t.columns.iter().map(|c| c.name.as_str()).collect()
}

#[test]
fn numbers_under_one_label_name_the_columns() {
    for merged in [false, true] {
        let n = normalize(&format!("years{merged}"), |ws| {
            ws.write_string(0, 0, "Table 4: Percentage receiving employment income, by cohort").unwrap();
            // the stub's header merged down over the numbers row and the unit row (HiTab 1220)
            ws.merge_range(2, 0, 4, 0, "Group and cohort", &rust_xlsxwriter::Format::new()).unwrap();
            let label = "Number of years since landing";
            if merged {
                ws.merge_range(2, 1, 2, 6, label, &rust_xlsxwriter::Format::new()).unwrap();
            } else {
                ws.write_string(2, 1, label).unwrap();
            }
            for k in 1..=6u16 {
                ws.write_string(3, k, k.to_string()).unwrap();
            }
            ws.write_string(4, 1, "percent").unwrap();
            let mut row = 5;
            for group in ["Refugee claimants", "Government-assisted refugees"] {
                ws.write_string(row, 0, group).unwrap();
                row += 1;
                for y in 1999..2004 {
                    ws.write_number(row, 0, y as f64).unwrap();
                    for k in 1..=6u16 {
                        ws.write_number(row, k, 50.0 + (y - 1999) as f64 + 4.0 * k as f64 + 0.1).unwrap();
                    }
                    row += 1;
                }
            }
        });
        let t = &n.tables[0];
        let cols = names(t);
        for k in 1..=6 {
            assert!(cols.iter().any(|c| c.starts_with(&format!("Number of years since landing / {k}"))), "{cols:?}");
        }
    }
}

#[test]
fn a_column_numbering_under_distinct_labels_does_not_name_the_columns() {
    let n = normalize("destatis", |ws| {
        for (j, h) in ["Jahr", "BIP", "BNE", "Saldo", "Netto"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        for j in 1..5u16 {
            ws.write_number(1, j, j as f64).unwrap();
        }
        for i in 0..6u32 {
            ws.write_number(2 + i, 0, 1991.0 + i as f64).unwrap();
            for j in 1..5u16 {
                ws.write_number(2 + i, j, 1500.25 + (i * 7 + j as u32) as f64).unwrap();
            }
        }
    });
    let cols = names(&n.tables[0]);
    assert!(cols.contains(&"BIP") && cols.contains(&"Netto"), "{cols:?}");
}
