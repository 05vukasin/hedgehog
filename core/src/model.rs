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
    use super::a1;

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
