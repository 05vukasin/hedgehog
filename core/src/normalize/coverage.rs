//! Time coverage: which period a table's data covers, from its date / period columns or from
//! period-like column headers (wide tables with years or months as columns). Written to the
//! manifest per table and for the workbook, so an agent can tell "the file has 2015–2017" before
//! it queries anything.

use std::collections::{BTreeSet, HashSet};

use super::json::Json;
use super::{ColRole, Dtype, Out, Table};
use crate::values::date::is_period_label;
use crate::values::{DatePrecision, norm_key, parse_date, serial_to_ymd, ymd_to_serial};

/// Granularity of a period, finest first (so `min` is the finest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Gran {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl Gran {
    fn as_str(self) -> &'static str {
        match self {
            Gran::Day => "day",
            Gran::Week => "week",
            Gran::Month => "month",
            Gran::Quarter => "quarter",
            Gran::Year => "year",
        }
    }
}

/// One period: its first day and granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Period {
    y: i32,
    m: u8,
    d: u8,
    gran: Gran,
}

impl Period {
    fn start(&self) -> (i32, u8, u8) {
        (self.y, self.m, self.d)
    }

    /// Last day of the period (months counted as 31 days; only used for ordering).
    fn end(&self) -> (i32, u8, u8) {
        match self.gran {
            Gran::Day => (self.y, self.m, self.d),
            Gran::Week => {
                serial_to_ymd(ymd_to_serial(self.y, self.m, self.d) + 6.0).unwrap_or(self.start())
            }
            Gran::Month => (self.y, self.m, 31),
            Gran::Quarter => (self.y, self.m + 2, 31),
            Gran::Year => (self.y, 12, 31),
        }
    }

    fn label(&self) -> String {
        match self.gran {
            Gran::Day => format!("{:04}-{:02}-{:02}", self.y, self.m, self.d),
            Gran::Week => {
                // ISO week-numbering year and week of the Monday.
                let serial = ymd_to_serial(self.y, self.m, self.d);
                let (iy, _, _) = serial_to_ymd(serial + 3.0).unwrap_or(self.start());
                let jan4 = ymd_to_serial(iy, 1, 4);
                let week1 = jan4 - ((jan4 as i64 - 2).rem_euclid(7)) as f64; // Monday (serial 2 = Monday)
                let w = ((serial - week1) / 7.0).floor() as i64 + 1;
                format!("{iy:04}-W{w:02}")
            }
            Gran::Month => format!("{:04}-{:02}", self.y, self.m),
            Gran::Quarter => format!("{:04}-Q{}", self.y, (self.m - 1) / 3 + 1),
            Gran::Year => format!("{:04}", self.y),
        }
    }
}

/// Time coverage of a table or workbook.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Coverage {
    from: Period,
    to: Period,
    gran: Gran,
    /// `column` (values of date / period columns) or `header` (period-like column headers).
    source: &'static str,
    columns: Vec<String>,
    tables: Vec<String>,
}

impl Coverage {
    pub(super) fn json(&self, workbook: bool) -> Json {
        let j = Json::obj()
            .with("from", self.from.label())
            .with("to", self.to.label())
            .with("granularity", self.gran.as_str())
            .with("source", self.source)
            .with("columns", self.columns.clone());
        if workbook {
            j.with("tables", self.tables.clone())
        } else {
            j
        }
    }

    pub(super) fn public(&self) -> super::TimeCoverage {
        super::TimeCoverage {
            from: self.from.label(),
            to: self.to.label(),
            granularity: self.gran.as_str(),
            source: self.source,
            columns: self.columns.clone(),
            tables: self.tables.clone(),
        }
    }
}

const YEAR_MIN: i32 = 1900;
const YEAR_MAX: i32 = 2100;

fn is_year(v: f64) -> bool {
    v.fract() == 0.0 && (YEAR_MIN as f64..=YEAR_MAX as f64).contains(&v)
}

/// Column names that say "year": year, Jahr, année, año, anno, år, rok, jaar, godina…
fn year_name(name: &str) -> bool {
    let key = norm_key(name);
    if key.contains("year") || key.contains("jahr") || key.contains("annee") {
        return true;
    }
    key.split(|c: char| !c.is_alphanumeric()).any(|t| {
        matches!(
            t,
            "ano"
                | "anno"
                | "anni"
                | "anos"
                | "ar"
                | "aar"
                | "rok"
                | "lata"
                | "jaar"
                | "godina"
                | "yr"
                | "fy"
                | "vuosi"
                | "annees"
                | "years"
        )
    })
}

/// A 4-digit year as text, optionally with a short word ("RA 2023", "2023 Ist", "FY 2024").
fn year_part(s: &str) -> Option<i32> {
    let toks: Vec<&str> = s.split_whitespace().collect();
    let year = |t: &str| -> Option<i32> {
        let t = t.trim_end_matches(['.', ':', ',']);
        (t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit()))
            .then(|| t.parse::<i32>().ok())
            .flatten()
            .filter(|y| (YEAR_MIN..=YEAR_MAX).contains(y))
    };
    let short =
        |t: &str| t.chars().count() <= 6 && t.chars().all(|c| c.is_alphabetic() || c == '.');
    match toks.as_slice() {
        [y] => year(y),
        [w, y] if short(w) => year(y),
        [y, w] if short(w) => year(y),
        _ => None,
    }
}

fn from_parsed(p: crate::values::ParsedDate) -> Period {
    let gran = match p.precision {
        DatePrecision::Day => Gran::Day,
        DatePrecision::Month => Gran::Month,
        DatePrecision::Quarter(_) => Gran::Quarter,
        DatePrecision::Week(_) => Gran::Week,
    };
    Period {
        y: p.year,
        m: p.month,
        d: p.day,
        gran,
    }
}

/// A period written as text: `2024 Q1`, `Jan 2024`, `2024-03`, `2024-W05`, `12.03.2024`.
fn period_text(s: &str) -> Option<Period> {
    let p = parse_date(s, None)?;
    (YEAR_MIN..=YEAR_MAX)
        .contains(&p.year)
        .then(|| from_parsed(p))
}

/// Periods of a date-typed column, with the granularity the values show (all on the first of a
/// month → month; also all in January → year; all in Jan/Apr/Jul/Oct → quarter).
fn date_column(values: &[Out]) -> Option<Vec<Period>> {
    let mut days: BTreeSet<(i32, u8, u8)> = BTreeSet::new();
    for v in values {
        if let Out::Date(x) = v
            && let Some(ymd) = serial_to_ymd(*x)
        {
            days.insert(ymd);
        }
    }
    if days.is_empty() {
        return None;
    }
    let first_of_month = days.len() >= 2 && days.iter().all(|d| d.2 == 1);
    let gran = if !first_of_month {
        Gran::Day
    } else if days.iter().all(|d| d.1 == 1) {
        Gran::Year
    } else if days.iter().all(|d| matches!(d.1, 1 | 4 | 7 | 10)) {
        Gran::Quarter
    } else {
        Gran::Month
    };
    Some(
        days.into_iter()
            .map(|(y, m, d)| Period { y, m, d, gran })
            .collect(),
    )
}

/// Periods from a column's values, if it is a date / period column.
fn column_periods(name: &str, role: ColRole, dtype: Dtype, values: &[Out]) -> Option<Vec<Period>> {
    match dtype {
        Dtype::Date | Dtype::Datetime => date_column(values),
        Dtype::Integer | Dtype::Number if year_name(name) => {
            let nums: Vec<f64> = values
                .iter()
                .filter_map(|v| if let Out::Num(x) = v { Some(*x) } else { None })
                .collect();
            (!nums.is_empty() && nums.iter().all(|&x| is_year(x))).then(|| {
                nums.iter()
                    .map(|&x| Period {
                        y: x as i32,
                        m: 1,
                        d: 1,
                        gran: Gran::Year,
                    })
                    .collect()
            })
        }
        Dtype::Text => {
            let texts: Vec<&str> = values
                .iter()
                .filter_map(|v| {
                    if let Out::Text(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            if texts.is_empty() {
                return None;
            }
            // Bare years as text: only in a year-named column or the group column (years as
            // group-header rows).
            if year_name(name) || role == ColRole::Group {
                let years: Vec<i32> = texts
                    .iter()
                    .filter_map(|s| year_part(s).filter(|_| s.trim().len() == 4))
                    .collect();
                if years.len() == texts.len() {
                    return Some(
                        years
                            .into_iter()
                            .map(|y| Period {
                                y,
                                m: 1,
                                d: 1,
                                gran: Gran::Year,
                            })
                            .collect(),
                    );
                }
            }
            if role == ColRole::Group {
                return None;
            }
            // Period text: at least 90% of the distinct values parse. Stops as soon as more than
            // a tenth of all values failed (then a tenth of the distinct ones fails too).
            let mut seen: HashSet<&str> = HashSet::new();
            let mut periods: Vec<Period> = Vec::new();
            let mut failed = 0;
            for s in &texts {
                if !seen.insert(s) {
                    continue;
                }
                match period_text(s) {
                    Some(p) => periods.push(p),
                    None => {
                        failed += 1;
                        if failed * 10 > texts.len() {
                            return None;
                        }
                    }
                }
            }
            (periods.len() * 10 >= seen.len() * 9).then_some(periods)
        }
        _ => None,
    }
}

/// Period named by a column header (its flattened name, parts joined with ` / `): a period
/// (`Jan 2024`, `2024 Q1`), a month or quarter label combined with a year part (`Oktober / 2022`),
/// or a year (`2019`, `RA 2023`).
fn header_period(name: &str) -> Option<Period> {
    // Without the " (2)" that makes a duplicate name unique.
    let name = match name.rsplit_once(" (") {
        Some((head, tail))
            if tail.ends_with(')')
                && tail[..tail.len() - 1].bytes().all(|b| b.is_ascii_digit()) =>
        {
            head
        }
        _ => name,
    };
    let parts: Vec<&str> = name
        .split(" / ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let year = parts.iter().find_map(|p| year_part(p));
    let label = parts.iter().find(|p| is_period_label(p));
    // "2022" alone is not a date for the parser; a period with a year is.
    if let Some(p) = parts
        .iter()
        .filter(|p| p.bytes().any(|b| b.is_ascii_digit()))
        .find_map(|p| period_text(p))
    {
        return Some(p);
    }
    if let (Some(l), Some(y)) = (label, year)
        && let Some(p) = period_text(&format!("{l} {y}"))
    {
        return Some(p);
    }
    year.map(|y| Period {
        y,
        m: 1,
        d: 1,
        gran: Gran::Year,
    })
}

fn span(periods: &[Period]) -> Option<(Period, Period, Gran)> {
    let from = *periods.iter().min_by_key(|p| (p.start(), p.gran))?;
    let to = *periods
        .iter()
        .max_by_key(|p| (p.end(), std::cmp::Reverse(p.gran)))?;
    let gran = periods.iter().map(|p| p.gran).min()?;
    Some((from, to, gran))
}

/// Time coverage of one table: from its date / period columns, or else from period-like headers
/// (two or more, or one that is more than a bare year).
pub(super) fn table(t: &Table) -> Option<Coverage> {
    let mut periods = Vec::new();
    let mut columns = Vec::new();
    for c in &t.columns {
        if !matches!(c.role, ColRole::Data | ColRole::Group) {
            continue;
        }
        if let Some(p) = column_periods(&c.name, c.role, c.dtype, &c.values) {
            periods.extend(p);
            columns.push(c.name.clone());
        }
    }
    let mut source = "column";
    if periods.is_empty() {
        source = "header";
        for c in t
            .columns
            .iter()
            .filter(|c| c.role == ColRole::Data && !c.original_header.is_empty())
        {
            if let Some(p) = header_period(&c.name) {
                periods.push(p);
                columns.push(c.name.clone());
            }
        }
        let only_years = periods.iter().all(|p| p.gran == Gran::Year);
        if periods.len() < 2 && only_years {
            return None;
        }
    }
    let (from, to, gran) = span(&periods)?;
    Some(Coverage {
        from,
        to,
        gran,
        source,
        columns,
        tables: vec![t.id()],
    })
}

/// Workbook coverage: the span over all tables' coverage, at the finest granularity found;
/// `source` is `column` when any table's coverage comes from column values.
pub(super) fn workbook(tables: &[(String, Coverage)]) -> Option<Coverage> {
    let ends: Vec<Period> = tables.iter().flat_map(|(_, c)| [c.from, c.to]).collect();
    let (from, to, _) = span(&ends)?;
    let gran = tables.iter().map(|(_, c)| c.gran).min()?;
    let source = if tables.iter().any(|(_, c)| c.source == "column") {
        "column"
    } else {
        "header"
    };
    let mut columns = Vec::new();
    for (id, c) in tables {
        columns.extend(c.columns.iter().map(|n| format!("{id}.{n}")));
    }
    Some(Coverage {
        from,
        to,
        gran,
        source,
        columns,
        tables: tables.iter().map(|(id, _)| id.clone()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(p: Option<Period>) -> Option<String> {
        p.map(|p| format!("{}:{}", p.label(), p.gran.as_str()))
    }

    #[test]
    fn header_periods() {
        assert_eq!(
            label(header_period("Oktober / 2022")),
            Some("2022-10:month".into())
        );
        assert_eq!(
            label(header_period("November / 2021 (2)")),
            Some("2021-11:month".into())
        );
        assert_eq!(label(header_period("2019")), Some("2019:year".into()));
        assert_eq!(
            label(header_period("Umsatz / RA 2023")),
            Some("2023:year".into())
        );
        assert_eq!(
            label(header_period("2024 Q1")),
            Some("2024-Q1:quarter".into())
        );
        assert_eq!(
            label(header_period("Jan 2024")),
            Some("2024-01:month".into())
        );
        assert_eq!(
            label(header_period("2024-03")),
            Some("2024-03:month".into())
        );
        assert_eq!(label(header_period("Revenue / Q1")), None);
        assert_eq!(label(header_period("Wanderungen 1950 bis 2024")), None);
        assert_eq!(label(header_period("Code 12345")), None);
    }

    #[test]
    fn period_labels() {
        let w = Period {
            y: 2024,
            m: 1,
            d: 29,
            gran: Gran::Week,
        };
        assert_eq!(w.label(), "2024-W05");
        let w = Period {
            y: 2020,
            m: 12,
            d: 28,
            gran: Gran::Week,
        };
        assert_eq!(w.label(), "2020-W53");
        assert!(
            year_name("Berichtsjahr") && year_name("Année") && year_name("Año") && year_name("År")
        );
        assert!(!year_name("Betrag") && !year_name("Artikal"));
    }

    #[test]
    fn date_granularity() {
        let d = |y, m, dd| Out::Date(ymd_to_serial(y, m, dd));
        let g = |v: Vec<Out>| date_column(&v).map(|p| p[0].gran);
        assert_eq!(g(vec![d(2020, 1, 1), d(2020, 2, 1)]), Some(Gran::Month));
        assert_eq!(g(vec![d(2020, 1, 1), d(2021, 1, 1)]), Some(Gran::Year));
        assert_eq!(g(vec![d(2020, 1, 1), d(2020, 4, 1)]), Some(Gran::Quarter));
        assert_eq!(g(vec![d(2020, 1, 1), d(2020, 1, 2)]), Some(Gran::Day));
        assert_eq!(g(vec![d(2020, 1, 1)]), Some(Gran::Day));
    }
}
