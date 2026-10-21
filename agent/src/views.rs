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
//! value's cell is `<_src_col><_src_row>`). Every row of `T<n>_all` is in it (total rows marked by
//! `_row_type`); empty cells are left out.
//!
//! The view is a plain SQLite view (a `UNION ALL` of one `SELECT` per period column over
//! `T<n>_all`): nothing is copied at load, and a filter on `period` skips the other branches
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
    /// `T<n>_long`.
    pub name: String,
    /// `T<n>_all`, the view's source.
    pub source: String,
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
        source: format!("{}_all", t.id()),
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

/// `CREATE VIEW` statement of a long view.
pub fn create_sql(t: &Table, lv: &LongView) -> String {
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
        sel.push("\"_row_type\"".into());
        sel.push("\"_src_row\"".into());
        sel.push(format!("{} AS {}", sql_text(&c.source_col.map(letter).unwrap_or_default()), quote_ident(&src_col)));
        branches.push(format!("SELECT {} FROM {} WHERE {col} IS NOT NULL", sel.join(", "), quote_ident(&lv.source)));
    }
    format!("CREATE VIEW {} AS {}", quote_ident(&lv.name), branches.join(" UNION ALL "))
}

/// Creates the long views of all tables (called by [`crate::sql::Db::load`] inside its
/// transaction, after the tables and their `T<n>` / `T<n>_all` views exist). Returns the
/// tables that got one.
pub fn create_all(tx: &Transaction<'_>, norm: &hh_core::normalize::Normalized) -> rusqlite::Result<Vec<usize>> {
    let mut made = Vec::new();
    for t in &norm.tables {
        let Some(lv) = table_view(t) else { continue };
        tx.execute_batch(&create_sql(t, &lv))?;
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
        s.push_str(" · incl. total rows (_row_type)");
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
    use hh_core::normalize::{Column, Normalized, Out, RowType, Transforms};
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
        let n = db.query("SELECT count(*) FROM T1_long WHERE _row_type = 'data'", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(n.rows[0][0], "7");
        let tot = db.query("SELECT value FROM T1_long WHERE _row_type = 'total' AND period = '2024'", 5, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(tot.rows[0][0], "44");
        let line = catalog_line(t, &lv, None, true);
        assert!(line.contains("T1_long: one row per (Region, period)"), "{line}");
        assert!(line.contains("period '2021'..'2024'") && line.contains("by year") && line.contains("total rows"), "{line}");
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
        assert_eq!(r.rows, vec![vec!["Cost", "3"], vec!["Sales", "3"]]);
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
