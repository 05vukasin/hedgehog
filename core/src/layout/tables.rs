//! Row-level rules inside one region: titles above, notes below, header depth, stacked-table splits,
//! totals, group headers. Produces tables + cell roles; leftovers go to `Out::loose`.

use std::collections::HashSet;
use std::ops::Range;

use crate::analysis::{DetectedTable, Role};
use crate::model::{Rect, Sheet, Value};
use crate::totals::keywords::REGISTER_MIN_ROWS;
use crate::totals::{incidental_keyword, total_keyword};

use super::blocks::{Entry, Region, Row, Ty};
use super::cells::{Agg, Kind, is_note_label, is_total_label, text_of};
use super::params::*;

pub(crate) struct Out {
    pub roles: Vec<Role>,
    pub tables: Vec<DetectedTable>,
    /// Filled things not in any table; classified once all tables are known.
    pub loose: Vec<Region>,
    /// Column-numbering rows join the header ([`COLUMN_NUMBER_ROWS`]; off for the row model's
    /// features, see [`super::analyze_for_model`]).
    pub column_numbers: bool,
    /// Incidental total words (free text in a register) do not make a row a total row
    /// ([`incidental_keyword`]; off for the row model's features, see [`super::analyze_for_model`]).
    pub registers: bool,
    /// Rows the totals take as labelled by a total word ([`crate::totals::labelled_rows`]: a
    /// label, a category of its column, or an incidental word confirmed by arithmetic), sorted.
    pub labelled_totals: Vec<u32>,
}

fn text<'a>(sheet: &'a Sheet, e: &Entry) -> &'a str {
    e.cell.and_then(|i| text_of(&sheet.cells[i])).unwrap_or("")
}

fn single_text(r: &Row) -> Option<&Entry> {
    let mut it = r.filled();
    let e = it.next()?;
    (it.next().is_none() && e.kind == Kind::Text).then_some(e)
}

/// Sparse row above a table: one text cell (anywhere), or one merged cell over most of the width
/// starting at the left edge. A partial merge (e.g. "Revenue" over C:E) is a parent header instead.
/// A lone label off the left edge, directly over a number in the next row, is a header cell of a
/// narrow table ("Monthly volume" over its column), not a title; so is one over one or two section
/// labels whose rows below have the number (`percentage` over `Q1. Food didn't last` over `Often
/// true | 15.9`, a one-column statistics table).
pub(crate) fn is_title_row(sheet: &Sheet, r: &Row, rect: Rect, below: &[Row]) -> bool {
    title_row(sheet, r, rect, below, TITLE_HEADER_OVER_NUMBER)
}

/// [`is_title_row`]; `direct`: a lone label directly over a number is a header cell too.
fn title_row(sheet: &Sheet, r: &Row, rect: Rect, below: &[Row], direct: bool) -> bool {
    let width = rect.right - rect.left + 1;
    if width < 2 {
        return false;
    }
    let Some(e) = single_text(r) else {
        return false;
    };
    if is_total_label(text(sheet, e)) {
        return false;
    }
    if e.left > rect.left {
        // The next rows, skipping up to two section labels at the left edge (directly over a
        // number only with `direct`: as a default worse on DECO dev, B-public).
        let mut prev = r.row;
        for (k, n) in below.iter().take(3).enumerate() {
            if n.row != prev + 1 {
                break;
            }
            if n.n_measure() > 0 {
                if (k > 0 || direct)
                    && n.entries.iter().any(|x| x.left <= e.right && x.right >= e.left && x.kind == Kind::Num)
                {
                    return false;
                }
                break;
            }
            // A section label: text at the left edge, nothing but missing marks beside it
            // (`Region of birth: | ...`).
            let mut cells = n.filled();
            let section = k < 2
                && cells.next().is_some_and(|x| x.kind == Kind::Text && x.left == rect.left && x.right < e.left)
                && cells.all(|x| x.kind == Kind::Missing);
            if !section {
                break;
            }
            prev = n.row;
        }
    }
    let span = e.right - e.left + 1;
    span == 1 || (e.left == rect.left && span as f32 >= TITLE_SPAN_FRAC * width as f32)
}

/// One text cell alone on its row, not a total label.
pub(crate) fn is_title_row_simple(sheet: &Sheet, r: &Row) -> bool {
    single_text(r).is_some_and(|e| !is_total_label(text(sheet, e)))
}

pub(crate) fn is_note_row(sheet: &Sheet, r: &Row) -> bool {
    let Some(e) = single_text(r) else {
        return false;
    };
    let t = text(sheet, e);
    !is_total_label(t) && (is_note_label(t) || t.trim().chars().count() >= NOTE_MIN_LEN)
}

/// A sentence alone on the last row of a block ("Data as reported by the departments; not
/// audited."): one text cell ending with a full stop, of at least [`NOTE_SENTENCE_WORDS`] words
/// and [`NOTE_SENTENCE_LEN`] characters. Only for the rows that end a block (B38).
fn is_trailing_sentence(sheet: &Sheet, r: &Row) -> bool {
    let Some(e) = single_text(r) else { return false };
    let t = text(sheet, e).trim();
    !is_total_label(t)
        && t.ends_with('.')
        && t.chars().count() >= NOTE_SENTENCE_LEN
        && t.split_whitespace().count() >= NOTE_SENTENCE_WORDS
}

/// Metadata written as key/value pairs on one row ("Date of publication: | 15 Sep 2026 | … |
/// Date of next publication: | 20 Oct 2026", B46): at least two cells, and at least half of them
/// a `Key:` label (a word, then a colon in the first 40 characters). Not a header.
pub(crate) fn is_key_value_row(sheet: &Sheet, r: &Row) -> bool {
    let n = r.n();
    if n < 2 {
        return false;
    }
    // A `Key:` label with its value in the cell beside it, or `Key: value` in one cell.
    let filled: Vec<&Entry> = r.filled().collect();
    let keys = filled
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == Kind::Text)
        .filter(|&(k, e)| {
            let t = text(sheet, e).trim();
            let colon = t.char_indices().take(40).find(|&(i, c)| {
                c == ':' && t[..i].chars().last().is_some_and(char::is_alphabetic)
                    && t[i + 1..].chars().next().is_none_or(|c| !c.is_ascii_digit())
            });
            match colon {
                Some((i, _)) if t[i + 1..].trim().is_empty() => {
                    filled.get(k + 1).is_some_and(|x| x.left <= e.right + 2 && !text(sheet, x).trim_end().ends_with(':'))
                }
                Some(_) => true,
                None => false,
            }
        })
        .count();
    let pure = r
        .filled()
        .filter(|e| e.kind == Kind::Text && text(sheet, e).trim_end().ends_with(':'))
        .count();
    2 * keys >= n && (keys >= 2 || (n == 2 && pure == 1 && r.n_text() == 1))
}

/// A text cell that is a `Key:` label (ends with a colon, ASCII or full-width).
fn has_key_label(sheet: &Sheet, r: &Row) -> bool {
    r.filled().any(|e| e.kind == Kind::Text && text(sheet, e).trim_end().ends_with([':', '：']))
}

/// Header row relative to the rows below it: more of its cells break the column's body type
/// (text or a year over plain numbers) than repeat it (Pytheas-style signature votes).
pub(crate) fn is_header_row(r: &Row, below: &[Row]) -> bool {
    if !is_headerish(r) {
        return false;
    }
    let window = &below[..below.len().min(HEADER_VOTE_WINDOW)];
    // Rows whose entries do not overlap (each ends left of the next one's start, the usual case):
    // the entries overlapping a column range are one run, found by binary search (B128).
    let disjoint: Vec<bool> = window.iter().map(|w| w.entries.windows(2).all(|p| p[0].right < p[1].left)).collect();
    let (mut head, mut data) = (0u32, 0u32);
    for e in r.filled() {
        let t = e.ty();
        if t == Ty::None {
            continue;
        }
        let (mut tot, mut same, mut numeric) = (0u32, 0u32, 0u32);
        for (w, &disjoint) in window.iter().zip(&disjoint) {
            let run = if disjoint {
                let lo = w.entries.partition_point(|x| x.right < e.left);
                let hi = w.entries.partition_point(|x| x.left <= e.right).max(lo);
                &w.entries[lo..hi]
            } else {
                &w.entries[..]
            };
            for x in run.iter().filter(|x| x.left <= e.right && x.right >= e.left) {
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

/// [`has_total_label`] for the row's role: in a register, a long text right of other text (a
/// description or a name holding a total word, [`incidental_keyword`]) does not label the row as a
/// total unless the totals take the row as labelled (`labelled`). Table boundaries still treat such
/// rows as before ([`is_total_row`]).
fn has_own_total_label(sheet: &Sheet, r: &Row, labelled: &[u32]) -> bool {
    if labelled.binary_search(&r.row).is_ok() {
        return has_total_label(sheet, r);
    }
    let rows = match (sheet.cells.first(), sheet.cells.last()) {
        (Some(a), Some(b)) => b.row - a.row + 1,
        _ => 0,
    };
    // Text right of the label is left to the normalized output's row overrides: the learned model
    // keeps these rows' roles as trained ("Total debt service (% of GNI) | country | …").
    // B50: those overrides know only the totals' words; a word only this layer reads as a total
    // ("Maximum", "Average" in "MAXIMUM MARKETING SERVICES INC", "MORE THAN AVERAGE LLC") does not
    // label a register row that holds other text, since nothing downstream would take it back.
    let texts = r.entries.iter().filter(|e| e.kind == Kind::Text).count();
    let other_text = rows >= REGISTER_MIN_ROWS && texts > 1;
    r.entries
        .iter()
        .filter(|e| e.kind == Kind::Text)
        .take(3)
        .enumerate()
        .any(|(k, e)| {
            let t = text(sheet, e);
            is_total_label(t)
                && !incidental_keyword(t, k > 0, false, rows)
                && !(other_text && total_keyword(t).is_none())
        })
}

/// Most of the row's numbers are aggregate formulas over the cells above.
fn has_vertical_aggregates(r: &Row) -> bool {
    let vagg = r.entries.iter().filter(|e| e.agg == Agg::Vertical).count();
    vagg > 0 && vagg as f32 >= TOTAL_FORMULA_FRAC * r.n_num() as f32
}

pub(crate) fn is_total_row(sheet: &Sheet, r: &Row) -> bool {
    has_total_label(sheet, r) || has_vertical_aggregates(r)
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

pub(crate) type Signature = Vec<(u32, String)>;

/// What a row says, cell by cell, for spotting a repeated header: each filled cell's column and
/// its lower-cased text (a number counts as an empty text, so the labels decide). A row without
/// any text is told by its values instead: a header of dates repeats only with the same dates,
/// and a row of numbers under it is data, not a repeat of it.
pub(crate) fn signature(sheet: &Sheet, r: &Row) -> Signature {
    let value = |e: &Entry| e.cell.and_then(|i| sheet.cells[i].value.as_ref());
    let labelled = r.filled().any(|e| matches!(value(e), Some(Value::Text(_))));
    r.filled()
        .map(|e| {
            let key = match value(e) {
                Some(Value::Text(s)) => s.trim().to_lowercase(),
                // A control character keeps a value apart from any text.
                Some(v) if !labelled => format!("\u{1}{v:?}"),
                _ => String::new(),
            };
            (e.left, key)
        })
        .collect()
}

/// `signature(sheet, r) == *sig`, without building the row's signature (B128).
pub(crate) fn signature_is(sheet: &Sheet, r: &Row, sig: &Signature) -> bool {
    let value = |e: &Entry| e.cell.and_then(|i| sheet.cells[i].value.as_ref());
    if r.filled().count() != sig.len() {
        return false;
    }
    let labelled = r.filled().any(|e| matches!(value(e), Some(Value::Text(_))));
    r.filled().zip(sig).all(|(e, (left, key))| {
        e.left == *left
            && match value(e) {
                Some(Value::Text(s)) => {
                    let t = s.trim();
                    if t.is_ascii() {
                        t.len() == key.len() && t.bytes().zip(key.bytes()).all(|(a, b)| a.to_ascii_lowercase() == b)
                    } else {
                        t.to_lowercase() == *key
                    }
                }
                Some(v) if !labelled => format!("\u{1}{v:?}") == *key,
                _ => key.is_empty(),
            }
    })
}

fn is_wide_text_row(r: &Row, rect: Rect) -> bool {
    let fill: u32 = r.filled().map(|e| e.right.min(rect.right) - e.left + 1).sum();
    let width = rect.right - rect.left + 1;
    r.n() >= 2 && r.n_text() == r.n() && fill as f32 >= TEXT_HEADER_MIN_FILL * width as f32
}

/// A short label at the table's left edge right over its header row (`Term` over `Delivery Pt |
/// Q3-2002 | …`, under the title "Last Week's Peak Power Prices"): the top row of the header, not
/// a second title (B55).
fn is_label_over_header(sheet: &Sheet, rows: &[Row], k: usize, end: usize, rect: Rect) -> bool {
    let Some(e) = single_text(&rows[k]) else { return false };
    let t = text(sheet, e).trim();
    let next = match rows.get(k + 1) {
        Some(n) if k + 2 < end && n.row == rows[k].row + 1 => n,
        _ => return false,
    };
    LABEL_OVER_HEADER
        && e.left == rect.left
        && e.right == e.left
        && t.chars().count() <= 20
        && t.split_whitespace().count() <= 2
        && !t.chars().any(|c| c.is_ascii_digit())
        && !super::cells::is_unit_text(t)
        && !is_total_label(t)
        && !is_note_label(t)
        && !t.contains(':')
        && next.n() >= 3
        && next.n_measure() == 0
        && next.filled().next().is_some_and(|x| x.left == rect.left && x.kind == Kind::Text)
        && is_header_row(next, &rows[k + 2..end])
}

/// Number of leading rows (from `start`) that form the header.
fn header_depth(sheet: &Sheet, rows: &[Row], start: usize, end: usize, rect: Rect, numbers: bool) -> usize {
    let mut d = 0;
    // A parent row the type votes did not take does not count against the header-row limit.
    let mut extra = 0;
    while d < MAX_HEADER_ROWS + extra && start + d < end {
        let r = &rows[start + d];
        if is_key_value_row(sheet, r) {
            // Rows above key/value metadata are titles or metadata too, not a header ("Profit
            // Sharing Summary" over "Operation Day: | 12 May 2001"); but a row of form labels
            // (`ATTORNEY:` over `DATE: | … | DEAL MAKER: | …`, B57) keeps its header place.
            let form_labels = rows[start..start + d]
                .iter()
                .any(|x| x.filled().any(|e| e.kind == Kind::Text && text(sheet, e).trim_end().ends_with(':')));
            if !form_labels {
                d = 0;
            }
            break;
        }
        if !is_header_row(r, &rows[start + d + 1..end]) || is_note_row(sheet, r) {
            // Parent labels over a header row (`Kalenderkorrigerad | … | Trend` over `2016K3 |
            // 2016K4 | …`; `2011 | 2015 | 2018` over the measures, B53): labels only, spread over
            // the columns of the header row below, which heads the numbers.
            // Up to two parent rows stack (`2011 | 2015` over `Starost | Starost` over the
            // measures).
            let k = start + d;
            // Right edge of the two rows under row `k` (needs `k + 2 < end`).
            let below = |k: usize| rows[k + 1].right().max(rows[k + 2].right());
            let parent_of = |k: usize| {
                PARENT_HEADER_ROWS
                    && k + 2 < end
                    // two labels or more, or one merged over a group of columns (`2019` over
                    // `Population | Area | Births`, B39), none starting right of the columns of
                    // the two rows below and a lone merged label not reaching past them (a title
                    // merged over the whole row, `A2:XFD2`, or a copyright line right of a
                    // six-column header is no group label, B90)
                    && (rows[k].n() >= 2 || rows[k].filled().next().is_some_and(|e| e.right > e.left && e.right <= below(k)))
                    && rows[k].filled().all(|e| e.left <= below(k))
                    && rows[k].n_measure() == 0
                    && rows[k].n_text() + (rows[k].n_num() - rows[k].n_measure()) == rows[k].n()
                    && rows[k + 1].row == rows[k].row + 1
                    && rows[k + 1].n() > rows[k].n()
                    && !is_key_value_row(sheet, &rows[k + 1])
            };
            let header_at = |k: usize| is_header_row(&rows[k], &rows[k + 1..end]);
            let parent = (d == 0 && is_label_over_header(sheet, rows, start, end, rect))
                || ((d == 0 || (d == 1 && parent_of(start) && !header_at(start)))
                    && parent_of(k)
                    && (header_at(k + 1) || (d == 0 && parent_of(k + 1) && header_at(k + 2))));
            if !parent {
                break;
            }
            extra = 1;
        }
        // A lone label below the first header row starts a group, not another header level.
        if d > 0 && single_text(r).is_some_and(|e| e.right == e.left) {
            break;
        }
        d += 1;
    }
    if !rows[start + d..end].iter().any(|r| r.n_measure() > 0) {
        // Text-only table (or no body): the first row is a header if it is wide and all text;
        // so is the first such row under up to two sparser rows of parent labels (`2012 | 2013 |
        // 2014` over `Fitch | S&P | Moody's` over `Company | Type of Audit | Month | …`,
        // RealHiTBench labor-table50).
        let width = rect.right - rect.left + 1;
        let wide_text = |r: &Row| {
            let fill: u32 = r.filled().map(|e| e.right.min(rect.right) - e.left + 1).sum();
            r.n() >= 2 && r.n_text() == r.n() && fill as f32 >= TEXT_HEADER_MIN_FILL * width as f32
        };
        let parent = |k: usize| {
            let r = &rows[k];
            r.n() >= 2
                && r.n_measure() == 0
                && r.n_text() + (r.n_num() - r.n_measure()) == r.n()
                && rows[k + 1].row == r.row + 1
                && r.n() * 2 <= rows[k + 1].n()
        };
        let mut j = start;
        // A wide row of parent labels (merged over their groups) counts as a parent too when a
        // wide all-text row twice as dense follows.
        while PARENT_HEADER_ROWS
            && j < start + 2
            && j + 2 < end
            && parent(j)
            && (!wide_text(&rows[j]) || wide_text(&rows[j + 1]))
        {
            j += 1;
        }
        let ok = end - j >= 2 && wide_text(&rows[j]);
        return if ok { j - start + 1 } else { 0 };
    }
    if d == 0 {
        // Prior: nearly every table has a header. A wide all-text first row counts, and so does
        // a label over period numbers (`Year | 1 | 2 | …`).
        let vals: Option<Vec<&Value>> =
            rows[start].filled().map(|e| e.cell.and_then(|i| sheet.cells[i].value.as_ref())).collect();
        // Several labels before the numbers only when no header row follows: over a header of
        // dates (`TIME BUCKET | REGION | 1 | 2 | …` over two rows of dates, DECO peter_keavey) the
        // numbers row is not the header.
        let next_header = || {
            start + 2 < end
                && rows[start + 1].row == rows[start].row + 1
                && is_header_row(&rows[start + 1], &rows[start + 2..end])
        };
        let periods = vals.is_some_and(|v| {
            super::labelled_numbering_single(v.iter().copied())
                || (super::labelled_numbering(v.iter().copied()) && !next_header())
        });
        if end - start >= 2 && (periods || (HEADER_PRIOR && is_wide_text_row(&rows[start], rect))) {
            d = 1;
        } else {
            return 0;
        }
    }
    // A ruling line drawn under the header (`-------- | -----`) underlines it.
    // A column-numbering row right under the header ("1 2 3 4 …") is part of it, and so is a
    // unit row ("Mrd. EUR | % | 2020 = 100", B45), in either order.
    for _ in 0..2 {
        if !numbers || start + d + 1 >= end {
            break;
        }
        let r = &rows[start + d];
        if is_column_number_row(sheet, r) || (UNIT_ROWS && is_unit_row(sheet, r)) {
            d += 1;
        }
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

/// A unit row under a header: at least two cells, all unit labels ([`super::cells::is_unit_text`]).
pub(crate) fn is_unit_row(sheet: &Sheet, r: &Row) -> bool {
    r.n() >= 2 && r.filled().all(|e| e.kind == Kind::Text && super::cells::is_unit_text(text(sheet, e)))
}

/// A column-numbering row ([`super::column_numbering`]).
pub(crate) fn is_column_number_row(sheet: &Sheet, r: &Row) -> bool {
    let vals: Option<Vec<&Value>> = r.filled().map(|e| e.cell.and_then(|i| sheet.cells[i].value.as_ref())).collect();
    vals.is_some_and(|v| super::column_numbering(v.into_iter()))
}

/// A header-like row inside a body with text where the body has numbers starts a new table.
fn starts_new_table(
    sheet: &Sheet,
    rows: &[Row],
    k: usize,
    end: usize,
    numeric_cols: &HashSet<u32>,
    header_sig: &Signature,
    seen: &HashSet<(u32, String)>,
) -> bool {
    let r = &rows[k];
    if r.filled().all(|e| e.rule) {
        return false; // a ruling line (B38)
    }
    if numeric_cols.is_empty() || r.n() < 2 || !is_header_row(r, &rows[k + 1..end]) {
        return false;
    }
    if !rows[k + 1..end].iter().take(MAX_HEADER_ROWS).any(|x| x.n_measure() > 0) {
        return false;
    }
    if !header_sig.is_empty() && signature_is(sheet, r, header_sig) {
        return false; // repeated header (H9), stays in the table
    }
    // A text that the body above already holds in the same column is one of its values (a
    // suppression mark, `F` = too unreliable, on a row of them), not a new header's label.
    let hits = r
        .filled()
        .skip(1)
        .filter(|e| (e.kind == Kind::Text || e.label_num) && numeric_cols.contains(&e.left))
        .filter(|e| !(e.kind == Kind::Text && seen.contains(&(e.left, seen_key(text(sheet, e))))))
        // nor is a value annotation (`F` too unreliable, `x` suppressed, `<5`)
        .filter(|e| !(e.kind == Kind::Text && crate::values::missing::numeric_annotation(text(sheet, e)).is_some()))
        .count();
    hits >= SPLIT_MIN_HITS && hits as f32 >= SPLIT_MIN_HIT_FRAC * numeric_cols.len() as f32
}

/// Texts up to this long are remembered per column while a body is read ([`starts_new_table`]):
/// markers and codes, not long labels.
const SEEN_MAX_CHARS: usize = 12;

fn seen_key(t: &str) -> String {
    t.trim().to_lowercase()
}

/// Long sparse tail of a table (helper formulas or a lone column running far below the data):
/// trailing rows that together use only a few columns compared with the header / first body
/// rows. Returns the removed rows.
fn trim_tail(sheet: &Sheet, rows: &[Row], seg: &mut Seg) -> Range<usize> {
    let body = seg.start + seg.header;
    if seg.end <= body + TAIL_MIN_ROWS {
        return seg.end..seg.end;
    }
    // Reference width: the header's last row and the first body rows.
    let first = rows[body..seg.end].iter().take(3).map(|r| r.n());
    let reference = first.chain((seg.header > 0).then(|| rows[body - 1].n())).max().unwrap_or(0);
    let limit = TAIL_MAX_FILL_FRAC * reference as f32;
    let mut cols: HashSet<u32> = HashSet::new();
    let mut k = seg.end;
    while k > body && !is_total_row(sheet, &rows[k - 1]) {
        cols.extend(rows[k - 1].filled().map(|e| e.left));
        if cols.len() as f32 > limit {
            break;
        }
        k -= 1;
    }
    if seg.end - k >= TAIL_MIN_ROWS && k >= body + 2 {
        let tail = k..seg.end;
        seg.end = k;
        tail
    } else {
        seg.end..seg.end
    }
}

/// Single label at the table's left edge (or merged across), followed — possibly after nested
/// group labels — by a run of at least `GROUP_MIN_RUN` multi-cell rows.
fn is_group_header(sheet: &Sheet, rows: &[Row], k: usize, end: usize, rect: Rect) -> bool {
    let Some(e) = single_text(&rows[k]) else {
        return false;
    };
    if e.left > rect.left && e.right == e.left {
        return false;
    }
    let mut j = k + 1;
    while j < end && j <= k + 2 && single_text(&rows[j]).is_some() {
        j += 1;
    }
    if j + GROUP_MIN_RUN <= end && rows[j..j + GROUP_MIN_RUN].iter().all(|r| r.n() >= 2) {
        return true;
    }
    // A short label ("Dec", "Region North") directly over one data row also opens a group.
    j == k + 1 && j < end && rows[j].n() >= 2 && is_short_label(text(sheet, e))
}

/// A label short enough to name a group: at most [`GROUP_SHORT_LABEL`] characters and 3 words,
/// not a numbered item ("1. Recalc …").
fn is_short_label(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty()
        && t.chars().count() <= GROUP_SHORT_LABEL
        && !t.starts_with(|c: char| c.is_ascii_digit())
        && t.split_whitespace().count() <= 3
}

/// Group labels written on the first data row of their group instead of a row of their own
/// ("Oct | … | 370,494" after "Sep" had a row to itself): in a table with group-header rows, a data
/// row that follows a blank row and starts with a short text label in the group labels' column,
/// where short labels are otherwise rare (not a column of item names). Its label cell becomes a
/// group header; the rest of the row stays data.
fn mark_inline_group_labels(sheet: &Sheet, rows: &[Row], body: Range<usize>, data_rows: &[usize], out: &mut Out) {
    let label_col = |k: usize| rows[k].filled().next().filter(|e| e.kind == Kind::Text).map(|e| e.left);
    let group_rows = body.clone().filter(|&k| {
        rows[k].filled().next().and_then(|e| e.cell).is_some_and(|i| out.roles[i] == Role::GroupHeader)
            && single_text(&rows[k]).is_some()
    });
    let mut cols: Vec<u32> = group_rows.filter_map(label_col).collect();
    cols.sort_unstable();
    cols.dedup();
    let [col] = cols[..] else { return };
    let short = |k: usize| {
        rows[k].filled().next().filter(|e| e.left == col && e.kind == Kind::Text && is_short_label(text(sheet, e)))
    };
    let n_short = data_rows.iter().filter(|&&k| short(k).is_some()).count();
    if n_short as f32 > INLINE_GROUP_MAX_LABEL_FRAC * data_rows.len() as f32 {
        return;
    }
    for &k in data_rows {
        if k == body.start || rows[k - 1].bottom() + 1 >= rows[k].row || rows[k].n() < 2 {
            continue;
        }
        if let Some(i) = short(k).and_then(|e| e.cell) {
            out.roles[i] = Role::GroupHeader;
        }
    }
}

#[derive(Clone)]
struct Seg {
    titles: Range<usize>,
    /// Piece cut off by a change point, or a long `label | number` list (a narrow headerless
    /// table is still a table).
    split: bool,
    start: usize,
    header: usize,
    end: usize,
}

#[allow(clippy::too_many_arguments)]
fn segment(
    sheet: &Sheet,
    rows: &[Row],
    titles: Range<usize>,
    s: usize,
    e: usize,
    rect: Rect,
    cut: bool,
    numbers: bool,
    sparse_pre: bool,
) -> Vec<Seg> {
    let mut segs = Vec::new();
    // Leading title rows belong to the first table.
    let (mut start, mut titles) = (s, titles);
    while start < e {
        let mut h = header_depth(sheet, rows, start, e, rect, numbers);
        // Metadata rows (dates, labels) between titles and the header: skip up to a few sparse rows.
        if h == 0 && segs.is_empty() {
            let mut o = 0;
            while o < MAX_PRE_HEADER_ROWS
                && start + o + 2 < e
                && (rows[start + o].n() <= 2
                    || is_key_value_row(sheet, &rows[start + o])
                    // `sparse_pre`: any row with a third of the cells of the row below it
                    // (`Stock D | Code: | 656839` over a 12-column header); without it, such a
                    // row that holds a `Key:` label (`Project Name: | Number of Clients: | 7`
                    // over a 13-column header, MiMoTable ProjectStatisticsTable1).
                    || ((sparse_pre || has_key_label(sheet, &rows[start + o]))
                        && rows[start + o].n() * 3 <= rows[start + o + 1].n()))
            {
                o += 1;
                let h2 = header_depth(sheet, rows, start + o, e, rect, numbers);
                // Past key/value rows, only a row that reads as a header by its types (not by
                // the wide-text prior) starts the table: a form's next `Key: | value` row is not
                // a header (B57).
                let past_kv = rows[start..start + o].iter().any(|r| is_key_value_row(sheet, r));
                let real = !past_kv
                    || (!is_key_value_row(sheet, &rows[start + o])
                        && is_header_row(&rows[start + o], &rows[start + o + 1..e]));
                if h2 > 0 && real {
                    titles = titles.start..start + o;
                    start += o;
                    h = h2;
                    break;
                }
            }
        }
        let body = start + h;
        let sig = if h > 0 { signature(sheet, &rows[body - 1]) } else { Vec::new() };
        let mut numeric_cols: HashSet<u32> = HashSet::new();
        let mut num_counts: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut split = None;
        // Short texts of the body rows with numbers so far, by column ([`starts_new_table`]).
        let mut seen: HashSet<(u32, String)> = HashSet::new();
        for k in body..e {
            if k > body && k + 1 < e && starts_new_table(sheet, rows, k, e, &numeric_cols, &sig, &seen) {
                split = Some(k);
                break;
            }
            // Only from rows with numbers: a header the body repeats (page headers) is no value.
            for en in rows[k].filled().filter(|en| en.kind == Kind::Text && rows[k].n_measure() > 0) {
                let t = text(sheet, en);
                if t.trim().chars().count() <= SEEN_MAX_CHARS {
                    seen.insert((en.left, seen_key(t)));
                }
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
            segs.push(Seg { titles, split: cut, start, header: h, end: e });
            break;
        };
        // Title rows just above the next header belong to the next table.
        let mut seg_end = sp;
        while seg_end > body + 1
            && sp - seg_end < 2
            && is_title_row(sheet, &rows[seg_end - 1], rect, &rows[seg_end..])
            && !is_note_row(sheet, &rows[seg_end - 1])
        {
            seg_end -= 1;
        }
        segs.push(Seg { titles, split: cut, start, header: h, end: seg_end });
        titles = seg_end..sp;
        start = sp;
    }
    segs
}

fn set(out: &mut Out, r: &Row, role: Role) {
    set_upto(out, r, role, u32::MAX);
}

/// Like [`set`], for entries starting at or left of `max_col` only.
fn set_upto(out: &mut Out, r: &Row, role: Role, max_col: u32) {
    for e in r.entries.iter().filter(|e| e.left <= max_col) {
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

/// Header + many body rows + key columns filled on nearly every row: a sparse but real table.
fn sparse_register(rows: &[Row], seg: &Seg, body_start: usize) -> bool {
    let body = &rows[body_start..seg.end];
    if seg.header == 0 || body.len() < SPARSE_MIN_BODY_ROWS {
        return false;
    }
    let mut per_col: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for r in body {
        for e in r.filled() {
            *per_col.entry(e.left).or_default() += 1;
        }
    }
    let need = (SPARSE_KEY_FILL * body.len() as f32).ceil() as usize;
    per_col.values().filter(|&&n| n >= need).count() >= SPARSE_KEY_COLS
}

/// The range of the table `seg` would be, if it qualifies as one (size, header, density).
fn qualifies(rows: &[Row], seg: &Seg) -> Option<Rect> {
    let trows = &rows[seg.start..seg.end];
    let body_start = seg.start + seg.header;
    if trows.len() < MIN_TABLE_ROWS || body_start >= seg.end {
        return None;
    }
    let mut rect = bounds(trows);
    // DECO counts title rows touching the table (no blank row between) as part of its range.
    if TABLE_INCLUDES_ADJACENT_TITLES {
        let mut top = rows[seg.start].row;
        for k in seg.titles.clone().rev() {
            if rows[k].row + 1 == top {
                top = rows[k].row;
            } else {
                break;
            }
        }
        rect.top = rect.top.min(top);
    }
    // Clamp a vertical merge hanging off the last row.
    rect.bottom = rect.bottom.max(trows.last().unwrap().row);
    let width = rect.right - rect.left + 1;
    if width < MIN_TABLE_COLS {
        return None;
    }
    // Without a header, a narrow or short block is a key/value list or stray numbers, not a table.
    if seg.header == 0 && ((width < HEADERLESS_MIN_COLS && !seg.split) || (seg.end - seg.start) < HEADERLESS_MIN_ROWS) {
        return None;
    }
    if seg.header > 0 && seg.end - body_start < MIN_BODY_ROWS {
        return None;
    }
    // Blank spacer columns (between row labels and numbers) don't count against the density.
    let mut spans: Vec<(u32, u32)> =
        trows.iter().flat_map(|r| r.filled()).map(|e| (e.left, e.right.min(rect.right))).collect();
    spans.sort_unstable();
    let (mut used, mut end) = (0u64, None::<u32>);
    for (l, r) in spans {
        let l = end.map_or(l, |e| l.max(e + 1));
        if r >= l {
            used += (r - l + 1) as u64;
            end = Some(r);
        }
    }
    let area = (rect.bottom - rect.top + 1) as f64 * used.max(1) as f64;
    let filled: f64 = trows
        .iter()
        .flat_map(|r| r.filled())
        .map(|e| (e.right.min(rect.right) - e.left + 1) as f64 * (e.bottom.min(rect.bottom) - e.row + 1) as f64)
        .sum();
    if filled / area < MIN_TABLE_DENSITY as f64 && !sparse_register(rows, seg, body_start) {
        return None;
    }
    Some(rect)
}

/// Emits a table for `seg` if it qualifies; returns false otherwise (caller makes it loose).
fn emit_table(sheet: &Sheet, rows: &[Row], seg: &Seg, out: &mut Out) -> bool {
    let Some(mut rect) = qualifies(rows, seg) else { return false };
    let trows = &rows[seg.start..seg.end];
    let body_start = seg.start + seg.header;


    for r in &rows[seg.titles.clone()] {
        set(out, r, if is_note_row(sheet, r) { Role::Note } else { Role::Title });
    }
    // Unlabelled helper columns at the right edge (no header cell above them) are not the table's.
    if RIGHT_TRIM_UNHEADED && seg.header > 0 {
        let covered = rows[seg.start..body_start].iter().map(|r| r.right()).max().unwrap_or(rect.right);
        rect.right = rect.right.min(covered.max(rect.left + 1));
    }
    // A comment column right of the table ("check!", "per KL 3/4" on a row or two): no header
    // cell, only text, on few body rows. Its cells are notes, not a data column (B38).
    let mut comments: Vec<usize> = Vec::new();
    if seg.header > 0 && rect.right > rect.left + 1 {
        let c = rect.right;
        let headed = rows[seg.start..body_start].iter().any(|r| r.filled().any(|e| e.left <= c && e.right >= c));
        let body = &rows[body_start..seg.end];
        let at_c: Vec<&Entry> = body.iter().flat_map(|r| r.filled().filter(|e| e.left == c && e.right == c)).collect();
        let others = body.iter().flat_map(|r| r.filled()).any(|e| e.left < c && e.right >= c);
        let left_fill = body.iter().filter(|r| r.filled().any(|e| e.left == c - 1)).count();
        if !headed
            && !others
            && !at_c.is_empty()
            && at_c.len() <= COMMENT_MAX_CELLS.max((COMMENT_MAX_FRAC * body.len() as f32) as usize)
            && at_c.iter().all(|e| e.kind == Kind::Text)
            // remarks are scattered, not a run of values down the column
            && at_c.windows(2).all(|w| w[1].row > w[0].row + 1)
            && left_fill * 2 > body.len()
        {
            comments = at_c.iter().filter_map(|e| e.cell).collect();
            rect.right = trows
                .iter()
                .flat_map(|r| r.entries.iter().filter(|e| e.left < c))
                .map(|e| e.right.min(c - 1))
                .max()
                .unwrap_or(c - 1);
        }
    }
    let mut header_rows = 0;
    if seg.header > 0 {
        let first_body = rows[body_start].row;
        let mut last = rows[body_start - 1].row;
        for r in &rows[seg.start..body_start] {
            set_upto(out, r, Role::Header, rect.right);
            for e in &r.entries {
                if e.bottom < first_body {
                    last = last.max(e.bottom);
                }
            }
        }
        header_rows = last - rows[seg.start].row + 1;
    }
    let sig = if seg.header > 0 { signature(sheet, &rows[body_start - 1]) } else { Vec::new() };
    let has_measures = rows[body_start..seg.end].iter().any(|r| r.n_measure() > 0);

    // Column type purity over data rows, for confidence. Ordered by column: the mean below is a
    // float sum, and a hash map's iteration order (random per run) changed its last bits (B68).
    let mut col_counts: std::collections::BTreeMap<u32, (u32, u32)> = std::collections::BTreeMap::new();
    // Formula totals on most rows are a running/computed column, not total rows.
    let body_len = seg.end - body_start;
    let formula_totals =
        rows[body_start..seg.end].iter().filter(|r| !has_total_label(sheet, r) && is_total_row(sheet, r)).count();
    let formula_totals_ok = (formula_totals as f32) <= TOTAL_MAX_ROW_FRAC * body_len as f32 || body_len < 4;
    let mut body_cells = 0usize;
    let mut data_rows: Vec<usize> = Vec::new();
    for k in body_start..seg.end {
        let r = &rows[k];
        let last_body = k + 1 == seg.end;
        let total_label =
            if out.registers { has_own_total_label(sheet, r, &out.labelled_totals) } else { has_total_label(sheet, r) };
        let role = if !sig.is_empty() && r.n() >= 2 && signature_is(sheet, r, &sig) {
            Role::Header
        } else if total_label || (formula_totals_ok && has_vertical_aggregates(r)) {
            Role::Total
        } else if is_note_row(sheet, r) && (last_body || is_note_label(text(sheet, single_text(r).unwrap()))) {
            Role::Note
        } else if has_measures && is_group_header(sheet, rows, k, seg.end, rect) {
            Role::GroupHeader
        } else {
            Role::Data
        };
        set_upto(out, r, role, rect.right);
        if role == Role::Data {
            data_rows.push(k);
            for e in r.entries.iter().filter(|e| e.left <= rect.right) {
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
    for i in comments {
        out.roles[i] = Role::Note;
    }
    if INLINE_GROUP_LABELS {
        mark_inline_group_labels(sheet, rows, body_start..seg.end, &data_rows, out);
    }
    if KEY_COLUMN_GROUPS {
        mark_key_column(rows, &data_rows, rect, out);
    }
    let purities: Vec<f32> =
        col_counts.values().filter(|(t, n)| t + n >= 2).map(|&(t, n)| t.max(n) as f32 / (t + n) as f32).collect();
    let purity = if purities.is_empty() { 0.5 } else { purities.iter().sum::<f32>() / purities.len() as f32 };
    let confidence = (CONF_BASE
        + if header_rows > 0 { CONF_HEADER } else { 0.0 }
        + CONF_TYPE * purity
        + CONF_SIZE * (body_cells as f32 / CONF_SIZE_CELLS).min(1.0))
    .clamp(0.0, 1.0);
    let header_top = rows[seg.start].row;
    out.tables.push(DetectedTable { rect, header_top, header_rows, confidence });
    true
}

/// Tables side by side without a blank column between them (B35): a title row with one title per
/// table right over a shared header row, each later title over a text label column that follows a
/// numeric column (`Extract from gapminder | … | Hospital activity` over `country | year | pop |
/// gdpPercap | Ward | Beds`). The pieces between the titles' columns.
fn split_adjacent_tables(region: &Region) -> Option<Vec<Region>> {
    let rows = &region.rows;
    if !SPLIT_ADJACENT_BY_TITLES || rows.len() < 5 {
        return None;
    }
    let (t, h) = (&rows[0], &rows[1]);
    let titles: Vec<&Entry> = t.filled().collect();
    if titles.len() < 2
        || titles.iter().any(|e| e.kind != Kind::Text)
        || titles[0].left != region.rect.left
        || h.row != t.row + 1
        || h.n() < 4
        || h.n_measure() > 0
    {
        return None;
    }
    let body = &rows[2..];
    let share = |c: u32, text: bool| {
        let hit = |e: &Entry| e.left == c && if text { e.kind == Kind::Text } else { e.kind == Kind::Num && !e.label_num };
        body.iter().filter(|r| r.filled().any(hit)).count() as f32 / body.len() as f32
    };
    let mut cuts = vec![region.rect.left];
    for e in &titles[1..] {
        let c = e.left;
        let headed = h.filled().any(|x| x.left == c) && h.filled().any(|x| x.right + 1 == c);
        if !headed || share(c, true) < ADJACENT_SPLIT_SHARE || share(c - 1, false) < ADJACENT_SPLIT_SHARE {
            return None;
        }
        cuts.push(c);
    }
    let mut out = Vec::new();
    for (k, &lo) in cuts.iter().enumerate() {
        let hi = cuts.get(k + 1).map_or(region.rect.right, |&c| c - 1);
        let piece: Vec<Row> = rows
            .iter()
            .map(|r| Row { row: r.row, entries: r.entries.iter().filter(|e| e.left >= lo && e.left <= hi).cloned().collect() })
            .filter(|r| !r.entries.is_empty())
            .collect();
        out.push(Region { rect: bounds(&piece), rows: piece });
    }
    Some(out)
}

pub(crate) fn analyze_region(sheet: &Sheet, region: Region, out: &mut Out) {
    if let Some(parts) = split_adjacent_tables(&region) {
        for p in parts {
            analyze_region(sheet, p, out);
        }
        return;
    }
    let rows = &region.rows;
    if rows.len() < MIN_TABLE_ROWS || region.width() < MIN_TABLE_COLS || is_form(sheet, &region) {
        out.loose.push(region);
        return;
    }
    // The rows' titles, body end and table segments; `direct`: a lone label directly over a
    // number is a header cell, not a title; `sparse_pre`: a sparse metadata row above a header
    // is skipped.
    let plan = |direct: bool, sparse_pre: bool| {
        let mut s = 0;
        while s + 1 < rows.len()
            && s < MAX_TITLE_ROWS
            && title_row(sheet, &rows[s], region.rect, &rows[s + 1..], direct)
            && !(s >= 1 && is_label_over_header(sheet, rows, s, rows.len(), region.rect))
        {
            s += 1;
        }
        let mut e = rows.len();
        while e > s + 1 && (is_note_row(sheet, &rows[e - 1]) || is_trailing_sentence(sheet, &rows[e - 1])) {
            e -= 1;
        }
        // A block of single text lines below a blank row at the end of the region (the footnotes,
        // sources and contacts under a statistics-database export's one or two data rows, B53):
        // notes, not body rows of the table above them.
        let mut k = e;
        while k > s + 1 && rows[k - 1].n() == 1 {
            k -= 1;
        }
        // The lines start at the region's left edge, one of them reads as a sentence or source line,
        // and the row above the gap is a data row with numbers.
        let lines = &rows[k..e];
        if e - k >= NOTE_BLOCK_MIN_ROWS
            && rows[k].row > rows[k - 1].bottom() + 1
            && rows[k - 1].n_measure() >= 2
            && lines.iter().all(|r| r.left() == region.rect.left)
            && lines.iter().any(|r| single_text(r).is_some_and(|x| text(sheet, x).trim().chars().count() >= NOTE_BLOCK_LONG))
        {
            e = k;
        }
        let numbers = out.column_numbers;
        let h0 = header_depth(sheet, rows, s, e, region.rect, numbers);
        let hsig = if h0 > 0 { signature(sheet, &rows[s + h0 - 1]) } else { Vec::new() };
        let cuts = super::changepoint::boundaries(sheet, rows, s + h0, e, &hsig);
        let mut segs = Vec::new();
        let mut ps = s;
        for pe in cuts.iter().copied().chain(std::iter::once(e)) {
            if ps == s {
                segs.extend(segment(sheet, rows, 0..s, s, pe, region.rect, false, numbers, sparse_pre));
            } else {
                // Title rows at the top of a piece belong to its table.
                let mut q = ps;
                while q + 2 < pe
                    && q - ps < 2
                    && is_title_row(sheet, &rows[q], region.rect, &rows[q + 1..])
                    && !is_note_row(sheet, &rows[q])
                {
                    q += 1;
                }
                segs.extend(segment(sheet, rows, ps..q, q, pe, region.rect, true, numbers, sparse_pre));
            }
            ps = pe;
        }
        (e, segs)
    };
    let (mut e, mut segs) = plan(false, false);
    // No table at all: try again with a lone label over the value column as its header
    // (`percent` over `Total | 30`, a one-value-column statistics table, HiTab statcan/1655;
    // `% Decrease` over one summary value beside a table, RealHiTBench society-table49).
    // Tables `segs` would emit, at most `width` columns wide.
    let any_table = |segs: &[Seg], width: u32| {
        segs.iter().any(|sg| {
            let mut sg = sg.clone();
            trim_tail(sheet, rows, &mut sg);
            qualifies(rows, &sg).is_some_and(|r| r.right - r.left < width)
        })
    };
    // Only narrow tables this way: a lone label over a wide row is a section label.
    if !TITLE_HEADER_OVER_NUMBER && !any_table(&segs, u32::MAX) {
        let (e2, segs2) = plan(true, false);
        if any_table(&segs2, 3) {
            (e, segs) = (e2, segs2);
        }
    }
    // Or past a sparse row over a much wider header (MiMoTable StockSituation4: a stock's name
    // and code over its trades).
    if !any_table(&segs, u32::MAX) {
        let (e2, segs2) = plan(false, true);
        if any_table(&segs2, u32::MAX) {
            (e, segs) = (e2, segs2);
        }
    }
    // Still none: a long headerless list of `label | number` rows (a decomposition, a list of
    // indicators; HiTab statcan/1328) is a two-column table, not a key/value block of metadata.
    if !any_table(&segs, u32::MAX) {
        for sg in &mut segs {
            let body = &rows[sg.start..sg.end];
            let pairs = body
                .iter()
                .filter(|r| {
                    let mut it = r.filled();
                    matches!((it.next(), it.next(), it.next()), (Some(a), Some(b), None)
                        if a.kind == Kind::Text && a.left == region.rect.left && b.kind == Kind::Num && !b.label_num)
                })
                .count();
            if sg.header == 0 && region.width() == 2 && pairs >= HEADERLESS_MIN_ROWS && pairs * 5 >= body.len() * 4 {
                sg.split = true;
            }
        }
    }
    if let Some(sg) = segs.first_mut() {
        label_column_header(sheet, rows, sg);
    }
    let mut loose_rows: Vec<Row> = Vec::new();
    let mut any = false;
    for seg in &segs {
        let mut seg = Seg { titles: seg.titles.clone(), ..*seg };
        // Trimmed tail rows keep the default role (Other).
        let _tail = trim_tail(sheet, rows, &mut seg);
        let seg = &seg;
        if emit_table(sheet, rows, seg, out) {
            any = true;
        } else {
            loose_rows.extend(rows[seg.titles.start..seg.end].iter().cloned());
        }
    }
    if any {
        for r in &rows[e..] {
            set(out, r, Role::Note);
        }
    } else {
        loose_rows.extend(rows[e..].iter().cloned());
    }
    if !loose_rows.is_empty() {
        loose_rows.sort_by_key(|r| r.row);
        out.loose.push(Region { rect: bounds(&loose_rows), rows: loose_rows });
    }
}

/// A headerless `label | value` table whose last title row, directly above it, is one short label
/// over the row-label column (`Reference area` over `Australia`, `· New South Wales | 681425574`,
/// the OECD data explorer's export, RealHiTBench science-table02, geography-table03): that label
/// is the header of the label column, not a title.
fn label_column_header(sheet: &Sheet, rows: &[Row], seg: &mut Seg) {
    // Only for a block that is a table already (a header must not make a list or a map one).
    if seg.header > 0
        || seg.titles.is_empty()
        || seg.titles.end != seg.start
        || seg.end <= seg.start + 1
        || qualifies(rows, seg).is_none()
    {
        return;
    }
    let body = &rows[seg.start..seg.end];
    let left = body.iter().map(|r| r.left()).min().unwrap_or(0);
    // The column under it holds the rows' text labels, and the rows have numbers beside them.
    let labels = body.iter().filter(|r| r.filled().any(|x| x.left == left && x.kind == Kind::Text)).count();
    let numbers = body.iter().filter(|r| r.n_measure() > 0).count();
    if labels * 5 < body.len() * 4 || numbers * 2 < body.len() {
        return;
    }
    let label = |k: usize| -> Option<&str> {
        let e = single_text(&rows[k])?;
        let t = text(sheet, e);
        let ok = e.left == left
            && e.right == e.left
            && is_short_label(t)
            // no subtitle (`United States, 2017`) and no `Key:` line
            && !t.contains([':', ','])
            && !t.chars().any(|c| c.is_ascii_digit())
            && !is_total_label(t)
            && !is_note_label(t)
            && !super::cells::is_unit_text(t);
        ok.then_some(t)
    };
    let indented = |t: &str| t.starts_with(|c: char| !c.is_alphanumeric());
    let first = body[0].filled().find(|x| x.left == left).map(|x| text(sheet, x)).unwrap_or("");
    let mut k = seg.start - 1;
    let Some(t) = label(k) else { return };
    // A label like the rows' own (`From Anadarko Pool` over `From Bondad Station`, an empty data
    // row) is one of them, not their header.
    let word = |s: &str| s.split_whitespace().next().unwrap_or("").to_lowercase();
    let sibling = body.iter().take(10).any(|r| {
        r.filled().find(|x| x.left == left).is_some_and(|x| word(text(sheet, x)) == word(t))
    });
    if sibling {
        return;
    }
    if rows[k].row + 1 != rows[seg.start].row {
        return;
    }
    // A label over indented labels is their group (`Australia` over `· New South Wales`); the
    // header is the label right above it.
    if indented(first) && !indented(t) {
        if k == seg.titles.start || rows[k - 1].row + 1 != rows[k].row || label(k - 1).is_none_or(indented) {
            return;
        }
        k -= 1;
    }
    seg.titles = seg.titles.start..k;
    seg.start = k;
    seg.header = 1;
}

/// Outline-style key columns: a label column of the table holds a label only on the first row of
/// each group and is blank on the rows that follow ("Management | Kevin Presto | …", then
/// " | Dana Davis | …"; "Receipts HH" / "Deliveries HH" once per block). Those labels are group
/// headers (DECO), and normalize fills them down. Checked for each of the leftmost
/// [`KEY_MAX_COLS`] columns used by the data rows, up to the first mostly numeric one.
fn mark_key_column(rows: &[Row], data_rows: &[usize], rect: Rect, out: &mut Out) {
    if data_rows.len() < KEY_MIN_ROWS {
        return;
    }
    let mut cols: Vec<u32> = data_rows.iter().flat_map(|&k| rows[k].filled().map(|e| e.left)).collect();
    cols.sort_unstable();
    cols.dedup();
    // A dense label column (text on nearly every data row): the item names a key column groups.
    let dense_text = |c: u32| {
        let n = data_rows.iter().filter(|&&k| rows[k].filled().any(|e| e.left == c && e.kind == Kind::Text)).count();
        n as f32 >= KEY_DENSE_FRAC * data_rows.len() as f32
    };
    for (pos, &col) in cols.iter().take(KEY_MAX_COLS).enumerate() {
        if col > rect.left + KEY_MAX_COLS as u32 {
            break;
        }
        // Past the first column, a key column needs the item names to its right.
        if pos > 0 && !cols.iter().skip(pos + 1).take(2).any(|&c| dense_text(c)) {
            continue;
        }
        let key = |k: usize| rows[k].filled().find(|e| e.left == col);
        let (mut filled, mut labels, mut starts, mut numbers) = (0usize, 0usize, 0usize, 0usize);
        for (j, &k) in data_rows.iter().enumerate() {
            let Some(e) = key(k) else { continue };
            filled += 1;
            if e.kind == Kind::Text || e.date {
                labels += 1;
            } else if e.kind == Kind::Num {
                numbers += 1;
            }
            // Starts a run: other cells on its row, and the next data row leaves the key blank.
            let others = rows[k].filled().any(|x| x.left != col);
            if others && data_rows.get(j + 1).is_some_and(|&n| key(n).is_none() && rows[n].n() > 0) {
                starts += 1;
            }
        }
        // A measure column ends the label columns.
        if numbers * 2 > filled {
            break;
        }
        // Past the first column, every key opens a block: on the first data row, or after a
        // row that is not data (a total, a blank row).
        let opens_block = |j: usize| {
            let k = data_rows[j];
            j == 0 || data_rows[j - 1] + 1 != k || rows[k - 1].bottom() + 1 < rows[k].row
        };
        if pos > 0 && (0..data_rows.len()).any(|j| key(data_rows[j]).is_some() && !opens_block(j)) {
            continue;
        }
        let n = data_rows.len();
        // Past the first column, blocks of a few rows at least (not a sparse column).
        let max_fill = if pos == 0 { KEY_MAX_FILL } else { KEY_MAX_FILL_INNER };
        if filled < 2
            || filled as f32 > max_fill * n as f32
            || (labels as f32) < KEY_MIN_LABEL_FRAC * filled as f32
            || (starts as f32) < KEY_MIN_START_FRAC * filled as f32
        {
            continue;
        }
        for &k in data_rows {
            if let Some(e) = key(k)
                && rows[k].filled().any(|x| x.left != col)
                && let Some(i) = e.cell
            {
                out.roles[i] = Role::GroupHeader;
            }
        }
    }
}
