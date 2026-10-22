//! Facts about a table's values that the catalog hands over before the first query: small tables
//! in full (total rows marked), how to select the total rows, sign conventions, sums of amount
//! columns (and which of them look alike) and a data-quality line. Each is a cheap pass over the
//! table's columns; the catalog adds the lines while it stays within its budget.

use std::collections::{HashMap, HashSet};

use hh_core::normalize::{ColRole, Column, Dtype, Normalized, Out, RowType, Table, T};

use crate::catalog::{data_cols, is_date, is_numeric, label_values, measure_classes, name_period, out_text, plural, RowInfo};
use crate::text::{csv_row, ident, num, quote_ident, short, thousands};
use crate::types::{ColType, Sem};

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
        let other = t.row_types.iter().any(|r| matches!(r, RowType::Average | RowType::OfWhich));
        vec![format!(
            "    all {} (CSV, as in {id}_all; rows with _row_type {} are not in {id}):",
            plural(rows, "row", "rows"),
            if other { "other than data" } else { "total/subtotal" },
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
                // B104: fractions to 6 significant digits (47117.8666667 → 47117.9): the
                // same value for an answer, half the tokens.
                let text = match c.values.get(r) {
                    Some(Out::Num(x)) if x.fract() != 0.0 && !is_date(c.dtype) => significant(*x),
                    v => v.map(|v| out_text(v, c.dtype)).unwrap_or_default(),
                };
                short(&text, 60)
            })
            .collect();
        if marked {
            vals.push(t.row_types.get(r).copied().unwrap_or(RowType::Data).as_str().into());
        }
        out.push(format!("    {}", csv_row(&vals)));
    }
    Some(out)
}

/// A number to 6 significant digits but every digit before the point, without trailing zeros;
/// as it is when it has at most 2 decimals (money: 31767420.82 stays).
fn significant(x: f64) -> String {
    let full = num(x);
    if full.split_once('.').is_none_or(|(_, f)| f.len() <= 2) {
        return full;
    }
    if x.abs() < 1.0 {
        let r: f64 = format!("{x:.5e}").parse().unwrap_or(x);
        return num(r);
    }
    let digits = (x.abs().log10().floor() as i32 + 1).max(1);
    let decimals = (6 - digits).max(0) as usize;
    let s = format!("{x:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
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

/// A label or column name of money or goods moving in or out (B49): receipts / deliveries,
/// debit / credit, in / out, purchases / sales, payments, imports / exports.
fn flow_word(s: &str) -> bool {
    const FLOW: &[&str] = &[
        "receipt", "receipts", "received", "delivery", "deliveries", "delivered", "in", "out", "inflow", "inflows",
        "outflow", "outflows", "incoming", "outgoing", "debit", "debits", "credit", "credits", "purchase", "purchases",
        "sale", "sales", "sold", "bought", "buy", "sell", "payment", "payments", "paid", "withdrawal", "withdrawals",
        "deposit", "deposits", "expense", "expenses", "expenditure", "expenditures", "cost", "costs", "income",
        "revenue", "revenues", "import", "imports", "export", "exports", "injection", "injections", "transfer",
        "transfers", "refund", "refunds", "spend", "spending", "einnahmen", "ausgaben", "soll", "haben", "uplata", "isplata",
    ];
    words(s).iter().any(|w| FLOW.contains(&w.as_str()))
}

/// A column of prices, rates or indices: negative values there are not outflows (B49).
fn rate_name(s: &str) -> bool {
    const RATE: &[&str] = &[
        "price", "prices", "index", "rate", "rates", "ratio", "percent", "pct", "yield", "growth", "change", "tariff",
        "average", "avg", "mean", "spread",
    ];
    words(s).iter().any(|w| RATE.contains(&w.as_str()))
}

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
    // B49: prices, rates and indices go negative without being outflows.
    if cols.iter().any(|c| rate_name(&c.name)) {
        return None;
    }
    let flow_cols = cols.iter().any(|c| flow_word(&c.name));
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
        // B49: only blocks named as flows (receipts / deliveries, debit / credit, …).
        if !neg.is_empty() && by.iter().any(|b| b.3 > 0) && (flow_cols || by.iter().any(|b| flow_word(b.0))) {
            let l: Vec<String> = neg.iter().take(3).map(|s| format!("'{}'", short(s, 40))).collect();
            return Some(format!(
                "    signs: {} {} rows are all negative {SIGN_TAIL}",
                ident(&names[i]),
                l.join(", ")
            ));
        }
    }
    // No label: runs of negative rows by source row (rows without numbers do not break a run).
    let mut runs: Vec<(u32, u32, usize)> = Vec::new();
    let mut cur: Option<(u32, u32, usize)> = None;
    for (k, &r) in rows.iter().enumerate() {
        let src = t.src_rows.get(r).map_or(0, |s| s + 1);
        match signs[k] {
            -1 => cur = Some(cur.map_or((src, src, 1), |(a, _, n)| (a, src, n + 1))),
            1 => {
                if let Some((a, b, n)) = cur.take()
                    && n >= 2
                {
                    runs.push((a, b, n));
                }
            }
            _ => {}
        }
    }
    if let Some((a, b, n)) = cur
        && n >= 2
    {
        runs.push((a, b, n));
    }
    // B49: without a flow label, only blocks: runs that hold at least a tenth of the signed rows
    // (a few scattered negative values, such as negative prices, are no sign convention).
    let in_runs: usize = runs.iter().map(|r| r.2).sum();
    let signed = signs.iter().filter(|s| **s != 0).count();
    if runs.is_empty() || !(flow_cols || in_runs * 10 >= signed) {
        return None;
    }
    let r: Vec<String> = runs.iter().take(3).map(|(a, b, _)| format!("{a}–{b}")).collect();
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
    let kinds: Vec<String> = [RowType::Subtotal, RowType::Total, RowType::Average, RowType::OfWhich]
        .into_iter()
        .filter(|&k| has(k))
        .map(|k| format!("'{}'", k.as_str()))
        .collect();
    let cond = match kinds.len() {
        0 => "_row_type = 'total'".to_string(),
        1 => format!("_row_type = {}", kinds[0]),
        _ => format!("_row_type IN ({})", kinds.join(", ")),
    };
    // The column whose text is the row's label, the most frequent one.
    let mut at: HashMap<usize, usize> = HashMap::new();
    for (&r, tr) in idx.iter().zip(&info.total_rows) {
        let want = tr.label.trim();
        let hit = t.columns.iter().enumerate().filter(|(_, c)| matches!(c.role, ColRole::Data | ColRole::Group)).find(
            |(_, c)| matches!(c.values.get(r), Some(Out::Text(s)) if s.trim() == want),
        );
        if let Some((i, _)) = hit {
            *at.entry(i).or_default() += 1;
        }
    }
    let head = format!("    total rows: SELECT … FROM {}_all WHERE {cond}", t.id());
    // B160: labels made of several key columns (`'Total/M'`), or a total code.
    if !info.key_cols.is_empty() || !info.codes.is_empty() {
        let entries = crate::catalog::total_entries(info);
        let keys: Vec<String> = info.key_cols.iter().map(|&k| ident(&names[k])).collect();
        let labels = if keys.is_empty() { String::new() } else { format!(" — labels in {}", keys.join("/")) };
        let sep = if labels.is_empty() { " — " } else { ": " };
        return Some(format!("{head}{labels}{sep}{}", crate::catalog::totals_text(t, info, &entries, true, 15)));
    }
    if let Some((&i, _)) = at.iter().max_by_key(|(i, n)| (**n, std::cmp::Reverse(**i))) {
        // B74: every label (up to 15) by level, the grand total named, what each adds up.
        let entries = crate::catalog::total_entries(info);
        let col = ident(&names[i]);
        return Some(format!(
            "{head} — labels in {col}: {}{}",
            crate::catalog::total_entries_text(&entries, &t.id(), true, 15),
            crate::catalog::group_totals_note(&entries, Some(&col)).unwrap_or_default()
        ));
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
    /// Index in `Table::columns`.
    idx: usize,
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
        out.push(Amount { idx: i, col: c, name, facts: f, money, summable });
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
    // B104: the fill rate only when cells are missing (a sum or a count over them is partial).
    if f.filled * 100 < rows * 99 {
        parts.push(format!("{} filled", pct(f.filled, rows)));
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
        // B104: a sum, or missing cells; a range only when the columns line has none.
        .filter(|(k, a)| !in_line.contains(k) && (a.summable || a.facts.filled * 10 < rows.len() * 9))
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
/// entity column) …
const SHARE_KEY_MIN_DISTINCT: usize = 10;
/// … and at most this many (more is a register of names, not a breakdown).
const SHARE_KEY_MAX_DISTINCT: usize = 1000;

/// A column naming the unit, currency or measure of each row: with several values in it, a sum
/// over all rows adds different things.
fn unit_column_name(name: &str) -> bool {
    const UNIT: &[&str] = &["unit", "units", "currency", "measure", "einheit", "währung", "waehrung", "unite", "unité", "unidade", "unidad", "moeda", "jedinica", "valuta", "uom"];
    words(name).iter().any(|w| UNIT.contains(&w.as_str()))
}

/// How to compute a share of the total (e.g. of the top 10) for a table with an additive amount
/// column: one ready query over the first such column, ranked by `key` (an SQL expression: an
/// entity's `<col>_id`) or else a dimension column with 10..1,000 values that is not a name or a
/// period. None for small tables, without such columns, for prices / rates (B89: their sum means
/// nothing) and when the rows mix units or currencies.
pub fn share_line(t: &Table, names: &[String], rows: &[usize], key: Option<String>, tys: &[Option<ColType>]) -> Option<String> {
    if rows.len() < NUMBERS_MIN_ROWS {
        return None;
    }
    let distinct = |c: &Column, cap: usize| {
        let mut seen: HashSet<&str> = HashSet::new();
        for &r in rows {
            if let Some(Out::Text(s)) = c.values.get(r) {
                seen.insert(s);
                if seen.len() > cap {
                    break;
                }
            }
        }
        seen.len()
    };
    if data_cols(t).any(|(i, c)| c.dtype == Dtype::Text && unit_column_name(&names[i]) && distinct(c, 2) >= 2) {
        return None;
    }
    // A unit price has few values (one per product: "Manufacturing Price"); a contract's price
    // is mostly its own and adds up.
    let unit_price = |a: &Amount| {
        let mut seen: HashSet<u64> = HashSet::new();
        for &r in rows {
            if let Some(Out::Num(x)) = a.col.values.get(r) {
                seen.insert(x.to_bits());
            }
        }
        seen.len() <= 12.max(rows.len() / 20)
    };
    let additive = |a: &&Amount| {
        let ws = words(a.name);
        let price = ws.iter().any(|w| matches!(w.as_str(), "price" | "prices" | "preis" | "preise" | "prix" | "precio" | "prezzo" | "cijena" | "cena"));
        a.summable
            && !(price && unit_price(a))
            && !(rate_name(a.name) && !price)
            && !tys.get(a.idx).and_then(|x| x.as_ref()).is_some_and(ColType::no_sum)
    };
    let am = amounts(t, names, rows);
    let a = am.iter().filter(additive).find(|a| a.money).or_else(|| am.iter().find(additive))?;
    let key = match key {
        Some(k) => k,
        // The breakdown with the most values (a middle initial or a flag has few).
        None => data_cols(t)
            .filter(|(i, c)| {
                let sem = tys.get(*i).and_then(|x| x.as_ref()).map(|x| x.sem);
                c.dtype == Dtype::Text && !matches!(sem, Some(Sem::Name | Sem::Period | Sem::Text)) && !id_like_name(&names[*i])
            })
            .map(|(i, c)| (i, distinct(c, SHARE_KEY_MAX_DISTINCT)))
            .filter(|(_, d)| (SHARE_KEY_MIN_DISTINCT..=SHARE_KEY_MAX_DISTINCT).contains(d))
            .max_by_key(|(i, d)| (*d, std::cmp::Reverse(*i)))
            .map(|(i, _)| quote_ident(&names[i]))?,
    };
    let (x, k, id) = (quote_ident(a.name), key, t.id());
    Some(format!(
        "    share of the total (e.g. top 10 by {k}): SELECT 100.0 * SUM(v) / (SELECT SUM({x}) FROM {id}) AS pct \
         FROM (SELECT SUM({x}) AS v FROM {id} GROUP BY {k} ORDER BY v DESC LIMIT 10)"
    ))
}

// ------------------------------------------------------------------------------------------------
// units from titles (B85)

/// The scale a title or note gives the table's numbers: `(All values in 000s)`, `in Tausend`,
/// `Mio. €`, `en milliers`, `(in millions)`: (factor, word).
pub fn scale_of(text: &str) -> Option<(f64, &'static str)> {
    const THOUSANDS: &[&str] = &["thousand", "thousands", "tausend", "milliers", "milhares", "miles", "hiljadama", "хиљадама", "hiljada", "migliaia"];
    const MILLIONS: &[&str] = &["million", "millions", "millionen", "millones", "milhões", "milhoes", "milioni", "milionima", "милионима"];
    const BILLIONS: &[&str] = &["billion", "billions", "milliarden", "milliards"];
    const BEFORE: &[&str] = &["in", "en", "em", "u", "у", "of", "de", "di"];
    let low = text.to_lowercase();
    // Tokens keep digits, letters and the marks that make "000s", "'000", "(000)", "mio." readable.
    let toks: Vec<&str> = low.split(|c: char| c.is_whitespace() || ",;:/[]".contains(c)).filter(|w| !w.is_empty()).collect();
    for (k, raw) in toks.iter().enumerate() {
        let w = raw.trim_matches(|c: char| "()'’.$€£".contains(c));
        let prev = k.checked_sub(1).map(|j| toks[j].trim_matches(|c: char| "()".contains(c))).unwrap_or("");
        let marked = BEFORE.contains(&prev) || raw.starts_with('(');
        // B140: "in 1 000 EUR", "(1 000 000)": thousands grouped with spaces.
        let next = |j: usize| toks.get(k + j).map(|n| n.trim_matches(|c: char| "()'’.$€£".contains(c)));
        if w == "1" && marked && next(1) == Some("000") {
            return Some(if next(2) == Some("000") { (1e6, "millions") } else { (1e3, "thousands") });
        }
        if matches!(w, "000s" | "000's" | "000’s" | "tsd" | "tsd." | "thous")
            || (w == "000" && (raw.starts_with(['\'', '’', '(', '$', '€', '£']) || marked))
            || ((w == "1.000" || w == "1,000") && marked)
            || (THOUSANDS.contains(&w) && marked)
        {
            return Some((1e3, "thousands"));
        }
        if matches!(w, "mio" | "mln" | "mn" | "mill") && (marked || raw.ends_with('.') || toks.get(k + 1).is_some_and(|n| n.contains(['€', '$', '£']) || n.len() == 3))
            || (MILLIONS.contains(&w) && marked)
        {
            return Some((1e6, "millions"));
        }
        if (matches!(w, "mrd" | "bn") && (marked || raw.ends_with('.'))) || (BILLIONS.contains(&w) && marked) {
            return Some((1e9, "billions"));
        }
    }
    None
}

/// The scale a table's title (or a note above it) gives its numbers: (factor, word, the text).
pub fn table_scale(t: &Table) -> Option<(f64, &'static str, &str)> {
    if !data_cols(t).any(|(_, c)| is_numeric(c.dtype) && c.dtype != Dtype::Percent) {
        return None;
    }
    let mut texts: Vec<&str> = t.title.iter().map(|(s, _)| s.as_str()).collect();
    texts.extend(t.notes.iter().filter(|(_, (r, _))| *r <= t.header_top).map(|(s, _)| s.as_str()));
    texts.into_iter().find_map(|s| scale_of(s).map(|(f, w)| (f, w, s)))
}

/// `units:` line of a table whose title (or a note above it) gives a scale for its numbers.
pub fn scale_line(t: &Table) -> Option<String> {
    let (f, word, text) = table_scale(t)?;
    Some(format!(
        "    units: amounts in {word} (× {}), says \"{}\" — multiply by {} for full figures; percentages and rates are as shown",
        thousands(f as usize),
        short(text, 60),
        thousands(f as usize)
    ))
}

// ------------------------------------------------------------------------------------------------
// notes that define columns (B83)

/// Word stems of a text for matching notes to headers: lowercase words without stop words, cut
/// to 6 characters; `16+` reads as `16 over`.
fn stems(text: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "a", "an", "of", "in", "to", "and", "or", "for", "by", "is", "are", "all", "on", "at", "as", "with", "from", "per",
        "headline", "der", "die", "das", "und", "le", "la", "les", "de", "des", "et", "o", "e", "do", "da", "y", "el",
    ];
    let mut s = String::with_capacity(text.len());
    let mut prev_digit = false;
    for ch in text.chars() {
        if ch == '+' && prev_digit {
            s.push_str(" over ");
        } else {
            s.push(ch);
        }
        prev_digit = ch.is_ascii_digit();
    }
    let mut out: Vec<String> = Vec::new();
    for w in s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !STOP.contains(w)) {
        let st: String = w.chars().take(6).collect();
        if !out.contains(&st) {
            out.push(st);
        }
    }
    out
}

/// A series code in a header cell (`LF24`, `MGSX`).
fn header_code(s: &str) -> bool {
    let s = s.trim();
    (3..=8).contains(&s.len()) && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && s.chars().any(|c| c.is_ascii_uppercase())
}

/// Notes of a table that define one of its columns: a sentence naming the column's series code
/// (`LF24`), or a definition (`The headline employment rate is …`) whose subject words are all in
/// one column's header and that matches that column best. (column index, the sentence, its cell).
pub fn column_notes(norm: &Normalized, t: &Table) -> Vec<(usize, String, (u32, u32))> {
    // The table's notes and the notes of its sheet that belong to no table (footnotes below it).
    let mut notes: Vec<(&str, (u32, u32))> = t.notes.iter().map(|(s, at)| (s.as_str(), *at)).collect();
    for n in norm.notes.iter().filter(|n| matches!(n.kind, "note" | "other")) {
        let mine = n.table == Some(t.n) || (n.table.is_none() && n.sheet == t.source_sheet);
        if mine && !notes.iter().any(|x| x.1 == (n.row, n.col)) {
            notes.push((n.text.as_str(), (n.row, n.col)));
        }
    }
    let cols: Vec<(usize, Vec<String>, Vec<String>)> = data_cols(t)
        .map(|(i, c)| {
            let codes: Vec<String> = c.original_header.iter().map(|(h, _)| h.trim().to_string()).filter(|h| header_code(h)).collect();
            (i, stems(&c.name), codes)
        })
        .collect();
    let mut out: Vec<(usize, String, (u32, u32))> = Vec::new();
    for (text, at) in notes {
        for sentence in text.split(". ").map(str::trim).filter(|s| s.chars().count() >= 20) {
            let words: Vec<&str> = sentence.split(|c: char| !c.is_alphanumeric()).collect();
            // The column whose code the sentence names first.
            let by_code = words.iter().find_map(|w| cols.iter().find(|(_, _, codes)| codes.iter().any(|c| c == w)));
            let hit = by_code.map(|c| c.0).or_else(|| {
                let low = sentence.to_lowercase();
                let cut = [" is ", " are ", " means ", " refers to ", " ist ", " sind ", " est ", " sont ", " é ", " es ", " je "]
                    .iter()
                    .filter_map(|v| low.find(v))
                    .min()?;
                let subject = stems(&low[..cut]);
                if subject.len() < 2 || subject.len() > 6 {
                    return None;
                }
                let all = stems(sentence);
                let scored: Vec<(usize, usize)> = cols
                    .iter()
                    .filter(|(_, st, _)| subject.iter().all(|w| st.contains(w)))
                    .map(|(i, st, _)| (*i, st.iter().filter(|w| all.contains(w)).count()))
                    .collect();
                let best = scored.iter().map(|x| x.1).max()?;
                let top: Vec<usize> = scored.iter().filter(|x| x.1 == best).map(|x| x.0).collect();
                (top.len() == 1).then(|| top[0])
            });
            if let Some(i) = hit
                && !out.iter().any(|x| x.0 == i)
            {
                // A long note naming the code: the part around it.
                let mut text = sentence.trim_end_matches('.').to_string();
                if let Some(code) = by_code.and_then(|c| c.2.iter().find(|k| words.contains(&k.as_str())))
                    && text.chars().count() > 180
                    && let Some(pos) = text.find(code.as_str())
                {
                    let start = text[..pos].char_indices().rev().nth(60).map_or(0, |x| x.0);
                    text = format!("…{}", &text[start..]);
                }
                out.push((i, text, at));
            }
        }
    }
    out
}

/// The `column notes:` line of a table (B83): what the file's own notes say a column is.
pub fn column_notes_line(norm: &Normalized, t: &Table, names: &[String]) -> Option<String> {
    let notes = column_notes(norm, t);
    if notes.is_empty() {
        return None;
    }
    let items: Vec<String> = notes
        .iter()
        .take(6)
        .map(|(i, s, (r, c))| format!("{} ← \"{}\" ({})", ident(&names[*i]), short(s, 180), hh_core::a1(*r, *c)))
        .collect();
    Some(format!("    column notes: {}", items.join(" · ")))
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

/// The label column and values that explain where a column is empty (most of the empty rows
/// share one or two labels, and most rows with those labels are empty).
fn empty_explained(t: &Table, names: &[String], target: &Column, rows: &[usize], empty: usize) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (i, c) in data_cols(t) {
        if std::ptr::eq(c, target) || label_values(c, rows).is_none_or(|v| v.len() < 2) {
            continue;
        }
        let mut by: HashMap<&str, (usize, usize)> = HashMap::new(); // label → (empty, rows)
        for &r in rows {
            if let Some(Out::Text(s)) = c.values.get(r) {
                let e = by.entry(s.as_str()).or_default();
                e.1 += 1;
                if matches!(target.values.get(r), Some(Out::Null) | None) {
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

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// B73: the first / last year of a date column that has fewer months than a full year while
/// another year has all 12: `Date: 2013 has Sep–Dec only (4 of 12 months)`.
fn partial_years(c: &Column, name: &str, rows: &[usize]) -> Option<String> {
    let mut by_year: std::collections::BTreeMap<i32, u16> = std::collections::BTreeMap::new();
    let mut last_day = f64::NAN;
    for &r in rows {
        let Some(Out::Num(x) | Out::Date(x)) = c.values.get(r) else { continue };
        let day = x.floor();
        if day == last_day {
            continue;
        }
        last_day = day;
        let s = hh_core::normalize::iso(day);
        let (Some(y), Some(m)) = (s.get(0..4).and_then(|y| y.parse::<i32>().ok()), s.get(5..7).and_then(|m| m.parse::<u32>().ok()))
        else {
            continue;
        };
        if (1..=12).contains(&m) {
            *by_year.entry(y).or_default() |= 1 << (m - 1);
        }
    }
    if by_year.len() < 2 || !by_year.values().any(|m| *m == 0xfff) {
        return None;
    }
    let part = |y: i32, mask: u16| -> Option<String> {
        let n = mask.count_ones();
        if n == 12 {
            return None;
        }
        let first = mask.trailing_zeros() as usize;
        let last = 15 - mask.leading_zeros() as usize;
        let span = match (first == last, last + 1 - first == n as usize) {
            (true, _) => format!("{} only", MONTHS[first]),
            (false, true) => format!("{}–{} only", MONTHS[first], MONTHS[last]),
            _ => "gaps".to_string(),
        };
        Some(format!("{y} has {span} ({n} of 12 months)"))
    };
    let (fy, fm) = by_year.iter().next().map(|(y, m)| (*y, *m))?;
    let (ly, lm) = by_year.iter().next_back().map(|(y, m)| (*y, *m))?;
    let parts: Vec<String> = [part(fy, fm), part(ly, lm)].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| format!("{}: {} — compare full years or the same months", ident(name), parts.join(", ")))
}

/// A column of changes or differences (`Veränderung absolut`, `coal_cons_change_twh`, `Delta`).
fn change_name(name: &str) -> bool {
    const CHANGE: &[&str] = &["change", "changes", "veränderung", "differenz", "diff", "difference", "delta", "growth", "variation", "promena"];
    words(name).iter().any(|w| CHANGE.contains(&w.as_str()))
}

/// A key of things to count by its name (`Order ID`, `customerID`, `Nr`), not a code such as a
/// postcode.
fn key_name(name: &str) -> bool {
    let ws = words(name);
    ws.iter().any(|w| matches!(w.as_str(), "id" | "ids" | "nr" | "no" | "number" | "nummer" | "broj") || (w.len() > 3 && w.ends_with("id")))
}

/// An ID column by its name, not a calendar column (`Order ID`, `empfaengerid`, `Nr`).
fn id_name(name: &str) -> bool {
    const CAL: &[&str] =
        &["ano", "year", "jahr", "month", "monat", "mes", "mês", "week", "woche", "day", "tag", "dia", "quarter", "quartal", "kalenderwoche"];
    id_like_name(name) && !words(name).iter().any(|w| CAL.contains(&w.as_str()))
}

/// The `quality:` line: duplicates in ID columns (a few in an otherwise unique ID; many in a
/// column that repeats one ID per line), empty cells of every column and the category that
/// explains them, first / last years with missing months, negative amounts. (Cells naming several
/// entities are on the entity line, `entity.rs`.)
pub fn quality_line(t: &Table, names: &[String], rows: &[usize], signs: bool) -> Option<String> {
    let n = rows.len();
    let mut items: Vec<String> = Vec::new();
    // B48: formula cells without a saved value are NULL here.
    let no_value = t.transforms.get(T::FormulasWithoutValue);
    if no_value > 0 {
        items.push(format!("{} without a saved value (NULL, not evaluated)", plural(no_value, "formula cell", "formula cells")));
    }
    if n < 20 {
        return (!items.is_empty()).then(|| format!("    quality: {}", items.join(" · ")));
    }
    // Duplicates in ID columns.
    let mut repeated = 0;
    for (i, c) in data_cols(t) {
        if !matches!(c.dtype, Dtype::Integer | Dtype::Text) || !id_name(&names[i]) {
            continue;
        }
        let mut seen: HashSet<(u64, &str)> = HashSet::with_capacity(n);
        let mut filled = 0;
        for &r in rows {
            let key = match c.values.get(r) {
                Some(Out::Num(x)) | Some(Out::Date(x)) => (x.to_bits(), ""),
                Some(Out::Text(s)) => (0, s.as_str()),
                Some(Out::Bool(b)) => (u64::from(*b), "bool"),
                Some(Out::Null) | None => continue,
            };
            filled += 1;
            seen.insert(key);
        }
        let dups = filled - seen.len();
        if filled < 20 || dups == 0 {
            continue;
        }
        if dups * 20 <= filled {
            // At most 5 % repeated: meant to be unique.
            items.push(format!("{} duplicate {} values", thousands(dups), ident(&names[i])));
        } else if repeated < 2 && seen.len() >= 3 && key_name(&names[i]) {
            // B73: one ID on several lines (order lines, returns): rows are not IDs.
            repeated += 1;
            items.push(format!(
                "{}: {} rows, {} distinct — count them with COUNT(DISTINCT {})",
                ident(&names[i]),
                thousands(filled),
                thousands(seen.len()),
                quote_ident(&names[i])
            ));
        }
    }
    // B73: empty cells of every column, with the category that explains them.
    let period_cols: Vec<String> =
        hh_core::normalize::table_time_coverage(t).filter(|c| c.source == "column").map(|c| c.columns).unwrap_or_default();
    let (mut explained, mut unexplained) = (0, 0);
    for (i, c) in data_cols(t) {
        if c.dtype == Dtype::Empty || c.role == ColRole::Group || explained >= 4 {
            continue;
        }
        let empty = rows.iter().filter(|&&r| matches!(c.values.get(r), Some(Out::Null) | None)).count();
        if empty == 0 || empty * 100 < n || empty == n {
            continue;
        }
        // B104: a label written once per block (one value) is no gap in the data.
        if c.dtype == Dtype::Text && label_values(c, rows).is_some_and(|v| v.len() <= 1) {
            continue;
        }
        let date = is_date(c.dtype);
        // Unexplained empties only for dates (counts by date drop them) and text columns (a NULL
        // group); the numbers line has the fill rate of amounts.
        let why = empty_explained(t, names, c, rows, empty);
        if why.is_none() && !date && !(c.dtype == Dtype::Text && unexplained < 2 && empty * 20 >= n) {
            continue;
        }
        let mut s = format!("{} empty in {} ({})", ident(&names[i]), plural(empty, "row", "rows"), pct(empty, n));
        match &why {
            Some(w) => {
                explained += 1;
                s.push_str(&format!(": {w}"));
            }
            None if !date => {
                unexplained += 1;
                s.push_str(" (a NULL group when grouping)");
            }
            None => {}
        }
        if date {
            s.push_str(" — counts by this date leave them out");
        }
        items.push(s);
    }
    // B73: incomplete first / last years of the table's (first) period column.
    if let Some((i, c)) = data_cols(t).find(|(_, c)| is_date(c.dtype) && (period_cols.is_empty() || period_cols.contains(&c.name))) {
        items.extend(partial_years(c, &names[i], rows));
    }
    // Negative amounts (a block of negative rows is a sign convention, said by `signs:`).
    if !signs {
        let neg: Vec<String> = amounts(t, names, rows)
            .iter()
            // B104: changes and differences are negative by nature; a negative amount changes a sum.
            .filter(|a| a.facts.neg > 0 && a.facts.neg * 2 < a.facts.filled - a.facts.zero && !change_name(a.name))
            .take(4)
            .map(|a| format!("{} negative {}", thousands(a.facts.neg), ident(a.name)))
            .collect();
        if !neg.is_empty() {
            items.push(neg.join(", "));
        }
    }
    // B104: columns empty in the same rows for the same reason once: `a, b empty in 158,866 rows …`.
    let mut merged: Vec<String> = Vec::new();
    for it in items {
        let Some(at) = it.find(" empty in ") else {
            merged.push(it);
            continue;
        };
        let (col, rest) = it.split_at(at);
        match merged.iter_mut().find(|m| m.find(" empty in ").is_some_and(|k| &m[k..] == rest)) {
            Some(m) => {
                let k = m.find(" empty in ").unwrap_or(m.len());
                m.insert_str(k, &format!(", {col}"));
            }
            None => merged.push(it),
        }
    }
    (!merged.is_empty()).then(|| format!("    quality: {}", merged.join(" · ")))
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
        assert!(c.contains("total rows: SELECT … FROM T1_all WHERE _row_type = 'total' — labels in Richtsatz: total 'Gesamt'@6"), "{c}");
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
        assert!(c.contains("\n    total rows: SELECT … FROM T1_all WHERE _row_type = 'total' — labels in col_C: totals 'Total'@5/9\n"), "{c}");
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
            c.contains("\n    precoContratual (sum 811,970) vs PrecoTotalEfetivo (sum 120,000) — different measures; pick deliberately\n"),
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
    fn units_from_titles() {
        // B85: a scale in the title becomes a units line.
        for (s, f) in [
            ("(All values in 000s)", 1e3),
            ("Umsatz in Tausend Euro", 1e3),
            ("Revenue ($'000)", 1e3),
            ("Ausgaben in Mio. €", 1e6),
            ("Population (in millions)", 1e6),
            ("Dépenses en milliers d'euros", 1e3),
            ("Budget in 1.000 EUR", 1e3),
            // B130: thousands grouped with a space.
            ("Municipal budget 2024, in 1 000 EUR", 1e3),
            ("Ausgaben (in 1 000 000 €)", 1e6),
        ] {
            assert_eq!(scale_of(s).map(|x| x.0), Some(f), "{s}");
        }
        for s in ["Sales 2024", "Top 1000 companies", "5 million customers served", "Plant 000 status", "Top 1 000 firms"] {
            assert_eq!(scale_of(s), None, "{s}");
        }
        let mut n = register();
        n.tables[0].title = Some(("Contracts (All values in 000s)".into(), (0, 0)));
        let c = catalog(&n, "x.xlsx", MAX_CHARS);
        assert!(c.contains("\n    units: amounts in thousands (× 1,000), says \"Contracts (All values in 000s)\" — multiply by 1,000"), "{c}");
    }

    #[test]
    fn notes_that_define_columns() {
        // B83, like the ONS labour market summary: the footnotes say which rate is the headline.
        let mut cols = Vec::new();
        for (age, codes) in [("All aged 16 & over", ["MGSR", "MGSX", "MGRZ"]), ("All aged 16 to 64", ["LF24", "LF2Q", "LF2G"])] {
            for (what, code) in ["Employment / rate (%)", "Unemployment / rate (%)", "Total in employment / level"].iter().zip(codes) {
                let mut c = col(&format!("{age} / {what} / {code}"), Dtype::Number, nums(&[60.5; 4]));
                c.original_header = vec![(age.into(), (5, 1)), ((*what).into(), (6, 1)), (code.into(), (8, 1))];
                cols.push(c);
            }
        }
        let mut n = book(cols, vec![RowType::Data; 4]);
        n.tables[0].notes = vec![
            ("The headline employment rate is the number of people aged 16 to 64 in employment divided by the population aged 16 to 64.".into(), (680, 0)),
            ("Source: Labour Force Survey".into(), (2, 18)),
        ];
        n.notes.push(hh_core::normalize::NoteItem {
            sheet: "Sheet 1".into(),
            row: 681,
            col: 0,
            kind: "note",
            table: None,
            text: "The headline unemployment rate is the number of unemployed people (aged 16+) divided by the economically active population (aged 16+).".into(),
            formula: None,
        });
        n.notes.push(hh_core::normalize::NoteItem {
            sheet: "Sheet 1".into(),
            row: 690,
            col: 0,
            kind: "note",
            table: None,
            text: "Series MGRZ is seasonally adjusted, like all others here".into(),
            formula: None,
        });
        let got: Vec<(String, u32)> = column_notes(&n, &n.tables[0]).iter().map(|(i, _, at)| (n.tables[0].columns[*i].name.clone(), at.0)).collect();
        assert_eq!(
            got,
            [
                ("All aged 16 to 64 / Employment / rate (%) / LF24".to_string(), 680),
                ("All aged 16 & over / Unemployment / rate (%) / MGSX".to_string(), 681),
                ("All aged 16 & over / Total in employment / level / MGRZ".to_string(), 690),
            ]
        );
        let c = catalog(&n, "x.xlsx", MAX_CHARS);
        assert!(c.contains("\n    column notes: \"All aged 16 to 64 / Employment / rate (%) / LF24\" ← \"The headline employment rate is"), "{c}");
    }

    #[test]
    fn coverage_facts() {
        // B73: empties of any column tied to a category, repeated IDs, a partial first year.
        let rows = 40;
        let kind: Vec<&str> = (0..rows).map(|i| if i % 4 == 0 { "HOURLY" } else { "SALARY" }).collect();
        let salary: Vec<Out> = (0..rows).map(|i| if i % 4 == 0 { Out::Null } else { Out::Num(50_000.0 + i as f64) }).collect();
        let employer: Vec<&str> = (0..rows).map(|i| if i % 3 == 0 { "" } else { ["Acme", "Beta", "Gamma"][i % 3] }).collect();
        let order: Vec<String> = (0..rows).map(|i| format!("O-{}", i / 3)).collect();
        // 2013-09-01 + 30 days per row: Sep 2013 .. Nov 2016, 2014 and 2015 full.
        let date: Vec<Out> = (0..rows).map(|i| Out::Date(41518.0 + 30.0 * i as f64)).collect();
        let cols = vec![
            col("Order ID", Dtype::Text, order.iter().map(|s| Out::Text(s.clone())).collect()),
            col("salary_or_hourly", Dtype::Text, text(&kind)),
            col("annual_salary", Dtype::Number, salary),
            col("employer", Dtype::Text, text(&employer)),
            col("Date", Dtype::Date, date),
        ];
        let c = catalog(&book(cols, vec![RowType::Data; rows]), "x.xlsx", MAX_CHARS);
        let q = c.lines().find(|l| l.starts_with("    quality: ")).unwrap_or_else(|| panic!("{c}"));
        assert!(q.contains("annual_salary empty in 10 rows (25 %): salary_or_hourly = 'HOURLY' (all its 10 rows)"), "{q}");
        assert!(q.contains("employer empty in 14 rows (35 %) (a NULL group when grouping)"), "{q}");
        assert!(q.contains("\"Order ID\": 40 rows, 14 distinct — count them with COUNT(DISTINCT \"Order ID\")"), "{q}");
        assert!(q.contains("Date: 2013 has Sep–Dec only (4 of 12 months), 2016 has Jan–Nov only (11 of 12 months)"), "{q}");
    }

    #[test]
    fn share_only_for_additive_amounts() {
        // B89: no share of a price, none over rows of mixed units, none ranked by a name column.
        let rows = 40;
        let region: Vec<String> = (0..rows).map(|i| format!("Region {}", i % 12)).collect();
        let cols = |price: &str, unit: bool| {
            let mut v = vec![
                col("region", Dtype::Text, region.iter().map(|s| Out::Text(s.clone())).collect()),
                // A unit price takes a few values (one per product); sales are each their own.
                col(price, Dtype::Number, nums(&(0..rows).map(|i| 10.5 * (if price.contains("Price") { i % 5 } else { i }) as f64 + 100.5).collect::<Vec<_>>())),
            ];
            if unit {
                v.push(col("currency", Dtype::Text, text(&(0..rows).map(|i| if i % 2 == 0 { "EUR" } else { "PPS" }).collect::<Vec<_>>())));
            }
            v
        };
        let share = |c: Vec<Column>| catalog(&book(c, vec![RowType::Data; rows]), "x.xlsx", MAX_CHARS).contains("share of the total");
        assert!(share(cols("Sales", false)));
        assert!(!share(cols("Manufacturing Price", false)));
        assert!(share(cols("Contract price", false)));
        assert!(!share(cols("Sales", true)));
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
