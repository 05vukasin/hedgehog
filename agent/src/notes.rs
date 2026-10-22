//! Notes under a query result that explain it (B75, B88).
//!
//! An empty result (no rows, or one row of NULLs or a zero count) is easy to read as "0". When a
//! WHERE filter compares a column with a value, these notes say why nothing matched, checked with
//! small probe queries on the same database:
//!
//! - the value is only on total / subtotal rows, which are in `T<n>_all`, not in `T<n>`: the note
//!   says so and gives the rows of the same query run on `T<n>_all`;
//! - the column has no such value: the values that contain it or are close to it
//!   (`'Somalia'` → `'Somalia, Fed. Rep.'`), or else its values (≤ 20) or its most frequent ones,
//!   so "no NYPD rows in this file" is not reported as "$0";
//! - a quoted value against a number or boolean column (`Returned = 'TRUE'` where booleans are
//!   stored 1/0).
//!
//! A non-empty result over `T<n>` whose filter value also labels total rows that are only in
//! `T<n>_all` gets a one-line note too. And a result with empty values from `a + b` over columns
//! that are sometimes empty says that `+` gives NULL there (use `COALESCE` or `total()`).
//!
//! Everything works on the database alone (table and column names from `pragma_table_info`), so
//! it covers the tables of other files in a multi-file query (`f2.T1`) as well.

use std::time::Duration;

use crate::sql::{Db, QueryResult};
use crate::sqlscan::{self, Lit, TableRef, Tok, Token};
use crate::text::{csv_row, quote_ident, short, similarity, thousands};

/// Time limit of one probe query.
const PROBE_MS: u64 = 500;
/// Filters looked at per query.
const MAX_FILTERS: usize = 4;
/// Values of one filter looked at.
const MAX_VALUES: usize = 3;
/// A column's values are listed in full up to this many distinct values.
const LIST_ALL: usize = 20;
/// Rows of the same query on `T<n>_all` shown.
const ALL_ROWS: usize = 5;
/// Characters of all notes at most.
const MAX_NOTES_CHARS: usize = 2500;

/// One table the statement reads, as the probes address it.
struct Source {
    /// How the statement names it (`T1`, `f2.T1_all`): for the notes.
    shown: String,
    /// The table's name as written (`T1`, `t1_all`) and its schema (`f2`).
    name: String,
    schema: Option<String>,
    /// The data view, qualified and quoted (`"T1"`, `"f2"."T1"`).
    data: String,
    /// The view with every row and `_row_type`.
    all: String,
    shown_all: String,
    /// The statement reads the `_all` view itself.
    reads_all: bool,
    /// Column names and declared types of the `_all` view.
    columns: Vec<(String, String)>,
}

fn probe(db: &Db, sql: &str, limit: usize, timeout: Duration) -> Option<QueryResult> {
    db.query(sql, limit, timeout.min(Duration::from_millis(PROBE_MS))).ok()
}

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn qualified(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", quote_ident(s), quote_ident(name)),
        None => quote_ident(name),
    }
}

/// The tables of `refs` that are views of ours (`T<n>`, `T<n>_all`, `T<n>_long`, `U<n>`, …) and
/// have an `_all` view with `_row_type`.
fn sources(db: &Db, refs: &[TableRef], timeout: Duration) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::new();
    for r in refs {
        let n = &r.name;
        let ours = n.strip_prefix(['T', 't', 'U', 'u']).is_some_and(|d| d.starts_with(|c: char| c.is_ascii_digit()));
        if !ours {
            continue;
        }
        let lower = n.to_ascii_lowercase();
        let (base, reads_all) = match lower.strip_suffix("_all") {
            Some(b) => (n[..b.len()].to_string(), true),
            None => (n.clone(), false),
        };
        let all_name = format!("{base}_all");
        let schema = r.schema.as_deref();
        let info = match schema {
            Some(s) => format!("SELECT name, type FROM pragma_table_info({}, {})", sql_lit(&all_name), sql_lit(s)),
            None => format!("SELECT name, type FROM pragma_table_info({})", sql_lit(&all_name)),
        };
        let Some(cols) = probe(db, &info, 2100, timeout) else { continue };
        let columns: Vec<(String, String)> = cols.rows.into_iter().map(|r| (r[0].clone(), r[1].clone())).collect();
        if !columns.iter().any(|c| c.0 == "_row_type") {
            continue;
        }
        let prefix = schema.map(|s| format!("{s}.")).unwrap_or_default();
        out.push(Source {
            shown: format!("{prefix}{n}"),
            name: n.clone(),
            schema: r.schema.clone(),
            data: qualified(schema, &base),
            all: qualified(schema, &all_name),
            shown_all: format!("{prefix}{all_name}"),
            reads_all,
            columns,
        });
    }
    out
}

fn is_numeric_type(ty: &str) -> bool {
    let t = ty.to_ascii_uppercase();
    t.contains("INT") || t.contains("REAL") || t.contains("NUM") || t.contains("FLOA") || t.contains("DOUB")
}

/// The source holding `column` (the one the qualifier names first).
fn source_of<'a>(srcs: &'a [Source], qualifier: Option<&str>, column: &str) -> Option<(&'a Source, &'a (String, String))> {
    let has = |s: &'a Source| s.columns.iter().find(|c| c.0.eq_ignore_ascii_case(column)).map(|c| (s, c));
    if let Some(q) = qualifier
        && let Some(hit) = srcs.iter().filter(|s| s.name.eq_ignore_ascii_case(q)).find_map(has)
    {
        return Some(hit);
    }
    srcs.iter().find_map(has)
}

/// Whether the result reads as "nothing matched": no rows, one row of NULLs, or a zero count.
pub fn looks_empty(res: &QueryResult, counts: bool) -> bool {
    if res.total == 0 {
        return true;
    }
    if res.total != 1 || res.rows.len() != 1 {
        return false;
    }
    let row = &res.rows[0];
    row.iter().all(String::is_empty)
        || (row.len() == 1 && row[0] == "0" && (counts || res.header[0].to_ascii_lowercase().starts_with("count(")))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

fn values_list(rows: &[Vec<String>]) -> String {
    rows.iter()
        .map(|r| {
            let n: usize = r.get(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            format!("{} ({})", sql_lit(&short(&r[0], 60)), plural(n, "row", "rows"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The statement with every reference to the data view `name` read from its `_all` view.
fn on_all(sql: &str, toks: &[Token], src: &Source) -> String {
    let mut out = String::new();
    let mut at = 0;
    for (i, t) in toks.iter().enumerate() {
        let named = matches!(&t.tok, Tok::Ident { name, .. } if name.eq_ignore_ascii_case(&src.name));
        let after_dot = i > 0 && toks[i - 1].is_op(".");
        let hit = named
            && match &src.schema {
                None => !after_dot,
                Some(s) => after_dot && i >= 2 && toks[i - 2].ident().is_some_and(|x| x.eq_ignore_ascii_case(s)),
            };
        if hit {
            out.push_str(&sql[at..t.start]);
            out.push_str(&quote_ident(&format!("{}_all", src.name)));
            at = t.end;
        }
    }
    out.push_str(&sql[at..]);
    out
}

/// Notes for the result `res` of `sql` (empty when there is nothing to say).
pub fn result_notes(db: &Db, sql: &str, res: &QueryResult, timeout: Duration) -> Vec<String> {
    let toks = sqlscan::tokens(sql);
    let ctes: Vec<String> = sqlscan::derived(sql, &toks).into_iter().filter_map(|d| d.name).collect();
    let refs: Vec<TableRef> = sqlscan::table_refs(&toks)
        .into_iter()
        .filter(|r| r.schema.is_some() || !ctes.iter().any(|c| c.eq_ignore_ascii_case(&r.name)))
        .collect();
    if refs.is_empty() {
        return Vec::new();
    }
    let counts = toks.windows(2).any(|w| w[0].is_kw("count") && w[1].is_op("("));
    let empty = looks_empty(res, counts);
    let filters = sqlscan::filters(&toks, MAX_FILTERS);
    let plus = sqlscan::plus_columns(&toks);
    let null_cells = res.rows.iter().filter(|r| r.iter().any(String::is_empty)).count();
    if filters.is_empty() && (plus.is_empty() || null_cells == 0) && !(empty && sqlscan::has_where(&toks)) {
        return Vec::new();
    }
    let srcs = sources(db, &refs, timeout);
    if srcs.is_empty() {
        return Vec::new();
    }
    let mut notes: Vec<String> = Vec::new();
    let mut only_in_all: Option<(&Source, bool)> = None;
    for f in &filters {
        let Some((src, (col, ty))) = source_of(&srcs, f.qualifier.as_deref(), &f.column) else { continue };
        let colq = quote_ident(col);
        let at = format!("{}.{}", src.shown, crate::text::ident(col));
        for v in f.values.iter().take(MAX_VALUES) {
            // On a non-empty result only total-row labels matter: text values.
            if !empty && matches!(v, Lit::Num(_)) {
                continue;
            }
            let (text, lit) = match v {
                Lit::Text(s) => (s.clone(), sql_lit(s)),
                Lit::Num(n) => (n.clone(), n.clone()),
            };
            let cond = if f.op == "like" { format!("{colq} LIKE {lit}") } else { format!("{colq} = {lit}") };
            let q = format!("SELECT \"_row_type\", count(*) FROM {} WHERE {cond} GROUP BY 1", src.all);
            let Some(r) = probe(db, &q, 20, timeout) else { continue };
            let count = |pred: &dyn Fn(&str) -> bool| -> usize {
                r.rows.iter().filter(|x| pred(&x[0])).map(|x| x[1].parse::<usize>().unwrap_or(0)).sum()
            };
            let data = count(&|t| t == "data");
            let other = count(&|t| t != "data");
            let kinds: Vec<String> = r.rows.iter().filter(|x| x[0] != "data").map(|x| format!("'{}'", x[0])).collect();
            let shown_v = if matches!(v, Lit::Text(_)) { sql_lit(&short(&text, 60)) } else { text.clone() };
            if data == 0 && other > 0 && !src.reads_all {
                if empty {
                    notes.push(format!(
                        "note: {at} = {shown_v} is only on {} (_row_type {}), which {} in {}, not in {}",
                        plural(other, "total row", "total rows"),
                        kinds.join("/"),
                        if other == 1 { "is" } else { "are" },
                        src.shown_all,
                        src.shown
                    ));
                    only_in_all.get_or_insert((src, true));
                } else {
                    notes.push(format!(
                        "note: {at} = {shown_v} also matches {} only in {} (_row_type {}); this result has data rows only",
                        plural(other, "total row", "total rows"),
                        src.shown_all,
                        kinds.join("/")
                    ));
                }
            } else if data > 0 && other > 0 && !src.reads_all && !empty {
                notes.push(format!(
                    "note: {at} = {shown_v} also labels {} that {} only in {} (_row_type {}); this result has the data rows only",
                    plural(other, "total row", "total rows"),
                    if other == 1 { "is" } else { "are" },
                    src.shown_all,
                    kinds.join("/")
                ));
            } else if data > 0 && other > 0 && !src.reads_all && empty {
                // The filters together may match total rows only.
                only_in_all.get_or_insert((src, false));
            } else if data == 0 && other == 0 && empty {
                notes.push(missing_value_note(db, src, col, ty, &at, &text, &shown_v, v, f.op, timeout));
            }
        }
    }
    if let Some((src, said)) = only_in_all {
        let q = on_all(sql, &toks, src);
        if let Some(r) = probe(db, &q, ALL_ROWS, timeout).filter(|r| !looks_empty(r, counts)) {
            if !said {
                notes.push(format!("note: no data row matches these filters, but total rows in {} do (not in {})", src.shown_all, src.shown));
            }
            let mut s = format!("The same query on {} (the total rows included):\n{}", src.shown_all, csv_row(&r.header));
            for row in &r.rows {
                s.push('\n');
                s.push_str(&csv_row(row));
            }
            if r.total > r.rows.len() {
                s.push_str(&format!("\n(first {} of {} rows)", r.rows.len(), thousands(r.total)));
            }
            notes.push(s);
        }
    }
    if empty && notes.is_empty() && res.total == 1 && res.rows[0].iter().all(String::is_empty) && sqlscan::has_where(&toks) {
        notes.push("note: the result is empty (NULL): no row matched the filter, or the values there are empty — not 0".into());
    }
    // B88: a + b is NULL when one of them is empty.
    if !plus.is_empty() && null_cells > 0 {
        let mut sparse: Vec<String> = Vec::new();
        for c in plus.iter().take(6) {
            let Some((src, (col, _))) = source_of(&srcs, None, c) else { continue };
            let q = format!("SELECT count(*) FROM {} WHERE {} IS NULL", src.data, quote_ident(col));
            if let Some(n) = probe(db, &q, 1, timeout).and_then(|r| r.rows.first().and_then(|x| x[0].parse::<usize>().ok())).filter(|&n| n > 0) {
                sparse.push(format!("{} empty in {}", crate::text::ident(col), plural(n, "row", "rows")));
            }
        }
        if !sparse.is_empty() {
            notes.push(format!(
                "note: {} have empty values: a + b is NULL when any part is empty ({}); use COALESCE(col, 0) for each part, or total(col) to sum a column",
                plural(null_cells, "result row", "result rows"),
                sparse.join(", ")
            ));
        }
    }
    let mut size = 0;
    notes.retain(|n| {
        size += n.len() + 1;
        size <= MAX_NOTES_CHARS
    });
    notes
}

/// The note for a filter value the column does not have: close values, or the column's values.
#[allow(clippy::too_many_arguments)]
fn missing_value_note(
    db: &Db,
    src: &Source,
    col: &str,
    ty: &str,
    at: &str,
    text: &str,
    shown_v: &str,
    v: &Lit,
    op: &str,
    timeout: Duration,
) -> String {
    let colq = quote_ident(col);
    let head = format!("note: {at} has no {shown_v} — 0 rows here means the file has no such rows, not a value of 0");
    // A quoted value against numbers (booleans are stored 1/0).
    if matches!(v, Lit::Text(_)) && is_numeric_type(ty) {
        let q = format!("SELECT {colq}, count(*) FROM {} WHERE {colq} IS NOT NULL GROUP BY 1 ORDER BY 2 DESC LIMIT 3", src.all);
        let vals = probe(db, &q, 3, timeout).map(|r| r.rows).unwrap_or_default();
        let boolish = !vals.is_empty() && vals.iter().all(|r| r[0] == "0" || r[0] == "1");
        let how = if boolish {
            format!("{} is a yes/no column stored as 1/0: use {} = 1 (or = 0)", crate::text::ident(col), crate::text::ident(col))
        } else {
            format!("{} holds numbers: compare without quotes", crate::text::ident(col))
        };
        return format!("{head}; {how}");
    }
    let needle: String = if op == "like" { text.replace(['%', '_'], " ").trim().to_string() } else { text.to_string() };
    if matches!(v, Lit::Num(_)) || is_numeric_type(ty) {
        let q = format!("SELECT min({colq}), max({colq}) FROM {}", src.data);
        if let Some(r) = probe(db, &q, 1, timeout).filter(|r| r.rows.first().is_some_and(|x| !x[0].is_empty())) {
            return format!("{head}; its values run from {} to {}", r.rows[0][0], r.rows[0][1]);
        }
        return head;
    }
    if !needle.is_empty() {
        let n = sql_lit(&needle);
        let q = format!(
            "SELECT {colq}, count(*) FROM {} WHERE {colq} LIKE '%' || {n} || '%' OR lower({colq}) = lower({n}) \
             OR (length({colq}) >= 3 AND instr(' ' || lower({n}) || ' ', ' ' || lower({colq}) || ' ') > 0) GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 6",
            src.all
        );
        if let Some(r) = probe(db, &q, 6, timeout).filter(|r| !r.rows.is_empty()) {
            return format!("{head}; values that contain it: {}", values_list(&r.rows));
        }
    }
    let distinct = probe(db, &format!("SELECT count(DISTINCT {colq}) FROM {}", src.all), 1, timeout)
        .and_then(|r| r.rows.first().and_then(|x| x[0].parse::<usize>().ok()))
        .unwrap_or(0);
    if distinct == 0 {
        return format!("{head}; the column is empty");
    }
    let q = format!("SELECT {colq}, count(*) FROM {} WHERE {colq} IS NOT NULL GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 5000", src.all);
    let Some(r) = probe(db, &q, 5000, timeout) else { return head };
    if distinct <= LIST_ALL {
        return format!("{head}; its {} values: {}", distinct, values_list(&r.rows));
    }
    let mut close: Vec<(f64, &Vec<String>)> =
        r.rows.iter().map(|x| (similarity(&needle, &x[0]), x)).filter(|(s, _)| *s >= 0.6).collect();
    close.sort_by(|a, b| b.0.total_cmp(&a.0));
    if !close.is_empty() {
        let rows: Vec<Vec<String>> = close.iter().take(5).map(|(_, x)| (*x).clone()).collect();
        return format!("{head}; closest values: {}", values_list(&rows));
    }
    let rows: Vec<Vec<String>> = r.rows.iter().take(8).cloned().collect();
    format!("{head}; most frequent of its {} values: {}", thousands(distinct), values_list(&rows))
}
