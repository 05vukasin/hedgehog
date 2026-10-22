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

/// Tables above this count are always listed one line each.
const BRIEF_TABLES_OVER: usize = 40;
/// Up to this many columns a table's columns are listed with ranges / examples.
const DETAIL_COLS: usize = 14;
/// Up to this many columns every column is listed by name (compactly, `name[dtype unit]`); wider
/// tables group runs of same-type columns.
const LIST_COLS: usize = 40;
/// A wide table's plain columns list up to this many characters; longer, it groups names by
/// prefix (B71: every name stays visible).
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
    let r: f64 = format!("{v:.5e}").parse().unwrap_or(v);
    num(r)
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

/// A brief table line in the making: the line so far, its column names in priority order with
/// their positions (None: same columns as an earlier table), the column count, the long-view and
/// notes text.
type BriefRow = (String, Option<Vec<(usize, String)>>, usize, String);

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
        let items: Vec<String> =
            entries.iter().filter(|e| e.kind == kind && !e.grand).take(max.saturating_sub(shown)).map(item).collect();
        if !items.is_empty() {
            shown += items.len();
            let name = if items.len() == 1 { one } else { many };
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

fn kind_text(c: &Column) -> String {
    let mut kind = c.dtype.as_str().to_string();
    if let Some(u) = c.currency.as_ref().or(c.unit.as_ref()) {
        kind.push(' ');
        kind.push_str(u);
    }
    if c.role == ColRole::Group {
        kind.push_str(" group");
    }
    kind
}

/// One column with its range (numbers, dates) or an example (text, unless its values are listed).
fn col_detail(c: &Column, name: &str, rows: usize, example: bool, ty: Option<&ColType>) -> String {
    if c.dtype == Dtype::Empty {
        return format!("{}[empty]", ident(name));
    }
    let st = col_stats(c);
    let mut kind = types::kind_with(&kind_text(c), c, ty); // B21
    if let (Some(lo), Some(hi)) = (st.min, st.max)
        && (is_numeric(c.dtype) || is_date(c.dtype))
    {
        kind.push_str(&format!(" {}..{}", brief_bound(lo, c.dtype), brief_bound(hi, c.dtype)));
        if is_date(c.dtype) {
            kind.push_str(&format!(", {} distinct", st.distinct));
        }
    } else if c.dtype == Dtype::Bool {
        kind.push_str(&format!(" {BOOL_STORED}"));
    } else if let Some(ex) = st.example.as_ref().filter(|_| example) {
        kind.push_str(&format!(" e.g. \"{}\"", short(ex, 24)));
    }
    // B89: only for label columns (a block label is filled on the block's first row, and its
    // labels rarely repeat); a sparse flag column ("e", "p") is just mostly empty.
    let filled = rows.saturating_sub(st.nulls);
    let label_like = matches!(c.values.first(), Some(Out::Text(_))) && st.distinct * 2 >= filled;
    if c.dtype == Dtype::Text && rows > 3 && st.nulls * 2 > rows && !label_like {
        kind.push_str(&format!(", {}/{} empty", st.nulls, rows));
    } else if c.dtype == Dtype::Text && rows > 3 && st.nulls * 2 > rows {
        kind.push_str(&format!(", {}/{} empty: label written once per block?", st.nulls, rows));
    }
    format!("{}[{kind}]", ident(name))
}

/// One column as `name[dtype unit]` (dates with their range).
fn col_compact(c: &Column, name: &str, ty: Option<&ColType>) -> String {
    let mut kind = types::kind_with(&kind_text(c), c, ty); // B21
    if c.dtype == Dtype::Bool {
        kind.push_str(" 1/0"); // B82: as stored in SQL
    }
    if is_date(c.dtype) {
        let st = col_stats(c);
        if let (Some(lo), Some(hi)) = (st.min, st.max) {
            kind.push_str(&format!(" {}..{}", bound(lo, c.dtype), bound(hi, c.dtype)));
        }
    }
    format!("{}[{kind}]", ident(name))
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
    let end = [name.find(" / ").map(|i| i + 3), name.find('_').map(|i| i + 1), name.find(' ').map(|i| i + 1)]
        .into_iter()
        .flatten()
        .min()?;
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
        format!("{}..{}[{} × {kind}]", ident(&names[cols[k].0]), ident(&names[cols[j - 1].0]), j - k)
    };
    let periods: Vec<Option<(u8, i64, String)>> = cols.iter().map(|(i, _)| name_period(&names[*i])).collect();
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
                        if kinds[m - k] == main { rest.to_string() } else { format!("{rest}[{}]", kinds[m - k]) }
                    })
                    .collect();
                let text = format!("{p}{{{}}}", members.join(", "));
                let quoted = (k..j).any(|m| ident(&names[cols[m].0]).starts_with('"'));
                out.push(if quoted { format!("\"{text}\"[{} × {main}]", j - k) } else { format!("{text}[{} × {main}]", j - k) });
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
        // B71: every name in full; wide tables group them by prefix when the plain list is long.
        let plain = compact_items(&cols, names, false, tys);
        if n <= LIST_COLS || len_of(&plain) <= WIDE_LINE { plain } else { compact_items(&cols, names, true, tys) }
    };
    let grouped = items.iter().any(|x| x.contains("}[") || x.contains("}\"["));
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

fn table_head(t: &Table, info: &RowInfo, cov: Option<&TimeCoverage>, brief: bool) -> String {
    let mut s = format!("{:<3} ", t.id());
    if let Some((title, _)) = &t.title {
        s.push_str(&format!("'{}' ", short(title, if brief { 60 } else { 100 })));
    }
    s.push_str(&format!("← {} · {}", sheet_ref(&t.source_sheet, &range_a1(t.rect)), plural(info.data, "row", "rows")));
    // Brief lines get their total rows from the caller (B74: all labels, repeated ones once).
    if !info.total_rows.is_empty() && !brief {
        let k = info.total_rows.len();
        s.push_str(&format!(
            " (+{} {}: {} — only in {}_all)",
            k,
            if k == 1 { "total row" } else { "total rows" },
            total_rows_text(&info.total_rows, 3),
            t.id()
        ));
    }
    if !info.kept_totals.is_empty() && !brief {
        let k = info.kept_totals.len();
        s.push_str(&format!(
            " · {} {} in {} ({}) — exclude {} before summing",
            plural(k, "total row", "total rows"),
            if k == 1 { "stays" } else { "stay" },
            t.id(),
            kept_totals_text(&info.kept_totals),
            if k == 1 { "it" } else { "them" }
        ));
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
}

impl Block {
    /// The table's lines with its total-rows and signs lines, and its rows when it is small.
    fn new(lines: Vec<String>, t: &Table, names: &[String], info: &RowInfo, inline: Option<Vec<String>>) -> Self {
        let data = data_rows(t);
        let mut parts: Vec<(u8, Vec<String>)> = Vec::new();
        parts.extend(facts::total_rows_line(t, names, info).map(|l| (0, vec![l])));
        parts.extend(facts::signs_line(t, names, &data).map(|l| (0, vec![l])));
        parts.extend(inline.map(|rows| (1, rows)));
        Block { lines, parts }
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
pub fn catalog(norm: &Normalized, display_name: &str, budget: usize) -> String {
    let budget = budget.min(MAX_CHARS);
    let file_name = display_name.rsplit(['/', '\\']).next().unwrap_or(display_name);
    let (mut covs, mut wb_cov) = time_coverages(norm);
    crate::views::fix_stamp_coverage(norm, &mut covs, &mut wb_cov); // B89
    let with_cov: Vec<(String, TimeCoverage)> =
        norm.tables.iter().zip(&covs).filter_map(|(t, c)| c.clone().map(|c| (t.id(), c))).collect();
    // Per-table periods only when there is more than one table (else the TIME line says it).
    let table_cov = |k: usize| covs[k].as_ref().filter(|_| norm.tables.len() > 1);
    let roles = if hh_core::learned::embedded().is_some() { "model" } else { "rules" };
    let pct = (100 * norm.cells.in_tables).checked_div(norm.cells.total).unwrap_or(0);
    let mut head = vec![
        format!(
            "FILE {display_name} · {} · {} ({pct}% in tables) · {} · roles: {roles}",
            plural(norm.sheets.len(), "sheet", "sheets"),
            plural(norm.cells.total, "cell", "cells"),
            plural(norm.tables.len(), "table", "tables"),
        ),
        time_line(wb_cov.as_ref(), &with_cov, &mentioned_years(norm, file_name)),
    ];
    let empty: Vec<&str> = norm.sheets.iter().filter(|s| s.tables.is_empty() && s.cells > 0).map(|s| s.name.as_str()).collect();
    if !empty.is_empty() {
        let mut s = format!(
            "SHEETS without tables: {}",
            empty.iter().take(10).map(|n| format!("'{}'", short(n, 30))).collect::<Vec<_>>().join(", ")
        );
        if empty.len() > 10 {
            s.push_str(&format!(" +{}", empty.len() - 10));
        }
        head.push(s);
    }

    let infos: Vec<RowInfo> = norm.tables.iter().map(|t| row_info(norm, t)).collect();
    let full = || -> Vec<Block> {
        let mut out = Vec::new();
        let mut layouts: HashMap<Vec<(String, &'static str)>, String> = HashMap::new();
        let mut shapes: HashMap<Vec<&'static str>, (String, Vec<String>)> = HashMap::new();
        for (k, (t, info)) in norm.tables.iter().zip(&infos).enumerate() {
            let names = sql_names(t);
            let cols: Vec<(String, &'static str)> =
                data_cols(t).map(|(i, c)| (names[i].clone(), c.dtype.as_str())).collect();
            let shape: Vec<&'static str> = cols.iter().map(|c| c.1).collect();
            let mut line = table_head(t, info, table_cov(k), false);
            let inline = facts::inline_rows(t, &names);
            if let Some(first) = layouts.get(&cols) {
                line.push_str(&format!(" · same columns as {first}"));
                line.extend(crate::views::table_view(t).map(|lv| crate::views::brief_text(&lv))); // B20
                let mut lines = vec![line];
                lines.extend(facts::scale_line(t)); // B85
                out.push(Block::new(lines, t, &names, info, inline));
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
            let mixed = mixed_measures_by(t, &data);
            let tys = types::classify(t, &names, mixed, &types::summed_cols(norm, t)); // B21
            let mut lines = vec![line, columns_line(t, &names, inline.is_none(), &tys)];
            lines.extend(facts::scale_line(t)); // B85: "(All values in 000s)"
            lines.extend(facts::column_notes_line(norm, t, &names)); // B83: footnotes defining columns
            // B2/B18: "<id> - <name>" columns and how to group them, right under the columns.
            let ents = crate::entity::detect(t);
            lines.extend(ents.iter().map(|e| crate::entity::line(e, &t.id())));
            // B80: an ID column with its names in another column.
            let pairs = crate::entity::id_name_pairs(t, &data);
            lines.extend(pairs.iter().map(|p| crate::entity::id_name_line(p, &t.id())));
            // B19: a share of the total, with the entity (or label) column to rank by.
            let key = ents.first().map(crate::entity::group_key).or_else(|| pairs.first().map(|p| crate::text::quote_ident(&p.id)));
            lines.extend(facts::share_line(t, &names, &data, key, &tys));
            // The values to filter on; a small inlined table shows them in its rows.
            if inline.is_none() {
                for (i, c) in data_cols(t) {
                    if let Some(vals) = label_values(c, &data) {
                        lines.push(format!("    {} values: {}", ident(&names[i]), values_text(&vals)));
                    }
                }
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
            if tt.count > 0 {
                extra.push(format!("totals: {} ok, {} mismatch, {} unknown", tt.ok, tt.mismatch, tt.unknown));
            }
            if !t.warnings.is_empty() {
                extra.push(format!("warnings: {} (e.g. {})", t.warnings.len(), short(&t.warnings[0], 90)));
            }
            if !extra.is_empty() {
                lines.push(format!("    {}", extra.join(" · ")));
            }
            let small = inline.is_some();
            let mut b = Block::new(lines, t, &names, info, inline);
            if !small && mixed.is_none() {
                let ranges_shown = data_cols(t).filter(|(_, c)| c.dtype != Dtype::Empty).count() <= DETAIL_COLS;
                let mut f = facts::numbers_lines(t, &names, &data, ranges_shown);
                f.extend(facts::quality_line(t, &names, &data, b.has_signs()));
                b.add_facts(f);
            }
            out.push(b);
        }
        // B74: a table whose total rows read like an earlier table's says so instead.
        let mut seen: HashMap<String, String> = HashMap::new();
        for (t, b) in norm.tables.iter().zip(out.iter_mut()) {
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
            out[k].lines.push(l);
        }
        out
    };
    // Tables one line each, within `avail` characters: total rows by level (B74, a repeated set
    // once), the scale of the numbers (B85), then the column names in full (B71: never cut; the
    // key columns and total columns first, the rest while the table's share of the room lasts).
    let brief = |avail: usize| -> Vec<String> {
        let mut out = vec!["(tables listed briefly; call describe_table for columns)".to_string()];
        let mut seen_totals: HashMap<String, String> = HashMap::new();
        let mut seen_cols: HashMap<Vec<String>, String> = HashMap::new();
        // (line so far, column items in priority order with their source position, columns
        // count, view text); None for columns: same columns as an earlier table (said already).
        let mut rows: Vec<BriefRow> = Vec::new();
        for (k, (t, info)) in norm.tables.iter().zip(&infos).enumerate() {
            let names = sql_names(t);
            let mut line = table_head(t, info, table_cov(k), true);
            let entries = total_entries(info);
            if !entries.is_empty() {
                let text = total_entries_text(&entries, &t.id(), false, 15);
                let key = format!("{text}|{}", entries.iter().map(|e| e.row.to_string()).collect::<Vec<_>>().join(","));
                match seen_totals.get(&key) {
                    Some(first) => line.push_str(&format!(" · total rows as in {first}")),
                    None => {
                        let kept = entries.iter().filter(|e| e.kept).count();
                        line.push_str(&format!(
                            " · {} ({}): {text}",
                            plural(entries.len(), "total row", "total rows"),
                            if kept == 0 { format!("only in {}_all", t.id()) } else { format!("only in {}_all unless marked", t.id()) }
                        ));
                        seen_totals.insert(key, t.id());
                    }
                }
            }
            if let Some((f, word, _)) = facts::table_scale(t) {
                line.push_str(&format!(" · amounts in {word} (× {})", thousands(f as usize)));
            }
            let cols: Vec<(usize, &Column)> = data_cols(t).filter(|(_, c)| c.dtype != Dtype::Empty).collect();
            let list: Vec<String> = cols.iter().map(|(i, _)| names[*i].clone()).collect();
            let mut view = crate::views::table_view(t).map(|lv| crate::views::brief_text(&lv)).unwrap_or_default();
            // B83: notes that define a column, on their own line under the table.
            if let Some(l) = facts::column_notes_line(norm, t, &names) {
                view.push('\n');
                view.push_str(&l);
            }
            if let Some(first) = seen_cols.get(&list) {
                line.push_str(&format!(" · {} cols, same as {first}", cols.len()));
                rows.push((line, None, cols.len(), view));
                continue;
            }
            seen_cols.insert(list, t.id());
            // Priority: the first two columns (keys, labels), then totals and amounts by name.
            let total_word = |n: &str| {
                let low = n.to_lowercase();
                ["total", "summe", "gesamt", "insgesamt", "ukupno", "totale", "totaal", "ensemble", "zusammen", "sum"].iter().any(|w| low.contains(w))
            };
            let mut order: Vec<usize> = (0..cols.len().min(2)).collect();
            order.extend((2..cols.len()).filter(|&j| total_word(&names[cols[j].0])));
            order.extend((2..cols.len()).filter(|&j| !total_word(&names[cols[j].0])));
            let items: Vec<(usize, String)> = order.into_iter().map(|j| (j, ident(&names[cols[j].0]))).collect();
            rows.push((line, Some(items), cols.len(), view));
        }
        let fixed: usize = out.iter().chain(rows.iter().map(|r| &r.0)).map(|l| l.len() + 1).sum::<usize>()
            + rows.iter().map(|r| r.3.len() + 20).sum::<usize>();
        let needing = rows.iter().filter(|r| r.1.is_some()).count().max(1);
        // When the lines alone overflow (many tables, cut anyway), one name each, so the cut
        // comes as late as before.
        let share = (avail.saturating_sub(fixed) / needing).max(if fixed > avail { 0 } else { 120 });
        for (mut line, items, n, view) in rows {
            if let Some(items) = items {
                let mut used = 0;
                let mut keep: Vec<(usize, String)> = Vec::new();
                for (j, name) in items {
                    if !keep.is_empty() && used + name.len() + 1 > share {
                        break;
                    }
                    used += name.len() + 1;
                    keep.push((j, name));
                }
                let shown = keep.len();
                keep.sort_by_key(|x| x.0);
                line.push_str(&format!(" · {n} cols: {}", keep.into_iter().map(|x| x.1).collect::<Vec<_>>().join(" ")));
                if shown < n {
                    line.push_str(&format!(" +{} more (describe_table)", n - shown));
                }
            }
            line.push_str(&view);
            out.push(line);
        }
        out
    };

    let mut tail = Vec::new();
    tail.extend(formulas_line(norm)); // B48
    let (ok, bad, unk) = status_counts(norm.totals.iter());
    if ok + bad + unk > 0 {
        tail.push(format!("TOTALS {ok} ok, {bad} mismatch, {unk} unknown (call totals for details)"));
    }
    if norm.notes.is_empty() {
        tail.push("NOTES none".into());
    } else {
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for n in &norm.notes {
            *kinds.entry(n.kind).or_default() += 1;
        }
        let k: Vec<String> = kinds.iter().map(|(k, v)| format!("{k} {v}")).collect();
        let mut s = format!("NOTES {} ({})", plural(norm.notes.len(), "cell", "cells"), k.join(", "));
        if let Some(n) = norm.notes.iter().find(|n| n.kind != "total_row" && n.kind != "repeated_header" && n.text.chars().count() > 12) {
            s.push_str(&format!("; e.g. {}: \"{}\"", sheet_ref(&n.sheet, &hh_core::a1(n.row, n.col)), short(&n.text, 90)));
        }
        tail.push(s);
    }
    if norm.warnings.is_empty() {
        tail.push("WARNINGS none".into());
    } else {
        tail.push(format!(
            "WARNINGS {}: {}",
            norm.warnings.len(),
            norm.warnings.iter().take(3).map(|w| short(w, 100)).collect::<Vec<_>>().join("; ")
        ));
    }
    if norm.tables.len() == 1 {
        tail.push(
            "SQL view T1 (data rows) and T1_all (+ total rows, _row_type); both have _src_row; quote names: SELECT \"col name\" FROM T1"
                .into(),
        );
    } else if let (Some(first), Some(last)) = (norm.tables.first(), norm.tables.last()) {
        tail.push(format!(
            "SQL views {}..{} (data rows) and {}_all..{}_all (+ total rows, _row_type); every view has _src_row; quote names: SELECT \"col name\" FROM {}",
            first.id(),
            last.id(),
            first.id(),
            last.id(),
            first.id()
        ));
    } else {
        tail.push("no tables detected; read values with the cells tool".into());
    }
    // B20: the long views (views.rs).
    let longs: Vec<String> = norm.tables.iter().filter_map(|t| crate::views::table_view(t).map(|lv| lv.name)).collect();
    if !longs.is_empty() {
        tail.push(format!(
            "SQL long {} {} (data rows) and the same with _long_all (+ total rows, _row_type): period columns as rows (period, period_label, value, _src_row, _src_col; the value's cell is <_src_col><_src_row>)",
            if longs.len() == 1 { "view" } else { "views" },
            id_list(&longs, 8)
        ));
    }

    let join = |body: &[String]| -> String {
        let mut all = head.clone();
        all.extend(body.iter().cloned());
        all.extend(tail.iter().cloned());
        all.join("\n")
    };
    let fixed = len_of(&head) + len_of(&tail);
    if norm.tables.len() <= BRIEF_TABLES_OVER {
        let blocks = full();
        let base: usize = fixed + blocks.iter().map(|b| len_of(&b.lines)).sum::<usize>();
        if base <= budget + 1 {
            // Optional parts while they fit: total rows and signs first, then small tables' rows,
            // then column facts; each tier in table order.
            let mut room = budget + 1 - base;
            let mut take: Vec<Vec<bool>> = blocks.iter().map(|b| vec![false; b.parts.len()]).collect();
            for tier in 0..3 {
                for (b, keep) in blocks.iter().zip(take.iter_mut()) {
                    for ((t, part), k) in b.parts.iter().zip(keep.iter_mut()) {
                        let need = len_of(part);
                        if *t == tier && need <= room {
                            room -= need;
                            *k = true;
                        }
                    }
                }
            }
            let mut body = Vec::new();
            for (b, keep) in blocks.into_iter().zip(take) {
                body.extend(b.lines);
                for ((_, part), k) in b.parts.into_iter().zip(keep) {
                    if k {
                        body.extend(part);
                    }
                }
            }
            return join(&body);
        }
    }
    let body = brief(budget.saturating_sub(fixed + 50));
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
    let rest = norm.tables.len() - listed;
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
            total_entries_text(&entries, &t.id(), true, 24),
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
        assert!(c.contains("T1  'Table 1 title' ← 'Sheet 1'!A1:D6 · 4 rows (+1 total row: 'Total'@6 — only in T1_all) · time 2001–2003 (year)"), "{c}");
        assert!(c.contains("columns (4): Region[text · dim] \"2001\"[number 0.25..0.25]"), "{c}");
        assert!(c.contains("T2  'Table 2 title' ← 'Sheet 2'!A1:D6 · 4 rows (+1 total row: 'Total'@6 — only in T2_all) · time 2001–2003 (year) · same columns as T1"), "{c}");
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
        // 1 + 31 columns with distinct names: all listed by name, no "more".
        let c = catalog(&book(1, 30, false), "x.xlsx", MAX_CHARS);
        for w in 0..31 {
            assert!(c.contains(&format!("\"Measure 1-{w} long header text\"[number]")), "{w}: {c}");
        }
        assert!(!c.contains("more"), "{c}");
        // Year columns: one run, nothing hidden.
        let c = catalog(&book(1, 15, true), "x.xlsx", MAX_CHARS);
        assert!(c.contains("columns (16): Region[text · dim] \"2001\"..\"2015\"[15 × number]"), "{c}");
        // B71: wider than 40 and long: grouped by name prefix, every member named.
        let c = catalog(&book(1, 75, false), "x.xlsx", MAX_CHARS);
        assert!(
            c.contains("columns (77; a_{b, c} = a_b, a_c): Region[text · dim] \"Measure {1-0 long header text, 1-1 long header text, "),
            "{c}"
        );
        assert!(c.contains(", 1-75 long header text}\"[76 × number]"), "{c}");
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
        assert!(c.contains("solar_{cons_change_pct[number · rate — don't sum], cons_change_twh, consumption, elec_per_capita, electricity, share_elec[number · rate — don't sum], share_energy[number · rate — don't sum], energy_per_capita, production}[9 × number]"), "{c}");
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
        // More than 12 distinct values: not listed.
        let mut many = book(1, 13, true);
        let t = &mut many.tables[0];
        for c in &mut t.columns {
            let v = c.values[0].clone();
            c.values = vec![v; 31];
        }
        t.columns[0].values = (0..31).map(|k| Out::Text(format!("r{k}"))).collect();
        t.src_rows = (1..=31).collect();
        t.row_types = vec![RowType::Data; 31];
        let c = catalog(&many, "x.xlsx", MAX_CHARS);
        assert!(!c.contains("Region values"), "{c}");
        assert!(c.contains("Region[text · id e.g. \"r0\"]"), "{c}");
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
        assert!(
            c.contains("(+1 total row: 'Gesamt'@6 — only in T1_all) · 1 total row stays in T1 ('Insgesamt'@3) — exclude it before summing"),
            "{c}"
        );
        assert!(c.contains("totals: 0 ok, 0 mismatch, 2 unknown"), "{c}");
        let d = describe_table(&n, &n.tables[0]);
        assert!(d.contains("total rows that stay in T1 (exclude before summing): 'Insgesamt'@3"), "{d}");
    }

    #[test]
    fn long_catalogs_go_brief_then_cut() {
        // 40 wide tables: too long in full, fits briefly.
        let c = catalog(&book(40, 20, false), "x.xlsx", MAX_CHARS);
        assert!(c.len() <= MAX_CHARS);
        assert!(c.contains("(tables listed briefly; call describe_table for columns)"), "{c}");
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
        assert!(c.contains("Returned[bool · dim 1/0 in SQL (TRUE/yes = 1, FALSE/no = 0; filter = 1, not 'TRUE')]"), "{c}");
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
