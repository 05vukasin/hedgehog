//! Table boundaries and header depth from the row model's roles (model path only; the rule
//! layout's tables are the starting point).
//!
//! The rule layout finds the table rectangles; the row model often knows better which rows of a
//! rectangle are header, data or neither. Per table, the row role is the majority role of the
//! row's cells inside the rectangle, and, in this order:
//!
//! 1. **Header-only tables**: a rules table whose rows the model calls header from top to bottom,
//!    directly above another table over the same columns, is the upper part of that table's
//!    header (ABS time series: ten metadata rows the rules split into stacked tables).
//! 2. **Continuations**: a table directly below another over the same columns, whose header rows
//!    the model calls data, is the rest of that table (the rules cut it at a change of content and
//!    took a data row for its header).
//! 3. **Header depth**: rows just above the header that the model calls header extend it; trailing
//!    header rows the model calls data (with high agreement) move into the body; a
//!    column-numbering row right under the header stays in it.
//! 4. **Stacked tables**: a wide run of rows the model calls header, after a blank row and between
//!    data rows, starts a new table — unless it repeats the table's own header, or the table would
//!    fall into more than [`SPLIT_MAX_PIECES`] pieces (a list with recurring section headers).
//! 5. **Edges**: trailing rows the model calls note / title (or other, where the rules do not
//!    call them data) leave the table; so do title / note / other rows above the header.
//! 6. **Empty tables** (no row the model calls header, data or total: a key/value block of
//!    metadata) are dropped; **rows below** a table that the model calls data, inside its columns
//!    and in no table, join it.
//! 7. No two tables overlap ([`crate::layout::separate_tables`]).
//!
//! Every step works on the non-empty rows of the rectangles, so the cost is O(cells in tables)
//! per sheet, plus O(tables²) for the neighbour checks.

use crate::analysis::{DetectedTable, Role, SheetLayout};
use crate::grid::Grid;
use crate::model::{Rect, Value};

fn rid(r: Role) -> usize {
    crate::eval::export::role_id(r) as usize
}

/// One non-empty sheet row inside a table rectangle.
#[derive(Debug, Clone)]
struct RowInfo {
    row: u32,
    /// Cells per role ([`Role::ALL`] order): final roles and the model's raw (unguarded) ones.
    counts: [u32; 7],
    raw: [u32; 7],
    rules: [u32; 7],
}

impl RowInfo {
    fn n(&self) -> u32 {
        self.counts.iter().sum()
    }

    fn top(c: &[u32; 7]) -> (Role, f32) {
        let k = (0..7).max_by_key(|&k| (c[k], std::cmp::Reverse(k))).unwrap();
        (Role::ALL[k], c[k] as f32 / c.iter().sum::<u32>().max(1) as f32)
    }

    fn role(&self) -> Role {
        Self::top(&self.counts).0
    }

    fn share(&self, r: Role) -> f32 {
        self.counts[rid(r)] as f32 / self.n().max(1) as f32
    }

    fn raw_role(&self) -> Role {
        Self::top(&self.raw).0
    }

    fn rules_role(&self) -> Role {
        Self::top(&self.rules).0
    }
}

/// Rows of `rect` with at least one cell inside it, top to bottom.
fn rows_in(grid: &Grid, rect: Rect, roles: &[Role], raw: &[Role], rules: &[Role]) -> Vec<RowInfo> {
    let cells = &grid.sheet.cells;
    let lo = cells.partition_point(|c| c.row < rect.top);
    let hi = cells.partition_point(|c| c.row <= rect.bottom);
    let mut out: Vec<RowInfo> = Vec::new();
    for i in lo..hi {
        let c = &cells[i];
        if c.col < rect.left || c.col > rect.right || is_blank(&c.value) {
            continue;
        }
        if out.last().is_none_or(|r| r.row != c.row) {
            out.push(RowInfo { row: c.row, counts: [0; 7], raw: [0; 7], rules: [0; 7] });
        }
        let r = out.last_mut().unwrap();
        r.counts[rid(roles[i])] += 1;
        r.raw[rid(raw[i])] += 1;
        r.rules[rid(rules[i])] += 1;
    }
    out
}

fn is_blank(v: &Option<Value>) -> bool {
    match v {
        None => true,
        Some(Value::Text(s)) => s.trim().is_empty(),
        _ => false,
    }
}

/// Which steps of [`refine`] run ([`Steps::ALL`] in the model path; the others for tests and
/// ablations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Steps {
    /// Header-only tables become the top of the header of the table below.
    pub merge_header_only: bool,
    /// A piece the rules cut off, whose "header" the model calls data, continues the table above.
    pub continuation: bool,
    /// Model header rows directly above the header extend it.
    pub header_up: bool,
    /// Trailing header rows the model calls data leave the header.
    pub header_shrink: bool,
    /// Runs of model header rows inside the body start new tables.
    pub split: bool,
    /// Trailing note / other / title rows leave the table.
    pub trim_bottom: bool,
    /// Title / note / other rows above the header leave the table.
    pub trim_top: bool,
    /// Data rows directly below a table join it.
    pub extend: bool,
    /// Tables without a header, data or total row are dropped.
    pub drop_empty: bool,
}

impl Steps {
    pub const ALL: Steps = Steps {
        merge_header_only: true,
        continuation: true,
        header_up: true,
        header_shrink: true,
        split: true,
        trim_bottom: true,
        trim_top: true,
        extend: true,
        drop_empty: true,
    };

    pub const NONE: Steps = Steps {
        merge_header_only: false,
        continuation: false,
        header_up: false,
        header_shrink: false,
        split: false,
        trim_bottom: false,
        trim_top: false,
        extend: false,
        drop_empty: false,
    };
}

/// A header row leaves the header when at least this share of its cells is model data.
const SHRINK_AGREE: f32 = 0.8;
/// A split needs this many data rows above and below the new header.
const SPLIT_MIN_DATA: usize = 2;
/// The new header's widest row has at least this share of the cells of a typical data row.
const SPLIT_MIN_WIDTH: f32 = 0.5;
/// A table that would split into more pieces than this is one long list with section headers.
const SPLIT_MAX_PIECES: usize = 10;
/// Share of a row's cells that must be data (or total) for a continuation.
const CONTINUATION_DATA: f32 = 0.8;

/// Rewrites `lay.tables` from the model's cell roles (`lay.roles`, after the guard) and its raw
/// predictions (`raw`, before the guard). May also set roles of rows that become header rows.
pub fn refine(grid: &Grid, lay: &mut SheetLayout, raw: &[Role], rules: &[Role], steps: Steps) {
    if lay.tables.is_empty() || steps == Steps::NONE {
        return;
    }
    let mut tables = std::mem::take(&mut lay.tables);
    tables.sort_by_key(|t| (t.rect.top, t.rect.left));
    if steps.merge_header_only {
        tables = merge_header_only(grid, tables, &lay.roles, raw, rules);
    }
    if steps.continuation {
        tables = merge_continuations(grid, tables, &lay.roles, raw, rules);
    }
    let mut out = Vec::with_capacity(tables.len());
    for t in tables {
        let rows = rows_in(grid, t.rect, &lay.roles, raw, rules);
        if rows.is_empty() {
            out.push(t);
            continue;
        }
        let mut t = t;
        header_depth(grid, &mut t, &rows, steps, &mut lay.roles);
        let pieces = if steps.split { split(grid, t, &rows, &mut lay.roles) } else { vec![t] };
        for mut p in pieces {
            let rows = rows_in(grid, p.rect, &lay.roles, raw, rules);
            trim_edges(grid, &mut p, &rows, steps);
            out.push(p);
        }
    }
    out.sort_by_key(|t| (t.rect.top, t.rect.left));
    if steps.drop_empty {
        // A "table" without a row the model calls header, data or total (a key/value block of
        // metadata above the real table) is not one.
        out.retain(|t| {
            let rows = rows_in(grid, t.rect, &lay.roles, raw, rules);
            rows.is_empty() || rows.iter().any(|r| matches!(r.role(), Role::Header | Role::Data | Role::Total | Role::GroupHeader))
        });
    }
    if steps.extend {
        extend_down(grid, &mut out, &lay.roles);
    }
    crate::layout::separate_tables(&mut out);
    lay.tables = out;
}

/// Records without values (B44): a row the model calls a group header, that the rules call data,
/// with at least two cells, each in a column that holds text on most data rows of its table (the
/// WDI row `Kosovo | XKX | GDP per capita | NY.GDP.PCAP.CD` with no numbers). A group label stands
/// alone on its row; a row that fills the label columns like the records around it is a record.
pub fn empty_records_are_data(grid: &Grid, lay: &mut SheetLayout, rules: &[Role]) {
    let cells = &grid.sheet.cells;
    for t in &lay.tables {
        let body = t.header_top + t.header_rows;
        let rect = Rect { top: body.max(t.rect.top), ..t.rect };
        if rect.top > rect.bottom {
            continue;
        }
        let idx = grid.cells_in(rect);
        // Per column: data rows with text, and data rows overall.
        let mut text_rows: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut data_rows: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for &i in &idx {
            let c = &cells[i as usize];
            if lay.roles[i as usize] == Role::Data && !is_blank(&c.value) {
                data_rows.insert(c.row);
                if matches!(c.value, Some(Value::Text(_))) {
                    *text_rows.entry(c.col).or_default() += 1;
                }
            }
        }
        if data_rows.len() < 4 {
            continue;
        }
        let label_col = |col: u32| text_rows.get(&col).is_some_and(|&n| 2 * n as usize > data_rows.len());
        let mut k = 0;
        while k < idx.len() {
            let row = cells[idx[k] as usize].row;
            let end = k + idx[k..].iter().take_while(|&&i| cells[i as usize].row == row).count();
            let filled: Vec<usize> =
                idx[k..end].iter().map(|&i| i as usize).filter(|&i| !is_blank(&cells[i].value)).collect();
            let group = filled.iter().filter(|&&i| lay.roles[i] == Role::GroupHeader).count();
            let rules_data = filled.iter().filter(|&&i| rules[i] == Role::Data).count();
            if filled.len() >= 2
                && 2 * group > filled.len()
                && 2 * rules_data > filled.len()
                && filled.iter().all(|&i| label_col(cells[i].col))
            {
                for &i in &filled {
                    lay.roles[i] = Role::Data;
                }
            }
            k = end;
        }
    }
}

/// Step 1: a table that is header from top to bottom, directly above a table over the same
/// columns, becomes the top of that table's header.
fn merge_header_only(
    grid: &Grid,
    tables: Vec<DetectedTable>,
    roles: &[Role],
    raw: &[Role],
    rules: &[Role],
) -> Vec<DetectedTable> {
    let mut out: Vec<DetectedTable> = Vec::with_capacity(tables.len());
    // Pending header-only tables (top to bottom), each directly above the next.
    for t in tables {
        let mut t = t;
        // Merge every directly preceding header-only table over the same columns.
        while let Some(prev) = out.last() {
            let p = prev.rect;
            let same_cols = p.left.abs_diff(t.rect.left) <= 1 && p.right.abs_diff(t.rect.right) <= 1;
            let adjacent = p.bottom < t.rect.top && t.rect.top - p.bottom <= 2;
            if !(same_cols && adjacent) {
                break;
            }
            let rows = rows_in(grid, p, roles, raw, rules);
            let all_header = !rows.is_empty() && rows.iter().all(|r| r.role() == Role::Header);
            if !all_header {
                break;
            }
            let prev = out.pop().unwrap();
            let top = prev.header_top.min(rows[0].row);
            let body = t.header_top + t.header_rows;
            t = DetectedTable {
                rect: Rect {
                    top: p.top.min(t.rect.top),
                    left: p.left.min(t.rect.left),
                    bottom: t.rect.bottom,
                    right: p.right.max(t.rect.right),
                },
                header_top: top,
                header_rows: body.max(top) - top,
                confidence: t.confidence.max(prev.confidence),
            };
        }
        out.push(t);
    }
    out
}

/// Step 1b: a table cut off directly below another over the same columns, whose "header" the
/// model calls data, continues that table (the rules split it at a change of content and took its
/// first data row for a header).
fn merge_continuations(
    grid: &Grid,
    tables: Vec<DetectedTable>,
    roles: &[Role],
    raw: &[Role],
    rules: &[Role],
) -> Vec<DetectedTable> {
    let mut out: Vec<DetectedTable> = Vec::with_capacity(tables.len());
    for t in tables {
        if let Some(p) = out.last_mut() {
            let (a, b) = (p.rect, t.rect);
            let same_cols = a.left.abs_diff(b.left) <= 1 && a.right.abs_diff(b.right) <= 1;
            let adjacent = a.bottom < b.top && b.top - a.bottom <= 1;
            if same_cols && adjacent && t.header_top == b.top && t.header_rows > 0 && p.header_rows > 0 {
                let rows = rows_in(grid, Rect { bottom: b.top + t.header_rows - 1, ..b }, roles, raw, rules);
                let data =
                    !rows.is_empty() && rows.iter().all(|r| r.share(Role::Data) + r.share(Role::Total) >= CONTINUATION_DATA);
                if data {
                    p.rect.bottom = b.bottom;
                    p.rect.left = a.left.min(b.left);
                    p.rect.right = a.right.max(b.right);
                    continue;
                }
            }
        }
        out.push(t);
    }
    out
}

/// Step 5: rows directly below a table (no blank row between) that the model calls data,
/// that no table holds and that lie inside the table's columns, continue it.
fn extend_down(grid: &Grid, tables: &mut [DetectedTable], roles: &[Role]) {
    let cells = &grid.sheet.cells;
    let n = tables.len();
    for k in 0..n {
        let t = tables[k].rect;
        let next_top = tables
            .iter()
            .filter(|o| o.rect.top > t.bottom && o.rect.left <= t.right && o.rect.right >= t.left)
            .map(|o| o.rect.top)
            .min()
            .unwrap_or(u32::MAX);
        let mut bottom = t.bottom;
        // Header-like rows (a sparse label row between data rows) count once data follows them.
        let (mut pending, mut last) = (0, t.bottom);
        let mut i = cells.partition_point(|c| c.row <= t.bottom);
        while i < cells.len() {
            let row = cells[i].row;
            if row > last + 1 || row >= next_top {
                break;
            }
            let j = i + cells[i..].iter().take_while(|c| c.row == row).count();
            let (mut inside, mut outside, mut data, mut label) = (0u32, 0u32, 0u32, 0u32);
            for (q, c) in cells[i..j].iter().enumerate() {
                if is_blank(&c.value) {
                    continue;
                }
                if c.col < t.left || c.col > t.right {
                    outside += 1;
                } else {
                    inside += 1;
                    data += matches!(roles[i + q], Role::Data | Role::Total) as u32;
                    label += matches!(roles[i + q], Role::Header | Role::GroupHeader) as u32;
                }
            }
            if inside == 0 {
                i = j;
                continue;
            }
            if outside > 0 {
                break;
            }
            if data * 2 <= inside {
                if label * 2 > inside && pending < 2 {
                    pending += 1;
                    last = row;
                    i = j;
                    continue;
                }
                break;
            }
            bottom = row;
            last = row;
            pending = 0;
            i = j;
        }
        tables[k].rect.bottom = bottom;
    }
}

/// Step 2: header depth from the model's row roles.
fn header_depth(grid: &Grid, t: &mut DetectedTable, rows: &[RowInfo], steps: Steps, roles: &mut [Role]) {
    let numbering = |row: u32| numbering_row(grid, t.rect, row);
    let body = t.header_top + t.header_rows;
    // Index of the first row at or below header_top.
    let first = rows.partition_point(|r| r.row < t.header_top);
    if steps.header_up {
        // Rows just above the header (no blank row between) that the model calls header.
        let mut k = first;
        let mut top = t.header_top;
        while k > 0 && rows[k - 1].role() == Role::Header && rows[k - 1].row + 1 == top {
            k -= 1;
            top = rows[k].row;
        }
        if top < t.header_top {
            t.header_rows = body - top;
            t.header_top = top;
        }
    }
    let body = t.header_top + t.header_rows;
    if t.header_rows > 0 && steps.header_shrink {
        // Trailing header rows the model calls data with high agreement (the first header row
        // stays: a piece the rules cut off is handled by `merge_continuations`).
        let start = rows.partition_point(|r| r.row < t.header_top);
        let mut end = rows.partition_point(|r| r.row < body);
        while end > start + 1 && rows[end - 1].share(Role::Data) >= SHRINK_AGREE && !numbering(rows[end - 1].row) {
            end -= 1;
        }
        if end < rows.partition_point(|r| r.row < body) {
            let new_body = rows[end].row;
            t.header_rows = new_body - t.header_top;
        }
    }
    // A column-numbering row right under the header is part of it, whatever the model says.
    let body = t.header_top + t.header_rows;
    let k = rows.partition_point(|r| r.row < body);
    if t.header_rows > 0 && k + 1 < rows.len() && rows[k].row == body && numbering(body) {
        t.header_rows += 1;
    }
    let body = t.header_top + t.header_rows;
    for r in rows.iter().filter(|r| r.row >= t.header_top && r.row < body) {
        if numbering(r.row) {
            for &i in grid.row_cells(r.row) {
                let c = grid.sheet.cells[i as usize].col;
                if t.rect.left <= c && c <= t.rect.right {
                    roles[i as usize] = Role::Header;
                }
            }
        }
    }
}

/// The filled cells of `row` inside `rect` number the columns ([`crate::layout::column_numbering`]).
fn numbering_row(grid: &Grid, rect: Rect, row: u32) -> bool {
    let cells = &grid.sheet.cells;
    let vals = grid.row_cells(row).iter().map(|&i| &cells[i as usize]).filter(|c| {
        c.col >= rect.left && c.col <= rect.right && !is_blank(&c.value)
    });
    let vals: Vec<&Value> = vals.filter_map(|c| c.value.as_ref()).collect();
    vals.len() >= 3 && crate::layout::column_numbering(vals.into_iter())
}

/// Step 3: split at a run of model header rows inside the body.
fn split(grid: &Grid, t: DetectedTable, rows: &[RowInfo], roles: &mut [Role]) -> Vec<DetectedTable> {
    let body = t.header_top + t.header_rows;
    let start = rows.partition_point(|r| r.row < body);
    let header_text = header_signature(grid, &t, t.header_top, body);
    let mut out = Vec::new();
    let orig = t.clone();
    let mut cur = t;
    let mut data_before = 0usize;
    let mut k = start;
    while k < rows.len() {
        let r = &rows[k];
        let is_head = r.raw_role() == Role::Header && r.n() >= 2;
        if !is_head {
            data_before += matches!(r.role(), Role::Data | Role::Total) as usize;
            k += 1;
            continue;
        }
        // A run of header rows.
        let mut e = k;
        while e < rows.len() && rows[e].raw_role() == Role::Header {
            e += 1;
        }
        let data_after = rows[e..].iter().take_while(|r| r.raw_role() != Role::Header).filter(|r| r.role() == Role::Data).count();
        let run_top = rows[k].row;
        let run_end = rows[e - 1].row + 1;
        let repeated = header_signature(grid, &cur, run_top, run_end) == header_text && !header_text.is_empty();
        // A new header spans the table like its data rows do (a lone label row is a group).
        let width = rows[k..e].iter().map(RowInfo::n).max().unwrap_or(0);
        let around: Vec<u32> = rows[start..k]
            .iter()
            .rev()
            .chain(&rows[e..])
            .filter(|r| r.role() == Role::Data)
            .take(16)
            .map(RowInfo::n)
            .collect();
        let typical = median(around);
        let wide = width as f32 >= SPLIT_MIN_WIDTH * typical as f32;
        // Title rows directly above the run belong to the new table.
        let mut top = k;
        while top > start && matches!(rows[top - 1].raw_role(), Role::Title) && rows[top - 1].row + 1 == rows[top].row {
            top -= 1;
        }
        // The new table is set apart by a blank row above it (sub-headers inside one table are not).
        let apart = top > start && rows[top - 1].row + 1 < rows[top].row;
        if data_before >= SPLIT_MIN_DATA && data_after >= SPLIT_MIN_DATA && !repeated && wide && apart {
            let prev_bottom = rows[top - 1].row;
            let mut upper = cur.clone();
            upper.rect.bottom = prev_bottom;
            out.push(upper);
            cur.rect.top = rows[top].row;
            cur.header_top = run_top;
            cur.header_rows = run_end - run_top;
            for i in grid.cells_in(Rect { top: run_top, left: cur.rect.left, bottom: run_end - 1, right: cur.rect.right }) {
                roles[i as usize] = Role::Header;
            }
            data_before = 0;
        }
        k = e;
    }
    out.push(cur);
    if out.len() > SPLIT_MAX_PIECES {
        // So many header runs: a long list with recurring section headers, one table.
        return vec![orig];
    }
    out
}

fn median(mut v: Vec<u32>) -> u32 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

/// Lower-cased texts of a table's rows `top..end` inside its columns.
fn header_signature(grid: &Grid, t: &DetectedTable, top: u32, end: u32) -> Vec<(u32, String)> {
    if end <= top {
        return Vec::new();
    }
    let rect = Rect { top, left: t.rect.left, bottom: end - 1, right: t.rect.right };
    let cells = &grid.sheet.cells;
    grid.cells_in(rect)
        .iter()
        .filter_map(|&i| {
            let c = &cells[i as usize];
            match &c.value {
                Some(Value::Text(s)) if !s.trim().is_empty() => Some((c.col, s.trim().to_lowercase())),
                _ => None,
            }
        })
        .collect()
}

/// A last row the model calls note or title; or other, where the rules do not call it data.
fn leaves_bottom(r: &RowInfo) -> bool {
    match r.role() {
        Role::Note | Role::Title => true,
        Role::Other => !matches!(r.rules_role(), Role::Data | Role::Total),
        _ => false,
    }
}

/// Step 4: trailing note / other / title rows, and title / note / other rows above the header.
fn trim_edges(grid: &Grid, t: &mut DetectedTable, rows: &[RowInfo], steps: Steps) {
    if rows.is_empty() {
        return;
    }
    let body = t.header_top + t.header_rows;
    let mut lo = 0;
    let mut hi = rows.len();
    if steps.trim_bottom {
        // Keep at least one body row.
        let first_body = rows.partition_point(|r| r.row < body);
        while hi > first_body + 1 && leaves_bottom(&rows[hi - 1]) {
            hi -= 1;
        }
    }
    if steps.trim_top {
        while lo + 1 < hi && rows[lo].row < t.header_top && matches!(rows[lo].role(), Role::Title | Role::Note | Role::Other) {
            lo += 1;
        }
    }
    if lo == 0 && hi == rows.len() {
        return;
    }
    let top = rows[lo].row.min(t.header_top);
    let bottom = rows[hi - 1].row.max(top);
    let inner = Rect { top, bottom, ..t.rect };
    // Columns: only the trimmed rows may have widened the rectangle.
    let cells = &grid.sheet.cells;
    let (mut left, mut right) = (u32::MAX, 0);
    for i in grid.cells_in(inner) {
        let c = &cells[i as usize];
        if !is_blank(&c.value) {
            left = left.min(c.col);
            right = right.max(c.col);
        }
    }
    for m in &grid.sheet.merges {
        if m.top <= bottom && m.bottom >= top && m.left <= t.rect.right && m.right >= t.rect.left {
            left = left.min(m.left.max(t.rect.left));
            right = right.max(m.right.min(t.rect.right));
        }
    }
    if left > right {
        return;
    }
    t.rect = Rect { top, left, bottom, right };
    t.header_top = t.header_top.clamp(top, bottom);
    t.header_rows = t.header_rows.min(bottom + 1 - t.header_top);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, Sheet};

    /// A sheet from rows of cells (`""` empty; numbers parse), with one model role per row.
    struct Fix {
        sheet: Sheet,
        row_roles: Vec<Role>,
    }

    fn fix(rows: &[(&[&str], Role)]) -> Fix {
        let mut cells = Vec::new();
        for (r, (vals, _)) in rows.iter().enumerate() {
            for (c, v) in vals.iter().enumerate() {
                if v.is_empty() {
                    continue;
                }
                let value = v.parse::<f64>().map_or_else(|_| Value::Text(v.to_string()), Value::Number);
                cells.push(Cell { row: r as u32, col: c as u32, value: Some(value), formula: None });
            }
        }
        Fix { sheet: Sheet { name: "s".into(), cells, merges: vec![], formats: vec![] }, row_roles: rows.iter().map(|r| r.1).collect() }
    }

    fn table(top: u32, bottom: u32, right: u32, header_top: u32, header_rows: u32) -> DetectedTable {
        DetectedTable { rect: Rect { top, left: 0, bottom, right }, header_top, header_rows, confidence: 1.0 }
    }

    /// Runs `steps` on `tables`; `raw_rows` overrides the model's raw (unguarded) role of rows.
    fn run(f: &Fix, tables: Vec<DetectedTable>, steps: Steps, raw_rows: &[(u32, Role)]) -> SheetLayout {
        let grid = Grid::new(&f.sheet);
        let roles: Vec<Role> = f.sheet.cells.iter().map(|c| f.row_roles[c.row as usize]).collect();
        let raw: Vec<Role> = f
            .sheet
            .cells
            .iter()
            .zip(&roles)
            .map(|(c, &r)| raw_rows.iter().find(|x| x.0 == c.row).map_or(r, |x| x.1))
            .collect();
        let mut lay = SheetLayout { tables, roles: roles.clone() };
        refine(&grid, &mut lay, &raw, &roles, steps);
        lay
    }

    const H: Role = Role::Header;
    const D: Role = Role::Data;
    const T: Role = Role::Title;
    const N: Role = Role::Note;
    const O: Role = Role::Other;

    fn only(f: impl FnOnce(&mut Steps)) -> Steps {
        let mut s = Steps::NONE;
        f(&mut s);
        s
    }

    #[test]
    fn trims_notes_below_and_titles_above() {
        let f = fix(&[
            (&["Sales by region"], T),
            (&["Region", "Q1", "Q2"], H),
            (&["North", "1", "2"], D),
            (&["South", "3", "4"], D),
            (&["Source: survey, all regions, 2024 edition"], N),
        ]);
        let l = run(&f, vec![table(0, 4, 2, 1, 1)], only(|s| (s.trim_top, s.trim_bottom) = (true, true)), &[]);
        assert_eq!(l.tables[0].rect, Rect { top: 1, left: 0, bottom: 3, right: 2 });
        assert_eq!((l.tables[0].header_top, l.tables[0].header_rows), (1, 1));
        // The last body row stays even when the model calls it a note.
        let g = fix(&[(&["A", "B"], H), (&["x", "1"], N)]);
        let l = run(&g, vec![table(0, 1, 1, 0, 1)], Steps::ALL, &[]);
        assert_eq!(l.tables[0].rect.bottom, 1);
    }

    #[test]
    fn other_rows_the_rules_call_data_stay() {
        let f = fix(&[(&["No", "Name"], H), (&["1", "a"], D), (&["2", "b"], O), (&["3", "c"], O)]);
        let grid = Grid::new(&f.sheet);
        let roles: Vec<Role> = f.sheet.cells.iter().map(|c| f.row_roles[c.row as usize]).collect();
        let rules = vec![D; roles.len()];
        let mut lay = SheetLayout { tables: vec![table(0, 3, 1, 0, 1)], roles: roles.clone() };
        refine(&grid, &mut lay, &roles, &rules, only(|s| s.trim_bottom = true));
        assert_eq!(lay.tables[0].rect.bottom, 3);
    }

    #[test]
    fn header_grows_up_and_shrinks_from_below() {
        let f = fix(&[
            (&["", "2023", "2024"], H),
            (&["Region", "Q1", "Q1"], H),
            (&["Total", "5", "6"], D),
            (&["North", "1", "2"], D),
            (&["South", "3", "4"], D),
        ]);
        // The rules: header rows 1–2 (row 0 a title inside the rectangle).
        let l = run(&f, vec![table(0, 4, 2, 1, 2)], only(|s| (s.header_up, s.header_shrink) = (true, true)), &[]);
        assert_eq!((l.tables[0].header_top, l.tables[0].header_rows), (0, 2));
    }

    #[test]
    fn column_numbering_row_stays_in_the_header() {
        let f = fix(&[
            (&["Year", "GDP", "GNI", "Saldo"], H),
            (&["", "1", "2", "3"], D),
            (&["1991", "10.5", "11.5", "1.0"], D),
            (&["1992", "12.5", "13.5", "1.0"], D),
        ]);
        let l = run(&f, vec![table(0, 3, 3, 0, 1)], Steps::ALL, &[]);
        assert_eq!((l.tables[0].header_top, l.tables[0].header_rows), (0, 2));
        let grid = Grid::new(&f.sheet);
        assert_eq!(l.roles[grid.cell_index(1, 2).unwrap()], H);
        assert_eq!(l.roles[grid.cell_index(2, 2).unwrap()], D);
    }

    #[test]
    fn header_only_table_joins_the_table_below() {
        let f = fix(&[
            (&["", "Series A", "Series B"], H),
            (&["Unit", "000", "000"], H),
            (&["Series ID", "A1", "B1"], H),
            (&["2020", "1", "2"], D),
            (&["2021", "3", "4"], D),
        ]);
        let l = run(&f, vec![table(0, 1, 2, 0, 1), table(2, 4, 2, 2, 1)], only(|s| s.merge_header_only = true), &[]);
        assert_eq!(l.tables.len(), 1);
        assert_eq!(l.tables[0].rect, Rect { top: 0, left: 0, bottom: 4, right: 2 });
        assert_eq!((l.tables[0].header_top, l.tables[0].header_rows), (0, 3));
    }

    #[test]
    fn continuation_joins_the_table_above() {
        let f = fix(&[
            (&["Type", "Name", "ID"], H),
            (&["Overlay", "a", "1"], D),
            (&["Overlay", "b", "2"], D),
            (&["Inactivation", "c", "3"], D),
            (&["Inactivation", "d", "4"], D),
        ]);
        // The rules cut the table at row 3 and took that data row for a header.
        let l = run(&f, vec![table(0, 2, 2, 0, 1), table(3, 4, 2, 3, 1)], only(|s| s.continuation = true), &[]);
        assert_eq!(l.tables.len(), 1);
        assert_eq!(l.tables[0].rect.bottom, 4);
    }

    #[test]
    fn splits_at_a_new_header_after_a_blank_row() {
        let f = fix(&[
            (&["Term", "Price", "Vol"], H),
            (&["Nov", "1", "2"], D),
            (&["Dec", "3", "4"], D),
            (&[], D),
            (&["Region", "Load", "Cap"], D),
            (&["North", "5", "6"], D),
            (&["South", "7", "8"], D),
        ]);
        // The guard turned the new header into data; the raw model role is header.
        let l = run(&f, vec![table(0, 6, 2, 0, 1)], only(|s| s.split = true), &[(4, H)]);
        assert_eq!(l.tables.len(), 2);
        assert_eq!((l.tables[0].rect.bottom, l.tables[1].rect.top, l.tables[1].header_top), (2, 4, 4));
        let grid = Grid::new(&f.sheet);
        assert_eq!(l.roles[grid.cell_index(4, 1).unwrap()], H);
        // A sub-header without a blank row above it is no new table.
        let g = fix(&[
            (&["Term", "Price", "Vol"], H),
            (&["Nov", "1", "2"], D),
            (&["Dec", "3", "4"], D),
            (&["Region", "Load", "Cap"], D),
            (&["North", "5", "6"], D),
            (&["South", "7", "8"], D),
        ]);
        assert_eq!(run(&g, vec![table(0, 5, 2, 0, 1)], only(|s| s.split = true), &[(3, H)]).tables.len(), 1);
        // Nor is a repeated header.
        let h = fix(&[
            (&["Term", "Price", "Vol"], H),
            (&["Nov", "1", "2"], D),
            (&["Dec", "3", "4"], D),
            (&[], D),
            (&["Term", "Price", "Vol"], D),
            (&["Jan", "5", "6"], D),
            (&["Feb", "7", "8"], D),
        ]);
        assert_eq!(run(&h, vec![table(0, 6, 2, 0, 1)], only(|s| s.split = true), &[(4, H)]).tables.len(), 1);
    }

    #[test]
    fn data_rows_below_join_and_metadata_tables_go() {
        let f = fix(&[
            (&["Data Source", "WDI"], O),
            (&["Updated", "2024"], O),
            (&[], D),
            (&["Country", "Code", "1960"], H),
            (&["Aruba", "ABW", "1"], D),
            (&["Angola", "AGO", "2"], D),
            (&["Kosovo", "XKX"], Role::GroupHeader),
            (&["Zambia", "ZMB", "3"], D),
        ]);
        let l = run(
            &f,
            vec![table(0, 1, 1, 0, 1), table(3, 5, 2, 3, 1)],
            only(|s| (s.extend, s.drop_empty) = (true, true)),
            &[],
        );
        assert_eq!(l.tables.len(), 1);
        assert_eq!(l.tables[0].rect, Rect { top: 3, left: 0, bottom: 7, right: 2 });
    }
}
