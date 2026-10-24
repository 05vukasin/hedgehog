//! StatCan/1535: a second cohort repeats merged disease parents, but inherits
//! the first cohort's Crude/ASHR/confidence-interval child header rows.
use hh_core::{normalize::{normalize_workbook, ColRole, Normalized, Options, Out}, open};
use rust_xlsxwriter::{Format, Workbook};

fn fixture(rules_only: bool, variant: u8) -> Normalized {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("hh_changed_parents_{}_{}_{}_{serial}.xlsx", std::process::id(), rules_only, variant));
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
    ws.write_string_with_format(10, 0, if variant == 6 { "Men" } else { "Women" }, &header).unwrap();
    for (left, label) in [(1, "All-cause (excluding pregnancy)"), (5, "Cancer"), (9, "Digestive diseases")] {
        let right = if variant == 1 && left == 5 { left + 2 } else { left + 3 };
        let label = if variant == 3 && left == 5 { "Cancer (%)" } else if variant == 5 {
            match left { 1 => "All-cause", 5 => "Circulatory diseases", _ => label }
        } else { label };
        ws.merge_range(10, left, 10, right, label, &header).unwrap();
    }
    for (i, label) in ["Canadian-born", "Total economic class", "Skilled workers", "Business class", "Live-in caregivers"].iter().enumerate() {
        let r = 11 + i as u32 + u32::from(variant == 2);
        ws.write_string(r, 0, *label).unwrap();
        for c in 1..=12u16 { ws.write_number(r, c, 200.0 + (19 * i + c as usize) as f64).unwrap(); }
    }
    if variant == 2 {
        ws.write_string(11, 0, "Reviewed by the statistics department").unwrap();
    }
    if variant == 4 || variant == 6 {
        ws.write_string_with_format(16, 0, if variant == 6 { "Men" } else { "All adults" }, &header).unwrap();
        for (left, label) in [(1, "All-cause"), (5, "Circulatory diseases"), (9, "Digestive diseases")] {
            ws.merge_range(16, left, 16, left + 3, label, &header).unwrap();
        }
        for r in 17..=18 {
            ws.write_string(r, 0, format!("Adult class {r}")).unwrap();
            for c in 1..=12u16 { ws.write_number(r, c, 400.0 + c as f64).unwrap(); }
        }
    }
    ws.write_number(11 + u32::from(variant == 2), 2, 668.0).unwrap();
    ws.write_number(12, 2, 310.0).unwrap();
    ws.write_number(12, 5, 65.8).unwrap();
    // The actual cleaned HiTab source stores every measured value as text.
    if variant == 7 {
        for (start, count, base, step) in [(6, 4, 100.0, 17), (11, 5, 200.0, 19)] {
            for i in 0..count {
                for c in 1..=12u16 {
                    let value = match (start + i, c) {
                        (7, 2) => 301.2, (7, 5) => 59.1, (11, 2) => 668.0,
                        (12, 2) => 310.0, (12, 5) => 65.8,
                        _ => base + (step * i + u32::from(c)) as f64,
                    };
                    ws.write_string(start + i, c, format!("{value:.1}")).unwrap();
                }
            }
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() })
}

#[test]
fn changed_merged_parents_keep_cohorts_children_and_source_coordinates() {
    for rules_only in [false, true] {
        let n = fixture(rules_only, 0);
        let female = n.tables.iter().find(|t| t.src_rows.contains(&12)).unwrap();
        let row = female.src_rows.iter().position(|r| *r == 12).unwrap();
        let group = female.columns.iter().find(|c| c.role == ColRole::Group).unwrap();
        assert!(group.values.iter().all(|v| *v == Out::Text("Women".into())), "female row still has male cohort");
        assert_eq!(female.src_rows, [11, 12, 13, 14, 15]);
        assert_eq!(female.header_top, 10);
        assert_eq!(female.header_rows, 1);
        assert!(female.warnings.iter().any(|w| w.contains("child header cells inherited")));
        let ashr = female.columns.iter().find(|c| c.name == "All-cause (excluding pregnancy) / ASHR").unwrap();
        assert_eq!(ashr.values[row], Out::Num(310.0));
        assert_eq!(ashr.values[0], Out::Num(668.0));
        assert_eq!(ashr.original_header, [("All-cause (excluding pregnancy)".into(), (10, 1)), ("ASHR".into(), (3, 2))]);
        let cancer = female.columns.iter().find(|c| c.name == "Cancer / Crude").unwrap();
        assert_eq!(cancer.values[row], Out::Num(65.8));
        assert_eq!(cancer.original_header, [("Cancer".into(), (10, 5)), ("Crude".into(), (3, 5))]);
        assert!(female.columns.iter().any(|c| c.name == "Cancer / 95% confidence interval / to"));
        let male = n.tables.iter().find(|t| t.src_rows.contains(&7)).unwrap();
        let row = male.src_rows.iter().position(|r| *r == 7).unwrap();
        assert_eq!(male.columns.iter().find(|c| c.name == "All-cause / ASHR").unwrap().values[row], Out::Num(301.2));
        assert_eq!(male.columns.iter().find(|c| c.name == "Circulatory diseases / Crude").unwrap().values[row], Out::Num(59.1));
        assert_eq!(male.src_rows, [6, 7, 8, 9]);
        assert_eq!(female.columns.iter().filter(|c| c.source_col.is_some()).count(), 13);
        for c in female.columns.iter().filter(|c| c.source_col.is_some_and(|c| c > 0)) {
            let source = c.source_col.unwrap();
            for (i, value) in c.values.iter().enumerate() {
                let expected = match (i, source) { (0, 2) => 668.0, (1, 2) => 310.0, (1, 5) => 65.8,
                    _ => 200.0 + (19 * i + source as usize) as f64 };
                assert_eq!(*value, Out::Num(expected), "female source column {source}, row {i}");
            }
        }
    }
}

#[test]
fn changed_parent_geometry_does_not_inherit_unproven_children() {
    for rules_only in [false, true] {
        let n = fixture(rules_only, 1);
        assert!(!n.tables.iter().any(|t| t.columns.iter().any(|c| c.name == "Cancer / Crude")), "different merge geometry inherited child headers");
    }
}

#[test]
fn changed_parent_metadata_gap_does_not_inherit_children() {
    for rules_only in [false, true] {
        let n = fixture(rules_only, 2);
        assert!(!n.tables.iter().any(|t| t.columns.iter().any(|c| c.name == "Cancer / Crude")));
    }
}

#[test]
fn cohorts_returning_to_original_or_unchanged_parents_keep_their_context() {
    for rules_only in [false, true] {
        for variant in [4, 5, 6] {
            let n = fixture(rules_only, variant);
            let (source_row, cohort) = match variant { 4 => (17, "All adults"), 6 => (17, "Men"), _ => (12, "Women") };
            let t = n.tables.iter().find(|t| t.src_rows.contains(&source_row)).unwrap();
            let row = t.src_rows.iter().position(|r| *r == source_row).unwrap();
            assert!(t.columns.iter().any(|c| c.role == ColRole::Group && c.values[row] == Out::Text(cohort.into())));
            assert!(t.columns.iter().any(|c| c.name == "Circulatory diseases / Crude"));
            assert!(!t.columns.iter().any(|c| c.name == "Cancer / Crude"));
        }
    }
}

#[test]
fn changed_parent_unit_text_is_preserved_before_typing() {
    for rules_only in [false, true] {
        let n = fixture(rules_only, 3);
        let t = n.tables.iter().find(|t| t.src_rows.contains(&12)).unwrap();
        let row = t.src_rows.iter().position(|r| *r == 12).unwrap();
        let c = t.columns.iter().find(|c| c.name == "Cancer (%) / Crude").unwrap();
        assert_eq!(c.values[row], Out::Num(65.8));
        assert_eq!(c.original_header[0], ("Cancer (%)".into(), (10, 5)));
    }
}

#[test]
fn numeric_text_cohorts_keep_changed_headers_and_exact_values() {
    for rules_only in [false, true] {
        let expected = fixture(rules_only, 0);
        let actual = fixture(rules_only, 7);
        assert_eq!(actual.tables.len(), expected.tables.len());
        for (got, want) in actual.tables.iter().zip(&expected.tables) {
            assert_eq!(got.src_rows, want.src_rows);
            assert_eq!(got.columns.len(), want.columns.len());
            for (got, want) in got.columns.iter().zip(&want.columns) {
                assert_eq!(got.name, want.name);
                assert_eq!(got.original_header, want.original_header);
                assert_eq!(got.values, want.values);
            }
        }
    }
}
