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
    open_workbook_auto, open_workbook_auto_from_rs, Data, DataRef, Dimensions, Range, Reader, Sheets, Xlsx, XlsxError,
};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::model::{Cell, Rect, Sheet, Value, Workbook};

mod csv;
mod limits;
mod merges;
mod styles;

pub use limits::{Limits, DEFAULT_MAX_CELLS, DEFAULT_MAX_DECOMPRESSED_BYTES, DEFAULT_MAX_SHARED_STRINGS};

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
    Error(if line.is_empty() { "unreadable file".into() } else { line })
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

fn sniff(path: &Path) -> Option<Format> {
    let mut f = File::open(path).ok()?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok()?;
    match magic {
        [0xD0, 0xCF, 0x11, 0xE0] => Some(Format::Xls),
        [b'P', b'K', 3, 4] => {
            f.rewind().ok()?;
            let mut zip = zip::ZipArchive::new(std::io::BufReader::new(f)).ok()?;
            // ODF: a `mimetype` entry (normally stored, sometimes deflated) names the type.
            if let Ok(mut m) = zip.by_name("mimetype") {
                let mut t = String::new();
                m.by_ref().take(256).read_to_string(&mut t).ok()?;
                return t.trim().starts_with("application/vnd.oasis.opendocument.spreadsheet").then_some(Format::Ods);
            }
            if zip.index_for_name("xl/workbook.bin").is_some() {
                return Some(Format::Xlsb);
            }
            if zip.index_for_name("xl/workbook.xml").is_some() {
                return Some(Format::Xlsx);
            }
            let mut ct = String::new();
            zip.by_name("[Content_Types].xml").ok()?.by_ref().take(1 << 20).read_to_string(&mut ct).ok()?;
            if ct.contains("sheet.binary.macroEnabled.main") {
                Some(Format::Xlsb)
            } else if ct.contains("spreadsheetml") || ct.contains("ms-excel") {
                Some(Format::Xlsx)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// CSV / TSV: any name but a workbook extension (`.csv`, `.tsv`, `.txt`, `.asp`, none, ...) with
/// content that is not a zip or OLE container (a workbook saved as `.csv` opens as the workbook
/// it is). Unreadable files go to calamine, which reports the error.
fn is_delimited_text(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("xls" | "xla" | "xlsx" | "xlsm" | "xlam" | "xlsb" | "ods")) {
        return false;
    }
    let mut magic = Vec::with_capacity(4);
    match File::open(path).and_then(|f| f.take(4).read_to_end(&mut magic)) {
        Ok(_) => !matches!(magic[..], [0xD0, 0xCF, 0x11, 0xE0] | [b'P', b'K', 3, 4]),
        Err(_) => false,
    }
}

/// Opens the workbook with the reader its content calls for (extension as a fallback).
fn open_sheets(path: &Path) -> Result<Sheets<BufReader<File>>, calamine::Error> {
    use calamine::{open_workbook, Error as E};
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let by_ext = match ext.as_deref() {
        Some("xls" | "xla") => Some(Format::Xls),
        Some("xlsx" | "xlsm" | "xlam") => Some(Format::Xlsx),
        Some("xlsb") => Some(Format::Xlsb),
        Some("ods") => Some(Format::Ods),
        _ => None,
    };
    match sniff(path).or(by_ext) {
        Some(Format::Xls) => Ok(Sheets::Xls(open_workbook(path).map_err(E::Xls)?)),
        Some(Format::Xlsx) => Ok(Sheets::Xlsx(open_workbook(path).map_err(E::Xlsx)?)),
        Some(Format::Xlsb) => Ok(Sheets::Xlsb(open_workbook(path).map_err(E::Xlsb)?)),
        Some(Format::Ods) => Ok(Sheets::Ods(open_workbook(path).map_err(E::Ods)?)),
        None => open_workbook_auto(path),
    }
}

/// What [`open_with`] reads besides values, formulas and merged areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadOptions {
    /// Cell formats (`Sheet::formats`, xlsx / xlsm only). Costs an extra look at every cell tag
    /// (about 15–35% more read time) and 4 bytes per cell.
    pub formats: bool,
    /// Size, cell and time limits for untrusted files.
    pub limits: Limits,
}

impl Default for ReadOptions {
    /// Formats only when the embedded learned model reads them; [`Limits::default`].
    fn default() -> Self {
        ReadOptions { formats: crate::learned::embedded().is_some_and(|m| m.reads_formats()), limits: Limits::default() }
    }
}

/// Opens a workbook with [`ReadOptions::default`].
pub fn open(path: impl AsRef<Path>) -> Result<Workbook, Error> {
    open_with(path, ReadOptions::default())
}

pub fn open_with(path: impl AsRef<Path>, opts: ReadOptions) -> Result<Workbook, Error> {
    let path = path.as_ref();
    let limits = &opts.limits;
    if is_delimited_text(path) {
        let wb = csv::open(path)?;
        limits.check_cells(wb.sheets.iter().map(|s| s.cells.len() as u64).sum())?;
        return Ok(wb);
    }
    match magic(path) {
        Some(Magic::Ole) => limits::check_ole(path)?,
        Some(Magic::Zip) => limits::check_zip(path, limits)?,
        None => {}
    }
    match open_sheets(path) {
        Ok(wb) => {
            let scan = || merges::Scanner::new(File::open(path).ok()?, opts.formats);
            read_all(wb, scan, limits)
        }
        // Some converted workbooks list a veryHidden sheet with `r:id=""` (a leftover VBA module
        // sheet); calamine rejects the whole file. Drop those entries and retry from memory.
        Err(e) if e.to_string().contains("Relationship not found") => {
            let repaired = repair_unlinked_sheets(path).map_err(|_| err(e))?;
            let wb = open_workbook_auto_from_rs(Cursor::new(repaired.as_slice())).map_err(err)?;
            read_all(wb, || merges::Scanner::new(Cursor::new(repaired.as_slice()), opts.formats), limits)
        }
        Err(e) => Err(err(e)),
    }
}

enum Magic {
    Ole,
    Zip,
}

fn magic(path: &Path) -> Option<Magic> {
    let mut m = [0u8; 4];
    File::open(path).ok()?.read_exact(&mut m).ok()?;
    match m {
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
    (merges::SCANNED.load(Relaxed), merges::FALLBACK.load(Relaxed))
}

/// `scanner` opens a second, lightweight view of the same xlsx package for [`merges::Scanner`];
/// it is only called for xlsx files.
fn read_all<RS: Read + Seek, R: Read + Seek>(
    mut wb: Sheets<RS>,
    scanner: impl FnOnce() -> Option<merges::Scanner<R>>,
    limits: &Limits,
) -> Result<Workbook, Error> {
    let names = wb.sheet_names();
    let mut sheets = Vec::with_capacity(names.len());
    let mut scanner = match &wb {
        Sheets::Xlsx(_) => scanner(),
        _ => None,
    };
    // Cells read so far, against `limits.max_cells`.
    let mut total: u64 = 0;
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
            sheets.push(Sheet { name, cells, merges, formats: Vec::new() });
            continue;
        }
        // xlsx: values and formulas in one streaming pass, straight into sparse cells.
        let cells = match &mut wb {
            Sheets::Xlsx(x) => match read_xlsx_cells(x, &name, limits, total) {
                Ok(c) => Some(c),
                Err(ReadErr::Limit(e)) => return Err(e),
                Err(ReadErr::Xlsx) => None,
            },
            _ => None,
        };
        let sheet = match cells {
            Some((cells, dims)) => {
                // Merged areas sit after the cells; a byte scan finds them (and every cell's
                // format) much faster than a second full XML parse. Anything unexpected →
                // calamine's parse of the merges, and no formats.
                let (merges, formats) = match scanner.as_mut().and_then(|s| s.scan(&name, dims, &cells)) {
                    Some((m, f)) => {
                        merges::SCANNED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        (m, f.unwrap_or_default())
                    }
                    None => {
                        merges::FALLBACK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        (merge_cells(&mut wb, &name), Vec::new())
                    }
                };
                Sheet { name, cells, merges, formats }
            }
            None if matches!(wb, Sheets::Xlsx(_)) => {
                // An xlsx part the one-pass reader rejected: values and formulas in two sparse
                // passes (calamine's dense range spans the cells' bounding box: 38 GB for a
                // hostile sheet), no formats.
                let Sheets::Xlsx(x) = &mut wb else { unreachable!() };
                let cells = xlsx_cells_fallback(x, &name, limits, total)?;
                let merges = merge_cells(&mut wb, &name);
                Sheet { name, cells, merges, formats: Vec::new() }
            }
            None => {
                // ods: its cell count was checked before calamine parsed it.
                let values = wb.worksheet_range(&name).map_err(err)?;
                // Some formats can't report formulas; values alone are still useful.
                let formulas = wb.worksheet_formula(&name).unwrap_or_default();
                let merges = merge_cells(&mut wb, &name);
                build_sheet(name, &values, &formulas, merges)
            }
        };
        total += sheet.cells.len() as u64;
        limits.check_cells(total)?;
        sheets.push(sheet);
    }
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
            limits.check_cells(before + recs.len() as u64).map_err(ReadErr::Limit)?;
        }
        let has_value = !matches!(rec.value, DataRef::Empty);
        let formula = rec.formula.filter(|f| !f.is_empty()).map(|f| match f.strip_prefix('=') {
            Some(_) => f.trim_start_matches('=').to_string(),
            None => f,
        });
        if !has_value && formula.is_none() {
            continue;
        }
        let value = if has_value { convert_ref(rec.value) } else { None };
        recs.push(Record { row: rec.pos.0, col: rec.pos.1, value, has_value, formula });
    }
    drop(reader);

    // Usually already in order; otherwise sort (stable) and fold duplicates like the dense ranges
    // would: the last non-empty value and the last non-empty formula of a position win.
    if !recs.windows(2).all(|w| (w[0].row, w[0].col) < (w[1].row, w[1].col)) {
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
    let cells = recs
        .into_iter()
        .filter(|r| r.value.is_some() || r.formula.is_some())
        .map(|r| Cell { row: r.row, col: r.col, value: r.value, formula: r.formula })
        .collect();
    Ok((cells, dims))
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
fn xls_cells<RS: Read + Seek>(x: &calamine::Xls<RS>, name: &str) -> Result<Vec<Cell>, calamine::XlsError> {
    let (values, formulas) = x.worksheet_cells(name)?;
    let mut cells: BTreeMap<(u32, u32), Cell> = BTreeMap::new();
    for c in values {
        let (row, col) = c.get_position();
        match convert(c.get_value()) {
            Some(value) => {
                cells.insert((row, col), Cell { row, col, value: Some(value), formula: None });
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
        cells.entry((row, col)).or_insert(Cell { row, col, value: None, formula: None }).formula =
            Some(f.trim_start_matches('=').to_string());
    }
    Ok(cells.into_values().collect())
}

/// An xlsb sheet's cells and formulas by its streaming readers (two passes).
fn xlsb_cells<RS: Read + Seek>(x: &mut calamine::Xlsb<RS>, name: &str, limits: &Limits, before: u64) -> Result<Vec<Cell>, Error> {
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
                    cells.insert((row, col), Cell { row, col, value: Some(value), formula: None });
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
        cells.entry((row, col)).or_insert(Cell { row, col, value: None, formula: None }).formula =
            Some(f.get_value().trim_start_matches('=').to_string());
    }
    Ok(cells.into_values().collect())
}

/// [`read_xlsx_cells`]'s fallback: values, then formulas (a formula error drops the formulas
/// only, as calamine's `worksheet_formula` did); a part that is no worksheet is an empty sheet.
fn xlsx_cells_fallback<RS: Read + Seek>(x: &mut Xlsx<RS>, name: &str, limits: &Limits, before: u64) -> Result<Vec<Cell>, Error> {
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
                    cells.insert((row, col), Cell { row, col, value: Some(value), formula: None });
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
            cells.entry((row, col)).or_insert(Cell { row, col, value: None, formula: None }).formula = Some(f);
        }
    }
    limits.check_cells(before + cells.len() as u64)?;
    Ok(cells.into_values().collect())
}

/// Copies the xlsx zip into memory with `<sheet … r:id="">` entries removed from `xl/workbook.xml`.
fn repair_unlinked_sheets(path: &Path) -> zip::result::ZipResult<Vec<u8>> {
    let mut src = zip::ZipArchive::new(File::open(path)?)?;
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
        let Some(len) = rest[start..].find("/>") else { break };
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
        .map(|d| Rect { top: d.start.0, left: d.start.1, bottom: d.end.0, right: d.end.1 })
        .collect()
}

fn build_sheet(name: String, values: &Range<Data>, formulas: &Range<String>, merges: Vec<Rect>) -> Sheet {
    let mut cells: BTreeMap<(u32, u32), Cell> = BTreeMap::new();
    let (vr, vc) = values.start().unwrap_or((0, 0));
    for (r, c, d) in values.used_cells() {
        if let Some(value) = convert(d) {
            let (row, col) = (vr + r as u32, vc + c as u32);
            cells.insert((row, col), Cell { row, col, value: Some(value), formula: None });
        }
    }
    let (fr, fc) = formulas.start().unwrap_or((0, 0));
    for (r, c, f) in formulas.used_cells() {
        if f.is_empty() {
            continue;
        }
        let (row, col) = (fr + r as u32, fc + c as u32);
        cells
            .entry((row, col))
            .or_insert(Cell { row, col, value: None, formula: None })
            .formula = Some(f.trim_start_matches('=').to_string());
    }
    Sheet { name, cells: cells.into_values().collect(), merges, formats: Vec::new() }
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
        DataRef::DateTime(dt) => Value::DateTime(dt.as_f64()),
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
        Data::DateTime(dt) => Value::DateTime(dt.as_f64()),
        // ODS gives ISO strings; keep them as text until the value normalizer parses dates.
        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::Text(s.clone()),
        Data::Error(e) => Value::Error(e.to_string()),
    })
}
