//! Row labels shared by side-by-side and stacked grids (the Enron Henry Hub report): a summary
//! table names the rows once (flow, pipeline); a daily grid beside it and one per month below it
//! repeat those rows without labels. The grids get copies of the summary's label columns; a grid
//! with another row layout does not. A first data row of zeros under a header of dates is data,
//! not a repeat of that header.

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, ColRole, Normalized, Options, Out, Table};
use hh_core::open;
use rust_xlsxwriter::{Format, Formula, Workbook, Worksheet};

const PIPES: [&str; 4] = ["ACADIAN", "BRIDGELINE", "MAINLINE", "NGPL"];

/// Value of pipeline `p` (0-based; receipts first, then deliveries) on day `d` of grid `g`; the
/// first pipeline flows nothing (a row of zeros right under the header).
fn flow(g: u32, p: usize, d: u32) -> f64 {
    if p.is_multiple_of(PIPES.len()) {
        return 0.0;
    }
    let v = (1000 * (p as u32 + 1) + 37 * d + 501 * g) as f64;
    if p >= PIPES.len() { -v } else { v }
}

/// A grid of `days` date columns at (`top`, `left`): a header of dates, then receipts, a total
/// row, deliveries and a total row, the same rows as the summary.
fn grid(ws: &mut Worksheet, g: u32, top: u32, left: u16, days: u32, date: &Format) {
    for d in 0..days {
        let col = left + d as u16;
        ws.write_number_with_format(top, col, 37257.0 + 31.0 * g as f64 + d as f64, date).unwrap();
        let mut row = top + 1;
        for block in 0..2 {
            let first = row;
            for k in 0..PIPES.len() {
                ws.write_number(row, col, flow(g, block * PIPES.len() + k, d)).unwrap();
                row += 1;
            }
            let sum: f64 = (0..PIPES.len()).map(|k| flow(g, block * PIPES.len() + k, d)).sum();
            let letter = rust_xlsxwriter::utility::column_number_to_name(col);
            let f = format!("=SUM({letter}{}:{letter}{})", first + 1, row);
            ws.write_formula(row, col, Formula::new(f).set_result(sum.to_string())).unwrap();
            row += 1;
        }
    }
}

fn fixture() -> Normalized {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_row_labels_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Flows").unwrap();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    // Summary A1:D11: flow (once per block), pipeline, capacity, average; a total after each block.
    for (c, h) in ["Flow", "Pipeline", "Capacity", "Average"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    let mut row = 1;
    for (block, name) in ["Receipts", "Deliveries"].iter().enumerate() {
        ws.write_string(row, 0, *name).unwrap();
        for (k, p) in PIPES.iter().enumerate() {
            ws.write_string(row, 1, *p).unwrap();
            ws.write_number(row, 2, 50000.0 * (k + 1) as f64).unwrap();
            ws.write_number(row, 3, flow(0, block * PIPES.len() + k, 0)).unwrap();
            row += 1;
        }
        ws.write_string(row, 1, "Total").unwrap();
        ws.write_number(row, 2, 500000.0).unwrap();
        ws.write_number(row, 3, (0..PIPES.len()).map(|k| flow(0, block * PIPES.len() + k, 0)).sum::<f64>()).unwrap();
        row += 1;
    }
    // Beside it (G..K, same rows) and below it (C.., three blank rows apart): daily grids.
    grid(ws, 0, 0, 6, 5, &date);
    grid(ws, 1, 14, 2, 5, &date);
    grid(ws, 2, 28, 2, 5, &date);
    // A grid with fewer rows (no deliveries block) keeps its plain numbers.
    for d in 0..5u16 {
        ws.write_number_with_format(42, 2 + d, 37400.0 + d as f64, &date).unwrap();
        for k in 0..6u32 {
            ws.write_number(43 + k, 2 + d, (100 * k + d as u32 + 1) as f64).unwrap();
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    normalize_workbook(&book, &Options { rules_only: true, ..Options::default() })
}

fn names(t: &Table) -> Vec<&str> {
    t.columns.iter().map(|c| c.name.as_str()).collect()
}

fn text(t: &Table, col: &str) -> Vec<Option<String>> {
    let c = t.columns.iter().find(|c| c.name == col).unwrap_or_else(|| panic!("no {col} in {:?}", names(t)));
    c.values
        .iter()
        .map(|v| match v {
            Out::Text(s) => Some(s.clone()),
            Out::Null => None,
            other => panic!("{col}: {other:?}"),
        })
        .collect()
}

#[test]
fn grids_beside_and_below_get_the_summary_row_labels() {
    let n = fixture();
    let ranges: Vec<String> = n.tables.iter().map(|t| format!("{:?}", t.rect)).collect();
    assert_eq!(n.tables.len(), 5, "{ranges:?}");
    let summary = &n.tables[0];
    assert_eq!(&names(summary)[..2], ["Flow", "Pipeline"]);
    let pipes: Vec<Option<String>> = PIPES.iter().chain(&PIPES).map(|p| Some(p.to_string())).collect();
    let flows: Vec<Option<String>> =
        ["Receipts"; 4].iter().chain(&["Deliveries"; 4]).map(|f| Some(f.to_string())).collect();
    assert_eq!(text(summary, "Pipeline"), pipes);
    assert_eq!(text(summary, "Flow"), flows);
    // The side-by-side grid, then the two monthly grids below: every data row is there (the zeros
    // under the date header too) and carries its pipeline and flow.
    for (k, g) in [(1, 0u32), (2, 1), (3, 2)] {
        let t = &n.tables[k];
        assert_eq!(&names(t)[..2], ["Flow", "Pipeline"], "T{}", t.n);
        assert_eq!(t.src_rows.len(), 8, "T{} rows {:?}", t.n, t.src_rows);
        assert_eq!(text(t, "Pipeline"), pipes, "T{}", t.n);
        assert_eq!(text(t, "Flow"), flows, "T{}", t.n);
        let copied = t.columns.iter().take(2).all(|c| c.source_col.is_none());
        assert!(copied, "T{}: copied columns have no source column", t.n);
        assert!(t.warnings.iter().any(|w| w.contains("copied from T1")), "T{}: {:?}", t.n, t.warnings);
        // The values line up: NGPL deliveries on the second day.
        let day2 = &t.columns[3];
        assert_eq!(day2.values[7], Out::Num(flow(g, 7, 1)), "T{}", t.n);
    }
    // Other row layout: no labels.
    let other = &n.tables[4];
    assert!(other.columns.iter().all(|c| c.source_col.is_some() || c.role != ColRole::Data), "{:?}", names(other));
    assert!(other.warnings.iter().all(|w| !w.contains("copied from")));
    // No data row ended up in Notes as a "repeated header".
    assert!(n.notes.iter().all(|x| x.kind != "repeated_header"), "{:?}", n.notes);
}

/// HiTab (NSF, StatCan): an outline of row labels by indentation, parents with their own values
/// (`All institutions` > `Top 10` > `PCORI`; `Diagnosed mood or anxiety disorder` > `Mood
/// disorder`). Each row gets its parent's label in `<label>_parent`, right after the label column;
/// a group header (no values) starts afresh and stays in the group column.
#[test]
fn indented_row_labels_get_their_parent() {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_outline_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let ind = |n: u8| Format::new().set_indent(n);
    ws.write_string(0, 0, "Table 8. Federal R&D obligations, by institution").unwrap();
    for (c, h) in ["Institution", "2016", "2017"].iter().enumerate() {
        ws.write_string(2, c as u16, *h).unwrap();
    }
    let rows: [(&str, u8, bool); 9] = [
        ("All institutions", 0, true),
        ("Top 10 nonprofit institutions", 1, true),
        ("Patient-Centered Outcomes Research Institute", 2, true),
        ("Mayo Clinic", 2, true),
        ("All other nonprofit institutions", 1, true),
        ("By region", 0, false),
        ("Northeast", 1, true),
        ("South", 1, true),
        ("West", 1, true),
    ];
    for (k, (label, level, values)) in rows.iter().enumerate() {
        let r = 3 + k as u32;
        ws.write_string_with_format(r, 0, *label, &ind(*level)).unwrap();
        if *values {
            ws.write_number(r, 1, 100.0 + k as f64).unwrap();
            ws.write_number(r, 2, 200.0 + k as f64).unwrap();
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let n = normalize_workbook(&book, &Options::default());
    let t = &n.tables[0];
    let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
    let at = names.iter().position(|&c| c == "Institution_parent").unwrap_or_else(|| panic!("{names:?}"));
    assert_eq!(names[at - 1], "Institution");
    assert_eq!(t.columns[at].derived_kind(), Some("parent"));
    let text = |c: usize, r: usize| match &t.columns[c].values[r] {
        Out::Text(s) => s.clone(),
        Out::Null => String::new(),
        v => format!("{v:?}"),
    };
    let row = |label: &str| (0..t.rows()).find(|&r| text(at - 1, r) == label).unwrap_or_else(|| panic!("{label}"));
    assert_eq!(text(at, row("All institutions")), "");
    assert_eq!(text(at, row("Top 10 nonprofit institutions")), "All institutions");
    assert_eq!(text(at, row("Mayo Clinic")), "Top 10 nonprofit institutions");
    assert_eq!(text(at, row("All other nonprofit institutions")), "All institutions");
    // Under the group header `By region` the rows have no parent row; the group column has it.
    assert_eq!(text(at, row("South")), "");
    let g = t.columns.iter().position(|c| c.role == ColRole::Group).expect("group column");
    assert_eq!(text(g, row("South")), "By region");
}

/// Labels at one indent level get no parent column.
#[test]
fn flat_row_labels_get_no_parent_column() {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_flat_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    for (c, h) in ["Region", "2016", "2017"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    for k in 0..5u32 {
        ws.write_string(1 + k, 0, format!("Region {k}")).unwrap();
        ws.write_number(1 + k, 1, k as f64).unwrap();
        ws.write_number(1 + k, 2, 2.0 * k as f64).unwrap();
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let n = normalize_workbook(&book, &Options::default());
    assert!(n.tables[0].columns.iter().all(|c| !c.name.ends_with("_parent")));
}

/// HiTab statcan/1004: the first section label sits in the header's last row, beside the unit row
/// (`Region of ancestry | percent`), the next ones are group-header rows (`Country of ancestry`).
/// The first one heads its rows in the group column like the others and leaves the label
/// column's name.
#[test]
fn first_section_label_in_the_header_row() {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_section_hdr_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "Table 2.2: Percentage with a diploma, by region or country of ancestry").unwrap();
    ws.write_string(2, 0, "Region or country of ancestry").unwrap();
    ws.write_string(2, 1, "Women").unwrap();
    ws.write_string(2, 2, "Men").unwrap();
    ws.write_string(3, 0, "Region of ancestry").unwrap();
    ws.merge_range(3, 1, 3, 2, "percent", &Format::new()).unwrap();
    let mut r = 4;
    for (sec, items) in [(None, ["Caribbean", "Africa", "Other regions"]), (Some("Country of ancestry"), ["Jamaica", "Haiti", "Nigeria"])] {
        if let Some(s) = sec {
            ws.write_string(r, 0, s).unwrap();
            r += 1;
        }
        for (k, it) in items.iter().enumerate() {
            ws.write_string(r, 0, *it).unwrap();
            ws.write_number(r, 1, 50.0 + k as f64).unwrap();
            ws.write_number(r, 2, 40.0 + k as f64).unwrap();
            r += 1;
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let n = normalize_workbook(&book, &Options::default());
    let t = &n.tables[0];
    let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"Region or country of ancestry"), "{names:?}");
    assert!(names.contains(&"Women / percent"), "{names:?}");
    let g = t.columns.iter().position(|c| c.role == ColRole::Group).expect("group column");
    let l = names.iter().position(|&c| c == "Region or country of ancestry").unwrap();
    let text = |c: usize, r: usize| match &t.columns[c].values[r] {
        Out::Text(s) => s.clone(),
        _ => String::new(),
    };
    let row = |label: &str| (0..t.rows()).find(|&r| text(l, r) == label).unwrap_or_else(|| panic!("{label}"));
    assert_eq!(text(g, row("Africa")), "Region of ancestry");
    assert_eq!(text(g, row("Haiti")), "Country of ancestry");
}
