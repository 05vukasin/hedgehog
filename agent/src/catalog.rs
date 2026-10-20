//! Compact, LLM-readable text views of a [`Normalized`] workbook: the catalog (`open_file`),
//! `describe_table`, the totals check and a time-coverage estimate. Pure functions, no I/O.

use std::collections::{BTreeMap, HashMap, HashSet};

use hh_core::normalize::{iso, ColRole, Column, Dtype, Normalized, Out, RowType, Table};

use crate::text::{csv_row, cut, ident, letter, num, range_a1, sheet_ref, short, thousands, years_in, MAX_CHARS};

/// Tables above this count are always listed one line each.
const BRIEF_TABLES_OVER: usize = 40;
/// Columns listed per table in the full catalog.
const CATALOG_COLS: usize = 14;
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
        Out::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Out::Text(t) => t.clone(),
    }
}

fn is_date(d: Dtype) -> bool {
    matches!(d, Dtype::Date | Dtype::Datetime)
}

fn is_numeric(d: Dtype) -> bool {
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

/// A total / subtotal row kept in `T<n>_all`.
#[derive(Debug, Clone)]
pub struct TotalRow {
    pub label: String,
    /// 1-based source row.
    pub row: u32,
    pub kind: RowType,
}

/// Row counts of a table split by row type, and its total rows.
#[derive(Debug, Clone, Default)]
pub struct RowInfo {
    pub data: usize,
    pub total_rows: Vec<TotalRow>,
}

pub fn row_info(norm: &Normalized, t: &Table) -> RowInfo {
    let mut info = RowInfo::default();
    for (i, rt) in t.row_types.iter().enumerate() {
        if *rt == RowType::Data {
            info.data += 1;
            continue;
        }
        let src = t.src_rows.get(i).copied().unwrap_or(0);
        let from_totals = norm
            .totals
            .iter()
            .find(|x| x.table == Some(t.n) && x.row == src && x.label.is_some())
            .and_then(|x| x.label.clone());
        let label = from_totals.or_else(|| {
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
    info
}

fn total_rows_text(info: &RowInfo, n: usize) -> String {
    let shown: Vec<String> =
        info.total_rows.iter().take(n).map(|r| format!("'{}'@{}", short(&r.label, 32), r.row)).collect();
    let more = info.total_rows.len().saturating_sub(n);
    let mut s = shown.join(", ");
    if more > 0 {
        s.push_str(&format!(" +{more} more"));
    }
    s
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

fn data_cols(t: &Table) -> impl Iterator<Item = (usize, &Column)> {
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

// ------------------------------------------------------------------------------------------------
// time coverage

/// Which years a workbook covers, from its data and its texts. Derived here until the manifest
/// carries a `time_coverage` block; then this can read it from there.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeCoverage {
    pub from: i32,
    pub to: i32,
    /// year / quarter / month / day as the dates and period columns show them (several when
    /// tables differ, e.g. a monthly and an annual table), coarsest first.
    pub granularities: Vec<&'static str>,
    /// Short human-readable sources, strongest first.
    pub evidence: Vec<String>,
    /// True when only titles, notes, sheet or file names mention the years.
    pub weak: bool,
}

const PERIOD_WORDS: &[&str] = &[
    "year", "jahr", "annee", "année", "ano", "año", "anno", "godina", "rok", "jaar", "år", "periode", "period",
    "zeitraum", "quarter", "quartal", "month", "monat", "mois", "mes", "mese", "date", "datum", "fecha", "data",
    "berichtsjahr", "berichtszeitraum", "fy",
];

fn period_name(name: &str) -> bool {
    let low = name.to_lowercase();
    low.split(|c: char| !c.is_alphanumeric()).any(|w| PERIOD_WORDS.contains(&w))
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

/// Granularity of a set of Excel serial dates.
fn date_granularity(values: &[f64]) -> Option<&'static str> {
    let ymd: Vec<(i32, u32, u32)> = values
        .iter()
        .filter_map(|v| {
            let s = iso(*v);
            let y = s.get(0..4)?.parse().ok()?;
            let m = s.get(5..7)?.parse().ok()?;
            let d = s.get(8..10)?.parse().ok()?;
            Some((y, m, d))
        })
        .collect();
    if ymd.len() < 2 {
        return None;
    }
    let days: HashSet<u32> = ymd.iter().map(|x| x.2).collect();
    let months: HashSet<u32> = ymd.iter().map(|x| x.1).collect();
    let month_end = |&(y, m, d): &(i32, u32, u32)| {
        let last = match m {
            2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        d == last
    };
    let per_month = days.len() == 1 || ymd.iter().all(month_end);
    if per_month && months.len() == 1 {
        Some("year")
    } else if per_month && months.iter().all(|m| [1, 4, 7, 10].contains(m) || [3, 6, 9, 12].contains(m)) && months.len() <= 4 {
        Some("quarter")
    } else if per_month {
        Some("month")
    } else {
        Some("day")
    }
}

pub fn time_coverage(norm: &Normalized, file_name: &str) -> Option<TimeCoverage> {
    let mut strong: Vec<i32> = Vec::new();
    let mut evidence: Vec<String> = Vec::new();
    let mut gran: Vec<&'static str> = Vec::new();
    let mut header_years: Vec<i32> = Vec::new();
    for t in &norm.tables {
        for (c, name) in t.columns.iter().zip(sql_names(t)) {
            if !matches!(c.role, ColRole::Data | ColRole::Group) {
                continue;
            }
            header_years.extend(years_in(&c.name));
            if is_date(c.dtype) {
                let vals: Vec<f64> = c
                    .values
                    .iter()
                    .filter_map(|v| match v {
                        Out::Date(x) | Out::Num(x) => Some(*x),
                        _ => None,
                    })
                    .collect();
                if vals.is_empty() {
                    continue;
                }
                let st = col_stats(c);
                let (lo, hi) = (st.min.unwrap_or(0.0), st.max.unwrap_or(0.0));
                let ys: Vec<i32> = [lo, hi].iter().filter_map(|v| iso(*v).get(0..4)?.parse().ok()).collect();
                strong.extend(&ys);
                let g = date_granularity(&vals);
                gran.extend(g);
                evidence.push(format!(
                    "{}.{} {}..{} ({} distinct{})",
                    t.id(),
                    ident(&name),
                    bound(lo, Dtype::Date),
                    bound(hi, Dtype::Date),
                    st.distinct,
                    g.map(|g| format!(", {g}ly").replace("dayly", "daily")).unwrap_or_default()
                ));
            } else if c.dtype == Dtype::Integer && period_name(&c.name) {
                let st = col_stats(c);
                if let (Some(lo), Some(hi)) = (st.min, st.max)
                    && (1900.0..=2100.0).contains(&lo)
                    && (1900.0..=2100.0).contains(&hi)
                {
                    strong.extend([lo as i32, hi as i32]);
                    gran.push("year");
                    evidence.push(format!("{}.{} {}..{} ({} distinct)", t.id(), ident(&name), num(lo), num(hi), st.distinct));
                }
            } else if c.dtype == Dtype::Text && period_name(&c.name) {
                let ys: Vec<i32> =
                    c.values.iter().take(5000).filter_map(|v| if let Out::Text(s) = v { Some(years_in(s)) } else { None }).flatten().collect();
                if !ys.is_empty() {
                    strong.extend(&ys);
                    evidence.push(format!("{}.{} values {}", t.id(), ident(&name), span(&ys)));
                }
            }
        }
    }
    if !header_years.is_empty() {
        strong.extend(&header_years);
        // Years as column headers: one column per year, unless the dates say otherwise.
        if gran.is_empty() {
            gran.push("year");
        }
        evidence.push(format!("column headers {}", span(&header_years)));
    }
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
    if !weak.is_empty() {
        evidence.push(format!("titles/notes/sheet or file names {}", span(&weak)));
    }
    let (years, is_weak) = if strong.is_empty() { (&weak, true) } else { (&strong, false) };
    let from = *years.iter().min()?;
    let to = *years.iter().max()?;
    let order = ["year", "quarter", "month", "day"];
    let granularities: Vec<&'static str> =
        if is_weak { Vec::new() } else { order.into_iter().filter(|g| gran.contains(g)).collect() };
    Some(TimeCoverage { from, to, granularities, evidence, weak: is_weak })
}

fn time_line(tc: Option<&TimeCoverage>) -> String {
    let Some(tc) = tc else { return "TIME no dates or years found".into() };
    let range = if tc.from == tc.to { tc.from.to_string() } else { format!("{}–{}", tc.from, tc.to) };
    let mut ev: Vec<String> = tc.evidence.iter().take(4).map(|e| short(e, 110)).collect();
    if tc.evidence.len() > 4 {
        ev.push(format!("+{} more", tc.evidence.len() - 4));
    }
    let g = match tc.granularities.as_slice() {
        [] => String::new(),
        [one] => format!("{one}; "),
        many => format!("mixed: {} — check what one row is before summing; ", many.join(", ")),
    };
    let weak = if tc.weak { "only mentioned in " } else { "" };
    format!("TIME {range} ({g}{weak}{})", ev.join("; "))
}

// ------------------------------------------------------------------------------------------------
// catalog

fn col_brief(c: &Column, name: &str, rows: usize) -> String {
    let st = col_stats(c);
    let mut kind = c.dtype.as_str().to_string();
    if let Some(u) = c.currency.as_ref().or(c.unit.as_ref()) {
        kind.push(' ');
        kind.push_str(u);
    }
    if c.role == ColRole::Group {
        kind.push_str(" group");
    }
    if c.dtype == Dtype::Empty {
        return format!("{}[empty]", ident(name));
    }
    if let (Some(lo), Some(hi)) = (st.min, st.max)
        && (is_numeric(c.dtype) || is_date(c.dtype))
    {
        kind.push_str(&format!(" {}..{}", brief_bound(lo, c.dtype), brief_bound(hi, c.dtype)));
        if is_date(c.dtype) {
            kind.push_str(&format!(", {} distinct", st.distinct));
        }
    } else if let Some(ex) = &st.example {
        kind.push_str(&format!(" e.g. \"{}\"", short(ex, 24)));
    }
    if c.dtype == Dtype::Text && rows > 3 && st.nulls * 2 > rows {
        kind.push_str(&format!(", {}/{} empty: label written once per block?", st.nulls, rows));
    }
    format!("{}[{kind}]", ident(name))
}

fn columns_line(t: &Table, names: &[String]) -> String {
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
    let shown: Vec<String> = if n <= CATALOG_COLS {
        cols.iter().map(|(i, c)| col_brief(c, &names[*i], rows)).collect()
    } else {
        let head = CATALOG_COLS - 3;
        let mut v: Vec<String> = cols[..head].iter().map(|(i, c)| col_brief(c, &names[*i], rows)).collect();
        v.push(format!("… {} more …", n - head - 2));
        v.extend(cols[n - 2..].iter().map(|(i, c)| col_brief(c, &names[*i], rows)));
        v
    };
    format!("    columns ({label}): {}", shown.join(" "))
}

fn table_head(norm: &Normalized, t: &Table, info: &RowInfo, brief: bool) -> String {
    let mut s = format!("{:<3} ", t.id());
    if let Some((title, _)) = &t.title {
        s.push_str(&format!("'{}' ", short(title, if brief { 60 } else { 100 })));
    }
    s.push_str(&format!("← {} · {}", sheet_ref(&t.source_sheet, &range_a1(t.rect)), plural(info.data, "row", "rows")));
    if !info.total_rows.is_empty() {
        let k = info.total_rows.len();
        s.push_str(&format!(
            " (+{} {}: {} — only in {}_all)",
            k,
            if k == 1 { "total row" } else { "total rows" },
            total_rows_text(info, if brief { 1 } else { 3 }),
            t.id()
        ));
    }
    if t.confidence < 0.6 {
        s.push_str(&format!(" · low detection confidence {:.2}", t.confidence));
    }
    let _ = norm;
    s
}

/// The `open_file` catalog: one header line, the time coverage, the tables (full or one line each)
/// with columns, group column and totals check, notes and warnings; at most `budget` characters,
/// cut with an explicit `… cut:` line.
pub fn catalog(norm: &Normalized, display_name: &str, budget: usize) -> String {
    let budget = budget.min(MAX_CHARS);
    let tc = time_coverage(norm, display_name.rsplit(['/', '\\']).next().unwrap_or(display_name));
    let roles = if hh_core::learned::embedded().is_some() { "model" } else { "rules" };
    let pct = (100 * norm.cells.in_tables).checked_div(norm.cells.total).unwrap_or(0);
    let mut head = vec![
        format!(
            "FILE {display_name} · {} · {} ({pct}% in tables) · {} · roles: {roles}",
            plural(norm.sheets.len(), "sheet", "sheets"),
            plural(norm.cells.total, "cell", "cells"),
            plural(norm.tables.len(), "table", "tables"),
        ),
        time_line(tc.as_ref()),
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
    let full = || -> Vec<String> {
        let mut out = Vec::new();
        let mut layouts: HashMap<Vec<(String, &'static str)>, String> = HashMap::new();
        let mut shapes: HashMap<Vec<&'static str>, (String, Vec<String>)> = HashMap::new();
        for (t, info) in norm.tables.iter().zip(&infos) {
            let names = sql_names(t);
            let cols: Vec<(String, &'static str)> =
                data_cols(t).map(|(i, c)| (names[i].clone(), c.dtype.as_str())).collect();
            let shape: Vec<&'static str> = cols.iter().map(|c| c.1).collect();
            let mut line = table_head(norm, t, info, false);
            if let Some(first) = layouts.get(&cols) {
                line.push_str(&format!(" · same columns as {first}"));
                out.push(line);
                continue;
            }
            if cols.len() >= 3
                && let Some((first, first_names)) = shapes.get(&shape)
            {
                let diff: Vec<String> =
                    cols.iter().filter(|c| !first_names.contains(&c.0)).map(|c| ident(&short(&c.0, 24))).collect();
                line.push_str(&format!(" · same layout as {first}"));
                if !diff.is_empty() {
                    line.push_str(&format!("; headers here: {}", diff.iter().take(6).cloned().collect::<Vec<_>>().join(" ")));
                    if diff.len() > 6 {
                        line.push_str(" …");
                    }
                }
                out.push(line);
                continue;
            }
            layouts.insert(cols.clone(), t.id());
            shapes.insert(shape, (t.id(), cols.iter().map(|c| c.0.clone()).collect()));
            out.push(line);
            out.push(columns_line(t, &names));
            let mut extra = Vec::new();
            if let Some(g) = t.columns.iter().position(|c| c.role == ColRole::Group) {
                extra.push(format!("group column: {}", ident(&names[g])));
            }
            let (ok, bad, unk) = status_counts(norm.totals.iter().filter(|x| x.table == Some(t.n)));
            if ok + bad + unk > 0 {
                extra.push(format!("totals: {ok} ok, {bad} mismatch, {unk} unknown"));
            }
            if !t.warnings.is_empty() {
                extra.push(format!("warnings: {} (e.g. {})", t.warnings.len(), short(&t.warnings[0], 90)));
            }
            if !extra.is_empty() {
                out.push(format!("    {}", extra.join(" · ")));
            }
        }
        out
    };
    let brief = || -> Vec<String> {
        let mut out = vec!["(tables listed briefly; call describe_table for columns)".to_string()];
        for (t, info) in norm.tables.iter().zip(&infos) {
            let names = sql_names(t);
            let cols: Vec<String> = data_cols(t).map(|(i, _)| ident(&short(&names[i], 24))).collect();
            let mut line = table_head(norm, t, info, true);
            line.push_str(&format!(" · {} cols: {}", cols.len(), cols.iter().take(4).cloned().collect::<Vec<_>>().join(" ")));
            if cols.len() > 4 {
                line.push_str(" …");
            }
            out.push(line);
        }
        out
    };

    let mut tail = Vec::new();
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

    let join = |body: &[String]| -> String {
        let mut all = head.clone();
        all.extend(body.iter().cloned());
        all.extend(tail.iter().cloned());
        all.join("\n")
    };
    if norm.tables.len() <= BRIEF_TABLES_OVER {
        let text = join(&full());
        if text.len() <= budget {
            return text;
        }
    }
    let body = brief();
    let text = join(&body);
    if text.len() <= budget {
        return text;
    }
    // Still too long: keep as many table lines as fit, then say what was cut.
    let fixed: usize = head.iter().chain(&tail).map(|l| l.len() + 1).sum::<usize>() + 200;
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
    if !info.total_rows.is_empty() {
        let tr: Vec<String> = info
            .total_rows
            .iter()
            .take(12)
            .map(|r| format!("'{}'@{} ({})", short(&r.label, 40), r.row, r.kind.as_str()))
            .collect();
        let more = info.total_rows.len().saturating_sub(12);
        lines.push(format!(
            "total rows (only in {}_all, _row_type): {}{}",
            t.id(),
            tr.join(", "),
            if more > 0 { format!(" +{more} more") } else { String::new() }
        ));
    }
    lines.push("columns (name | dtype | unit | nulls | distinct | min..max or example | source column ← header cells):".into());
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
            _ => st.example.as_ref().map(|e| format!("e.g. \"{}\"", short(e, 40))).unwrap_or_default(),
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
        lines.push(format!(
            "  {} | {} | {} | {} | {} | {} | {}",
            ident(&names[*i]),
            c.dtype.as_str(),
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
        assert!(c.contains("TIME 2001–2003 (year; column headers 2001–2003)"), "{c}");
        assert!(c.contains("T1  'Table 1 title' ← 'Sheet 1'!A1:D6 · 4 rows (+1 total row: 'Total'@6 — only in T1_all)"), "{c}");
        assert!(c.contains("columns (4): Region[text e.g. \"r0\"] \"2001\"[number 0.25..0.25]"), "{c}");
        assert!(c.contains("T2  'Table 2 title' ← 'Sheet 2'!A1:D6 · 4 rows (+1 total row: 'Total'@6 — only in T2_all) · same columns as T1"), "{c}");
        assert!(!c.contains("… cut:"));
    }

    #[test]
    fn long_catalogs_go_brief_then_cut() {
        // 30 wide tables: too long in full, fits briefly.
        let c = catalog(&book(30, 40, false), "x.xlsx", MAX_CHARS);
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
        assert!(d.contains("total rows (only in T1_all, _row_type): 'Total'@6 (total)"), "{d}");
        assert!(d.contains("… 12 more columns: "), "{d}");
        assert!(d.contains("first 4 rows (first 12 columns):"), "{d}");
        assert!(find_table(&n, "t1").is_ok() && find_table(&n, "T1_all").is_ok());
        assert!(find_table(&n, "T7").unwrap_err().contains("tables are T1"));
        assert!(totals(&n, None, Some("weird")).is_err());
        assert_eq!(totals(&n, None, None).unwrap(), "TOTALS: 0 ok, 0 mismatch, 0 unknown\nno totals found");
    }

    #[test]
    fn sql_names_are_unique_without_case() {
        let mut n = book(1, 1, true);
        n.tables[0].columns[1].name = "region".into();
        assert_eq!(sql_names(&n.tables[0]), ["Region", "region (2)", "_src_row"]);
    }

    #[test]
    fn granularity() {
        let months: Vec<f64> = (0..12).map(|m| 44927.0 + m as f64 * 31.0).collect(); // ~2023 monthly, not day 1
        assert_eq!(date_granularity(&months), Some("day"));
        let firsts = [44927.0, 44958.0, 44986.0]; // 2023-01-01, 02-01, 03-01
        assert_eq!(date_granularity(&firsts), Some("month"));
        let jan = [44927.0, 45292.0]; // 2023-01-01, 2024-01-01
        assert_eq!(date_granularity(&jan), Some("year"));
    }
}
