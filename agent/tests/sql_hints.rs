//! SQL error hints (other dialects, total-row labels used as columns) and entity columns
//! (`"<id> - <name>"` split into `<col>_id` / `<col>_name`), through a `Session`.

use std::path::PathBuf;

use hh_agent::Session;
use rust_xlsxwriter::{Formula, Workbook};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hh_agent_hints_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A register: buyer = "<tax id> - <name>" with several spellings of one body; supplier plain text.
fn contracts_csv() -> String {
    let buyers = [
        "503933813 - IP - Infraestruturas de Portugal, S.A.",
        "503933813 - Infraestruturas de Portugal, S. A.",
        "503933813 - INFRAESTRUTURAS DE PORTUGAL SA",
        "503278602 - Metro do Porto, S. A.",
        "600012662 - Ministério da Defesa Nacional - Marinha",
    ];
    let mut csv = String::from("buyer,supplier,price,signed\n");
    for i in 0..30 {
        let b = buyers[i % buyers.len()];
        let supplier = if i == 7 {
            "\"504223933 - MEDTRONIC, Lda 508107997 - Generis, SA\"".to_string()
        } else {
            format!("Supplier {}", i % 4)
        };
        csv.push_str(&format!("\"{b}\",{supplier},{},2020-{:02}-15\n", 1000 + i * 10, 1 + i % 12));
    }
    csv
}

#[test]
fn entity_columns_group_by_id() {
    let d = dir("entity");
    std::fs::write(d.join("contracts.csv"), contracts_csv()).unwrap();
    let mut s = Session::new(&d, &[]).unwrap();
    let cat = s.open_file("contracts.csv").unwrap();
    assert!(
        cat.contains("buyer = \"<id> - <name>\"") && cat.contains("3 IDs, 1 with several spellings (e.g. 503933813: 3 names)"),
        "{cat}"
    );
    assert!(cat.contains("group by buyer_id"), "{cat}");
    assert!(!cat.contains("supplier = \"<id>"), "plain names are no entity column:\n{cat}");

    let q = s
        .query("contracts.csv", "SELECT buyer_id, count(DISTINCT buyer_name) AS names, count(*) AS n FROM T1 GROUP BY buyer_id ORDER BY n DESC, buyer_id", None)
        .unwrap();
    assert_eq!(q, "buyer_id,names,n\n503933813,3,18\n503278602,1,6\n600012662,1,6\n(3 rows)");
    // The IDs keep their text (leading zeros would survive) and are in T1_all too; cited as the source cell.
    let q = s.query("contracts.csv", "SELECT buyer_id, buyer_name, _src_row FROM T1_all LIMIT 1", None).unwrap();
    assert!(q.starts_with("buyer_id,buyer_name,_src_row\n503933813,\"IP - Infraestruturas de Portugal, S.A.\",2"), "{q}");
    assert!(q.contains("cite as: buyer_id = contracts!A<_src_row> (ID part)"), "{q}");

    let desc = s.describe_table("contracts.csv", "T1").unwrap();
    assert!(desc.contains("entity columns (the SQL views T1 and T1_all also have buyer_id, buyer_name)"), "{desc}");
    // A misspelled derived column is suggested.
    let e = s.query("contracts.csv", "SELECT buyerid FROM T1", None).unwrap_err().0;
    assert!(e.contains("T1.\"buyer_id\""), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn other_dialects_get_the_sqlite_way() {
    let d = dir("dialect");
    std::fs::write(d.join("contracts.csv"), contracts_csv()).unwrap();
    let mut s = Session::new(&d, &[]).unwrap();
    let err = |s: &mut Session, sql: &str| s.query("contracts.csv", sql, None).unwrap_err().0;
    let e = err(&mut s, "SELECT MONTH(signed) AS m, count(*) FROM T1 GROUP BY m");
    assert!(e.contains("no such function: MONTH") && e.contains("strftime('%m', col)"), "{e}");
    let e = err(&mut s, "SELECT YEAR(signed) FROM T1");
    assert!(e.contains("strftime('%Y', col)"), "{e}");
    let e = err(&mut s, "SELECT EXTRACT(MONTH FROM signed) FROM T1");
    assert!(e.contains("EXTRACT") && e.contains("strftime"), "{e}");
    let e = err(&mut s, "SELECT TOP 3 buyer FROM T1");
    assert!(e.contains("LIMIT"), "{e}");
    let e = err(&mut s, "SELECT buyer FROM T1 WHERE buyer ILIKE '%porto%'");
    assert!(e.contains("LIKE is already case-insensitive"), "{e}");
    let e = err(&mut s, "SELECT DATE_FORMAT(signed, '%Y-%m') FROM T1");
    assert!(e.contains("strftime('%Y-%m', col)"), "{e}");
    let e = err(&mut s, "SELECT DATEDIFF(signed, '2020-01-01') FROM T1");
    assert!(e.contains("julianday"), "{e}");
    // The replacement works.
    let ok = s.query("contracts.csv", "SELECT CAST(strftime('%m', signed) AS INTEGER) AS m, count(*) FROM T1 GROUP BY m LIMIT 1", None).unwrap();
    assert!(ok.starts_with("m,count(*)\n1,3"), "{ok}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn total_row_labels_are_rows() {
    let d = dir("total");
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Flows").unwrap();
    for (c, h) in ["Pipeline", "Receipts", "Deliveries"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    let rows = [("SEA ROBIN", 123.0, 10.0), ("NGPL", 105.0, 20.0), ("MAINLINE", 100.0, 30.0)];
    for (i, (p, a, b)) in rows.iter().enumerate() {
        ws.write_string(1 + i as u32, 0, *p).unwrap();
        ws.write_number(1 + i as u32, 1, *a).unwrap();
        ws.write_number(1 + i as u32, 2, *b).unwrap();
    }
    ws.write_string(4, 0, "Total").unwrap();
    ws.write_formula(4, 1, Formula::new("=SUM(B2:B4)").set_result("328")).unwrap();
    ws.write_formula(4, 2, Formula::new("=SUM(C2:C4)").set_result("60")).unwrap();
    wb.save(d.join("flows.xlsx")).unwrap();
    let mut s = Session::new(&d, &[]).unwrap();
    s.open_file("flows.xlsx").unwrap();
    let e = s.query("flows.xlsx", "SELECT \"Total\" FROM T1_all", None).unwrap_err().0;
    assert!(
        e.contains(
            "\"Total\" is a row label, not a column: total rows are in T1_all with _row_type = 'total' (label in Pipeline): \
             SELECT * FROM T1_all WHERE _row_type = 'total'"
        ),
        "{e}"
    );
    let e = s.query("flows.xlsx", "SELECT Gesamt FROM T1", None).unwrap_err().0;
    assert!(e.contains("row label, not a column") && e.contains("T1_all"), "{e}");
    // A data-row label used as a column.
    let e = s.query("flows.xlsx", "SELECT \"SEA ROBIN\" FROM T1", None).unwrap_err().0;
    assert!(e.contains("is a value of column Pipeline, not a column: SELECT * FROM T1 WHERE \"Pipeline\" = 'SEA ROBIN'"), "{e}");
    // A plain typo still gets the closest names.
    let e = s.query("flows.xlsx", "SELECT Receits FROM T1", None).unwrap_err().0;
    assert!(e.contains("did you mean T1.\"Receipts\""), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}
