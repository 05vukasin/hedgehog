//! Compact forms for the catalog when room is short (B100/B101): column names with shared header
//! levels written once, and one line for a run of tables that repeat an earlier table's columns.
//! Pure text functions; catalog.rs decides where they are used.

use crate::text::{ident, short};

/// The level separator of flattened multi-row headers (`Ensemble / Total`).
const SEP: &str = " / ";

/// How a reader expands the groups, for a legend: `a / {b, c} = "a / b", "a / c"`.
pub const LEGEND: &str = "a / {b, c} = \"a / b\", \"a / c\"";

/// Column names with shared header levels written once: names that share their first level as
/// `a / {b, c}`, names that share their last level as `{a, b} / c`, nested; groups of 2+
/// consecutive names. Every name can be read back (score.py's `expand_braces`); members are
/// separated by `, `, or by `; ` when a member has a comma of its own. Items are SQL identifiers:
/// a plain name bare, anything else quoted.
pub fn compress_names(names: &[String]) -> Vec<String> {
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    groups(&refs).into_iter().map(|g| if g.contains('{') { format!("\"{g}\"") } else { ident(&g) }).collect()
}

/// Is there a `, ` outside braces (a member that would need `; ` between members)?
fn top_comma(s: &str) -> bool {
    let mut depth = 0i32;
    let b = s.as_bytes();
    for (k, &c) in b.iter().enumerate() {
        match c {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b',' if depth == 0 && b.get(k + 1) == Some(&b' ') => return true,
            _ => {}
        }
    }
    false
}

fn join(items: &[String]) -> String {
    items.join(if items.iter().any(|x| top_comma(x)) { "; " } else { ", " })
}

/// The grouped texts (unquoted) of consecutive names.
fn groups(names: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < names.len() {
        let head = names[i].split_once(SEP).map(|x| x.0);
        let tail = names[i].rsplit_once(SEP).map(|x| x.1);
        let run = |want: Option<&str>, f: fn(&str) -> Option<&str>| {
            let mut j = i;
            while j < names.len() && want.is_some() && f(names[j]) == want {
                j += 1;
            }
            j
        };
        let jp = run(head, |n| n.split_once(SEP).map(|x| x.0));
        let js = run(tail, |n| n.rsplit_once(SEP).map(|x| x.1));
        if jp - i >= 2 && jp >= js {
            let rests: Vec<&str> = names[i..jp].iter().filter_map(|n| n.split_once(SEP).map(|x| x.1)).collect();
            let inner = groups(&rests);
            let head = head.unwrap_or_default();
            out.push(if inner.len() == 1 { format!("{head}{SEP}{}", inner[0]) } else { format!("{head}{SEP}{{{}}}", join(&inner)) });
            i = jp;
        } else if js - i >= 2 {
            let fronts: Vec<&str> = names[i..js].iter().filter_map(|n| n.rsplit_once(SEP).map(|x| x.0)).collect();
            let inner = groups(&fronts);
            let tail = tail.unwrap_or_default();
            out.push(if inner.len() == 1 { format!("{}{SEP}{tail}", inner[0]) } else { format!("{{{}}}{SEP}{tail}", join(&inner)) });
            i = js;
        } else {
            out.push(names[i].to_string());
            i += 1;
        }
    }
    out
}

/// A table whose columns are an earlier table's columns with one more header level (B100): its
/// own leading keys (also in `a`), then `a[p..q]`, each with ` / <suffix>` (`T29`: `Jahr`, then
/// every `T27` column from "Einnahmen / insgesamt" to "Finanzierungs-saldo" + " / % des
/// Bruttoinlandsprodukts"). The text: `Jahr + T27's "Einnahmen / insgesamt".."Finanzierungs-saldo",
/// each + " / % des Bruttoinlandsprodukts"`.
pub fn derived(b: &[String], a: &[String], a_id: &str) -> Option<String> {
    let k = b.iter().take_while(|n| a.contains(n)).count();
    let rest = &b[k..];
    if rest.len() < 3 {
        return None;
    }
    // Every split of the first name at a level separator: front (a column of `a`) / suffix.
    let first = rest[0].as_str();
    for (at, _) in first.match_indices(SEP) {
        let (front, suffix) = (&first[..at], &first[at + SEP.len()..]);
        let Some(p) = a.iter().position(|x| x == front) else { continue };
        if p + rest.len() > a.len() {
            continue;
        }
        if rest.iter().zip(&a[p..]).all(|(n, x)| n.strip_suffix(suffix).and_then(|n| n.strip_suffix(SEP)) == Some(x.as_str())) {
            let keys: Vec<String> = b[..k].iter().map(|x| ident(x)).collect();
            let keys = if keys.is_empty() { String::new() } else { format!("{} + ", keys.join(" ")) };
            let q = p + rest.len() - 1;
            return Some(format!("{keys}{a_id}'s \"{}\"..\"{}\", each + \" / {suffix}\"", a[p], a[q]));
        }
    }
    None
}

/// One table of a run: its id, title and sheet.
pub struct RunMember<'a> {
    pub id: String,
    pub title: Option<&'a str>,
    pub sheet: &'a str,
}

/// What tells the members of a run apart: the words of their titles that differ (`Burgenland`,
/// `2022`), else their sheet names. `('Ankünfte … Juli 2026', ["T2 Burgenland", …])`; the pattern
/// is the first title with its differing words as `…`.
pub fn member_names(members: &[RunMember]) -> (Option<String>, Vec<String>) {
    let titles: Vec<String> = members.iter().filter_map(|m| m.title.map(String::from)).collect();
    if titles.len() == members.len() && titles.len() >= 2 {
        let parts = crate::unions::differing_parts(&titles);
        let distinct: std::collections::HashSet<&String> = parts.iter().collect();
        if distinct.len() == parts.len() && parts.iter().all(|p| !p.is_empty()) {
            // The words all titles share in front and at the end, the rest as `…`.
            let words: Vec<Vec<&str>> = titles.iter().map(|t| t.split_whitespace().collect()).collect();
            let min = words.iter().map(Vec::len).min().unwrap_or(0);
            let pre = (0..min).take_while(|&i| words.iter().all(|w| w[i] == words[0][i])).count();
            let suf = (0..min - pre).take_while(|&i| words.iter().all(|w| w[w.len() - 1 - i] == words[0][words[0].len() - 1 - i])).count();
            let w0 = &words[0];
            let pattern = [w0[..pre].join(" "), "…".to_string(), w0[w0.len() - suf..].join(" ")]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let items = members.iter().zip(&parts).map(|(m, p)| format!("{} {}", m.id, short(p, 40))).collect();
            return (Some(short(&pattern, 90)), items);
        }
    }
    (None, members.iter().map(|m| format!("{} {}", m.id, short(m.sheet, 30))).collect())
}

/// The line for a run of consecutive tables that repeat the columns of an earlier table `first`:
/// `T2..T10 same columns as T1 · <facts> · titles 'Ankünfte … Juli 2026': T2 Burgenland, …`.
pub fn run_line(members: &[RunMember], first: &str, facts: &str) -> String {
    let (pattern, items) = member_names(members);
    let what = match pattern {
        Some(p) => format!("titles '{p}': "),
        None => "sheets: ".to_string(),
    };
    format!(
        "{}..{} same columns as {first}{facts} · {what}{}",
        members[0].id,
        members[members.len() - 1].id,
        items.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn shared_levels_once() {
        let n = v(&[
            "Jahr",
            "In jeweiligen Preisen / Ursprungswerte / Mrd. EUR",
            "In jeweiligen Preisen / Ursprungswerte / %",
            "Preisbereinigt, verkettet / Ursprungswerte / 2020 = 100",
            "Preisbereinigt, verkettet / Ursprungswerte / %",
        ]);
        assert_eq!(
            compress_names(&n),
            [
                "Jahr",
                "\"In jeweiligen Preisen / Ursprungswerte / {Mrd. EUR, %}\"",
                "\"Preisbereinigt, verkettet / Ursprungswerte / {2020 = 100, %}\""
            ]
        );
        // A shared last level: `{a, b} / c`.
        let n = v(&["Jahr", "Einnahmen / % des BIP", "Ausgaben / % des BIP", "Saldo / % des BIP"]);
        assert_eq!(compress_names(&n), ["Jahr", "\"{Einnahmen, Ausgaben, Saldo} / % des BIP\""]);
        // Members with commas: `; ` between members.
        let n = v(&["A / x, y", "A / z"]);
        assert_eq!(compress_names(&n), ["\"A / {x, y; z}\""]);
        // Nothing shared: the names as they are.
        assert_eq!(compress_names(&v(&["a", "b c"])), ["a", "\"b c\""]);
    }

    #[test]
    fn columns_of_an_earlier_table_with_one_more_level() {
        let a = v(&["group", "Jahr", "Einnahmen / insgesamt", "Ausgaben / insgesamt", "Saldo"]);
        let b = v(&["Jahr", "Einnahmen / insgesamt / % des BIP", "Ausgaben / insgesamt / % des BIP", "Saldo / % des BIP"]);
        assert_eq!(
            derived(&b, &a, "T27").as_deref(),
            Some("Jahr + T27's \"Einnahmen / insgesamt\"..\"Saldo\", each + \" / % des BIP\"")
        );
        // One name that does not follow: no.
        let mut c = b.clone();
        c[3] = "Saldo / in Mrd. EUR".into();
        assert_eq!(derived(&c, &a, "T27"), None);
    }

    #[test]
    fn runs_named_by_their_titles() {
        let m = |id: &str, t: &'static str, s: &'static str| RunMember { id: id.into(), title: Some(t), sheet: s };
        let ms = [
            m("T2", "Nächte in Burgenland – Juli 2026", "Bgld"),
            m("T3", "Nächte in Kärnten – Juli 2026", "Ktn"),
            m("T4", "Nächte in Tirol – Juli 2026", "T"),
        ];
        assert_eq!(
            run_line(&ms, "T1", " · 72 rows each"),
            "T2..T4 same columns as T1 · 72 rows each · titles 'Nächte in … – Juli 2026': T2 Burgenland, T3 Kärnten, T4 Tirol"
        );
        let ms = [RunMember { id: "T2".into(), title: None, sheet: "2022" }, RunMember { id: "T3".into(), title: None, sheet: "2021" }];
        assert_eq!(run_line(&ms, "T1", ""), "T2..T3 same columns as T1 · sheets: T2 2022, T3 2021");
    }
}
