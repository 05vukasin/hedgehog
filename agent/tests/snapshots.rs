//! Tier 0 feature snapshots: one small generated workbook per agent-facing feature, its `open_file`
//! catalog and a few query results compared with text files in `agent/tests/snapshots/`.
//!
//! Instant and deterministic: no LLM, no network, no data on disk besides what the test writes.
//! A change in the catalog or a query result fails with a readable line diff; accept it with
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p hh_agent --test snapshots     # or: make quick-update
//! ```
//!
//! and review the changed files in `git diff agent/tests/snapshots/`.

use std::path::{Path, PathBuf};

use hh_agent::{Options, Session};
use rust_xlsxwriter::{DocProperties, ExcelDateTime, Formula, Workbook, Worksheet};

// ---------------------------------------------------------------------------------------------
// Snapshot helper
// ---------------------------------------------------------------------------------------------

/// One feature's snapshot: the files written, the catalog of each opened file, then each query.
struct Snap {
    name: &'static str,
    dir: PathBuf,
    session: Session,
    out: String,
}

impl Snap {
    /// A fresh, empty fixture folder for `name` (fixed path under cargo's test tmp dir, no pid).
    fn new(name: &'static str) -> Snap {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hh_snapshots").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No disk cache: the result must not depend on what an earlier run left behind.
        let opts = Options { cache: None, ..Options::default() };
        let session = Session::with_options(&dir, &[], opts).unwrap();
        Snap { name, dir, session, out: format!("# snapshot: {name}\n") }
    }

    fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    fn write(&self, file: &str, text: &str) {
        std::fs::write(self.path(file), text).unwrap();
    }

    fn write_bytes(&self, file: &str, bytes: &[u8]) {
        std::fs::write(self.path(file), bytes).unwrap();
    }

    fn save(&self, file: &str, wb: &mut Workbook) {
        wb.set_properties(&DocProperties::new().set_creation_datetime(&ExcelDateTime::from_ymd(2024, 1, 1).unwrap()));
        wb.save(self.path(file)).unwrap();
    }

    /// Drops what changes from run to run: the fixture folder and the normalize time.
    fn clean(&self, s: &str) -> String {
        let mut s = s.replace(&self.dir.display().to_string(), "<dir>");
        const AT: &str = "(normalized in ";
        let mut from = 0;
        while let Some(i) = s[from..].find(AT).map(|i| i + from + AT.len()) {
            let Some(j) = s[i..].find(" s)").map(|j| j + i) else { break };
            s.replace_range(i..j, "<t>");
            from = i;
        }
        s
    }

    fn open(&mut self, file: &str) {
        let r = self.session.open_file(file);
        let text = match r {
            Ok(t) => t,
            Err(e) => format!("ERROR: {}", e.0),
        };
        let text = self.clean(&text);
        self.out.push_str(&format!("\n## open_file {file}\n{text}\n"));
    }

    fn query(&mut self, file: &str, sql: &str) {
        let r = self.session.query(file, sql, None);
        let text = match r {
            Ok(t) => t,
            Err(e) => format!("ERROR: {}", e.0),
        };
        let text = self.clean(&text);
        self.out.push_str(&format!("\n## query {file}: {sql}\n{text}\n"));
    }

    fn describe(&mut self, file: &str, table: &str) {
        let text = match self.session.describe_table(file, table) {
            Ok(t) => t,
            Err(e) => format!("ERROR: {}", e.0),
        };
        let text = self.clean(&text);
        self.out.push_str(&format!("\n## describe_table {file} {table}\n{text}\n"));
    }

    fn totals(&mut self, file: &str) {
        let text = match self.session.totals(file, None, None) {
            Ok(t) => t,
            Err(e) => format!("ERROR: {}", e.0),
        };
        let text = self.clean(&text);
        self.out.push_str(&format!("\n## totals {file}\n{text}\n"));
    }

    /// Compares with (or, with `UPDATE_SNAPSHOTS=1`, rewrites) `snapshots/<name>.txt`.
    fn check(self) {
        let rel = format!("agent/tests/snapshots/{}.txt", self.name);
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("{}.txt", self.name));
        let got = self.out;
        let update = std::env::var("UPDATE_SNAPSHOTS").is_ok_and(|v| !v.is_empty() && v != "0");
        let want = std::fs::read_to_string(&file).ok();
        let _ = std::fs::remove_dir_all(&self.dir);
        if want.as_deref() == Some(got.as_str()) {
            return;
        }
        if update {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, &got).unwrap();
            eprintln!("updated {rel}");
            return;
        }
        match want {
            None => panic!(
                "no snapshot {rel} yet; create it with UPDATE_SNAPSHOTS=1 (make quick-update)\n--- output ---\n{got}"
            ),
            Some(want) => panic!(
                "snapshot {rel} differs (- stored, + now); accept with UPDATE_SNAPSHOTS=1 (make quick-update)\n{}",
                diff(&want, &got)
            ),
        }
    }
}

/// A line diff (LCS) with two lines of context around each change; `...` between hunks.
fn diff(a: &str, b: &str) -> String {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut l = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            l[i][j] = if a[i] == b[j] { l[i + 1][j + 1] + 1 } else { l[i + 1][j].max(l[i][j + 1]) };
        }
    }
    // (kind, line): ' ' same, '-' only in a, '+' only in b
    let mut ops: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if j < m && (i == n || l[i][j + 1] > l[i + 1][j]) {
            ops.push(('+', b[j]));
            j += 1;
        } else {
            ops.push(('-', a[i]));
            i += 1;
        }
    }
    let near = |k: usize| ops[k.saturating_sub(2)..(k + 3).min(ops.len())].iter().any(|o| o.0 != ' ');
    let mut out = String::new();
    let mut skipped = false;
    for (k, (c, line)) in ops.iter().enumerate() {
        if near(k) {
            if skipped {
                out.push_str("...\n");
                skipped = false;
            }
            out.push_str(&format!("{c} {line}\n"));
        } else {
            skipped = true;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Workbook helpers
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum C<'a> {
    S(&'a str),
    N(f64),
    B(bool),
}
use C::*;

fn rows(ws: &mut Worksheet, top: u32, left: u16, rows: &[Vec<C>]) {
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            let (r, c) = (top + r as u32, left + c as u16);
            match *v {
                S(s) => {
                    ws.write_string(r, c, s).unwrap();
                }
                N(x) => {
                    ws.write_number(r, c, x).unwrap();
                }
                B(b) => {
                    ws.write_boolean(r, c, b).unwrap();
                }
            }
        }
    }
}

fn sheet<'a>(wb: &'a mut Workbook, name: &str) -> &'a mut Worksheet {
    let ws = wb.add_worksheet();
    ws.set_name(name).unwrap();
    ws
}

// ---------------------------------------------------------------------------------------------
// Features
// ---------------------------------------------------------------------------------------------

/// Total rows: a grand total, "of which" rows, an average row, and subtotals above their groups.
#[test]
fn total_rows() {
    let mut s = Snap::new("total_rows");
    let mut wb = Workbook::new();
    rows(
        sheet(&mut wb, "Sales"),
        0,
        0,
        &[
            vec![S("Region"), S("Sales"), S("Units")],
            vec![S("North"), N(500.0), N(50.0)],
            vec![S("davon Inland"), N(300.0), N(20.0)],
            vec![S("South"), N(200.0), N(10.0)],
            vec![S("  of which: exports"), N(80.0), N(4.0)],
            vec![S("West"), N(100.0), N(5.0)],
            vec![S("East"), N(200.0), N(15.0)],
            vec![S("Average"), N(250.0), N(20.0)],
            vec![S("Total"), N(1000.0), N(80.0)],
        ],
    );
    rows(
        sheet(&mut wb, "Nights"),
        0,
        0,
        &[
            vec![S("Herkunft"), S("Nächte"), S("Ankünfte")],
            vec![S("Insgesamt"), N(1000.0), N(310.0)],
            vec![S("Österreich gesamt"), N(600.0), N(200.0)],
            vec![S("Wien"), N(400.0), N(120.0)],
            vec![S("Tirol"), N(200.0), N(80.0)],
            vec![S("Deutschland gesamt"), N(300.0), N(100.0)],
            vec![S("Bayern"), N(250.0), N(70.0)],
            vec![S("Berlin"), N(50.0), N(30.0)],
            vec![S("Ausland sonst"), N(100.0), N(10.0)],
        ],
    );
    s.save("totals.xlsx", &mut wb);
    s.open("totals.xlsx");
    s.query("totals.xlsx", "SELECT Region, Sales FROM T1 ORDER BY Sales DESC");
    s.query("totals.xlsx", "SELECT Region, Sales, _row_type FROM T1_all ORDER BY _src_row");
    s.query("totals.xlsx", "SELECT SUM(\"Nächte\") AS n FROM T2");
    s.totals("totals.xlsx");
    s.check();
}

/// Periods: wide years become a long view; text periods ('Jan-24') and averages ('Ø 22') as columns.
#[test]
fn periods() {
    let mut s = Snap::new("periods");
    let mut csv = String::from("Country,2019,2020,2021,2022,2023\n");
    for (i, c) in ["Austria", "Belgium", "Croatia", "Denmark", "Estonia", "Finland"].iter().enumerate() {
        let b = 100.0 + 13.0 * i as f64;
        csv.push_str(&format!("{c},{},{},{},{},{}\n", b, b + 1.5, b + 3.0, b + 2.0, b + 4.5));
    }
    s.write("gdp.csv", &csv);
    let mut wb = Workbook::new();
    let mut data = vec![vec![S("Product"), S("Ø 22"), S("Jan-24"), S("Feb-24"), S("Mar-24"), S("Apr-24")]];
    let products = ["Apples", "Pears", "Plums", "Cherries", "Grapes"];
    for (i, p) in products.iter().enumerate() {
        let b = 10.0 * (i + 1) as f64;
        data.push(vec![S(p), N(b), N(b + 1.0), N(b + 2.0), N(b + 3.0), N(b + 4.0)]);
    }
    rows(sheet(&mut wb, "Prices"), 0, 0, &data);
    s.save("prices.xlsx", &mut wb);
    s.open("gdp.csv");
    s.query("gdp.csv", "SELECT * FROM T1_long WHERE Country = 'Croatia' ORDER BY 2");
    s.open("prices.xlsx");
    s.describe("prices.xlsx", "T1");
    s.check();
}

/// Entity columns: `"<id> - <name>"` split into `_id` / `_name`, and separate ID + name columns.
#[test]
fn entities() {
    let mut s = Snap::new("entities");
    let buyers = [
        "503933813 - IP - Infraestruturas de Portugal, S.A.",
        "503933813 - Infraestruturas de Portugal, S. A.",
        "503933813 - INFRAESTRUTURAS DE PORTUGAL SA",
        "503278602 - Metro do Porto, S. A.",
        "600012662 - Ministério da Defesa Nacional - Marinha",
    ];
    let mut csv = String::from("buyer,supplier,price,signed\n");
    for i in 0..30 {
        csv.push_str(&format!("\"{}\",Supplier {},{},2020-{:02}-15\n", buyers[i % buyers.len()], i % 4, 1000 + i * 10, 1 + i % 12));
    }
    s.write("contracts.csv", &csv);
    let names = ["Acme GmbH", "Acme Gmbh.", "Beta AG", "Gamma KG", "Delta SE"];
    let ids = ["DE100", "DE100", "DE200", "DE300", "DE400"];
    let mut csv = String::from("customer_id,customer_name,amount\n");
    for i in 0..25 {
        csv.push_str(&format!("{},{},{}\n", ids[i % 5], names[i % 5], 50 + 7 * i));
    }
    s.write("customers.csv", &csv);
    s.open("contracts.csv");
    s.query("contracts.csv", "SELECT buyer_id, count(DISTINCT buyer_name) AS names, count(*) AS n FROM T1 GROUP BY buyer_id ORDER BY n DESC, buyer_id");
    s.query("contracts.csv", "SELECT buyer, count(*) AS n FROM T1 GROUP BY buyer ORDER BY buyer");
    s.open("customers.csv");
    s.query("customers.csv", "SELECT customer_name, SUM(amount) AS a FROM T1 GROUP BY customer_name ORDER BY customer_name");
    s.check();
}

/// Several files in one session: `f1` / `f2` prefixes and the JOIN KEYS line.
#[test]
fn multi_file_join() {
    let mut s = Snap::new("multi_file_join");
    let mut orders = String::from("orderID,employeeID,orderDate\n");
    let mut lines = String::from("orderID,productID,unitPrice,quantity\n");
    for i in 0..20 {
        let id = 1000 + i;
        orders.push_str(&format!("{id},{},{}-03-{:02}\n", 1 + i % 3, if i < 12 { 2021 } else { 2022 }, 1 + i % 28));
        for k in 0..(1 + i % 3) {
            lines.push_str(&format!("{id},{},{}.5,{}\n", 10 + k, 5 + k, 1 + i % 4));
        }
    }
    s.write("orders.csv", &orders);
    s.write("order_lines.csv", &lines);
    s.open("orders.csv");
    s.open("order_lines.csv");
    s.query(
        "orders.csv",
        "SELECT o.employeeID, SUM(l.unitPrice * l.quantity) AS revenue FROM T1 o JOIN f2.T1 l ON o.orderID = l.orderID \
         WHERE o.orderDate LIKE '2021%' GROUP BY 1 ORDER BY 2 DESC",
    );
    s.query("orders.csv", "SELECT nosuch FROM f2.T1");
    s.check();
}

/// Union views: the same table on several sheets, the sheet's subject only in the title.
#[test]
fn union_views() {
    let mut s = Snap::new("union_views");
    let mut wb = Workbook::new();
    for (k, state) in ["Burgenland", "Tirol", "Wien"].iter().enumerate() {
        let ws = sheet(&mut wb, &state[..3]);
        ws.write(0, 0, format!("Nächtigungen in {state} – nach Herkunftsland Juli 2026")).unwrap();
        let mut data = vec![vec![S("Herkunftsland"), S("Ankünfte"), S("Nächtigungen")]];
        let countries = ["Deutschland", "Italien", "Niederlande", "Schweiz", "Ungarn", "Polen"];
        for (r, c) in countries.iter().enumerate() {
            let a = (100 * (k + 1) + r * 7) as f64;
            data.push(vec![S(c), N(a), N(3.0 * a + r as f64)]);
        }
        rows(ws, 2, 0, &data);
    }
    s.save("states.xlsx", &mut wb);
    s.open("states.xlsx");
    s.query("states.xlsx", "SELECT * FROM U1 WHERE Herkunftsland = 'Italien' ORDER BY 1");
    s.check();
}

/// The quality line: duplicate rows, a column empty for one category, negative values.
#[test]
fn quality_line() {
    let mut s = Snap::new("quality_line");
    let mut csv = String::from("id,category,amount,discount\n");
    for i in 0..30 {
        let cat = ["Retail", "Wholesale", "Online"][i % 3];
        let discount = if cat == "Online" { String::new() } else { format!("{}", i % 5) };
        let amount = if i % 10 == 9 { -(i as i64) * 3 } else { 100 + i as i64 * 4 };
        csv.push_str(&format!("{},{cat},{amount},{discount}\n", 100 + i));
    }
    // two exact duplicates of earlier rows
    csv.push_str("101,Wholesale,104,1\n");
    csv.push_str("104,Wholesale,116,4\n");
    s.write("orders.csv", &csv);
    s.open("orders.csv");
    s.query("orders.csv", "SELECT category, count(*) AS n, count(discount) AS with_discount FROM T1 GROUP BY 1 ORDER BY 1");
    s.check();
}

/// Signs: a profit-and-loss sheet with costs as negatives and accounting parentheses.
#[test]
fn signs() {
    let mut s = Snap::new("signs");
    let mut wb = Workbook::new();
    rows(
        sheet(&mut wb, "P&L"),
        0,
        0,
        &[
            vec![S("Line item"), S("2023"), S("2024")],
            vec![S("Revenue"), N(1200.0), N(1350.0)],
            vec![S("Cost of sales"), N(-700.0), N(-760.0)],
            vec![S("Gross profit"), N(500.0), N(590.0)],
            vec![S("Operating expenses"), S("(200)"), S("(230)")],
            vec![S("Depreciation"), N(-50.0), N(-55.0)],
            vec![S("Operating profit"), N(250.0), N(305.0)],
        ],
    );
    s.save("pl.xlsx", &mut wb);
    s.open("pl.xlsx");
    s.query("pl.xlsx", "SELECT * FROM T1_all ORDER BY _src_row");
    s.check();
}

/// Units from titles: "in 1 000 EUR" and "(thousands)" above the table.
#[test]
fn units_from_titles() {
    let mut s = Snap::new("units_from_titles");
    let mut wb = Workbook::new();
    let ws = sheet(&mut wb, "Budget");
    ws.write(0, 0, "Municipal budget 2024, in 1 000 EUR").unwrap();
    rows(
        ws,
        2,
        0,
        &[
            vec![S("Department"), S("Planned"), S("Spent")],
            vec![S("Education"), N(5400.0), N(5120.0)],
            vec![S("Health"), N(3100.0), N(3310.0)],
            vec![S("Roads"), N(2200.0), N(1980.0)],
            vec![S("Culture"), N(800.0), N(760.0)],
            vec![S("Parks"), N(650.0), N(700.0)],
        ],
    );
    let ws = sheet(&mut wb, "People");
    ws.write(0, 0, "Population by district (thousands)").unwrap();
    rows(
        ws,
        2,
        0,
        &[
            vec![S("District"), S("Men"), S("Women")],
            vec![S("Centre"), N(41.2), N(44.8)],
            vec![S("North"), N(30.5), N(31.9)],
            vec![S("South"), N(27.1), N(28.3)],
            vec![S("East"), N(19.9), N(20.4)],
        ],
    );
    s.save("budget.xlsx", &mut wb);
    s.open("budget.xlsx");
    s.describe("budget.xlsx", "T1");
    s.query("budget.xlsx", "SELECT SUM(Spent) FROM T1");
    s.check();
}

/// Booleans as stored: TRUE/FALSE and yes/no columns become 1/0; a quoted 'TRUE' gets a hint.
#[test]
fn booleans() {
    let mut s = Snap::new("booleans");
    let mut wb = Workbook::new();
    let mut data = vec![vec![S("Employee"), S("Remote"), S("Certified"), S("Salary")]];
    let names = ["Ana", "Ben", "Cleo", "Dan", "Eva", "Finn", "Gus", "Hana"];
    for (i, n) in names.iter().enumerate() {
        data.push(vec![S(n), B(i % 2 == 0), S(if i % 3 == 0 { "yes" } else { "no" }), N(3000.0 + 250.0 * i as f64)]);
    }
    rows(sheet(&mut wb, "Staff"), 0, 0, &data);
    s.save("staff.xlsx", &mut wb);
    s.open("staff.xlsx");
    s.query("staff.xlsx", "SELECT count(*) AS n FROM T1 WHERE Remote = 'TRUE'");
    s.query("staff.xlsx", "SELECT Remote, Certified, count(*) AS n FROM T1 GROUP BY 1, 2 ORDER BY 1, 2");
    s.check();
}

/// Notes on empty or surprising results: a total label filtered on T1, a value the column lacks,
/// a close spelling, a number out of range, NULL from `a + b`.
#[test]
fn result_notes() {
    let mut s = Snap::new("result_notes");
    let mut csv = String::from("region,city,amount,bonus\n");
    let mut sum = 0;
    for i in 0..18 {
        let amount = 100 + i * 10;
        sum += amount;
        let bonus = if i % 3 == 0 { String::new() } else { (i * 2).to_string() };
        csv.push_str(&format!("{},City {i},{amount},{bonus}\n", ["North", "South", "East"][i % 3]));
    }
    csv.push_str(&format!("Total,,{sum},\n"));
    s.write("sales.csv", &csv);
    s.open("sales.csv");
    s.query("sales.csv", "SELECT amount FROM T1 WHERE region = 'Total'");
    s.query("sales.csv", "SELECT SUM(amount) FROM T1 WHERE region = 'West'");
    s.query("sales.csv", "SELECT count(*) FROM T1 WHERE region = 'north'");
    s.query("sales.csv", "SELECT * FROM T1 WHERE amount = 7");
    s.query("sales.csv", "SELECT city, amount + bonus AS x FROM T1 ORDER BY _src_row LIMIT 3");
    s.check();
}

/// SQL errors that teach: other dialects' functions, total-row labels used as columns, typos.
#[test]
fn sql_hints() {
    let mut s = Snap::new("sql_hints");
    let mut csv = String::from("buyer,price,signed\n");
    for i in 0..12 {
        csv.push_str(&format!("Buyer {},{},2020-{:02}-15\n", i % 3, 1000 + i * 10, 1 + i % 12));
    }
    s.write("contracts.csv", &csv);
    let mut wb = Workbook::new();
    let ws = sheet(&mut wb, "Flows");
    rows(
        ws,
        0,
        0,
        &[
            vec![S("Pipeline"), S("Receipts"), S("Deliveries")],
            vec![S("SEA ROBIN"), N(123.0), N(10.0)],
            vec![S("NGPL"), N(105.0), N(20.0)],
            vec![S("MAINLINE"), N(100.0), N(30.0)],
            vec![S("Total")],
        ],
    );
    ws.write_formula(4, 1, Formula::new("=SUM(B2:B4)").set_result("328")).unwrap();
    ws.write_formula(4, 2, Formula::new("=SUM(C2:C4)").set_result("60")).unwrap();
    s.save("flows.xlsx", &mut wb);
    s.open("contracts.csv");
    for q in [
        "SELECT MONTH(signed) AS m, count(*) FROM T1 GROUP BY m",
        "SELECT TOP 3 buyer FROM T1",
        "SELECT buyer FROM T1 WHERE buyer ILIKE '%1%'",
        "SELECT DATEDIFF(signed, '2020-01-01') FROM T1",
        "SELECT buyerr FROM T1",
        "DELETE FROM T1",
    ] {
        s.query("contracts.csv", q);
    }
    s.open("flows.xlsx");
    s.query("flows.xlsx", "SELECT \"Total\" FROM T1_all");
    s.query("flows.xlsx", "SELECT \"SEA ROBIN\" FROM T1");
    s.query("flows.xlsx", "SELECT Receits FROM T1");
    s.check();
}

/// Footnote marks on values and headers, and the footnotes below the table.
#[test]
fn footnotes() {
    let mut s = Snap::new("footnotes");
    let mut wb = Workbook::new();
    let ws = sheet(&mut wb, "Table 1");
    ws.write(0, 0, "Table 1. Hospital beds by region").unwrap();
    rows(
        ws,
        2,
        0,
        &[
            vec![S("Region"), S("Beds 2022"), S("Beds 2023¹")],
            vec![S("Alpha"), N(1200.0), N(1250.0)],
            vec![S("Beta"), N(830.0), S("845*")],
            vec![S("Gamma"), N(410.0), N(400.0)],
            vec![S("Delta²"), N(95.0), N(101.0)],
            vec![S("Epsilon"), N(660.0), S("702 p")],
            vec![],
            vec![S("¹ Provisional figures.")],
            vec![S("² Without private clinics.")],
            vec![S("* Estimated.")],
            vec![S("Source: Ministry of Health")],
        ],
    );
    s.save("beds.xlsx", &mut wb);
    s.open("beds.xlsx");
    s.query("beds.xlsx", "SELECT * FROM T1 ORDER BY 1");
    s.check();
}

/// Locale numbers: a European CSV (`;`, `1.234,56`) and a US one (`,`, `"1,234.56"`).
#[test]
fn locale_numbers() {
    let mut s = Snap::new("locale_numbers");
    let mut eu = String::from("Gemeinde;Einwohner;Fläche km²;Datum\n");
    let mut us = String::from("County,Population,Income,Date\n");
    let towns = ["Aachen", "Bonn", "Celle", "Dachau", "Erding", "Fulda", "Gotha"];
    for (i, t) in towns.iter().enumerate() {
        let pop = 12_345 + i * 101_010;
        eu.push_str(&format!("{t};{}.{:03};{},{:02};{:02}.03.2024\n", pop / 1000, pop % 1000, 40 + i * 7, 5 + i * 11, 1 + i));
        us.push_str(&format!("{t} County,\"{},{:03}\",\"{},{:03}.{:02}\",03/{:02}/2024\n", pop / 1000, pop % 1000, 40 + i, 100 + i * 37, 5 + i, 1 + i));
    }
    s.write("gemeinden.csv", &eu);
    s.write("counties.csv", &us);
    s.open("gemeinden.csv");
    s.query("gemeinden.csv", "SELECT SUM(Einwohner) AS p, MIN(Datum) AS d FROM T1");
    s.open("counties.csv");
    s.query("counties.csv", "SELECT SUM(Population) AS p, MAX(Income) AS i, MIN(Date) AS d FROM T1");
    // Latin-1 bytes with a decimal comma.
    s.write_bytes("latin1.csv", b"Stadt;Wert\nK\xf6ln;1,5\nM\xfcnchen;2,25\nN\xfcrnberg;3,75\nD\xfcsseldorf;4,0\n");
    s.open("latin1.csv");
    s.query("latin1.csv", "SELECT * FROM T1");
    s.check();
}

/// Side-by-side tables on one sheet, and stacked tables whose schema changes down the sheet.
#[test]
fn side_by_side_and_stacked() {
    let mut s = Snap::new("side_by_side_and_stacked");
    let mut wb = Workbook::new();
    let ws = sheet(&mut wb, "Two");
    let mut left = vec![vec![S("City"), S("Visitors")]];
    let mut right = vec![vec![S("Month"), S("Rain mm"), S("Sun h")]];
    let cities = ["Graz", "Linz", "Salzburg", "Innsbruck", "Klagenfurt", "Villach"];
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul"];
    for (i, c) in cities.iter().enumerate() {
        left.push(vec![S(c), N(1000.0 + 37.0 * i as f64)]);
    }
    for (i, m) in months.iter().enumerate() {
        right.push(vec![S(m), N(40.0 + 3.0 * i as f64), N(80.0 + 20.0 * i as f64)]);
    }
    rows(ws, 0, 0, &left);
    rows(ws, 0, 4, &right);
    let ws = sheet(&mut wb, "Stacked");
    ws.write(0, 0, "Table A: Staff by department").unwrap();
    let mut a = vec![vec![S("Department"), S("Staff"), S("Vacancies")]];
    for (i, d) in ["Finance", "IT", "Legal", "Sales", "Support"].iter().enumerate() {
        a.push(vec![S(d), N(10.0 + 4.0 * i as f64), N((i % 3) as f64)]);
    }
    rows(ws, 2, 0, &a);
    ws.write(9, 0, "Table B: Contracts by year").unwrap();
    let mut b = vec![vec![S("Year"), S("Contracts"), S("Value EUR"), S("Average EUR")]];
    for i in 0..6 {
        b.push(vec![N(2018.0 + i as f64), N(20.0 + 3.0 * i as f64), N(150000.0 + 12500.0 * i as f64), N(7500.0 - 100.0 * i as f64)]);
    }
    rows(ws, 11, 0, &b);
    s.save("layout.xlsx", &mut wb);
    s.open("layout.xlsx");
    s.query("layout.xlsx", "SELECT count(*) FROM T2");
    s.query("layout.xlsx", "SELECT * FROM T4 ORDER BY 1 LIMIT 3");
    s.check();
}

#[test]
fn diff_marks_changed_lines() {
    let d = diff("a\nb\nc\nd\ne\nf\ng\nh\n", "a\nb\nc\nD\ne\nf\ng\nh\n");
    assert_eq!(d, "...\n  b\n  c\n- d\n+ D\n  e\n  f\n");
}
