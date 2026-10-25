//! Side-by-side copies of one table with no blank column between them: a long list printed in two
//! or more halves (`State | Amount | Percent | State | Amount | Percent`, the second half Missouri
//! to Wyoming). Read as one table, the right half becomes `State (2)`, `Amount (2)` columns and its
//! values sit in the rows of the left half's labels (Ohio's numbers in the row labelled Florida,
//! HiTab 163_81_tab2). Each copy is a table of its own.

use std::collections::{HashMap, HashSet};

use super::blocks::{Entry, Region, Row};
use super::cells::{Kind, text_of};
use crate::model::{Rect, Sheet};

/// Header rows looked at, from the top of the region.
const MAX_HEADER_ROW: usize = 6;
/// A column of a copy is its label column when this share of the body rows hold text there.
const LABEL_SHARE: f32 = 0.5;
/// Copies whose label columns hold the same text on this share of the rows are a comparison of the
/// same items (`original | new`), one table.
const SAME_ITEMS_SHARE: f32 = 0.5;
/// Share of distinct texts in a label column (`F` on every row is a suppression mark, no label).
const LABEL_DISTINCT: f32 = 0.6;
/// Periods tried per header row.
const MAX_PERIODS: usize = 3;

fn norm(sheet: &Sheet, e: &Entry) -> String {
    let t = e.cell.and_then(|i| text_of(&sheet.cells[i])).unwrap_or("");
    t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The copies' column ranges, when a header row repeats its cells `k >= 2` times at a fixed period
/// and each copy has a text label column (the same one in every copy) and numbers below it.
fn copies(sheet: &Sheet, region: &Region) -> Option<Vec<(u32, u32)>> {
    let rows = &region.rows;
    if rows.len() < 4 {
        return None;
    }
    let left = region.rect.left;
    for h in 0..rows.len().min(MAX_HEADER_ROW) {
        let hdr: Vec<&Entry> = rows[h].filled().collect();
        if hdr.len() < 4 || hdr.iter().any(|e| e.kind != Kind::Text) {
            continue;
        }
        let texts: Vec<(u32, String)> = hdr.iter().map(|e| (e.left, norm(sheet, e))).collect();
        let c0 = texts[0].0;
        let by_col: HashMap<u32, &str> = texts.iter().map(|(l, t)| (*l, t.as_str())).collect();
        let at = |c: u32| by_col.get(&c).copied();
        // A period is where the first header cell comes again (the first few times: a header of
        // thousands of equal names stays linear).
        let periods: Vec<u32> =
            texts[1..].iter().filter(|(_, t)| *t == texts[0].1).map(|(l, _)| l - c0).filter(|&p| p >= 2).take(MAX_PERIODS).collect();
        for p in periods {
            let first: Vec<&(u32, String)> = texts.iter().take_while(|(l, _)| *l < c0 + p).collect();
            // A copy's header names more than one thing (a header of thousands of equal names is
            // no repeated copy and stays linear).
            if first.iter().map(|(_, t)| t).collect::<HashSet<_>>().len() < 2 {
                continue;
            }
            // The header row starts at the region's left edge: each copy's label column has its
            // header cell (`State`), not a header of value columns only (`Not reported | Reported`
            // repeated per territory over a label column, HiTab 2768).
            if first.len() < 2 || c0 != left {
                continue;
            }
            // Copies while every header cell repeats at the same offset and nothing else is there.
            let mut k = 1;
            loop {
                let (lo, hi) = (c0 + k * p, c0 + (k + 1) * p);
                let same = first.iter().all(|(l, t)| at(l + k * p) == Some(t.as_str()));
                let extra = texts.partition_point(|(l, _)| *l < hi) - texts.partition_point(|(l, _)| *l < lo);
                if !same || extra != first.len() {
                    break;
                }
                k += 1;
            }
            if k < 2 {
                continue;
            }
            let ranges: Vec<(u32, u32)> = (0..k)
                .map(|j| (left + j * p, if j + 1 == k { region.rect.right } else { left + (j + 1) * p - 1 }))
                .collect();
            // A header row again further down: stacked tables, each side by side; the stacked
            // split comes first (left as one region here).
            // (rows of the header under row h, before the first row of numbers, do not count)
            let body0 = rows[h + 1..].iter().position(|r| r.n_measure() > 0).map_or(rows.len(), |i| h + 1 + i);
            let stacked = rows[body0..].iter().any(|r| {
                let n = r.n();
                n >= 4 && r.filled().all(|e| e.kind == Kind::Text) && n == r.n_text() && {
                    let t: Vec<(u32, String)> = r.filled().map(|e| (e.left, norm(sheet, e))).collect();
                    let first: Vec<&(u32, String)> = t.iter().filter(|(l, _)| *l <= ranges[0].1).collect();
                    first.len() >= 2 && first.iter().all(|(l, x)| t.iter().any(|(l2, x2)| *l2 == l + p && x2 == x))
                }
            });
            if !stacked && body_fits(sheet, &rows[h + 1..], &ranges, p) {
                return Some(ranges);
            }
        }
    }
    None
}

/// Every copy has a text label column at the same offset, numbers, and other items than the first
/// copy on most rows.
fn body_fits(sheet: &Sheet, rows: &[Row], ranges: &[(u32, u32)], p: u32) -> bool {
    let body: Vec<&Row> = rows.iter().filter(|r| r.n_measure() > 0).collect();
    if body.len() < 2 {
        return false;
    }
    let text_at = |r: &Row, c: u32| r.filled().find(|e| e.left == c && e.kind == Kind::Text).map(|e| norm(sheet, e));
    // Over the rows the copy fills (the last copy of a long list may be shorter).
    // Labels name items: mostly distinct texts, not a repeated mark (`F`, `x`: suppressed values).
    let share = |lo: u32, hi: u32, c: u32| {
        let rows: Vec<&&Row> = body.iter().filter(|r| r.filled().any(|e| e.left >= lo && e.left <= hi)).collect();
        let texts: Vec<String> = rows.iter().filter_map(|r| text_at(r, c)).collect();
        let distinct = texts.iter().collect::<HashSet<_>>().len();
        if (distinct as f32) < LABEL_DISTINCT * texts.len() as f32 {
            return 0.0;
        }
        texts.len() as f32 / rows.len().max(1) as f32
    };
    let labels: Vec<u32> = (0..p).filter(|&o| ranges.iter().all(|&(lo, hi)| share(lo, hi, lo + o) >= LABEL_SHARE)).collect();
    if labels.is_empty() {
        return false;
    }
    let numbers = |lo: u32, hi: u32| body.iter().any(|r| r.filled().any(|e| e.left >= lo && e.left <= hi && e.kind == Kind::Num && !e.label_num));
    if !ranges.iter().all(|&(lo, hi)| numbers(lo, hi)) {
        return false;
    }
    let o = labels[0];
    ranges[1..].iter().all(|&(lo, _)| {
        let (mut both, mut same) = (0, 0);
        for r in &body {
            if let (Some(a), Some(b)) = (text_at(r, ranges[0].0 + o), text_at(r, lo + o)) {
                both += 1;
                same += (a == b) as u32;
            }
        }
        both > 0 && (same as f32) < SAME_ITEMS_SHARE * both as f32
    })
}

/// The copies and whether they came from the validated stacked/ranked path.
/// The transient proof flag scopes shared-header handling to those direct children.
pub(crate) fn split_repeated_copies(sheet: &Sheet, region: &Region) -> Option<(Vec<Region>, bool)> {
    // Explicit full-width titles anchor stacked blocks before their horizontal
    // copies are considered. Validate every block, so a later comparison or
    // unrelated table cannot inherit the first block's split.
    let starts: Vec<_> = region.rows.iter().enumerate().filter_map(|(k, row)| {
        let mut filled = row.filled();
        let title = filled.next()?;
        (filled.next().is_none() && title.kind == Kind::Text
            && sheet.merges.iter().any(|m| m.top == row.row && m.bottom == row.row
                && m.left == region.rect.left && m.right == region.rect.right)
            && title.left == region.rect.left && title.right == region.rect.right).then_some(k)
    }).collect();
    if starts.len() >= 2 && starts[0] == 0 {
        let parts: Vec<_> = starts.iter().enumerate().map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(region.rows.len());
            let rows = region.rows[start..end].to_vec();
            let rect = Rect { top: rows[0].row, bottom: rows.last().unwrap().bottom(), ..region.rect };
            Region { rect, rows }
        }).collect();
        if let Some(ranges) = copies(sheet, &parts[0]) {
            let ranked = |part: &Region| {
                let rows = &part.rows;
                rows.len() >= 5 && rows[1].n() == ranges.len()
                    && rows[1].filled().zip(&ranges).all(|(e, &(left, _))| e.left == left && e.right == left && e.bottom == e.row && e.kind == Kind::Text)
                    && rows[1].filled().map(|e| norm(sheet, e)).collect::<HashSet<_>>().len() == ranges.len()
                    && rows[2].n() == region.width() as usize
                    && rows[2].filled().enumerate().all(|(k, e)| e.left == region.rect.left + k as u32 && e.right == e.left && e.bottom == e.row && e.kind == Kind::Text)
                    && rows.iter().enumerate().all(|(k, row)| row.row == rows[0].row + k as u32)
                    && rows[3..].iter().all(|row| row.n() == region.width() as usize
                        && row.filled().enumerate().all(|(k, e)| e.left == region.rect.left + k as u32 && e.right == e.left && e.bottom == e.row
                            && if ranges.iter().any(|&(left, _)| left == e.left) { e.kind == Kind::Text } else { e.kind == Kind::Num && !e.label_num }))
            };
            if parts.iter().all(|part| ranked(part) && copies(sheet, part).as_ref() == Some(&ranges)) {
                let mut out = Vec::new();
                for part in parts {
                    let (mut children, _) = split_repeated_copies(sheet, &part)?;
                    let caption = &part.rows[0];
                    for child in &mut children {
                        // Share only this proven real metric caption, never body cells.
                        if child.rows[0].row != caption.row {
                            let entries = caption.entries.iter().map(|e| Entry { left: child.rect.left, right: child.rect.right, ..e.clone() }).collect();
                            child.rows.insert(0, Row { row: caption.row, entries });
                            child.rect.top = caption.row;
                        }
                    }
                    out.extend(children);
                }
                return Some((out, true));
            }
        }
        // An unsuccessful new plan must not suppress a valid legacy copy split.
    }
    let ranges = copies(sheet, region)?;
    let mut out = Vec::new();
    for (lo, hi) in ranges {
        let piece: Vec<Row> = region
            .rows
            .iter()
            .map(|r| Row {
                row: r.row,
                // Legacy ownership: cells belong to the copy containing their anchor.
                entries: r
                    .entries
                    .iter()
                    .filter(|e| e.left >= lo && e.left <= hi)
                    .map(|e| Entry { right: e.right.min(hi), ..e.clone() })
                    .collect(),
            })
            .filter(|r| !r.entries.is_empty())
            .collect();
        if piece.is_empty() {
            continue;
        }
        let mut rect = Rect { top: u32::MAX, left: u32::MAX, bottom: 0, right: 0 };
        for r in &piece {
            rect.top = rect.top.min(r.row);
            rect.bottom = rect.bottom.max(r.bottom());
            rect.left = rect.left.min(r.left());
            rect.right = rect.right.max(r.right());
        }
        out.push(Region { rect, rows: piece });
    }
    (out.len() >= 2).then_some((out, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::cells::Agg;
    use crate::model::{Cell, Value};

    #[test]
    fn stacked_copies_require_every_block_and_never_share_body_merges() {
        for case in 0..7 {
            let mut sheet = Sheet::default();
            let mut rows = Vec::new();
            for block in 0..2u32 {
                let top = 9 * block;
                if !(case == 3 && block == 1) { sheet.merges.push(Rect { top, bottom: top, left: 0, right: 3 }); }
                for offset in 0..8u32 {
                    if case == 6 && offset == 1 { continue; }
                    let row = top + offset;
                    let values = match offset {
                        0 => vec![(0, Value::Text(format!("Metric {block}")))],
                        1 => vec![(0, Value::Text("Highest".into())), (2, Value::Text("Least".into()))],
                        2 => vec![(0, Value::Text("State".into())), (1, Value::Text("Value".into())), (2, Value::Text("State".into())), (3, Value::Text(if case == 2 && block == 1 { "Other" } else { "Value" }.into()))],
                        _ => vec![(0, Value::Text(format!("Left {block} {offset}"))), (1, Value::Number(offset as f64)),
                            (2, Value::Text(format!("{} {block} {offset}", if case == 1 && block == 1 { "Left" } else { "Right" }))), (3, Value::Number(10.0 + offset as f64))],
                    };
                    let mut entries = Vec::new();
                    for (col, value) in values {
                        if (case == 4 || case == 5) && block == 1 && offset == 4 && col == 2 { continue; }
                        let cell = sheet.cells.len();
                        let kind = if matches!(value, Value::Number(_)) { Kind::Num } else { Kind::Text };
                        sheet.cells.push(Cell { row, col, value: Some(value), formula: None });
                        let right = if offset == 0 { 3 } else if (case == 4 || case == 5) && block == 1 && offset == 4 && col == 1 { 2 } else { col };
                        entries.push(Entry { row, left: col, right, bottom: row, kind, label_num: false, date: false, agg: Agg::None, rule: false, cell: Some(cell) });
                    }
                    rows.push(Row { row, entries });
                }
            }
            if case == 4 || case == 5 {
                sheet.merges.push(Rect { top: 13, bottom: 13, left: 1, right: 2 });
                if case == 5 {
                    let i = sheet.cells.iter().position(|c| c.row == 13 && c.col == 1).unwrap();
                    sheet.cells[i].value = Some(Value::Text("Merged record label".into()));
                    rows.iter_mut().find(|r| r.row == 13).unwrap().entries.iter_mut().find(|e| e.left == 1).unwrap().kind = Kind::Text;
                }
            }
            let region = Region { rect: Rect { top: 0, bottom: 16, left: 0, right: 3 }, rows };
            let split = split_repeated_copies(&sheet, &region);
            if case == 0 {
                let (parts, shared) = split.unwrap();
                assert!(shared);
                assert_eq!(parts.len(), 4);
                for part in &parts {
                    assert_eq!(part.rows.len(), 8);
                    let title = &part.rows[0].entries[0];
                    assert_eq!((title.left, title.right), (part.rect.left, part.rect.right));
                    assert_eq!(sheet.cells[title.cell.unwrap()].col, 0, "actual merged caption anchor retained");
                }
                let owned: HashSet<_> = parts.iter().flat_map(|p| &p.rows).flat_map(|r| &r.entries).filter_map(|e| e.cell).collect();
                assert_eq!(owned.len(), sheet.cells.len(), "no metadata, title or record disappears");
            } else if case == 2 {
                // The changed second leaf defeats the legacy stacked-header veto.
                // The new proof must fail, while the accepted anchor-only split
                // still owns exactly the same source cells and clipped geometry.
                let (parts, shared) = split.unwrap();
                assert!(!shared);
                assert_eq!(parts.len(), 2);
                for (part, (left, right)) in parts.iter().zip([(0, 1), (2, 3)]) {
                    let expected: Vec<_> = region.rows.iter().flat_map(|r| &r.entries)
                        .filter(|e| left <= e.left && e.left <= right)
                        .map(|e| (e.row, e.left, e.right.min(right), e.bottom, e.cell)).collect();
                    let actual: Vec<_> = part.rows.iter().flat_map(|r| &r.entries)
                        .map(|e| (e.row, e.left, e.right, e.bottom, e.cell)).collect();
                    assert_eq!(actual, expected);
                    assert_eq!(part.rect, Rect { top: if left == 0 { 0 } else { 1 }, bottom: 16, left, right });
                    assert_eq!(part.rows.len(), if left == 0 { 16 } else { 14 });
                    assert_eq!(part.rows.iter().filter(|r| r.row == 0 || r.row == 9).count(), if left == 0 { 2 } else { 0 });
                }
                let mut owned: Vec<_> = parts.iter().flat_map(|p| &p.rows).flat_map(|r| &r.entries).filter_map(|e| e.cell).collect();
                owned.sort_unstable();
                assert_eq!(owned, (0..sheet.cells.len()).collect::<Vec<_>>(), "no duplicated or missing source cells");
            } else { assert!(split.is_none(), "case {case}"); }
        }
    }
}
