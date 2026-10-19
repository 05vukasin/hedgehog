//! Table-driven tests per locale preset: numbers and dates as people write them in the US, UK,
//! Germany, France, Spain, Italy, the Netherlands, Switzerland and Serbia.

use hh_core::values::{parse_date, parse_number, parse_time, DatePrecision, Locale};

fn check_numbers(loc: Locale, cases: &[(&str, f64, Option<&str>)]) {
    assert!(cases.len() >= 15, "{loc:?}: need ≥15 number cases");
    for &(s, want, cur) in cases {
        let p = parse_number(s, loc.number_hint()).unwrap_or_else(|| panic!("{loc:?}: {s:?} not parsed"));
        let got = p.normalized();
        assert!((got - want).abs() <= 1e-9 * want.abs().max(1.0), "{loc:?}: {s:?} → {got}, want {want}");
        if let Some(c) = cur {
            assert_eq!(p.currency, Some(c), "{loc:?}: {s:?} currency");
        }
    }
}

/// (text, year, month, day); month-precision cases use day 1.
fn check_dates(loc: Locale, cases: &[(&str, i32, u8, u8)]) {
    assert!(cases.len() >= 15, "{loc:?}: need ≥15 date cases");
    for &(s, y, m, d) in cases {
        let p = parse_date(s, loc.date_order()).unwrap_or_else(|| panic!("{loc:?}: {s:?} not parsed"));
        assert_eq!((p.year, p.month, p.day), (y, m, d), "{loc:?}: {s:?}");
        assert!(!p.ambiguous, "{loc:?}: {s:?} flagged ambiguous despite locale");
    }
}

#[test]
fn us() {
    check_numbers(
        Locale::Us,
        &[
            ("1,234.56", 1234.56, None),
            ("1,234", 1234.0, None),
            ("1.234", 1.234, None),
            ("$1,234.56", 1234.56, Some("USD")),
            ("-$500", -500.0, Some("USD")),
            ("$(500)", -500.0, Some("USD")),
            ("($1,200.00)", -1200.0, Some("USD")),
            ("$1.2M", 1.2e6, Some("USD")),
            ("1.2MM", 1.2e6, None),
            ("$5k", 5000.0, Some("USD")),
            ("USD 1,000.00", 1000.0, Some("USD")),
            ("US$ 99.99", 99.99, Some("USD")),
            ("12.5%", 0.125, None),
            ("1,000,000", 1e6, None),
            ("3.5 lbs", 3.5, None),
            ("$2.5bn", 2.5e9, Some("USD")),
            ("(1,234.56)", -1234.56, None),
            ("$3m", 3e6, Some("USD")),
        ],
    );
    check_dates(
        Locale::Us,
        &[
            ("12/31/2024", 2024, 12, 31),
            ("1/2/2024", 2024, 1, 2),
            ("01/02/24", 2024, 1, 2),
            ("12/31/99", 1999, 12, 31),
            ("12-31-2024", 2024, 12, 31),
            ("Dec 31, 2024", 2024, 12, 31),
            ("December 31st, 2024", 2024, 12, 31),
            ("31-Dec-2024", 2024, 12, 31),
            ("Tuesday, December 31, 2024", 2024, 12, 31),
            ("2024-12-31", 2024, 12, 31),
            ("12/31/2024 6:00 PM", 2024, 12, 31),
            ("Dec 2024", 2024, 12, 1),
            ("Q4 2024", 2024, 10, 1),
            ("4th quarter 2024", 2024, 10, 1),
            ("July 4, 1976", 1976, 7, 4),
            ("Mar 1st 2024", 2024, 3, 1),
        ],
    );
    assert_eq!(parse_date("12/31/2024 6:00 PM", Locale::Us.date_order()).unwrap().time, Some(18 * 3600));
}

#[test]
fn uk() {
    check_numbers(
        Locale::Uk,
        &[
            ("£1,234.56", 1234.56, Some("GBP")),
            ("1,234.56", 1234.56, None),
            ("1,234", 1234.0, None),
            ("-£50", -50.0, Some("GBP")),
            ("£(50)", -50.0, Some("GBP")),
            ("£2.5m", 2.5e6, Some("GBP")),
            ("£3bn", 3e9, Some("GBP")),
            ("£500k", 500e3, Some("GBP")),
            ("GBP 1,000", 1000.0, Some("GBP")),
            ("12.5%", 0.125, None),
            ("0.5", 0.5, None),
            ("1,000,000", 1e6, None),
            ("(1,234)", -1234.0, None),
            ("7.5 kg", 7.5, None),
            ("€1,234.56", 1234.56, Some("EUR")),
            ("1.234", 1.234, None),
        ],
    );
    check_dates(
        Locale::Uk,
        &[
            ("31/12/2024", 2024, 12, 31),
            ("01/02/2024", 2024, 2, 1),
            ("1/2/24", 2024, 2, 1),
            ("31-12-2024", 2024, 12, 31),
            ("31 Dec 2024", 2024, 12, 31),
            ("31 December 2024", 2024, 12, 31),
            ("31st December 2024", 2024, 12, 31),
            ("Tuesday 31st December 2024", 2024, 12, 31),
            ("the 31st of December 2024", 2024, 12, 31),
            ("1st Jan 2024", 2024, 1, 1),
            ("2024-12-31", 2024, 12, 31),
            ("Dec-24", 2024, 12, 1),
            ("Q1 2025", 2025, 1, 1),
            ("31/12/2024 18:00", 2024, 12, 31),
            ("31.12.2024", 2024, 12, 31),
            ("3rd March 2024", 2024, 3, 3),
        ],
    );
}

#[test]
fn de() {
    check_numbers(
        Locale::De,
        &[
            ("1.234,56", 1234.56, None),
            ("1.234,56 €", 1234.56, Some("EUR")),
            ("-1.234,56", -1234.56, None),
            ("1.234,56-", -1234.56, None),
            ("1.234", 1234.0, None),
            ("0,5", 0.5, None),
            ("12,5 %", 0.125, None),
            ("€ 1.234,56", 1234.56, Some("EUR")),
            ("1.234.567,89", 1234567.89, None),
            ("1,5 Mio.", 1.5e6, None),
            ("2 Mrd.", 2e9, None),
            ("3 Tsd.", 3000.0, None),
            ("EUR 1.000,00", 1000.0, Some("EUR")),
            ("1 234,56", 1234.56, None),
            ("500,- €", 500.0, Some("EUR")),
            ("12 Stk.", 12.0, None),
            ("(1.234,56)", -1234.56, None),
        ],
    );
    check_dates(
        Locale::De,
        &[
            ("31.12.2024", 2024, 12, 31),
            ("1.1.2024", 2024, 1, 1),
            ("31.12.24", 2024, 12, 31),
            ("31. Dezember 2024", 2024, 12, 31),
            ("31. Dez. 2024", 2024, 12, 31),
            ("1. März 2024", 2024, 3, 1),
            ("1. Marz 2024", 2024, 3, 1),
            ("Di, 31.12.2024", 2024, 12, 31),
            ("Dienstag, 31. Dezember 2024", 2024, 12, 31),
            ("2024-12-31", 2024, 12, 31),
            ("Dezember 2024", 2024, 12, 1),
            ("Mär 2024", 2024, 3, 1),
            ("3. Quartal 2024", 2024, 7, 1),
            ("Q3/2024", 2024, 7, 1),
            ("KW 5/2024", 2024, 1, 29),
            ("31.12.2024 18:00 Uhr", 2024, 12, 31),
            ("am 31.12.2024", 2024, 12, 31),
        ],
    );
}

#[test]
fn fr() {
    check_numbers(
        Locale::Fr,
        &[
            ("1\u{202F}234,56 €", 1234.56, Some("EUR")),
            ("1\u{A0}234,56", 1234.56, None),
            ("1 234,56", 1234.56, None),
            ("1234,56", 1234.56, None),
            ("-1 234,56", -1234.56, None),
            ("12,5 %", 0.125, None),
            ("0,5", 0.5, None),
            ("1 234 567,89", 1234567.89, None),
            ("€ 1 234,56", 1234.56, Some("EUR")),
            ("1 234,56 EUR", 1234.56, Some("EUR")),
            ("(1 234,56)", -1234.56, None),
            ("3 kg", 3.0, None),
            ("1,2 million", 1.2e6, None),
            ("1,5 milliard", 1.5e9, None),
            ("15 €", 15.0, Some("EUR")),
            ("1.234", 1234.0, None),
        ],
    );
    check_dates(
        Locale::Fr,
        &[
            ("31/12/2024", 2024, 12, 31),
            ("1/2/2024", 2024, 2, 1),
            ("31/12/24", 2024, 12, 31),
            ("31 décembre 2024", 2024, 12, 31),
            ("31 decembre 2024", 2024, 12, 31),
            ("1er janvier 2024", 2024, 1, 1),
            ("1 févr. 2024", 2024, 2, 1),
            ("mardi 31 décembre 2024", 2024, 12, 31),
            ("31-12-2024", 2024, 12, 31),
            ("2024-12-31", 2024, 12, 31),
            ("décembre 2024", 2024, 12, 1),
            ("T3 2024", 2024, 7, 1),
            ("3e trimestre 2024", 2024, 7, 1),
            ("31/12/2024 18h30", 2024, 12, 31),
            ("août 2024", 2024, 8, 1),
            ("15 aout 2024", 2024, 8, 15),
        ],
    );
    assert_eq!(parse_date("31/12/2024 18h30", Locale::Fr.date_order()).unwrap().time, Some(18 * 3600 + 1800));
}

#[test]
fn es() {
    check_numbers(
        Locale::Es,
        &[
            ("1.234,56 €", 1234.56, Some("EUR")),
            ("1.234,56", 1234.56, None),
            ("-1.234,56", -1234.56, None),
            ("1.234", 1234.0, None),
            ("0,5", 0.5, None),
            ("12,5 %", 0.125, None),
            ("€1.234,56", 1234.56, Some("EUR")),
            ("1.234.567,89", 1234567.89, None),
            ("1,5 millones", 1.5e6, None),
            ("(1.234,56)", -1234.56, None),
            ("25 kg", 25.0, None),
            ("US$ 1.234,56", 1234.56, Some("USD")),
            ("100 €", 100.0, Some("EUR")),
            ("1.234,56 EUR", 1234.56, Some("EUR")),
            ("5 uds.", 5.0, None),
        ],
    );
    check_dates(
        Locale::Es,
        &[
            ("31/12/2024", 2024, 12, 31),
            ("1/2/2024", 2024, 2, 1),
            ("31-12-2024", 2024, 12, 31),
            ("31/12/24", 2024, 12, 31),
            ("31 de diciembre de 2024", 2024, 12, 31),
            ("1 de enero de 2024", 2024, 1, 1),
            ("31-dic-2024", 2024, 12, 31),
            ("martes, 31 de diciembre de 2024", 2024, 12, 31),
            ("diciembre 2024", 2024, 12, 1),
            ("dic. 2024", 2024, 12, 1),
            ("2024-12-31", 2024, 12, 31),
            ("T4 2024", 2024, 10, 1),
            ("4º trimestre 2024", 2024, 10, 1),
            ("31/12/2024 18:00", 2024, 12, 31),
            ("15 ago 2024", 2024, 8, 15),
            ("miércoles 1 de mayo de 2024", 2024, 5, 1),
        ],
    );
}

#[test]
fn it() {
    check_numbers(
        Locale::It,
        &[
            ("1.234,56 €", 1234.56, Some("EUR")),
            ("€ 1.234,56", 1234.56, Some("EUR")),
            ("-1.234,56", -1234.56, None),
            ("1.234", 1234.0, None),
            ("0,5", 0.5, None),
            ("12,5%", 0.125, None),
            ("1.234.567,89", 1234567.89, None),
            ("(1.234,56)", -1234.56, None),
            ("1,5 mln", 1.5e6, None),
            ("2 mld", 2e9, None),
            ("25 kg", 25.0, None),
            ("100 €", 100.0, Some("EUR")),
            ("1.234,56 EUR", 1234.56, Some("EUR")),
            ("3 pz", 3.0, None),
            ("1.000,-", 1000.0, None),
        ],
    );
    check_dates(
        Locale::It,
        &[
            ("31/12/2024", 2024, 12, 31),
            ("1/2/2024", 2024, 2, 1),
            ("31-12-2024", 2024, 12, 31),
            ("31.12.2024", 2024, 12, 31),
            ("31 dicembre 2024", 2024, 12, 31),
            ("1 gennaio 2024", 2024, 1, 1),
            ("31 dic 2024", 2024, 12, 31),
            ("martedì 31 dicembre 2024", 2024, 12, 31),
            ("dicembre 2024", 2024, 12, 1),
            ("gen 2024", 2024, 1, 1),
            ("2024-12-31", 2024, 12, 31),
            ("Q1 2024", 2024, 1, 1),
            ("III trimestre 2024", 2024, 7, 1),
            ("31/12/2024 18:00", 2024, 12, 31),
            ("il 31 dicembre 2024", 2024, 12, 31),
            ("15 agosto 2024", 2024, 8, 15),
        ],
    );
}

#[test]
fn nl() {
    check_numbers(
        Locale::Nl,
        &[
            ("€ 1.234,56", 1234.56, Some("EUR")),
            ("1.234,56", 1234.56, None),
            ("€ -1.234,56", -1234.56, Some("EUR")),
            ("-€ 1.234,56", -1234.56, Some("EUR")),
            ("1.234", 1234.0, None),
            ("0,5", 0.5, None),
            ("12,5%", 0.125, None),
            ("1.234.567,89", 1234567.89, None),
            ("1,5 mln", 1.5e6, None),
            ("€ 1.234,-", 1234.0, Some("EUR")),
            ("25 kg", 25.0, None),
            ("EUR 1.000", 1000.0, Some("EUR")),
            ("(1.234,56)", -1234.56, None),
            ("2 mld", 2e9, None),
            ("1.234,56-", -1234.56, None),
        ],
    );
    check_dates(
        Locale::Nl,
        &[
            ("31-12-2024", 2024, 12, 31),
            ("31-12-24", 2024, 12, 31),
            ("1-2-2024", 2024, 2, 1),
            ("31/12/2024", 2024, 12, 31),
            ("31 december 2024", 2024, 12, 31),
            ("1 januari 2024", 2024, 1, 1),
            ("31 dec 2024", 2024, 12, 31),
            ("1 mrt 2024", 2024, 3, 1),
            ("dinsdag 31 december 2024", 2024, 12, 31),
            ("december 2024", 2024, 12, 1),
            ("maart 2024", 2024, 3, 1),
            ("2024-12-31", 2024, 12, 31),
            ("Q4 2024", 2024, 10, 1),
            ("4e kwartaal 2024", 2024, 10, 1),
            ("31-12-2024 18:00", 2024, 12, 31),
            ("1 mei 2024", 2024, 5, 1),
        ],
    );
}

#[test]
fn ch() {
    check_numbers(
        Locale::Ch,
        &[
            ("CHF 1'234.50", 1234.5, Some("CHF")),
            ("1'234.50", 1234.5, None),
            ("Fr. 1'234.50", 1234.5, Some("CHF")),
            ("1’234.50", 1234.5, None),
            ("-1'234.50", -1234.5, None),
            ("1'234'567.89", 1234567.89, None),
            ("1234.5", 1234.5, None),
            ("0.5", 0.5, None),
            ("12.5%", 0.125, None),
            ("CHF 1'000.-", 1000.0, Some("CHF")),
            ("1'234.50 CHF", 1234.5, Some("CHF")),
            ("(1'234.50)", -1234.5, None),
            ("1.5 Mio.", 1.5e6, None),
            ("2 Mrd.", 2e9, None),
            ("25 kg", 25.0, None),
            ("SFr. 99.90", 99.9, Some("CHF")),
        ],
    );
    check_dates(
        Locale::Ch,
        &[
            ("31.12.2024", 2024, 12, 31),
            ("1.2.2024", 2024, 2, 1),
            ("31.12.24", 2024, 12, 31),
            ("31. Dezember 2024", 2024, 12, 31),
            ("1er janvier 2024", 2024, 1, 1),
            ("31 décembre 2024", 2024, 12, 31),
            ("2024-12-31", 2024, 12, 31),
            ("Dezember 2024", 2024, 12, 1),
            ("Q4 2024", 2024, 10, 1),
            ("31.12.2024 18:00", 2024, 12, 31),
            ("Di, 31.12.2024", 2024, 12, 31),
            ("3. Quartal 2024", 2024, 7, 1),
            ("1. März 2024", 2024, 3, 1),
            ("31/12/2024", 2024, 12, 31),
            ("KW 1 2024", 2024, 1, 1),
        ],
    );
}

#[test]
fn rs() {
    check_numbers(
        Locale::Rs,
        &[
            ("1.234,56", 1234.56, None),
            ("1.234,56 RSD", 1234.56, Some("RSD")),
            ("1.234,56 din.", 1234.56, Some("RSD")),
            ("-1.234,56", -1234.56, None),
            ("1.234", 1234.0, None),
            ("0,5", 0.5, None),
            ("12,5%", 0.125, None),
            ("1.234.567,89", 1234567.89, None),
            ("1,5 mil.", 1.5e6, None),
            ("2 mlrd", 2e9, None),
            ("1.200,-", 1200.0, None),
            ("(1.234,56)", -1234.56, None),
            ("3 kom", 3.0, None),
            ("1.234,56 дин.", 1234.56, Some("RSD")),
            ("15 €", 15.0, Some("EUR")),
        ],
    );
    check_dates(
        Locale::Rs,
        &[
            ("31.12.2024.", 2024, 12, 31),
            ("31.12.2024", 2024, 12, 31),
            ("1.2.2024.", 2024, 2, 1),
            ("31. decembar 2024.", 2024, 12, 31),
            ("31. децембар 2024.", 2024, 12, 31),
            ("1. januara 2024. godine", 2024, 1, 1),
            ("decembar 2024.", 2024, 12, 1),
            ("2024-12-31", 2024, 12, 31),
            ("III kvartal 2024", 2024, 7, 1),
            ("IV квартал 2024", 2024, 10, 1),
            ("31.12.2024. 18:00", 2024, 12, 31),
            ("utorak, 31. decembar 2024.", 2024, 12, 31),
            ("31/12/2024", 2024, 12, 31),
            ("dec 2024", 2024, 12, 1),
            ("15. avgusta 2024.", 2024, 8, 15),
        ],
    );
}

#[test]
fn other_european_month_names() {
    for (s, y, m, d) in [
        ("1 de janeiro de 2024", 2024, 1, 1),
        ("31 de dezembro de 2024", 2024, 12, 31),
        ("15 março 2024", 2024, 3, 15),
        ("31 grudnia 2024 r.", 2024, 12, 31),
        ("1 stycznia 2024", 2024, 1, 1),
        ("15 sierpnia 2024", 2024, 8, 15),
        ("1. siječnja 2024.", 2024, 1, 1),
        ("31. prosinca 2024.", 2024, 12, 31),
        ("1. marec 2024", 2024, 3, 1),
        ("31. Jänner 2024", 2024, 1, 31),
    ] {
        let p = parse_date(s, None).unwrap_or_else(|| panic!("{s:?}"));
        assert_eq!((p.year, p.month, p.day), (y, m, d), "{s:?}");
    }
    // listopad is November in Polish but October in Croatian: deliberately not a month.
    assert_eq!(parse_date("1 listopada 2024", None), None);
}

#[test]
fn iso_weeks() {
    for (s, y, m, d) in [
        ("2024-W05", 2024, 1, 29),
        ("2024W05", 2024, 1, 29),
        ("2024-W05-3", 2024, 1, 31),
        ("2025-W01", 2024, 12, 30),
        ("2020-W53", 2020, 12, 28),
        ("W05 2024", 2024, 1, 29),
        ("week 5 2024", 2024, 1, 29),
    ] {
        let p = parse_date(s, None).unwrap_or_else(|| panic!("{s:?}"));
        assert_eq!((p.year, p.month, p.day), (y, m, d), "{s:?}");
        assert!(matches!(p.precision, DatePrecision::Week(_)), "{s:?}");
    }
    assert_eq!(parse_date("2024-W53", None), None);
    assert_eq!(parse_date("2024-W00", None), None);
}

#[test]
fn times() {
    for (s, secs) in [
        ("6:00", 6 * 3600),
        ("06:00", 6 * 3600),
        ("6:00 PM", 18 * 3600),
        ("6:00 pm", 18 * 3600),
        ("6 pm", 18 * 3600),
        ("12:00 AM", 0),
        ("12:30 PM", 12 * 3600 + 1800),
        ("6:00 a.m.", 6 * 3600),
        ("18:00:00", 18 * 3600),
        ("18:00:30.5", 18 * 3600 + 30),
        ("18:00 Uhr", 18 * 3600),
        ("18h30", 18 * 3600 + 1800),
        ("18h", 18 * 3600),
        ("23:59", 23 * 3600 + 59 * 60),
    ] {
        assert_eq!(parse_time(s), Some(secs), "{s:?}");
    }
    for s in ["24:00", "6:60", "13:00 PM", "6", "abc", "1:2", "2024-12-31", "6:00:00:00", "h30"] {
        assert_eq!(parse_time(s), None, "{s:?}");
    }
}

#[test]
fn ambiguity_is_symmetric_and_flagged() {
    // No hint, no evidence: tie-break reading, flagged.
    let p = parse_number("1.200", Default::default()).unwrap();
    assert!(p.ambiguous);
    let p = parse_number("1,200", Default::default()).unwrap();
    assert!(p.ambiguous);
    // Evidence in either direction is decisive.
    assert_eq!(parse_number("1.200", Locale::De.number_hint()).unwrap().value, 1200.0);
    assert_eq!(parse_number("1.200", Locale::Us.number_hint()).unwrap().value, 1.2);
    assert_eq!(parse_number("1,200", Locale::De.number_hint()).unwrap().value, 1.2);
    assert_eq!(parse_number("1,200", Locale::Us.number_hint()).unwrap().value, 1200.0);
    // Currency with a single convention breaks the tie in both directions; € does not.
    assert_eq!(parse_number("1.200 RSD", Default::default()).unwrap().value, 1200.0);
    assert_eq!(parse_number("$1.200", Default::default()).unwrap().value, 1.2);
    assert_eq!(parse_number("1,200 zł", Default::default()).unwrap().value, 1.2);
    assert_eq!(parse_number("£1,200", Default::default()).unwrap().value, 1200.0);
    assert!(parse_number("1.200 €", Default::default()).unwrap().ambiguous);
    // Dates: both orders under a hint, flagged without one.
    let d = parse_date("03/04/2024", None).unwrap();
    assert!(d.ambiguous);
    assert_eq!(parse_date("03/04/2024", Locale::Us.date_order()).unwrap().month, 3);
    assert_eq!(parse_date("03/04/2024", Locale::Uk.date_order()).unwrap().month, 4);
}

#[test]
fn currency_positions_and_ambiguity() {
    let p = parse_number("CHF 1'234.50", Default::default()).unwrap();
    assert!(p.currency_prefix);
    let p = parse_number("1 234,56 €", Default::default()).unwrap();
    assert!(!p.currency_prefix);
    let p = parse_number("100 kr", Default::default()).unwrap();
    assert_eq!((p.currency, p.currency_ambiguous), (Some("SEK"), true));
    let p = parse_number("¥1,000", Default::default()).unwrap();
    assert_eq!((p.currency, p.currency_ambiguous), (Some("JPY"), true));
    for (s, c) in [
        ("R$ 1.234,56", "BRL"),
        ("C$100", "CAD"),
        ("A$100", "AUD"),
        ("HK$100", "HKD"),
        ("₹1,00", "INR"),
        ("100 ₽", "RUB"),
        ("100 ₺", "TRY"),
        ("₩1,000", "KRW"),
        ("₪100", "ILS"),
        ("100 zł", "PLN"),
        ("100 Kč", "CZK"),
        ("100 PEN", "PEN"),
        ("ZAR 100", "ZAR"),
        ("100 XAF", "XAF"),
    ] {
        let p = parse_number(s, Default::default()).unwrap_or_else(|| panic!("{s:?}"));
        assert_eq!(p.currency, Some(c), "{s:?}");
    }
    assert_eq!(parse_number("100 pen", Default::default()), None);
}
