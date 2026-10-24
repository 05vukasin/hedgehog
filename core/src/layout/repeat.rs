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
            let stacked = rows[h + 1..].iter().any(|r| {
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

/// The region cut into its side-by-side copies (None: no repeated copies).
pub(crate) fn split_repeated_copies(sheet: &Sheet, region: &Region) -> Option<Vec<Region>> {
    let ranges = copies(sheet, region)?;
    let mut out = Vec::new();
    for (lo, hi) in ranges {
        let piece: Vec<Row> = region
            .rows
            .iter()
            .map(|r| Row {
                row: r.row,
                // A title merged across the copies stays with the first one, cut at its edge.
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
    (out.len() >= 2).then_some(out)
}
