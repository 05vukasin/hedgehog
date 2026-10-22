//! Columns computed from another column of the same table, placed right after it: a sortable
//! period for a label column of period texts (`<name>_period`, B70). A derived column is a data
//! column without a source column or header cells ([`Column::is_derived`]); the manifest names
//! the column it comes from.

use std::collections::HashMap;

use super::coverage::{column_label_periods, span_label};
use super::extract::UniqueNames;
use super::{ColRole, Column, Dtype, Out, T, Table};

/// Adds the derived columns of one table.
pub(super) fn add_columns(t: &mut Table) {
    add_period_columns(t);
    add_key_columns(t);
}

/// A spelling's grouping key: lowercase, diacritics folded, without spaces and punctuation
/// (`Mc Kay`, `McKay`, `MCKAY` and `Mc-Kay` share `mckay`).
fn fold(s: &str) -> String {
    crate::values::norm_key(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Columns of at most this many distinct values are checked for spelling variants.
const KEY_MAX_DISTINCT: usize = 5000;
/// Variant groups named in the table warning.
const KEY_WARN_GROUPS: usize = 3;

/// B78: a text column of names or categories in which some values differ only in case, spaces,
/// punctuation or diacritics ('Mc Kay' / 'McKay') gets `<name>_key`, every value replaced by the
/// most frequent spelling of its group, so `GROUP BY "<name>_key"` counts them together; a table
/// warning names the groups. Only for names and categories: values repeat (at most half of them
/// distinct) and are short (at most 40 characters on average), not registers of descriptions.
fn add_key_columns(t: &mut Table) {
    let mut adds: Vec<(usize, Column, String)> = Vec::new();
    for (i, c) in t.columns.iter().enumerate() {
        if !matches!(c.role, ColRole::Data | ColRole::Group) || c.dtype != Dtype::Text || c.is_derived() {
            continue;
        }
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let (mut n, mut chars) = (0usize, 0usize);
        for v in &c.values {
            if let Out::Text(s) = v {
                *counts.entry(s.as_str()).or_default() += 1;
                n += 1;
                chars += s.chars().count();
                if counts.len() > KEY_MAX_DISTINCT {
                    break;
                }
            }
        }
        if counts.len() < 2 || counts.len() > KEY_MAX_DISTINCT || chars > 40 * n || counts.len() * 2 > n {
            continue;
        }
        // Spellings per key, most frequent first (ties: alphabetical, for determinism).
        let mut groups: HashMap<String, Vec<(&str, usize)>> = HashMap::new();
        for (&s, &k) in &counts {
            let key = fold(s);
            if !key.is_empty() {
                groups.entry(key).or_default().push((s, k));
            }
        }
        let mut variants: Vec<Vec<(&str, usize)>> = groups.into_values().filter(|g| g.len() >= 2).collect();
        if variants.is_empty() {
            continue;
        }
        for g in &mut variants {
            g.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        }
        variants.sort_by(|a, b| b.iter().map(|x| x.1).sum::<usize>().cmp(&a.iter().map(|x| x.1).sum()).then(a[0].0.cmp(b[0].0)));
        let canon: HashMap<&str, &str> = variants.iter().flat_map(|g| g.iter().map(move |x| (x.0, g[0].0))).collect();
        let values = c
            .values
            .iter()
            .map(|v| match v {
                Out::Text(s) => Out::Text(canon.get(s.as_str()).copied().unwrap_or(s).to_string()),
                other => other.clone(),
            })
            .collect();
        let name = format!("{}_key", c.name);
        let shown: Vec<String> = variants
            .iter()
            .take(KEY_WARN_GROUPS)
            .map(|g| g.iter().map(|x| format!("'{}'", x.0)).collect::<Vec<_>>().join(" / "))
            .collect();
        let more = if variants.len() > KEY_WARN_GROUPS { format!(" and {} more", variants.len() - KEY_WARN_GROUPS) } else { String::new() };
        let warning =
            format!("\"{}\" has spellings of the same value ({}{}): \"{{key}}\" holds one spelling per group, group by it", c.name, shown.join(", "), more);
        adds.push((i + 1, derived_column(name, values), warning));
    }
    if adds.is_empty() {
        return;
    }
    let mut names = UniqueNames::taken(t.columns.iter().map(|c| c.name.as_str()));
    t.transforms.add(T::KeyColumnsAdded, adds.len());
    for (at, mut c, warning) in adds.into_iter().rev() {
        let unique = names.make(std::mem::take(&mut c.name), &mut t.transforms);
        t.warnings.push(warning.replace("{key}", &unique));
        c.name = unique;
        t.columns.insert(at, c);
    }
}

/// Name of a derived column: `<base>_<suffix>`, or just `<suffix>` for an unnamed base (`col_A`).
fn derived_name(base: &str, suffix: &str) -> String {
    let unnamed = base.strip_prefix("col_").is_some_and(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_uppercase()));
    if unnamed { suffix.to_string() } else { format!("{base}_{suffix}") }
}

fn derived_column(name: String, values: Vec<Out>) -> Column {
    Column {
        name,
        role: ColRole::Data,
        source_col: None,
        original_header: Vec::new(),
        dtype: Dtype::Text,
        unit: None,
        currency: None,
        decimal: None,
        date_order: None,
        ambiguous: 0,
        footnote_marks: Vec::new(),
        values,
    }
}

/// B70: a text column whose labels name periods (`Jän.21`, `Ø 22`, `Mar-May 1971`, `2025 Q1`,
/// stored dates among them) gets `<name>_period` with each label's period in a form that sorts
/// and compares as text: `2022`, `2022-Q1`, `2022-01`, `2025-11/2026-01` (a range). Only when at
/// least three labels name a period and at least one is not already written that way.
fn add_period_columns(t: &mut Table) {
    let mut adds: Vec<(usize, Column)> = Vec::new();
    for (i, c) in t.columns.iter().enumerate() {
        if c.role != ColRole::Data || c.dtype != Dtype::Text || c.is_derived() {
            continue;
        }
        let Some(spans) = column_label_periods(&c.values) else { continue };
        if spans.iter().flatten().count() < 3 {
            continue;
        }
        let labels: Vec<Option<String>> = spans.iter().map(|s| s.as_ref().map(span_label)).collect();
        let differs = c.values.iter().zip(&labels).any(|(v, l)| matches!((v, l), (Out::Text(s), Some(l)) if s.trim() != l));
        if !differs {
            continue;
        }
        let values = labels.into_iter().map(|l| l.map_or(Out::Null, Out::Text)).collect();
        adds.push((i + 1, derived_column(derived_name(&c.name, "period"), values)));
    }
    if adds.is_empty() {
        return;
    }
    let mut names = UniqueNames::taken(t.columns.iter().map(|c| c.name.as_str()));
    t.transforms.add(T::PeriodColumnsAdded, adds.len());
    for (at, mut c) in adds.into_iter().rev() {
        c.name = names.make(std::mem::take(&mut c.name), &mut t.transforms);
        t.columns.insert(at, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &[&str]) -> Vec<Out> {
        v.iter().map(|s| if s.is_empty() { Out::Null } else { Out::Text((*s).into()) }).collect()
    }

    fn labels(v: &[&str]) -> Option<Vec<String>> {
        column_label_periods(&text(v)).map(|s| s.iter().map(|x| x.as_ref().map_or(String::new(), span_label)).collect())
    }

    #[test]
    fn period_labels_of_a_cpi_column() {
        let got = labels(&[
            "Ø 2011", "2012-01-01T00:00:00", "2012-02-01T00:00:00", "Ø 2012", "Ø 13", "2020-03-01T00:00:00", "Apr.201",
            "Mai.201", "Dez.201", "Ø 20", "Jän.211", "Feb.21", "Ø 25", "",
        ])
        .unwrap();
        assert_eq!(
            got,
            [
                "2011", "2012-01", "2012-02", "2012", "2013", "2020-03", "2020-04", "2020-05", "2020-12", "2020", "2021-01",
                "2021-02", "2025", ""
            ]
        );
    }

    #[test]
    fn month_ranges_and_other_forms() {
        assert_eq!(
            labels(&["Jan-Mar 1971", "Feb-Apr 1971", "Nov-Jan 2026", "Dec - Feb 2026"]).unwrap(),
            ["1971-01/1971-03", "1971-02/1971-04", "2025-11/2026-01", "2025-12/2026-02"]
        );
        assert_eq!(labels(&["2025 Q1", "2025 Q2", "Q3 2025"]).unwrap(), ["2025-Q1", "2025-Q2", "2025-Q3"]);
        assert_eq!(labels(&["Jänner 2022", "Februar 2022", "März 2022"]).unwrap(), ["2022-01", "2022-02", "2022-03"]);
        assert_eq!(labels(&["janvier 2022", "février 2022", "mars 2022"]).unwrap(), ["2022-01", "2022-02", "2022-03"]);
        assert_eq!(labels(&["Jan-24", "Feb-24", "Dec-24"]).unwrap(), ["2024-01", "2024-02", "2024-12"]);
        assert_eq!(labels(&["2024", "Jan 2025", "Feb 2025"]).unwrap(), ["2024", "2025-01", "2025-02"]);
        // Names, month names without a year and bare years alone are no period labels.
        assert_eq!(labels(&["Wien", "Graz", "Linz"]), None);
        assert_eq!(labels(&["Januar", "Februar", "März"]), None);
        assert_eq!(labels(&["2019", "2020", "2021"]), None);
        // A footnote digit is only stripped after a period it continues.
        assert_eq!(labels(&["Apr.201", "Mai.201", "Jun.201"]), None);
    }

    fn data_col(name: &str, values: Vec<Out>, src: u32) -> Column {
        Column {
            source_col: Some(src),
            original_header: vec![(name.to_string(), (0, src))],
            ..derived_column(name.into(), values)
        }
    }

    fn table(columns: Vec<Column>) -> Table {
        let n = columns[0].values.len();
        Table {
            n: 1,
            sheet: "T1".into(),
            source_sheet: "S".into(),
            rect: crate::model::Rect { top: 0, left: 0, bottom: n as u32, right: 1 },
            header_top: 0,
            header_rows: 1,
            title: None,
            notes: Vec::new(),
            confidence: 1.0,
            columns,
            src_rows: (1..=n as u32).collect(),
            row_types: vec![super::super::RowType::Data; n],
            transforms: Default::default(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn adds_a_period_column_after_its_label_column() {
        let col = data_col;
        let mut t = Table {
            n: 1,
            sheet: "T1".into(),
            source_sheet: "S".into(),
            rect: crate::model::Rect { top: 0, left: 0, bottom: 4, right: 1 },
            header_top: 0,
            header_rows: 1,
            title: None,
            notes: Vec::new(),
            confidence: 1.0,
            columns: vec![
                col("Jahr / Monat", text(&["Jän.21", "Feb.21", "Mär.21", "Ø 21"]), 0),
                Column { dtype: Dtype::Number, ..col("Rate", vec![Out::Num(1.0); 4], 1) },
                col("Name", text(&["a", "b", "c", "d"]), 2),
            ],
            src_rows: vec![1, 2, 3, 4],
            row_types: vec![super::super::RowType::Data; 4],
            transforms: Default::default(),
            warnings: Vec::new(),
        };
        add_columns(&mut t);
        let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Jahr / Monat", "Jahr / Monat_period", "Rate", "Name"]);
        assert_eq!(t.columns[1].values[3], Out::Text("2021".into()));
        assert_eq!(t.columns[1].derived_kind(), Some("period"));
        assert_eq!(t.transforms.get(T::PeriodColumnsAdded), 1);
        assert_eq!(derived_name("col_A", "period"), "period");
    }

    #[test]
    fn spelling_variants_get_a_key_column() {
        let mut t = table(vec![
            data_col("Trader", text(&["McKay", "Mc Kay", "McKay", "Smith", "SMITH", "Jones", "Smith", "Jones", "McKay", "Smith"]), 0),
            data_col("Desk", text(&["East", "West", "East", "West", "East", "West", "East", "West", "East", "West"]), 1),
        ]);
        add_columns(&mut t);
        let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Trader", "Trader_key", "Desk"]);
        let keys: Vec<&str> = t.columns[1].values.iter().map(|v| if let Out::Text(s) = v { s.as_str() } else { "" }).collect();
        assert_eq!(keys, ["McKay", "McKay", "McKay", "Smith", "Smith", "Jones", "Smith", "Jones", "McKay", "Smith"]);
        assert_eq!(t.columns[1].derived_kind(), Some("key"));
        assert!(t.warnings[0].contains("'Smith' / 'SMITH'") && t.warnings[0].contains("\"Trader_key\""), "{:?}", t.warnings);
        // Mostly distinct values (a register of descriptions): no key column.
        let mut t = table(vec![data_col("Text", text(&["a b", "A B", "c", "d", "e", "f", "g", "h", "i", "j", "k"]), 0)]);
        add_columns(&mut t);
        assert_eq!(t.columns.len(), 1);
    }
}
