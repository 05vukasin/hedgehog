//! DEV environment-table09: repeated complete child blocks establish unmerged price parents.
use hh_core::{normalize::{normalize_workbook, Options, Out}, open};
use rust_xlsxwriter::Workbook;

#[test]
fn unmerged_price_parents_keep_repeated_children_and_standalone_caption_separate() {
    let path = std::env::temp_dir().join(format!("hh_unmerged_prices_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "Table 4f: Average Weekly Expenditure on all Goods and on Fuel, Light and Power (£/wk)").unwrap();
    ws.write_string(3, 1, "Contemporary prices").unwrap();
    ws.write_string(3, 3, "2009 prices").unwrap();
    ws.write_string(3, 5, "% on fuel, light & power").unwrap();
    for (col, text) in [(0, "Year"), (1, "All goods (£)"), (2, "Fuel, light & power (£)"), (3, "All goods (£)"), (4, "Fuel, light & power (£)")] {
        ws.write_string(5, col, text).unwrap();
    }
    let values = [[1970.0, 28.57, 1.79, 330.02468712973, 20.6770805027027, 0.0626531326566328],
        [1971.0, 30.99, 1.85, 326.237179965517, 19.4752753448276, 0.0596966763472088],
        [1972.0, 35.06, 2.06, 345.270993096774, 20.2868866451613, 0.0587564175698802]];
    for (i, row) in values.iter().enumerate() {
        for (col, value) in row.iter().enumerate() { ws.write_number(6 + i as u32, col as u16, *value).unwrap(); }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    for rules_only in [true, false] {
        let n = normalize_workbook(&book, &Options { rules_only, keep_totals: true, ..Options::default() });
        let t = n.tables.iter().find(|t| t.columns.iter().any(|c| c.source_col == Some(1))).unwrap();
        let expected = ["Year", "Contemporary prices / All goods (£)", "Contemporary prices / Fuel, light & power (£)", "2009 prices / All goods (£)", "2009 prices / Fuel, light & power (£)", "% on fuel, light & power"];
        assert_eq!(t.src_rows, vec![6, 7, 8]);
        for (source, name) in expected.iter().enumerate() {
            let c = t.columns.iter().find(|c| c.source_col == Some(source as u32)).unwrap();
            assert_eq!(&c.name, name, "rules={rules_only}");
            assert_eq!(c.values, values.iter().map(|row| Out::Num(row[source])).collect::<Vec<_>>());
            let parent = match source { 1 | 2 => Some(("Contemporary prices", (3, 1))), 3 | 4 => Some(("2009 prices", (3, 3))), 5 => Some(("% on fuel, light & power", (3, 5))), _ => None };
            if let Some((text, cell)) = parent { assert!(c.original_header.contains(&(text.into(), cell))); }
            if source < 5 { assert!(c.original_header.iter().any(|(_, cell)| *cell == (5, source as u32))); }
            if source == 5 { assert_eq!(c.original_header, vec![("% on fuel, light & power".into(), (3, 5))]); }
        }
    }
}
