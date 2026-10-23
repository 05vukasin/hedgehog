//! Compact, LLM-readable text views of a [`Normalized`] workbook: the catalog (`open_file`),
//! `describe_table` and the totals check. Pure functions, no I/O. Time coverage and per-table
//! totals come from hh_core, the same functions the manifest is written from.

use std::collections::{BTreeMap, HashMap, HashSet};

pub use hh_core::normalize::TimeCoverage;
use hh_core::normalize::{
    iso, table_totals, time_coverages, workbook_time_coverage, ColRole, Column, Dtype, Normalized, Out, RowType,
    Table, TableTotals,
};

use crate::facts;
use crate::types::{self, ColType}; // B21 semantic types (types.rs)
use crate::text::{csv_row, cut, ident, letter, num, range_a1, sheet_ref, short, thousands, years_in, MAX_CHARS};

/// The cap of a brief catalog (one line per table), in characters, plus this much per table (up to
/// the catalog's budget).
const BRIEF_CHARS: usize = 8_500;
const BRIEF_CHARS_PER_TABLE: usize = 25;
/// Characters of small tables' rows (inlined CSV) in one catalog (B104).
const INLINE_TOTAL: usize = 3500;
/// Tables above this count are always listed one line each.
const BRIEF_TABLES_OVER: usize = 40;
/// Up to this many columns a table's columns are listed with ranges / examples.
const DETAIL_COLS: usize = 14;
/// The differing headers of a table with the same layout as an earlier one, up to this many
/// characters.
const WIDE_LINE: usize = 2500;
/// A columns line at most this long; beyond, names only, then cut with a count.
const MAX_COLS_LINE: usize = 6000;
/// Text columns with at most this many distinct values get all of them listed.
const LABEL_VALUES: usize = 12;
/// Columns with details in `describe_table`.
const DESCRIBE_COLS: usize = 60;
/// Items listed by `totals`.
const TOTAL_ITEMS: usize = 50;

// ------------------------------------------------------------------------------------------------
// per-column and per-table facts

/// Statistics of one column's values.
#[derive(Debug, Clone, Default)]
pub struct ColStats {
    pub nulls: usize,
    pub distinct: usize,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub example: Option<String>,
}

pub fn col_stats(c: &Column) -> ColStats {
    let mut s = ColStats::default();
    let mut seen: HashSet<u64> = HashSet::new();
    for v in &c.values {
        let key = match v {
            Out::Null => {
                s.nulls += 1;
                continue;
            }
            Out::Num(x) | Out::Date(x) => {
                s.min = Some(s.min.map_or(*x, |m| m.min(*x)));
                s.max = Some(s.max.map_or(*x, |m| m.max(*x)));
                x.to_bits()
            }
            Out::Bool(b) => u64::from(*b) + 1,
            Out::Text(t) => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                t.hash(&mut h);
                h.finish()
            }
        };
        if s.example.is_none() {
            s.example = Some(out_text(v, c.dtype));
        }
        seen.insert(key);
    }
    s.distinct = seen.len();
    s
}

/// Display / CSV text of one normalized value (dates as ISO, percent as fraction).
pub fn out_text(v: &Out, dtype: Dtype) -> String {
    match v {
        Out::Null => String::new(),
        Out::Num(x) if matches!(dtype, Dtype::Date | Dtype::Datetime) => iso(*x),
        Out::Num(x) => num(*x),
        Out::Date(x) => iso(*x),
        // B82: as SQL stores it (INTEGER 1 / 0), so a filter copied from the catalog matches.
        Out::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Out::Text(t) => t.clone(),
    }
}

/// B82: how a boolean column is stored, for its catalog item and `describe_table`.
const BOOL_STORED: &str = "1/0 in SQL (TRUE/yes = 1, FALSE/no = 0; filter = 1, not 'TRUE')";

pub(crate) fn is_date(d: Dtype) -> bool {
    matches!(d, Dtype::Date | Dtype::Datetime)
}

pub(crate) fn is_numeric(d: Dtype) -> bool {
    matches!(d, Dtype::Integer | Dtype::Number | Dtype::Percent | Dtype::Currency)
}

/// Like [`bound`], numbers to 6 significant digits (catalog lines).
fn brief_bound(v: f64, d: Dtype) -> String {
    if is_date(d) || v == v.trunc() {
        return bound(v, d);
    }
    // B104: a range bound says where the values lie: 4 significant digits, every digit before
    // the point (-28.7469 → -28.75, 46322.1 → 46322).
    if v.abs() < 1.0 {
        let r: f64 = format!("{v:.3e}").parse().unwrap_or(v);
        return num(r);
    }
    let digits = v.abs().log10().floor() as i32 + 1;
    let decimals = (4 - digits).max(0) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

fn bound(v: f64, d: Dtype) -> String {
    if is_date(d) {
        let s = iso(v);
        if d == Dtype::Date { s.chars().take(10).collect() } else { s }
    } else {
        num(v)
    }
}

/// Column names as they are in SQL: the normalized names, made unique case-insensitively (SQLite
/// compares identifiers without case), control characters removed.
pub fn sql_names(t: &Table) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    t.columns
        .iter()
        .map(|c| {
            let base: String = c.name.chars().filter(|ch| !ch.is_control()).collect();
            let base = if base.trim().is_empty() { format!("col_{}", c.source_col.map(letter).unwrap_or_default()) } else { base };
            let mut name = base.clone();
            let mut k = 2;
            while !seen.insert(name.to_lowercase()) {
                name = format!("{base} ({k})");
                k += 1;
            }
            name
        })
        .collect()
}

/// A total / subtotal row.
#[derive(Debug, Clone)]
pub struct TotalRow {
    pub label: String,
    /// 1-based source row.
    pub row: u32,
    pub kind: RowType,
}

/// Row counts of a table split by row type, its total rows and its totals check.
#[derive(Debug, Clone, Default)]
pub struct RowInfo {
    pub data: usize,
    /// Total rows taken out of the data: only in `T<n>_all`.
    pub total_rows: Vec<TotalRow>,
    /// Rows the totals check found to be totals that stay in the data (`T<n>`), such as
    /// "Insgesamt" rows of a long table: the manifest's `totals.rows` with `kept_in_data`.
    pub kept_totals: Vec<TotalRow>,
    /// The table's totals as in the manifest (`tables[].totals`).
    pub totals: TableTotals,
    /// B74: source rows each total row adds up (1-based row → ranges), from its vertical checks.
    pub scopes: HashMap<u32, Vec<(u32, u32)>>,
    /// B74: total rows the totals check calls the grand total.
    pub grand_rows: HashSet<u32>,
    /// B160: total rows are labelled with these key columns' values joined by `/` (`Total/M`),
    /// when one column alone does not tell them apart; empty otherwise.
    pub key_cols: Vec<usize>,
    /// B160: total codes (`Sexo 'T' = M + F`), checked on the numbers.
    pub codes: Vec<crate::codes::TotalCode>,
}

/// Indexes of a table's data rows (all rows when it has no row types).
pub(crate) fn data_rows(t: &Table) -> Vec<usize> {
    (0..t.rows()).filter(|&r| t.row_types.get(r).is_none_or(|x| *x == RowType::Data)).collect()
}

pub fn row_info(norm: &Normalized, t: &Table) -> RowInfo {
    let totals = table_totals(t, &norm.totals);
    let label_at = |row1: u32| totals.rows.iter().find(|r| r.row == row1).and_then(|r| r.label.clone());
    let mut info = RowInfo::default();
    for (i, rt) in t.row_types.iter().enumerate() {
        if *rt == RowType::Data {
            info.data += 1;
            continue;
        }
        let src = t.src_rows.get(i).copied().unwrap_or(0);
        let label = label_at(src + 1).or_else(|| {
            t.columns.iter().filter(|c| matches!(c.role, ColRole::Data | ColRole::Group)).find_map(|c| match c.values.get(i) {
                Some(Out::Text(s)) if !s.trim().is_empty() => Some(s.clone()),
                _ => None,
            })
        });
        info.total_rows.push(TotalRow { label: label.unwrap_or_else(|| "(no label)".into()), row: src + 1, kind: *rt });
    }
    // Tables normalized without keep_totals have only data rows.
    if t.row_types.is_empty() {
        info.data = t.rows();
    }
    let labels: Vec<String> = info.total_rows.iter().map(|r| r.label.clone()).collect();
    relabel_totals(t, &mut info.total_rows);
    // (B103's labels from a code column stay as they are.)
    if info.total_rows.iter().map(|r| &r.label).eq(labels.iter()) {
        info.key_cols = full_key_labels(t, &mut info.total_rows);
    }
    info.codes = crate::codes::total_codes(t);
    let in_data: HashSet<u32> = data_rows(t).into_iter().filter_map(|r| t.src_rows.get(r).map(|s| s + 1)).collect();
    info.kept_totals = totals
        .rows
        .iter()
        .filter(|r| in_data.contains(&r.row))
        .map(|r| TotalRow {
            label: r.label.clone().unwrap_or_else(|| "(no label)".into()),
            row: r.row,
            kind: if r.level == "subtotal" { RowType::Subtotal } else { RowType::Total },
        })
        .collect();
    // What a total adds up: the rows most of its confirmed checks add (one odd formula in one
    // column does not decide), inside the table.
    let mut votes: HashMap<u32, Vec<(Scope, usize)>> = HashMap::new();
    let (top, bottom) = (t.rect.top + 1, t.rect.bottom + 1);
    for x in norm.totals.iter().filter(|x| x.table == Some(t.n) && x.direction == "vertical") {
        let scope = source_rows(&x.sources);
        if x.status == "ok" && !scope.is_empty() && scope.iter().all(|(a, b)| *a >= top && *b <= bottom) {
            let v = votes.entry(x.row + 1).or_default();
            match v.iter_mut().find(|(s, _)| *s == scope) {
                Some(e) => e.1 += x.count,
                None => v.push((scope, x.count)),
            }
        }
        if x.level == "grand_total" {
            info.grand_rows.insert(x.row + 1);
        }
    }
    for (row, v) in votes {
        if let Some((scope, _)) = v.into_iter().max_by_key(|(s, n)| (*n, std::cmp::Reverse(s.clone()))) {
            info.scopes.insert(row, scope);
        }
    }
    info.totals = totals;
    info
}

/// B103: total rows that all carry one label (a constant column such as a dataset code) get the
/// value of the column that names them: the text column whose value on most total rows is never a
/// data row's value (`EU27_2020` in a `geo` column).
fn relabel_totals(t: &Table, rows: &mut [TotalRow]) {
    if rows.len() < 2 || rows.iter().any(|r| r.label != rows[0].label) {
        return;
    }
    // The total rows in table order, as `rows` was built.
    let idx: Vec<usize> = (0..t.rows()).filter(|&r| t.row_types.get(r).is_some_and(|x| *x != RowType::Data)).collect();
    if idx.len() != rows.len() {
        return;
    }
    let data = data_rows(t);
    let mut best: Option<(usize, usize)> = None; // (total rows it names, column)
    for (i, c) in t.columns.iter().enumerate() {
        if c.dtype != Dtype::Text || !matches!(c.role, ColRole::Data | ColRole::Group) {
            continue;
        }
        let in_data: HashSet<&str> =
            data.iter().filter_map(|&r| if let Some(Out::Text(s)) = c.values.get(r) { Some(s.as_str()) } else { None }).collect();
        let named = idx.iter().filter(|&&r| matches!(c.values.get(r), Some(Out::Text(s)) if !in_data.contains(s.as_str()))).count();
        // The label's own column names them already ('Total' in a label column).
        if named * 2 > idx.len() && matches!(c.values.get(idx[0]), Some(Out::Text(s)) if *s == rows[0].label) {
            return;
        }
        if named * 2 > idx.len() && best.is_none_or(|(b, _)| named > b) {
            best = Some((named, i));
        }
    }
    if let Some((_, i)) = best {
        for (&r, tr) in idx.iter().zip(rows.iter_mut()) {
            if let Some(Out::Text(s)) = t.columns[i].values.get(r) {
                tr.label = s.clone();
            }
        }
    }
}

/// B160: total rows whose labels one column does not tell apart (`Total`/`M`, `Total`/`F`,
/// `Total`/`T`: the label taken from either column) are labelled with their full key, every key
/// column's value joined by `/` (`'Total/M'`), when a key column holds codes that split every
/// group and the full key tells more of them apart. The key columns used; empty when the labels
/// stay.
fn full_key_labels(t: &Table, rows: &mut [TotalRow]) -> Vec<usize> {
    let keys = crate::codes::key_columns(t);
    if keys.len() < 2 || rows.len() < 2 || t.rows() > crate::codes::MAX_ROWS {
        return Vec::new();
    }
    let idx: Vec<usize> = (0..t.rows()).filter(|&r| t.row_types.get(r).is_some_and(|x| *x != RowType::Data)).collect();
    if idx.len() != rows.len() {
        return Vec::new();
    }
    // Only where the total rows differ in a key column of codes that split every group (`Sexo`:
    // M, F, T), not for labels repeated down a group column.
    let split = keys.iter().any(|&k| {
        let vals: HashSet<Option<&str>> = idx
            .iter()
            .zip(rows.iter())
            .filter(|(_, tr)| tr.kind == RowType::Total)
            .map(|(&r, _)| match t.columns[k].values.get(r) {
                Some(Out::Text(s)) => Some(s.trim()),
                _ => None,
            })
            .collect();
        vals.len() >= 2 && crate::codes::code_dimension(t, k)
    });
    if !split {
        return Vec::new();
    }
    // Total rows only: subtotals of one code per group read well by their code ('T'@13/16/19).
    let full: Vec<Option<String>> = idx
        .iter()
        .zip(rows.iter())
        .map(|(&r, tr)| if tr.kind == RowType::Total { crate::codes::row_key(t, &keys, r) } else { None })
        .collect();
    if full.iter().all(Option::is_none) {
        return Vec::new();
    }
    for (r, f) in rows.iter_mut().zip(full) {
        if let Some(f) = f {
            r.label = f;
        }
    }
    keys
}

/// B162: the rolling-windows line of a table; a table with the same windows as one listed
/// before (`seen`) names it instead.
fn rolling_text(t: &Table, seen: &mut HashMap<(i64, i64), String>) -> Option<String> {
    let r = crate::views::rolling(t)?;
    let first = seen.get(&(r.len, r.step)).cloned();
    seen.entry((r.len, r.step)).or_insert_with(|| t.id());
    Some(crate::views::rolling_line(t, &r, first.as_deref()))
}

/// B160: total rows as the catalog lists them: with a total code, the code's phrase first (the
/// grand total by the code column, its subtotals per group), then the other total rows as usual.
pub(crate) fn totals_text(t: &Table, info: &RowInfo, entries: &[TotalEntry], scopes: bool, max: usize) -> String {
    if let Some(code) = info.codes.first()
        && let Some((text, covered)) = crate::codes::coded_totals(t, &sql_names(t), code, entries)
    {
        let rest: Vec<TotalEntry> = entries.iter().filter(|e| !covered.contains(&e.row)).cloned().collect();
        return if rest.is_empty() { text } else { format!("{text}; {}", total_entries_text(&rest, &t.id(), scopes, max)) };
    }
    total_entries_text(entries, &t.id(), scopes, max)
}

fn total_rows_text(rows: &[TotalRow], n: usize) -> String {
    let shown: Vec<String> = rows.iter().take(n).map(|r| format!("'{}'@{}", short(&r.label, 32), r.row)).collect();
    let more = rows.len().saturating_sub(n);
    let mut s = shown.join(", ");
    if more > 0 {
        s.push_str(&format!(" +{more} more"));
    }
    s
}

/// Total rows that stay in the data: up to 3 listed, else their distinct labels and first row.
fn kept_totals_text(rows: &[TotalRow]) -> String {
    if rows.len() <= 3 {
        return total_rows_text(rows, 3);
    }
    let mut labels: Vec<&str> = Vec::new();
    for r in rows {
        if !labels.contains(&r.label.as_str()) {
            labels.push(&r.label);
        }
    }
    let mut l: Vec<String> = labels.iter().take(3).map(|s| format!("'{}'", short(s, 32))).collect();
    if labels.len() > 3 {
        l.push(format!("+{} more labels", labels.len() - 3));
    }
    format!("labelled {}; first @{}", l.join(", "), rows[0].row)
}

// ------------------------------------------------------------------------------------------------
// total rows: labels, levels and what each adds up (B74)

/// Ranges of source rows (1-based, inclusive).
type Scope = Vec<(u32, u32)>;

/// A table in a brief catalog: what tells it apart in a run line.
#[derive(Clone)]
struct BriefMember<'a> {
    n: usize,
    title: Option<&'a str>,
    sheet: &'a str,
    rows: usize,
    /// The line's facts after the rows (period, total rows, scale): what a run shares.
    tail: String,
    /// The first table on its sheet (its main table): column names first.
    first_of_sheet: bool,
}

/// A brief table's columns: its own (grouped names), the same as an earlier table's, or a run line.
enum BriefCols {
    /// Grouped names, the plain names (SQL identifiers), and the names as they are.
    Own(Vec<String>, Vec<String>, Vec<String>),
    Same(String),
    Run,
}

/// A brief table line in the making: the line without columns, its columns, the column count and
/// the long-view / column-notes text that follows.
struct BriefRow<'a> {
    line: String,
    cols: BriefCols,
    n: usize,
    view: String,
    member: BriefMember<'a>,
    /// B150: a table the question matches: listed first, with every column name.
    prio: bool,
}

/// B150: how a brief listing is introduced.
#[derive(Clone, Copy, PartialEq)]
enum BriefMode {
    /// Every table, one line each.
    Plain,
    /// The tables a question does not match, after its tables in full.
    Others,
    /// Every table, those a question matches first.
    Focus,
}

/// Source rows (1-based) a total adds up, from its check's `sources` ("B5:B15", "B41 B47"), as
/// ranges.
fn source_rows(sources: &[String]) -> Vec<(u32, u32)> {
    let row_of = |cell: &str| cell.trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '$').parse::<u32>().ok();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for tok in sources.iter().flat_map(|s| s.split([' ', ',', ';'])).filter(|s| !s.is_empty()) {
        let tok = tok.rsplit('!').next().unwrap_or(tok);
        let (a, b) = tok.split_once(':').unwrap_or((tok, tok));
        if let (Some(a), Some(b)) = (row_of(a), row_of(b)) {
            out.push((a.min(b), a.max(b)));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// One total / subtotal row with its level and scope.
#[derive(Debug, Clone)]
pub(crate) struct TotalEntry {
    pub label: String,
    pub row: u32,
    pub kind: RowType,
    /// The row stays in `T<n>` (a total the check found inside the data).
    pub kept: bool,
    /// Source rows it adds up (empty when unknown).
    pub scope: Vec<(u32, u32)>,
    pub grand: bool,
}

/// A table's total rows (taken out and kept in the data), in source order, with what each adds
/// up and which one is the grand total: the only total no other total includes, or, when several
/// are on top, the one the totals check calls the grand total.
pub(crate) fn total_entries(info: &RowInfo) -> Vec<TotalEntry> {
    let scope_of = |row: u32| info.scopes.get(&row).cloned().unwrap_or_default();
    let mut out: Vec<TotalEntry> = info
        .total_rows
        .iter()
        .map(|r| TotalEntry { label: r.label.clone(), row: r.row, kind: r.kind, kept: false, scope: scope_of(r.row), grand: false })
        .chain(info.kept_totals.iter().map(|r| TotalEntry {
            label: r.label.clone(),
            row: r.row,
            kind: r.kind,
            kept: true,
            scope: scope_of(r.row),
            grand: false,
        }))
        .collect();
    out.sort_by_key(|e| e.row);
    let inside = |row: u32, e: &TotalEntry| e.row != row && e.scope.iter().any(|(a, b)| (*a..=*b).contains(&row));
    let top: Vec<usize> = (0..out.len()).filter(|&k| !out.iter().any(|e| inside(out[k].row, e))).collect();
    let grand = match top.as_slice() {
        // One total on top of others, or the only total and it adds up rows.
        [k] if out[*k].kind == RowType::Total && (out.len() > 1 || !out[*k].scope.is_empty()) => Some(*k),
        _ => {
            let marked: Vec<usize> = top.iter().copied().filter(|&k| info.grand_rows.contains(&out[k].row)).collect();
            (marked.len() == 1).then(|| marked[0])
        }
    };
    if let Some(k) = grand {
        out[k].grand = true;
    }
    out
}

/// Ranges of rows as text: `5–15`, `7, 49, 57, 63`, at most 4 parts.
fn ranges_text(scope: &[(u32, u32)]) -> String {
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for &(a, b) in scope {
        match merged.last_mut() {
            Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let mut parts: Vec<String> = merged.iter().take(4).map(|(a, b)| if a == b { a.to_string() } else { format!("{a}–{b}") }).collect();
    if merged.len() > 4 {
        parts.push("…".into());
    }
    parts.join(", ")
}

/// Total rows listed by level: `grand total 'Total Expenses'@65 (= rows 7, 49, 57, 63); totals
/// 'Total O&M'@49 (= 41, 47); subtotals (_row_type 'subtotal') 'Subtotal Fixed O&M'@41 …`, up to
/// `max` rows; with `scopes`, the rows each adds up.
pub(crate) fn total_entries_text(entries: &[TotalEntry], table: &str, scopes: bool, max: usize) -> String {
    let item = |e: &TotalEntry| {
        let mut s = format!("'{}'@{}", short(&e.label, 40), e.row);
        if scopes && !e.scope.is_empty() {
            s.push_str(&format!(" (= rows {})", ranges_text(&e.scope)));
        }
        if e.kept {
            s.push_str(&format!(" [stays in {table}]"));
        }
        s
    };
    // B103: many total rows with repeated labels (one per period or measure): each label once.
    let mut distinct: Vec<&str> = Vec::new();
    for e in entries {
        if !distinct.contains(&e.label.as_str()) {
            distinct.push(&e.label);
        }
    }
    if entries.len() > max && distinct.len() * 2 <= entries.len() && !entries.iter().any(|e| e.grand) {
        let l: Vec<String> = distinct.iter().take(max).map(|s| format!("'{}'", short(s, 40))).collect();
        let more = if distinct.len() > max { format!(" +{} more labels", distinct.len() - max) } else { String::new() };
        return format!("{} rows labelled {}{more}", entries.len(), l.join(", "));
    }
    let mut groups: Vec<String> = Vec::new();
    let mut shown = 0;
    if let Some(g) = entries.iter().find(|e| e.grand) {
        groups.push(format!("grand total {}", item(g)));
        shown += 1;
    }
    let both = entries.iter().any(|e| e.kind == RowType::Subtotal) && entries.iter().any(|e| e.kind == RowType::Total);
    let note = if both { " (_row_type 'subtotal', not 'total')" } else { "" };
    for (kind, one, many) in [
        (RowType::Total, "total", "totals"),
        (RowType::Subtotal, "subtotal", "subtotals"),
        (RowType::Average, "average", "averages"),
        (RowType::OfWhich, "of-which row", "of-which rows"),
    ] {
        let of_kind: Vec<&TotalEntry> = entries.iter().filter(|e| e.kind == kind && !e.grand).take(max.saturating_sub(shown)).collect();
        // B104: rows of one label without a scope of their own once: `'T'@13/16/19`.
        let plain = |e: &TotalEntry| !e.kept && (!scopes || e.scope.is_empty());
        let mut items: Vec<String> = Vec::new();
        let mut done: Vec<bool> = vec![false; of_kind.len()];
        for (a, e) in of_kind.iter().enumerate() {
            if done[a] {
                continue;
            }
            let same: Vec<usize> =
                (a + 1..of_kind.len()).filter(|&b| plain(e) && plain(of_kind[b]) && of_kind[b].label == e.label).collect();
            if same.is_empty() {
                items.push(item(e));
                continue;
            }
            let rows: Vec<String> = std::iter::once(e.row).chain(same.iter().map(|&b| of_kind[b].row)).map(|r| r.to_string()).collect();
            items.push(format!("'{}'@{}", short(&e.label, 40), rows.join("/")));
            for b in same {
                done[b] = true;
            }
        }
        if !items.is_empty() {
            shown += of_kind.len();
            let name = if of_kind.len() == 1 { one } else { many };
            let note = if kind == RowType::Subtotal { note } else { "" };
            groups.push(format!("{name}{note} {}", items.join(", ")));
        }
    }
    let mut s = groups.join("; ");
    if entries.len() > shown {
        s.push_str(&format!(" +{} more", entries.len() - shown));
    }
    s
}

/// A word that marks a total in a row label.
fn total_word(w: &str) -> bool {
    const W: &[&str] = &[
        "total", "totals", "gesamt", "insgesamt", "zusammen", "summe", "sum", "ukupno", "totale", "totaal", "ensemble",
        "subtotal", "zwischensumme", "gesamtsumme", "grand",
    ];
    W.contains(&w)
}

/// B105: with a grand total, the other total rows whose label names a group next to the total word
/// (`'Österreich gesamt'` = guests from Austria, `'EU 27 insgesamt'`): they total one group of the
/// label column, not the table, and a reader takes "Österreich gesamt" for Austria's total.
/// ` · 'Österreich gesamt', 'Deutschland gesamt' each total one group of <col> (e.g. Österreich), not the table`.
pub(crate) fn group_totals_note(entries: &[TotalEntry], col: Option<&str>) -> Option<String> {
    entries.iter().find(|e| e.grand)?;
    let mut labels: Vec<&str> = Vec::new();
    let mut example = None;
    for e in entries.iter().filter(|e| !e.grand) {
        let low = e.label.to_lowercase();
        let ws: Vec<&str> = low.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        let rest: Vec<&str> = ws.iter().copied().filter(|w| !total_word(w)).collect();
        if rest.len() < ws.len() && rest.iter().any(|w| w.chars().any(char::is_alphabetic)) && !labels.contains(&e.label.as_str()) {
            if example.is_none() {
                // The group's name: the label without its total word, as written.
                let g: Vec<&str> = e.label.split_whitespace().filter(|w| !total_word(&w.to_lowercase())).collect();
                example = Some(g.join(" "));
            }
            labels.push(&e.label);
        }
    }
    if labels.is_empty() {
        return None;
    }
    let more = labels.len().saturating_sub(4);
    let mut l: Vec<String> = labels.iter().take(4).map(|s| format!("'{}'", short(s, 40))).collect();
    if more > 0 {
        l.push(format!("+{more} more"));
    }
    let of = col.map(|c| format!(" of {c}")).unwrap_or_default();
    let each = if labels.len() == 1 { "totals" } else { "each total" };
    Some(format!(" · {} {each} one group{of} (e.g. {}), not the table", l.join(", "), example.unwrap_or_default()))
}

pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

pub(crate) fn data_cols(t: &Table) -> impl Iterator<Item = (usize, &Column)> {
    t.columns.iter().enumerate().filter(|(_, c)| matches!(c.role, ColRole::Data | ColRole::Group))
}

fn status_counts<'a>(items: impl Iterator<Item = &'a hh_core::normalize::TotalItem>) -> (usize, usize, usize) {
    let (mut ok, mut bad, mut unk) = (0, 0, 0);
    for x in items {
        match x.status {
            "ok" => ok += x.count,
            "mismatch" => bad += x.count,
            _ => unk += x.count,
        }
    }
    (ok, bad, unk)
}

/// All distinct values (first-appearance order) of a text column over the data rows, when there
/// are at most [`LABEL_VALUES`] of them: the values to filter on.
pub fn label_values(c: &Column, rows: &[usize]) -> Option<Vec<String>> {
    if c.dtype != Dtype::Text || !matches!(c.role, ColRole::Data | ColRole::Group) {
        return None;
    }
    let mut vals: Vec<&str> = Vec::new();
    for &r in rows {
        if let Some(Out::Text(s)) = c.values.get(r)
            && !vals.contains(&s.as_str())
        {
            if vals.len() == LABEL_VALUES {
                return None;
            }
            vals.push(s);
        }
    }
    (!vals.is_empty()).then(|| vals.into_iter().map(String::from).collect())
}

/// B103: a code dimension listed in full: at most this many distinct codes …
const CODE_VALUES: usize = 60;
/// … each at most this long (`AT`, `EU27_2020`, `KWH2500-4999`).
const CODE_LEN: usize = 12;

/// B103: all values of a short code column (Eurostat `geo`: `AT`, `DE`, …) in a large table: codes
/// are what a filter needs and cannot be guessed from the question. Repeated codes (a dimension),
/// no spaces, at most [`CODE_VALUES`] of at most [`CODE_LEN`] characters.
fn code_values(c: &Column, rows: &[usize]) -> Option<Vec<String>> {
    if c.dtype != Dtype::Text || !matches!(c.role, ColRole::Data | ColRole::Group) || rows.len() < 2 * CODE_VALUES {
        return None;
    }
    let mut vals: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for &r in rows {
        if let Some(Out::Text(s)) = c.values.get(r)
            && seen.insert(s.as_str())
        {
            // Codes: 2..12 capitals, digits and `_.-` (`AT`, `EU27_2020`, `KWH2500-4999`), not
            // words or initials.
            let code = (2..=CODE_LEN).contains(&s.chars().count())
                && s.chars().all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || "_.-".contains(ch));
            if vals.len() == CODE_VALUES || !code {
                return None;
            }
            vals.push(s);
        }
    }
    // Period codes ('2019-S1') are on the TIME line.
    if vals.len() <= LABEL_VALUES || crate::views::text_period_column(c, rows).is_some() {
        return None;
    }
    Some({
        vals.sort_unstable();
        vals.into_iter().map(String::from).collect()
    })
}

/// Rows of a small table whose row labels are all listed (B102): as many as an inlined table has.
const ROW_LABELS_ROWS: usize = 30;

/// B102: a small table that is not inlined (too wide) lists every row label (the first text column
/// that is not the group column, over all rows of `T<n>_all`), since the labels are its row index:
/// `Year values: "Equity" | … | "NPV @ 14%" | "IRR"`. Only when the values line has not listed them.
fn row_labels_line(t: &Table, names: &[String]) -> Option<String> {
    if t.rows() > ROW_LABELS_ROWS {
        return None;
    }
    let (i, c) = data_cols(t).find(|(_, c)| c.dtype == Dtype::Text && c.role == ColRole::Data)?;
    if label_values(c, &data_rows(t)).is_some() {
        return None;
    }
    let mut vals: Vec<String> = Vec::new();
    for v in &c.values {
        if let Out::Text(s) = v
            && !s.trim().is_empty()
            && !vals.contains(s)
        {
            vals.push(s.clone());
        }
    }
    (vals.len() > LABEL_VALUES).then(|| format!("    {} values: {}", ident(&names[i]), values_text(&vals)))
}

fn values_text(vals: &[String]) -> String {
    let s = vals.iter().map(|v| format!("\"{}\"", short(v, 60))).collect::<Vec<_>>().join(" | ");
    short(&s, 700)
}

// ------------------------------------------------------------------------------------------------
// mixed measures

/// What a label says it measures: currency, percent, count, average, index.
pub(crate) fn measure_classes(label: &str) -> u8 {
    let low = label.to_lowercase();
    let mut m = 0u8;
    if low.contains(['€', '$', '£']) {
        m |= 1;
    }
    if low.contains('%') {
        m |= 2;
    }
    for w in low.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        m |= match w {
            "eur" | "euro" | "euros" | "usd" | "chf" | "gbp" | "dollar" | "dollars" | "rsd" | "din" | "dinar" | "dinara" => 1,
            "prozent" | "percent" | "pct" | "anteil" | "share" | "rate" | "quote" | "procenat" | "pourcentage" => 2,
            "anzahl" | "number" | "count" | "zahl" | "personen" | "persons" | "people" | "cases" | "broj" | "nombre" => 4,
            "average" | "mean" | "avg" | "median" | "mittelwert" | "ø" | "prosek" | "moyenne" => 8,
            "index" | "messzahl" | "indeks" => 16,
            _ if w.starts_with("durchschnitt") => 8,
            _ if w.starts_with("anzahl") => 4,
            _ => 0,
        };
    }
    m
}

fn median_abs(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mid = v.len() / 2;
    Some(*v.select_nth_unstable_by(mid, f64::total_cmp).1)
}

/// The label column (group column first, then text columns) whose values distinguish different
/// measures in one table (counts vs amounts vs percentages): by the units / words in the labels,
/// or by numbers of very different size under each label. Linear in the rows: a column with more
/// than [`LABEL_VALUES`] labels is left at the first label too many.
pub(crate) fn mixed_measures_by(t: &Table, rows: &[usize]) -> Option<usize> {
    let mut cands: Vec<usize> = t.columns.iter().position(|c| c.role == ColRole::Group).into_iter().collect();
    cands.extend(t.columns.iter().enumerate().filter(|(_, c)| c.role == ColRole::Data && c.dtype == Dtype::Text).map(|(i, _)| i));
    let numeric: Vec<&Column> = data_cols(t).map(|(_, c)| c).filter(|c| is_numeric(c.dtype) && c.dtype != Dtype::Percent).collect();
    'cand: for i in cands {
        let c = &t.columns[i];
        // Labels written once per block count for the rows below them: each row's label index.
        let mut distinct: Vec<&str> = Vec::new();
        let mut labels: Vec<Option<usize>> = Vec::with_capacity(rows.len());
        let mut last: Option<usize> = None;
        for &r in rows {
            if let Some(Out::Text(s)) = c.values.get(r) {
                last = Some(match distinct.iter().position(|d| *d == s.as_str()) {
                    Some(k) => k,
                    None if distinct.len() == LABEL_VALUES => continue 'cand,
                    None => {
                        distinct.push(s);
                        distinct.len() - 1
                    }
                });
            }
            labels.push(last);
        }
        if distinct.len() < 2 {
            continue;
        }
        let classes: HashSet<u8> = distinct.iter().map(|l| measure_classes(l)).filter(|m| *m != 0).collect();
        if classes.len() >= 2 {
            return Some(i);
        }
        // By size only for blocks of rows per label (a label per row, such as countries of very
        // different size, is one measure).
        if distinct.len() >= labels.iter().flatten().count() {
            continue;
        }
        let (mut votes, mut cols) = (0, 0);
        for n in &numeric {
            let mut by: Vec<Vec<f64>> = vec![Vec::new(); distinct.len()];
            for (k, &r) in rows.iter().enumerate() {
                if let (Some(l), Some(Out::Num(x))) = (labels[k], n.values.get(r))
                    && *x != 0.0
                {
                    by[l].push(x.abs());
                }
            }
            let meds: Vec<f64> = by.into_iter().filter_map(median_abs).collect();
            if meds.len() < 2 {
                continue;
            }
            let (lo, hi) = meds.iter().fold((f64::MAX, 0.0f64), |(lo, hi), m| (lo.min(*m), hi.max(*m)));
            cols += 1;
            if hi >= 100.0 * lo {
                votes += 1;
            }
        }
        if cols > 0 && votes * 3 >= cols * 2 {
            return Some(i);
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------
// time coverage

/// Time coverage of the workbook as the manifest has it (`time_coverage`).
pub fn time_coverage(norm: &Normalized) -> Option<TimeCoverage> {
    workbook_time_coverage(norm)
}

/// Years mentioned in titles, notes, sheet names and the file name: a weaker hint than the data.
pub fn mentioned_years(norm: &Normalized, file_name: &str) -> Vec<i32> {
    let mut weak: Vec<i32> = Vec::new();
    for t in &norm.tables {
        if let Some((title, _)) = &t.title {
            weak.extend(years_in(title));
        }
        for (n, _) in &t.notes {
            weak.extend(years_in(n));
        }
    }
    for n in &norm.notes {
        if n.kind == "title" || n.kind == "note" {
            weak.extend(years_in(&n.text));
        }
    }
    for s in &norm.sheets {
        weak.extend(years_in(&s.name));
    }
    weak.extend(years_in(file_name));
    weak.sort_unstable();
    weak.dedup();
    weak
}

fn span(years: &[i32]) -> String {
    let mut ys = years.to_vec();
    ys.sort_unstable();
    ys.dedup();
    match ys.as_slice() {
        [] => String::new(),
        [y] => y.to_string(),
        [a, .., b] => {
            let mut s = format!("{a}–{b}");
            if ys.len() > 2 && ys.len() < (b - a + 1) as usize {
                s.push_str(&format!(" ({} distinct)", ys.len()));
            }
            s
        }
    }
}

fn period_range(tc: &TimeCoverage) -> String {
    if tc.from == tc.to { tc.from.clone() } else { format!("{}–{}", tc.from, tc.to) }
}

fn label_year(label: &str) -> Option<i32> {
    label.get(0..4)?.parse().ok()
}

fn id_list(ids: &[String], n: usize) -> String {
    let mut s = ids.iter().take(n).cloned().collect::<Vec<_>>().join(", ");
    if ids.len() > n {
        s.push_str(&format!(" +{} more", ids.len() - n));
    }
    s
}

/// The TIME line: the manifest's time coverage (period, granularity, which columns or headers it
/// comes from); years only mentioned in titles, notes, sheet or file names as a secondary note.
fn time_line(wb: Option<&TimeCoverage>, tables: &[(String, TimeCoverage)], weak: &[i32]) -> String {
    let Some(wb) = wb else {
        if weak.is_empty() {
            return "TIME no dates or periods found".into();
        }
        return format!("TIME {} (only mentioned in titles/notes/sheet or file names; no date or period columns)", span(weak));
    };
    const ORDER: [&str; 5] = ["year", "quarter", "month", "week", "day"];
    let mut by_gran: Vec<(&str, Vec<String>)> = Vec::new();
    for g in ORDER {
        let ids: Vec<String> = tables.iter().filter(|(_, c)| c.granularity == g).map(|(id, _)| id.clone()).collect();
        if !ids.is_empty() {
            by_gran.push((g, ids));
        }
    }
    let gran = if by_gran.len() > 1 {
        format!(
            "mixed: {} — check what one row is before summing",
            by_gran.iter().map(|(g, ids)| format!("{g} ({})", id_list(ids, 3))).collect::<Vec<_>>().join(", ")
        )
    } else {
        wb.granularity.to_string()
    };
    let mut from = Vec::new();
    let cols: Vec<String> = tables
        .iter()
        .filter(|(_, c)| c.source == "column")
        .flat_map(|(id, c)| c.columns.iter().map(move |n| format!("{id}.{}", ident(&short(n, 30)))))
        .collect();
    if !cols.is_empty() {
        from.push(format!("from columns {}", id_list(&cols, 4)));
    }
    let hdr: Vec<String> = tables.iter().filter(|(_, c)| c.source == "header").map(|(id, _)| id.clone()).collect();
    if !hdr.is_empty() {
        from.push(format!("from column headers in {}", id_list(&hdr, 6)));
    }
    let (lo, hi) = (label_year(&wb.from).unwrap_or(i32::MIN), label_year(&wb.to).unwrap_or(i32::MAX));
    let outside: Vec<i32> = weak.iter().copied().filter(|y| *y < lo || *y > hi).collect();
    let mut s = format!("TIME {} ({}{}{})", period_range(wb), gran, if by_gran.len() > 1 { "; " } else { ", " }, from.join("; "));
    if !outside.is_empty() {
        s.insert_str(s.len() - 1, &format!("; titles/notes/sheet or file names also mention {}", span(&outside)));
    }
    s
}

// ------------------------------------------------------------------------------------------------
// catalog

/// The storage words of a column item that matter (B104): dates, percent (stored as a fraction),
/// booleans, the unit or currency, `group`; plain text and numbers say nothing.
fn kind_text(c: &Column) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if matches!(c.dtype, Dtype::Date | Dtype::Datetime | Dtype::Percent | Dtype::Bool | Dtype::Empty) {
        parts.push(c.dtype.as_str());
    }
    if let Some(u) = c.currency.as_ref().or(c.unit.as_ref()) {
        parts.push(u);
    }
    if c.role == ColRole::Group {
        parts.push("group");
    }
    parts.join(" ")
}

/// `name[kind]`, or the bare name when there is nothing to say.
fn item(name: &str, kind: &str) -> String {
    if kind.is_empty() { ident(name) } else { format!("{}[{kind}]", ident(name)) }
}

/// Appends a word to a kind text, with a space when it is not empty.
fn push_word(kind: &mut String, w: &str) {
    if !kind.is_empty() {
        kind.push(' ');
    }
    kind.push_str(w);
}

/// One column with its range (measures, dates) or an example (text, unless its values are listed).
fn col_detail(c: &Column, name: &str, rows: usize, example: bool, ty: Option<&ColType>) -> String {
    if c.dtype == Dtype::Empty {
        return format!("{}[empty]", ident(name));
    }
    let st = col_stats(c);
    let mut kind = types::kind_with(&kind_text(c), c, ty); // B21
    // B104: no range for IDs and codes (they are not measured).
    let id = ty.is_some_and(|t| t.sem == types::Sem::Id);
    if let (Some(lo), Some(hi)) = (st.min, st.max)
        && (is_numeric(c.dtype) || is_date(c.dtype))
    {
        if !id {
            push_word(&mut kind, &format!("{}..{}", brief_bound(lo, c.dtype), brief_bound(hi, c.dtype)));
        }
        if is_date(c.dtype) {
            kind.push_str(&format!(", {} distinct", st.distinct));
        }
    } else if c.dtype == Dtype::Bool {
        push_word(&mut kind, BOOL_STORED);
    } else if let Some(ex) = st.example.as_ref().filter(|_| example) {
        push_word(&mut kind, &format!("e.g. \"{}\"", short(ex, 24)));
    }
    // B89: only for label columns (a block label is filled on the block's first row, and its
    // labels rarely repeat); a sparse flag column ("e", "p") is just mostly empty.
    let filled = rows.saturating_sub(st.nulls);
    let label_like = matches!(c.values.first(), Some(Out::Text(_))) && st.distinct * 2 >= filled;
    let sep = if kind.is_empty() { "" } else { ", " };
    if c.dtype == Dtype::Text && rows > 3 && st.nulls * 2 > rows && !label_like {
        kind.push_str(&format!("{sep}{}/{} empty", st.nulls, rows));
    } else if c.dtype == Dtype::Text && rows > 3 && st.nulls * 2 > rows {
        kind.push_str(&format!("{sep}{}/{} empty: label written once per block?", st.nulls, rows));
    }
    item(name, &kind)
}

/// One column as `name[unit]` (dates with their range), or the bare name.
fn col_compact(c: &Column, name: &str, ty: Option<&ColType>) -> String {
    let mut kind = types::kind_with(&kind_text(c), c, ty); // B21
    if c.dtype == Dtype::Bool {
        push_word(&mut kind, "1/0"); // B82: as stored in SQL
    }
    if is_date(c.dtype) {
        let st = col_stats(c);
        if let (Some(lo), Some(hi)) = (st.min, st.max) {
            push_word(&mut kind, &format!("{}..{}", bound(lo, c.dtype), bound(hi, c.dtype)));
        }
    }
    item(name, &kind)
}

const WEEKDAYS: &[&str] = &[
    "mon", "tue", "wed", "thu", "fri", "sat", "sun", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday",
    "sunday", "mo", "di", "mi", "do", "fr", "sa", "so", "montag", "dienstag", "mittwoch", "donnerstag", "freitag",
    "samstag", "sonntag",
];

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn day_number(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The period a column name stands for, as (unit, index) so that consecutive periods differ by
/// one, and the rest of the name (lowercased; a weekday counts as empty): a bare year (`2019`),
/// a month (`2024-03`, `Revenue 2024-03`) or a day (`Thu / 2002-01-31`).
pub(crate) fn name_period(name: &str) -> Option<(u8, i64, String)> {
    let s = name.trim();
    if s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()) {
        let y: i64 = s.parse().ok()?;
        return (1900..=2100).contains(&y).then(|| (0, y, String::new()));
    }
    let b = s.as_bytes();
    for i in 0..b.len().saturating_sub(6) {
        let digits = |r: std::ops::Range<usize>| r.clone().all(|k| b.get(k).is_some_and(u8::is_ascii_digit));
        if (i > 0 && b[i - 1].is_ascii_digit()) || !digits(i..i + 4) || b[i + 4] != b'-' || !digits(i + 5..i + 7) {
            continue;
        }
        let y: i64 = s[i..i + 4].parse().ok()?;
        let m: i64 = s[i + 5..i + 7].parse().ok()?;
        if !(1900..=2100).contains(&y) || !(1..=12).contains(&m) {
            return None;
        }
        let (unit, idx, end) = if b.get(i + 7) == Some(&b'-') && digits(i + 8..i + 10) {
            let d: i64 = s[i + 8..i + 10].parse().ok()?;
            (2, day_number(y, m, d), i + 10)
        } else {
            (1, y * 12 + m, i + 7)
        };
        if b.get(end).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        let rest = format!("{}{}", &s[..i], &s[end..]).to_lowercase();
        let rest = rest.trim_matches(|c: char| c.is_whitespace() || "/-,:".contains(c)).to_string();
        let rest = if WEEKDAYS.contains(&rest.trim_end_matches('.')) { String::new() } else { rest };
        return Some((unit, idx, rest));
    }
    None
}

/// The prefix a column name shares with its neighbours: up to and with the first `_`, ` / ` or
/// space (`biofuel_`, `Ensemble / `), when something follows it.
fn name_prefix(name: &str) -> Option<&str> {
    // A header path's first level (`Total / 2021` → `Total / `), else the first word or `_` part.
    let end = match name.find(" / ") {
        Some(i) => i + 3,
        None => [name.find('_').map(|i| i + 1), name.find(' ').map(|i| i + 1)].into_iter().flatten().min()?,
    };
    (end >= 3 && end < name.len()).then(|| &name[..end])
}

/// Columns compactly: runs of 4+ columns of one type named by consecutive periods (years, months
/// or days) as `"2011".."2025"[15 × number EUR]` (nothing hidden); with `runs`, also runs of 3+
/// columns with one name prefix, every member named (B71).
fn compact_items(cols: &[(usize, &Column)], names: &[String], runs: bool, tys: &[Option<ColType>]) -> Vec<String> {
    let ty = |k: usize| tys.get(cols[k].0).and_then(|x| x.as_ref()); // B21
    let same_col = |a: &Column, b: &Column| a.dtype == b.dtype && a.role == b.role && a.unit == b.unit && a.currency == b.currency;
    let same = |k: usize, j: usize| same_col(cols[k].1, cols[j].1) && ty(k) == ty(j);
    let run_text = |k: usize, j: usize| {
        let kind = types::kind_with(&kind_text(cols[k].1), cols[k].1, ty(k));
        let range = format!("{}..{}", ident(&names[cols[k].0]), ident(&names[cols[j - 1].0]));
        if kind.is_empty() { format!("{range}[{} cols]", j - k) } else { format!("{range}[{} × {kind}]", j - k) }
    };
    // Periods, or plain numbers as names (`"1".."20"`: years of a projection, B104).
    let number = |s: &str| (s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<i64>().ok()).flatten();
    let periods: Vec<Option<(u8, i64, String)>> =
        cols.iter().map(|(i, _)| name_period(&names[*i]).or_else(|| number(&names[*i]).map(|n| (9, n, String::new())))).collect();
    // Index of period column j if it continues the run started at k with direction `step`.
    let follows = |k: usize, j: usize, step: i64| -> bool {
        match (&periods[k], &periods[j]) {
            (Some((u0, i0, r0)), Some((u, i, r))) => u == u0 && r == r0 && *i == i0 + step * (j - k) as i64,
            _ => false,
        }
    };
    let n = cols.len();
    let mut out = Vec::new();
    let mut k = 0;
    while k < n {
        if periods[k].is_some() {
            let step = if k + 1 < n && follows(k, k + 1, -1) { -1 } else { 1 };
            let mut j = k + 1;
            while j < n && same(k, j) && follows(k, j, step) {
                j += 1;
            }
            if j - k >= 4 {
                out.push(run_text(k, j));
                k = j;
                continue;
            }
        }
        // B71: wide tables group 3+ consecutive columns by their name's prefix, every member
        // named: `biofuel_{consumption, electricity, share_elec[number · rate — don't sum]}[3 × number]`.
        if runs && let Some(p) = name_prefix(&names[cols[k].0]) {
            let mut j = k + 1;
            while j < n && name_prefix(&names[cols[j].0]) == Some(p) {
                j += 1;
            }
            if j - k >= 3 {
                let kinds: Vec<String> = (k..j).map(|m| types::kind_with(&kind_text(cols[m].1), cols[m].1, ty(m))).collect();
                let mut count: Vec<(&String, usize)> = Vec::new();
                for x in &kinds {
                    match count.iter_mut().find(|c| c.0 == x) {
                        Some(c) => c.1 += 1,
                        None => count.push((x, 1)),
                    }
                }
                let main = count.iter().max_by_key(|c| c.1).map(|c| c.0.clone()).unwrap_or_default();
                let members: Vec<String> = (k..j)
                    .map(|m| {
                        let rest = &names[cols[m].0][p.len()..];
                        if kinds[m - k] == main || kinds[m - k].is_empty() { rest.to_string() } else { format!("{rest}[{}]", kinds[m - k]) }
                    })
                    .collect();
                let text = format!("{p}{{{}}}", members.join(", "));
                let quoted = (k..j).any(|m| ident(&names[cols[m].0]).starts_with('"'));
                let text = if quoted { format!("\"{text}\"") } else { text };
                out.push(if main.is_empty() { text } else { format!("{text}[{} × {main}]", j - k) });
                k = j;
                continue;
            }
        }
        out.push(col_compact(cols[k].1, &names[cols[k].0], ty(k)));
        k += 1;
    }
    out
}

fn columns_line(t: &Table, names: &[String], examples: bool, tys: &[Option<ColType>]) -> String {
    let all = data_cols(t).count();
    // Empty columns (spacers in the source) take room and say nothing.
    let cols: Vec<(usize, &Column)> = data_cols(t).filter(|(_, c)| c.dtype != Dtype::Empty).collect();
    let empty = all - cols.len();
    let rows = t.rows();
    let n = cols.len();
    let label = if empty > 0 { format!("{all}, {empty} empty not shown") } else { all.to_string() };
    if n == 0 {
        return format!("    columns ({label})");
    }
    let data = data_rows(t);
    let mut items: Vec<String> = if n <= DETAIL_COLS {
        cols.iter()
            .map(|(i, c)| col_detail(c, &names[*i], rows, examples && label_values(c, &data).is_none(), tys.get(*i).and_then(|x| x.as_ref())))
            .collect()
    } else {
        // B71: every name in full; grouped by prefix when that is a tenth shorter (B104).
        let plain = compact_items(&cols, names, false, tys);
        let grouped = compact_items(&cols, names, true, tys);
        if len_of(&grouped) * 10 <= len_of(&plain) * 9 { grouped } else { plain }
    };
    let grouped = items.iter().any(|x| x.contains("{") && x.contains("}"));
    // Last resort for very wide tables: the rest by name only, still every one.
    if len_of(&items) > MAX_COLS_LINE {
        // Names only (types in describe_table), then cut at the cap with what is left.
        let all: Vec<String> = cols.iter().map(|(i, _)| ident(&names[*i])).collect();
        let mut used = 0;
        let keep = all.iter().take_while(|x| {
            used += x.len() + 1;
            used <= MAX_COLS_LINE
        });
        let keep = keep.count().max(1);
        items = all[..keep].to_vec();
        items.insert(0, "(names only; describe_table has the types)".into());
        if keep < all.len() {
            items.push(format!("… +{} more (describe_table lists them)", all.len() - keep));
        }
    }
    let legend = if grouped { "; a_{b, c} = a_b, a_c" } else { "" };
    format!("    columns ({label}{legend}): {}", items.join(" "))
}

/// The total-row part of a full table head line: with labels (`long`), and without them (`short`)
/// for when the table's `total rows:` line names them (B104: labels once per table).
struct HeadTotals {
    long: String,
    short: String,
}

fn head_totals(t: &Table, info: &RowInfo) -> Option<HeadTotals> {
    let (mut long, mut short) = (String::new(), String::new());
    if !info.total_rows.is_empty() {
        let k = info.total_rows.len();
        let what = if k == 1 { "total row" } else { "total rows" };
        long.push_str(&format!(" (+{k} {what}: {} — only in {}_all)", total_rows_text(&info.total_rows, 3), t.id()));
        short.push_str(&format!(" (+{k} {what} only in {}_all)", t.id()));
    }
    if !info.kept_totals.is_empty() {
        let k = info.kept_totals.len();
        let (stay, them) = if k == 1 { ("stays", "it") } else { ("stay", "them") };
        let what = plural(k, "total row", "total rows");
        long.push_str(&format!(
            " · {what} {stay} in {} ({}) — exclude {them} before summing",
            t.id(),
            kept_totals_text(&info.kept_totals)
        ));
        short.push_str(&format!(" · {what} {stay} in {} — exclude {them} before summing", t.id()));
    }
    (!long.is_empty()).then_some(HeadTotals { long, short })
}

fn table_head(t: &Table, info: &RowInfo, cov: Option<&TimeCoverage>, brief: bool) -> String {
    let mut s = format!("{:<3} ", t.id());
    if let Some((title, _)) = &t.title {
        s.push_str(&format!("'{}' ", short(title, if brief { 60 } else { 100 })));
    }
    s.push_str(&format!("← {} · {}", sheet_ref(&t.source_sheet, &range_a1(t.rect)), plural(info.data, "row", "rows")));
    // Brief lines get their total rows from the caller (B74: all labels, repeated ones once).
    if !brief && let Some(h) = head_totals(t, info) {
        s.push_str(&h.long);
    }
    if let Some(c) = cov {
        s.push_str(&format!(" · time {} ({})", period_range(c), c.granularity));
    }
    if t.confidence < 0.6 {
        s.push_str(&format!(" · low detection confidence {:.2}", t.confidence));
    }
    s
}

/// One table in the full catalog: its lines, then optional parts (in output order) added while the
/// catalog stays within its budget, by tier: how to select total rows and sign conventions (0),
/// then small tables' rows (1), then column facts (2), each tier in table order.
struct Block {
    lines: Vec<String>,
    parts: Vec<(u8, Vec<String>)>,
    /// The head line's total-row labels and their short form, used when the `total rows:` part
    /// (which names them) is in the catalog.
    head_totals: Option<HeadTotals>,
    /// A table that repeats an earlier table's columns: what a run of such tables must share.
    run: Option<RunKey>,
}

/// A table with the same columns as an earlier one, for one line per run of them (B100).
struct RunKey {
    /// The earlier table with the columns.
    first: String,
    /// The table number (runs are consecutive).
    n: usize,
    /// Facts the members must share: the head line's total-row part (table id as `<id>`) and time.
    key: String,
    rows: usize,
    title: Option<String>,
    sheet: String,
}

/// Merges runs of 3+ consecutive tables that repeat an earlier table's columns, total rows and
/// facts into one line each: `T2..T10 same columns as T1 · 72 rows each (+3 total rows only in
/// T<n>_all) · total rows labelled as in T1 · titles '… in … – Juli 2026': T2 Burgenland, …`.
fn merge_runs(blocks: Vec<Block>) -> Vec<Block> {
    // The table whose total-row labels a block's only part refers to; None when it has no part;
    // not mergeable when it has other parts or lines.
    let labels_as = |b: &Block| -> Option<Option<String>> {
        if b.lines.len() != 1 {
            return None;
        }
        match b.parts.as_slice() {
            [] => Some(None),
            [(0, p)] if p.len() == 1 => p[0].split_once(" — labels as in ").map(|(_, x)| Some(x.to_string())),
            _ => None,
        }
    };
    let mut out: Vec<Block> = Vec::new();
    let mut i = 0;
    let mut blocks: Vec<Option<Block>> = blocks.into_iter().map(Some).collect();
    while i < blocks.len() {
        let start = blocks[i].as_ref().and_then(|b| Some((b.run.as_ref()?, labels_as(b)?)));
        let mut j = i + 1;
        if let Some((r0, l0)) = start {
            while j < blocks.len() {
                let ok = blocks[j].as_ref().is_some_and(|b| {
                    b.run.as_ref().is_some_and(|r| r.first == r0.first && r.key == r0.key && r.n == r0.n + (j - i))
                        && labels_as(b).as_ref() == Some(&l0)
                });
                if !ok {
                    break;
                }
                j += 1;
            }
            if j - i >= 3 {
                let runs: Vec<&RunKey> = blocks[i..j].iter().filter_map(|b| b.as_ref()?.run.as_ref()).collect();
                let members: Vec<crate::brief::RunMember> = runs
                    .iter()
                    .map(|r| crate::brief::RunMember { id: format!("T{}", r.n), title: r.title.as_deref(), sheet: &r.sheet })
                    .collect();
                let (lo, hi) = runs.iter().fold((usize::MAX, 0), |(lo, hi), r| (lo.min(r.rows), hi.max(r.rows)));
                let rows = if lo == hi { format!(" · {} each", plural(lo, "row", "rows")) } else { format!(" · {lo}–{hi} rows") };
                let (totals, cov) = r0.key.split_once('|').unwrap_or((&r0.key, ""));
                let totals = totals.replace("<id>_all", "T<n>_all").replace("in <id>", "in each");
                let labels = l0.map(|x| format!(" · total rows labelled as in {x}")).unwrap_or_default();
                let line = crate::brief::run_line(&members, &r0.first, &format!("{rows}{totals}{cov}{labels}"));
                out.push(Block { lines: vec![line], parts: Vec::new(), head_totals: None, run: None });
                i = j;
                continue;
            }
        }
        out.extend(blocks[i].take());
        i += 1;
    }
    out
}

impl Block {
    /// The table's lines with its total-rows and signs lines, and its rows when it is small.
    fn new(lines: Vec<String>, t: &Table, names: &[String], info: &RowInfo, inline: Option<Vec<String>>) -> Self {
        let data = data_rows(t);
        let mut parts: Vec<(u8, Vec<String>)> = Vec::new();
        let totals = facts::total_rows_line(t, names, info);
        // The labels move to the `total rows:` line only when it names them.
        let named = totals.as_ref().is_some_and(|l| l.contains(" — labels in "));
        let head_totals = if named { head_totals(t, info) } else { None };
        parts.extend(totals.map(|l| (0, vec![l])));
        parts.extend(facts::signs_line(t, names, &data).map(|l| (0, vec![l])));
        parts.extend(inline.map(|rows| (1, rows)));
        Block { lines, parts, head_totals, run: None }
    }

    /// The head line without the total-row labels when the kept parts name them.
    fn shorten_head(&mut self, kept: &[bool]) {
        let named = self.parts.iter().zip(kept).any(|((_, p), k)| *k && p.first().is_some_and(|l| l.starts_with("    total rows: ")));
        if let (true, Some(h), Some(first)) = (named, self.head_totals.as_ref(), self.lines.first_mut()) {
            *first = first.replacen(&h.long, &h.short, 1);
        }
    }

    fn has_signs(&self) -> bool {
        self.parts.iter().any(|(_, p)| p.iter().any(|l| l.starts_with("    signs:")))
    }

    /// Column facts, one part per line, before the inline rows.
    fn add_facts(&mut self, lines: Vec<String>) {
        let at = self.parts.iter().position(|(tier, _)| *tier == 1).unwrap_or(self.parts.len());
        for (k, l) in lines.into_iter().enumerate() {
            self.parts.insert(at + k, (2, vec![l]));
        }
    }
}

fn len_of(lines: &[String]) -> usize {
    lines.iter().map(|l| l.len() + 1).sum()
}

/// The `open_file` catalog: one header line, the time coverage, the tables (full or one line each)
/// with columns, label values, group column and totals check, small tables' rows, notes and
/// warnings; at most `budget` characters, cut with an explicit `… cut:` line.
/// Time per catalog step on this thread, for `HEDGEHOG_TRACE` (`cat.*` lines).
const STEPS: [&str; 9] =
    ["cat.time", "cat.row_info", "cat.types", "cat.entities", "cat.id_pairs", "cat.share", "cat.values", "cat.numbers", "cat.quality"];

thread_local! {
    static STEP_TIME: std::cell::RefCell<[std::time::Duration; STEPS.len()]> = const { std::cell::RefCell::new([std::time::Duration::ZERO; STEPS.len()]) };
}

fn tm<R>(k: usize, f: impl FnOnce() -> R) -> R {
    let t = std::time::Instant::now();
    let r = f();
    STEP_TIME.with(|s| s.borrow_mut()[k] += t.elapsed());
    r
}

/// Prints and resets the step times of this thread.
fn trace_steps(total: std::time::Duration) {
    let steps = STEP_TIME.with(|s| std::mem::take(&mut *s.borrow_mut()));
    for (name, d) in STEPS.iter().zip(steps) {
        crate::cache::trace_ms(name, d);
    }
    crate::cache::trace_ms("cat.total", total);
}

/// The blocks' lines with their optional parts while they fit in `room` characters: total rows and
/// signs first, then small tables' rows, then column facts; each tier in table order.
fn with_parts(blocks: Vec<Block>, mut room: usize) -> Vec<String> {
    let mut take: Vec<Vec<bool>> = blocks.iter().map(|b| vec![false; b.parts.len()]).collect();
    // B104: small tables' rows up to INLINE_TOTAL characters in all (the first tables first); more
    // rows repeat what a query gives.
    let mut inline_room = INLINE_TOTAL;
    for tier in 0..3 {
        for (b, keep) in blocks.iter().zip(take.iter_mut()) {
            for ((t, part), k) in b.parts.iter().zip(keep.iter_mut()) {
                let need = len_of(part);
                if *t == tier && need <= room && (tier != 1 || need <= inline_room) {
                    room -= need;
                    if tier == 1 {
                        inline_room -= need;
                    }
                    *k = true;
                }
            }
        }
    }
    let mut body = Vec::new();
    for (mut b, keep) in blocks.into_iter().zip(take) {
        b.shorten_head(&keep);
        body.extend(b.lines);
        for ((_, part), k) in b.parts.into_iter().zip(keep) {
            if k {
                body.extend(part);
            }
        }
    }
    body
}

pub fn catalog(norm: &Normalized, display_name: &str, budget: usize) -> String {
    catalog_for(norm, display_name, budget, None)
}

/// B150: the catalog for a question: the tables it matches in full (first), the others one line
/// each. Without a question (or with an empty one) the same as [`catalog`].
pub fn catalog_for(norm: &Normalized, display_name: &str, budget: usize, question: Option<&str>) -> String {
    let t = std::time::Instant::now();
    let question = question.map(str::trim).filter(|q| !q.is_empty());
    let text = catalog_inner(norm, display_name, budget, question);
    trace_steps(t.elapsed());
    text
}

/// B150: tables shown in full for a question at most …
const FOCUS_TABLES: usize = 8;
/// … and tables a question may match (more: it tells none apart, the catalog as usual).
const FOCUS_MAX: usize = 16;
/// B150: the other tables' one-line listing gets this many characters of titles and column names
/// per table, within these bounds.
const OTHER_CHARS_PER_TABLE: usize = 20;
const OTHER_CHARS: (usize, usize) = (200, 1000);
/// B150: distinct values per text column (over this many rows) a question is matched against.
const MATCH_VALUES: usize = 60;
const MATCH_ROWS: usize = 5000;

/// B150: what a question is matched against in one table: title, sheet name, column names, the
/// distinct values of its text columns, its time coverage.
fn table_text(t: &Table, cov: Option<&TimeCoverage>) -> crate::question::TableText {
    let mut texts: Vec<(f64, String)> = Vec::new();
    if let Some((title, _)) = &t.title {
        texts.push((3.0, title.clone()));
    }
    texts.push((2.5, t.source_sheet.clone()));
    let names: Vec<&str> = data_cols(t).filter(|(_, c)| c.dtype != Dtype::Empty).map(|(_, c)| c.name.as_str()).collect();
    texts.push((2.0, names.join(" | ")));
    let mut vals: Vec<String> = Vec::new();
    for (_, c) in data_cols(t).filter(|(_, c)| c.dtype == Dtype::Text) {
        let mut seen: HashSet<&str> = HashSet::new();
        for v in c.values.iter().take(MATCH_ROWS) {
            if let Out::Text(s) = v
                && seen.len() < MATCH_VALUES
                && seen.insert(s.as_str())
            {
                vals.push(short(s, 80));
            }
        }
    }
    texts.push((1.0, vals.join(" | ")));
    crate::question::TableText { texts, from: cov.map(|c| c.from.clone()), to: cov.map(|c| c.to.clone()) }
}

/// B150: the tables a question matches, best first, and how the catalog names them
/// (`T3 (daily, index, 2023)`).
struct Focus {
    ks: Vec<usize>,
    items: String,
}

fn focus(norm: &Normalized, covs: &[Option<TimeCoverage>], question: &str) -> Option<Focus> {
    let n = norm.tables.len();
    if n < 2 {
        return None;
    }
    let q = crate::question::Question::parse(question);
    if q.is_empty() {
        return None;
    }
    let texts: Vec<_> = norm.tables.iter().zip(covs).map(|(t, c)| table_text(t, c.as_ref())).collect();
    let rel = crate::question::rank(&q, &texts);
    let ks = crate::question::relevant(&rel);
    if crate::cache::tracing() {
        for (k, r) in rel.iter().enumerate().filter(|(_, r)| r.score > 0.0) {
            eprintln!("focus T{} {:.3} {} {:?}", k + 1, r.score, r.champion, r.matched);
        }
        eprintln!("focus relevant {ks:?}");
    }
    // No table told apart (none, too many or every table matches): the catalog as usual.
    if ks.is_empty() || ks.len() > FOCUS_MAX || ks.len() == n {
        return None;
    }
    let items: Vec<String> = ks
        .iter()
        .map(|&k| {
            // A table number first (`table 1`), then words, then years and months.
            let num = |w: &&String| w.starts_with(|c: char| c.is_ascii_digit());
            let mut m: Vec<&String> = rel[k].matched.iter().filter(|w| w.starts_with("table ")).collect();
            m.extend(rel[k].matched.iter().filter(|w| !num(w) && !w.starts_with("table ")));
            m.extend(rel[k].matched.iter().filter(num));
            m.truncate(2);
            format!("{} ({})", norm.tables[k].id(), m.iter().map(|w| w.as_str()).collect::<Vec<_>>().join(", "))
        })
        .collect();
    Some(Focus { ks, items: items.join(", ") })
}

fn catalog_inner(norm: &Normalized, display_name: &str, budget: usize, question: Option<&str>) -> String {
    let budget = budget.min(MAX_CHARS);
    let file_name = display_name.rsplit(['/', '\\']).next().unwrap_or(display_name);
    let (mut covs, mut wb_cov) = tm(0, || time_coverages(norm));
    crate::views::fix_stamp_coverage(norm, &mut covs, &mut wb_cov); // B89
    let with_cov: Vec<(String, TimeCoverage)> =
        norm.tables.iter().zip(&covs).filter_map(|(t, c)| c.clone().map(|c| (t.id(), c))).collect();
    // Per-table periods only when there is more than one table (else the TIME line says it).
    let table_cov = |k: usize| covs[k].as_ref().filter(|_| norm.tables.len() > 1);
    let pct = (100 * norm.cells.in_tables).checked_div(norm.cells.total).unwrap_or(0);
    let mut head = vec![
        format!(
            "FILE {display_name} · {} · {} ({pct}% in tables) · {}",
            plural(norm.sheets.len(), "sheet", "sheets"),
            plural(norm.cells.total, "cell", "cells"),
            plural(norm.tables.len(), "table", "tables"),
        ),
        time_line(wb_cov.as_ref(), &with_cov, &mentioned_years(norm, file_name)),
    ];
    let empty: Vec<&str> = norm.sheets.iter().filter(|s| s.tables.is_empty() && s.cells > 0).map(|s| s.name.as_str()).collect();
    if !empty.is_empty() {
        // Names as shown (shortened), a repeated one once with its count.
        let mut shown: Vec<(String, usize)> = Vec::new();
        for n in &empty {
            let s = short(n, 30);
            match shown.iter_mut().find(|x| x.0 == s) {
                Some(x) => x.1 += 1,
                None => shown.push((s, 1)),
            }
        }
        let mut s = format!(
            "SHEETS without tables: {}",
            shown
                .iter()
                .take(10)
                .map(|(n, k)| if *k > 1 { format!("'{n}' ×{k}") } else { format!("'{n}'") })
                .collect::<Vec<_>>()
                .join(", ")
        );
        if shown.len() > 10 {
            s.push_str(&format!(" +{}", shown.len() - 10));
        }
        head.push(s);
    }

    let infos: Vec<RowInfo> = tm(1, || norm.tables.iter().map(|t| row_info(norm, t)).collect());
    // `keep`: the tables to list (all of them without a question).
    // Key/value tables have their own line (KEY/VALUE), not a block.
    let listed = |keep: &[bool]| -> Vec<bool> { keep.iter().zip(&norm.tables).map(|(&k, t)| k && !t.is_key_value()).collect() };
    let full = |keep: &[bool]| -> Vec<Block> {
        let keep = &listed(keep)[..];
        let mut out = Vec::new();
        let mut layouts: HashMap<Vec<(String, &'static str)>, String> = HashMap::new();
        let mut shapes: HashMap<Vec<&'static str>, (String, Vec<String>)> = HashMap::new();
        let mut rolls: HashMap<(i64, i64), String> = HashMap::new();
        for (k, (t, info)) in norm.tables.iter().zip(&infos).enumerate() {
            if !keep[k] {
                continue;
            }
            let names = sql_names(t);
            let cols: Vec<(String, &'static str)> =
                data_cols(t).map(|(i, c)| (names[i].clone(), c.dtype.as_str())).collect();
            let shape: Vec<&'static str> = cols.iter().map(|c| c.1).collect();
            let mut line = table_head(t, info, table_cov(k), false);
            let inline = facts::inline_rows(t, &names);
            if let Some(first) = layouts.get(&cols) {
                line.push_str(&format!(" · same columns as {first}"));
                let view = crate::views::table_view(t);
                line.extend(view.as_ref().map(crate::views::brief_text)); // B20
                let mut lines = vec![line];
                lines.extend(facts::scale_line(t)); // B85
                let mut b = Block::new(lines, t, &names, info, inline);
                if view.is_none() {
                    let short = head_totals(t, info).map(|h| h.short.replace(&t.id(), "<id>")).unwrap_or_default();
                    let cov = table_cov(k).map(|c| format!(" · time {} ({})", period_range(c), c.granularity)).unwrap_or_default();
                    b.run = Some(RunKey {
                        first: first.clone(),
                        n: t.n,
                        key: format!("{short}|{cov}"),
                        rows: info.data,
                        title: t.title.as_ref().map(|x| x.0.clone()),
                        sheet: t.source_sheet.clone(),
                    });
                }
                out.push(b);
                continue;
            }
            if cols.len() >= 3
                && let Some((first, first_names)) = shapes.get(&shape)
            {
                // The headers that differ, period runs collapsed.
                let diff: Vec<(usize, &Column)> = data_cols(t).filter(|(i, _)| !first_names.contains(&names[*i])).collect();
                let items = compact_items(&diff, &names, false, &[]);
                line.push_str(&format!(" · same layout as {first}"));
                line.extend(crate::views::table_view(t).map(|lv| crate::views::brief_text(&lv))); // B20
                if !items.is_empty() {
                    // B71: every differing header, up to a byte budget.
                    let mut used = 0;
                    let keep = items.iter().take_while(|x| {
                        used += x.len() + 1;
                        used <= WIDE_LINE
                    });
                    let keep = keep.count().max(1);
                    line.push_str(&format!("; headers here: {}", items[..keep].join(" ")));
                    if items.len() > keep {
                        line.push_str(&format!(" … +{} more (describe_table lists them)", items.len() - keep));
                    }
                }
                let mut lines = vec![line];
                lines.extend(facts::scale_line(t)); // B85
                out.push(Block::new(lines, t, &names, info, inline));
                continue;
            }
            layouts.insert(cols.clone(), t.id());
            shapes.insert(shape, (t.id(), cols.iter().map(|c| c.0.clone()).collect()));
            let data = data_rows(t);
            let (mixed, tys) = tm(2, || {
                let mixed = mixed_measures_by(t, &data);
                (mixed, types::classify(t, &names, mixed, &types::summed_cols(norm, t))) // B21
            });
            let mut lines = vec![line, columns_line(t, &names, inline.is_none(), &tys)];
            lines.extend(facts::scale_line(t)); // B85: "(All values in 000s)"
            lines.extend(facts::column_notes_line(norm, t, &names)); // B83: footnotes defining columns
            lines.extend(rolling_text(t, &mut rolls)); // B162: overlapping periods
            // B2/B18: "<id> - <name>" columns and how to group them, right under the columns.
            let ents = tm(3, || crate::entity::detect(t));
            lines.extend(ents.iter().map(|e| crate::entity::line(e, &t.id())));
            // B80: an ID column with its names in another column.
            let pairs = tm(4, || crate::entity::id_name_pairs(t, &data));
            for (k, p) in pairs.iter().enumerate() {
                lines.push(if k == 0 { crate::entity::id_name_line(p, &t.id()) } else { crate::entity::id_name_line_short(p, &pairs[0]) });
            }
            // B19: a share of the total, with the entity (or label) column to rank by.
            let key = ents.first().map(crate::entity::group_key).or_else(|| pairs.first().map(|p| crate::text::quote_ident(&p.id)));
            lines.extend(tm(5, || facts::share_line(t, &names, &data, key, &tys)));
            // The values to filter on; a small inlined table shows them in its rows. Code lists
            // (B103) and row labels (B102) are optional, after the rows of small tables.
            let mut more_values = Vec::new();
            if inline.is_none() {
                tm(6, || {
                    for (i, c) in data_cols(t) {
                        if let Some(vals) = label_values(c, &data) {
                            lines.push(format!("    {} values: {}", ident(&names[i]), values_text(&vals)));
                        } else if let Some(vals) = code_values(c, &data).filter(|_| !pairs.iter().any(|p| p.id == names[i])) {
                            // (an ID with a name column: the names say more than the codes)
                            more_values.push(format!("    {} values: {}", ident(&names[i]), values_text(&vals)));
                        }
                    }
                    more_values.extend(row_labels_line(t, &names));
                });
            }
            if let Some(i) = mixed {
                lines.push(format!("    rows mix measures by {} — filter before aggregating", ident(&names[i])));
            }
            // B20 long view (views.rs), B21 do-not-sum hint (types.rs)
            if let Some(lv) = crate::views::long_view(t, &tys) {
                lines.push(crate::views::catalog_line(t, &lv, mixed, !info.total_rows.is_empty()));
            }
            lines.extend(types::no_sum_line(t, &names, &tys, mixed));
            let mut extra = Vec::new();
            if let Some(g) = t.columns.iter().position(|c| c.role == ColRole::Group) {
                extra.push(format!("group column: {}", ident(&names[g])));
            }
            let tt = &info.totals;
            // B104: only a failed check changes an answer (the TOTALS line has the counts).
            if tt.mismatch > 0 {
                extra.push(format!("totals: {} mismatch (call totals)", tt.mismatch));
            }
            if !t.warnings.is_empty() {
                extra.push(format!("warnings: {} (e.g. {})", t.warnings.len(), short(&t.warnings[0], 90)));
            }
            if !extra.is_empty() {
                lines.push(format!("    {}", extra.join(" · ")));
            }
            let small = inline.is_some();
            let mut b = Block::new(lines, t, &names, info, inline);
            b.add_facts(more_values);
            if !small && mixed.is_none() {
                let ranges_shown = data_cols(t).filter(|(_, c)| c.dtype != Dtype::Empty).count() <= DETAIL_COLS;
                let mut f = tm(7, || facts::numbers_lines(t, &names, &data, ranges_shown));
                f.extend(tm(8, || facts::quality_line(t, &names, &data, b.has_signs())));
                b.add_facts(f);
            }
            out.push(b);
        }
        // B74: a table whose total rows read like an earlier table's says so instead.
        let mut seen: HashMap<String, String> = HashMap::new();
        for (t, b) in norm.tables.iter().zip(keep).filter(|(_, k)| **k).map(|(t, _)| t).zip(out.iter_mut()) {
            for (_, part) in b.parts.iter_mut() {
                let Some(l) = part.first_mut().filter(|l| l.starts_with("    total rows: ")) else { continue };
                let key = l.replace(&format!("{}_all", t.id()), "_all").replace(&format!("[stays in {}]", t.id()), "[stays]");
                match seen.get(&key) {
                    Some(first) => {
                        let cond = l.split(" — labels in ").next().unwrap_or(l).to_string();
                        *l = format!("{cond} — labels as in {first}");
                    }
                    None => {
                        seen.insert(key, t.id());
                    }
                }
            }
        }
        // B16: a table that is the long form of another, on both of them.
        for (k, l) in facts::long_forms(&norm.tables) {
            if keep[k] {
                let at = keep[..k].iter().filter(|x| **x).count();
                out[at].lines.push(l);
            }
        }
        merge_runs(out)
    };
    // Tables one line each, within `avail` characters (B100): the sheet, rows, total rows by level
    // (B74, a repeated set once), the scale (B85), then the column names with shared header levels
    // written once; tables that repeat an earlier table's columns say so, runs of them in one line;
    // in table order, as many tables as fit show every column name, the others their first two.
    // B150: `keep`: the tables to list; `shown`: tables listed in full above (a question's
    // tables), which the brief lines refer to; `prio`: tables a question matches, listed first
    // with every column name; `room_cap`: at most this many characters of titles and column names
    // for the other tables.
    let brief = |avail: usize, keep: &[bool], shown: &[bool], prio: &[bool], room_cap: Option<usize>, mode: BriefMode| -> Vec<String> {
        let keep = &listed(keep)[..];
        let what = match mode {
            BriefMode::Plain => "tables listed briefly",
            BriefMode::Others => "other tables, one line each",
            BriefMode::Focus => "tables one line each, the question's first",
        };
        let mut out = vec![format!(
            "({what}; a table without ← is on the sheet above; column names grouped: {}; describe_table lists a table's columns with types)",
            crate::brief::LEGEND
        )];
        let mut seen_totals: HashMap<String, String> = HashMap::new();
        let mut seen_cols: HashMap<Vec<String>, String> = HashMap::new();
        let mut seen_prio: HashMap<Vec<String>, String> = HashMap::new();
        // B162: the rolling-windows line on the first table listed with those windows, naming the
        // others (the question's tables come first).
        let mut rolls: Vec<Option<String>> = vec![None; norm.tables.len()];
        {
            let order: Vec<usize> = (0..norm.tables.len()).filter(|&k| keep[k] && prio[k]).chain((0..norm.tables.len()).filter(|&k| keep[k] && !prio[k])).collect();
            // (length and step, the first table, its windows, the other tables)
            type Windows = ((i64, i64), usize, crate::views::Rolling, Vec<String>);
            let mut by: Vec<Windows> = Vec::new();
            for k in order {
                let t = &norm.tables[k];
                let Some(r) = crate::views::rolling(t) else { continue };
                match by.iter_mut().find(|x| x.0 == (r.len, r.step)) {
                    Some(x) => x.3.push(t.id()),
                    None => by.push(((r.len, r.step), k, r, Vec::new())),
                }
            }
            for (_, k, r, others) in by {
                let mut l = crate::views::rolling_line(&norm.tables[k], &r, None);
                if !others.is_empty() {
                    l.push_str(&format!("; the same windows in {}", id_list(&others, 6)));
                }
                rolls[k] = Some(l);
            }
        }
        for (t, _) in norm.tables.iter().zip(shown).filter(|(_, s)| **s) {
            let names = sql_names(t);
            let list: Vec<String> = data_cols(t).filter(|(_, c)| c.dtype != Dtype::Empty).map(|(i, _)| names[i].clone()).collect();
            seen_cols.entry(list).or_insert_with(|| t.id());
        }
        // A table's period only when it is not inside the TIME line's span at its granularity (the
        // most common one is said once there too).
        let inside = |c: &TimeCoverage| {
            wb_cov.as_ref().is_some_and(|w| w.granularity == c.granularity && w.from <= c.from && c.to <= w.to)
        };
        let covs_text: Vec<Option<String>> = (0..norm.tables.len())
            .map(|k| table_cov(k).filter(|c| !inside(c)).map(|c| format!(" · time {} ({})", period_range(c), c.granularity)))
            .collect();
        let mut count: HashMap<&str, usize> = HashMap::new();
        for c in covs_text.iter().flatten() {
            *count.entry(c.as_str()).or_default() += 1;
        }
        let common = count.iter().max_by_key(|(c, n)| (**n, std::cmp::Reverse(**c))).filter(|(_, n)| **n >= 2).map(|(c, _)| c.to_string());
        let mut sheets_seen: HashSet<&str> = HashSet::new();
        let mut rows: Vec<BriefRow> = Vec::new();
        for (k, (t, info)) in norm.tables.iter().zip(&infos).enumerate() {
            if !keep[k] {
                continue;
            }
            let names = sql_names(t);
            let mut tail = String::new();
            if let Some(c) = covs_text[k].as_ref().filter(|c| Some(*c) != common.as_ref()) {
                tail.push_str(c);
            }
            if t.confidence < 0.6 {
                tail.push_str(&format!(" · low detection confidence {:.2}", t.confidence));
            }
            let entries = total_entries(info);
            if !entries.is_empty() && mode != BriefMode::Plain && !prio[k] {
                // B150: a table the question does not match: how many, not which.
                tail.push_str(&format!(" · {} in {}_all", plural(entries.len(), "total row", "total rows"), t.id()));
                // B160: a total code, briefly.
                if let Some(code) = info.codes.first() {
                    tail.push_str(&format!(" ({})", crate::codes::brief_note(code, &names)));
                }
            } else if !entries.is_empty() {
                let groups = if info.codes.is_empty() && info.key_cols.is_empty() { group_totals_note(&entries, None) } else { None };
                let text = totals_text(t, info, &entries, false, 15) + &groups.unwrap_or_default();
                let key = format!("{text}|{}", entries.iter().map(|e| e.row.to_string()).collect::<Vec<_>>().join(","));
                match seen_totals.get(&key) {
                    Some(first) => tail.push_str(&format!(" · total rows as in {first}")),
                    None => {
                        let kept = entries.iter().filter(|e| e.kept).count();
                        tail.push_str(&format!(
                            " · {} ({}): {text}",
                            plural(entries.len(), "total row", "total rows"),
                            if kept == 0 { format!("only in {}_all", t.id()) } else { format!("only in {}_all unless marked", t.id()) }
                        ));
                        seen_totals.insert(key, t.id());
                    }
                }
            }
            if let Some((f, word, _)) = facts::table_scale(t) {
                tail.push_str(&format!(" · amounts in {word} (× {})", thousands(f as usize)));
            }
            let cols: Vec<(usize, &Column)> = data_cols(t).filter(|(_, c)| c.dtype != Dtype::Empty).collect();
            let list: Vec<String> = cols.iter().map(|(i, _)| names[*i].clone()).collect();
            let mut view = crate::views::table_view(t).map(|lv| crate::views::brief_text(&lv)).unwrap_or_default();
            // B83: notes that define a column, on their own line under the table.
            if let Some(l) = facts::column_notes_line(norm, t, &names) {
                view.push('\n');
                view.push_str(&l);
            }
            // B162: overlapping periods, once for all tables with the same windows.
            if let Some(l) = &rolls[k] {
                view.push('\n');
                view.push_str(l);
            }
            let member = BriefMember {
                n: t.n,
                title: t.title.as_ref().map(|x| x.0.as_str()),
                sheet: t.source_sheet.as_str(),
                rows: info.data,
                tail,
                first_of_sheet: sheets_seen.insert(t.source_sheet.as_str()),
            };
            // A table the question matches lists its own columns (unless another one above has them).
            let same = if prio[k] { seen_prio.get(&list) } else { seen_cols.get(&list) };
            let cols = match same {
                Some(first) => BriefCols::Same(first.clone()),
                None => {
                    seen_cols.entry(list.clone()).or_insert_with(|| t.id());
                    if prio[k] {
                        seen_prio.insert(list.clone(), t.id());
                    }
                    BriefCols::Own(crate::brief::compress_names(&list), list.iter().map(|x| ident(x)).collect(), list.clone())
                }
            };
            rows.push(BriefRow { line: String::new(), cols, n: list.len(), view, member, prio: prio[k] });
        }
        // B100: one line per run of 2+ consecutive tables with an earlier table's columns and the
        // same facts (total rows, scale, period).
        let key_of = |r: &BriefRow| r.member.tail.replace(&format!("T{}_all", r.member.n), "T<n>_all");
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < rows.len() {
            let mut j = i + 1;
            if let BriefCols::Same(first) = &rows[i].cols
                && rows[i].view.is_empty()
                && !rows[i].prio
            {
                while j < rows.len()
                    && matches!(&rows[j].cols, BriefCols::Same(f) if f == first)
                    && rows[j].view.is_empty()
                    && !rows[j].prio
                    && rows[j].member.n == rows[i].member.n + (j - i)
                    && key_of(&rows[j]) == key_of(&rows[i])
                {
                    j += 1;
                }
            }
            if j - i >= 2 {
                spans.push((i, j));
                i = j;
            } else {
                i += 1;
            }
        }
        let mut merged: Vec<BriefRow> = Vec::new();
        let mut run: Vec<BriefRow> = Vec::new();
        for (k, r) in rows.into_iter().enumerate() {
            let Some(&(_, end)) = spans.iter().find(|(a, b)| (*a..*b).contains(&k)) else {
                merged.push(r);
                continue;
            };
            run.push(r);
            if k + 1 == end {
                let ms: Vec<crate::brief::RunMember> = run
                    .iter()
                    .map(|r| crate::brief::RunMember { id: format!("T{}", r.member.n), title: r.member.title, sheet: r.member.sheet })
                    .collect();
                let (lo, hi) = run.iter().fold((usize::MAX, 0), |(lo, hi), r| (lo.min(r.member.rows), hi.max(r.member.rows)));
                let n_rows = if lo == hi { format!(" · {} each", plural(lo, "row", "rows")) } else { format!(" · {lo}–{hi} rows") };
                let BriefCols::Same(first) = &run[0].cols else { unreachable!("runs repeat an earlier table") };
                let line = crate::brief::run_line(&ms, first, &format!("{n_rows}{}", key_of(&run[0])));
                let first = run.remove(0);
                run.clear();
                merged.push(BriefRow { line, cols: BriefCols::Run, view: String::new(), ..first });
            }
        }
        let rows = merged;
        // The order of the lines: B150: the question's tables first.
        let emit: Vec<usize> = (0..rows.len()).filter(|&k| rows[k].prio).chain((0..rows.len()).filter(|&k| !rows[k].prio)).collect();
        // Each line in parts: the id, the title (optional), the rest (the sheet unless it is the one
        // above, rows, period, total rows, scale), the columns, the view.
        let mut prev_sheet: Option<&str> = None;
        let mut rest: Vec<String> = vec![String::new(); rows.len()];
        for &k in &emit {
            let r = &rows[k];
            if matches!(r.cols, BriefCols::Run) {
                prev_sheet = None;
                continue;
            }
            let m = &r.member;
            let sheet = if prev_sheet == Some(m.sheet) {
                String::new()
            } else {
                format!(" ← {}", sheet_ref(m.sheet, "").trim_end_matches('!'))
            };
            prev_sheet = Some(m.sheet);
            rest[k] = format!("{sheet} · {}{}", plural(m.rows, "row", "rows"), m.tail);
        }
        // A title the table above has too is not repeated.
        let mut prev_title: Option<&str> = None;
        let mut titles: Vec<String> = vec![String::new(); rows.len()];
        for &k in &emit {
            let r = &rows[k];
            let t = r.member.title.filter(|t| prev_title != Some(*t));
            if r.member.title.is_some() {
                prev_title = r.member.title;
            }
            titles[k] = t.map(|t| format!(" '{}'", short(t, 60))).unwrap_or_default();
        }
        let title = |k: usize| titles[k].as_str();
        // The first names (keys, labels) within a few characters.
        let shortest = |plain: &[String]| -> String {
            let mut used = 0;
            let first: Vec<&str> = plain
                .iter()
                .take(2)
                .take_while(|x| {
                    used += x.len() + 1;
                    used <= 48
                })
                .map(String::as_str)
                .collect();
            match plain.len() - first.len() {
                0 => format!(": {}", first.join(" ")),
                m if first.is_empty() => format!(" (+{m})"),
                m => format!(": {} +{m} more", first.join(" ")),
            }
        };
        // B100: columns that are an earlier table's with one more header level, as a reference.
        let derived: Vec<Option<(usize, String)>> = (0..rows.len())
            .map(|k| {
                let BriefCols::Own(_, _, b) = &rows[k].cols else { return None };
                (0..k).find_map(|j| match &rows[j].cols {
                    BriefCols::Own(_, _, a) => crate::brief::derived(b, a, &format!("T{}", rows[j].member.n)).map(|t| (j, t)),
                    _ => None,
                })
            })
            .collect();
        // Detail levels: 0 nothing, 1 + title and first names, 2 + title and every name, 3 + title
        // and the names as an earlier table's (when that one has every name).
        let cols_text = |k: usize, level: u8| -> String {
            let r = &rows[k];
            match &r.cols {
                BriefCols::Own(items, plain, _) => match level {
                    0 => String::new(),
                    1 => format!(" · {} cols{}", r.n, shortest(plain)),
                    3 => format!(" · {} cols: {}", r.n, derived[k].as_ref().map(|d| d.1.as_str()).unwrap_or_default()),
                    _ => {
                        let grouped = items.join(" ");
                        let flat = plain.join(" ");
                        format!(" · {} cols: {}", r.n, if grouped.len() < flat.len() { grouped } else { flat })
                    }
                },
                BriefCols::Same(first) => format!(" · same columns as {first}"),
                BriefCols::Run => String::new(),
            }
        };
        let cost = |k: usize, level: u8| -> usize {
            let t = if level > 0 { title(k).len() } else { 0 };
            t + cols_text(k, level).len()
        };
        let fixed: usize = out.iter().map(|l| l.len() + 1).sum::<usize>()
            + rows.iter().zip(&rest).enumerate().map(|(k, (r, s))| 5 + r.line.len() + s.len() + r.view.len() + cost(k, 0)).sum::<usize>();
        let mut room = avail.saturating_sub(fixed);
        let mut level: Vec<u8> = vec![0; rows.len()];
        // Tables that repeat no other table's columns get detail: the first table of each sheet
        // first (the sheet's main table), then the others, each in table order; every name if it
        // fits, else the first names.
        // Tables of one or two rows (a header fragment, a note) come last.
        let rank = |k: usize| -> u8 {
            let m = &rows[k].member;
            match (m.rows >= 3, m.first_of_sheet) {
                (true, true) => 0,
                (true, false) => 1,
                _ => 2,
            }
        };
        let mut order: Vec<usize> = (0..rows.len()).filter(|&k| matches!(rows[k].cols, BriefCols::Own(..))).collect();
        // B150: the question's tables first (with their titles), then the others within `room_cap`.
        order.sort_by_key(|&k| (!rows[k].prio, rank(k), k));
        for k in (0..rows.len()).filter(|&k| rows[k].prio && matches!(rows[k].cols, BriefCols::Same(_))) {
            room = room.saturating_sub(title(k).len());
            level[k] = 1;
        }
        let mut capped = false;
        for &k in &order {
            if !capped && !rows[k].prio {
                capped = true;
                if let Some(cap) = room_cap {
                    room = room.min(cap);
                }
            }
            if level[k] > 0 {
                continue;
            }
            let by_ref = derived[k].as_ref().is_some_and(|(a, _)| level[*a] == 2);
            for l in [3u8, 2, 1] {
                if l == 3 && !by_ref {
                    continue;
                }
                let extra = cost(k, l) - cost(k, 0);
                if extra <= room {
                    room -= extra;
                    level[k] = l;
                    break;
                }
            }
            // A table with every name: the tables that repeat its columns with one more level
            // follow it, by reference (cheap, and often what a question needs: `% des BIP`).
            if level[k] == 2 {
                for j in (k + 1..rows.len()).filter(|&j| derived[j].as_ref().is_some_and(|(a, _)| *a == k)) {
                    let extra = cost(j, 3) - cost(j, 0);
                    if level[j] == 0 && extra <= room {
                        room -= extra;
                        level[j] = 3;
                    }
                }
            }
        }
        if !capped && let Some(cap) = room_cap {
            room = room.min(cap);
        }
        // Titles of tables with an earlier table's columns, in table order.
        for k in 0..rows.len() {
            if matches!(rows[k].cols, BriefCols::Same(_)) && title(k).len() <= room {
                room -= title(k).len();
                level[k] = 1;
            }
        }
        let mut after_prio = false;
        let first_line = out.len();
        // B150: with a question, 3+ consecutive other tables that show nothing but their sheet and
        // rows share one line: `T10..T26 · 17 tables, 34–35 rows ← sheets '1.4', '1.5', …`.
        let bare = |k: usize| {
            let r = &rows[k];
            mode != BriefMode::Plain
                && !r.prio
                && level[k] == 0
                && matches!(r.cols, BriefCols::Own(..))
                && r.view.is_empty()
                && r.member.tail.is_empty()
        };
        let mut i = 0;
        while i < emit.len() {
            let k = emit[i];
            let r = &rows[k];
            if mode == BriefMode::Focus && !r.prio && !after_prio {
                after_prio = true;
                out.push("(the other tables)".into());
            }
            let mut j = i;
            while j + 1 < emit.len() && bare(emit[j]) && bare(emit[j + 1]) && rows[emit[j + 1]].member.n == rows[emit[j]].member.n + 1 {
                j += 1;
            }
            if bare(k) && j >= i + 2 {
                let group: Vec<&BriefRow> = emit[i..=j].iter().map(|&x| &rows[x]).collect();
                let (lo, hi) = group.iter().fold((usize::MAX, 0), |(lo, hi), r| (lo.min(r.member.rows), hi.max(r.member.rows)));
                let n_rows = if lo == hi { format!("{} each", plural(lo, "row", "rows")) } else { format!("{lo}–{hi} rows") };
                let mut sheets: Vec<&str> = Vec::new();
                for r in &group {
                    if !sheets.contains(&r.member.sheet) {
                        sheets.push(r.member.sheet);
                    }
                }
                let mut shown: Vec<String> = sheets.iter().take(6).map(|s| sheet_ref(s, "").trim_end_matches('!').to_string()).collect();
                if sheets.len() > 6 {
                    shown.push(format!("+{} more", sheets.len() - 6));
                }
                let on = if sheets.len() == 1 { "sheet" } else { "sheets" };
                out.push(format!(
                    "T{}..T{} · {} tables, {n_rows} ← {on} {}",
                    group[0].member.n,
                    group[group.len() - 1].member.n,
                    group.len(),
                    shown.join(", ")
                ));
                i = j + 1;
                continue;
            }
            if matches!(r.cols, BriefCols::Run) {
                out.push(r.line.clone());
                i += 1;
                continue;
            }
            let t = if level[k] > 0 { title(k) } else { "" };
            out.push(format!("{:<3}{t}{}{}{}", format!("T{}", r.member.n), rest[k], cols_text(k, level[k]), r.view));
            i += 1;
        }
        // B150: with a question, the legend says only what the lines use.
        if mode != BriefMode::Plain {
            let lines = &out[first_line..];
            let mut legend = vec![what.to_string()];
            if lines.iter().any(|l| l.starts_with('T') && !l.contains(" ← ") && !l.contains(" same columns as ")) {
                legend.push("a table without ← is on the sheet above".into());
            }
            if lines.iter().any(|l| l.contains('{')) {
                legend.push(format!("column names grouped: {}", crate::brief::LEGEND));
            }
            out[0] = format!("({})", legend.join("; "));
        }
        out
    };

    let mut tail = Vec::new();
    tail.extend(formulas_line(norm)); // B48
    let (ok, bad, unk) = status_counts(norm.totals.iter());
    if ok + bad + unk > 0 {
        tail.push(format!("TOTALS {ok} ok, {bad} mismatch, {unk} unknown (details: totals)"));
    }
    // B104: notes and warnings only when there are some; notes without the per-kind counts.
    if !norm.notes.is_empty() {
        let mut s = format!("NOTES {}", plural(norm.notes.len(), "cell", "cells"));
        if let Some(n) = norm.notes.iter().find(|n| n.kind != "total_row" && n.kind != "repeated_header" && n.text.chars().count() > 12) {
            s.push_str(&format!("; e.g. {}: \"{}\"", sheet_ref(&n.sheet, &hh_core::a1(n.row, n.col)), short(&n.text, 90)));
        }
        tail.push(s);
    }
    if !norm.warnings.is_empty() {
        tail.push(format!(
            "WARNINGS {}: {}",
            norm.warnings.len(),
            norm.warnings.iter().take(3).map(|w| short(w, 100)).collect::<Vec<_>>().join("; ")
        ));
    }
    if norm.tables.len() == 1 {
        tail.push(
            "SQL view T1 = data rows, T1_all = + total rows (_row_type); both with _src_row; quote names: SELECT \"col name\" FROM T1"
                .into(),
        );
    } else if let (Some(first), Some(last)) = (norm.tables.first(), norm.tables.last()) {
        tail.push(format!(
            "SQL views {}..{} = data rows, {}_all..{}_all = + total rows (_row_type); all with _src_row; quote names: SELECT \"col name\" FROM {}",
            first.id(),
            last.id(),
            first.id(),
            last.id(),
            first.id()
        ));
    } else {
        tail.push("no tables detected; read values with the cells tool".into());
    }
    // B-public: key/value cells outside the tables, one short line for all of them.
    let kv: Vec<&Table> = norm.tables.iter().filter(|t| t.is_key_value()).collect();
    if let Some(first) = kv.first() {
        let list: Vec<String> = kv.iter().take(3).map(|t| format!("{} ({}, {})", t.id(), t.rows(), short(&t.source_sheet, 20))).collect();
        let more = if kv.len() > 3 { format!(" +{}", kv.len() - 3) } else { String::new() };
        tail.push(format!(
            "KEY/VALUE {}{more}: labelled numbers outside the tables — SELECT key, value, cell FROM {}",
            list.join(", "),
            first.id()
        ));
    }
    // B20: the long views (views.rs).
    let longs: Vec<String> = norm.tables.iter().filter_map(|t| crate::views::table_view(t).map(|lv| lv.name)).collect();
    if !longs.is_empty() {
        tail.push(format!(
            "SQL long {} {} (sum these; …_long_all adds the total rows): period columns as rows (period, period_label, value, _src_row, _src_col: the value's cell)",
            if longs.len() == 1 { "view" } else { "views" },
            id_list(&longs, 8)
        ));
    }
    // B79: union views (unions.rs); briefly when the catalog lists its tables briefly.
    let brief_mode = norm.tables.len() > BRIEF_TABLES_OVER;
    tail.extend(crate::unions::catalog_lines(norm, brief_mode));

    let join = |body: &[String]| -> String {
        let mut all = head.clone();
        all.extend(body.iter().cloned());
        all.extend(tail.iter().cloned());
        all.join("\n")
    };
    let fixed = len_of(&head) + len_of(&tail);
    let all = vec![true; norm.tables.len()];
    // B150: with a question, the tables it matches in full, then the others one line each.
    let none = vec![false; norm.tables.len()];
    let n = norm.tables.len();
    let cap_for = |others: usize| (OTHER_CHARS_PER_TABLE * others).clamp(OTHER_CHARS.0, OTHER_CHARS.1);
    // Every table in full when that fits (else one line each).
    let all_blocks: Option<Vec<Block>> = (n <= BRIEF_TABLES_OVER)
        .then(|| full(&all))
        .filter(|blocks| fixed + blocks.iter().map(|b| len_of(&b.lines)).sum::<usize>() <= budget + 1);
    let in_full = all_blocks.is_some();
    let (all2, none2) = (all.clone(), none.clone());
    let normal = move || -> String {
        if let Some(blocks) = all_blocks {
            let base: usize = fixed + blocks.iter().map(|b| len_of(&b.lines)).sum::<usize>();
            return join(&with_parts(blocks, budget + 1 - base));
        }
        // B104: a brief catalog fills its room with column names; this much is enough for the
        // main tables' names (de-gdp, uk-ons) and keeps big files cheap.
        let budget = budget.min(BRIEF_CHARS + BRIEF_CHARS_PER_TABLE * n);
        let body = brief(budget.saturating_sub(fixed + 50), &all2, &none2, &none2, None, BriefMode::Plain);
        let text = join(&body);
        if text.len() <= budget {
            return text;
        }
        // Still too long: keep as many table lines as fit, then say what was cut.
        let fixed = fixed + 200;
        let mut kept = Vec::new();
        let mut used = fixed;
        for l in &body {
            if used + l.len() + 1 > budget {
                break;
            }
            used += l.len() + 1;
            kept.push(l.clone());
        }
        let listed = kept.len().saturating_sub(1);
        let rest = n - listed;
        let first_cut = norm.tables.get(listed).map(|t| t.id()).unwrap_or_default();
        let last_id = norm.tables.last().map(|t| t.id()).unwrap_or_default();
        let text = format!(
            "{}\n… cut: {rest} more tables ({first_cut}..{last_id}) not listed; ask for describe_table with a table id, or query sqlite_master",
            join(&kept)
        );
        if text.len() <= budget {
            text
        } else {
            cut(text, budget, |n| format!("{n} more lines"), "describe_table with a table id")
        }
    };
    // B150: with a question, the tables it matches first. Like the catalog without it: in full
    // (the others one line each) when that lists every table in full, else every table one line
    // each, the matching ones first with every column name. Only when that is shorter.
    let Some(f) = question.and_then(|q| focus(norm, &covs, q)) else { return normal() };
    let ids = |ks: &[usize]| ks.iter().map(|&k| norm.tables[k].id()).collect::<Vec<_>>().join(", ");
    let rest_text = plural(n - f.ks.len(), "other table", "other tables");
    let mut focused: Option<String> = None;
    if in_full {
        let (top, more) = f.ks.split_at(f.ks.len().min(FOCUS_TABLES));
        let mut keep = none.clone();
        let mut prio = none.clone();
        top.iter().for_each(|&k| keep[k] = true);
        more.iter().for_each(|&k| prio[k] = true);
        let others: Vec<bool> = keep.iter().map(|k| !k).collect();
        let line = if more.is_empty() {
            format!("QUESTION matches {}: first, in full; then {rest_text}, one line each", f.items)
        } else {
            format!("QUESTION matches {}: {} first, in full; then {} and {rest_text}, one line each", f.items, ids(top), ids(more))
        };
        let blocks = full(&keep);
        let rest = brief(budget, &others, &keep, &prio, Some(cap_for(n - f.ks.len())), BriefMode::Others);
        let base: usize = fixed + line.len() + 1 + blocks.iter().map(|b| len_of(&b.lines)).sum::<usize>() + len_of(&rest);
        if base <= budget + 1 {
            let mut body = vec![line];
            body.extend(with_parts(blocks, budget + 1 - base));
            body.extend(rest);
            focused = Some(join(&body));
        }
    } else {
        let line = format!("QUESTION matches {}: first, with every column name; then {rest_text}", f.items);
        let mut prio = none.clone();
        f.ks.iter().for_each(|&k| prio[k] = true);
        let budget = budget.min(BRIEF_CHARS + BRIEF_CHARS_PER_TABLE * n);
        let mut body = vec![line];
        let avail = budget.saturating_sub(fixed + len_of(&body) + 50);
        body.extend(brief(avail, &all, &none, &prio, Some(cap_for(n - f.ks.len())), BriefMode::Focus));
        let text = join(&body);
        focused = (text.len() <= budget).then_some(text);
    }
    let normal = normal();
    match focused {
        Some(text) if text.len() < normal.len() => text,
        _ => normal,
    }
}

// ------------------------------------------------------------------------------------------------
// describe_table

/// Finds a table by id (`T2`, `t2`, `2`, `T2_all`) or output sheet name.
pub fn find_table<'a>(norm: &'a Normalized, id: &str) -> Result<&'a Table, String> {
    let raw = id.trim().trim_matches('"');
    let key = raw.strip_suffix("_all").unwrap_or(raw);
    let key = key.strip_suffix("_long").unwrap_or(key); // B20: T<n>_long, T<n>_long_all
    let n = key.trim_start_matches(['T', 't']).parse::<usize>().ok();
    norm.tables
        .iter()
        .find(|t| Some(t.n) == n || t.sheet.eq_ignore_ascii_case(raw))
        .ok_or_else(|| {
            if norm.tables.is_empty() {
                "this file has no tables; read values with the cells tool".to_string()
            } else {
                format!(
                    "no table {raw:?}; tables are {}",
                    norm.tables.iter().map(|t| t.id()).collect::<Vec<_>>().join(", ")
                )
            }
        })
}

pub fn describe_table(norm: &Normalized, t: &Table) -> String {
    let info = row_info(norm, t);
    let names = sql_names(t);
    let mut lines = Vec::new();
    let mut head = format!(
        "{} = {} · {} · confidence {:.2}",
        t.id(),
        sheet_ref(&t.source_sheet, &range_a1(t.rect)),
        plural(info.data, "data row", "data rows"),
        t.confidence
    );
    if let Some((title, (r, c))) = &t.title {
        head.push_str(&format!(" · title \"{}\" ({})", short(title, 120), hh_core::a1(*r, *c)));
    }
    lines.push(head);
    if t.header_rows > 0 {
        let top = t.header_top;
        let bottom = top + t.header_rows - 1;
        lines.push(format!(
            "header: {} ({} row{}; names below are flattened from it)",
            sheet_ref(
                &t.source_sheet,
                &range_a1(hh_core::Rect { top, left: t.rect.left, bottom, right: t.rect.right })
            ),
            t.header_rows,
            if t.header_rows == 1 { "" } else { "s" }
        ));
    } else {
        lines.push("header: none detected (columns named col_<letter>)".into());
    }
    if let Some(g) = t.columns.iter().position(|c| c.role == ColRole::Group) {
        lines.push(format!("group column: {} (labels of group-header rows, filled down)", ident(&names[g])));
    }
    // B74: every total row by level, the grand total named, what each adds up.
    let entries = total_entries(&info);
    if !entries.is_empty() {
        lines.push(format!(
            "total rows (only in {id}_all, _row_type, unless marked): {}",
            totals_text(t, &info, &entries, true, 24),
            id = t.id()
        ));
    }
    if !info.kept_totals.is_empty() {
        lines.push(format!("total rows that stay in {} (exclude before summing): {}", t.id(), total_rows_text(&info.kept_totals, 12)));
    }
    let mixed = mixed_measures_by(t, &data_rows(t));
    if let Some(i) = mixed {
        lines.push(format!("rows mix measures by {} — filter before aggregating", ident(&names[i])));
    }
    // B20 long view, B21 semantic types (views.rs, types.rs)
    let tys = types::classify(t, &names, mixed, &types::summed_cols(norm, t));
    if let Some(lv) = crate::views::long_view(t, &tys) {
        lines.push(crate::views::catalog_line(t, &lv, mixed, !info.total_rows.is_empty()).trim_start().to_string());
    }
    lines.extend(types::no_sum_line(t, &names, &tys, mixed).map(|l| l.trim_start().to_string()));
    lines.extend(facts::scale_line(t).map(|l| l.trim_start().to_string())); // B85
    lines.extend(facts::column_notes_line(norm, t, &names).map(|l| l.trim_start().to_string())); // B83
    lines.extend(rolling_text(t, &mut HashMap::new()).map(|l| l.trim_start().to_string())); // B162
    // B80: ID columns with their names in another column.
    let pairs = crate::entity::id_name_pairs(t, &data_rows(t));
    lines.extend(pairs.iter().map(|p| crate::entity::id_name_line(p, &t.id()).trim_start().to_string()));
    lines.push("columns (name | dtype · type | unit | nulls | distinct | min..max, example or all values | source column ← header cells):".into());
    let data = data_rows(t);
    let cols: Vec<(usize, &Column)> = data_cols(t).collect();
    for (k, (i, c)) in cols.iter().enumerate() {
        if k == DESCRIBE_COLS {
            let rest: Vec<String> = cols[k..].iter().take(60).map(|(i, _)| ident(&short(&names[*i], 30))).collect();
            lines.push(format!("  … {} more columns: {}", cols.len() - k, rest.join(", ")));
            break;
        }
        let st = col_stats(c);
        let unit = c.currency.clone().or(c.unit.clone()).unwrap_or_default();
        let range = match (st.min, st.max) {
            (Some(lo), Some(hi)) if is_numeric(c.dtype) || is_date(c.dtype) => {
                format!("{}..{}", bound(lo, c.dtype), bound(hi, c.dtype))
            }
            _ if c.dtype == Dtype::Bool => BOOL_STORED.to_string(), // B82
            _ => match label_values(c, &data) {
                Some(vals) if vals.len() > 1 => values_text(&vals),
                _ => st.example.as_ref().map(|e| format!("e.g. \"{}\"", short(e, 40))).unwrap_or_default(),
            },
        };
        let src = match c.source_col {
            Some(col) => {
                let hdr: Vec<String> = c
                    .original_header
                    .iter()
                    .take(3)
                    .map(|(text, (r, cc))| format!("{} \"{}\"", hh_core::a1(*r, *cc), short(text, 30)))
                    .collect();
                if hdr.is_empty() { letter(col) } else { format!("{} ← {}", letter(col), hdr.join(" / ")) }
            }
            None => c.role.as_str().to_string(),
        };
        let dtype = match tys[*i].as_ref() {
            Some(ty) => format!("{} · {}", c.dtype.as_str(), ty.tag()), // B21
            None => c.dtype.as_str().to_string(),
        };
        lines.push(format!(
            "  {} | {} | {} | {} | {} | {} | {}",
            ident(&names[*i]),
            dtype,
            unit,
            st.nulls,
            st.distinct,
            range,
            src
        ));
    }
    if !t.notes.is_empty() {
        let n: Vec<String> =
            t.notes.iter().take(6).map(|(s, (r, c))| format!("{}: \"{}\"", hh_core::a1(*r, *c), short(s, 80))).collect();
        lines.push(format!("notes: {}", n.join(" | ")));
    }
    if !t.warnings.is_empty() {
        let w: Vec<String> = t.warnings.iter().take(4).map(|w| short(w, 120)).collect();
        lines.push(format!("warnings ({}): {}", t.warnings.len(), w.join("; ")));
    }
    // First 5 data rows, at most 12 columns, plus _src_row.
    let mut show: Vec<usize> = cols.iter().map(|(i, _)| *i).take(12).collect();
    if let Some(src) = t.columns.iter().position(|c| c.role == ColRole::SrcRow) {
        show.push(src);
    }
    let rows: Vec<usize> = (0..t.rows()).filter(|&r| t.row_types.get(r).is_none_or(|x| *x == RowType::Data)).take(5).collect();
    lines.push(format!("first {} rows{}:", rows.len(), if cols.len() > 12 { " (first 12 columns)" } else { "" }));
    lines.push(csv_row(&show.iter().map(|&i| short(&names[i], 80)).collect::<Vec<_>>()));
    for r in rows {
        let vals: Vec<String> = show
            .iter()
            .map(|&i| {
                let c = &t.columns[i];
                short(&c.values.get(r).map(|v| out_text(v, c.dtype)).unwrap_or_default(), 60)
            })
            .collect();
        lines.push(csv_row(&vals));
    }
    cut(lines.join("\n"), MAX_CHARS, |n| format!("{n} more lines"), "query on the table for specific columns")
}

// ------------------------------------------------------------------------------------------------
// totals

/// The totals check: counts per status, then up to 50 items, mismatches first.
pub fn totals(norm: &Normalized, table: Option<&Table>, status: Option<&str>) -> Result<String, String> {
    if let Some(s) = status
        && !["ok", "mismatch", "unknown"].contains(&s)
    {
        return Err(format!("status must be ok, mismatch or unknown (got {s:?})"));
    }
    let items: Vec<&hh_core::normalize::TotalItem> = norm
        .totals
        .iter()
        .filter(|x| table.is_none_or(|t| x.table == Some(t.n)))
        .filter(|x| status.is_none_or(|s| (s == "unknown" && x.status != "ok" && x.status != "mismatch") || x.status == s))
        .collect();
    let (ok, bad, unk) = status_counts(items.iter().copied());
    let scope = match (table, status) {
        (Some(t), Some(s)) => format!(" ({}, status {s})", t.id()),
        (Some(t), None) => format!(" ({})", t.id()),
        (None, Some(s)) => format!(" (status {s})"),
        (None, None) => String::new(),
    };
    let mut lines = vec![format!("TOTALS{scope}: {ok} ok, {bad} mismatch, {unk} unknown")];
    if items.is_empty() {
        lines.push("no totals found".into());
        return Ok(lines.join("\n"));
    }
    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for x in items.iter().filter(|x| x.status != "ok" && x.status != "mismatch") {
        *reasons.entry(x.reason.as_str()).or_default() += x.count;
    }
    if !reasons.is_empty() {
        let r: Vec<String> = reasons.iter().map(|(k, v)| format!("{k} {v}")).collect();
        lines.push(format!("unknown because: {}", r.join(", ")));
    }
    let rank = |x: &hh_core::normalize::TotalItem| -> (u8, u8) {
        let s = match x.status {
            "mismatch" => 0,
            "ok" => 1,
            _ => 2,
        };
        let l = match x.level {
            "grand_total" => 0,
            "total" => 1,
            _ => 2,
        };
        (s, l)
    };
    let mut sorted = items.clone();
    sorted.sort_by_key(|x| rank(x));
    lines.push("status,table,cell,label,level,value,recomputed,sources,reason".into());
    for x in sorted.iter().take(TOTAL_ITEMS) {
        let cell = sheet_ref(&x.sheet, &x.cell_a1());
        let cell = if x.count > 1 { format!("{cell} ({} totals)", x.count) } else { cell };
        lines.push(csv_row(&[
            x.status.to_string(),
            x.table.map(|n| format!("T{n}")).unwrap_or_default(),
            cell,
            short(x.label.as_deref().unwrap_or(""), 40),
            x.level.to_string(),
            x.value.map(num).unwrap_or_default(),
            x.recomputed.map(num).unwrap_or_default(),
            short(&x.sources.join(" "), 60),
            short(&x.reason, 60),
        ]));
    }
    if sorted.len() > TOTAL_ITEMS {
        lines.push(format!(
            "… cut: {} more items not listed; ask for totals with table=T<n> or status=mismatch|ok|unknown",
            sorted.len() - TOTAL_ITEMS
        ));
    }
    Ok(cut(lines.join("\n"), MAX_CHARS, |n| format!("{n} more lines"), "totals with table=T<n> or a status"))
}

/// B48: formula cells saved without a value (the file was written by a tool that does not
/// calculate): their table values are NULL. Count and the first such cell, to check with `cells`.
pub fn formulas_line(norm: &Normalized) -> Option<String> {
    let n: usize = norm.sheets.iter().map(|s| s.formulas_without_value).sum();
    let s = norm.sheets.iter().find(|s| s.formulas_without_value > 0)?;
    let (r, c) = s.formula_without_value_at?;
    Some(format!(
        "FORMULAS {} without a saved value (e.g. {}): NULL in the tables, not evaluated; cells shows the formula",
        plural(n, "cell", "cells"),
        sheet_ref(&s.name, &hh_core::a1(r, c))
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hh_core::normalize::{SheetInfo, Transforms};
    use hh_core::Rect;

    fn col(name: &str, role: ColRole, dtype: Dtype, values: Vec<Out>) -> Column {
        Column {
            name: name.into(),
            role,
            source_col: Some(1),
            original_header: vec![(name.to_string(), (0, 1))],
            dtype,
            unit: None,
            currency: None,
            decimal: None,
            date_order: None,
            ambiguous: 0,
            footnote_marks: Vec::new(),
            values,
        }
    }

    /// `n` tables of `width` year columns (2001…) and 5 rows each, the last a total row.
    fn book(n: usize, width: usize, same: bool) -> Normalized {
        let rows = 5;
        let tables = (1..=n)
            .map(|k| {
                let mut columns = vec![col("Region", ColRole::Data, Dtype::Text, (0..rows).map(|i| Out::Text(if i == rows - 1 { "Total".into() } else { format!("r{i}") })).collect())];
                for w in 0..width + if same { 0 } else { k } {
                    let name = if same { format!("{}", 2001 + w) } else { format!("Measure {k}-{w} long header text") };
                    columns.push(col(&name, ColRole::Data, Dtype::Number, (0..rows).map(|i| Out::Num((i * w) as f64 + 0.25)).collect()));
                }
                columns.push(col("_src_row", ColRole::SrcRow, Dtype::Integer, (0..rows).map(|i| Out::Num(i as f64 + 2.0)).collect()));
                Table {
                    n: k,
                    sheet: format!("T{k}_s"),
                    source_sheet: format!("Sheet {k}"),
                    rect: Rect { top: 0, left: 0, bottom: rows as u32, right: width as u32 },
                    header_top: 0,
                    header_rows: 1,
                    title: Some((format!("Table {k} title"), (0, 0))),
                    notes: vec![],
                    confidence: 0.9,
                    columns,
                    src_rows: (1..=rows as u32).collect(),
                    row_types: (0..rows).map(|i| if i == rows - 1 { RowType::Total } else { RowType::Data }).collect(),
                    transforms: Transforms::default(),
                    warnings: vec![],
                }
            })
            .collect();
        let sheets = (1..=n).map(|k| SheetInfo { name: format!("Sheet {k}"), tables: vec![k], cells: 10, ..Default::default() }).collect();
        Normalized { sheets, tables, ..Default::default() }
    }

    #[test]
    fn small_catalog_is_full() {
        let c = catalog(&book(2, 3, true), "x.xlsx", MAX_CHARS);
        assert!(c.starts_with("FILE x.xlsx · 2 sheets · "), "{c}");
        assert!(c.contains("TIME 2001–2003 (year, from column headers in T1, T2)"), "{c}");
        // B104: the total-row labels are on the `total rows:` line, not on the head line too.
        assert!(c.contains("T1  'Table 1 title' ← 'Sheet 1'!A1:D6 · 4 rows (+1 total row only in T1_all) · time 2001–2003 (year)"), "{c}");
        assert!(c.contains("columns (4): Region \"2001\"[0.25..0.25]"), "{c}");
        assert!(c.contains("T2  'Table 2 title' ← 'Sheet 2'!A1:D6 · 4 rows (+1 total row only in T2_all) · time 2001–2003 (year) · same columns as T1"), "{c}");
        // Small tables: all rows inline (total rows marked), so the values to filter on need no
        // extra line.
        assert!(
            c.contains(
                "\n    all 5 rows (CSV, as in T1_all; rows with _row_type total/subtotal are not in T1):\n    Region,2001,2002,2003,_row_type\n    r0,0.25,0.25,0.25,data\n    r1,0.25,1.25,2.25,data\n"
            ),
            "{c}"
        );
        assert!(c.contains("\n    Total,0.25,4.25,8.25,total\n"), "{c}");
        assert_eq!(c.matches("all 5 rows (CSV").count(), 2, "{c}");
        // How to select the total rows.
        assert!(c.contains("\n    total rows: SELECT … FROM T1_all WHERE _row_type = 'total' — labels in Region: total 'Total'@6\n"), "{c}");
        assert!(!c.contains("values:"), "{c}");
        assert!(!c.contains("… cut:"));
    }

    #[test]
    fn inline_rows_only_within_budget() {
        let n = book(2, 3, true);
        let full = catalog(&n, "x.xlsx", MAX_CHARS);
        // Room for one table's rows only: the first stays inline, the second is dropped.
        let c = catalog(&n, "x.xlsx", full.len() - 10);
        assert!(c.len() <= full.len() - 10, "{c}");
        assert_eq!(c.matches("all 5 rows (CSV").count(), 1, "{c}");
        assert!(!c.contains("tables listed briefly"), "{c}");
        // Up to 400 cells (rows × columns with _row_type) inlined, whatever the width.
        assert!(catalog(&book(1, 50, true), "x.xlsx", MAX_CHARS).contains("all 5 rows (CSV"));
        // More than 400 cells or 30 rows: not inlined.
        assert!(!catalog(&book(1, 80, true), "x.xlsx", MAX_CHARS).contains("(CSV"));
        let tall = |n: usize| {
            let mut b = book(1, 2, true);
            let t = &mut b.tables[0];
            for c in &mut t.columns {
                let v = c.values[0].clone();
                c.values = vec![v; n];
            }
            t.src_rows = (1..=n as u32).collect();
            t.row_types = vec![RowType::Data; n];
            b
        };
        assert!(catalog(&tall(30), "x.xlsx", MAX_CHARS).contains("all 30 rows (CSV):"));
        assert!(!catalog(&tall(31), "x.xlsx", MAX_CHARS).contains("(CSV"));
    }

    #[test]
    fn every_column_listed_up_to_40() {
        // 1 + 31 columns with distinct names: all listed by name (grouped by their prefix), no "more".
        let c = catalog(&book(1, 30, false), "x.xlsx", MAX_CHARS);
        for w in 0..31 {
            assert!(c.contains(&format!("1-{w} long header text")), "{w}: {c}");
        }
        assert!(!c.contains("more"), "{c}");
        // Year columns: one run, nothing hidden.
        let c = catalog(&book(1, 15, true), "x.xlsx", MAX_CHARS);
        assert!(c.contains("columns (16): Region \"2001\"..\"2015\"[15 cols]"), "{c}");
        // B71: wider than 40 and long: grouped by name prefix, every member named.
        let c = catalog(&book(1, 90, false), "x.xlsx", MAX_CHARS);
        assert!(
            c.contains("columns (92; a_{b, c} = a_b, a_c): Region \"Measure {1-0 long header text, 1-1 long header text, "),
            "{c}"
        );
        assert!(c.contains(", 1-90 long header text}\""), "{c}");
        // Like owid-energy: prefixes of snake_case names, a member of another type marked.
        let mut n = book(1, 1, false);
        let t = &mut n.tables[0];
        let src = t.columns.pop().unwrap();
        for f in ["biofuel", "coal", "gas", "hydro", "nuclear", "oil", "solar", "wind", "fossil"] {
            for m in ["cons_change_pct", "cons_change_twh", "consumption", "elec_per_capita", "electricity", "share_elec", "share_energy", "energy_per_capita", "production"] {
                t.columns.push(col(&format!("{f}_{m}"), ColRole::Data, Dtype::Number, (0..5).map(|i| Out::Num(i as f64 + 0.5)).collect()));
            }
        }
        t.columns.push(src);
        let c = catalog(&n, "x.xlsx", MAX_CHARS);
        assert!(c.contains("solar_{cons_change_pct[rate], cons_change_twh, consumption, elec_per_capita, electricity, share_elec[rate], share_energy[rate], energy_per_capita, production}"), "{c}");
        assert!(!c.contains(".."), "{c}");
    }

    #[test]
    fn period_names() {
        assert_eq!(name_period("2019"), Some((0, 2019, String::new())));
        assert_eq!(name_period("Thu / 2002-01-31").map(|p| p.2), Some(String::new()));
        assert_eq!(name_period("Thu / 2002-01-31").unwrap().1 - name_period("Wed / 2002-01-30").unwrap().1, 1);
        assert_eq!(name_period("Thu / 2002-03-01").unwrap().1 - name_period("Wed / 2002-02-28").unwrap().1, 1);
        assert_eq!(name_period("Sales 2024-03"), Some((1, 2024 * 12 + 3, "sales".into())));
        assert_eq!(name_period("MTD Avg / 2002-02-05").unwrap().2, "mtd avg");
        assert_eq!(name_period("12345"), None);
        assert_eq!(name_period("2020/ 2025"), None);
    }

    /// One table like the Austrian pension file: a group column with three measures (counts,
    /// euro amounts, rates) and a status column, years as columns.
    fn stacked() -> Normalized {
        let mut n = book(1, 4, true);
        let t = &mut n.tables[0];
        let groups =
            ["Anzahl Bezieher:innen", "Anzahl Bezieher:innen", "Durchschnittshöhe in Euro", "Durchschnittshöhe in Euro", "Richtsätze in Euro"];
        let status = ["Alleinstehend", "Verheiratet", "Alleinstehend", "Verheiratet", "Alleinstehend"];
        let size = [196_489.0, 38_182.0, 264.3, 372.97, 793.4];
        let mut cols = vec![
            col("group", ColRole::Group, Dtype::Text, groups.iter().map(|s| Out::Text(s.to_string())).collect()),
            col("Status", ColRole::Data, Dtype::Text, status.iter().map(|s| Out::Text(s.to_string())).collect()),
        ];
        for w in 0..4 {
            let vals = size.iter().map(|v| Out::Num(v * (1.0 + w as f64 / 50.0))).collect();
            cols.push(col(&format!("{}", 2011 + w), ColRole::Data, Dtype::Number, vals));
        }
        cols.push(t.columns.last().unwrap().clone());
        t.columns = cols;
        t.row_types = vec![RowType::Data; 5];
        n
    }

    #[test]
    fn label_values_and_mixed_measures() {
        let n = stacked();
        let c = catalog(&n, "x.xlsx", MAX_CHARS);
        // 5 rows × 6 columns: inlined, the measures are visible in the rows.
        assert!(c.contains("rows mix measures by \"group\" — filter before aggregating"), "{c}");
        assert!(c.contains("all 5 rows (CSV)"), "{c}");
        // Too wide to inline: the values are listed.
        let mut wide = n.clone();
        for w in 4..84 {
            let mut x = wide.tables[0].columns[2].clone();
            x.name = format!("{}", 2011 + w);
            let at = wide.tables[0].columns.len() - 1;
            wide.tables[0].columns.insert(at, x);
        }
        let c = catalog(&wide, "x.xlsx", MAX_CHARS);
        assert!(!c.contains("(CSV)"), "{c}");
        assert!(
            c.contains("\n    \"group\" values: \"Anzahl Bezieher:innen\" | \"Durchschnittshöhe in Euro\" | \"Richtsätze in Euro\"\n"),
            "{c}"
        );
        assert!(c.contains("\n    Status values: \"Alleinstehend\" | \"Verheiratet\"\n"), "{c}");
        assert!(c.contains("rows mix measures by \"group\""), "{c}");
        let d = describe_table(&wide, &wide.tables[0]);
        assert!(d.contains("rows mix measures by \"group\""), "{d}");
        assert!(d.contains("| \"Alleinstehend\" | \"Verheiratet\" |"), "{d}");
        // By size alone (labels without units): still found.
        let mut plain = n.clone();
        for (k, v) in plain.tables[0].columns[0].values.iter_mut().enumerate() {
            *v = Out::Text(["A", "A", "B", "B", "C"][k].into());
        }
        assert!(catalog(&plain, "x.xlsx", MAX_CHARS).contains("rows mix measures by \"group\""));
        // Regions of similar size: one measure.
        let c = catalog(&book(1, 3, true), "x.xlsx", MAX_CHARS);
        assert!(!c.contains("mix measures"), "{c}");
        // More than 12 distinct values: not listed, unless the table is small (B102).
        let rows_of = |n: usize| {
            let mut many = book(1, 13, true);
            let t = &mut many.tables[0];
            for c in &mut t.columns {
                let v = c.values[0].clone();
                c.values = vec![v; n];
            }
            t.columns[0].values = (0..n).map(|k| Out::Text(format!("r{k}"))).collect();
            t.src_rows = (1..=n as u32).collect();
            t.row_types = vec![RowType::Data; n];
            many
        };
        let c = catalog(&rows_of(31), "x.xlsx", MAX_CHARS);
        assert!(c.contains("Region[id e.g. \"r0\"]"), "{c}");
        assert!(!c.contains("Region values"), "{c}");
        // B102: a small table (≤ 30 rows) too wide to inline (30 × 15 cells) lists every row label.
        let c = catalog(&rows_of(30), "x.xlsx", MAX_CHARS);
        assert!(!c.contains("(CSV"), "{c}");
        assert!(c.contains("\n    Region values: \"r0\" | \"r1\" | "), "{c}");
        assert!(c.contains(" | \"r29\"\n"), "{c}");
    }

    #[test]
    fn time_from_the_manifest() {
        // Monthly dates in a column: the manifest's coverage, and the same as hh_core's.
        let mut n = book(1, 1, false);
        let t = &mut n.tables[0];
        // 2024-01-01 .. 2024-05-01
        let months: Vec<Out> = [0.0, 31.0, 60.0, 91.0, 121.0].iter().map(|d| Out::Date(45292.0 + d)).collect();
        t.columns.insert(0, col("Datum", ColRole::Data, Dtype::Date, months));
        t.row_types = vec![RowType::Data; 5];
        let wb = time_coverage(&n).unwrap();
        assert_eq!((wb.from.as_str(), wb.to.as_str(), wb.granularity), ("2024-01", "2024-05", "month"));
        let c = catalog(&n, "report_2019.xlsx", MAX_CHARS);
        assert!(
            c.contains("TIME 2024-01–2024-05 (month, from columns T1.Datum; titles/notes/sheet or file names also mention 2019)"),
            "{c}"
        );
        // No dates in the data: only the weak hint.
        let mut n = book(1, 1, false);
        n.tables[0].title = Some(("Sales report 2023".into(), (0, 0)));
        let c = catalog(&n, "x.xlsx", MAX_CHARS);
        assert!(c.contains("TIME 2023 (only mentioned in titles/notes/sheet or file names; no date or period columns)"), "{c}");
        let mut n = book(1, 1, false);
        n.tables[0].title = None;
        assert!(catalog(&n, "x.xlsx", MAX_CHARS).contains("TIME no dates or periods found"));
    }

    fn total_item(table: usize, row: u32, label: &str) -> hh_core::normalize::TotalItem {
        hh_core::normalize::TotalItem {
            table: Some(table),
            sheet: "Sheet 1".into(),
            row,
            col: 1,
            count: 1,
            row_end: row,
            label: Some(label.into()),
            label_cell: Some((row, 0)),
            level: "total",
            direction: "vertical",
            origin: "typed",
            value: Some(1.0),
            recomputed: None,
            status: "unknown",
            reason: "not_verified".into(),
            sources: vec![],
            formula: None,
            kept_in_data: true,
        }
    }

    #[test]
    fn totals_from_the_manifest() {
        let mut n = book(1, 3, true);
        // Source row 3 (0-based 2) is a data row: a total the check found that stays in T1;
        // source row 6 is the removed total row.
        n.totals = vec![total_item(1, 2, "Insgesamt"), total_item(1, 5, "Gesamt")];
        let info = row_info(&n, &n.tables[0]);
        assert_eq!(info.totals, hh_core::normalize::table_totals(&n.tables[0], &n.totals));
        assert_eq!(info.totals.unknown, 2);
        assert_eq!(info.kept_totals.iter().map(|r| (r.label.as_str(), r.row)).collect::<Vec<_>>(), [("Insgesamt", 3)]);
        assert_eq!(info.total_rows.iter().map(|r| (r.label.as_str(), r.row)).collect::<Vec<_>>(), [("Gesamt", 6)]);
        let c = catalog(&n, "x.xlsx", MAX_CHARS);
        // The total-rows line has no labels here (no label column), so the head line keeps them.
        assert!(
            c.contains("(+1 total row: 'Gesamt'@6 — only in T1_all) · 1 total row stays in T1 ('Insgesamt'@3) — exclude it before summing"),
            "{c}"
        );
        // B104: the per-table check counts only when one failed (the TOTALS line has them).
        assert!(!c.contains("totals: 0 ok"), "{c}");
        let d = describe_table(&n, &n.tables[0]);
        assert!(d.contains("total rows that stay in T1 (exclude before summing): 'Insgesamt'@3"), "{d}");
    }

    #[test]
    fn long_catalogs_go_brief_then_cut() {
        // 40 wide tables: too long in full, fits briefly.
        let c = catalog(&book(40, 20, false), "x.xlsx", MAX_CHARS);
        assert!(c.len() <= MAX_CHARS);
        assert!(c.contains("(tables listed briefly; "), "{c}");
        assert!(c.contains("T30 "), "{c}");
        // 400 tables: cut, with an explicit last line.
        let c = catalog(&book(400, 40, false), "x.xlsx", MAX_CHARS);
        assert!(c.len() <= MAX_CHARS, "{}", c.len());
        let last = c.lines().last().unwrap();
        assert!(last.starts_with("… cut: ") && last.contains("more tables") && last.contains("ask for"), "{last}");
        assert!(c.contains("SQL views T1..T400"));
        // A smaller budget is honoured too.
        assert!(catalog(&book(400, 40, false), "x.xlsx", 3000).len() <= 3000);
    }

    #[test]
    fn describe_and_totals() {
        let n = book(1, 70, false);
        let d = describe_table(&n, &n.tables[0]);
        assert!(d.len() <= MAX_CHARS);
        assert!(d.contains("total rows (only in T1_all, _row_type, unless marked): total 'Total'@6"), "{d}");
        assert!(d.contains("… 12 more columns: "), "{d}");
        assert!(d.contains("first 4 rows (first 12 columns):"), "{d}");
        assert!(find_table(&n, "t1").is_ok() && find_table(&n, "T1_all").is_ok());
        assert!(find_table(&n, "T7").unwrap_err().contains("tables are T1"));
        assert!(totals(&n, None, Some("weird")).is_err());
        assert_eq!(totals(&n, None, None).unwrap(), "TOTALS: 0 ok, 0 mismatch, 0 unknown\nno totals found");
    }

    #[test]
    fn total_rows_by_level() {
        // B74, like the Enron O&M budget: two subtotals, 'Total O&M' over them, and 'Total
        // Expenses' over 'Total O&M' (the check calls 'Total O&M' the grand total; the inclusion
        // says otherwise).
        let mut n = book(1, 3, true);
        let t = &mut n.tables[0];
        let labels = ["a", "Subtotal Fixed", "b", "Subtotal Variable", "Total O&M", "Total Expenses"];
        let types = [RowType::Data, RowType::Subtotal, RowType::Data, RowType::Subtotal, RowType::Total, RowType::Total];
        for c in &mut t.columns {
            let v = c.values[0].clone();
            c.values = vec![v; 6];
        }
        t.columns[0].values = labels.iter().map(|s| Out::Text(s.to_string())).collect();
        t.src_rows = (1..=6).collect();
        t.row_types = types.to_vec();
        let item = |row: u32, label: &str, level: &'static str, sources: &[&str]| hh_core::normalize::TotalItem {
            level,
            status: "ok",
            kept_in_data: false,
            sources: sources.iter().map(|s| s.to_string()).collect(),
            ..total_item(1, row, label)
        };
        n.totals = vec![
            item(2, "Subtotal Fixed", "subtotal", &["B2:B2"]),
            item(4, "Subtotal Variable", "subtotal", &["B4"]),
            item(5, "Total O&M", "grand_total", &["B3 B5"]),
            item(6, "Total Expenses", "total", &["B6"]),
        ];
        let c = catalog(&n, "x.xlsx", MAX_CHARS);
        assert!(
            c.contains("labels in Region: grand total 'Total Expenses'@7 (= rows 6); total 'Total O&M'@6 (= rows 3, 5); subtotals (_row_type 'subtotal', not 'total') 'Subtotal Fixed'@3 (= rows 2), 'Subtotal Variable'@5 (= rows 4)"),
            "{c}"
        );
        // describe_table: the same levels.
        let d = describe_table(&n, &n.tables[0]);
        assert!(d.contains("grand total 'Total Expenses'@7"), "{d}");
    }

    #[test]
    fn booleans_as_stored() {
        // B82: SQL stores booleans as INTEGER 1 / 0; the catalog says so and shows 1, not "TRUE".
        let mut n = book(1, 1, true);
        let t = &mut n.tables[0];
        t.columns.insert(1, col("Returned", ColRole::Data, Dtype::Bool, (0..5).map(|i| Out::Bool(i % 2 == 0)).collect()));
        let c = catalog(&n, "x.xlsx", MAX_CHARS);
        assert!(c.contains("Returned[bool 1/0 in SQL (TRUE/yes = 1, FALSE/no = 0; filter = 1, not 'TRUE')]"), "{c}");
        assert!(c.contains("\n    r0,1,0.25,data\n"), "{c}");
        assert!(!c.contains("TRUE\""), "{c}");
        let d = describe_table(&n, &n.tables[0]);
        assert!(d.contains("  Returned | bool · dim |  | 0 | 2 | 1/0 in SQL"), "{d}");
    }

    #[test]
    fn sql_names_are_unique_without_case() {
        let mut n = book(1, 1, true);
        n.tables[0].columns[1].name = "region".into();
        assert_eq!(sql_names(&n.tables[0]), ["Region", "region (2)", "_src_row"]);
    }
}
