//! Node.js bindings (napi-rs). Kept thin, like the Python bindings in `py/`: all logic lives in
//! `hh_core`; these functions only look things up and convert to JS objects. Keys are camelCase
//! (`usedRange`, `labelAddress`); enum-like string values stay snake_case as in Python and the
//! manifest (`grand_total`, `formula:sum`).
//!
//! Slow work (`normalize`, `openAsync`) runs on the libuv thread pool and returns a Promise, so
//! the JS event loop keeps running.

#![deny(clippy::all)]

use hh_core::totals::{self, Direction, Evidence, RefKind, TotalLevel, TotalOrigin};
use hh_core::{a1, Cell, Grid, Rect, Sheet, Value};
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi_derive::napi;

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

fn num_json(n: f64) -> serde_json::Value {
    serde_json::Number::from_f64(n).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

/// Plain JS value: number, string, boolean, ISO date string, `#ERR` string, or null.
fn value_js(v: Option<&Value>) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        None => J::Null,
        Some(Value::Bool(b)) => J::Bool(*b),
        Some(Value::Number(n)) => num_json(*n),
        Some(Value::Text(s)) => J::String(s.clone()),
        Some(Value::DateTime(d)) => J::String(iso_date(*d)),
        Some(Value::Error(e)) => J::String(e.clone()),
    }
}

fn invalid(msg: String) -> Error {
    Error::new(Status::InvalidArg, msg)
}

/// One local reference parsed with the core formula parser: `D43`, `$D$43`, `A1:F20`, `Sheet!B2`.
fn parse_ref(text: &str, kinds: &[RefKind]) -> Result<(Option<String>, Rect)> {
    let parsed = totals::parse(text.trim().trim_start_matches('='));
    match parsed.refs.as_slice() {
        [r] if !r.external && kinds.contains(&r.kind) => Ok((r.sheet.clone(), r.rect)),
        _ => Err(invalid(format!("not a cell reference: {text:?}"))),
    }
}

/// A cell: `{sheet, address, value, type, formula, merged}`. An empty cell has value null and
/// type "empty"; `formula` (with a leading `=`) and `merged` (the merged range the cell belongs to)
/// are null when absent.
#[napi(object)]
pub struct CellInfo {
    pub sheet: String,
    pub address: String,
    #[napi(ts_type = "number | string | boolean | null")]
    pub value: serde_json::Value,
    #[napi(js_name = "type", ts_type = "'empty' | 'bool' | 'number' | 'text' | 'date' | 'error'")]
    pub kind: String,
    #[napi(ts_type = "string | null")]
    pub formula: serde_json::Value,
    #[napi(ts_type = "string | null")]
    pub merged: serde_json::Value,
}

fn opt_str(s: Option<String>) -> serde_json::Value {
    s.map_or(serde_json::Value::Null, serde_json::Value::String)
}

fn cell_info(sheet: &Sheet, row: u32, col: u32, cell: Option<&Cell>) -> CellInfo {
    let v = cell.and_then(|c| c.value.as_ref());
    let merged = sheet.merges.iter().find(|m| m.top <= row && row <= m.bottom && m.left <= col && col <= m.right);
    CellInfo {
        sheet: sheet.name.clone(),
        address: a1(row, col),
        value: value_js(v),
        kind: value_type(v).to_string(),
        formula: opt_str(cell.and_then(|c| c.formula.as_ref()).map(|f| format!("={f}"))),
        merged: opt_str(merged.map(|m| range_a1(*m))),
    }
}

/// One sheet: name, used range (`A1:F20` or null), rows, cols, cells, formulas, merges.
#[napi(object)]
pub struct SheetInfo {
    pub name: String,
    #[napi(ts_type = "string | null")]
    pub used_range: serde_json::Value,
    pub rows: u32,
    pub cols: u32,
    pub cells: u32,
    pub formulas: u32,
    pub merges: u32,
}

/// Values of a rectangular range, row by row (null for empty cells).
#[napi(object)]
pub struct RangeValues {
    pub sheet: String,
    pub range: String,
    #[napi(ts_type = "Array<Array<number | string | boolean | null>>")]
    pub rows: serde_json::Value,
}

/// Options of `Workbook.findText`.
#[napi(object)]
pub struct FindTextOptions {
    /// Search only this sheet (default: all sheets).
    pub sheet: Option<String>,
    /// Maximum hits (default 50).
    pub limit: Option<u32>,
}

/// A detected total with its verification (`hh_core::totals::{detect, verify}`).
#[napi(object)]
pub struct TotalInfo {
    pub sheet: String,
    pub address: String,
    #[napi(ts_type = "'formula' | 'typed'")]
    pub origin: String,
    #[napi(ts_type = "'total' | 'subtotal' | 'grand_total'")]
    pub level: String,
    #[napi(ts_type = "'vertical' | 'horizontal'")]
    pub direction: String,
    /// Summed ranges (`B2:B9`).
    pub sources: Vec<String>,
    pub excluded: Vec<String>,
    pub double_counted: Vec<String>,
    #[napi(ts_type = "string | null")]
    pub label_address: serde_json::Value,
    #[napi(ts_type = "number | string | boolean | null")]
    pub label: serde_json::Value,
    #[napi(ts_type = "string | null")]
    pub formula: serde_json::Value,
    /// `formula:sum`, `keyword:total`, `group_label:A5`, ...
    pub evidence: Vec<String>,
    /// Value in the total cell.
    #[napi(ts_type = "number | null")]
    pub actual: serde_json::Value,
    /// Recomputed sum of the sources.
    #[napi(ts_type = "number | null")]
    pub expected: serde_json::Value,
    #[napi(ts_type = "number | null")]
    pub diff: serde_json::Value,
    /// true / false, or null when it could not be checked.
    #[napi(ts_type = "boolean | null")]
    pub ok: serde_json::Value,
    pub reason: String,
    /// Number of source values summed.
    pub summed: u32,
    pub numbers_as_text: Vec<String>,
}

fn opt_num(n: Option<f64>) -> serde_json::Value {
    n.map_or(serde_json::Value::Null, num_json)
}

fn cells_a1(v: &[(u32, u32)]) -> Vec<String> {
    v.iter().map(|&(r, c)| a1(r, c)).collect()
}

fn sheet_totals(s: &Sheet, out: &mut Vec<TotalInfo>) {
    let grid = Grid::new(s);
    for cand in totals::detect(&grid) {
        let check = totals::verify(&grid, &cand);
        let label_cell = cand.label.and_then(|(r, c)| grid.cell(r, c));
        out.push(TotalInfo {
            sheet: s.name.clone(),
            address: a1(cand.row, cand.col),
            origin: match cand.origin {
                TotalOrigin::Formula => "formula",
                TotalOrigin::Typed => "typed",
            }
            .into(),
            level: match cand.level {
                TotalLevel::Total => "total",
                TotalLevel::Subtotal => "subtotal",
                TotalLevel::GrandTotal => "grand_total",
            }
            .into(),
            direction: match cand.direction {
                Direction::Vertical => "vertical",
                Direction::Horizontal => "horizontal",
            }
            .into(),
            sources: cand.ranges.iter().map(|r| range_a1(*r)).collect(),
            excluded: cells_a1(&cand.exclude),
            double_counted: cells_a1(&cand.double_counted),
            label_address: opt_str(cand.label.map(|(r, c)| a1(r, c))),
            label: value_js(label_cell.and_then(|c| c.value.as_ref())),
            formula: opt_str(grid.cell(cand.row, cand.col).and_then(|c| c.formula.as_ref()).map(|f| format!("={f}"))),
            evidence: cand
                .evidence
                .iter()
                .map(|e| match e {
                    Evidence::Formula(k) => format!("formula:{}", snake(k)),
                    Evidence::Keyword(k) => format!("keyword:{}", snake(k)),
                    Evidence::GroupLabel(r, c) => format!("group_label:{}", a1(*r, *c)),
                    other => snake(other),
                })
                .collect(),
            actual: opt_num(check.actual),
            expected: opt_num(check.expected),
            diff: opt_num(check.diff),
            ok: check.ok.map_or(serde_json::Value::Null, serde_json::Value::Bool),
            reason: snake(check.reason),
            summed: check.summed as u32,
            numbers_as_text: cells_a1(&check.numbers_as_text),
        });
    }
}

/// A workbook read into memory: xlsx / xlsm / xlsb / xls / ods, or a CSV / TSV file.
#[napi]
pub struct Workbook {
    inner: hh_core::Workbook,
}

impl Workbook {
    fn names(&self) -> Vec<String> {
        self.inner.sheets.iter().map(|s| s.name.clone()).collect()
    }

    fn sheet(&self, name: &str) -> Result<&Sheet> {
        self.inner
            .sheets
            .iter()
            .find(|s| s.name == name)
            .or_else(|| self.inner.sheets.iter().find(|s| s.name.eq_ignore_ascii_case(name)))
            .ok_or_else(|| invalid(format!("no sheet named {name:?}; sheets: {:?}", self.names())))
    }

    /// Sheet named in the reference wins over the `sheet` argument; with neither, the first sheet.
    fn sheet_for(&self, sheet: Option<&str>, in_ref: Option<String>) -> Result<&Sheet> {
        match in_ref.as_deref().or(sheet) {
            Some(n) => self.sheet(n),
            None => self.inner.sheets.first().ok_or_else(|| invalid("workbook has no sheets".into())),
        }
    }
}

#[napi]
impl Workbook {
    /// Sheet names in workbook order.
    #[napi(getter)]
    pub fn sheet_names(&self) -> Vec<String> {
        self.names()
    }

    /// Short plain-text overview, one line per sheet.
    #[napi]
    pub fn describe(&self) -> String {
        hh_core::describe(&self.inner)
    }

    /// One object per sheet: name, usedRange, rows, cols, cells, formulas, merges.
    #[napi]
    pub fn sheets(&self) -> Vec<SheetInfo> {
        self.inner
            .sheets
            .iter()
            .map(|s| {
                let used = s.used_range();
                SheetInfo {
                    name: s.name.clone(),
                    used_range: opt_str(used.map(range_a1)),
                    rows: used.map_or(0, |r| r.bottom - r.top + 1),
                    cols: used.map_or(0, |r| r.right - r.left + 1),
                    cells: s.cells.len() as u32,
                    formulas: s.formula_count() as u32,
                    merges: s.merges.len() as u32,
                }
            })
            .collect()
    }

    /// `getCell("Sheet1", "D43")` or `getCell(null, "Sheet1!D43")`. With neither a sheet argument
    /// nor a sheet in the reference, the first sheet.
    #[napi]
    pub fn get_cell(&self, sheet: Option<String>, address: String) -> Result<CellInfo> {
        let (in_ref, rect) = parse_ref(&address, &[RefKind::Cell])?;
        let s = self.sheet_for(sheet.as_deref(), in_ref)?;
        let grid = Grid::new(s);
        Ok(cell_info(s, rect.top, rect.left, grid.cell(rect.top, rect.left)))
    }

    /// Values of a rectangular range (`"A1:F20"` or `"Sheet1!A1:F20"`), row by row. Ranges larger
    /// than `maxCells` (default 5000) are rejected.
    #[napi]
    pub fn get_range(&self, sheet: Option<String>, range: String, max_cells: Option<u32>) -> Result<RangeValues> {
        let max_cells = max_cells.unwrap_or(5000) as u64;
        let (in_ref, rect) = parse_ref(&range, &[RefKind::Cell, RefKind::Range])?;
        let s = self.sheet_for(sheet.as_deref(), in_ref)?;
        let area = (rect.bottom - rect.top + 1) as u64 * (rect.right - rect.left + 1) as u64;
        if area > max_cells {
            return Err(invalid(format!(
                "range {} has {area} cells, more than maxCells={max_cells}",
                range_a1(rect)
            )));
        }
        let grid = Grid::new(s);
        let rows = (rect.top..=rect.bottom)
            .map(|r| {
                serde_json::Value::Array(
                    (rect.left..=rect.right).map(|c| value_js(grid.cell(r, c).and_then(|x| x.value.as_ref()))).collect(),
                )
            })
            .collect();
        Ok(RangeValues { sheet: s.name.clone(), range: range_a1(rect), rows: serde_json::Value::Array(rows) })
    }

    /// Cells whose text contains `query` (case-, diacritic- and script-insensitive), on one sheet
    /// or all. Numbers match on their plain digits.
    #[napi]
    pub fn find_text(&self, query: String, opts: Option<FindTextOptions>) -> Result<Vec<CellInfo>> {
        let (sheet, limit) = opts.map_or((None, 50), |o| (o.sheet, o.limit.unwrap_or(50) as usize));
        let key = hh_core::values::norm_key(&query);
        let mut out = Vec::new();
        if key.is_empty() {
            return Ok(out);
        }
        let sheets: Vec<&Sheet> = match sheet.as_deref() {
            Some(n) => vec![self.sheet(n)?],
            None => self.inner.sheets.iter().collect(),
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
                    out.push(cell_info(s, c.row, c.col, Some(c)));
                }
            }
        }
        Ok(out)
    }

    /// Detected totals with their verification, of one sheet or (without `sheet`) of every sheet.
    #[napi]
    pub fn totals(&self, sheet: Option<String>) -> Result<Vec<TotalInfo>> {
        let mut out = Vec::new();
        match sheet.as_deref() {
            Some(n) => sheet_totals(self.sheet(n)?, &mut out),
            None => self.inner.sheets.iter().for_each(|s| sheet_totals(s, &mut out)),
        }
        Ok(out)
    }

    /// `Workbook(sheets=[...])`, like Python's repr.
    #[napi(js_name = "toString")]
    pub fn repr(&self) -> String {
        format!("Workbook(sheets={:?})", self.names())
    }
}

fn read(path: &str) -> Result<hh_core::Workbook> {
    // The reader's own message for a missing file does not name it.
    std::fs::metadata(path).map_err(|e| invalid(format!("{path}: {e}")))?;
    hh_core::open(path).map_err(|e| invalid(format!("{path}: {e}")))
}

/// Reads a workbook (format detected from content) on the calling thread.
#[napi]
pub fn open(path: String) -> Result<Workbook> {
    read(&path).map(|inner| Workbook { inner })
}

pub struct OpenTask {
    path: String,
}

impl Task for OpenTask {
    type Output = hh_core::Workbook;
    type JsValue = Workbook;

    fn compute(&mut self) -> Result<Self::Output> {
        read(&self.path)
    }

    fn resolve(&mut self, _env: Env, inner: Self::Output) -> Result<Self::JsValue> {
        Ok(Workbook { inner })
    }
}

/// Like `open`, read off the main thread.
#[napi(ts_return_type = "Promise<Workbook>")]
pub fn open_async(path: String) -> AsyncTask<OpenTask> {
    AsyncTask::new(OpenTask { path })
}

/// Options of `normalize` (all optional).
#[napi(object)]
#[derive(Default)]
pub struct NormalizeOptions {
    /// Keep total / subtotal rows in the table sheets, marked in a `_row_type` column.
    pub keep_totals: Option<bool>,
    /// Also write one UTF-8 CSV per table.
    pub csv: Option<bool>,
    /// e.g. "de-DE", "en-US", "sr": overrides per-column inference of decimal separator and
    /// day/month order.
    pub locale: Option<String>,
    /// Label cells with the layout rules alone, without the built-in learned model.
    pub rules_only: Option<bool>,
    /// Sheets with more cells use the rules alone (default 1,000,000).
    pub model_max_cells: Option<u32>,
}

/// Paths written by `normalize` and the manifest text (`index.js` parses it into `manifest`, which
/// keeps the file's key order).
#[napi(object)]
pub struct NormalizeResult {
    pub xlsx_path: String,
    pub manifest_path: String,
    pub csv_paths: Vec<String>,
    /// The manifest (`<stem>.manifest.json`) as written; see docs/normalized-output.md.
    pub manifest_json: String,
}

fn core_options(o: Option<NormalizeOptions>) -> Result<hh_core::NormalizeOptions> {
    let o = o.unwrap_or_default();
    let locale = match o.locale.as_deref() {
        Some(tag) => {
            Some(hh_core::values::Locale::parse(tag).ok_or_else(|| invalid(format!("unknown locale {tag:?}")))?)
        }
        None => None,
    };
    Ok(hh_core::NormalizeOptions {
        keep_totals: o.keep_totals.unwrap_or(false),
        csv: o.csv.unwrap_or(false),
        locale,
        rules_only: o.rules_only.unwrap_or(false),
        model_max_cells: o.model_max_cells.map(|n| n as usize),
        deadline: None,
    })
}

fn run_normalize(path: &str, out_dir: &str, opts: &hh_core::NormalizeOptions) -> Result<NormalizeResult> {
    let out = hh_core::normalize_file(path, out_dir, opts).map_err(|e| Error::new(Status::GenericFailure, e.0))?;
    let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
    Ok(NormalizeResult {
        xlsx_path: s(&out.xlsx_path),
        manifest_path: s(&out.manifest_path),
        csv_paths: out.csv_paths.iter().map(|p| s(p)).collect(),
        manifest_json: out.manifest_json,
    })
}

pub struct NormalizeTask {
    path: String,
    out_dir: String,
    opts: Option<NormalizeOptions>,
}

impl Task for NormalizeTask {
    type Output = NormalizeResult;
    type JsValue = NormalizeResult;

    fn compute(&mut self) -> Result<Self::Output> {
        run_normalize(&self.path, &self.out_dir, &core_options(self.opts.take())?)
    }

    fn resolve(&mut self, _env: Env, out: Self::Output) -> Result<Self::JsValue> {
        Ok(out)
    }
}

/// Writes `<stem>.normalized.xlsx` + `<stem>.manifest.json` (+ CSVs) into `outDir` (created if
/// missing) on the libuv thread pool. The source file is never modified. Errors (missing file,
/// unknown locale) reject the Promise.
#[napi(ts_return_type = "Promise<NormalizeResult>")]
pub fn normalize(path: String, out_dir: String, opts: Option<NormalizeOptions>) -> AsyncTask<NormalizeTask> {
    AsyncTask::new(NormalizeTask { path, out_dir, opts })
}

/// `normalize` on the calling thread (blocks the event loop; for scripts and tests).
#[napi]
pub fn normalize_sync(path: String, out_dir: String, opts: Option<NormalizeOptions>) -> Result<NormalizeResult> {
    run_normalize(&path, &out_dir, &core_options(opts)?)
}

/// hedgehog version (the Rust workspace version this addon was built from).
#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// -- agent tools ----------------------------------------------------------------------------------

type ToolFn = Box<dyn FnOnce(&mut hh_agent::Session) -> hh_agent::ToolResult + Send>;

/// One tool call on the libuv thread pool. Calls of the same session take turns on its mutex
/// (a session owns SQLite connections, which must not be used from two threads at once).
pub struct ToolTask {
    session: Arc<Mutex<hh_agent::Session>>,
    run: Option<ToolFn>,
}

fn lock(m: &Mutex<hh_agent::Session>) -> std::sync::MutexGuard<'_, hh_agent::Session> {
    // A panic inside a tool poisons the mutex; the session holds only caches, so keep going.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Task for ToolTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        let run = self.run.take().ok_or_else(|| Error::new(Status::GenericFailure, "tool task ran twice"))?;
        run(&mut lock(&self.session)).map_err(|e| Error::new(Status::GenericFailure, e.0))
    }

    fn resolve(&mut self, _env: Env, text: Self::Output) -> Result<Self::JsValue> {
        Ok(text)
    }
}

/// Options of the native agent-tools session.
#[napi(object)]
pub struct SessionOptions {
    /// Folder that bounds file access.
    pub root: String,
    /// Extra files or folders outside `root`.
    pub allow: Option<Vec<String>>,
}

/// The agent tools of `hedgehog mcp` in-process (`hh_agent::Session`). Wrapped by
/// `HedgehogSession` in index.js; every method resolves to the tool's compact text, and a tool
/// error rejects with its one-line message.
#[napi]
pub struct NativeSession {
    inner: Arc<Mutex<hh_agent::Session>>,
}

#[napi]
impl NativeSession {
    #[napi(constructor)]
    pub fn new(options: SessionOptions) -> Result<Self> {
        let allow: Vec<std::path::PathBuf> = options.allow.unwrap_or_default().into_iter().map(Into::into).collect();
        let session = hh_agent::Session::new(&options.root, &allow).map_err(|e| invalid(e.0))?;
        Ok(NativeSession { inner: Arc::new(Mutex::new(session)) })
    }

    fn task(&self, run: ToolFn) -> AsyncTask<ToolTask> {
        AsyncTask::new(ToolTask { session: self.inner.clone(), run: Some(run) })
    }

    /// The canonical root folder.
    #[napi(getter)]
    pub fn root(&self) -> String {
        lock(&self.inner).bounds().root().to_string_lossy().into_owned()
    }

    /// Allows one more file or folder outside the root.
    #[napi]
    pub fn allow(&self, path: String) -> Result<()> {
        lock(&self.inner).allow(&path).map_err(|e| invalid(e.0))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn open_file(&self, path: String) -> AsyncTask<ToolTask> {
        self.task(Box::new(move |s| s.open_file(&path)))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn describe_table(&self, path: String, table: String) -> AsyncTask<ToolTask> {
        self.task(Box::new(move |s| s.describe_table(&path, &table)))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn query(&self, path: String, sql: String, limit: Option<u32>) -> AsyncTask<ToolTask> {
        self.task(Box::new(move |s| s.query(&path, &sql, limit.map(|n| n as usize))))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn cells(&self, path: String, sheet: String, range: String) -> AsyncTask<ToolTask> {
        self.task(Box::new(move |s| s.cells(&path, &sheet, &range)))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn totals(&self, path: String, table: Option<String>, status: Option<String>) -> AsyncTask<ToolTask> {
        self.task(Box::new(move |s| s.totals(&path, table.as_deref(), status.as_deref())))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn list_files(&self, folder: Option<String>, pattern: Option<String>) -> AsyncTask<ToolTask> {
        self.task(Box::new(move |s| s.list_files(folder.as_deref(), pattern.as_deref())))
    }

    /// Runs a tool by its MCP name with JSON arguments (`{"path": ..., "sql": ...}`).
    #[napi(ts_return_type = "Promise<string>")]
    pub fn call(&self, name: String, args: Option<serde_json::Value>) -> AsyncTask<ToolTask> {
        let args = args.unwrap_or(serde_json::Value::Null);
        self.task(Box::new(move |s| s.call(&name, &args)))
    }
}

/// The tools as MCP `tools/list` entries: `{name, title, description, inputSchema, annotations}`.
#[napi(ts_return_type = "Array<ToolSpec>")]
pub fn tool_specs() -> serde_json::Value {
    hh_agent::tool_specs()
}

/// One entry of `toolSpecs()`.
#[napi(object)]
pub struct ToolSpec {
    pub name: String,
    pub title: String,
    pub description: String,
    #[napi(ts_type = "Record<string, any>")]
    pub input_schema: serde_json::Value,
    #[napi(ts_type = "Record<string, any>")]
    pub annotations: serde_json::Value,
}
