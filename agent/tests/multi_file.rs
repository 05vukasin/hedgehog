//! Several files in one session (B76: `f<k>.T1` joins tables of different files, the catalog
//! lists the files and their join keys) and union views of same-layout tables (B79: `U1`), through
//! a `Session` with the disk cache on and off.

use std::path::{Path, PathBuf};

use hh_agent::cache::Cache;
use hh_agent::{Options, Session};
use rust_xlsxwriter::Workbook;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hh_agent_multi_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn session(root: &Path, cache: Option<&Path>) -> Session {
    let opts = Options { cache: cache.map(|c| Cache { dir: c.to_path_buf(), max_bytes: 1 << 30 }), ..Options::default() };
    Session::with_options(root, &[], opts).unwrap()
}

/// Orders (one row per order) and order lines (several per order), in two files.
fn write_orders(d: &Path) {
    let mut orders = String::from("orderID,employeeID,orderDate\n");
    let mut lines = String::from("orderID,productID,unitPrice,quantity\n");
    for i in 0..40 {
        let id = 1000 + i;
        orders.push_str(&format!("{id},{},{}-03-{:02}\n", 1 + i % 3, if i < 25 { 2021 } else { 2022 }, 1 + i % 28));
        for k in 0..(1 + i % 3) {
            lines.push_str(&format!("{id},{},{}.5,{}\n", 10 + k, 5 + k, 1 + i % 4));
        }
    }
    std::fs::write(d.join("orders.csv"), orders).unwrap();
    std::fs::write(d.join("order_lines.csv"), lines).unwrap();
}

/// Expected revenue per employee in 2021, computed here.
fn expected() -> Vec<(usize, f64)> {
    let mut by = [0.0; 4];
    for i in 0..25usize {
        for k in 0..(1 + i % 3) {
            by[1 + i % 3] += (5.5 + k as f64) * (1 + i % 4) as f64;
        }
    }
    let mut v: Vec<(usize, f64)> = (1..4).map(|e| (e, by[e])).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v
}

fn check_join(s: &mut Session) {
    let a = s.open_file("orders.csv").unwrap();
    assert!(!a.contains("FILES open"), "one file open: no session line\n{a}");
    let b = s.open_file("order_lines.csv").unwrap();
    assert!(
        b.contains("FILES open in this session") && b.contains("f1 = orders.csv (T1)") && b.contains("f2 = order_lines.csv (T1; this file)"),
        "{b}"
    );
    assert!(b.contains("JOIN KEYS f1.T1.orderID = f2.T1.orderID (40 shared values; unique in f1.T1)"), "{b}");
    assert_eq!(s.open_files(), [("f1".to_string(), "orders.csv".to_string()), ("f2".to_string(), "order_lines.csv".to_string())]);

    let sql = "SELECT o.employeeID, SUM(l.unitPrice * l.quantity) AS revenue FROM T1 o JOIN f2.T1 l ON o.orderID = l.orderID \
               WHERE o.orderDate LIKE '2021%' GROUP BY 1 ORDER BY 2 DESC";
    let q = s.query("orders.csv", sql, None).unwrap();
    let want: Vec<String> = expected().iter().map(|(e, v)| format!("{e},{v}")).collect();
    assert_eq!(q, format!("employeeID,revenue\n{}\n(3 rows)", want.join("\n")));
    // The same from the other file's side, with both prefixes; T1 alone is the file in path.
    let q2 = s.query("order_lines.csv", &sql.replace("FROM T1 o JOIN f2.T1 l", "FROM f1.T1 o JOIN f2.T1 l"), None).unwrap();
    assert_eq!(q, q2);
    let q3 = s.query("order_lines.csv", "SELECT count(*) FROM T1", None).unwrap();
    assert_eq!(q3, "count(*)\n79\n(1 row)");
    // Notes and hints work across files.
    let e = s.query("orders.csv", "SELECT nosuch FROM f2.T1", None).unwrap_err().0;
    assert!(e.contains("orderID, productID, unitPrice, quantity") && e.contains("open files: f1 = orders.csv, f2 = order_lines.csv"), "{e}");
    let q = s.query("orders.csv", "SELECT SUM(quantity) FROM f2.T1 WHERE productID = 99", None).unwrap();
    assert!(q.contains("note: f2.T1.productID has no 99") && q.contains("from 10 to 12"), "{q}");
    // Still read-only: no ATTACH, no writes through a prefix.
    assert!(s.query("orders.csv", "ATTACH DATABASE ':memory:' AS f3", None).is_err());
    assert!(s.query("orders.csv", "SELECT * FROM f2.T1; DELETE FROM f2.T1_all", None).is_err());
    let e = s.query("orders.csv", "SELECT * FROM f5.T1", None).unwrap_err().0;
    assert!(e.contains("no such table") && e.contains("open files"), "{e}");
}

#[test]
fn join_across_files_in_memory() {
    let d = dir("mem");
    write_orders(&d);
    check_join(&mut session(&d, None));
}

#[test]
fn join_across_files_from_the_disk_cache() {
    let d = dir("disk");
    write_orders(&d);
    let cache = d.join("cache");
    check_join(&mut session(&d, Some(&cache)));
    // A new session reads both files from the cache and joins them the same way.
    check_join(&mut session(&d, Some(&cache)));
}

/// Three sheets with one table each, the same columns, the state only in the title.
fn write_states(d: &Path) {
    let mut wb = Workbook::new();
    for (k, state) in ["Burgenland", "Tirol", "Wien"].iter().enumerate() {
        let ws = wb.add_worksheet().set_name(&state[..3]).unwrap();
        ws.write(0, 0, format!("Nächtigungen in {state} – nach Herkunftsland Juli 2026")).unwrap();
        for (c, h) in ["Herkunftsland", "Ankünfte", "Nächtigungen"].iter().enumerate() {
            ws.write(2, c as u16, *h).unwrap();
        }
        let countries = ["Deutschland", "Italien", "Niederlande", "Schweiz", "Ungarn", "Polen"];
        let mut sum = (0, 0);
        for (r, c) in countries.iter().enumerate() {
            let a = 100 * (k + 1) + r * 7;
            let n = 3 * a + r;
            sum.0 += a;
            sum.1 += n;
            ws.write(3 + r as u32, 0, *c).unwrap();
            ws.write(3 + r as u32, 1, a as f64).unwrap();
            ws.write(3 + r as u32, 2, n as f64).unwrap();
        }
        let last = 3 + countries.len() as u32;
        ws.write(last, 0, "Insgesamt").unwrap();
        ws.write(last, 1, sum.0 as f64).unwrap();
        ws.write(last, 2, sum.1 as f64).unwrap();
    }
    wb.save(d.join("states.xlsx")).unwrap();
}

#[test]
fn union_view_of_same_layout_tables() {
    let d = dir("union");
    write_states(&d);
    for cache in [None, Some(d.join("cache"))] {
        let mut s = session(&d, cache.as_deref());
        let cat = s.open_file("states.xlsx").unwrap();
        assert!(
            cat.contains("SQL union view U1 = T1..T3 (3 tables with the same columns; adds _table, sheet, title, part = 'Burgenland' | 'Tirol' | 'Wien')"),
            "{cat}"
        );
        let q = s
            .query("states.xlsx", "SELECT part, \"Nächtigungen\" FROM U1_all WHERE _row_type = 'total' ORDER BY 2 DESC", None)
            .unwrap();
        assert!(q.starts_with("part,Nächtigungen\nWien,") && q.contains("\nTirol,") && q.ends_with("(3 rows)"), "{q}");
        let q = s.query("states.xlsx", "SELECT count(*), count(DISTINCT _table), count(DISTINCT sheet) FROM U1", None).unwrap();
        assert_eq!(q, "count(*),count(DISTINCT _table),count(DISTINCT sheet)\n18,3,3\n(1 row)");
    }
}
