//! Entity columns: text columns whose values are `"<id> - <name>"`, such as a buyer column with
//! `"503933813 - IP - Infraestruturas de Portugal, S.A."`.
//!
//! Registers often write one entity (a company, a public body) under several spellings with the
//! same code in front; grouping by the text then lists one entity several times. For each such
//! column the SQL views get two more columns, `<col>_id` (the code) and `<col>_name` (the rest),
//! and the catalog says how many IDs have several spellings and to group by `<col>_id`.
//!
//! A value without the separator is either a bare ID (`<col>_id` set, `<col>_name` empty) or a
//! name without an ID (`<col>_id` empty). A cell naming several entities (`"a | b"`, `"a; b"`, one
//! per line) is not split: its `<col>_id` lists every ID joined by `" | "`, so it stays one group.

use std::collections::{HashMap, HashSet};

use hh_core::normalize::{ColRole, Column, Dtype, Out, Table};

use crate::catalog::sql_names;
use crate::text::{ident, quote_ident, short, thousands};

/// Separators between the ID and the name.
const SEPS: [&str; 3] = [" - ", " – ", " — "];
/// Separators between several entities in one cell.
const MULTI_SEPS: [char; 3] = ['|', ';', '\n'];
/// Share of non-empty values that must be `<id> - <name>` (or several of them).
const MIN_SHARE: f64 = 0.8;
/// Values checked before a column is scanned in full.
const SAMPLE: usize = 300;

/// One detected entity column.
#[derive(Debug, Clone)]
pub struct Entity {
    /// Index in `Table::columns`.
    pub col: usize,
    /// SQL names: the column, its ID part, its name part.
    pub name: String,
    pub id_col: String,
    pub name_col: String,
    /// The separator most values use.
    pub sep: &'static str,
    /// Non-empty values, values `<id> - <name>`, bare IDs, values without an ID, cells with
    /// several entities (and the separator they use).
    pub values: usize,
    pub split: usize,
    pub bare_ids: usize,
    pub no_id: usize,
    pub multi: usize,
    pub multi_sep: Option<char>,
    /// Distinct IDs and how many of them appear under more than one name.
    pub ids: usize,
    pub several: usize,
    /// The ID with several names that has the most rows: (id, names); the most names of one ID.
    pub most: Option<(String, usize)>,
    pub max_names: usize,
    /// A cell with several entities.
    pub multi_example: String,
    /// A typical value.
    pub example: String,
}

/// A code: 2–24 characters, letters, digits and `./-_`, at least 40 % digits.
fn is_code(s: &str) -> bool {
    let n = s.chars().count();
    if !(2..=24).contains(&n) {
        return false;
    }
    let digits = s.chars().filter(char::is_ascii_digit).count();
    s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-' | '_')) && digits * 5 >= n * 2
}

/// A name: at least two letters, and not a number range ("10 - 20 km", "2020 - 2021": the part
/// after the separator starts with a plain number).
fn is_name(s: &str) -> bool {
    let first = s.split_whitespace().next().unwrap_or("");
    s.chars().filter(|c| c.is_alphabetic()).count() >= 2 && !first.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ','))
}

/// Two codes of the same shape: digits where the other has digits, letters where it has letters,
/// other characters equal (the IDs of one column share it: "503933813", "45247220-8").
fn same_shape(a: &str, b: &str) -> bool {
    let class = |c: char| if c.is_ascii_digit() { '9' } else if c.is_alphabetic() { 'a' } else { c };
    a.len() == b.len() && a.chars().zip(b.chars()).all(|(x, y)| class(x) == class(y))
}

/// `<id> - <name>` with any of [`SEPS`]: (id, name, separator).
fn split_one(s: &str) -> Option<(&str, &str, &'static str)> {
    let s = s.trim();
    let (i, sep) = SEPS.iter().filter_map(|sep| s.find(sep).map(|i| (i, *sep))).min_by_key(|x| x.0)?;
    let (id, name) = (s[..i].trim(), s[i + sep.len()..].trim());
    (is_code(id) && is_name(name)).then_some((id, name, sep))
}

/// How one cell reads.
#[derive(Debug, PartialEq)]
pub enum Parsed<'a> {
    One { id: &'a str, name: &'a str, sep: &'static str },
    /// Several `<id> - <name>` parts; `sep` is the character before the second one (`' '` when
    /// they are only run together).
    Many { parts: Vec<(&'a str, &'a str)>, sep: char },
    /// A code alone (or `"<id> - <id>"`).
    BareId(&'a str),
    /// No ID: the name (an empty ID such as `"- - Name"` left out).
    NoId(&'a str),
}

/// Where further entities start in `t`, whose first ID is `id0`: after whitespace or one of
/// [`MULTI_SEPS`], a code of the same shape followed by a separator.
fn entity_starts(t: &str, id0: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut prev = ' ';
    for (i, c) in t.char_indices() {
        if i > 0 && (prev.is_whitespace() || MULTI_SEPS.contains(&prev)) && !c.is_whitespace() {
            let rest = &t[i..];
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let tok = &rest[..end];
            if same_shape(tok, id0) && SEPS.iter().any(|s| rest[end..].starts_with(s)) {
                starts.push(i);
            }
        }
        prev = c;
    }
    starts
}

pub fn parse(s: &str) -> Parsed<'_> {
    let t = s.trim();
    let Some((id, name, sep)) = split_one(t) else {
        // "500934290 - 500934290" and bare codes.
        if let Some((a, b)) = SEPS.iter().find_map(|sep| t.split_once(sep)) {
            let (a, b) = (a.trim(), b.trim());
            if is_code(a) && (a == b || b.is_empty()) {
                return Parsed::BareId(a);
            }
        }
        if is_code(t) {
            return Parsed::BareId(t);
        }
        // An empty ID: "- - Name", "- Name".
        let mut rest = t;
        while let Some(r) = rest.strip_prefix(['-', '–', '—']) {
            rest = r.trim_start();
        }
        return Parsed::NoId(rest);
    };
    let starts = entity_starts(t, id);
    if starts.is_empty() {
        return Parsed::One { id, name, sep };
    }
    let mut bounds = vec![0];
    bounds.extend(&starts);
    bounds.push(t.len());
    let parts: Option<Vec<(&str, &str)>> = bounds
        .windows(2)
        .map(|w| {
            let piece = t[w[0]..w[1]].trim_end().trim_end_matches(MULTI_SEPS).trim_end_matches(',').trim_end();
            split_one(piece).map(|(i, n, _)| (i, n))
        })
        .collect();
    match parts {
        Some(parts) => {
            let before = t[..starts[0]].trim_end().chars().last().filter(|c| MULTI_SEPS.contains(c)).unwrap_or(' ');
            Parsed::Many { parts, sep: before }
        }
        None => Parsed::One { id, name, sep },
    }
}

/// The values of the two SQL columns for one cell: (`<col>_id`, `<col>_name`).
pub fn derived(s: &str) -> (Option<String>, Option<String>) {
    match parse(s) {
        Parsed::One { id, name, .. } => (Some(id.to_string()), Some(name.to_string())),
        Parsed::Many { parts, .. } => (
            Some(parts.iter().map(|p| p.0).collect::<Vec<_>>().join(" | ")),
            Some(parts.iter().map(|p| p.1).collect::<Vec<_>>().join(" | ")),
        ),
        Parsed::BareId(id) => (Some(id.to_string()), None),
        Parsed::NoId(name) => (None, Some(name.to_string()).filter(|x| !x.is_empty())),
    }
}

/// One name per ID: the spelling most rows of the column use (the first seen on a tie), so
/// `<col>_name` is the same on every row of an ID and grouping by it does not split one entity.
pub fn canonical_names(c: &Column) -> HashMap<String, String> {
    let mut counts: HashMap<&str, Vec<(&str, usize)>> = HashMap::new();
    for s in c.values.iter().filter_map(text) {
        let parts = match parse(s) {
            Parsed::One { id, name, .. } => vec![(id, name)],
            Parsed::Many { parts, .. } => parts,
            _ => continue,
        };
        for (id, name) in parts {
            let v = counts.entry(id).or_default();
            if let Some(x) = v.iter_mut().find(|x| x.0 == name) {
                x.1 += 1;
            } else if v.len() < 1000 {
                v.push((name, 1));
            }
        }
    }
    counts
        .into_iter()
        .filter_map(|(id, v)| {
            // max_by_key keeps the last maximum; reversed, the first seen wins a tie.
            let best = v.iter().rev().max_by_key(|x| x.1)?.0;
            Some((id.to_string(), best.to_string()))
        })
        .collect()
}

/// [`derived`] with the name replaced by the canonical one of its ID(s) ([`canonical_names`]).
pub fn derived_canonical(s: &str, names: &HashMap<String, String>) -> (Option<String>, Option<String>) {
    let (id, name) = derived(s);
    let canon = id.as_deref().and_then(|ids| {
        ids.split(" | ").map(|i| names.get(i).map(String::as_str)).collect::<Option<Vec<&str>>>().map(|v| v.join(" | "))
    });
    match canon {
        Some(c) if name.is_some() => (id, Some(c)),
        _ => (id, name),
    }
}

fn text(v: &Out) -> Option<&str> {
    match v {
        Out::Text(s) if !s.trim().is_empty() => Some(s.as_str()),
        _ => None,
    }
}

/// The entity columns of a table (data and group text columns), in column order.
pub fn detect(t: &Table) -> Vec<Entity> {
    let names = sql_names(t);
    let taken: HashSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mut out = Vec::new();
    for (ci, c) in t.columns.iter().enumerate() {
        if !matches!(c.role, ColRole::Data | ColRole::Group) || c.dtype != Dtype::Text {
            continue;
        }
        // Quick look at the first values.
        let sample: Vec<&str> = c.values.iter().filter_map(text).take(SAMPLE).collect();
        if sample.len() < 3 {
            continue;
        }
        let ok = sample.iter().filter(|s| matches!(parse(s), Parsed::One { .. } | Parsed::Many { .. })).count();
        if (ok as f64) < MIN_SHARE * 0.75 * sample.len() as f64 {
            continue;
        }
        let (id_col, name_col) = (format!("{}_id", names[ci]), format!("{}_name", names[ci]));
        if taken.contains(&id_col.to_lowercase()) || taken.contains(&name_col.to_lowercase()) {
            continue;
        }
        let mut e = Entity {
            col: ci,
            name: names[ci].clone(),
            id_col,
            name_col,
            sep: SEPS[0],
            values: 0,
            split: 0,
            bare_ids: 0,
            no_id: 0,
            multi: 0,
            multi_sep: None,
            ids: 0,
            several: 0,
            most: None,
            max_names: 0,
            multi_example: String::new(),
            example: String::new(),
        };
        // id → (rows, distinct names)
        let mut by_id: HashMap<&str, (usize, Vec<&str>)> = HashMap::new();
        let mut seps: HashMap<&'static str, usize> = HashMap::new();
        for s in c.values.iter().filter_map(text) {
            e.values += 1;
            match parse(s) {
                Parsed::One { id, name, sep } => {
                    e.split += 1;
                    *seps.entry(sep).or_default() += 1;
                    let x = by_id.entry(id).or_default();
                    x.0 += 1;
                    if x.1.len() < 1000 && !x.1.contains(&name) {
                        x.1.push(name);
                    }
                }
                Parsed::Many { sep, .. } => {
                    e.multi += 1;
                    if e.multi_sep.is_none() {
                        e.multi_sep = Some(sep);
                        e.multi_example = short(s, 70);
                    }
                }
                Parsed::BareId(_) => e.bare_ids += 1,
                Parsed::NoId(_) => e.no_id += 1,
            }
        }
        if ((e.split + e.multi) as f64) < MIN_SHARE * e.values as f64 || by_id.len() < 2 {
            continue;
        }
        e.sep = seps.into_iter().max_by_key(|x| x.1).map_or(SEPS[0], |x| x.0);
        e.ids = by_id.len();
        e.several = by_id.values().filter(|x| x.1.len() > 1).count();
        e.most = by_id
            .iter()
            .filter(|x| x.1.1.len() > 1)
            .max_by(|a, b| (a.1.0, b.0).cmp(&(b.1.0, a.0)))
            .map(|(id, x)| (id.to_string(), x.1.len()));
        e.max_names = by_id.values().map(|x| x.1.len()).max().unwrap_or(0);
        e.example = c
            .values
            .iter()
            .filter_map(text)
            .find(|s| matches!(parse(s), Parsed::One { .. }))
            .map(|s| short(s, 60))
            .unwrap_or_default();
        out.push(e);
    }
    out
}

/// One line per entity column, for the catalog and `describe_table` (indented by 4).
pub fn lines(t: &Table) -> Vec<String> {
    detect(t).iter().map(|e| line(e, &t.id())).collect()
}

/// What to group entity column `e` by: `<col>_id`, or `coalesce(<col>_id, <col>_name)` when some
/// values have no ID (they would all fall into one empty-ID group).
pub fn group_key(e: &Entity) -> String {
    match e.no_id {
        0 => quote_ident(&e.id_col),
        _ => format!("coalesce({}, {})", quote_ident(&e.id_col), quote_ident(&e.name_col)),
    }
}

/// The ready query for grouping entity column `e` of table `id` by entity (B18).
pub fn group_sql(e: &Entity, id: &str) -> String {
    format!("SELECT {}, {}, count(*) FROM {id} GROUP BY {}", quote_ident(&e.id_col), quote_ident(&e.name_col), group_key(e))
}

pub fn line(e: &Entity, table: &str) -> String {
    let mut s = format!(
        "    {} = \"<id>{}<name>\" (e.g. \"{}\"): {} IDs",
        ident(&e.name),
        e.sep,
        e.example.replace('"', "'"),
        thousands(e.ids)
    );
    match &e.most {
        Some((id, n)) => s.push_str(&format!(
            ", {} with several spellings (e.g. {id}: {n} names{}) — group by {}, not by the text: {}",
            thousands(e.several),
            if e.max_names > *n { format!("; up to {}", e.max_names) } else { String::new() },
            ident(&e.id_col),
            group_sql(e, table)
        )),
        None => s.push_str(&format!(", one spelling each — group by {}", ident(&e.id_col))),
    }
    s.push_str(&format!("; {} holds one name per ID (its most frequent spelling)", ident(&e.name_col)));
    let mut extra = Vec::new();
    if e.multi > 0 {
        let how = match e.multi_sep {
            Some('\n') => "one per line".to_string(),
            Some(' ') | None => "run together".to_string(),
            Some(c) => format!("joined by '{c}'"),
        };
        extra.push(format!(
            "{} {} name several entities, {how} (e.g. \"{}\"): {} lists every ID as 'id1 | id2'",
            thousands(e.multi),
            if e.multi == 1 { "cell" } else { "cells" },
            e.multi_example.replace('"', "'"),
            ident(&e.id_col)
        ));
    }
    if e.bare_ids > 0 {
        extra.push(format!("{} only an ID", thousands(e.bare_ids)));
    }
    if e.no_id > 0 {
        extra.push(format!(
            "{} without an ID ({} empty; group by coalesce({}, {}) to keep them apart)",
            thousands(e.no_id),
            ident(&e.id_col),
            ident(&e.id_col),
            ident(&e.name_col)
        ));
    }
    if !extra.is_empty() {
        s.push_str(&format!(" · {}", extra.join(" · ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cells() {
        assert_eq!(
            parse("503933813 - IP - Infraestruturas de Portugal, S.A."),
            Parsed::One { id: "503933813", name: "IP - Infraestruturas de Portugal, S.A.", sep: " - " }
        );
        assert_eq!(parse("ESB12345678 – Acme, S.L."), Parsed::One { id: "ESB12345678", name: "Acme, S.L.", sep: " – " });
        assert!(matches!(parse("501 - Alpha | 502 - Beta"), Parsed::Many { ref parts, sep: '|' } if parts.len() == 2));
        assert!(matches!(parse("501 - Alpha; 502 - Beta, Lda"), Parsed::Many { sep: ';', .. }));
        assert_eq!(parse("503933813"), Parsed::BareId("503933813"));
        assert_eq!(parse("Câmara Municipal"), Parsed::NoId("Câmara Municipal"));
        assert_eq!(parse("- - NOVARTIS FARMA, S.A."), Parsed::NoId("NOVARTIS FARMA, S.A."));
        assert_eq!(parse("500934290 - 500934290"), Parsed::BareId("500934290"));
        assert_eq!(parse("980560241 - 3M España, S.L."), Parsed::One { id: "980560241", name: "3M España, S.L.", sep: " - " });
        // Several entities run together, the later ones found by the shape of the first ID.
        let p = parse("502064080 - Veisil - A. Mendes da Silva, Lda 506058654 - FRANKLAB PORTUGAL, LDA");
        assert_eq!(
            p,
            Parsed::Many {
                parts: vec![("502064080", "Veisil - A. Mendes da Silva, Lda"), ("506058654", "FRANKLAB PORTUGAL, LDA")],
                sep: ' '
            }
        );
        // A '|' inside one name is not a second entity.
        assert!(matches!(parse("500900469 - SUCH | Serviço de Utilização Comum"), Parsed::One { id: "500900469", .. }));
        // Ranges, years and words with dashes are not entities.
        assert_eq!(parse("10 - 20 km"), Parsed::NoId("10 - 20 km"));
        assert_eq!(parse("2020 - 2021"), Parsed::NoId("2020 - 2021"));
        assert_eq!(parse("IP - Infraestruturas"), Parsed::NoId("IP - Infraestruturas"));
        assert_eq!(derived("501 - Alpha | 502 - Beta"), (Some("501 | 502".into()), Some("Alpha | Beta".into())));
        assert_eq!(derived("501"), (Some("501".into()), None));
        assert_eq!(derived("Alpha"), (None, Some("Alpha".into())));
    }
}
