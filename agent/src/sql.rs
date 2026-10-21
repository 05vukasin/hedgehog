//! Read-only SQL over a normalized workbook: one in-memory SQLite database per file.
//!
//! Layout: table `T<n>_all` holds every row of table n (data, subtotal and total rows, with
//! `_row_type` and `_src_row`), view `T<n>` the data rows without `_row_type`. Values are typed:
//! integers INTEGER, numbers / currency / percent (as a fraction) REAL, dates ISO TEXT
//! (`YYYY-MM-DD`, or `YYYY-MM-DD hh:mm:ss`), booleans 0/1. An entity column (`"<id> - <name>"`,
//! [`crate::entity`]) is followed by `<col>_id` and `<col>_name`, split at load.
//!
//! After loading, the connection is locked down: an authorizer allows only SELECT, reads, a few
//! read-only pragmas and functions (no ATTACH, no writes, no schema changes, no transactions),
//! `query_only` is on, double-quoted strings are identifiers only, value sizes are limited, every
//! query runs under a time limit and the SQLite heap under a hard limit.

use std::time::{Duration, Instant};

use hh_core::normalize::{iso, ColRole, Dtype, Normalized, Out};
use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::{Value, ValueRef};
use rusqlite::Connection;

use crate::catalog::sql_names;
use crate::entity::{self, Entity};
use crate::text::{num, quote_ident, short};

/// Default per-query time limit.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// Rows counted past the displayed ones before the count is given as "more than".
pub const COUNT_CAP: usize = 1_000_000;
/// SQLite's default maximum number of columns is 2000.
const MAX_SQL_COLUMNS: usize = 1990;
/// Largest string or blob a query may build.
const MAX_VALUE_BYTES: i32 = 1_000_000;
/// Heap SQLite may use on top of the loaded data.
const HEAP_HEADROOM: i64 = 512 << 20;

/// Functions a query may not call even if the build has them.
const DENIED_FUNCTIONS: &[&str] = &["load_extension", "fts3_tokenizer", "readfile", "writefile", "edit", "sqlite_dbpage"];
/// Read-only pragmas allowed (schema introspection).
const ALLOWED_PRAGMAS: &[&str] = &["table_info", "table_xinfo", "table_list"];

pub struct Db {
    conn: Connection,
    /// (table id, columns left out because SQLite allows at most 2000).
    pub skipped_columns: Vec<(String, usize)>,
    /// (table number, its entity columns): each adds `<col>_id` and `<col>_name` to the views.
    pub entities: Vec<(usize, Vec<Entity>)>,
}

/// One SQL column of a table: a normalized column, or the ID / name part of entity column k.
enum Slot {
    Col(usize),
    Id(usize),
    Name(usize),
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Rows the query returned (counted up to [`COUNT_CAP`] or until the time limit).
    pub total: usize,
    /// False when counting stopped early: `total` is a lower bound.
    pub complete: bool,
    pub ms: f64,
}

fn sql_type(d: Dtype) -> &'static str {
    match d {
        Dtype::Integer | Dtype::Bool => "INTEGER",
        Dtype::Number | Dtype::Percent | Dtype::Currency => "REAL",
        Dtype::Date | Dtype::Datetime | Dtype::Text | Dtype::Empty => "TEXT",
    }
}

fn sql_value(v: &Out, d: Dtype) -> Value {
    match v {
        Out::Null => Value::Null,
        Out::Num(x) if matches!(d, Dtype::Date | Dtype::Datetime) => Value::Text(iso(*x)),
        Out::Num(x) if matches!(d, Dtype::Integer | Dtype::Bool) && x.fract() == 0.0 && x.abs() < 9e15 => {
            Value::Integer(*x as i64)
        }
        Out::Num(x) => Value::Real(*x),
        Out::Date(x) => Value::Text(iso(*x)),
        Out::Bool(b) => Value::Integer(i64::from(*b)),
        Out::Text(s) => Value::Text(s.clone()),
    }
}

impl Db {
    /// Loads every table of `norm` (one transaction, rows streamed through a prepared insert)
    /// and locks the connection down.
    pub fn load(norm: &Normalized) -> rusqlite::Result<Db> {
        set_heap_limit(0);
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA temp_store=MEMORY;")?;
        let mut skipped_columns = Vec::new();
        let mut entities = Vec::new();
        let tx = conn.unchecked_transaction()?;
        for t in &norm.tables {
            let names = sql_names(t);
            // Columns to load: all special ones, data/group columns up to SQLite's limit.
            let mut keep: Vec<usize> = Vec::new();
            let mut skipped = 0;
            for (i, c) in t.columns.iter().enumerate() {
                let special = matches!(c.role, ColRole::SrcRow | ColRole::RowType);
                if special || keep.len() < MAX_SQL_COLUMNS {
                    keep.push(i);
                } else {
                    skipped += 1;
                }
            }
            if skipped > 0 {
                skipped_columns.push((t.id(), skipped));
            }
            let id = t.id();
            let all = format!("{id}_all");
            let has_type = t.columns.iter().any(|c| c.role == ColRole::RowType);
            // Entity columns get their ID and name parts right after them.
            let ents: Vec<Entity> = entity::detect(t).into_iter().filter(|e| keep.contains(&e.col)).collect();
            let mut slots: Vec<Slot> = Vec::with_capacity(keep.len() + 2 * ents.len());
            for &i in &keep {
                slots.push(Slot::Col(i));
                if let Some(k) = ents.iter().position(|e| e.col == i) {
                    slots.push(Slot::Id(k));
                    slots.push(Slot::Name(k));
                }
            }
            let slot_name = |s: &Slot| match *s {
                Slot::Col(i) => names[i].clone(),
                Slot::Id(k) => ents[k].id_col.clone(),
                Slot::Name(k) => ents[k].name_col.clone(),
            };
            let defs: Vec<String> = slots
                .iter()
                .map(|s| {
                    let ty = if let Slot::Col(i) = *s { sql_type(t.columns[i].dtype) } else { "TEXT" };
                    format!("{} {ty}", quote_ident(&slot_name(s)))
                })
                .collect();
            let base = if has_type { all.clone() } else { id.clone() };
            tx.execute_batch(&format!("CREATE TABLE {} ({})", quote_ident(&base), defs.join(", ")))?;
            {
                let marks = vec!["?"; slots.len()].join(",");
                let mut ins = tx.prepare(&format!("INSERT INTO {} VALUES ({marks})", quote_ident(&base)))?;
                let mut row: Vec<Value> = Vec::with_capacity(slots.len());
                let mut parts: Vec<(Option<String>, Option<String>)> = vec![(None, None); ents.len()];
                for r in 0..t.rows() {
                    row.clear();
                    for (k, e) in ents.iter().enumerate() {
                        parts[k] = match t.columns[e.col].values.get(r) {
                            Some(Out::Text(s)) => entity::derived(s),
                            _ => (None, None),
                        };
                    }
                    for s in &slots {
                        row.push(match *s {
                            Slot::Col(i) => {
                                let c = &t.columns[i];
                                c.values.get(r).map_or(Value::Null, |v| sql_value(v, c.dtype))
                            }
                            Slot::Id(k) => parts[k].0.take().map_or(Value::Null, Value::Text),
                            Slot::Name(k) => parts[k].1.take().map_or(Value::Null, Value::Text),
                        });
                    }
                    ins.execute(rusqlite::params_from_iter(row.iter()))?;
                }
            }
            if has_type {
                let cols: Vec<String> = slots
                    .iter()
                    .filter(|s| !matches!(s, Slot::Col(i) if t.columns[*i].role == ColRole::RowType))
                    .map(|s| quote_ident(&slot_name(s)))
                    .collect();
                tx.execute_batch(&format!(
                    "CREATE VIEW {} AS SELECT {} FROM {} WHERE \"_row_type\" = 'data'",
                    quote_ident(&id),
                    cols.join(", "),
                    quote_ident(&all)
                ))?;
            } else {
                tx.execute_batch(&format!(
                    "CREATE VIEW {} AS SELECT *, 'data' AS \"_row_type\" FROM {}",
                    quote_ident(&all),
                    quote_ident(&id)
                ))?;
            }
            if !ents.is_empty() {
                entities.push((t.n, ents));
            }
        }
        crate::views::create_all(&tx, norm)?; // B20: T<n>_long views (views.rs)
        tx.commit()?;
        lock_down(&conn)?;
        set_heap_limit(memory_used() + HEAP_HEADROOM);
        Ok(Db { conn, skipped_columns, entities })
    }

    /// Runs one read-only statement; returns up to `limit` rows (cells cut to 80 characters) and
    /// the number of rows the query produced.
    pub fn query(&self, sql: &str, limit: usize, timeout: Duration) -> Result<QueryResult, String> {
        let q = sql.trim().trim_end_matches(';').trim();
        if q.is_empty() {
            return Err("empty SQL; write one SELECT statement".into());
        }
        let first = first_keyword(q);
        if !matches!(first.as_str(), "select" | "with" | "values") {
            return Err(format!(
                "read-only: only SELECT / WITH queries are allowed (got {})",
                if first.is_empty() { "nothing".to_string() } else { first.to_uppercase() }
            ));
        }
        let t0 = Instant::now();
        let deadline = t0 + timeout;
        self.conn
            .progress_handler(1000, Some(move || Instant::now() > deadline))
            .map_err(|e| e.to_string())?;
        let mut stmt = self.conn.prepare(q).map_err(|e| match e {
            rusqlite::Error::MultipleStatement => "only one statement per query".to_string(),
            other => sql_error(&other),
        })?;
        if !stmt.readonly() {
            return Err("read-only: only SELECT / WITH queries are allowed".into());
        }
        let header: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let ncols = header.len();
        let mut rows_out = Vec::new();
        let mut total = 0usize;
        let mut complete = true;
        let mut rows = stmt.query([]).map_err(|e| sql_error(&e))?;
        loop {
            match rows.next() {
                Ok(Some(row)) => {
                    if rows_out.len() < limit {
                        let mut vals = Vec::with_capacity(ncols);
                        for i in 0..ncols {
                            vals.push(cell_text(row.get_ref(i).unwrap_or(ValueRef::Null)));
                        }
                        rows_out.push(vals);
                    }
                    total += 1;
                    if total >= limit + COUNT_CAP {
                        complete = false;
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) if is_interrupt(&e) && !rows_out.is_empty() => {
                    complete = false;
                    break;
                }
                Err(e) if is_interrupt(&e) => {
                    return Err(format!(
                        "query stopped after the {:.0} s time limit; simplify it (aggregate, filter with WHERE, avoid large joins)",
                        timeout.as_secs_f64()
                    ));
                }
                Err(e) => return Err(sql_error(&e)),
            }
        }
        Ok(QueryResult { header, rows: rows_out, total, complete, ms: t0.elapsed().as_secs_f64() * 1000.0 })
    }
}

fn is_interrupt(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::OperationInterrupted)
}

fn sql_error(e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(f, Some(msg)) if f.code == rusqlite::ErrorCode::AuthorizationForStatementDenied => {
            format!("not allowed: {msg} (read-only: SELECT over the tables only)")
        }
        rusqlite::Error::SqliteFailure(_, Some(msg)) => msg.clone(),
        other => other.to_string(),
    }
}

fn first_keyword(q: &str) -> String {
    let mut s = q;
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("--") {
            s = rest.split_once('\n').map_or("", |x| x.1);
        } else if let Some(rest) = s.strip_prefix("/*") {
            s = rest.split_once("*/").map_or("", |x| x.1);
        } else if let Some(rest) = s.strip_prefix('(') {
            s = rest;
        } else {
            break;
        }
    }
    s.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase()
}

fn cell_text(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => String::new(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(x) => num(x),
        ValueRef::Text(t) => short(&String::from_utf8_lossy(t), 80),
        ValueRef::Blob(b) => format!("<blob {} bytes>", b.len()),
    }
}

fn authorize(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::Select | AuthAction::Read { .. } | AuthAction::Recursive => Authorization::Allow,
        AuthAction::Function { function_name } => {
            if DENIED_FUNCTIONS.contains(&function_name.to_ascii_lowercase().as_str()) {
                Authorization::Deny
            } else {
                Authorization::Allow
            }
        }
        // These take a table name as their argument and only read the schema.
        AuthAction::Pragma { pragma_name, .. }
            if ALLOWED_PRAGMAS.contains(&pragma_name.to_ascii_lowercase().as_str()) =>
        {
            Authorization::Allow
        }
        _ => Authorization::Deny,
    }
}

fn lock_down(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA query_only = ON;")?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)?;
    // "Unknown col" must be an error, not a string literal.
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)?;
    conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES)?;
    conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 100_000)?;
    conn.authorizer(Some(authorize))?;
    Ok(())
}

fn memory_used() -> i64 {
    // SAFETY: plain query of a global counter.
    unsafe { rusqlite::ffi::sqlite3_memory_used() }
}

/// Process-wide hard limit of SQLite's heap (0 = none).
fn set_heap_limit(bytes: i64) {
    // SAFETY: sets a global limit; thread-safe in SQLite.
    unsafe {
        rusqlite::ffi::sqlite3_hard_heap_limit64(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hh_core::normalize::{Column, Normalized, RowType, Table, Transforms};
    use hh_core::Rect;

    fn col(name: &str, role: ColRole, dtype: Dtype, values: Vec<Out>) -> Column {
        Column {
            name: name.into(),
            role,
            source_col: Some(0),
            original_header: vec![],
            dtype,
            unit: None,
            currency: None,
            decimal: None,
            date_order: None,
            ambiguous: 0,
            values,
        }
    }

    pub(crate) fn sample() -> Normalized {
        let n = 300;
        let t = Table {
            n: 1,
            sheet: "T1_x".into(),
            source_sheet: "Sheet1".into(),
            rect: Rect { top: 0, left: 0, bottom: n as u32 + 1, right: 2 },
            header_top: 0,
            header_rows: 1,
            title: None,
            notes: vec![],
            confidence: 0.9,
            columns: vec![
                col("City", ColRole::Data, Dtype::Text, (0..n).map(|i| Out::Text(format!("c{i}"))).collect()),
                col("Revenue / Q1", ColRole::Data, Dtype::Number, (0..n).map(|i| Out::Num(i as f64 + 0.5)).collect()),
                col("Day", ColRole::Data, Dtype::Date, (0..n).map(|i| Out::Date(45000.0 + i as f64)).collect()),
                col("_src_row", ColRole::SrcRow, Dtype::Integer, (0..n).map(|i| Out::Num(i as f64 + 2.0)).collect()),
                col(
                    "_row_type",
                    ColRole::RowType,
                    Dtype::Text,
                    (0..n).map(|i| Out::Text(if i == n - 1 { "total" } else { "data" }.into())).collect(),
                ),
            ],
            src_rows: (1..=n as u32).collect(),
            row_types: (0..n).map(|i| if i == n - 1 { RowType::Total } else { RowType::Data }).collect(),
            transforms: Transforms::default(),
            warnings: vec![],
        };
        Normalized { tables: vec![t], ..Default::default() }
    }

    fn db() -> Db {
        Db::load(&sample()).unwrap()
    }

    #[test]
    fn select_works_and_counts() {
        let d = db();
        let r = d.query("SELECT City, \"Revenue / Q1\", Day FROM T1", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(r.header, ["City", "Revenue / Q1", "Day"]);
        assert_eq!(r.rows.len(), 5);
        assert_eq!(r.total, 299);
        assert!(r.complete);
        assert_eq!(r.rows[0], ["c0", "0.5", "2023-03-15"]);
        let all = d.query("select count(*) from T1_all where _row_type = 'total'", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(all.rows[0][0], "1");
        let w = d.query("WITH x AS (SELECT 1 AS a) SELECT a FROM x;", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(w.rows[0][0], "1");
    }

    #[test]
    fn writes_and_escapes_are_rejected() {
        let d = db();
        for q in [
            "INSERT INTO T1_all (City) VALUES ('x')",
            "UPDATE T1_all SET City = 'x'",
            "DELETE FROM T1_all",
            "DROP TABLE T1_all",
            "CREATE TABLE x (a)",
            "ATTACH DATABASE '/tmp/x.db' AS x",
            "PRAGMA writable_schema = ON",
            "PRAGMA query_only = OFF",
            "BEGIN",
            "SELECT 1; DELETE FROM T1_all",
            "VACUUM INTO '/tmp/x.db'",
            "SELECT load_extension('/tmp/x')",
            "WITH x AS (SELECT 1) DELETE FROM T1_all",
        ] {
            assert!(d.query(q, 5, DEFAULT_TIMEOUT).is_err(), "{q} should be rejected");
        }
        // Even if the keyword check is bypassed, the authorizer denies.
        assert!(d.conn.execute("DELETE FROM T1_all", []).is_err());
        assert!(d.conn.execute_batch("ATTACH DATABASE ':memory:' AS x").is_err());
        assert!(d.conn.execute_batch("PRAGMA query_only = OFF").is_err());
        let n = d.query("SELECT count(*) FROM T1_all", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(n.rows[0][0], "300");
        // read-only pragma functions are fine
        assert!(d.query("SELECT name FROM pragma_table_info('T1')", 50, DEFAULT_TIMEOUT).is_ok());
    }

    #[test]
    fn unknown_column_is_an_error_not_a_string() {
        let d = db();
        let e = d.query("SELECT \"Revenue Q1\" FROM T1", 5, DEFAULT_TIMEOUT).unwrap_err();
        assert!(e.contains("no such column"), "{e}");
    }

    #[test]
    fn time_limit_and_value_size() {
        let d = db();
        let e = d
            .query(
                "WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM r) SELECT count(*) FROM r",
                5,
                Duration::from_millis(200),
            )
            .unwrap_err();
        assert!(e.contains("time limit"), "{e}");
        assert!(d.query("SELECT length(zeroblob(100000000))", 5, DEFAULT_TIMEOUT).is_err());
        // Endless rows: shown rows come back, the count stops.
        let r = d
            .query("WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM r) SELECT i FROM r", 3, Duration::from_millis(200))
            .unwrap();
        assert_eq!(r.rows.len(), 3);
        assert!(!r.complete);
    }

    #[test]
    fn row_cap() {
        let d = db();
        let r = d.query("SELECT * FROM T1_all", 200, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(r.rows.len(), 200);
        assert_eq!(r.total, 300);
    }
}
