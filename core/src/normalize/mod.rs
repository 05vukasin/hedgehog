//! Normalized output: turn a messy workbook into one clean, typed sheet per detected table plus a
//! JSON manifest, so another agent can load the data with pandas / openpyxl and trace every value
//! back to the original cell. See `docs/NORMALIZE.md` for the file layout and manifest schema.
//!
//! Pipeline, once per sheet (each result computed once and reused):
//! [`Grid`] → [`layout::analyze`] (tables, cell roles) → [`totals::detect`] + [`totals::verify`] →
//! per table: flat header, typed columns ([`values`] parsers with per-column locale inference),
//! group-header rows → a column, total rows → `Totals` sheet, everything else → `Notes` sheet.
//! Then [`write`] produces `<stem>.normalized.xlsx` (+ optional CSVs) and `<stem>.manifest.json`.
//!
//! The source file is only read, never modified.

mod coverage;
mod extract;
mod headers;
pub mod json;
mod manifest;
pub mod sha256;
mod typing;
mod write;

use std::path::{Path, PathBuf};

use crate::model::{Rect, Workbook};
use crate::values::{DateOrder, DecimalSep, Locale};

pub use manifest::SCHEMA_VERSION;
/// Shortest decimal text of a number and ISO text of an Excel serial date, as the manifest writes them.
pub use typing::{fmt_num, iso};

/// Options for [`normalize_file`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Keep total / subtotal rows in the table sheets (with a `_row_type` column) instead of
    /// moving them to the `Totals` sheet only.
    pub keep_totals: bool,
    /// Also write one UTF-8 CSV per table.
    pub csv: bool,
    /// Explicit locale for every column (overrides per-column inference of decimal separator and
    /// day/month order).
    pub locale: Option<Locale>,
    /// Cell roles from the layout rules only, without the learned row model ([`crate::learned`]).
    pub rules_only: bool,
    /// Sheets with more cells than this use the rules alone (the model's features take about
    /// 440 bytes per cell). None: [`DEFAULT_MODEL_MAX_CELLS`].
    pub model_max_cells: Option<usize>,
}

/// Default for [`Options::model_max_cells`].
pub const DEFAULT_MODEL_MAX_CELLS: usize = 1_000_000;

/// One output cell value.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Null,
    Num(f64),
    /// Excel serial date (1900 system).
    Date(f64),
    Bool(bool),
    Text(String),
}

/// Column data type in the normalized output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dtype {
    Integer,
    Number,
    Percent,
    Currency,
    Date,
    Datetime,
    Text,
    Bool,
    Empty,
}

impl Dtype {
    pub fn as_str(self) -> &'static str {
        match self {
            Dtype::Integer => "integer",
            Dtype::Number => "number",
            Dtype::Percent => "percent",
            Dtype::Currency => "currency",
            Dtype::Date => "date",
            Dtype::Datetime => "datetime",
            Dtype::Text => "text",
            Dtype::Bool => "bool",
            Dtype::Empty => "empty",
        }
    }

    /// Excel number format written for the column (None = General).
    pub fn num_format(self) -> Option<&'static str> {
        match self {
            Dtype::Percent => Some("0.00%"),
            Dtype::Currency => Some("#,##0.00"),
            Dtype::Date => Some("yyyy-mm-dd"),
            Dtype::Datetime => Some("yyyy-mm-dd hh:mm:ss"),
            _ => None,
        }
    }
}

/// What a column of a normalized table holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColRole {
    /// A column of the source table.
    Data,
    /// Labels of group-header rows ("Region: North" → `Region` = `North`).
    Group,
    /// `_src_row`: 1-based row number in the source sheet.
    SrcRow,
    /// `_row_type`: data | subtotal | total (only with `keep_totals`).
    RowType,
}

impl ColRole {
    pub fn as_str(self) -> &'static str {
        match self {
            ColRole::Data => "data",
            ColRole::Group => "group",
            ColRole::SrcRow => "source_row",
            ColRole::RowType => "row_type",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Flat, unique, non-empty header.
    pub name: String,
    pub role: ColRole,
    /// 0-based source sheet column (Data columns only).
    pub source_col: Option<u32>,
    /// Header cell texts top to bottom with their (row, col) in the source sheet.
    pub original_header: Vec<(String, (u32, u32))>,
    pub dtype: Dtype,
    pub unit: Option<String>,
    pub currency: Option<String>,
    /// Decimal separator / day-month order used to read text values (None: no evidence).
    pub decimal: Option<DecimalSep>,
    pub date_order: Option<DateOrder>,
    /// Values whose separator or day/month order was a guess.
    pub ambiguous: usize,
    /// Footnote marks stripped from numbers in this column (`¹`, `(r)`, `p`, `e`), each with the
    /// source cells (0-based row, col) that carried it, in mark order.
    pub footnote_marks: Vec<(String, Vec<(u32, u32)>)>,
    pub values: Vec<Out>,
}

/// Row kind inside a normalized table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowType {
    Data,
    Subtotal,
    Total,
}

impl RowType {
    pub fn as_str(self) -> &'static str {
        match self {
            RowType::Data => "data",
            RowType::Subtotal => "subtotal",
            RowType::Total => "total",
        }
    }
}

/// Counted transformations, reported per table in the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T {
    NumbersFromText,
    PercentFromText,
    CurrencyStripped,
    UnitStripped,
    ScaleApplied,
    AccountingNegatives,
    FootnoteMarksStripped,
    DatesFromText,
    DatesFromSerialNumbers,
    BoolsFromText,
    MissingToNull,
    ErrorsToNull,
    FormulasWithoutValue,
    FormulasReplacedByValues,
    WhitespaceTrimmed,
    ValuesToText,
    UnparsedKeptAsText,
    TextTruncated,
    HeadersFlattened,
    HeadersCleaned,
    HeaderCellsForwardFilled,
    MergedCellsFilled,
    DuplicateHeadersRenamed,
    MissingHeadersNamed,
    TotalRowsRemoved,
    TotalRowsKept,
    RepeatedHeaderRowsRemoved,
    BlankRowsRemoved,
    GroupHeaderRowsToColumn,
    GroupKeysFilledDown,
    NoteRowsMoved,
    TitleRowsMoved,
    EmptyColumnsRemoved,
    RowLabelsCopied,
}

impl T {
    pub const ALL: [T; 34] = [
        T::NumbersFromText,
        T::PercentFromText,
        T::CurrencyStripped,
        T::UnitStripped,
        T::ScaleApplied,
        T::AccountingNegatives,
        T::FootnoteMarksStripped,
        T::DatesFromText,
        T::DatesFromSerialNumbers,
        T::BoolsFromText,
        T::MissingToNull,
        T::ErrorsToNull,
        T::FormulasWithoutValue,
        T::FormulasReplacedByValues,
        T::WhitespaceTrimmed,
        T::ValuesToText,
        T::UnparsedKeptAsText,
        T::TextTruncated,
        T::HeadersFlattened,
        T::HeadersCleaned,
        T::HeaderCellsForwardFilled,
        T::MergedCellsFilled,
        T::DuplicateHeadersRenamed,
        T::MissingHeadersNamed,
        T::TotalRowsRemoved,
        T::TotalRowsKept,
        T::RepeatedHeaderRowsRemoved,
        T::BlankRowsRemoved,
        T::GroupHeaderRowsToColumn,
        T::GroupKeysFilledDown,
        T::NoteRowsMoved,
        T::TitleRowsMoved,
        T::EmptyColumnsRemoved,
        T::RowLabelsCopied,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            T::NumbersFromText => "numbers_from_text",
            T::PercentFromText => "percent_from_text",
            T::CurrencyStripped => "currency_stripped",
            T::UnitStripped => "unit_stripped",
            T::ScaleApplied => "scale_applied",
            T::AccountingNegatives => "accounting_negatives",
            T::FootnoteMarksStripped => "footnote_marks_stripped",
            T::DatesFromText => "dates_from_text",
            T::DatesFromSerialNumbers => "dates_from_serial_numbers",
            T::BoolsFromText => "bools_from_text",
            T::MissingToNull => "missing_to_null",
            T::ErrorsToNull => "errors_to_null",
            T::FormulasWithoutValue => "formulas_without_value",
            T::FormulasReplacedByValues => "formulas_replaced_by_values",
            T::WhitespaceTrimmed => "whitespace_trimmed",
            T::ValuesToText => "values_to_text",
            T::UnparsedKeptAsText => "unparsed_kept_as_text",
            T::TextTruncated => "text_truncated",
            T::HeadersFlattened => "headers_flattened",
            T::HeadersCleaned => "headers_cleaned",
            T::HeaderCellsForwardFilled => "header_cells_forward_filled",
            T::MergedCellsFilled => "merged_cells_filled",
            T::DuplicateHeadersRenamed => "duplicate_headers_renamed",
            T::MissingHeadersNamed => "missing_headers_named",
            T::TotalRowsRemoved => "total_rows_removed",
            T::TotalRowsKept => "total_rows_kept",
            T::RepeatedHeaderRowsRemoved => "repeated_header_rows_removed",
            T::BlankRowsRemoved => "blank_rows_removed",
            T::GroupHeaderRowsToColumn => "group_header_rows_to_column",
            T::GroupKeysFilledDown => "group_keys_filled_down",
            T::NoteRowsMoved => "note_rows_moved",
            T::TitleRowsMoved => "title_rows_moved",
            T::EmptyColumnsRemoved => "empty_columns_removed",
            T::RowLabelsCopied => "row_labels_copied",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transforms(pub [usize; T::ALL.len()]);

impl Default for Transforms {
    fn default() -> Self {
        Transforms([0; T::ALL.len()])
    }
}

impl Transforms {
    pub fn add(&mut self, t: T, n: usize) {
        self.0[t as usize] += n;
    }
    pub fn get(&self, t: T) -> usize {
        self.0[t as usize]
    }
    pub fn merge(&mut self, o: &Transforms) {
        for (a, b) in self.0.iter_mut().zip(o.0) {
            *a += b;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// 1-based table number; the table id is `T{n}`.
    pub n: usize,
    /// Output sheet name = Excel table (ListObject) name: `T{n}_{slug}`.
    pub sheet: String,
    pub source_sheet: String,
    pub rect: Rect,
    pub header_top: u32,
    pub header_rows: u32,
    pub title: Option<(String, (u32, u32))>,
    pub notes: Vec<(String, (u32, u32))>,
    pub confidence: f32,
    /// Output columns in order (group, data…, `_src_row`, `_row_type`).
    pub columns: Vec<Column>,
    /// 0-based source row of each output row.
    pub src_rows: Vec<u32>,
    pub row_types: Vec<RowType>,
    pub transforms: Transforms,
    pub warnings: Vec<String>,
}

impl TotalItem {
    /// `B13`, or `AB6:AB10494` for a run.
    pub fn cell_a1(&self) -> String {
        if self.row_end > self.row {
            format!("{}:{}", crate::model::a1(self.row, self.col), crate::model::a1(self.row_end, self.col))
        } else {
            crate::model::a1(self.row, self.col)
        }
    }
}

impl Table {
    pub fn id(&self) -> String {
        format!("T{}", self.n)
    }
    pub fn rows(&self) -> usize {
        self.src_rows.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TotalItem {
    pub table: Option<usize>,
    pub sheet: String,
    pub row: u32,
    pub col: u32,
    /// Number of totals this item stands for: 1, or a run of row totals down one column that all
    /// check out (rows `row..=row_end`; `value` / `recomputed` are then None and `sources` /
    /// `formula` are those of the first one).
    pub count: usize,
    pub row_end: u32,
    pub label: Option<String>,
    pub label_cell: Option<(u32, u32)>,
    /// total | subtotal | grand_total
    pub level: &'static str,
    /// vertical | horizontal
    pub direction: &'static str,
    /// formula | typed | layout
    pub origin: &'static str,
    pub value: Option<f64>,
    pub recomputed: Option<f64>,
    /// ok | mismatch | unknown
    pub status: &'static str,
    pub reason: String,
    pub sources: Vec<String>,
    pub formula: Option<String>,
    /// The total's row is still part of its table's data (`keep_totals`, or a total column).
    pub kept_in_data: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteItem {
    pub sheet: String,
    pub row: u32,
    pub col: u32,
    /// title | note | repeated_header | total_row | other
    pub kind: &'static str,
    pub table: Option<usize>,
    pub text: String,
    pub formula: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SheetInfo {
    pub name: String,
    pub used_range: Option<Rect>,
    pub cells: usize,
    pub formulas: usize,
    pub merges: usize,
    pub tables: Vec<usize>,
}

/// Where the non-empty source cells ended up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CellCounts {
    pub total: usize,
    pub in_tables: usize,
    pub in_totals: usize,
    pub in_notes: usize,
}

/// Everything the writers need; built by [`normalize_workbook`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Normalized {
    pub sheets: Vec<SheetInfo>,
    pub tables: Vec<Table>,
    pub totals: Vec<TotalItem>,
    pub notes: Vec<NoteItem>,
    pub cells: CellCounts,
    pub warnings: Vec<String>,
}

/// Time coverage of a table or workbook, as the manifest's `time_coverage` writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeCoverage {
    /// First period: `2020`, `2020-Q1`, `2020-01`, `2020-W05` or `2020-01-31`.
    pub from: String,
    pub to: String,
    /// day | week | month | quarter | year (the finest found).
    pub granularity: &'static str,
    /// `column` (values of date / period columns) or `header` (period-like column headers).
    pub source: &'static str,
    /// Column names (workbook: `T<n>.<name>`).
    pub columns: Vec<String>,
    /// Table ids the coverage comes from.
    pub tables: Vec<String>,
}

/// Time coverage of one table (the manifest's `tables[].time_coverage`).
pub fn table_time_coverage(t: &Table) -> Option<TimeCoverage> {
    coverage::table(t).map(|c| c.public())
}

/// The period a column header names (`2025`, `Jan 2024`, `2024 Q1`, `Oktober / 2022`,
/// `Thu / 2002-01-31`), as the time coverage reads headers: its label (`2025`, `2024-01`,
/// `2024-Q1`, `2022-10`, `2002-01-31`) and granularity (`year`, `quarter`, `month`, `week`, `day`).
pub fn header_period(name: &str) -> Option<(String, &'static str)> {
    coverage::header_period_label(name)
}

/// Time coverage of the workbook (the manifest's top-level `time_coverage`).
pub fn workbook_time_coverage(norm: &Normalized) -> Option<TimeCoverage> {
    let with_cov: Vec<(String, coverage::Coverage)> =
        norm.tables.iter().filter_map(|t| coverage::table(t).map(|c| (t.id(), c))).collect();
    coverage::workbook(&with_cov).map(|c| c.public())
}

/// Time coverage of every table and of the workbook in one pass (each table's columns read once).
pub fn time_coverages(norm: &Normalized) -> (Vec<Option<TimeCoverage>>, Option<TimeCoverage>) {
    let per: Vec<Option<coverage::Coverage>> = norm.tables.iter().map(coverage::table).collect();
    let with_cov: Vec<(String, coverage::Coverage)> =
        norm.tables.iter().zip(&per).filter_map(|(t, c)| c.clone().map(|c| (t.id(), c))).collect();
    let wb = coverage::workbook(&with_cov).map(|c| c.public());
    (per.into_iter().map(|c| c.map(|c| c.public())).collect(), wb)
}

/// One total row of a table (the manifest's `tables[].totals.rows[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableTotalRow {
    /// 1-based source row.
    pub row: u32,
    pub label: Option<String>,
    /// total | subtotal | grand_total
    pub level: &'static str,
    pub kept_in_data: bool,
}

/// A table's totals (the manifest's `tables[].totals`): counts by status and one entry per total
/// row (vertical totals), in source row order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableTotals {
    pub count: usize,
    pub ok: usize,
    pub mismatch: usize,
    pub unknown: usize,
    pub rows: Vec<TableTotalRow>,
}

/// The totals of table `t` among `totals` (usually [`Normalized::totals`]).
pub fn table_totals(t: &Table, totals: &[TotalItem]) -> TableTotals {
    let mut out = TableTotals::default();
    let mut rows: std::collections::BTreeMap<u32, TableTotalRow> = std::collections::BTreeMap::new();
    for x in totals.iter().filter(|x| x.table == Some(t.n)) {
        match x.status {
            "ok" => out.ok += x.count,
            "mismatch" => out.mismatch += x.count,
            _ => out.unknown += x.count,
        }
        if x.direction == "vertical" {
            let e = rows.entry(x.row).or_insert(TableTotalRow { row: x.row + 1, label: None, level: x.level, kept_in_data: false });
            if e.label.is_none() {
                e.label = x.label.clone();
            }
            e.kept_in_data |= x.kept_in_data;
        }
    }
    out.count = out.ok + out.mismatch + out.unknown;
    out.rows = rows.into_values().collect();
    out
}

/// Files written by [`normalize_file`].
#[derive(Debug, Clone)]
pub struct Output {
    pub xlsx_path: PathBuf,
    pub manifest_path: PathBuf,
    pub csv_paths: Vec<PathBuf>,
    /// The manifest as written (pretty JSON).
    pub manifest_json: String,
    pub tables: usize,
    pub cells: CellCounts,
    pub transforms: Transforms,
}

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Analyzes every sheet and builds the normalized tables, totals and notes (no I/O).
pub fn normalize_workbook(wb: &Workbook, opts: &Options) -> Normalized {
    let mut out = Normalized::default();
    let layouts = layouts(wb, opts);
    for (sheet, lay) in wb.sheets.iter().zip(layouts) {
        extract::process_sheet(sheet, opts, lay, &mut out);
    }
    out
}

/// Workbooks whose model sheets hold at most this many cells in total analyze their sheets on
/// parallel threads (the model's features take memory per cell; larger workbooks go one sheet at
/// a time, so peak memory is set by the largest sheet).
const PARALLEL_SHEETS_MAX_CELLS: usize = 300_000;

/// A sheet's layout; when the model was skipped for its size, a warning; and the sheet's total
/// detection if the layout already ran it (so the extraction does not run it again).
pub(super) type SheetAnalysis = (crate::analysis::SheetLayout, Option<String>, Option<crate::totals::Detection>);

/// Cell roles and tables of every sheet: the rules plus the learned row model for sheets within
/// [`Options::model_max_cells`], the rules alone otherwise (or with `rules_only`). The second
/// value is a warning for sheets above the model limit.
fn layouts(wb: &Workbook, opts: &Options) -> Vec<SheetAnalysis> {
    let limit = opts.model_max_cells.unwrap_or(DEFAULT_MODEL_MAX_CELLS);
    let model = crate::learned::embedded().filter(|_| !opts.rules_only);
    let one = |sheet: &crate::model::Sheet| {
        let grid = crate::grid::Grid::new(sheet);
        let n = sheet.cells.len();
        let (lay, warning) = match model {
            Some(m) if n <= limit => (crate::learned::analyze(&grid, m), None),
            Some(_) => (
                crate::layout::analyze(&grid),
                Some(format!("{}: {n} cells, more than the model limit of {limit}; cell roles from the rules alone", sheet.name)),
            ),
            None => (crate::layout::analyze(&grid), None),
        };
        (lay, warning, grid.into_totals())
    };
    let model_cells: usize = wb.sheets.iter().map(|s| s.cells.len()).filter(|&n| n <= limit).sum();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(4).min(wb.sheets.len());
    if model.is_none() || threads <= 1 || model_cells > PARALLEL_SHEETS_MAX_CELLS {
        return wb.sheets.iter().map(one).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut slots: Vec<Option<SheetAnalysis>> = vec![None; wb.sheets.len()];
    let done: Vec<Vec<(usize, SheetAnalysis)>> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(sheet) = wb.sheets.get(i) else { break };
                        mine.push((i, one(sheet)));
                    }
                    mine
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
    });
    for (i, r) in done.into_iter().flatten() {
        slots[i] = Some(r);
    }
    slots.into_iter().map(|s| s.expect("every sheet analyzed")).collect()
}

/// Reads `path`, writes `<stem>.normalized.xlsx`, `<stem>.manifest.json` (and CSVs with
/// `opts.csv`) into `out_dir` (created if missing). The source file is never modified.
pub fn normalize_file(path: impl AsRef<Path>, out_dir: impl AsRef<Path>, opts: &Options) -> Result<Output, Error> {
    let path = path.as_ref();
    let out_dir = out_dir.as_ref();
    let bytes = std::fs::read(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    let sha = sha256::hex(&bytes);
    let size = bytes.len();
    drop(bytes);
    let t0 = std::time::Instant::now();
    let wb = crate::reader::open(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    trace("open", t0);
    let norm = normalize_workbook(&wb, opts);
    trace("analyze", t0);
    drop(wb);
    std::fs::create_dir_all(out_dir).map_err(|e| Error(format!("{}: {e}", out_dir.display())))?;
    let file_name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = path.file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "workbook".into());
    let xlsx_name = format!("{stem}.normalized.xlsx");
    let manifest_name = format!("{stem}.manifest.json");
    let xlsx_path = out_dir.join(&xlsx_name);
    let manifest_path = out_dir.join(&manifest_name);
    if same_file(path, &xlsx_path) || same_file(path, &manifest_path) {
        return Err(Error("output would overwrite the source file".into()));
    }
    let src = manifest::Source { path: path.display().to_string(), file_name, sha256: sha, size };
    write::xlsx(&norm, &src, &xlsx_name, &manifest_name, &xlsx_path)?;
    trace("write_xlsx", t0);
    let mut csv_paths = Vec::new();
    if opts.csv {
        for t in &norm.tables {
            let p = out_dir.join(format!("{stem}.{}.csv", t.sheet));
            write::csv(t, &p)?;
            csv_paths.push(p);
        }
    }
    let csv_names: Vec<String> =
        csv_paths.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
    let manifest_json = manifest::build(&norm, &src, opts, &xlsx_name, &manifest_name, &csv_names).pretty();
    std::fs::write(&manifest_path, &manifest_json).map_err(|e| Error(format!("{}: {e}", manifest_path.display())))?;
    let mut transforms = Transforms::default();
    for t in &norm.tables {
        transforms.merge(&t.transforms);
    }
    Ok(Output {
        xlsx_path,
        manifest_path,
        csv_paths,
        manifest_json,
        tables: norm.tables.len(),
        cells: norm.cells,
        transforms,
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Development aid: with `HEDGEHOG_TRACE=1`, prints elapsed time and peak memory (Linux) per stage
/// to stderr.
pub(crate) fn trace(stage: &str, since: std::time::Instant) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("HEDGEHOG_TRACE").is_some_and(|v| !v.is_empty() && v != "0")) {
        return;
    }
    let peak = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).map(|l| l[6..].trim().to_string()))
        .unwrap_or_default();
    eprintln!("[trace] {stage:<16} {:>9.1} ms  peak {peak}", since.elapsed().as_secs_f64() * 1000.0);
}
