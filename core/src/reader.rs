//! Reads xlsx / xlsm / xlsb / xls / ods into the [`Workbook`] model via calamine.
//! Calamine gives values, formulas and merges but no styles; a style-aware reader can replace
//! this module later without changing the model.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;

use calamine::{
    open_workbook_auto, open_workbook_auto_from_rs, Data, DataRef, Dimensions, Range, Reader, Sheets, Xlsx, XlsxError,
};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::model::{Cell, Rect, Sheet, Value, Workbook};

mod merges;

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err(e: impl fmt::Display) -> Error {
    Error(e.to_string())
}

pub fn open(path: impl AsRef<Path>) -> Result<Workbook, Error> {
    let path = path.as_ref();
    match open_workbook_auto(path) {
        Ok(wb) => {
            let scan = || merges::Scanner::new(File::open(path).ok()?);
            read_all(wb, scan)
        }
        // Some converted workbooks list a veryHidden sheet with `r:id=""` (a leftover VBA module
        // sheet); calamine rejects the whole file. Drop those entries and retry from memory.
        Err(e) if e.to_string().contains("Relationship not found") => {
            let repaired = repair_unlinked_sheets(path).map_err(|_| err(e))?;
            let wb = open_workbook_auto_from_rs(Cursor::new(repaired.as_slice())).map_err(err)?;
            read_all(wb, || merges::Scanner::new(Cursor::new(repaired.as_slice())))
        }
        Err(e) => Err(err(e)),
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
) -> Result<Workbook, Error> {
    let names = wb.sheet_names();
    let mut sheets = Vec::with_capacity(names.len());
    let mut scanner = match &wb {
        Sheets::Xlsx(_) => scanner(),
        _ => None,
    };
    for name in names {
        // xlsx: values and formulas in one streaming pass, straight into sparse cells.
        let cells = match &mut wb {
            Sheets::Xlsx(x) => read_xlsx_cells(x, &name).ok(),
            _ => None,
        };
        let sheet = match cells {
            Some((cells, dims)) => {
                // Merged areas sit after the cells; a byte scan finds them much faster than a
                // second full XML parse. Anything unexpected → calamine's parse.
                let merges = match scanner.as_mut().and_then(|s| s.merges(&name, dims)) {
                    Some(m) => {
                        merges::SCANNED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        m
                    }
                    None => {
                        merges::FALLBACK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        merge_cells(&mut wb, &name)
                    }
                };
                Sheet { name, cells, merges }
            }
            None => {
                let values = wb.worksheet_range(&name).map_err(err)?;
                // Some formats can't report formulas; values alone are still useful.
                let formulas = wb.worksheet_formula(&name).unwrap_or_default();
                let merges = merge_cells(&mut wb, &name);
                build_sheet(name, &values, &formulas, merges)
            }
        };
        sheets.push(sheet);
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
fn read_xlsx_cells<RS: Read + Seek>(x: &mut Xlsx<RS>, name: &str) -> Result<(Vec<Cell>, Dimensions), XlsxError> {
    let mut reader = x.worksheet_cells_reader(name)?;
    let dims = reader.dimensions();
    let mut recs: Vec<Record> = Vec::new();
    while let Some(rec) = reader.next_cell_with_formula()? {
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

/// Copies the xlsx zip into memory with `<sheet … r:id="">` entries removed from `xl/workbook.xml`.
fn repair_unlinked_sheets(path: &Path) -> zip::result::ZipResult<Vec<u8>> {
    let mut src = zip::ZipArchive::new(File::open(path)?)?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..src.len() {
        let entry = src.by_index(i)?;
        if entry.name()? == "xl/workbook.xml" {
            let mut entry = entry;
            let mut xml = String::new();
            entry.read_to_string(&mut xml)?;
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
    Sheet { name, cells: cells.into_values().collect(), merges }
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
