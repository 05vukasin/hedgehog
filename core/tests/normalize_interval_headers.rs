//! StatCan DEV 46/504: a scale and bounds band below an explicit merged interval header.
use hh_core::{normalize::{normalize_workbook, Options, Out}, open};
use rust_xlsxwriter::{Format, Workbook};

#[test]
fn scale_and_interval_bounds_retain_source_header_provenance() {
    for nested in [false, true] {
        let path = std::env::temp_dir().join(format!("hh_interval_{nested}_{}.xlsx", std::process::id()));
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        ws.write_string(0, 0, "Table 1: Number and percentage by selected characteristics").unwrap();
        let top = 2;
        let h = if nested {
            ws.merge_range(top, 1, top, 4, "Population", &Format::new()).unwrap();
            ws.merge_range(top, 5, top, 8, "Unperceived", &Format::new()).unwrap();
            top + 1
        } else { top };
        ws.write_string(top, 0, "Sex and age group").unwrap();
        ws.write_string(h, 1, "Number").unwrap();
        ws.write_string(h, 2, "Percent").unwrap();
        ws.merge_range(h, 3, h, 4, "95% confidence interval", &Format::new()).unwrap();
        ws.write_string(h + 1, 1, "'000").unwrap();
        ws.write_string(h + 1, 3, "from").unwrap();
        ws.write_string(h + 1, 4, "to").unwrap();
        if nested {
            ws.write_string(h, 5, "Number").unwrap();
            ws.write_string(h, 6, "Percent").unwrap();
            ws.merge_range(h, 7, h, 8, "95% confidence interval", &Format::new()).unwrap();
            ws.write_string(h + 1, 5, "'000").unwrap();
            ws.write_string(h + 1, 7, "from").unwrap();
            ws.write_string(h + 1, 8, "to").unwrap();
        }
        for (i, label) in ["Total", "Men", "Women", "Other"].iter().enumerate() {
            let r = h + 2 + i as u32;
            ws.write_string(r, 0, *label).unwrap();
            for (c, v) in [979.0, 33.7, 32.3, 35.0].iter().enumerate() {
                ws.write_number(r, 1 + c as u16, v + i as f64).unwrap();
            }
            if nested {
                for (c, v) in [6307.0, 77.1, 73.8, 80.1].iter().enumerate() {
                    ws.write_number(r, 5 + c as u16, v + i as f64).unwrap();
                }
            }
        }
        wb.save(&path).unwrap();
        let book = open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        for rules_only in [true, false] {
            let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
            let t = n.tables.iter().find(|t| t.columns.iter().any(|c| c.source_col == Some(1))).unwrap();
            for (source, suffix, value) in [(1, "Number / '000", 979.0), (3, "95% confidence interval / from", 32.3), (4, "95% confidence interval / to", 35.0)] {
                let c = t.columns.iter().find(|c| c.source_col == Some(source)).unwrap();
                assert!(c.name.ends_with(suffix), "rules={rules_only} nested={nested}: {}", c.name);
                assert!(c.original_header.contains(&(if source == 1 { "'000" } else if source == 3 { "from" } else { "to" }.into(), (h + 1, source))));
                assert_eq!(c.values[0], Out::Num(value));
            }
            assert_eq!(t.src_rows, (h + 2..h + 6).collect::<Vec<_>>());
            for (source, base) in [(1, 979.0), (2, 33.7), (3, 32.3), (4, 35.0), (5, 6307.0), (6, 77.1), (7, 73.8), (8, 80.1)] {
                if source > 4 && !nested { continue; }
                let c = t.columns.iter().find(|c| c.source_col == Some(source)).unwrap();
                assert_eq!(c.values, (0..4).map(|i| Out::Num(base + i as f64)).collect::<Vec<_>>());
                if source >= 7 {
                    let bound = if source == 7 { "from" } else { "to" };
                    assert!(c.name.ends_with(&format!("95% confidence interval / {bound}")));
                    assert!(c.original_header.contains(&(bound.into(), (h + 1, source))));
                    assert!(c.original_header.contains(&("Unperceived".into(), (top, 5))));
                }
            }
        }
    }
}

#[test]
fn mixed_records_under_interval_titles_are_not_headers() {
    for merged in [false, true] {
        let path = std::env::temp_dir().join(format!("hh_interval_records_{merged}_{}.xlsx", std::process::id()));
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        ws.write_string(0, 0, "Record").unwrap();
        ws.write_string(0, 1, "Count").unwrap();
        ws.write_string(0, 2, "Percent").unwrap();
        if merged {
            ws.merge_range(0, 3, 0, 4, "95% confidence interval", &Format::new()).unwrap();
        } else {
            ws.write_string(0, 3, "Lower").unwrap();
            ws.write_string(0, 4, "Upper").unwrap();
        }
        for i in 0..6u32 {
            ws.write_string(i + 1, 0, format!("Record {i}")).unwrap();
            ws.write_number(i + 1, 1, 100.0 + i as f64).unwrap();
            ws.write_number(i + 1, 2, 20.0 + i as f64).unwrap();
            ws.write_string(i + 1, 3, "from").unwrap();
            ws.write_string(i + 1, 4, "to").unwrap();
        }
        wb.save(&path).unwrap();
        let book = open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        for rules_only in [true, false] {
            let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
            let t = n.tables.iter().find(|t| t.columns.iter().any(|c| c.source_col == Some(1))).unwrap();
            assert_eq!(t.src_rows, (1..7).collect::<Vec<_>>());
            let count = t.columns.iter().find(|c| c.source_col == Some(1)).unwrap();
            assert_eq!(count.values[0], Out::Num(100.0));
            assert!(t.columns.iter().all(|c| c.original_header.iter().all(|(_, (row, _))| *row == 0)));
        }
    }
}

/// DEV StatCan 1521: cohort -> units/interval -> bounds, then a body section.
#[test]
fn two_missing_interval_bands_keep_cohort_ownership_and_body_section() {
    let path = std::env::temp_dir().join(format!("hh_interval_two_bands_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "Table 1: Number and percentage by sex and age").unwrap();
    ws.merge_range(2, 0, 4, 0, "", &Format::new()).unwrap();
    let cohorts = ["Total", "Males", "Females"];
    let values = [[2180.8, 54.1, 52.6, 55.6], [1109.0, 53.1, 50.9, 55.2], [1071.8, 55.2, 53.2, 57.3]];
    for (group, cohort) in cohorts.iter().enumerate() {
        let c = 1 + 4 * group as u16;
        ws.merge_range(2, c, 2, c + 3, *cohort, &Format::new()).unwrap();
        ws.merge_range(3, c, 4, c, "000'", &Format::new()).unwrap();
        ws.merge_range(3, c + 1, 4, c + 1, "%", &Format::new()).unwrap();
        ws.merge_range(3, c + 2, 3, c + 3, "95% confidence interval", &Format::new()).unwrap();
        ws.write_string(4, c + 2, "from").unwrap();
        ws.write_string(4, c + 3, "to").unwrap();
        for i in 0..4u32 {
            for (offset, base) in values[group].iter().enumerate() {
                ws.write_number(6 + i, c + offset as u16, base + i as f64).unwrap();
            }
        }
    }
    ws.write_string(5, 0, "Currently sexually active").unwrap();
    ws.merge_range(5, 1, 5, 12, "", &Format::new()).unwrap();
    for (i, label) in ["Total, 15 to 24", "15 to 17", "18 to 19", "20 to 24"].iter().enumerate() {
        ws.write_string(6 + i as u32, 0, *label).unwrap();
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    for rules_only in [true, false] {
        let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
        let t = n.tables.iter().find(|t| t.columns.iter().any(|c| c.source_col == Some(1))).unwrap();
        assert_eq!(t.src_rows, (6..10).collect::<Vec<_>>());
        for (group, cohort) in cohorts.iter().enumerate() {
            for (offset, label) in ["000'", "%", "95% confidence interval / from", "95% confidence interval / to"].iter().enumerate() {
                let source = 1 + group as u32 * 4 + offset as u32;
                let c = t.columns.iter().find(|c| c.source_col == Some(source)).unwrap();
                assert_eq!(c.name, format!("{cohort} / {label}"), "rules={rules_only}");
                assert!(c.original_header.contains(&(cohort.to_string(), (2, 1 + group as u32 * 4))));
                assert_eq!(c.values, (0..4).map(|i| Out::Num(values[group][offset] + i as f64)).collect::<Vec<_>>());
                let child = if offset < 2 { *label } else if offset == 2 { "from" } else { "to" };
                assert!(c.original_header.contains(&(child.into(), (if offset < 2 { 3 } else { 4 }, source))));
            }
        }
        let section = t.columns.iter().find(|c| c.values.iter().any(|v| v == &Out::Text("Currently sexually active".into()))).unwrap();
        assert!(section.values.iter().all(|v| v == &Out::Text("Currently sexually active".into())));
        assert!(t.columns.iter().all(|c| c.original_header.iter().all(|(_, (row, _))| *row < 5)));
    }
}
