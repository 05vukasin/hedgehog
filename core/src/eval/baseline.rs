//! Baseline analyzer: every 8-connected component of filled positions (values, formulas, merged
//! areas) holding ≥ 2 non-empty cells is a table (its bounding box); the table's first row is
//! Header, the rest Data; cells of smaller components are Other. The number every layout change
//! must beat.

use std::collections::HashMap;

use crate::analysis::{DetectedTable, Role, SheetLayout};
use crate::grid::Grid;
use crate::model::Rect;

pub fn analyze(grid: &Grid) -> SheetLayout {
    let sheet = grid.sheet;
    // Position → component id (usize::MAX = not yet visited). Covers cells and merged areas.
    let mut comp: HashMap<(u32, u32), usize> = HashMap::new();
    for c in &sheet.cells {
        comp.insert((c.row, c.col), usize::MAX);
    }
    // Clip merges to the cells' bounding box (one pathological whole-column merge must not
    // allocate a million positions).
    let Some(first) = sheet.cells.first() else { return SheetLayout::default() };
    let mut bb = Rect { top: first.row, left: first.col, bottom: first.row, right: first.col };
    for c in &sheet.cells {
        bb.left = bb.left.min(c.col);
        bb.right = bb.right.max(c.col);
        bb.bottom = bb.bottom.max(c.row);
    }
    for m in &sheet.merges {
        let (top, bottom) = (m.top.max(bb.top.saturating_sub(1)), m.bottom.min(bb.bottom + 1));
        let (left, right) = (m.left.max(bb.left.saturating_sub(1)), m.right.min(bb.right + 1));
        for r in top..=bottom {
            for c in left..=right {
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
