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
    /// Date value (subset of `label_num`).
    pub date: bool,
    pub agg: Agg,
    /// Index into `sheet.cells`.
    pub cell: Option<usize>,
}

/// Fine type used for header-vs-body votes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ty {
    Text,
    Plain,
    Year,
    Date,
    None,
}

impl Entry {
    pub fn ty(&self) -> Ty {
        match self.kind {
            Kind::Text => Ty::Text,
            Kind::Num if self.date => Ty::Date,
            Kind::Num if self.label_num => Ty::Year,
            Kind::Num => Ty::Plain,
            _ => Ty::None,
        }
    }
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
            date: matches!(c.value, Some(crate::model::Value::DateTime(_))),
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
                date: false,
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

/// Facts about a block (or a group of merged blocks) used to decide gap merges.
#[derive(Clone)]
struct Profile {
    rect: Rect,
    starts_with_header: bool,
    /// A single row holding a single entry (a title or note line).
    single_line: bool,
    /// Only header-like rows (e.g. a header separated from its body by a blank row).
    header_only: bool,
    /// Lowercased texts of the first row (to tell repeated side-by-side copies apart).
    first_sig: Vec<String>,
    /// Per column: (text, numeric) entry counts over body rows.
    counts: BTreeMap<u32, (u32, u32)>,
}

impl Profile {
    fn has_label_col(&self) -> bool {
        self.counts.values().any(|&(t, n)| t > 0 && t as f32 >= LABEL_COL_TEXT_FRAC * (t + n) as f32)
    }
    fn has_numeric_col(&self) -> bool {
        self.counts.values().any(|&(t, n)| n > t)
    }
    fn absorb(&mut self, other: &Profile) {
        self.rect = Rect {
            top: self.rect.top.min(other.rect.top),
            left: self.rect.left.min(other.rect.left),
            bottom: self.rect.bottom.max(other.rect.bottom),
            right: self.rect.right.max(other.rect.right),
        };
        // Header-only pieces carry no body types.
        if self.header_only && !other.header_only {
            self.counts.clear();
        }
        let add = !other.header_only || self.header_only;
        self.single_line = false;
        self.header_only = false;
        for (c, (t, n)) in other.counts.iter().filter(|_| add) {
            let e = self.counts.entry(*c).or_default();
            e.0 += t;
            e.1 += n;
        }
    }
}

fn profile(sheet: &Sheet, region: &Region) -> Profile {
    let rows = &region.rows;
    let mut s = 0;
    while s + 1 < rows.len() && s < MAX_TITLE_ROWS && super::tables::is_title_row(sheet, &rows[s], region.rect, rows.get(s + 1)) {
        s += 1;
    }
    let starts_with_header = s < rows.len()
        && rows[s].n() >= 2
        && super::tables::is_header_row(&rows[s], &rows[s + 1..])
        && rows[s + 1..].iter().take(MAX_HEADER_ROWS).any(|r| r.n_measure() > 0);
    let header_only = rows.len() <= MAX_HEADER_ROWS
        && rows.iter().all(|r| r.n_measure() == 0 && r.n_text() >= 1)
        && rows.last().is_some_and(|r| r.n() >= 2);
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
    let single_line = rows.len() == 1 && rows[0].n() <= 1;
    let first_sig = rows
        .first()
        .map(|r| {
            r.filled()
                .filter_map(|e| e.cell.and_then(|i| super::cells::text_of(&sheet.cells[i])))
                .map(|t| t.trim().to_lowercase())
                .collect()
        })
        .unwrap_or_default();
    Profile { rect: region.rect, starts_with_header, single_line, header_only, first_sig, counts }
}

fn overlap(a0: u32, a1: u32, b0: u32, b1: u32) -> u32 {
    let lo = a0.max(b0);
    let hi = a1.min(b1);
    if hi >= lo { hi - lo + 1 } else { 0 }
}

/// Should block `low` join the group `up` that lies above (or around) it?
fn merge_vertically(up: &Profile, low: &Profile) -> bool {
    let inside = low.rect.left + 1 >= up.rect.left && low.rect.right <= up.rect.right + 1;
    // A block within the bounding box of a bigger group (sparse columns of one table).
    if inside && low.rect.top <= up.rect.bottom {
        return true;
    }
    let gap = low.rect.top as i64 - up.rect.bottom as i64 - 1;
    let max_gap = if up.header_only { HEADER_ONLY_MAX_GAP } else { MAX_ROW_GAP };
    if !(VERTICAL_MIN_GAP..=max_gap as i64).contains(&gap) || low.starts_with_header || low.single_line {
        return false;
    }
    if inside && !up.single_line {
        return true;
    }
    if up.header_only && overlap(up.rect.left, up.rect.right, low.rect.left, low.rect.right) > 0 {
        let wl = low.rect.right - low.rect.left + 1;
        let wu = up.rect.right - up.rect.left + 1;
        let ov = overlap(up.rect.left, up.rect.right, low.rect.left, low.rect.right);
        return ov as f32 >= GAP_MIN_COL_OVERLAP * wl.min(wu) as f32;
    }
    let (wu, wl) = (up.rect.right - up.rect.left + 1, low.rect.right - low.rect.left + 1);
    let ov = overlap(up.rect.left, up.rect.right, low.rect.left, low.rect.right);
    if (ov as f32) < GAP_MIN_COL_OVERLAP * wu.max(wl) as f32 {
        return false;
    }
    let (mut shared, mut agree) = (0u32, 0u32);
    for (c, &(t, n)) in &low.counts {
        if let Some(&(t2, n2)) = up.counts.get(c)
            && t + n > 0
            && t2 + n2 > 0
        {
            shared += 1;
            agree += ((n > t) == (n2 > t2)) as u32;
        }
    }
    shared > 0 && agree as f32 >= GAP_MIN_TYPE_AGREE * shared as f32
}

fn merge_horizontally(left: &Profile, right: &Profile) -> bool {
    let gap = right.rect.left as i64 - left.rect.right as i64 - 1;
    if gap < 1 {
        return false;
    }
    // Header cells over empty columns, on the table's header row.
    if gap <= MAX_COL_GAP_ALIGNED as i64
        && right.header_only
        && right.rect.top >= left.rect.top
        && right.rect.bottom < left.rect.bottom
        && right.rect.top <= left.rect.top + 2
    {
        return true;
    }
    // Exactly row-aligned blocks with different headers: one wide table split by spacer columns.
    // (Identical headers side by side are repeated copies — separate tables.)
    if gap <= MAX_COL_GAP_ALIGNED as i64
        && left.rect.top == right.rect.top
        && left.rect.bottom.abs_diff(right.rect.bottom) <= 1
        && left.rect.bottom > left.rect.top + 1
        && !right.first_sig.is_empty()
        && left.first_sig != right.first_sig
    {
        return true;
    }
    if gap > MAX_COL_GAP as i64 || right.has_label_col() || !right.has_numeric_col() {
        return false;
    }
    let (hl, hr) = (left.rect.bottom - left.rect.top + 1, right.rect.bottom - right.rect.top + 1);
    let ov = overlap(left.rect.top, left.rect.bottom, right.rect.top, right.rect.bottom);
    ov as f32 >= GAP_MIN_ROW_OVERLAP * hl.max(hr) as f32
}

/// Connected components, then gap-tolerant merging into regions.
pub(crate) fn regions(grid: &Grid) -> Vec<Region> {
    let mut blocks: Vec<Region> = components(grid).into_iter().map(make_region).collect();
    if blocks.len() <= 1 {
        return blocks;
    }
    blocks.sort_by_key(|b| (b.rect.top, b.rect.left));
    let profiles: Vec<Profile> = blocks.iter().map(|b| profile(grid.sheet, b)).collect();
    let mut group: Vec<Profile> = profiles.clone();
    let mut dsu = Dsu::new(blocks.len());
    let join = |dsu: &mut Dsu, group: &mut Vec<Profile>, a: u32, b: u32| {
        let (ra, rb) = (dsu.find(a), dsu.find(b));
        if ra != rb {
            dsu.union(ra, rb);
            let (keep, gone) = if dsu.find(ra) == ra { (ra, rb) } else { (rb, ra) };
            let g = group[gone as usize].clone();
            group[keep as usize].absorb(&g);
        }
    };

    // Spacer columns: pairwise on raw blocks.
    let mut by_left: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, p) in profiles.iter().enumerate() {
        by_left.entry(p.rect.left).or_default().push(i);
    }
    for (i, p) in profiles.iter().enumerate() {
        for gap in 1..=MAX_COL_GAP.max(MAX_COL_GAP_ALIGNED) {
            let Some(l) = p.rect.right.checked_add(gap + 1) else { continue };
            for &j in by_left.get(&l).into_iter().flatten() {
                if merge_horizontally(p, &profiles[j]) {
                    join(&mut dsu, &mut group, i as u32, j as u32);
                }
            }
        }
    }

    // Row labels left of numbers, across blank spacer columns ("Gas Annuity =" in E, numbers in
    // G:L; month/task text in A): a text-only narrow block sharing a row with a numeric block.
    if LABEL_MAX_GAP > 0 {
        for (i, p) in profiles.iter().enumerate() {
            if !p.is_label_block() {
                continue;
            }
            let rows_i: Vec<u32> = blocks[i].rows.iter().map(|r| r.row).collect();
            for gap in 1..=LABEL_MAX_GAP {
                let Some(l) = p.rect.right.checked_add(gap + 1) else { continue };
                for &j in by_left.get(&l).into_iter().flatten() {
                    let q = &profiles[j];
                    if !q.has_numeric_col() || q.rect.top > p.rect.bottom + 1 || q.rect.bottom < p.rect.top {
                        continue;
                    }
                    // Same row (row label) or the row just below (group label above its rows).
                    let hit = |r: u32| rows_i.binary_search(&r).is_ok();
                    if blocks[j].rows.iter().any(|r| hit(r.row) || r.row.checked_sub(1).is_some_and(hit)) {
                        join(&mut dsu, &mut group, i as u32, j as u32);
                    }
                }
            }
        }
    }

    // Spacer rows and sparse columns: sweep blocks top-down against groups still "open" above.
    let mut active: Vec<u32> = Vec::new();
    for (i, p) in profiles.iter().enumerate() {
        active.retain(|&g| dsu.find(g) == g && group[g as usize].rect.bottom as u64 + 1 + MAX_ROW_GAP.max(HEADER_ONLY_MAX_GAP) as u64 >= p.rect.top as u64);
        let mut k = 0;
        while k < active.len() {
            let g = active[k];
            let me = dsu.find(i as u32);
            if dsu.find(g) == g && g != me && merge_vertically(&group[g as usize], &group[me as usize].clone().with_header(p)) {
                join(&mut dsu, &mut group, g, i as u32);
            }
            k += 1;
        }
        let me = dsu.find(i as u32);
        active.retain(|&g| dsu.find(g) == g);
        if !active.contains(&me) {
            active.push(me);
        }
    }

    // Blocks lying inside another group's bounding box (a lone group label in a label column)
    // belong to it.
    if ABSORB_CONTAINED {
        let mut roots: Vec<u32> = (0..blocks.len() as u32).filter(|&g| dsu.find(g) == g).collect();
        if roots.len() <= ABSORB_MAX_GROUPS {
            roots.sort_by_key(|&g| std::cmp::Reverse(area(&group[g as usize].rect)));
            for k in 0..roots.len() {
                let outer = roots[k];
                if dsu.find(outer) != outer {
                    continue;
                }
                for &inner in &roots[k + 1..] {
                    if dsu.find(inner) != inner || inner == outer {
                        continue;
                    }
                    let (o, n) = (group[outer as usize].rect, group[inner as usize].rect);
                    if n.top >= o.top && n.bottom <= o.bottom && n.left >= o.left && n.right <= o.right {
                        join(&mut dsu, &mut group, outer, inner);
                    }
                }
            }
        }
    }

    let mut groups: BTreeMap<u32, Vec<Entry>> = BTreeMap::new();
    let mut size = vec![0u32; blocks.len()];
    for i in 0..blocks.len() {
        size[dsu.find(i as u32) as usize] += 1;
    }
    let mut out = Vec::new();
    for (i, b) in blocks.into_iter().enumerate() {
        let root = dsu.find(i as u32);
        if size[root as usize] == 1 {
            out.push(b);
        } else {
            groups.entry(root).or_default().extend(b.rows.into_iter().flat_map(|r| r.entries));
        }
    }
    out.extend(groups.into_values().map(make_region));
    out.sort_by_key(|r| (r.rect.top, r.rect.left));
    out
}

fn area(r: &Rect) -> u64 {
    (r.bottom - r.top + 1) as u64 * (r.right - r.left + 1) as u64
}

impl Profile {
    /// Narrow block of text only (row or group labels).
    fn is_label_block(&self) -> bool {
        self.rect.right - self.rect.left < LABEL_MAX_WIDTH
            && self.counts.values().all(|&(_, n)| n == 0)
            && self.counts.values().any(|&(t, _)| t > 0)
    }

    /// The group's shape, but header/single-line facts of the block being placed.
    fn with_header(mut self, block: &Profile) -> Profile {
        self.starts_with_header = block.starts_with_header;
        self.single_line = block.single_line;
        self
    }
}
