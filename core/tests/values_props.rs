//! Property-style round trips with a deterministic generator (no extra dependencies):
//! format a value in some locale/style → parse → same value. Plus a no-panic fuzz over
//! random strings built from characters the parsers care about.

use hh_core::values::{
    self, cyrillic_to_latin, norm_key, parse_date, parse_number, DateOrder, DatePrecision, DecimalSep, NumberHint,
};

/// xorshift64* — small, deterministic.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }
}

/// Formats `int_part.frac` with the given thousands and decimal separators.
fn format_num(int_part: u64, frac: Option<(u64, usize)>, thousands: Option<&str>, decimal: char) -> String {
    let digits = int_part.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if let Some(t) = thousands.filter(|_| i > 0 && (digits.len() - i).is_multiple_of(3)) {
            out.push_str(t);
        }
        out.push(ch);
    }
    if let Some((f, w)) = frac {
        out.push(decimal);
        out.push_str(&format!("{f:0w$}"));
    }
    out
}

#[test]
fn number_round_trip() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for _ in 0..20_000 {
        let int_part = match rng.below(4) {
            0 => rng.below(1000),
            1 => rng.below(1_000_000),
            _ => rng.below(10_000_000_000),
        };
        let frac = match rng.below(3) {
            0 => None,
            _ => {
                let w = 1 + rng.below(4) as usize;
                Some((rng.below(10u64.pow(w as u32)), w))
            }
        };
        let decimal = *rng.pick(&[DecimalSep::Dot, DecimalSep::Comma]);
        let dch = decimal.as_char();
        let thousands: Option<&str> = match (rng.below(5), decimal) {
            (0, _) => None,
            (1, _) => Some(" "),
            (2, _) => Some("\u{A0}"),
            (3, _) => Some("'"),
            (_, DecimalSep::Dot) => Some(","),
            (_, DecimalSep::Comma) => Some("."),
        };
        let body = format_num(int_part, frac, thousands, dch);
        let expected_abs = int_part as f64 + frac.map_or(0.0, |(f, w)| f as f64 / 10f64.powi(w as i32));
        let (text, sign) = match rng.below(5) {
            0 => (format!("-{body}"), -1.0),
            1 => (format!("({body})"), -1.0),
            2 => (format!("{body}-"), -1.0),
            _ => (body.clone(), 1.0),
        };
        let (text, pct, cur) = match rng.below(6) {
            0 => (format!("{text} %"), true, None),
            1 => (format!("{text} RSD"), false, Some("RSD")),
            2 => (format!("€ {text}"), false, Some("EUR")),
            3 => (format!(" {text}\u{A0}"), false, None),
            _ => (text, false, None),
        };
        let p = parse_number(&text, NumberHint::with_decimal(Some(decimal)))
            .unwrap_or_else(|| panic!("failed to parse {text:?}"));
        let want = sign * expected_abs;
        assert!((p.value - want).abs() <= 1e-9 * want.abs().max(1.0), "{text:?}: {} != {want}", p.value);
        assert_eq!(p.percent, pct, "{text:?}");
        assert_eq!(p.currency, cur, "{text:?}");
    }
}

#[test]
fn date_round_trip() {
    let mut rng = Rng(42);
    let en_months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let sr_months = ["januar", "februar", "mart", "april", "maj", "jun", "jul", "avgust", "septembar", "oktobar", "novembar", "decembar"];
    let sr_cyr = ["јануар", "фебруар", "март", "април", "мај", "јун", "јул", "август", "септембар", "октобар", "новембар", "децембар"];
    for _ in 0..10_000 {
        let serial = 20000.0 + rng.below(40000) as f64;
        let (y, m, d) = values::serial_to_ymd(serial).unwrap();
        let mi = (m - 1) as usize;
        let (text, order) = match rng.below(10) {
            0 => (format!("{d:02}.{m:02}.{y}."), None),
            1 => (format!("{d}.{m}.{y}"), None),
            2 => (format!("{y}-{m:02}-{d:02}"), None),
            3 => (format!("{d:02}/{m:02}/{y}"), Some(DateOrder::DayMonth)),
            4 => (format!("{m}/{d}/{y}"), Some(DateOrder::MonthDay)),
            5 => (format!("{d} {} {y}", en_months[mi]), None),
            6 => (format!("{} {d}, {y}", en_months[mi]), None),
            7 => (format!("{d}. {} {y}.", sr_months[mi]), None),
            8 => (format!("{d}. {} {y}.", sr_cyr[mi]), None),
            _ => (format!("{d}-{}-{:02}", &en_months[mi][..3], y % 100), None),
        };
        let p = parse_date(&text, order).unwrap_or_else(|| panic!("failed to parse {text:?}"));
        let want_year = if text.ends_with(&format!("-{:02}", y % 100)) && !text.contains(&y.to_string()) {
            if y % 100 < 30 { 2000 + y % 100 } else { 1900 + y % 100 }
        } else {
            y
        };
        assert_eq!((p.year, p.month, p.day, p.precision), (want_year, m, d, DatePrecision::Day), "{text:?}");
        if want_year == y {
            assert_eq!(p.serial(), serial, "{text:?}");
        }
    }
}

#[test]
fn norm_key_script_and_case_invariant() {
    let mut rng = Rng(7);
    let cyr: Vec<char> = "абвгдђежзијклљмнњопрстћуфхцчџшАБВГДЂЕЖЗИЈКЛЉМНЊОПРСТЋУФХЦЧЏШ ".chars().collect();
    for _ in 0..5_000 {
        let n = 1 + rng.below(12) as usize;
        let s: String = (0..n).map(|_| *rng.pick(&cyr)).collect();
        let lat = cyrillic_to_latin(&s).into_owned();
        let k = norm_key(&s);
        assert_eq!(norm_key(&lat), k, "{s:?} / {lat:?}");
        assert_eq!(norm_key(&lat.to_uppercase()), k, "{s:?}");
        assert_eq!(norm_key(&k), k, "idempotent {s:?}");
        assert!(k.is_ascii(), "{s:?} → {k:?}");
    }
}

#[test]
fn parsers_never_panic() {
    let mut rng = Rng(0xDEADBEEF);
    let alphabet: Vec<char> = "0123456789.,'’ \u{A0}\u{200B}-−–+()%*¹²$€£RSDdin.kgmMkKQqIVivx/:T#N/Aa?Σукупноdžđ…\t\nEe"
        .chars()
        .collect();
    for _ in 0..200_000 {
        let n = rng.below(14) as usize;
        let s: String = (0..n).map(|_| *rng.pick(&alphabet)).collect();
        let _ = parse_number(&s, NumberHint::default());
        let _ = parse_number(&s, NumberHint::with_decimal(Some(DecimalSep::Comma)));
        let _ = parse_date(&s, None);
        let _ = parse_date(&s, Some(DateOrder::DayMonth));
        let _ = values::date::is_period_label(&s);
        let _ = values::missing_token(&s);
        let _ = values::parse_bool(&s);
        let _ = values::keywords::total_score(&s);
        let _ = values::keywords::is_note(&s);
        let _ = values::keywords::starts_with_enumerator(&s);
        let _ = values::clean(&s);
        let _ = values::fingerprint(&s);
        let cell = hh_core::Cell { row: 0, col: 0, value: Some(hh_core::Value::Text(s)), formula: None };
        let _ = values::cell_features(&cell, values::ColumnHints::default());
    }
}

/// Every locale preset: format numbers and dates the way that locale writes them → parse with the
/// preset as hint → same value.
#[test]
fn round_trip_every_locale() {
    use hh_core::values::Locale;
    let months: [(Locale, [&str; 12]); 12] = [
        (Locale::Us, ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]),
        (Locale::Uk, ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]),
        (Locale::De, ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"]),
        (Locale::Fr, ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"]),
        (Locale::Es, ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"]),
        (Locale::It, ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"]),
        (Locale::Nl, ["januari", "februari", "maart", "april", "mei", "juni", "juli", "augustus", "september", "oktober", "november", "december"]),
        (Locale::Pt, ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"]),
        (Locale::Pl, ["stycznia", "lutego", "marca", "kwietnia", "maja", "czerwca", "lipca", "sierpnia", "września", "października", "lis.", "grudnia"]),
        (Locale::Ch, ["Jan.", "Feb.", "Mär.", "Apr.", "Mai", "Jun.", "Jul.", "Aug.", "Sep.", "Okt.", "Nov.", "Dez."]),
        (Locale::Rs, ["januar", "februar", "mart", "april", "maj", "jun", "jul", "avgust", "septembar", "oktobar", "novembar", "decembar"]),
        (Locale::Iso, ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]),
    ];
    let mut rng = Rng(0xC0FFEE);
    for (loc, names) in months {
        let dch = loc.decimal().as_char();
        let thousands: &[&str] = match loc {
            Locale::Us | Locale::Uk => &[",", ""],
            Locale::Iso => &[""],
            Locale::Ch => &["'", "’", ""],
            Locale::Fr | Locale::Pl => &["\u{202F}", "\u{A0}", " ", ""],
            _ => &[".", ""],
        };
        for _ in 0..3_000 {
            let int_part = rng.below(100_000_000);
            let frac = (rng.below(2) == 0).then(|| (rng.below(100), 2usize));
            let t = *rng.pick(thousands);
            let body = format_num(int_part, frac, (!t.is_empty()).then_some(t), dch);
            let want = int_part as f64 + frac.map_or(0.0, |(f, _)| f as f64 / 100.0);
            let text = match rng.below(4) {
                0 => format!("-{body}"),
                1 => format!("({body})"),
                _ => body.clone(),
            };
            let sign = if text.starts_with(['-', '(']) { -1.0 } else { 1.0 };
            let p = parse_number(&text, loc.number_hint()).unwrap_or_else(|| panic!("{loc:?}: {text:?}"));
            assert!((p.value - sign * want).abs() <= 1e-9 * want.max(1.0), "{loc:?}: {text:?} → {}", p.value);

            let serial = 20000.0 + rng.below(40000) as f64;
            let (y, m, d) = values::serial_to_ymd(serial).unwrap();
            let numeric = match loc {
                Locale::Us => format!("{m}/{d}/{y}"),
                Locale::Uk | Locale::Fr | Locale::Es | Locale::It | Locale::Pt => format!("{d:02}/{m:02}/{y}"),
                Locale::Nl => format!("{d}-{m}-{y}"),
                Locale::De | Locale::Ch | Locale::Pl => format!("{d}.{m}.{y}"),
                Locale::Rs => format!("{d:02}.{m:02}.{y}."),
                Locale::Iso => format!("{y}-{m:02}-{d:02}"),
            };
            let name = names[(m - 1) as usize];
            let worded = match loc {
                Locale::Us => format!("{name} {d}, {y}"),
                Locale::De | Locale::Ch | Locale::Rs => format!("{d}. {name} {y}"),
                Locale::Es | Locale::Pt => format!("{d} de {name} de {y}"),
                _ => format!("{d} {name} {y}"),
            };
            for s in [numeric, worded] {
                let p = parse_date(&s, loc.date_order()).unwrap_or_else(|| panic!("{loc:?}: {s:?}"));
                assert_eq!(p.serial(), serial, "{loc:?}: {s:?}");
                assert!(!p.ambiguous, "{loc:?}: {s:?}");
            }
        }
    }
}
