//! DEV StatCan 550: a long merged ancestor must not erase its province/unit descendants.
use hh_core::{normalize::{normalize_workbook, Options, Out}, open};
use rust_xlsxwriter::{Format, Workbook};

#[test]
fn long_parent_budget_preserves_source_province_and_unit_paths() {
    let provinces = ["N.L.", "P.E.I.", "N.S.", "N.B.", "Que.", "Ont.", "Man.", "Sask.", "Alta.", "B.C."];
    let data = [[2.3, 1.0, 3.5, 3.2, 13.1, 100.0, 4.4, 3.1, 9.3, 9.2], [17.3, 14.4, 22.3, 20.9, 27.5, 100.0, 24.9, 21.8, 28.3, 28.5], [28.2, 18.6, 37.3, 35.3, 50.4, 100.0, 38.0, 34.6, 47.0, 45.9], [18.8, 14.7, 24.3, 22.8, 30.9, 100.0, 26.6, 23.5, 30.9, 30.9]];
    for long in [false, true] {
        // The literal slash belongs to ONE source cell; the payload includes multibyte characters.
        let parent = if long { format!("Share of own province: N ij / N i MathType@{}", "αβγδ".repeat(100)) } else { "Share of own province".into() };
        let path = std::env::temp_dir().join(format!("hh_long_parent_{long}_{}.xlsx", std::process::id()));
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        ws.merge_range(0, 0, 0, 9, "Table 3: Provincial firm networks for Ontario by sector", &Format::new()).unwrap();
        ws.merge_range(2, 0, 4, 0, "Sector", &Format::new()).unwrap();
        ws.merge_range(2, 1, 2, 10, &parent, &Format::new()).unwrap();
        ws.merge_range(4, 1, 4, 10, "percent", &Format::new()).unwrap();
        for (j, province) in provinces.iter().enumerate() { ws.write_string(3, 1 + j as u16, *province).unwrap(); }
        for (i, sector) in ["Goods only", "Services only", "Hybrid goods", "Total business sector"].iter().enumerate() {
            ws.write_string(5 + i as u32, 0, *sector).unwrap();
            for j in 0..10 { ws.write_number(5 + i as u32, 1 + j as u16, data[i][j]).unwrap(); }
        }
        wb.save(&path).unwrap();
        let book = open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        for rules_only in [true, false] {
            let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
            assert_eq!(n.tables.len(), 1);
            let t = &n.tables[0];
            assert_eq!(t.src_rows, vec![5, 6, 7, 8]);
            let stub = t.columns.iter().find(|c| c.source_col == Some(0)).unwrap();
            assert_eq!(stub.name, "Sector");
            for (j, province) in provinces.iter().enumerate() {
                let c = t.columns.iter().find(|c| c.source_col == Some(1 + j as u32)).unwrap();
                let tail = format!("{province} / percent");
                let expected = if long { format!("{}… / {tail}", parent.chars().take(255 - tail.chars().count() - 4).collect::<String>()) } else { format!("{parent} / {tail}") };
                assert_eq!(c.name, expected, "rules={rules_only}");
                assert!(c.name.chars().count() <= 255);
                assert_eq!(c.original_header, vec![(parent.clone(), (2, 1)), (province.to_string(), (3, 1 + j as u32)), ("percent".into(), (4, 1))]);
                assert_eq!(c.values, data.iter().map(|row| Out::Num(row[j])).collect::<Vec<_>>());
            }
        }
    }
}

#[test]
fn long_flat_headers_and_oversized_descendants_keep_existing_prefix_policy() {
    for hierarchical in [false, true] {
        let leaf = format!("Literal / inside one source cell {}", "Ω".repeat(300));
        let path = std::env::temp_dir().join(format!("hh_long_leaf_{hierarchical}_{}.xlsx", std::process::id()));
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        let row = u32::from(hierarchical);
        if hierarchical { ws.merge_range(0, 1, 0, 2, "Province", &Format::new()).unwrap(); }
        ws.write_string(row, 0, "Sector").unwrap();
        ws.write_string(row, 1, &leaf).unwrap();
        if hierarchical { ws.write_string(row, 2, "Other").unwrap(); }
        for i in 0..4u32 {
            ws.write_string(row + 1 + i, 0, format!("Sector {i}")).unwrap();
            ws.write_number(row + 1 + i, 1, 12.5 + i as f64).unwrap();
            if hierarchical { ws.write_number(row + 1 + i, 2, 20.5 + i as f64).unwrap(); }
        }
        wb.save(&path).unwrap();
        let book = open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        for rules_only in [true, false] {
            let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
            let c = n.tables.iter().flat_map(|t| &t.columns).find(|c| c.source_col == Some(1)).unwrap();
            let raw = if hierarchical { format!("Province / {leaf}") } else { leaf.clone() };
            assert_eq!(c.name, raw.chars().take(255).collect::<String>());
            assert!(c.original_header.contains(&(leaf.clone(), (row, 1))));
            assert_eq!(c.values, (0..4).map(|i| Out::Num(12.5 + i as f64)).collect::<Vec<_>>());
        }
    }
}
