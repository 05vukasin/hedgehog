// SPDX-License-Identifier: MIT
//
// Copyright 2016-2026, Johann Tuffe.

//! A module to parse Open Document Spreadsheets
//!
/// # Reference
///
/// OASIS Open Document Format for Office Application 1.2 ([ODF 1.2]).
///
/// [ODF 1.2]: http://docs.oasis-open.org/office/v1.2/OpenDocument-v1.2.pdf
///
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Seek};

use log::warn;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::Reader as XmlReader;
use zip::read::ZipArchive;
use zip::result::ZipError;

use crate::attrs::{decode_attr, RawAttributes};
use crate::utils::unescape_entity_to_buffer;
use crate::vba::VbaProject;
use crate::{Data, DataType, HeaderRow, Metadata, Range, Reader, Sheet, SheetType, SheetVisible};
use std::marker::PhantomData;

const MIMETYPE: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";

/// Maximum number of rows allowed in an ODS file (matches XLSX limit).
const MAX_ROWS: u32 = 1_048_576;

/// Maximum number of columns allowed in an ODS file (matches XLSX limit).
const MAX_COLUMNS: u32 = 16_384;

/// Maximum number of cells to prevent memory exhaustion from malicious files.
const MAX_CELLS: usize = 100_000_000;


/// An enum for ods specific errors
#[derive(Debug)]
pub enum OdsError {
    /// Io error
    Io(std::io::Error),
    /// Zip error
    Zip(zip::result::ZipError),
    /// Xml error
    Xml(quick_xml::Error),
    /// Xml attribute error
    XmlAttr(quick_xml::events::attributes::AttrError),
    /// Error while parsing string
    Parse(std::string::ParseError),
    /// Error while parsing integer
    ParseInt(std::num::ParseIntError),
    /// Error while parsing float
    ParseFloat(std::num::ParseFloatError),
    /// Error while parsing bool
    ParseBool(std::str::ParseBoolError),

    /// Invalid MIME
    InvalidMime(Vec<u8>),
    /// File not found
    FileNotFound(&'static str),
    /// Unexpected end of file
    Eof(&'static str),
    /// Unexpected error
    Mismatch {
        /// Expected
        expected: &'static str,
        /// Found
        found: String,
    },
    /// Workbook is password protected
    Password,
    /// Worksheet not found
    WorksheetNotFound(String),

    /// XML attribute error
    AttrError(quick_xml::events::attributes::AttrError),
    /// XML encoding error
    EncodingError(quick_xml::encoding::EncodingError),
    /// File exceeds maximum cell count
    CellLimitExceeded {
        /// Number of cells requested
        requested: usize,
        /// Maximum allowed cells
        max: usize,
    },
}

/// Ods reader options
#[derive(Debug, Default)]
#[non_exhaustive]
struct OdsOptions {
    pub header_row: HeaderRow,
}

from_err!(std::io::Error, OdsError, Io);
from_err!(zip::result::ZipError, OdsError, Zip);
from_err!(quick_xml::Error, OdsError, Xml);
from_err!(std::str::ParseBoolError, OdsError, ParseBool);
from_err!(std::num::ParseFloatError, OdsError, ParseFloat);
from_err!(std::num::ParseIntError, OdsError, ParseInt);
from_err!(quick_xml::events::attributes::AttrError, OdsError, XmlAttr);
from_err!(quick_xml::encoding::EncodingError, OdsError, Xml);

impl std::fmt::Display for OdsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OdsError::Io(e) => write!(f, "I/O error: {e}"),
            OdsError::Zip(e) => write!(f, "Zip error: {e:?}"),
            OdsError::Xml(e) => write!(f, "Xml error: {e}"),
            OdsError::XmlAttr(e) => write!(f, "Xml attribute error: {e}"),
            OdsError::Parse(e) => write!(f, "Parse string error: {e}"),
            OdsError::ParseInt(e) => write!(f, "Parse integer error: {e}"),
            OdsError::ParseFloat(e) => write!(f, "Parse float error: {e}"),
            OdsError::ParseBool(e) => write!(f, "Parse bool error: {e}"),
            OdsError::InvalidMime(mime) => write!(f, "Invalid MIME type: {mime:?}"),
            OdsError::FileNotFound(file) => write!(f, "'{file}' file not found in archive"),
            OdsError::Eof(node) => write!(f, "Expecting '{node}' node, found end of xml file"),
            OdsError::Mismatch { expected, found } => {
                write!(f, "Expecting '{expected}', found '{found}'")
            }
            OdsError::Password => write!(f, "Workbook is password protected"),
            OdsError::WorksheetNotFound(name) => write!(f, "Worksheet '{name}' not found"),
            OdsError::AttrError(e) => write!(f, "XML attribute Error: {e}"),
            OdsError::EncodingError(e) => write!(f, "XML encoding Error: {e}"),
            OdsError::CellLimitExceeded { requested, max } => {
                write!(
                    f,
                    "Cell limit exceeded ({requested} cells requested, max {max})"
                )
            }
        }
    }
}

impl std::error::Error for OdsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            OdsError::Io(e) => Some(e),
            OdsError::Zip(e) => Some(e),
            OdsError::Xml(e) => Some(e),
            OdsError::Parse(e) => Some(e),
            OdsError::ParseInt(e) => Some(e),
            OdsError::ParseFloat(e) => Some(e),
            OdsError::AttrError(e) => Some(e),
            OdsError::EncodingError(e) => Some(e),
            _ => None,
        }
    }
}

/// An `OpenDocument` Spreadsheet document parser
///
/// # Reference
///
/// OASIS Open Document Format for Office Application 1.2 ([ODF 1.2]).
///
/// [ODF 1.2]: http://docs.oasis-open.org/office/v1.2/OpenDocument-v1.2.pdf
///
pub struct Ods<RS> {
    sheets: BTreeMap<String, (Range<Data>, Range<String>)>,
    metadata: Metadata,
    marker: PhantomData<RS>,
    #[cfg(feature = "picture")]
    pictures: Option<Vec<(String, Vec<u8>)>>,
    /// Reader options
    options: OdsOptions,
}

impl<RS> Ods<RS> {
    /// hedgehog: moves sheet `name`'s values and formulas out of the reader (what
    /// `worksheet_range` and `worksheet_formula` return as copies, with the default header row);
    /// `None` if there is no such sheet.
    pub fn take_sheet(&mut self, name: &str) -> Option<(Range<Data>, Range<String>)> {
        if !matches!(self.options.header_row, HeaderRow::FirstNonEmptyRow) {
            return None;
        }
        self.sheets.remove(name)
    }
}

impl<RS> Reader<RS> for Ods<RS>
where
    RS: Read + Seek,
{
    type Error = OdsError;

    fn new(reader: RS) -> Result<Self, OdsError> {
        let mut zip = ZipArchive::new(reader)?;

        // check mimetype
        match zip.by_name("mimetype") {
            Ok(mut f) => {
                let mut buf = [0u8; 46];
                f.read_exact(&mut buf)?;
                if &buf[..] != MIMETYPE {
                    return Err(OdsError::InvalidMime(buf.to_vec()));
                }
            }
            Err(ZipError::FileNotFound) => return Err(OdsError::FileNotFound("mimetype")),
            Err(e) => return Err(OdsError::Zip(e)),
        }

        check_for_password_protected(&mut zip)?;

        #[cfg(feature = "picture")]
        let pictures = read_pictures(&mut zip)?;

        // hedgehog: large tables on several threads, with the same result (`parallel_content`)
        let content = match parallel_content(&mut zip, crate::xlsx::parallel::parallel_read_threads()) {
            Some(c) => c,
            None => parse_content(zip)?,
        };
        let Content {
            sheets,
            sheets_metadata,
            defined_names,
        } = content;
        let metadata = Metadata {
            sheets: sheets_metadata,
            names: defined_names,
        };

        Ok(Ods {
            marker: PhantomData,
            metadata,
            sheets,
            #[cfg(feature = "picture")]
            pictures,
            options: OdsOptions::default(),
        })
    }

    fn with_header_row(&mut self, header_row: HeaderRow) -> &mut Self {
        self.options.header_row = header_row;
        self
    }

    /// Gets `VbaProject`
    fn vba_project(&mut self) -> Result<Option<VbaProject>, OdsError> {
        Ok(None)
    }

    /// Read sheets from workbook.xml and get their corresponding path from relationships
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Read worksheet data in corresponding worksheet path
    fn worksheet_range(&mut self, name: &str) -> Result<Range<Data>, OdsError> {
        let sheet = self
            .sheets
            .get(name)
            .ok_or_else(|| OdsError::WorksheetNotFound(name.into()))?
            .0
            .to_owned();

        match self.options.header_row {
            HeaderRow::FirstNonEmptyRow => Ok(sheet),
            HeaderRow::Row(header_row_idx) => {
                // If `header_row` is a row index, adjust the range
                if let (Some(start), Some(end)) = (sheet.start(), sheet.end()) {
                    Ok(sheet.range((header_row_idx, start.1), end))
                } else {
                    Ok(sheet)
                }
            }
        }
    }

    fn worksheets(&mut self) -> Vec<(String, Range<Data>)> {
        self.sheets
            .iter()
            .map(|(name, (range, _formula))| (name.to_owned(), range.clone()))
            .collect()
    }

    /// Read worksheet data in corresponding worksheet path
    fn worksheet_formula(&mut self, name: &str) -> Result<Range<String>, OdsError> {
        self.sheets
            .get(name)
            .ok_or_else(|| OdsError::WorksheetNotFound(name.into()))
            .map(|r| r.1.to_owned())
    }

    #[cfg(feature = "picture")]
    fn pictures(&self) -> Option<Vec<(String, Vec<u8>)>> {
        self.pictures.to_owned()
    }
}

struct Content {
    sheets: BTreeMap<String, (Range<Data>, Range<String>)>,
    sheets_metadata: Vec<Sheet>,
    defined_names: Vec<(String, String)>,
}

/// hedgehog: the entry `name`, or its spelling with backslash separators
/// (`META-INF\manifest.xml`, written by some Windows tools); `name` when neither exists.
fn entry_name<RS: Read + Seek>(zip: &ZipArchive<RS>, name: &str) -> String {
    if zip.index_for_name(name).is_none() {
        let alt = name.replace('/', "\\");
        if zip.index_for_name(&alt).is_some() {
            return alt;
        }
    }
    name.to_string()
}

/// Check password protection
fn check_for_password_protected<RS: Read + Seek>(zip: &mut ZipArchive<RS>) -> Result<(), OdsError> {
    let manifest = entry_name(zip, "META-INF/manifest.xml");
    let mut reader = match zip.by_name(&manifest) {
        Ok(f) => {
            let mut r = XmlReader::from_reader(BufReader::new(f));
            let config = r.config_mut();
            config.check_end_names = false;
            config.trim_text(false);
            config.check_comments = false;
            config.expand_empty_elements = true;
            r
        }
        Err(ZipError::FileNotFound) => return Err(OdsError::FileNotFound("META-INF/manifest.xml")),
        Err(e) => return Err(OdsError::Zip(e)),
    };

    let mut buf = Vec::new();
    let mut inner = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name() == QName("manifest:file-entry") => {
                loop {
                    match reader.read_event_into(&mut inner) {
                        Ok(Event::Start(e)) if e.name() == QName("manifest:encryption-data") => {
                            return Err(OdsError::Password)
                        }
                        Ok(Event::Eof) => break,
                        Err(e) => return Err(OdsError::Xml(e)),
                        _ => (),
                    }
                }
                inner.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(OdsError::Xml(e)),
            _ => (),
        }
        buf.clear();
    }

    Ok(())
}

/// Parses content.xml and store the result in `self.content`
fn parse_content<RS: Read + Seek>(mut zip: ZipArchive<RS>) -> Result<Content, OdsError> {
    let mut reader = match zip.by_name("content.xml") {
        Ok(f) => {
            let mut r = XmlReader::from_reader(BufReader::new(f));
            let config = r.config_mut();
            config.check_end_names = false;
            config.trim_text(false);
            config.check_comments = false;
            config.expand_empty_elements = true;
            r
        }
        Err(ZipError::FileNotFound) => return Err(OdsError::FileNotFound("content.xml")),
        Err(e) => return Err(OdsError::Zip(e)),
    };
    let mut buf = Vec::with_capacity(1024);
    let mut sheets = BTreeMap::new();
    let mut defined_names = Vec::new();
    let mut sheets_metadata = Vec::new();
    let mut styles = HashMap::new();
    let mut style_name: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name() == QName("style:style") => {
                style_name = e.raw_attr("style:name")?.map(decode_attr).transpose()?;
            }
            Ok(Event::Start(e))
                if style_name.is_some() && e.name() == QName("style:table-properties") =>
            {
                let visible = match e.raw_attr("table:display")? {
                    Some(v) => {
                        if decode_attr(v)?.parse()? {
                            SheetVisible::Visible
                        } else {
                            SheetVisible::Hidden
                        }
                    }
                    None => SheetVisible::Visible,
                };
                styles.insert(style_name.clone(), visible);
            }
            Ok(Event::Start(e)) if e.name() == QName("table:table") => {
                let visible = styles
                    .get(
                        &e.raw_attr("table:style-name")?
                            .map(decode_attr)
                            .transpose()?,
                    )
                    .cloned()
                    .unwrap_or(SheetVisible::Visible);
                if let Some(v) = e.raw_attr("table:name")? {
                    let name = decode_attr(v)?;
                    let (range, formulas) = read_table(&mut reader)?;
                    sheets_metadata.push(Sheet {
                        name: name.clone(),
                        typ: SheetType::WorkSheet,
                        visible,
                    });
                    sheets.insert(name, (range, formulas));
                }
            }
            Ok(Event::Start(e)) if e.name() == QName("table:named-expressions") => {
                defined_names = read_named_expressions(&mut reader)?;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(OdsError::Xml(e)),
            _ => (),
        }
        buf.clear();
    }
    Ok(Content {
        sheets,
        sheets_metadata,
        defined_names,
    })
}

fn read_table<R: BufRead>(reader: &mut XmlReader<R>) -> Result<(Range<Data>, Range<String>), OdsError> {
    let mut cells = Vec::new();
    let mut rows_repeats = Vec::new();
    let mut formulas = Vec::new();
    let mut cols = Vec::new();
    let mut buf = Vec::with_capacity(1024);
    let mut row_buf = Vec::with_capacity(1024);
    let mut cell_buf = Vec::with_capacity(1024);
    let mut total_rows = 0;
    cols.push(0);
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name() == QName("table:table-row") => {
                let row_repeats = match e.raw_attr("table:number-rows-repeated")? {
                    Some(v) => decode_attr(v)?.parse()?,
                    None => 1,
                };

                // Cap row_repeats so total rows don't exceed MAX_ROWS
                let remaining_rows = (MAX_ROWS as usize).saturating_sub(total_rows);
                let capped_repeats = row_repeats.min(remaining_rows);
                if capped_repeats < row_repeats {
                    warn!(
                        "ods row repeat count capped ({row_repeats} -> {capped_repeats}, max rows {MAX_ROWS})"
                    );
                }
                total_rows = total_rows.saturating_add(capped_repeats);

                read_row(
                    reader,
                    &mut row_buf,
                    &mut cell_buf,
                    &mut cells,
                    &mut formulas,
                )?;
                cols.push(cells.len());
                rows_repeats.push(capped_repeats);
            }
            Ok(Event::End(e)) if e.name() == QName("table:table") => break,
            Err(e) => return Err(OdsError::Xml(e)),
            Ok(_) => (),
        }
        buf.clear();
    }
    Ok((
        get_range(cells, &cols, &rows_repeats)?,
        get_range(formulas, &cols, &rows_repeats)?,
    ))
}

fn is_empty_row<T: Default + Clone + PartialEq>(row: &[T]) -> bool {
    row.iter().all(|x| x == &T::default())
}

fn get_range<T: Default + Clone + PartialEq>(
    mut cells: Vec<T>,
    cols: &[usize],
    rows_repeats: &[usize],
) -> Result<Range<T>, OdsError> {
    // find smallest area with non empty Cells
    let mut row_min = None;
    let mut row_max = 0;
    let mut col_min = usize::MAX;
    let mut col_max = 0;
    let mut first_empty_rows_repeated = 0;
    {
        for (i, w) in cols.windows(2).enumerate() {
            let row = &cells[w[0]..w[1]];
            if let Some(p) = row.iter().position(|c| c != &T::default()) {
                if row_min.is_none() {
                    row_min = Some(i);
                    first_empty_rows_repeated =
                        rows_repeats.iter().take(i).sum::<usize>().saturating_sub(i);
                }
                row_max = i;
                if p < col_min {
                    col_min = p;
                }
                if let Some(p) = row.iter().rposition(|c| c != &T::default()) {
                    if p > col_max {
                        col_max = p;
                    }
                }
            }
        }
    }
    let Some(row_min) = row_min else {
        return Ok(Range::default());
    };

    // rebuild cells into its smallest non empty area
    let row_width = col_max + 1 - col_min;
    let cells_len = (row_max + 1 - row_min) * row_width;
    {
        let mut new_cells = Vec::with_capacity(cells_len.min(MAX_CELLS));
        let empty_cells = vec![T::default(); col_max + 1];
        let mut empty_row_repeats = 0_usize;
        let mut consecutive_empty_rows = 0_usize;
        for (w, row_repeats) in cols
            .windows(2)
            .skip(row_min)
            .take(row_max + 1)
            .zip(rows_repeats.iter().skip(row_min).take(row_max + 1))
        {
            let row = &cells[w[0]..w[1]];
            let row_repeats = *row_repeats;

            if is_empty_row(row) {
                empty_row_repeats = empty_row_repeats.saturating_add(row_repeats);
                consecutive_empty_rows += 1;
                continue;
            }

            if empty_row_repeats > 0 {
                // Check if expanding empty rows would exceed MAX_CELLS
                let cells_to_add = empty_row_repeats.saturating_mul(row_width);
                if new_cells.len().saturating_add(cells_to_add) > MAX_CELLS {
                    return Err(OdsError::CellLimitExceeded {
                        requested: new_cells.len().saturating_add(cells_to_add),
                        max: MAX_CELLS,
                    });
                }
                row_max = row_max + empty_row_repeats - consecutive_empty_rows;
                for _ in 0..empty_row_repeats {
                    new_cells.extend_from_slice(&empty_cells[col_min..]);
                }
                empty_row_repeats = 0;
                consecutive_empty_rows = 0;
            }

            if row_repeats > 1 {
                row_max = row_max + row_repeats - 1;
            }

            // Check if expanding this row would exceed MAX_CELLS
            let cells_to_add = row_repeats.saturating_mul(row_width);
            if new_cells.len().saturating_add(cells_to_add) > MAX_CELLS {
                return Err(OdsError::CellLimitExceeded {
                    requested: new_cells.len().saturating_add(cells_to_add),
                    max: MAX_CELLS,
                });
            }

            for _ in 0..row_repeats {
                match row.len().cmp(&(col_max + 1)) {
                    std::cmp::Ordering::Less => {
                        new_cells.extend_from_slice(&row[col_min..]);
                        new_cells.extend_from_slice(&empty_cells[row.len()..]);
                    }
                    std::cmp::Ordering::Equal => {
                        new_cells.extend_from_slice(&row[col_min..]);
                    }
                    std::cmp::Ordering::Greater => {
                        new_cells.extend_from_slice(&row[col_min..=col_max]);
                    }
                }
            }
        }
        cells = new_cells;
    }
    let row_min = row_min + first_empty_rows_repeated;
    let row_max = row_max + first_empty_rows_repeated;
    Ok(Range {
        start: (row_min as u32, col_min as u32),
        end: (row_max as u32, col_max as u32),
        inner: cells,
    })
}

fn read_row<R: BufRead>(
    reader: &mut XmlReader<R>,
    row_buf: &mut Vec<u8>,
    cell_buf: &mut Vec<u8>,
    cells: &mut Vec<Data>,
    formulas: &mut Vec<String>,
) -> Result<(), OdsError> {
    let mut empty_col_repeats = 0;
    let row_start = cells.len();
    loop {
        row_buf.clear();
        match reader.read_event_into(row_buf) {
            Ok(Event::Start(e))
                if e.name() == QName("table:table-cell")
                    || e.name() == QName("table:covered-table-cell") =>
            {
                let repeats = match e.raw_attr("table:number-columns-repeated")? {
                    Some(v) => v.parse()?,
                    None => 1,
                };

                let (value, formula, is_closed) = get_datatype(reader, &e, cell_buf)?;

                // Cap empty_col_repeats to not exceed MAX_COLUMNS
                let current_cols = cells.len() - row_start;
                let remaining = (MAX_COLUMNS as usize).saturating_sub(current_cols);
                let capped_empty = empty_col_repeats.min(remaining);
                if capped_empty < empty_col_repeats {
                    warn!(
                        "ods column repeat count capped ({empty_col_repeats} -> {capped_empty}, max columns {MAX_COLUMNS})"
                    );
                }

                for _ in 0..capped_empty {
                    cells.push(Data::Empty);
                    formulas.push("".to_string());
                }
                empty_col_repeats = 0;

                // Cap repeats to not exceed MAX_COLUMNS
                let current_cols = cells.len() - row_start;
                let remaining = (MAX_COLUMNS as usize).saturating_sub(current_cols);
                let capped_repeats = repeats.min(remaining);
                if capped_repeats < repeats {
                    warn!(
                        "ods column repeat count capped ({repeats} -> {capped_repeats}, max columns {MAX_COLUMNS})"
                    );
                }

                if value.is_empty() && formula.is_empty() {
                    empty_col_repeats = capped_repeats;
                } else {
                    for _ in 0..capped_repeats {
                        cells.push(value.clone());
                        formulas.push(formula.clone());
                    }
                }
                if !is_closed {
                    reader.read_to_end_into(e.name(), cell_buf)?;
                }
            }
            Ok(Event::End(e)) if e.name() == QName("table:table-row") => break,
            Err(e) => return Err(OdsError::Xml(e)),
            Ok(e) => {
                return Err(OdsError::Mismatch {
                    expected: "table-cell",
                    found: format!("{e:?}"),
                });
            }
        }
    }
    Ok(())
}

/// Converts table-cell element into a `Data`
///
/// ODF 1.2-19.385
fn get_datatype<R: BufRead>(
    reader: &mut XmlReader<R>,
    e: &BytesStart<'_>,
    buf: &mut Vec<u8>,
) -> Result<(Data, String, bool), OdsError> {
    let mut is_string = false;
    let mut is_value_set = false;
    let mut val = Data::Empty;
    let mut formula = String::new();
    for attr in e.iter_raw_attrs() {
        let (key, value) = attr?;
        match key {
            "office:value" if !is_value_set => {
                val = Data::Float(
                    fast_float2::parse(value.as_bytes())
                        .map_err(|_| OdsError::ParseFloat(value.parse::<f64>().unwrap_err()))?,
                );
                is_value_set = true;
            }
            "office:string-value" | "office:date-value" | "office:time-value" if !is_value_set => {
                let attr = decode_attr(value)?;
                val = match key {
                    "office:date-value" => Data::DateTimeIso(attr),
                    "office:time-value" => Data::DurationIso(attr),
                    _ => Data::String(attr),
                };
                is_value_set = true;
            }
            "office:boolean-value" if !is_value_set => {
                let b = value == "TRUE" || value == "true";
                val = Data::Bool(b);
                is_value_set = true;
            }
            "office:value-type" if !is_value_set => is_string = value == "string",
            "table:formula" => {
                formula = decode_attr(value)?;
            }
            _ => (),
        }
    }
    if !is_value_set && is_string {
        // If the value type is string and the office:string-value attribute
        // is not present, the element content defines the value.
        let mut s = String::new();
        let mut first_paragraph = true;
        loop {
            buf.clear();
            match reader.read_event_into(buf) {
                Ok(Event::Text(t)) => {
                    s.push_str(&t.xml10_content());
                }
                Ok(Event::GeneralRef(e)) => {
                    unescape_entity_to_buffer(&e, &mut s)?;
                }
                Ok(Event::End(e))
                    if e.name() == QName("table:table-cell")
                        || e.name() == QName("table:covered-table-cell") =>
                {
                    return Ok((Data::String(s), formula, true));
                }
                Ok(Event::Start(e)) if e.name() == QName("office:annotation") => loop {
                    match reader.read_event_into(buf) {
                        Ok(Event::End(e)) if e.name() == QName("office:annotation") => {
                            break;
                        }
                        Err(e) => return Err(OdsError::Xml(e)),
                        _ => (),
                    }
                },
                Ok(Event::Start(e)) if e.name() == QName("text:p") => {
                    if first_paragraph {
                        first_paragraph = false;
                    } else {
                        s.push('\n');
                    }
                }
                Ok(Event::Start(e)) if e.name() == QName("text:s") => {
                    let count = match e.raw_attr("text:c")? {
                        Some(v) => decode_attr(v)?.parse()?,
                        None => 1,
                    };
                    for _ in 0..count {
                        s.push(' ');
                    }
                }
                Err(e) => return Err(OdsError::Xml(e)),
                Ok(Event::Eof) => return Err(OdsError::Eof("table:table-cell")),
                _ => (),
            }
        }
    } else {
        Ok((val, formula, false))
    }
}

fn read_named_expressions<R: BufRead>(
    reader: &mut XmlReader<R>,
) -> Result<Vec<(String, String)>, OdsError> {
    let mut defined_names = Vec::new();
    let mut buf = Vec::with_capacity(512);
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e))
                if e.name() == QName("table:named-range")
                    || e.name() == QName("table:named-expression") =>
            {
                let mut name = String::new();
                let mut formula = String::new();
                for attr in e.iter_raw_attrs() {
                    let (key, value) = attr?;
                    match key {
                        "table:name" => {
                            name = decode_attr(value)?;
                        }
                        "table:cell-range-address" | "table:expression" => {
                            formula = decode_attr(value)?;
                        }
                        _ => (),
                    }
                }
                defined_names.push((name, formula));
            }
            Ok(Event::End(e))
                if e.name() == QName("table:named-range")
                    || e.name() == QName("table:named-expression") => {}
            Ok(Event::End(e)) if e.name() == QName("table:named-expressions") => break,
            Err(e) => return Err(OdsError::Xml(e)),
            Ok(e) => {
                return Err(OdsError::Mismatch {
                    expected: "table:named-expressions",
                    found: format!("{e:?}"),
                });
            }
        }
    }
    Ok(defined_names)
}

// Read pictures.
#[cfg(feature = "picture")]
#[allow(clippy::type_complexity)]
fn read_pictures<RS: Read + Seek>(
    zip: &mut ZipArchive<RS>,
) -> Result<Option<Vec<(String, Vec<u8>)>>, OdsError> {
    let mut pics = Vec::new();
    for i in 0..zip.len() {
        let mut zfile = zip.by_index(i)?;
        let zname = zfile.name()?;
        // no Thumbnails
        if zname.starts_with("Pictures") {
            if let Some(ext) = zname.split('.').next_back() {
                if [
                    "emf", "wmf", "pict", "jpeg", "jpg", "png", "dib", "gif", "tiff", "eps", "bmp",
                    "wpg",
                ]
                .contains(&ext)
                {
                    let ext = ext.to_string();
                    let mut buf: Vec<u8> = Vec::new();
                    zfile.read_to_end(&mut buf)?;
                    pics.push((ext, buf));
                }
            }
        }
    }
    if pics.is_empty() {
        Ok(None)
    } else {
        Ok(Some(pics))
    }
}

// hedgehog: everything below is not part of calamine 0.37.0.

/// `content.xml` parts smaller than this (uncompressed) are parsed on one thread.
const PARALLEL_MIN_CONTENT: u64 = 2 << 20;

/// hedgehog: [`parse_content`] with the rows of every table read on `threads` threads: the outer
/// loop is calamine's, and a table's rows are cut into chunks at `</table:table-row>` and read
/// with `read_row` in parallel, then joined in order (row repeats capped in order, as
/// `read_table` does). `None` gives up (anything unusual: comments, CDATA or processing
/// instructions in a table, a nested table, a declared encoding other than UTF-8, any error,
/// which the one-thread parse then reports); the caller parses `content.xml` again on one
/// thread.
fn parallel_content<RS: Read + Seek>(zip: &mut ZipArchive<RS>, threads: usize) -> Option<Content> {
    if threads < 2 {
        return None;
    }
    let f = zip.by_name("content.xml").ok()?;
    if f.size() < crate::xlsx::parallel::min_part(PARALLEL_MIN_CONTENT) {
        return None;
    }
    let mut reader = XmlReader::from_reader(Rewind::new(f));
    let config = reader.config_mut();
    config.check_end_names = false;
    config.trim_text(false);
    config.check_comments = false;
    config.expand_empty_elements = true;
    let mut buf = Vec::with_capacity(1024);
    let mut sheets = BTreeMap::new();
    let mut defined_names = Vec::new();
    let mut sheets_metadata = Vec::new();
    let mut styles = HashMap::new();
    let mut style_name: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name() == QName("style:style") => {
                style_name = e.raw_attr("style:name").ok()?.map(decode_attr).transpose().ok()?;
            }
            Ok(Event::Start(e))
                if style_name.is_some() && e.name() == QName("style:table-properties") =>
            {
                let visible = match e.raw_attr("table:display").ok()? {
                    Some(v) => {
                        if decode_attr(v).ok()?.parse().ok()? {
                            SheetVisible::Visible
                        } else {
                            SheetVisible::Hidden
                        }
                    }
                    None => SheetVisible::Visible,
                };
                styles.insert(style_name.clone(), visible);
            }
            Ok(Event::Start(e)) if e.name() == QName("table:table") => {
                let visible = styles
                    .get(
                        &e.raw_attr("table:style-name")
                            .ok()?
                            .map(decode_attr)
                            .transpose()
                            .ok()?,
                    )
                    .cloned()
                    .unwrap_or(SheetVisible::Visible);
                if let Some(v) = e.raw_attr("table:name").ok()? {
                    let name = decode_attr(v).ok()?;
                    if reader.encoding() != encoding_rs::UTF_8 {
                        return None;
                    }
                    let (range, formulas) = read_table_parallel(&mut reader, threads)?;
                    sheets_metadata.push(Sheet {
                        name: name.clone(),
                        typ: SheetType::WorkSheet,
                        visible,
                    });
                    sheets.insert(name, (range, formulas));
                }
            }
            Ok(Event::Start(e)) if e.name() == QName("table:named-expressions") => {
                defined_names = read_named_expressions(&mut reader).ok()?;
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => (),
        }
        buf.clear();
    }
    Some(Content {
        sheets,
        sheets_metadata,
        defined_names,
    })
}

/// A buffered reader whose unread bytes can be taken out and put back (the table bodies are
/// read past the XML reader, and what follows a table goes back to it).
struct Rewind<R> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
}

impl<R: Read> Rewind<R> {
    fn new(inner: R) -> Self {
        Rewind {
            inner,
            buf: Vec::new(),
            pos: 0,
        }
    }

    /// The buffered bytes not read yet.
    fn take_buffered(&mut self) -> Vec<u8> {
        let rest = self.buf[self.pos..].to_vec();
        self.buf.clear();
        self.pos = 0;
        rest
    }

    /// Bytes to read before the rest of the input (the buffer is empty).
    fn put_back(&mut self, bytes: Vec<u8>) {
        self.buf = bytes;
        self.pos = 0;
    }
}

impl<R: Read> Read for Rewind<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let avail = self.fill_buf()?;
        let n = avail.len().min(out.len());
        out[..n].copy_from_slice(&avail[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl<R: Read> BufRead for Rewind<R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.pos == self.buf.len() {
            self.buf.resize(64 << 10, 0);
            self.pos = 0;
            let n = loop {
                match self.inner.read(&mut self.buf) {
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    r => break r,
                }
            };
            match n {
                Ok(n) => self.buf.truncate(n),
                Err(e) => {
                    self.buf.clear();
                    return Err(e);
                }
            }
        }
        Ok(&self.buf[self.pos..])
    }

    fn consume(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.buf.len());
    }
}

/// One chunk of a table's rows: the cells and formulas of its rows back to back, and per row
/// where its cells end and its declared repeat count.
struct RowsChunk {
    cells: Vec<Data>,
    formulas: Vec<String>,
    rows: Vec<(usize, usize)>,
    /// The chunk ended right after a row.
    after_row: bool,
}

/// Reads the rows of one chunk exactly as `read_table` reads them. No element is open: an end
/// tag that closes an element opened before the chunk is an error, and the table is read on one
/// thread (the one-thread read, deeper at that point, may accept it).
fn read_rows_chunk(bytes: &[u8]) -> Result<RowsChunk, OdsError> {
    // The space keeps the reader from taking the chunk's first bytes for a byte order mark.
    let mut reader = XmlReader::from_reader(Read::chain(&b" "[..], bytes));
    let config = reader.config_mut();
    config.check_end_names = false;
    config.trim_text(false);
    config.check_comments = false;
    config.expand_empty_elements = true;
    let mut out = RowsChunk {
        cells: Vec::new(),
        formulas: Vec::new(),
        rows: Vec::new(),
        after_row: false,
    };
    let mut buf = Vec::with_capacity(1024);
    let mut row_buf = Vec::with_capacity(1024);
    let mut cell_buf = Vec::with_capacity(1024);
    loop {
        let mut after_row = false;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name() == QName("table:table-row") => {
                let row_repeats = match e.raw_attr("table:number-rows-repeated")? {
                    Some(v) => decode_attr(v)?.parse()?,
                    None => 1,
                };
                read_row(
                    &mut reader,
                    &mut row_buf,
                    &mut cell_buf,
                    &mut out.cells,
                    &mut out.formulas,
                )?;
                out.rows.push((out.cells.len(), row_repeats));
                after_row = true;
            }
            // The table's end tag is not in any chunk.
            Ok(Event::End(e)) if e.name() == QName("table:table") => {
                return Err(OdsError::Mismatch {
                    expected: "table-row",
                    found: "table end".into(),
                })
            }
            Ok(Event::Eof) => return Ok(out),
            Err(e) => return Err(OdsError::Xml(e)),
            Ok(_) => (),
        }
        out.after_row = after_row;
        buf.clear();
    }
}

/// `read_table` for the table whose start tag `reader` has just read, on `threads` threads.
/// The bytes after the table's end tag (the tag included) go back to `reader`.
fn read_table_parallel<R: Read>(
    reader: &mut XmlReader<Rewind<R>>,
    threads: usize,
) -> Option<(Range<Data>, Range<String>)> {
    use crate::xlsx::parallel::{run_chunks, Flow};
    use memchr::memmem;

    let src = reader.get_mut();
    let mut pending = src.take_buffered();
    let mut scanned = 0usize; // bytes of `pending` checked for markup that could hide a boundary
    let mut leftover: Option<Vec<u8>> = None;
    let mut eof = false;
    let mut failed = false;
    let end_finder = memmem::Finder::new(b"</table:table>");
    let row_end = memmem::FinderRev::new(b"</table:table-row>");
    // Bytes of table XML per chunk (512 kB).
    let chunk = crate::xlsx::parallel::chunk_bytes();
    let mut next_chunk = || -> Result<Option<(Vec<u8>, bool)>, ()> {
        if leftover.is_some() {
            return Ok(None);
        }
        let mut target = chunk;
        loop {
            while pending.len() < target && !eof {
                let old = pending.len();
                pending.resize(old + chunk, 0);
                let mut n = 0;
                while old + n < pending.len() {
                    match src.inner.read(&mut pending[old + n..]) {
                        Ok(0) => break,
                        Ok(k) => n += k,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => {
                            failed = true;
                            return Err(());
                        }
                    }
                }
                pending.truncate(old + n);
                eof = n < chunk;
            }
            // Markup in the table that could hide a boundary or start a nested table: give up.
            let end = end_finder.find(&pending);
            let from = scanned.saturating_sub(16);
            let fresh = &pending[from.min(end.unwrap_or(usize::MAX))..end.unwrap_or(pending.len())];
            if memmem::find(fresh, b"<!").is_some() || memmem::find(fresh, b"<?").is_some() {
                return Err(());
            }
            for i in memmem::find_iter(fresh, b"<table:table") {
                match fresh.get(i + b"<table:table".len()) {
                    Some(b) if b.is_ascii_whitespace() || *b == b'>' || *b == b'/' => return Err(()),
                    None if !eof => {} // decided once more bytes are there
                    _ => {}
                }
            }
            scanned = pending.len();
            if let Some(end) = end {
                leftover = Some(pending.split_off(end));
                return Ok(Some((std::mem::take(&mut pending), true)));
            }
            if eof {
                return Err(()); // the part ends inside the table
            }
            match row_end.rfind(&pending) {
                Some(i) => {
                    let cut = i + b"</table:table-row>".len();
                    let bytes = pending[..cut].to_vec();
                    pending.drain(..cut);
                    scanned = scanned.saturating_sub(cut);
                    return Ok(Some((bytes, false)));
                }
                None => target = pending.len() + chunk,
            }
        }
    };
    let work = |_: usize, bytes: Vec<u8>, _: bool| read_rows_chunk(&bytes);
    let mut cells = Vec::new();
    let mut formulas = Vec::new();
    let mut rows_repeats = Vec::new();
    let mut cols = vec![0];
    let mut total_rows = 0usize;
    let mut merge = |r: Result<RowsChunk, OdsError>, last: bool| {
        let Ok(c) = r else {
            return Ok::<_, ()>(Flow::GiveUp);
        };
        if !last && !c.after_row {
            return Ok(Flow::GiveUp);
        }
        let base = cells.len();
        for (end, row_repeats) in c.rows {
            let remaining_rows = (MAX_ROWS as usize).saturating_sub(total_rows);
            let capped_repeats = row_repeats.min(remaining_rows);
            total_rows = total_rows.saturating_add(capped_repeats);
            cols.push(base + end);
            rows_repeats.push(capped_repeats);
        }
        cells.extend(c.cells);
        formulas.extend(c.formulas);
        Ok(if last { Flow::Done } else { Flow::Continue })
    };
    let done = run_chunks(&mut next_chunk, threads, &work, &mut merge) == Ok(true) && !failed;
    crate::xlsx::parallel::count_part(done);
    if !done {
        return None;
    }
    src.put_back(leftover?);
    Some((
        get_range(cells, &cols, &rows_repeats).ok()?,
        get_range(formulas, &cols, &rows_repeats).ok()?,
    ))
}
