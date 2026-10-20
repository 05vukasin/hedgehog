//! Totals: detection, verification, dependency index and features on generated xlsx fixtures.
//! rust_xlsxwriter does not evaluate formulas, so every cached result is written explicitly.

use std::path::PathBuf;

use hh_core::totals::{
    self, detect, verify, CheckReason, DependencyIndex, Direction, Evidence, FormulaKind, TotalCandidate, TotalLevel,
    TotalOrigin,
};
use hh_core::{open, Cell, Features, Grid, Rect, Sheet, Value};
use rust_xlsxwriter::{Formula, Workbook, Worksheet};

fn fixture(name: &str, build: impl FnOnce(&mut Worksheet)) -> Sheet {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_totals_{}_{name}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet().set_name("Data").unwrap();
    build(ws);
    wb.save(&path).unwrap();
    let mut book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    book.sheets.remove(0)
}

fn f(ws: &mut Worksheet, row: u32, col: u16, formula: &str, result: f64) {
    ws.write_formula(row, col, Formula::new(formula).set_result(result.to_string())).unwrap();
}

/// Writes rows of (label, numbers…) starting at `row`.
fn rows(ws: &mut Worksheet, row: u32, data: &[(&str, &[f64])]) {
    for (i, (label, nums)) in data.iter().enumerate() {
        let r = row + i as u32;
        ws.write_string(r, 0, *label).unwrap();
        for (j, v) in nums.iter().enumerate() {
            ws.write_number(r, 1 + j as u16, *v).unwrap();
        }
    }
}

fn at(c: &[TotalCandidate], row: u32, col: u32) -> &TotalCandidate {
    c.iter().find(|x| (x.row, x.col) == (row, col)).unwrap_or_else(|| panic!("no total at ({row},{col}): {c:#?}"))
}

fn rect(top: u32, left: u32, bottom: u32, right: u32) -> Rect {
    Rect { top, left, bottom, right }
}

#[test]
fn formula_column_totals_and_a_wrong_cached_value() {
    let sheet = fixture("col", |ws| {
        ws.write_string(0, 0, "City").unwrap();
        ws.write_string(0, 1, "Budget").unwrap();
        ws.write_string(0, 2, "Cost").unwrap();
        rows(ws, 1, &[("Beograd", &[100.0, 10.0]), ("Novi Sad", &[200.0, 20.0]), ("Niš", &[300.5, 30.0])]);
        ws.write_string(4, 0, "Ukupno").unwrap();
        f(ws, 4, 1, "=SUM(B2:B4)", 600.5);
        f(ws, 4, 2, "=SUM(C2:C4)", 70.0); // wrong: should be 60
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 2, "{c:#?}");
    let b = at(&c, 4, 1);
    assert_eq!((b.origin, b.direction, b.level), (TotalOrigin::Formula, Direction::Vertical, TotalLevel::Total));
    assert_eq!(b.ranges, vec![rect(1, 1, 3, 1)]);
    assert_eq!(b.label, Some((4, 0)));
    assert!(b.evidence.contains(&Evidence::Formula(FormulaKind::Sum)));
    assert!(b.evidence.iter().any(|e| matches!(e, Evidence::Keyword(_))));
    let ok = verify(&grid, b);
    assert_eq!((ok.ok, ok.reason, ok.summed), (Some(true), CheckReason::Match, 3));
    let bad = verify(&grid, at(&c, 4, 2));
    assert_eq!((bad.ok, bad.reason), (Some(false), CheckReason::Mismatch));
    assert_eq!((bad.expected, bad.actual, bad.diff), (Some(60.0), Some(70.0), Some(10.0)));
}

#[test]
fn formula_row_totals_and_addition_chain() {
    let sheet = fixture("row", |ws| {
        for (j, h) in ["Item", "Q1", "Q2", "Q3", "Total"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        rows(ws, 1, &[("A", &[1.0, 2.0, 3.0]), ("B", &[4.0, 5.0, 6.0])]);
        f(ws, 1, 4, "=SUM(B2:D2)", 6.0);
        f(ws, 2, 4, "=B3+C3+D3", 15.0);
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 2, "{c:#?}");
    for t in &c {
        assert_eq!(t.direction, Direction::Horizontal);
        assert_eq!(t.label, Some((0, 4)));
        assert_eq!(verify(&grid, t).ok, Some(true));
    }
    assert!(at(&c, 2, 4).evidence.contains(&Evidence::Formula(FormulaKind::AdditionChain)));
    assert_eq!(at(&c, 2, 4).ranges.len(), 3);
}

#[test]
fn typed_total_row_with_one_wrong_number() {
    let sheet = fixture("typed", |ws| {
        for (j, h) in ["Region", "2023", "2024"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        rows(
            ws,
            1,
            &[("North", &[10.0, 11.0]), ("South", &[20.0, 21.0]), ("East", &[30.0, 31.0]), ("Total", &[60.0, 64.0])],
        );
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 2, "{c:#?}");
    let good = at(&c, 4, 1);
    assert_eq!((good.origin, good.direction), (TotalOrigin::Typed, Direction::Vertical));
    assert_eq!(good.ranges, vec![rect(1, 1, 3, 1)]);
    assert_eq!(verify(&grid, good).ok, Some(true));
    let bad = verify(&grid, at(&c, 4, 2));
    assert_eq!((bad.ok, bad.expected, bad.actual), (Some(false), Some(63.0), Some(64.0)));
}

#[test]
fn typed_total_over_missing_tokens() {
    // "n/a" / "-" in the column are missing values, not header text: the source covers all the
    // data rows and the total (which skips them) verifies.
    let sheet = fixture("typed_missing", |ws| {
        for (j, h) in ["Region", "2023", "2024"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        rows(ws, 1, &[("North", &[10.0, 11.0]), ("South", &[]), ("East", &[30.0, 31.0]), ("West", &[40.0])]);
        ws.write_string(2, 1, "n/a").unwrap();
        ws.write_string(2, 2, "-").unwrap();
        ws.write_string(4, 2, "N/A").unwrap();
        rows(ws, 5, &[("Total", &[80.0, 42.0])]);
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 2, "{c:#?}");
    for col in [1, 2] {
        let t = at(&c, 5, col);
        assert_eq!(t.ranges, vec![rect(1, col, 4, col)], "{t:#?}");
        let chk = verify(&grid, t);
        assert_eq!((chk.ok, chk.reason), (Some(true), CheckReason::Match), "{chk:#?}");
    }
}

#[test]
fn typed_total_rounded_to_integer_is_ok() {
    let sheet = fixture("rounded", |ws| {
        ws.write_string(0, 0, "Name").unwrap();
        ws.write_string(0, 1, "Amount").unwrap();
        rows(ws, 1, &[("a", &[10.4]), ("b", &[20.3]), ("Svega", &[31.0])]);
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    let chk = verify(&grid, at(&c, 3, 1));
    assert_eq!((chk.ok, chk.reason), (Some(true), CheckReason::Rounded));
}

#[test]
fn subtotals_grand_total_and_double_counting() {
    // B: grand total adds the two subtotal cells; C: grand total SUMs the whole column including
    // the subtotals (double counting); D: SUBTOTAL(9) grand over nested SUBTOTALs (Excel skips them).
    let sheet = fixture("nested", |ws| {
        for (j, h) in ["Item", "Chain", "Sum", "Subtotal"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        rows(ws, 1, &[("a1", &[1.0, 1.0, 1.0]), ("a2", &[2.0, 2.0, 2.0])]);
        ws.write_string(3, 0, "Subtotal A").unwrap();
        f(ws, 3, 1, "=SUM(B2:B3)", 3.0);
        f(ws, 3, 2, "=SUM(C2:C3)", 3.0);
        f(ws, 3, 3, "=SUBTOTAL(9,D2:D3)", 3.0);
        rows(ws, 4, &[("b1", &[3.0, 3.0, 3.0]), ("b2", &[4.0, 4.0, 4.0])]);
        ws.write_string(6, 0, "Subtotal B").unwrap();
        f(ws, 6, 1, "=SUM(B5:B6)", 7.0);
        f(ws, 6, 2, "=SUM(C5:C6)", 7.0);
        f(ws, 6, 3, "=SUBTOTAL(9,D5:D6)", 7.0);
        ws.write_string(7, 0, "Grand total").unwrap();
        f(ws, 7, 1, "=B4+B7", 10.0);
        f(ws, 7, 2, "=SUM(C2:C7)", 20.0);
        f(ws, 7, 3, "=SUBTOTAL(9,D2:D7)", 10.0);
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 9, "{c:#?}");
    for col in 1..=3 {
        assert_eq!(at(&c, 3, col).level, TotalLevel::Subtotal);
        assert_eq!(at(&c, 6, col).level, TotalLevel::Subtotal);
        assert_eq!(at(&c, 7, col).level, TotalLevel::GrandTotal);
    }
    let chain = at(&c, 7, 1);
    assert!(chain.evidence.contains(&Evidence::SumsTotals) && chain.double_counted.is_empty());
    assert_eq!(verify(&grid, chain).ok, Some(true));

    let sum = at(&c, 7, 2);
    assert_eq!(sum.double_counted, vec![(3, 2), (6, 2)]);
    let chk = verify(&grid, sum);
    assert_eq!((chk.ok, chk.reason, chk.expected, chk.actual), (Some(false), CheckReason::DoubleCounted, Some(10.0), Some(20.0)));

    let sub = at(&c, 7, 3);
    assert_eq!(sub.exclude, vec![(3, 3), (6, 3)]);
    assert!(sub.double_counted.is_empty());
    assert_eq!(verify(&grid, sub).ok, Some(true));
}

#[test]
fn typed_subtotals_and_grand_total() {
    let sheet = fixture("typed_nested", |ws| {
        ws.write_string(0, 0, "Opština").unwrap();
        ws.write_string(0, 1, "Stanovnika").unwrap();
        ws.write_string(1, 0, "Vojvodina").unwrap(); // group header row, no number
        rows(ws, 2, &[("Novi Sad", &[350.0]), ("Subotica", &[140.0]), ("Ukupno Vojvodina", &[490.0])]);
        ws.write_string(5, 0, "Šumadija").unwrap();
        rows(ws, 6, &[("Kragujevac", &[180.0]), ("Aranđelovac", &[45.0]), ("Ukupno Šumadija", &[225.0])]);
        rows(ws, 9, &[("Sveukupno", &[715.0])]);
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 3, "{c:#?}");
    let first = at(&c, 4, 1);
    assert_eq!(first.ranges, vec![rect(1, 1, 3, 1)], "from the group label row");
    let second = at(&c, 8, 1);
    assert_eq!(second.ranges, vec![rect(5, 1, 7, 1)], "group \"Šumadija\"");
    assert_eq!(second.level, TotalLevel::Subtotal);
    assert_eq!(verify(&grid, second).ok, Some(true));
    let grand = at(&c, 9, 1);
    assert_eq!(grand.level, TotalLevel::GrandTotal);
    assert_eq!(grand.exclude, vec![(8, 1), (4, 1)]);
    let chk = verify(&grid, grand);
    assert_eq!((chk.ok, chk.expected), (Some(true), Some(715.0)));
}

#[test]
fn typed_total_column_needs_arithmetic_support() {
    let sheet = fixture("typed_col", |ws| {
        for (j, h) in ["Code", "Name", "Jan", "Feb", "Mar", "Ukupno"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        for (i, (code, vals, tot)) in
            [(101.0, [1.0, 2.0, 3.0], 6.0), (102.0, [4.0, 5.0, 6.0], 15.0), (103.0, [7.0, 8.0, 9.0], 25.0)]
                .iter()
                .enumerate()
        {
            let r = 1 + i as u32;
            ws.write_number(r, 0, *code).unwrap();
            ws.write_string(r, 1, "x").unwrap();
            for (j, v) in vals.iter().enumerate() {
                ws.write_number(r, 2 + j as u16, *v).unwrap();
            }
            ws.write_number(r, 5, *tot).unwrap();
        }
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 3, "{c:#?}");
    assert!(c.iter().all(|t| t.direction == Direction::Horizontal && t.ranges[0].left == 2));
    let oks: Vec<_> = c.iter().map(|t| verify(&grid, t).ok).collect();
    assert_eq!(oks, vec![Some(true), Some(true), Some(false)]);

    // A "Total sales" measure column whose values are not sums of anything is not a total.
    let sheet = fixture("measure_col", |ws| {
        for (j, h) in ["Rep", "Region code", "Total sales"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        rows(ws, 1, &[("Ann", &[3.0, 120.0]), ("Bob", &[4.0, 90.0]), ("Cid", &[5.0, 70.0])]);
    });
    assert!(detect(&Grid::new(&sheet)).is_empty());
}

#[test]
fn numbers_as_text_are_noted_not_summed() {
    let sheet = fixture("as_text", |ws| {
        ws.write_string(0, 0, "Item").unwrap();
        ws.write_string(0, 1, "Amount").unwrap();
        rows(ws, 1, &[("a", &[10.0]), ("c", &[30.0])]);
        ws.write_string(3, 0, "b").unwrap();
        ws.write_string(3, 1, "20").unwrap();
        ws.write_string(4, 0, "Total").unwrap();
        f(ws, 4, 1, "=SUM(B2:B4)", 40.0);
        ws.write_string(5, 0, "Total (chain)").unwrap();
        f(ws, 5, 1, "=B2+B3+B4", 60.0);
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    let sum = verify(&grid, at(&c, 4, 1));
    assert_eq!((sum.ok, sum.summed, sum.numbers_as_text.clone()), (Some(true), 2, vec![(3, 1)]));
    let chain = verify(&grid, at(&c, 5, 1));
    assert_eq!((chain.ok, chain.expected), (Some(true), Some(60.0)));
}

#[test]
fn header_row_with_total_column_is_not_a_total_row() {
    let sheet = fixture("hdr", |ws| {
        for (j, h) in ["Name", "A", "B", "Total"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        rows(ws, 1, &[("x", &[1.0, 2.0, 3.0]), ("y", &[3.0, 4.0, 7.0])]);
    });
    let grid = Grid::new(&sheet);
    assert!(totals::keyword_rows(&grid).is_empty());
    let c = detect(&grid);
    assert_eq!(c.len(), 2);
    assert!(c.iter().all(|t| t.direction == Direction::Horizontal && verify(&grid, t).ok == Some(true)));
}

#[test]
fn other_sheet_and_non_sum_formulas_are_not_totals() {
    let sheet = fixture("remote", |ws| {
        ws.write_string(0, 0, "Total").unwrap();
        f(ws, 0, 1, "=SUM(Other!B2:B9)", 5.0);
        ws.write_number(1, 1, 2.0).unwrap();
        ws.write_number(2, 1, 3.0).unwrap();
        f(ws, 3, 1, "=B2*B3", 6.0);
        f(ws, 4, 1, "=AVERAGE(B2:B3)", 2.5);
        f(ws, 5, 1, "=SUM(B3)", 3.0); // single-cell link without a keyword
    });
    assert!(detect(&Grid::new(&sheet)).is_empty());
}

fn synthetic(cells: Vec<Cell>) -> Sheet {
    Sheet { name: "S".into(), cells, merges: vec![] }
}

fn num(row: u32, col: u32, v: f64) -> Cell {
    Cell { row, col, value: Some(Value::Number(v)), formula: None }
}

#[test]
fn missing_cached_values_are_unknown() {
    let sheet = synthetic(vec![
        num(0, 0, 1.0),
        num(1, 0, 2.0),
        Cell { row: 2, col: 0, value: None, formula: Some("SUM(A1:A2)".into()) },
        num(0, 1, 1.0),
        Cell { row: 1, col: 1, value: None, formula: Some("B1*2".into()) },
        Cell { row: 2, col: 1, value: Some(Value::Number(3.0)), formula: Some("SUM(B1:B2)".into()) },
        num(0, 2, 1.0),
        Cell { row: 1, col: 2, value: Some(Value::Error("#DIV/0!".into())), formula: None },
        Cell { row: 2, col: 2, value: Some(Value::Error("#DIV/0!".into())), formula: Some("SUM(C1:C2)".into()) },
    ]);
    let mut sheet = sheet;
    sheet.cells.sort_by_key(|c| (c.row, c.col));
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 3);
    assert_eq!(verify(&grid, at(&c, 2, 0)).reason, CheckReason::NoCachedValue);
    assert_eq!(verify(&grid, at(&c, 2, 1)).reason, CheckReason::MissingSourceValue);
    assert_eq!(verify(&grid, at(&c, 2, 2)).reason, CheckReason::NotNumeric);
    assert!(c.iter().all(|t| verify(&grid, t).ok.is_none()));
}

#[test]
fn dependency_index() {
    let sheet = fixture("deps", |ws| {
        ws.write_number(0, 0, 1.0).unwrap();
        ws.write_number(1, 0, 2.0).unwrap();
        ws.write_number(2, 0, 3.0).unwrap();
        f(ws, 3, 0, "=SUM(A1:A3)", 6.0);
        f(ws, 3, 1, "=A4*2+Other!C3", 12.0);
        f(ws, 4, 1, "=A2", 2.0);
    });
    let grid = Grid::new(&sheet);
    let deps = DependencyIndex::new(&grid);
    assert_eq!(deps.dependents(1, 0), vec![(3, 0), (4, 1)]);
    assert_eq!(deps.dependents(3, 0), vec![(3, 1)]);
    assert_eq!(deps.dependents(0, 5), Vec::<(u32, u32)>::new());
    let pre = deps.precedents(3, 1);
    assert_eq!(pre.len(), 2);
    assert_eq!(pre[1].sheet.as_deref(), Some("Other"));
    assert!(deps.precedents(0, 0).is_empty());
    let idx = |r, c| grid.cell_index(r, c).unwrap();
    assert!(deps.is_referenced(idx(0, 0)) && deps.referenced_by_aggregate(idx(0, 0)));
    assert!(deps.is_referenced(idx(1, 0)));
    assert!(deps.is_referenced(idx(3, 0)) && !deps.referenced_by_aggregate(idx(3, 0)));
    assert!(!deps.is_referenced(idx(4, 1)));
}

#[test]
fn feature_columns() {
    let sheet = fixture("features", |ws| {
        ws.write_string(0, 0, "Name").unwrap();
        ws.write_string(0, 1, "Amount").unwrap();
        rows(ws, 1, &[("a", &[1.0]), ("b", &[2.0])]);
        ws.write_string(3, 0, "Total").unwrap();
        f(ws, 3, 1, "=SUM(B2:B3)", 3.0);
        ws.write_string(4, 0, "Wrong total").unwrap();
        f(ws, 4, 1, "=B2+B3", 4.0);
        ws.write_number(1, 2, 5.0).unwrap();
        f(ws, 1, 3, "=C2+B2", 6.0);
    });
    let grid = Grid::new(&sheet);
    let mut feats = Features::new(sheet.cells.len());
    totals::compute(&grid, &mut feats);
    assert_eq!(feats.names(), totals::features::NAMES);
    let v = |name: &str, r, c| feats.value(name, grid.cell_index(r, c).unwrap()).unwrap();
    assert_eq!(v("t_is_formula", 3, 1), 1.0);
    assert_eq!(v("t_is_formula", 1, 1), 0.0);
    assert_eq!(v("t_is_aggregate_formula", 3, 1), 1.0);
    assert_eq!(v("t_is_aggregate_formula", 1, 3), 1.0); // `+` chain
    assert_eq!(v("t_refs_above", 3, 1), 1.0);
    assert_eq!(v("t_refs_left", 1, 3), 1.0);
    assert_eq!(v("t_refs_above", 1, 3), 0.0);
    assert_eq!(v("t_ref_count", 4, 1), 2.0);
    assert_eq!(v("t_is_referenced", 1, 1), 1.0);
    assert_eq!(v("t_is_referenced", 0, 1), 0.0);
    assert_eq!(v("t_referenced_by_aggregate", 2, 1), 1.0);
    assert_eq!(v("t_in_keyword_total_row", 3, 0), 1.0);
    assert_eq!(v("t_in_keyword_total_row", 1, 1), 0.0);
    assert_eq!(v("t_total_verified", 3, 1), 1.0);
    assert_eq!(v("t_total_verified", 4, 1), 0.0);
    assert_eq!(v("t_total_verified", 1, 1), -1.0);
}

#[test]
fn pivot_style_group_totals() {
    let sheet = fixture("pivot", |ws| {
        for (j, h) in ["Region", "City", "Item", "Amount"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        let data: [(u32, u16, &str); 9] = [
            (1, 0, "East"),
            (1, 1, "Beograd"),
            (3, 1, "Beograd Total"),
            (4, 1, "Niš"),
            (5, 1, "Niš Total"),
            (6, 0, "East Total"),
            (7, 0, "West"),
            (8, 0, "West Total"),
            (9, 0, "Grand Total"),
        ];
        for (r, c, t) in data {
            ws.write_string(r, c, t).unwrap();
        }
        for (r, v) in [(1, 1.0), (2, 2.0), (3, 3.0), (4, 4.0), (5, 4.0), (6, 7.0), (7, 5.0), (8, 6.0), (9, 13.0)] {
            ws.write_number(r, 3, v).unwrap();
        }
        ws.write_string(1, 2, "a").unwrap();
        ws.write_string(2, 2, "b").unwrap();
        ws.write_string(4, 2, "a").unwrap();
        ws.write_string(7, 2, "c").unwrap();
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 5, "{c:#?}");
    let east = at(&c, 6, 3);
    assert_eq!(east.ranges, vec![rect(1, 3, 5, 3)]);
    assert_eq!(east.exclude, vec![(3, 3), (5, 3)]);
    assert_eq!(east.level, TotalLevel::Subtotal);
    assert!(east.evidence.contains(&Evidence::GroupLabel(1, 0)));
    let west = at(&c, 8, 3);
    assert_eq!(west.ranges, vec![rect(7, 3, 7, 3)]);
    assert_eq!(verify(&grid, west).ok, Some(false), "West Total 6 ≠ 5, supported by the matching siblings");
    assert_eq!(at(&c, 3, 3).ranges, vec![rect(1, 3, 2, 3)]);
    assert_eq!(at(&c, 9, 3).level, TotalLevel::GrandTotal);
    for (r, ok) in [(3, true), (5, true), (6, true)] {
        assert_eq!(verify(&grid, at(&c, r, 3)).ok, Some(ok), "row {r}");
    }
}

#[test]
fn stray_keyword_above_a_sum_is_not_a_total() {
    // "TOTAL:" sits one row above the real SUM; the number in its row is data.
    let sheet = fixture("stray", |ws| {
        ws.write_string(0, 0, "Well").unwrap();
        ws.write_string(0, 2, "Volume").unwrap();
        rows(ws, 1, &[("w1", &[0.0, 10.0]), ("w2", &[0.0, 20.0])]);
        ws.write_string(3, 1, "TOTAL:").unwrap();
        ws.write_number(3, 2, 5.0).unwrap();
        f(ws, 4, 2, "=SUM(C2:C4)", 35.0);
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    assert_eq!(c.len(), 1, "{c:#?}");
    let chk = verify(&grid, &c[0]);
    assert_eq!((c[0].row, chk.ok, c[0].double_counted.len()), (4, Some(true), 0));
}

#[test]
fn collapsed_list_of_group_totals_has_no_visible_source() {
    let sheet = fixture("collapsed", |ws| {
        ws.write_string(0, 0, "Group").unwrap();
        ws.write_string(0, 1, "Value").unwrap();
        rows(ws, 1, &[("East Total", &[10.0]), ("West Total", &[20.0]), ("North Total", &[7.0])]);
    });
    let grid = Grid::new(&sheet);
    let c = detect(&grid);
    // Only the first could be read as a total of nothing; none has a source on the sheet.
    assert!(c.is_empty(), "{c:#?}");
}
