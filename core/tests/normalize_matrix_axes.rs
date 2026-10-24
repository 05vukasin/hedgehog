//! DEV environment-table03: lower triangular matrices put their column axis below the values.
use hh_core::{normalize::{normalize_workbook, Options, Out}, open};
use rust_xlsxwriter::{Format, Workbook};

#[test]
fn triangular_matrix_footer_labels_name_columns_without_becoming_records() {
    let path = std::env::temp_dir().join(format!("hh_matrix_axes_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let labels = ["ET", "PAR", "Ta", "VPD", "Sal", "WL"];
    let rows = [vec![Some(0.9)], vec![Some(0.53), Some(0.49)], vec![Some(0.77), Some(0.73), Some(0.71)], vec![Some(-0.09), None, Some(-0.26), Some(-0.05)], vec![Some(0.1), Some(0.06), None, Some(0.14), Some(0.21)]];
    let pvalues = [vec![Some(0.0)], vec![Some(0.0), Some(0.0)], vec![Some(0.0), Some(0.0), Some(0.0)], vec![Some(0.0), Some(0.1), Some(0.0), Some(0.0)], vec![Some(0.0), Some(0.0), Some(0.56), Some(0.0), Some(0.0)]];
    for (top, title) in [(0, "Pearson correlation coefficients"), (9, "Statistical significance (p-value)")] {
        let data = if top == 0 { &rows } else { &pvalues };
        ws.write_string(top, 0, "(a) 30-min").unwrap();
        ws.merge_range(top, 1, top, 5, title, &Format::new()).unwrap();
        for (i, row) in data.iter().enumerate() {
            ws.write_string(top + 1 + i as u32, 0, labels[i + 1]).unwrap();
            for (j, value) in row.iter().enumerate() {
                if let Some(v) = value { ws.write_number(top + 1 + i as u32, 1 + j as u16, *v).unwrap(); }
                else { ws.write_string(top + 1 + i as u32, 1 + j as u16, "N.S.").unwrap(); }
            }
        }
        for (j, label) in labels[..5].iter().enumerate() { ws.write_string(top + 6, 1 + j as u16, *label).unwrap(); }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    for rules_only in [true, false] {
        let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
        assert_eq!(n.tables.len(), 2);
        for (t, (top, title)) in n.tables.iter().zip([(0, "Pearson correlation coefficients"), (9, "Statistical significance (p-value)")]) {
            let data = if top == 0 { &rows } else { &pvalues };
            assert_eq!(t.src_rows, (top + 1..top + 6).collect::<Vec<_>>());
            assert_eq!(t.header_rows, 1);
            assert_eq!(t.transforms.get(hh_core::normalize::T::BlankRowsRemoved), 0);
            let stub = t.columns.iter().find(|c| c.source_col == Some(0)).unwrap();
            assert_eq!(stub.values, labels[1..].iter().map(|s| Out::Text((*s).into())).collect::<Vec<_>>());
            for j in 0..5 {
                let c = t.columns.iter().find(|c| c.source_col == Some(1 + j as u32)).unwrap();
                assert_eq!(c.name, format!("{title} / {}", labels[j]));
                assert!(c.original_header.contains(&(title.into(), (top, 1))));
                assert!(c.original_header.contains(&(labels[j].into(), (top + 6, 1 + j as u32))));
                for (i, row) in data.iter().enumerate() {
                    if j <= i {
                        assert_eq!(c.values[i], row[j].map_or_else(|| Out::Text("N.S.".into()), Out::Num));
                    } else { assert_eq!(c.values[i], Out::Null); }
                }
            }
        }
    }
}
