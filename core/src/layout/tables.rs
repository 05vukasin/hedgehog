//! Row-level rules inside one region: titles above, notes below, header depth, stacked-table splits,
//! totals, group headers. Produces tables + cell roles; leftovers go to `Out::loose`.

use std::collections::HashSet;
use std::ops::Range;

use crate::analysis::{DetectedTable, Role};
use crate::model::{Rect, Sheet};

use super::blocks::{Entry, Region, Row, Ty};
use super::cells::{is_note_label, is_total_label, text_of, Agg, Kind};
use super::params::*;

pub(crate) struct Out {
    pub roles: Vec<Role>,
    pub tables: Vec<DetectedTable>,
    /// Filled things not in any table; classified once all tables are known.
    pub loose: Vec<Region>,
}

fn text<'a>(sheet: &'a Sheet, e: &Entry) -> &'a str {
    e.cell.and_then(|i| text_of(&sheet.cells[i])).unwrap_or("")
}

fn single_text<'a>(r: &'a Row) -> Option<&'a Entry> {
    let mut it = r.filled();
    let e = it.next()?;
    (it.next().is_none() && e.kind == Kind::Text).then_some(e)
}

/// Sparse row above a table: one text cell (anywhere), or one merged cell over most of the width
/// starting at the left edge. A partial merge (e.g. "Revenue" over C:E) is a parent header instead.
pub(crate) fn is_title_row(sheet: &Sheet, r: &Row, rect: Rect) -> bool {
    let width = rect.right - rect.left + 1;
    if width < 2 {
        return false;
    }
    let Some(e) = single_text(r) else { return false };
    if is_total_label(text(sheet, e)) {
        return false;
    }
    let span = e.right - e.left + 1;
    span == 1 || (e.left == rect.left && span as f32 >= TITLE_SPAN_FRAC * width as f32)
}

pub(crate) fn is_note_row(sheet: &Sheet, r: &Row) -> bool {
    let Some(e) = single_text(r) else { return false };
    let t = text(sheet, e);
    !is_total_label(t) && (is_note_label(t) || t.trim().chars().count() >= NOTE_MIN_LEN)
}

/// Header row relative to the rows below it: more of its cells break the column's body type
/// (text or a year over plain numbers) than repeat it (Pytheas-style signature votes).
pub(crate) fn is_header_row(r: &Row, below: &[Row]) -> bool {
    if !is_headerish(r) {
        return false;
    }
    let window = &below[..below.len().min(HEADER_VOTE_WINDOW)];
    let (mut head, mut data) = (0u32, 0u32);
    for e in r.filled() {
        let t = e.ty();
        if t == Ty::None {
            continue;
        }
        let (mut tot, mut same, mut numeric) = (0u32, 0u32, 0u32);
        for w in window {
            for x in w.entries.iter().filter(|x| x.left <= e.right && x.right >= e.left) {
                let xt = x.ty();
                if xt == Ty::None {
                    continue;
                }
                tot += 1;
                same += (xt == t) as u32;
                numeric += (xt != Ty::Text) as u32;
            }
        }
        if tot == 0 {
            continue;
        }
        let share = same as f32 / tot as f32;
        if t != Ty::Text && share >= HEADER_VOTE_SHARE {
            data += 1;
        } else if numeric * 2 >= tot && share < HEADER_VOTE_SHARE {
            head += 1;
        }
    }
    head > data
}

/// Mostly labels: text (or years/dates), few plain numbers.
pub(crate) fn is_headerish(r: &Row) -> bool {
    let n = r.n();
    let years = r.n_num() - r.n_measure();
    n > 0 && (r.n_text() >= 1 || years >= 2) && r.n_measure() as f32 <= HEADER_MAX_NUM_FRAC * n as f32
}

fn has_total_label(sheet: &Sheet, r: &Row) -> bool {
    r.entries.iter().filter(|e| e.kind == Kind::Text).take(3).any(|e| is_total_label(text(sheet, e)))
}

fn is_total_row(sheet: &Sheet, r: &Row) -> bool {
    if has_total_label(sheet, r) {
        return true;
    }
    let vagg = r.entries.iter().filter(|e| e.agg == Agg::Vertical).count();
    vagg > 0 && vagg as f32 >= TOTAL_FORMULA_FRAC * r.n_num() as f32
}

fn is_form(sheet: &Sheet, region: &Region) -> bool {
    if region.width() > 3 {
        return false;
    }
    let colon = region
        .rows
        .iter()
        .filter(|r| r.filled().next().is_some_and(|e| text(sheet, e).trim_end().ends_with(':')))
        .count();
    colon as f32 >= FORM_COLON_FRAC * region.rows.len() as f32
}

type Signature = Vec<(u32, String)>;

fn signature(sheet: &Sheet, r: &Row) -> Signature {
    r.filled().map(|e| (e.left, text(sheet, e).trim().to_lowercase())).collect()
}

/// Number of leading rows (from `start`) that form the header.
fn header_depth(sheet: &Sheet, rows: &[Row], start: usize, end: usize, rect: Rect) -> usize {
    let mut d = 0;
    while d < MAX_HEADER_ROWS && start + d < end {
        let r = &rows[start + d];
        if !is_header_row(r, &rows[start + d + 1..end]) || is_note_row(sheet, r) {
            break;
        }
        // A lone label below the first header row starts a group, not another header level.
        if d > 0 && single_text(r).is_some_and(|e| e.right == e.left) {
            break;
        }
        d += 1;
    }
    if !rows[start + d..end].iter().any(|r| r.n_measure() > 0) {
        // Text-only table (or no body): the first row is a header if it is wide and all text.
        let r = &rows[start];
        let fill: u32 = r.filled().map(|e| e.right.min(rect.right) - e.left + 1).sum();
        let width = rect.right - rect.left + 1;
        let ok = end - start >= 2
            && r.n() >= 2
            && r.n_text() == r.n()
            && fill as f32 >= TEXT_HEADER_MIN_FILL * width as f32;
        return ok as usize;
    }
    if d == 0 {
        return 0;
    }
    // Vertical merges in the header (e.g. "Region" over two header rows) push its bottom down.
    let mut last = rows[start + d - 1].row;
    for r in &rows[start..start + d] {
        for e in &r.entries {
            last = last.max(e.bottom);
        }
    }
    while start + d + 1 < end && rows[start + d].row <= last && rows[start + d].n_measure() == 0 {
        d += 1;
    }
    d
}

/// A header-like row inside a body with text where the body has numbers starts a new table.
fn starts_new_table(
    sheet: &Sheet,
    rows: &[Row],
    k: usize,
    end: usize,
    numeric_cols: &HashSet<u32>,
    header_sig: &Signature,
) -> bool {
    let r = &rows[k];
    if numeric_cols.is_empty() || r.n() < 2 || !is_header_row(r, &rows[k + 1..end]) {
        return false;
    }
    if !rows[k + 1..end].iter().take(MAX_HEADER_ROWS).any(|x| x.n_measure() > 0) {
        return false;
    }
    if !header_sig.is_empty() && signature(sheet, r) == *header_sig {
        return false; // repeated header (H9), stays in the table
    }
    let hits = r
        .filled()
        .skip(1)
        .filter(|e| (e.kind == Kind::Text || e.label_num) && numeric_cols.contains(&e.left))
        .count();
    hits >= SPLIT_MIN_HITS && hits as f32 >= SPLIT_MIN_HIT_FRAC * numeric_cols.len() as f32
}

struct Seg {
    titles: Range<usize>,
    start: usize,
    header: usize,
    end: usize,
}

fn segment(sheet: &Sheet, rows: &[Row], s: usize, e: usize, rect: Rect) -> Vec<Seg> {
    let mut segs = Vec::new();
    let (mut start, mut titles) = (s, s..s);
    while start < e {
        let h = header_depth(sheet, rows, start, e, rect);
        let body = start + h;
        let sig = if h > 0 { signature(sheet, &rows[body - 1]) } else { Vec::new() };
        let mut numeric_cols: HashSet<u32> = HashSet::new();
        let mut num_counts: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut split = None;
        for k in body..e {
            if k > body && k + 1 < e && starts_new_table(sheet, rows, k, e, &numeric_cols, &sig) {
                split = Some(k);
                break;
            }
            let seen = (k - body + 1) as u32;
            for en in &rows[k].entries {
                if en.kind == Kind::Num && !en.label_num {
                    *num_counts.entry(en.left).or_default() += 1;
                }
            }
            numeric_cols.clear();
            numeric_cols.extend(num_counts.iter().filter(|&(_, &n)| n * 2 >= seen).map(|(&c, _)| c));
        }
        let Some(sp) = split else {
            segs.push(Seg { titles, start, header: h, end: e });
            break;
        };
        // Title rows just above the next header belong to the next table.
        let mut seg_end = sp;
        while seg_end > body + 1
            && sp - seg_end < 2
            && is_title_row(sheet, &rows[seg_end - 1], rect)
            && !is_note_row(sheet, &rows[seg_end - 1])
        {
            seg_end -= 1;
        }
        segs.push(Seg { titles, start, header: h, end: seg_end });
        titles = seg_end..sp;
        start = sp;
    }
    segs
}

fn set(out: &mut Out, r: &Row, role: Role) {
    for e in &r.entries {
        if let Some(i) = e.cell {
            out.roles[i] = role;
        }
    }
}

fn bounds(rows: &[Row]) -> Rect {
    let mut rect = Rect { top: u32::MAX, left: u32::MAX, bottom: 0, right: 0 };
    for r in rows {
        rect.top = rect.top.min(r.row);
        rect.bottom = rect.bottom.max(r.bottom());
        rect.left = rect.left.min(r.left());
        rect.right = rect.right.max(r.right());
    }
    rect
}

/// Emits a table for `seg` if it qualifies; returns false otherwise (caller makes it loose).
fn emit_table(sheet: &Sheet, rows: &[Row], seg: &Seg, out: &mut Out) -> bool {
    let trows = &rows[seg.start..seg.end];
    let body_start = seg.start + seg.header;
    if trows.len() < MIN_TABLE_ROWS || body_start >= seg.end {
        return false;
    }
    let mut rect = bounds(trows);
    // Clamp a vertical merge hanging off the last row.
    rect.bottom = rect.bottom.max(trows.last().unwrap().row);
    let width = rect.right - rect.left + 1;
    if width < MIN_TABLE_COLS {
        return false;
    }
    let area = (rect.bottom - rect.top + 1) as f64 * width as f64;
    let filled: f64 = trows
        .iter()
        .flat_map(|r| r.filled())
        .map(|e| (e.right.min(rect.right) - e.left + 1) as f64 * (e.bottom.min(rect.bottom) - e.row + 1) as f64)
        .sum();
    if filled / area < MIN_TABLE_DENSITY as f64 {
        return false;
    }

    for r in &rows[seg.titles.clone()] {
        set(out, r, if is_note_row(sheet, r) { Role::Note } else { Role::Title });
    }
    let mut header_rows = 0;
    if seg.header > 0 {
        let first_body = rows[body_start].row;
        let mut last = rows[body_start - 1].row;
        for r in &rows[seg.start..body_start] {
            set(out, r, Role::Header);
            for e in &r.entries {
                if e.bottom < first_body {
                    last = last.max(e.bottom);
                }
            }
        }
        header_rows = last - rect.top + 1;
    }
    let sig = if seg.header > 0 { signature(sheet, &rows[body_start - 1]) } else { Vec::new() };
    let has_measures = rows[body_start..seg.end].iter().any(|r| r.n_measure() > 0);

    // Column type purity over data rows, for confidence.
    let mut col_counts: std::collections::HashMap<u32, (u32, u32)> = std::collections::HashMap::new();
    let mut body_cells = 0usize;
    for k in body_start..seg.end {
        let r = &rows[k];
        let last_body = k + 1 == seg.end;
        let role = if !sig.is_empty() && r.n() >= 2 && signature(sheet, r) == sig {
            Role::Header
        } else if is_total_row(sheet, r) {
            Role::Total
        } else if is_note_row(sheet, r) && (last_body || is_note_label(text(sheet, single_text(r).unwrap()))) {
            Role::Note
        } else if has_measures && !last_body && single_text(r).is_some() {
            Role::GroupHeader
        } else {
            Role::Data
        };
        set(out, r, role);
        if role == Role::Data {
            for e in &r.entries {
                body_cells += (e.kind != Kind::Blank) as usize;
                let c = col_counts.entry(e.left).or_default();
                match e.kind {
                    Kind::Text => c.0 += 1,
                    Kind::Num => c.1 += 1,
                    _ => {}
                }
                if MARK_ROW_SUM_CELLS_AS_TOTAL
                    && e.agg == Agg::Horizontal
                    && let Some(i) = e.cell
                {
                    out.roles[i] = Role::Total;
                }
            }
        }
    }
    let purities: Vec<f32> = col_counts
        .values()
        .filter(|(t, n)| t + n >= 2)
        .map(|&(t, n)| t.max(n) as f32 / (t + n) as f32)
        .collect();
    let purity = if purities.is_empty() { 0.5 } else { purities.iter().sum::<f32>() / purities.len() as f32 };
    let confidence = (CONF_BASE
        + if header_rows > 0 { CONF_HEADER } else { 0.0 }
        + CONF_TYPE * purity
        + CONF_SIZE * (body_cells as f32 / CONF_SIZE_CELLS).min(1.0))
    .clamp(0.0, 1.0);
    out.tables.push(DetectedTable { rect, header_rows, confidence });
    true
}

pub(crate) fn analyze_region(sheet: &Sheet, region: Region, out: &mut Out) {
    let rows = &region.rows;
    if rows.len() < MIN_TABLE_ROWS || region.width() < MIN_TABLE_COLS || is_form(sheet, &region) {
        out.loose.push(region);
        return;
    }
    let mut s = 0;
    while s + 1 < rows.len() && s < MAX_TITLE_ROWS && is_title_row(sheet, &rows[s], region.rect) {
        s += 1;
    }
    let mut e = rows.len();
    while e > s + 1 && is_note_row(sheet, &rows[e - 1]) {
        e -= 1;
    }
    let segs = segment(sheet, rows, s, e, region.rect);
    let mut loose_rows: Vec<Row> = Vec::new();
    let mut any = false;
    for seg in &segs {
        if emit_table(sheet, rows, seg, out) {
            any = true;
        } else {
            loose_rows.extend(rows[seg.titles.start..seg.end].iter().cloned());
        }
    }
    if any {
        for r in &rows[..s] {
            set(out, r, Role::Title);
        }
        for r in &rows[e..] {
            set(out, r, Role::Note);
        }
    } else {
        loose_rows.splice(0..0, rows[..s].iter().cloned());
        loose_rows.extend(rows[e..].iter().cloned());
    }
    if !loose_rows.is_empty() {
        loose_rows.sort_by_key(|r| r.row);
        out.loose.push(Region { rect: bounds(&loose_rows), rows: loose_rows });
    }
}
