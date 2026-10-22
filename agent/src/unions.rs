//! Union views of tables with the same layout (B79).
//!
//! A workbook often repeats one table per sheet: one per state (`'Ankünfte … in Tirol …'`), per
//! day (`May1` .. `May14`), per month. Comparing them took one query per table, and a model that
//! did not see that the state is only in the title compared nothing. Tables whose columns have
//! the same types in the same order, and mostly the same names, form a family; the family gets
//! two views over the members' `T<n>_all`:
//!
//! - `U<k>_all`: every row of every member, with `_table` (`T8`), `sheet` (the source sheet),
//!   `title`, `part` (the part of the title that differs between members: `Tirol`, when the
//!   titles differ) and `variant` (the header words that differ between members, when the
//!   column names do: `Day-Ahead` | `Real-Time Transactions`), then the columns, `_src_row` and
//!   `_row_type`;
//! - `U<k>`: the same with the data rows only (no `_row_type`), like `T<n>`.
//!
//! The union's column names are the members' names with dates and periods dropped
//! (`2001-05-09 / Day-Ahead / Qty` → `Day-Ahead / Qty`); where members still name a column
//! differently, the common trailing part (`Qty`), or else the first member's name. The views
//! are plain `UNION ALL` views, so nothing is copied at load.

use hh_core::normalize::{ColRole, Dtype, Normalized, Table};
use rusqlite::Transaction;

use crate::catalog::{data_cols, sql_names};
use crate::text::{ident, quote_ident, short};

/// Fewest columns of a family member (smaller tables are compared by their names only).
const MIN_SHAPE_COLS: usize = 3;
/// Share of columns whose names must agree (after dropping dates) for tables of one shape.
const SAME_NAMES: f64 = 0.5;
/// Most members of one view (SQLite allows 500 terms in one compound SELECT).
const MAX_MEMBERS: usize = 480;
/// Wider tables are loaded in part; they get no union view.
const MAX_TABLE_COLUMNS: usize = 1990;

/// One family of same-layout tables and its union view.
#[derive(Debug, Clone)]
pub struct Family {
    /// `U1`, `U2`, …
    pub name: String,
    /// Indexes into `Normalized::tables`.
    pub members: Vec<usize>,
    /// The union's names of the data columns.
    pub columns: Vec<String>,
    /// Per member, its data columns (indexes into `Table::columns`) in union order.
    pub member_cols: Vec<Vec<usize>>,
    /// Per member, the differing part of its title (when the titles differ).
    pub parts: Option<Vec<String>>,
    /// Per member, the header words that differ (when the column names differ).
    pub variants: Option<Vec<String>>,
    /// Names of the added columns: `_table`, `sheet`, `title`, `part`, `variant` (with a leading
    /// `_` when a data column has the name).
    pub extra: [String; 5],
}

fn class(d: Dtype) -> u8 {
    match d {
        Dtype::Integer | Dtype::Number | Dtype::Percent | Dtype::Currency => 1,
        Dtype::Text => 2,
        Dtype::Date | Dtype::Datetime => 3,
        Dtype::Bool => 4,
        Dtype::Empty => 5,
    }
}

/// A `/`-segment that is only a date or period (`2001-05-09`, `Jan 2024`, `2024`).
fn is_period(seg: &str) -> bool {
    let s = seg.trim();
    if s.is_empty() {
        return false;
    }
    let digits = s.chars().any(|c| c.is_ascii_digit());
    (digits && s.chars().all(|c| c.is_ascii_digit() || "-./: ".contains(c))) || crate::views::period_of(s).is_some_and(|(p, _)| p.len() >= 4 && s.len() <= p.len() + 12)
}

/// A column name without its date / period segments.
fn stripped(name: &str) -> String {
    let segs: Vec<&str> = name.split(" / ").filter(|s| !is_period(s)).collect();
    if segs.is_empty() { name.to_string() } else { segs.join(" / ") }
}

/// The longest run of trailing ` / ` segments all names share.
fn common_suffix(names: &[String]) -> Option<String> {
    let split: Vec<Vec<&str>> = names.iter().map(|n| n.split(" / ").collect()).collect();
    let mut k = 0;
    loop {
        let seg = split[0].len().checked_sub(k + 1).map(|i| split[0][i]);
        let Some(seg) = seg else { break };
        if split.iter().all(|s| s.len() > k && s[s.len() - k - 1] == seg) {
            k += 1;
        } else {
            break;
        }
    }
    (k > 0).then(|| split[0][split[0].len() - k..].join(" / "))
}

/// The words of each string that the strings do not share at the start or end.
fn differing_parts(texts: &[String]) -> Vec<String> {
    let words: Vec<Vec<&str>> = texts.iter().map(|t| t.split_whitespace().collect()).collect();
    let min = words.iter().map(Vec::len).min().unwrap_or(0);
    let mut pre = 0;
    while pre < min && words.iter().all(|w| w[pre] == words[0][pre]) {
        pre += 1;
    }
    let mut suf = 0;
    while suf < min - pre && words.iter().all(|w| w[w.len() - 1 - suf] == words[0][words[0].len() - 1 - suf]) {
        suf += 1;
    }
    words
        .iter()
        .map(|w| {
            let mid = &w[pre..w.len() - suf];
            // "in der Steiermark" → "Steiermark": short lowercase words in front (in, im, der, of, the)
            let start = mid
                .iter()
                .position(|x| !(x.chars().count() <= 3 && x.chars().all(char::is_lowercase)))
                .unwrap_or(mid.len());
            let rest = if start < mid.len() { &mid[start..] } else { mid };
            rest.join(" ").trim_matches(|c: char| c == '–' || c == '-' || c == ',' || c == ':' || c.is_whitespace()).to_string()
        })
        .collect()
}

fn eligible(t: &Table) -> bool {
    t.columns.len() <= MAX_TABLE_COLUMNS && t.columns.iter().any(|c| c.role == ColRole::SrcRow)
}

/// The families of `norm` with two or more members, in table order.
pub fn families(norm: &Normalized) -> Vec<Family> {
    // (shape, members, stripped names of the first member)
    let mut groups: Vec<(Vec<u8>, Vec<usize>, Vec<String>)> = Vec::new();
    for (k, t) in norm.tables.iter().enumerate() {
        if !eligible(t) {
            continue;
        }
        let names = sql_names(t);
        let cols: Vec<usize> = data_cols(t).map(|(i, _)| i).collect();
        if cols.is_empty() {
            continue;
        }
        let shape: Vec<u8> = cols.iter().map(|&i| class(t.columns[i].dtype)).collect();
        let own: Vec<String> = cols.iter().map(|&i| stripped(&names[i])).collect();
        let fits = |g: &(Vec<u8>, Vec<usize>, Vec<String>)| {
            if g.0 != shape || g.1.len() >= MAX_MEMBERS {
                return false;
            }
            let same = g.2.iter().zip(&own).filter(|(a, b)| a.eq_ignore_ascii_case(b)).count();
            if cols.len() < MIN_SHAPE_COLS {
                same == cols.len()
            } else {
                same as f64 >= SAME_NAMES * cols.len() as f64
            }
        };
        match groups.iter_mut().find(|g| fits(g)) {
            Some(g) => g.1.push(k),
            None => groups.push((shape, vec![k], own)),
        }
    }
    let mut out = Vec::new();
    for (_, members, _) in groups.into_iter().filter(|g| g.1.len() >= 2) {
        out.push(family(norm, members, out.len() + 1));
    }
    out
}

fn family(norm: &Normalized, members: Vec<usize>, k: usize) -> Family {
    let tables: Vec<&Table> = members.iter().map(|&m| &norm.tables[m]).collect();
    let member_cols: Vec<Vec<usize>> = tables.iter().map(|t| data_cols(t).map(|(i, _)| i).collect()).collect();
    let member_names: Vec<Vec<String>> = tables
        .iter()
        .zip(&member_cols)
        .map(|(t, cols)| {
            let names = sql_names(t);
            cols.iter().map(|&i| stripped(&names[i])).collect()
        })
        .collect();
    let width = member_cols[0].len();
    let mut columns: Vec<String> = Vec::with_capacity(width);
    let mut variants: Option<Vec<String>> = None;
    for p in 0..width {
        let at: Vec<String> = member_names.iter().map(|n| n[p].clone()).collect();
        if at.iter().all(|n| n == &at[0]) {
            columns.push(at[0].clone());
            continue;
        }
        let name = common_suffix(&at).unwrap_or_else(|| at[0].clone());
        if variants.is_none() {
            // The words in front of the shared part, per member (the first column that differs).
            let v: Vec<String> = at
                .iter()
                .map(|n| n.strip_suffix(name.as_str()).unwrap_or(n).trim_end_matches([' ', '/']).trim().to_string())
                .collect();
            if v.iter().any(|x| !x.is_empty()) {
                variants = Some(v);
            }
        }
        columns.push(name);
    }
    // Distinct names (SQLite compares names without case).
    let mut seen: Vec<String> = Vec::new();
    for c in columns.iter_mut() {
        let base = c.clone();
        let mut n = 2;
        while seen.contains(&c.to_lowercase()) {
            *c = format!("{base} ({n})");
            n += 1;
        }
        seen.push(c.to_lowercase());
    }
    let titles: Vec<Option<&str>> = tables.iter().map(|t| t.title.as_ref().map(|x| x.0.trim()).filter(|s| !s.is_empty())).collect();
    let parts = if titles.iter().all(Option::is_some) && titles.iter().any(|t| *t != titles[0]) {
        let texts: Vec<String> = titles.iter().map(|t| t.unwrap_or_default().to_string()).collect();
        Some(differing_parts(&texts))
    } else {
        None
    };
    let extra = ["_table", "sheet", "title", "part", "variant"].map(|n| {
        let mut name = n.to_string();
        while seen.contains(&name.to_lowercase()) {
            name.insert(0, '_');
        }
        name
    });
    Family { name: format!("U{k}"), members, columns, member_cols, parts, variants, extra }
}

fn sql_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `CREATE VIEW` statements of `U<k>_all` and `U<k>`.
pub fn create_sql(norm: &Normalized, f: &Family) -> [String; 2] {
    let mut selects = Vec::with_capacity(f.members.len());
    for (j, &m) in f.members.iter().enumerate() {
        let t = &norm.tables[m];
        let names = sql_names(t);
        let mut cols = vec![
            format!("{} AS {}", sql_text(&t.id()), quote_ident(&f.extra[0])),
            format!("{} AS {}", sql_text(&t.source_sheet), quote_ident(&f.extra[1])),
            format!("{} AS {}", t.title.as_ref().map_or("NULL".to_string(), |x| sql_text(&x.0)), quote_ident(&f.extra[2])),
        ];
        if let Some(p) = &f.parts {
            cols.push(format!("{} AS {}", sql_text(&p[j]), quote_ident(&f.extra[3])));
        }
        if let Some(v) = &f.variants {
            cols.push(format!("{} AS {}", sql_text(&v[j]), quote_ident(&f.extra[4])));
        }
        for (&i, u) in f.member_cols[j].iter().zip(&f.columns) {
            cols.push(format!("{} AS {}", quote_ident(&names[i]), quote_ident(u)));
        }
        cols.push("\"_src_row\"".into());
        cols.push("\"_row_type\"".into());
        selects.push(format!("SELECT {} FROM {}", cols.join(", "), quote_ident(&format!("{}_all", t.id()))));
    }
    let all = format!("{}_all", f.name);
    let mut outer: Vec<String> = f.extra[..3].iter().map(|n| quote_ident(n)).collect();
    if f.parts.is_some() {
        outer.push(quote_ident(&f.extra[3]));
    }
    if f.variants.is_some() {
        outer.push(quote_ident(&f.extra[4]));
    }
    outer.extend(f.columns.iter().map(|c| quote_ident(c)));
    outer.push("\"_src_row\"".into());
    [
        format!("CREATE VIEW {} AS {}", quote_ident(&all), selects.join(" UNION ALL ")),
        format!("CREATE VIEW {} AS SELECT {} FROM {} WHERE \"_row_type\" = 'data'", quote_ident(&f.name), outer.join(", "), quote_ident(&all)),
    ]
}

/// Creates the union views (called by [`crate::sql::Db::load`] inside its transaction, after the
/// `T<n>` / `T<n>_all` views exist).
pub fn create_all(tx: &Transaction<'_>, norm: &Normalized) -> rusqlite::Result<usize> {
    let fams = families(norm);
    for f in &fams {
        for s in create_sql(norm, f) {
            tx.execute_batch(&s)?;
        }
    }
    Ok(fams.len())
}

/// Table ids of `ids` as a short list: `T2..T10` when consecutive.
fn id_range(norm: &Normalized, members: &[usize]) -> String {
    let ns: Vec<usize> = members.iter().map(|&m| norm.tables[m].n).collect();
    if ns.windows(2).all(|w| w[1] == w[0] + 1) && ns.len() > 2 {
        return format!("T{}..T{}", ns[0], ns[ns.len() - 1]);
    }
    let mut s: Vec<String> = ns.iter().take(8).map(|n| format!("T{n}")).collect();
    if ns.len() > 8 {
        s.push(format!("+{} more", ns.len() - 8));
    }
    s.join(", ")
}

/// The catalog's lines for the union views (one per family).
pub fn catalog_lines(norm: &Normalized) -> Vec<String> {
    families(norm)
        .iter()
        .map(|f| {
            let mut adds = vec![f.extra[0].clone(), f.extra[1].clone(), f.extra[2].clone()];
            let mut by = ident(&f.extra[1]);
            if let Some(p) = &f.parts {
                let mut vals: Vec<String> = p.iter().take(6).map(|x| sql_text(&short(x, 30))).collect();
                if p.len() > 6 {
                    vals.push("…".into());
                }
                adds.push(format!("{} = {}", f.extra[3], vals.join(" | ")));
                by = ident(&f.extra[3]);
            }
            if let Some(v) = &f.variants {
                let mut d: Vec<String> = Vec::new();
                for x in v {
                    let x = sql_text(&short(x, 40));
                    if !d.contains(&x) {
                        d.push(x);
                    }
                }
                adds.push(format!("{} = {}", f.extra[4], d.iter().take(4).cloned().collect::<Vec<_>>().join(" | ")));
            }
            format!(
                "SQL union view {} = {} ({} tables with the same columns; adds {}): compare them in one query, e.g. SELECT {by}, SUM(…) FROM {} GROUP BY {by}; {}_all also has the total rows (_row_type)",
                f.name,
                id_range(norm, &f.members),
                f.members.len(),
                adds.join(", "),
                f.name,
                f.name
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_parts() {
        assert_eq!(stripped("2001-05-09 / Day-Ahead Transactions / Qty"), "Day-Ahead Transactions / Qty");
        assert_eq!(stripped("Ankünfte / Absolut"), "Ankünfte / Absolut");
        assert_eq!(common_suffix(&["Day-Ahead / Hour".into(), "Real-Time Transactions / Hour".into()]).as_deref(), Some("Hour"));
        let parts = differing_parts(&[
            "Ankünfte in Österreich – nach Herkunftsland Juli 2026".into(),
            "Ankünfte im Burgenland – nach Herkunftsland Juli 2026".into(),
            "Ankünfte in der Steiermark – nach Herkunftsland Juli 2026".into(),
        ]);
        assert_eq!(parts, ["Österreich", "Burgenland", "Steiermark"]);
    }
}
