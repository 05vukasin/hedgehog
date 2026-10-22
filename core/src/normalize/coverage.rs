//! Time coverage: which period a table's data covers, from its date / period columns or from
//! period-like column headers (wide tables with years or months as columns). Written to the
//! manifest per table and for the workbook, so an agent can tell "the file has 2015–2017" before
//! it queries anything.

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};

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
    // An index named after its base year ("VPI 2000", "HICP 2015", "Index 2010") is not a
    // period (B46).
    const INDEX: [&str; 14] =
        ["vpi", "hvpi", "cpi", "hicp", "rpi", "ppi", "khpi", "index", "indeks", "indice", "basis", "base", "idx", "lhki"];
    let short = |t: &str| {
        t.chars().count() <= 6
            && t.chars().all(|c| c.is_alphabetic() || c == '.')
            && !INDEX.contains(&t.trim_end_matches('.').to_lowercase().as_str())
    };
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

/// A year written with two digits (Excel's pivot: `00`–`29` → 2000s) or four.
fn short_year(t: &str) -> Option<i32> {
    let t = t.trim().trim_end_matches(['.', ':']);
    if !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let y = match t.len() {
        2 => t.parse::<i32>().ok().map(|v| if v < 30 { 2000 + v } else { 1900 + v })?,
        4 => t.parse::<i32>().ok()?,
        _ => return None,
    };
    (YEAR_MIN..=YEAR_MAX).contains(&y).then_some(y)
}

fn month_period(y: i32, m: u8) -> Period {
    Period { y, m, d: 1, gran: Gran::Month }
}

fn year_period(y: i32) -> Period {
    Period { y, m: 1, d: 1, gran: Gran::Year }
}

/// A rolling range of months, `Mar-May 1971`, `Nov–Jan 2026`, `Dec - Feb 26`: the year is the
/// last month's (ONS labour-market style).
fn month_range(s: &str) -> Option<(Period, Period)> {
    let (head, year) = s.trim().rsplit_once(char::is_whitespace)?;
    let y = short_year(year)?;
    let (a, b) = head.split_once(['-', '–', '—'])?;
    let (m1, m2) = (crate::values::date::month_number(a.trim())?, crate::values::date::month_number(b.trim())?);
    let y1 = if m1 > m2 { y - 1 } else { y };
    Some((month_period(y1, m1), month_period(y, m2)))
}

/// The period a row label names, as its first and last period: what [`period_text`] reads, plus
/// an annual-average label `Ø 22` / `Ø 2012` (the year), a month range `Mar-May 1971` and a bare
/// year among other labels.
fn label_period(s: &str) -> Option<(Period, Period)> {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix(['Ø', '⌀']) {
        let p = year_period(short_year(rest)?);
        return Some((p, p));
    }
    if let Some(r) = month_range(t) {
        return Some(r);
    }
    if t.len() == 4
        && let Some(y) = short_year(t)
    {
        return Some((year_period(y), year_period(y)));
    }
    period_text(t).map(|p| (p, p))
}

/// The sortable form of a label's period: `2022`, `2022-Q1`, `2022-01`, `2024-W05`, `2022-01-15`,
/// or `2025-11/2026-01` for a range.
pub(super) fn span_label(span: &(Period, Period)) -> String {
    if span.0 == span.1 { span.0.label() } else { format!("{}/{}", span.0.label(), span.1.label()) }
}

/// The period of each value of a text label column (`None` for empty cells), when at least 90%
/// of its distinct values name a period ([`label_period`]) and at least three do, not all of
/// them bare years (those are left to the year-column rule). A label that reads as a period only
/// without a trailing footnote digit (`Apr.201` = `Apr.20` + mark `1`) counts when it follows the
/// period above it in the same or the next year. Day labels all on the 1st of a month among
/// month labels (`2012-01-01T00:00:00` next to `Jän.21`) are months.
pub(super) fn column_label_periods(values: &[Out]) -> Option<Vec<Option<(Period, Period)>>> {
    let texts = values.iter().filter(|v| matches!(v, Out::Text(_))).count();
    let mut out: Vec<Option<(Period, Period)>> = Vec::with_capacity(values.len());
    // Each distinct label is read once (a label column of 600k records repeats a few hundred
    // values, B90); the footnote-digit reading below depends on the row above, so it is not kept.
    let mut seen: HashMap<&str, Option<(Period, Period)>> = HashMap::new();
    let (mut failed, mut parsed, mut non_year) = (0usize, 0usize, false);
    let mut prev: Option<(Period, Period)> = None;
    for v in values {
        let Out::Text(s) = v else {
            out.push(None);
            continue;
        };
        let (fresh, mut p) = match seen.entry(s.as_str()) {
            Entry::Occupied(e) => (false, *e.get()),
            Entry::Vacant(e) => (true, *e.insert(label_period(s))),
        };
        if p.is_none()
            && let Some(before) = prev
            && s.trim_end().ends_with(|c: char| c.is_ascii_digit())
        {
            let t = s.trim_end();
            p = label_period(&t[..t.len() - 1])
                .filter(|q| q.0.start() >= before.0.start() && q.0.y <= before.1.y + 1);
        }
        match p {
            Some(q) => {
                if fresh {
                    parsed += 1;
                    non_year |= q.0.gran != Gran::Year || q.0 != q.1 || s.trim().len() != 4;
                }
                prev = Some(q);
            }
            None if fresh => {
                failed += 1;
                if failed * 10 > texts {
                    return None;
                }
            }
            None => {}
        }
        out.push(p);
    }
    if parsed == 0 || !non_year || failed * 10 > seen.len() {
        return None;
    }
    // Days all on the 1st among months (or alone, two or more): months.
    let days: Vec<&Period> = out.iter().flatten().flat_map(|(a, b)| [a, b]).filter(|p| p.gran == Gran::Day).collect();
    let months = out.iter().flatten().any(|(a, _)| a.gran == Gran::Month);
    if !days.is_empty() && days.iter().all(|p| p.d == 1) && (months || days.len() >= 2) {
        for (a, b) in out.iter_mut().flatten() {
            for p in [a, b] {
                if p.gran == Gran::Day {
                    p.gran = Gran::Month;
                }
            }
        }
    }
    Some(out)
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

/// A date column that describes the row's entity rather than the period of the data: a start,
/// hire, birth or expiry date (`agency_start_date`, `Hire Date`, `DOB`, `valid_from`), unless the
/// name also says period, week, month, quarter, year or day (`period_start`, `Week start`).
pub(super) fn attribute_date_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    const PERIOD: &[&str] = &["period", "week", "weekly", "month", "monthly", "quarter", "year", "fiscal", "day", "reporting"];
    const ATTRIBUTE: &[&str] = &[
        "start", "started", "startdate", "hire", "hired", "hiredate", "birth", "born", "dob", "birthdate", "birthday",
        "expiry", "expiration", "expires", "valid", "anniversary", "seniority",
    ];
    !words.iter().any(|w| PERIOD.contains(w)) && words.iter().any(|w| ATTRIBUTE.contains(w))
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
            // Period labels: at least 90% of the distinct values name a period (B70: also `Ø 22`
            // annual rows and `Mar-May 1971` ranges).
            let spans = column_label_periods(values)?;
            Some(spans.into_iter().flatten().flat_map(|(a, b)| [a, b]).collect())
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

/// The period a column header names, as its label (`2025`, `2025-Q1`, `2025-03`, `2025-W05`,
/// `2025-03-31`) and granularity (`year` … `day`): what [`super::header_period`] exposes.
pub(super) fn header_period_label(name: &str) -> Option<(String, &'static str)> {
    header_period(name).map(|p| (p.label(), p.gran.as_str()))
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
    // B49: dates that describe the row's entity (hire, start, birth, expiry) are not the period
    // the data covers; they count only when the table has no other date / period column.
    let mut attribute = (Vec::new(), Vec::new());
    for c in &t.columns {
        if !matches!(c.role, ColRole::Data | ColRole::Group) || c.is_derived() {
            continue;
        }
        if let Some(p) = column_periods(&c.name, c.role, c.dtype, &c.values) {
            if attribute_date_name(&c.name) {
                attribute.0.extend(p);
                attribute.1.push(c.name.clone());
            } else {
                periods.extend(p);
                columns.push(c.name.clone());
            }
        }
    }
    if periods.is_empty() {
        (periods, columns) = attribute;
    }
    // A derived `<name>_period` column (B70) is listed after the column it was read from.
    let derived: Vec<(usize, String)> = t
        .columns
        .windows(2)
        .filter(|w| w[1].derived_kind() == Some("period"))
        .filter_map(|w| columns.iter().position(|n| *n == w[0].name).map(|k| (k + 1, w[1].name.clone())))
        .collect();
    for (k, name) in derived.into_iter().rev() {
        columns.insert(k, name);
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

    #[test]
    fn attribute_dates() {
        for n in ["agency_start_date", "Hire Date", "DOB", "valid_from", "Date of birth", "expiration_date", "StartDate"] {
            assert!(attribute_date_name(n), "{n}");
        }
        for n in ["Date", "period_start", "Week start", "fiscal_year", "payment_date", "Startup costs date"] {
            assert!(!attribute_date_name(n), "{n}");
        }
    }
}
