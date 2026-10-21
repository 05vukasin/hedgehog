//! Per-sheet extraction: layout tables → flat header + typed columns; totals → `TotalItem`s;
//! every other filled cell → `NoteItem`s. Each source cell ends up in exactly one place
//! (table, Totals or Notes), which `CellCounts` reports.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use super::headers::{clean_header, is_nav_link, not_additive};
use super::typing::{Slot, display, fit, type_column};
use super::{
    CellCounts, ColRole, Column, Dtype, Normalized, NoteItem, Options, Out, RowType, SheetInfo, T, Table, TotalItem,
    Transforms,
};
use crate::analysis::{DetectedTable, Role, SheetLayout};
use crate::grid::Grid;
use crate::model::{Rect, Sheet, Value, a1};
use crate::totals::{
    self, CheckReason, Direction, Evidence, KeywordLevel, NonSumKind, TotalCandidate, TotalCheck, TotalLevel, TotalOrigin,
};
use crate::values::{ColumnHints, NativeNumbers, infer_column_with, norm_key, sheet_column_hints_with};

const OWN_NONE: u8 = 0;
const OWN_TABLE: u8 = 1;
const OWN_TOTALS: u8 = 2;
const OWN_NOTES: u8 = 3;

/// Max blank rows between a loose title (note) and the table below (above) it.
const ASSOC_MAX_GAP: u32 = 6;
/// Data rows a table needs before it takes row labels from a neighbour ([`copy_row_labels`]).
const COPY_LABELS_MIN_ROWS: usize = 4;
/// Share of its rows a table's label column must name to lend its labels.
const COPY_LABELS_MIN_FILL: f32 = 0.8;
/// Excel's limit for a table column name.
const MAX_HEADER_LEN: usize = 255;
pub(super) const SRC_ROW: &str = "_src_row";
pub(super) const ROW_TYPE: &str = "_row_type";

fn col_letter(col: u32) -> String {
    let s = a1(0, col);
    s.trim_end_matches('1').to_string()
}

fn range_a1(r: Rect) -> String {
    if r.top == r.bottom && r.left == r.right {
        a1(r.top, r.left)
    } else {
        format!("{}:{}", a1(r.top, r.left), a1(r.bottom, r.right))
    }
}

fn level_str(l: TotalLevel) -> &'static str {
    match l {
        TotalLevel::Total => "total",
        TotalLevel::Subtotal => "subtotal",
        TotalLevel::GrandTotal => "grand_total",
    }
}

/// `CheckReason::DoubleCounted` → `double_counted`.
fn snake(debug: impl std::fmt::Debug) -> String {
    let s = format!("{debug:?}");
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// What a body row of a table is, from its cells' layout roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Data,
    Total,
    Group,
    RepeatedHeader,
    Note,
    Title,
    /// An "of which" or average row ([`totals::NonSumRow`]).
    NonSum(NonSumKind),
}

/// What the totals know about a row better than its cells' layout roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowOverride {
    /// A total row: its total word and its numbers agree, its numbers add up without a total word,
    /// or it is a known aggregate (World Bank region codes).
    Total,
    /// A record whose total word is part of its text or a name ("Total knee replacement implant",
    /// "Somme"), and whose numbers do not add up.
    Data,
    NonSum(NonSumKind),
}

/// Row overrides from the sheet's total detection (see [`RowOverride`]). A total word confirms a
/// total row only as the row's first text (its label): in long-format data a total category beside
/// other keys ("2020 | Wien | Insgesamt") stays in the data, marked on Totals as kept.
///
/// Row → overrides, each with the columns it was found in (a table gets an override only when it
/// holds one of them: a total row of one block does not make the row of the block beside it a
/// total); no columns: the whole row.
type Overrides = HashMap<u32, Vec<(RowOverride, Vec<u32>)>>;

fn row_overrides(grid: &Grid, det: &totals::Detection) -> Overrides {
    let mut out: Overrides = HashMap::new();
    let mut add = |row: u32, o: RowOverride, cols: Vec<u32>| out.entry(row).or_default().push((o, cols));
    for &r in &det.incidental_rows {
        add(r, RowOverride::Data, Vec::new());
    }
    let first_text = |row: u32| {
        grid.row_cells(row).iter().map(|&i| &grid.sheet.cells[i as usize]).find_map(|c| match &c.value {
            Some(Value::Text(t)) if !t.trim().is_empty() && totals::numeric_text(t).is_none() => Some(c.col),
            _ => None,
        })
    };
    for c in &det.candidates {
        let labelled = c.evidence.iter().any(|e| matches!(e, Evidence::Keyword(_)))
            && c.label.is_some_and(|(_, lc)| first_text(c.row) == Some(lc));
        let confirmed = c.direction == Direction::Vertical
            && match c.origin {
                TotalOrigin::Typed => {
                    c.evidence.contains(&Evidence::ArithmeticMatch)
                        && (labelled || c.evidence.contains(&Evidence::Unlabelled))
                }
                // A SUM over the rows below (the layout only knows sums over the rows above).
                TotalOrigin::Formula => labelled || c.evidence.contains(&Evidence::SourceBelow),
            };
        if confirmed {
            let mut cols = vec![c.col];
            cols.extend(c.label.map(|l| l.1));
            add(c.row, RowOverride::Total, cols);
        }
    }
    for &(r, lc) in &det.aggregates {
        add(r, RowOverride::Total, vec![lc]);
    }
    for n in &det.nonsum {
        let mut cols: Vec<u32> = n.cells.iter().map(|c| c.col).collect();
        cols.push(n.label_col);
        add(n.row, RowOverride::NonSum(n.kind), cols);
    }
    out
}

fn row_kind(roles: impl Iterator<Item = Role>) -> RowKind {
    let (mut total, mut group, mut header, mut note, mut title, mut data) = (false, false, false, false, false, false);
    for r in roles {
        match r {
            Role::Total => total = true,
            Role::GroupHeader => group = true,
            Role::Header => header = true,
            Role::Note => note = true,
            Role::Title => title = true,
            Role::Data | Role::Other => data = true,
        }
    }
    if total {
        RowKind::Total
    } else if group && !data {
        // A group label alone on its row. With data beside it, the label is an outline key
        // (first row of its group) and the row is data; the key is filled down below.
        RowKind::Group
    } else if header {
        RowKind::RepeatedHeader
    } else if data {
        RowKind::Data
    } else if note {
        RowKind::Note
    } else if title {
        RowKind::Title
    } else {
        RowKind::Data
    }
}

/// Columns whose only texts are one label on the first data row of each block of rows between
/// total rows ([`build_table`]'s `block_of`; `u32::MAX` for total rows): at least two blocks of
/// two or more rows, every block labelled on its first row and blank below it.
fn block_label_columns(slots: &[Vec<Slot>], block_of: &[u32], skip: &HashSet<usize>) -> Vec<usize> {
    let mut blocks: Vec<(usize, usize)> = Vec::new(); // (first row, rows)
    for (j, &b) in block_of.iter().enumerate() {
        if b == u32::MAX {
            continue;
        }
        match blocks.last_mut() {
            Some((first, n)) if block_of[*first] == b => *n += 1,
            _ => blocks.push((j, 1)),
        }
    }
    if blocks.iter().filter(|(_, n)| *n >= 2).count() < 2 {
        return Vec::new();
    }
    let text = |s: &Slot| matches!(s, Slot::Val(Value::Text(t)) if !t.trim().is_empty());
    (0..slots.len())
        .filter(|k| !skip.contains(k))
        .filter(|&k| {
            let col = &slots[k];
            blocks.iter().all(|&(first, n)| {
                text(&col[first])
                    && (first + 1..first + n).all(|j| block_of[j] == u32::MAX || matches!(col[j], Slot::Empty))
            })
        })
        .collect()
}

/// Header + group column name for "Region: North"-style labels, if all labels share the prefix.
fn group_prefix(labels: &[&str]) -> Option<String> {
    let mut prefix: Option<(String, String)> = None;
    for l in labels {
        let (p, rest) = l.split_once(':')?;
        let (p, rest) = (p.trim(), rest.trim());
        if p.is_empty() || rest.is_empty() || p.chars().count() > 40 {
            return None;
        }
        let key = norm_key(p);
        match &prefix {
            None => prefix = Some((p.to_string(), key)),
            Some((_, k)) if *k == key => {}
            Some(_) => return None,
        }
    }
    prefix.map(|p| p.0)
}

/// Lowercase ASCII `[a-z0-9_]` slug (Cyrillic transliterated, diacritics folded).
pub(super) fn slug(s: &str, max: usize) -> String {
    let key = norm_key(s);
    let mut out = String::new();
    for ch in key.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
        if out.len() >= max {
            break;
        }
    }
    out.truncate(max);
    out.trim_matches('_').to_string()
}

/// Flat column name from its header parts: each part cleaned from the raw cell text (footnote
/// marks, line-break hyphenation, whitespace; see [`clean_header`]), repeats dropped, joined with
/// ` / `.
fn clean_name(grid: &Grid, parts: &[(String, (u32, u32))]) -> String {
    let mut out: Vec<String> = Vec::with_capacity(parts.len());
    for (text, (r, c)) in parts {
        let cleaned = match grid.cell(*r, *c).and_then(|c| c.value.as_ref()) {
            Some(Value::Text(raw)) => clean_header(raw),
            _ => text.clone(),
        };
        if !cleaned.is_empty() && out.last() != Some(&cleaned) {
            out.push(cleaned);
        }
    }
    out.join(" / ")
}

/// Makes `name` unique (case-insensitive, like Excel) by appending ` (2)`, ` (3)`…
fn unique(name: String, seen: &mut HashSet<String>, tr: &mut Transforms) -> String {
    if seen.insert(name.to_lowercase()) {
        return name;
    }
    tr.add(T::DuplicateHeadersRenamed, 1);
    let mut k = 2;
    loop {
        let cand = format!("{name} ({k})");
        if seen.insert(cand.to_lowercase()) {
            return cand;
        }
        k += 1;
    }
}

struct Built {
    table: Table,
    /// Source rows that stayed in the table's data.
    kept: HashSet<u32>,
    /// Removed total rows: (row, row type, cell indices inside the table).
    removed_totals: Vec<(u32, RowType, Vec<usize>)>,
    notes: Vec<NoteItem>,
}

type Head = Option<(String, (u32, u32))>;

/// A total row as detected: its level and the first row of its source (the topmost range start
/// among the row's vertical totals), when known.
#[derive(Debug, Clone, Copy)]
struct RowTotal {
    level: TotalLevel,
    top: Option<u32>,
}

#[allow(clippy::too_many_arguments)]
fn build_table(
    grid: &Grid,
    lay: &SheetLayout,
    t: &DetectedTable,
    n: usize,
    sheet_hints: &OnceCell<HashMap<u32, ColumnHints>>,
    row_total: &HashMap<u32, RowTotal>,
    overrides: &Overrides,
    opts: &Options,
    owner: &mut [u8],
) -> Built {
    let sheet = grid.sheet;
    let cells = &sheet.cells;
    let r = t.rect;
    let ncols = (r.right - r.left + 1) as usize;
    let mut tr = Transforms::default();
    let mut notes: Vec<NoteItem> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let note = |i: usize, kind: &'static str| {
        let c = &cells[i];
        NoteItem {
            sheet: sheet.name.clone(),
            row: c.row,
            col: c.col,
            kind,
            table: Some(n),
            text: c.value.as_ref().map(display).unwrap_or_default(),
            formula: c.formula.as_ref().map(|f| format!("={f}")),
        }
    };

    // Cells of the table, grouped by row (sheet.cells is row-major).
    let lo = cells.partition_point(|c| c.row < r.top);
    let hi = cells.partition_point(|c| c.row <= r.bottom);
    let mut rows: Vec<(u32, Vec<usize>)> = Vec::new();
    for (i, c) in cells.iter().enumerate().take(hi).skip(lo) {
        if c.col < r.left || c.col > r.right {
            continue;
        }
        match rows.last_mut() {
            Some((row, v)) if *row == c.row => v.push(i),
            _ => rows.push((c.row, vec![i])),
        }
    }
    let htop = t.header_top.clamp(r.top, r.bottom);
    let body_top = (htop + t.header_rows).min(r.bottom + 1);
    let nh = (body_top - htop) as usize;

    // Title rows inside the rect (above the header).
    let mut title: Option<(String, (u32, u32))> = None;
    let mut title_rows = 0;
    for (_, idxs) in rows.iter().filter(|(row, _)| *row < htop) {
        title_rows += 1;
        for &i in idxs {
            owner[i] = OWN_NOTES;
            let mut it = note(i, "title");
            if is_nav_link(&it.text) {
                // "zur Inhaltsübersicht": a link back to the contents, never the title.
                it.kind = "other";
            } else if title.is_none() && !it.text.is_empty() {
                title = Some((it.text.clone(), (it.row, it.col)));
            }
            notes.push(it);
        }
    }
    tr.add(T::TitleRowsMoved, title_rows);

    // --- Header ----------------------------------------------------------------------------
    let mut head: Vec<Vec<Head>> = vec![vec![None; ncols]; nh];
    for (row, idxs) in rows.iter().filter(|(row, _)| *row >= htop && *row < body_top) {
        for &i in idxs {
            owner[i] = OWN_TABLE;
            let c = &cells[i];
            if let Some(v) = &c.value {
                let s = display(v);
                if !s.is_empty() {
                    head[(row - htop) as usize][(c.col - r.left) as usize] = Some((s, (c.row, c.col)));
                }
            }
        }
    }
    // Merged header cells: every covered position takes the top-left text.
    let mut merged_in_header: HashSet<(usize, usize)> = HashSet::new();
    if nh > 0 {
        for m in &sheet.merges {
            if m.bottom < htop || m.top >= body_top || m.right < r.left || m.left > r.right || m.top < htop {
                continue;
            }
            let Some(text) =
                grid.cell(m.top, m.left).and_then(|c| c.value.as_ref()).map(display).filter(|s| !s.is_empty())
            else {
                continue;
            };
            for rr in m.top..=m.bottom.min(body_top - 1) {
                for cc in m.left.max(r.left)..=m.right.min(r.right) {
                    let (hr, hc) = ((rr - htop) as usize, (cc - r.left) as usize);
                    merged_in_header.insert((hr, hc));
                    if head[hr][hc].is_none() {
                        head[hr][hc] = Some((text.clone(), (m.top, m.left)));
                        tr.add(T::MergedCellsFilled, 1);
                    }
                }
            }
        }
    }
    // Blank cells right of a parent label (not merged) take it, while columns below are labelled
    // and the parent's own parent does not change.
    for hr in 0..nh.saturating_sub(1) {
        let mut carry: Option<(Head, usize)> = None;
        for hc in 0..ncols {
            let below = (hr + 1..nh).any(|k| head[k][hc].is_some());
            if head[hr][hc].is_some() {
                carry = (below && !merged_in_header.contains(&(hr, hc))).then(|| (head[hr][hc].clone(), hc));
                continue;
            }
            let Some((val, from)) = &carry else { continue };
            let same_parent = hr == 0 || head[hr - 1][hc] == head[hr - 1][*from];
            if below && same_parent {
                head[hr][hc] = val.clone();
                tr.add(T::HeaderCellsForwardFilled, 1);
            } else {
                carry = None;
            }
        }
    }

    // --- Body --------------------------------------------------------------------------------
    // Vertically merged body cells (e.g. a label over 3 rows): fill down in the label's column.
    let mut fill: HashMap<(u32, u32), &Value> = HashMap::new();
    for m in &sheet.merges {
        if m.top < body_top || m.top > r.bottom || m.left < r.left || m.left > r.right || m.bottom == m.top {
            continue;
        }
        if let Some(v) = grid.cell(m.top, m.left).and_then(|c| c.value.as_ref()) {
            for rr in m.top + 1..=m.bottom.min(r.bottom) {
                fill.insert((rr, m.left), v);
            }
        }
    }
    let mut slots: Vec<Vec<Slot>> = vec![Vec::new(); ncols];
    let mut src_rows: Vec<u32> = Vec::new();
    let mut row_types: Vec<RowType> = Vec::new();
    let mut row_group: Vec<Option<usize>> = Vec::new();
    let mut paths: Vec<Vec<String>> = Vec::new();
    // The current group path and the rows of its group headers; `run` collects consecutive
    // group-header rows (label, row) until the next data row places them in the path.
    let mut path: Vec<String> = Vec::new();
    let mut path_rows: Vec<u32> = Vec::new();
    let mut run: Vec<(String, u32)> = Vec::new();
    let mut cur_path: Option<usize> = None;
    // Column of the group-header rows' labels; a group-header cell there on a data row is a group
    // label written on the group's first data row (it replaces the innermost group).
    let mut group_label_col: Option<u32> = None;
    let mut group_labels: Vec<String> = Vec::new();
    let mut removed_totals = Vec::new();
    let mut body_rows_seen = 0u32;
    let mut key_cols: HashSet<usize> = HashSet::new();
    // Block of each output row: the rows between two total rows (removed or kept).
    let mut block_of: Vec<u32> = Vec::new();
    let mut block = 0u32;
    for (row, idxs) in rows.iter().filter(|(row, _)| *row >= body_top) {
        body_rows_seen += 1;
        let mut kind = row_kind(idxs.iter().map(|&i| lay.roles[i]));
        if matches!(kind, RowKind::Data | RowKind::Total) {
            // The last override that applies to this table wins ("of which" over a total).
            let here = overrides.get(row).and_then(|list| {
                list.iter().rev().find(|(_, cols)| cols.is_empty() || cols.iter().any(|c| (r.left..=r.right).contains(c)))
            });
            kind = match here.map(|x| x.0) {
                Some(RowOverride::Total) => RowKind::Total,
                Some(RowOverride::Data) => RowKind::Data,
                Some(RowOverride::NonSum(k)) => RowKind::NonSum(k),
                None => kind,
            };
        }
        match kind {
            RowKind::Group => {
                let label = idxs
                    .iter()
                    .filter_map(|&i| cells[i].value.as_ref())
                    .map(display)
                    .find(|s| !s.is_empty())
                    .unwrap_or_default();
                for &i in idxs {
                    owner[i] = OWN_TABLE;
                }
                if let Some(&i) = idxs.iter().find(|&&i| cells[i].value.as_ref().is_some_and(|v| !display(v).is_empty())) {
                    group_label_col = Some(cells[i].col);
                }
                tr.add(T::GroupHeaderRowsToColumn, 1);
                group_labels.push(label.clone());
                run.push((label, *row));
                continue;
            }
            RowKind::RepeatedHeader => {
                tr.add(T::RepeatedHeaderRowsRemoved, 1);
                for &i in idxs {
                    owner[i] = OWN_NOTES;
                    notes.push(note(i, "repeated_header"));
                }
                continue;
            }
            RowKind::Note | RowKind::Title => {
                tr.add(T::NoteRowsMoved, 1);
                for &i in idxs {
                    owner[i] = OWN_NOTES;
                    notes.push(note(i, if kind == RowKind::Note { "note" } else { "title" }));
                }
                continue;
            }
            RowKind::Total | RowKind::Data | RowKind::NonSum(_) => {}
        }
        let level = if kind == RowKind::Total {
            row_total.get(row).map(|t| t.level).or_else(|| {
                idxs.iter()
                    .filter_map(|&i| match &cells[i].value {
                        Some(Value::Text(s)) => totals::total_keyword(s),
                        _ => None,
                    })
                    .next()
                    .map(|k| match k {
                        KeywordLevel::Subtotal => TotalLevel::Subtotal,
                        KeywordLevel::GrandTotal => TotalLevel::GrandTotal,
                        KeywordLevel::Total => TotalLevel::Total,
                    })
            })
        } else {
            None
        };
        let rt = match (kind, level) {
            (RowKind::Total, Some(TotalLevel::Subtotal)) => RowType::Subtotal,
            (RowKind::Total, _) => RowType::Total,
            (RowKind::NonSum(NonSumKind::Average), _) => RowType::Average,
            (RowKind::NonSum(NonSumKind::OfWhich), _) => RowType::OfWhich,
            _ => RowType::Data,
        };
        if rt != RowType::Data {
            block += 1;
        }
        if rt != RowType::Data && !opts.keep_totals {
            tr.add(T::TotalRowsRemoved, 1);
            removed_totals.push((*row, rt, idxs.clone()));
            continue;
        }
        if rt != RowType::Data {
            tr.add(T::TotalRowsKept, 1);
        }
        let inline_label = idxs.iter().copied().find(|&i| {
            rt == RowType::Data
                && run.is_empty()
                && lay.roles[i] == Role::GroupHeader
                && Some(cells[i].col) == group_label_col
                && cells[i].value.as_ref().is_some_and(|v| !display(v).is_empty())
        });
        if let Some(i) = inline_label {
            let label = cells[i].value.as_ref().map(display).unwrap_or_default();
            tr.add(T::GroupHeaderRowsToColumn, 1);
            group_labels.push(label.clone());
            run.push((label, *row));
        }
        if !run.is_empty() {
            let keep = path.len().saturating_sub(run.len());
            path.truncate(keep);
            path_rows.truncate(keep);
            for (label, row) in run.drain(..) {
                path.push(label);
                path_rows.push(row);
            }
            paths.push(path.clone());
            cur_path = Some(paths.len() - 1);
        }
        // A kept total row belongs only to the groups that contain its whole source: a subtotal
        // to its group, a grand total (or a total over several groups) to none of them. A source
        // range reaching into the header (a year header read as a number, B17) starts at the body.
        let depth = match (level, row_total.get(row).and_then(|t| t.top)) {
            _ if matches!(rt, RowType::Data | RowType::Average | RowType::OfWhich) => path.len(),
            (Some(TotalLevel::GrandTotal), _) => 0,
            (_, Some(top)) => path_rows.iter().take_while(|&&h| h <= top.max(body_top)).count(),
            _ => path.len(),
        };
        row_group.push(match depth {
            0 => None,
            d if d < path.len() => {
                paths.push(path[..d].to_vec());
                Some(paths.len() - 1)
            }
            _ => cur_path,
        });
        let mut row_slots: Vec<Slot> = vec![Slot::Empty; ncols];
        for &i in idxs {
            owner[i] = OWN_TABLE;
            if Some(i) == inline_label {
                continue;
            }
            let c = &cells[i];
            let k = (c.col - r.left) as usize;
            if lay.roles[i] == Role::GroupHeader {
                key_cols.insert(k);
            }
            row_slots[k] = match &c.value {
                Some(v) => {
                    tr.add(T::FormulasReplacedByValues, c.formula.is_some() as usize);
                    Slot::Val(v)
                }
                None if c.formula.is_some() => Slot::NoCache,
                None => Slot::Empty,
            };
        }
        for (k, s) in row_slots.iter_mut().enumerate() {
            if matches!(s, Slot::Empty)
                && let Some(v) = fill.get(&(*row, r.left + k as u32))
            {
                *s = Slot::Val(v);
                tr.add(T::MergedCellsFilled, 1);
            }
        }
        for (k, s) in row_slots.into_iter().enumerate() {
            slots[k].push(s);
        }
        src_rows.push(*row);
        row_types.push(rt);
        block_of.push(if rt == RowType::Data { block } else { u32::MAX });
        if rt != RowType::Data {
            block += 1;
        }
    }
    if r.bottom + 1 > body_top {
        tr.add(T::BlankRowsRemoved, (r.bottom + 1 - body_top).saturating_sub(body_rows_seen) as usize);
    }

    // Group labels written once per block of rows between subtotals ("Belgium" on the first row
    // of the block its "Razem Belgium" closes, or under its "Total Belgium"): filled down the block.
    for k in block_label_columns(&slots, &block_of, &key_cols) {
        let mut cur: (u32, Option<&Value>) = (u32::MAX, None);
        for (j, s) in slots[k].iter_mut().enumerate() {
            if block_of[j] == u32::MAX {
                continue;
            }
            match s {
                Slot::Val(v) => cur = (block_of[j], Some(*v)),
                Slot::Empty if cur.0 == block_of[j] => {
                    if let Some(v) = cur.1 {
                        *s = Slot::Val(v);
                        tr.add(T::GroupKeysFilledDown, 1);
                    }
                }
                _ => {}
            }
        }
    }

    // Outline key columns: each label covers the data rows below it until the next label.
    for &k in &key_cols {
        let mut last: Option<&Value> = None;
        for (j, s) in slots[k].iter_mut().enumerate() {
            if row_types[j] != RowType::Data {
                continue;
            }
            match s {
                Slot::Val(v) => last = Some(*v),
                Slot::Empty => {
                    if let Some(v) = last {
                        *s = Slot::Val(v);
                        tr.add(T::GroupKeysFilledDown, 1);
                    }
                }
                Slot::NoCache => {}
            }
        }
    }
    // Total rows take the keys of the block they close (B17): an empty text cell left of the
    // total's label gets the value every data row of the block (since the previous total row)
    // shares there, so `WHERE key = '…' AND _row_type = 'total'` finds the block's total.
    let mut block_start = 0;
    for j in 0..row_types.len() {
        if row_types[j] == RowType::Data {
            continue;
        }
        let block = block_start..j;
        block_start = j + 1;
        if block.len() < 2 {
            continue;
        }
        let is_text = |s: &Slot| matches!(s, Slot::Val(Value::Text(t)) if !t.trim().is_empty());
        let Some(label) = (0..ncols).find(|&k| is_text(&slots[k][j])) else { continue };
        for col in slots.iter_mut().take(label) {
            if !matches!(col[j], Slot::Empty) {
                continue;
            }
            let Slot::Val(v @ Value::Text(first)) = col[block.start] else { continue };
            if first.trim().is_empty() {
                continue;
            }
            let shared = block.clone().all(|b| matches!(col[b], Slot::Val(x) if x == v));
            if shared {
                col[j] = Slot::Val(v);
                tr.add(T::GroupKeysFilledDown, 1);
            }
        }
    }

    // --- Columns -----------------------------------------------------------------------------
    let mut data_cols: Vec<Column> = Vec::new();
    let mut col_warnings = Vec::new();
    for k in 0..ncols {
        let col = r.left + k as u32;
        let mut parts: Vec<(String, (u32, u32))> = Vec::new();
        for h in head.iter().flat_map(|row| &row[k]) {
            if parts.last().is_none_or(|p| p.0 != h.0) {
                parts.push(h.clone());
            }
        }
        let raw_name = parts.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join(" / ");
        let mut name = clean_name(grid, &parts);
        if name != raw_name {
            tr.add(T::HeadersCleaned, 1);
        }
        if name.chars().count() > MAX_HEADER_LEN {
            name = name.chars().take(MAX_HEADER_LEN).collect();
        }
        let label = if name.is_empty() { format!("col_{}", col_letter(col)) } else { name.clone() };
        let first = opts.locale.map(|l| l.hints()).unwrap_or_default();
        let mut refine = |sample: &[&str], ambiguous: bool, native: &NativeNumbers| -> ColumnHints {
            let inferred = infer_column_with(sample.iter().copied(), native);
            if let Some(l) = opts.locale {
                return ColumnHints { currency_context: inferred.currency_context, ..l.hints() };
            }
            let mut h = inferred;
            // The sheet-wide vote is only worth its cost when a guess is actually needed.
            if ambiguous && (h.decimal.is_none() || h.date_order.is_none()) {
                let sh = sheet_hints
                    .get_or_init(|| sheet_column_hints_with(grid, None))
                    .get(&col)
                    .copied()
                    .unwrap_or_default();
                h.decimal = h.decimal.or(sh.decimal);
                h.date_order = h.date_order.or(sh.date_order);
                h.currency_context |= sh.currency_context;
            }
            h
        };
        let typed = type_column(&label, col, &slots[k], &src_rows, first, &mut refine, &mut tr);
        if parts.is_empty() && typed.dtype == Dtype::Empty {
            tr.add(T::EmptyColumnsRemoved, 1);
            continue;
        }
        if parts.is_empty() {
            tr.add(T::MissingHeadersNamed, 1);
        }
        if parts.len() > 1 {
            tr.add(T::HeadersFlattened, 1);
        }
        col_warnings.extend(typed.warnings);
        data_cols.push(Column {
            name: label,
            role: ColRole::Data,
            source_col: Some(col),
            original_header: parts,
            dtype: typed.dtype,
            unit: typed.unit,
            currency: typed.currency,
            decimal: typed.decimal,
            date_order: typed.date_order,
            ambiguous: typed.ambiguous,
            footnote_marks: typed.footnote_marks,
            values: typed.values,
        });
    }

    // Group column from group-header rows.
    let mut group_col: Option<Column> = None;
    if !group_labels.is_empty() {
        let refs: Vec<&str> = group_labels.iter().map(String::as_str).collect();
        let prefix = group_prefix(&refs);
        let strip = |l: &str| -> String {
            match &prefix {
                Some(_) => l.split_once(':').map(|(_, rest)| rest.trim().to_string()).unwrap_or_else(|| l.to_string()),
                None => l.to_string(),
            }
        };
        let joined: Vec<String> =
            paths.iter().map(|p| p.iter().map(|l| strip(l)).collect::<Vec<_>>().join(" / ")).collect();
        let values: Vec<Out> =
            row_group.iter().map(|g| g.map_or(Out::Null, |i| Out::Text(fit(joined[i].clone(), &mut tr)))).collect();
        group_col = Some(Column {
            name: prefix.unwrap_or_else(|| "group".to_string()),
            role: ColRole::Group,
            source_col: None,
            original_header: Vec::new(),
            dtype: if values.iter().all(|v| *v == Out::Null) { Dtype::Empty } else { Dtype::Text },
            unit: None,
            currency: None,
            decimal: None,
            date_order: None,
            ambiguous: 0,
            footnote_marks: Vec::new(),
            values,
        });
    }

    // Unique names: reserved columns claim theirs first.
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(SRC_ROW.to_string());
    if opts.keep_totals {
        seen.insert(ROW_TYPE.to_string());
    }
    let mut columns: Vec<Column> = Vec::new();
    for mut c in group_col.into_iter().chain(data_cols) {
        c.name = unique(std::mem::take(&mut c.name), &mut seen, &mut tr);
        columns.push(c);
    }
    let plain = |name: &str, role: ColRole, dtype: Dtype, values: Vec<Out>| Column {
        name: name.to_string(),
        role,
        source_col: None,
        original_header: Vec::new(),
        dtype,
        unit: None,
        currency: None,
        decimal: None,
        date_order: None,
        ambiguous: 0,
        footnote_marks: Vec::new(),
        values,
    };
    columns.push(plain(
        SRC_ROW,
        ColRole::SrcRow,
        Dtype::Integer,
        src_rows.iter().map(|&r| Out::Num(r as f64 + 1.0)).collect(),
    ));
    if opts.keep_totals {
        let values = row_types.iter().map(|t| Out::Text(t.as_str().to_string())).collect();
        columns.push(plain(ROW_TYPE, ColRole::RowType, Dtype::Text, values));
    }

    if t.header_rows == 0 {
        warnings.push("no header row detected; columns are named col_<letter> after their source column".to_string());
    }
    if src_rows.is_empty() {
        warnings.push("no data rows".to_string());
    }
    if t.confidence < 0.5 {
        warnings.push(format!("low table-detection confidence ({:.2})", t.confidence));
    }
    warnings.extend(col_warnings);

    let kept: HashSet<u32> = src_rows.iter().copied().collect();
    Built {
        table: Table {
            n,
            sheet: String::new(),
            source_sheet: sheet.name.clone(),
            rect: r,
            header_top: htop,
            header_rows: t.header_rows,
            title,
            notes: Vec::new(),
            confidence: t.confidence,
            columns,
            src_rows,
            row_types,
            transforms: tr,
            warnings,
        },
        kept,
        removed_totals,
        notes,
    }
}

/// Nearest table below (title) or above (note) a loose cell, within `ASSOC_MAX_GAP` blank rows
/// and overlapping its columns (with a little slack on the left).
fn associate(tables: &[Table], row: u32, col: u32, title: bool) -> Option<usize> {
    let mut best: Option<(u32, usize)> = None;
    for (k, t) in tables.iter().enumerate() {
        let r = t.rect;
        if col > r.right || col + 2 < r.left {
            continue;
        }
        let dist = if title {
            if r.top <= row {
                continue;
            }
            r.top - row - 1
        } else {
            if row <= r.bottom {
                continue;
            }
            row - r.bottom - 1
        };
        if dist <= ASSOC_MAX_GAP && best.is_none_or(|b| dist < b.0) {
            best = Some((dist, k));
        }
    }
    best.map(|b| b.1)
}

/// A table's body rows as (source row, is a total row), top to bottom: its data and kept total
/// rows plus the total rows taken out of it.
fn row_layout(t: &Table, removed: &[(u32, RowType, Vec<usize>)]) -> Vec<(u32, bool)> {
    let mut v: Vec<(u32, bool)> =
        t.src_rows.iter().zip(&t.row_types).map(|(&r, &rt)| (r, rt != RowType::Data)).collect();
    v.extend(removed.iter().map(|(r, _, _)| (*r, true)));
    v.sort_unstable();
    v
}

/// The leading label columns of a table: its group column and the text columns before its first
/// other column ("Henry Hub | Receipts HH | ACADIAN | 200000 | …" → the first three). Empty unless
/// one of them names nearly every row (a group column alone only names blocks of rows).
fn label_columns(t: &Table) -> Vec<usize> {
    let n: Vec<usize> = t
        .columns
        .iter()
        .take_while(|c| c.role == ColRole::Group || (c.role == ColRole::Data && c.dtype == Dtype::Text))
        .enumerate()
        .map(|(i, _)| i)
        .collect();
    let rows = t.src_rows.len();
    let names_rows = n.iter().any(|&i| {
        let c = &t.columns[i];
        let filled = c.values.iter().filter(|v| !matches!(v, Out::Null)).count();
        c.role == ColRole::Data && filled as f32 >= COPY_LABELS_MIN_FILL * rows as f32
    });
    if names_rows { n } else { Vec::new() }
}

/// Nothing but measures: no group, text, date or bool column, and no key-like whole-number column
/// (distinct values in order: years, ids, line numbers), so no row says what it is.
fn has_no_labels(t: &Table) -> bool {
    t.columns.iter().all(|c| match c.role {
        ColRole::Data if c.dtype == Dtype::Integer => !is_sequence(&c.values),
        ColRole::Data => matches!(c.dtype, Dtype::Number | Dtype::Percent | Dtype::Currency | Dtype::Empty),
        ColRole::Group => false,
        ColRole::SrcRow | ColRole::RowType => true,
    })
}

/// Distinct numbers, strictly rising or strictly falling (nulls skipped): a key, not a measure.
fn is_sequence(values: &[Out]) -> bool {
    let v: Vec<f64> = values.iter().filter_map(|x| if let Out::Num(n) = x { Some(*n) } else { None }).collect();
    v.len() >= 2 && (v.windows(2).all(|w| w[0] < w[1]) || v.windows(2).all(|w| w[0] > w[1]))
}

/// Row labels for tables that have none (side-by-side and stacked grids that share the row labels
/// of a neighbouring table). A table of numbers only, whose body rows — data rows, total rows and
/// the blank rows between them — repeat those of a table with label columns row for row, gets
/// copies of that table's label columns:
/// - **beside it** (the same sheet rows, the labelled table on the left), or else
/// - **above it** (the same rows shifted down, overlapping columns; the nearest such table):
///   a month's daily grid under a summary that names the rows once.
///
/// Both need at least [`COPY_LABELS_MIN_ROWS`] data rows and exactly the same body layout, so a
/// grid with another number of rows, or totals in other places, keeps its plain numbers. The copied
/// columns have no source column (their values come from the other table's cells) and are named
/// like the originals, so the tables join on them.
fn copy_row_labels(tables: &mut [Table], layouts: &[Vec<(u32, bool)>]) {
    let shape = |l: &[(u32, bool)]| -> Vec<(u32, bool)> {
        let first = l.first().map_or(0, |x| x.0);
        l.iter().map(|&(r, t)| (r - first, t)).collect()
    };
    let data_rows = |l: &[(u32, bool)]| l.iter().filter(|x| !x.1).count();
    let takers: Vec<usize> = (0..tables.len())
        .filter(|&u| has_no_labels(&tables[u]) && data_rows(&layouts[u]) >= COPY_LABELS_MIN_ROWS)
        .collect();
    if takers.is_empty() {
        return;
    }
    let lenders: Vec<(usize, Vec<(u32, bool)>)> =
        (0..tables.len()).filter(|&d| !label_columns(&tables[d]).is_empty()).map(|d| (d, shape(&layouts[d]))).collect();
    let mut plan: Vec<(usize, usize)> = Vec::new();
    for u in takers {
        let (ru, lu, su) = (tables[u].rect, &layouts[u], shape(&layouts[u]));
        let mut beside: Option<usize> = None;
        let mut above: Option<usize> = None;
        for (d, sd) in &lenders {
            let rd = tables[*d].rect;
            if rd.right < ru.left && rd.top <= ru.bottom && ru.top <= rd.bottom && layouts[*d] == *lu {
                beside = beside.or(Some(*d));
            } else if rd.bottom < ru.top
                && rd.left <= ru.right
                && ru.left <= rd.right
                && *sd == su
                && above.is_none_or(|a| tables[a].rect.bottom < rd.bottom)
            {
                above = Some(*d);
            }
        }
        if let Some(d) = beside.or(above) {
            plan.push((u, d));
        }
    }
    for (u, d) in plan {
        let delta = layouts[u][0].0 - layouts[d][0].0;
        let (src, n) = (&tables[d], tables[d].n);
        let at: HashMap<u32, usize> = src.src_rows.iter().enumerate().map(|(i, &r)| (r, i)).collect();
        let rows: Vec<Option<usize>> = tables[u].src_rows.iter().map(|&r| at.get(&(r - delta)).copied()).collect();
        let copies: Vec<Column> = label_columns(src)
            .into_iter()
            .map(|k| {
                let c = &src.columns[k];
                Column {
                    source_col: None,
                    values: rows.iter().map(|i| i.map_or(Out::Null, |i| c.values[i].clone())).collect(),
                    ..c.clone()
                }
            })
            .collect();
        let t = &mut tables[u];
        let mut seen: HashSet<String> = t.columns.iter().map(|c| c.name.to_lowercase()).collect();
        let mut names = Vec::with_capacity(copies.len());
        let mut copies = copies;
        for c in &mut copies {
            c.name = unique(std::mem::take(&mut c.name), &mut seen, &mut t.transforms);
            names.push(format!("\"{}\"", c.name));
        }
        t.transforms.add(T::RowLabelsCopied, copies.len());
        t.warnings.push(format!(
            "{} copied from T{n}: these rows have no labels of their own and repeat T{n}'s rows one for one",
            names.join(", ")
        ));
        t.columns.splice(0..0, copies);
    }
}

pub(super) fn process_sheet(
    sheet: &Sheet,
    opts: &Options,
    layout: super::SheetAnalysis,
    out: &mut Normalized,
) {
    let grid = Grid::new(sheet);
    let (lay, warning, detection) = layout;
    if let Some(d) = detection {
        grid.set_totals(d);
    }
    let layout = (lay, warning);
    let cells = &sheet.cells;
    let mut info = SheetInfo {
        name: sheet.name.clone(),
        used_range: sheet.used_range(),
        cells: cells.len(),
        formulas: sheet.formula_count(),
        merges: sheet.merges.len(),
        tables: Vec::new(),
    };
    if cells.is_empty() {
        out.sheets.push(info);
        return;
    }
    let t0 = std::time::Instant::now();
    super::trace("grid", t0);
    let (lay, warning) = layout;
    out.warnings.extend(warning);
    super::trace("layout", t0);
    let mut owner = vec![OWN_NONE; cells.len()];
    let sheet_hints = OnceCell::new();
    let det = totals::detection(&grid);
    let cands = &det.candidates;
    let overrides = row_overrides(&grid, det);
    super::trace("totals_detect", t0);
    let checks: Vec<_> = cands.iter().map(|c| totals::verify(&grid, c)).collect();
    super::trace("totals_verify", t0);
    let mut row_total: HashMap<u32, RowTotal> = HashMap::new();
    for c in cands.iter().filter(|c| c.direction == Direction::Vertical) {
        let top = c.ranges.iter().map(|r| r.top).min();
        let e = row_total.entry(c.row).or_insert(RowTotal { level: c.level, top });
        if c.level == TotalLevel::Subtotal {
            e.level = TotalLevel::Subtotal;
        }
        e.top = match (e.top, top) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }

    // Tables.
    let first = out.tables.len();
    let mut kept: Vec<HashSet<u32>> = Vec::new();
    let mut removed: Vec<(usize, u32, RowType, Vec<usize>)> = Vec::new();
    let mut notes: Vec<NoteItem> = Vec::new();
    let mut layouts: Vec<Vec<(u32, bool)>> = Vec::new();
    for t in &lay.tables {
        let n = out.tables.len() + 1;
        let b = build_table(&grid, &lay, t, n, &sheet_hints, &row_total, &overrides, opts, &mut owner);
        info.tables.push(n);
        layouts.push(row_layout(&b.table, &b.removed_totals));
        kept.push(b.kept);
        removed.extend(b.removed_totals.into_iter().map(|(row, rt, idxs)| (n, row, rt, idxs)));
        notes.extend(b.notes);
        out.tables.push(b.table);
    }
    copy_row_labels(&mut out.tables[first..], &layouts);
    super::trace("build_tables", t0);
    let tables = &out.tables[first..];
    let table_at = |row: u32, col: u32| -> Option<usize> {
        tables
            .iter()
            .position(|t| t.rect.top <= row && row <= t.rect.bottom && t.rect.left <= col && col <= t.rect.right)
    };

    // Totals: detected candidates with their verification.
    let mut items: Vec<TotalItem> = Vec::new();
    let mut listed: HashSet<(u32, u32)> = HashSet::new();
    for (cand, check) in cands.iter().zip(&checks) {
        let k = table_at(cand.row, cand.col);
        let kept_in_data = k.is_some_and(|k| kept[k].contains(&cand.row));
        if let Some(i) = grid.cell_index(cand.row, cand.col)
            && owner[i] != OWN_TABLE
        {
            owner[i] = OWN_TOTALS;
        }
        let label = cand.label.and_then(|(lr, lc)| {
            let i = grid.cell_index(lr, lc)?;
            if owner[i] == OWN_NONE {
                owner[i] = OWN_TOTALS;
            }
            cells[i].value.as_ref().map(display)
        });
        listed.insert((cand.row, cand.col));
        let header = k.and_then(|k| column_name(&tables[k], cand.col));
        let (status, reason) = judge(&grid, cand, check, header, label.as_deref());
        items.push(TotalItem {
            table: k.map(|k| first + k + 1),
            sheet: sheet.name.clone(),
            row: cand.row,
            col: cand.col,
            count: 1,
            row_end: cand.row,
            label,
            label_cell: cand.label,
            level: level_str(cand.level),
            direction: match cand.direction {
                Direction::Vertical => "vertical",
                Direction::Horizontal => "horizontal",
            },
            origin: match cand.origin {
                TotalOrigin::Formula => "formula",
                TotalOrigin::Typed => "typed",
            },
            value: check.actual,
            recomputed: check.expected,
            status,
            reason,
            sources: cand.ranges.iter().map(|r| range_a1(*r)).collect(),
            formula: grid.cell(cand.row, cand.col).and_then(|c| c.formula.as_ref()).map(|f| format!("={f}")),
            kept_in_data,
        });
    }
    // "Of which" and average rows that ended up as such in a table (removed, or kept with
    // `keep_totals`): one item per number, checked against the rows above.
    let nonsum_rows: HashSet<(usize, u32)> = removed
        .iter()
        .filter(|(_, _, rt, _)| matches!(rt, RowType::Average | RowType::OfWhich))
        .map(|(n, row, _, _)| (*n, *row))
        .chain(tables.iter().flat_map(|t| {
            t.src_rows
                .iter()
                .zip(&t.row_types)
                .filter(|(_, rt)| matches!(rt, RowType::Average | RowType::OfWhich))
                .map(move |(&r, _)| (t.n, r))
        }))
        .collect();
    // The table of a cell of an "of which" / average row, when that table took the row out of its
    // data (or marked it): (table index, table number).
    let nonsum_table = |row: u32, col: u32| {
        table_at(row, col).map(|k| (k, first + k + 1)).filter(|&(_, n)| nonsum_rows.contains(&(n, row)))
    };
    for ns in &det.nonsum {
        let label_table = nonsum_table(ns.row, ns.label_col);
        let label = grid.cell_index(ns.row, ns.label_col).and_then(|i| {
            if label_table.is_some() && owner[i] == OWN_NONE {
                owner[i] = OWN_TOTALS;
            }
            cells[i].value.as_ref().map(display)
        });
        let (level, median) = match ns.kind {
            NonSumKind::Average => ("average", label.as_deref().is_some_and(|l| norm_key(l).starts_with("median"))),
            NonSumKind::OfWhich => ("of_which", false),
        };
        for ch in &ns.cells {
            let Some((k, n)) = nonsum_table(ns.row, ch.col) else { continue };
            let kept_in_data = kept[k].contains(&ns.row);
            if let Some(i) = grid.cell_index(ns.row, ch.col)
                && owner[i] == OWN_NONE
            {
                owner[i] = OWN_TOTALS;
            }
            if !listed.insert((ns.row, ch.col)) {
                continue;
            }
            let (status, reason) = match (ns.kind, ch.ok, ch.expected) {
                (NonSumKind::Average, Some(true), _) => ("ok", if median { "matches_median" } else { "matches_mean" }),
                (NonSumKind::Average, _, Some(_)) => ("unknown", "unconfirmed"),
                (NonSumKind::Average, _, None) => ("unknown", "no_source"),
                (NonSumKind::OfWhich, Some(true), _) => ("ok", "within_parent"),
                // A larger part is not reported as wrong: rates, shares and changes are no parts.
                (NonSumKind::OfWhich, Some(false), _) => ("unknown", "exceeds_parent"),
                (NonSumKind::OfWhich, None, _) => ("unknown", "no_parent"),
            };
            items.push(TotalItem {
                table: Some(n),
                sheet: sheet.name.clone(),
                row: ns.row,
                col: ch.col,
                count: 1,
                row_end: ns.row,
                label: label.clone(),
                label_cell: Some((ns.row, ns.label_col)),
                level,
                direction: "vertical",
                origin: if grid.cell(ns.row, ch.col).is_some_and(|c| c.formula.is_some()) { "formula" } else { "typed" },
                value: Some(ch.value),
                recomputed: ch.expected,
                status,
                reason: reason.into(),
                sources: ch.source.iter().map(|r| range_a1(*r)).collect(),
                formula: grid.cell(ns.row, ch.col).and_then(|c| c.formula.as_ref()).map(|f| format!("={f}")),
                kept_in_data,
            });
        }
    }
    let aggregate_rows: HashSet<u32> = det.aggregates.iter().map(|a| a.0).collect();
    // Removed total rows: numbers not found by `totals::detect` are listed unverified; the row's
    // label text is attached; other text goes to Notes.
    for (n, row, rt, idxs) in &removed {
        let label_idx = idxs.iter().copied().find(|&i| matches!(cells[i].value, Some(Value::Text(_))));
        let label = label_idx.and_then(|i| cells[i].value.as_ref()).map(display);
        for &i in idxs {
            let c = &cells[i];
            if owner[i] != OWN_NONE {
                continue;
            }
            let num = match &c.value {
                Some(Value::Number(v)) => Some(*v),
                _ => None,
            };
            if Some(i) == label_idx {
                owner[i] = OWN_TOTALS;
            } else if num.is_some() || (c.value.is_none() && c.formula.is_some()) {
                owner[i] = OWN_TOTALS;
                if listed.insert((c.row, c.col)) {
                    let header = n.checked_sub(first + 1).and_then(|k| tables.get(k)).and_then(|t| column_name(t, c.col));
                    let rate = header.is_some_and(not_additive) || label.as_deref().is_some_and(not_additive);
                    items.push(TotalItem {
                        table: Some(*n),
                        sheet: sheet.name.clone(),
                        row: c.row,
                        col: c.col,
                        count: 1,
                        row_end: c.row,
                        label: label.clone(),
                        label_cell: label_idx.map(|j| (cells[j].row, cells[j].col)),
                        level: rt.as_str(),
                        direction: "vertical",
                        origin: "layout",
                        value: num,
                        recomputed: None,
                        status: "unknown",
                        reason: if aggregate_rows.contains(row) {
                            "known_aggregate"
                        } else if rate {
                            "not_additive"
                        } else {
                            "not_verified"
                        }
                        .into(),
                        sources: Vec::new(),
                        formula: c.formula.as_ref().map(|f| format!("={f}")),
                        kept_in_data: false,
                    });
                }
            } else {
                owner[i] = OWN_NOTES;
                notes.push(NoteItem {
                    sheet: sheet.name.clone(),
                    row: *row,
                    col: c.col,
                    kind: "total_row",
                    table: Some(*n),
                    text: c.value.as_ref().map(display).unwrap_or_default(),
                    formula: c.formula.as_ref().map(|f| format!("={f}")),
                });
            }
        }
    }
    let mut items = collapse_row_totals(items);
    items.sort_by_key(|t| (t.row, t.col));

    // Everything not yet placed → Notes; titles / notes are attached to the nearest table.
    let mut nav_cells: Vec<(u32, u32)> = Vec::new();
    for (i, c) in cells.iter().enumerate() {
        if owner[i] != OWN_NONE {
            continue;
        }
        owner[i] = OWN_NOTES;
        let text = c.value.as_ref().map(display).unwrap_or_default();
        let nav = is_nav_link(&text);
        let kind = match lay.roles[i] {
            _ if nav => "other",
            Role::Title => "title",
            Role::Note => "note",
            Role::Total => "total",
            _ => "other",
        };
        if nav {
            nav_cells.push((c.row, c.col));
        }
        let table = match kind {
            "title" => associate(tables, c.row, c.col, true),
            "note" => associate(tables, c.row, c.col, false),
            _ => None,
        };
        notes.push(NoteItem {
            sheet: sheet.name.clone(),
            row: c.row,
            col: c.col,
            kind,
            table: table.map(|k| first + k + 1),
            text,
            formula: c.formula.as_ref().map(|f| format!("={f}")),
        });
    }
    notes.sort_by_key(|n| (n.row, n.col));
    // A table under a navigation link and without a title: the first title-like line between the
    // link and the table is its title ("zur Inhaltsübersicht" in A1, the real title in A2).
    for t in out.tables[first..].iter() {
        if t.title.is_some() || notes.iter().any(|x| x.table == Some(t.n) && x.kind == "title" && x.row < t.rect.top) {
            continue;
        }
        let Some(&(nav_row, _)) = nav_cells
            .iter()
            .filter(|(r, c)| *r < t.rect.top && t.rect.top - r - 1 <= ASSOC_MAX_GAP && *c <= t.rect.right && c + 2 >= t.rect.left)
            .max_by_key(|(r, _)| *r)
        else {
            continue;
        };
        if let Some(x) = notes.iter_mut().find(|x| {
            x.row > nav_row
                && x.row < t.rect.top
                && matches!(x.kind, "title" | "note")
                && !x.text.is_empty()
                && x.text.chars().count() <= 300
                && x.col <= t.rect.right
                && x.col + 2 >= t.rect.left
        }) {
            x.kind = "title";
            x.table = Some(t.n);
        }
    }

    // Titles / notes per table, then output names.
    let tables = &mut out.tables[first..];
    for t in tables.iter_mut() {
        let mine: Vec<&NoteItem> = notes
            .iter()
            .filter(|x| x.table == Some(t.n) && x.kind != "repeated_header" && !x.text.is_empty())
            .collect();
        if t.title.is_none() {
            // Nearest loose title above the table.
            t.title = mine
                .iter()
                .filter(|x| x.kind == "title" && x.row < t.rect.top)
                .max_by_key(|x| x.row)
                .map(|x| (x.text.clone(), (x.row, x.col)));
        }
        let title_cell = t.title.as_ref().map(|x| x.1);
        t.notes = mine
            .iter()
            .filter(|x| matches!(x.kind, "title" | "note") && Some((x.row, x.col)) != title_cell)
            .map(|x| (x.text.clone(), (x.row, x.col)))
            .collect();
        let prefix = format!("T{}_", t.n);
        let base = t.title.as_ref().map(|x| slug(&x.0, 31 - prefix.len())).filter(|s| !s.is_empty());
        let base = base
            .or_else(|| Some(slug(&t.source_sheet, 31 - prefix.len())).filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "table".to_string());
        t.sheet = format!("{prefix}{base}");
    }

    let mut counts = CellCounts { total: cells.len(), ..Default::default() };
    for o in &owner {
        match *o {
            OWN_TABLE => counts.in_tables += 1,
            OWN_TOTALS => counts.in_totals += 1,
            _ => counts.in_notes += 1,
        }
    }
    out.cells.total += counts.total;
    out.cells.in_tables += counts.in_tables;
    out.cells.in_totals += counts.in_totals;
    out.cells.in_notes += counts.in_notes;
    out.totals.extend(items);
    out.notes.extend(notes);
    out.sheets.push(info);
}

/// Flattened name of the table column read from source column `col`.
fn column_name(t: &Table, col: u32) -> Option<&str> {
    t.columns.iter().find(|c| c.source_col == Some(col)).map(|c| c.name.as_str())
}

/// Status and reason of a total from its check, with three cases where a typed total that does
/// not add up is not reported as wrong (`unknown` with a reason instead of `mismatch`):
///
/// * `not_additive`: its column header or row label names a rate, share, percentage, index,
///   average or per-capita value ("je Einwohner", "j.E.", "%", "Anteil", "Gesamt-VPI (2020=100)");
///   such values do not add up.
/// * `rounding_difference`: the difference is what rounding each summed value to the decimals
///   shown typically adds up to (at most √n units of the last decimal for n values, at least one)
///   and under 1% of the total, as in statistics published "in 1 000".
/// * `average_like`: every source value is >= 0 and the total lies between the smallest and the
///   largest of them, so it cannot be their sum (an average, index or rate).
///
/// Formula totals are never re-judged: their formula says what they add.
fn judge(
    grid: &Grid,
    cand: &TotalCandidate,
    check: &TotalCheck,
    header: Option<&str>,
    label: Option<&str>,
) -> (&'static str, String) {
    let status = match check.ok {
        Some(true) => "ok",
        Some(false) => "mismatch",
        None => "unknown",
    };
    let reason = if check.reason == CheckReason::Match { "match".into() } else { snake(check.reason) };
    if check.reason != CheckReason::Mismatch || cand.origin != TotalOrigin::Typed {
        return (status, reason);
    }
    if header.is_some_and(not_additive) || label.is_some_and(not_additive) {
        return ("unknown", "not_additive".into());
    }
    let (Some(actual), Some(expected)) = (check.actual, check.expected) else { return (status, reason) };
    let values = source_values(grid, cand);
    if values.len() >= 2 {
        let decimals = values.iter().chain([&actual]).map(|&v| decimals(v)).max().unwrap_or(0);
        let unit = 10f64.powi(-(decimals as i32));
        let diff = (actual - expected).abs();
        if decimals <= 4 && diff <= unit * (values.len() as f64).sqrt().max(1.0) + 1e-9 && diff <= 0.01 * actual.abs() {
            return ("unknown", "rounding_difference".into());
        }
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if min >= 0.0 && actual >= min && actual <= max {
            return ("unknown", "average_like".into());
        }
    }
    (status, reason)
}

/// Decimals a value is written with (0-4; 5 for more).
fn decimals(v: f64) -> u32 {
    for d in 0..=4 {
        let x = v * 10f64.powi(d);
        if (x - x.round()).abs() <= 1e-6 * x.abs().max(1.0) {
            return d as u32;
        }
    }
    5
}

/// The numbers a typed total adds (as [`totals::verify`] reads them for a typed total).
fn source_values(grid: &Grid, cand: &TotalCandidate) -> Vec<f64> {
    let Some(used) = grid.used_range() else { return Vec::new() };
    let exclude: HashSet<(u32, u32)> = cand.exclude.iter().copied().collect();
    let mut out = Vec::new();
    for r in &cand.ranges {
        let r = Rect {
            top: r.top.max(used.top),
            left: r.left.max(used.left),
            bottom: r.bottom.min(used.bottom),
            right: r.right.min(used.right),
        };
        for i in grid.cells_in(r) {
            let c = &grid.sheet.cells[i as usize];
            if (c.row, c.col) == (cand.row, cand.col) || exclude.contains(&(c.row, c.col)) {
                continue;
            }
            match &c.value {
                Some(Value::Number(v) | Value::DateTime(v)) => out.push(*v),
                Some(Value::Text(s)) => out.extend(totals::verify::numeric_text(s)),
                _ => {}
            }
        }
    }
    out
}

/// Shortest run of row totals collapsed into one item.
const MIN_TOTAL_RUN: usize = 3;

/// A total column (one row total per data row, all checking out) becomes one item per run instead
/// of one per row: registers with a formula total on each of 60k rows would otherwise list 60k
/// identical "ok" items. Mismatches and unverified totals are always listed one by one.
fn collapse_row_totals(items: Vec<TotalItem>) -> Vec<TotalItem> {
    let collapsible = |t: &TotalItem| t.direction == "horizontal" && t.status == "ok";
    if items.iter().filter(|t| collapsible(t)).count() < MIN_TOTAL_RUN {
        return items;
    }
    let mut by_col: std::collections::BTreeMap<u32, Vec<TotalItem>> = std::collections::BTreeMap::new();
    for t in items {
        by_col.entry(t.col).or_default().push(t);
    }
    let same_run = |a: &TotalItem, b: &TotalItem| {
        a.table == b.table
            && a.level == b.level
            && a.origin == b.origin
            && a.kept_in_data == b.kept_in_data
            && a.label == b.label
    };
    let mut out = Vec::new();
    for (_, mut col) in by_col {
        col.sort_by_key(|t| t.row);
        let mut i = 0;
        while i < col.len() {
            let mut j = i + 1;
            if collapsible(&col[i]) {
                while j < col.len() && collapsible(&col[j]) && same_run(&col[i], &col[j]) {
                    j += 1;
                }
            }
            if j - i >= MIN_TOTAL_RUN {
                let all_match = col[i..j].iter().all(|t| t.reason == "match");
                let last = col[j - 1].row;
                let mut run = col[i].clone();
                run.count = j - i;
                run.row_end = last;
                run.value = None;
                run.recomputed = None;
                run.reason = if all_match { "match".into() } else { "rounded".into() };
                out.push(run);
            } else {
                out.extend(col[i..j].iter().cloned());
            }
            i = j;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Prodaja po regionima — 2024", 26), "prodaja_po_regionima_2024");
        assert_eq!(slug("Укупно Београд", 26), "ukupno_beograd");
        assert_eq!(slug("  ***  ", 26), "");
        assert!(slug("a very long title that goes on and on and on", 10).len() <= 10);
    }

    #[test]
    fn group_prefixes() {
        assert_eq!(group_prefix(&["Region: North", "region: South"]), Some("Region".into()));
        assert_eq!(group_prefix(&["Region: North", "Area: South"]), None);
        assert_eq!(group_prefix(&["North"]), None);
    }

    fn row_total(row: u32, status: &'static str) -> TotalItem {
        TotalItem {
            table: Some(1),
            sheet: "S".into(),
            row,
            col: 5,
            count: 1,
            row_end: row,
            label: Some("Total".into()),
            label_cell: Some((0, 5)),
            level: "total",
            direction: "horizontal",
            origin: "formula",
            value: Some(1.0),
            recomputed: Some(1.0),
            status,
            reason: if status == "ok" { "match".into() } else { "mismatch".into() },
            sources: vec![format!("B{0}:E{0}", row + 1)],
            formula: None,
            kept_in_data: true,
        }
    }

    #[test]
    fn row_total_runs_collapse_around_mismatches() {
        let mut items: Vec<TotalItem> = (1..=6).map(|r| row_total(r, "ok")).collect();
        items[3].status = "mismatch";
        items.push(row_total(7, "ok"));
        let mut out = collapse_row_totals(items);
        out.sort_by_key(|t| t.row);
        let cells: Vec<(String, usize)> = out.iter().map(|t| (t.cell_a1(), t.count)).collect();
        assert_eq!(cells, [("F2:F4".to_string(), 3), ("F5".into(), 1), ("F6:F8".into(), 3)]);
        assert_eq!((out[0].value, out[0].reason.as_str()), (None, "match"));
    }
}
