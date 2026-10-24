//! Total rows the normalized output must keep apart from the data (M1 of eval/accuracy): totals in
//! sheets whose numbers are stored as text (HiTab, many statistics offices: "2,117,353"), long
//! labels that start with the total word ("Total Criminal Code traffic violations"), and "All …"
//! rows over or under the rows they add up ("All HBCUs", "All industries"); an "All …" row whose
//! numbers do not add up stays data.

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, Normalized, Options, Out, RowType, Table};
use hh_core::open;
use rust_xlsxwriter::{Workbook, Worksheet};

fn build(name: &str, fill: impl Fn(&mut Worksheet)) -> Vec<Normalized> {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_totals_text_{name}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    fill(ws);
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    // Both analyzers: the rules alone and rules + the learned row model.
    [true, false]
        .into_iter()
        .map(|rules_only| normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() }))
        .collect()
}

/// Rows of `t` as (first label, row type).
fn rows(t: &Table) -> Vec<(String, RowType)> {
    let labels = &t.columns.iter().find(|c| c.values.iter().any(|v| matches!(v, Out::Text(_)))).unwrap().values;
    labels
        .iter()
        .zip(&t.row_types)
        .map(|(v, rt)| (if let Out::Text(s) = v { s.clone() } else { String::new() }, *rt))
        .collect()
}

fn row_type(t: &Table, label: &str) -> RowType {
    rows(t).into_iter().find(|(l, _)| l == label).unwrap_or_else(|| panic!("no row {label}: {:?}", rows(t))).1
}

fn write_rows(ws: &mut Worksheet, top: u32, rows: &[(&str, &[&str])]) {
    for (k, (label, vals)) in rows.iter().enumerate() {
        let r = top + k as u32;
        ws.write_string(r, 0, *label).unwrap();
        for (c, v) in vals.iter().enumerate() {
            ws.write_string(r, 1 + c as u16, *v).unwrap();
        }
    }
}

#[test]
fn totals_of_numbers_stored_as_text() {
    for n in build("text", |ws| {
        ws.write_string(0, 0, "Table 2: Police-reported crime, selected offences").unwrap();
        write_rows(ws, 2, &[("Type of offence", &["2017", "2018", "2019"])]);
        write_rows(
            ws,
            3,
            &[
                ("Total violent crime", &["1,500", "2,250", "3,375"]),
                ("Homicide", &["600", "1,200", "1,800"]),
                ("Robbery", &["900", "1,050", "1,575"]),
                ("Total Criminal Code traffic violations", &["1,300", "2,500", "3,700"]),
                ("Alcohol-impaired driving", &["500", "1,100", "1,600"]),
                ("Other Criminal Code traffic violations", &["800", "1,400", "2,100"]),
                ("Total other federal statute violations", &["710", "1,320", "1,930"]),
                ("Youth Criminal Justice Act", &["310", "620", "930"]),
                ("Other federal statutes", &["400", "700", "1,000"]),
                ("Breaking and entering", &["120", "130", "140"]),
            ],
        );
    }) {
        let t = &n.tables[0];
        for label in ["Total violent crime", "Total Criminal Code traffic violations", "Total other federal statute violations"] {
            assert_ne!(row_type(t, label), RowType::Data, "{label}: {:?}", rows(t));
        }
        for label in ["Homicide", "Robbery", "Alcohol-impaired driving", "Youth Criminal Justice Act", "Breaking and entering"] {
            assert_eq!(row_type(t, label), RowType::Data, "{label}: {:?}", rows(t));
        }
    }
}

#[test]
fn all_rows_that_add_up_are_totals() {
    for n in build("all", |ws| {
        ws.write_string(0, 0, "TABLE 2. Federal obligations to colleges, by agency: FY 2016").unwrap();
        write_rows(ws, 2, &[("Institution", &["All agencies", "DOD", "DOE", "Share (%)"])]);
        write_rows(
            ws,
            3,
            &[
                ("All HBCUs", &["37,297.8", "1,995.6", "582.4", "41.3"]),
                ("North Carolina A&T", &["12,608.5", "230.9", "200.0", "12.5"]),
                ("Morehouse School of Medicine", &["14,542.8", "1,000.2", "182.4", "17.1"]),
                ("Jackson State U.", &["10,146.5", "764.5", "200.0", "11.9"]),
            ],
        );
        // A second block: an "All …" row where a single column adds up by chance stays data; so
        // does "All other …" (a remainder category), even where it adds up.
        write_rows(ws, 9, &[("Region", &["Firms", "Workers", "Sales"])]);
        write_rows(
            ws,
            10,
            &[
                ("North", &["120", "4,500", "310.5"]),
                ("South", &["80", "2,100", "220.25"]),
                ("All ages program", &["200", "7,000", "990.5"]),
                ("All other regions", &["400", "13,600", "1,521.25"]),
            ],
        );
    }) {
        let all = n.tables.iter().find(|t| rows(t).iter().any(|r| r.0 == "All HBCUs")).expect("table with All HBCUs");
        assert_ne!(row_type(all, "All HBCUs"), RowType::Data, "{:?}", rows(all));
        for label in ["North Carolina A&T", "Morehouse School of Medicine", "Jackson State U."] {
            assert_eq!(row_type(all, label), RowType::Data, "{label}: {:?}", rows(all));
        }
        let other = n.tables.iter().find(|t| rows(t).iter().any(|r| r.0 == "North")).expect("second table");
        for label in ["North", "South", "All ages program", "All other regions"] {
            assert_eq!(row_type(other, label), RowType::Data, "{label}: {:?}", rows(other));
        }
    }
}

#[test]
fn a_section_total_keeps_its_section_label() {
    for n in build("section", |ws| {
        ws.write_string(0, 0, "TABLE 4. U.S. R&D expenditures, by type of work: Selected years").unwrap();
        write_rows(ws, 2, &[("Type of work", &["2000", "2010", "2012"])]);
        ws.write_string(3, 0, "Current $billions").unwrap();
        write_rows(
            ws,
            4,
            &[
                ("All R&D", &["267.9", "406.6", "433.6"]),
                ("Basic research", &["42", "76", "73.4"]),
                ("Applied research", &["56.5", "79.2", "87"]),
                ("Experimental development", &["169.4", "251.4", "273.2"]),
            ],
        );
        ws.write_string(8, 0, "Percent distribution").unwrap();
        write_rows(
            ws,
            9,
            &[
                ("All R&D", &["100", "100", "100"]),
                ("Basic research", &["15.7", "18.7", "16.9"]),
                ("Applied research", &["21.1", "19.5", "20.1"]),
                ("Experimental development", &["63.2", "61.8", "63"]),
            ],
        );
    }) {
        let t = &n.tables[0];
        assert_eq!(t.row_types.len(), 8, "{:?}", rows(t));
        assert_ne!(t.row_types[0], RowType::Data, "{:?}", rows(t));
        // The group column names the section of every row, the total rows' too.
        let group = t.columns.iter().find(|c| c.values.iter().any(|v| *v == Out::Text("Percent distribution".into())));
        let group = group.unwrap_or_else(|| panic!("no group column: {:?}", t.columns.iter().map(|c| &c.name).collect::<Vec<_>>()));
        assert_eq!(group.values[0], Out::Text("Current $billions".into()), "{:?}", group.values);
        assert_eq!(group.values[4], Out::Text("Percent distribution".into()), "{:?}", group.values);
    }
}

#[test]
fn a_kept_subtotal_keeps_its_parent() {
    for n in build("parent", |ws| {
        ws.write_string(0, 0, "Table 2. Higher education R&D expenditures, by source of funds").unwrap();
        write_rows(ws, 2, &[("Source of funds", &["2011", "2012"])]);
        write_rows(
            ws,
            3,
            &[
                ("All R&D expenditures", &["65,274", "65,729"]),
                ("  All federal R&D expenditures", &["40,768", "40,142"]),
                ("    DOD", &["14,814", "14,908"]),
                ("    HHS", &["25,954", "25,234"]),
                ("  All nonfederal R&D expenditures", &["24,506", "25,587"]),
                ("    Institution funds", &["12,580", "13,587"]),
                ("    Business", &["11,926", "12,000"]),
            ],
        );
    }) {
        let t = &n.tables[0];
        let parent = t.columns.iter().find(|c| c.name.ends_with("_parent"));
        let parent = parent.unwrap_or_else(|| panic!("no parent column: {:?}", t.columns.iter().map(|c| &c.name).collect::<Vec<_>>()));
        for (j, (label, rt)) in rows(t).into_iter().enumerate() {
            if label.contains("All federal") || label.contains("All nonfederal") {
                assert_ne!(rt, RowType::Data, "{label}: {:?}", rows(t));
                assert_eq!(parent.values[j], Out::Text("All R&D expenditures".into()), "{label}: {:?}", parent.values);
            }
        }
    }
}

#[test]
fn a_group_named_like_a_total_names_its_total_row() {
    // The learned row model reads the label-only "Total drug violations" row as a group header (the
    // rules alone read it as a total row of its own): the model path, as the CLI runs it.
    for n in build("named", |ws| {
        ws.write_string(0, 0, "Table 1: Police-reported drug offences, Canada").unwrap();
        write_rows(ws, 2, &[("Violation", &["2012", "", "2013", ""])]);
        write_rows(ws, 3, &[("", &["number", "rate", "number", "rate"])]);
        write_rows(
            ws,
            4,
            &[
                ("Cannabis violations", &[]),
                ("Possession", &["57,895", "167", "58,965", "168"]),
                ("Trafficking", &["9,773", "28", "9,110", "26"]),
                ("Total cannabis violations", &["67,668", "195", "68,075", "194"]),
                ("Cocaine violations", &[]),
                ("Possession", &["7,885", "23", "7,000", "20"]),
                ("Trafficking", &["10,358", "30", "9,500", "27"]),
                ("Total cocaine violations", &["18,243", "53", "16,500", "47"]),
                ("Precursor or equipment", &["30", "0", "36", "0"]),
                ("Total drug violations", &[]),
                ("Possession", &["65,780", "190", "65,965", "188"]),
                ("Trafficking", &["20,131", "58", "18,610", "53"]),
                ("Precursor or equipment", &["30", "0", "36", "0"]),
                ("Total", &["85,941", "248", "84,611", "241"]),
            ],
        );
    })
    .into_iter()
    .skip(1)
    {
        let t = &n.tables[0];
        let last = t.row_types.len() - 1;
        assert_ne!(t.row_types[last], RowType::Data, "{:?}", rows(t));
        let group = t.columns.iter().find(|c| c.values.iter().any(|v| *v == Out::Text("Cocaine violations".into())));
        let group = group.unwrap_or_else(|| panic!("no group column: {:?}", t.columns.iter().map(|c| &c.name).collect::<Vec<_>>()));
        assert_eq!(group.values[last], Out::Text("Total drug violations".into()), "{:?}", group.values);
    }
}
