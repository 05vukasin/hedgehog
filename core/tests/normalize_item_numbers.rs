//! Values read by their column (MiMoTable en-0371, en-0496): an item number `(4)` is no
//! accounting negative unless the column writes amounts that way, and plain Excel serials are
//! dates only under a date header.

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, Dtype, Options, Out, Table};
use hh_core::open;
use rust_xlsxwriter::Workbook;

/// A cell to write: text or number.
enum V<'a> {
    T(&'a str),
    N(f64),
}

/// One table: header row, then the body rows; its first table.
fn table(name: &str, header: &[&str], body: &[Vec<V>]) -> Table {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_items_{name}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    for (c, h) in header.iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    for (r, row) in body.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            match v {
                V::T(s) => ws.write_string(1 + r as u32, c as u16, *s).unwrap(),
                V::N(x) => ws.write_number(1 + r as u32, c as u16, *x).unwrap(),
            };
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    normalize_workbook(&book, &Options::default()).tables.into_iter().next().expect("a table")
}

fn col<'a>(t: &'a Table, name: &str) -> &'a hh_core::normalize::Column {
    t.columns.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no {name}"))
}

/// en-0371: an item-number column with sub-items `(1)`, `(2)` among plain numbers 1, 2, 3 keeps
/// `(1)` as written, never -1.
#[test]
fn item_number_in_parentheses_is_no_negative() {
    use V::*;
    let mut body = Vec::new();
    for (k, item) in ["(1)", "1", "2", "3", "(2)", "1", "2", "(3)", "1", "2", "3", "4"].iter().enumerate() {
        body.push(vec![T(item), T(["Field leveling", "Earth backfill", "Sod laying"][k % 3]), N(10.5 + k as f64)]);
    }
    let t = table("item", &["No.", "Item", "Quantity"], &body);
    let no = col(&t, "No.");
    assert!(!no.values.contains(&Out::Num(-1.0)), "{:?}", no.values);
    assert!(no.values.contains(&Out::Text("(1)".into())), "{:?}", no.values);
    assert!(no.values.contains(&Out::Text("(3)".into())), "{:?}", no.values);
}

/// An amount column that writes negatives in parentheses (a thousands separator, another
/// negative) keeps reading `(4)` as -4.
#[test]
fn accounting_negatives_in_an_amount_column() {
    use V::*;
    let rows: [(&str, V); 6] = [
        ("Sales", T("1,250")),
        ("Returns", T("(4)")),
        ("Fees", T("(35)")),
        ("Interest", T("12")),
        ("Other", T("(1,020)")),
        ("Net", T("203")),
    ];
    let body: Vec<Vec<V>> = rows.into_iter().map(|(l, v)| vec![T(l), v]).collect();
    let t = table("acct", &["Item", "Amount"], &body);
    let a = col(&t, "Amount");
    assert!(a.values.contains(&Out::Num(-4.0)), "{:?}", a.values);
    assert!(a.values.contains(&Out::Num(-35.0)), "{:?}", a.values);
    let body: Vec<Vec<V>> = vec![
        vec![T("A"), N(-3.0)],
        vec![T("B"), T("(4)")],
        vec![T("C"), N(7.0)],
        vec![T("D"), N(9.0)],
    ];
    let t = table("neg", &["Item", "Change"], &body);
    assert!(col(&t, "Change").values.contains(&Out::Num(-4.0)), "{:?}", col(&t, "Change").values);
}

/// en-0496: a `Date` column of plain serials (no date format) is a date column; serials under
/// another header, or small numbers under a date header, stay numbers.
#[test]
fn plain_serials_under_a_date_header() {
    use V::*;
    let body: Vec<Vec<V>> = (0..6).map(|k| vec![N(1.0 + k as f64), N(44301.0 + k as f64), N(44301.0 + 7.0 * k as f64), N(3.0 + k as f64)]).collect();
    let t = table("serial", &["Serial Number", "Date", "Amount", "Date of week"], &body);
    let d = col(&t, "Date");
    assert_eq!(d.dtype, Dtype::Date);
    assert_eq!(d.values[0], Out::Date(44301.0));
    assert_eq!(col(&t, "Amount").values[0], Out::Num(44301.0));
    assert_eq!(col(&t, "Date of week").values[0], Out::Num(3.0));
    assert_eq!(col(&t, "Serial Number").values[0], Out::Num(1.0));
}
