//! Side-by-side copies of one table with no blank column between them (a long list of states
//! printed in two halves, `State | Amount | Percent | State | Amount | Percent`): each copy is a
//! table of its own, so every value sits in the row of its own label. Kept as one table, the right
//! half became `State (2)`, `Amount (2)` columns and Ohio's numbers sat in the row labelled
//! Florida (HiTab 163_81_tab2). A comparison (`original | new` with the same items on each row)
//! stays one table (the halves
//! adjacent, as in the states table).

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, Normalized, Options, Out, Table};
use hh_core::open;
use rust_xlsxwriter::Workbook;

fn normalize(name: &str, build: impl FnOnce(&mut rust_xlsxwriter::Worksheet)) -> Normalized {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_sbs_{name}_{}.xlsx", std::process::id()));
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

fn col<'a>(t: &'a Table, name: &str) -> &'a [Out] {
    &t.columns.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no column {name:?} in {:?}", names(t))).values
}

/// The row of `label` in column `key`: the value of column `value` (None: no such row).
fn lookup(n: &Normalized, key: &str, label: &str, value: &str) -> Option<f64> {
    n.tables.iter().find_map(|t| {
        if !t.columns.iter().any(|c| c.name == key) || !t.columns.iter().any(|c| c.name == value) {
            return None;
        }
        let (k, v) = (col(t, key), col(t, value));
        (0..t.rows()).find_map(|i| match (&k[i], &v[i]) {
            (Out::Text(s), Out::Num(x)) if s == label => Some(*x),
            _ => None,
        })
    })
}

const LEFT: [&str; 8] = ["Alabama", "Alaska", "Arizona", "Arkansas", "California", "Colorado", "Connecticut", "Florida"];
const RIGHT: [&str; 8] = ["Missouri", "Montana", "Nebraska", "Nevada", "New Jersey", "New York", "North Dakota", "Ohio"];

fn states(ws: &mut rust_xlsxwriter::Worksheet, two_level: bool) {
    ws.write_string(0, 0, "Table 2. State agency expenditures for R&D, by state and performer").unwrap();
    ws.write_string(1, 0, "(Thousands of current dollars)").unwrap();
    let h = if two_level { 3 } else { 2 };
    for half in 0..2u16 {
        let c = half * 6;
        if two_level {
            ws.write_string(2, c + 1, "All R&D expenditures").unwrap();
            ws.write_string(2, c + 2, "Intramural performers").unwrap();
            ws.write_string(2, c + 4, "Extramural performers").unwrap();
            // HiTab 208_99: 'State' on the upper header row, the lower one starts at the amounts
            ws.write_string(2, c, "State").unwrap();
        }
        let one = ["State", "All R&D", "Intramural", "Intramural %", "Extramural", "Extramural %"];
        let two = ["", "", "Amount", "Percent", "Amount", "Percent"];
        for (k, s) in (if two_level { two } else { one }).iter().enumerate() {
            if !s.is_empty() {
                ws.write_string(h, c + k as u16, *s).unwrap();
            }
        }
        let labels = if half == 0 { LEFT } else { RIGHT };
        for (i, s) in labels.iter().enumerate() {
            let r = h + 1 + i as u32;
            let base = 1000.0 * (1 + i + 10 * half as usize) as f64 + 17.0;
            let intra = (base * 0.3).round();
            ws.write_string(r, c, *s).unwrap();
            ws.write_number(r, c + 1, base).unwrap();
            ws.write_number(r, c + 2, intra).unwrap();
            ws.write_number(r, c + 3, (1000.0 * intra / base).round() / 10.0).unwrap();
            ws.write_number(r, c + 4, base - intra).unwrap();
            ws.write_number(r, c + 5, 100.0 - (1000.0 * intra / base).round() / 10.0).unwrap();
        }
    }
}

#[test]
fn side_by_side_copies_are_separate_tables() {
    for two_level in [true, false] {
        let n = normalize(&format!("states{two_level}"), |ws| states(ws, two_level));
        let all = if two_level { "All R&D expenditures" } else { "All R&D" };
        // Ohio (the 8th state of the right half) has its own value, not Florida's row.
        assert_eq!(lookup(&n, "State", "Ohio", all), Some(18017.0), "{:?}", n.tables.iter().map(names).collect::<Vec<_>>());
        assert_eq!(lookup(&n, "State", "Florida", all), Some(8017.0));
        assert_eq!(lookup(&n, "State", "Missouri", all), Some(11017.0));
        for t in &n.tables {
            assert!(!names(t).iter().any(|c| c.ends_with("(2)")), "{:?}", names(t));
        }
        assert_eq!(n.tables.len(), 2, "{:?}", n.tables.iter().map(names).collect::<Vec<_>>());
    }
}

#[test]
fn a_comparison_of_the_same_items_stays_one_table() {
    let n = normalize("compare", |ws| {
        ws.write_string(0, 0, "original vs new").unwrap();
        for half in 0..2u16 {
            for (k, h) in ["ITEM", "BATCH", "QTY", "UNIT PRICE", "TOTAL"].iter().enumerate() {
                ws.write_string(1, half * 5 + k as u16, *h).unwrap();
            }
        }
        ws.write_string(1, 10, "D/P").unwrap();
        let items = ["TO TMA VEG", "PO PTT VEG", "BAN FF FRUIT", "APP PP FRUIT", "PI PIA FRUIT", "TE TEE FOOD"];
        for (i, it) in items.iter().enumerate() {
            let r = 2 + i as u32;
            let (q, p1, p2) = (10.0 * (i + 1) as f64, 5.0 + i as f64, 6.0 + i as f64);
            for (half, p) in [(0u16, p1), (1, p2)] {
                ws.write_number(r, half * 5, (i + 1) as f64).unwrap();
                ws.write_string(r, half * 5 + 1, *it).unwrap();
                ws.write_number(r, half * 5 + 2, q).unwrap();
                ws.write_number(r, half * 5 + 3, p).unwrap();
                ws.write_number(r, half * 5 + 4, q * p).unwrap();
            }
            ws.write_number(r, 10, q * (p2 - p1)).unwrap();
        }
    });
    assert_eq!(n.tables.len(), 1, "{:?}", n.tables.iter().map(names).collect::<Vec<_>>());
}

#[test]
fn repeated_value_headers_over_one_label_column_stay_one_table() {
    // `Not reported | Reported` per territory over one label column, `F` for suppressed values
    // (HiTab 2768): one table, not a copy per territory.
    let n = normalize("territories", |ws| {
        ws.write_string(0, 0, "Table 8: Victimization incidents, by reporting to police and territory").unwrap();
        ws.write_string(2, 0, "Type of offence").unwrap();
        for (k, t) in ["Yukon", "Northwest Territories", "Nunavut", "All territories"].iter().enumerate() {
            ws.write_string(2, 1 + 2 * k as u16, *t).unwrap();
            ws.write_string(3, 1 + 2 * k as u16, "Not reported").unwrap();
            ws.write_string(3, 2 + 2 * k as u16, "Reported").unwrap();
        }
        let rows = ["Total victimization", "Sexual assault", "Robbery", "Physical assault", "Break and enter", "Vandalism", "Theft"];
        for (i, r) in rows.iter().enumerate() {
            let row = 4 + i as u32;
            ws.write_string(row, 0, *r).unwrap();
            for c in 1..9u16 {
                if (i + c as usize) % 3 == 0 {
                    ws.write_number(row, c, (30 + i * 5 + c as usize) as f64).unwrap();
                } else {
                    ws.write_string(row, c, "F").unwrap();
                }
            }
        }
    });
    assert_eq!(n.tables.len(), 1, "{:?}", n.tables.iter().map(names).collect::<Vec<_>>());
}

#[test]
fn merged_titles_units_and_footnotes_do_not_disable_legacy_state_copies() {
    let n = normalize("states_shared_notes", |ws| {
        states(ws, true);
        ws.merge_range(0, 0, 0, 11, "Table 2. State agency expenditures for R&D, by state and performer", &rust_xlsxwriter::Format::new()).unwrap();
        ws.merge_range(1, 0, 1, 11, "(Thousands of current dollars)", &rust_xlsxwriter::Format::new()).unwrap();
        ws.merge_range(13, 0, 13, 11, "a Intramural performers include employees within the same state department", &rust_xlsxwriter::Format::new()).unwrap();
        ws.merge_range(14, 0, 14, 11, "SOURCE: National Science Foundation", &rust_xlsxwriter::Format::new()).unwrap();
    });
    assert_eq!(n.tables.len(), 2);
    assert_eq!(lookup(&n, "State", "Ohio", "All R&D expenditures"), Some(18017.0));
    assert_eq!(lookup(&n, "State", "Florida", "All R&D expenditures"), Some(8017.0));
    for t in &n.tables {
        assert!(names(t).iter().all(|name| !name.ends_with("(2)")), "{:?}", names(t));
    }
}
