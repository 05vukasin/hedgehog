//! Bold section labels at the left edge head the plain labels below them at the same indent, also
//! after a first section that heads indented rows (HiTab 959: bold `Custody` over indented
//! `Pre-trial detention`, then bold `Community supervision` over plain `Probation`, `Other community`
//! at indent 0). A bold row after a section's plain rows that heads nothing (a total) is no parent.

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, Normalized, Options, Out, Table};
use hh_core::open;
use rust_xlsxwriter::{Format, Workbook};

fn normalize(name: &str, build: impl FnOnce(&mut rust_xlsxwriter::Worksheet)) -> Normalized {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_obold_{name}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    build(wb.add_worksheet());
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    normalize_workbook(&book, &Options { keep_totals: true, ..Options::default() })
}

fn text(v: &Out) -> String {
    match v {
        Out::Text(s) => s.clone(),
        _ => String::new(),
    }
}

/// (label, parent) of every row.
fn parents(t: &Table) -> Vec<(String, String)> {
    let label = &t.columns[0];
    let parent = t.columns.iter().find(|c| c.name == format!("{}_parent", label.name));
    (0..t.rows())
        .map(|i| (text(&label.values[i]), parent.map(|p| text(&p.values[i])).unwrap_or_default()))
        .collect()
}

#[test]
fn bold_sections_head_plain_rows_at_their_indent() {
    let n = normalize("959", |ws| {
        let bold = Format::new().set_bold();
        let ind = |k: u8| Format::new().set_indent(k);
        let bold_ind = |k: u8| Format::new().set_bold().set_indent(k);
        ws.write_string(0, 0, "Table 7: Initial entry of youth into correctional services").unwrap();
        ws.write_string(2, 0, "Type of correctional supervision").unwrap();
        ws.write_string(2, 1, "Number").unwrap();
        ws.write_string(2, 2, "Percent").unwrap();
        let rows: [(&str, Format, f64); 11] = [
            ("Custody", bold.clone(), 1435.0),
            ("Pre-trial detention", ind(1), 1369.0),
            ("Sentenced custody", bold_ind(1), 66.0),
            ("Secure custody and supervision", ind(2), 31.0),
            ("Open custody and supervision", ind(2), 35.0),
            ("Community supervision", bold.clone(), 3735.0),
            ("Probation", Format::new(), 2440.0),
            ("Deferred custody and supervision", Format::new(), 80.0),
            ("Intensive support and supervision", Format::new(), 23.0),
            ("Other community", Format::new(), 1192.0),
            ("Total correctional services", bold.clone(), 5170.0),
        ];
        for (i, (label, f, v)) in rows.iter().enumerate() {
            let r = 3 + i as u32;
            ws.write_string_with_format(r, 0, *label, f).unwrap();
            ws.write_number(r, 1, *v).unwrap();
            ws.write_number(r, 2, (100.0 * v / 5170.0).round()).unwrap();
        }
    });
    let p = parents(&n.tables[0]);
    let of = |l: &str| p.iter().find(|(x, _)| x == l).map(|(_, y)| y.clone()).unwrap_or_else(|| panic!("{l} not in {p:?}"));
    assert_eq!(of("Pre-trial detention"), "Custody", "{p:?}");
    assert_eq!(of("Probation"), "Community supervision", "{p:?}");
    assert_eq!(of("Other community"), "Community supervision", "{p:?}");
    assert_eq!(of("Community supervision"), "", "{p:?}");
    if p.iter().any(|(x, _)| x == "Total correctional services") {
        assert_eq!(of("Total correctional services"), "", "{p:?}");
    }
}

#[test]
fn a_bold_row_over_deeper_rows_only_heads_no_plain_row_at_its_indent() {
    // `Custody` (bold) over indented rows, then a plain `Other` at indent 0: no bold label of the
    // run heads a plain one at its own indent, so `Other` stays at the top.
    let n = normalize("other", |ws| {
        let bold = Format::new().set_bold();
        ws.write_string(2, 0, "Type").unwrap();
        ws.write_string(2, 1, "Number").unwrap();
        let rows: [(&str, Format, f64); 5] = [
            ("Custody", bold.clone(), 100.0),
            ("Pre-trial detention", Format::new().set_indent(1), 60.0),
            ("Sentenced custody", Format::new().set_indent(1), 40.0),
            ("Other", Format::new(), 7.0),
            ("Remand", Format::new(), 9.0),
        ];
        for (i, (label, f, v)) in rows.iter().enumerate() {
            ws.write_string_with_format(3 + i as u32, 0, *label, f).unwrap();
            ws.write_number(3 + i as u32, 1, *v).unwrap();
        }
    });
    let p = parents(&n.tables[0]);
    assert!(p.iter().any(|(x, y)| x == "Pre-trial detention" && y == "Custody"), "{p:?}");
    assert!(p.iter().any(|(x, y)| x == "Other" && y.is_empty()), "{p:?}");
}
