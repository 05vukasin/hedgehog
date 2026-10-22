//! Notes that explain a query result (B75: empty results, values only in `T<n>_all`, close
//! spellings; B88: NULL from `a + b`, errors about `_row_type` and subquery columns), through a
//! `Session`.

use std::path::PathBuf;

use hh_agent::Session;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hh_agent_notes_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Sales by region with a total row, a yes/no column and a sparse bonus column.
fn sales(d: &std::path::Path) -> Session {
    let mut csv = String::from("region,city,amount,bonus,paid\n");
    let regions = ["North", "South", "East"];
    let mut sum = 0;
    for i in 0..30 {
        let amount = 100 + i * 10;
        sum += amount;
        let bonus = if i % 3 == 0 { String::new() } else { (i * 2).to_string() };
        let paid = if i % 2 == 0 { "TRUE" } else { "FALSE" };
        csv.push_str(&format!("{},City {i},{amount},{bonus},{paid}\n", regions[i % 3]));
    }
    csv.push_str(&format!("Total,,{sum},,\n"));
    std::fs::write(d.join("sales.csv"), csv).unwrap();
    let mut s = Session::new(d, &[]).unwrap();
    let cat = s.open_file("sales.csv").unwrap();
    assert!(cat.contains("total row"), "{cat}");
    s
}

#[test]
fn empty_results_explain_themselves() {
    let d = dir("empty");
    let mut s = sales(&d);
    // A total-row label filtered on T1: the row is in T1_all, and its values come back.
    let q = s.query("sales.csv", "SELECT amount FROM T1 WHERE region = 'Total'", None).unwrap();
    assert!(q.contains("T1.region = 'Total' is only on 1 total row (_row_type 'total'), which is in T1_all, not in T1"), "{q}");
    assert!(q.contains("The same query on T1_all (the total rows included):\namount\n7350"), "{q}");
    assert!(q.ends_with("(0 rows)"), "{q}");
    // A value the column does not have: its values, and that this is not a 0.
    let q = s.query("sales.csv", "SELECT SUM(amount) FROM T1 WHERE region = 'West'", None).unwrap();
    assert!(q.contains("T1.region has no 'West' — 0 rows here means the file has no such rows, not a value of 0"), "{q}");
    assert!(q.contains("its 4 values: 'East' (10 rows), 'North' (10 rows), 'South' (10 rows), 'Total' (1 row)"), "{q}");
    // A close spelling.
    let q = s.query("sales.csv", "SELECT count(*) FROM T1 WHERE region = 'north'", None).unwrap();
    assert!(q.contains("values that contain it: 'North' (10 rows)"), "{q}");
    // A quoted value against a yes/no column stored as 1/0.
    let q = s.query("sales.csv", "SELECT count(*) AS n FROM T1 WHERE paid = 'TRUE'", None).unwrap();
    assert!(q.contains("paid is a yes/no column stored as 1/0: use paid = 1"), "{q}");
    // A number outside the column's range.
    let q = s.query("sales.csv", "SELECT * FROM T1 WHERE amount = 7", None).unwrap();
    assert!(q.contains("T1.amount has no 7") && q.contains("its values run from 100 to 390"), "{q}");
    // A filter that matches: no note.
    let q = s.query("sales.csv", "SELECT count(*) FROM T1 WHERE region = 'North'", None).unwrap();
    assert_eq!(q, "count(*)\n10\n(1 row)");
}

#[test]
fn sums_of_sparse_columns_and_misleading_errors() {
    let d = dir("errors");
    let mut s = sales(&d);
    // B88: a + b is NULL where b is empty.
    let q = s.query("sales.csv", "SELECT city, amount + bonus AS x FROM T1 ORDER BY _src_row LIMIT 3", None).unwrap();
    assert!(q.contains("note: 1 result row have empty values") || q.contains("note: 1 result row has empty values"), "{q}");
    assert!(q.contains("bonus empty in 10 rows") && q.contains("COALESCE(col, 0)"), "{q}");
    // B88: _row_type is in T1_all only.
    let e = s.query("sales.csv", "SELECT region FROM T1 WHERE _row_type = 'total'", None).unwrap_err().0;
    assert!(e.contains("T1 has the data rows only and no _row_type") && e.contains("are in T1_all"), "{e}");
    // B88: the outer query names a column the subquery does not select.
    let e = s
        .query("sales.csv", "SELECT SUM(amount) FROM (SELECT region, SUM(amount) AS total FROM T1 GROUP BY region)", None)
        .unwrap_err()
        .0;
    assert!(e.contains("amount is a column of T1, but this part of the query reads from a subquery") && e.contains("region, total"), "{e}");
    assert!(!e.contains("did you mean"), "{e}");
    let e = s.query("sales.csv", "WITH r AS (SELECT region, SUM(amount) AS total FROM T1 GROUP BY region) SELECT SUM(amout) FROM r", None).unwrap_err().0;
    assert!(e.contains("the closest column is T1.\"amount\"") && e.contains("r (CTE): region, total"), "{e}");
}
