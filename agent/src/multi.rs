//! Several files in one session (B76).
//!
//! Each file opened in a session gets a number in the order it was opened; in SQL, `f<k>.T1` is
//! table T1 of file k (the files' databases attached under the schema names `f1`, `f2`, … by
//! [`crate::sql::Db::join`]), so one query joins orders with order lines kept in two files. A
//! plain `T1` stays the table of the file in `path`.
//!
//! With two or more files open, the `open_file` catalog ends with the session's files and the
//! keys that join them: columns of the same name in tables of different files whose values
//! overlap, one side unique (`f1.T1.orderID = f2.T1.orderID`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use hh_core::normalize::{Normalized, Table};

use crate::catalog::{data_cols, data_rows, is_numeric, out_text, sql_names};
use crate::text::{ident, short, thousands};

/// An open file of the session.
pub struct FileInfo {
    /// `f1`, `f2`, …
    pub prefix: String,
    /// The display name (relative to the root).
    pub name: String,
    pub norm: Arc<Normalized>,
}

/// Tables per file looked at for join keys.
const MAX_TABLES: usize = 40;
/// Rows per column read for join keys.
const MAX_ROWS: usize = 200_000;
/// Join keys listed.
const MAX_KEYS: usize = 4;

/// A column name compared across files: letters and digits, lower case.
fn key_name(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn ids(norm: &Normalized) -> String {
    let ids: Vec<String> = norm.tables.iter().map(Table::id).collect();
    match ids.len() {
        0 => "no tables".into(),
        1..=4 => ids.join(", "),
        n => format!("{}..{} ({n} tables)", ids[0], ids[n - 1]),
    }
}

/// The values of a column (data rows, as text) and how many rows have one.
fn values(t: &Table, col: usize) -> (HashSet<String>, usize) {
    let c = &t.columns[col];
    let mut set = HashSet::new();
    let mut n = 0;
    for r in data_rows(t).into_iter().take(MAX_ROWS) {
        let v = c.values.get(r).map(|v| out_text(v, c.dtype)).unwrap_or_default();
        let v = v.trim();
        if !v.is_empty() {
            n += 1;
            set.insert(v.to_string());
        }
    }
    (set, n)
}

struct Key {
    left: String,
    right: String,
    shared: usize,
    unique: String,
}

/// Columns that join tables of different files.
fn join_keys(files: &[FileInfo]) -> Vec<Key> {
    // (file, table index) → (key name → column index)
    let mut cols: HashMap<(usize, usize), Vec<(String, usize, String)>> = HashMap::new();
    for (f, info) in files.iter().enumerate() {
        for (k, t) in info.norm.tables.iter().enumerate().take(MAX_TABLES) {
            let names = sql_names(t);
            cols.insert((f, k), data_cols(t).map(|(i, _)| (key_name(&names[i]), i, names[i].clone())).filter(|x| !x.0.is_empty()).collect());
        }
    }
    let mut cache: HashMap<(usize, usize, usize), (HashSet<String>, usize)> = HashMap::new();
    let mut out: Vec<Key> = Vec::new();
    for fa in 0..files.len() {
        for fb in fa + 1..files.len() {
            for (ka, ta) in files[fa].norm.tables.iter().enumerate().take(MAX_TABLES) {
                for (kb, tb) in files[fb].norm.tables.iter().enumerate().take(MAX_TABLES) {
                    for (na, ia, sa) in &cols[&(fa, ka)] {
                        for (_, ib, sb) in cols[&(fb, kb)].iter().filter(|x| &x.0 == na) {
                            if is_numeric(ta.columns[*ia].dtype) != is_numeric(tb.columns[*ib].dtype) {
                                continue;
                            }
                            for (f, k, t, i) in [(fa, ka, ta, *ia), (fb, kb, tb, *ib)] {
                                cache.entry((f, k, i)).or_insert_with(|| values(t, i));
                            }
                            let (va, ra) = &cache[&(fa, ka, *ia)];
                            let (vb, rb) = &cache[&(fb, kb, *ib)];
                            let shared = va.intersection(vb).count();
                            let smaller = va.len().min(vb.len());
                            let ua = !va.is_empty() && va.len() as f64 >= 0.95 * *ra as f64;
                            let ub = !vb.is_empty() && vb.len() as f64 >= 0.95 * *rb as f64;
                            if shared == 0 || (shared as f64) < 0.5 * smaller as f64 || !(ua || ub) {
                                continue;
                            }
                            let left = format!("{}.{}.{}", files[fa].prefix, ta.id(), ident(sa));
                            let right = format!("{}.{}.{}", files[fb].prefix, tb.id(), ident(sb));
                            let unique = if ua { format!("{}.{}", files[fa].prefix, ta.id()) } else { format!("{}.{}", files[fb].prefix, tb.id()) };
                            out.push(Key { left, right, shared, unique });
                        }
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.shared.cmp(&a.shared));
    out.truncate(MAX_KEYS);
    out
}

/// The catalog lines of a session with several files; `current` is the prefix of the file the
/// catalog is about.
pub fn session_lines(files: &[FileInfo], current: &str) -> Vec<String> {
    let list: Vec<String> = files
        .iter()
        .map(|f| format!("{} = {} ({}{})", f.prefix, short(&f.name, 80), ids(&f.norm), if f.prefix == current { "; this file" } else { "" }))
        .collect();
    let keys = join_keys(files);
    let example = match keys.first() {
        Some(k) => {
            let (l, r) = (&k.left, &k.right);
            let lt = l.rsplitn(2, '.').nth(1).unwrap_or(l);
            let rt = r.rsplitn(2, '.').nth(1).unwrap_or(r);
            format!("SELECT … FROM {lt} AS a JOIN {rt} AS b ON a.{} = b.{}", l.rsplit('.').next().unwrap_or(""), r.rsplit('.').next().unwrap_or(""))
        }
        None => "SELECT … FROM f1.T1 AS a JOIN f2.T1 AS b ON a.<key> = b.<key>".to_string(),
    };
    let mut out = vec![format!(
        "FILES open in this session — one query can join them (any open file as path; plain T1 is the file in path): {}; e.g. {example}",
        list.join(", ")
    )];
    if !keys.is_empty() {
        let k: Vec<String> = keys
            .iter()
            .map(|k| format!("{} = {} ({} shared values; unique in {})", k.left, k.right, thousands(k.shared), k.unique))
            .collect();
        out.push(format!("JOIN KEYS {}", k.join(" · ")));
    }
    out
}
