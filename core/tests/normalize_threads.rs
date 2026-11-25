//! `NormalizeOptions::threads` bounds every fan-out inside a sheet, not only the sheet-level one
//! (issue #7): with `Some(1)` no thread is started, and the result is the same as with the
//! default (B68). The sheet is generated large enough to cross every inner threshold: totals
//! beside the layout (100k cells), the model's first passes and stream chunks (20k cells),
//! column typing (200k values in one table).

use hh_core::{Cell, NormalizeOptions, Normalized, Sheet, Value, Workbook, normalize_workbook};

fn cell(row: u32, col: u32, value: Value) -> Cell {
    Cell {
        row,
        col,
        value: Some(value),
        formula: None,
    }
}

/// A title, then a table of `rows` × 12 columns (a label, a date-like and ten number columns,
/// some cells text) with a total row, then a second, smaller table with its own total.
fn sheet(name: &str, rows: u32) -> Sheet {
    let mut cells = vec![cell(
        0,
        0,
        Value::Text(format!("{name}: figures by account")),
    )];
    let header = |cells: &mut Vec<Cell>, r: u32| {
        cells.push(cell(r, 0, Value::Text("Account".into())));
        cells.push(cell(r, 1, Value::Text("Date".into())));
        for c in 2..12 {
            cells.push(cell(r, c, Value::Text(format!("Q{c}"))));
        }
    };
    let body = |cells: &mut Vec<Cell>, top: u32, n: u32| {
        for i in 0..n {
            let r = top + i;
            cells.push(cell(r, 0, Value::Text(format!("Account {i}"))));
            cells.push(cell(r, 1, Value::DateTime(40_000.0 + f64::from(i % 3_000))));
            for c in 2..12 {
                let v = if (i + c).is_multiple_of(97) {
                    Value::Text("n/a".into())
                } else {
                    Value::Number(f64::from((i * 31 + c * 7) % 1_009) + 0.5)
                };
                cells.push(cell(r, c, v));
            }
        }
        cells.push(cell(top + n, 0, Value::Text("Total".into())));
        for c in 2..12 {
            cells.push(cell(top + n, c, Value::Number(1.0)));
        }
    };
    header(&mut cells, 2);
    body(&mut cells, 3, rows);
    let second = rows + 6;
    header(&mut cells, second);
    body(&mut cells, second + 1, 200);
    Sheet {
        name: name.into(),
        cells,
        ..Default::default()
    }
}

fn run(wb: &Workbook, threads: Option<usize>, rules_only: bool) -> (Normalized, usize) {
    let opts = NormalizeOptions {
        threads,
        rules_only,
        ..Default::default()
    };
    let before = hh_core::inner_threads_spawned();
    let out = normalize_workbook(wb, &opts);
    (out, hh_core::inner_threads_spawned() - before)
}

#[test]
fn threads_bound_the_inner_fan_outs() {
    // ~220k cells, 200k+ values in the first table.
    let wb = Workbook {
        sheets: vec![sheet("Big", 18_000), sheet("Small", 300)],
    };
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    for rules_only in [false, true] {
        let (one, spawned) = run(&wb, Some(1), rules_only);
        assert_eq!(
            spawned, 0,
            "threads: Some(1) started {spawned} threads (rules_only {rules_only})"
        );
        assert!(!one.tables.is_empty());
        let (default, spawned) = run(&wb, None, rules_only);
        if cpus > 1 {
            assert!(
                spawned > 0,
                "the default should fan out on {cpus} CPUs (rules_only {rules_only})"
            );
        }
        assert!(
            default == one,
            "threads None and Some(1) differ (rules_only {rules_only})"
        );
        let (two, _) = run(&wb, Some(2), rules_only);
        assert!(
            two == one,
            "threads Some(2) and Some(1) differ (rules_only {rules_only})"
        );
    }
}
