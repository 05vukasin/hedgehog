//! Random access over a sheet's sparse cells, shared by every signal and layout module.

use std::collections::HashMap;

use crate::model::{Cell, Rect, Sheet};

pub struct Grid<'a> {
    pub sheet: &'a Sheet,
    index: HashMap<(u32, u32), usize>,
    /// Every position covered by a merged area → index into `sheet.merges`.
    merged: HashMap<(u32, u32), usize>,
}

impl<'a> Grid<'a> {
    pub fn new(sheet: &'a Sheet) -> Self {
        let index = sheet.cells.iter().enumerate().map(|(i, c)| ((c.row, c.col), i)).collect();
        let mut merged = HashMap::new();
        for (i, m) in sheet.merges.iter().enumerate() {
            for r in m.top..=m.bottom {
                for c in m.left..=m.right {
                    merged.insert((r, c), i);
                }
            }
        }
        Grid { sheet, index, merged }
    }

    pub fn cell(&self, row: u32, col: u32) -> Option<&'a Cell> {
        self.index.get(&(row, col)).map(|&i| &self.sheet.cells[i])
    }

    /// Index of the cell at (row, col) in `sheet.cells` — the row index into [`crate::Features`].
    pub fn cell_index(&self, row: u32, col: u32) -> Option<usize> {
        self.index.get(&(row, col)).copied()
    }

    /// Merged area covering (row, col), if any.
    pub fn merge_at(&self, row: u32, col: u32) -> Option<Rect> {
        self.merged.get(&(row, col)).map(|&i| self.sheet.merges[i])
    }

    /// True if the position holds a value/formula or is covered by a merged area.
    pub fn is_filled(&self, row: u32, col: u32) -> bool {
        self.index.contains_key(&(row, col)) || self.merged.contains_key(&(row, col))
    }

    pub fn used_range(&self) -> Option<Rect> {
        self.sheet.used_range()
    }
}
