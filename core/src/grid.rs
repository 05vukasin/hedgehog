//! Random access over a sheet's sparse cells, shared by every signal and layout module.
//!
//! Built once per sheet in O(cells + merges · min(height, width)); memory is proportional to the
//! number of cells, never to the area of the used range or of merged areas (a merge spanning
//! A1:XFD1048576 costs a few bytes).

use std::sync::OnceLock;

use rustc_hash::FxHashMap;

use crate::model::{Cell, Rect, Sheet};

pub struct Grid<'a> {
    pub sheet: &'a Sheet,
    /// (row, col) packed into a u64 → index into `sheet.cells`. For duplicate positions the last
    /// cell wins.
    index: FxHashMap<u64, u32>,
    merges: MergeIndex,
    used: Option<Rect>,
    /// Built on first use: per row / per column, the cell indices sorted by column / row.
    rows: OnceLock<Lines>,
    cols: OnceLock<Lines>,
}

#[inline]
fn key(row: u32, col: u32) -> u64 {
    ((row as u64) << 32) | col as u64
}

impl<'a> Grid<'a> {
    pub fn new(sheet: &'a Sheet) -> Self {
        let mut index = FxHashMap::with_capacity_and_hasher(sheet.cells.len(), Default::default());
        for (i, c) in sheet.cells.iter().enumerate() {
            index.insert(key(c.row, c.col), i as u32);
        }
        Grid {
            sheet,
            index,
            merges: MergeIndex::new(&sheet.merges),
            used: sheet.used_range(),
            rows: OnceLock::new(),
            cols: OnceLock::new(),
        }
    }

    #[inline]
    pub fn cell(&self, row: u32, col: u32) -> Option<&'a Cell> {
        let sheet: &'a Sheet = self.sheet;
        self.index.get(&key(row, col)).map(|&i| &sheet.cells[i as usize])
    }

    /// Index of the cell at (row, col) in `sheet.cells` — the row index into [`crate::Features`].
    #[inline]
    pub fn cell_index(&self, row: u32, col: u32) -> Option<usize> {
        self.index.get(&key(row, col)).map(|&i| i as usize)
    }

    /// Merged area covering (row, col), if any. For overlapping (malformed) merges, the one listed
    /// last in `sheet.merges`.
    #[inline]
    pub fn merge_at(&self, row: u32, col: u32) -> Option<Rect> {
        self.merge_index_at(row, col).map(|i| self.sheet.merges[i])
    }

    /// Index into `sheet.merges` of the merged area covering (row, col) — see [`Grid::merge_at`].
    #[inline]
    pub fn merge_index_at(&self, row: u32, col: u32) -> Option<usize> {
        self.merges.at(row, col)
    }

    /// True if the position holds a value/formula or is covered by a merged area.
    #[inline]
    pub fn is_filled(&self, row: u32, col: u32) -> bool {
        self.index.contains_key(&key(row, col)) || self.merges.at(row, col).is_some()
    }

    /// Bounding box of cells and merged areas ([`Sheet::used_range`]), computed once.
    #[inline]
    pub fn used_range(&self) -> Option<Rect> {
        self.used
    }

    /// Indices (into `sheet.cells`) of the cells in `row`, ordered by column.
    pub fn row_cells(&self, row: u32) -> &[u32] {
        self.rows.get_or_init(|| Lines::new(self, |c| (c.row, c.col))).get(row)
    }

    /// Indices (into `sheet.cells`) of the cells in `col`, ordered by row.
    pub fn col_cells(&self, col: u32) -> &[u32] {
        self.columns().get(col)
    }

    fn columns(&self) -> &Lines {
        self.cols.get_or_init(|| Lines::new(self, |c| (c.col, c.row)))
    }

    /// Indices (into `sheet.cells`) of the cells inside `rect`, row-major — the same cells, in the
    /// same order, as probing every position of `rect` with [`Grid::cell_index`], but in
    /// O(cells found · log) instead of O(area).
    pub fn cells_in(&self, rect: Rect) -> Vec<u32> {
        if rect.top > rect.bottom || rect.left > rect.right {
            return Vec::new();
        }
        let area = (rect.bottom - rect.top + 1) as u64 * (rect.right - rect.left + 1) as u64;
        if area <= 32 {
            let mut out = Vec::new();
            for r in rect.top..=rect.bottom {
                for c in rect.left..=rect.right {
                    if let Some(i) = self.index.get(&key(r, c)) {
                        out.push(*i);
                    }
                }
            }
            return out;
        }
        let cols = self.columns();
        let cells = &self.sheet.cells;
        let lo = cols.keys.partition_point(|&c| c < rect.left);
        let hi = cols.keys.partition_point(|&c| c <= rect.right);
        let mut out = Vec::new();
        for &col in &cols.keys[lo..hi] {
            let line = cols.get(col);
            let a = line.partition_point(|&i| cells[i as usize].row < rect.top);
            let b = line.partition_point(|&i| cells[i as usize].row <= rect.bottom);
            out.extend_from_slice(&line[a..b]);
        }
        if hi - lo > 1 {
            out.sort_unstable_by_key(|&i| (cells[i as usize].row, cells[i as usize].col));
        }
        out
    }
}

/// Cell indices grouped by a major coordinate (row or column), sorted by the minor one.
struct Lines {
    order: Vec<u32>,
    /// major → (start, end) in `order`.
    spans: FxHashMap<u32, (u32, u32)>,
    /// Every major coordinate that has cells, ascending.
    keys: Vec<u32>,
}

impl Lines {
    fn new(grid: &Grid, major_minor: impl Fn(&Cell) -> (u32, u32)) -> Lines {
        let cells = &grid.sheet.cells;
        // Only the cell the position index points at (duplicates: the last one).
        let mut order: Vec<u32> = (0..cells.len() as u32)
            .filter(|&i| {
                let c = &cells[i as usize];
                grid.index.get(&key(c.row, c.col)) == Some(&i)
            })
            .collect();
        let sorted = order.windows(2).all(|w| major_minor(&cells[w[0] as usize]) < major_minor(&cells[w[1] as usize]));
        if !sorted {
            order.sort_unstable_by_key(|&i| major_minor(&cells[i as usize]));
        }
        let mut spans = FxHashMap::default();
        let mut keys = Vec::new();
        let mut start = 0usize;
        while start < order.len() {
            let major = major_minor(&cells[order[start] as usize]).0;
            let mut end = start + 1;
            while end < order.len() && major_minor(&cells[order[end] as usize]).0 == major {
                end += 1;
            }
            spans.insert(major, (start as u32, end as u32));
            keys.push(major);
            start = end;
        }
        Lines { order, spans, keys }
    }

    fn get(&self, major: u32) -> &[u32] {
        self.spans.get(&major).map_or(&[], |&(a, b)| &self.order[a as usize..b as usize])
    }
}

/// Merged areas, indexed so that a lookup costs one hash probe plus a scan of the (few) merges
/// on that row or column. Short merges (height ≤ [`SPAN`]) are listed under every row they cover,
/// tall-but-narrow ones under every column; the rare merges that are both tall and wide are
/// scanned linearly.
#[derive(Default)]
struct MergeIndex {
    /// row → (left, right, merge index), sorted by `left`.
    by_row: FxHashMap<u32, Vec<(u32, u32, u32)>>,
    /// col → (top, bottom, merge index), sorted by `top`.
    by_col: FxHashMap<u32, Vec<(u32, u32, u32)>>,
    big: Vec<(Rect, u32)>,
    empty: bool,
}

const SPAN: u32 = 64;

impl MergeIndex {
    fn new(merges: &[Rect]) -> MergeIndex {
        let mut m = MergeIndex { empty: merges.is_empty(), ..Default::default() };
        for (i, r) in merges.iter().enumerate() {
            let i = i as u32;
            if r.bottom < r.top || r.right < r.left {
                continue; // covers nothing
            }
            if r.bottom - r.top < SPAN {
                for row in r.top..=r.bottom {
                    m.by_row.entry(row).or_default().push((r.left, r.right, i));
                }
            } else if r.right - r.left < SPAN {
                for col in r.left..=r.right {
                    m.by_col.entry(col).or_default().push((r.top, r.bottom, i));
                }
            } else {
                m.big.push((*r, i));
            }
        }
        for v in m.by_row.values_mut().chain(m.by_col.values_mut()) {
            v.sort_unstable();
        }
        m
    }

    #[inline]
    fn at(&self, row: u32, col: u32) -> Option<usize> {
        if self.empty {
            return None;
        }
        let mut best: Option<u32> = None;
        let mut scan = |list: Option<&Vec<(u32, u32, u32)>>, x: u32| {
            for &(lo, hi, i) in list.into_iter().flatten() {
                if lo > x {
                    break;
                }
                if x <= hi && best.is_none_or(|b| i > b) {
                    best = Some(i);
                }
            }
        };
        scan(self.by_row.get(&row), col);
        scan(self.by_col.get(&col), row);
        for &(r, i) in &self.big {
            if r.top <= row && row <= r.bottom && r.left <= col && col <= r.right && best.is_none_or(|b| i > b) {
                best = Some(i);
            }
        }
        best.map(|i| i as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Value;

    fn cell(row: u32, col: u32) -> Cell {
        Cell { row, col, value: Some(Value::Number(1.0)), formula: None }
    }

    #[test]
    fn merges_short_tall_big_and_overlapping() {
        let merges = vec![
            Rect { top: 0, left: 0, bottom: 0, right: 3 },                // short
            Rect { top: 2, left: 0, bottom: 500, right: 0 },              // tall, narrow
            Rect { top: 0, left: 10, bottom: 1_048_575, right: 16_383 },  // whole-sheet-ish
            Rect { top: 0, left: 2, bottom: 1, right: 2 },                // overlaps the first
        ];
        let sheet = Sheet { name: "s".into(), cells: vec![cell(0, 0), cell(5, 5)], merges };
        let g = Grid::new(&sheet);
        assert_eq!(g.merge_index_at(0, 1), Some(0));
        assert_eq!(g.merge_index_at(0, 2), Some(3)); // last listed wins
        assert_eq!(g.merge_index_at(1, 2), Some(3));
        assert_eq!(g.merge_index_at(300, 0), Some(1));
        assert_eq!(g.merge_index_at(501, 0), None);
        assert_eq!(g.merge_index_at(1_048_575, 16_383), Some(2));
        assert_eq!(g.merge_index_at(5, 5), None);
        assert!(g.is_filled(5, 5) && g.is_filled(7, 12) && !g.is_filled(7, 7));
        assert_eq!(g.used_range(), Some(Rect { top: 0, left: 0, bottom: 1_048_575, right: 16_383 }));
    }

    #[test]
    fn rows_and_columns_unsorted_with_duplicates() {
        let mut dup = cell(1, 1);
        dup.value = Some(Value::Number(2.0));
        let sheet = Sheet {
            name: "s".into(),
            cells: vec![cell(3, 0), cell(1, 1), cell(1, 0), dup, cell(0, 1)],
            merges: vec![],
        };
        let g = Grid::new(&sheet);
        assert_eq!(g.cell_index(1, 1), Some(3));
        assert_eq!(g.row_cells(1), &[2, 3]);
        assert_eq!(g.col_cells(1), &[4, 3]);
        assert_eq!(g.col_cells(0), &[2, 0]);
        assert!(g.row_cells(2).is_empty());
        let all = Rect { top: 0, left: 0, bottom: 1000, right: 1000 };
        assert_eq!(g.cells_in(all), vec![4, 2, 3, 0]);
        assert_eq!(g.cells_in(Rect { top: 1, left: 1, bottom: 3, right: 1 }), vec![3]);
        let small = Rect { top: 0, left: 0, bottom: 3, right: 1 };
        assert_eq!(g.cells_in(small), vec![4, 2, 3, 0]);
    }
}
