//! Facts about a table's values that the catalog hands over before the first query: small tables
//! in full (total rows marked), how to select the total rows, sign conventions, sums of amount
//! columns (and which of them look alike) and a data-quality line. Each is a cheap pass over the
//! table's columns; the catalog adds the lines while it stays within its budget.

use std::collections::{HashMap, HashSet};

use hh_core::normalize::{ColRole, Column, Dtype, Out, RowType, Table};

use crate::catalog::{data_cols, is_date, is_numeric, label_values, measure_classes, name_period, out_text, plural, RowInfo};
use crate::text::{csv_row, ident, num, quote_ident, short, thousands};

/// A small table is inlined in full when its rows × columns (as in `T<n>_all`, with `_row_type`)
/// are at most this many cells …
const INLINE_CELLS: usize = 400;
/// … and it has at most this many rows.
const INLINE_ROWS: usize = 30;
/// Amount columns get sums / fill / range only in tables with at least this many data rows (smaller
/// tables show their values or ranges already).
const NUMBERS_MIN_ROWS: usize = 30;
/// Items of one `numbers:` line.
const NUMBER_ITEMS: usize = 6;

// ------------------------------------------------------------------------------------------------
// small tables in full

/// All rows of a small table as CSV: the rows of `T<n>_all` (data and total rows in source order)
/// with a `_row_type` column when the table has total rows; None when it is over the cell budget.
pub fn inline_rows(t: &Table, names: &[String]) -> Option<Vec<String>> {
    let cols: Vec<usize> = data_cols(t).filter(|(_, c)| c.dtype != Dtype::Empty).map(|(i, _)| i).collect();
    let rows = t.rows();
    let marked = t.row_types.iter().any(|r| *r != RowType::Data);
    let width = cols.len() + usize::from(marked);
    if rows == 0 || cols.is_empty() || rows > INLINE_ROWS || rows * width > INLINE_CELLS {
        return None;
    }
    let mut head: Vec<String> = cols.iter().map(|&i| short(&names[i], 60)).collect();
    let mut out = if marked {
        head.push("_row_type".into());
        vec![format!(
            "    all {} (CSV, as in {id}_all; rows with _row_type total/subtotal are not in {id}):",
            plural(rows, "row", "rows"),
            id = t.id()
        )]
    } else {
        vec![format!("    all {} (CSV):", plural(rows, "row", "rows"))]
    };
    out.push(format!("    {}", csv_row(&head)));
    for r in 0..rows {
        let mut vals: Vec<String> = cols
            .iter()
            .map(|&i| {
                let c = &t.columns[i];
                short(&c.values.get(r).map(|v| out_text(v, c.dtype)).unwrap_or_default(), 60)
            })
            .collect();
        if marked {
            vals.push(t.row_types.get(r).copied().unwrap_or(RowType::Data).as_str().into());
        }
        out.push(format!("    {}", csv_row(&vals)));
    }
    Some(out)
}

// ------------------------------------------------------------------------------------------------
// signs

/// Numeric columns whose values can be added (not percentages).
fn value_cols(t: &Table) -> Vec<&Column> {
    data_cols(t).map(|(_, c)| c).filter(|c| is_numeric(c.dtype) && c.dtype != Dtype::Percent).collect()
}

/// +1 when a row's non-zero numbers are all positive, -1 when all negative, 0 otherwise.
fn row_sign(cols: &[&Column], r: usize) -> i8 {
    let (mut pos, mut neg) = (0, 0);
    for c in cols {
        if let Some(Out::Num(x)) = c.values.get(r) {
            if *x > 0.0 {
                pos += 1;
            } else if *x < 0.0 {
                neg += 1;
            }
        }
    }
    match (pos, neg) {
        (p, 0) if p > 0 => 1,
        (0, n) if n > 0 => -1,
        _ => 0,
    }
}

fn sign_word(s: i8) -> &'static str {
    match s {
        1 => "positive",
        -1 => "negative",
        _ => "mixed",
    }
}

/// Labels of a text column for the given rows, a label written once per block counting for the
/// rows below it.
fn filled_labels<'a>(c: &'a Column, rows: &[usize]) -> Vec<Option<&'a str>> {
    let mut last: Option<&str> = None;
    rows.iter()
        .map(|&r| {
            if let Some(Out::Text(s)) = c.values.get(r)
                && !s.trim().is_empty()
            {
                last = Some(s.as_str());
            }
            last
        })
        .collect()
}

const SIGN_TAIL: &str = "(outflows?) — a net figure is the SUM over all rows, not a difference";

/// Sign convention: a block of rows whose numbers are all negative while others are positive
/// (deliveries next to receipts). By a label column when one marks the block, else by source rows.
pub fn signs_line(t: &Table, names: &[String], rows: &[usize]) -> Option<String> {
    let cols = value_cols(t);
    if cols.is_empty() || rows.len() < 3 {
        return None;
    }
    let signs: Vec<i8> = rows.iter().map(|&r| row_sign(&cols, r)).collect();
    if !signs.contains(&1) || !signs.contains(&-1) {
        return None;
    }
    // A label column: group column first, then text columns with 2..12 labels.
    let mut cands: Vec<usize> = t.columns.iter().position(|c| c.role == ColRole::Group).into_iter().collect();
    cands.extend(t.columns.iter().enumerate().filter(|(_, c)| c.role == ColRole::Data && c.dtype == Dtype::Text).map(|(i, _)| i));
    for i in cands {
        let labels = filled_labels(&t.columns[i], rows);
        let mut by: Vec<(&str, usize, usize, usize)> = Vec::new(); // label, rows, negative rows, positive rows
        for (k, l) in labels.iter().enumerate() {
            let Some(l) = l else { continue };
            let at = match by.iter().position(|b| b.0 == *l) {
                Some(p) => p,
                None if by.len() > 12 => break,
                None => {
                    by.push((l, 0, 0, 0));
                    by.len() - 1
                }
            };
            by[at].1 += 1;
            match signs[k] {
                -1 => by[at].2 += 1,
                1 => by[at].3 += 1,
                _ => {}
            }
        }
        if !(2..=12).contains(&by.len()) {
            continue;
        }
        let neg: Vec<&str> = by.iter().filter(|b| b.2 >= 2 && b.3 == 0).map(|b| b.0).collect();
        if !neg.is_empty() && by.iter().any(|b| b.3 > 0) {
            let l: Vec<String> = neg.iter().take(3).map(|s| format!("'{}'", short(s, 40))).collect();
            return Some(format!(
                "    signs: {} {} rows are all negative {SIGN_TAIL}",
                ident(&names[i]),
                l.join(", ")
            ));
        }
    }
    // No label: runs of negative rows by source row (rows without numbers do not break a run).
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let mut cur: Option<(u32, u32, usize)> = None;
    for (k, &r) in rows.iter().enumerate() {
        let src = t.src_rows.get(r).map_or(0, |s| s + 1);
        match signs[k] {
            -1 => cur = Some(cur.map_or((src, src, 1), |(a, _, n)| (a, src, n + 1))),
            1 => {
                if let Some((a, b, n)) = cur.take()
                    && n >= 2
                {
                    runs.push((a, b));
                }
            }
            _ => {}
        }
    }
    if let Some((a, b, n)) = cur
        && n >= 2
    {
        runs.push((a, b));
    }
    if runs.is_empty() {
        return None;
    }
    let r: Vec<String> = runs.iter().take(3).map(|(a, b)| format!("{a}–{b}")).collect();
    Some(format!("    signs: rows with _src_row {} are all negative {SIGN_TAIL}", r.join(", ")))
}

// ------------------------------------------------------------------------------------------------
// total rows

/// How to select a table's total rows, with the column that holds their labels (or, without a
/// label, their source rows and signs).
pub fn total_rows_line(t: &Table, names: &[String], info: &RowInfo) -> Option<String> {
    if info.total_rows.is_empty() {
        return None;
    }
    let idx: Vec<usize> = (0..t.rows()).filter(|&r| t.row_types.get(r).is_some_and(|x| *x != RowType::Data)).collect();
    let has = |k: RowType| idx.iter().any(|&r| t.row_types.get(r) == Some(&k));
    let cond = match (has(RowType::Total), has(RowType::Subtotal)) {
        (true, true) => "_row_type IN ('subtotal', 'total')",
        (false, true) => "_row_type = 'subtotal'",
        _ => "_row_type = 'total'",
    };
    // The column whose text is the row's label, the most frequent one.
    let mut at: HashMap<usize, usize> = HashMap::new();
    let mut labels: Vec<&str> = Vec::new();
    for (&r, tr) in idx.iter().zip(&info.total_rows) {
        let want = tr.label.trim();
        let hit = t.columns.iter().enumerate().filter(|(_, c)| matches!(c.role, ColRole::Data | ColRole::Group)).find(
            |(_, c)| matches!(c.values.get(r), Some(Out::Text(s)) if s.trim() == want),
        );
        if let Some((i, _)) = hit {
            *at.entry(i).or_default() += 1;
            if !labels.contains(&want) {
                labels.push(want);
            }
        }
    }
    let head = format!("    total rows: SELECT … FROM {}_all WHERE {cond}", t.id());
    if let Some((&i, _)) = at.iter().max_by_key(|(i, n)| (**n, std::cmp::Reverse(**i))) {
        let mut l: Vec<String> = labels.iter().take(3).map(|s| format!("'{}'", short(s, 32))).collect();
        if labels.len() > 3 {
            l.push(format!("+{} more", labels.len() - 3));
        }
        return Some(format!("{head} (label in {}: {})", ident(&names[i]), l.join(", ")));
    }
    let cols = value_cols(t);
    let rows: Vec<String> = idx
        .iter()
        .take(6)
        .map(|&r| {
            let src = t.src_rows.get(r).map_or(0, |s| s + 1);
            match row_sign(&cols, r) {
                0 => src.to_string(),
                s => format!("{src} {}", sign_word(s)),
            }
        })
        .collect();
    let more = idx.len().saturating_sub(6);
    Some(format!(
        "{head} (no label; _src_row {}{})",
        rows.join(", "),
        if more > 0 { format!(" +{more} more") } else { String::new() }
    ))
}

// ------------------------------------------------------------------------------------------------
// amount columns

/// Words of a column name, lowercase: split at non-alphanumerics and at camelCase humps.
fn words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in name.chars() {
        if !ch.is_alphanumeric() && !"€$£".contains(ch) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        cur.extend(ch.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// An identifier, code or calendar column by its name (`idcontrato`, `contractId`, `Nr`,
/// `Kalenderwoche`, `Ano`).
fn id_like_name(name: &str) -> bool {
    const EXACT: &[&str] = &[
        "id", "ids", "nr", "no", "nº", "num", "code", "codigo", "código", "key", "ref", "uuid", "ano", "year", "jahr",
        "month", "monat", "mes", "mês", "week", "woche", "day", "tag", "dia", "plz", "zip", "cpv", "nuts", "isin", "nif",
        "nipc", "vat", "pib", "jmbg", "quarter", "quartal",
    ];
    const STEMS: &[&str] = &["schlüssel", "schluessel", "nummer", "kennziffer", "kalenderwoche", "identif", "postcode"];
    let ws = words(name);
    let low = name.to_lowercase();
    ws.first().is_some_and(|w| w.starts_with("id") && w.len() > 3 && !w.starts_with("ide") && !w.starts_with("ida"))
        || ws.iter().any(|w| EXACT.contains(&w.as_str()))
        || STEMS.iter().any(|s| low.contains(s))
}

/// A money column by its name or currency.
fn money_like(c: &Column, name: &str) -> bool {
    const STEMS: &[&str] = &[
        "preco", "preço", "price", "precio", "prezzo", "prix", "valor", "value", "amount", "betrag", "kosten", "costo",
        "custo", "cost", "revenue", "umsatz", "erlös", "erloes", "sales", "salary", "gehalt", "lohn", "budget", "payment",
        "zahlung", "iznos", "cijena", "cena", "montant", "importe", "ausgaben", "einnahmen", "expense", "income", "profit",
        "gewinn", "spend", "turnover",
    ];
    const EXACT: &[&str] = &["eur", "euro", "euros", "usd", "chf", "gbp", "rsd", "din", "€", "$", "£"];
    if c.dtype == Dtype::Currency || c.currency.is_some() {
        return true;
    }
    let low = name.to_lowercase();
    STEMS.iter().any(|s| low.contains(s)) || words(name).iter().any(|w| EXACT.contains(&w.as_str()))
}

/// Facts of a numeric column over the data rows.
#[derive(Debug, Clone, Default)]
struct NumFacts {
    filled: usize,
    zero: usize,
    neg: usize,
    sum: f64,
    min: f64,
    max: f64,
    all_int: bool,
}

fn num_facts(c: &Column, rows: &[usize]) -> NumFacts {
    let mut f = NumFacts { min: f64::INFINITY, max: f64::NEG_INFINITY, all_int: true, ..Default::default() };
    for &r in rows {
        match c.values.get(r) {
            Some(Out::Num(x)) => {
                f.filled += 1;
                f.sum += x;
                f.min = f.min.min(*x);
                f.max = f.max.max(*x);
                if *x == 0.0 {
                    f.zero += 1;
                } else if *x < 0.0 {
                    f.neg += 1;
                }
                if x.fract() != 0.0 {
                    f.all_int = false;
                }
            }
            Some(Out::Null) | None => {}
            Some(_) => f.filled += 1,
        }
    }
    f
}

/// An amount column: one whose values may be added or ranged (not an ID, code, year, period
/// column, percentage, average or index).
struct Amount<'a> {
    col: &'a Column,
    name: &'a str,
    facts: NumFacts,
    money: bool,
    /// Its sum means something (money or a count).
    summable: bool,
}

fn amounts<'a>(t: &'a Table, names: &'a [String], rows: &[usize]) -> Vec<Amount<'a>> {
    let mut out = Vec::new();
    for (i, c) in data_cols(t) {
        let name = names[i].as_str();
        if !is_numeric(c.dtype) || c.dtype == Dtype::Percent || name_period(name).is_some() || id_like_name(name) {
            continue;
        }
        let low = name.to_lowercase();
        if measure_classes(name) & (2 | 8 | 16) != 0 || low.contains("index") || low.contains("quote") || low.contains("rate") {
            continue;
        }
        let f = num_facts(c, rows);
        if f.filled == 0 || f.min > f.max {
            continue;
        }
        // Years, flags, weeks, months, small codes.
        if f.all_int && ((f.min >= 1900.0 && f.max <= 2100.0) || (f.min >= 0.0 && f.max <= 60.0)) {
            continue;
        }
        let money = money_like(c, name);
        let summable = money || measure_classes(name) & 4 != 0;
        out.push(Amount { col: c, name, facts: f, money, summable });
    }
    out
}

/// 11.16 bn, 352.4 M, 1,234.5: big numbers short.
fn big(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.2} bn", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.2} M", v / 1e6)
    } else {
        // 6 significant digits, thousands separated.
        let r: f64 = format!("{v:.5e}").parse().unwrap_or(v);
        let s = num(r);
        let (int, frac) = s.split_once('.').map_or((s.as_str(), None), |(i, f)| (i, Some(f)));
        let digits = int.trim_start_matches('-');
        let mut out = String::from(if int.starts_with('-') { "-" } else { "" });
        out.push_str(&thousands(digits.parse().unwrap_or(0)));
        if let Some(f) = frac {
            out.push('.');
            out.push_str(f);
        }
        out
    }
}

fn pct(part: usize, whole: usize) -> String {
    if whole == 0 {
        return "0 %".into();
    }
    let p = 100.0 * part as f64 / whole as f64;
    if p > 0.0 && p < 1.0 {
        format!("{p:.1} %")
    } else if p > 99.0 && p < 100.0 {
        format!("{} %", (p * 10.0).floor() / 10.0)
    } else {
        format!("{} %", p.round())
    }
}

fn amount_text(a: &Amount, rows: usize, range: bool) -> String {
    let f = &a.facts;
    let mut parts = Vec::new();
    if a.summable {
        let cur = a.col.currency.as_ref().map(|c| format!(" {c}")).unwrap_or_default();
        parts.push(format!("sum {}{cur}", big(f.sum)));
    }
    parts.push(format!("{} filled", pct(f.filled, rows)));
    if f.zero * 10 >= f.filled.max(1) {
        parts.push(format!("{} zero", pct(f.zero, f.filled)));
    }
    if range {
        parts.push(format!("{}..{}", big(f.min), big(f.max)));
    }
    format!("{} ({})", ident(a.name), parts.join(", "))
}

fn similar_names(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    common >= 5 || (a.len() >= 5 && b.contains(&a)) || (b.len() >= 5 && a.contains(&b))
}

/// Sums, fill rates and ranges of the amount columns (`numbers:`), and one line per group of
/// look-alike amount columns (both money, or similar names) whose sums differ.
pub fn numbers_lines(t: &Table, names: &[String], rows: &[usize], ranges_shown: bool) -> Vec<String> {
    if rows.len() < NUMBERS_MIN_ROWS {
        return Vec::new();
    }
    let am = amounts(t, names, rows);
    // Look-alike groups among summable columns.
    let mut group: Vec<Option<usize>> = vec![None; am.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..am.len() {
        if !am[i].summable {
            continue;
        }
        let alike = |j: usize| {
            am[j].summable
                && ((am[i].money && am[j].money && am[i].col.currency == am[j].col.currency)
                    || similar_names(am[i].name, am[j].name))
        };
        let g = match (0..i).find(|&j| alike(j)).and_then(|j| group[j]) {
            Some(g) => g,
            None => {
                groups.push(Vec::new());
                groups.len() - 1
            }
        };
        group[i] = Some(g);
        groups[g].push(i);
    }
    let mut out = Vec::new();
    let mut in_line: HashSet<usize> = HashSet::new();
    for g in groups.iter().filter(|g| g.len() >= 2).take(2) {
        let sums: Vec<f64> = g.iter().map(|&k| am[k].facts.sum).collect();
        let (lo, hi) = sums.iter().fold((f64::MAX, f64::MIN), |(lo, hi), s| (lo.min(*s), hi.max(*s)));
        if hi - lo <= 0.01 * hi.abs().max(lo.abs()) {
            continue;
        }
        let items: Vec<String> = g.iter().take(4).map(|&k| amount_text(&am[k], rows.len(), false)).collect();
        out.push(format!("    {} — different measures; pick deliberately", items.join(" vs ")));
        in_line.extend(g.iter().copied());
    }
    let rest: Vec<String> = am
        .iter()
        .enumerate()
        .filter(|(k, a)| !in_line.contains(k) && (!ranges_shown || a.summable || a.facts.filled * 100 < rows.len() * 99))
        .take(NUMBER_ITEMS)
        .map(|(_, a)| amount_text(a, rows.len(), !ranges_shown))
        .collect();
    if !rest.is_empty() {
        out.insert(0, format!("    numbers: {}", rest.join(" · ")));
    }
    out
}

// ------------------------------------------------------------------------------------------------
// shares (B19)

/// A text column with at least this many distinct values can be ranked by (when there is no
/// entity column).
const SHARE_KEY_MIN_DISTINCT: usize = 10;

/// How to compute a share of the total (e.g. of the top 10) for a table with a summable amount
/// column: one ready query over the first such column, ranked by `key` (an SQL expression: an
/// entity's `<col>_id`) or else a text column with many values. None for small tables or without such columns.
pub fn share_line(t: &Table, names: &[String], rows: &[usize], key: Option<String>) -> Option<String> {
    if rows.len() < NUMBERS_MIN_ROWS {
        return None;
    }
    let am = amounts(t, names, rows);
    let a = am.iter().find(|a| a.money && a.summable).or_else(|| am.iter().find(|a| a.summable))?;
    let key = match key {
        Some(k) => k,
        None => data_cols(t)
            .find(|(_, c)| {
                if c.dtype != Dtype::Text {
                    return false;
                }
                let mut seen: HashSet<&str> = HashSet::new();
                for &r in rows {
                    if let Some(Out::Text(s)) = c.values.get(r) {
                        seen.insert(s);
                        if seen.len() >= SHARE_KEY_MIN_DISTINCT {
                            return true;
                        }
                    }
                }
                false
            })
            .map(|(i, _)| quote_ident(&names[i]))?,
    };
    let (x, k, id) = (quote_ident(a.name), key, t.id());
    Some(format!(
        "    share of the total (e.g. top 10 by {k}): SELECT 100.0 * SUM(v) / (SELECT SUM({x}) FROM {id}) AS pct \
         FROM (SELECT SUM({x}) AS v FROM {id} GROUP BY {k} ORDER BY v DESC LIMIT 10)"
    ))
}

// ------------------------------------------------------------------------------------------------
// long and wide forms of one table (B16)

/// Rows of the long table compared with the wide one.
const TWIN_SAMPLE: usize = 40;
/// Distinct values of the long table's category column, at most.
const TWIN_MAX_CATEGORIES: usize = 50;

/// A value as a join key.
fn key_text(v: Option<&Out>) -> String {
    match v {
        None | Some(Out::Null) => String::new(),
        Some(Out::Text(s)) => s.trim().to_string(),
        Some(Out::Num(x) | Out::Date(x)) => num(*x),
        Some(Out::Bool(b)) => b.to_string(),
    }
}

fn num_at(c: &Column, r: usize) -> Option<f64> {
    match c.values.get(r) {
        Some(Out::Num(x)) => Some(*x),
        _ => None,
    }
}

/// Pairs of tables where one is the long form of the other: the long table has a category column
/// whose values are column names of the wide table and a value column, and on sampled rows its
/// value equals the wide table's cell (same key columns, the category's column). Returns
/// (table index, line) for both tables of each pair, e.g. `T5 is the long form of T3: one row per
/// Datum × Saisonbereinigung, value in Indexwert`.
pub fn long_forms(tables: &[Table]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (li, l) in tables.iter().enumerate() {
        let lrows = crate::catalog::data_rows(l);
        if lrows.len() < 4 || out.iter().any(|(k, _)| *k == li) {
            continue;
        }
        let lnames = crate::catalog::sql_names(l);
        for (ci, c) in data_cols(l) {
            if c.dtype != Dtype::Text {
                continue;
            }
            let mut cats: Vec<&str> = Vec::new();
            for &r in &lrows {
                if let Some(Out::Text(s)) = c.values.get(r)
                    && !cats.contains(&s.trim())
                {
                    cats.push(s.trim());
                    if cats.len() > TWIN_MAX_CATEGORIES {
                        break;
                    }
                }
            }
            if !(2..=TWIN_MAX_CATEGORIES).contains(&cats.len()) {
                continue;
            }
            for (wi, w) in tables.iter().enumerate() {
                if wi == li || out.iter().any(|(k, _)| *k == wi) {
                    continue;
                }
                let wnames = crate::catalog::sql_names(w);
                let find = |name: &str| wnames.iter().position(|n| n.trim().eq_ignore_ascii_case(name.trim()));
                let Some(targets) = cats
                    .iter()
                    .map(|cat| find(cat).filter(|&k| is_numeric(w.columns[k].dtype)))
                    .collect::<Option<Vec<usize>>>()
                else {
                    continue;
                };
                // Key columns: the other columns both tables have, by name.
                let keys: Vec<(usize, usize)> = data_cols(l)
                    .filter(|(i, lc)| *i != ci && !matches!(lc.dtype, Dtype::Number | Dtype::Currency | Dtype::Percent | Dtype::Empty))
                    .filter_map(|(i, _)| find(&lnames[i]).filter(|k| !targets.contains(k)).map(|k| (i, k)))
                    .collect();
                if keys.is_empty() {
                    continue;
                }
                let wrows = crate::catalog::data_rows(w);
                let mut index: HashMap<Vec<String>, usize> = HashMap::with_capacity(wrows.len());
                for &r in &wrows {
                    index.insert(keys.iter().map(|&(_, k)| key_text(w.columns[k].values.get(r))).collect(), r);
                }
                if index.len() * 10 < wrows.len() * 9 {
                    continue; // the keys do not identify the wide table's rows
                }
                let step = (lrows.len() / TWIN_SAMPLE).max(1);
                let sample: Vec<usize> = lrows.iter().step_by(step).take(TWIN_SAMPLE).copied().collect();
                let value = data_cols(l).filter(|(i, vc)| *i != ci && is_numeric(vc.dtype) && !keys.iter().any(|k| k.0 == *i)).find(|(_, vc)| {
                    let (mut seen, mut same) = (0, 0);
                    for &r in &sample {
                        let Some(Out::Text(cat)) = c.values.get(r) else { continue };
                        let Some(pos) = cats.iter().position(|x| *x == cat.trim()) else { continue };
                        let key: Vec<String> = keys.iter().map(|&(i, _)| key_text(l.columns[i].values.get(r))).collect();
                        let (Some(&wr), Some(a)) = (index.get(&key), num_at(vc, r)) else { continue };
                        seen += 1;
                        if num_at(&w.columns[targets[pos]], wr).is_some_and(|b| (a - b).abs() <= 1e-9 * a.abs().max(1.0)) {
                            same += 1;
                        }
                    }
                    seen >= 4.min(sample.len()) && same * 10 >= seen * 9
                });
                let Some((vi, _)) = value else { continue };
                // The key shown: the first that alone tells the wide rows apart, else all of them.
                let unique = keys.iter().find(|&&(_, k)| {
                    let set: HashSet<String> = wrows.iter().map(|&r| key_text(w.columns[k].values.get(r))).collect();
                    set.len() == wrows.len()
                });
                let key_names: Vec<String> = match unique {
                    Some(&(i, _)) => vec![ident(&lnames[i])],
                    None => keys.iter().map(|&(i, _)| ident(&lnames[i])).collect(),
                };
                let line = format!(
                    "    {} is the long form of {}: one row per {} × {}, value in {} ({} has one column per {} value)",
                    l.id(),
                    w.id(),
                    key_names.join(" × "),
                    ident(&lnames[ci]),
                    ident(&lnames[vi]),
                    w.id(),
                    ident(&lnames[ci])
                );
                out.push((li, line.clone()));
                out.push((wi, line));
                break;
            }
            if out.iter().any(|(k, _)| *k == li) {
                break;
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------
// data quality

/// The label column and values that explain where a date column is empty (most of the empty
/// rows share one or two labels, and most rows with those labels are empty).
fn empty_explained(t: &Table, names: &[String], date: &Column, rows: &[usize], empty: usize) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (i, c) in data_cols(t) {
        if label_values(c, rows).is_none() {
            continue;
        }
        let mut by: HashMap<&str, (usize, usize)> = HashMap::new(); // label → (empty, rows)
        for &r in rows {
            if let Some(Out::Text(s)) = c.values.get(r) {
                let e = by.entry(s.as_str()).or_default();
                e.1 += 1;
                if matches!(date.values.get(r), Some(Out::Null) | None) {
                    e.0 += 1;
                }
            }
        }
        let mut v: Vec<(&str, usize, usize)> = by.into_iter().map(|(k, (e, n))| (k, e, n)).filter(|x| x.1 > 0).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        // A label explains the empty cells when most of its rows are empty, clearly more than
        // the column overall.
        let overall = empty as f64 / rows.len() as f64;
        let need = (2.0 * overall).clamp(0.5, 0.9);
        let mut covered = 0;
        let mut used = Vec::new();
        for (l, e, n) in v.iter().take(2) {
            if (*e as f64) < need * *n as f64 {
                break;
            }
            covered += e;
            used.push(if e == n {
                format!("'{}' (all its {} rows)", short(l, 48), thousands(*n))
            } else {
                format!("'{}' ({} of its {} rows)", short(l, 48), thousands(*e), thousands(*n))
            });
        }
        if used.is_empty() || covered * 100 < empty * 95 {
            continue;
        }
        if best.as_ref().is_none_or(|(k, _)| used.len() < *k) {
            best = Some((used.len(), format!("{} = {}", ident(&names[i]), used.join(" or "))));
        }
    }
    best.map(|(_, s)| s)
}

/// The `quality:` line: duplicates in ID columns, empty date cells (and which category they are
/// in), negative amounts. (Cells naming several entities are on the entity line, `entity.rs`.)
pub fn quality_line(t: &Table, names: &[String], rows: &[usize], signs: bool) -> Option<String> {
    let n = rows.len();
    if n < 20 {
        return None;
    }
    let mut items: Vec<String> = Vec::new();
    // Duplicates in an ID column that is unique but for a few values.
    for (i, c) in data_cols(t) {
        if !matches!(c.dtype, Dtype::Integer | Dtype::Text) || !id_like_name(&names[i]) {
            continue;
        }
        let mut seen: HashSet<(u64, &str)> = HashSet::with_capacity(n);
        let mut filled = 0;
        let mut dups = 0;
        for &r in rows {
            let key = match c.values.get(r) {
                Some(Out::Num(x)) | Some(Out::Date(x)) => (x.to_bits(), ""),
                Some(Out::Text(s)) => (0, s.as_str()),
                Some(Out::Bool(b)) => (u64::from(*b), "bool"),
                Some(Out::Null) | None => continue,
            };
            filled += 1;
            if !seen.insert(key) {
                dups += 1;
                if dups * 20 > filled.max(n / 2) {
                    break;
                }
            }
        }
        // At most 5 % repeated: meant to be unique (a key with many repeats is a reference).
        if dups > 0 && dups * 20 <= filled && filled >= 20 {
            items.push(format!("{} duplicate {} values", thousands(dups), ident(&names[i])));
        }
    }
    // Empty date cells.
    for (i, c) in data_cols(t).filter(|(_, c)| is_date(c.dtype)).take(6) {
        let empty = rows.iter().filter(|&&r| matches!(c.values.get(r), Some(Out::Null) | None)).count();
        if empty == 0 || empty * 100 < n || empty == n {
            continue;
        }
        let mut s = format!("{} empty in {} ({})", ident(&names[i]), plural(empty, "row", "rows"), pct(empty, n));
        if let Some(why) = empty_explained(t, names, c, rows, empty) {
            s.push_str(&format!(": {why}"));
        }
        s.push_str(" — counts by this date leave them out");
        items.push(s);
        if items.len() >= 4 {
            break;
        }
    }
    // Negative amounts (a block of negative rows is a sign convention, said by `signs:`).
    if !signs {
        let neg: Vec<String> = amounts(t, names, rows)
            .iter()
            .filter(|a| a.facts.neg > 0 && a.facts.neg * 2 < a.facts.filled - a.facts.zero)
            .take(4)
            .map(|a| format!("{} negative {}", thousands(a.facts.neg), ident(a.name)))
            .collect();
        if !neg.is_empty() {
            items.push(neg.join(", "));
        }
    }
    (!items.is_empty()).then(|| format!("    quality: {}", items.join(" · ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog;
    use crate::text::MAX_CHARS;
    use hh_core::normalize::{Normalized, SheetInfo, Transforms};
    use hh_core::Rect;

    fn col(name: &str, dtype: Dtype, values: Vec<Out>) -> Column {
        Column {
            name: name.into(),
            role: ColRole::Data,
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

    fn text(v: &[&str]) -> Vec<Out> {
        v.iter().map(|s| if s.is_empty() { Out::Null } else { Out::Text(s.to_string()) }).collect()
    }

    fn nums(v: &[f64]) -> Vec<Out> {
        v.iter().map(|x| Out::Num(*x)).collect()
    }

    /// One table of `columns` (plus `_src_row`), rows of the given types.
    fn book(mut columns: Vec<Column>, types: Vec<RowType>) -> Normalized {
        let rows = types.len();
        columns.push(Column {
            role: ColRole::SrcRow,
            ..col("_src_row", Dtype::Integer, (0..rows).map(|i| Out::Num(i as f64 + 2.0)).collect())
        });
        let t = Table {
            n: 1,
            sheet: "T1_s".into(),
            source_sheet: "Sheet 1".into(),
            rect: Rect { top: 0, left: 0, bottom: rows as u32, right: columns.len() as u32 },
            header_top: 0,
            header_rows: 1,
            title: None,
            notes: vec![],
            confidence: 0.9,
            columns,
            src_rows: (1..=rows as u32).collect(),
            row_types: types,
            transforms: Transforms::default(),
            warnings: vec![],
        };
        let sheets = vec![SheetInfo { name: "Sheet 1".into(), tables: vec![1], cells: 10, ..Default::default() }];
        Normalized { sheets, tables: vec![t], ..Default::default() }
    }

    #[test]
    fn name_words() {
        assert_eq!(words("PrecoTotalEfetivo"), ["preco", "total", "efetivo"]);
        assert_eq!(words("idINCM"), ["id", "incm"]);
        assert!(id_like_name("idcontrato") && id_like_name("contractId") && id_like_name("Kalenderwoche"));
        assert!(id_like_name("Ano") && id_like_name("Stat. Schlüsselnummer"));
        assert!(!id_like_name("precoContratual") && !id_like_name("Idade") && !id_like_name("Zuzüge insgesamt"));
        assert_eq!(big(11_158_516_539.0), "11.16 bn");
        assert_eq!(big(352_400_000.0), "352.40 M");
        assert_eq!(pct(26_722, 176_474), "15 %");
        assert_eq!(pct(176_473, 176_474), "99.9 %");
    }

    /// Like the Austrian pension file: 12 rows × 19 columns, a total row in the middle.
    #[test]
    fn small_wide_table_inline_with_total_rows() {
        let groups = ["Anzahl", "Anzahl", "Anzahl", "Anzahl", "", "Euro", "Euro", "Euro", "Euro", "Satz", "Satz"];
        let status = ["Allein", "M", "F", "Verh", "Gesamt", "Gesamt", "Allein", "M", "F", "Verh", "Allein", "Verh"];
        let mut cols = vec![
            Column { role: ColRole::Group, ..col("group", Dtype::Text, text(&groups)) },
            col("Richtsatz", Dtype::Text, text(&status)),
        ];
        for y in 2011..=2025 {
            cols.push(col(&y.to_string(), Dtype::Number, nums(&[(y - 2000) as f64 + 0.5; 12])));
        }
        cols.push(col("2020/ 2025", Dtype::Number, nums(&[0.1; 12])));
        cols.push(col("2024/ 2025", Dtype::Number, nums(&[0.2; 12])));
        let mut types = vec![RowType::Data; 12];
        types[4] = RowType::Total;
        let n = book(cols, types);
        let c = catalog(&n, "x.xlsx", MAX_CHARS);
        assert!(c.contains("all 12 rows (CSV, as in T1_all; rows with _row_type total/subtotal are not in T1):"), "{c}");
        assert!(c.contains("\n    group,Richtsatz,2011,2012,2013,2014,2015,2016,2017,2018,2019,2020,2021,2022,2023,2024,2025,2020/ 2025,2024/ 2025,_row_type\n"), "{c}");
        assert!(c.contains("\n    ,Gesamt,11.5,12.5,13.5,14.5,15.5,16.5,17.5,18.5,19.5,20.5,21.5,22.5,23.5,24.5,25.5,0.1,0.2,total\n"), "{c}");
        assert!(c.contains("\n    Euro,Allein,11.5,"), "{c}");
        assert!(c.contains("total rows: SELECT … FROM T1_all WHERE _row_type = 'total' (label in Richtsatz: 'Gesamt')"), "{c}");
    }

    /// Like the Enron Henry Hub report: receipts, then deliveries as negative numbers, a total row
    /// after each block.
    fn flows(labels: bool) -> Normalized {
        let kind = ["Receipts", "", "", "", "Deliveries", "", "", ""];
        let name = ["A", "B", "C", "Total", "D", "E", "F", "Total"];
        let v = [10.0, 0.0, 5.0, 15.0, -3.0, -4.0, 0.0, -7.0];
        let mut types = vec![RowType::Data; 8];
        types[3] = RowType::Total;
        types[7] = RowType::Total;
        let mut cols = vec![col("Feb", Dtype::Integer, nums(&v)), col("Jan", Dtype::Integer, nums(&v.map(|x| 2.0 * x)))];
        if labels {
            cols.insert(0, col("col_C", Dtype::Text, text(&name)));
            cols.insert(0, col("col_B", Dtype::Text, text(&kind)));
        }
        book(cols, types)
    }

    #[test]
    fn total_rows_and_signs() {
        let c = catalog(&flows(true), "x.xlsx", MAX_CHARS);
        assert!(c.contains("\n    total rows: SELECT … FROM T1_all WHERE _row_type = 'total' (label in col_C: 'Total')\n"), "{c}");
        assert!(
            c.contains("\n    signs: col_B 'Deliveries' rows are all negative (outflows?) — a net figure is the SUM over all rows, not a difference\n"),
            "{c}"
        );
        // No label column: source rows, and the sign of each total row.
        let c = catalog(&flows(false), "x.xlsx", MAX_CHARS);
        assert!(c.contains("total rows: SELECT … FROM T1_all WHERE _row_type = 'total' (no label; _src_row 5 positive, 9 negative)"), "{c}");
        assert!(c.contains("signs: rows with _src_row 6–7 are all negative"), "{c}");
        // All positive: no sign line.
        let mut n = flows(true);
        for c in &mut n.tables[0].columns {
            for v in &mut c.values {
                if let Out::Num(x) = v {
                    *x = x.abs();
                }
            }
        }
        assert!(!catalog(&n, "x.xlsx", MAX_CHARS).contains("signs:"));
    }

    /// Like the Portuguese contracts register: an ID with one duplicate, two price columns, signing
    /// dates missing for one procedure type, a negative price.
    fn register() -> Normalized {
        let rows = 40;
        let ids: Vec<f64> = (0..rows).map(|i| if i == 39 { 1000.0 } else { 1000.0 + i as f64 }).collect();
        let tipo: Vec<&str> = (0..rows).map(|i| if i % 4 == 0 { "Simplificado" } else { "Concurso" }).collect();
        let date: Vec<Out> = (0..rows).map(|i| if i % 4 == 0 { Out::Null } else { Out::Date(43831.0 + i as f64) }).collect();
        let preco: Vec<f64> = (0..rows).map(|i| if i == 7 { -50.0 } else { 1000.0 * (i + 1) as f64 + 0.5 }).collect();
        let efetivo: Vec<f64> = (0..rows).map(|i| if i % 2 == 0 { 0.0 } else { 300.0 * i as f64 }).collect();
        let winners: Vec<String> =
            (0..rows).map(|i| if i % 10 == 3 { format!("50{i:07} - Alpha, Lda 51{i:07} - Beta SA") } else { format!("50{i:07} - Alpha, Lda") }).collect();
        let cols = vec![
            col("idcontrato", Dtype::Integer, nums(&ids)),
            col("tipoprocedimento", Dtype::Text, text(&tipo)),
            col("adjudicatarios", Dtype::Text, winners.iter().map(|s| Out::Text(s.clone())).collect()),
            col("dataCelebracaoContrato", Dtype::Date, date),
            col("precoContratual", Dtype::Number, nums(&preco)),
            col("PrecoTotalEfetivo", Dtype::Number, nums(&efetivo)),
            col("Ano", Dtype::Integer, nums(&[2020.0; 40])),
        ];
        book(cols, vec![RowType::Data; rows])
    }

    #[test]
    fn amounts_and_quality() {
        let c = catalog(&register(), "x.xlsx", MAX_CHARS);
        assert!(
            c.contains("\n    precoContratual (sum 811,970, 100 % filled) vs PrecoTotalEfetivo (sum 120,000, 100 % filled, 50 % zero) — different measures; pick deliberately\n"),
            "{c}"
        );
        // IDs and years are not amounts.
        assert!(!c.contains("idcontrato (") && !c.contains("Ano ("), "{c}");
        let q = c.lines().find(|l| l.starts_with("    quality: ")).unwrap_or_else(|| panic!("{c}"));
        assert!(q.contains("1 duplicate idcontrato values"), "{q}");
        assert!(
            q.contains("dataCelebracaoContrato empty in 10 rows (25 %): tipoprocedimento = 'Simplificado' (all its 10 rows) — counts by this date leave them out"),
            "{q}"
        );
        assert!(q.contains("1 negative precoContratual"), "{q}");
        // Too few rows: no column facts.
        let mut small = register();
        let t = &mut small.tables[0];
        for c in &mut t.columns {
            c.values.truncate(19);
        }
        t.src_rows.truncate(19);
        t.row_types.truncate(19);
        let c = catalog(&small, "x.xlsx", MAX_CHARS);
        assert!(!c.contains("quality:") && !c.contains("pick deliberately"), "{c}");
    }

    #[test]
    fn optional_parts_stay_within_the_budget() {
        let n = register();
        let full = catalog(&n, "x.xlsx", MAX_CHARS);
        assert!(full.contains("quality:") && full.contains("pick deliberately"), "{full}");
        // Less room: column facts dropped first, the catalog stays full (not brief) and in budget.
        let q = full.lines().find(|l| l.starts_with("    quality: ")).unwrap().len();
        let c = catalog(&n, "x.xlsx", full.len() - q);
        assert!(c.len() <= full.len() - q, "{c}");
        assert!(!c.contains("quality:") && c.contains("pick deliberately") && !c.contains("listed briefly"), "{c}");
        let mut f = flows(true);
        let full = catalog(&f, "x.xlsx", MAX_CHARS);
        let c = catalog(&f, "x.xlsx", full.len() - 5);
        assert!(c.len() <= full.len() - 5 && c.contains("signs:") && !c.contains("(CSV"), "{c}");
        // A table with nothing to add.
        f.tables[0].row_types = vec![RowType::Data; 8];
        assert!(!catalog(&f, "x.xlsx", MAX_CHARS).contains("total rows:"));
    }
}
