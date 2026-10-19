//! Occupancy blocks: 8-connected components over filled cells and merged areas, then gap tolerance
//! (blank spacer rows/columns inside one table). Sparse throughout — never allocates per sheet area.

use std::collections::{BTreeMap, HashMap};

use crate::grid::Grid;
use crate::model::{Rect, Sheet};

use super::cells::{formula_agg, is_label_number, kind_of, Agg, Kind};
use super::params::*;

/// One filled thing on the sheet: a cell, or an empty merged area. Merged cells span `left..=right`
/// and `row..=bottom`, anchored at their top-left.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub row: u32,
    pub left: u32,
    pub right: u32,
    pub bottom: u32,
    pub kind: Kind,
    /// Year or date: a number that often labels a column.
    pub label_num: bool,
    pub agg: Agg,
    /// Index into `sheet.cells`.
    pub cell: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub row: u32,
    /// Sorted by `left`.
    pub entries: Vec<Entry>,
}

impl Row {
    /// Entries that hold something (not empty merges).
    pub fn filled(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.kind != Kind::Blank)
    }
    pub fn n(&self) -> usize {
        self.filled().count()
    }
    pub fn n_text(&self) -> usize {
        self.entries.iter().filter(|e| e.kind == Kind::Text).count()
    }
    /// Numbers that are not years/dates.
    pub fn n_measure(&self) -> usize {
        self.entries.iter().filter(|e| e.kind == Kind::Num && !e.label_num).count()
    }
    pub fn n_num(&self) -> usize {
        self.entries.iter().filter(|e| e.kind == Kind::Num).count()
    }
    pub fn left(&self) -> u32 {
        self.entries.first().map_or(0, |e| e.left)
    }
    pub fn right(&self) -> u32 {
        self.entries.iter().map(|e| e.right).max().unwrap_or(0)
    }
    pub fn bottom(&self) -> u32 {
        self.entries.iter().map(|e| e.bottom).max().unwrap_or(self.row)
    }
}

/// A connected (possibly gap-merged) group of rows.
#[derive(Debug, Clone)]
pub(crate) struct Region {
    pub rect: Rect,
    pub rows: Vec<Row>,
}

impl Region {
    pub fn width(&self) -> u32 {
        self.rect.right - self.rect.left + 1
    }
}

struct Dsu(Vec<u32>);

impl Dsu {
    fn new(n: usize) -> Self {
        Dsu((0..n as u32).collect())
    }
    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            let p = self.0[self.0[x as usize] as usize];
            self.0[x as usize] = p;
            x = p;
        }
        x
    }
    fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b) as usize] = a.min(b);
        }
    }
}

fn entries_of(grid: &Grid) -> Vec<Entry> {
    let sheet = grid.sheet;
    let mut out = Vec::with_capacity(sheet.cells.len() + sheet.merges.len());
    for (i, c) in sheet.cells.iter().enumerate() {
        let (mut right, mut bottom) = (c.col, c.row);
        if let Some(m) = grid.merge_at(c.row, c.col)
            && m.top == c.row
            && m.left == c.col
        {
            right = m.right;
            bottom = m.bottom;
        }
        let agg = c.formula.as_deref().map_or(Agg::None, |f| formula_agg(f, c.row, c.col));
        out.push(Entry {
            row: c.row,
            left: c.col,
            right,
            bottom,
            kind: kind_of(c),
            label_num: is_label_number(c),
            agg,
            cell: Some(i),
        });
    }
    for m in &sheet.merges {
        if grid.cell_index(m.top, m.left).is_none() {
            out.push(Entry {
                row: m.top,
                left: m.left,
                right: m.right,
                bottom: m.bottom,
                kind: Kind::Blank,
                label_num: false,
                agg: Agg::None,
                cell: None,
            });
        }
    }
    out
}

/// 8-connected components; merged areas count as filled.
fn components(grid: &Grid) -> Vec<Vec<Entry>> {
    let sheet = grid.sheet;
    let n = sheet.cells.len();
    let m = sheet.merges.len();
    let mut dsu = Dsu::new(n + m);
    // Merge index by anchor, to map Grid::merge_at rects back to a node.
    let merge_node: HashMap<(u32, u32), u32> =
        sheet.merges.iter().enumerate().map(|(k, r)| ((r.top, r.left), (n + k) as u32)).collect();
    let node_at = |row: u32, col: u32| -> Option<u32> {
        if let Some(i) = grid.cell_index(row, col) {
            return Some(i as u32);
        }
        grid.merge_at(row, col).and_then(|r| merge_node.get(&(r.top, r.left)).copied())
    };
    for (i, c) in sheet.cells.iter().enumerate() {
        if let Some(r) = grid.merge_at(c.row, c.col)
            && let Some(&k) = merge_node.get(&(r.top, r.left))
        {
            dsu.union(i as u32, k);
        }
        for (dr, dc) in [(0i64, 1i64), (1, -1), (1, 0), (1, 1)] {
            let (nr, nc) = (c.row as i64 + dr, c.col as i64 + dc);
            if nc < 0 || nr > u32::MAX as i64 || nc > u32::MAX as i64 {
                continue;
            }
            if let Some(j) = node_at(nr as u32, nc as u32) {
                dsu.union(i as u32, j);
            }
        }
    }
    // Merged areas: look at the ring around each rect (O(perimeter), never O(area)).
    for (k, r) in sheet.merges.iter().enumerate() {
        let me = (n + k) as u32;
        let top = r.top.checked_sub(1);
        let left = r.left.checked_sub(1);
        let (bottom, right) = (r.bottom.saturating_add(1), r.right.saturating_add(1));
        let c0 = left.unwrap_or(r.left);
        for col in c0..=right {
            if let Some(t) = top
                && let Some(j) = node_at(t, col)
            {
                dsu.union(me, j);
            }
            if let Some(j) = node_at(bottom, col) {
                dsu.union(me, j);
            }
        }
        for row in r.top..=r.bottom {
            if let Some(l) = left
                && let Some(j) = node_at(row, l)
            {
                dsu.union(me, j);
            }
            if let Some(j) = node_at(row, right) {
                dsu.union(me, j);
            }
        }
    }
    let entries = entries_of(grid);
    let mut groups: HashMap<u32, Vec<Entry>> = HashMap::new();
    let mut order = Vec::new();
    for e in entries {
        let node = match e.cell {
            Some(i) => i as u32,
            None => merge_node[&(e.row, e.left)],
        };
        let root = dsu.find(node);
        let g = groups.entry(root).or_insert_with(|| {
            order.push(root);
            Vec::new()
        });
        g.push(e);
    }
    order.into_iter().map(|r| groups.remove(&r).unwrap()).collect()
}

fn make_region(mut entries: Vec<Entry>) -> Region {
    entries.sort_by_key(|e| (e.row, e.left));
    let mut rect = Rect { top: u32::MAX, left: u32::MAX, bottom: 0, right: 0 };
    for e in &entries {
        rect.top = rect.top.min(e.row);
        rect.left = rect.left.min(e.left);
        rect.bottom = rect.bottom.max(e.bottom);
        rect.right = rect.right.max(e.right);
    }
    let mut rows: Vec<Row> = Vec::new();
    for e in entries {
        match rows.last_mut() {
            Some(r) if r.row == e.row => r.entries.push(e),
            _ => rows.push(Row { row: e.row, entries: vec![e] }),
        }
    }
    Region { rect, rows }
}

/// Facts about a block used to decide gap merges.
struct Profile {
    rect: Rect,
    starts_with_header: bool,
    has_label_col: bool,
    /// Majority type per column over body rows: true = numeric.
    col_numeric: BTreeMap<u32, bool>,
}

fn profile(sheet: &Sheet, region: &Region) -> Profile {
    let rows = &region.rows;
    let mut s = 0;
    while s + 1 < rows.len() && s < MAX_TITLE_ROWS && super::tables::is_title_row(sheet, &rows[s], region.rect) {
        s += 1;
    }
    let starts_with_header = s < rows.len()
        && super::tables::is_headerish(&rows[s])
        && rows[s].n() >= 2
        && rows[s + 1..].iter().take(MAX_HEADER_ROWS).any(|r| r.n_measure() > 0);
    let body = if starts_with_header { s + 1 } else { s };
    let mut counts: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    for r in &rows[body.min(rows.len())..] {
        for e in &r.entries {
            let c = counts.entry(e.left).or_default();
            match e.kind {
                Kind::Text => c.0 += 1,
                Kind::Num => c.1 += 1,
                _ => {}
            }
        }
    }
    let has_label_col =
        counts.values().any(|&(t, n)| t > 0 && t as f32 >= LABEL_COL_TEXT_FRAC * (t + n) as f32);
    let col_numeric = counts.into_iter().filter(|(_, (t, n))| t + n > 0).map(|(c, (t, n))| (c, n > t)).collect();
    Profile { rect: region.rect, starts_with_header, has_label_col, col_numeric }
}

fn overlap(a0: u32, a1: u32, b0: u32, b1: u32) -> u32 {
    let lo = a0.max(b0);
    let hi = a1.min(b1);
    if hi >= lo { hi - lo + 1 } else { 0 }
}

fn merge_vertically(up: &Profile, low: &Profile) -> bool {
    let gap = low.rect.top as i64 - up.rect.bottom as i64 - 1;
    if !(1..=MAX_ROW_GAP as i64).contains(&gap) || low.starts_with_header {
        return false;
    }
    let (wu, wl) = (up.rect.right - up.rect.left + 1, low.rect.right - low.rect.left + 1);
    let ov = overlap(up.rect.left, up.rect.right, low.rect.left, low.rect.right);
    if (ov as f32) < GAP_MIN_COL_OVERLAP * wu.max(wl) as f32 {
        return false;
    }
    let (mut shared, mut agree) = (0u32, 0u32);
    for (c, num) in &low.col_numeric {
        if let Some(n2) = up.col_numeric.get(c) {
            shared += 1;
            agree += (num == n2) as u32;
        }
    }
    shared > 0 && agree as f32 >= GAP_MIN_TYPE_AGREE * shared as f32
}

fn merge_horizontally(left: &Profile, right: &Profile) -> bool {
    let gap = right.rect.left as i64 - left.rect.right as i64 - 1;
    if !(1..=MAX_COL_GAP as i64).contains(&gap) || right.has_label_col {
        return false;
    }
    if !right.col_numeric.values().any(|&n| n) {
        return false;
    }
    let (hl, hr) = (left.rect.bottom - left.rect.top + 1, right.rect.bottom - right.rect.top + 1);
    let ov = overlap(left.rect.top, left.rect.bottom, right.rect.top, right.rect.bottom);
    ov as f32 >= GAP_MIN_ROW_OVERLAP * hl.max(hr) as f32
}

/// Connected components, then gap-tolerant merging into regions.
pub(crate) fn regions(grid: &Grid) -> Vec<Region> {
    let blocks: Vec<Region> = components(grid).into_iter().map(make_region).collect();
    if blocks.len() <= 1 {
        return blocks;
    }
    let profiles: Vec<Profile> = blocks.iter().map(|b| profile(grid.sheet, b)).collect();
    let mut by_top: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut by_left: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, p) in profiles.iter().enumerate() {
        by_top.entry(p.rect.top).or_default().push(i);
        by_left.entry(p.rect.left).or_default().push(i);
    }
    let mut dsu = Dsu::new(blocks.len());
    for (i, p) in profiles.iter().enumerate() {
        for gap in 1..=MAX_ROW_GAP {
            let Some(t) = p.rect.bottom.checked_add(gap + 1) else { continue };
            for &j in by_top.get(&t).into_iter().flatten() {
                if merge_vertically(p, &profiles[j]) {
                    dsu.union(i as u32, j as u32);
                }
            }
        }
        for gap in 1..=MAX_COL_GAP {
            let Some(l) = p.rect.right.checked_add(gap + 1) else { continue };
            for &j in by_left.get(&l).into_iter().flatten() {
                if merge_horizontally(p, &profiles[j]) {
                    dsu.union(i as u32, j as u32);
                }
            }
        }
    }
    let mut groups: BTreeMap<u32, Vec<Entry>> = BTreeMap::new();
    let mut singles = Vec::new();
    let mut multi = vec![0u32; blocks.len()];
    for i in 0..blocks.len() {
        multi[dsu.find(i as u32) as usize] += 1;
    }
    for (i, b) in blocks.into_iter().enumerate() {
        let root = dsu.find(i as u32);
        if multi[root as usize] == 1 {
            singles.push(b);
        } else {
            groups.entry(root).or_default().extend(b.rows.into_iter().flat_map(|r| r.entries));
        }
    }
    singles.extend(groups.into_values().map(make_region));
    singles.sort_by_key(|r| (r.rect.top, r.rect.left));
    singles
}
