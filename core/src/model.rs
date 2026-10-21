//! In-memory workbook model every later stage works on.
//! Coordinates are 0-based (row, col) in sheet space, independent of where the used range starts.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Number(f64),
    Text(String),
    /// Excel serial date (days since 1899-12-30), kept numeric so arithmetic stays exact.
    DateTime(f64),
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub row: u32,
    pub col: u32,
    /// `None` for a formula cell whose cached value was not saved, or an empty cell that only has a formula.
    pub value: Option<Value>,
    /// Formula text without the leading `=`.
    pub formula: Option<String>,
}

/// Inclusive rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Sheet {
    pub name: String,
    /// Non-empty cells only, sorted row-major.
    pub cells: Vec<Cell>,
    pub merges: Vec<Rect>,
    /// Formatting of each cell, same order as `cells`; empty when the file format carries no
    /// styles hedgehog reads (only xlsx / xlsm do).
    pub formats: Vec<Format>,
}

/// Horizontal alignment of a cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    General,
    Left,
    Center,
    Right,
    /// Justify, fill, distributed.
    Other,
}

/// What a cell's number format shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NumFmt {
    #[default]
    General,
    Integer,
    Decimal,
    /// Digit grouping (`#,##0`), with or without decimals.
    Thousands,
    Percent,
    Currency,
    /// Dates and times.
    Date,
    Text,
    /// Scientific, fractions, anything else.
    Other,
}

const ALIGNS: [Align; 5] = [Align::General, Align::Left, Align::Center, Align::Right, Align::Other];
const NUMFMTS: [NumFmt; 9] = [
    NumFmt::General,
    NumFmt::Integer,
    NumFmt::Decimal,
    NumFmt::Thousands,
    NumFmt::Percent,
    NumFmt::Currency,
    NumFmt::Date,
    NumFmt::Text,
    NumFmt::Other,
];

/// Cell formatting, unpacked ([`Format::pack`] / [`Format::unpack`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormatSpec {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Font colour other than the default (automatic / black / theme text).
    pub color: bool,
    /// A visible background fill.
    pub fill: bool,
    pub border_top: bool,
    pub border_bottom: bool,
    pub border_left: bool,
    pub border_right: bool,
    pub wrap: bool,
    pub align: Align,
    /// Indent level, 0..=15.
    pub indent: u8,
    pub num_fmt: NumFmt,
    /// Font size in half points (22 = 11 pt), 0..=511.
    pub size_half_pt: u16,
}

/// Cell formatting packed into 32 bits (4 bytes per cell). `Format(0)` = unknown.
///
/// Bits: 0 bold, 1 italic, 2 underline, 3 colour, 4 fill, 5–8 borders top / bottom / left /
/// right, 9 wrap, 10–12 alignment, 13–16 indent, 17–20 number format, 21–29 font size (half
/// points), 31 known.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Format(pub u32);

impl Format {
    const KNOWN: u32 = 1 << 31;

    pub fn pack(s: &FormatSpec) -> Format {
        let flags = [
            s.bold,
            s.italic,
            s.underline,
            s.color,
            s.fill,
            s.border_top,
            s.border_bottom,
            s.border_left,
            s.border_right,
            s.wrap,
        ];
        let mut x = Self::KNOWN;
        for (k, &on) in flags.iter().enumerate() {
            x |= (on as u32) << k;
        }
        let align = ALIGNS.iter().position(|&a| a == s.align).unwrap_or(0) as u32;
        let num = NUMFMTS.iter().position(|&n| n == s.num_fmt).unwrap_or(0) as u32;
        x |= align << 10 | (s.indent.min(15) as u32) << 13 | num << 17 | (s.size_half_pt.min(511) as u32) << 21;
        Format(x)
    }

    pub fn known(self) -> bool {
        self.0 & Self::KNOWN != 0
    }

    /// `None` when unknown.
    pub fn unpack(self) -> Option<FormatSpec> {
        if !self.known() {
            return None;
        }
        let x = self.0;
        let bit = |k: u32| x >> k & 1 == 1;
        Some(FormatSpec {
            bold: bit(0),
            italic: bit(1),
            underline: bit(2),
            color: bit(3),
            fill: bit(4),
            border_top: bit(5),
            border_bottom: bit(6),
            border_left: bit(7),
            border_right: bit(8),
            wrap: bit(9),
            align: ALIGNS.get((x >> 10 & 7) as usize).copied().unwrap_or_default(),
            indent: (x >> 13 & 15) as u8,
            num_fmt: NUMFMTS.get((x >> 17 & 15) as usize).copied().unwrap_or_default(),
            size_half_pt: (x >> 21 & 511) as u16,
        })
    }
}

impl Sheet {
    /// Bounding box of all non-empty cells and merged areas (a merged title stores its text only
    /// in its top-left cell but visibly spans the whole area). `None` for an empty sheet.
    pub fn used_range(&self) -> Option<Rect> {
        let first = self.cells.first()?;
        let mut r = Rect { top: first.row, left: first.col, bottom: first.row, right: first.col };
        for c in &self.cells {
            r.left = r.left.min(c.col);
            r.right = r.right.max(c.col);
            r.bottom = r.bottom.max(c.row);
        }
        for m in &self.merges {
            r.top = r.top.min(m.top);
            r.left = r.left.min(m.left);
            r.bottom = r.bottom.max(m.bottom);
            r.right = r.right.max(m.right);
        }
        Some(r)
    }

    pub fn formula_count(&self) -> usize {
        self.cells.iter().filter(|c| c.formula.is_some()).count()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
}

/// `A1`-style address for a 0-based (row, col).
pub fn a1(row: u32, col: u32) -> String {
    let mut letters = Vec::new();
    let mut c = col + 1;
    while c > 0 {
        let rem = (c - 1) % 26;
        letters.push(b'A' + rem as u8);
        c = (c - 1) / 26;
    }
    letters.reverse();
    format!("{}{}", String::from_utf8(letters).unwrap(), row + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_pack_and_unpack() {
        assert_eq!(Format::default().unpack(), None);
        let s = FormatSpec {
            bold: true,
            border_right: true,
            wrap: true,
            align: Align::Other,
            indent: 15,
            num_fmt: NumFmt::Other,
            size_half_pt: 511,
            ..FormatSpec::default()
        };
        assert_eq!(Format::pack(&s).unpack(), Some(s));
        assert_eq!(Format::pack(&FormatSpec::default()).unpack(), Some(FormatSpec::default()));
    }

    #[test]
    fn a1_addresses() {
        assert_eq!(a1(0, 0), "A1");
        assert_eq!(a1(42, 3), "D43");
        assert_eq!(a1(0, 25), "Z1");
        assert_eq!(a1(0, 26), "AA1");
        assert_eq!(a1(9, 701), "ZZ10");
        assert_eq!(a1(0, 702), "AAA1");
    }
}
