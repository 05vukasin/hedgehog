//! hedgehog's agent tools as a plain Rust API.
//!
//! A [`Session`] answers the six tools of the `hedgehog mcp` contract (`docs/mcp.md`):
//! `open_file`, `describe_table`, `query`, `cells`, `totals` and `list_files`. Every method returns
//! the compact text the contract defines (≤ ~12,000 characters, cut with a `… cut:` line) or a
//! one-line [`ToolError`] an LLM can act on. The MCP server (`hedgehog mcp`), the Python bindings
//! and a Node package are thin wrappers over this crate.
//!
//! Files are read only. Each file is normalized once per content hash (sha256) in memory
//! ([`hh_core::normalize_workbook`] with total rows kept) and loaded into its own read-only
//! in-memory SQLite database ([`sql`]). File access is bounded by a root folder plus explicitly
//! allowed paths ([`files::Bounds`]).
//!
//! This crate is separate from `hh_core` so the core stays free of SQLite (a C library) and of
//! session state; `hh_core` keeps the analysis, this crate the agent-facing views of it.

pub mod catalog;
pub mod cells;
pub mod dialect;
pub mod entity;
pub mod facts;
pub mod files;
pub mod sql;
pub mod text;
pub mod tools;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use hh_core::model::Workbook;
use hh_core::normalize::{sha256, ColRole, Dtype, Normalized, Out, Table};

pub use catalog::{mentioned_years, time_coverage, TimeCoverage};
pub use files::Bounds;
pub use tools::{tool_specs, TOOL_NAMES};

use crate::text::{closest, csv_row, ident, quote_ident, sheet_ref, short, MAX_CHARS};

/// A tool failure: one line of text for the model (`isError: true` in MCP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError(pub String);

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ToolError {}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        ToolError(s)
    }
}

impl From<&str> for ToolError {
    fn from(s: &str) -> Self {
        ToolError(s.to_string())
    }
}

pub type ToolResult = Result<String, ToolError>;

/// Session limits.
#[derive(Debug, Clone)]
pub struct Options {
    /// Time limit of one SQL query.
    pub query_timeout: Duration,
    /// Larger files are refused (reading them could exhaust memory).
    pub max_file_bytes: u64,
    /// Normalized files kept in memory; the least recently used is dropped beyond this.
    pub max_open_files: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { query_timeout: sql::DEFAULT_TIMEOUT, max_file_bytes: 256 << 20, max_open_files: 8 }
    }
}

/// Default and maximum number of rows `query` returns.
pub const QUERY_DEFAULT_LIMIT: usize = 50;
pub const QUERY_MAX_LIMIT: usize = 200;
/// Workbooks up to this many cells stay in memory after `open_file` for `cells`.
const KEEP_BOOK_CELLS: usize = 1_000_000;

struct Opened {
    path: PathBuf,
    norm: Normalized,
    db: sql::Db,
    seconds: f64,
    last_used: u64,
}

/// One client's tool session: file bounds, normalized files and their databases.
pub struct Session {
    bounds: Bounds,
    opts: Options,
    files: HashMap<String, Opened>,
    /// Canonical path → (size, mtime, sha256), so unchanged files are not hashed again.
    hashes: HashMap<PathBuf, (u64, Option<SystemTime>, String)>,
    /// The last workbook read for `cells` (path, size, mtime).
    book: Option<(PathBuf, u64, Option<SystemTime>, Workbook)>,
    clock: u64,
}

impl Session {
    /// A session whose file access is bounded by `root` (canonicalized) plus `allow` paths.
    pub fn new(root: impl AsRef<Path>, allow: &[PathBuf]) -> Result<Session, ToolError> {
        Session::with_options(root, allow, Options::default())
    }

    pub fn with_options(root: impl AsRef<Path>, allow: &[PathBuf], opts: Options) -> Result<Session, ToolError> {
        let root = root.as_ref();
        let mut bounds = Bounds::new(root).map_err(|e| ToolError(format!("{}: {e}", root.display())))?;
        for a in allow {
            bounds.allow(a)?;
        }
        Ok(Session { bounds, opts, files: HashMap::new(), hashes: HashMap::new(), book: None, clock: 0 })
    }

    pub fn bounds(&self) -> &Bounds {
        &self.bounds
    }

    /// Allows one more file or folder outside the root (e.g. a file the user referenced).
    pub fn allow(&mut self, path: impl AsRef<Path>) -> Result<(), ToolError> {
        Ok(self.bounds.allow(path)?)
    }

    fn resolve_file(&self, raw: &str) -> Result<PathBuf, ToolError> {
        match self.bounds.resolve(raw, false) {
            Ok(p) => Ok(p),
            Err(e) => {
                // A bare file name of a file opened before.
                let want = Path::new(raw.trim().trim_start_matches('@')).file_name().map(|f| f.to_string_lossy().to_lowercase());
                let hits: Vec<&PathBuf> = self
                    .files
                    .values()
                    .map(|o| &o.path)
                    .filter(|p| p.file_name().map(|f| f.to_string_lossy().to_lowercase()) == want)
                    .collect();
                match hits.as_slice() {
                    [one] => Ok((*one).clone()),
                    _ => Err(ToolError(e)),
                }
            }
        }
    }

    /// Normalizes the file (once per content) and returns its sha256 key and whether it was cached.
    fn load(&mut self, raw: &str) -> Result<(String, bool), ToolError> {
        let path = self.resolve_file(raw)?;
        let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if !files::EXTENSIONS.contains(&ext.as_str()) {
            return Err(ToolError(format!(
                "{} is not a spreadsheet (supported: {})",
                self.bounds.display(&path),
                files::EXTENSIONS.join(", ")
            )));
        }
        let meta = std::fs::metadata(&path).map_err(|e| ToolError(format!("{}: {e}", self.bounds.display(&path))))?;
        if meta.len() > self.opts.max_file_bytes {
            return Err(ToolError(format!(
                "{} is {} — larger than the {} limit of this session",
                self.bounds.display(&path),
                files::human_size(meta.len()),
                files::human_size(self.opts.max_file_bytes)
            )));
        }
        let mtime = meta.modified().ok();
        self.clock += 1;
        let sha = match self.hashes.get(&path) {
            Some((len, m, sha)) if *len == meta.len() && *m == mtime => sha.clone(),
            _ => {
                let bytes = std::fs::read(&path).map_err(|e| ToolError(format!("{}: {e}", self.bounds.display(&path))))?;
                let sha = sha256::hex(&bytes);
                self.hashes.insert(path.clone(), (meta.len(), mtime, sha.clone()));
                sha
            }
        };
        if let Some(o) = self.files.get_mut(&sha) {
            o.last_used = self.clock;
            o.path = path;
            return Ok((sha, true));
        }
        let t0 = Instant::now();
        let wb = hh_core::open(&path).map_err(|e| ToolError(format!("{}: cannot read: {e}", self.bounds.display(&path))))?;
        let opts = hh_core::NormalizeOptions { keep_totals: true, ..Default::default() };
        let norm = hh_core::normalize_workbook(&wb, &opts);
        // Keep a small workbook for `cells`; large ones are read again when needed.
        let cells: usize = wb.sheets.iter().map(|s| s.cells.len()).sum();
        self.book = (cells <= KEEP_BOOK_CELLS).then(|| (path.clone(), meta.len(), mtime, wb));
        let db = sql::Db::load(&norm).map_err(|e| ToolError(format!("{}: loading into SQL failed: {e}", self.bounds.display(&path))))?;
        let seconds = t0.elapsed().as_secs_f64();
        while self.files.len() >= self.opts.max_open_files.max(1) {
            let Some(old) = self.files.iter().min_by_key(|(_, o)| o.last_used).map(|(k, _)| k.clone()) else { break };
            self.files.remove(&old);
        }
        self.files.insert(sha.clone(), Opened { path, norm, db, seconds, last_used: self.clock });
        Ok((sha, false))
    }

    fn opened(&mut self, raw: &str) -> Result<&Opened, ToolError> {
        let (sha, _) = self.load(raw)?;
        Ok(&self.files[&sha])
    }

    // -- tools ------------------------------------------------------------------------------------

    /// `open_file`: normalizes the file and returns its catalog.
    pub fn open_file(&mut self, path: &str) -> ToolResult {
        let (sha, cached) = self.load(path)?;
        let o = &self.files[&sha];
        let name = self.bounds.display(&o.path);
        let mut text = catalog::catalog(&o.norm, &name, MAX_CHARS - 300);
        for (t, n) in &o.db.skipped_columns {
            text.push_str(&format!("\nNOTE {t}: {n} columns beyond SQLite's 2000-column limit are not queryable"));
        }
        if !text.contains("\n… cut:") {
            if cached {
                text.push_str("\n(cached)");
            } else {
                text.push_str(&format!("\n(normalized in {:.1} s)", o.seconds));
            }
        }
        Ok(text)
    }

    /// `describe_table`: all columns with stats, the first rows, header, group column, total rows.
    pub fn describe_table(&mut self, path: &str, table: &str) -> ToolResult {
        let o = self.opened(path)?;
        let t = catalog::find_table(&o.norm, table)?;
        let text = catalog::describe_table(&o.norm, t);
        let ents = o.db.entities.iter().find(|(n, _)| *n == t.n).map(|x| x.1.as_slice()).unwrap_or_default();
        Ok(with_entities(text, t, ents))
    }

    /// `query`: one read-only SELECT; CSV with header, at most `limit` rows (default 50, max 200).
    pub fn query(&mut self, path: &str, sql: &str, limit: Option<usize>) -> ToolResult {
        let timeout = self.opts.query_timeout;
        let limit = limit.unwrap_or(QUERY_DEFAULT_LIMIT).clamp(1, QUERY_MAX_LIMIT);
        let o = self.opened(path)?;
        let used = tables_in_sql(&o.norm, sql);
        let res = match o.db.query(sql, limit, timeout) {
            Ok(r) => r,
            Err(e) => return Err(ToolError(sql_error_hint(&o.norm, &o.db, &used, sql, &e, timeout))),
        };
        let mut lines = vec![csv_row(&res.header)];
        let mut shown = 0;
        let mut size = lines[0].len();
        let mut cut_for_size = false;
        for r in &res.rows {
            let line = csv_row(r);
            if size + line.len() + 1 > MAX_CHARS - 1200 {
                cut_for_size = true;
                break;
            }
            size += line.len() + 1;
            lines.push(line);
            shown += 1;
        }
        if res.total == 0 {
            for t in &used {
                let info = catalog::row_info(&o.norm, t);
                if !info.total_rows.is_empty() && !sql_mentions(sql, &format!("{}_all", t.id())) {
                    lines.push(format!(
                        "note: {} has no total rows; {} total row(s) ({}) are in {}_all",
                        t.id(),
                        info.total_rows.len(),
                        info.total_rows.iter().take(3).map(|r| format!("'{}'@{}", short(&r.label, 30), r.row)).collect::<Vec<_>>().join(", "),
                        t.id()
                    ));
                }
            }
        }
        if let Some(hint) = cite_hint(&o.db, &used, &res.header) {
            lines.push(hint);
        }
        if let Some(note) = spellings_note(&o.db, &used, &res) {
            lines.push(note);
        }
        let n = text::thousands(res.total);
        lines.push(match (res.complete, shown < res.total) {
            (true, false) => format!("({n} row{})", if res.total == 1 { "" } else { "s" }),
            (true, true) => format!("({n} rows, showing first {shown})"),
            (false, _) => format!("(more than {n} rows, showing first {shown}; counting stopped)"),
        });
        if cut_for_size {
            lines.push(format!(
                "… cut: {} of the requested rows did not fit; ask for fewer columns or a smaller limit",
                res.rows.len() - shown
            ));
        }
        Ok(lines.join("\n"))
    }

    /// `cells`: original source cells of a range (max 200 cells).
    pub fn cells(&mut self, path: &str, sheet: &str, range: &str) -> ToolResult {
        let p = self.resolve_file(path)?;
        let meta = std::fs::metadata(&p).map_err(|e| ToolError(e.to_string()))?;
        if meta.len() > self.opts.max_file_bytes {
            return Err(ToolError(format!("{} is larger than the limit of this session", self.bounds.display(&p))));
        }
        let mtime = meta.modified().ok();
        let fresh = matches!(&self.book, Some((bp, len, m, _)) if *bp == p && *len == meta.len() && *m == mtime);
        if !fresh {
            self.book = None;
            let wb = hh_core::open(&p).map_err(|e| ToolError(format!("{}: cannot read: {e}", self.bounds.display(&p))))?;
            self.book = Some((p, meta.len(), mtime, wb));
        }
        let wb = &self.book.as_ref().expect("workbook loaded").3;
        Ok(cells::cells(wb, sheet, range)?)
    }

    /// `totals`: the totals check, optionally for one table and/or one status.
    pub fn totals(&mut self, path: &str, table: Option<&str>, status: Option<&str>) -> ToolResult {
        let o = self.opened(path)?;
        let t = table.map(|id| catalog::find_table(&o.norm, id)).transpose()?;
        Ok(catalog::totals(&o.norm, t, status)?)
    }

    /// `list_files`: spreadsheet files under a folder (relative to the root).
    pub fn list_files(&self, folder: Option<&str>, pattern: Option<&str>) -> ToolResult {
        Ok(self.bounds.list_files(folder, pattern)?)
    }

    /// The normalized form of an opened file (for embedding the session in other tools).
    pub fn normalized(&mut self, path: &str) -> Result<&Normalized, ToolError> {
        Ok(&self.opened(path)?.norm)
    }
}

fn sql_mentions(sql: &str, name: &str) -> bool {
    sql.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w.eq_ignore_ascii_case(name))
}

/// Tables named in the SQL (`T2`, `t2_all`, `"T2"`).
fn tables_in_sql<'a>(norm: &'a Normalized, sql: &str) -> Vec<&'a Table> {
    let mut out: Vec<&Table> = Vec::new();
    for w in sql.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        let w = w.strip_suffix("_all").or_else(|| w.strip_suffix("_ALL")).unwrap_or(w);
        let Some(n) = w.strip_prefix(['T', 't']).and_then(|d| d.parse::<usize>().ok()) else { continue };
        if let Some(t) = norm.tables.iter().find(|t| t.n == n)
            && !out.iter().any(|x| x.n == n)
        {
            out.push(t);
        }
    }
    out
}

/// Rows an error returns when it names the rows the query was after (B15).
const ERROR_ROWS: usize = 10;
/// Characters of those rows at most.
const ERROR_ROWS_CHARS: usize = 3000;

fn sql_error_hint(norm: &Normalized, db: &sql::Db, used: &[&Table], sql: &str, err: &str, timeout: Duration) -> String {
    let mut err = err.replace(" - should this be a string literal in single-quotes?", "");
    // SQLite echoes the whole statement ("… in SELECT … at offset 7"); keep the position only.
    if let Some(pos) = err.rfind(" at offset ") {
        let before = &err[..pos];
        if let Some(i) = before.find(" in SELECT").or_else(|| before.find(" in WITH")).or_else(|| before.find(" in select")) {
            err = format!("{} (at character {})", &before[..i], &err[pos + " at offset ".len()..]);
        }
    }
    let err = err.as_str();
    let mut msg = format!("SQL error: {}", short(err, 300));
    let low = err.to_lowercase();
    if let Some(rest) = low.find("no such column:").map(|i| &err[i + "no such column:".len()..]) {
        let wanted = missing_column(rest);
        let wanted = wanted.as_str();
        let scope: Vec<&Table> = if used.is_empty() { norm.tables.iter().collect() } else { used.to_vec() };
        let mut names: Vec<(String, String)> = Vec::new();
        for t in &scope {
            for (c, name) in t.columns.iter().zip(catalog::sql_names(t)) {
                if c.role != ColRole::RowType {
                    names.push((t.id(), name));
                }
            }
            for e in db.entities.iter().filter(|(n, _)| *n == t.n).flat_map(|x| &x.1) {
                names.push((t.id(), e.id_col.clone()));
                names.push((t.id(), e.name_col.clone()));
            }
        }
        let close = closest(wanted, names.iter().map(|(_, n)| n.as_str()), 4);
        let hits: Vec<String> = names
            .iter()
            .filter(|(_, n)| close.contains(&n.as_str()))
            .take(6)
            .map(|(t, n)| format!("{t}.{}", quote_ident(n)))
            .collect();
        if let Some((h, rows_sql)) = row_label_hint(norm, &scope, wanted) {
            msg.push_str(&format!(" — {h}"));
            if !hits.is_empty() {
                msg.push_str(&format!(" (columns with similar names: {})", hits.join(", ")));
            }
            if let Some(rows) = rows_sql.and_then(|q| rows_csv(db, &q, timeout)) {
                msg.push_str(&format!("\n{rows}"));
            }
        } else if !hits.is_empty() {
            msg.push_str(&format!(" — did you mean {}? (quote names with double quotes)", hits.join(", ")));
        } else if let Some(d) = dialect::hint(err, sql) {
            msg.push_str(&format!(" — {d}"));
        } else if scope.len() <= 3 {
            let lists: Vec<String> = scope
                .iter()
                .map(|t| {
                    let ns: Vec<String> = catalog::sql_names(t)
                        .iter()
                        .zip(&t.columns)
                        .filter(|(_, c)| c.role != ColRole::RowType)
                        .map(|(n, _)| ident(n))
                        .collect();
                    format!("{}: {}", t.id(), ns.join(", "))
                })
                .collect();
            msg.push_str(&format!(" — columns are {}", short(&lists.join("; "), 1500)));
        } else {
            msg.push_str(" — use describe_table to see the exact column names");
        }
    } else if let Some(word) = err.strip_prefix("near \"").and_then(|r| r.split_once("\": syntax error")).map(|x| x.0) {
        let is_col = norm.tables.iter().any(|t| catalog::sql_names(t).iter().any(|n| n.eq_ignore_ascii_case(word)));
        if is_col {
            msg.push_str(&format!(" — {word} is a column name that is also an SQL keyword; quote it: {}", quote_ident(word)));
        } else if let Some(d) = dialect::hint(err, sql) {
            msg.push_str(&format!(" — {d}"));
        }
    } else if low.contains("no such table") {
        let ids: Vec<String> = norm.tables.iter().map(|t| t.id()).collect();
        if ids.is_empty() {
            msg.push_str(" — this file has no tables");
        } else {
            msg.push_str(&format!(
                " — tables are {} (data rows) and the same with _all (with total rows)",
                short(&ids.join(", "), 400)
            ));
        }
    } else if let Some(d) = dialect::hint(err, sql) {
        msg.push_str(&format!(" — {d}"));
    }
    msg
}

/// Words that name a total row ("Total", "Gesamt", …), compared without case.
const TOTAL_WORDS: &[&str] = &[
    "total", "totals", "grand total", "subtotal", "sub-total", "sub total", "gesamt", "insgesamt", "summe", "gesamtsumme",
    "zusammen", "totale", "totale generale", "ukupno", "svega", "укупно", "свега", "total general", "totaal", "totalt",
    "i alt", "yhteensä", "razem", "celkem", "összesen", "suma", "somme", "ensemble", "overall",
];

fn sql_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The text column holding `label` in row `i` of `t`.
fn label_column(t: &Table, names: &[String], i: usize, label: &str) -> Option<String> {
    t.columns.iter().zip(names).find_map(|(c, n)| {
        let hit = matches!(c.role, ColRole::Data | ColRole::Group)
            && matches!(c.values.get(i), Some(Out::Text(s)) if s.trim().eq_ignore_ascii_case(label.trim()));
        hit.then(|| n.clone())
    })
}

/// The rows of `sql` as CSV for an error text: at most [`ERROR_ROWS`] rows and
/// [`ERROR_ROWS_CHARS`] characters, under a short time limit; None when it fails or is empty.
fn rows_csv(db: &sql::Db, sql: &str, timeout: Duration) -> Option<String> {
    let r = db.query(sql, ERROR_ROWS, timeout.min(Duration::from_millis(500))).ok()?;
    if r.rows.is_empty() {
        return None;
    }
    let mut lines = vec![csv_row(&r.header)];
    let mut size = lines[0].len();
    for row in &r.rows {
        let line = csv_row(row);
        if size + line.len() + 1 > ERROR_ROWS_CHARS {
            break;
        }
        size += line.len() + 1;
        lines.push(line);
    }
    let shown = lines.len() - 1;
    let more = if r.total > shown { format!(", first {shown} of {}", text::thousands(r.total)) } else { String::new() };
    Some(format!("Those rows (the query above, run for you{more}; pick the columns you need):\n{}", lines.join("\n")))
}

/// When the "missing column" is the label of a total row (or a total word such as "Total") or a
/// value of a text column: where those rows are and how to select them, and the query that
/// returns them (run for the error text, B15).
fn row_label_hint(norm: &Normalized, scope: &[&Table], wanted: &str) -> Option<(String, Option<String>)> {
    let w = wanted.trim();
    if w.chars().count() < 2 {
        return None;
    }
    let lw = w.to_lowercase();
    let generic = TOTAL_WORDS.contains(&lw.trim_end_matches([':', '.']).trim());
    let infos: Vec<(&Table, catalog::RowInfo)> = scope.iter().map(|t| (*t, catalog::row_info(norm, t))).collect();
    let find = |exact: bool| {
        infos.iter().find_map(|(t, info)| {
            let hit = |r: &&catalog::TotalRow| !exact || r.label.trim().eq_ignore_ascii_case(w);
            if let Some(r) = info.total_rows.iter().find(hit) {
                return Some((*t, r.clone(), true));
            }
            info.kept_totals.iter().find(hit).map(|r| (*t, r.clone(), false))
        })
    };
    let quoted = quote_ident(w);
    if let Some((t, r, in_all)) = find(true).or_else(|| if generic { find(false) } else { None }) {
        let names = catalog::sql_names(t);
        let col = t.src_rows.iter().position(|s| s + 1 == r.row).and_then(|i| label_column(t, &names, i, &r.label));
        let id = t.id();
        let at = col.as_ref().map(|c| format!(" (label in {})", ident(c))).unwrap_or_default();
        return Some(if in_all {
            let kind = r.kind.as_str();
            let q = format!("SELECT * FROM {id}_all WHERE _row_type = '{kind}'");
            (format!("{quoted} is a row label, not a column: {kind} rows are in {id}_all with _row_type = '{kind}'{at}: {q}"), Some(q))
        } else {
            let filter = match &col {
                Some(c) => format!("WHERE {} = {}", quote_ident(c), sql_text(&r.label)),
                None => format!("WHERE _src_row = {}", r.row),
            };
            let q = format!("SELECT * FROM {id} {filter}");
            (format!("{quoted} is a row label, not a column: that total row stays in {id}{at}: {q}"), Some(q))
        });
    }
    if generic {
        let id = scope.first()?.id();
        let h = format!("{quoted} is not a column and {id} has no total rows: compute it, e.g. SELECT SUM(<column>) FROM {id}");
        return Some((h, None));
    }
    // A value of a text column (a pipeline, a region) used as a column name.
    for t in scope {
        let names = catalog::sql_names(t);
        for (c, n) in t.columns.iter().zip(&names) {
            if !matches!(c.role, ColRole::Data | ColRole::Group) || c.dtype != Dtype::Text {
                continue;
            }
            if let Some(Out::Text(v)) = c.values.iter().find(|v| matches!(v, Out::Text(s) if s.trim().eq_ignore_ascii_case(w))) {
                let q = format!("SELECT * FROM {} WHERE {} = {}", t.id(), quote_ident(n), sql_text(v));
                return Some((format!("{quoted} is a value of column {}, not a column: {q}", ident(n)), Some(q)));
            }
        }
    }
    None
}

/// `describe_table` text with the table's entity columns (their `<col>_id` and `<col>_name` in the
/// SQL views) added before the first rows.
fn with_entities(text: String, t: &Table, ents: &[entity::Entity]) -> String {
    if ents.is_empty() {
        return text;
    }
    let cols: Vec<String> = ents.iter().map(|e| format!("{}, {}", ident(&e.id_col), ident(&e.name_col))).collect();
    let mut add = format!("entity columns (the SQL views {} and {}_all also have {}):", t.id(), t.id(), cols.join(", "));
    for e in ents {
        add.push_str(&format!("\n  {}", entity::line(e, &t.id()).trim_start()));
    }
    let out = match text.find("\nfirst ") {
        Some(i) => format!("{}\n{add}{}", &text[..i], &text[i..]),
        None => format!("{text}\n{add}"),
    };
    text::cut(out, MAX_CHARS, |n| format!("{n} more lines"), "query on the table for specific columns")
}

/// The column name in SQLite's `no such column: <name> - …` (`"quoted name"`, `T1.name`, `name`).
fn missing_column(rest: &str) -> String {
    let s = rest.trim_start();
    // Optional table prefix: T1. or "T1".
    let mut name = String::new();
    let mut chars = s.chars().peekable();
    loop {
        name.clear();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        name.push('"');
                    } else {
                        break;
                    }
                } else {
                    name.push(c);
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '.' {
                    break;
                }
                name.push(c);
                chars.next();
            }
        }
        if chars.peek() == Some(&'.') {
            chars.next();
            continue;
        }
        return name;
    }
}

/// A result keyed by an entity column's text that lists one entity (one ID) on several rows under
/// different spellings (B18): which rows, and the query grouped by the ID instead.
fn spellings_note(db: &sql::Db, used: &[&Table], res: &sql::QueryResult) -> Option<String> {
    for t in used {
        let ents = db.entities.iter().find(|(n, _)| *n == t.n).map(|x| x.1.as_slice()).unwrap_or_default();
        for e in ents {
            let has = |n: &str| res.header.iter().position(|h| h.eq_ignore_ascii_case(n));
            let Some(j) = has(&e.name) else { continue };
            if has(&e.id_col).is_some() {
                continue;
            }
            let mut first: HashMap<&str, (usize, &str)> = HashMap::new();
            for (r, row) in res.rows.iter().enumerate() {
                let Some(cell) = row.get(j) else { continue };
                let entity::Parsed::One { id, .. } = entity::parse(cell) else { continue };
                match first.get(id) {
                    Some(&(r0, text)) if text != cell.as_str() => {
                        return Some(format!(
                            "note: rows {} and {} are one {} (ID {id}) under different spellings — group by {} instead, e.g. {}",
                            r0 + 1,
                            r + 1,
                            ident(&e.name),
                            ident(&e.id_col),
                            entity::group_sql(e, &t.id())
                        ));
                    }
                    Some(_) => {}
                    None => {
                        first.insert(id, (r, cell));
                    }
                }
            }
        }
    }
    None
}

/// With `_src_row` in the result and one table queried: which source cell each column comes from.
fn cite_hint(db: &sql::Db, used: &[&Table], header: &[String]) -> Option<String> {
    if !header.iter().any(|h| h == "_src_row") || used.len() != 1 {
        return None;
    }
    let t = used[0];
    let names = catalog::sql_names(t);
    let ents = db.entities.iter().find(|(n, _)| *n == t.n).map(|x| x.1.as_slice()).unwrap_or_default();
    let parts: Vec<String> = header
        .iter()
        .filter_map(|h| {
            // An entity column's ID or name part is cited as the cell it comes from.
            let (i, part) = match names.iter().position(|n| n == h) {
                Some(i) => (i, ""),
                None => {
                    let e = ents.iter().find(|e| e.id_col == *h || e.name_col == *h)?;
                    (e.col, if e.id_col == *h { " (ID part)" } else { " (name part)" })
                }
            };
            let col = t.columns[i].source_col?;
            Some(format!("{} = {}{part}", ident(h), sheet_ref(&t.source_sheet, &format!("{}<_src_row>", text::letter(col)))))
        })
        .take(8)
        .collect();
    (!parts.is_empty()).then(|| format!("cite as: {}", parts.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::missing_column;

    #[test]
    fn missing_column_names() {
        assert_eq!(missing_column(" \"Dezember / 2021\" - should this be …"), "Dezember / 2021");
        assert_eq!(missing_column(" T1.Revenue in SELECT"), "Revenue");
        assert_eq!(missing_column(" \"T1\".\"a \"\"b\"\"\""), "a \"b\"");
        assert_eq!(missing_column(" x"), "x");
    }
}
