//! Outlines of row labels (HiTab: StatCan, NSF): a row's full path of parent labels, from
//! indentation and from bold labels that head the plain labels below them. Every ancestor is a
//! label of the sheet's own rows, never invented.

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, ColRole, Options, Out, Table};
use hh_core::open;
use rust_xlsxwriter::{Format, Workbook};

/// One body row: label, indent, bold, with values.
type Row<'a> = (&'a str, u8, bool, bool);

/// A sheet with a title, a header `Region | Workers | Hours` and the given body rows; its first table.
fn table(name: &str, rows: &[Row]) -> Table {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_outline_{name}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "Table 1. Workers by region").unwrap();
    for (c, h) in ["Region", "Workers", "Hours"].iter().enumerate() {
        ws.write_string(2, c as u16, *h).unwrap();
    }
    for (k, (label, indent, bold, values)) in rows.iter().enumerate() {
        let r = 3 + k as u32;
        let mut f = Format::new().set_indent(*indent);
        if *bold {
            f = f.set_bold();
        }
        ws.write_string_with_format(r, 0, *label, &f).unwrap();
        if *values {
            let nf = if *bold { Format::new().set_bold() } else { Format::new() };
            ws.write_number_with_format(r, 1, 150.5 + 3.7 * k as f64, &nf).unwrap();
            ws.write_number_with_format(r, 2, 40.25 + 1.5 * k as f64, &nf).unwrap();
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let n = normalize_workbook(&book, &Options { keep_totals: true, ..Options::default() });
    n.tables.into_iter().next().expect("a table")
}

fn names(t: &Table) -> Vec<&str> {
    t.columns.iter().map(|c| c.name.as_str()).collect()
}

fn cell(t: &Table, col: &str, label: &str) -> String {
    let at = |name: &str| t.columns.iter().position(|c| c.name == name).unwrap_or_else(|| panic!("no {name} in {:?}", names(t)));
    let (l, c) = (at("Region"), at(col));
    let text = |c: usize, r: usize| match &t.columns[c].values[r] {
        Out::Text(s) => s.clone(),
        Out::Null => String::new(),
        v => format!("{v:?}"),
    };
    let r = (0..t.rows()).find(|&r| text(l, r) == label).unwrap_or_else(|| panic!("no row {label}"));
    text(c, r)
}

/// Three indent levels: each row gets its whole path (`Region_parent`, `Region_parent_2`).
#[test]
fn indented_outline_keeps_the_full_path() {
    let t = table(
        "indent",
        &[
            ("All institutions", 0, false, true),
            ("Top 10 nonprofit institutions", 1, false, true),
            ("Patient-Centered Outcomes Research Institute", 2, false, true),
            ("Mayo Clinic", 2, false, true),
            ("All other nonprofit institutions", 1, false, true),
        ],
    );
    assert_eq!(&names(&t)[..3], ["Region", "Region_parent", "Region_parent_2"], "{:?}", names(&t));
    for c in &t.columns[1..3] {
        assert_eq!(c.derived_kind(), Some("parent"), "{}", c.name);
        assert_eq!(c.role, ColRole::Data);
    }
    assert_eq!(cell(&t, "Region_parent", "Mayo Clinic"), "Top 10 nonprofit institutions");
    assert_eq!(cell(&t, "Region_parent_2", "Mayo Clinic"), "All institutions");
    assert_eq!(cell(&t, "Region_parent", "All other nonprofit institutions"), "All institutions");
    assert_eq!(cell(&t, "Region_parent_2", "All other nonprofit institutions"), "");
    assert_eq!(cell(&t, "Region_parent", "All institutions"), "");
}

/// StatCan 1719: a bold `Canada` with values heads the plain rows at its own indent; the
/// indented provinces are under their region and under Canada.
#[test]
fn bold_label_heads_the_plain_labels_below_it() {
    let t = table(
        "bold",
        &[
            ("Canada", 0, true, true),
            ("Atlantic provinces", 0, false, true),
            ("Nova Scotia", 1, false, true),
            ("New Brunswick", 1, false, true),
            ("Quebec", 0, false, true),
            ("Ontario", 0, false, true),
        ],
    );
    assert_eq!(cell(&t, "Region_parent", "Ontario"), "Canada");
    assert_eq!(cell(&t, "Region_parent", "Atlantic provinces"), "Canada");
    assert_eq!(cell(&t, "Region_parent", "Nova Scotia"), "Atlantic provinces");
    assert_eq!(cell(&t, "Region_parent_2", "Nova Scotia"), "Canada");
    assert_eq!(cell(&t, "Region_parent", "Canada"), "");
}

/// StatCan 1901: the same labels under several bold headings (`Total population`, `Females`)
/// are told apart by their parent.
#[test]
fn repeated_labels_under_bold_headings() {
    let t = table(
        "blocks",
        &[
            ("Total population", 0, true, true),
            ("Management occupations", 0, false, true),
            ("Health occupations", 0, false, true),
            ("Females", 0, true, true),
            ("Management occupations", 0, false, true),
            ("Health occupations", 0, false, true),
        ],
    );
    let p = t.columns.iter().position(|c| c.name == "Region_parent").unwrap_or_else(|| panic!("{:?}", names(&t)));
    let parents: Vec<String> = t.columns[p]
        .values
        .iter()
        .map(|v| match v {
            Out::Text(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(parents, ["", "Total population", "Total population", "", "Females", "Females"]);
}

/// A bold row after plain rows at its indent closes them (a total): it heads nothing, and the
/// table gets no parent column.
#[test]
fn bold_total_after_its_rows_heads_nothing() {
    let t = table(
        "total",
        &[
            ("North", 0, false, true),
            ("South", 0, false, true),
            ("Subtotal regions", 0, true, true),
            ("East", 0, false, true),
            ("West", 0, false, true),
            ("All regions", 0, true, true),
        ],
    );
    assert!(t.columns.iter().all(|c| !c.name.contains("_parent")), "{:?}", names(&t));
}

/// NSF 180_86: a short table (three rows) still gets its outline.
#[test]
fn short_table_outline() {
    let t = table(
        "short",
        &[("All industries", 0, false, true), ("Manufacturing industries", 1, false, true), ("Nonmanufacturing industries", 1, false, true)],
    );
    assert_eq!(cell(&t, "Region_parent", "Manufacturing industries"), "All industries");
    assert_eq!(cell(&t, "Region_parent", "Nonmanufacturing industries"), "All industries");
}

fn group(t: &Table, label: &str) -> String {
    let g = t.columns.iter().position(|c| c.role == ColRole::Group).unwrap_or_else(|| panic!("no group in {:?}", names(t)));
    let l = t.columns.iter().position(|c| c.name == "Region").unwrap();
    let text = |c: usize, r: usize| match &t.columns[c].values[r] {
        Out::Text(s) => s.clone(),
        _ => String::new(),
    };
    let r = (0..t.rows()).find(|&r| text(l, r) == label).unwrap_or_else(|| panic!("no row {label}"));
    text(g, r)
}

/// StatCan 2090: bold year labels alone on their rows (`2015`, `2016`) over plain rows are
/// sections: group labels, not data rows without values.
#[test]
fn bold_label_alone_on_its_row_is_a_section() {
    let t = table(
        "years",
        &[
            ("Average", 0, true, false),
            ("Exports", 0, false, true),
            ("Imports", 0, false, true),
            ("Spring", 0, true, false),
            ("Exports", 0, false, true),
            ("Imports", 0, false, true),
            ("Autumn", 0, true, false),
            ("Exports", 0, false, true),
            ("Imports", 0, false, true),
        ],
    );
    assert_eq!(t.rows(), 6, "{:?}", t.src_rows);
    let g: Vec<String> = (0..t.rows())
        .map(|r| match &t.columns.iter().find(|c| c.role == ColRole::Group).unwrap().values[r] {
            Out::Text(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(g, ["Average", "Average", "Spring", "Spring", "Autumn", "Autumn"]);
}

/// StatCan 914: section labels without values at two indents (`Public spaces` > `Unwanted …`):
/// the group path nests by indent, and the next top section replaces both. Each level is a
/// column of its own (`group`, `group_parent`).
#[test]
fn indented_sections_nest() {
    let t = table(
        "sections",
        &[
            ("Public spaces", 0, true, false),
            ("Unwanted behaviour in public spaces", 1, true, false),
            ("Unwanted physical contact", 2, false, true),
            ("Indecent exposure", 2, false, true),
            ("Online", 0, true, false),
            ("Unwanted behaviour online", 1, true, false),
            ("Threatening emails", 2, false, true),
            ("Pressure to send images", 2, false, true),
            ("Protective measure taken online", 1, true, false),
            ("Changed username", 2, false, true),
            ("Limited Internet use", 2, false, true),
        ],
    );
    assert_eq!(t.rows(), 6, "{:?}", t.src_rows);
    let all = |label: &str| {
        let mut v: Vec<String> = t
            .columns
            .iter()
            .filter(|c| c.role == ColRole::Group || c.derived_kind() == Some("parent"))
            .map(|c| {
                let l = t.columns.iter().position(|c| c.name == "Region").unwrap();
                let r = (0..t.rows()).find(|&r| t.columns[l].values[r] == Out::Text(label.into())).unwrap();
                match &c.values[r] {
                    Out::Text(s) => s.clone(),
                    _ => String::new(),
                }
            })
            .filter(|s| !s.is_empty())
            .collect();
        v.sort();
        v
    };
    assert_eq!(all("Indecent exposure").join(" | "), "Public spaces | Unwanted behaviour in public spaces");
    assert_eq!(all("Pressure to send images").join(" | "), "Online | Unwanted behaviour online");
    assert_eq!(all("Changed username").join(" | "), "Online | Protective measure taken online");
    // The group column holds the innermost section, `group_parent` the one around it.
    assert_eq!(group(&t, "Changed username"), "Protective measure taken online");
    let gp = t.columns.iter().position(|c| c.name == "group_parent").unwrap_or_else(|| panic!("{:?}", names(&t)));
    assert_eq!(t.columns[gp].derived_kind(), Some("parent"));
}

/// Enron: consecutive group rows at one indent, then an indented one, all before the first
/// data row (`Segment Information:` > `Net sales:` > `  Beers`): each level is kept.
#[test]
fn consecutive_sections_at_one_indent_nest() {
    let t = table(
        "consecutive",
        &[
            ("Segment information:", 0, true, false),
            ("Net sales:", 0, true, false),
            ("Beers and spirits", 1, true, false),
            ("Imported beers", 2, false, true),
            ("Spirits", 2, false, true),
            ("Wines", 1, true, false),
            ("Branded wine", 2, false, true),
            ("Wholesale", 2, false, true),
        ],
    );
    assert_eq!(group(&t, "Branded wine"), "Wines");
    assert_eq!(cell(&t, "group_parent", "Branded wine"), "Net sales:");
    assert_eq!(cell(&t, "group_parent_2", "Branded wine"), "Segment information:");
    assert_eq!(group(&t, "Spirits"), "Beers and spirits");
}

/// BLS injuries: a ` no data` placeholder alone on its row is no section and no label.
#[test]
fn missing_value_token_is_no_section() {
    let t = table(
        "nodata",
        &[
            ("Employee status", 0, true, false),
            ("Wage and salary workers", 0, false, true),
            ("Self-employed", 0, false, true),
            (" no data", 0, false, false),
            ("Gender", 0, true, false),
            ("Men", 0, false, true),
            ("Women", 0, false, true),
        ],
    );
    assert_eq!(group(&t, "Men"), "Gender");
    assert!(t.columns.iter().all(|c| c.values.iter().all(|v| *v != Out::Text("no data".into()))), "{:?}", names(&t));
}

/// MiMoTable 1237(2): a group label with no row of its own below it (the last line of the
/// table) is not lost: it goes to Notes.
#[test]
fn group_label_heading_nothing_goes_to_notes() {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_outline_lonely_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let bold = Format::new().set_bold();
    for (c, h) in ["Item", "Budget", "Spent"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    let rows = [("Office", true), ("Paper", false), ("Pens", false), ("Travel", true), ("Train", false), ("Hotel", false), ("Pending items", true)];
    for (k, (label, group)) in rows.iter().enumerate() {
        let r = 1 + k as u32;
        if *group {
            ws.write_string_with_format(r, 0, *label, &bold).unwrap();
        } else {
            ws.write_string(r, 0, *label).unwrap();
            ws.write_number(r, 1, 120.5 + k as f64).unwrap();
            ws.write_number(r, 2, 80.25 + k as f64).unwrap();
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let n = normalize_workbook(&book, &Options::default());
    let t = &n.tables[0];
    let in_table = t.columns.iter().any(|c| c.values.contains(&Out::Text("Pending items".into())));
    let in_notes = n.notes.iter().any(|x| x.text == "Pending items");
    assert!(in_table || in_notes, "lost: {:?}", n.notes);
}
