//! StatCan/1535: a second cohort repeats merged disease parents, but inherits
//! the first cohort's Crude/ASHR/confidence-interval child header rows.
use hh_core::{normalize::{normalize_workbook, Normalized, Options, Out}, open};
use rust_xlsxwriter::{Format, Workbook};

fn fixture(rules_only: bool, changed_geometry: bool) -> Normalized {
    let path = std::env::temp_dir().join(format!("hh_changed_parents_{}_{}_{}.xlsx", std::process::id(), rules_only, changed_geometry));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let header = Format::new().set_bold();
    ws.merge_range(0, 0, 0, 9, "Hospitalization rates by sex, economic class and disease", &header).unwrap();
    ws.merge_range(2, 0, 4, 0, "Origin, sex and ECPA category", &header).unwrap();
    for (left, label) in [(1, "All-cause"), (5, "Circulatory diseases"), (9, "Digestive diseases")] {
        ws.merge_range(2, left, 2, left + 3, label, &header).unwrap();
        ws.merge_range(3, left, 4, left, "Crude", &header).unwrap();
        ws.merge_range(3, left + 1, 4, left + 1, "ASHR", &header).unwrap();
        ws.merge_range(3, left + 2, 3, left + 3, "95% confidence interval", &header).unwrap();
        ws.write_string_with_format(4, left + 2, "from", &header).unwrap();
        ws.write_string_with_format(4, left + 3, "to", &header).unwrap();
    }
    ws.write_string_with_format(5, 0, "Men", &header).unwrap();
    ws.merge_range(5, 1, 5, 12, "", &header).unwrap();
    for (i, label) in ["Canadian-born", "Total economic class", "Skilled workers", "Business class"].iter().enumerate() {
        let r = 6 + i as u32;
        ws.write_string(r, 0, *label).unwrap();
        for c in 1..=12u16 { ws.write_number(r, c, 100.0 + (17 * i + c as usize) as f64).unwrap(); }
    }
    ws.write_number(7, 2, 301.2).unwrap();
    ws.write_number(7, 5, 59.1).unwrap();
    ws.write_string_with_format(10, 0, "Women", &header).unwrap();
    for (left, label) in [(1, "All-cause (excluding pregnancy)"), (5, "Cancer"), (9, "Digestive diseases")] {
        let right = if changed_geometry && left == 5 { left + 2 } else { left + 3 };
        ws.merge_range(10, left, 10, right, label, &header).unwrap();
    }
    for (i, label) in ["Canadian-born", "Total economic class", "Skilled workers", "Business class", "Live-in caregivers"].iter().enumerate() {
        let r = 11 + i as u32;
        ws.write_string(r, 0, *label).unwrap();
        for c in 1..=12u16 { ws.write_number(r, c, 200.0 + (19 * i + c as usize) as f64).unwrap(); }
    }
    ws.write_number(11, 2, 668.0).unwrap();
    ws.write_number(12, 2, 310.0).unwrap();
    ws.write_number(12, 5, 65.8).unwrap();
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() })
}

#[test]
fn changed_merged_parents_keep_cohorts_children_and_source_coordinates() {
    for rules_only in [false, true] {
        let n = fixture(rules_only, false);
        let female = n.tables.iter().find(|t| t.src_rows.contains(&12)).unwrap();
        let row = female.src_rows.iter().position(|r| *r == 12).unwrap();
        assert!(female.columns.iter().any(|c| c.values[row] == Out::Text("Women".into())), "female row still has male cohort");
        let ashr = female.columns.iter().find(|c| c.name == "All-cause (excluding pregnancy) / ASHR").unwrap();
        assert_eq!(ashr.values[row], Out::Num(310.0));
        assert_eq!(ashr.original_header, [("All-cause (excluding pregnancy)".into(), (10, 1)), ("ASHR".into(), (3, 2))]);
        let cancer = female.columns.iter().find(|c| c.name == "Cancer / Crude").unwrap();
        assert_eq!(cancer.values[row], Out::Num(65.8));
        assert_eq!(cancer.original_header, [("Cancer".into(), (10, 5)), ("Crude".into(), (3, 5))]);
        assert!(female.columns.iter().any(|c| c.name == "Cancer / 95% confidence interval / to"));
        let male = n.tables.iter().find(|t| t.src_rows.contains(&7)).unwrap();
        let row = male.src_rows.iter().position(|r| *r == 7).unwrap();
        assert_eq!(male.columns.iter().find(|c| c.name == "All-cause / ASHR").unwrap().values[row], Out::Num(301.2));
        assert_eq!(male.columns.iter().find(|c| c.name == "Circulatory diseases / Crude").unwrap().values[row], Out::Num(59.1));
        assert!(!male.src_rows.contains(&12));
    }
}

#[test]
fn changed_parent_geometry_does_not_inherit_unproven_children() {
    for rules_only in [false, true] {
        let n = fixture(rules_only, true);
        assert!(!n.tables.iter().any(|t| t.columns.iter().any(|c| c.name == "Cancer / Crude")), "different merge geometry inherited child headers");
    }
}
