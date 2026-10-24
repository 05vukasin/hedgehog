//! 8-connected components of filled positions (cells + merged areas) — the "blocks" of a sheet.

use rustc_hash::FxHashMap as HashMap;

use crate::grid::Grid;
use crate::model::Rect;

/// A connected block of filled positions.
///
/// Connectivity is 8-connected (diagonal neighbours touch) over every position that holds a cell
/// or is covered by a merged area, so a merged title spanning several columns joins whatever touches
/// any part of it, and an *empty* merged area still bridges the cells around it.
/// No gap tolerance: one fully empty row or column separates two components.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    /// Bounding box of the component's cells and merged areas.
    pub rect: Rect,
    /// Indices into `sheet.cells`, ascending (row-major when the sheet is).
    pub cells: Vec<usize>,
    /// Indices into `sheet.merges` that belong to this component.
    pub merges: Vec<usize>,
}

/// Components plus the cell → component map.
#[derive(Debug, Clone, Default)]
pub struct Labeling {
    /// Components that contain at least one cell, sorted by (top, left) of their rect.
    /// Merged areas without any cell in their component are dropped.
    pub components: Vec<Component>,
    /// For every entry of `sheet.cells`: index into `components`.
    pub cell_component: Vec<u32>,
}

/// Connected blocks of the sheet, sorted top-to-bottom then left-to-right.
/// See [`Component`] for the connectivity rule. O(cells + merge perimeters).
pub fn components(grid: &Grid) -> Vec<Component> {
    shared_label(grid).components.clone()
}

/// Like [`components`], also returning which component each cell belongs to.
pub fn label(grid: &Grid) -> Labeling {
    shared_label(grid).clone()
}

/// Compute connectivity once, including when layout and model context request it
/// concurrently. The public owned APIs retain their independent-value contract.
pub(super) fn shared_label<'a>(grid: &'a Grid<'_>) -> &'a Labeling {
    grid.components.get_or_init(|| label_uncached(grid))
}

fn label_uncached(grid: &Grid) -> Labeling {
    let cells = &grid.sheet.cells;
    let merges = &grid.sheet.merges;
    let n = cells.len();
    let mut uf = UnionFind::new(n + merges.len());

    // Node of a merged area: the last merge listed with the same top-left corner.
    let merge_index: HashMap<(u32, u32), usize> =
        merges.iter().enumerate().map(|(i, r)| ((r.top, r.left), i)).collect();
    let merge_node = |r: u32, c: u32| -> Option<usize> {
        let rect = grid.merge_at(r, c)?;
        merge_index.get(&(rect.top, rect.left)).map(|&i| n + i)
    };
    let has_merges = !merges.is_empty();

    for (i, cell) in cells.iter().enumerate() {
        let (r, c) = (cell.row, cell.col);
        // Cell–cell: the four "earlier" neighbours suffice, the others see this cell later.
        for (dr, dc) in [(-1i64, -1i64), (-1, 0), (-1, 1), (0, -1)] {
            if let Some((nr, nc)) = offset(r, c, dr, dc)
                && let Some(j) = grid.cell_index(nr, nc)
            {
                uf.union(i, j);
            }
        }
        if has_merges {
            if let Some(m) = merge_node(r, c) {
                uf.union(i, m);
            }
            for (dr, dc) in NEIGHBOURS {
                if let Some((nr, nc)) = offset(r, c, dr, dc)
                    && let Some(m) = merge_node(nr, nc)
                {
                    uf.union(i, m);
                }
            }
        }
    }
    // Merge–merge adjacency: walk the ring around each merged area.
    for (k, m) in merges.iter().enumerate() {
        let node = n + k;
        if let Some(other) = merge_node(m.top, m.left) {
            uf.union(node, other); // overlapping (malformed) merges
        }
        let top = m.top.checked_sub(1);
        let left = m.left.checked_sub(1);
        let (bottom, right) = (m.bottom.saturating_add(1), m.right.saturating_add(1));
        let c0 = left.unwrap_or(m.left);
        for c in c0..=right {
            if let Some(t) = top
                && let Some(o) = merge_node(t, c)
            {
                uf.union(node, o);
            }
            if let Some(o) = merge_node(bottom, c) {
                uf.union(node, o);
            }
        }
        for r in m.top..=m.bottom {
            if let Some(l) = left
                && let Some(o) = merge_node(r, l)
            {
                uf.union(node, o);
            }
            if let Some(o) = merge_node(r, right) {
                uf.union(node, o);
            }
        }
    }

    // Root → component id, only for roots that own a cell.
    let mut root_comp: HashMap<usize, u32> = HashMap::default();
    let mut comps: Vec<Component> = Vec::new();
    let mut cell_component = Vec::with_capacity(n);
    for (i, cell) in cells.iter().enumerate() {
        let root = uf.find(i);
        let id = *root_comp.entry(root).or_insert_with(|| {
            comps.push(Component {
                rect: Rect { top: cell.row, left: cell.col, bottom: cell.row, right: cell.col },
                cells: Vec::new(),
                merges: Vec::new(),
            });
            (comps.len() - 1) as u32
        });
        let comp = &mut comps[id as usize];
        grow(&mut comp.rect, Rect { top: cell.row, left: cell.col, bottom: cell.row, right: cell.col });
        comp.cells.push(i);
        cell_component.push(id);
    }
    for (k, m) in merges.iter().enumerate() {
        if let Some(&id) = root_comp.get(&uf.find(n + k)) {
            let comp = &mut comps[id as usize];
            grow(&mut comp.rect, *m);
            comp.merges.push(k);
        }
    }

    // Sort by position and remap ids.
    let mut order: Vec<usize> = (0..comps.len()).collect();
    order.sort_by_key(|&i| (comps[i].rect.top, comps[i].rect.left));
    let mut new_id = vec![0u32; comps.len()];
    for (new, &old) in order.iter().enumerate() {
        new_id[old] = new as u32;
    }
    for id in &mut cell_component {
        *id = new_id[*id as usize];
    }
    let mut slots: Vec<Option<Component>> = comps.into_iter().map(Some).collect();
    let components = order.iter().map(|&i| slots[i].take().unwrap()).collect();
    Labeling { components, cell_component }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::model::{Cell, Sheet, Value};

    #[test]
    fn cached_labeling_matches_uncached_and_is_shared_across_threads() {
        let sheet = Sheet {
            cells: vec![
                Cell { row: 0, col: 0, value: Some(Value::Text("Header".into())), formula: None },
                Cell { row: 1, col: 1, value: Some(Value::Number(12.0)), formula: None },
                Cell { row: 4, col: 5, value: Some(Value::Number(13.0)), formula: None },
            ],
            merges: vec![Rect { top: 0, bottom: 0, left: 0, right: 2 }],
            ..Sheet::default()
        };
        let grid = Grid::new(&sheet);
        let expected = label_uncached(&grid);
        std::thread::scope(|scope| {
            let a = scope.spawn(|| shared_label(&grid));
            let b = scope.spawn(|| shared_label(&grid));
            let (a, b) = (a.join().unwrap(), b.join().unwrap());
            assert!(std::ptr::eq(a, b));
            assert_eq!(a.components, expected.components);
            assert_eq!(a.cell_component, expected.cell_component);
        });
        let mut owned = label(&grid);
        owned.components.clear();
        assert_eq!(components(&grid), expected.components);
    }
}

pub(crate) const NEIGHBOURS: [(i64, i64); 8] =
    [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)];

/// (row + dr, col + dc) if it stays on the sheet's non-negative quadrant.
pub(crate) fn offset(r: u32, c: u32, dr: i64, dc: i64) -> Option<(u32, u32)> {
    let nr = u32::try_from(r as i64 + dr).ok()?;
    let nc = u32::try_from(c as i64 + dc).ok()?;
    Some((nr, nc))
}

fn grow(r: &mut Rect, o: Rect) {
    r.top = r.top.min(o.top);
    r.left = r.left.min(o.left);
    r.bottom = r.bottom.max(o.bottom);
    r.right = r.right.max(o.right);
}

struct UnionFind {
    parent: Vec<usize>,
    size: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind { parent: (0..n).collect(), size: vec![1; n] }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (mut a, mut b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        if self.size[a] < self.size[b] {
            std::mem::swap(&mut a, &mut b);
        }
        self.parent[b] = a;
        self.size[a] += self.size[b];
    }
}
