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
fn record_named_like_a_total(grid: &Grid, cand: &TotalCandidate, t: &Table, arith_rows: &HashSet<u32>) -> bool {
    let Some(i) = t.src_rows.iter().position(|&r| r == cand.row) else { return false };
    t.row_types[i] == RowType::Data && !arith_rows.contains(&cand.row) && named_like_a_total(grid, cand)
}

/// The keyword candidate of a register record named with a leading total word, without
/// arithmetic behind it ([`record_named_like_a_total`]).
fn named_like_a_total(grid: &Grid, cand: &TotalCandidate) -> bool {
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
    // A column of names or purposes: more than a third of its texts are distinct.
    let mut texts: HashSet<&str> = HashSet::new();
    let mut n = 0usize;
    for &i in grid.col_cells(lc) {
        if let Some(Value::Text(x)) = &sheet.cells[i as usize].value {
            texts.insert(x.as_str());
            n += 1;
        }
    }
    texts.len() * 3 > n
}

/// A total label recurring this often in its column is a category of long-format data.
const LABEL_CATEGORY_REPEATS: usize = 3;
/// A total label confirming a total row has at most this many words ("Grand Total", "Razem
/// Działanie", "SUMME Tirol mit Innsbruck"); longer texts are measure names or codes.
const LABEL_MAX_WORDS: usize = 4;

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
    let mut named_rows: Vec<u32> = Vec::new();
    for c in &det.candidates {
        let label_text = c.label.and_then(|(lr, lc)| match grid.cell(lr, lc).and_then(|x| x.value.as_ref()) {
            Some(Value::Text(t)) => Some((lc, t.as_str())),
            _ => None,
        });
        let labelled = c.evidence.iter().any(|e| matches!(e, Evidence::Keyword(_)))
            && label_text.is_some_and(|(lc, t)| {
                first_text(c.row) == Some(lc)
                    && label_words(t) <= LABEL_MAX_WORDS
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
        } else if c.direction == Direction::Vertical && named_like_a_total(grid, c) {
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

/// A caption that starts with a table word: "Table 1: …", "Tabelle 3", "Quadro 1", "Cuadro 2.1".
fn table_word(s: &str) -> bool {
    let w = s.split(|c: char| !c.is_alphabetic()).next().unwrap_or("").to_lowercase();
    [
        "table", "tabelle", "tableau", "tabla", "tabela", "tabella", "tabulka", "tabell", "taulukko", "tabel", "quadro",
        "quadros", "cuadro", "cuadros",
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
        let keys: Vec<Option<String>> = body.iter().map(|(_, idxs)| text_at(idxs, col).map(|t| norm_key(&t))).collect();
        let mut blocks: Vec<(usize, [usize; 3])> = Vec::new(); // (first body index, triple positions m/f/t)
        let mut i = 0;
        while i + 2 < body.len() {
            let three = [&keys[i], &keys[i + 1], &keys[i + 2]];
            let contiguous = body[i + 1].0 == body[i].0 + 1 && body[i + 2].0 == body[i].0 + 2;
            let hit = contiguous
                .then(|| {
                    SEX_TRIPLES.iter().find_map(|tr| {
                        let pos = |w: &str| three.iter().position(|k| k.as_deref() == Some(w));
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
    let body_top = (htop + header_rows).min(bottom + 1);
    let filled = |idxs: &[usize]| idxs.iter().filter(|&&i| cells[i].value.as_ref().is_some_and(|v| !display(v).is_empty())).count();
    let widest = rows.iter().filter(|(row, _)| (htop..body_top).contains(row)).map(|(_, idxs)| filled(idxs)).max().unwrap_or(0);
    let is_header = |idxs: &[usize]| idxs.iter().all(|&i| matches!(roles[i], Role::Header | Role::Title | Role::Note) || filled(&[i]) == 0);
    let mut found: Option<usize> = None;
    for (k, (_, idxs)) in rows.iter().enumerate().filter(|(_, (row, _))| *row >= body_top) {
        if idxs.iter().any(|&i| matches!(roles[i], Role::Data | Role::Total | Role::Other) && filled(&[i]) > 0) {
            break;
        }
        let n = filled(idxs);
        // Distinct labels (not a unit or legend row: "EURO EURO EURO", "nichts vorhanden" × 6).
        let distinct: HashSet<String> = idxs.iter().filter_map(|&i| cells[i].value.as_ref().map(display)).filter(|s| !s.is_empty()).collect();
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
    let Some(k) = found else { return (htop, body_top) };
    let top = rows[k].0;
    let mut end = top + 1;
    for (row, idxs) in &rows[k + 1..] {
        if *row != end || !is_header(idxs) || !idxs.iter().any(|&i| roles[i] == Role::Header) {
            break;
        }
        end += 1;
    }
    (top, end.min(bottom + 1))
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
    let (htop, body_top) = promote_header(&rows, cells, &lay.roles, t.header_top.clamp(r.top, r.bottom), t.header_rows, r.bottom);
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
    // `original_header` but does not name the columns (B45: not "… / 1").
    let numbering_rows: Vec<bool> = (0..nh)
        .map(|hr| {
            let vals: Vec<&Value> = rows
                .iter()
                .filter(|(row, _)| *row == htop + hr as u32)
                .flat_map(|(_, idxs)| idxs.iter().filter_map(|&i| cells[i].value.as_ref()))
                .filter(|v| !display(v).is_empty())
                .collect();
            crate::layout::column_numbering(vals.into_iter())
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
    let sex = sex_blocks(&rows, cells, r, body_top);
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
    // Text label columns of the records (text on most data rows): a "group header" row that fills
    // two or more of them like a record, only without values (a WDI country with no data), is a
    // record, not a group label (B44).
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
        text_rows.into_iter().filter(|&(_, n)| data_rows >= 4 && 2 * n > data_rows).map(|(c, _)| c).collect()
    };
    let mut key_cols: HashSet<usize> = HashSet::new();
    // Block of each output row: the rows between two total rows (removed or kept).
    let mut block_of: Vec<u32> = Vec::new();
    let mut block = 0u32;
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
        if kind == RowKind::Group {
            let filled: Vec<usize> = idxs.iter().copied().filter(|&i| !blank(i)).collect();
            if filled.len() >= 2 && filled.iter().all(|&i| label_cols.contains(&cells[i].col)) {
                kind = RowKind::Data;
            }
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
        let mut named: Vec<(String, (u32, u32))> = Vec::new();
        for (hr, h) in head.iter().enumerate().filter_map(|(hr, row)| row[k].as_ref().map(|h| (hr, h))) {
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
    let mut seen = UniqueNames::taken(std::iter::once(SRC_ROW).chain(opts.keep_totals.then_some(ROW_TYPE)));
    let mut columns: Vec<Column> = Vec::new();
    for mut c in group_col.into_iter().chain(data_cols) {
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
fn repeated_names(t: &Table) -> HashSet<&str> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for c in t.columns.iter().filter(|c| c.source_col.is_some()) {
        *seen.entry(name_base(&c.name)).or_default() += 1;
    }
    seen.into_iter().filter(|&(_, n)| n >= 3).map(|(s, _)| s).collect()
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
        let own: Vec<Option<String>> = data
            .iter()
            .map(|&i| Some(name_base(&t.columns[i].name).to_string()).filter(|n| !n.starts_with("col_")))
            .collect();
        let phrases: HashSet<&String> = own.iter().flatten().collect();
        let block = match phrases.len() {
            1 => phrases.into_iter().next().cloned(),
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
                    Some(p) if numeric => format!("{base} / {p}"),
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
    for (cand, check) in cands.iter().zip(&checks) {
        let k = table_at(cand.row, cand.col);
        let kept_in_data = k.is_some_and(|k| kept[k].contains(&cand.row));
        if let Some(k) = k
            && kept_in_data
            && record_named_like_a_total(&grid, cand, &tables[k], &arith_rows)
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
    // Titles / notes per table, then output names.
    let tables = &mut out.tables[first..];
    for t in tables.iter_mut() {
        let mine: Vec<&NoteItem> = notes
            .iter()
            .filter(|x| x.table == Some(t.n) && x.kind != "repeated_header" && !x.text.is_empty())
            .collect();
        if t.title.is_none() {
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
