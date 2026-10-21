//! Disk cache of normalized files, so a new session reopens a file it has seen before in
//! milliseconds instead of normalizing it again.
//!
//! One entry per file content, keyed by the file's sha256, this build ([`BUILD_ID`]: a hash of
//! the core and agent sources and the row model) and the normalize options. An entry is a folder
//! holding the SQLite database of the file's tables (`db.sqlite`, opened read-only in place), the
//! normalized form (`norm.bin`, [`write_norm`]) and `meta.json` (the catalog per display name,
//! normalize time, sizes). A file's (path, size, mtime) → sha256 shortcut is kept in `hashes.tsv`,
//! so an unchanged file is not hashed again either.
//!
//! Location: `$HEDGEHOG_CACHE_DIR`, else the user cache folder (`$XDG_CACHE_HOME/hedgehog` or
//! `~/.cache/hedgehog`, `~/Library/Caches/hedgehog`, `%LOCALAPPDATA%\hedgehog\cache`).
//! `HEDGEHOG_CACHE=0` turns it off. Size cap `HEDGEHOG_CACHE_MAX_MB` (default 2048), least
//! recently used entries are removed first. Entries are written into a temporary folder and
//! renamed into place; an entry that does not validate is removed and rebuilt. Folders are 0700
//! and files 0600 (the cache holds the user's data).

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use hh_core::model::Rect;
use hh_core::normalize::{
    sha256, CellCounts, ColRole, Column, Dtype, NoteItem, Normalized, Out, RowType, SheetInfo, Table, TotalItem, Transforms, T,
};
use hh_core::values::{DateOrder, DecimalSep};
use serde_json::{json, Value};

/// Hash of the sources that decide a normalized file and its catalog (set by `build.rs`).
pub const BUILD_ID: &str = env!("HH_BUILD_ID");
/// Layout version of an entry.
const FORMAT: u64 = 1;
const DEFAULT_MAX_MB: u64 = 2048;
const NORM_MAGIC: &[u8; 8] = b"HHNORM03";
/// Display names whose catalog an entry keeps.
const MAX_CATALOGS: usize = 8;
/// Lines kept in `hashes.tsv`.
const MAX_HASHES: usize = 2000;

/// A cache folder and its size cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    pub dir: PathBuf,
    pub max_bytes: u64,
}

/// A valid entry found in the cache.
#[derive(Debug, Clone)]
pub struct Entry {
    pub dir: PathBuf,
    meta: Value,
}

impl Entry {
    pub fn db_path(&self) -> PathBuf {
        self.dir.join("db.sqlite")
    }
    pub fn norm_path(&self) -> PathBuf {
        self.dir.join("norm.bin")
    }
    /// Seconds the first normalization took.
    pub fn seconds(&self) -> f64 {
        self.meta.get("seconds").and_then(Value::as_f64).unwrap_or(0.0)
    }
    /// The catalog stored for a display name.
    pub fn catalog(&self, name: &str) -> Option<String> {
        self.meta.pointer("/catalogs").and_then(|c| c.get(name)).and_then(Value::as_str).map(str::to_string)
    }
}

/// The user cache folder of this platform (without `hedgehog`).
fn user_cache_base() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        return env("LOCALAPPDATA").map(|p| p.join("hedgehog").join("cache"));
    }
    if cfg!(target_os = "macos") {
        return env("HOME").map(|h| h.join("Library/Caches/hedgehog"));
    }
    env("XDG_CACHE_HOME").filter(|p| p.is_absolute()).or_else(|| env("HOME").map(|h| h.join(".cache"))).map(|p| p.join("hedgehog"))
}

impl Cache {
    /// The cache the environment asks for: None when `HEDGEHOG_CACHE=0` (or `off`, `false`) or no
    /// folder is known.
    pub fn from_env() -> Option<Cache> {
        if let Some(v) = std::env::var_os("HEDGEHOG_CACHE") {
            let v = v.to_string_lossy().trim().to_ascii_lowercase();
            if matches!(v.as_str(), "0" | "off" | "false" | "no") {
                return None;
            }
        }
        let dir = std::env::var_os("HEDGEHOG_CACHE_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(user_cache_base)?;
        let max_mb = std::env::var("HEDGEHOG_CACHE_MAX_MB").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(DEFAULT_MAX_MB);
        Some(Cache { dir, max_bytes: max_mb << 20 })
    }

    fn root(&self) -> PathBuf {
        self.dir.join(format!("v{FORMAT}"))
    }

    /// The entry key of a file content under the normalize options `opts` (a short description).
    pub fn key(sha: &str, opts: &str) -> String {
        let id = format!("{sha}\n{}\n{BUILD_ID}\n{opts}", env!("CARGO_PKG_VERSION"));
        sha256::hex(id.as_bytes())[..40].to_string()
    }

    // -- the (path, size, mtime) → sha256 shortcut ---------------------------------------------------

    fn hashes_path(&self) -> PathBuf {
        self.root().join("hashes.tsv")
    }

    fn read_hashes(&self) -> Vec<(String, u64, u128, String)> {
        let Ok(s) = fs::read_to_string(self.hashes_path()) else { return Vec::new() };
        s.lines()
            .filter_map(|l| {
                let mut it = l.splitn(4, '\t');
                let size = it.next()?.parse().ok()?;
                let mtime = it.next()?.parse().ok()?;
                let sha = it.next()?;
                let path = it.next()?;
                (sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (path.to_string(), size, mtime, sha.to_string()))
            })
            .collect()
    }

    /// The sha256 recorded for this path, size and mtime.
    pub fn known_hash(&self, path: &Path, size: u64, mtime: Option<SystemTime>) -> Option<String> {
        let m = nanos(mtime?)?;
        let p = path.to_string_lossy();
        self.read_hashes().into_iter().find(|(q, s, t, _)| *q == p && *s == size && *t == m).map(|x| x.3)
    }

    /// Records a file's sha256 (best effort).
    pub fn remember_hash(&self, path: &Path, size: u64, mtime: Option<SystemTime>, sha: &str) {
        let (Some(m), Some(p)) = (mtime.and_then(nanos), path.to_str()) else { return };
        if p.contains(['\t', '\n', '\r']) {
            return;
        }
        let _lock = LOCK.lock();
        let mut all = self.read_hashes();
        all.retain(|(q, ..)| q != p);
        all.push((p.to_string(), size, m, sha.to_string()));
        if all.len() > MAX_HASHES {
            all.drain(..all.len() - MAX_HASHES);
        }
        let text: String = all.iter().map(|(q, s, t, h)| format!("{s}\t{t}\t{h}\t{q}\n")).collect();
        let _ = self.ensure_root().and_then(|root| write_atomic(&root, &self.hashes_path(), text.as_bytes()));
    }

    fn ensure_root(&self) -> io::Result<PathBuf> {
        let root = self.root();
        create_private_dir_all(&root)?;
        Ok(root)
    }

    // -- entries ------------------------------------------------------------------------------------

    /// The valid entry for `key`, marked as just used. An entry that does not validate is removed.
    pub fn get(&self, key: &str) -> Option<Entry> {
        let dir = self.root().join(key);
        let meta: Option<Value> = fs::read(dir.join("meta.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let valid = meta.as_ref().is_some_and(|m| {
            let size = |f: &str, k: &str| fs::metadata(dir.join(f)).ok().map(|x| x.len()) == m.get(k).and_then(Value::as_u64);
            m.get("format").and_then(Value::as_u64) == Some(FORMAT)
                && m.get("key").and_then(Value::as_str) == Some(key)
                && size("db.sqlite", "db_bytes")
                && size("norm.bin", "norm_bytes")
        });
        if !valid {
            if dir.exists() {
                let _ = fs::remove_dir_all(&dir);
            }
            return None;
        }
        touch(&dir.join("meta.json"));
        Some(Entry { dir, meta: meta? })
    }

    /// Writes a complete entry: the SQLite database and normalized form of `norm`, and `catalog`
    /// for `name`. Written into a temporary folder and renamed into place, then the cache is
    /// trimmed to its size cap. Returns the entry and the database's [`crate::sql::Plan`].
    pub fn put(
        &self,
        key: &str,
        norm: &Normalized,
        seconds: f64,
        name: &str,
        catalog: Option<&str>,
    ) -> Result<(Entry, crate::sql::Plan), String> {
        let t0 = Instant::now();
        let root = self.ensure_root().map_err(|e| format!("cache folder {}: {e}", self.root().display()))?;
        let tmp = root.join(format!(".tmp-{key}-{}-{}", std::process::id(), nanos(SystemTime::now()).unwrap_or(0)));
        let res = (|| -> Result<(Entry, crate::sql::Plan), String> {
            create_private_dir_all(&tmp).map_err(|e| e.to_string())?;
            let db = tmp.join("db.sqlite");
            let plan = crate::sql::Db::write_file(norm, &db).map_err(|e| format!("cache database: {e}"))?;
            set_private(&db);
            trace("cache: database", t0);
            let nb = tmp.join("norm.bin");
            write_norm(norm, &nb).map_err(|e| format!("cache norm.bin: {e}"))?;
            trace("cache: norm.bin", t0);
            let mut catalogs = serde_json::Map::new();
            if let Some(c) = catalog {
                catalogs.insert(name.to_string(), Value::String(c.to_string()));
            }
            let meta = json!({
                "format": FORMAT,
                "key": key,
                "version": env!("CARGO_PKG_VERSION"),
                "build": BUILD_ID,
                "seconds": seconds,
                "db_bytes": fs::metadata(&db).map_err(|e| e.to_string())?.len(),
                "norm_bytes": fs::metadata(&nb).map_err(|e| e.to_string())?.len(),
                "catalogs": catalogs,
            });
            write_private(&tmp.join("meta.json"), &serde_json::to_vec(&meta).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let dir = root.join(key);
            if fs::rename(&tmp, &dir).is_err() {
                // Another process was faster (or a broken entry is in the way).
                if self.get(key).is_none() {
                    let _ = fs::remove_dir_all(&dir);
                    fs::rename(&tmp, &dir).map_err(|e| e.to_string())?;
                }
            }
            Ok((Entry { dir, meta }, plan))
        })();
        if tmp.exists() {
            let _ = fs::remove_dir_all(&tmp);
        }
        let (entry, plan) = res?;
        self.trim(Some(&entry.dir));
        trace("cache: entry written", t0);
        Ok((entry, plan))
    }

    /// Adds the catalog of another display name to an entry (best effort).
    pub fn add_catalog(&self, entry: &mut Entry, name: &str, catalog: &str) {
        let _lock = LOCK.lock();
        let Some(cats) = entry.meta.get_mut("catalogs").and_then(Value::as_object_mut) else { return };
        if cats.len() >= MAX_CATALOGS {
            cats.clear();
        }
        cats.insert(name.to_string(), Value::String(catalog.to_string()));
        if let Ok(bytes) = serde_json::to_vec(&entry.meta) {
            let _ = write_atomic(&entry.dir, &entry.dir.join("meta.json"), &bytes);
        }
    }

    /// Removes the least recently used entries until the cache fits its cap, and temporary
    /// folders left by crashed writers. `keep` is never removed.
    pub fn trim(&self, keep: Option<&Path>) {
        let _lock = LOCK.lock();
        let Ok(rd) = fs::read_dir(self.root()) else { return };
        let mut entries: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let now = SystemTime::now();
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(".tmp-") {
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| now.duration_since(m).ok());
                if old.is_some_and(|d| d.as_secs() > 6 * 3600) {
                    let _ = fs::remove_dir_all(&p);
                }
                continue;
            }
            let used = fs::metadata(p.join("meta.json")).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let size: u64 = fs::read_dir(&p).map(|r| r.flatten().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0);
            entries.push((used, size, p));
        }
        let mut total: u64 = entries.iter().map(|e| e.1).sum();
        entries.sort();
        for (_, size, p) in entries {
            if total <= self.max_bytes {
                break;
            }
            if Some(p.as_path()) == keep {
                continue;
            }
            if fs::remove_dir_all(&p).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }
}

/// Serializes cache writes within this process (other processes only ever rename whole files
/// or folders into place).
static LOCK: Mutex<()> = Mutex::new(());

fn nanos(t: SystemTime) -> Option<u128> {
    t.duration_since(SystemTime::UNIX_EPOCH).ok().map(|d| d.as_nanos())
}

fn touch(p: &Path) {
    if let Ok(f) = File::options().append(true).open(p) {
        let _ = f.set_modified(SystemTime::now());
    }
}

fn create_private_dir_all(p: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(p)
}

fn set_private(p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = p;
}

fn create_private(p: &Path) -> io::Result<File> {
    let mut o = File::options();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(p)
}

fn write_private(p: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = create_private(p)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Writes `p` through a temporary file in `dir` and a rename.
fn write_atomic(dir: &Path, p: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!(".w-{}-{}", std::process::id(), nanos(SystemTime::now()).unwrap_or(0)));
    let r = write_private(&tmp, bytes).and_then(|_| fs::rename(&tmp, p));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// Development aid: with `HEDGEHOG_TRACE=1`, elapsed time per stage to stderr.
pub(crate) fn trace(stage: &str, since: Instant) {
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HEDGEHOG_TRACE").is_some_and(|v| !v.is_empty() && v != "0")) {
        eprintln!("[trace] {stage:<24} {:>9.1} ms", since.elapsed().as_secs_f64() * 1000.0);
    }
}

// -- norm.bin: the normalized form, field by field -------------------------------------------------

struct W<O: Write>(O);

impl<O: Write> W<O> {
    fn u8(&mut self, v: u8) -> io::Result<()> {
        self.0.write_all(&[v])
    }
    fn u32(&mut self, v: u32) -> io::Result<()> {
        self.0.write_all(&v.to_le_bytes())
    }
    fn u64(&mut self, v: u64) -> io::Result<()> {
        self.0.write_all(&v.to_le_bytes())
    }
    fn usize(&mut self, v: usize) -> io::Result<()> {
        self.u64(v as u64)
    }
    fn f64(&mut self, v: f64) -> io::Result<()> {
        self.u64(v.to_bits())
    }
    fn bool(&mut self, v: bool) -> io::Result<()> {
        self.u8(u8::from(v))
    }
    fn str(&mut self, s: &str) -> io::Result<()> {
        self.usize(s.len())?;
        self.0.write_all(s.as_bytes())
    }
    fn opt<V>(&mut self, v: &Option<V>, f: impl FnOnce(&mut Self, &V) -> io::Result<()>) -> io::Result<()> {
        match v {
            None => self.u8(0),
            Some(x) => {
                self.u8(1)?;
                f(self, x)
            }
        }
    }
    fn vec<V>(&mut self, v: &[V], mut f: impl FnMut(&mut Self, &V) -> io::Result<()>) -> io::Result<()> {
        self.usize(v.len())?;
        v.iter().try_for_each(|x| f(self, x))
    }
    fn pos(&mut self, p: &(u32, u32)) -> io::Result<()> {
        self.u32(p.0)?;
        self.u32(p.1)
    }
    fn text_at(&mut self, x: &(String, (u32, u32))) -> io::Result<()> {
        self.str(&x.0)?;
        self.pos(&x.1)
    }
    fn rect(&mut self, r: &Rect) -> io::Result<()> {
        [r.top, r.left, r.bottom, r.right].iter().try_for_each(|v| self.u32(*v))
    }
}

/// Writes the normalized form of a file to `path` (0600).
pub fn write_norm(n: &Normalized, path: &Path) -> io::Result<()> {
    let mut w = W(BufWriter::with_capacity(1 << 20, create_private(path)?));
    w.0.write_all(NORM_MAGIC)?;
    w.vec(&n.sheets, |w, s| {
        w.str(&s.name)?;
        w.opt(&s.used_range, |w, r| w.rect(r))?;
        w.usize(s.cells)?;
        w.usize(s.formulas)?;
        w.usize(s.formulas_without_value)?;
        w.opt(&s.formula_without_value_at, |w, (r, c)| w.u32(*r).and_then(|_| w.u32(*c)))?;
        w.usize(s.merges)?;
        w.vec(&s.tables, |w, t| w.usize(*t))
    })?;
    w.vec(&n.tables, |w, t| {
        w.usize(t.n)?;
        w.str(&t.sheet)?;
        w.str(&t.source_sheet)?;
        w.rect(&t.rect)?;
        w.u32(t.header_top)?;
        w.u32(t.header_rows)?;
        w.opt(&t.title, |w, x| w.text_at(x))?;
        w.vec(&t.notes, |w, x| w.text_at(x))?;
        w.u32(t.confidence.to_bits())?;
        w.vec(&t.columns, |w, c| {
            w.str(&c.name)?;
            w.u8(c.role as u8)?;
            w.opt(&c.source_col, |w, v| w.u32(*v))?;
            w.vec(&c.original_header, |w, x| w.text_at(x))?;
            w.u8(c.dtype as u8)?;
            w.opt(&c.unit, |w, s| w.str(s))?;
            w.opt(&c.currency, |w, s| w.str(s))?;
            w.opt(&c.decimal, |w, d| w.u8(*d as u8))?;
            w.opt(&c.date_order, |w, d| w.u8(*d as u8))?;
            w.usize(c.ambiguous)?;
            w.vec(&c.footnote_marks, |w, (m, cells)| {
                w.str(m)?;
                w.vec(cells, |w, p| w.pos(p))
            })?;
            w.vec(&c.values, |w, v| match v {
                Out::Null => w.u8(0),
                Out::Num(x) => {
                    w.u8(1)?;
                    w.f64(*x)
                }
                Out::Date(x) => {
                    w.u8(2)?;
                    w.f64(*x)
                }
                Out::Bool(b) => w.u8(if *b { 4 } else { 3 }),
                Out::Text(s) => {
                    w.u8(5)?;
                    w.str(s)
                }
            })
        })?;
        w.vec(&t.src_rows, |w, r| w.u32(*r))?;
        w.vec(&t.row_types, |w, r| w.u8(*r as u8))?;
        w.vec(&t.transforms.0, |w, v| w.usize(*v))?;
        w.vec(&t.warnings, |w, s| w.str(s))
    })?;
    w.vec(&n.totals, |w, t| {
        w.opt(&t.table, |w, v| w.usize(*v))?;
        w.str(&t.sheet)?;
        w.u32(t.row)?;
        w.u32(t.col)?;
        w.usize(t.count)?;
        w.u32(t.row_end)?;
        w.opt(&t.label, |w, s| w.str(s))?;
        w.opt(&t.label_cell, |w, p| w.pos(p))?;
        w.str(t.level)?;
        w.str(t.direction)?;
        w.str(t.origin)?;
        w.opt(&t.value, |w, v| w.f64(*v))?;
        w.opt(&t.recomputed, |w, v| w.f64(*v))?;
        w.str(t.status)?;
        w.str(&t.reason)?;
        w.vec(&t.sources, |w, s| w.str(s))?;
        w.opt(&t.formula, |w, s| w.str(s))?;
        w.bool(t.kept_in_data)
    })?;
    w.vec(&n.notes, |w, x| {
        w.str(&x.sheet)?;
        w.u32(x.row)?;
        w.u32(x.col)?;
        w.str(x.kind)?;
        w.opt(&x.table, |w, v| w.usize(*v))?;
        w.str(&x.text)?;
        w.opt(&x.formula, |w, s| w.str(s))
    })?;
    for v in [n.cells.total, n.cells.in_tables, n.cells.in_totals, n.cells.in_notes] {
        w.usize(v)?;
    }
    w.vec(&n.warnings, |w, s| w.str(s))?;
    w.0.write_all(NORM_MAGIC)?;
    let f = w.0.into_inner().map_err(|e| e.into_error())?;
    f.sync_all()
}

/// Reads the bytes of `norm.bin`; every length is checked against what is left.
struct R<'a> {
    buf: &'a [u8],
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("norm.bin: bad {what}"))
}

impl R<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if n > self.buf.len() {
            return Err(bad("length"));
        }
        let (a, b) = self.buf.split_at(n);
        self.buf = b;
        Ok(a)
    }
    fn bytes<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let mut b = [0u8; N];
        b.copy_from_slice(self.take(N)?);
        Ok(b)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }
    fn usize(&mut self) -> io::Result<usize> {
        usize::try_from(self.u64()?).map_err(|_| bad("number"))
    }
    fn f64(&mut self) -> io::Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }
    fn bool(&mut self) -> io::Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(bad("bool")),
        }
    }
    /// A length of a list whose items take at least `min` bytes each.
    fn len(&mut self, min: usize) -> io::Result<usize> {
        let n = self.usize()?;
        if n.saturating_mul(min.max(1)) > self.buf.len() {
            return Err(bad("length"));
        }
        Ok(n)
    }
    fn str(&mut self) -> io::Result<String> {
        let n = self.usize()?;
        std::str::from_utf8(self.take(n)?).map(str::to_string).map_err(|_| bad("text"))
    }
    fn opt<V>(&mut self, f: impl FnOnce(&mut Self) -> io::Result<V>) -> io::Result<Option<V>> {
        match self.u8()? {
            0 => Ok(None),
            1 => f(self).map(Some),
            _ => Err(bad("option")),
        }
    }
    fn vec<V>(&mut self, min: usize, mut f: impl FnMut(&mut Self) -> io::Result<V>) -> io::Result<Vec<V>> {
        let n = self.len(min)?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(f(self)?);
        }
        Ok(v)
    }
    fn pos(&mut self) -> io::Result<(u32, u32)> {
        Ok((self.u32()?, self.u32()?))
    }
    fn text_at(&mut self) -> io::Result<(String, (u32, u32))> {
        Ok((self.str()?, self.pos()?))
    }
    fn rect(&mut self) -> io::Result<Rect> {
        Ok(Rect { top: self.u32()?, left: self.u32()?, bottom: self.u32()?, right: self.u32()? })
    }
    /// One of the fixed words of a total or note item.
    fn word(&mut self) -> io::Result<&'static str> {
        let s = self.str()?;
        Ok(intern(s))
    }
}

/// The `&'static str` of a word that a total or note item holds. Known words map to literals;
/// an unknown one (from a newer core) is leaked once and reused.
fn intern(s: String) -> &'static str {
    const KNOWN: &[&str] = &[
        "total", "subtotal", "grand_total", "vertical", "horizontal", "formula", "typed", "layout", "ok", "mismatch",
        "unknown", "title", "note", "repeated_header", "total_row", "other",
    ];
    if let Some(k) = KNOWN.iter().find(|k| **k == s) {
        return k;
    }
    static MORE: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut more = MORE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = more.iter().find(|k| **k == s) {
        return k;
    }
    let k: &'static str = Box::leak(s.into_boxed_str());
    more.push(k);
    k
}

const ROLES: [ColRole; 4] = [ColRole::Data, ColRole::Group, ColRole::SrcRow, ColRole::RowType];
const DTYPES: [Dtype; 9] =
    [Dtype::Integer, Dtype::Number, Dtype::Percent, Dtype::Currency, Dtype::Date, Dtype::Datetime, Dtype::Text, Dtype::Bool, Dtype::Empty];
const ROW_TYPES: [RowType; 5] = [RowType::Data, RowType::Subtotal, RowType::Total, RowType::Average, RowType::OfWhich];
const DECIMALS: [DecimalSep; 2] = [DecimalSep::Dot, DecimalSep::Comma];
const DATE_ORDERS: [DateOrder; 2] = [DateOrder::DayMonth, DateOrder::MonthDay];

fn pick<V: Copy>(all: &[V], i: u8, what: &str) -> io::Result<V> {
    all.get(usize::from(i)).copied().ok_or_else(|| bad(what))
}

/// Reads what [`write_norm`] wrote. Any damage is an error, never a panic.
pub fn read_norm(path: &Path) -> io::Result<Normalized> {
    // The enum tables above must list every variant in declaration order.
    debug_assert!(ROLES.iter().enumerate().all(|(i, r)| *r as usize == i));
    debug_assert!(DTYPES.iter().enumerate().all(|(i, r)| *r as usize == i));
    let bytes = fs::read(path)?;
    let mut r = R { buf: &bytes };
    if &r.bytes::<8>()? != NORM_MAGIC {
        return Err(bad("header"));
    }
    let sheets = r.vec(40, |r| {
        Ok(SheetInfo {
            name: r.str()?,
            used_range: r.opt(|r| r.rect())?,
            cells: r.usize()?,
            formulas: r.usize()?,
            formulas_without_value: r.usize()?,
            formula_without_value_at: r.opt(|r| Ok((r.u32()?, r.u32()?)))?,
            merges: r.usize()?,
            tables: r.vec(8, |r| r.usize())?,
        })
    })?;
    let tables = r.vec(64, |r| {
        let n = r.usize()?;
        let sheet = r.str()?;
        let source_sheet = r.str()?;
        let rect = r.rect()?;
        let header_top = r.u32()?;
        let header_rows = r.u32()?;
        let title = r.opt(|r| r.text_at())?;
        let notes = r.vec(16, |r| r.text_at())?;
        let confidence = f32::from_bits(r.u32()?);
        let columns = r.vec(40, |r| {
            Ok(Column {
                name: r.str()?,
                role: pick(&ROLES, r.u8()?, "role")?,
                source_col: r.opt(|r| r.u32())?,
                original_header: r.vec(16, |r| r.text_at())?,
                dtype: pick(&DTYPES, r.u8()?, "dtype")?,
                unit: r.opt(|r| r.str())?,
                currency: r.opt(|r| r.str())?,
                decimal: r.opt(|r| pick(&DECIMALS, r.u8()?, "decimal"))?,
                date_order: r.opt(|r| pick(&DATE_ORDERS, r.u8()?, "date order"))?,
                ambiguous: r.usize()?,
                footnote_marks: r.vec(16, |r| Ok((r.str()?, r.vec(8, |r| r.pos())?)))?,
                values: r.vec(1, |r| {
                    Ok(match r.u8()? {
                        0 => Out::Null,
                        1 => Out::Num(r.f64()?),
                        2 => Out::Date(r.f64()?),
                        3 => Out::Bool(false),
                        4 => Out::Bool(true),
                        5 => Out::Text(r.str()?),
                        _ => return Err(bad("value")),
                    })
                })?,
            })
        })?;
        let src_rows = r.vec(4, |r| r.u32())?;
        let row_types = r.vec(1, |r| pick(&ROW_TYPES, r.u8()?, "row type"))?;
        let counts = r.vec(8, |r| r.usize())?;
        let mut transforms = Transforms::default();
        if counts.len() != T::ALL.len() {
            return Err(bad("transforms"));
        }
        transforms.0.copy_from_slice(&counts);
        let warnings = r.vec(8, |r| r.str())?;
        Ok(Table {
            n,
            sheet,
            source_sheet,
            rect,
            header_top,
            header_rows,
            title,
            notes,
            confidence,
            columns,
            src_rows,
            row_types,
            transforms,
            warnings,
        })
    })?;
    let totals = r.vec(60, |r| {
        Ok(TotalItem {
            table: r.opt(|r| r.usize())?,
            sheet: r.str()?,
            row: r.u32()?,
            col: r.u32()?,
            count: r.usize()?,
            row_end: r.u32()?,
            label: r.opt(|r| r.str())?,
            label_cell: r.opt(|r| r.pos())?,
            level: r.word()?,
            direction: r.word()?,
            origin: r.word()?,
            value: r.opt(|r| r.f64())?,
            recomputed: r.opt(|r| r.f64())?,
            status: r.word()?,
            reason: r.str()?,
            sources: r.vec(8, |r| r.str())?,
            formula: r.opt(|r| r.str())?,
            kept_in_data: r.bool()?,
        })
    })?;
    let notes = r.vec(30, |r| {
        Ok(NoteItem {
            sheet: r.str()?,
            row: r.u32()?,
            col: r.u32()?,
            kind: r.word()?,
            table: r.opt(|r| r.usize())?,
            text: r.str()?,
            formula: r.opt(|r| r.str())?,
        })
    })?;
    let cells = CellCounts { total: r.usize()?, in_tables: r.usize()?, in_totals: r.usize()?, in_notes: r.usize()? };
    let warnings = r.vec(8, |r| r.str())?;
    if &r.bytes::<8>()? != NORM_MAGIC || !r.buf.is_empty() {
        return Err(bad("end"));
    }
    Ok(Normalized { sheets, tables, totals, notes, cells, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Normalized {
        let mut n = crate::sql::tests::sample();
        n.totals.push(TotalItem {
            table: Some(1),
            sheet: "Sheet1".into(),
            row: 4,
            col: 1,
            count: 1,
            row_end: 4,
            label: Some("Total".into()),
            label_cell: Some((4, 0)),
            level: "grand_total",
            direction: "vertical",
            origin: "formula",
            value: Some(f64::NAN),
            recomputed: None,
            status: "a-new-status",
            reason: "r".into(),
            sources: vec!["B2:B3".into()],
            formula: Some("SUM(B2:B3)".into()),
            kept_in_data: true,
        });
        n.notes.push(NoteItem { sheet: "S".into(), row: 1, col: 2, kind: "note", table: None, text: "ünï".into(), formula: None });
        n.warnings.push("w".into());
        n
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hh_agent_cache_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn norm_round_trip_and_damage() {
        let d = tmp("norm");
        let n = sample();
        let p = d.join("norm.bin");
        write_norm(&n, &p).unwrap();
        let back = read_norm(&p).unwrap();
        // NaN != NaN: compare the debug text, which prints NaN the same way.
        assert_eq!(format!("{back:?}"), format!("{n:?}"));
        assert_eq!(back.totals[0].status, "a-new-status");
        let bytes = fs::read(&p).unwrap();
        // Truncated anywhere, or with a flipped byte in a length: an error, never a panic.
        for cut in [0, 7, 8, 9, bytes.len() / 2, bytes.len() - 1] {
            fs::write(&p, &bytes[..cut]).unwrap();
            assert!(read_norm(&p).is_err(), "cut at {cut}");
        }
        for i in (8..bytes.len()).step_by(7) {
            let mut b = bytes.clone();
            b[i] ^= 0xff;
            fs::write(&p, &b).unwrap();
            let _ = read_norm(&p);
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn entries_hashes_and_trim() {
        let d = tmp("entries");
        let c = Cache { dir: d.join("c"), max_bytes: 1 << 30 };
        let n = sample();
        let k1 = Cache::key(&"a".repeat(64), "keep_totals");
        assert_ne!(k1, Cache::key(&"a".repeat(64), "rules_only"));
        assert!(c.get(&k1).is_none());
        let (mut e, _) = c.put(&k1, &n, 1.5, "x.xlsx", Some("catalog of x")).unwrap();
        let got = c.get(&k1).unwrap();
        assert_eq!(got.catalog("x.xlsx").as_deref(), Some("catalog of x"));
        assert_eq!(got.catalog("y.xlsx"), None);
        assert_eq!(got.seconds(), 1.5);
        assert_eq!(read_norm(&got.norm_path()).unwrap().tables, n.tables);
        c.add_catalog(&mut e, "y.xlsx", "catalog of y");
        assert_eq!(c.get(&k1).unwrap().catalog("y.xlsx").as_deref(), Some("catalog of y"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&c.root()), 0o700);
            assert_eq!(mode(&got.dir), 0o700);
            for f in ["db.sqlite", "norm.bin", "meta.json"] {
                assert_eq!(mode(&got.dir.join(f)), 0o600, "{f}");
            }
        }
        // A damaged entry is dropped.
        fs::write(got.dir.join("norm.bin"), b"short").unwrap();
        assert!(c.get(&k1).is_none());
        assert!(!got.dir.exists());
        // Hash shortcut.
        let f = d.join("f.csv");
        fs::write(&f, b"a,b\n1,2\n").unwrap();
        let m = fs::metadata(&f).unwrap();
        assert_eq!(c.known_hash(&f, m.len(), m.modified().ok()), None);
        c.remember_hash(&f, m.len(), m.modified().ok(), &"b".repeat(64));
        assert_eq!(c.known_hash(&f, m.len(), m.modified().ok()), Some("b".repeat(64)));
        assert_eq!(c.known_hash(&f, m.len() + 1, m.modified().ok()), None);
        // Size cap: the older entry goes first, the one just written stays.
        let small = Cache { dir: c.dir.clone(), max_bytes: 1 };
        let ka = Cache::key(&"c".repeat(64), "o");
        let kb = Cache::key(&"d".repeat(64), "o");
        small.put(&ka, &n, 0.1, "a", None).unwrap();
        let (eb, _) = small.put(&kb, &n, 0.1, "b", None).unwrap();
        assert!(small.get(&ka).is_none());
        assert!(eb.dir.exists());
        let _ = fs::remove_dir_all(&d);
    }
}
