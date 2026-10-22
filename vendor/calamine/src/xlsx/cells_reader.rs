// SPDX-License-Identifier: MIT
//
// Copyright 2016-2026, Johann Tuffe.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader as XmlReader;
use std::io::{BufRead, Read, Seek};

use super::{
    expand_shared_formula, get_dimension, get_row, get_row_column, read_string_with_bufs,
    Dimensions, XlReader,
};
use crate::attrs::RawAttributes;
use crate::{
    datatype::DataRef,
    formats::{format_excel_f64_ref, CellFormat},
    utils::{unescape_entity_to_buffer, unescape_xml},
    Cell, XlsxError,
};

#[derive(Clone, Debug)]
pub(crate) struct SharedFormula {
    formula: String,
    range: Dimensions,
}

impl SharedFormula {
    /// hedgehog: the formula of a derived cell at `pos`, as `read_formula_record` expands it.
    pub(crate) fn expand(&self, pos: (u32, u32)) -> Result<String, XlsxError> {
        expand_shared_formula(&self.formula, self.range.start, pos)
    }
}

/// Workbook-level context used when reading cell values.
pub(crate) struct WorkbookContext<'a> {
    pub(crate) strings: &'a [String],
    pub(crate) formats: &'a [CellFormat],
    pub(crate) is_1904: bool,
}

/// Reusable scratch buffers for cell value parsing (avoid per-cell allocations).
struct ValueBufs {
    xml: Vec<u8>,
    value: String,
    str_inner: Vec<u8>,
}

impl ValueBufs {
    fn new() -> Self {
        Self {
            xml: Vec::with_capacity(1024),
            value: String::with_capacity(64),
            str_inner: Vec::with_capacity(1024),
        }
    }
}

/// Formula metadata attached to an XLSX cell record.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XlsxFormulaMetadata {
    /// Ordinary, non-shared formula.
    Normal {
        /// Formula text.
        formula: String,
    },

    /// Shared formula anchor/template cell.
    Shared {
        /// Shared formula index (`si`). Shared formula indices are worksheet-local.
        shared_index: usize,
        /// Shared formula range, when present.
        range: Option<Dimensions>,
        /// Template formula text as stored on the anchor cell.
        formula: String,
    },

    /// Shared formula derived cell.
    SharedDerived {
        /// Shared formula index (`si`). Shared formula indices are worksheet-local.
        shared_index: usize,
    },
}

impl XlsxFormulaMetadata {
    /// Return the shared formula index when this record belongs to a shared group.
    pub fn shared_index(&self) -> Option<usize> {
        match self {
            Self::Normal { .. } => None,
            Self::Shared { shared_index, .. } | Self::SharedDerived { shared_index } => {
                Some(*shared_index)
            }
        }
    }
}

/// Internal formula metadata used while streaming a cell.
///
/// This mirrors the public [`XlsxFormulaMetadata`] but can also carry an
/// expanded formula string for shared-formula derived cells when callers ask
/// for the convenience text API. The public metadata API intentionally drops
/// that string to avoid per-derived-cell expansion and allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FormulaMetadata {
    Normal {
        formula: String,
    },

    Shared {
        shared_index: usize,
        range: Option<Dimensions>,
        formula: String,
    },

    SharedDerived {
        shared_index: usize,
        translated_formula: Option<String>,
    },
}

impl FormulaMetadata {
    fn formula_text(&self) -> Option<&str> {
        match self {
            Self::Normal { formula } | Self::Shared { formula, .. } => Some(formula),
            Self::SharedDerived {
                translated_formula, ..
            } => translated_formula.as_deref(),
        }
    }

    fn into_metadata(self) -> XlsxFormulaMetadata {
        match self {
            Self::Normal { formula } => XlsxFormulaMetadata::Normal { formula },
            Self::Shared {
                shared_index,
                range,
                formula,
            } => XlsxFormulaMetadata::Shared {
                shared_index,
                range,
                formula,
            },
            Self::SharedDerived { shared_index, .. } => {
                XlsxFormulaMetadata::SharedDerived { shared_index }
            }
        }
    }
}

/// A single XLSX cell structure containing both cached/literal value and expanded formula text.
#[derive(Clone, Debug, PartialEq)]
pub struct XlsxCellFormula<'a> {
    /// Zero-based `(row, column)` cell position.
    pub pos: (u32, u32),
    /// Literal or cached value associated with the cell.
    pub value: DataRef<'a>,
    /// Formula text, expanded for shared formulas when the shared-formula anchor
    /// has already been observed in stream order.
    pub formula: Option<String>,
}

/// A single XLSX cell record containing cached/literal value and formula metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct XlsxCellFormulaMetadataRecord<'a> {
    /// Zero-based `(row, column)` cell position.
    pub pos: (u32, u32),
    /// Literal or cached value associated with the cell.
    pub value: DataRef<'a>,
    /// Formula metadata, when the cell contains a formula.
    pub formula: Option<XlsxFormulaMetadata>,
}

pub(crate) struct XlsxCellFormulaMetadataRecordInternal<'a> {
    pos: (u32, u32),
    value: DataRef<'a>,
    formula: Option<FormulaMetadata>,
}

impl<'a> XlsxCellFormulaMetadataRecordInternal<'a> {
    /// hedgehog: the shared index of a derived shared formula whose anchor was not found.
    pub(crate) fn unresolved_shared(&self) -> Option<usize> {
        match self.formula {
            Some(FormulaMetadata::SharedDerived {
                shared_index,
                translated_formula: None,
            }) => Some(shared_index),
            _ => None,
        }
    }

    /// hedgehog: the record as [`XlsxCellReader::next_cell_with_formula`] returns it.
    pub(crate) fn into_formula_record(self) -> XlsxCellFormula<'a> {
        XlsxCellFormula {
            pos: self.pos,
            value: self.value,
            formula: self
                .formula
                .and_then(|formula| formula.formula_text().map(str::to_string)),
        }
    }
}

/// hedgehog: the streaming state of a cell reader, kept apart from its XML reader so that the
/// same code reads one sheet in chunks on several threads (`parallel.rs`).
pub(crate) struct CellState {
    pub(crate) row_index: u32,
    col_index: u32,
    buf: Vec<u8>,
    cell_buf: Vec<u8>,
    value_bufs: ValueBufs,
    pub(crate) formulas: Vec<Option<SharedFormula>>,
    /// `Some` when reading a chunk of the sheet data rather than the whole part.
    pub(crate) chunk: Option<ChunkFlags>,
}

/// hedgehog: what a chunk read found out about its dependence on the chunks before it.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ChunkFlags {
    /// The row index is known: the chunk starts the sheet data, or a row with an `r` attribute
    /// has been read.
    pub(crate) row_known: bool,
    /// The chunk used state it does not have (a row index before any `r`, a shared index too
    /// large to track here): it must be read again in order, with the state the chunks before
    /// it leave. (Shared formulas anchored in an earlier chunk are looked up afterwards.)
    pub(crate) needs_context: bool,
    /// The chunk ended (end of the input) outside any element.
    pub(crate) at_eof: bool,
    /// The last event read was the end of a row.
    pub(crate) last_row_end: bool,
    /// The chunk is read with the state of every chunk before it (in order): shared indexes are
    /// tracked whatever their size, as in a whole-part read.
    pub(crate) in_order: bool,
}

/// hedgehog: shared indexes above this make a chunk read depend on its context (the in-order
/// read then grows the table exactly as calamine does).
const CHUNK_MAX_SHARED_INDEX: usize = 1 << 16;

impl CellState {
    pub(crate) fn new(chunk: Option<ChunkFlags>) -> Self {
        CellState {
            row_index: 0,
            col_index: 0,
            buf: Vec::with_capacity(1024),
            cell_buf: Vec::with_capacity(1024),
            value_bufs: ValueBufs::new(),
            formulas: Vec::with_capacity(1024),
            chunk,
        }
    }
}

/// hedgehog: [`XlsxCellReader::new`]'s walk to `<sheetData>`, for any XML reader: the declared
/// dimensions.
pub(crate) fn read_sheet_head<R: BufRead>(
    xml: &mut XmlReader<R>,
    buf: &mut Vec<u8>,
) -> Result<Dimensions, XlsxError> {
    let mut dimensions = Dimensions::default();
    let mut sh_type = None;
    'xml: loop {
        buf.clear();
        match xml.read_event_into(buf).map_err(XlsxError::Xml)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "dimension" => {
                    if let Some(rdim) = e.raw_attr("ref")? {
                        dimensions = get_dimension(rdim)?;
                        continue 'xml;
                    }
                    return Err(XlsxError::UnexpectedNode("dimension"));
                }
                "sheetData" => break,
                typ => {
                    if sh_type.is_none() {
                        sh_type = Some(typ.to_string());
                    }
                }
            },
            Event::Eof => {
                if let Some(typ) = sh_type {
                    return Err(XlsxError::NotAWorksheet(typ));
                } else {
                    return Err(XlsxError::XmlEof("worksheet"));
                }
            }
            _ => (),
        }
    }
    Ok(dimensions)
}

/// An xlsx Cell Iterator.
///
/// The `next_*` methods all advance the same XML stream. Use one streaming
/// method per reader; mixing methods on a single reader will consume cells from
/// the current stream position.
pub struct XlsxCellReader<'a, RS>
where
    RS: Read + Seek,
{
    xml: XlReader<'a, RS>,
    strings: &'a [String],
    formats: &'a [CellFormat],
    is_1904: bool,
    dimensions: Dimensions,
    // hedgehog: row/column index, buffers and shared formulas in `CellState`
    st: CellState,
}

impl<'a, RS> XlsxCellReader<'a, RS>
where
    RS: Read + Seek,
{
    /// Create a new XLSX cell reader over a worksheet XML stream.
    pub fn new(
        mut xml: XlReader<'a, RS>,
        strings: &'a [String],
        formats: &'a [CellFormat],
        is_1904: bool,
    ) -> Result<Self, XlsxError> {
        let mut buf = Vec::with_capacity(1024);
        let dimensions = read_sheet_head(&mut xml, &mut buf)?;
        Ok(Self {
            xml,
            strings,
            formats,
            is_1904,
            dimensions,
            st: CellState::new(None),
        })
    }

    /// Return the worksheet dimensions declared by the sheet XML.
    pub fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    /// Return the next cell value in XML stream order.
    pub fn next_cell(&mut self) -> Result<Option<Cell<DataRef<'a>>>, XlsxError> {
        loop {
            self.st.buf.clear();
            match self.xml.read_event_into(&mut self.st.buf) {
                Ok(Event::Start(row_element)) if row_element.local_name().as_ref() == "row" => {
                    if let Some(r) = row_element.raw_attr("r")? {
                        self.st.row_index = get_row(r)?;
                    }
                }
                Ok(Event::End(row_element)) if row_element.local_name().as_ref() == "row" => {
                    self.st.row_index += 1;
                    self.st.col_index = 0;
                }
                Ok(Event::Start(c_element)) if c_element.local_name().as_ref() == "c" => {
                    let (pos_attr, style_attr, type_attr) =
                        get_attrs!(c_element, "r" => r, "s" => s, "t" => t)?;
                    let pos = if let Some(range) = pos_attr {
                        let (row, col) = get_row_column(range)?;
                        self.st.col_index = col;
                        (row, col)
                    } else {
                        (self.st.row_index, self.st.col_index)
                    };
                    let mut value = DataRef::Empty;
                    loop {
                        self.st.cell_buf.clear();
                        match self.xml.read_event_into(&mut self.st.cell_buf) {
                            Ok(Event::Start(e)) => {
                                let ctx = WorkbookContext {
                                    strings: self.strings,
                                    formats: self.formats,
                                    is_1904: self.is_1904,
                                };
                                value = read_value(
                                    &ctx,
                                    &mut self.xml,
                                    &e,
                                    style_attr,
                                    type_attr,
                                    &mut self.st.value_bufs,
                                )?;
                            }
                            Ok(Event::End(e)) if e.local_name().as_ref() == "c" => break,
                            Ok(Event::Eof) => return Err(XlsxError::XmlEof("c")),
                            Err(e) => return Err(XlsxError::Xml(e)),
                            _ => (),
                        }
                    }
                    self.st.col_index += 1;
                    return Ok(Some(Cell::new(pos, value)));
                }
                Ok(Event::End(e)) if e.local_name().as_ref() == "sheetData" => {
                    return Ok(None);
                }
                Ok(Event::Eof) => return Err(XlsxError::XmlEof("sheetData")),
                Err(e) => return Err(XlsxError::Xml(e)),
                _ => (),
            }
        }
    }

    /// Return the next cell record, exposing cached/literal value plus expanded
    /// per-cell formula text. Shared-formula metadata is intentionally not
    /// exposed through this compatibility-oriented one-pass API; use
    /// [`Self::next_cell_with_formula_metadata`] when shared-formula metadata is needed.
    pub fn next_cell_with_formula(&mut self) -> Result<Option<XlsxCellFormula<'a>>, XlsxError> {
        Ok(self
            .next_cell_formula_record_impl(true)?
            .map(XlsxCellFormulaMetadataRecordInternal::into_formula_record))
    }

    /// Return the next cell record, exposing cached/literal value plus formula
    /// metadata. Shared formulas are reported semantically as anchors/derived
    /// placements instead of only as expanded text. Derived shared formulas carry
    /// only their shared index, avoiding per-cell formula expansion/allocation.
    pub fn next_cell_with_formula_metadata(
        &mut self,
    ) -> Result<Option<XlsxCellFormulaMetadataRecord<'a>>, XlsxError> {
        Ok(self
            .next_cell_formula_record_impl(false)?
            .map(|record| XlsxCellFormulaMetadataRecord {
                pos: record.pos,
                value: record.value,
                formula: record.formula.map(FormulaMetadata::into_metadata),
            }))
    }

    fn next_cell_formula_record_impl(
        &mut self,
        expand_shared_derived: bool,
    ) -> Result<Option<XlsxCellFormulaMetadataRecordInternal<'a>>, XlsxError> {
        let ctx = WorkbookContext {
            strings: self.strings,
            formats: self.formats,
            is_1904: self.is_1904,
        };
        next_cell_formula_record(&mut self.xml, &mut self.st, &ctx, expand_shared_derived)
    }

    /// Return the next formula in XML stream order, expanding shared formulas.
    pub fn next_formula(&mut self) -> Result<Option<Cell<String>>, XlsxError> {
        loop {
            self.st.buf.clear();
            match self.xml.read_event_into(&mut self.st.buf) {
                Ok(Event::Start(row_element)) if row_element.local_name().as_ref() == "row" => {
                    if let Some(r) = row_element.raw_attr("r")? {
                        self.st.row_index = get_row(r)?;
                    }
                }
                Ok(Event::End(row_element)) if row_element.local_name().as_ref() == "row" => {
                    self.st.row_index += 1;
                    self.st.col_index = 0;
                }
                Ok(Event::Start(c_element)) if c_element.local_name().as_ref() == "c" => {
                    let pos = if let Some(r) = c_element.raw_attr("r")? {
                        let (row, col) = get_row_column(r)?;
                        self.st.col_index = col;
                        (row, col)
                    } else {
                        (self.st.row_index, self.st.col_index)
                    };
                    let mut value = None;
                    loop {
                        self.st.cell_buf.clear();
                        match self.xml.read_event_into(&mut self.st.cell_buf) {
                            Ok(Event::Start(e)) => {
                                let formula_record = read_formula_record(
                                    &mut self.xml,
                                    &mut self.st.formulas,
                                    &mut None,
                                    &e,
                                    pos,
                                    true,
                                )?;
                                if let Some(formula_text) = formula_record
                                    .and_then(|record| record.formula_text().map(str::to_string))
                                {
                                    value = Some(formula_text);
                                }
                            }
                            Ok(Event::End(e)) if e.local_name().as_ref() == "c" => break,
                            Ok(Event::Eof) => return Err(XlsxError::XmlEof("c")),
                            Err(e) => return Err(XlsxError::Xml(e)),
                            _ => (),
                        }
                    }
                    self.st.col_index += 1;
                    return Ok(Some(Cell::new(pos, value.unwrap_or_default())));
                }
                Ok(Event::End(e)) if e.local_name().as_ref() == "sheetData" => {
                    return Ok(None);
                }
                Ok(Event::Eof) => return Err(XlsxError::XmlEof("sheetData")),
                Err(e) => return Err(XlsxError::Xml(e)),
                _ => (),
            }
        }
    }
}

/// hedgehog: a free function over any XML reader; in a chunk read (`chunk`, `parallel.rs`) a
/// shared index too large to track makes the chunk depend on its context.
fn read_formula_record<R: BufRead>(
    xml: &mut XmlReader<R>,
    formulas: &mut Vec<Option<SharedFormula>>,
    chunk: &mut Option<ChunkFlags>,
    e: &BytesStart<'_>,
    pos: (u32, u32),
    expand_shared_derived: bool,
) -> Result<Option<FormulaMetadata>, XlsxError> {
    let formula = read_formula(xml, e)?;

    let (t_attr, si_attr, ref_attr) = get_attrs!(e, "t" => t, "si" => si, "ref" => ref_)?;
    if t_attr == Some("shared") {
        let shared_index = match si_attr {
            Some(res) => match atoi_simd::parse::<usize, true, false>(res.as_bytes()) {
                Ok(res) => res,
                Err(_) => return Err(XlsxError::Unexpected("si attribute must be a number")),
            },
            None => {
                return Err(XlsxError::Unexpected(
                    "si attribute is mandatory if it is shared",
                ));
            }
        };

        return match ref_attr {
            Some(res) => {
                let range = get_dimension(res)?;
                let formula = formula.unwrap_or_default();
                if let Some(c) = chunk.as_mut().filter(|c| !c.in_order && shared_index >= CHUNK_MAX_SHARED_INDEX) {
                    c.needs_context = true;
                } else if expand_shared_derived {
                    if formulas.len() <= shared_index {
                        formulas.resize(shared_index + 1, None);
                    }
                    formulas[shared_index] = Some(SharedFormula {
                        formula: formula.clone(),
                        range,
                    });
                }
                Ok(Some(FormulaMetadata::Shared {
                    shared_index,
                    range: Some(range),
                    formula,
                }))
            }
            None => {
                let translated_formula = if expand_shared_derived {
                    // hedgehog: in a chunk read, an anchor not seen yet is looked up later
                    // (`XlsxCellFormulaMetadataRecordInternal::unresolved_shared`)
                    formulas
                        .get(shared_index)
                        .and_then(|template| template.as_ref())
                        .map(|template| {
                            expand_shared_formula(&template.formula, template.range.start, pos)
                        })
                        .transpose()?
                } else {
                    None
                };
                Ok(Some(FormulaMetadata::SharedDerived {
                    shared_index,
                    translated_formula,
                }))
            }
        };
    }

    Ok(formula.map(|formula| FormulaMetadata::Normal { formula }))
}


/// hedgehog: the body of [`XlsxCellReader::next_cell_with_formula`] over any XML reader and a
/// separate [`CellState`]. With `st.chunk` set, the end of the input outside an element ends the
/// chunk (`at_eof`) instead of being an error, and uses of state the chunk does not have are
/// recorded (`needs_context`).
pub(crate) fn next_cell_formula_record<'s, R: BufRead>(
    xml: &mut XmlReader<R>,
    st: &mut CellState,
    ctx: &WorkbookContext<'s>,
    expand_shared_derived: bool,
) -> Result<Option<XlsxCellFormulaMetadataRecordInternal<'s>>, XlsxError> {
    loop {
        st.buf.clear();
        let after_row = st.chunk.as_mut().is_some_and(|c| std::mem::take(&mut c.last_row_end));
        match xml.read_event_into(&mut st.buf) {
            Ok(Event::Start(row_element)) if row_element.local_name().as_ref() == "row" => {
                if let Some(r) = row_element.raw_attr("r")? {
                    st.row_index = get_row(r)?;
                    if let Some(c) = st.chunk.as_mut() {
                        c.row_known = true;
                    }
                }
            }
            Ok(Event::End(row_element)) if row_element.local_name().as_ref() == "row" => {
                st.row_index += 1;
                st.col_index = 0;
                if let Some(c) = st.chunk.as_mut() {
                    c.needs_context |= !c.row_known;
                    c.last_row_end = true;
                }
            }
            Ok(Event::Start(c_element)) if c_element.local_name().as_ref() == "c" => {
                let (pos_attr, style_attr, type_attr) =
                    get_attrs!(c_element, "r" => r, "s" => s, "t" => t)?;
                let pos = if let Some(range) = pos_attr {
                    let (row, col) = get_row_column(range)?;
                    st.col_index = col;
                    (row, col)
                } else {
                    if let Some(c) = st.chunk.as_mut() {
                        c.needs_context |= !c.row_known;
                    }
                    (st.row_index, st.col_index)
                };
                let mut value = DataRef::Empty;
                let mut formula = None;
                loop {
                    st.cell_buf.clear();
                    match xml.read_event_into(&mut st.cell_buf) {
                        Ok(Event::Start(e)) if e.local_name().as_ref() == "f" => {
                            formula = read_formula_record(
                                xml,
                                &mut st.formulas,
                                &mut st.chunk,
                                &e,
                                pos,
                                expand_shared_derived,
                            )?;
                        }
                        Ok(Event::Start(e)) => {
                            value = read_value(
                                ctx,
                                xml,
                                &e,
                                style_attr,
                                type_attr,
                                &mut st.value_bufs,
                            )?;
                        }
                        Ok(Event::End(e)) if e.local_name().as_ref() == "c" => break,
                        Ok(Event::Eof) => return Err(XlsxError::XmlEof("c")),
                        Err(e) => return Err(XlsxError::Xml(e)),
                        _ => (),
                    }
                }
                st.col_index += 1;
                return Ok(Some(XlsxCellFormulaMetadataRecordInternal {
                    pos,
                    value,
                    formula,
                }));
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "sheetData" => {
                return Ok(None);
            }
            Ok(Event::Eof) => {
                if let Some(c) = st.chunk.as_mut() {
                    c.at_eof = true;
                    c.last_row_end = after_row;
                    return Ok(None);
                }
                return Err(XlsxError::XmlEof("sheetData"));
            }
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => (),
        }
    }
}

/// Reads a cell value using pre-extracted `s` and `t` attributes
/// (avoids repeating attribute iteration on the `<c>` element).
fn read_value<'s, R>(
    ctx: &WorkbookContext<'s>,
    xml: &mut XmlReader<R>,
    e: &BytesStart<'_>,
    style_attr: Option<&str>,
    type_attr: Option<&str>,
    bufs: &mut ValueBufs,
) -> Result<DataRef<'s>, XlsxError>
where
    R: BufRead, // hedgehog: any XML reader (was the zip part's)
{
    Ok(match e.local_name().as_ref() {
        "is" => {
            // inlineStr
            read_string_with_bufs(xml, e.name(), &mut bufs.xml, &mut bufs.str_inner)?
                .map_or(DataRef::Empty, DataRef::String)
        }
        // Ignore <v> for inlineStr cells since it is redundant. The value is in
        // the <is> element, which is handled above.
        "v" if matches!(type_attr, Some("inlineStr") | Some("is")) => {
            bufs.xml.clear();
            xml.read_to_end_into(e.name(), &mut bufs.xml)?;
            DataRef::Empty
        }
        "v" => match type_attr {
            Some("n") | Some("s") | Some("b") | Some("e") | None => {
                // These types are always plain ASCII (no CR/LF or entities), so we can
                // parse directly from raw bytes, skipping `xml10_content()` + String
                bufs.xml.clear();
                let val = match xml.read_event_into(&mut bufs.xml)? {
                    Event::Text(t) => read_v(ctx, t.as_ref(), style_attr, type_attr)?,
                    Event::End(end) if end.name() == e.name() => return Ok(DataRef::Empty),
                    Event::Eof => return Err(XlsxError::XmlEof("v")),
                    _ => DataRef::Empty,
                };
                bufs.xml.clear();
                xml.read_to_end_into(e.name(), &mut bufs.xml)?;
                val
            }
            _ => {
                // Types that may contain entities, or need owned Strings (eg: "str", "d")
                bufs.value.clear();
                loop {
                    bufs.xml.clear();
                    match xml.read_event_into(&mut bufs.xml)? {
                        Event::Text(t) => bufs.value.push_str(&t.xml10_content()),
                        Event::GeneralRef(e) => unescape_entity_to_buffer(&e, &mut bufs.value)?,
                        Event::End(end) if end.name() == e.name() => break,
                        Event::Eof => return Err(XlsxError::XmlEof("v")),
                        _ => (),
                    }
                }
                read_v(ctx, &bufs.value, style_attr, type_attr)?
            }
        },
        "f" => {
            bufs.xml.clear();
            xml.read_to_end_into(e.name(), &mut bufs.xml)?;
            DataRef::Empty
        }
        _n => return Err(XlsxError::UnexpectedNode("v, f, or is")),
    })
}

/// Parse a `<v>` cell value from raw text with pre-extracted
/// `s` (style) and `t` (type) attributes.
fn read_v<'s>(
    ctx: &WorkbookContext<'s>,
    v: &str,
    style_attr: Option<&str>,
    type_attr: Option<&str>,
) -> Result<DataRef<'s>, XlsxError> {
    let cell_format = match style_attr {
        Some(style) => {
            let id = atoi_simd::parse::<usize, true, false>(style.as_bytes()).unwrap_or(0);
            ctx.formats.get(id)
        }
        None => Some(&CellFormat::Other),
    };
    match type_attr {
        Some("s") => {
            if v.is_empty() {
                return Ok(DataRef::Empty);
            }
            let idx = atoi_simd::parse::<usize, true, false>(v.as_bytes()).unwrap_or(0);
            ctx.strings
                .get(idx)
                .map(|s| DataRef::SharedString(s))
                .ok_or(XlsxError::Unexpected(
                    "Cell string index not found in shared strings table",
                ))
        }
        Some("b") => read_bool(v),
        Some("d") => Ok(DataRef::DateTimeIso(v.to_string())),
        Some("e") => Ok(DataRef::Error(v.parse()?)),
        Some("str") => Ok(DataRef::String(unescape_xml(v).into_owned())),
        Some("n") | None => {
            if v.is_empty() {
                return Ok(DataRef::Empty);
            }
            // If type is not known, we try to parse as Float for utility, but fall back to
            // String if this fails.
            fast_float2::parse::<f64, _>(v.as_bytes())
                .map(|n| format_excel_f64_ref(n, cell_format, ctx.is_1904))
                .or_else(|_| {
                    if type_attr.is_none() {
                        // No explicit type: fall back to String if not a valid float
                        Ok(DataRef::String(v.to_string()))
                    } else {
                        Err(XlsxError::ParseFloat(v.parse::<f64>().unwrap_err()))
                    }
                })
        }
        Some(t) => Err(XlsxError::CellTAttribute(t.to_string())),
    }
}

fn read_formula<R>(xml: &mut XmlReader<R>, e: &BytesStart) -> Result<Option<String>, XlsxError>
where
    R: BufRead, // hedgehog: any XML reader (was the zip part's)
{
    match e.local_name().as_ref() {
        "is" | "v" => {
            xml.read_to_end_into(e.name(), &mut Vec::new())?;
            Ok(None)
        }
        "f" => {
            let mut f_buf = Vec::with_capacity(512);
            let mut f = String::new();
            loop {
                match xml.read_event_into(&mut f_buf)? {
                    Event::Text(t) => f.push_str(&t.xml10_content()),
                    Event::GeneralRef(e) => unescape_entity_to_buffer(&e, &mut f)?,
                    Event::End(end) if end.name() == e.name() => break,
                    Event::Eof => return Err(XlsxError::XmlEof("f")),
                    _ => (),
                }
                f_buf.clear();
            }
            Ok(Some(f))
        }
        _ => Err(XlsxError::UnexpectedNode("v, f, or is")),
    }
}

/// Parse the `<v>` value of a `t="b"` boolean cell.
///
/// Excel only writes `0` or `1` for booleans but the value is defined in an XML
/// Schema boolean so `false` and `true` are also accepted. Any other value is
/// an error.
fn read_bool<'s>(v: &str) -> Result<DataRef<'s>, XlsxError> {
    let value = v.trim();

    if value.is_empty() {
        Ok(DataRef::Empty)
    } else if value == "0" || value.eq_ignore_ascii_case("false") {
        Ok(DataRef::Bool(false))
    } else if value == "1" || value.eq_ignore_ascii_case("true") {
        Ok(DataRef::Bool(true))
    } else {
        Err(XlsxError::Unrecognized {
            typ: "boolean cell value",
            val: v.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test that boolean cell values accept the XML Schema boolean forms and
    // reject anything else instead of decoding it as true. See issue #724.
    #[test]
    fn test_read_bool_cell_values() {
        for value in ["0", "false", "FALSE", "False", " 0 "] {
            assert_eq!(read_bool(value).unwrap(), DataRef::Bool(false), "{value:?}");
        }

        for value in ["1", "true", "TRUE", "True", " 1 "] {
            assert_eq!(read_bool(value).unwrap(), DataRef::Bool(true), "{value:?}");
        }

        assert_eq!(read_bool("").unwrap(), DataRef::Empty);
        assert_eq!(read_bool("  ").unwrap(), DataRef::Empty);

        for value in ["00", "0.0", "-1", "2", "banana", "#REF!"] {
            assert!(
                matches!(
                    read_bool(value),
                    Err(XlsxError::Unrecognized { typ: "boolean cell value", ref val }) if val == value
                ),
                "{value:?}"
            );
        }
    }
}
