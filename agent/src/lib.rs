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
//! SQLite database ([`sql`]). The result is kept in a disk cache ([`cache`]), so a later session
//! reopens the file in milliseconds. File access is bounded by a root folder plus explicitly
//! allowed paths ([`files::Bounds`]).
//!
//! This crate is separate from `hh_core` so the core stays free of SQLite (a C library) and of
//! session state; `hh_core` keeps the analysis, this crate the agent-facing views of it.

pub mod cache;
pub mod catalog;
pub mod cells;
pub mod dialect;
pub mod entity;
pub mod facts;
pub mod files;
pub mod notes;
pub mod sql;
pub mod sqlscan;
pub mod text;
pub mod tools;
pub mod types;
pub mod views;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
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
    /// Cells per workbook; larger files are refused (memory grows with cells).
    pub max_cells: u64,
    /// Time limit for reading and normalizing one file.
    pub max_seconds: Duration,
    /// Disk cache of normalized files ([`cache`]); default from the environment
    /// (`HEDGEHOG_CACHE`, `HEDGEHOG_CACHE_DIR`, `HEDGEHOG_CACHE_MAX_MB`), None = off.
    pub cache: Option<cache::Cache>,
}

/// Default of [`Options::max_seconds`].
pub const DEFAULT_MAX_SECONDS: u64 = 120;

impl Default for Options {
    fn default() -> Self {
        Options {
            query_timeout: sql::DEFAULT_TIMEOUT,
            max_file_bytes: 256 << 20,
            max_open_files: 8,
            max_cells: hh_core::reader::DEFAULT_MAX_CELLS,
            max_seconds: Duration::from_secs(DEFAULT_MAX_SECONDS),
            cache: cache::Cache::from_env(),
        }
    }
}

impl Options {
    /// Reader options for one file with these limits, its clock starting now.
    fn read_options(&self) -> hh_core::reader::ReadOptions {
        let mut o = hh_core::reader::ReadOptions::default();
        o.limits.max_cells = self.max_cells;
        o.limits.deadline = Instant::now().checked_add(self.max_seconds);
        o
    }
}

/// Default and maximum number of rows `query` returns.
pub const QUERY_DEFAULT_LIMIT: usize = 50;
pub const QUERY_MAX_LIMIT: usize = 200;
/// Workbooks up to this many cells stay in memory after `open_file` for `cells`.
const KEEP_BOOK_CELLS: usize = 1_000_000;

/// Normalize options of a session; part of the disk cache key.
const NORMALIZE_TAG: &str = "keep_totals";

fn normalize_options() -> hh_core::NormalizeOptions {
    hh_core::NormalizeOptions { keep_totals: true, ..Default::default() }
}

/// A value computed on another thread: waited for when first needed.
enum Pending<V> {
    Ready(V),
    Running(mpsc::Receiver<Result<V, String>>),
    Failed(String),
}

impl<V: Send + 'static> Pending<V> {
    fn spawn(f: impl FnOnce() -> Result<V, String> + Send + 'static) -> Pending<V> {
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new().name("hedgehog-load".into()).spawn(move || {
            let _ = tx.send(f());
        }) {
            Ok(_) => Pending::Running(rx),
            Err(e) => Pending::Failed(format!("cannot start a thread: {e}")),
        }
    }

    /// Waits until the value is there; gives up with "cancelled" once `cancel` is set (the work
    /// goes on and is picked up by the next wait).
    fn wait(&mut self, cancel: &AtomicBool) -> Result<&mut V, ToolError> {
        if let Pending::Running(rx) = self {
            let got = loop {
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(r) => break r,
                    Err(mpsc::RecvTimeoutError::Timeout) if cancel.load(Ordering::Relaxed) => return Err(cancelled()),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break Err("internal error while loading the file".into()),
                }
            };
            *self = match got {
                Ok(v) => Pending::Ready(v),
                Err(e) => Pending::Failed(e),
            };
        }
        match self {
            Pending::Ready(v) => Ok(v),
            Pending::Failed(e) => Err(ToolError(e.clone())),
            Pending::Running(_) => Err(cancelled()),
        }
    }
}

fn cancelled() -> ToolError {
    ToolError("cancelled".into())
}

/// The normalized form of a file and, when it came from the disk cache, what its database still
/// needs ([`sql::plan`]).
struct Norm {
    norm: Arc<Normalized>,
    plan: Option<sql::Plan>,
}

struct Opened {
    path: PathBuf,
    /// Ready right after normalizing; decoded on another thread when it came from the cache.
    norm: Pending<Norm>,
    /// Built on another thread after the catalog is out (into the disk cache when it is on);
    /// opened in place when it came from the cache.
    db: Pending<(sql::Db, Option<cache::Entry>)>,
    /// Both of the above, once they are there.
    loaded: Option<Loaded>,
    /// The file's disk cache entry (once written).
    entry: Option<cache::Entry>,
    /// Catalog text per display name (before the timing line).
    catalogs: HashMap<String, String>,
    from_disk: bool,
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
    /// Set (from any thread) to stop the running tool call: a query is interrupted, waiting for
    /// a normalization ends with "cancelled" (the normalization finishes in the background and
    /// goes to the disk cache).
    cancel: Arc<AtomicBool>,
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
        Ok(Session {
            bounds,
            opts,
            files: HashMap::new(),
            hashes: HashMap::new(),
            book: None,
            clock: 0,
            cancel: Arc::default(),
        })
    }

    /// The flag that cancels the running tool call when set (from another thread). The caller
    /// clears it before the next call.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
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
        let t0 = Instant::now();
        let cache = self.opts.cache.clone();
        let sha = match self.hashes.get(&path) {
            Some((len, m, sha)) if *len == meta.len() && *m == mtime => sha.clone(),
            _ => {
                let known = cache.as_ref().and_then(|c| c.known_hash(&path, meta.len(), mtime));
                let sha = match known {
                    Some(sha) => sha,
                    None => {
                        let bytes = std::fs::read(&path).map_err(|e| ToolError(format!("{}: {e}", self.bounds.display(&path))))?;
                        let sha = sha256::hex(&bytes);
                        if let Some(c) = &cache {
                            c.remember_hash(&path, meta.len(), mtime, &sha);
                        }
                        sha
                    }
                };
                self.hashes.insert(path.clone(), (meta.len(), mtime, sha.clone()));
                sha
            }
        };
        cache::trace("hash", t0);
        if let Some(o) = self.files.get_mut(&sha) {
            o.last_used = self.clock;
            o.path = path;
            return Ok((sha, true));
        }
        let key = cache::Cache::key(&sha, NORMALIZE_TAG);
        // A file seen before: its catalog, database and (decoded on another thread) normalized form
        // come from the disk cache.
        if let Some(c) = &cache
            && let Some(entry) = c.get(&key)
        {
            match sql::Db::open_file(&entry.db_path()) {
                Ok(db) => {
                    let norm_path = entry.norm_path();
                    let norm = Pending::spawn(move || {
                        let t = Instant::now();
                        let norm = cache::read_norm(&norm_path).map_err(|e| format!("disk cache: {e}"))?;
                        cache::trace("cache: norm read", t);
                        let plan = sql::plan(&norm);
                        cache::trace("cache: norm decoded", t);
                        Ok(Norm { norm: Arc::new(norm), plan: Some(plan) })
                    });
                    let seconds = entry.seconds();
                    cache::trace("cache: entry opened", t0);
                    self.insert(sha.clone(), Opened {
                        path,
                        norm,
                        db: Pending::Ready((db, None)),
                        loaded: None,
                        entry: Some(entry),
                        catalogs: HashMap::new(),
                        from_disk: true,
                        seconds,
                        last_used: self.clock,
                    });
                    return Ok((sha, true));
                }
                Err(_) => {
                    let _ = std::fs::remove_dir_all(&entry.dir);
                }
            }
        }
        let display = self.bounds.display(&path);
        // Normalized on another thread, so a cancel ends the wait; a cancelled normalization still
        // goes to the disk cache when it ends.
        let mut work = {
            let (path, cache, key) = (path.clone(), cache.clone(), key.clone());
            // Limits (cells, decompressed size, time) for an untrusted file; the clock starts now.
            let read = self.opts.read_options();
            let max_seconds = self.opts.max_seconds.as_secs();
            Pending::spawn(move || {
                let wb = hh_core::reader::open_with(&path, read).map_err(|e| format!("{display}: cannot read: {}", limit_hint(&e)))?;
                let opts = hh_core::NormalizeOptions { deadline: read.limits.deadline, ..normalize_options() };
                let norm = hh_core::try_normalize_workbook(&wb, &opts)
                    .map_err(|e| format!("{display}: {e} ({max_seconds} s; the server's --max-seconds)"))?;
                Ok((wb, norm, t0.elapsed().as_secs_f64(), cache, key))
            })
        };
        let waited = work.wait(&self.cancel).map(|_| ());
        let (wb, norm, seconds, _, _) = match waited {
            Ok(()) => match work {
                Pending::Ready(v) => v,
                _ => return Err(cancelled()),
            },
            Err(e) => {
                if let Pending::Running(rx) = work {
                    // Nobody waits any more: cache the result when it comes.
                    std::thread::spawn(move || {
                        if let Ok(Ok((_, norm, seconds, Some(c), key))) = rx.recv() {
                            let _ = c.put(&key, &norm, seconds, "", None);
                        }
                    });
                }
                return Err(e);
            }
        };
        cache::trace("normalized", t0);
        // Keep a small workbook for `cells`; large ones are read again when needed.
        let cells: usize = wb.sheets.iter().map(|s| s.cells.len()).sum();
        self.book = (cells <= KEEP_BOOK_CELLS).then(|| (path.clone(), meta.len(), mtime, wb));
        let norm = Arc::new(norm);
        // The catalog first; the SQL database is built on another thread meanwhile.
        let name = self.bounds.display(&path);
        let text = catalog_text(&norm, &name, &sql::skipped_columns(&norm));
        cache::trace("catalog", t0);
        let db = {
            let (norm, text, name, shown) = (norm.clone(), text.clone(), name.clone(), self.bounds.display(&path));
            Pending::spawn(move || {
                let t = Instant::now();
                if let Some(c) = cache {
                    match c.put(&key, &norm, seconds, &name, Some(&text)) {
                        Ok((entry, (skipped, entities))) => match sql::Db::open_file(&entry.db_path()) {
                            Ok(mut db) => {
                                db.skipped_columns = skipped;
                                db.entities = entities;
                                cache::trace("sql (cache file)", t);
                                return Ok((db, Some(entry)));
                            }
                            Err(e) => eprintln!("[hedgehog] disk cache: {e}"),
                        },
                        Err(e) => eprintln!("[hedgehog] disk cache: {e}"),
                    }
                }
                let db = sql::Db::load(&norm).map_err(|e| format!("{shown}: loading into SQL failed: {e}"))?;
                cache::trace("sql (memory)", t);
                Ok((db, None))
            })
        };
        let mut catalogs = HashMap::new();
        catalogs.insert(name, text);
        self.insert(sha.clone(), Opened {
            path,
            norm: Pending::Ready(Norm { norm, plan: None }),
            db,
            loaded: None,
            entry: None,
            catalogs,
            from_disk: false,
            seconds,
            last_used: self.clock,
        });
        Ok((sha, false))
    }

    fn insert(&mut self, sha: String, o: Opened) {
        while self.files.len() >= self.opts.max_open_files.max(1) {
            let Some(old) = self.files.iter().min_by_key(|(_, o)| o.last_used).map(|(k, _)| k.clone()) else { break };
            self.files.remove(&old);
        }
        self.files.insert(sha, o);
    }

    /// Waits until the normalized form and the database of an opened file are there.
    fn ready(&mut self, sha: &str) -> Result<(), ToolError> {
        let cancel = self.cancel.clone();
        let Some(o) = self.files.get_mut(sha) else { return Err(ToolError("internal error: file not open".into())) };
        if o.loaded.is_some() {
            return Ok(());
        }
        o.norm.wait(&cancel)?;
        o.db.wait(&cancel)?;
        let gone = "internal error: file loaded twice";
        let (Pending::Ready(n), Pending::Ready((mut db, entry))) = (std::mem::replace(&mut o.norm, Pending::Failed(gone.into())), std::mem::replace(&mut o.db, Pending::Failed(gone.into())))
        else {
            return Err(ToolError("internal error: file not loaded".into()));
        };
        db.set_cancel(cancel);
        if let Some((skipped, entities)) = n.plan {
            db.skipped_columns = skipped;
            db.entities = entities;
        }
        if entry.is_some() {
            o.entry = entry;
        }
        o.loaded = Some(Loaded { norm: n.norm, db });
        Ok(())
    }

    /// Loads the file and waits for its database only.
    fn database(&mut self, raw: &str) -> Result<&sql::Db, ToolError> {
        let (sha, _) = self.load(raw)?;
        let cancel = self.cancel.clone();
        let o = self.files.get_mut(&sha).ok_or_else(|| ToolError("internal error: file not open".into()))?;
        if let Some(l) = &o.loaded {
            return Ok(&l.db);
        }
        let (db, _) = o.db.wait(&cancel)?;
        db.set_cancel(cancel);
        Ok(db)
    }

    /// Loads the file and waits for its normalized form and database. A damaged disk cache entry
    /// is dropped and the file normalized again.
    fn opened(&mut self, raw: &str) -> Result<&Loaded, ToolError> {
        let (mut sha, _) = self.load(raw)?;
        if let Err(e) = self.ready(&sha) {
            let from_disk = self.files.get(&sha).is_some_and(|o| o.from_disk);
            if !from_disk || e == cancelled() {
                return Err(e);
            }
            if let Some(dir) = self.files.remove(&sha).and_then(|o| o.entry).map(|e| e.dir) {
                let _ = std::fs::remove_dir_all(dir);
            }
            sha = self.load(raw)?.0;
            self.ready(&sha)?;
        }
        self.files[&sha].loaded.as_ref().ok_or_else(|| ToolError("internal error: file not loaded".into()))
    }

    // -- tools ------------------------------------------------------------------------------------

    /// `open_file`: normalizes the file and returns its catalog.
    pub fn open_file(&mut self, path: &str) -> ToolResult {
        let (sha, cached) = self.load(path)?;
        let o = &self.files[&sha];
        let name = self.bounds.display(&o.path);
        let stored = o.catalogs.get(&name).cloned().or_else(|| o.entry.as_ref().and_then(|e| e.catalog(&name)));
        let mut text = match stored {
            Some(t) => t,
            None => {
                // A name this file has no catalog for yet (opened under another path).
                let o = self.opened(path)?;
                let t = catalog_text(&o.norm, &name, &o.db.skipped_columns);
                let o = self.files.get_mut(&sha).expect("file is open");
                if let (Some(c), Some(e)) = (&self.opts.cache, o.entry.as_mut()) {
                    c.add_catalog(e, &name, &t);
                }
                t
            }
        };
        let o = self.files.get_mut(&sha).expect("file is open");
        o.catalogs.entry(name).or_insert_with(|| text.clone());
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
        // A file from the disk cache answers before its normalized form is decoded: the notes
        // below need that form only after an error, for an empty result, with `_src_row` or with
        // "<id> - <name>" values (spellings note).
        let first = self.database(path)?.query(sql, limit, timeout);
        let need_norm = first.as_ref().map_or(true, |r| {
            r.total == 0
                || r.header.iter().any(|h| h == "_src_row")
                || r.rows.iter().flatten().any(|c| matches!(entity::parse(c), entity::Parsed::One { .. }))
        });
        let empty = Normalized::default();
        let (norm, db) = if need_norm {
            let o = self.opened(path)?;
            (&*o.norm, &o.db)
        } else {
            (&empty, self.database(path)?)
        };
        let used = tables_in_sql(norm, sql);
        let res = match first {
            Ok(r) => r,
            Err(e) if e == sql::CANCELLED => return Err(cancelled()),
            Err(e) => return Err(ToolError(sql_error_hint(norm, db, &used, sql, &e, timeout))),
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
        // B75/B88: why a result is empty, values only in T<n>_all, NULL from a + b (notes.rs).
        let notes = notes::result_notes(db, sql, &res, timeout);
        if res.total == 0 && notes.is_empty() {
            for t in &used {
                let info = catalog::row_info(norm, t);
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
        lines.extend(notes);
        if let Some(hint) = cite_hint(db, &used, &res.header) {
            lines.push(hint);
        }
        if let Some(note) = spellings_note(db, &used, &res) {
            lines.push(note);
        }
        let n = text::thousands(res.total);
        lines.push(match (res.complete, shown < res.total) {
            (true, false) => format!("({n} row{})", if res.total == 1 { "" } else { "s" }),
            (true, true) => format!("({n} rows, showing first {shown})"),
            (false, _) => format!("(more than {n} rows, showing first {shown}; counting stopped)"),
        });
        if res.cut_values > 0 {
            lines.push(format!(
                "note: {} value(s) longer than {} characters end in …[+N chars]; read the rest with substr(col, {}, {})",
                res.cut_values,
                text::thousands(sql::MAX_CELL_CHARS),
                sql::MAX_CELL_CHARS + 1,
                sql::MAX_CELL_CHARS
            ));
        }
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
            let wb = hh_core::reader::open_with(&p, self.opts.read_options())
                .map_err(|e| ToolError(format!("{}: cannot read: {}", self.bounds.display(&p), limit_hint(&e))))?;
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

/// A reader error, with the server flag to raise when the file was refused over a limit.
fn limit_hint(e: &hh_core::Error) -> String {
    let s = e.to_string();
    if !e.is_limit() {
        return s;
    }
    let flag = if s.contains("cells") {
        " (raise with --max-cells)"
    } else if s.contains("time limit") {
        " (raise with --max-seconds)"
    } else {
        ""
    };
    format!("{s}{flag}")
}

/// An opened file whose normalized form and database are ready.
struct Loaded {
    norm: Arc<Normalized>,
    db: sql::Db,
}

/// The catalog of `open_file` without the timing line.
fn catalog_text(norm: &Normalized, name: &str, skipped: &[(String, usize)]) -> String {
    let mut text = catalog::catalog(norm, name, MAX_CHARS - 300);
    for (t, n) in skipped {
        text.push_str(&format!("\nNOTE {t}: {n} columns beyond SQLite's 2000-column limit are not queryable"));
    }
    text
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
        // B88: `_row_type` is only in the _all views.
        if wanted.eq_ignore_ascii_case("_row_type") {
            let id = used
                .iter()
                .find(|t| !sql_mentions(sql, &format!("{}_all", t.id())))
                .or(used.first())
                .map_or_else(|| "T1".to_string(), |t| t.id());
            msg.push_str(&format!(
                " — {id} has the data rows only and no _row_type; the total and subtotal rows and _row_type are in {id}_all: SELECT … FROM {id}_all WHERE _row_type = 'total'"
            ));
            return msg;
        }
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
        if let Some(h) = derived_hint(db, sql, wanted, &names) {
            msg.push_str(&h); // B88
        } else if let Some((h, rows_sql)) = row_label_hint(norm, &scope, wanted) {
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

/// B88: a column the outer query takes from a subquery or CTE that does not select it (an outer
/// `SUM("price")` over `(SELECT buyer, SUM("price") AS total …)`): the columns it does have.
fn derived_hint(db: &sql::Db, sql: &str, wanted: &str, base: &[(String, String)]) -> Option<String> {
    let toks = sqlscan::tokens(sql);
    let ds = sqlscan::derived(sql, &toks);
    if ds.is_empty() {
        return None;
    }
    let cols: Vec<(String, Vec<String>)> = ds
        .iter()
        .filter_map(|d| {
            let c = db.columns(&d.columns_sql)?;
            let name = d.name.as_ref().map_or_else(|| "the subquery".to_string(), |n| format!("{} ({})", ident(n), if d.cte { "CTE" } else { "subquery" }));
            Some((name, c))
        })
        .collect();
    if cols.is_empty() || cols.iter().any(|(_, c)| c.iter().any(|x| x.eq_ignore_ascii_case(wanted))) {
        return None;
    }
    let listed: Vec<String> = cols
        .iter()
        .map(|(n, c)| format!("{n}: {}", short(&c.iter().map(|x| ident(x)).collect::<Vec<_>>().join(", "), 300)))
        .collect();
    let near = closest(wanted, base.iter().map(|(_, n)| n.as_str()), 1);
    let exact = base.iter().find(|(_, n)| n.eq_ignore_ascii_case(wanted));
    if let Some((t, n)) = exact.or_else(|| base.iter().find(|(_, n)| near.first() == Some(&n.as_str()))) {
        let what = if exact.is_some() { format!("{} is a column of {t}", ident(n)) } else { format!("the closest column is {t}.{}", quote_ident(n)) };
        return Some(format!(
            " — {what}, but this part of the query reads from a subquery or CTE that does not select it ({}); select {} inside it, with an alias, and use that name outside",
            listed.join("; "),
            ident(n)
        ));
    }
    let all: Vec<&str> = cols.iter().flat_map(|(_, c)| c.iter().map(String::as_str)).collect();
    let close = closest(wanted, all, 3);
    (!close.is_empty()).then(|| {
        format!(
            " — did you mean {}? (columns of the subquery or CTE: {})",
            close.iter().map(|c| ident(c)).collect::<Vec<_>>().join(", "),
            listed.join("; ")
        )
    })
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
