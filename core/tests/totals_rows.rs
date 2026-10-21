//! Rows that are totals or not, beyond the English/Serbian total words: "of which" and average rows,
//! totals above their rows, totals without a total word, total words of more languages, total words
//! that are names, and known aggregates — in the detection and in the normalized output.

use hh_core::normalize::{normalize_workbook, Normalized, Options, RowType, TotalItem};
use hh_core::totals::{self, Evidence, NonSumKind, TotalLevel};
use hh_core::{Cell, Grid, Sheet, Value, Workbook};

/// A cell value: a number, or text.
#[derive(Clone, Copy)]
enum V<'a> {
    N(f64),
    T(&'a str),
    F(&'a str, f64),
    E,
}

/// A sheet from rows of values starting at A1.
fn sheet(rows: &[Vec<V>]) -> Sheet {
    let mut cells = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            let (value, formula) = match *v {
                V::N(x) => (Some(Value::Number(x)), None),
                V::T(s) => (Some(Value::Text(s.to_string())), None),
                V::F(f, x) => (Some(Value::Number(x)), Some(f.to_string())),
                V::E => continue,
            };
            cells.push(Cell { row: r as u32, col: c as u32, value, formula });
        }
    }
    Sheet { name: "Data".into(), cells, merges: vec![], formats: vec![] }
}

fn normalize(s: &Sheet, keep_totals: bool, rules_only: bool) -> Normalized {
    let wb = Workbook { sheets: vec![s.clone()] };
    normalize_workbook(&wb, &Options { keep_totals, rules_only, ..Default::default() })
}

fn items_at(n: &Normalized, row: u32) -> Vec<&TotalItem> {
    n.totals.iter().filter(|t| t.row == row && t.direction == "vertical").collect()
}

/// Source rows (0-based) kept as data rows in table 1.
fn data_rows(n: &Normalized) -> Vec<u32> {
    let t = &n.tables[0];
    t.src_rows.iter().zip(&t.row_types).filter(|(_, rt)| **rt == RowType::Data).map(|(r, _)| *r).collect()
}

fn both_paths(s: &Sheet, check: impl Fn(&Normalized, bool)) {
    for rules_only in [true, false] {
        check(&normalize(s, false, rules_only), rules_only);
    }
}

#[test]
fn of_which_rows_are_parts_not_data() {
    use V::*;
    let s = sheet(&[
        vec![T("Region"), T("Sales"), T("Units")],
        vec![T("North"), N(500.0), N(50.0)],
        vec![T("davon Inland"), N(300.0), N(20.0)],
        vec![T("South"), N(200.0), N(10.0)],
        vec![T("  of which: exports"), N(80.0), N(4.0)],
        vec![T("West"), N(100.0), N(5.0)],
        vec![T("Total"), N(800.0), N(65.0)],
    ]);
    let grid = Grid::new(&s);
    let d = totals::detection(&grid);
    let kinds: Vec<(u32, NonSumKind)> = d.nonsum.iter().map(|n| (n.row, n.kind)).collect();
    assert_eq!(kinds, vec![(2, NonSumKind::OfWhich), (4, NonSumKind::OfWhich)]);
    assert!(d.nonsum.iter().all(|n| n.cells.iter().all(|c| c.ok == Some(true))), "{:?}", d.nonsum);
    // The total adds the data rows only.
    let t = d.candidates.iter().find(|c| (c.row, c.col) == (6, 1)).unwrap();
    assert_eq!(totals::verify(&grid, t).ok, Some(true), "{t:?}");
    both_paths(&s, |n, rules| {
        assert_eq!(data_rows(n), vec![1, 3, 5], "rules {rules}");
        let w = items_at(n, 2);
        assert_eq!(w.len(), 2);
        assert!(w.iter().all(|t| t.level == "of_which" && t.status == "ok" && t.reason == "within_parent"));
        assert_eq!(w[0].recomputed, Some(500.0));
        assert!(items_at(n, 6).iter().all(|t| t.status == "ok"), "{:?}", items_at(n, 6));
    });
    let kept = normalize(&s, true, false);
    assert_eq!(kept.tables[0].row_types[1], RowType::OfWhich);
    assert_eq!(RowType::OfWhich.as_str(), "of_which");
}

#[test]
fn average_rows_are_neither_data_nor_sums() {
    use V::*;
    let s = sheet(&[
        vec![T("Country"), T("Exports"), T("Imports")],
        vec![T("Austria"), N(10.0), N(4.0)],
        vec![T("Belgium"), N(20.0), N(6.0)],
        vec![T("Croatia"), N(30.0), N(8.0)],
        vec![T("Average"), N(20.0), N(6.0)],
        vec![T("Total"), N(60.0), N(18.0)],
        vec![T("Ø"), N(20.0), N(6.0)],
    ]);
    let grid = Grid::new(&s);
    let d = totals::detection(&grid);
    let rows: Vec<u32> = d.nonsum.iter().map(|n| n.row).collect();
    assert_eq!(rows, vec![4, 6]);
    let t = d.candidates.iter().find(|c| (c.row, c.col) == (5, 1)).unwrap();
    assert_eq!(totals::verify(&grid, t).ok, Some(true), "average then total: {t:?}");
    both_paths(&s, |n, rules| {
        assert_eq!(data_rows(n), vec![1, 2, 3], "rules {rules}");
        for row in [4, 6] {
            let a = items_at(n, row);
            assert!(a.iter().all(|t| t.level == "average" && t.status == "ok" && t.reason == "matches_mean"), "{a:?}");
        }
    });
    let kept = normalize(&s, true, false);
    assert_eq!(kept.tables[0].row_types[3], RowType::Average);
}

#[test]
fn average_word_in_the_middle_of_a_list_is_data() {
    use V::*;
    // "Media" (a category) between data rows, its value no mean: data.
    let s = sheet(&[
        vec![T("Item"), T("Cost")],
        vec![T("Rent"), N(1000.0)],
        vec![T("Media"), N(250.0)],
        vec![T("Travel"), N(300.0)],
        vec![T("Food"), N(120.0)],
    ]);
    assert!(totals::detection(&Grid::new(&s)).nonsum.is_empty());
}

#[test]
fn subtotals_above_their_group_and_grand_total_at_the_top() {
    use V::*;
    let s = sheet(&[
        vec![T("Herkunft"), T("Nächte"), T("Ankünfte")],
        vec![T("Insgesamt"), N(1000.0), N(310.0)],
        vec![T("Österreich gesamt"), N(600.0), N(200.0)],
        vec![T("Wien"), N(400.0), N(120.0)],
        vec![T("Tirol"), N(200.0), N(80.0)],
        vec![T("Deutschland gesamt"), N(300.0), N(100.0)],
        vec![T("Bayern"), N(250.0), N(70.0)],
        vec![T("Berlin"), N(50.0), N(30.0)],
        vec![T("Ausland sonst"), N(100.0), N(10.0)],
    ]);
    let grid = Grid::new(&s);
    let c = totals::detect(&grid);
    let at = |r: u32| c.iter().find(|x| (x.row, x.col) == (r, 1)).unwrap_or_else(|| panic!("{r}: {c:#?}"));
    for r in [1, 2, 5] {
        assert!(at(r).evidence.contains(&Evidence::SourceBelow), "{:?}", at(r));
        assert_eq!(totals::verify(&grid, at(r)).ok, Some(true), "{:?}", at(r));
    }
    assert_eq!(at(1).level, TotalLevel::GrandTotal);
    assert_eq!(at(1).exclude, vec![(2, 1), (5, 1)]);
    both_paths(&s, |n, rules| assert_eq!(data_rows(n), vec![3, 4, 6, 7, 8], "rules {rules}"));
}

#[test]
fn sum_formula_over_the_rows_below() {
    use V::*;
    let s = sheet(&[
        vec![T("District"), T("Town"), T("Population")],
        vec![T("Total Alder"), E, F("SUM(C3:C4)", 300.0)],
        vec![T("Alder"), T("Harrow"), N(100.0)],
        vec![T("Alder"), T("Jasper"), N(200.0)],
        vec![T("Total Dunmore"), E, F("SUM(C6:C7)", 70.0)],
        vec![T("Dunmore"), T("Irwin"), N(30.0)],
        vec![T("Dunmore"), T("Fernley"), N(40.0)],
    ]);
    both_paths(&s, |n, rules| {
        assert_eq!(data_rows(n), vec![2, 3, 5, 6], "rules {rules}");
        assert!(items_at(n, 1).iter().all(|t| t.status == "ok" && t.origin == "formula"), "{:?}", n.totals);
    });
}

#[test]
fn group_label_written_once_is_filled_down_across_subtotals() {
    use V::*;
    let s = sheet(&[
        vec![T("Country"), T("Product"), T("Exports")],
        vec![T("Belgium"), T("Metals"), N(10.0)],
        vec![E, T("Food"), N(20.0)],
        vec![E, T("Paper"), N(30.0)],
        vec![T("Razem Belgium"), E, N(60.0)],
        vec![T("Croatia"), T("Vehicles"), N(5.0)],
        vec![E, T("Chemicals"), N(7.0)],
        vec![T("Razem Croatia"), E, N(12.0)],
        vec![T("TOTAL"), E, N(72.0)],
    ]);
    both_paths(&s, |n, rules| {
        let t = &n.tables[0];
        assert_eq!(t.src_rows, vec![1, 2, 3, 5, 6], "rules {rules}");
        let country: Vec<String> = t.columns[0].values.iter().map(|v| format!("{v:?}")).collect();
        assert!(country.iter().take(3).all(|v| v.contains("Belgium")), "rules {rules}: {country:?}");
        assert!(country.iter().skip(3).all(|v| v.contains("Croatia")), "rules {rules}: {country:?}");
    });
}

#[test]
fn totals_in_more_languages_and_with_typos() {
    use V::*;
    for label in ["Всего", "Celkem", "I alt", "Yhteensä", "Összesen", "Σύνολο", "Toal", "Grand Totl", "T o t a l", "∑"] {
        let s = sheet(&[
            vec![T("Region"), T("Sales"), T("Cost")],
            vec![T("North"), N(100.0), N(10.0)],
            vec![T("South"), N(200.0), N(20.0)],
            vec![T("East"), N(300.0), N(30.0)],
            vec![T(label), N(600.0), N(60.0)],
        ]);
        both_paths(&s, |n, rules| {
            assert_eq!(data_rows(n), vec![1, 2, 3], "{label} rules {rules}");
            let t = items_at(n, 4);
            assert!(t.len() == 2 && t.iter().all(|x| x.status == "ok"), "{label} rules {rules}: {t:?}");
        });
    }
}

#[test]
fn total_without_a_total_word_that_adds_up() {
    use V::*;
    let mut rows = vec![vec![T("Dép."), T("Nom"), T("Total"), T("0-19")]];
    let deps = [
        ("01", "Ain", 170_727.0, 40_112.0),
        ("02", "Aisne", 125_684.0, 30_005.0),
        ("03", "Allier", 65_821.0, 12_430.0),
        ("80", "Somme", 132_050.0, 31_009.0),
    ];
    for (code, name, a, b) in deps {
        rows.push(vec![T(code), T(name), N(a), N(b)]);
    }
    rows.push(vec![T("France métropolitaine"), E, N(494_282.0), N(113_556.0)]);
    rows.push(vec![T("971"), T("Guadeloupe"), N(86_938.0), N(20_101.0)]);
    rows.push(vec![T("972"), T("Martinique"), N(72_873.0), N(14_007.0)]);
    rows.push(vec![T("973"), T("Guyane"), N(122_773.0), N(50_310.0)]);
    rows.push(vec![T("DOM"), E, N(282_584.0), N(84_418.0)]);
    rows.push(vec![T("France métropolitaine et DOM"), E, N(776_866.0), N(197_974.0)]);
    let s = sheet(&rows);
    let grid = Grid::new(&s);
    let c = totals::detect(&grid);
    for (r, lvl) in [(5, TotalLevel::Subtotal), (9, TotalLevel::Subtotal), (10, TotalLevel::GrandTotal)] {
        let t = c.iter().find(|x| (x.row, x.col) == (r, 2)).unwrap_or_else(|| panic!("{r}: {c:#?}"));
        assert!(t.evidence.contains(&Evidence::Unlabelled));
        assert_eq!(totals::verify(&grid, t).ok, Some(true));
        assert_eq!(t.level, lvl, "{r}");
    }
    // "Somme" (a département) is not a total: its numbers do not add up.
    assert!(!c.iter().any(|x| x.row == 4), "{c:#?}");
    both_paths(&s, |n, rules| assert_eq!(data_rows(n), vec![1, 2, 3, 4, 6, 7, 8], "rules {rules}"));
}

#[test]
fn small_whole_numbers_that_add_up_by_chance_are_data() {
    use V::*;
    let s = sheet(&[
        vec![T("Country"), T("1971"), T("1982"), T("1983"), T("1984"), T("1985")],
        vec![T("Ireland"), N(2.0), N(6.0), N(6.0), N(6.0), N(6.0)],
        vec![T("Iran"), N(0.0), N(3.0), N(3.0), N(3.0), N(4.0)],
        vec![T("Iraq"), N(0.0), N(3.0), N(3.0), N(3.0), N(3.0)],
        vec![T("Iceland"), N(3.0), N(14.0), N(12.0), N(12.0), N(13.0)],
        vec![T("Israel"), N(2.0), N(7.0), N(8.0), N(7.0), N(7.0)],
    ]);
    assert!(totals::detect(&Grid::new(&s)).is_empty());
}

#[test]
fn names_holding_a_total_word_stay_data() {
    use V::*;
    let s = sheet(&[
        vec![T("Trader"), T("Location"), T("Volume"), T("Orig")],
        vec![T("McKay"), T("Malin"), N(10.0), N(100.0)],
        vec![T("Mc Kay"), T("Opal"), N(20.0), N(250.0)],
        vec![T("Lagrasta"), T("Sumas"), N(30.0), N(300.0)],
        vec![T("Tycholiz"), T("Sumas"), N(15.0), N(410.0)],
        vec![T("Ring"), T("Malin"), N(5.0), N(80.0)],
    ]);
    let grid = Grid::new(&s);
    assert!(totals::detect(&grid).is_empty(), "{:#?}", totals::detect(&grid));
    both_paths(&s, |n, rules| assert_eq!(data_rows(n), vec![1, 2, 3, 4, 5], "rules {rules}"));
}

#[test]
fn register_descriptions_starting_with_a_total_word_are_records() {
    use V::*;
    const DESCR: [&str; 4] = ["Office supplies", "IT support contract", "Total knee replacement implant", "Sum insured adjustment"];
    let ids: Vec<String> = (0..300).map(|i| format!("K-24-{:05}", 100 + 7 * i)).collect();
    let mut rows = vec![vec![T("Contract no."), T("Description"), T("Supplier"), T("Quantity"), T("Amount")]];
    for (i, id) in ids.iter().enumerate() {
        let d = DESCR[i % 4];
        rows.push(vec![T(id), T(d), T("Acme d.o.o."), N((i % 7 + 1) as f64), N(1000.0 + (i * 37 % 101) as f64)]);
    }
    let s = sheet(&rows);
    let grid = Grid::new(&s);
    assert!(totals::detect(&grid).is_empty());
    let n = normalize(&s, false, false);
    assert_eq!(n.tables[0].rows(), 300);
    assert!(n.totals.is_empty(), "{:?}", &n.totals[..3.min(n.totals.len())]);
}

#[test]
fn world_bank_aggregates_are_marked() {
    use V::*;
    let mut rows = vec![vec![T("Country Name"), T("Country Code"), T("2023"), T("2024")]];
    // Country codes ZAA, ZAB, … (no aggregate code), then five aggregates.
    let mut names: Vec<(String, String)> = (0..40u8)
        .map(|i| (format!("Country {i}"), format!("Z{}{}", (b'A' + i / 26) as char, (b'A' + i % 26) as char)))
        .collect();
    for (n, c) in [("World", "WLD"), ("European Union", "EUU"), ("High income", "HIC"), ("Euro area", "EMU"), ("Arab World", "ARB")] {
        names.push((n.into(), c.into()));
    }
    for (i, (n, c)) in names.iter().enumerate() {
        rows.push(vec![V::T(n), V::T(c), N(1000.0 + i as f64), N(1100.0 + i as f64)]);
    }
    let s = sheet(&rows);
    let d = totals::detection(&Grid::new(&s)).clone();
    assert_eq!(d.aggregates.iter().map(|a| a.0).collect::<Vec<_>>(), vec![41, 42, 43, 44, 45]);
    let n = normalize(&s, false, false);
    assert_eq!(n.tables[0].rows(), 40);
    assert!(n.totals.iter().all(|t| t.reason == "known_aggregate" && t.status == "unknown"));
}
