//! Long views of wide period tables (B20): `T<n>_long` turns a run of period columns (`"2011"` ..
//! `"2025"`, `Jan 2024`, `2024 Q1`, `Thu / 2002-01-31`) into rows, so a model filters
//! `WHERE period = '2025'` instead of choosing a column.
//!
//! For a table with at least [`MIN_PERIODS`] numeric columns whose headers name periods of one
//! granularity (read as the time coverage reads headers, [`hh_core::normalize::header_period`]),
//! the view has the table's key columns (group, labels, IDs, dimensions), `measure` when the
//! period columns repeat for several measures (`Umsatz / 2023`, `Kosten / 2023`), `period`
//! (normalized: `2025`, `2025-Q1`, `2025-03`, `2025-W05`, `2025-03-31`), `period_label` (the
//! column's name), `value`, `_row_type`, `_src_row` and `_src_col` (the source column letter: the
//! value's cell is `<_src_col><_src_row>`). As with `T<n>` / `T<n>_all`, `T<n>_long` has the data
//! rows only and `T<n>_long_all` also the total rows, with `_row_type`; empty cells are left out.
//!
//! The views are plain SQLite views (a `UNION ALL` of one `SELECT` per period column over `T<n>`
//! or `T<n>_all`): nothing is copied at load, and a filter on `period` skips the other branches
//! before they read a row.

use hh_core::normalize::{header_period, ColRole, Dtype, Table};
use rusqlite::Transaction;

use crate::catalog::{data_cols, is_numeric, sql_names};
use crate::text::{ident, letter, quote_ident, short};
use crate::types::{ColType, Sem};

/// Fewest period columns that make a long view.
pub const MIN_PERIODS: usize = 4;
/// Most period columns in one view (SQLite allows 500 `SELECT`s in one compound statement).
const MAX_PERIODS: usize = 480;
/// Most key columns copied into the view.
const MAX_KEYS: usize = 8;
/// Tables wider than SQLite's column limit are loaded in part; they get no long view.
const MAX_TABLE_COLUMNS: usize = 1990;

const WEEKDAYS: &[&str] = &[
    "mon", "tue", "wed", "thu", "fri", "sat", "sun", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday",
    "sunday", "mo", "di", "mi", "do", "fr", "sa", "so", "montag", "dienstag", "mittwoch", "donnerstag", "freitag",
    "samstag", "sonntag", "lun", "mar", "mer", "jeu", "ven", "sam", "dim", "lu", "ma", "me", "je", "ve", "di", "pon",
    "uto", "sre", "čet", "pet", "sub", "ned",
];

/// One period column of a long view.
#[derive(Debug, Clone)]
pub struct PeriodCol {
    /// Index in `Table::columns`.
    pub col: usize,
    /// Normalized period (`2025`, `2025-03`, `2025-Q1`, `2025-03-31`).
    pub period: String,
    /// What the header says besides the period (`Umsatz` in `Umsatz / 2023`); empty for a bare period.
    pub measure: String,
}

/// The long view of one table.
#[derive(Debug, Clone)]
pub struct LongView {
    /// `T<n>_long` (data rows; source `T<n>`).
    pub name: String,
    /// `T<n>_long_all` (with total rows and `_row_type`; source `T<n>_all`).
    pub all_name: String,
    /// `T<n>`.
    pub table: String,
    /// Key columns (indexes in `Table::columns`).
    pub keys: Vec<usize>,
    pub periods: Vec<PeriodCol>,
    /// year | quarter | month | week | day
    pub granularity: &'static str,
    /// The period columns stand for several measures: the view has a `measure` column.
    pub has_measure: bool,
}

/// A word that is (part of) a period: digits and separators only (`2024`, `2024-03`,
/// `31.01.2002`, `2020/`), a quarter / half / week number (`Q1`, `H2`, `W05`, `1Q`), a month or
/// period label, a weekday, or anything the date parser reads.
/// `dated`: the header has a full date or ISO month (`2002-02-05`), so a month word next to it is
/// not its period (`Avg` is August in Serbian, `Mar` March).
fn period_word(w: &str, dated: bool) -> bool {
    let lw = w.trim_end_matches(['.', ',', ':']).to_lowercase();
    let digits = lw.chars().filter(|c| c.is_ascii_digit()).count();
    if digits > 0 && lw.chars().all(|c| c.is_ascii_digit() || "-./:".contains(c)) {
        return true;
    }
    let b = lw.as_bytes();
    let short_num = |x: &[u8]| !x.is_empty() && x.len() <= 2 && x.iter().all(u8::is_ascii_digit);
    if b.len() >= 2 && (matches!(b[0], b'q' | b'h' | b'w' | b'k') && short_num(&b[1..]) || matches!(b[b.len() - 1], b'q' | b'h') && short_num(&b[..b.len() - 1])) {
        return true;
    }
    WEEKDAYS.contains(&lw.as_str())
        || (!dated && hh_core::values::date::is_period_label(w))
        || (digits > 0 && hh_core::values::parse_date(w, None).is_some())
}

/// What a header says besides its period: parts and words that are not years, dates, period
/// labels or weekdays (`MTD Avg / 2002-02-05` → `MTD Avg`, `Thu / 2002-01-31` → ``).
fn header_rest(name: &str) -> String {
    // Without the " (2)" that makes a duplicate name unique.
    let name = match name.rsplit_once(" (") {
        Some((head, tail)) if tail.ends_with(')') && tail[..tail.len() - 1].bytes().all(|b| b.is_ascii_digit()) => head,
        _ => name,
    };
    let dated = name.split(|c: char| c.is_whitespace()).any(|w| {
        w.len() >= 7 && w.bytes().filter(u8::is_ascii_digit).count() >= 6 && w.bytes().all(|b| b.is_ascii_digit() || b"-./".contains(&b))
    });
    let mut parts: Vec<String> = Vec::new();
    for part in name.split(" / ") {
        let p = part.trim();
        if p.is_empty() || (!dated && hh_core::values::date::is_period_label(p)) {
            continue;
        }
        let words: Vec<&str> = p
            .split_whitespace()
            .filter(|w| !period_word(w, dated))
            .collect();
        if !words.is_empty() {
            parts.push(words.join(" "));
        }
    }
    parts.join(" / ")
}

/// The long view a table gets, if it has a run of period columns. `types` (from
/// [`crate::types::classify`]) picks the key columns.
pub fn long_view(t: &Table, types: &[Option<ColType>]) -> Option<LongView> {
    if t.columns.len() > MAX_TABLE_COLUMNS {
        return None;
    }
    let names = sql_names(t);
    // Period columns by granularity and rest of the header.
    let mut cands: Vec<(usize, String, &'static str, String)> = Vec::new();
    for (i, c) in data_cols(t) {
        if c.role != ColRole::Data || !is_numeric(c.dtype) {
            continue;
        }
        if let Some((period, gran)) = period_of(&names[i]) {
            cands.push((i, period, gran, header_rest(&names[i])));
        }
    }
    if cands.len() < MIN_PERIODS {
        return None;
    }
    // Groups of (granularity, rest) with at least MIN_PERIODS distinct periods.
    let mut groups: Vec<((&'static str, String), Vec<usize>)> = Vec::new();
    for (k, (_, _, gran, rest)) in cands.iter().enumerate() {
        let key = (*gran, rest.to_lowercase());
        match groups.iter_mut().find(|(g, _)| *g == key) {
            Some((_, v)) => v.push(k),
            None => groups.push((key, vec![k])),
        }
    }
    let distinct = |ks: &[usize]| {
        let mut ps: Vec<&str> = ks.iter().map(|&k| cands[k].1.as_str()).collect();
        ps.sort_unstable();
        ps.dedup();
        ps.len()
    };
    groups.retain(|(_, ks)| distinct(ks) >= MIN_PERIODS);
    let (best_gran, _) = groups.iter().max_by_key(|(_, ks)| ks.len())?.0.clone();
    // Every group of that granularity whose periods repeat another's: one measure each.
    let biggest = groups.iter().filter(|((g, _), _)| *g == best_gran).max_by_key(|(_, ks)| ks.len())?.1.clone();
    let mut chosen: Vec<Vec<usize>> = vec![biggest.clone()];
    for ((g, _), ks) in &groups {
        if *g == best_gran && *ks != biggest {
            chosen.push(ks.clone());
        }
    }
    let has_measure = chosen.len() > 1;
    let mut periods: Vec<PeriodCol> = Vec::new();
    for ks in &chosen {
        for &k in ks {
            let (col, period, _, rest) = &cands[k];
            periods.push(PeriodCol { col: *col, period: period.clone(), measure: rest.clone() });
        }
    }
    periods.sort_by_key(|p| p.col);
    periods.truncate(MAX_PERIODS);
    let in_view: Vec<usize> = periods.iter().map(|p| p.col).collect();
    let mut keys: Vec<usize> = Vec::new();
    for (i, c) in data_cols(t) {
        if in_view.contains(&i) || c.dtype == Dtype::Empty {
            continue;
        }
        let key = c.role == ColRole::Group
            || matches!(
                types.get(i).and_then(|x| x.as_ref()).map(|x| x.sem),
                Some(Sem::Id | Sem::Name | Sem::Dimension | Sem::Period | Sem::Text)
            );
        if key && keys.len() < MAX_KEYS {
            keys.push(i);
        }
    }
    Some(LongView {
        name: format!("{}_long", t.id()),
        all_name: format!("{}_long_all", t.id()),
        table: t.id(),
        keys,
        periods,
        granularity: best_gran,
        has_measure,
    })
}

/// The period a header names: as the time coverage reads headers, or else an ISO month or day
/// inside other words (`Rev 2024-03`, `Umsatz 2024-03-31`, as the catalog's period runs read them).
pub fn period_of(name: &str) -> Option<(String, &'static str)> {
    if let Some(p) = header_period(name) {
        return Some(p);
    }
    let b = name.as_bytes();
    let digit = |k: usize| b.get(k).is_some_and(u8::is_ascii_digit);
    for i in 0..b.len().saturating_sub(6) {
        if (i > 0 && digit(i - 1)) || !(i..i + 4).all(digit) || b[i + 4] != b'-' || !(i + 5..i + 7).all(digit) {
            continue;
        }
        let y: u32 = name[i..i + 4].parse().ok()?;
        let m: u32 = name[i + 5..i + 7].parse().ok()?;
        if !(1900..=2100).contains(&y) || !(1..=12).contains(&m) {
            return None;
        }
        if b.get(i + 7) == Some(&b'-') && (i + 8..i + 10).all(digit) && !digit(i + 10) {
            let d: u32 = name[i + 8..i + 10].parse().ok()?;
            return (1..=31).contains(&d).then(|| (name[i..i + 10].to_string(), "day"));
        }
        return (!digit(i + 7)).then(|| (name[i..i + 7].to_string(), "month"));
    }
    None
}

// ------------------------------------------------------------------------------------------------
// periods written as text (B89)

/// A period code as statistics offices write it in a text column: `2019`, `2019-S1` / `2019S1` /
/// `2019-H1` (half-year), `2019-Q3` / `2019Q3`, `2019-03` / `2019M03`, `2019-W05`: (normalized,
/// granularity). Strict, so labels like "Jan-Mar 1971" are not read.
pub fn text_period(s: &str) -> Option<(String, &'static str)> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 4 || !b[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let y: u32 = s[..4].parse().ok()?;
    if !(1800..=2200).contains(&y) {
        return None;
    }
    let rest = s[4..].strip_prefix('-').unwrap_or(&s[4..]);
    if rest.is_empty() {
        return (b.len() == 4).then(|| (s.to_string(), "year"));
    }
    let (tag, num) = rest.split_at(rest.find(|c: char| c.is_ascii_digit())?);
    let n: u32 = num.parse().ok()?;
    match tag.to_ascii_uppercase().as_str() {
        "S" | "H" if (1..=2).contains(&n) && num.len() == 1 => Some((format!("{y}-S{n}"), "half-year")),
        "Q" | "T" if (1..=4).contains(&n) && num.len() == 1 => Some((format!("{y}-Q{n}"), "quarter")),
        "W" if (1..=53).contains(&n) && num.len() == 2 => Some((format!("{y}-W{num}"), "week")),
        "M" | "" if (1..=12).contains(&n) && num.len() == 2 && (tag == "M" || s.as_bytes()[4] == b'-') => {
            Some((format!("{y}-{num}"), "month"))
        }
        _ => None,
    }
}

/// A text column whose filled values (all of the first 400) are period codes of one granularity:
/// (first, last, granularity) over `rows`.
pub fn text_period_column(c: &hh_core::normalize::Column, rows: &[usize]) -> Option<(String, String, &'static str)> {
    if c.dtype != Dtype::Text {
        return None;
    }
    let mut gran: Option<&'static str> = None;
    let (mut lo, mut hi): (Option<String>, Option<String>) = (None, None);
    let mut checked = 0;
    for &r in rows {
        let Some(hh_core::normalize::Out::Text(s)) = c.values.get(r) else { continue };
        let (p, g) = text_period(s)?;
        if gran.is_some_and(|x| x != g) {
            return None;
        }
        gran = Some(g);
        checked += 1;
        if lo.as_ref().is_none_or(|x| p < *x) {
            lo = Some(p.clone());
        }
        if hi.as_ref().is_none_or(|x| p > *x) {
            hi = Some(p);
        }
    }
    (checked >= 2).then_some(())?;
    Some((lo?, hi?, gran?))
}

/// A column that stamps when the data was published or loaded, not the period it covers
/// (`LAST UPDATE`, `Stand`, `extraction_date`).
pub fn update_stamp(name: &str) -> bool {
    const STAMP: &[&str] = &[
        "update", "updated", "aktualisiert", "aktualisierung", "stand", "extraction", "extracted", "published",
        "publication", "downloaded", "download", "timestamp", "loaded", "retrieved", "maj", "atualização", "atualizacao",
    ];
    let low = name.to_lowercase();
    low.split(|c: char| !c.is_alphanumeric()).any(|w| STAMP.contains(&w))
}

/// B89: time coverage from period codes written as text (`TIME_PERIOD` '2019-S1') for tables
/// whose coverage comes only from update stamps (or that have none); the workbook's coverage is
/// rebuilt from the tables when one changed.
pub fn fix_stamp_coverage(
    norm: &hh_core::normalize::Normalized,
    covs: &mut [Option<hh_core::normalize::TimeCoverage>],
    wb: &mut Option<hh_core::normalize::TimeCoverage>,
) {
    let mut changed = false;
    for (t, cov) in norm.tables.iter().zip(covs.iter_mut()) {
        let stamped = cov.as_ref().is_some_and(|c| c.source == "column" && !c.columns.is_empty() && c.columns.iter().all(|n| update_stamp(n)));
        if !(stamped || cov.is_none()) {
            continue;
        }
        let rows = crate::catalog::data_rows(t);
        let names = sql_names(t);
        let found = data_cols(t).find_map(|(i, c)| text_period_column(c, &rows).map(|p| (names[i].clone(), p)));
        if let Some((name, (from, to, gran))) = found {
            *cov = Some(hh_core::normalize::TimeCoverage {
                from,
                to,
                granularity: gran,
                source: "column",
                columns: vec![name],
                tables: vec![t.id()],
            });
            changed = true;
        } else if stamped {
            // A publication stamp is no period of the data.
            *cov = None;
            changed = true;
        }
    }
    if !changed {
        return;
    }
    let with: Vec<&hh_core::normalize::TimeCoverage> = covs.iter().flatten().collect();
    *wb = match with.as_slice() {
        [] => None,
        [one, ..] if with.iter().all(|c| c.granularity == one.granularity) => Some(hh_core::normalize::TimeCoverage {
            from: with.iter().map(|c| c.from.clone()).min().unwrap_or_default(),
            to: with.iter().map(|c| c.to.clone()).max().unwrap_or_default(),
            granularity: one.granularity,
            source: one.source,
            columns: norm
                .tables
                .iter()
                .zip(covs.iter())
                .filter_map(|(t, c)| c.as_ref().map(|c| (t, c)))
                .flat_map(|(t, c)| c.columns.iter().map(move |n| format!("{}.{n}", t.id())))
                .collect(),
            tables: with.iter().flat_map(|c| c.tables.clone()).collect(),
        }),
        _ => wb.take(),
    };
}

/// Names of the view's own columns, made distinct from the key columns' names.
fn own_names(t: &Table, lv: &LongView) -> [String; 6] {
    let names = sql_names(t);
    let taken: Vec<String> = lv.keys.iter().map(|&i| names[i].to_lowercase()).collect();
    ["measure", "period", "period_label", "value", "_src_col", "_row_type"].map(|n| {
        let mut name = n.to_string();
        while n != "_row_type" && taken.contains(&name.to_lowercase()) {
            name.push('_');
        }
        name
    })
}

fn sql_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `CREATE VIEW` statement of a long view: `T<n>_long` over `T<n>`, or with `all`
/// `T<n>_long_all` over `T<n>_all` (with `_row_type`).
pub fn create_sql(t: &Table, lv: &LongView, all: bool) -> String {
    let names = sql_names(t);
    let [measure, period, label, value, src_col, _] = own_names(t, lv);
    let keys: Vec<String> = lv.keys.iter().map(|&i| quote_ident(&names[i])).collect();
    let mut branches = Vec::with_capacity(lv.periods.len());
    for p in &lv.periods {
        let c = &t.columns[p.col];
        let col = quote_ident(&names[p.col]);
        let mut sel = keys.clone();
        if lv.has_measure {
            sel.push(format!("{} AS {}", sql_text(&p.measure), quote_ident(&measure)));
        }
        sel.push(format!("{} AS {}", sql_text(&p.period), quote_ident(&period)));
        sel.push(format!("{} AS {}", sql_text(&names[p.col]), quote_ident(&label)));
        sel.push(format!("{col} AS {}", quote_ident(&value)));
        if all {
            sel.push("\"_row_type\"".into());
        }
        sel.push("\"_src_row\"".into());
        sel.push(format!("{} AS {}", sql_text(&c.source_col.map(letter).unwrap_or_default()), quote_ident(&src_col)));
        let source = if all { format!("{}_all", lv.table) } else { lv.table.clone() };
        branches.push(format!("SELECT {} FROM {} WHERE {col} IS NOT NULL", sel.join(", "), quote_ident(&source)));
    }
    let name = if all { &lv.all_name } else { &lv.name };
    format!("CREATE VIEW {} AS {}", quote_ident(name), branches.join(" UNION ALL "))
}

/// Creates the long views of all tables (called by [`crate::sql::Db::load`] inside its
/// transaction, after the tables and their `T<n>` / `T<n>_all` views exist). Returns the
/// tables that got one.
pub fn create_all(tx: &Transaction<'_>, norm: &hh_core::normalize::Normalized) -> rusqlite::Result<Vec<usize>> {
    let mut made = Vec::new();
    for t in &norm.tables {
        let Some(lv) = table_view(t) else { continue };
        tx.execute_batch(&create_sql(t, &lv, false))?;
        tx.execute_batch(&create_sql(t, &lv, true))?;
        made.push(t.n);
    }
    Ok(made)
}

/// The long view of a table (types computed here).
pub fn table_view(t: &Table) -> Option<LongView> {
    // Cheap check first: most tables have no period headers at all.
    let names = sql_names(t);
    let n = data_cols(t)
        .filter(|(i, c)| c.role == ColRole::Data && is_numeric(c.dtype) && period_of(&names[*i]).is_some())
        .take(MIN_PERIODS)
        .count();
    if n < MIN_PERIODS {
        return None;
    }
    let types = crate::types::classify(t, &names, None, &[]);
    long_view(t, &types)
}

fn period_span(lv: &LongView) -> String {
    let mut ps: Vec<&str> = lv.periods.iter().map(|p| p.period.as_str()).collect();
    ps.sort_unstable();
    ps.dedup();
    match (ps.first(), ps.last()) {
        (Some(a), Some(b)) if a != b => format!("'{a}'..'{b}'"),
        (Some(a), _) => format!("'{a}'"),
        _ => String::new(),
    }
}

/// The catalog line of a table's long view: `T1_long: one row per ("group", Richtsatz, period) —
/// "2011".."2025" as rows (period '2011'..'2025', value); use for trends/filters by year`, with
/// the label column to filter first when the rows mix measures, and a note on total rows.
pub fn catalog_line(t: &Table, lv: &LongView, mixed: Option<usize>, total_rows: bool) -> String {
    let names = sql_names(t);
    let [measure, period, _, value, _, _] = own_names(t, lv);
    let mut per: Vec<String> = lv.keys.iter().take(4).map(|&i| ident(&short(&names[i], 30))).collect();
    if lv.keys.len() > 4 {
        per.push("…".into());
    }
    if lv.has_measure {
        per.push(ident(&measure));
    }
    per.push(ident(&period));
    let first = &names[lv.periods[0].col];
    let last = &names[lv.periods[lv.periods.len() - 1].col];
    let mut s = format!(
        "    {}: one row per ({}) — {} columns {}..{} as rows: {} {}, {}; use for trends/filters by {}",
        lv.name,
        per.join(", "),
        lv.periods.len(),
        ident(&short(first, 30)),
        ident(&short(last, 30)),
        ident(&period),
        period_span(lv),
        ident(&value),
        lv.granularity
    );
    if lv.has_measure {
        let mut ms: Vec<&str> = Vec::new();
        for p in &lv.periods {
            if !ms.contains(&p.measure.as_str()) {
                ms.push(&p.measure);
            }
        }
        let shown: Vec<String> = ms.iter().take(4).map(|m| format!("'{}'", short(m, 24))).collect();
        s.push_str(&format!(" · {} = {}{}", ident(&measure), shown.join(" | "), if ms.len() > 4 { " | …" } else { "" }));
    }
    if let Some(m) = mixed {
        s.push_str(&format!(" · rows mix measures: filter {} first", ident(&names[m])));
    }
    if total_rows {
        s.push_str(&format!(" · total rows only in {} (_row_type)", lv.all_name));
    }
    s
}

/// The long view in a brief table line: ` · T1_long (by year)`.
pub fn brief_text(lv: &LongView) -> String {
    format!(" · {} (by {})", lv.name, lv.granularity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::{Db, DEFAULT_TIMEOUT};
    use hh_core::normalize::{Column, Normalized, Out, RowType, TimeCoverage, Transforms};
    use hh_core::Rect;

    fn col(name: &str, role: ColRole, dtype: Dtype, src: u32, values: Vec<Out>) -> Column {
        Column {
            name: name.into(),
            role,
            source_col: Some(src),
            original_header: vec![(name.into(), (0, src))],
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

    /// Region × 2021..2024 plus a change column and a total row.
    fn wide() -> Normalized {
        let regions = ["North", "South", "Total"];
        let mut columns = vec![
            col("Region", ColRole::Data, Dtype::Text, 0, regions.iter().map(|r| Out::Text((*r).into())).collect()),
        ];
        for (k, y) in [2021, 2022, 2023, 2024].iter().enumerate() {
            let vals = vec![Out::Num(10.0 * (k + 1) as f64), Out::Num(1.0 + k as f64), Out::Num(11.0 * (k + 1) as f64)];
            let vals = if *y == 2022 { vec![Out::Null, vals[1].clone(), vals[2].clone()] } else { vals };
            columns.push(col(&y.to_string(), ColRole::Data, Dtype::Number, 1 + k as u32, vals));
        }
        columns.push(col("2021/ 2024", ColRole::Data, Dtype::Number, 5, vec![Out::Num(0.5); 3]));
        columns.push(col("_src_row", ColRole::SrcRow, Dtype::Integer, 0, (0..3).map(|i| Out::Num(i as f64 + 2.0)).collect()));
        columns.push(col(
            "_row_type",
            ColRole::RowType,
            Dtype::Text,
            0,
            ["data", "data", "total"].iter().map(|s| Out::Text((*s).into())).collect(),
        ));
        let t = Table {
            n: 1,
            sheet: "T1_x".into(),
            source_sheet: "Sheet1".into(),
            rect: Rect { top: 0, left: 0, bottom: 3, right: 5 },
            header_top: 0,
            header_rows: 1,
            title: Some(("Sales in EUR".into(), (0, 0))),
            notes: vec![],
            confidence: 0.9,
            columns,
            src_rows: (1..=3).collect(),
            row_types: vec![RowType::Data, RowType::Data, RowType::Total],
            transforms: Transforms::default(),
            warnings: vec![],
        };
        Normalized { tables: vec![t], ..Default::default() }
    }

    #[test]
    fn long_view_rows_and_filters() {
        let norm = wide();
        let t = &norm.tables[0];
        let lv = table_view(t).expect("a long view");
        assert_eq!(lv.name, "T1_long");
        assert_eq!(lv.periods.len(), 4, "the change column 2021/ 2024 is not a period");
        assert_eq!(lv.keys, vec![0]);
        assert!(!lv.has_measure);
        let db = Db::load(&norm).unwrap();
        let r = db.query("SELECT period, value, _src_col, _src_row FROM T1_long WHERE Region = 'South' ORDER BY period", 50, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(r.rows, vec![
            vec!["2021", "1", "B", "3"],
            vec!["2022", "2", "C", "3"],
            vec!["2023", "3", "D", "3"],
            vec!["2024", "4", "E", "3"],
        ]);
        // Empty cells are left out; total rows are in, marked.
        let n = db.query("SELECT count(*) FROM T1_long", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(n.rows[0][0], "7");
        assert!(db.query("SELECT _row_type FROM T1_long", 5, DEFAULT_TIMEOUT).is_err(), "T1_long has data rows only");
        let n = db.query("SELECT count(*) FROM T1_long_all WHERE _row_type = 'data'", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(n.rows[0][0], "7");
        let tot = db.query("SELECT value FROM T1_long_all WHERE _row_type = 'total' AND period = '2024'", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(tot.rows[0][0], "44");
        let line = catalog_line(t, &lv, None, true);
        assert!(line.contains("T1_long: one row per (Region, period)"), "{line}");
        assert!(line.contains("period '2021'..'2024'") && line.contains("by year") && line.contains("total rows only in T1_long_all"), "{line}");
        // B21: types from the title (bare year headers), the change column, the label column.
        let tys = crate::types::classify(t, &sql_names(t), None, &[]);
        let ty = |i: usize| tys[i].clone().unwrap();
        assert_eq!((ty(1).sem, ty(1).currency.as_deref()), (Sem::Amount, Some("EUR")));
        assert_eq!(ty(5).sem, Sem::Rate);
        assert_eq!(ty(0).sem, Sem::Dimension);
        let no_sum = crate::types::no_sum_line(t, &sql_names(t), &tys, None).unwrap();
        assert!(no_sum.contains("\"2021/ 2024\" (rate)"), "{no_sum}");
    }

    #[test]
    fn repeated_periods_get_a_measure_column() {
        let mut norm = wide();
        let t = &mut norm.tables[0];
        let extra: Vec<Column> = (0..4)
            .map(|k| col(&format!("Cost / {}", 2021 + k), ColRole::Data, Dtype::Number, 10 + k, vec![Out::Num(1.0); 3]))
            .collect();
        for (k, c) in t.columns.iter_mut().enumerate().skip(1).take(4) {
            c.name = format!("Sales / {}", 2020 + k);
        }
        let at = t.columns.len() - 2;
        t.columns.splice(at..at, extra);
        let lv = table_view(t).expect("a long view");
        assert!(lv.has_measure);
        assert_eq!(lv.periods.len(), 8);
        let db = Db::load(&norm).unwrap();
        let r = db.query("SELECT measure, count(*) FROM T1_long WHERE period = '2023' GROUP BY measure ORDER BY measure", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(r.rows, vec![vec!["Cost", "2"], vec!["Sales", "2"]], "data rows only");
    }

    #[test]
    fn rests() {
        assert_eq!(header_rest("MTD Avg / 2002-02-05"), "MTD Avg");
        assert_eq!(header_rest("Thu / 2002-01-31"), "");
        assert_eq!(header_rest("2025"), "");
        assert_eq!(header_rest("Umsatz / 2023"), "Umsatz");
        assert_eq!(header_rest("Oktober / 2022 (2)"), "");
        assert_eq!(header_rest("Revenue 2024-03"), "Revenue");
        assert_eq!(header_rest("2024 Q1"), "");
        assert_eq!(header_rest("Month-2 Avg / 2001-12-05"), "Month-2 Avg");
        assert_eq!(header_rest("FY 2024"), "FY");
    }

    #[test]
    fn periods_written_as_text() {
        // B89: Eurostat's TIME_PERIOD codes; labels that only mention a year are not periods.
        assert_eq!(text_period("2019-S1"), Some(("2019-S1".into(), "half-year")));
        assert_eq!(text_period("2019S2"), Some(("2019-S2".into(), "half-year")));
        assert_eq!(text_period("2024-Q3"), Some(("2024-Q3".into(), "quarter")));
        assert_eq!(text_period("2024M03"), Some(("2024-03".into(), "month")));
        assert_eq!(text_period("2024-03"), Some(("2024-03".into(), "month")));
        assert_eq!(text_period("2024"), Some(("2024".into(), "year")));
        for s in ["Jan-Mar 1971", "2024-13", "20241", "2024 total", "12345", "2024-S3"] {
            assert_eq!(text_period(s), None, "{s}");
        }
        assert!(update_stamp("LAST UPDATE") && update_stamp("Stand") && !update_stamp("TIME_PERIOD"));
        // A table dated only by its update stamp: the coverage comes from TIME_PERIOD.
        let mut n = wide();
        let t = &mut n.tables[0];
        let rows = t.rows();
        let periods: Vec<Out> = (0..rows).map(|i| Out::Text(format!("{}-S{}", 2019 + i / 2, 1 + i % 2))).collect();
        t.columns.insert(0, col("TIME_PERIOD", ColRole::Data, Dtype::Text, 0, periods));
        let stamp = |n: &str| TimeCoverage {
            from: "2026-10-19".into(),
            to: "2026-10-19".into(),
            granularity: "day",
            source: "column",
            columns: vec![n.into()],
            tables: vec!["T1".into()],
        };
        let mut covs = vec![Some(stamp("LAST UPDATE"))];
        let mut wb = Some(stamp("T1.LAST UPDATE"));
        fix_stamp_coverage(&n, &mut covs, &mut wb);
        let wb = wb.unwrap();
        assert_eq!((wb.from.as_str(), wb.granularity, wb.columns.clone()), ("2019-S1", "half-year", vec!["T1.TIME_PERIOD".to_string()]));
        assert_eq!(covs[0].as_ref().unwrap().columns, ["TIME_PERIOD"]);
        // A real date column is left alone.
        let mut covs = vec![Some(stamp("Datum"))];
        let mut wb = Some(stamp("T1.Datum"));
        fix_stamp_coverage(&n, &mut covs, &mut wb);
        assert_eq!(wb.unwrap().granularity, "day");
    }

    #[test]
    fn periods_of_headers() {
        let p = |s: &str| period_of(s).map(|(l, g)| format!("{l}:{g}"));
        assert_eq!(p("2025").as_deref(), Some("2025:year"));
        assert_eq!(p("Jan 2024").as_deref(), Some("2024-01:month"));
        assert_eq!(p("2024 Q1").as_deref(), Some("2024-Q1:quarter"));
        assert_eq!(p("Rev 2024-03").as_deref(), Some("2024-03:month"));
        assert_eq!(p("Umsatz 2024-03-31").as_deref(), Some("2024-03-31:day"));
        assert_eq!(p("Thu / 2002-01-31").as_deref(), Some("2002-01-31:day"));
        assert_eq!(p("2020/ 2025"), None);
        assert_eq!(p("Code 12345"), None);
    }
}
