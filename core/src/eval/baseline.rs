//! Baseline analyzer: every 8-connected component of filled positions (values, formulas, merged
//! areas) holding ≥ 2 non-empty cells is a table (its bounding box); the table's first row is
//! Header, the rest Data; cells of smaller components are Other. The number every layout change
//! must beat.

use rustc_hash::FxHashMap as HashMap;

use crate::analysis::{DetectedTable, Role, SheetLayout};
use crate::grid::Grid;
use crate::model::Rect;

pub fn analyze(grid: &Grid) -> SheetLayout {
    let sheet = grid.sheet;
    // Clip merges to the cells' bounding box (± 1).
    let Some(first) = sheet.cells.first() else { return SheetLayout::default() };
    let mut bb = Rect { top: first.row, left: first.col, bottom: first.row, right: first.col };
    for c in &sheet.cells {
        bb.left = bb.left.min(c.col);
        bb.right = bb.right.max(c.col);
        bb.bottom = bb.bottom.max(c.row);
    }
    let clipped: Vec<Rect> = sheet
        .merges
        .iter()
        .map(|m| Rect {
            top: m.top.max(bb.top.saturating_sub(1)),
            left: m.left.max(bb.left.saturating_sub(1)),
            bottom: m.bottom.min(bb.bottom + 1),
            right: m.right.min(bb.right + 1),
        })
        .filter(|m| m.top <= m.bottom && m.left <= m.right)
        .collect();
    let area: u64 = clipped.iter().map(|m| (m.bottom - m.top + 1) as u64 * (m.right - m.left + 1) as u64).sum();
    // Flood fill over positions is fastest for ordinary sheets; when merged areas cover many
    // positions (A1:XFD1048576 with cells far apart: 17G) union-find over cells and whole merges
    // gives the same result in O(cells + merges).
    if area <= 4 * sheet.cells.len() as u64 + (1 << 20) {
        flood(grid, &clipped)
    } else {
        by_union_find(grid, &clipped)
    }
}

fn flood(grid: &Grid, clipped: &[Rect]) -> SheetLayout {
    let sheet = grid.sheet;
    // Position → component id (usize::MAX = not yet visited). Covers cells and merged areas.
    let mut comp: HashMap<(u32, u32), usize> = HashMap::default();
    for c in &sheet.cells {
        comp.insert((c.row, c.col), usize::MAX);
    }
    for m in clipped {
        for r in m.top..=m.bottom {
            for c in m.left..=m.right {
                comp.insert((r, c), usize::MAX);
            }
        }
    }

    let mut rects: Vec<Rect> = Vec::new();
    let mut stack: Vec<(u32, u32)> = Vec::new();
    // Seed in row-major cell order so component ids are deterministic.
    let mut seeds: Vec<(u32, u32)> = comp.keys().copied().collect();
    seeds.sort_unstable();
    for seed in seeds {
        if comp[&seed] != usize::MAX {
            continue;
        }
        let id = rects.len();
        let mut rect = Rect { top: seed.0, left: seed.1, bottom: seed.0, right: seed.1 };
        comp.insert(seed, id);
        stack.push(seed);
        while let Some((r, c)) = stack.pop() {
            rect.top = rect.top.min(r);
            rect.bottom = rect.bottom.max(r);
            rect.left = rect.left.min(c);
            rect.right = rect.right.max(c);
            for dr in -1i64..=1 {
                for dc in -1i64..=1 {
                    let (nr, nc) = (r as i64 + dr, c as i64 + dc);
                    if (dr, dc) == (0, 0) || nr < 0 || nc < 0 {
                        continue;
                    }
                    let n = (nr as u32, nc as u32);
                    if let Some(v) = comp.get_mut(&n)
                        && *v == usize::MAX
                    {
                        *v = id;
                        stack.push(n);
                    }
                }
            }
        }
        rects.push(rect);
    }

    let mut cell_count = vec![0usize; rects.len()];
    for c in &sheet.cells {
        cell_count[comp[&(c.row, c.col)]] += 1;
    }
    let is_table: Vec<bool> = cell_count.iter().map(|&n| n >= 2).collect();

    let roles = sheet
        .cells
        .iter()
        .map(|c| {
            let id = comp[&(c.row, c.col)];
            if !is_table[id] {
                Role::Other
            } else if c.row == rects[id].top {
                Role::Header
            } else {
                Role::Data
            }
        })
        .collect();

    let tables = rects
        .iter()
        .zip(&is_table)
        .filter(|(_, t)| **t)
        .map(|(r, _)| DetectedTable { rect: *r, header_rows: 1, confidence: 0.5 })
        .collect();

    SheetLayout { tables, roles }
}

/// Same components as [`flood`], with cells and (clipped) merged areas as union-find nodes:
/// positions are 8-connected, so a cell joins a merge iff it lies in the merge grown by one, and
/// two merges join iff one grown by one meets the other. Components are ordered by their first
/// position in row-major order, as the flood fill's sorted seeds order them.
fn by_union_find(grid: &Grid, clipped: &[Rect]) -> SheetLayout {
    let sheet = grid.sheet;
    let cells = &sheet.cells;
    let n = cells.len();
    let mut uf = UnionFind::new(n + clipped.len());
    for (i, c) in cells.iter().enumerate() {
        if let Some(j) = grid.cell_index(c.row, c.col) {
            uf.union(i, j); // duplicate positions are one node
        }
        for (dr, dc) in [(-1i64, -1i64), (-1, 0), (-1, 1), (0, -1)] {
            let (r, cc) = (c.row as i64 + dr, c.col as i64 + dc);
            if r >= 0 && cc >= 0 {
                if let Some(j) = grid.cell_index(r as u32, cc as u32) {
                    uf.union(i, j);
                }
            }
        }
    }
    let grow = |m: &Rect| Rect {
        top: m.top.saturating_sub(1),
        left: m.left.saturating_sub(1),
        bottom: m.bottom.saturating_add(1),
        right: m.right.saturating_add(1),
    };
    for (k, m) in clipped.iter().enumerate() {
        for i in grid.cells_in(grow(m)) {
            uf.union(i as usize, n + k);
        }
    }
    // Merge–merge: sweep by top, keeping the merges whose grown bottom still reaches.
    let mut order: Vec<usize> = (0..clipped.len()).collect();
    order.sort_by_key(|&k| clipped[k].top);
    let mut active: Vec<usize> = Vec::new();
    for &k in &order {
        let m = clipped[k];
        active.retain(|&p| clipped[p].bottom.saturating_add(1) >= m.top);
        for &p in &active {
            let o = clipped[p];
            if o.left <= m.right.saturating_add(1) && m.left <= o.right.saturating_add(1) {
                uf.union(n + p, n + k);
            }
        }
        active.push(k);
    }

    // Components that hold cells: bounding box, first position, cell count.
    let mut comp_of_root: HashMap<usize, usize> = HashMap::default();
    let mut rects: Vec<Rect> = Vec::new();
    let mut first: Vec<(u32, u32)> = Vec::new();
    let mut count: Vec<usize> = Vec::new();
    let mut cell_comp = Vec::with_capacity(n);
    for (i, c) in cells.iter().enumerate() {
        let root = uf.find(i);
        let id = *comp_of_root.entry(root).or_insert_with(|| {
            rects.push(Rect { top: c.row, left: c.col, bottom: c.row, right: c.col });
            first.push((c.row, c.col));
            count.push(0);
            rects.len() - 1
        });
        let r = &mut rects[id];
        r.top = r.top.min(c.row);
        r.left = r.left.min(c.col);
        r.bottom = r.bottom.max(c.row);
        r.right = r.right.max(c.col);
        first[id] = first[id].min((c.row, c.col));
        count[id] += 1;
        cell_comp.push(id);
    }
    for (k, m) in clipped.iter().enumerate() {
        if let Some(&id) = comp_of_root.get(&uf.find(n + k)) {
            let r = &mut rects[id];
            r.top = r.top.min(m.top);
            r.left = r.left.min(m.left);
            r.bottom = r.bottom.max(m.bottom);
            r.right = r.right.max(m.right);
            first[id] = first[id].min((m.top, m.left));
        }
    }

    let roles = cells
        .iter()
        .zip(&cell_comp)
        .map(|(c, &id)| {
            if count[id] < 2 {
                Role::Other
            } else if c.row == rects[id].top {
                Role::Header
            } else {
                Role::Data
            }
        })
        .collect();
    let mut ids: Vec<usize> = (0..rects.len()).filter(|&id| count[id] >= 2).collect();
    ids.sort_by_key(|&id| first[id]);
    let tables = ids.iter().map(|&id| DetectedTable { rect: rects[id], header_rows: 1, confidence: 0.5 }).collect();
    SheetLayout { tables, roles }
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind { parent: (0..n).collect() }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[a.max(b)] = a.min(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, Sheet, Value};

    /// The union-find path must reproduce the flood fill exactly, including overlapping and
    /// touching merges, merges without cells and duplicate cells.
    #[test]
    fn union_find_matches_flood_fill() {
        let mut seed: u64 = 1;
        let mut rnd = move |m: u32| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as u32) % m
        };
        for case in 0..400 {
            let size = 3 + rnd(12);
            let mut cells: Vec<Cell> = (0..rnd(25) + 1)
                .map(|_| Cell { row: rnd(size), col: rnd(size), value: Some(Value::Number(1.0)), formula: None })
                .collect();
            if case % 2 == 0 {
                cells.sort_by_key(|c| (c.row, c.col));
            }
            let merges = (0..rnd(5))
                .map(|_| {
                    let (t, l) = (rnd(size + 2), rnd(size + 2));
                    Rect { top: t, left: l, bottom: t + rnd(4), right: l + rnd(4) }
                })
                .collect();
            let sheet = Sheet { name: "s".into(), cells, merges };
            let grid = Grid::new(&sheet);
            let Some(first) = sheet.cells.first() else { continue };
            let mut bb = Rect { top: first.row, left: first.col, bottom: first.row, right: first.col };
            for c in &sheet.cells {
                bb.left = bb.left.min(c.col);
                bb.right = bb.right.max(c.col);
                bb.bottom = bb.bottom.max(c.row);
            }
            let clipped: Vec<Rect> = sheet
                .merges
                .iter()
                .map(|m| Rect {
                    top: m.top.max(bb.top.saturating_sub(1)),
                    left: m.left.max(bb.left.saturating_sub(1)),
                    bottom: m.bottom.min(bb.bottom + 1),
                    right: m.right.min(bb.right + 1),
                })
                .filter(|m| m.top <= m.bottom && m.left <= m.right)
                .collect();
            assert_eq!(flood(&grid, &clipped), by_union_find(&grid, &clipped), "case {case}: {sheet:?}");
        }
    }

    #[test]
    fn giant_merge_with_far_cells_is_cheap() {
        let cell = |row, col| Cell { row, col, value: Some(Value::Number(1.0)), formula: None };
        let sheet = Sheet {
            name: "s".into(),
            cells: vec![cell(0, 0), cell(0, 1), cell(1_048_575, 16_383)],
            merges: vec![Rect { top: 0, left: 0, bottom: 1_048_575, right: 16_383 }],
        };
        let layout = analyze(&Grid::new(&sheet));
        assert_eq!(layout.tables.len(), 1);
        assert_eq!(layout.tables[0].rect, Rect { top: 0, left: 0, bottom: 1_048_575, right: 16_383 });
        assert_eq!(layout.roles, vec![Role::Header, Role::Header, Role::Data]);
    }
}
