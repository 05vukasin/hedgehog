//! `hh_fuzz`: a stable-Rust mutation fuzzer for hedgehog's readers and normalize pipeline.
//!
//! cargo-fuzz needs a nightly toolchain; this needs none. It is a plain in-process loop: take an
//! input from the corpus, mutate it (structure-aware for zip packages: the XML parts are mutated
//! and the package re-zipped, so the mutations reach the XML parsers instead of failing the CRC),
//! write it to `<out>/cur.<ext>`, open it with [`hh_core::reader`] and, when it is small, run the
//! whole normalize pipeline plus the agent catalog and SQL load on it.
//!
//! - a panic is caught, the input saved to `<out>/crashes/` (one per panic location) and the loop
//!   goes on;
//! - an input that runs longer than `--timeout` seconds is saved to `<out>/hangs/` and the process
//!   exits with code 70;
//! - more than `--rss-mb` of live heap makes the allocation fail, which aborts the process.
//!
//! `fuzz/run.sh` restarts the process after an exit and keeps the input (`cur.<ext>`) that killed
//! it. Inputs that produce a new outcome (an error message never seen, a new shape of result) are
//! added to the in-memory corpus, a cheap stand-in for coverage feedback.
//!
//! Usage:
//!   hh_fuzz fuzz <target> --corpus DIR --out DIR [--seconds N] [--timeout S] [--rss-mb N] [--seed N]
//!   hh_fuzz run <file>...            (one exec per file, prints the outcome; for reproducing)
//!   hh_fuzz seeds <dir>              (writes generated seed workbooks)
//! targets: csv, xlsx, xls, ods, normalize

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod grid;
mod seeds;

// -- memory limit -------------------------------------------------------------------------------

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size() > LIMIT.load(Ordering::Relaxed) {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            over_limit(l.size());
            return std::ptr::null_mut();
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size() > LIMIT.load(Ordering::Relaxed) {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            over_limit(l.size());
            return std::ptr::null_mut();
        }
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if new > l.size() {
            let d = new - l.size();
            if LIVE.fetch_add(d, Ordering::Relaxed) + d > LIMIT.load(Ordering::Relaxed) {
                LIVE.fetch_sub(d, Ordering::Relaxed);
                over_limit(new);
                return std::ptr::null_mut();
            }
        } else {
            LIVE.fetch_sub(l.size() - new, Ordering::Relaxed);
        }
        let q = unsafe { System.realloc(p, l, new) };
        if q.is_null() {
            // undo: the old block stays allocated
            if new > l.size() {
                LIVE.fetch_sub(new - l.size(), Ordering::Relaxed);
            } else {
                LIVE.fetch_add(l.size() - new, Ordering::Relaxed);
            }
        }
        q
    }
}

/// With `HH_FUZZ_BACKTRACE` set (`run` mode): print where the heap limit was hit, then abort.
static OOM_BACKTRACE: AtomicBool = AtomicBool::new(false);

fn over_limit(size: usize) {
    if OOM_BACKTRACE.swap(false, Ordering::Relaxed) {
        LIMIT.store(usize::MAX, Ordering::Relaxed);
        eprintln!("heap limit hit allocating {size} bytes (live {} MB) at:\n{}", LIVE.load(Ordering::Relaxed) >> 20, std::backtrace::Backtrace::force_capture());
        std::process::abort();
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

// -- PRNG ---------------------------------------------------------------------------------------

pub struct Rng(u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.next_u64() % den < num
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

// -- targets ------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Csv,
    Xlsx,
    Xls,
    Ods,
    Normalize,
}

impl Target {
    fn parse(s: &str) -> Option<Target> {
        Some(match s {
            "csv" => Target::Csv,
            "xlsx" => Target::Xlsx,
            "xls" => Target::Xls,
            "ods" => Target::Ods,
            "normalize" => Target::Normalize,
            _ => return None,
        })
    }
    fn ext(self) -> &'static str {
        match self {
            Target::Csv => "csv",
            Target::Xlsx => "xlsx",
            Target::Xls => "xls",
            Target::Ods => "ods",
            Target::Normalize => "grid",
        }
    }
    fn zipped(self) -> bool {
        matches!(self, Target::Xlsx | Target::Ods)
    }
}

/// Cells up to which an opened file also runs through normalize, catalog and SQL.
const PIPELINE_MAX_CELLS: usize = 4000;

/// Runs one input; returns an outcome signature (for corpus feedback).
fn exec(target: Target, path: &Path, data: &[u8]) -> String {
    let wb = match target {
        Target::Normalize => grid::workbook(data),
        _ => {
            let opts = hh_core::reader::ReadOptions { formats: true, ..Default::default() };
            // B136: large parts are read on several threads; here every part is, in chunks of a
            // few hundred bytes, and must read exactly as on one thread.
            calamine::set_parallel_read_for_tests(Some((1, 1 << 20, u64::MAX)));
            let one = hh_core::reader::open_with(path, opts);
            calamine::set_parallel_read_for_tests(Some((3, 64 + data.len() % 512, 0)));
            let par = hh_core::reader::open_with(path, opts);
            calamine::set_parallel_read_for_tests(None);
            let same = match (&one, &par) {
                (Ok(a), Ok(b)) => {
                    a.sheets.len() == b.sheets.len()
                        // as text: NaN equals NaN, -0 differs from 0
                        && a.sheets.iter().zip(&b.sheets).all(|(x, y)| {
                            format!("{:?}", (&x.name, &x.cells, &x.merges, &x.formats)) == format!("{:?}", (&y.name, &y.cells, &y.merges, &y.formats))
                        })
                }
                (Err(a), Err(b)) => a.to_string() == b.to_string(),
                _ => false,
            };
            assert!(same, "parallel read differs from the one-thread read");
            match one {
                Ok(wb) => wb,
                Err(e) => return format!("err:{}", squash(&e.to_string())),
            }
        }
    };
    if !QUIET.load(Ordering::Relaxed) {
        eprint!("{}", hh_core::describe(&wb));
        for s in &wb.sheets {
            eprintln!("  {} merges: {:?}", s.name, s.merges);
        }
    }
    let cells: usize = wb.sheets.iter().map(|s| s.cells.len()).sum();
    let merges: usize = wb.sheets.iter().map(|s| s.merges.len()).sum();
    let formulas: usize = wb.sheets.iter().map(|s| s.formula_count()).sum();
    let mut sig = format!("ok:{}:{}:{}:{}", wb.sheets.len().min(5), bucket(cells), bucket(merges), bucket(formulas));
    if cells <= PIPELINE_MAX_CELLS {
        sig.push_str(&pipeline(&wb));
    }
    sig
}

/// normalize → agent catalog, describe_table, totals → SQLite load and one query.
fn pipeline(wb: &hh_core::Workbook) -> String {
    let verbose = !QUIET.load(Ordering::Relaxed);
    let t = Instant::now();
    let stage = |name: &str| {
        if verbose {
            eprintln!("  {name}: {:.2} s", t.elapsed().as_secs_f64());
        }
    };
    let opts = hh_core::NormalizeOptions { keep_totals: true, ..Default::default() };
    let norm = hh_core::normalize_workbook(wb, &opts);
    stage("normalize");
    if verbose {
        for t in &norm.tables {
            eprintln!("  table {} on {}: {} rows x {} cols, rect {:?}", t.id(), t.sheet, t.rows(), t.columns.len(), t.rect);
        }
    }
    let mut text = hh_agent::catalog::catalog(&norm, "fuzz.xlsx", 12_000);
    stage("catalog");
    for t in &norm.tables {
        text.push_str(&hh_agent::catalog::describe_table(&norm, t));
    }
    stage("describe");
    let _ = hh_agent::catalog::totals(&norm, None, None);
    stage("totals");
    if let Ok(db) = hh_agent::sql::Db::load(&norm)
        && let Some(t) = norm.tables.first()
    {
        let _ = db.query(&format!("SELECT * FROM {} LIMIT 5", t.id()), 5, Duration::from_secs(2));
    }
    stage("sql");
    format!(":t{}", norm.tables.len().min(6))
}

fn bucket(n: usize) -> usize {
    match n {
        0 => 0,
        1..=9 => 1,
        10..=99 => 2,
        100..=999 => 3,
        _ => 4,
    }
}

/// Error text without numbers and paths, so similar errors share a signature.
fn squash(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
    let s = s.replace("##", "#");
    s.chars().take(60).collect()
}

// -- panics, hangs --------------------------------------------------------------------------------

static PANIC_INFO: Mutex<Option<(String, String)>> = Mutex::new(None);
/// Start of the current exec in ms since the fuzzer started; 0 = idle.
static EXEC_START: AtomicU64 = AtomicU64::new(0);
static QUIET: AtomicBool = AtomicBool::new(false);

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "?".into()
        };
        if !QUIET.load(Ordering::Relaxed) {
            eprintln!("panic at {loc}: {msg}\n{}", std::backtrace::Backtrace::force_capture());
        }
        if let Ok(mut p) = PANIC_INFO.lock() {
            *p = Some((loc, msg));
        }
    }));
}

fn watchdog(t0: Instant, timeout: Duration, cur: PathBuf, hangs: PathBuf) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let s = EXEC_START.load(Ordering::Relaxed);
            if s == 0 {
                continue;
            }
            let now = t0.elapsed().as_millis() as u64;
            if now.saturating_sub(s) > timeout.as_millis() as u64 {
                let _ = std::fs::create_dir_all(&hangs);
                let data = std::fs::read(&cur).unwrap_or_default();
                let name = format!("hang-{:016x}.{}", fnv(&data), cur.extension().and_then(|e| e.to_str()).unwrap_or("bin"));
                let _ = std::fs::write(hangs.join(&name), &data);
                eprintln!("HANG: input ran over {:?}; saved {}", timeout, hangs.join(&name).display());
                std::process::exit(70);
            }
        }
    });
}

pub fn fnv(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

// -- corpus and mutation --------------------------------------------------------------------------

struct Entry {
    bytes: Vec<u8>,
    /// Unzipped parts (name, content) of a zip package.
    parts: Option<Vec<(String, Vec<u8>)>>,
}

fn unzip(bytes: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).ok()?;
        if f.size() > 4 << 20 {
            return None;
        }
        let name = f.name().ok()?.to_string();
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        out.push((name, v));
    }
    Some(out)
}

fn rezip(parts: &[(String, Vec<u8>)], deflate: bool) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let method = if deflate { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
    let opts = zip::write::SimpleFileOptions::default().compression_method(method);
    for (name, data) in parts {
        if w.start_file(name.as_str(), opts).is_err() {
            continue;
        }
        let _ = w.write_all(data);
    }
    w.finish().map(|c| c.into_inner()).unwrap_or_default()
}

const XML_TOKENS: &[&str] = &[
    "<c r=\"XFD1048576\" t=\"s\"><v>0</v></c>",
    "<c r=\"A1\" t=\"s\"><v>4294967295</v></c>",
    "<c r=\"A1\" t=\"s\"><v>-1</v></c>",
    "<c r=\"B2\" t=\"n\"><v>1e308</v></c>",
    "<c r=\"B2\" t=\"d\"><v>9999-99-99T99:99:99</v></c>",
    "<c r=\"C3\" t=\"inlineStr\"><is><t>x</t></is></c>",
    "<c r=\"C3\"><f t=\"shared\" ref=\"C3:C9\" si=\"0\">A1+1</f><v>1</v></c>",
    "<c r=\"C4\"><f t=\"shared\" si=\"0\"/></c>",
    "<c r=\"D4\" s=\"999999\"><v>1</v></c>",
    "<c t=\"e\"><v>#DIV/0!</v></c>",
    "<row r=\"1048577\">",
    "<row r=\"0\">",
    "<row r=\"4294967296\">",
    "<mergeCell ref=\"A1:XFD1048576\"/>",
    "<mergeCell ref=\"B2:A1\"/>",
    "<mergeCells count=\"1\"><mergeCell ref=\"A1:C3\"/></mergeCells>",
    "<dimension ref=\"A1:XFD1048576\"/>",
    "<dimension ref=\"XFD1048576\"/>",
    "<si><t>Total</t></si>",
    "<si><r><t>a</t></r><r><t>b</t></r></si>",
    "<si/>",
    "<xf numFmtId=\"999\" fontId=\"999\" fillId=\"999\" borderId=\"999\" applyNumberFormat=\"1\"/>",
    "<numFmt numFmtId=\"164\" formatCode=\"yyyy-mm-dd;;;@\"/>",
    "<numFmt numFmtId=\"165\" formatCode=\"[$-409]#,##0.00 &quot;€&quot;\"/>",
    "<sheet name=\"S\" sheetId=\"9\" r:id=\"rId99\"/>",
    "<sheet name=\"S&amp;\" sheetId=\"1\" r:id=\"\"/>",
    "<definedName name=\"_xlnm.Print_Area\">S!$A$1:$B$2</definedName>",
    "<workbookPr date1904=\"1\"/>",
    "&amp;", "&#0;", "&#x110000;", "&bogus;", "<![CDATA[x]]>", "<!DOCTYPE x [<!ENTITY a \"aaaa\">]>",
    "<table:table-row table:number-rows-repeated=\"1048576\">",
    "<table:table-cell table:number-columns-repeated=\"16384\" office:value-type=\"float\" office:value=\"1\"/>",
    "<table:table-cell table:number-columns-repeated=\"4294967296\"/>",
    "<table:table-cell office:value-type=\"date\" office:date-value=\"-0001-13-45T25:61:61\"/>",
    "<table:table-cell office:value-type=\"time\" office:time-value=\"PT99999999H\"/>",
    "<table:table-cell table:number-columns-spanned=\"16384\" table:number-rows-spanned=\"1048576\"/>",
    "<table:table-cell office:value-type=\"string\"><text:p>Total<text:s text:c=\"99999999\"/></text:p></table:table-cell>",
    "<table:covered-table-cell/>",
    "</table:table-row>", "</row>", "</c>", "</sheetData>", "</si>", "<", ">", "\"", "=", "/>",
];

const CSV_TOKENS: &[&str] = &[
    "\"", "\"\"", ",", ";", "\t", "|", "\r\n", "\n", "\r", "sep=;\n", "\u{feff}", "\u{fffd}", "=SUM(A1:A9)",
    "1.234,56", "1,234.56", "(1,234)", "-0", "1e999", "NaN", "31.12.2023", "2023-02-30", "Total", "Ukupno",
    "Summe", "%", "€", "\0", "\"a\nb\"", "  ", "#N/A", "TRUE",
];

const NUMBERS: &[&str] = &[
    "0", "1", "-1", "2", "255", "256", "65535", "65536", "1048575", "1048576", "1048577", "16383", "16384",
    "2147483647", "2147483648", "4294967295", "4294967296", "18446744073709551615", "99999999999999999999",
    "1e308", "-1e308", "NaN", "inf", "0.5", "",
];

const REFS: &[&str] = &["A1", "XFD1048576", "XFE1", "A0", "A1048577", "AAAA1", "ZZZZZZZ9", "A4294967296", "1A", "", "$A$1", "A", "B2:A1"];

fn mutate_bytes(rng: &mut Rng, v: &mut Vec<u8>, tokens: &[&str], other: Option<&[u8]>) {
    let n = 1 + rng.below(4);
    for _ in 0..n {
        let len = v.len();
        match rng.below(13) {
            0 if len > 0 => {
                let i = rng.below(len);
                v[i] ^= 1 << rng.below(8);
            }
            1 if len > 0 => {
                let i = rng.below(len);
                v[i] = *rng.pick(&[0u8, 0xff, 0x7f, 0x80, b'<', b'"', b'0', b'9', b'\n']);
            }
            2 => {
                let i = rng.below(len + 1);
                let k = 1 + rng.below(8);
                let bytes: Vec<u8> = (0..k).map(|_| rng.next_u64() as u8).collect();
                v.splice(i..i, bytes);
            }
            3 if len > 0 => {
                let i = rng.below(len);
                let k = 1 + rng.below((len - i).min(64));
                v.drain(i..i + k);
            }
            4 if len > 0 => {
                // duplicate a chunk
                let i = rng.below(len);
                let k = 1 + rng.below((len - i).min(256));
                let chunk = v[i..i + k].to_vec();
                let j = rng.below(len + 1);
                v.splice(j..j, chunk);
            }
            5..=7 => {
                let i = rng.below(len + 1);
                v.splice(i..i, rng.pick(tokens).bytes());
            }
            8 => {
                // replace a run of digits by an interesting number
                if let Some((s, e)) = find_digits(rng, v) {
                    v.splice(s..e, rng.pick(NUMBERS).bytes());
                }
            }
            9 => {
                // replace the value of a cell reference attribute
                if let Some((s, e)) = find_attr(rng, v, &[b"r=\"", b"ref=\"", b"sqref=\""]) {
                    v.splice(s..e, rng.pick(REFS).bytes());
                }
            }
            10 if len > 1 => {
                // truncate
                v.truncate(rng.below(len));
            }
            11 if len > 0 => {
                // splice from another input
                if let Some(o) = other.filter(|o| !o.is_empty()) {
                    let i = rng.below(o.len());
                    let k = 1 + rng.below((o.len() - i).min(512));
                    let j = rng.below(len + 1);
                    v.splice(j..j, o[i..i + k].iter().copied());
                }
            }
            12 if len > 0 => {
                // repeat a chunk many times (size / repetition stress)
                let i = rng.below(len);
                let k = 1 + rng.below((len - i).min(64));
                let chunk = v[i..i + k].to_vec();
                let times = 2 + rng.below(200);
                let j = rng.below(len + 1);
                let rep: Vec<u8> = chunk.iter().copied().cycle().take(chunk.len() * times).collect();
                v.splice(j..j, rep);
            }
            _ => {}
        }
    }
}

fn find_digits(rng: &mut Rng, v: &[u8]) -> Option<(usize, usize)> {
    if v.is_empty() {
        return None;
    }
    let start = rng.below(v.len());
    let i = (start..v.len()).chain(0..start).find(|&i| v[i].is_ascii_digit())?;
    let mut s = i;
    while s > 0 && v[s - 1].is_ascii_digit() {
        s -= 1;
    }
    let mut e = i;
    while e < v.len() && v[e].is_ascii_digit() {
        e += 1;
    }
    Some((s, e))
}

fn find_attr(rng: &mut Rng, v: &[u8], keys: &[&[u8]]) -> Option<(usize, usize)> {
    let key = *rng.pick(keys);
    let hits: Vec<usize> = v.windows(key.len()).enumerate().filter(|(_, w)| *w == key).map(|(i, _)| i + key.len()).take(4096).collect();
    if hits.is_empty() {
        return None;
    }
    let s = *rng.pick(&hits);
    let e = s + v[s..].iter().position(|&b| b == b'"')?;
    Some((s, e))
}

/// One mutated input from the corpus.
fn mutant(rng: &mut Rng, target: Target, corpus: &[Entry]) -> Vec<u8> {
    let e = &corpus[rng.below(corpus.len())];
    let other = &corpus[rng.below(corpus.len())].bytes;
    match (&e.parts, target) {
        (_, Target::Normalize) | (_, Target::Csv) => {
            let mut v = e.bytes.clone();
            let toks = if target == Target::Csv { CSV_TOKENS } else { &["\0", "\u{1}", "\u{ff}"][..] };
            mutate_bytes(rng, &mut v, toks, Some(other));
            v.truncate(1 << 20);
            v
        }
        (Some(parts), _) if !rng.chance(1, 10) => {
            let mut parts = parts.clone();
            match rng.below(20) {
                0 if parts.len() > 1 => {
                    let i = rng.below(parts.len());
                    parts.remove(i);
                }
                1 => {
                    let i = rng.below(parts.len().max(1));
                    if let Some(p) = parts.get(i).cloned() {
                        parts.push(p);
                    }
                }
                _ => {
                    // Mostly the XML parts that matter.
                    let weights: Vec<usize> = parts
                        .iter()
                        .map(|(n, _)| {
                            if n.contains("sheet") || n.contains("content.xml") || n.contains("sharedStrings") || n.contains("styles") {
                                8
                            } else if n.ends_with(".xml") || n.ends_with(".rels") {
                                2
                            } else {
                                1
                            }
                        })
                        .collect();
                    let total: usize = weights.iter().sum();
                    let mut r = rng.below(total.max(1));
                    let mut i = 0;
                    while i + 1 < weights.len() && r >= weights[i] {
                        r -= weights[i];
                        i += 1;
                    }
                    if let Some((_, data)) = parts.get_mut(i) {
                        mutate_bytes(rng, data, XML_TOKENS, Some(other));
                        data.truncate(8 << 20);
                    }
                }
            }
            rezip(&parts, rng.chance(1, 4))
        }
        _ => {
            let mut v = e.bytes.clone();
            mutate_bytes(rng, &mut v, XML_TOKENS, Some(other));
            v
        }
    }
}

fn load_corpus(dir: &Path, target: Target) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    paths.sort();
    for p in paths {
        let Ok(bytes) = std::fs::read(&p) else { continue };
        if bytes.len() > 1 << 20 {
            continue;
        }
        let parts = if target.zipped() { unzip(&bytes) } else { None };
        out.push(Entry { bytes, parts });
    }
    out
}

// -- commands ---------------------------------------------------------------------------------------

fn fuzz(args: &[String]) -> Result<(), String> {
    let target = args.first().and_then(|t| Target::parse(t)).ok_or("fuzz <csv|xlsx|xls|ods|normalize> ...")?;
    let mut corpus_dir = None;
    let mut out = PathBuf::from("fuzz-out");
    let mut seconds = 60u64;
    let mut timeout = 10u64;
    let mut rss_mb = 1500usize;
    let mut seed = 0u64;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--corpus" => corpus_dir = Some(PathBuf::from(val()?)),
            "--out" => out = PathBuf::from(val()?),
            "--seconds" => seconds = val()?.parse().map_err(|_| "--seconds: number")?,
            "--timeout" => timeout = val()?.parse().map_err(|_| "--timeout: number")?,
            "--rss-mb" => rss_mb = val()?.parse().map_err(|_| "--rss-mb: number")?,
            "--seed" => seed = val()?.parse().map_err(|_| "--seed: number")?,
            _ => return Err(format!("unknown argument {a}")),
        }
    }
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let crashes = out.join("crashes");
    let hangs = out.join("hangs");
    let mut corpus = match &corpus_dir {
        Some(d) => load_corpus(d, target),
        None => Vec::new(),
    };
    if corpus.is_empty() {
        corpus.push(Entry { bytes: b"a,b\n1,2\n".to_vec(), parts: None });
    }
    let n_seeds = corpus.len();
    LIMIT.store(rss_mb << 20, Ordering::Relaxed);
    install_panic_hook();
    QUIET.store(true, Ordering::Relaxed);
    let cur = out.join(format!("cur.{}", target.ext()));
    let t0 = Instant::now();
    watchdog(t0, Duration::from_secs(timeout), cur.clone(), hangs);
    if seed == 0 {
        seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    }
    let mut rng = Rng(seed);
    let mut seen: HashSet<String> = HashSet::new();
    let mut panics: HashSet<String> = HashSet::new();
    let deadline = Duration::from_secs(seconds);
    let mut execs = 0u64;
    let mut last_report = Instant::now();
    let saved = Arc::new(AtomicUsize::new(0));
    eprintln!("hh_fuzz {target:?}: {n_seeds} seeds, seed {seed}, {seconds} s, timeout {timeout} s, heap limit {rss_mb} MB");
    // Seeds first, unmutated.
    let mut queue: Vec<Vec<u8>> = corpus.iter().map(|e| e.bytes.clone()).collect();
    while t0.elapsed() < deadline {
        let data = queue.pop().unwrap_or_else(|| mutant(&mut rng, target, &corpus));
        if std::fs::write(&cur, &data).is_err() {
            return Err(format!("cannot write {}", cur.display()));
        }
        EXEC_START.store(t0.elapsed().as_millis() as u64 + 1, Ordering::Relaxed);
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| exec(target, &cur, &data)));
        EXEC_START.store(0, Ordering::Relaxed);
        execs += 1;
        match r {
            Ok(sig) => {
                if seen.insert(sig) && corpus.len() < 4000 {
                    let parts = if target.zipped() { unzip(&data) } else { None };
                    corpus.push(Entry { bytes: data, parts });
                }
            }
            Err(_) => {
                let (loc, msg) = PANIC_INFO.lock().ok().and_then(|mut p| p.take()).unwrap_or_default();
                if panics.insert(loc.clone()) {
                    let _ = std::fs::create_dir_all(&crashes);
                    let name = format!("panic-{:016x}.{}", fnv(loc.as_bytes()), target.ext());
                    let _ = std::fs::write(crashes.join(&name), &data);
                    let _ = std::fs::write(crashes.join(format!("{name}.txt")), format!("{loc}\n{msg}\n"));
                    saved.fetch_add(1, Ordering::Relaxed);
                    eprintln!("PANIC #{} at {loc}: {msg} → {}", panics.len(), crashes.join(&name).display());
                }
            }
        }
        if last_report.elapsed() > Duration::from_secs(60) {
            last_report = Instant::now();
            eprintln!(
                "  {:>5} s · {execs} execs ({:.0}/s) · corpus {} · outcomes {} · panics {}",
                t0.elapsed().as_secs(),
                execs as f64 / t0.elapsed().as_secs_f64(),
                corpus.len(),
                seen.len(),
                panics.len()
            );
        }
    }
    eprintln!("done: {execs} execs in {:.0} s, {} outcomes, {} panic locations", t0.elapsed().as_secs_f64(), seen.len(), panics.len());
    Ok(())
}

fn run(files: &[String]) -> Result<(), String> {
    install_panic_hook();
    let mb = std::env::var("HH_FUZZ_RSS_MB").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(1500);
    LIMIT.store(mb << 20, Ordering::Relaxed);
    OOM_BACKTRACE.store(true, Ordering::Relaxed);
    for f in files {
        let p = Path::new(f);
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let target = Target::parse(&ext).unwrap_or(if ext == "grid" { Target::Normalize } else { Target::Xlsx });
        let data = std::fs::read(p).map_err(|e| format!("{f}: {e}"))?;
        let t = Instant::now();
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| exec(target, p, &data)));
        match r {
            Ok(sig) => println!("{f}: {sig} ({:.2} s)", t.elapsed().as_secs_f64()),
            Err(_) => println!("{f}: PANIC ({:.2} s)", t.elapsed().as_secs_f64()),
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("fuzz") => fuzz(&args[1..]),
        Some("run") => run(&args[1..]),
        Some("seeds") => args.get(1).ok_or("seeds <dir>".to_string()).and_then(|d| seeds::write(Path::new(d))),
        _ => Err("usage: hh_fuzz fuzz <target> --corpus DIR --out DIR [--seconds N] [--timeout S] [--rss-mb N] | run <file>... | seeds <dir>".into()),
    };
    if let Err(e) = r {
        eprintln!("{e}");
        std::process::exit(2);
    }
}
