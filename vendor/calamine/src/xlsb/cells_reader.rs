// SPDX-License-Identifier: MIT
//
// Copyright 2016-2026, Johann Tuffe.

use std::io::{Read, Seek};

use crate::{
    datatype::DataRef,
    formats::{format_excel_f64_ref, CellFormat},
    utils::{read_f64, read_i32, read_u32, read_usize},
    Cell, CellErrorType, Dimensions, XlsbError,
};

use super::{byte, cell_format, parse_formula, slice, tail, wide_str, RecordIter};

/// A cells reader for xlsb files
pub struct XlsbCellsReader<'a, RS>
where
    RS: Read + Seek,
{
    iter: RecordIter<'a, RS>,
    formats: &'a [CellFormat],
    strings: &'a [String],
    extern_sheets: &'a [String],
    metadata_names: &'a [(String, String)],
    typ: u16,
    row: u32,
    is_1904: bool,
    dimensions: Dimensions,
    buf: Vec<u8>,
}

impl<'a, RS> XlsbCellsReader<'a, RS>
where
    RS: Read + Seek,
{
    pub(crate) fn new(
        mut iter: RecordIter<'a, RS>,
        formats: &'a [CellFormat],
        strings: &'a [String],
        extern_sheets: &'a [String],
        metadata_names: &'a [(String, String)],
        is_1904: bool,
    ) -> Result<Self, XlsbError> {
        let mut buf = Vec::with_capacity(1024);
        // BrtWsDim
        let len = iter.next_skip_blocks(
            0x0094,
            &[
                (0x0081, None), // BrtBeginSheet
                (0x0093, None), // BrtWsProp
            ],
            &mut buf,
        )?;
        // hedgehog: bounds-checked, in the record (a BrtWsDim shorter than 16 bytes panicked)
        let dimensions = parse_dimensions(slice(&buf[..len], 0, 16, "BrtWsDim")?);

        // BrtBeginSheetData
        let _ = iter.next_skip_blocks(
            0x0091,
            &[
                (0x0085, Some(0x0086)), // Views
                (0x0025, Some(0x0026)), // AC blocks
                (0x01E5, None),         // BrtWsFmtInfo
                (0x0186, Some(0x0187)), // Col Infos
            ],
            &mut buf,
        )?;

        Ok(XlsbCellsReader {
            iter,
            formats,
            is_1904,
            strings,
            extern_sheets,
            metadata_names,
            dimensions,
            typ: 0,
            row: 0,
            buf,
        })
    }

    pub fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    pub fn next_cell(&mut self) -> Result<Option<Cell<DataRef<'a>>>, XlsbError> {
        // loop until end of sheet
        let (value, col) = loop {
            self.typ = self.iter.read_type()?;
            let len = self.iter.fill_buffer(&mut self.buf)?;
            // hedgehog: fields are read from the record (`rec`), each bounds-checked (`slice`,
            // `tail`, `byte`). They were read from the whole buffer, which keeps the bytes of
            // longer earlier records: a short record read stale bytes (an empty BrtCellSt
            // copied an earlier string, a cheap way to fill memory) or, past them, panicked.
            // A shared string index past the table panicked too.
            let rec = &self.buf[..len];
            let value = match self.typ {
                // 0x0001 => continue, // Data::Empty, // BrtCellBlank
                0x0002 => {
                    // BrtCellRk MS-XLSB 2.5.122
                    let mut rk = [0u8; 4];
                    rk.copy_from_slice(slice(rec, 8, 12, "BrtCellRk")?);
                    let d100 = (rk[0] & 1) != 0;
                    let is_int = (rk[0] & 2) != 0;
                    rk[0] &= 0xFC;

                    if is_int {
                        let v = (read_i32(&rk) >> 2) as i64;
                        if d100 {
                            let v = (v as f64) / 100.0;
                            format_excel_f64_ref(v, cell_format(self.formats, rec), self.is_1904)
                        } else {
                            DataRef::Int(v)
                        }
                    } else {
                        let mut v = [0u8; 8];
                        v[4..].copy_from_slice(&rk);
                        let v = read_f64(&v);
                        let v = if d100 { v / 100.0 } else { v };
                        format_excel_f64_ref(v, cell_format(self.formats, rec), self.is_1904)
                    }
                }
                0x0003 => {
                    let error = match byte(rec, 8, "BrtCellError")? {
                        0x00 => CellErrorType::Null,
                        0x07 => CellErrorType::Div0,
                        0x0F => CellErrorType::Value,
                        0x17 => CellErrorType::Ref,
                        0x1D => CellErrorType::Name,
                        0x24 => CellErrorType::Num,
                        0x2A => CellErrorType::NA,
                        0x2B => CellErrorType::GettingData,
                        c => return Err(XlsbError::CellError(c)),
                    };
                    // BrtCellError
                    DataRef::Error(error)
                }
                0x0004 | 0x000A => DataRef::Bool(byte(rec, 8, "BrtCellBool")? != 0), // BrtCellBool or BrtFmlaBool
                0x0005 | 0x0009 => {
                    let v = read_f64(slice(rec, 8, 16, "BrtCellReal")?);
                    format_excel_f64_ref(v, cell_format(self.formats, rec), self.is_1904)
                } // BrtCellReal or BrtFmlaNum
                0x0006 | 0x0008 => {
                    DataRef::String(wide_str(tail(rec, 8, "BrtCellSt")?, &mut 0)?.into_owned())
                } // BrtCellSt or BrtFmlaString
                0x0007 => {
                    // BrtCellIsst
                    let isst = read_usize(slice(rec, 8, 12, "BrtCellIsst")?);
                    let strings = self.strings;
                    let s = strings.get(isst).ok_or(XlsbError::Len {
                        expected: isst + 1,
                        found: strings.len(),
                        typ: "BrtCellIsst (shared strings)",
                    })?;
                    DataRef::SharedString(s)
                }
                0x0000 => {
                    // BrtRowHdr
                    self.row = read_u32(rec);
                    if self.row > 0x0010_0000 {
                        return Ok(None); // invalid row
                    }
                    continue;
                }
                0x0092 => return Ok(None), // BrtEndSheetData
                _ => continue, // anything else, ignore and try next, without changing idx
            };
            break (value, column(rec)?);
        };
        Ok(Some(Cell::new((self.row, col), value)))
    }

    pub fn next_formula(&mut self) -> Result<Option<Cell<String>>, XlsbError> {
        let (value, col) = loop {
            self.typ = self.iter.read_type()?;
            let len = self.iter.fill_buffer(&mut self.buf)?;
            // hedgehog: read from the record, as in `next_cell`. The string length `cch` and
            // the token count `cce` come from the record; every slice they make is
            // bounds-checked (past the record, they panicked).
            let rec = &self.buf[..len];
            let (extern_sheets, metadata_names) = (self.extern_sheets, self.metadata_names);
            let formula = |rgce: Result<&[u8], XlsbError>| {
                rgce.and_then(|rgce| parse_formula(rgce, extern_sheets, metadata_names))
            };
            let value = match self.typ {
                // 0x0001 => continue, // Data::Empty, // BrtCellBlank
                // hedgehog: a formula that cannot be read (a token array cut short or past the
                // record, an unsupported token, nesting or length past the bounds) is an empty
                // formula, which callers skip: the cell keeps its cached value and the rest of the
                // sheet still reads. It failed the whole workbook (issue #4). The record itself
                // is whole (its length came from the header), so the next one is in place.
                0x0008 => {
                    // BrtFmlaString
                    formula(tail(rec, 8, "BrtFmlaString").and_then(|cch| {
                        let cch = read_u32(cch) as usize;
                        let at = cch.saturating_mul(2).saturating_add(14);
                        rgce(tail(rec, at, "BrtFmlaString")?, "BrtFmlaString.rgce")
                    }))
                    .unwrap_or_default()
                }
                0x0009 => {
                    // BrtFmlaNum
                    formula(
                        tail(rec, 18, "BrtFmlaNum").and_then(|f| rgce(f, "BrtFmlaNum.rgce")),
                    )
                    .unwrap_or_default()
                }
                0x000A | 0x000B => {
                    // BrtFmlaBool | BrtFmlaError
                    formula(
                        tail(rec, 11, "BrtFmlaBool").and_then(|f| rgce(f, "BrtFmlaBool.rgce")),
                    )
                    .unwrap_or_default()
                }
                0x0000 => {
                    // BrtRowHdr
                    self.row = read_u32(rec);
                    if self.row > 0x0010_0000 {
                        return Ok(None); // invalid row
                    }
                    continue;
                }
                0x0092 => return Ok(None), // BrtEndSheetData
                _ => continue, // anything else, ignore and try next, without changing idx
            };
            break (value, column(rec)?);
        };
        Ok(Some(Cell::new((self.row, col), value)))
    }
}

/// hedgehog: the token array of a formula's `CellParsedFormula` (`cce`, then `cce` bytes),
/// bounds-checked.
fn rgce<'b>(formula: &'b [u8], typ: &'static str) -> Result<&'b [u8], XlsbError> {
    let cce = read_u32(formula) as usize;
    slice(formula, 4, cce.saturating_add(4), typ)
}

/// hedgehog: a cell's column (MS-XLSB 2.5.9 Cell: `column` is at most 0x3FFF, Excel's 16,384
/// columns). A larger one, up to u32::MAX, was passed on and overflowed hedgehog's A1 addresses
/// and layout; it is an error.
fn column(rec: &[u8]) -> Result<u32, XlsbError> {
    let col = read_u32(rec);
    if col > 0x3FFF {
        return Err(XlsbError::Unrecognized {
            typ: "Cell.column",
            val: col.to_string(),
        });
    }
    Ok(col)
}

fn parse_dimensions(buf: &[u8]) -> Dimensions {
    Dimensions {
        start: (read_u32(&buf[0..4]), read_u32(&buf[8..12])),
        end: (read_u32(&buf[4..8]), read_u32(&buf[12..16])),
    }
}
