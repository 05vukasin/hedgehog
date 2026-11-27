//! Reads xlsx / xlsm / xlsb / xls / ods into the [`Workbook`] model via calamine, and CSV / TSV
//! with the reader in `reader/csv.rs`.
//! Calamine gives values, formulas and merges but no styles; for xlsx / xlsm the cell formats
//! come from our own scan of `xl/styles.xml` and the sheet XML (`reader/styles.rs`).

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, Write};
use std::path::Path;

use calamine::{
    Data, DataRef, Dimensions, Range, Reader, Sheets, Xlsx, XlsxError, open_workbook_auto_from_rs,
};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::model::{Cell, Rect, Sheet, Value, Workbook};

mod csv;
mod limits;
mod merges;
mod styles;

pub use limits::{
    DEFAULT_MAX_CELLS, DEFAULT_MAX_DECOMPRESSED_BYTES, DEFAULT_MAX_SHARED_STRINGS, Limits,
};

/// A read failure: one line of text. [`Error::is_limit`] tells a refused file (over a [`Limits`]
/// value) from a broken or unsupported one.
#[derive(Debug)]
pub struct Error(String);

impl Error {
    fn limit(msg: impl Into<String>) -> Error {
        Error(format!("{LIMIT_PREFIX}{}", msg.into()))
    }

    fn from_io(e: std::io::Error) -> Error {
        Error(e.to_string())
    }

    /// The file was refused because it is over one of the [`Limits`].
    pub fn is_limit(&self) -> bool {
        self.0.starts_with(LIMIT_PREFIX)
    }
}

const LIMIT_PREFIX: &str = "file refused: ";

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// A calamine error as one line (password errors get the clear password message).
fn err(e: impl fmt::Display) -> Error {
    let msg = e.to_string();
    if let Some(p) = limits::password_error(&msg) {
        return p;
    }
    let line = msg.split_whitespace().collect::<Vec<_>>().join(" ");
    Error(if line.is_empty() {
        "unreadable file".into()
    } else {
        line
    })
}

/// Where a workbook's bytes come from: a file, read where it lies (never copied into memory as a
/// whole), or a buffer already in memory ([`open_bytes_with`]). Every step of [`read`] takes its
/// own view of the content from the start, so both sources go through exactly the same checks
/// and readers.
trait Source {
    /// A plain view: format sniff, limit checks, the merge scanner.
    type Raw: Read + Seek + Send;
    /// A buffered view for calamine (what its `open_workbook` uses for a path).
    type Buf: Read + Seek + Send;
    fn raw(&self) -> std::io::Result<Self::Raw>;
    fn buffered(&self) -> std::io::Result<Self::Buf>;
    /// Size in bytes; `None` when unknown.
    fn size(&self) -> Option<u64>;
    /// The file name: its extension is a format hint, its stem names a CSV sheet.
    fn name(&self) -> Option<&Path>;
    /// Reads the content as delimited text.
    fn csv(&self, max_cells: u64) -> Result<Workbook, Error>;

    /// The extension, lowercase.
    fn extension(&self) -> Option<String> {
        self.name()?
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
    }
}

struct FileSource<'a>(&'a Path);

impl Source for FileSource<'_> {
    type Raw = File;
    type Buf = BufReader<File>;
    fn raw(&self) -> std::io::Result<File> {
        File::open(self.0)
    }
    fn buffered(&self) -> std::io::Result<BufReader<File>> {
        File::open(self.0).map(BufReader::new)
    }
    fn size(&self) -> Option<u64> {
        std::fs::metadata(self.0).ok().map(|m| m.len())
    }
    fn name(&self) -> Option<&Path> {
        Some(self.0)
    }
    fn csv(&self, max_cells: u64) -> Result<Workbook, Error> {
        csv::open(self.0, max_cells)
    }
}

struct BytesSource<'a> {
    bytes: &'a [u8],
    name: Option<&'a Path>,
}

impl<'a> Source for BytesSource<'a> {
    type Raw = Cursor<&'a [u8]>;
    type Buf = Cursor<&'a [u8]>;
    fn raw(&self) -> std::io::Result<Self::Raw> {
        Ok(Cursor::new(self.bytes))
    }
    fn buffered(&self) -> std::io::Result<Self::Buf> {
        Ok(Cursor::new(self.bytes))
    }
    fn size(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }
    fn name(&self) -> Option<&Path> {
        self.name
    }
    fn csv(&self, max_cells: u64) -> Result<Workbook, Error> {
        csv::open_bytes(self.bytes, self.name, max_cells)
    }
}

/// File format from the file's content, not its name (`.xls` files that are really xlsx are
/// common in open-data portals). `None`: unknown, let calamine go by the extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Xls,
    Xlsx,
    Xlsb,
    Ods,
}

/// The first (up to) four bytes.
fn head(src: &impl Source) -> std::io::Result<Vec<u8>> {
    let mut magic = Vec::with_capacity(4);
    src.raw()?.take(4).read_to_end(&mut magic)?;
    Ok(magic)
}

fn sniff(src: &impl Source) -> Option<Format> {
    match magic(src)? {
        Magic::Ole => Some(Format::Xls),
        Magic::Zip => {
            let mut zip = zip::ZipArchive::new(src.buffered().ok()?).ok()?;
            // ODF: a `mimetype` entry (normally stored, sometimes deflated) names the type.
            if let Ok(mut m) = zip.by_name("mimetype") {
                let mut t = String::new();
                m.by_ref().take(256).read_to_string(&mut t).ok()?;
                return t
                    .trim()
                    .starts_with("application/vnd.oasis.opendocument.spreadsheet")
                    .then_some(Format::Ods);
            }
            if zip.index_for_name("xl/workbook.bin").is_some() {
                return Some(Format::Xlsb);
            }
            if zip.index_for_name("xl/workbook.xml").is_some() {
                return Some(Format::Xlsx);
            }
            let mut ct = String::new();
            zip.by_name("[Content_Types].xml")
                .ok()?
                .by_ref()
                .take(1 << 20)
                .read_to_string(&mut ct)
                .ok()?;
            if ct.contains("sheet.binary.macroEnabled.main") {
                Some(Format::Xlsb)
            } else if ct.contains("spreadsheetml") || ct.contains("ms-excel") {
                Some(Format::Xlsx)
            } else {
                None
            }
        }
    }
}

/// CSV / TSV: any name but a workbook extension (`.csv`, `.tsv`, `.txt`, `.asp`, none, ...) with
/// content that is not a zip or OLE container (a workbook saved as `.csv` opens as the workbook
/// it is). Unreadable files go to calamine, which reports the error.
fn is_delimited_text(src: &impl Source) -> bool {
    if matches!(
        src.extension().as_deref(),
        Some("xls" | "xla" | "xlsx" | "xlsm" | "xlam" | "xlsb" | "ods")
    ) {
        return false;
    }
    match head(src) {
        Ok(magic) => !matches!(magic[..], [0xD0, 0xCF, 0x11, 0xE0] | [b'P', b'K', 3, 4]),
        Err(_) => false,
    }
}

/// Opens the workbook with the reader its content calls for (extension as a fallback; neither:
/// each reader in turn, like calamine's `open_workbook_auto` for an unknown extension).
fn open_sheets<S: Source>(src: &S) -> Result<Sheets<S::Buf>, calamine::Error> {
    use calamine::{Error as E, Ods, Xls, Xlsb};
    fn new<R: Reader<B>, B: Read + Seek>(view: std::io::Result<B>) -> Result<R, R::Error> {
        R::new(view?)
    }
    let by_ext = match src.extension().as_deref() {
        Some("xls" | "xla") => Some(Format::Xls),
        Some("xlsx" | "xlsm" | "xlam") => Some(Format::Xlsx),
        Some("xlsb") => Some(Format::Xlsb),
        Some("ods") => Some(Format::Ods),
        _ => None,
    };
    match sniff(src).or(by_ext) {
        Some(Format::Xls) => Ok(Sheets::Xls(new(src.buffered()).map_err(E::Xls)?)),
        Some(Format::Xlsx) => Ok(Sheets::Xlsx(new(src.buffered()).map_err(E::Xlsx)?)),
        Some(Format::Xlsb) => Ok(Sheets::Xlsb(new(src.buffered()).map_err(E::Xlsb)?)),
        Some(Format::Ods) => Ok(Sheets::Ods(new(src.buffered()).map_err(E::Ods)?)),
        None => {
            if let Ok(x) = new::<Xls<_>, _>(src.buffered()) {
                Ok(Sheets::Xls(x))
            } else if let Ok(x) = new::<Xlsx<_>, _>(src.buffered()) {
                Ok(Sheets::Xlsx(x))
            } else if let Ok(x) = new::<Xlsb<_>, _>(src.buffered()) {
                Ok(Sheets::Xlsb(x))
            } else if let Ok(x) = new::<Ods<_>, _>(src.buffered()) {
                Ok(Sheets::Ods(x))
            } else {
                Err(E::Msg("Cannot detect file format"))
            }
        }
    }
}

/// What [`open_with`] reads besides values, formulas and merged areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadOptions {
    /// Cell formats (`Sheet::formats`, xlsx / xlsm only). Costs an extra look at every cell tag
    /// (about 15–35% more read time) and 4 bytes per cell. Files up to [`FORMATS_MAX_BYTES`] get
    /// them anyway (the indent of row labels, `<label>_parent` in normalize).
    pub formats: bool,
    /// Size, cell and time limits for untrusted files.
    pub limits: Limits,
    /// The most threads a large sheet, shared string table or ODS `content.xml` is read on;
    /// `Some(1)` (or `Some(0)`) reads everything on the calling thread. None: up to 4 (fewer on
    /// fewer CPUs). Hosts that run several files at once pass their per-file budget. The
    /// workbook is the same for every thread count.
    pub threads: Option<usize>,
}

impl Default for ReadOptions {
    /// Formats only when the embedded learned model reads them; [`Limits::default`].
    fn default() -> Self {
        ReadOptions {
            formats: crate::learned::embedded().is_some_and(|m| m.reads_formats()),
            limits: Limits::default(),
            threads: None,
        }
    }
}

/// Opens a workbook with [`ReadOptions::default`].
pub fn open(path: impl AsRef<Path>) -> Result<Workbook, Error> {
    open_with(path, ReadOptions::default())
}

/// Files up to this size are read with their cell formats whatever [`ReadOptions::formats`] says.
pub const FORMATS_MAX_BYTES: u64 = 2 << 20;

/// Opens a workbook file. The format comes from the content (zip entries, the OLE signature),
/// the extension only when the content does not tell; any other name with content that is no
/// zip or OLE container reads as CSV / TSV.
pub fn open_with(path: impl AsRef<Path>, opts: ReadOptions) -> Result<Workbook, Error> {
    read(&FileSource(path.as_ref()), opts)
}

/// [`open_bytes_with`] with [`ReadOptions::default`].
pub fn open_bytes(bytes: &[u8], name: Option<&str>) -> Result<Workbook, Error> {
    open_bytes_with(bytes, name, ReadOptions::default())
}

/// Opens a workbook held in memory (an object streamed from storage, an upload), exactly as
/// [`open_with`] opens a file with these bytes: the same format detection, [`Limits`] and
/// readers, and the same result.
///
/// `name` is the file name or object key it came from, if known (`"reports/q3.xlsx"`). Like a
/// path's, its extension only matters when the content does not tell the format: content that
/// is no zip or OLE container reads as CSV / TSV unless the name has a workbook extension
/// (`.tsv` means tab-separated), and a zip or OLE container is read as what its content is,
/// whatever the name says. The CSV sheet is named after the name's stem, `Sheet1` without one.
/// [`FORMATS_MAX_BYTES`] applies to `bytes.len()`.
pub fn open_bytes_with(
    bytes: &[u8],
    name: Option<&str>,
    opts: ReadOptions,
) -> Result<Workbook, Error> {
    read(
        &BytesSource {
            bytes,
            name: name.map(Path::new),
        },
        opts,
    )
}

/// [`open_with`] and [`open_bytes_with`], with the parallel reads on at most
/// [`ReadOptions::threads`] threads.
fn read<S: Source>(src: &S, opts: ReadOptions) -> Result<Workbook, Error> {
    calamine::with_parallel_read_threads(opts.threads, || read_inner(src, opts))
}

/// Gives memory back to the system ([`crate::normalize::release_free_memory`]) when a parallel
/// read started since `started` ([`calamine::parallel_reads_started`]): its worker threads freed
/// it in their own allocator arenas. A one-thread read skips the trim, which walks and locks
/// every arena of the process.
fn release_after_parallel(started: usize) {
    if calamine::parallel_reads_started() != started {
        let t = std::time::Instant::now();
        crate::normalize::release_free_memory();
        trace("read.trim", t);
    }
}

fn read_inner<S: Source>(src: &S, opts: ReadOptions) -> Result<Workbook, Error> {
    let small = || src.size().is_some_and(|n| n <= FORMATS_MAX_BYTES);
    let opts = ReadOptions {
        formats: opts.formats || small(),
        ..opts
    };
    let limits = &opts.limits;
    if is_delimited_text(src) {
        let t = std::time::Instant::now();
        let wb = src.csv(limits.max_cells)?;
        trace("read.csv", t);
        limits.check_cells(wb.sheets.iter().map(|s| s.cells.len() as u64).sum())?;
        return Ok(wb);
    }
    let t = std::time::Instant::now();
    match magic(src) {
        Some(Magic::Ole) => limits::check_ole(src.raw().map_err(Error::from_io)?)?,
        Some(Magic::Zip) => limits::check_zip(src.buffered().map_err(Error::from_io)?, limits)?,
        None => {}
    }
    trace("read.limits", t);
    let t = std::time::Instant::now();
    let started = calamine::parallel_reads_started();
    let opened = open_sheets(src);
    // ODS content and large shared string tables are parsed on several threads: what they
    // freed goes back to the system before the sheets are read.
    release_after_parallel(started);
    trace("read.workbook", t);
    match opened {
        Ok(wb) => {
            let scan = || merges::Scanner::new(src.raw().ok()?, opts.formats);
            read_all(wb, scan, limits)
        }
        // Some converted workbooks list a veryHidden sheet with `r:id=""` (a leftover VBA module
        // sheet); calamine rejects the whole file. Drop those entries and retry from memory.
        Err(e) if e.to_string().contains("Relationship not found") => {
            let repaired = src
                .raw()
                .map_err(Into::into)
                .and_then(repair_unlinked_sheets)
                .map_err(|_| err(e))?;
            let wb = open_workbook_auto_from_rs(Cursor::new(repaired.as_slice())).map_err(err)?;
            read_all(
                wb,
                || merges::Scanner::new(Cursor::new(repaired.as_slice()), opts.formats),
                limits,
            )
        }
        Err(e) => Err(err(e)),
    }
}

enum Magic {
    Ole,
    Zip,
}

fn magic(src: &impl Source) -> Option<Magic> {
    match head(src).ok()?[..] {
        [0xD0, 0xCF, 0x11, 0xE0] => Some(Magic::Ole),
        [b'P', b'K', 3, 4] => Some(Magic::Zip),
        _ => None,
    }
}

/// (sheets whose merged areas came from the byte scan, sheets that fell back to calamine) since
/// the process started — for tests and benchmarks.
#[doc(hidden)]
pub fn merge_scan_stats() -> (usize, usize) {
    use std::sync::atomic::Ordering::Relaxed;
    (
        merges::SCANNED.load(Relaxed),
        merges::FALLBACK.load(Relaxed),
    )
}

fn trace(step: &str, since: std::time::Instant) {
    crate::normalize::trace(step, since);
}

/// `scanner` opens a second, lightweight view of the same xlsx package for [`merges::Scanner`];
/// it is only called for xlsx files.
fn read_all<RS: Read + Seek, R: Read + Seek + Send>(
    mut wb: Sheets<RS>,
    scanner: impl FnOnce() -> Option<merges::Scanner<R>>,
    limits: &Limits,
) -> Result<Workbook, Error> {
    let names = wb.sheet_names();
    let sheet_names = names.clone();
    let mut sheets = Vec::with_capacity(names.len());
    let mut scanner = match &wb {
        Sheets::Xlsx(_) => scanner(),
        _ => None,
    };
    // Cells read so far, against `limits.max_cells`.
    let mut total: u64 = 0;
    let t = std::time::Instant::now();
    for name in names {
        limits.check_time()?;
        // xls / xlsb: sparse cells too; their dense ranges (calamine's `worksheet_range`) span
        // the bounding box, gigabytes for two cells far apart.
        let sparse = match &mut wb {
            Sheets::Xls(x) => Some(xls_cells(x, &name).map_err(err)?),
            Sheets::Xlsb(x) => Some(xlsb_cells(x, &name, limits, total)?),
            _ => None,
        };
        if let Some(cells) = sparse {
            total += cells.len() as u64;
            limits.check_cells(total)?;
            let merges = merge_cells(&mut wb, &name);
            sheets.push(Sheet {
                name,
                cells,
                merges,
                formats: Vec::new(),
            });
            continue;
        }
        // xlsx: values and formulas in one streaming pass, straight into sparse cells. When the
        // merge scan needs no cells (no formats), it reads its own view of the part meanwhile.
        let read_cells = |wb: &mut Sheets<RS>| -> Result<Option<(Vec<Cell>, Dimensions)>, Error> {
            Ok(match wb {
                Sheets::Xlsx(x) => match read_xlsx_parallel(x, &name, limits, total)? {
                    Some(c) => Some(c),
                    None => match read_xlsx_cells(x, &name, limits, total) {
                        Ok(c) => Some(c),
                        Err(ReadErr::Limit(e)) => return Err(e),
                        Err(ReadErr::Xlsx) => None,
                    },
                },
                _ => None,
            })
        };
        let (cells, early_scan) = match scanner.as_mut() {
            Some(s) if !s.needs_cells() && matches!(wb, Sheets::Xlsx(_)) => {
                std::thread::scope(|sc| {
                    let scan = sc.spawn(|| s.scan_merges(&name));
                    let cells = read_cells(&mut wb);
                    let scanned = scan.join().unwrap_or_else(|e| std::panic::resume_unwind(e));
                    (cells, Some(scanned))
                })
            }
            _ => (read_cells(&mut wb), None),
        };
        let cells = cells?;
        let sheet = match cells {
            Some((cells, dims)) => {
                // Merged areas sit after the cells; a byte scan finds them (and every cell's
                // format) much faster than a second full XML parse. Anything unexpected →
                // calamine's parse of the merges, and no formats.
                let scanned = match early_scan {
                    Some(early) => early
                        .filter(|(declared, _)| *declared == dims)
                        .map(|(_, m)| (m, None)),
                    None => scanner.as_mut().and_then(|s| s.scan(&name, dims, &cells)),
                };
                let (merges, formats) = match scanned {
                    Some((m, f)) => {
                        merges::SCANNED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        (m, f.unwrap_or_default())
                    }
                    None => {
                        merges::FALLBACK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        (merge_cells(&mut wb, &name), Vec::new())
                    }
                };
                Sheet {
                    name,
                    cells,
                    merges,
                    formats,
                }
            }
            None if matches!(wb, Sheets::Xlsx(_)) => {
                // An xlsx part the one-pass reader rejected: values and formulas in two sparse
                // passes (calamine's dense range spans the cells' bounding box: 38 GB for a
                // hostile sheet), no formats.
                let Sheets::Xlsx(x) = &mut wb else {
                    unreachable!()
                };
                let cells = xlsx_cells_fallback(x, &name, limits, total)?;
                let merges = merge_cells(&mut wb, &name);
                Sheet {
                    name,
                    cells,
                    merges,
                    formats: Vec::new(),
                }
            }
            None => {
                // ods: its cell count was checked before calamine parsed it. A sheet name that
                // occurs once is moved out of the reader rather than copied.
                let taken = match &mut wb {
                    Sheets::Ods(o) if sheet_names.iter().filter(|n| **n == name).count() == 1 => {
                        o.take_sheet(&name)
                    }
                    _ => None,
                };
                let (values, formulas) = match taken {
                    Some(t) => t,
                    None => {
                        let values = wb.worksheet_range(&name).map_err(err)?;
                        // Some formats can't report formulas; values alone are still useful.
                        (values, wb.worksheet_formula(&name).unwrap_or_default())
                    }
                };
                let merges = merge_cells(&mut wb, &name);
                build_sheet(name, &values, &formulas, merges)
            }
        };
        total += sheet.cells.len() as u64;
        limits.check_cells(total)?;
        sheets.push(sheet);
    }
    trace("read.sheets", t);
    for s in &mut sheets {
        ordered_corners(&mut s.merges);
    }
    Ok(Workbook { sheets })
}

/// One xlsx cell record: position, converted value (`None` = blank text), whether the record had
/// a non-empty value at all, and the formula.
struct Record {
    row: u32,
    col: u32,
    value: Option<Value>,
    has_value: bool,
    formula: Option<String>,
}

/// Reads values and formulas of an xlsx sheet in a single pass over its XML, producing exactly the
/// cells [`build_sheet`] makes from `worksheet_range` + `worksheet_formula` — without their two
/// passes and their dense `rows × cols` grids (which explode on a sheet using A1 and XFD1048576).
/// Any error (also a non-worksheet part) → `Err`, and the caller takes the generic path.
/// Also returns the sheet's declared `<dimension>`, which the merge scanner uses to check that it
/// found the same sheet part.
fn read_xlsx_cells<RS: Read + Seek>(
    x: &mut Xlsx<RS>,
    name: &str,
    limits: &Limits,
    before: u64,
) -> Result<(Vec<Cell>, Dimensions), ReadErr> {
    let mut reader = x.worksheet_cells_reader(name)?;
    let dims = reader.dimensions();
    let mut recs: Vec<Record> = Vec::new();
    let mut seen: u64 = 0;
    while let Some(rec) = reader.next_cell_with_formula()? {
        seen += 1;
        if seen.is_multiple_of(65_536) {
            limits.check_time().map_err(ReadErr::Limit)?;
            limits
                .check_cells(before + recs.len() as u64)
                .map_err(ReadErr::Limit)?;
        }
        if let Some(r) = record(rec) {
            recs.push(r);
        }
    }
    drop(reader);
    Ok((cells_of_records(recs), dims))
}

/// Sheet parts of at least this many bytes (uncompressed) are read on several threads.
const PARALLEL_MIN_PART: u64 = 2 << 20;

/// [`read_xlsx_cells`] for a large sheet, on up to four threads (at most
/// [`ReadOptions::threads`]; calamine's `worksheet_cells_parallel`: the same records, in
/// chunks). `Ok(None)`: not read this way, read it with [`read_xlsx_cells`] (also a sheet whose
/// records are not in row order, which that sorts and folds).
fn read_xlsx_parallel<RS: Read + Seek>(
    x: &mut Xlsx<RS>,
    name: &str,
    limits: &Limits,
    before: u64,
) -> Result<Option<(Vec<Cell>, Dimensions)>, Error> {
    let threads = calamine::parallel_read_threads();
    // A record of blank text stays a cell without value or formula until the order is known.
    let cell = |rec: calamine::XlsxCellFormula<'_>| {
        record(rec).map(|r| Cell {
            row: r.row,
            col: r.col,
            value: r.value,
            formula: r.formula,
        })
    };
    let mut cells: Vec<Cell> = Vec::new();
    let (mut kept, mut ordered, mut prev) = (0u64, true, None);
    // Each chunk is copied as soon as its turn comes: the chunks' memory is reused by the next
    // chunks rather than held until the end.
    let mut sink = |chunk: Vec<Cell>| {
        kept += chunk.len() as u64;
        limits.check_time()?;
        limits.check_cells(before + kept)?;
        for c in chunk {
            ordered &= prev.is_none_or(|p| p < (c.row, c.col));
            prev = Some((c.row, c.col));
            if ordered && (c.value.is_some() || c.formula.is_some()) {
                cells.push(c);
            }
        }
        Ok(())
    };
    let started = calamine::parallel_reads_started();
    let read = x.worksheet_cells_parallel(name, threads, PARALLEL_MIN_PART, cell, &mut sink);
    // What the worker threads freed goes back to the system before the next sheet.
    release_after_parallel(started);
    let Some(dims) = read? else {
        return Ok(None);
    };
    if !ordered {
        return Ok(None);
    }
    Ok(Some((cells, dims)))
}

/// One record of `next_cell_with_formula` (`None`: neither a value nor a formula).
fn record(rec: calamine::XlsxCellFormula<'_>) -> Option<Record> {
    let has_value = !matches!(rec.value, DataRef::Empty);
    let formula = rec
        .formula
        .filter(|f| !f.is_empty())
        .map(|f| match f.strip_prefix('=') {
            Some(_) => f.trim_start_matches('=').to_string(),
            None => f,
        });
    if !has_value && formula.is_none() {
        return None;
    }
    let value = if has_value {
        convert_ref(rec.value)
    } else {
        None
    };
    Some(Record {
        row: rec.pos.0,
        col: rec.pos.1,
        value,
        has_value,
        formula,
    })
}

impl Record {
    /// The cell, unless the record holds only blank text.
    fn into_cell(self) -> Option<Cell> {
        (self.value.is_some() || self.formula.is_some()).then_some(Cell {
            row: self.row,
            col: self.col,
            value: self.value,
            formula: self.formula,
        })
    }
}

/// The cells of a sheet's records in file order, as the dense ranges would hold them.
fn cells_of_records(mut recs: Vec<Record>) -> Vec<Cell> {
    // Usually already in order; otherwise sort (stable) and fold duplicates like the dense ranges
    // would: the last non-empty value and the last non-empty formula of a position win.
    if !recs
        .windows(2)
        .all(|w| (w[0].row, w[0].col) < (w[1].row, w[1].col))
    {
        recs.sort_by_key(|r| (r.row, r.col));
        let mut out: Vec<Record> = Vec::with_capacity(recs.len());
        for r in recs {
            match out.last_mut() {
                Some(last) if (last.row, last.col) == (r.row, r.col) => {
                    if r.has_value {
                        last.value = r.value;
                        last.has_value = true;
                    }
                    if r.formula.is_some() {
                        last.formula = r.formula;
                    }
                }
                _ => out.push(r),
            }
        }
        recs = out;
    }
    recs.into_iter().filter_map(Record::into_cell).collect()
}

/// Why the streaming xlsx read failed: over a limit (final), or a calamine error (the caller
/// falls back to calamine's own path, which reports it).
enum ReadErr {
    Limit(Error),
    Xlsx,
}

impl From<XlsxError> for ReadErr {
    fn from(_: XlsxError) -> Self {
        ReadErr::Xlsx
    }
}

/// An xls sheet's cells from calamine's sparse records (the last record of a position wins, as
/// in its dense range).
fn xls_cells<RS: Read + Seek>(
    x: &calamine::Xls<RS>,
    name: &str,
) -> Result<Vec<Cell>, calamine::XlsError> {
    let (values, formulas) = x.worksheet_cells(name)?;
    let mut cells: BTreeMap<(u32, u32), Cell> = BTreeMap::new();
    for c in values {
        let (row, col) = c.get_position();
        match convert(c.get_value()) {
            Some(value) => {
                cells.insert(
                    (row, col),
                    Cell {
                        row,
                        col,
                        value: Some(value),
                        formula: None,
                    },
                );
            }
            None => {
                cells.remove(&(row, col));
            }
        }
    }
    let mut last: BTreeMap<(u32, u32), &String> = BTreeMap::new();
    for f in formulas {
        last.insert(f.get_position(), f.get_value());
    }
    for ((row, col), f) in last {
        if f.is_empty() {
            continue;
        }
        cells
            .entry((row, col))
            .or_insert(Cell {
                row,
                col,
                value: None,
                formula: None,
            })
            .formula = Some(f.trim_start_matches('=').to_string());
    }
    Ok(cells.into_values().collect())
}

/// An xlsb sheet's cells and formulas by its streaming readers (two passes).
fn xlsb_cells<RS: Read + Seek>(
    x: &mut calamine::Xlsb<RS>,
    name: &str,
    limits: &Limits,
    before: u64,
) -> Result<Vec<Cell>, Error> {
    let mut cells: BTreeMap<(u32, u32), Cell> = BTreeMap::new();
    {
        let mut r = x.worksheet_cells_reader(name).map_err(err)?;
        let mut seen: u64 = 0;
        while let Some(c) = r.next_cell().map_err(err)? {
            seen += 1;
            if seen.is_multiple_of(65_536) {
                limits.check_time()?;
                limits.check_cells(before + cells.len() as u64)?;
            }
            let (row, col) = c.get_position();
            match convert_ref(c.get_value().clone()) {
                Some(value) => {
                    cells.insert(
                        (row, col),
                        Cell {
                            row,
                            col,
                            value: Some(value),
                            formula: None,
                        },
                    );
                }
                None => {
                    cells.remove(&(row, col));
                }
            }
        }
    }
    let mut r = x.worksheet_cells_reader(name).map_err(err)?;
    let mut seen: u64 = 0;
    while let Some(f) = r.next_formula().map_err(err)? {
        seen += 1;
        if seen.is_multiple_of(65_536) {
            limits.check_time()?;
            limits.check_cells(before + cells.len() as u64)?;
        }
        if f.get_value().is_empty() {
            continue;
        }
        let (row, col) = f.get_position();
        cells
            .entry((row, col))
            .or_insert(Cell {
                row,
                col,
                value: None,
                formula: None,
            })
            .formula = Some(f.get_value().trim_start_matches('=').to_string());
    }
    Ok(cells.into_values().collect())
}

/// [`read_xlsx_cells`]'s fallback: values, then formulas (a formula error drops the formulas
/// only, as calamine's `worksheet_formula` did); a part that is no worksheet is an empty sheet.
fn xlsx_cells_fallback<RS: Read + Seek>(
    x: &mut Xlsx<RS>,
    name: &str,
    limits: &Limits,
    before: u64,
) -> Result<Vec<Cell>, Error> {
    let mut cells: BTreeMap<(u32, u32), Cell> = BTreeMap::new();
    {
        let mut r = match x.worksheet_cells_reader(name) {
            Ok(r) => r,
            Err(XlsxError::NotAWorksheet(_)) => return Ok(Vec::new()),
            Err(e) => return Err(err(e)),
        };
        let mut seen: u64 = 0;
        while let Some(c) = r.next_cell().map_err(err)? {
            seen += 1;
            if seen.is_multiple_of(65_536) {
                limits.check_time()?;
                limits.check_cells(before + cells.len() as u64)?;
            }
            let (row, col) = c.get_position();
            match convert_ref(c.get_value().clone()) {
                Some(value) => {
                    cells.insert(
                        (row, col),
                        Cell {
                            row,
                            col,
                            value: Some(value),
                            formula: None,
                        },
                    );
                }
                None => {
                    cells.remove(&(row, col));
                }
            }
        }
    }
    let mut formulas: Vec<(u32, u32, String)> = Vec::new();
    let read_formulas = (|| -> Result<(), XlsxError> {
        let mut r = x.worksheet_cells_reader(name)?;
        while let Some(f) = r.next_formula()? {
            if !f.get_value().is_empty() {
                let (row, col) = f.get_position();
                formulas.push((row, col, f.get_value().trim_start_matches('=').to_string()));
            }
        }
        Ok(())
    })();
    if read_formulas.is_ok() {
        for (row, col, f) in formulas {
            cells
                .entry((row, col))
                .or_insert(Cell {
                    row,
                    col,
                    value: None,
                    formula: None,
                })
                .formula = Some(f);
        }
    }
    limits.check_cells(before + cells.len() as u64)?;
    Ok(cells.into_values().collect())
}

/// Copies the xlsx zip into memory with `<sheet … r:id="">` entries removed from `xl/workbook.xml`.
fn repair_unlinked_sheets(file: impl Read + Seek) -> zip::result::ZipResult<Vec<u8>> {
    let mut src = zip::ZipArchive::new(file)?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..src.len() {
        let entry = src.by_index(i)?;
        if entry.name()? == "xl/workbook.xml" {
            let mut entry = entry;
            let mut xml = String::new();
            entry.by_ref().take(256 << 20).read_to_string(&mut xml)?;
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            out.start_file("xl/workbook.xml", opts)?;
            out.write_all(strip_unlinked_sheets(&xml).as_bytes())?;
        } else {
            drop(entry);
            out.raw_copy_file(src.by_index_raw(i)?)?;
        }
    }
    Ok(out.finish()?.into_inner())
}

fn strip_unlinked_sheets(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<sheet ") {
        let Some(len) = rest[start..].find("/>") else {
            break;
        };
        let end = start + len + 2;
        out.push_str(&rest[..start]);
        let tag = &rest[start..end];
        if !tag.contains("r:id=\"\"") {
            out.push_str(tag);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A merged area written bottom-right first (`B2:A1`, which Excel reads as `A1:B2`) gets its
/// corners swapped: everything downstream relies on `top <= bottom` and `left <= right`.
fn ordered_corners(merges: &mut [Rect]) {
    for m in merges {
        if m.top > m.bottom {
            std::mem::swap(&mut m.top, &mut m.bottom);
        }
        if m.left > m.right {
            std::mem::swap(&mut m.left, &mut m.right);
        }
    }
}

fn merge_cells<RS: Read + Seek>(wb: &mut Sheets<RS>, name: &str) -> Vec<Rect> {
    let dims: Vec<Dimensions> = match wb {
        Sheets::Xlsx(x) => x.merge_cells_by_sheet_name(name).unwrap_or_default(),
        Sheets::Xls(x) => x.merge_cells_by_sheet_name(name).unwrap_or_default(),
        _ => Vec::new(),
    };
    dims.into_iter()
        .map(|d| Rect {
            top: d.start.0,
            left: d.start.1,
            bottom: d.end.0,
            right: d.end.1,
        })
        .collect()
}

/// The cells of a dense value range and a dense formula range: values that convert, with the
/// non-empty formula of the same position, and formulas alone. Both ranges are walked in row
/// order, so the cells come out sorted by position (B136: a merge instead of a sorted map).
fn build_sheet(
    name: String,
    values: &Range<Data>,
    formulas: &Range<String>,
    merges: Vec<Rect>,
) -> Sheet {
    let (vr, vc) = values.start().unwrap_or((0, 0));
    let mut vals = values
        .used_cells()
        .filter_map(|(r, c, d)| convert(d).map(|v| ((vr + r as u32, vc + c as u32), v)))
        .peekable();
    let (fr, fc) = formulas.start().unwrap_or((0, 0));
    let mut forms = formulas
        .used_cells()
        .filter(|(_, _, f)| !f.is_empty())
        .map(|(r, c, f)| ((fr + r as u32, fc + c as u32), f))
        .peekable();
    let mut cells = Vec::new();
    loop {
        let cell = match (vals.peek(), forms.peek()) {
            (None, None) => break,
            (Some(&(v, _)), Some(&(f, _))) if v == f => {
                let ((row, col), value) = vals.next().unwrap();
                let (_, f) = forms.next().unwrap();
                Cell {
                    row,
                    col,
                    value: Some(value),
                    formula: Some(f.trim_start_matches('=').to_string()),
                }
            }
            (Some(&(v, _)), f) if f.is_none_or(|&(f, _)| v < f) => {
                let ((row, col), value) = vals.next().unwrap();
                Cell {
                    row,
                    col,
                    value: Some(value),
                    formula: None,
                }
            }
            _ => {
                let ((row, col), f) = forms.next().unwrap();
                Cell {
                    row,
                    col,
                    value: None,
                    formula: Some(f.trim_start_matches('=').to_string()),
                }
            }
        };
        cells.push(cell);
    }
    Sheet {
        name,
        cells,
        merges,
        formats: Vec::new(),
    }
}

/// [`convert`] for a borrowed xlsx value (shared strings are copied once, not twice).
fn convert_ref(d: DataRef<'_>) -> Option<Value> {
    Some(match d {
        DataRef::Empty => return None,
        DataRef::SharedString(s) if s.trim().is_empty() => return None,
        DataRef::SharedString(s) => Value::Text(s.to_string()),
        DataRef::String(s) if s.trim().is_empty() => return None,
        DataRef::String(s) => Value::Text(s),
        DataRef::Int(i) => Value::Number(i as f64),
        DataRef::Float(f) => Value::Number(f),
        DataRef::Bool(b) => Value::Bool(b),
        DataRef::DateTime(dt) => Value::DateTime(dt.as_f64_1900()),
        DataRef::DateTimeIso(s) | DataRef::DurationIso(s) => Value::Text(s),
        DataRef::Error(e) => Value::Error(e.to_string()),
    })
}

fn convert(d: &Data) -> Option<Value> {
    Some(match d {
        Data::Empty => return None,
        Data::String(s) if s.trim().is_empty() => return None,
        Data::String(s) => Value::Text(s.clone()),
        Data::Int(i) => Value::Number(*i as f64),
        Data::Float(f) => Value::Number(*f),
        Data::Bool(b) => Value::Bool(*b),
        Data::DateTime(dt) => Value::DateTime(dt.as_f64_1900()),
        // ODS gives ISO strings; keep them as text until the value normalizer parses dates.
        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::Text(s.clone()),
        Data::Error(e) => Value::Error(e.to_string()),
    })
}
