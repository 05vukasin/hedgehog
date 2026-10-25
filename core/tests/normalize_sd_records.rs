//! DEV StatCan 632: compound SD measurements are body records, not replacement headers.
use hh_core::{normalize::{normalize_workbook, Dtype, Options, Out}, open};
use rust_xlsxwriter::{Format, Workbook};

const ROWS: &[(u32, [&str; 5])] = &[
    (4, ["Total", "24,900", "100", "32,400", "100"]),
    (5, ["Average age (years)", "40.3 (SD 12.9", "...", "39.7 (SD 12.6", "..."]),
    (6, ["Died during follow-up", "4,000", "16", "3,800", "12"]),
    (7, ["Marital status", "", "", "", ""]),
    (8, ["Single", "5,400", "22", "5,100", "16"]),
    (9, ["Married/Common-law", "17,000", "69", "22,200", "68"]),
    (10, ["Divorced/Widowed", "2,400", "10", "5,100", "16"]),
    (11, ["Income quintile", "", "", "", ""]),
    (12, ["1 (lowest)", "9,800", "40", "13,700", "42"]),
    (13, ["2", "6,400", "26", "8,100", "25"]),
    (14, ["3", "4,300", "17", "5,400", "17"]),
    (15, ["4", "2,900", "12", "3,400", "11"]),
    (16, ["5 (highest)", "1,400", "6", "1,700", "5"]),
    (17, ["Educational attainment", "", "", "", ""]),
    (18, ["Less than secondary school graduation", "14,800", "60", "18,200", "56"]),
    (19, ["Secondary school graduation", "8,000", "32", "9,200", "28"]),
    (20, ["Some postsecondary", "1,700", "7", "4,100", "13"]),
    (21, ["University degree or more", "400", "2", "900", "3"]),
    (22, ["Labour force status", "", "", "", ""]),
    (23, ["Employed", "12,500", "50", "13,100", "40"]),
    (24, ["Unemployed", "5,300", "21", "3,500", "11"]),
    (25, ["Not in labour force", "7,100", "29", "15,800", "49"]),
    (26, ["Dwelling in need of major repair", "", "", "", ""]),
    (27, ["No", "16,300", "65", "21,700", "67"]),
    (28, ["Yes", "8,600", "35", "10,700", "33"]),
    (29, ["Move distance", "", "", "", ""]),
    (30, ["Non-mover", "12,400", "50", "15,800", "49"]),
    (31, ["Within Census Subdivision/Census Division", "7,400", "30", "9,900", "31"]),
    (32, ["Across Census Division/Province", "5,100", "20", "6,600", "20"]),
    (33, ["Metropolitan Influence Zone (MIZ)", "", "", "", ""]),
    (34, ["Census Metropoitan Area (CMA)", "5,800", "23", "8,600", "26"]),
    (35, ["Moderate/Strong", "4,200", "17", "5,300", "16"]),
    (36, ["Weak", "8,600", "35", "1,110", "34"]),
    (37, ["No", "6,300", "25", "7,400", "23"]),
    (38, ["1991 Community Well-Being Index (CWB) score", "", "", "", ""]),
    (39, ["Above average for First Nations communities", "13,300", "54", "18,200", "56"]),
    (40, ["Below average for First Nations communities", "11,500", "46", "14,200", "44"]),
    (41, ["Income component", "", "", "", ""]),
    (42, ["Above average for First Nations communities", "14,400", "54", "18,100", "59"]),
    (43, ["Below average for First Nations communities", "11,500", "46", "13,300", "41"]),
    (44, ["Education component", "", "", "", ""]),
    (45, ["Above average for First Nations communities", "13,800", "56", "19,100", "59"]),
    (46, ["Below average for First Nations communities", "11,000", "44", "13,300", "41"]),
    (47, ["Labour force activity component", "", "", "", ""]),
    (48, ["Above average for First Nations communities", "13,700", "55", "18,800", "58"]),
    (49, ["Below average for First Nations communities", "11,100", "45", "13,600", "42"]),
    (50, ["Housing component", "", "", "", ""]),
    (51, ["Above average for First Nations communities", "13,700", "55", "18,600", "57"]),
    (52, ["Below average for First Nations communities", "11,100", "45", "13,800", "43"]),
    (53, ["Average 1991 CWB score", "", "", "", ""]),
    (54, ["Total", "55 (SD 13", "...", "57 (SD 14", "..."]),
    (55, ["Income component", "55 (SD 17", "...", "57 (SD 17", "..."]),
    (56, ["Education component", "27 (SD 13", "...", "28 (SD 14", "..."]),
    (57, ["Labour force component", "69 (SD 12", "...", "70 (SD 12", "..."]),
    (58, ["Housing component", "70 (SD 17", "...", "71 (SD 18", "..."]),
];

#[test]
fn compound_sd_records_preserve_complete_cohort_table() {
    let path = std::env::temp_dir().join(format!("hh_sd_records_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.merge_range(0, 0, 0, 4, "Characteristics of First Nations adults by sex", &Format::new()).unwrap();
    ws.merge_range(2, 0, 3, 0, "", &Format::new()).unwrap();
    ws.merge_range(2, 1, 2, 2, "Men", &Format::new()).unwrap();
    ws.merge_range(2, 3, 2, 4, "Women", &Format::new()).unwrap();
    for (col, label) in [(1, "Number"), (2, "Percent"), (3, "Number"), (4, "Percent")] {
        ws.write_string(3, col, label).unwrap();
    }
    for &(row, values) in ROWS {
        for (col, value) in values.iter().enumerate() {
            if !value.is_empty() { ws.write_string(row, col as u16, *value).unwrap(); }
        }
        if values[1].is_empty() { ws.merge_range(row, 1, row, 4, "", &Format::new()).unwrap(); }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let records: Vec<_> = ROWS.iter().filter(|(_, v)| !v[1].is_empty()).collect();
    assert_eq!(records.len(), 42);
    for rules_only in [true, false] {
        let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
        assert_eq!(n.tables.len(), 1, "rules={rules_only}");
        let t = &n.tables[0];
        assert_eq!((t.rect.top, t.rect.bottom, t.rect.left, t.rect.right), (2, 58, 0, 4));
        assert_eq!((t.header_top, t.header_rows), (2, 2));
        assert_eq!(t.src_rows, records.iter().map(|(r, _)| *r).collect::<Vec<_>>());
        let stub = t.columns.iter().find(|c| c.source_col == Some(0)).unwrap();
        assert!(stub.original_header.is_empty());
        assert_eq!(stub.values, records.iter().map(|(_, v)| Out::Text(v[0].into())).collect::<Vec<_>>());
        for col in 1..5usize {
            let c = t.columns.iter().find(|c| c.source_col == Some(col as u32)).unwrap();
            let parent = if col < 3 { "Men" } else { "Women" };
            let leaf = if col % 2 == 1 { "Number" } else { "Percent" };
            assert_eq!(c.name, format!("{parent} / {leaf}"));
            assert_eq!(c.original_header, vec![(parent.into(), (2, if col < 3 { 1 } else { 3 })), (leaf.into(), (3, col as u32))]);
            if col % 2 == 1 { assert_eq!(c.dtype, Dtype::Text); }
            else { assert!(matches!(c.dtype, Dtype::Integer | Dtype::Number | Dtype::Percent)); }
            let expected: Vec<_> = records.iter().map(|(_, v)| {
                if col % 2 == 1 { Out::Text(v[col].into()) }
                else if v[col] == "..." { Out::Null }
                else { Out::Num(v[col].parse().unwrap()) }
            }).collect();
            assert_eq!(c.values, expected, "rules={rules_only}, col={col}");
        }
        let mut group = "";
        let mut index = 0;
        for &(_, values) in ROWS {
            if values[1].is_empty() { group = values[0]; }
            else {
                if !group.is_empty() {
                    assert!(t.columns.iter().any(|c| c.is_derived() && c.values[index] == Out::Text(group.into())), "missing group {group} at {index}, rules={rules_only}");
                }
                index += 1;
            }
        }
    }
}
