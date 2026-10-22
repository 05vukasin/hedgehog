//! B160: total rows split by a code column. Reports such as a social report list every group in
//! three rows, `M`, `F` and `T`, where the code `T` (or `Total`, `Todos`, `Insgesamt`, `Ukupno`,
//! `Σ` …) in the `Sexo` column is the sum of the other codes of its group. Summing every total row
//! (`… WHERE _row_type = 'total'`) then counts each person twice.
//!
//! A code is found by its numbers, never by its name: in every group (the rows that share the
//! other key columns) that has the code and at least two other codes, the code's row must be the
//! sum of the others' rows. The catalog then says so in one phrase (`grand total by Sexo: M 4,233
//! · F 3,245 · T 7,478 (T = M + F — don't add)`), and total rows are labelled with their full key
//! (`'Total/M'`), not with whichever column happened to hold a word.

use std::collections::HashMap;

use hh_core::normalize::{ColRole, Column, Dtype, Out, RowType, Table};

use crate::catalog::{is_numeric, TotalEntry};
use crate::text::{ident, num, short, thousands};

/// Most key (label) columns a total row's key is made of.
const MAX_KEYS: usize = 4;
/// Most distinct values of a code column (`M`, `F`, `T`: a handful).
const MAX_CODES: usize = 12;
/// Most characters of a code and of the codes it adds up.
const MAX_CODE_LEN: usize = 20;
/// Tables with more rows are not checked (the check reads every row once per candidate).
const MAX_ROWS: usize = 20_000;

/// A total code: in column `col`, the rows with `code` are the sum of the rows with `parts`, in
/// every group of the `by` columns that has them.
#[derive(Debug, Clone, PartialEq)]
pub struct TotalCode {
    pub col: usize,
    pub code: String,
    /// The other codes, in table order (`M`, `F`).
    pub parts: Vec<String>,
    /// The other key columns: the groups.
    pub by: Vec<usize>,
    /// Groups where the sum was checked and held.
    pub groups: usize,
}

/// The key (label) columns of a table: its text columns that hold labels or groups, at most
/// [`MAX_KEYS`], in table order.
pub fn key_columns(t: &Table) -> Vec<usize> {
    t.columns
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c.role, ColRole::Data | ColRole::Group) && c.dtype == Dtype::Text && !c.is_derived())
        .map(|(i, _)| i)
        .take(MAX_KEYS)
        .collect()
}

/// A column of codes that split every group (`Sexo`: M, F, T): a data column (not a filled-down
/// group label) of at most [`MAX_CODES`] values, 3 or more of them on 2 rows or more, which fill
/// 80 % of the rows or more (a stray title row in the column aside).
pub fn code_dimension(t: &Table, d: usize) -> bool {
    let c = &t.columns[d];
    if c.role != ColRole::Data || c.dtype != Dtype::Text {
        return false;
    }
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for r in 0..t.rows() {
        let Some(s) = text_at(c, r) else { continue };
        if let Some(x) = counts.iter_mut().find(|x| x.0 == s) {
            x.1 += 1;
        } else if counts.len() == MAX_CODES {
            return false;
        } else {
            counts.push((s, 1));
        }
    }
    let repeated: Vec<usize> = counts.iter().filter(|x| x.1 >= 2).map(|x| x.1).collect();
    repeated.len() >= 3 && repeated.iter().sum::<usize>() * 5 >= t.rows() * 4
}

fn text_at(c: &Column, r: usize) -> Option<&str> {
    match c.values.get(r) {
        Some(Out::Text(s)) if !s.trim().is_empty() => Some(s.trim()),
        _ => None,
    }
}

/// A cell as a number for a sum: empty is 0, text is not a number.
fn num_at(c: &Column, r: usize) -> Option<f64> {
    match c.values.get(r) {
        None | Some(Out::Null) => Some(0.0),
        Some(Out::Num(x)) => Some(*x),
        _ => None,
    }
}

/// Decimals a value is written with (up to 6): the unit of its last digit.
fn decimals(v: f64) -> i32 {
    let s = num(v);
    s.split_once('.').map_or(0, |(_, f)| f.len().min(6) as i32)
}

/// `a` is `parts` added up, exactly or within the rounding of the figures.
fn adds_up(a: f64, parts: &[f64]) -> bool {
    let sum: f64 = parts.iter().sum();
    let diff = (a - sum).abs();
    if diff <= 1e-9 * a.abs().max(1.0) {
        return true;
    }
    // Published figures are rounded: within half a unit of the last digit per part, when that is
    // small next to the value.
    let d = std::iter::once(a).chain(parts.iter().copied()).map(decimals).max().unwrap_or(0);
    let unit = 10f64.powi(-d);
    a.abs() >= 50.0 * unit && diff <= 0.51 * unit * parts.len() as f64
}

/// Checks `code` of column `col` over `groups`: in every group with the code once and at least two
/// other codes (each once), the code's row is the sum of the others' rows. (groups checked, the
/// other codes, the code's rows) when it holds; None when it does not, or a group has the code
/// twice.
fn check_code(col: &Column, nums: &[&Column], groups: &[Vec<usize>], code: &str) -> Option<(usize, Vec<String>, Vec<usize>)> {
    let (mut checked, mut nonzero) = (0usize, 0usize);
    let mut parts: Vec<&str> = Vec::new();
    let mut rows: Vec<usize> = Vec::new();
    // Per numeric column: comparisons that held / failed (a rate column is no sum).
    let mut held = vec![0usize; nums.len()];
    let mut broke = vec![0usize; nums.len()];
    for g in groups {
        let at_code: Vec<usize> = g.iter().copied().filter(|&r| text_at(col, r) == Some(code)).collect();
        if at_code.len() > 1 {
            return None;
        }
        rows.extend(&at_code);
        let others: Vec<usize> = g.iter().copied().filter(|&r| text_at(col, r) != Some(code)).collect();
        let mut seen: Vec<&str> = Vec::new();
        for &r in &others {
            if let Some(s) = text_at(col, r)
                && !seen.contains(&s)
            {
                seen.push(s);
            }
        }
        if at_code.len() != 1 || others.len() < 2 || seen.len() != others.len() {
            continue;
        }
        checked += 1;
        let mut any = false;
        for (k, c) in nums.iter().enumerate() {
            let (Some(a), Some(ps)) = (num_at(c, at_code[0]), others.iter().map(|&r| num_at(c, r)).collect::<Option<Vec<f64>>>()) else {
                continue;
            };
            if adds_up(a, &ps) {
                if a != 0.0 {
                    held[k] += 1;
                    any = true;
                }
            } else {
                broke[k] += 1;
            }
        }
        nonzero += usize::from(any);
        for x in seen {
            if !parts.contains(&x) {
                parts.push(x);
            }
        }
    }
    // Columns where the code is the sum in every group, and those where it is not (rates,
    // averages): the sums must hold in most columns, in two groups with numbers or more.
    let good = (0..nums.len()).filter(|&k| held[k] > 0 && broke[k] == 0).count();
    let bad = (0..nums.len()).filter(|&k| broke[k] > 0).count();
    if checked < 2 || nonzero < 2 || good == 0 || bad > good {
        return None;
    }
    Some((checked, parts.iter().map(|s| s.to_string()).collect(), rows))
}

/// The total codes of a table, checked on its numbers (data and total rows alike). The code of a
/// column with the most groups checked first.
pub fn total_codes(t: &Table) -> Vec<TotalCode> {
    if t.rows() > MAX_ROWS || t.rows() < 3 {
        return Vec::new();
    }
    let keys = key_columns(t);
    let nums: Vec<&Column> = t.columns.iter().filter(|c| c.role == ColRole::Data && is_numeric(c.dtype)).collect();
    if nums.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<TotalCode> = Vec::new();
    for &d in &keys {
        if !code_dimension(t, d) {
            continue;
        }
        let col = &t.columns[d];
        let mut values: Vec<&str> = Vec::new();
        for r in 0..t.rows() {
            if let Some(s) = text_at(col, r)
                && !values.contains(&s)
            {
                if values.len() == MAX_CODES {
                    values.clear();
                    break;
                }
                values.push(s);
            }
        }
        if values.len() < 3 {
            continue;
        }
        let by: Vec<usize> = keys.iter().copied().filter(|&k| k != d).collect();
        let key_of = |r: usize| -> Vec<Option<&str>> { by.iter().map(|&k| text_at(&t.columns[k], r)).collect() };
        // The rows of each group (the other key columns' values), in table order; and the same
        // with a group per run of consecutive rows, for stacked blocks that repeat their labels.
        let (mut groups, mut runs): (Vec<Vec<usize>>, Vec<Vec<usize>>) = (Vec::new(), Vec::new());
        let mut at: HashMap<Vec<Option<&str>>, usize> = HashMap::new();
        let mut last: Option<Vec<Option<&str>>> = None;
        for r in 0..t.rows() {
            if text_at(col, r).is_none() {
                continue;
            }
            let key = key_of(r);
            // A new run where the key changes or a code comes again.
            let again = runs.last().is_some_and(|run| run.iter().any(|&x| text_at(col, x) == text_at(col, r)));
            if last.as_ref() != Some(&key) || again {
                runs.push(Vec::new());
            }
            runs.last_mut().expect("a run").push(r);
            last = Some(key.clone());
            let g = *at.entry(key).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[g].push(r);
        }
        let mut best: Option<TotalCode> = None;
        // Codes are short (`T`, `Total`, `Insgesamt`, `Todos os sexos`); a long label that adds up
        // its group (`Goodwill, Ending Balance`) is a total row of a statement, not a code.
        for &code in values.iter().filter(|v| v.chars().count() <= MAX_CODE_LEN) {
            let Some((checked, parts, rows)) = check_code(col, &nums, &groups, code).or_else(|| check_code(col, &nums, &runs, code)) else {
                continue;
            };
            if parts.iter().any(|p| p.chars().count() > MAX_CODE_LEN) {
                continue;
            }
            // A code on one row only is a plain total row (the catalog says so already), unless
            // it stays in the data.
            let in_data = rows.iter().any(|&r| t.row_types.get(r).is_none_or(|x| *x == RowType::Data));
            if rows.len() < 2 && !in_data {
                continue;
            }
            if best.as_ref().is_none_or(|b| checked > b.groups) {
                best = Some(TotalCode { col: d, code: code.to_string(), parts, by: by.clone(), groups: checked });
            }
        }
        out.extend(best);
    }
    // The code with the fewest parts first (`Sexo 'T' = M + F` before "the rest").
    // On a tie the column nearer the numbers (the inner split, `Sexo` after `Relação`).
    out.sort_by_key(|c| (c.parts.len(), std::cmp::Reverse(c.groups), std::cmp::Reverse(c.col)));
    out
}

/// The full key of row `r`: every key column's value, joined with `/` (`Total/M`).
pub fn row_key(t: &Table, keys: &[usize], r: usize) -> Option<String> {
    let parts: Vec<&str> = keys.iter().filter_map(|&k| text_at(&t.columns[k], r)).collect();
    (parts.len() >= 2).then(|| parts.join("/"))
}

/// `M + F`, `the 5 others` when there are many.
fn parts_text(code: &TotalCode) -> String {
    if code.parts.len() <= 4 {
        code.parts.join(" + ")
    } else {
        format!("the {} others", code.parts.len())
    }
}

/// A column's name in a phrase: as in SQL, a long one (a flattened header of several levels) by
/// its last level (`"… / Sexo"`).
fn col_name(names: &[String], i: usize) -> String {
    let n = &names[i];
    match n.rsplit_once(" / ") {
        Some((_, last)) if n.chars().count() > 40 => format!("\"… / {last}\""),
        _ => ident(n),
    }
}

/// The short form for a table listed in one line: `Sexo 'T' = M + F: don't add`.
pub fn brief_note(code: &TotalCode, names: &[String]) -> String {
    format!("{} '{}' = {}: don't add", col_name(names, code.col), short(&code.code, 20), parts_text(code))
}

/// A value for the catalog: integers with thousands separators, else as stored.
fn value_text(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        let s = thousands(v.abs() as usize);
        if v < 0.0 { format!("-{s}") } else { s }
    } else {
        num(v)
    }
}

/// The column whose values a "grand total by" phrase shows: the one named total (`TOTAL`,
/// `(EM DIAS) / TOTAL`), else the only numeric one.
fn value_column(t: &Table) -> Option<usize> {
    let nums: Vec<usize> = (0..t.columns.len()).filter(|&i| t.columns[i].role == ColRole::Data && is_numeric(t.columns[i].dtype)).collect();
    let named = nums.iter().copied().find(|&i| {
        let n = t.columns[i].name.rsplit(" / ").next().unwrap_or("").trim().to_lowercase();
        matches!(n.as_str(), "total" | "totals" | "totais" | "gesamt" | "insgesamt" | "summe" | "ukupno" | "svega" | "totale" | "totaal" | "ensemble" | "sum")
    });
    named.or_else(|| (nums.len() == 1).then(|| nums[0]))
}

/// B160: how a table's total rows read when a total code splits them: the grand total by the code
/// column with its values, the code's subtotals per group, and what that leaves (`rest`, the other
/// total rows, for the caller to list as usual). None without a code.
/// `grand total 'Total' by Sexo @8–10: M 4,233 · F 3,245 · T 7,478 in TOTAL (T = M + F: don't
/// add); subtotals (_row_type 'subtotal') Sexo 'T' @13/16/19/22/25`.
pub(crate) fn coded_totals(t: &Table, names: &[String], code: &TotalCode, entries: &[TotalEntry]) -> Option<(String, Vec<u32>)> {
    let col = &t.columns[code.col];
    let row_of = |src: u32| t.src_rows.iter().position(|s| s + 1 == src);
    let d = col_name(names, code.col);
    let mut covered: Vec<u32> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    // The grand total: the group whose rows are all total rows (not subtotals) and hold the code
    // and its parts.
    let group_of = |r: usize| -> Vec<Option<&str>> { code.by.iter().map(|&k| text_at(&t.columns[k], r)).collect() };
    let totals: Vec<(&TotalEntry, usize)> =
        entries.iter().filter(|e| e.kind == RowType::Total).filter_map(|e| row_of(e.row).map(|r| (e, r))).collect();
    let mut grand: Option<Vec<(&TotalEntry, usize)>> = None;
    for (_, r) in &totals {
        let g = group_of(*r);
        // The first block of the group: each code once (a stacked table below repeats them).
        let mut members: Vec<(&TotalEntry, usize)> = Vec::new();
        for &(e, x) in totals.iter().filter(|(_, x)| group_of(*x) == g) {
            if members.iter().any(|(_, y)| text_at(col, *y) == text_at(col, x)) {
                break;
            }
            members.push((e, x));
        }
        let has = |v: &str| members.iter().any(|(_, x)| text_at(col, *x) == Some(v));
        if has(&code.code) && code.parts.iter().all(|p| has(p)) {
            grand = Some(members);
            break;
        }
    }
    let codes_line = format!("{} = {}: don't add", short(&code.code, 20), parts_text(code));
    if let Some(members) = &grand {
        let vc = value_column(t);
        let mut order: Vec<&str> = code.parts.iter().map(String::as_str).collect();
        order.push(&code.code);
        let rows: Vec<u32> = members.iter().map(|(e, _)| e.row).collect();
        let (lo, hi) = (rows.iter().min().copied().unwrap_or(0), rows.iter().max().copied().unwrap_or(0));
        let at = if hi == lo + rows.len() as u32 - 1 && rows.len() > 1 { format!("@{lo}–{hi}") } else {
            format!("@{}", rows.iter().map(|r| r.to_string()).collect::<Vec<_>>().join("/"))
        };
        // The group's own label ('Total' in "Relação Jurídica").
        let label: Vec<String> = group_of(members[0].1).into_iter().flatten().map(|v| short(v, 30)).collect();
        let label = if label.is_empty() { String::new() } else { format!(" '{}'", label.join("/")) };
        let values: Vec<String> = order
            .iter()
            .filter_map(|v| {
                let (_, r) = members.iter().find(|(_, x)| text_at(col, *x) == Some(*v))?;
                let shown = vc.and_then(|i| match t.columns[i].values.get(*r) {
                    Some(Out::Num(x)) => Some(value_text(*x)),
                    Some(Out::Null) | None => Some("0".to_string()),
                    _ => None,
                });
                Some(match shown {
                    Some(x) => format!("{} {x}", short(v, 20)),
                    None => format!("'{}'", short(v, 20)),
                })
            })
            .collect();
        let inn = vc.filter(|_| values.iter().all(|v| !v.starts_with('\''))).map(|i| format!(" in {}", col_name(names, i))).unwrap_or_default();
        out.push(format!("grand total{label} by {d} {at}: {}{inn} ({codes_line})", values.join(" · ")));
        covered.extend(rows);
    }
    // The code's rows in the other groups: one subtotal per group.
    let subs: Vec<&TotalEntry> = entries
        .iter()
        .filter(|e| !covered.contains(&e.row) && row_of(e.row).is_some_and(|r| text_at(col, r) == Some(code.code.as_str())))
        .collect();
    if !subs.is_empty() {
        let kinds: Vec<&str> = {
            let mut k: Vec<&str> = subs.iter().map(|e| e.kind.as_str()).collect();
            k.dedup();
            k
        };
        let kind = if kinds.len() == 1 && kinds[0] == "subtotal" { "subtotals (_row_type 'subtotal')".to_string() } else { "total rows".to_string() };
        let rows: Vec<String> = subs.iter().map(|e| e.row.to_string()).collect();
        let tail = if grand.is_none() { format!(" ({codes_line})") } else { String::new() };
        out.push(format!("{kind} {d} '{}' @{}{tail}", short(&code.code, 20), rows.join("/")));
        covered.extend(subs.iter().map(|e| e.row));
    }
    // Code rows that stay in the data view would be added in.
    let kept: usize = (0..t.rows()).filter(|&r| t.row_types.get(r).is_none_or(|x| *x == RowType::Data) && text_at(col, r) == Some(code.code.as_str())).count();
    if kept > 0 {
        out.push(format!("{kept} '{}' rows stay in {}: filter {d} <> '{}' before summing", short(&code.code, 20), t.id(), code.code.replace('\'', "''")));
    }
    (!out.is_empty()).then(|| (out.join("; "), covered))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table from rows of (key, key, number, number); `kinds` the row types.
    type Row<'a> = (&'a str, &'a str, Option<f64>, Option<f64>);

    fn table(rows: &[Row], kinds: &[RowType]) -> Table {
        let text = |f: for<'a> fn(&'a Row<'a>) -> &'a str| -> Vec<Out> {
            rows.iter().map(|r| if f(r).is_empty() { Out::Null } else { Out::Text(f(r).to_string()) }).collect()
        };
        let nums = |f: fn(&Row) -> Option<f64>| -> Vec<Out> { rows.iter().map(|r| f(r).map_or(Out::Null, Out::Num)).collect() };
        let col = |name: &str, dtype: Dtype, values: Vec<Out>, k: u32| Column {
            name: name.into(),
            role: ColRole::Data,
            source_col: Some(k),
            original_header: vec![(name.to_string(), (0, k))],
            dtype,
            unit: None,
            currency: None,
            decimal: None,
            date_order: None,
            ambiguous: 0,
            footnote_marks: Vec::new(),
            values,
        };
        Table {
            n: 1,
            sheet: "T1_s".into(),
            source_sheet: "S".into(),
            rect: hh_core::Rect { top: 5, left: 0, bottom: 6 + rows.len() as u32, right: 3 },
            header_top: 5,
            header_rows: 1,
            title: None,
            notes: vec![],
            confidence: 0.9,
            columns: vec![
                col("Relação", Dtype::Text, text(|r| r.0), 0),
                col("Sexo", Dtype::Text, text(|r| r.1), 1),
                col("Técnico", Dtype::Integer, nums(|r| r.2), 2),
                col("TOTAL", Dtype::Integer, nums(|r| r.3), 3),
            ],
            // 1-based source rows 7, 8, …
            src_rows: (0..rows.len() as u32).map(|r| r + 6).collect(),
            row_types: kinds.to_vec(),
            transforms: hh_core::normalize::Transforms::default(),
            warnings: vec![],
        }
    }

    use RowType::{Data as D, Subtotal as S, Total as T};

    fn social() -> Table {
        table(
            &[
                ("Total", "M", Some(716.0), Some(4233.0)),
                ("Total", "F", Some(1432.0), Some(3245.0)),
                ("Total", "T", Some(2148.0), Some(7478.0)),
                ("Comissão", "M", None, Some(70.0)),
                ("Comissão", "F", None, Some(99.0)),
                ("Comissão", "T", Some(0.0), Some(169.0)),
                ("CTFP", "M", Some(716.0), Some(4163.0)),
                ("CTFP", "F", Some(1432.0), Some(3146.0)),
                ("CTFP", "T", Some(2148.0), Some(7309.0)),
            ],
            &[T, T, T, D, D, S, D, D, S],
        )
    }

    #[test]
    fn a_code_that_adds_up_is_a_total_code() {
        let t = social();
        let codes = total_codes(&t);
        assert_eq!(codes[0].col, 1, "{codes:?}");
        assert_eq!(codes[0].code, "T");
        assert_eq!(codes[0].parts, ["M", "F"]);
        assert_eq!(codes[0].by, [0]);
        assert_eq!(codes[0].groups, 3);
        // "Relação" 'Total' is the sum of the other groups too (per Sexo).
        assert!(codes.iter().any(|c| c.col == 0 && c.code == "Total"), "{codes:?}");
        let names: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
        let entries: Vec<TotalEntry> = [(7, T), (8, T), (9, T), (12, S), (15, S)]
            .iter()
            .map(|&(row, kind)| TotalEntry { label: String::new(), row, kind, kept: false, scope: Vec::new(), grand: false })
            .collect();
        let (text, covered) = coded_totals(&t, &names, &codes[0], &entries).unwrap();
        assert_eq!(
            text,
            "grand total 'Total' by Sexo @7–9: M 4,233 · F 3,245 · T 7,478 in TOTAL (T = M + F: don't add); \
             subtotals (_row_type 'subtotal') Sexo 'T' @12/15"
        );
        assert_eq!(covered, [7, 8, 9, 12, 15]);
        assert_eq!(brief_note(&codes[0], &names), "Sexo 'T' = M + F: don't add");
        assert_eq!(row_key(&t, &key_columns(&t), 1).as_deref(), Some("Total/F"));
    }

    #[test]
    fn a_code_column_that_is_not_a_sum_is_no_total() {
        // 'T' (say, temporary staff) is a category of its own: its numbers are not M + F.
        let t = table(
            &[
                ("A", "M", Some(10.0), Some(10.0)),
                ("A", "F", Some(5.0), Some(5.0)),
                ("A", "T", Some(3.0), Some(3.0)),
                ("B", "M", Some(7.0), Some(7.0)),
                ("B", "F", Some(2.0), Some(2.0)),
                ("B", "T", Some(9.0), Some(9.0)),
            ],
            &[D; 6],
        );
        assert!(total_codes(&t).iter().all(|c| c.col != 1), "{:?}", total_codes(&t));
        // One group that happens to add up is not enough either.
        let t = table(
            &[("A", "M", None, Some(1.0)), ("A", "F", None, Some(2.0)), ("A", "T", None, Some(3.0)), ("B", "M", None, Some(4.0))],
            &[D; 4],
        );
        assert!(total_codes(&t).is_empty(), "{:?}", total_codes(&t));
    }

    #[test]
    fn stacked_blocks_that_repeat_their_labels() {
        // Two blocks with the same group labels (a table and the one under it): checked per run.
        let t = table(
            &[
                ("Total", "M", Some(3.0), Some(30.0)),
                ("Total", "F", Some(1.0), Some(10.0)),
                ("Total", "T", Some(4.0), Some(40.0)),
                ("Total", "M", Some(5.0), Some(50.0)),
                ("Total", "F", Some(2.0), Some(20.0)),
                ("Total", "T", Some(7.0), Some(70.0)),
            ],
            &[T; 6],
        );
        let codes = total_codes(&t);
        assert_eq!(codes.len(), 1, "{codes:?}");
        assert_eq!((codes[0].col, codes[0].code.as_str(), codes[0].groups), (1, "T", 2));
        // A dimension that is not a code column (a value on one row only) is not checked.
        let t = table(&[("A", "M", None, Some(1.0)), ("A", "F", None, Some(2.0)), ("A", "T", None, Some(3.0))], &[D, D, T]);
        assert!(total_codes(&t).is_empty());
    }

    #[test]
    fn code_rows_in_the_data_are_named() {
        // The 'T' rows were not taken out: the catalog says to filter them.
        let mut t = social();
        t.row_types = vec![T, T, T, D, D, D, D, D, D];
        let codes = total_codes(&t);
        let names: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
        let entries: Vec<TotalEntry> = [7, 8, 9]
            .iter()
            .map(|&row| TotalEntry { label: String::new(), row, kind: T, kept: false, scope: Vec::new(), grand: false })
            .collect();
        let (text, _) = coded_totals(&t, &names, &codes[0], &entries).unwrap();
        assert!(text.ends_with("2 'T' rows stay in T1: filter Sexo <> 'T' before summing"), "{text}");
    }
}
