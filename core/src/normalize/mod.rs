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

mod extract;
pub mod json;
mod manifest;
pub mod sha256;
mod typing;
mod write;

use std::path::{Path, PathBuf};

use crate::model::{Rect, Workbook};
use crate::values::{DateOrder, DecimalSep, Locale};

pub use manifest::SCHEMA_VERSION;

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
}

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
}

impl T {
    pub const ALL: [T; 31] = [
        T::NumbersFromText,
        T::PercentFromText,
        T::CurrencyStripped,
        T::UnitStripped,
        T::ScaleApplied,
        T::AccountingNegatives,
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
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            T::NumbersFromText => "numbers_from_text",
            T::PercentFromText => "percent_from_text",
            T::CurrencyStripped => "currency_stripped",
            T::UnitStripped => "unit_stripped",
            T::ScaleApplied => "scale_applied",
            T::AccountingNegatives => "accounting_negatives",
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Transforms(pub [usize; T::ALL.len()]);

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
    for sheet in &wb.sheets {
        extract::process_sheet(sheet, opts, &mut out);
    }
    out
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
    let wb = crate::reader::open(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    let norm = normalize_workbook(&wb, opts);
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
    Ok(Output { xlsx_path, manifest_path, csv_paths, manifest_json, tables: norm.tables.len(), cells: norm.cells, transforms })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}
