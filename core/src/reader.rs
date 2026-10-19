//! Reads xlsx / xlsm / xlsb / xls / ods into the [`Workbook`] model via calamine.
//! Calamine gives values, formulas and merges but no styles; a style-aware reader can replace
//! this module later without changing the model.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use calamine::{open_workbook_auto, Data, Dimensions, Range, Reader, Sheets};

use crate::model::{Cell, Rect, Sheet, Value, Workbook};

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
    let mut wb = open_workbook_auto(path).map_err(err)?;
    let names = wb.sheet_names();
    let mut sheets = Vec::with_capacity(names.len());
    for name in names {
        let values = wb.worksheet_range(&name).map_err(err)?;
        // Some formats can't report formulas; values alone are still useful.
        let formulas = wb.worksheet_formula(&name).unwrap_or_default();
        let merges = merge_cells(&mut wb, &name);
        sheets.push(build_sheet(name, &values, &formulas, merges));
    }
    Ok(Workbook { sheets })
}

fn merge_cells<RS: std::io::Read + std::io::Seek>(wb: &mut Sheets<RS>, name: &str) -> Vec<Rect> {
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
