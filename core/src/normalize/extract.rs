//! Per-sheet extraction: layout tables → flat header + typed columns; totals → `TotalItem`s;
//! every other filled cell → `NoteItem`s. Each source cell ends up in exactly one place
//! (table, Totals or Notes), which `CellCounts` reports.

use std::sync::OnceLock;
use std::collections::{BTreeMap, HashMap, HashSet};

use super::headers::{clean_header_with, is_nav_link, not_additive};
use super::typing::{Slot, Typed, display, fit, type_column};
use super::{
    CellCounts, ColRole, Column, Dtype, Normalized, NoteItem, Options, Out, RowType, SheetInfo, T, Table, TotalItem,
    Transforms,
};
use crate::analysis::{DetectedTable, Role, SheetLayout};
use crate::grid::Grid;
use crate::model::{Rect, Value, a1};
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

/// Words of a label; spaced letters ("T o t a l", "O G Ó Ł E M") are one word.
fn label_words(t: &str) -> usize {
    let words: Vec<&str> = t.split(|ch: char| !ch.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if words.len() >= 3 && words.iter().all(|w| w.chars().count() == 1) { 1 } else { words.len() }
}

/// A series identifier code: 3–8 uppercase letters and digits with at least two letters
/// (`MGSL`, `LF24`, `G7AU`, `YBWF`).
fn is_series_code(t: &str) -> bool {
    let t = t.trim();
    (3..=8).contains(&t.len())
        && t.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && t.bytes().filter(u8::is_ascii_uppercase).count() >= 2
}

/// B81: a register record whose name starts with a total word ("TOTAL RECRUIT LTD", "Zusammen
/// unterwegs", "Total Energies SE") and that stayed a data row: its keyword candidate is no total
/// unless its numbers add up, so it is not listed on Totals (where an agent would be told to
/// exclude it). A name: the total word followed by words that are no total word or year, in a
/// column of many distinct values (names, purposes), not of categories ("Summe der bezogenen
/// Rohstoffe" once per industry, "Total liabilities" in long-format data), on a row where no
/// number adds up (`arith_rows`).
fn record_named_like_a_total(
    grid: &Grid,
    cand: &TotalCandidate,
    t: &Table,
    arith_rows: &HashSet<u32>,
    columns: &mut NameColumns,
) -> bool {
    // `src_rows` rise down the table.
    let Ok(i) = t.src_rows.binary_search(&cand.row) else { return false };
    t.row_types[i] == RowType::Data && !arith_rows.contains(&cand.row) && named_like_a_total(grid, cand, columns)
}

/// Per label column: (distinct texts, texts), counted once per sheet (a sheet has thousands of
/// keyword candidates in a few columns; counting the column per candidate took seconds, B90).
type NameColumns = HashMap<u32, (usize, usize)>;

/// The keyword candidate of a register record named with a leading total word, without
/// arithmetic behind it ([`record_named_like_a_total`]).
fn named_like_a_total(grid: &Grid, cand: &TotalCandidate, columns: &mut NameColumns) -> bool {
    let sheet = grid.sheet;
    let register = match (sheet.cells.first(), sheet.cells.last()) {
        (Some(a), Some(b)) => b.row - a.row + 1 >= totals::keywords::REGISTER_MIN_ROWS,
        _ => false,
    };
    if !register
        || cand.origin != TotalOrigin::Typed
        || cand.evidence.iter().any(|e| matches!(e, Evidence::ArithmeticMatch | Evidence::Unlabelled | Evidence::SourceBelow))
    {
        return false;
    }
    let Some((lr, lc)) = cand.label else { return false };
    let Some(Value::Text(label)) = grid.cell(lr, lc).and_then(|c| c.value.as_ref()) else {
        return false;
    };
    let words: Vec<&str> = label.split_whitespace().collect();
    let Some((first, rest)) = words.split_first() else { return false };
    let name = !rest.is_empty()
        && totals::total_keyword(first).is_some()
        && rest.iter().all(|w| totals::total_keyword(w).is_none() && !w.trim_matches(|c: char| !c.is_alphanumeric()).bytes().all(|b| b.is_ascii_digit()));
    if !name {
        return false;
    }
    // A legal form makes it a company name ("TOTAL RECRUIT LTD", "Summa Holding GmbH").
    const LEGAL: &[&str] = &[
        "ltd", "limited", "plc", "llp", "llc", "inc", "corp", "co", "gmbh", "ggmbh", "ag", "ev", "kg", "ug", "sa", "sas", "sarl",
        "srl", "spa", "bv", "nv", "ab", "as", "oy", "doo", "ad", "sp", "zoo", "cic",
    ];
    let legal = rest.iter().any(|w| LEGAL.contains(&w.to_lowercase().trim_matches(|c: char| !c.is_alphanumeric()).replace('.', "").as_str()));
    // Otherwise a column of names or purposes: more than a third of its texts are distinct, and
    // at least a thousand of them (a register of records, not a statement's line items such as
    // "Total Assets" or "Total exports").
    let (distinct, n) = *columns.entry(lc).or_insert_with(|| {
        let mut texts: HashSet<&str> = HashSet::new();
        let mut n = 0usize;
        for &i in grid.col_cells(lc) {
            if let Some(Value::Text(x)) = &sheet.cells[i as usize].value {
                texts.insert(x.as_str());
                n += 1;
            }
        }
        (texts.len(), n)
    });
    distinct * 3 > n && (legal || distinct >= NAME_COLUMN_MIN_DISTINCT)
}

/// Distinct texts a column needs to count as a register's name column without a legal form (B81).
const NAME_COLUMN_MIN_DISTINCT: usize = 1000;

/// A total label recurring this often in its column is a category of long-format data.
const LABEL_CATEGORY_REPEATS: usize = 3;
/// A total label confirming a total row has at most this many words ("Grand Total", "Razem
/// Działanie", "SUMME Tirol mit Innsbruck"); longer texts are measure names or codes.
const LABEL_MAX_WORDS: usize = 4;

/// A total label longer than [`LABEL_MAX_WORDS`] that still confirms a total row: it starts with
/// the total word ("Total Criminal Code traffic violations", "Total other federal statute
/// violations") and the row adds up in at least [`LONG_LABEL_MIN_MATCHES`] columns. A measure name
/// or a code with a total word inside ("Debt service total (% of GNI)") does neither.
fn long_total_label(t: &str, matched_cols: usize) -> bool {
    let first = t.split(|ch: char| !ch.is_alphanumeric()).find(|w| !w.is_empty()).unwrap_or("");
    matched_cols >= LONG_LABEL_MIN_MATCHES && totals::total_keyword(first).is_some()
}

/// Columns that must add up for a long label starting with a total word ([`long_total_label`]).
const LONG_LABEL_MIN_MATCHES: usize = 2;

fn row_overrides(grid: &Grid, det: &totals::Detection) -> Overrides {
    let mut out: Overrides = HashMap::new();
    let mut add = |row: u32, o: RowOverride, cols: Vec<u32>| out.entry(row).or_default().push((o, cols));
    // A row with a SUM formula is a total whatever its text says.
    let with_formula: HashSet<u32> =
        det.candidates.iter().filter(|c| c.origin == TotalOrigin::Formula).map(|c| c.row).collect();
    for &r in det.incidental_rows.iter().filter(|r| !with_formula.contains(r)) {
        add(r, RowOverride::Data, Vec::new());
    }
    let first_text = |row: u32| {
        grid.row_cells(row).iter().map(|&i| &grid.sheet.cells[i as usize]).find_map(|c| match &c.value {
            Some(Value::Text(t)) if !t.trim().is_empty() && totals::numeric_text(t).is_none() => Some(c.col),
            _ => None,
        })
    };
    // A label that recurs in its column is a category of long-format data ("Totaal van woningen …"
    // per region, "A,NRG_CL_E,TOTAL,GWH,AL"): such rows stay where the layout put them.
    let mut repeats: HashMap<(u32, String), usize> = HashMap::new();
    let mut category = |col: u32, t: &str| {
        *repeats.entry((col, t.to_string())).or_insert_with(|| {
            grid.col_cells(col)
                .iter()
                .filter(|&&i| matches!(&grid.sheet.cells[i as usize].value, Some(Value::Text(x)) if x == t))
                .count()
        }) >= LABEL_CATEGORY_REPEATS
    };
    // Columns whose typed number adds up, per keyword row.
    let mut matched: HashMap<u32, usize> = HashMap::new();
    for c in &det.candidates {
        if c.direction == Direction::Vertical
            && c.origin == TotalOrigin::Typed
            && c.evidence.contains(&Evidence::ArithmeticMatch)
            && c.evidence.iter().any(|e| matches!(e, Evidence::Keyword(_)))
        {
            *matched.entry(c.row).or_default() += 1;
        }
    }
    let matched_cols = |row: u32| matched.get(&row).copied().unwrap_or(0);
    let mut named_rows: Vec<u32> = Vec::new();
    let mut name_columns = NameColumns::new();
    for c in &det.candidates {
        let label_text = c.label.and_then(|(lr, lc)| match grid.cell(lr, lc).and_then(|x| x.value.as_ref()) {
            Some(Value::Text(t)) => Some((lc, t.as_str())),
            _ => None,
        });
        let labelled = c.evidence.iter().any(|e| matches!(e, Evidence::Keyword(_)))
            && label_text.is_some_and(|(lc, t)| {
                first_text(c.row) == Some(lc)
                    && (label_words(t) <= LABEL_MAX_WORDS || long_total_label(t, matched_cols(c.row)))
                    && !category(lc, t)
            });
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
        } else if c.direction == Direction::Vertical && named_like_a_total(grid, c, &mut name_columns) {
            named_rows.push(c.row);
        }
    }
    // B81: "TOTAL RECRUIT LTD" in a register is a record unless some of its numbers add up.
    for r in named_rows {
        if !det.candidates.iter().any(|c| c.row == r && c.evidence.contains(&Evidence::ArithmeticMatch)) {
            add(r, RowOverride::Data, Vec::new());
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
/// One row label of a table's body in source order (see [`outline_paths`]).
struct OutlineRow {
    /// Indent level of the label cell's style (0..=15).
    indent: u8,
    /// Leading spaces of the label text.
    spaces: u8,
    /// The label cell is bold.
    bold: bool,
    label: String,
    /// Output row, `None` for a group header or a removed total row.
    out: Option<usize>,
    /// A group-header row (its label goes to the group column).
    group: bool,
}

/// Most ancestors kept per row (`<label>_parent` .. `<label>_parent_<MAX>`).
const MAX_OUTLINE_DEPTH: usize = 6;

/// The ancestors of each output data row in an outline of row labels, root first: the rows above
/// it whose labels are indented less, within the same group (a group header starts afresh).
/// Indent levels come from the cell styles, or from leading spaces when no label has a style
/// indent. A bold label heads the plain labels below it at its own indent (`Canada` over
/// `Atlantic provinces`, `Quebec`, …) when the first label at that indent under the same
/// ancestors is such a heading: a bold row after plain ones at its indent (a total closing them)
/// heads nothing. Bold-only ancestry needs at least two rows (and a tenth of the rows)
/// with an ancestor; explicit indentation edges survive without that density threshold.
/// Ancestors are always labels of the sheet's own rows.
/// From the run's first label `k` at level `l` to the run's end (a shallower label or a group header):
/// some bold label at level `l` directly followed by a plain one at level `l`.
fn bold_heads_plain(outline: &[OutlineRow], k: usize, l: u16, base: &impl Fn(&OutlineRow) -> u16) -> bool {
    // directly: no deeper label between them
    let mut prev: Option<&OutlineRow> = None;
    for o in outline[k..].iter().take_while(|o| !o.group && base(o) >= l) {
        if base(o) == l {
            if prev.is_some_and(|p| p.bold && !o.bold) {
                return true;
            }
            prev = Some(o);
        } else {
            prev = None;
        }
    }
    false
}

fn outline_paths(outline: &[OutlineRow], n: usize) -> Option<Vec<Vec<String>>> {
    let style = outline.iter().any(|o| o.indent > 0);
    let base = |o: &OutlineRow| u16::from(if style { o.indent } else { o.spaces });
    // Effective level: 2 * indent + 1, one less for a bold heading.
    let mut eff: Vec<u16> = outline.iter().map(|o| 2 * base(o) + 1).collect();
    // Indent levels whose run (since the last group header or shallower label) is headed by bold
    // labels: (level, headed).
    let mut runs: Vec<(u16, bool)> = Vec::new();
    for (k, o) in outline.iter().enumerate() {
        if o.group {
            runs.clear();
            continue;
        }
        let l = base(o);
        while runs.last().is_some_and(|&(rl, _)| rl > l) {
            runs.pop();
        }
        if runs.last().is_none_or(|&(rl, _)| rl < l) {
            // The first label at this level is a bold heading: followed by a plain one at the same
            // level (`Canada` over `Quebec`), or by deeper rows while a later bold label of the run
            // heads plain ones at this level (`Custody` over indented rows, then `Community
            // supervision` over `Probation`, HiTab 959). A run of bold labels one after the other
            // (codes of a geography list, `K04000001` then `E92000001`) heads nothing.
            let next = outline.get(k + 1).filter(|x| !x.group);
            let headed = o.bold
                && next.is_some_and(|x| (base(x) == l && !x.bold) || (base(x) > l && bold_heads_plain(outline, k, l, &base)));
            runs.push((l, headed));
        }
        if runs.last().is_some_and(|&(_, h)| h) && o.bold {
            eff[k] -= 1;
        }
    }
    let mut levels: Vec<u16> = outline.iter().zip(&eff).filter(|(o, _)| !o.group).map(|(_, &e)| e).collect();
    levels.sort_unstable();
    levels.dedup();
    if levels.len() < 2 {
        return None;
    }
    let mut paths: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut stack: Vec<(u16, usize)> = Vec::new();
    for (k, o) in outline.iter().enumerate() {
        if o.group {
            stack.clear();
            continue;
        }
        let l = eff[k];
        while stack.last().is_some_and(|&(sl, _)| sl >= l) {
            stack.pop();
        }
        if let Some(out) = o.out
            && out < n
        {
            let from = stack.len().saturating_sub(MAX_OUTLINE_DEPTH);
            paths[out] = stack[from..].iter().map(|&(_, p)| outline[p].label.clone()).collect();
        }
        stack.push((l, k));
    }
    let with = paths.iter().filter(|p| !p.is_empty()).count();
    if with < 2 || with * 10 < n {
        // Sparse hierarchy needs source indentation for each ancestor, rather than the
        // bold-heading inference above. One leading space may just be record padding;
        // text indentation needs a two-space step, while style indentation is explicit.
        // Reuse the stack without retaining another row index.
        stack.clear();
        for (k, o) in outline.iter().enumerate() {
            if o.group {
                stack.clear();
                continue;
            }
            while stack.last().is_some_and(|&(sl, _)| sl >= eff[k]) {
                stack.pop();
            }
            if let Some(out) = o.out.filter(|&out| out < n) {
                paths[out].clear();
                let from = stack.len().saturating_sub(MAX_OUTLINE_DEPTH);
                paths[out].extend(stack[from..].iter()
                    .filter(|&&(_, p)| {
                        let parent = &outline[p];
                        base(parent) < base(o) && (style || o.spaces.saturating_sub(parent.spaces) >= 2)
                    })
                    .map(|&(_, p)| outline[p].label.clone()));
            }
            stack.push((eff[k], k));
        }
    }
    paths.iter().any(|p| !p.is_empty()).then_some(paths)
}

/// Rows of a table body that are section labels the layout took for data (see the call in
/// [`build_table`]): the label column's cell is the row's only content, a text (or a year) that
/// is no total, and the next row with a label there is indented deeper, or has values while
/// this label stands out from it (bold, all capitals, merged across the row).
fn section_rows(
    grid: &Grid,
    rows: &[(u32, Vec<usize>)],
    body_top: u32,
    label_col: Option<u32>,
    roles: &[Role],
) -> HashSet<u32> {
    let mut out = HashSet::new();
    let Some(lc) = label_col else { return out };
    let cells = &grid.sheet.cells;
    let filled = |i: usize| cells[i].value.as_ref().is_some_and(|v| !display(v).trim().is_empty());
    let body: Vec<&(u32, Vec<usize>)> = rows.iter().filter(|(row, _)| *row >= body_top).collect();
    let label = |idxs: &[usize]| idxs.iter().copied().find(|&i| cells[i].col == lc && filled(i));
    let fmt = |i: usize| grid.sheet.formats.get(i).and_then(|f| f.unpack()).unwrap_or_default();
    let level = |i: usize| {
        let spaces = match &cells[i].value {
            Some(Value::Text(t)) => t.chars().take_while(|&c| c == ' ' || c == '\u{a0}').count(),
            _ => 0,
        };
        (fmt(i).indent, spaces)
    };
    let caps = |i: usize| match &cells[i].value {
        Some(Value::Text(t)) => {
            let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
            letters.len() >= 3 && letters.iter().all(|c| c.is_uppercase()) && letters.iter().any(|c| c.is_ascii())
        }
        _ => false,
    };
    let merged = |row: u32| grid.sheet.merges.iter().any(|m| m.top == row && m.left == lc && m.right > m.left);
    let values = |idxs: &[usize]| idxs.iter().any(|&i| cells[i].col != lc && filled(i));
    // From the bottom up, so a section right below is known (`Segment Information:` over the
    // section `Net sales:`).
    for (k, (row, idxs)) in body.iter().enumerate().rev() {
        let Some(i) = label(idxs) else { continue };
        // A label the layout took for a note mid-table (StatCan 132: a bold section after the
        // indented rows of the one above) is a candidate too, when rows with values follow.
        // So is a label alone on its row the layout took for a repeated header (StatCan 2433:
        // `Individual health` over empty cells merged down into the next section row).
        let note_mid = matches!(roles[i], Role::Note | Role::Header) && body[k + 1..].iter().any(|(_, ix)| values(ix));
        // A total-worded label of several words alone on its row, without a number (StatCan 108:
        // bold `Total household income` over `Below median income`): a section, no total row.
        let words = |t: &str| t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).count();
        let total_heading = matches!(&cells[i].value, Some(Value::Text(t)) if words(t) >= 2)
            && matches!(roles[i], Role::Total | Role::Data | Role::Other);
        if values(idxs) || !(matches!(roles[i], Role::Data | Role::Other) || note_mid || total_heading) {
            continue;
        }
        let text_ok = match &cells[i].value {
            Some(Value::Text(t)) => {
                let t = t.trim();
                (crate::totals::total_keyword(t).is_none() || total_heading)
                    && !crate::layout::is_note_label(t)
                    && crate::values::table_missing_token(t).is_none()
                    && t.chars().any(|c| c.is_alphanumeric())
            }
            Some(Value::Number(x)) => x.fract() == 0.0 && (1900.0..=2100.0).contains(x),
            _ => false,
        };
        if !text_ok {
            continue;
        }
        // The next row with a label in the label column, right below (blank rows aside).
        let Some((next_row, next)) = body[k + 1..].iter().find(|(_, ix)| ix.iter().any(|&j| filled(j))).map(|x| (x.0, &x.1)) else {
            continue;
        };
        let Some(j) = label(next) else { continue };
        // A section heads data rows: not only a total row (`CONTINGENCY` over its `TOTAL`).
        let total_next = next.iter().any(|&x| matches!(&cells[x].value, Some(Value::Text(t)) if crate::totals::total_keyword(t.trim()).is_some()));
        if total_next {
            continue;
        }
        let deeper = level(j) > level(i);
        let apart = (fmt(i).bold && !fmt(j).bold) || (caps(i) && !caps(j)) || merged(*row);
        let heading = caps(i) || matches!(&cells[i].value, Some(Value::Text(t)) if t.trim_end().ends_with(':'));
        let next_section = !values(next) && (out.contains(&next_row) || matches!(roles[j], Role::GroupHeader)) && level(j) >= level(i);
        if deeper || (apart && values(next)) || (heading && next_section) {
            out.insert(*row);
        }
    }
    out
}

/// A unit written alone on a body row (StatCan): `mean`, `number`, `percentage points`, `dollars`,
/// `2016 dollars`, `percentage distribution`: a unit or scale label, or every word one of a few
/// measure words (numbers allowed).
fn is_body_unit(t: &str) -> bool {
    const WORDS: [&str; 34] = [
        "percent", "percentage", "percentages", "points", "point", "number", "numbers", "dollars", "dollar", "constant",
        "current", "mean", "average", "median", "proportion", "proportions", "rate", "rates", "ratio", "ratios",
        "index", "hours", "months", "years", "days", "weeks", "minutes", "thousands", "millions", "billions", "of",
        "distribution", "count", "share",
    ];
    let t = t.trim();
    if t.is_empty() || t.chars().count() > 32 {
        return false;
    }
    if crate::layout::is_unit_text(t) {
        return true;
    }
    let mut any = false;
    for w in t.split(|c: char| c.is_whitespace() || c == '/' || c == ',') {
        let w = w.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']'));
        if w.is_empty() || w.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let lw = w.to_lowercase();
        if !WORDS.contains(&lw.as_str()) && w != "$" && w != "%" {
            return false;
        }
        any |= lw != "of";
    }
    any
}

/// Unit rows inside a table body (StatCan `mean`, `number`, `percentage points` between the rows):
/// the row's only content is a unit (`is_body_unit`) in value columns, its label column is blank,
/// there are rows with values above it, and the next row with values below has a number in each
/// column the unit names. The unit heads the rows below it (their values are in that unit): a
/// section, like a label alone on its row. Row -> the unit text.
fn unit_section_rows(grid: &Grid, rows: &[(u32, Vec<usize>)], body_top: u32, label_col: Option<u32>) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let Some(lc) = label_col else { return out };
    let cells = &grid.sheet.cells;
    let filled = |i: usize| cells[i].value.as_ref().is_some_and(|v| !display(v).trim().is_empty());
    let number = |i: usize| match &cells[i].value {
        Some(Value::Number(_)) => true,
        Some(Value::Text(t)) => crate::values::number::parse_number(t.trim(), Default::default()).is_some(),
        _ => false,
    };
    let body: Vec<&(u32, Vec<usize>)> = rows.iter().filter(|(row, _)| *row >= body_top).collect();
    let mut values_above = false;
    for (k, (row, idxs)) in body.iter().enumerate() {
        let f: Vec<usize> = idxs.iter().copied().filter(|&i| filled(i)).collect();
        let unit = !f.is_empty()
            && f.iter().all(|&i| cells[i].col != lc && matches!(&cells[i].value, Some(Value::Text(t)) if is_body_unit(t)));
        if unit && values_above {
            // the next row with a number: a number in every column of the unit row
            let below = body[k + 1..].iter().find(|(_, ix)| ix.iter().any(|&j| number(j)));
            let fits = below.is_some_and(|(_, ix)| f.iter().all(|&i| ix.iter().any(|&j| cells[j].col == cells[i].col && number(j))));
            if fits {
                let mut label: Vec<String> = Vec::new();
                for &i in &f {
                    let t = display(cells[i].value.as_ref().unwrap()).trim().to_string();
                    if !label.contains(&t) {
                        label.push(t);
                    }
                }
                out.insert(*row, label.join(" / "));
                continue;
            }
        }
        values_above |= f.iter().any(|&i| number(i));
    }
    out
}

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
/// marks, line-break hyphenation, whitespace; see [`clean_header_with`]), repeats dropped, joined with
/// ` / `.
fn clean_name(grid: &Grid, parts: &[(String, (u32, u32))]) -> String {
    let mut out: Vec<String> = Vec::with_capacity(parts.len());
    for (text, (r, c)) in parts {
        let cleaned = match grid.cell(*r, *c).and_then(|c| c.value.as_ref()) {
            Some(Value::Text(raw)) => clean_header_with(raw, &|w| grid.has_word(w), grid.is_english()),
            _ => text.clone(),
        };
        if !cleaned.is_empty() && out.last() != Some(&cleaned) {
            out.push(cleaned);
        }
    }
    out.join(" / ")
}

/// Column names made unique (case-insensitive, like Excel) by appending ` (2)`, ` (3)`…
#[derive(Default)]
pub(super) struct UniqueNames {
    seen: HashSet<String>,
    /// Lowercased name → the suffix to try first: every smaller one is taken. Without it,
    /// thousands of equal headers (a merged header across a wide table) took quadratic time.
    next: HashMap<String, usize>,
}

impl UniqueNames {
    pub(super) fn taken<'a>(names: impl IntoIterator<Item = &'a str>) -> UniqueNames {
        UniqueNames { seen: names.into_iter().map(str::to_lowercase).collect(), next: HashMap::new() }
    }

    pub(super) fn make(&mut self, name: String, tr: &mut Transforms) -> String {
        let lower = name.to_lowercase();
        if self.seen.insert(lower.clone()) {
            return name;
        }
        tr.add(T::DuplicateHeadersRenamed, 1);
        let mut k = self.next.get(&lower).copied().unwrap_or(2);
        loop {
            let cand = format!("{name} ({k})");
            k += 1;
            if self.seen.insert(cand.to_lowercase()) {
                self.next.insert(lower, k);
                return cand;
            }
        }
    }
}

/// Columns of one table with the same name, each without a header cell above its own (a parent
/// label merged over only part of its group, `Main Income` over F:G of F:I; written once at the
/// left of its group): each takes the nearest label to its left on the header rows above as its
/// parent (`Main Income / This Year`), the header-path convention, instead of a counter (`This
/// Year (2)`). Only when that tells all of them apart; otherwise the counter stays.
fn parent_names_for_duplicates(head: &[Vec<Head>], heads: &mut [(Vec<HeaderPart>, String)], grid: &Grid, tr: &mut Transforms) {
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (k, (parts, label)) in heads.iter().enumerate() {
        if !parts.is_empty() {
            by_name.entry(label.to_lowercase()).or_default().push(k);
        }
    }
    let taken: HashSet<String> = heads.iter().map(|(_, l)| l.to_lowercase()).collect();
    let mut groups: Vec<Vec<usize>> = by_name.into_values().filter(|ks| ks.len() >= 2).collect();
    groups.sort_unstable();
    for ks in groups {
        // The header row of each column's topmost own label: parents come from the rows above it.
        let top_row = |k: usize| (0..head.len()).find(|&hr| head[hr][k].is_some());
        let mut names: Vec<String> = Vec::with_capacity(ks.len());
        for &k in &ks {
            let Some(top) = top_row(k) else { break };
            let mut parents: Vec<(String, (u32, u32))> = Vec::new();
            for row in &head[..top] {
                if let Some(p) = row[..k].iter().rev().find_map(|h| h.as_ref()) {
                    parents.push(p.clone());
                }
            }
            if parents.is_empty() {
                break;
            }
            let prefix = clean_name(grid, &parents);
            names.push(format!("{prefix} / {}", heads[k].1));
        }
        let distinct: HashSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
        if names.len() != ks.len() || distinct.len() != names.len() || distinct.iter().any(|n| taken.contains(n)) {
            continue;
        }
        for (&k, name) in ks.iter().zip(names) {
            heads[k].1 = name;
            tr.add(T::HeaderCellsForwardFilled, 1);
        }
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

/// The header cells of row `hr` all sit under one and the same header cell of the row above (a
/// parent label over exactly these columns, merged or filled right), and that label heads nothing else.
fn numbers_under_one_label(head: &[Vec<Head>], hr: usize) -> bool {
    if hr == 0 {
        return false;
    }
    // (a cell merged down from the row above, `Group and cohort` over A3:A5, is no number of this row)
    let cols: Vec<usize> = (0..head[hr].len()).filter(|&hc| head[hr][hc].is_some() && head[hr][hc] != head[hr - 1][hc]).collect();
    let Some(parent) = cols.first().and_then(|&hc| head[hr - 1][hc].as_ref()) else {
        return false;
    };
    cols.iter().all(|&hc| head[hr - 1][hc].as_ref() == Some(parent))
        && (0..head[hr].len()).all(|hc| cols.contains(&hc) || head[hr - 1][hc].as_ref() != Some(parent))
}

/// A total row as detected: its level and the first row of its source (the topmost range start
/// among the row's vertical totals), when known.
#[derive(Debug, Clone, Copy)]
struct RowTotal {
    level: TotalLevel,
    top: Option<u32>,
}

/// A caption that starts with a table word: "Table 1: …", "Tabelle 3", "Quadro 1", "Cuadro 2.1".
fn table_word(s: &str) -> bool {
    let w = s.split(|c: char| !c.is_alphabetic()).next().unwrap_or("").to_lowercase();
    [
        "table", "tabelle", "tableau", "tabla", "tabela", "tabella", "tabulka", "tabell", "taulukko", "tabel", "tabl",
        "tab", "quadro", "quadros", "cuadro", "cuadros",
    ]
    .contains(&w.as_str())
}

/// The title from caption cell `c`: its text, joined with the texts right of it on its row when
/// it is only a number ("Quadro 1", "Table 3:").
fn caption_title<'a>(c: &NoteItem, row_notes: impl Iterator<Item = &'a NoteItem>) -> (String, (u32, u32)) {
    let mut text = c.text.trim().to_string();
    if text.chars().count() <= 14 {
        let mut rest: Vec<&NoteItem> = row_notes.filter(|x| x.row == c.row && x.col > c.col && !x.text.trim().is_empty()).collect();
        rest.sort_by_key(|x| x.col);
        for x in rest {
            text.push(' ');
            text.push_str(x.text.trim());
        }
    }
    (text, (c.row, c.col))
}

/// Sex breakdown codes and words as (male, female, both) after `norm_key`.
const SEX_TRIPLES: &[[&str; 3]] = &[
    ["m", "f", "t"],
    ["h", "m", "hm"],
    ["m", "f", "mf"],
    ["m", "w", "i"],
    ["m", "z", "u"],
    ["masculino", "feminino", "total"],
    ["homens", "mulheres", "total"],
    ["hombres", "mujeres", "total"],
    ["male", "female", "total"],
    ["males", "females", "total"],
    ["men", "women", "total"],
    ["manner", "frauen", "insgesamt"],
    ["manner", "frauen", "zusammen"],
    ["mannlich", "weiblich", "insgesamt"],
    ["mannlich", "weiblich", "zusammen"],
    ["hommes", "femmes", "ensemble"],
    ["hommes", "femmes", "total"],
    ["uomini", "donne", "totale"],
    ["maschi", "femmine", "totale"],
    ["muski", "zenski", "ukupno"],
    ["muskarci", "zene", "ukupno"],
    ["mannen", "vrouwen", "totaal"],
    ["mezczyzni", "kobiety", "ogolem"],
];

/// B72: blocks of three rows that split a row label by sex (M / F / T, Männer / Frauen /
/// Insgesamt, Masculino / Feminino / Total, …).
struct SexBlocks {
    /// Sheet column of the sex codes.
    col: u32,
    /// Row → its kind and total level: the both-sexes row of a block is a subtotal of the two
    /// rows above it (when its numbers add up), and every row of a block labelled with a total
    /// word ("Total") is a total; the others are data.
    rows: HashMap<u32, (RowKind, Option<TotalLevel>)>,
    /// (row, column) → the block's label in that column, written on every row of the block:
    /// labels centred on the middle row or split over the rows ("CTFP / Por tempo /
    /// indeterminado") are joined.
    labels: HashMap<(u32, u32), Value>,
}

fn sex_blocks(rows: &[(u32, Vec<usize>)], cells: &[crate::model::Cell], r: Rect, body_top: u32) -> Option<SexBlocks> {
    let body: Vec<&(u32, Vec<usize>)> = rows.iter().filter(|(row, _)| *row >= body_top).collect();
    let text_at = |idxs: &[usize], col: u32| -> Option<String> {
        idxs.iter().find(|&&i| cells[i].col == col).and_then(|&i| match &cells[i].value {
            Some(Value::Text(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
            _ => None,
        })
    };
    let num_at = |idxs: &[usize], col: u32| -> Option<f64> {
        idxs.iter().find(|&&i| cells[i].col == col).and_then(|&i| match &cells[i].value {
            Some(Value::Number(x)) => Some(*x),
            _ => None,
        })
    };
    for col in r.left..=r.right.min(r.left + 3) {
        // Each distinct text is keyed once (a column of 600k records repeats a few hundred
        // values, B90); `keys` holds an index into `distinct`, whose flag says whether the key
        // is a word of some triple at all.
        let mut ids: HashMap<&str, u32> = HashMap::new();
        let mut distinct: Vec<(String, bool)> = Vec::new();
        let keys: Vec<Option<u32>> = body
            .iter()
            .map(|(_, idxs)| {
                let i = *idxs.iter().find(|&&i| cells[i].col == col)?;
                let Some(Value::Text(t)) = &cells[i].value else { return None };
                let t = t.trim();
                if t.is_empty() {
                    return None;
                }
                Some(*ids.entry(t).or_insert_with(|| {
                    let k = norm_key(t);
                    let word = SEX_TRIPLES.iter().any(|tr| tr.contains(&k.as_str()));
                    distinct.push((k, word));
                    (distinct.len() - 1) as u32
                }))
            })
            .collect();
        let word = |k: &Option<u32>| k.is_some_and(|id| distinct[id as usize].1);
        let mut blocks: Vec<(usize, [usize; 3])> = Vec::new(); // (first body index, triple positions m/f/t)
        let mut i = 0;
        while i + 2 < body.len() {
            let three = [&keys[i], &keys[i + 1], &keys[i + 2]];
            let contiguous = body[i + 1].0 == body[i].0 + 1 && body[i + 2].0 == body[i].0 + 2;
            // A triple needs a word on each of the three rows.
            let hit = (contiguous && three.iter().all(|k| word(k)))
                .then(|| {
                    SEX_TRIPLES.iter().find_map(|tr| {
                        let pos = |w: &str| three.iter().position(|k| k.is_some_and(|id| distinct[id as usize].0 == w));
                        let (m, f, t) = (pos(tr[0])?, pos(tr[1])?, pos(tr[2])?);
                        (m != f && f != t && m != t).then_some([m, f, t])
                    })
                })
                .flatten();
            match hit {
                Some(p) => {
                    blocks.push((i, p));
                    i += 3;
                }
                None => i += 1,
            }
        }
        let coded = keys.iter().filter(|k| k.is_some()).count();
        if blocks.len() < 2 || blocks.len() * 3 * 10 < coded * 6 {
            continue;
        }
        let mut out = SexBlocks { col, rows: HashMap::new(), labels: HashMap::new() };
        for &(i, [m, f, t]) in &blocks {
            let idx = |k: usize| &body[i + k].1;
            let row = |k: usize| body[i + k].0;
            let mut label_first: Option<String> = None;
            for lc in r.left..col {
                let parts: Vec<String> = (0..3).filter_map(|k| text_at(idx(k), lc)).collect();
                if parts.is_empty() {
                    continue;
                }
                let joined = parts.join(" ");
                label_first.get_or_insert_with(|| joined.clone());
                for k in 0..3 {
                    out.labels.insert((row(k), lc), Value::Text(joined.clone()));
                }
            }
            // The both-sexes row adds the two others in the number columns right of the codes.
            let (mut ok, mut bad) = (0, 0);
            for c in col + 1..=r.right {
                let (a, b, s) = (num_at(idx(m), c), num_at(idx(f), c), num_at(idx(t), c));
                if a.is_none() && b.is_none() && s.is_none() {
                    continue;
                }
                let sum = a.unwrap_or(0.0) + b.unwrap_or(0.0);
                if (sum - s.unwrap_or(0.0)).abs() <= 0.5 + 1e-9 * sum.abs() {
                    ok += 1;
                } else {
                    bad += 1;
                }
            }
            let adds_up = ok >= 1 && bad * 3 <= ok;
            let total_block = label_first.as_deref().is_some_and(|l| totals::total_keyword(l).is_some());
            for k in 0..3 {
                let kind = if total_block && adds_up {
                    (RowKind::Total, Some(TotalLevel::Total))
                } else if k == t && adds_up {
                    (RowKind::Total, Some(TotalLevel::Subtotal))
                } else {
                    (RowKind::Data, None)
                };
                out.rows.insert(row(k), kind);
            }
        }
        return Some(out);
    }
    None
}

/// B84: rolls a header row of dates over into the next year where a monthly sequence steps back
/// from December to January of the same year. Returns (last date before the step, last date
/// after the fix) when something changed; the header texts are rewritten in place.
fn roll_over_header_dates(row_heads: &mut [Head], grid: &Grid) -> Option<(String, String)> {
    use crate::values::{serial_to_ymd, ymd_to_serial};
    let dated: Vec<(usize, (i32, u8, u8))> = row_heads
        .iter()
        .enumerate()
        .filter_map(|(hc, h)| {
            let (_, (row, col)) = h.as_ref()?;
            match grid.cell(*row, *col)?.value {
                Some(Value::DateTime(x)) => Some((hc, serial_to_ymd(x)?)),
                _ => None,
            }
        })
        .collect();
    if dated.len() < 4 {
        return None;
    }
    let day = |y: i32, m: u8, d: u8| ymd_to_serial(y, m, d.min(crate::values::date::days_in_month(y, m)));
    let mut shift = 0;
    let mut prev: Option<(f64, (i32, u8, u8))> = None;
    let mut fixed: Vec<(usize, (i32, u8, u8))> = Vec::new();
    let mut before = None;
    for &(hc, (y, m, d)) in &dated {
        let mut cur = (y + shift, m, d);
        if let Some((p, pd)) = prev {
            let mut s = day(cur.0, m, d);
            if s < p {
                let next = day(cur.0 + 1, m, d);
                // Only the December → January step of a monthly sequence.
                if !(pd.1 == 12 && m == 1 && next > p && next - p <= 62.0) {
                    return None;
                }
                shift += 1;
                before.get_or_insert(pd);
                cur = (cur.0 + 1, m, d);
                s = next;
            } else if s - p > 62.0 {
                return None;
            }
            prev = Some((s, cur));
        } else {
            prev = Some((day(y, m, d), cur));
        }
        if shift > 0 {
            fixed.push((hc, cur));
        }
    }
    let iso = |(y, m, d): (i32, u8, u8)| format!("{y:04}-{m:02}-{:02}", d.min(crate::values::date::days_in_month(y, m)));
    let (from, last) = (before?, *fixed.last()?);
    for (hc, ymd) in fixed {
        if let Some(h) = row_heads[hc].as_mut() {
            h.0 = iso(ymd);
        }
    }
    Some((iso(from), iso(last.1)))
}

/// A row of distinct labels (at least two cells, two of them words; no measured number), at
/// least half of its filled cells marked as header cells by the layout, some as data. The caller
/// also wants it to span half the table's columns and to hold no `Key:` label (a form line).
fn mixed_header_row(cells: &[crate::model::Cell], roles: &[Role], idxs: &[usize]) -> bool {
    let filled: Vec<usize> = idxs.iter().copied().filter(|&i| cells[i].value.as_ref().is_some_and(|v| !display(v).trim().is_empty())).collect();
    let header = filled.iter().filter(|&&i| roles[i] == Role::Header).count();
    if filled.len() < 2 || header * 2 < filled.len() || header == filled.len() {
        return false;
    }
    if filled.iter().any(|&i| matches!(&cells[i].value, Some(Value::Number(x)) if !(x.fract() == 0.0 && (1900.0..=2100.0).contains(x))))
    {
        return false;
    }
    let texts: Vec<String> = filled.iter().filter_map(|&i| cells[i].value.as_ref().map(display)).map(|t| t.trim().to_string()).collect();
    let distinct: HashSet<&String> = texts.iter().collect();
    let worded = distinct
        .iter()
        .filter(|t| t.chars().filter(|c| c.is_alphabetic()).count() >= 2 && crate::values::missing_token(t).is_none())
        .count();
    distinct.len() * 10 >= texts.len() * 8 && worded >= 2
}

/// Rows that together name the columns: the labels of each column joined over the rows are
/// distinct for at least four in five columns, and at least two columns hold words (not a
/// block of unit or placeholder rows).
fn header_block(block: &[(u32, Vec<usize>)], cells: &[crate::model::Cell]) -> bool {
    let mut paths: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for (_, idxs) in block {
        // A measured number is a value, not a label (`Physical Gas | 6500` beside a header).
        if idxs.iter().any(|&i| matches!(&cells[i].value, Some(Value::Number(x)) if !(x.fract() == 0.0 && (1900.0..=2100.0).contains(x)))) {
            return false;
        }
        for &i in idxs {
            let t = cells[i].value.as_ref().map(display).unwrap_or_default();
            if !t.trim().is_empty() {
                paths.entry(cells[i].col).or_default().push(t.trim().to_string());
            }
        }
    }
    let n = paths.len();
    let distinct: HashSet<&Vec<String>> = paths.values().collect();
    let worded = paths
        .values()
        .filter(|p| p.iter().any(|t| t.chars().filter(|c| c.is_alphabetic()).count() >= 2 && crate::values::missing_token(t).is_none()))
        .count();
    n >= 2 && distinct.len() * 10 >= n * 8 && worded >= 2
}

/// A row with a measured number (not a whole number that could be a year, not a column
/// numbering) whose cells are mostly of their columns' kinds (three in five) and whose texts
/// mostly do not repeat the header's labels: a data row, whatever roles the layout gave its cells.
fn data_like_row(cells: &[crate::model::Cell], idxs: &[usize], header_texts: &HashSet<String>, kinds: &HashMap<u32, u8>) -> bool {
    let measured = idxs.iter().any(|&i| match &cells[i].value {
        Some(Value::Number(x)) => !(x.fract() == 0.0 && (1900.0..=2100.0).contains(x)),
        _ => false,
    });
    if !measured || numbering_row(cells, idxs) {
        return false;
    }
    let (mut filled, mut matched) = (0, 0);
    for &i in idxs {
        let k = match &cells[i].value {
            Some(Value::Number(_)) => 0,
            Some(Value::DateTime(_)) => 1,
            Some(Value::Text(t)) if !t.trim().is_empty() => 2,
            _ => continue,
        };
        filled += 1;
        matched += usize::from(kinds.get(&cells[i].col) == Some(&k));
    }
    if matched * 5 < filled * 3 {
        return false;
    }
    let texts: Vec<String> = idxs
        .iter()
        .filter_map(|&i| match &cells[i].value {
            Some(Value::Text(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
            _ => None,
        })
        .collect();
    texts.iter().filter(|t| header_texts.contains(*t)).count() * 2 < texts.len().max(1)
}

/// A row of column numbers counting up by one (at least three), its blank cells aside.
fn numbering_row(cells: &[crate::model::Cell], idxs: &[usize]) -> bool {
    let values: Vec<&Value> =
        idxs.iter().filter_map(|&i| cells[i].value.as_ref()).filter(|v| !matches!(v, Value::Text(t) if t.trim().is_empty())).collect();
    if crate::layout::column_numbering(values.iter().copied()) {
        return true;
    }
    // A helper row with a gap or two (`5 | 6 | … | 14 | 16 | 17`, a column left out): whole
    // numbers from 1 to 999 counting up, at least five in six steps by one.
    let nums: Vec<f64> = values
        .iter()
        .filter_map(|v| match v {
            Value::Number(x) if x.fract() == 0.0 && (1.0..1000.0).contains(x) => Some(*x),
            _ => None,
        })
        .collect();
    nums.len() == values.len()
        && nums.len() >= 6
        && nums.windows(2).all(|w| w[1] > w[0])
        && nums.windows(2).filter(|w| w[1] - w[0] == 1.0).count() * 6 >= (nums.len() - 1) * 5
}

/// Blocks of cells outside every layout table that hold values: connected cells (consecutive
/// rows, overlapping columns, at most one blank column between) the layout marked neither title
/// nor note, at least two columns wide, with at least two rows of two or more numbers or dates
/// (a key/value list has one value per row), no `Key:` line, apart from every layout table. A row
/// the layout marked as header starts a block. A first row of labels (or years) over the values
/// is the header; otherwise the columns are col_<letter>.
fn value_blocks(grid: &Grid, lay: &SheetLayout) -> Vec<DetectedTable> {
    let cells = &grid.sheet.cells;
    let inside = |c: &crate::model::Cell| {
        lay.tables.iter().any(|t| (t.rect.top..=t.rect.bottom).contains(&c.row) && (t.rect.left..=t.rect.right).contains(&c.col))
    };
    let number = |i: usize| matches!(&cells[i].value, Some(Value::Number(_) | Value::DateTime(_)));
    let year = |i: usize| matches!(&cells[i].value, Some(Value::Number(x)) if x.fract() == 0.0 && (1900.0..=2100.0).contains(x));
    let key = |i: usize| matches!(&cells[i].value, Some(Value::Text(t)) if t.trim_end().ends_with([':', '：']));
    let header_cell = |i: usize| lay.roles[i] == Role::Header;
    // Row segments of eligible cells: (row, left, right, cells), split at two blank columns.
    let mut segs: Vec<(u32, u32, u32, Vec<usize>)> = Vec::new();
    for (i, c) in cells.iter().enumerate() {
        if c.value.as_ref().is_none_or(|v| display(v).trim().is_empty()) || inside(c) || matches!(lay.roles[i], Role::Title | Role::Note) {
            continue;
        }
        match segs.last_mut() {
            Some((r, _, right, v)) if *r == c.row && c.col <= *right + 2 => {
                *right = c.col;
                v.push(i);
            }
            _ => segs.push((c.row, c.col, c.col, vec![i])),
        }
    }
    // Components: segments of consecutive rows whose columns overlap.
    let mut parent: Vec<usize> = (0..segs.len()).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut x = x;
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let mut prev_start = 0;
    let mut k = 0;
    while k < segs.len() {
        let row = segs[k].0;
        let mut e = k;
        while e < segs.len() && segs[e].0 == row {
            e += 1;
        }
        for a in k..e {
            for b in prev_start..k {
                if segs[b].0 + 1 == row && segs[b].1 <= segs[a].2 + 1 && segs[a].1 <= segs[b].2 + 1 {
                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                    parent[ra] = rb;
                }
            }
        }
        prev_start = k;
        k = e;
    }
    let mut comps: BTreeMap<usize, BTreeMap<u32, Vec<usize>>> = BTreeMap::new();
    for (k, seg) in segs.iter().enumerate() {
        let root = find(&mut parent, k);
        comps.entry(root).or_default().entry(seg.0).or_default().extend(seg.3.iter().copied());
    }
    let mut out = Vec::new();
    for rows in comps.values() {
        let rows: Vec<(u32, &Vec<usize>)> = rows.iter().map(|(r, v)| (*r, v)).collect();
        // A row the layout marked as header starts a block of its own (its header).
        let mut starts: Vec<usize> = vec![0];
        starts.extend((1..rows.len()).filter(|&j| rows[j].1.iter().any(|&i| header_cell(i))));
        starts.push(rows.len());
        for w in starts.windows(2) {
            let block = &rows[w[0]..w[1]];
            // A `Key: | value` line: a key with a cell after it (a lone "Operations & Maintenance:"
            // is a section label).
            let key_line = |idxs: &[usize]| idxs.iter().enumerate().any(|(k, &i)| key(i) && k + 1 < idxs.len());
            if block.len() < 2 || block.iter().any(|(_, idxs)| key_line(idxs)) {
                continue;
            }
            let numeric_rows = block.iter().filter(|(_, idxs)| idxs.iter().filter(|&&i| number(i)).count() >= 2).count();
            let (left, right) = block
                .iter()
                .flat_map(|(_, idxs)| idxs.iter().map(|&i| cells[i].col))
                .fold((u32::MAX, 0), |(a, b), c| (a.min(c), b.max(c)));
            if numeric_rows < 2 || right < left + 1 {
                continue;
            }
            let first = block[0].1;
            // Labels (or a year over a column of prices) over values: the header.
            let labels = first.iter().all(|&i| matches!(&cells[i].value, Some(Value::Text(_))) || year(i))
                && first.iter().any(|&i| matches!(&cells[i].value, Some(Value::Text(_))) || first.len() < block[1].1.len())
                && block[1..].iter().any(|(_, idxs)| idxs.iter().any(|&i| number(i)));
            if !labels && first.iter().any(|&i| header_cell(i)) {
                continue;
            }
            let (top, bottom) = (block[0].0, block[block.len() - 1].0);
            // Every cell of the rectangle (cells of other blocks inside it too): no title or
            // note, no header cell below a first row of labels (it would be a repeated header).
            let lo = cells.partition_point(|c| c.row < top);
            let hi = cells.partition_point(|c| c.row <= bottom);
            let stray = (lo..hi).any(|i| {
                let c = &cells[i];
                (left..=right).contains(&c.col)
                    && c.value.as_ref().is_some_and(|v| !display(v).trim().is_empty())
                    && (matches!(lay.roles[i], Role::Title | Role::Note) || (header_cell(i) && (c.row != top || !labels)))
            });
            if stray {
                continue;
            }
            // Never over a table of the layout.
            if lay.tables.iter().any(|t| t.rect.top <= bottom && top <= t.rect.bottom && t.rect.left <= right && left <= t.rect.right) {
                continue;
            }
            out.push(DetectedTable {
                rect: Rect { top, left, bottom, right },
                header_top: top,
                header_rows: u32::from(labels),
                confidence: 0.5,
            });
        }
    }
    // Blocks that share rows and columns (a component's bounding box over another's) are left out.
    let mut keep = vec![true; out.len()];
    for a in 0..out.len() {
        for b in 0..out.len() {
            let (x, y) = (&out[a].rect, &out[b].rect);
            if a != b && keep[b] && x.top <= y.bottom && y.top <= x.bottom && x.left <= y.right && y.left <= x.right {
                keep[a] = false;
            }
        }
    }
    out.into_iter().zip(keep).filter(|(_, k)| *k).map(|(t, _)| t).collect()
}

/// Most `Key: | value` lines skipped above the header of a table without header rows.
const MAX_FORM_LINES: usize = 12;

/// B72: the header rows the layout found, or a fuller header row below them. When every header
/// row is sparse (a year label "Ano: 2016" over a numbered caption) and a row of header cells
/// with at least three labels and twice as many as any header row comes before the first data
/// row, the first such row (and header rows right under it) is the header; the rows above it are
/// titles. Its labels must be distinct (not a unit row or a legend).
fn promote_header(
    rows: &[(u32, Vec<usize>)],
    cells: &[crate::model::Cell],
    roles: &[Role],
    htop: u32,
    header_rows: u32,
    bottom: u32,
) -> (u32, u32) {
    let (top, mut end) = promote_header_inner(rows, cells, roles, htop, header_rows, bottom);
    // A last header row that reads like the rows below it is the first data row (a table cut
    // from its header: "Outside Services | 2340 | 3057 | …" as column names; a first record
    // "2020-01-01 | Li | JD Income | 3128" under the real header row).
    let kinds = body_kinds(rows, cells, end);
    while end > top {
        let Some((_, idxs)) = rows.iter().find(|(row, _)| *row + 1 == end) else { break };
        if !data_like_header_row(cells, idxs, &kinds) {
            break;
        }
        end -= 1;
    }
    (top, end)
}

/// Value kind of each column's body cells (up to 200 rows below the header): 0 number, 1 date,
/// 2 text, when at least three in five filled cells (and two cells) are of it.
fn body_kinds(rows: &[(u32, Vec<usize>)], cells: &[crate::model::Cell], body_top: u32) -> HashMap<u32, u8> {
    let mut counts: HashMap<u32, [usize; 3]> = HashMap::new();
    for (_, idxs) in rows.iter().filter(|(row, _)| *row >= body_top).take(200) {
        for &i in idxs {
            let k = match &cells[i].value {
                Some(Value::Number(_)) => 0,
                Some(Value::DateTime(_)) => 1,
                Some(Value::Text(t)) if !t.trim().is_empty() => 2,
                _ => continue,
            };
            counts.entry(cells[i].col).or_default()[k] += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(col, c)| {
            let n: usize = c.iter().sum();
            let (k, best) = c.iter().enumerate().max_by_key(|&(k, v)| (*v, std::cmp::Reverse(k)))?;
            (n >= 2 && best * 5 >= n * 3).then_some((col, k as u8))
        })
        .collect()
}

/// A header row whose cells are values of their columns' kinds: at least two measured numbers
/// over number columns or dates over date columns, three in five cells of their column's kind,
/// no text over a column of numbers or dates, and not a scale of numbers counting up (a grid of strikes `Strike | 2.5 | 3.0 | 3.5` names
/// its columns) or a column numbering.
fn data_like_header_row(cells: &[crate::model::Cell], idxs: &[usize], kinds: &HashMap<u32, u8>) -> bool {
    let (mut filled, mut matched, mut strong, mut labels_over_values) = (0, 0, 0, 0);
    let mut numbers: Vec<f64> = Vec::new();
    for &i in idxs {
        let kind = kinds.get(&cells[i].col).copied();
        match &cells[i].value {
            Some(Value::Number(x)) => {
                filled += 1;
                numbers.push(*x);
                let measured = !(x.fract() == 0.0 && (1900.0..=2100.0).contains(x));
                if kind == Some(0) {
                    matched += 1;
                    if measured {
                        strong += 1;
                    }
                }
            }
            Some(Value::DateTime(_)) => {
                filled += 1;
                if kind == Some(1) {
                    matched += 1;
                    strong += 1;
                }
            }
            Some(Value::Text(t)) if !t.trim().is_empty() => {
                filled += 1;
                if kind == Some(2) {
                    matched += 1;
                } else if matches!(kind, Some(0 | 1)) {
                    // A label over a column of numbers or dates: the header of a table beside
                    // ("2020-01-01 | Li | 3128 | Category | Income | Expenditure"), keep the row.
                    labels_over_values += 1;
                }
            }
            _ => {}
        }
    }
    let scale = numbers.len() >= 3 && numbers.windows(2).all(|w| w[1] > w[0]);
    strong >= 2 && matched * 5 >= filled * 3 && labels_over_values == 0 && !scale && !numbering_row(cells, idxs)
}

fn promote_header_inner(
    rows: &[(u32, Vec<usize>)],
    cells: &[crate::model::Cell],
    roles: &[Role],
    htop: u32,
    header_rows: u32,
    bottom: u32,
) -> (u32, u32) {
    let body_top = (htop + header_rows).min(bottom + 1);
    let filled = |idxs: &[usize]| idxs.iter().filter(|&&i| cells[i].value.as_ref().is_some_and(|v| !display(v).is_empty())).count();
    let widest = rows.iter().filter(|(row, _)| (htop..body_top).contains(row)).map(|(_, idxs)| filled(idxs)).max().unwrap_or(0);
    let is_header = |idxs: &[usize]| idxs.iter().all(|&i| matches!(roles[i], Role::Header | Role::Title | Role::Note) || filled(&[i]) == 0);
    let mut found: Option<usize> = None;
    // A table without header rows may start with one or two short key/value lines ("Project
    // Name: | Number of Clients: | 7") above its real header row: they are skipped (and become
    // title rows) instead of ending the search, so the header row below still names the columns.
    let width = rows.iter().flat_map(|(_, idxs)| idxs.iter().map(|&i| cells[i].col)).collect::<HashSet<u32>>().len();
    let mut skipped = 0;
    // A form of `Key: | value` lines (a deal sheet's "Counterparty Name: | … | Deal Date: | …",
    // an ONS "Date of publication: | … | Inquiries: | …" block) above a table without header
    // rows: every filled cell is a key ending in a colon or the value right after one, or the
    // line is one or two cells in a wide table. Up to MAX_FORM_LINES such lines are skipped.
    let key_text = |i: usize| matches!(&cells[i].value, Some(Value::Text(t)) if t.trim_end().ends_with([':', '：']));
    let form_line = |idxs: &[usize]| {
        let f: Vec<usize> = idxs.iter().copied().filter(|&i| filled(&[i]) > 0).collect();
        let keyed = f.iter().enumerate().all(|(k, &i)| key_text(i) || (k > 0 && key_text(f[k - 1])));
        (keyed && f.iter().any(|&i| key_text(i))) || (f.len() <= 2 && f.len() * 3 <= width)
    };
    let numbering = |idxs: &[usize]| numbering_row(cells, idxs);
    let mut form_lines = 0;
    // The run of layout header rows right above the first data row (sheet rows at most one blank
    // row apart), for a header of several rows none of which names the columns alone.
    let mut run: Vec<usize> = Vec::new();
    let mut block: Option<(usize, usize)> = None;
    for (k, (row, idxs)) in rows.iter().enumerate().filter(|(_, (row, _))| *row >= body_top) {
        // Without header rows, a row of labels most of whose cells the layout marked as header
        // cells (the rest as data: "Date | 18/06/99 | 18/12/99 | …", "Date | Day | BOL/BRAZ-POWER |
        // …") is the header too: it would go to Notes as a repeated header.
        if header_rows == 0 && found.is_none() && filled(idxs) * 2 >= width && !idxs.iter().any(|&i| key_text(i)) && mixed_header_row(cells, roles, idxs) {
            found = Some(k);
            break;
        }
        if idxs.iter().any(|&i| matches!(roles[i], Role::Data | Role::Total | Role::Other) && filled(&[i]) > 0) {
            if header_rows == 0 && run.len() >= 2 && rows[*run.last().unwrap_or(&k)].0 + 2 >= *row {
                block = header_block(&rows[run[0]..=run[run.len() - 1]], cells).then(|| (run[0], run[run.len() - 1]));
            }
            let n = filled(idxs);
            if header_rows == 0 && skipped < 2 && width >= 6 && (n <= 3 || n * 3 <= width) {
                skipped += 1;
                continue;
            }
            // A row of column numbers (`1 | 2 | 3 | …`, `5 | 6 | 7 | …`, a helper row over the
            // header) is no data either.
            if header_rows == 0 && width >= 3 && form_lines < MAX_FORM_LINES && (form_line(idxs) || numbering(idxs)) {
                form_lines += 1;
                run.clear();
                continue;
            }
            break;
        }
        if is_header(idxs) && idxs.iter().any(|&i| roles[i] == Role::Header) {
            if run.last().is_some_and(|&j| rows[j].0 + 2 < *row) {
                run.clear();
            }
            run.push(k);
        } else if filled(idxs) > 0 {
            run.clear();
        }
        let n = filled(idxs);
        // Distinct labels (not a unit or legend row: "EURO EURO EURO", "nichts vorhanden" × 6).
        let distinct: HashSet<String> = idxs.iter().filter_map(|&i| cells[i].value.as_ref().map(display)).filter(|s| !s.is_empty()).collect();
        // Without header rows, a row the layout itself marked as header cells (it would end up
        // on Notes as a repeated header, the columns named col_<letter>) is the header: "WP ID |
        // Teaching Group | Condition | 1 | 2 | …" has numbers among its labels.
        // Its labels must be distinct and include words (not a row of "x" placeholders).
        let worded = distinct
            .iter()
            .filter(|t| t.chars().filter(|c| c.is_alphabetic()).count() >= 2 && crate::values::missing_token(t).is_none())
            .count();
        if header_rows == 0
            && is_header(idxs)
            && n >= 2
            && distinct.len() * 10 >= n * 8
            && worded >= 2
            && idxs.iter().any(|&i| roles[i] == Role::Header)
        {
            found = Some(k);
            break;
        }
        if is_header(idxs)
            && n >= 3
            && n >= 2 * widest.max(1)
            && distinct.len() * 10 >= n * 8
            // Words, not a legend of symbols ("– | = | nichts vorhanden").
            && distinct.iter().filter(|t| t.chars().filter(|c| c.is_alphabetic()).count() >= 2).count() * 10 >= n * 8
            && idxs.iter().any(|&i| roles[i] == Role::Header)
        {
            found = Some(k);
            break;
        }
    }
    if found.is_none()
        && let Some((a, b)) = block
    {
        if form_lines > 0 && rows.len() - b - 1 <= skipped + form_lines {
            return (htop, body_top);
        }
        return (rows[a].0, (rows[b].0 + 1).min(bottom + 1));
    }
    let Some(k) = found else { return (htop, body_top) };
    // Skipped form lines are titles only when the table under the header is longer than the form.
    if form_lines > 0 && rows.len() - k - 1 <= skipped + form_lines {
        return (htop, body_top);
    }
    let top = rows[k].0;
    let mut end = top + 1;
    for (row, idxs) in &rows[k + 1..] {
        if *row != end || !is_header(idxs) || !idxs.iter().any(|&i| roles[i] == Role::Header) {
            break;
        }
        end += 1;
    }
    // Without header rows, the band also takes label rows the layout did not mark, up to a later
    // row it did (ONS: "People | Men | Women", a blank row, "Level | Rate …", then the dataset
    // codes): every row on the way holds only text labels (no numbers, no missing-value marks)
    // with at most one blank row between them.
    if header_rows == 0 {
        let labels = |idxs: &[usize]| {
            idxs.iter().all(|&i| match &cells[i].value {
                Some(Value::Text(t)) => t.trim().is_empty() || crate::values::missing_token(t).is_none(),
                None => true,
                Some(_) => false,
            }) && filled(idxs) > 0
        };
        let mut prev = end - 1;
        let from = end;
        for (seen, (row, idxs)) in rows[k + 1..].iter().filter(|(row, _)| *row >= from).enumerate() {
            if *row > prev + 2 || seen >= 4 || !labels(idxs) {
                break;
            }
            prev = *row;
            if is_header(idxs) && idxs.iter().any(|&i| roles[i] == Role::Header) {
                end = row + 1;
            }
        }
    }
    (top, end.min(bottom + 1))
}

/// Recover explicit merged parents above repeated child headers, across at most one
/// blank row. A caption spanning the whole table or unrelated metadata is insufficient.
fn merged_parent_top(grid: &Grid, r: Rect, htop: u32, owner: &[u8]) -> Option<u32> {
    let cells = &grid.sheet.cells;
    for parent_row in (htop.saturating_sub(2)..htop).rev() {
        let parents: Vec<_> = grid.sheet.merges.iter().filter(|m| {
            m.top == parent_row && m.bottom == parent_row && m.left >= r.left
                && m.right <= r.right && m.right > m.left
        }).collect();
        if parents.len() < 2 {
            continue;
        }
        let mut labels = HashSet::new();
        let mut children: Option<Vec<String>> = None;
        let valid = parents.iter().all(|m| {
            let Some(Value::Text(label)) = grid.cell(m.top, m.left).and_then(|c| c.value.as_ref()) else { return false };
            if label.trim().is_empty() || !labels.insert(norm_key(label)) {
                return false;
            }
            let own: Vec<String> = (m.left..=m.right).filter_map(|col| {
                grid.cell(htop, col).and_then(|c| c.value.as_ref()).map(display).filter(|s| !s.is_empty())
            }).collect();
            if own.len() != (m.right - m.left + 1) as usize {
                return false;
            }
            match &children {
                Some(first) => *first == own,
                None => { children = Some(own); true }
            }
        });
        // The intervening row must be empty within this table's columns.
        let gap_empty = cells.iter().skip(cells.partition_point(|c| c.row <= parent_row))
            .take_while(|c| c.row < htop)
            .all(|c| c.col < r.left || c.col > r.right || c.value.as_ref().is_none_or(|v| display(v).is_empty()));
        let unowned = cells.iter().enumerate().skip(cells.partition_point(|c| c.row < parent_row))
            .take_while(|(_, c)| c.row < htop)
            .filter(|(_, c)| c.col >= r.left && c.col <= r.right)
            .all(|(i, _)| owner[i] == OWN_NONE);
        if valid && gap_empty && unowned {
            return Some(parent_row);
        }
    }
    None
}

/// Cohorts that repeat only merged parent headers inherit the original child rows.
/// Require identical complete parent spans, a stub cohort, and numeric records on both sides.
fn changed_parent_blocks(grid: &Grid, t: &DetectedTable) -> Vec<DetectedTable> {
    if t.header_rows < 2 { return vec![t.clone()]; }
    let body = t.header_top.saturating_add(t.header_rows);
    let parents: Vec<Rect> = grid.sheet.merges.iter().copied()
        .filter(|m| m.top == t.header_top && m.bottom == m.top && m.left > t.rect.left && m.right <= t.rect.right)
        .collect();
    let mut parents = parents;
    parents.sort_by_key(|m| m.left);
    let text = |r, c| grid.cell(r, c).and_then(|c| match &c.value { Some(Value::Text(t)) if !t.trim().is_empty() => Some(t.trim()), _ => None });
    let numeric = |r| grid.row_cells(r).iter().filter(|&&i| {
        let c = &grid.sheet.cells[i as usize];
        c.col > t.rect.left && c.col <= t.rect.right && matches!(c.value, Some(Value::Number(_)))
    }).count() >= 2;
    let cohort = |r| text(r, t.rect.left).is_some_and(|s| s.chars().count() <= 60 && !s.ends_with(':'));
    if parents.len() < 2 || parents[0].left != t.rect.left + 1
        || parents.last().is_none_or(|m| m.right != t.rect.right)
        || parents.windows(2).any(|w| w[0].right + 1 != w[1].left)
        || !cohort(body) || numeric(body) || !numeric(body + 1)
        || grid.row_cells(body).iter().any(|&i| {
            let c = &grid.sheet.cells[i as usize];
            c.col > t.rect.left && c.col <= t.rect.right && c.value.as_ref().is_some_and(|v| !display(v).is_empty())
        })
        || (t.rect.left + 1..=t.rect.right).any(|c| !(t.header_top + 1..body).any(|r| {
            grid.cell(r, c).or_else(|| grid.merge_at(r, c).and_then(|m| grid.cell(m.top, m.left)))
                .is_some_and(|cell| matches!(&cell.value, Some(Value::Text(s)) if !s.trim().is_empty()))
        }))
    { return vec![t.clone()]; }
    let mut cuts = Vec::new();
    let mut previous_cohort = text(body, t.rect.left);
    let mut previous_parent_row = t.header_top;
    for r in body + 2..t.rect.bottom {
        if !cohort(r) || !numeric(r - 1) || !numeric(r + 1) { continue; }
        if grid.row_cells(r).iter().any(|&i| {
            let c = &grid.sheet.cells[i as usize];
            c.col >= t.rect.left && c.col <= t.rect.right && c.value.as_ref().is_some_and(|v| !display(v).is_empty())
                && (c.col != t.rect.left && !parents.iter().any(|m| m.left == c.col) || !matches!(c.value, Some(Value::Text(_))))
        }) { continue; }
        let mut changed = false;
        let matched = parents.iter().all(|p| {
            let Some(m) = grid.merge_at(r, p.left) else { return false };
            let (Some(old), Some(new)) = (text(previous_parent_row, p.left), text(r, p.left)) else { return false };
            changed |= old != new;
            m.top == r && m.bottom == r && m.left == p.left && m.right == p.right
        });
        if matched && (changed || text(r, t.rect.left) != previous_cohort) {
            cuts.push(r);
            previous_cohort = text(r, t.rect.left);
            previous_parent_row = r;
        }
    }
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut part = t.clone();
    for row in cuts {
        part.rect.bottom = row - 1;
        out.push(part);
        part = DetectedTable { rect: Rect { top: row, ..t.rect }, header_top: row, header_rows: 1, confidence: t.confidence };
    }
    out.push(part);
    out
}

#[allow(clippy::too_many_arguments)]
fn build_table(
    grid: &Grid,
    lay: &SheetLayout,
    t: &DetectedTable,
    inherited: Option<&Table>,
    n: usize,
    sheet_hints: &OnceLock<HashMap<u32, ColumnHints>>,
    row_total: &HashMap<u32, RowTotal>,
    overrides: &Overrides,
    opts: &Options,
    owner: &mut [u8],
) -> Built {
    let sheet = grid.sheet;
    let cells = &sheet.cells;
    let mut r = t.rect;
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
    let (mut htop, body_top) = promote_header(&rows, cells, &lay.roles, t.header_top.clamp(r.top, r.bottom), t.header_rows, r.bottom);
    if body_top > htop && let Some(parent_row) = merged_parent_top(grid, r, htop, owner) {
        let old_top = r.top;
        r.top = r.top.min(parent_row);
        if r.top < old_top {
            let start = cells.partition_point(|c| c.row < r.top);
            let mut prefix: Vec<(u32, Vec<usize>)> = Vec::new();
            for (i, c) in cells.iter().enumerate().skip(start).take_while(|(_, c)| c.row < old_top) {
                if c.col < r.left || c.col > r.right { continue; }
                match prefix.last_mut() {
                    Some((row, idxs)) if *row == c.row => idxs.push(i),
                    _ => prefix.push((c.row, vec![i])),
                }
            }
            prefix.append(&mut rows);
            rows = prefix;
        }
        htop = parent_row;
    }
    let physical_nh = (body_top - htop) as usize;
    let mut nh = physical_nh;
    // Columns of the rect left and right of every cell of the header and body hold nothing but
    // a merged area reaching past the data (a full-row merge `A2:XFD2` over six columns, B90):
    // they are left out, so the table costs its cells, not its rows × the width of a merge.
    let (first_col, last_col) = rows
        .iter()
        .filter(|(row, _)| *row >= htop)
        .flat_map(|(_, idxs)| idxs.iter().map(|&i| cells[i].col))
        .fold((u32::MAX, 0), |(a, b), c| (a.min(c), b.max(c)));
    if first_col <= last_col && (first_col > r.left || last_col < r.right) {
        tr.add(T::EmptyColumnsRemoved, ((first_col - r.left) + (r.right - last_col)) as usize);
        (r.left, r.right) = (first_col, last_col);
    }
    let ncols = (r.right - r.left + 1) as usize;

    // Title rows inside the rect (above the header).
    let mut title: Option<(String, (u32, u32))> = None;
    let mut title_rows = 0;
    for (_, idxs) in rows.iter().filter(|(row, _)| *row < htop) {
        title_rows += 1;
        let numbers = numbering_row(cells, idxs);
        // A `Key: | … | Key: | value` line above the header: neither its keys nor the value after
        // each key are a title (`Number of Clients: | 5` made the title "5"); other text on the
        // line still is (`Stock D | Code: | 656839`).
        let is_key = |i: usize| matches!(&cells[i].value, Some(Value::Text(t)) if t.trim_end().ends_with([':', '：']));
        let filled_idx: Vec<usize> =
            idxs.iter().copied().filter(|&i| cells[i].value.as_ref().is_some_and(|v| !display(v).is_empty())).collect();
        let key_part: HashSet<usize> = filled_idx
            .iter()
            .enumerate()
            .filter(|&(k, &i)| is_key(i) || (k > 0 && is_key(filled_idx[k - 1])))
            .map(|(_, &i)| i)
            .collect();
        for &i in idxs {
            let key_line = key_part.contains(&i);
            owner[i] = OWN_NOTES;
            let mut it = note(i, "title");
            if is_nav_link(&it.text) {
                // "zur Inhaltsübersicht": a link back to the contents, never the title.
                it.kind = "other";
            } else if numbers {
                // A helper row of column numbers above the header (`5 | 6 | 7 | …`).
                it.kind = "other";
            } else if title.is_none()
                && !key_line
                && !it.text.is_empty()
                && !crate::layout::is_note_label(&it.text)
                // A key of a "Project Name: | …" line above the header is no title.
                && !it.text.trim_end().ends_with([':', '：'])
            {
                title = Some((it.text.clone(), (it.row, it.col)));
            }
            notes.push(it);
        }
    }
    // B72: a numbered caption row ("Quadro 1 | Contagem dos trabalhadores …") under other title
    // lines ("Ano: 2016") is the title, its short number joined with the text beside it.
    if let Some(c) = notes.iter().find(|x| x.kind == "title" && table_word(&x.text)) {
        title = Some(caption_title(c, notes.iter().filter(|x| x.kind == "title")));
    }
    tr.add(T::TitleRowsMoved, title_rows);

    // --- Header ----------------------------------------------------------------------------
    // Ruling lines drawn with text (`=====` rows, a `|` column) are not content (B38).
    let rule_text = |i: usize| matches!(&cells[i].value, Some(Value::Text(t)) if crate::layout::is_rule_text(t));
    let pipe = |i: usize| matches!(&cells[i].value, Some(Value::Text(t)) if matches!(t.trim(), "|" | "||" | "¦"));
    let mut head: Vec<Vec<Head>> = vec![vec![None; ncols]; nh];
    for (row, idxs) in rows.iter().filter(|(row, _)| *row >= htop && *row < body_top) {
        let all_rules = idxs.iter().all(|&i| rule_text(i) || cells[i].value.as_ref().is_none_or(|v| display(v).is_empty()));
        for &i in idxs {
            if pipe(i) || (all_rules && rule_text(i)) {
                owner[i] = OWN_NOTES;
                notes.push(note(i, "other"));
                continue;
            }
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
    // A changed cohort repeats its parent row only. Reuse the donor's actual child
    // source cells, keeping own parent text and typing the new complete header independently.
    let cohort = inherited.and_then(|_| grid.cell(htop, r.left)).and_then(|c| c.value.as_ref()).map(display);
    if let Some(donor) = inherited {
        let child_rows = donor.header_rows.saturating_sub(1) as usize;
        head.extend((0..child_rows).map(|_| vec![None; ncols]));
        nh = head.len();
        for col in &donor.columns {
            let Some(c) = col.source_col.filter(|c| *c >= r.left && *c <= r.right) else { continue };
            let k = (c - r.left) as usize;
            if c == r.left { head[0][k] = None; }
            for part in &col.original_header {
                if c == r.left || part.1.0 > donor.header_top {
                    let hr = part.1.0.saturating_sub(donor.header_top) as usize;
                    if hr < head.len() { head[hr][k] = Some(part.clone()); }
                }
            }
        }
        warnings.push(format!("child header cells inherited from {} with their original source coordinates", donor.id()));
    }
    // B84: month-end dates across a header that step back from December to January of the same
    // year (Jul … Dec 2001, Jan … Mar 2001) were typed without the year change: from the step on
    // they are read one year later, when that continues the monthly sequence.
    for row_heads in head.iter_mut() {
        if let Some((from, to)) = roll_over_header_dates(row_heads, grid) {
            warnings.push(format!(
                "header dates after {from} that went back to the start of the year are read as the next year (to {to})"
            ));
        }
    }
    // Parent labels centred over their children without a merge (B39): when blank parent cells
    // over labelled measure columns lie left of the row's first label, the labels sit in the
    // middle of their spans, and each such blank takes the nearest label (the left one on a tie).
    let measure = |hc: usize| {
        let col = r.left + hc as u32;
        let vals: Vec<bool> = rows
            .iter()
            .filter(|(row, _)| *row >= body_top)
            .take(5)
            .filter_map(|(_, idxs)| idxs.iter().find(|&&i| cells[i].col == col))
            .map(|&i| matches!(cells[i].value, Some(Value::Number(_))))
            .collect();
        !vals.is_empty() && vals.iter().all(|&b| b)
    };
    for hr in 0..nh.saturating_sub(1) {
        let labelled_below = |hc: usize| (hr + 1..nh).any(|k| head[k][hc].is_some());
        if (0..ncols).any(|hc| merged_in_header.contains(&(hr, hc))) {
            continue;
        }
        let labels: Vec<usize> = (0..ncols).filter(|&hc| head[hr][hc].is_some() && labelled_below(hc)).collect();
        let Some(&first) = labels.first() else { continue };
        let centred = (0..first).any(|hc| head[hr][hc].is_none() && labelled_below(hc) && measure(hc));
        if !centred {
            continue;
        }
        let same_parent = |a: usize, b: usize| hr == 0 || head[hr - 1][a] == head[hr - 1][b];
        let mut fill: Vec<(usize, usize)> = Vec::new();
        for (hc, cell) in head[hr].iter().enumerate() {
            if cell.is_some() || !labelled_below(hc) || !measure(hc) {
                continue;
            }
            let near = labels.iter().copied().filter(|&l| same_parent(l, hc)).min_by_key(|&l| (l.abs_diff(hc), l > hc));
            if let Some(l) = near {
                fill.push((hc, l));
            }
        }
        for (hc, l) in fill {
            head[hr][hc] = head[hr][l].clone();
            tr.add(T::HeaderCellsForwardFilled, 1);
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

    // A column-numbering header row ("1 2 3 …", so notes can say "Sp. 3–4") stays in
    // `original_header` but does not name the columns (B45: not "… / 1"). Unless the numbers sit
    // under one label over all of them: then they are that label's values (`Number of years since
    // landing` over `1 2 3 4 5 6`, HiTab 1220), not column numbers.
    let numbering_rows: Vec<bool> = (0..nh)
        .map(|hr| {
            if hr >= physical_nh { return false; }
            let vals: Vec<&Value> = rows
                .iter()
                .filter(|(row, _)| *row == htop + hr as u32)
                .flat_map(|(_, idxs)| idxs.iter().filter_map(|&i| cells[i].value.as_ref()))
                .filter(|v| !display(v).is_empty())
                .collect();
            crate::layout::column_numbering(vals.into_iter()) && !numbers_under_one_label(&head, hr)
        })
        .collect();
    // B70: the row label of a header row of series codes ("Dataset identifier code | MGSL | MGSF
    // …", ONS) labels that row, not the column below it (the periods): it stays in
    // `original_header` but does not name the column. The label must say code / identifier /
    // series, so "Country name | USA | GBR | FRA" keeps its name.
    let code_row_labels: HashSet<(usize, usize)> = (0..nh)
        .filter_map(|hr| {
            let filled: Vec<(usize, &str)> =
                head[hr].iter().enumerate().filter_map(|(hc, h)| h.as_ref().map(|h| (hc, h.0.as_str()))).collect();
            let (&(hc0, label), rest) = filled.split_first()?;
            let codes: HashSet<&str> = rest.iter().map(|(_, t)| *t).filter(|t| is_series_code(t)).collect();
            (rest.len() >= 3
                && codes.len() * 10 >= rest.len() * 9
                && !is_series_code(label)
                && label.split(|c: char| !c.is_alphanumeric()).any(|w| {
                    matches!(w.to_lowercase().as_str(), "code" | "codes" | "identifier" | "series" | "cdid" | "id" | "kennung" | "kennziffer" | "código" | "codice")
                }))
            .then_some((hr, hc0))
        })
        .collect();

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
    // The rect's own left edge: the codes are looked for in its first four columns.
    let sex = sex_blocks(&rows, cells, Rect { left: t.rect.left, ..r }, body_top);
    // B72: the caption of the next table ("Quadros 1.1 | Contagem dos prestadores …") as the last
    // row of this one, after a blank row and without numbers, is a loose title (attached to the
    // table below it in `process_sheet`).
    let next_caption: Option<u32> = rows.iter().rposition(|(row, _)| *row >= body_top).and_then(|k| {
        let (row, idxs) = &rows[k];
        let texts: Vec<&str> = idxs
            .iter()
            .filter_map(|&i| match &cells[i].value {
                Some(Value::Text(t)) if !t.trim().is_empty() => Some(t.as_str()),
                _ => None,
            })
            .collect();
        let gap = k > 0 && rows[k - 1].0 + 1 < *row && rows[k - 1].0 >= body_top;
        let numbers = idxs.iter().any(|&i| matches!(cells[i].value, Some(Value::Number(_))));
        (gap && !numbers && (1..=3).contains(&texts.len()) && table_word(texts[0])).then_some(*row)
    });
    let mut slots: Vec<Vec<Slot>> = vec![Vec::new(); ncols];
    // Inline group labels (output row, column, value): see the fill-down below.
    let mut inline_labels: Vec<(usize, usize, &Value)> = Vec::new();
    let mut src_rows: Vec<u32> = Vec::new();
    let mut row_types: Vec<RowType> = Vec::new();
    let mut row_group: Vec<Option<usize>> = Vec::new();
    let mut paths: Vec<Vec<String>> = Vec::new();
    // The source rows of each path's group labels, and the label cell of every group-header row.
    let mut paths_rows: Vec<Vec<u32>> = Vec::new();
    let mut group_cells: Vec<(usize, u32)> = Vec::new();
    // The current group path and the rows of its group headers; `run` collects consecutive
    // group-header rows (label, row) until the next data row places them in the path.
    let mut path: Vec<String> = Vec::new();
    let mut path_rows: Vec<u32> = Vec::new();
    // Indent level of each path entry and of each run entry (style indent, leading spaces).
    let mut path_levels: Vec<(u8, usize)> = Vec::new();
    let mut run: Vec<(String, u32, (u8, usize))> = Vec::new();
    let label_level = |i: usize| -> (u8, usize) {
        let indent = grid.sheet.formats.get(i).and_then(|f| f.unpack()).map_or(0, |f| f.indent);
        let spaces = match &cells[i].value {
            Some(Value::Text(t)) => t.chars().take_while(|&c| c == ' ' || c == '\u{a0}').count(),
            _ => 0,
        };
        (indent, spaces)
    };
    let mut cur_path: Option<usize> = None;
    // Data and total rows since the last section label (group run) was applied.
    let mut since_section = 0usize;
    // The last data row's outline label, source row and indent; the data row that heads the
    // sections indented below it (see the group path below).
    let mut prev_data: Option<(String, u32, (u8, usize))> = None;
    let mut anchor_row: Option<u32> = None;
    // Column of the group-header rows' labels; a group-header cell there on a data row is a group
    // label written on the group's first data row (it replaces the innermost group).
    let mut group_label_col: Option<u32> = None;
    let mut group_labels: Vec<String> = Vec::new();
    if let Some(cohort) = cohort {
        group_labels.push(cohort.clone());
        run.push((cohort, htop, (0, 0)));
        group_label_col = Some(r.left);
    }
    let mut removed_totals = Vec::new();
    let mut body_rows_seen = 0u32;
    // Text label columns of the records (text on most data rows): a "group header" row that fills
    // two or more of them like a record, only without values (a WDI country with no data), is a
    // record, not a group label (B44).
    // The outline below also takes a label column of a short table (2 or 3 data rows: `All
    // industries` over `Manufacturing` and `Nonmanufacturing`).
    let outline_col: Option<u32>;
    let label_cols: HashSet<u32> = {
        let mut text_rows: HashMap<u32, usize> = HashMap::new();
        let mut data_rows = 0usize;
        for (_, idxs) in rows.iter().filter(|(row, _)| *row >= body_top) {
            if !idxs.iter().any(|&i| lay.roles[i] == Role::Data) {
                continue;
            }
            data_rows += 1;
            for &i in idxs {
                if lay.roles[i] == Role::Data && matches!(&cells[i].value, Some(Value::Text(t)) if !t.trim().is_empty()) {
                    *text_rows.entry(cells[i].col).or_default() += 1;
                }
            }
        }
        outline_col = text_rows.iter().filter(|&(_, &n)| data_rows >= 2 && 2 * n > data_rows).map(|(&c, _)| c).min();
        text_rows.into_iter().filter(|&(_, n)| data_rows >= 4 && 2 * n > data_rows).map(|(c, _)| c).collect()
    };
    // Section rows the layout read as data (B-labels): a label alone on its row, in the label
    // column, that heads the rows below it - they are indented deeper (`Public spaces` over
    // `  Unwanted sexual behaviour …`), or it is set apart from them by bold, capitals or a merge
    // across the row (`2015` over the components of that year). It becomes a group header.
    let section_rows: HashSet<u32> = section_rows(grid, &rows, body_top, outline_col, &lay.roles);
    let unit_rows: HashMap<u32, String> = unit_section_rows(grid, &rows, body_top, outline_col);
    let mut key_cols: HashSet<usize> = HashSet::new();
    // Outline of the row labels (B-public): the label column's cells with their indent (cell style)
    // and leading spaces, in order, for the `<label>_parent` column built after the loop.
    // Only for a real label column (text on most data rows).
    let label_col = label_cols.iter().copied().min();
    let mut outline: Vec<OutlineRow> = Vec::new();
    let outline_row = |idxs: &[usize], out: Option<usize>, group: bool| -> Option<OutlineRow> {
        let i = idxs.iter().copied().find(|&i| Some(cells[i].col) == outline_col)?;
        // A label split across M/F/T rows is one reconstructed label. Fragment whitespace
        // (Balanço Social A349:A351) is not indentation of a child record.
        let value = sex.as_ref().and_then(|sb| sb.labels.get(&(cells[i].row, cells[i].col))).or(cells[i].value.as_ref());
        let Some(Value::Text(t)) = value else { return None };
        let label = t.trim();
        // A missing-value token (`no data`, `n/a`) in the label column is no label.
        if label.is_empty() || crate::values::table_missing_token(label).is_some() {
            return None;
        }
        let fmt = grid.sheet.formats.get(i).and_then(|f| f.unpack());
        let (indent, bold) = fmt.map_or((0, false), |f| (f.indent, f.bold));
        // Centered/right-aligned labels use spaces for visual padding, not nesting.
        let spaces = if fmt.is_some_and(|f| matches!(f.align, crate::model::Align::Center | crate::model::Align::Right)) {
            0
        } else {
            t.chars().take_while(|&c| c == ' ' || c == '\u{a0}').count().min(255) as u8
        };
        Some(OutlineRow { indent, spaces, bold, label: label.to_string(), out, group })
    };
    // Block of each output row: the rows between two total rows (removed or kept).
    let mut block_of: Vec<u32> = Vec::new();
    let mut block = 0u32;
    // The header's labels, to tell a repeated header row from a data row with header cells.
    let header_texts: HashSet<String> = rows
        .iter()
        .filter(|(row, _)| *row >= htop && *row < body_top)
        .flat_map(|(_, idxs)| idxs.iter().filter_map(|&i| cells[i].value.as_ref().map(display)))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let body_kind = body_kinds(&rows, cells, body_top);
    for (row, idxs) in rows.iter().filter(|(row, _)| *row >= body_top) {
        body_rows_seen += 1;
        if Some(*row) == next_caption {
            tr.add(T::NoteRowsMoved, 1);
            for &i in idxs {
                owner[i] = OWN_NOTES;
                // No table yet: `process_sheet` attaches it to the table below.
                notes.push(NoteItem { table: None, ..note(i, "title") });
            }
            continue;
        }
        let blank = |i: usize| cells[i].value.as_ref().is_none_or(|v| display(v).is_empty());
        if idxs.iter().any(|&i| rule_text(i)) && idxs.iter().all(|&i| rule_text(i) || blank(i)) {
            for &i in idxs {
                owner[i] = OWN_NOTES;
                if !blank(i) {
                    notes.push(note(i, "other"));
                }
            }
            continue;
        }
        let idxs_kept: Vec<usize>;
        let idxs = if idxs.iter().any(|&i| pipe(i)) {
            for &i in idxs.iter().filter(|&&i| pipe(i)) {
                owner[i] = OWN_NOTES;
                notes.push(note(i, "other"));
            }
            idxs_kept = idxs.iter().copied().filter(|&i| !pipe(i)).collect();
            &idxs_kept
        } else {
            idxs
        };
        let mut kind = row_kind(idxs.iter().map(|&i| lay.roles[i]));
        // A body row with header cells is a repeated header only when it reads like one: a row
        // with measured numbers in it that does not repeat the header's labels is data (Enron
        // "Tuition and Fees | 98834698 | …", a first trade "1 | 2000-06-07 | Vitro | …" under a
        // subtotal line), not Notes.
        if kind == RowKind::RepeatedHeader && data_like_row(cells, idxs, &header_texts, &body_kind) {
            kind = RowKind::Data;
        }
        if kind == RowKind::Group {
            let filled: Vec<usize> = idxs.iter().copied().filter(|&i| !blank(i)).collect();
            if filled.len() >= 2 && filled.iter().all(|&i| label_cols.contains(&cells[i].col)) {
                kind = RowKind::Data;
            }
        }
        if matches!(kind, RowKind::Data | RowKind::Note | RowKind::RepeatedHeader) && section_rows.contains(row) {
            kind = RowKind::Group;
        }
        if unit_rows.contains_key(row) && !matches!(kind, RowKind::Total) {
            kind = RowKind::Group;
        }
        // A total-worded section label (no number on its row): see `section_rows`.
        if kind == RowKind::Total && section_rows.contains(row) {
            kind = RowKind::Group;
        }
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
        // B72: rows of a sex breakdown block are data, their both-sexes row a subtotal.
        let sex_row = sex.as_ref().and_then(|sb| sb.rows.get(row).copied());
        if let Some((k, _)) = sex_row
            && matches!(kind, RowKind::Data | RowKind::Total | RowKind::Group | RowKind::NonSum(_))
        {
            kind = k;
        }
        // A "group" row whose label is a missing-value token (` no data`): no group, a note.
        if kind == RowKind::Group
            && idxs.iter().any(|&i| !blank(i))
            && idxs.iter().filter(|&&i| !blank(i)).all(|&i| {
                matches!(&cells[i].value, Some(Value::Text(t)) if crate::values::table_missing_token(t.trim()).is_some())
            })
        {
            for &i in idxs {
                owner[i] = OWN_NOTES;
                if !blank(i) {
                    notes.push(note(i, "other"));
                }
            }
            continue;
        }
        match kind {
            RowKind::Group => {
                let label = idxs
                    .iter()
                    .filter_map(|&i| cells[i].value.as_ref())
                    .map(display)
                    .find(|s| !s.is_empty())
                    .unwrap_or_default();
                let label_cell = idxs.iter().copied().find(|&i| cells[i].value.as_ref().is_some_and(|v| !display(v).is_empty()));
                for &i in idxs {
                    // Other content of a group row ("III. Discount | | Determined by adding
                    // points …") is not part of the label: it goes to Notes, not nowhere.
                    if Some(i) != label_cell && cells[i].value.as_ref().is_some_and(|v| !display(v).is_empty()) {
                        owner[i] = OWN_NOTES;
                        notes.push(note(i, "other"));
                    } else {
                        owner[i] = OWN_TABLE;
                    }
                }
                if let Some(i) = label_cell {
                    if !unit_rows.contains_key(row) {
                        group_label_col = Some(cells[i].col);
                    }
                    group_cells.push((i, *row));
                }
                tr.add(T::GroupHeaderRowsToColumn, 1);
                group_labels.push(label.clone());
                run.push((label, *row, label_cell.map_or((0, 0), label_level)));
                outline.extend(outline_row(idxs, None, true));
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
            sex_row.and_then(|x| x.1).or_else(|| row_total.get(row).map(|t| t.level)).or_else(|| {
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
            outline.extend(outline_row(idxs, None, false));
            tr.add(T::TotalRowsRemoved, 1);
            removed_totals.push((*row, rt, idxs.clone()));
            continue;
        }
        if rt != RowType::Data {
            tr.add(T::TotalRowsKept, 1);
        }
        let inline_label = idxs.iter().copied().find(|&i| {
            rt == RowType::Data
                && sex_row.is_none()
                && run.is_empty()
                && lay.roles[i] == Role::GroupHeader
                && Some(cells[i].col) == group_label_col
                && cells[i].value.as_ref().is_some_and(|v| !display(v).is_empty())
        });
        if let Some(i) = inline_label {
            let label = cells[i].value.as_ref().map(display).unwrap_or_default();
            tr.add(T::GroupHeaderRowsToColumn, 1);
            group_labels.push(label.clone());
            run.push((label, *row, label_level(i)));
        }
        // This row's label in the outline column and its indent (for the anchor below).
        let own_label = idxs.iter().copied().find(|&i| {
            Some(cells[i].col) == outline_col && matches!(&cells[i].value, Some(Value::Text(t)) if !t.trim().is_empty())
        });
        if !run.is_empty() {
            // A data row right above sections indented deeper than it (`Recent immigrant owners`
            // over `  From China`, `  From India`, StatCan 1856) heads them: it is their parent.
            if let Some((label, arow, alevel)) = prev_data.take()
                && run.first().is_some_and(|x| x.2 > alevel)
            {
                while path_levels.last().is_some_and(|&l| l >= alevel) {
                    path.pop();
                    path_rows.pop();
                    path_levels.pop();
                }
                path.push(label);
                path_rows.push(arow);
                path_levels.push(alevel);
                anchor_row = Some(arow);
            }
        } else if let Some(arow) = anchor_row
            && let Some(a) = path_rows.iter().position(|&h| h == arow)
            && own_label.is_some_and(|i| label_level(i) <= path_levels[a])
        {
            // a row back at the anchor's indent (or left of it) leaves it and its sections
            path.truncate(a);
            path_rows.truncate(a);
            path_levels.truncate(a);
            anchor_row = None;
            paths.push(path.clone());
            paths_rows.push(path_rows.clone());
            cur_path = Some(paths.len() - 1);
        }
        if !run.is_empty() {
            // Group labels at several indents nest by their indent (`Public spaces` > `  Unwanted
            // sexual behaviour …`, then `Online` replaces both); at one indent a run of group
            // rows replaces as many innermost groups.
            let mut levels = run.iter().map(|x| x.2).chain(path_levels.iter().copied());
            let first = levels.next();
            let indented = levels.any(|l| Some(l) != first);
            if indented {
                // Consecutive group rows at one indent (`Segment Information:` over `Net
                // sales:`, no rows between) nest like a run without indents.
                let mut fresh = 0usize; // entries on top of the path pushed by this run
                for (label, row, level) in run.drain(..) {
                    while path_levels.last().is_some_and(|&l| if fresh > 0 { l > level } else { l >= level }) {
                        path.pop();
                        path_rows.pop();
                        path_levels.pop();
                        fresh = fresh.saturating_sub(1);
                    }
                    path.push(label);
                    path_rows.push(row);
                    path_levels.push(level);
                    fresh += 1;
                }
            } else {
                let keep = path.len().saturating_sub(run.len());
                path.truncate(keep);
                path_rows.truncate(keep);
                path_levels.truncate(keep);
                for (label, row, level) in run.drain(..) {
                    path.push(label);
                    path_rows.push(row);
                    path_levels.push(level);
                }
            }
            paths.push(path.clone());
            paths_rows.push(path_rows.clone());
            cur_path = Some(paths.len() - 1);
            since_section = 0;
        }
        // A kept total row belongs only to the groups that contain its whole source: a subtotal
        // to its group, a grand total (or a total over several groups) to none of them. A source
        // range reaching into the header (a year header read as a number, B17) starts at the body.
        // A total that names its innermost group (`Total capital DEL` under `Capital DEL`) closes it.
        let names_group = path.last().is_some_and(|g| {
            let g = norm_key(g);
            g.chars().filter(|c| c.is_alphanumeric()).count() >= 3
                && idxs.iter().any(|&i| matches!(&cells[i].value, Some(Value::Text(t)) if norm_key(t).contains(g.as_str())))
        });
        let depth = match (level, row_total.get(row).and_then(|t| t.top)) {
            _ if matches!(rt, RowType::Data | RowType::Average | RowType::OfWhich) => path.len(),
            _ if names_group => path.len(),
            // A total right under its section label (`Constant 2012 $` over `Total R&D, all
            // performers` and its parts) heads that section, whatever rows above it add up to.
            _ if since_section == 0 && !path.is_empty() => path.len(),
            // A group named like a total ("Total drug violations") names its total rows, a grand
            // total over the groups above too.
            _ if path.last().is_some_and(|l| totals::total_keyword(l).is_some()) => path.len(),
            (Some(TotalLevel::GrandTotal), _) => 0,
            (_, Some(top)) => path_rows.iter().take_while(|&&h| h <= top.max(body_top)).count(),
            _ => path.len(),
        };
        row_group.push(match depth {
            0 => None,
            d if d < path.len() => {
                paths.push(path[..d].to_vec());
                paths_rows.push(path_rows[..d].to_vec());
                Some(paths.len() - 1)
            }
            _ => cur_path,
        });
        let mut row_slots: Vec<Slot> = vec![Slot::Empty; ncols];
        for &i in idxs {
            owner[i] = OWN_TABLE;
            let c = &cells[i];
            let k = (c.col - r.left) as usize;
            if Some(i) == inline_label {
                if let Some(v) = &c.value {
                    inline_labels.push((src_rows.len(), k, v));
                }
                continue;
            }
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
        if let Some(sb) = &sex {
            for (k, s) in row_slots.iter_mut().enumerate() {
                if let Some(v) = sb.labels.get(&(*row, r.left + k as u32)) {
                    *s = Slot::Val(v);
                }
            }
        }
        for (k, s) in row_slots.into_iter().enumerate() {
            slots[k].push(s);
        }
        src_rows.push(*row);
        // A kept total row has its parent too ("All federal R&D expenditures" under "All R&D").
        let with_parent = matches!(rt, RowType::Data | RowType::Total | RowType::Subtotal);
        outline.extend(outline_row(idxs, with_parent.then(|| src_rows.len() - 1), false));
        row_types.push(rt);
        since_section += 1;
        prev_data = match (rt, own_label) {
            (RowType::Data, Some(i)) => Some((cells[i].value.as_ref().map(display).unwrap_or_default().trim().to_string(), *row, label_level(i))),
            _ => None,
        };
        block_of.push(if rt == RowType::Data { block } else { u32::MAX });
        if rt != RowType::Data {
            block += 1;
        }
    }
    if r.bottom + 1 > body_top {
        tr.add(T::BlankRowsRemoved, (r.bottom + 1 - body_top).saturating_sub(body_rows_seen) as usize);
    }

    // An inline group label also stays in its own column when that column holds row labels of
    // other rows: left empty there, the fill-down below wrote the row above's label in its place
    // ("I. Short-term Loans" on the row of "II. Medium and Long-term Loans"). A column of nothing
    // but group labels stays out of the table, as before.
    for &(j, k, v) in &inline_labels {
        if slots[k].iter().enumerate().any(|(jj, s)| jj != j && matches!(s, Slot::Val(_))) {
            slots[k][j] = Slot::Val(v);
        }
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
    // Names first (in column order), then every column typed (on parallel threads for a large
    // table: each column's typing reads only its own values and the sheet's shared hints, B120),
    // then the columns assembled in order.
    // Per column: its header parts (text, cell) and its name.
    // B-public: the first section label of the body written in the header's last row, in the
    // label column (`Region of ancestry` beside the unit row's `percent`, or one label merged
    // across the row, `Current $billions`), when the later sections are group-header rows: it
    // heads the rows above the first group header (group column), not a column.
    let section_in_header: Option<(usize, usize, bool, String)> = (|| {
        let lc = label_col?;
        if nh < 2 || group_labels.is_empty() || row_group.first() != Some(&None) || lc < r.left || lc > r.right {
            return None;
        }
        let (hr, k0) = (nh - 1, (lc - r.left) as usize);
        let (text, at) = head[hr][k0].as_ref()?;
        if *at != (htop + hr as u32, lc) {
            return None;
        }
        let others: Vec<&(String, (u32, u32))> =
            head[hr].iter().enumerate().filter(|&(k, _)| k != k0).filter_map(|(_, h)| h.as_ref()).collect();
        let whole = !others.is_empty() && others.iter().all(|h| h.1 == *at);
        let units = !others.is_empty() && others.iter().all(|h| h.1.0 == at.0 && h.1 != *at && crate::layout::is_unit_text(&h.0));
        (whole || units).then(|| (hr, k0, whole, text.clone()))
    })();
    if let Some((_, _, _, label)) = &section_in_header {
        paths.push(vec![label.clone()]);
        paths_rows.push(Vec::new());
        let idx = paths.len() - 1;
        for (j, g) in row_group.iter_mut().enumerate() {
            if g.is_some() {
                break;
            }
            // A total of the section's rows (not a grand total) belongs to it too.
            let section_total = matches!(row_types[j], RowType::Total | RowType::Subtotal)
                && row_total.get(&src_rows[j]).is_some_and(|t| t.level != TotalLevel::GrandTotal);
            if row_types[j] == RowType::Data || section_total {
                *g = Some(idx);
            }
        }
        group_labels.push(label.clone());
        tr.add(T::GroupHeaderRowsToColumn, 1);
    }
    // A group label that heads no row of the table (only removed totals or nothing below it,
    // `Subtotal` alone on its row) is not lost: it goes to Notes.
    let used: HashSet<u32> = row_group.iter().flatten().flat_map(|&g| paths_rows[g].iter().copied()).collect();
    for &(i, row) in &group_cells {
        if !used.contains(&row) {
            owner[i] = OWN_NOTES;
            notes.push(note(i, "other"));
        }
    }
    let mut heads: Vec<(Vec<HeaderPart>, String)> = Vec::with_capacity(ncols);
    for k in 0..ncols {
        let col = r.left + k as u32;
        let mut parts: Vec<(String, (u32, u32))> = Vec::new();
        let mut named: Vec<(String, (u32, u32))> = Vec::new();
        let section = |hr: usize| section_in_header.as_ref().is_some_and(|s| s.0 == hr && (s.1 == k || s.2));
        for (hr, h) in head.iter().enumerate().filter(|&(hr, _)| !section(hr)).filter_map(|(hr, row)| row[k].as_ref().map(|h| (hr, h))) {
            if parts.last().is_none_or(|p| p.0 != h.0) {
                parts.push(h.clone());
            }
            if !numbering_rows[hr] && !code_row_labels.contains(&(hr, k)) && named.last().is_none_or(|p| p.0 != h.0) {
                named.push(h.clone());
            }
        }
        let raw_name = parts.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join(" / ");
        let mut name = clean_name(grid, &named);
        if name != raw_name {
            tr.add(T::HeadersCleaned, 1);
        }
        if name.chars().count() > MAX_HEADER_LEN {
            name = name.chars().take(MAX_HEADER_LEN).collect();
        }
        let label = if !name.is_empty() {
            name.clone()
        } else if sex.as_ref().is_some_and(|sb| sb.col == col) {
            "sex".to_string()
        } else {
            format!("col_{}", col_letter(col))
        };
        heads.push((parts, label));
    }
    parent_names_for_duplicates(&head, &mut heads, grid, &mut tr);
    let type_one = |k: usize| -> (Typed, Transforms) {
        let col = r.left + k as u32;
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
        let mut tr = Transforms::default();
        let typed = type_column(&heads[k].1, col, &slots[k], &src_rows, first, &mut refine, &mut tr);
        (typed, tr)
    };
    let t_typing = std::time::Instant::now();
    let typed_cols: Vec<(Typed, Transforms)> = in_parallel(ncols, ncols * src_rows.len() >= PARALLEL_TYPING_MIN_VALUES, type_one);
    super::typing::add_typing_time(t_typing.elapsed());
    let mut data_cols: Vec<Column> = Vec::new();
    let mut col_warnings = Vec::new();
    for (k, ((parts, label), (typed, col_tr))) in heads.into_iter().zip(typed_cols).enumerate() {
        let col = r.left + k as u32;
        tr.merge(&col_tr);
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

    // B-public: the ancestors of each row in an outline of row labels (`Mood disorder` under
    // `Diagnosed mood or anxiety disorder`, both with values; `Ontario` under a bold `Canada`):
    // `<label>_parent` (the nearest), `<label>_parent_2` (its parent), …, right after the label
    // column.
    if let Some(paths) = outline_paths(&outline, src_rows.len())
        && let Some(at) = data_cols.iter().position(|c| c.source_col.is_some() && c.source_col == outline_col)
    {
        let base = data_cols[at].name.clone();
        let unnamed = base.strip_prefix("col_").is_some_and(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_uppercase()));
        let depth = paths.iter().map(Vec::len).max().unwrap_or(0);
        for d in 1..=depth {
            let stem = if unnamed { "parent".to_string() } else { format!("{base}_parent") };
            let name = if d == 1 { stem } else { format!("{stem}_{d}") };
            let values: Vec<Out> = paths
                .iter()
                .map(|p| match p.len().checked_sub(d) {
                    Some(k) => Out::Text(fit(p[k].clone(), &mut tr)),
                    None => Out::Null,
                })
                .collect();
            tr.add(T::ParentColumnsAdded, 1);
            data_cols.insert(
                at + d,
                Column {
                    name,
                    role: ColRole::Data,
                    source_col: None,
                    original_header: Vec::new(),
                    dtype: Dtype::Text,
                    unit: None,
                    currency: None,
                    decimal: None,
                    date_order: None,
                    ambiguous: 0,
                    footnote_marks: Vec::new(),
                    values,
                },
            );
        }
    }

    // Group column from group-header rows: the innermost group; the groups around it in
    // `group_parent`, `group_parent_2`, … (like a row label's `_parent` columns), so every level
    // is a whole label to filter on.
    let mut group_cols: Vec<Column> = Vec::new();
    if !group_labels.is_empty() {
        let refs: Vec<&str> = group_labels.iter().map(String::as_str).collect();
        let prefix = group_prefix(&refs);
        let strip = |l: &str| -> String {
            match &prefix {
                Some(_) => l.split_once(':').map(|(_, rest)| rest.trim().to_string()).unwrap_or_else(|| l.to_string()),
                None => l.to_string(),
            }
        };
        let name = prefix.clone().unwrap_or_else(|| "group".to_string());
        let used: Vec<&Vec<String>> = row_group.iter().flatten().map(|&i| &paths[i]).collect();
        let depth = used.iter().map(|p| p.len()).max().unwrap_or(1).clamp(1, MAX_OUTLINE_DEPTH + 1);
        for d in 0..depth {
            let values: Vec<Out> = row_group
                .iter()
                .map(|g| match g.map(|i| &paths[i]).and_then(|p| p.len().checked_sub(d + 1).map(|k| &p[k])) {
                    Some(l) => Out::Text(fit(strip(l), &mut tr)),
                    None => Out::Null,
                })
                .collect();
            let dtype = if values.iter().all(|v| *v == Out::Null) { Dtype::Empty } else { Dtype::Text };
            let (cname, role) = match d {
                0 => (name.clone(), ColRole::Group),
                1 => (format!("{name}_parent"), ColRole::Data),
                _ => (format!("{name}_parent_{d}"), ColRole::Data),
            };
            group_cols.push(Column {
                name: cname,
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
            });
        }
    }

    // Unique names: reserved columns claim theirs first.
    let mut seen = UniqueNames::taken(std::iter::once(SRC_ROW).chain(opts.keep_totals.then_some(ROW_TYPE)));
    let mut columns: Vec<Column> = Vec::new();
    for mut c in group_cols.into_iter().chain(data_cols) {
        c.name = seen.make(std::mem::take(&mut c.name), &mut tr);
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

    if nh == 0 {
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
            rect: Rect { top: t.rect.top.min(htop), ..t.rect },
            header_top: htop,
            // The header rows used (promote_header may move them below the layout's).
            header_rows: physical_nh as u32,
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

/// Repeated numeric parameter captions scope child tables. An explicit merged caption gives
/// its width; an unmerged caption needs the same pair (or more) of merged child titles directly
/// below every occurrence. Keep every title/note string and coordinate exact.
fn shared_parameter_captions(grid: &Grid, tables: &mut [Table], notes: &[NoteItem]) {
    fn parameter_key(text: &str) -> Option<String> {
        let (key, value) = text.split_once('=')?;
        let key = key.trim();
        (!key.is_empty() && key.chars().count() <= 32
            && key.chars().all(|c| c.is_alphabetic() || c.is_whitespace())
            && value.trim().starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+')
            && text.chars().count() <= 120).then(|| key.to_lowercase())
    }
    type CaptionSpan = (String, u32, u32);
    let mut groups: HashMap<CaptionSpan, Vec<(&NoteItem, u32)>> = HashMap::new();
    let mut unmerged: HashMap<(String, u32), Vec<&NoteItem>> = HashMap::new();
    let candidates: Vec<_> = notes.iter().filter(|n| matches!(n.kind, "title" | "other"))
        .filter_map(|n| parameter_key(&n.text).map(|key| (key, n))).collect();
    for (key, note) in &candidates {
        if let Some(merge) = grid.sheet.merges.iter().find(|m| m.top == note.row && m.bottom == note.row && m.left == note.col && m.right > m.left) {
            groups.entry((key.clone(), merge.left, merge.right)).or_default().push((note, merge.right));
        } else {
            unmerged.entry((key.clone(), note.col)).or_default().push(note);
        }
    }
    for ((key, left), captions) in unmerged.into_iter().filter(|(_, v)| v.len() >= 2) {
        let mut layout: Option<Vec<(u32, u32, String)>> = None;
        let valid = captions.iter().all(|caption| {
            // A lateral section with the same parameter owns its own child titles.
            let limit = grid.row_cells(caption.row).iter().map(|&i| &grid.sheet.cells[i as usize])
                .filter(|c| c.col > left && matches!(c.value.as_ref(), Some(Value::Text(text)) if parameter_key(text).as_ref() == Some(&key)))
                .map(|c| c.col).min().unwrap_or(u32::MAX);
            let mut merges: Vec<_> = grid.sheet.merges.iter().filter(|m| m.top == caption.row + 1
                && m.bottom == m.top && m.left >= left && m.right < limit && m.right > m.left).collect();
            merges.sort_by_key(|m| m.left);
            let mut child = Vec::new();
            let mut next_left = left;
            for m in merges {
                if m.left > next_left.saturating_add(2) || (child.is_empty() && m.left != left) { break; }
                let Some(Value::Text(label)) = grid.cell(m.top, m.left).and_then(|c| c.value.as_ref()) else { return false };
                if label.trim().is_empty() { return false; }
                child.push((m.left, m.right, label.clone()));
                next_left = m.right.saturating_add(1);
            }
            if child.len() < 2 { return false; }
            let right = child.last().unwrap().1;
            if grid.row_cells(caption.row).iter().map(|&i| &grid.sheet.cells[i as usize]).any(|c| c.col > left && c.col <= right
                && (c.value.is_some() || c.formula.is_some())) { return false; }
            match &layout {
                Some(first) => *first == child,
                None => { layout = Some(child); true }
            }
        });
        if valid && let Some(layout) = layout {
            let right = layout.last().unwrap().1;
            groups.entry((key, left, right)).or_default().extend(captions.into_iter().map(|n| (n, right)));
        }
    }
    if !groups.values().any(|v| v.len() >= 2) { return; }
    // Even a parameter consumed by a differently shaped table is a boundary. It need not
    // qualify for propagation, or share the earlier caption's inferred width.
    let boundaries: Vec<_> = grid.sheet.cells.iter().filter_map(|c| {
        let Value::Text(text) = c.value.as_ref()? else { return None };
        parameter_key(text).map(|key| (key, c.row, c.col))
    }).collect();
    for ((key, left, _), captions) in groups.iter_mut().filter(|(_, v)| v.len() >= 2) {
        captions.sort_by_key(|(n, _)| n.row);
        for (caption, right) in captions.iter() {
            let end = boundaries.iter().filter(|(k, row, col)| k == key && *col == *left && *row > caption.row)
                .map(|(_, row, _)| *row).min().unwrap_or(u32::MAX);
            for t in tables.iter_mut().filter(|t| t.rect.top > caption.row && t.rect.bottom < end && t.rect.left >= caption.col && t.rect.right <= *right) {
                let pos = (caption.row, caption.col);
                if t.title.as_ref().is_none_or(|(_, at)| *at != pos) && !t.notes.iter().any(|(_, at)| *at == pos) {
                    t.notes.push((caption.text.clone(), pos));
                    t.notes.sort_by_key(|(_, at)| *at);
                }
            }
        }
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

/// `name` without the " (2)" that makes a duplicate unique.
fn name_base(name: &str) -> &str {
    name.rsplit_once(" (").filter(|(_, t)| t.trim_end_matches(')').bytes().all(|b| b.is_ascii_digit())).map_or(name, |(h, _)| h)
}

/// A column name that does not say what the column holds: `col_F`, or a phrase repeated over the
/// block's columns ("Mrd. EUR", "Messzahl (1991 = 100) (2)").
fn weak_name(name: &str, repeated: &HashSet<&str>) -> bool {
    name.strip_prefix("col_").is_some_and(|l| l.bytes().all(|b| b.is_ascii_uppercase())) || repeated.contains(name_base(name))
}

/// Names (without the duplicate suffix) used by three or more source columns of `t` (a unit row).
/// Clock times and dates are header values, not units: the same times on every day of a week
/// ("09:00:00" three times) still name their columns.
fn repeated_names(t: &Table) -> HashSet<&str> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for c in t.columns.iter().filter(|c| c.source_col.is_some()) {
        *seen.entry(name_base(&c.name)).or_default() += 1;
    }
    let moment = |s: &str| {
        let b = s.as_bytes();
        (b.len() == 8 && b[2] == b':' && b[5] == b':') || (b.len() >= 10 && b[4] == b'-' && b[7] == b'-' && b[..4].iter().all(u8::is_ascii_digit))
    };
    seen.into_iter().filter(|&(s, n)| n >= 3 && !moment(s)).map(|(s, _)| s).collect()
}

/// B86: stacked blocks of one statistical table (levels, then "% change on the previous year",
/// then "% of GDP" or an index) where only the first block has a header: a block below whose
/// columns are unnamed (`col_D`, or a unit repeated over them) takes the header of the nearest
/// block above with the same source columns, each name followed by the column's own unit or the
/// block's phrase or short title (`Finanzierungs-saldo / % des Bruttoinlandsprodukts`). A block
/// that borrowed passes on the header it borrowed, not its phrase. A table warning says where the
/// names come from.
fn borrow_headers(tables: &mut [Table]) {
    // Per table: its header by source column (own strong names, or what it borrowed).
    let mut headers: Vec<Option<HashMap<u32, String>>> = Vec::with_capacity(tables.len());
    for low in 0..tables.len() {
        let t = &tables[low];
        let data: Vec<usize> =
            (0..t.columns.len()).filter(|&i| t.columns[i].source_col.is_some() && t.columns[i].role == ColRole::Data).collect();
        let rep = repeated_names(t);
        let strong: HashMap<u32, String> = data
            .iter()
            .map(|&i| &t.columns[i])
            .filter(|c| !weak_name(&c.name, &rep))
            .filter_map(|c| Some((c.source_col?, c.name.clone())))
            .collect();
        let weak = data.len() - strong.len();
        // Unnamed: (nearly) every column, or all but one (a unit written over the first one).
        if data.len() < 3 || (weak * 10 < data.len() * 8 && weak + 1 < data.len()) {
            headers.push((!strong.is_empty()).then_some(strong));
            continue;
        }
        let cols: Vec<u32> = data.iter().filter_map(|&i| t.columns[i].source_col).collect();
        // The nearest block above on the same sheet with a header for (nearly) all these columns.
        let donor = (0..low).rev().find(|&up| {
            let u = &tables[up];
            u.source_sheet == t.source_sheet
                && u.rect.bottom < t.rect.top
                && headers[up].as_ref().is_some_and(|h| cols.iter().filter(|c| h.contains_key(c)).count() * 10 >= cols.len() * 8)
        });
        let Some(up) = donor else {
            headers.push((!strong.is_empty()).then_some(strong));
            continue;
        };
        let header = headers[up].clone().unwrap_or_default();
        // Each column's phrase: its own name unless it is a column letter, else the block's one
        // phrase (a unit centred over the block) or its short title.
        // A column's own phrase is kept when it is a unit ("Mrd. EUR", "%", "Index (2020 = 100)",
        // "1 000 Personen"); other names of a headerless block are often its first labels or
        // footnote letters ("Belgio", "Sport", "z"). One phrase written once over the block
        // ("% des Bruttoinlandsprodukts") is the block's phrase.
        let unit = |n: &str| {
            let l = n.to_lowercase();
            n.contains('%')
                || n.bytes().any(|b| b.is_ascii_digit())
                || l.split(|c: char| !c.is_alphanumeric()).any(|w| {
                    matches!(
                        w,
                        "eur" | "usd" | "chf" | "gbp" | "pln" | "czk" | "sek" | "nok" | "dkk" | "huf" | "rsd" | "mrd" | "mio" | "tsd"
                            | "euro" | "index" | "indeks" | "indice" | "prozent" | "percent" | "personen" | "anzahl" | "tonnen"
                            | "kwh" | "gwh" | "twh" | "mwh"
                    )
                })
        };
        let names: Vec<Option<String>> = data
            .iter()
            .map(|&i| Some(name_base(&t.columns[i].name).to_string()).filter(|n| !n.starts_with("col_")))
            .collect();
        let own: Vec<Option<String>> = names.iter().map(|n| n.clone().filter(|n| unit(n))).collect();
        let phrases: HashSet<&String> = names.iter().flatten().collect();
        let once = names.iter().flatten().count() == 1;
        let block = match phrases.len() {
            1 if once || phrases.iter().all(|p| unit(p)) => phrases.into_iter().next().cloned(),
            0 => t.title.as_ref().map(|x| x.0.clone()).filter(|x| x.chars().count() <= 60),
            _ => None,
        };
        let years = |c: &Column| {
            c.values.iter().all(|v| match v {
                Out::Num(x) => x.fract() == 0.0 && (1900.0..=2100.0).contains(x),
                Out::Null => true,
                _ => false,
            })
        };
        let names: Vec<Option<String>> = data
            .iter()
            .zip(&own)
            .map(|(&i, own)| {
                let c = &t.columns[i];
                let base = header.get(&c.source_col?)?;
                let numeric = !matches!(c.dtype, Dtype::Text | Dtype::Empty) && !years(c);
                Some(match own.as_ref().or(block.as_ref()) {
                    Some(p) if numeric && !base.ends_with(&format!("/ {p}")) => format!("{base} / {p}"),
                    _ => base.clone(),
                })
            })
            .collect();
        let donor_id = tables[up].id();
        let t = &mut tables[low];
        let keep: Vec<&str> = t.columns.iter().enumerate().filter(|(i, _)| !data.contains(i)).map(|(_, c)| c.name.as_str()).collect();
        let mut seen = UniqueNames::taken(keep);
        let mut tr = t.transforms;
        for (&i, name) in data.iter().zip(names) {
            let name = name.unwrap_or_else(|| std::mem::take(&mut t.columns[i].name));
            t.columns[i].name = seen.make(name, &mut tr);
        }
        t.transforms = tr;
        t.warnings.push(format!(
            "column names taken from {donor_id} above (same columns): this block has no header of its own{}",
            block.map_or(String::new(), |p| format!("; its values are \"{p}\""))
        ));
        headers.push(Some(header));
    }
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
        let mut seen = UniqueNames::taken(t.columns.iter().map(|c| c.name.as_str()));
        let mut names = Vec::with_capacity(copies.len());
        let mut copies = copies;
        for c in &mut copies {
            c.name = seen.make(std::mem::take(&mut c.name), &mut t.transforms);
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

/// One header cell's text and position (row, column).
type HeaderPart = (String, (u32, u32));

/// Tables with at least this many values (columns × rows) type their columns on parallel threads.
const PARALLEL_TYPING_MIN_VALUES: usize = 200_000;
/// Threads for typing one table's columns.
const TYPING_THREADS: usize = 4;

/// `f(0), f(1), …, f(n - 1)` in this order; on up to [`TYPING_THREADS`] threads when `parallel`
/// (each index is taken by one thread; the results are put back in index order).
fn in_parallel<R: Send>(n: usize, parallel: bool, f: impl Fn(usize) -> R + Sync) -> Vec<R> {
    let threads = if parallel { std::thread::available_parallelism().map_or(1, |x| x.get()).min(TYPING_THREADS).min(n) } else { 1 };
    if threads <= 1 {
        return (0..n).map(f).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done: Vec<Vec<(usize, R)>> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        mine.push((i, f(i)));
                    }
                    mine
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
    });
    let mut slots: Vec<Option<R>> = (0..n).map(|_| None).collect();
    for (i, r) in done.into_iter().flatten() {
        slots[i] = Some(r);
    }
    slots.into_iter().map(|r| r.expect("every index computed")).collect()
}

pub(super) fn process_sheet(grid: &Grid, opts: &Options, layout: super::SheetAnalysis, out: &mut Normalized) {
    let sheet = grid.sheet;
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
        formulas_without_value: 0,
        formula_without_value_at: None,
        merges: sheet.merges.len(),
        tables: Vec::new(),
    };
    for c in cells.iter().filter(|c| c.formula.is_some() && c.value.is_none()) {
        info.formulas_without_value += 1;
        if info.formula_without_value_at.is_none_or(|at| (c.row, c.col) < at) {
            info.formula_without_value_at = Some((c.row, c.col));
        }
    }
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
    let sheet_hints = OnceLock::new();
    let det = totals::detection(grid);
    let cands = &det.candidates;
    super::trace("totals_detect.detection", t0);
    let overrides = row_overrides(grid, det);
    super::trace("totals_detect", t0);
    let checks: Vec<_> = cands.iter().map(|c| totals::verify(grid, c)).collect();
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
    // Small blocks of values the layout left outside every table (fewer rows than a headerless
    // table needs) are tables too, not Notes.
    let mut detected = lay.tables.clone();
    detected.extend(value_blocks(grid, &lay));
    detected.sort_by_key(|t| (t.rect.top, t.rect.left));
    for t in &detected {
        let donor = out.tables.len();
        for (part, t) in changed_parent_blocks(grid, t).iter().enumerate() {
            let n = out.tables.len() + 1;
            let inherited = (part > 0).then(|| &out.tables[donor]);
            let b = build_table(grid, &lay, t, inherited, n, &sheet_hints, &row_total, &overrides, opts, &mut owner);
            info.tables.push(n);
            layouts.push(row_layout(&b.table, &b.removed_totals));
            kept.push(b.kept);
            removed.extend(b.removed_totals.into_iter().map(|(row, rt, idxs)| (n, row, rt, idxs)));
            notes.extend(b.notes);
            out.tables.push(b.table);
        }
    }
    copy_row_labels(&mut out.tables[first..], &layouts);
    // Captions a table moved out of its last row belong to the table below them (B72).
    for x in notes.iter_mut().filter(|x| x.kind == "title" && x.table.is_none()) {
        x.table = associate(&out.tables[first..], x.row, x.col, true).map(|k| first + k + 1);
    }
    super::trace("build_tables", t0);
    super::trace_ms("build_tables.typing", super::typing::take_typing_time());
    let tables = &out.tables[first..];
    let table_at = |row: u32, col: u32| -> Option<usize> {
        tables
            .iter()
            .position(|t| t.rect.top <= row && row <= t.rect.bottom && t.rect.left <= col && col <= t.rect.right)
    };

    // Totals: detected candidates with their verification.
    let arith_rows: HashSet<u32> =
        cands.iter().filter(|c| c.evidence.contains(&Evidence::ArithmeticMatch)).map(|c| c.row).collect();
    let mut items: Vec<TotalItem> = Vec::new();
    let mut listed: HashSet<(u32, u32)> = HashSet::new();
    let mut name_columns = NameColumns::new();
    for (cand, check) in cands.iter().zip(&checks) {
        let k = table_at(cand.row, cand.col);
        let kept_in_data = k.is_some_and(|k| kept[k].contains(&cand.row));
        if let Some(k) = k
            && kept_in_data
            && record_named_like_a_total(grid, cand, &tables[k], &arith_rows, &mut name_columns)
        {
            continue;
        }
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
        let (status, reason) = judge(grid, cand, check, header, label.as_deref());
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
    let aggregate_rows: HashMap<u32, u32> = det.aggregates.iter().copied().collect();
    // Removed total rows: numbers not found by `totals::detect` are listed unverified; the row's
    // label text is attached; other text goes to Notes.
    for (n, row, rt, idxs) in &removed {
        // A known aggregate's label is its code or name (`geo` = 'EU27_2020', B87).
        let label_idx = aggregate_rows
            .get(row)
            .and_then(|&lc| idxs.iter().copied().find(|&i| cells[i].col == lc && matches!(cells[i].value, Some(Value::Text(_)))))
            .or_else(|| idxs.iter().copied().find(|&i| matches!(cells[i].value, Some(Value::Text(_)))));
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
                        reason: if aggregate_rows.contains_key(row) {
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

    // B-public: key/value cells outside every table (`Total Income | 9666 | Total Expenditure |
    // 7266` above a cash journal, a summary block beside a table) become a table of their own.
    let kv_table = key_value_table(grid, &lay, &mut owner, out.tables.len() + 1, opts);

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
            // A loose caption ("Quadro 1", "Table 3: …") is a title whatever its cell role (B72).
            _ if table_word(&text) => "title",
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

    // Rows of loose `Key:` cells (key/value metadata above a table), for the title choice.
    let mut notes_keys: HashMap<u32, usize> = HashMap::new();
    for x in &notes {
        let t = x.text.trim();
        if t.char_indices().take(40).any(|(i, c)| c == ':' && t[..i].chars().last().is_some_and(char::is_alphabetic)) {
            *notes_keys.entry(x.row).or_default() += 1;
        }
    }
    if let Some(t) = kv_table {
        info.tables.push(t.n);
        out.tables.push(t);
    }
    // Titles / notes per table, then output names.
    let tables = &mut out.tables[first..];
    for t in tables.iter_mut() {
        let mine: Vec<&NoteItem> = notes
            .iter()
            .filter(|x| x.table == Some(t.n) && x.kind != "repeated_header" && !x.text.is_empty())
            .collect();
        // A loose caption above ("Table 1: …", "TABL. 6 (157). …") is preferred to a title line
        // inside the table's range that is no caption.
        let caption_above = t.title.as_ref().is_some_and(|x| !table_word(&x.0))
            && mine.iter().any(|x| x.kind == "title" && x.row < t.rect.top && table_word(&x.text));
        if t.title.is_none() || caption_above {
            // Nearest loose title above the table; not a source line or key/value metadata
            // ("Source: …", "Date of publication:", B46), and a "Table 1: …" line first.
            let above: Vec<&&NoteItem> = mine.iter().filter(|x| x.kind == "title" && x.row < t.rect.top).collect();
            let keyed = |row: u32| {
                notes_keys.get(&row).is_some_and(|&n| n >= 2)
            };
            let plain: Vec<&&&NoteItem> =
                above.iter().filter(|x| !crate::layout::is_note_label(&x.text) && !keyed(x.row)).collect();
            let pick = plain
                .iter()
                .filter(|x| table_word(&x.text))
                .max_by_key(|x| x.row)
                .map(|x| caption_title(x, notes.iter()))
                .or_else(|| plain.iter().max_by_key(|x| x.row).map(|x| (x.text.clone(), (x.row, x.col))));
            t.title = pick.or_else(|| above.iter().max_by_key(|x| x.row).map(|x| (x.text.clone(), (x.row, x.col))));
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
    shared_parameter_captions(grid, tables, &notes);
    // After the titles: a block's title can be the phrase of its borrowed names.
    borrow_headers(&mut out.tables[first..]);

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

/// Key/value pairs found among the cells no table took: on one row, a number (not followed by
/// another number: that is a row of data) with the text cells just left of it (at most two, since the last value) as its key; dates and text values (the
/// publication date, an e-mail address of a statistics release) stay notes. Needs two pairs on the
/// sheet at least; keys are short labels, not notes. The table has the columns `key`, `value` and
/// `cell` (the value's address), one row per pair, in sheet order.
fn key_value_table(grid: &Grid, lay: &SheetLayout, owner: &mut [u8], n: usize, opts: &Options) -> Option<Table> {
    let cells = &grid.sheet.cells;
    // (key cells, value cell)
    let mut pairs: Vec<(Vec<usize>, usize)> = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let row = cells[i].row;
        let mut j = i;
        while j < cells.len() && cells[j].row == row {
            j += 1;
        }
        let mut texts: Vec<usize> = Vec::new();
        let mut last_col: Option<u32> = None;
        for k in i..j {
            let c = &cells[k];
            if owner[k] != OWN_NONE || lay.roles[k] == Role::Note {
                texts.clear();
                last_col = None;
                continue;
            }
            // Cells of a pair sit close together: at most two blank columns between them.
            if last_col.is_some_and(|l| c.col > l + 3) {
                texts.clear();
            }
            last_col = Some(c.col);
            let number = match &c.value {
                Some(Value::Number(_)) => true,
                Some(Value::Text(x)) => crate::values::parse_number(x, Default::default()).is_some_and(|p| !p.leading_zero),
                _ => false,
            };
            // A number with another number right after it is a row of data, not a pair.
            let run = number
                && cells[k + 1..j].iter().take(1).any(|d| {
                    d.col <= c.col + 3
                        && match &d.value {
                            Some(Value::Number(_)) => true,
                            Some(Value::Text(x)) => crate::values::parse_number(x, Default::default()).is_some(),
                            _ => false,
                        }
                });
            // A year beside a label is a period label (`POLSKA | 2021`), not a value.
            let year = matches!(c.value, Some(Value::Number(x)) if x.fract() == 0.0 && (1900.0..=2100.0).contains(&x));
            match &c.value {
                _ if run || year => texts.clear(),
                _ if number && !texts.is_empty() => {
                    pairs.push((std::mem::take(&mut texts), k));
                }
                // The text value of a `Key:` label (an e-mail address): neither key nor value here.
                Some(Value::Text(_))
                    if texts.last().is_some_and(|&t| matches!(&cells[t].value, Some(Value::Text(x)) if x.trim_end().ends_with(':'))) =>
                {
                    texts.clear();
                }
                Some(Value::Text(x)) if !number => {
                    let x = x.trim();
                    let label = !x.is_empty()
                        && x.chars().count() <= 60
                        && x.chars().any(char::is_alphabetic)
                        && !crate::layout::is_note_label(x)
                        && !is_nav_link(x);
                    if label {
                        texts.push(k);
                        if texts.len() > 2 {
                            texts.remove(0);
                        }
                    } else {
                        texts.clear();
                    }
                }
                _ => texts.clear(),
            }
        }
        i = j;
    }
    if pairs.len() < 2 {
        return None;
    }
    let mut tr = Transforms::default();
    let text_of = |k: usize| cells[k].value.as_ref().map(display).unwrap_or_default().trim().to_string();
    let keys: Vec<Out> = pairs
        .iter()
        .map(|(ks, _)| Out::Text(fit(ks.iter().map(|&k| text_of(k)).collect::<Vec<_>>().join(" / "), &mut tr)))
        .collect();
    let slots: Vec<Slot> = pairs.iter().map(|&(_, v)| cells[v].value.as_ref().map_or(Slot::Empty, Slot::Val)).collect();
    let rows: Vec<u32> = pairs.iter().map(|&(_, v)| cells[v].row).collect();
    let typed = type_column("value", 0, &slots, &rows, ColumnHints::default(), &mut |_, _, _| ColumnHints::default(), &mut tr);
    let mut rect = Rect { top: u32::MAX, left: u32::MAX, bottom: 0, right: 0 };
    for (ks, v) in &pairs {
        for &k in ks.iter().chain(std::iter::once(v)) {
            owner[k] = OWN_TABLE;
            let c = &cells[k];
            rect = Rect { top: rect.top.min(c.row), left: rect.left.min(c.col), bottom: rect.bottom.max(c.row), right: rect.right.max(c.col) };
        }
    }
    let column = |name: &str, role: ColRole, dtype: Dtype, values: Vec<Out>| Column {
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
    let mut columns = vec![
        column("key", ColRole::Data, Dtype::Text, keys),
        Column {
            unit: typed.unit,
            currency: typed.currency,
            decimal: typed.decimal,
            date_order: typed.date_order,
            ..column("value", ColRole::Data, typed.dtype, typed.values)
        },
    ];
    columns.extend([
        column("cell", ColRole::Data, Dtype::Text, pairs.iter().map(|&(_, v)| Out::Text(a1(cells[v].row, cells[v].col))).collect()),
        column(SRC_ROW, ColRole::SrcRow, Dtype::Integer, rows.iter().map(|&r| Out::Num(r as f64 + 1.0)).collect()),
    ]);
    if opts.keep_totals {
        columns.push(column(ROW_TYPE, ColRole::RowType, Dtype::Text, rows.iter().map(|_| Out::Text("data".into())).collect()));
    }
    let mut warnings = typed.warnings;
    warnings.push("key/value cells outside the other tables, one row per value; `cell` is the value's address".to_string());
    Some(Table {
        n,
        sheet: String::new(),
        source_sheet: grid.sheet.name.clone(),
        rect,
        header_top: rect.top,
        header_rows: 0,
        title: Some((super::KEY_VALUE_TITLE.to_string(), (cells[pairs[0].0[0]].row, cells[pairs[0].0[0]].col))),
        notes: Vec::new(),
        confidence: 0.6,
        columns,
        row_types: vec![RowType::Data; rows.len()],
        src_rows: rows,
        transforms: tr,
        warnings,
    })
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
    use crate::model::{Cell, Value};

    #[test]
    fn merged_parent_recovery_does_not_reuse_owned_cells() {
        use crate::model::{Rect, Sheet};
        let rows = vec![
            vec![Some(Value::Text("North".into())), None, Some(Value::Text("South".into()))],
            vec![],
            vec![Some(Value::Number(2005.0)), Some(Value::Number(2009.0)), Some(Value::Number(2005.0)), Some(Value::Number(2009.0))],
        ];
        let refs: Vec<&[Option<Value>]> = rows.iter().map(Vec::as_slice).collect();
        let (cells, _) = grid_rows(&refs);
        let sheet = Sheet {
            cells,
            merges: vec![Rect { top: 0, bottom: 0, left: 0, right: 1 }, Rect { top: 0, bottom: 0, left: 2, right: 3 }],
            ..Sheet::default()
        };
        let grid = crate::grid::Grid::new(&sheet);
        let rect = Rect { top: 2, bottom: 8, left: 0, right: 3 };
        let mut owner = vec![super::OWN_NONE; sheet.cells.len()];
        assert_eq!(super::merged_parent_top(&grid, rect, 2, &owner), Some(0));
        owner[0] = super::OWN_TABLE;
        assert_eq!(super::merged_parent_top(&grid, rect, 2, &owner), None);
    }

    fn grid_rows(rows: &[&[Option<Value>]]) -> (Vec<Cell>, Vec<(u32, Vec<usize>)>) {
        let mut cells = Vec::new();
        let mut out = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            let mut idxs = Vec::new();
            for (c, v) in row.iter().enumerate() {
                if let Some(v) = v {
                    idxs.push(cells.len());
                    cells.push(Cell { row: r as u32, col: c as u32, value: Some(v.clone()), formula: None });
                }
            }
            out.push((r as u32, idxs));
        }
        (cells, out)
    }

    /// promote_header over rows of (value, role) cells; returns (header top, body top).
    fn promote(rows: &[Vec<(Option<Value>, Role)>], htop: u32, header_rows: u32) -> (u32, u32) {
        let vals: Vec<Vec<Option<Value>>> = rows.iter().map(|r| r.iter().map(|(v, _)| v.clone()).collect()).collect();
        let refs: Vec<&[Option<Value>]> = vals.iter().map(|r| r.as_slice()).collect();
        let (cells, grouped) = grid_rows(&refs);
        let mut roles = Vec::new();
        for r in rows {
            roles.extend(r.iter().filter(|(v, _)| v.is_some()).map(|(_, role)| *role));
        }
        let grouped: Vec<(u32, Vec<usize>)> = grouped.into_iter().filter(|(_, i)| !i.is_empty()).collect();
        super::promote_header(&grouped, &cells, &roles, htop, header_rows, rows.len() as u32 - 1)
    }

    #[test]
    fn a_data_row_the_layout_took_for_the_header_is_body() {
        // Enron "Texas Nat_Gas Ops O&M": a table cut from its header at "Outside Services | 2340 |
        // 3057 | 2000 | …"; the layout made that first record the header row.
        let t = |s: &str| (Some(Value::Text(s.into())), Role::Data);
        let n = |x: f64| (Some(Value::Number(x)), Role::Data);
        let mut rows = vec![vec![
            (Some(Value::Text("Outside Services".into())), Role::Header),
            (Some(Value::Number(2340.0)), Role::Header),
            (Some(Value::Number(3057.0)), Role::Header),
            (Some(Value::Number(2000.0)), Role::Header),
        ]];
        for (k, item) in ["Supplies", "Marketing", "Corporate IT", "Corporate Rent", "Other"].iter().enumerate() {
            rows.push(vec![t(item), n(1093.0 + k as f64), n(1150.5), n(670.0 * k as f64)]);
        }
        assert_eq!(promote(&rows, 0, 1), (0, 0));
        // A real header row stays.
        rows[0] = vec![(Some(Value::Text("Item".into())), Role::Header), (Some(Value::Text("Jan".into())), Role::Header),
                       (Some(Value::Text("Feb".into())), Role::Header), (Some(Value::Text("Mar".into())), Role::Header)];
        assert_eq!(promote(&rows, 0, 1), (0, 1));
    }

    #[test]
    fn a_header_of_several_rows_without_one_naming_the_columns() {
        // Enron "Noresco Gas Usage" / "ENRON_$orion_1099": "Monthly | Hourly | Hourly", "Total |
        // Avg | Peak", "(kWh) | (kW) | (kW)" over the data; no row is distinct alone.
        let h = |s: &str| (Some(Value::Text(s.into())), Role::Header);
        let n = |x: f64| (Some(Value::Number(x)), Role::Data);
        let mut rows = vec![
            vec![h("Monthly"), h("Hourly"), h("Hourly"), h("Monthly"), h("Hourly"), h("Hourly")],
            vec![h("Total"), h("Avg"), h("Peak"), h("Total"), h("Avg"), h("Peak")],
            vec![h("(kWh)"), h("(kW)"), h("(kW)"), h("(mmbtu)"), h("(mmbtu/hr)"), h("(mmbtu/hr)")],
        ];
        for k in 0..6 {
            let x = k as f64;
            rows.push(vec![n(3296555.0 + x), n(4431.5 + x), n(8767.0 + x), n(30328.3 + x), n(40.76 + x), n(80.7 + x)]);
        }
        // Without header rows the layout's table starts at its first row.
        assert_eq!(promote(&rows, 0, 0), (0, 3));
        // A value among the labels makes no header block.
        let block: Vec<Vec<Option<Value>>> = rows[..3]
            .iter()
            .enumerate()
            .map(|(k, r)| r.iter().enumerate().map(|(c, (v, _))| if (k, c) == (2, 1) { Some(Value::Number(17.5)) } else { v.clone() }).collect())
            .collect();
        let refs: Vec<&[Option<Value>]> = block.iter().map(|r| r.as_slice()).collect();
        let (cells, grouped) = grid_rows(&refs);
        assert!(!super::header_block(&grouped, &cells));
    }

    #[test]
    fn a_label_row_the_layout_marked_partly_as_header() {
        // EUSES "pfs_0409": "CC | Authority | DATE | NUMBERS | Kind of data | …" with some cells
        // marked as data; without header rows it went to Notes as a repeated header.
        let h = |s: &str| (Some(Value::Text(s.into())), Role::Header);
        let d = |s: &str| (Some(Value::Text(s.into())), Role::Data);
        let n = |x: f64| (Some(Value::Number(x)), Role::Data);
        let mut rows = vec![vec![h("CC"), h("Authority"), d("Date from"), h("Numbers"), d("Kind of data")]];
        for k in 0..6 {
            rows.push(vec![d("AP"), d("ARIPO"), d("1984-11-01"), n(8400001.0 + k as f64), d("Patent")]);
        }
        assert_eq!(promote(&rows, 0, 0), (0, 1));
        // A form line (`Committee: | Bill Dulle | …`) is no header.
        rows[0] = vec![h("Committee:"), h("Bill Dulle"), d("bill@example.com"), h("314-613"), d("Ameren")];
        assert_eq!(promote(&rows, 0, 0), (0, 0));
    }

    #[test]
    fn data_rows_with_header_cells_are_data() {
        // Enron "EES CASH BURN" / "Tuition and Fees": a record whose label the layout marked as a
        // header cell went to Notes as a repeated header; a repeated header row stays one.
        let t = |s: &str| Some(Value::Text(s.into()));
        let n = |x: f64| Some(Value::Number(x));
        let body: Vec<Vec<Option<Value>>> = (0..6).map(|k| vec![t(&format!("State {k}")), n(1000.0 * k as f64 + 0.5), n(12.25)]).collect();
        let header: HashSet<String> = ["Region", "Budget", "Spent"].iter().map(|s| s.to_string()).collect();
        let check = |row: Vec<Option<Value>>| {
            let mut all: Vec<&[Option<Value>]> = body.iter().map(|r| r.as_slice()).collect();
            all.push(&row);
            let (cells, rows) = grid_rows(&all);
            let kinds = super::body_kinds(&rows, &cells, 0);
            super::data_like_row(&cells, &rows[6].1, &header, &kinds)
        };
        assert!(check(vec![t("California"), n(225000.0), n(1010000.0)]));
        assert!(!check(vec![t("Region"), t("Budget"), t("Spent")]));
        assert!(!check(vec![t("Region"), n(2001.0), n(2002.0)]));
        // labels of another table beside a value (`Emissions | 10 | 12/29 - 1/4 | …`): not of the columns' kinds
        assert!(!check(vec![t("Emissions"), t("12/29 - 1/4"), t("1/5 - 1/11"), n(10.0)]));
    }

    #[test]
    fn data_like_header_rows() {
        let t = |s: &str| Some(Value::Text(s.into()));
        let n = |x: f64| Some(Value::Number(x));
        let body: Vec<Vec<Option<Value>>> =
            (0..6).map(|k| vec![t(&format!("Item {k}")), n(100.0 + k as f64 * 7.5), n(3.25 * k as f64), n(12.0)]).collect();
        let check = |head: Vec<Option<Value>>| {
            let mut all: Vec<&[Option<Value>]> = vec![&head];
            all.extend(body.iter().map(|r| r.as_slice()));
            let (cells, rows) = grid_rows(&all);
            let kinds = super::body_kinds(&rows, &cells, 1);
            super::data_like_header_row(&cells, &rows[0].1, &kinds)
        };
        // a first record: values of the columns' kinds
        assert!(check(vec![t("Outside Services"), n(2340.0), n(3057.5), n(2000.0)]));
        // labels; years; a scale of strikes counting up; a column numbering
        assert!(!check(vec![t("Item"), t("Volume"), t("Rate"), t("Count")]));
        assert!(!check(vec![t("Item"), n(2019.0), n(2020.0), n(2021.0)]));
        assert!(!check(vec![t("Strike"), n(2.5), n(3.0), n(3.5)]));
        assert!(!check(vec![n(1.0), n(2.0), n(3.0), n(4.0)]));
        // one number among labels is no record
        assert!(!check(vec![t("Item"), n(32.27), t("Rate"), t("Count")]));
        // a record beside the header of another table (labels over number columns)
        assert!(!check(vec![t("Outside Services"), n(2340.0), n(3057.5), t("Income")]));
    }

    use super::*;

    #[test]
    fn sparse_explicit_outline_does_not_enable_unrelated_bold_inference() {
        let row = |label: &str, indent, bold, out, group| OutlineRow {
            indent, spaces: 0, bold, label: label.into(), out, group,
        };
        let mut rows = vec![
            row("Explicit parent", 0, false, Some(0), false),
            row("Indented child", 1, false, Some(1), false),
            row("New section", 0, true, None, true),
            row("Bold candidate", 0, true, Some(2), false),
            row("Plain peer", 0, false, Some(3), false),
            row("Another section", 0, true, None, true),
        ];
        rows.extend((4..40).map(|k| row("Flat peer", 0, false, Some(k), false)));
        let paths = outline_paths(&rows, 40).unwrap();
        assert_eq!(paths[1], ["Explicit parent"]);
        assert!(paths[3].is_empty(), "sparse bold-only inference still needs coverage");
    }

    #[test]
    fn sparse_explicit_outline_drops_same_indent_bold_ancestor() {
        let row = |label: &str, indent, bold, out, group| OutlineRow {
            indent, spaces: 0, bold, label: label.into(), out, group,
        };
        let mut rows = vec![
            row("Explicit parent", 0, false, Some(0), false),
            row("Bold candidate", 1, true, Some(1), false),
            row("Indented child", 1, false, Some(2), false),
            row("New section", 0, true, None, true),
        ];
        rows.extend((3..40).map(|k| row("Flat peer", 0, false, Some(k), false)));
        let paths = outline_paths(&rows, 40).unwrap();
        assert_eq!(paths[1], ["Explicit parent"]);
        assert_eq!(paths[2], ["Explicit parent"], "same-indent bold ancestry still needs coverage");
    }

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
