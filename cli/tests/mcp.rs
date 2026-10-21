//! Spawns `hedgehog mcp` and talks JSON-RPC to it over stdio, on a generated xlsx and a CSV.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use rust_xlsxwriter::{Format, Formula, Workbook};
use serde_json::{json, Value};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: i64,
}

impl Server {
    fn start(root: &Path) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hedgehog"))
            .args(["mcp", "--root"])
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn hedgehog mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Server { child, stdin, stdout, next: 1 }
    }

    fn send_raw(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "server closed stdout");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad JSON from server: {e}: {line}"))
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send_raw(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string());
        let r = self.recv();
        assert_eq!(r["jsonrpc"], "2.0");
        assert_eq!(r["id"], id, "{r}");
        r
    }

    fn notify(&mut self, method: &str) {
        self.send_raw(&json!({"jsonrpc": "2.0", "method": method}).to_string());
    }

    /// Calls a tool; returns (text, isError).
    fn tool(&mut self, name: &str, args: Value) -> (String, bool) {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        assert!(res.is_object(), "{r}");
        let content = res["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        let text = content[0]["text"].as_str().unwrap().to_string();
        assert!(text.len() <= 12_000, "result over 12,000 chars: {}", text.len());
        (text, res["isError"].as_bool().unwrap())
    }

    fn ok(&mut self, name: &str, args: Value) -> String {
        let (text, err) = self.tool(name, args.clone());
        assert!(!err, "{name} {args} failed: {text}");
        text
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// root/ with sales.xlsx (title, header, 4 rows, a total row with formulas) and data/prices.csv;
/// outside/secret.csv next to root.
fn fixtures(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hh_mcp_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("root/data")).unwrap();
    std::fs::create_dir_all(dir.join("outside")).unwrap();

    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Sales").unwrap();
    ws.merge_range(0, 0, 0, 3, "Sales report 2023", &Format::new()).unwrap();
    for (c, h) in ["City", "Units", "Revenue Q1", "Revenue Q2"].iter().enumerate() {
        ws.write_string(2, c as u16, *h).unwrap();
    }
    let rows = [("Beograd", 10.0, 1200.5, 1300.0), ("Niš", 7.0, 800.0, 950.25), ("Novi Sad", 8.0, 640.0, 700.0), ("Subotica", 3.0, 210.0, 190.0)];
    for (i, (city, units, q1, q2)) in rows.iter().enumerate() {
        let r = 3 + i as u32;
        ws.write_string(r, 0, *city).unwrap();
        ws.write_number(r, 1, *units).unwrap();
        ws.write_number(r, 2, *q1).unwrap();
        ws.write_number(r, 3, *q2).unwrap();
    }
    ws.write_string(7, 0, "Total").unwrap();
    ws.write_formula(7, 1, Formula::new("=SUM(B4:B7)").set_result("28")).unwrap();
    ws.write_formula(7, 2, Formula::new("=SUM(C4:C7)").set_result("2850.5")).unwrap();
    ws.write_formula(7, 3, Formula::new("=SUM(D4:D7)").set_result("3140.25")).unwrap();
    wb.save(dir.join("root/sales.xlsx")).unwrap();

    let mut csv = String::from("Date,Product,Price\n");
    for m in 1..=12 {
        csv.push_str(&format!("2024-{m:02}-01,Milk,{}\n", 1.0 + m as f64 / 10.0));
        csv.push_str(&format!("2024-{m:02}-01,Bread,{}\n", 2.0 + m as f64 / 20.0));
    }
    std::fs::write(dir.join("root/data/prices.csv"), csv).unwrap();
    std::fs::write(dir.join("outside/secret.csv"), "a,b\n1,2\n").unwrap();
    dir
}

#[test]
fn mcp_session_end_to_end() {
    let dir = fixtures("e2e");
    let mut s = Server::start(&dir.join("root"));

    // initialize: current and older protocol versions
    let init = s.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
    );
    let r = &init["result"];
    assert_eq!(r["protocolVersion"], "2025-06-18");
    assert_eq!(r["serverInfo"]["name"], "hedgehog");
    assert!(r["capabilities"]["tools"].is_object());
    s.notify("notifications/initialized");
    s.notify("notifications/something_unknown");
    assert_eq!(s.request("ping", json!({}))["result"], json!({}));

    // tools/list
    let list = s.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["open_file", "describe_table", "query", "cells", "totals", "list_files"]);
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert!(t["description"].as_str().unwrap().len() > 40);
    }
    let query_desc = tools[2]["description"].as_str().unwrap();
    assert!(query_desc.contains("SQLite") && query_desc.contains("strftime('%m', d)"), "{query_desc}");

    // open_file on the xlsx
    let cat = s.ok("open_file", json!({"path": "sales.xlsx"}));
    assert!(cat.starts_with("FILE sales.xlsx · 1 sheet"), "{cat}");
    assert!(cat.contains("TIME 2023"), "{cat}");
    assert!(cat.contains("T1 "), "{cat}");
    assert!(cat.contains("'Total'@8"), "total row label and row in the catalog:\n{cat}");
    assert!(cat.contains("\"Revenue Q1\"[number"), "{cat}");
    // a small table: its rows inline, the total row marked; how to select total rows
    assert!(cat.contains("all 5 rows (CSV, as in T1_all; rows with _row_type total/subtotal are not in T1):\n    City,Units,Revenue Q1,Revenue Q2,_row_type\n    Beograd,10,1200.5,1300,data\n"), "{cat}");
    assert!(cat.contains("\n    Total,28,2850.5,3140.25,total\n"), "{cat}");
    assert!(cat.contains("total rows: SELECT … FROM T1_all WHERE _row_type = 'total' (label in City: 'Total')"), "{cat}");

    // query: data rows, sorted
    let q = s.ok("query", json!({"path": "sales.xlsx", "sql": "SELECT City, \"Revenue Q1\", _src_row FROM T1 ORDER BY \"Revenue Q1\" DESC"}));
    let lines: Vec<&str> = q.lines().collect();
    assert_eq!(lines[0], "City,Revenue Q1,_src_row", "{q}");
    assert_eq!(lines[1], "Beograd,1200.5,4", "{q}");
    assert!(q.contains("cite as: City = Sales!A<_src_row>"), "{q}");
    assert!(q.ends_with("(4 rows)"), "{q}");
    // the total row is in T1_all only
    let t = s.ok("query", json!({"path": "sales.xlsx", "sql": "SELECT \"Revenue Q1\" FROM T1_all WHERE _row_type = 'total'"}));
    assert!(t.contains("2850.5") && t.ends_with("(1 row)"), "{t}");
    let sum = s.ok("query", json!({"path": "sales.xlsx", "sql": "SELECT sum(\"Revenue Q2\") AS s FROM T1"}));
    assert!(sum.contains("3140.25"), "{sum}");
    // limit
    let lim = s.ok("query", json!({"path": "sales.xlsx", "sql": "SELECT City FROM T1", "limit": 2}));
    assert!(lim.ends_with("(4 rows, showing first 2)"), "{lim}");
    // unknown column → closest names
    let (e, err) = s.tool("query", json!({"path": "sales.xlsx", "sql": "SELECT \"Revenue / Q1\" FROM T1"}));
    assert!(err && e.contains("no such column") && e.contains("T1.\"Revenue Q1\""), "{e}");
    // writes and file access are refused
    for sql in ["DELETE FROM T1_all", "ATTACH DATABASE 'x.db' AS x", "PRAGMA query_only=OFF", "SELECT 1; DROP TABLE T1_all"] {
        let (e, err) = s.tool("query", json!({"path": "sales.xlsx", "sql": sql}));
        assert!(err, "{sql} was accepted: {e}");
    }
    let n = s.ok("query", json!({"path": "sales.xlsx", "sql": "SELECT count(*) FROM T1_all"}));
    assert!(n.starts_with("count(*)\n5"), "{n}");

    // describe_table
    let d = s.ok("describe_table", json!({"path": "sales.xlsx", "table": "T1"}));
    assert!(d.starts_with("T1 = Sales!"), "{d}");
    assert!(d.contains("total rows (only in T1_all"), "{d}");
    assert!(d.contains("first 4 rows"), "{d}");
    let (e, err) = s.tool("describe_table", json!({"path": "sales.xlsx", "table": "T9"}));
    assert!(err && e.contains("tables are T1"), "{e}");

    // cells: source values, merged title, formula
    let c = s.ok("cells", json!({"path": "sales.xlsx", "sheet": "Sales", "range": "A1:D8"}));
    assert!(c.contains("A1\tSales report 2023\t[merged A1:D1]"), "{c}");
    assert!(c.contains("B1\t[merged into A1]"), "{c}");
    assert!(c.contains("C8\t2850.5\t=SUM(C4:C7)"), "{c}");
    let (e, err) = s.tool("cells", json!({"path": "sales.xlsx", "sheet": "Sales", "range": "A1:Z100"}));
    assert!(err && e.contains("at most 200"), "{e}");
    let (e, err) = s.tool("cells", json!({"path": "sales.xlsx", "sheet": "Nope", "range": "A1"}));
    assert!(err && e.contains("'Sales'"), "{e}");

    // totals
    let tot = s.ok("totals", json!({"path": "sales.xlsx"}));
    assert!(tot.starts_with("TOTALS: "), "{tot}");
    assert!(s.tool("totals", json!({"path": "sales.xlsx", "status": "bogus"})).1);

    // CSV
    let cat = s.ok("open_file", json!({"path": "data/prices.csv"}));
    assert!(cat.starts_with("FILE data/prices.csv"), "{cat}");
    assert!(cat.contains("TIME 2024-01–2024-12 (month, from columns T1.Date)"), "{cat}");
    // 24 rows × 3 columns: inline (the values to filter on are in the rows)
    assert!(cat.contains("all 24 rows (CSV):\n    Date,Product,Price\n"), "{cat}");
    assert!(cat.contains(",Milk,") && cat.contains(",Bread,"), "{cat}");
    let q = s.ok("query", json!({"path": "data/prices.csv", "sql": "SELECT Product, count(*) AS n, min(Date) AS first FROM T1 GROUP BY Product ORDER BY Product"}));
    assert_eq!(q, "Product,n,first\nBread,12,2024-01-01\nMilk,12,2024-01-01\n(2 rows)");

    // list_files
    let l = s.ok("list_files", json!({}));
    assert!(l.starts_with("2 spreadsheet files under ."), "{l}");
    assert!(l.contains("data/prices.csv") && l.contains("sales.xlsx"), "{l}");
    assert!(s.ok("list_files", json!({"pattern": "*.csv"})).contains("1 spreadsheet file"));

    // path bounds
    let (e, err) = s.tool("open_file", json!({"path": "../outside/secret.csv"}));
    assert!(err && e.contains("outside the working folder"), "{e}");
    let abs = dir.join("outside/secret.csv");
    let (_, err) = s.tool("open_file", json!({"path": abs.to_str().unwrap()}));
    assert!(err);
    let (_, err) = s.tool("list_files", json!({"folder": ".."}));
    assert!(err);

    // protocol errors
    let r = s.request("no/such/method", json!({}));
    assert_eq!(r["error"]["code"], -32601);
    let r = s.request("tools/call", json!({"name": "rm_rf", "arguments": {}}));
    assert_eq!(r["error"]["code"], -32602);
    s.send_raw("{not json");
    assert_eq!(s.recv()["error"]["code"], -32700);
    // still alive
    assert_eq!(s.request("ping", json!({}))["result"], json!({}));
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allow_paths_from_initialize_and_old_protocol() {
    let dir = fixtures("allow");
    let mut s = Server::start(&dir.join("root"));
    let abs = dir.join("outside/secret.csv");
    let init = s.request(
        "initialize",
        json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"},
               "_meta": {"hedgehog": {"allow": [abs.to_str().unwrap()]}}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    let cat = s.ok("open_file", json!({"path": abs.to_str().unwrap()}));
    assert!(cat.starts_with("FILE "), "{cat}");
    // the allowed file only, not its folder
    let (_, err) = s.tool("list_files", json!({"folder": dir.join("outside").to_str().unwrap()}));
    assert!(err);
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}
