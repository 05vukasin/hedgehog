//! HiTab StatCan/521: two indented subcategories in a long, otherwise flat table.
//! Explicit hierarchy must survive even when very few rows use it.
use hh_core::{normalize::{normalize_workbook, Normalized, Options, Out}, open};
use rust_xlsxwriter::{Format, Workbook};

fn fixture(rules_only: bool, spaces: bool, child_count: usize) -> Normalized {
    let path = std::env::temp_dir().join(format!(
        "hh_sparse_outline_{}_{}_{}_{}.xlsx", std::process::id(), rules_only, spaces, child_count
    ));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    for (c, h) in ["Characteristics", "number", "%"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    // Source shape: a section, two valued peers, and indented valued children.
    ws.write_string_with_format(1, 0, "Country risk level", &Format::new().set_bold()).unwrap();
    ws.write_string(2, 0, "Low").unwrap();
    ws.write_string(3, 0, "High (WHO+IRCC-specific)").unwrap();
    for (k, label) in ["WHO", "IRCC-specific"].iter().take(child_count).enumerate() {
        let text = if spaces { format!("  {label}") } else { label.to_string() };
        let fmt = if spaces { Format::new() } else { Format::new().set_indent(1) };
        ws.write_string_with_format(4 + k as u32, 0, text, &fmt).unwrap();
    }
    let next = 4 + child_count as u32;
    // An explicit new section must stop the parent, even if its children are indented.
    ws.write_string_with_format(next, 0, "Intended region", &Format::new().set_bold()).unwrap();
    ws.write_string_with_format(next + 1, 0, "Atlantic", &Format::new().set_indent(if spaces { 0 } else { 1 })).unwrap();
    for k in 0..30u32 {
        ws.write_string(next + 2 + k, 0, format!("Region {k}")).unwrap();
    }
    for r in 2..next + 32 {
        if r == next { continue; }
        ws.write_number(r, 1, 1000.0 + r as f64).unwrap();
        ws.write_number(r, 2, 10.0 + r as f64 / 10.0).unwrap();
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() })
}

#[test]
fn sparse_explicit_outline_keeps_parent_and_section_boundary() {
    for rules in [false, true] {
        for spaces in [false, true] {
            for child_count in [1, 2] {
                let n = fixture(rules, spaces, child_count);
                let t = n.tables.iter().find(|t| t.src_rows.contains(&4)).unwrap();
                let row = t.src_rows.iter().position(|r| *r == 4).unwrap();
                let parent = t.columns.iter().find(|c| c.name == "Characteristics_parent")
                    .unwrap_or_else(|| panic!("rules={rules}, spaces={spaces}, children={child_count}: {:?}", t.columns.iter().map(|c| &c.name).collect::<Vec<_>>()));
                assert_eq!(parent.values[row], Out::Text("High (WHO+IRCC-specific)".into()));
                let after = t.src_rows.iter().position(|r| *r == 5 + child_count as u32).unwrap();
                assert_eq!(parent.values[after], Out::Null, "parent crossed a section boundary");
                let low = t.src_rows.iter().position(|r| *r == 2).unwrap();
                assert_eq!(parent.values[low], Out::Null, "parent leaked backward");
            }
        }
    }
}

#[test]
fn flat_labels_do_not_gain_outline_parents() {
    for rules in [false, true] {
        let n = fixture(rules, true, 0);
        assert!(n.tables.iter().all(|t| t.columns.iter().all(|c| c.derived_kind() != Some("parent"))));
    }
}

/// StatCan/2674: a section contains indent-one No/Yes, and just three indent-two
/// drug categories under Yes. The rest of the sheet uses indent one only.
#[test]
fn sparse_nested_outline_keeps_nearest_parent() {
    for rules_only in [false, true] {
        let path = std::env::temp_dir().join(format!("hh_sparse_nested_{}_{}.xlsx", std::process::id(), rules_only));
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        for (c, h) in ["Characteristics", "Number", "Percent"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        ws.write_string_with_format(1, 0, "Used drugs during the month preceding the survey", &Format::new().set_bold()).unwrap();
        let labels = ["No", "Yes", "Cannabis - not everyday", "Cannabis - everyday", "Other drug - at least once"];
        for (k, label) in labels.iter().enumerate() {
            let r = 2 + k as u32;
            ws.write_string_with_format(r, 0, *label, &Format::new().set_indent(if k < 2 { 1 } else { 2 })).unwrap();
            ws.write_number(r, 1, [465.0, 145.0, 80.0, 63.0, 46.0][k]).unwrap();
            ws.write_number(r, 2, [17.0, 71.0, 52.0, 144.0, 235.0][k]).unwrap();
        }
        ws.write_string_with_format(7, 0, "Alcohol consumption", &Format::new().set_bold()).unwrap();
        for k in 0..40u32 {
            ws.write_string_with_format(8 + k, 0, format!("Category {k}"), &Format::new().set_indent(1)).unwrap();
            ws.write_number(8 + k, 1, 347.0 + k as f64).unwrap();
            ws.write_number(8 + k, 2, 16.0 + k as f64).unwrap();
        }
        wb.save(&path).unwrap();
        let book = open(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
        let t = n.tables.iter().find(|t| t.src_rows.contains(&5)).unwrap();
        let parent = t.columns.iter().find(|c| c.name == "Characteristics_parent").unwrap();
        let child = t.src_rows.iter().position(|r| *r == 5).unwrap();
        assert_eq!(parent.values[child], Out::Text("Yes".into()));
        let next = t.src_rows.iter().position(|r| *r == 8).unwrap();
        assert_eq!(parent.values[next], Out::Null);
    }
}

/// Enron spin-reserve log: separately dated entries occasionally have one padding
/// space in the free-text check column. That does not make the preceding entry a parent.
#[test]
fn single_padding_space_does_not_create_sparse_log_ancestry() {
    for rules_only in [false, true] {
        let path = std::env::temp_dir().join(format!("hh_sparse_log_{}_{}.xlsx", std::process::id(), rules_only));
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        for (c, h) in ["Date", "Actual 4 minute increment that you checked", "Person", "Period"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        for k in 0..30u32 {
            ws.write_number_with_format(k + 1, 0, 36625.0 + k as f64, &Format::new().set_num_format("yyyy-mm-dd")).unwrap();
            ws.write_string(k + 1, 1, if k == 15 { " ~23:25 just a minute before the check".to_string() } else { format!("Independent check {k}") }).unwrap();
            ws.write_string(k + 1, 2, "Brian").unwrap();
            ws.write_number(k + 1, 3, 4.0 + k as f64).unwrap();
        }
        wb.save(&path).unwrap();
        let book = open(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
        assert!(n.tables.iter().all(|t| t.columns.iter().all(|c| c.derived_kind() != Some("parent"))), "padding became hierarchy");
    }
}

/// MiMoTable 气体灭火: page subtotals are centered with leading padding,
/// rather than nested below the preceding bill-of-quantities item code.
#[test]
fn aligned_total_padding_is_not_text_indentation() {
    use rust_xlsxwriter::FormatAlign;
    for rules_only in [false, true] {
        for alignment in [FormatAlign::Center, FormatAlign::Right] {
            let path = std::env::temp_dir().join(format!("hh_aligned_padding_{}_{}_{alignment:?}.xlsx", std::process::id(), rules_only));
            let mut wb = Workbook::new();
            let ws = wb.add_worksheet();
            for (c, h) in ["返回", "定额名称", "工程量"].iter().enumerate() {
                ws.write_string(0, c as u16, *h).unwrap();
            }
            for k in 1..=35u32 {
                if k == 26 || k == 35 {
                    let label = if k == 26 { "              本        页        小        计" } else { "              总                            计" };
                    ws.write_string_with_format(k, 0, label, &Format::new().set_align(alignment)).unwrap();
                    ws.write_number(k, 2, 0.0).unwrap();
                } else {
                    ws.write_string(k, 0, format!("11-{k}")).unwrap();
                    ws.write_string(k, 1, format!("Pipe item {k}")).unwrap();
                    ws.write_number(k, 2, k as f64).unwrap();
                }
            }
            wb.save(&path).unwrap();
            let book = open(&path).unwrap();
            std::fs::remove_file(path).unwrap();
            let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
            assert!(n.tables.iter().all(|t| t.columns.iter().all(|c| c.derived_kind() != Some("parent"))), "aligned padding became ancestry: {alignment:?}");
        }
    }
}
