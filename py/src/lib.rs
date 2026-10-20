//! Python bindings. Kept thin: all logic lives in `hh_core`; these functions only look things up
//! and convert to Python dicts / lists.

use hh_core::totals::{self, Direction, Evidence, RefKind, TotalLevel, TotalOrigin};
use hh_core::{a1, Cell, Grid, Rect, Sheet, Value};
use pyo3::exceptions::{PyKeyError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use pyo3::IntoPyObjectExt;

#[pyclass(frozen)]
struct Workbook(hh_core::Workbook);

/// Snake-case name of a Rust enum variant (`GrandTotal` → `grand_total`).
fn snake(debug: impl std::fmt::Debug) -> String {
    let s = format!("{debug:?}");
    let s = s.split('(').next().unwrap_or("");
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn range_a1(r: Rect) -> String {
    if r.top == r.bottom && r.left == r.right {
        a1(r.top, r.left)
    } else {
        format!("{}:{}", a1(r.top, r.left), a1(r.bottom, r.right))
    }
}

/// ISO date (`2024-03-31`) for an Excel serial, or the serial itself as text if out of range.
fn iso_date(serial: f64) -> String {
    match hh_core::values::serial_to_ymd(serial) {
        Some((y, m, d)) => format!("{y:04}-{m:02}-{d:02}"),
        None => format!("{serial}"),
    }
}

fn value_type(v: Option<&Value>) -> &'static str {
    match v {
        None => "empty",
        Some(Value::Bool(_)) => "bool",
        Some(Value::Number(_)) => "number",
        Some(Value::Text(_)) => "text",
        Some(Value::DateTime(_)) => "date",
        Some(Value::Error(_)) => "error",
    }
}

/// Plain Python value: float, str, bool, ISO date string, `#ERR` string, or None.
fn value_py<'py>(py: Python<'py>, v: Option<&Value>) -> PyResult<Bound<'py, PyAny>> {
    match v {
        None => Ok(py.None().into_bound(py)),
        Some(Value::Bool(b)) => b.into_bound_py_any(py),
        Some(Value::Number(n)) => n.into_bound_py_any(py),
        Some(Value::Text(s)) => s.into_bound_py_any(py),
        Some(Value::DateTime(d)) => iso_date(*d).into_bound_py_any(py),
        Some(Value::Error(e)) => e.into_bound_py_any(py),
    }
}

/// One local reference parsed with the core formula parser: `D43`, `$D$43`, `A1:F20`, `Sheet!B2`.
fn parse_ref(text: &str, kinds: &[RefKind]) -> PyResult<(Option<String>, Rect)> {
    let parsed = totals::parse(text.trim().trim_start_matches('='));
    match parsed.refs.as_slice() {
        [r] if !r.external && kinds.contains(&r.kind) => Ok((r.sheet.clone(), r.rect)),
        _ => Err(PyValueError::new_err(format!("not a cell reference: {text:?}"))),
    }
}

fn cell_dict<'py>(py: Python<'py>, sheet: &Sheet, row: u32, col: u32, cell: Option<&Cell>) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("sheet", &sheet.name)?;
    d.set_item("address", a1(row, col))?;
    let v = cell.and_then(|c| c.value.as_ref());
    d.set_item("value", value_py(py, v)?)?;
    d.set_item("type", value_type(v))?;
    d.set_item("formula", cell.and_then(|c| c.formula.as_ref()).map(|f| format!("={f}")))?;
    Ok(d)
}

impl Workbook {
    fn sheet(&self, name: &str) -> PyResult<&Sheet> {
        self.0
            .sheets
            .iter()
            .find(|s| s.name == name)
            .or_else(|| self.0.sheets.iter().find(|s| s.name.eq_ignore_ascii_case(name)))
            .ok_or_else(|| PyKeyError::new_err(format!("no sheet named {name:?}; sheets: {:?}", self.names())))
    }

    /// Sheet named in the reference wins over the `sheet` argument; with neither, the first sheet.
    fn sheet_for(&self, sheet: Option<&str>, in_ref: Option<String>) -> PyResult<&Sheet> {
        match in_ref.as_deref().or(sheet) {
            Some(n) => self.sheet(n),
            None => self.0.sheets.first().ok_or_else(|| PyValueError::new_err("workbook has no sheets")),
        }
    }

    fn names(&self) -> Vec<String> {
        self.0.sheets.iter().map(|s| s.name.clone()).collect()
    }
}

#[pymethods]
impl Workbook {
    #[getter]
    fn sheet_names(&self) -> Vec<String> {
        self.names()
    }

    fn describe(&self) -> String {
        hh_core::describe(&self.0)
    }

    /// One dict per sheet: name, used range (`A1:F20` or None), rows, cols, cells, formulas, merges.
    fn sheets<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let out = PyList::empty(py);
        for s in &self.0.sheets {
            let d = PyDict::new(py);
            d.set_item("name", &s.name)?;
            let used = s.used_range();
            d.set_item("used_range", used.map(range_a1))?;
            d.set_item("rows", used.map_or(0, |r| r.bottom - r.top + 1))?;
            d.set_item("cols", used.map_or(0, |r| r.right - r.left + 1))?;
            d.set_item("cells", s.cells.len())?;
            d.set_item("formulas", s.formula_count())?;
            d.set_item("merges", s.merges.len())?;
            out.append(d)?;
        }
        Ok(out)
    }

    /// `get_cell("Sheet1", "D43")` or `get_cell(None, "Sheet1!D43")` →
    /// `{sheet, address, value, type, formula}`; an empty cell has value None, type "empty".
    #[pyo3(signature = (sheet, address))]
    fn get_cell<'py>(&self, py: Python<'py>, sheet: Option<&str>, address: &str) -> PyResult<Bound<'py, PyDict>> {
        let (in_ref, rect) = parse_ref(address, &[RefKind::Cell])?;
        let s = self.sheet_for(sheet, in_ref)?;
        let grid = Grid::new(s);
        cell_dict(py, s, rect.top, rect.left, grid.cell(rect.top, rect.left))
    }

    /// Values of a rectangular range, row by row (None for empty cells). Larger ranges than
    /// `max_cells` are rejected so a caller cannot dump a huge sheet by accident.
    #[pyo3(signature = (sheet, range, max_cells = 5000))]
    fn get_range<'py>(&self, py: Python<'py>, sheet: Option<&str>, range: &str, max_cells: u64) -> PyResult<Bound<'py, PyDict>> {
        let (in_ref, rect) = parse_ref(range, &[RefKind::Cell, RefKind::Range])?;
        let s = self.sheet_for(sheet, in_ref)?;
        let area = (rect.bottom - rect.top + 1) as u64 * (rect.right - rect.left + 1) as u64;
        if area > max_cells {
            return Err(PyValueError::new_err(format!(
                "range {} has {area} cells, more than max_cells={max_cells}",
                range_a1(rect)
            )));
        }
        let grid = Grid::new(s);
        let rows = PyList::empty(py);
        for r in rect.top..=rect.bottom {
            let row = PyList::empty(py);
            for c in rect.left..=rect.right {
                row.append(value_py(py, grid.cell(r, c).and_then(|x| x.value.as_ref()))?)?;
            }
            rows.append(row)?;
        }
        let d = PyDict::new(py);
        d.set_item("sheet", &s.name)?;
        d.set_item("range", range_a1(rect))?;
        d.set_item("rows", rows)?;
        Ok(d)
    }

    /// Cells whose text contains `query` (case-, diacritic- and script-insensitive via
    /// `values::norm_key`), on one sheet or all. Each hit is a cell dict.
    #[pyo3(signature = (query, sheet = None, limit = 50))]
    fn find_text<'py>(&self, py: Python<'py>, query: &str, sheet: Option<&str>, limit: usize) -> PyResult<Bound<'py, PyList>> {
        let key = hh_core::values::norm_key(query);
        let out = PyList::empty(py);
        if key.is_empty() {
            return Ok(out);
        }
        let sheets: Vec<&Sheet> = match sheet {
            Some(n) => vec![self.sheet(n)?],
            None => self.0.sheets.iter().collect(),
        };
        for s in sheets {
            for c in &s.cells {
                if out.len() >= limit {
                    return Ok(out);
                }
                let hit = match &c.value {
                    Some(Value::Text(t)) => hh_core::values::norm_key(t).contains(&key),
                    Some(Value::Number(n)) => format!("{n}").contains(query.trim()),
                    _ => false,
                };
                if hit {
                    out.append(cell_dict(py, s, c.row, c.col, Some(c))?)?;
                }
            }
        }
        Ok(out)
    }

    /// Detected totals of a sheet with their verification (`hh_core::totals::{detect, verify}`).
    fn totals<'py>(&self, py: Python<'py>, sheet: &str) -> PyResult<Bound<'py, PyList>> {
        let s = self.sheet(sheet)?;
        let grid = Grid::new(s);
        let out = PyList::empty(py);
        for cand in totals::detect(&grid) {
            let check = totals::verify(&grid, &cand);
            let d = PyDict::new(py);
            d.set_item("sheet", &s.name)?;
            d.set_item("address", a1(cand.row, cand.col))?;
            d.set_item("origin", match cand.origin {
                TotalOrigin::Formula => "formula",
                TotalOrigin::Typed => "typed",
            })?;
            d.set_item("level", match cand.level {
                TotalLevel::Total => "total",
                TotalLevel::Subtotal => "subtotal",
                TotalLevel::GrandTotal => "grand_total",
            })?;
            d.set_item("direction", match cand.direction {
                Direction::Vertical => "vertical",
                Direction::Horizontal => "horizontal",
            })?;
            d.set_item("sources", cand.ranges.iter().map(|r| range_a1(*r)).collect::<Vec<_>>())?;
            d.set_item("excluded", cand.exclude.iter().map(|&(r, c)| a1(r, c)).collect::<Vec<_>>())?;
            d.set_item("double_counted", cand.double_counted.iter().map(|&(r, c)| a1(r, c)).collect::<Vec<_>>())?;
            let label_cell = cand.label.and_then(|(r, c)| grid.cell(r, c));
            d.set_item("label_address", cand.label.map(|(r, c)| a1(r, c)))?;
            d.set_item("label", value_py(py, label_cell.and_then(|c| c.value.as_ref()))?)?;
            d.set_item("formula", grid.cell(cand.row, cand.col).and_then(|c| c.formula.as_ref()).map(|f| format!("={f}")))?;
            d.set_item("evidence", cand.evidence.iter().map(|e| match e {
                Evidence::Formula(k) => format!("formula:{}", snake(k)),
                Evidence::Keyword(k) => format!("keyword:{}", snake(k)),
                Evidence::GroupLabel(r, c) => format!("group_label:{}", a1(*r, *c)),
                other => snake(other),
            }).collect::<Vec<_>>())?;
            d.set_item("actual", check.actual)?;
            d.set_item("expected", check.expected)?;
            d.set_item("diff", check.diff)?;
            d.set_item("ok", check.ok)?;
            d.set_item("reason", snake(check.reason))?;
            d.set_item("summed", check.summed)?;
            d.set_item("numbers_as_text", check.numbers_as_text.iter().map(|&(r, c)| a1(r, c)).collect::<Vec<_>>())?;
            out.append(d)?;
        }
        Ok(out)
    }

    fn __repr__(&self) -> String {
        format!("Workbook(sheets={:?})", self.names())
    }
}

#[pyfunction]
fn open(path: &str) -> PyResult<Workbook> {
    hh_core::open(path).map(Workbook).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Writes `<stem>.normalized.xlsx` + `<stem>.manifest.json` (+ CSVs) into `out_dir`; see
/// `hh_core::normalize`. Returns `{xlsx_path, manifest_path, csv_paths, manifest_json}`.
#[pyfunction]
#[pyo3(signature = (path, out_dir, keep_totals = false, csv = false, locale = None, rules_only = false, model_max_cells = None))]
#[allow(clippy::too_many_arguments)] // one per Python keyword argument
fn normalize<'py>(
    py: Python<'py>,
    path: &str,
    out_dir: &str,
    keep_totals: bool,
    csv: bool,
    locale: Option<&str>,
    rules_only: bool,
    model_max_cells: Option<usize>,
) -> PyResult<Bound<'py, PyDict>> {
    let locale = match locale {
        Some(tag) => Some(
            hh_core::values::Locale::parse(tag).ok_or_else(|| PyValueError::new_err(format!("unknown locale {tag:?}")))?,
        ),
        None => None,
    };
    let opts = hh_core::normalize::Options { keep_totals, csv, locale, rules_only, model_max_cells };
    let (path, out_dir) = (path.to_string(), out_dir.to_string());
    let out = py
        .detach(move || hh_core::normalize_file(&path, &out_dir, &opts))
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    let d = PyDict::new(py);
    d.set_item("xlsx_path", out.xlsx_path.to_string_lossy())?;
    d.set_item("manifest_path", out.manifest_path.to_string_lossy())?;
    d.set_item("csv_paths", out.csv_paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>())?;
    d.set_item("manifest_json", out.manifest_json)?;
    Ok(d)
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Workbook>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(normalize, m)?)?;
    Ok(())
}
