//! DEV health-table01: four metric blocks, each printed as independent highest/least lists.
use hh_core::{normalize::{normalize_workbook, Options, Out}, open};
use rust_xlsxwriter::{Format, Workbook};

#[test]
fn stacked_ranked_lists_keep_metric_rank_and_each_state_value_pair() {
    let path = std::env::temp_dir().join(format!("hh_stacked_ranked_{}.xlsx", std::process::id()));
    let blocks = [
        (4, "Total Cases", "Cases", ["Maharashtra", "Kerala", "Karnataka", "Tamil Nadu", "Andhra Pradesh"], [6623344.0, 5055224.0, 2991614.0, 2714025.0, 2069770.0], ["Andaman Nicobar", "Lakshadweep", "Daman and Diu", "Ladakh", "Nagaland"], [7670.0, 10365.0, 10682.0, 21148.0, 31978.0]),
        (13, "Recovered", "Recovered %", ["Daman and Diu", "Lakshadweep", "Arunachal Pradesh", "Andhra Pradesh", "Rajasthan"], [99.96, 99.51, 99.42, 99.15, 99.05], ["Mizoram", "Punjab", "Nagaland", "Maharashtra", "Manipur"], [95.25, 97.2, 97.33, 97.64, 97.79]),
        (21, "Death", "Death %", ["Punjab", "Nagaland", "Uttarakhand", "Maharashtra", "Goa"], [2.75, 2.16, 2.15, 2.12, 1.89], ["Daman and Diu", "Mizoram", "Lakshadweep", "Arunachal Pradesh", "Telengana"], [0.04, 0.36, 0.49, 0.51, 0.59]),
        (30, "Active", "Active %", ["Mizoram", "Kerala", "Ladakh", "Manipur", "Telengana"], [4.39, 1.37, 0.73, 0.64, 0.56], ["Lakshadweep", "Daman and Diu", "Bihar", "MP", "Rajasthan"], [0.0, 0.0, 0.0, 0.01, 0.01]),
    ];
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    for (top, metric, measure, high, hv, low, lv) in &blocks {
        ws.merge_range(*top, 0, *top, 3, *metric, &Format::new()).unwrap();
        for (col, rank, states, values) in [(0u16, "Highest", high, hv), (2, "Least", low, lv)] {
            ws.write_string(top + 1, col, rank).unwrap();
            ws.write_string(top + 2, col, "States").unwrap();
            ws.write_string(top + 2, col + 1, *measure).unwrap();
            for i in 0..5 {
                ws.write_string(top + 3 + i as u32, col, states[i]).unwrap();
                ws.write_number(top + 3 + i as u32, col + 1, values[i]).unwrap();
            }
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    for rules_only in [true, false] {
        let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
        assert_eq!(n.tables.len(), 8, "rules={rules_only}");
        for (top, metric, measure, high, hv, low, lv) in &blocks {
            for (col, rank, states, values) in [(0u32, "Highest", high, hv), (2, "Least", low, lv)] {
                let rows: Vec<_> = (top + 3..top + 8).collect();
                let t = n.tables.iter().find(|t| t.src_rows == rows && t.columns.iter().any(|c| c.source_col == Some(col))).unwrap();
                let key = t.columns.iter().find(|c| c.source_col == Some(col)).unwrap();
                let value = t.columns.iter().find(|c| c.source_col == Some(col + 1)).unwrap();
                assert_eq!(key.name, format!("{metric} / {rank} / States"));
                assert_eq!(value.name, format!("{metric} / {rank} / {measure}"));
                assert_eq!(key.values, states.iter().map(|s| Out::Text((*s).into())).collect::<Vec<_>>());
                assert_eq!(value.values, values.iter().copied().map(Out::Num).collect::<Vec<_>>());
                for c in [key, value] {
                    assert!(c.original_header.contains(&(metric.to_string(), (*top, 0))));
                    assert!(c.original_header.contains(&(rank.into(), (top + 1, col))));
                    assert!(c.original_header.iter().all(|(_, (r, _))| (*top..top + 3).contains(r)));
                }
                assert!(key.original_header.contains(&("States".into(), (top + 2, col))));
                assert!(value.original_header.contains(&(measure.to_string(), (top + 2, col + 1))));
                assert_eq!(t.columns.iter().filter(|c| c.source_col.is_some()).count(), 2);
            }
        }
    }
}
