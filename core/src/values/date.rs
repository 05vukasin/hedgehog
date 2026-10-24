//! Dates written as text and Excel serial dates (irregularities V5, V6, L9).
//!
//! [`parse_date`] understands:
//! * numeric: `12.03.2024` / `12.03.2024.` / `12. 3. 2024.` (Serbian), `2024-03-12` (ISO, with
//!   optional time `T10:30:00Z`), `2024/03/12`, `12/03/2024` vs `03/12/2024` (per-column order,
//!   see [`infer_date_order`]), `12-03-24`, month only `03/2024`, `2024-03`, `03.2024.`;
//! * month names (see below), e.g.
//!   `12 March 2024`, `March 12, 2024`, `12-Mar-24`, `12. mart 2024.`, `јануар 2024`, `okt 2026`;
//! * quarters: `Q3 2024`, `2024-Q3`, `3Q24`, `III kvartal 2024`, `3. kvartal 2024.`,
//!   `III квартал 2024`, `3rd quarter 2024`;
//! * an optional trailing time `10:30`, `10:30:15`, `2:05 PM`, and a leading weekday.
//!
//! Month and weekday names: English, Serbian/Croatian/Bosnian/Slovenian (Latin and Cyrillic),
//! German, French, Spanish, Italian, Dutch, Portuguese, Polish — full, abbreviated, genitive,
//! with or without diacritics (`März`/`Marz`/`Mär`, `févr.`, `août`/`aout`, `dic`). Ordinals and
//! fillers are understood (`December 31st, 2024`, `1er janvier`, `31 de diciembre de 2024`).
//! Also ISO weeks (`2024-W05`, `KW 5/2024`) and stand-alone times ([`parse_time`]).
//!
//! Day/month order for `a/b/yyyy`: a field > 12 decides; otherwise the hint ([`DateOrder`] from
//! a [`super::Locale`] or [`infer_date_order`]); otherwise a tie-break flagged `ambiguous`:
//! `/` → month/day, `-` → day/month (`.` is always day/month).
//! Two-digit years use Excel's pivot: `00`–`29` → 2000–2029, `30`–`99` → 1930–1999.
//!
//! A bare year (`2024`) is not a date (see `v_is_year`). [`ymd_to_serial`] / [`serial_to_ymd`]
//! convert to/from Excel's 1900 date system.

use super::text::{norm_key, trim};

/// Order of day and month in an all-numeric date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DateOrder {
    /// 12/03/2024 = 12 March (Serbia, most of Europe).
    DayMonth,
    /// 03/12/2024 = 12 March (US).
    MonthDay,
}

/// How much of a calendar date the text specified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePrecision {
    Day,
    Month,
    /// Quarter 1–4; `month` is the quarter's first month.
    Quarter(u8),
    /// ISO week 1–53; the date is that week's Monday (or the ISO weekday given).
    Week(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedDate {
    pub year: i32,
    pub month: u8,
    /// 1 when the precision is coarser than a day.
    pub day: u8,
    pub precision: DatePrecision,
    /// Seconds after midnight if a time was given.
    pub time: Option<u32>,
    /// Day/month order was a guess: both fields ≤ 12, separator `/` or `-`, and no hint.
    pub ambiguous: bool,
}

impl ParsedDate {
    /// Excel serial (1900 system) of the start of the period, plus time as a day fraction.
    pub fn serial(&self) -> f64 {
        ymd_to_serial(self.year, self.month, self.day) + self.time.map_or(0.0, |t| t as f64 / 86400.0)
    }
}

pub fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i32, m: u8) -> u8 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

fn valid_ymd(y: i32, m: u8, d: u8) -> bool {
    (1700..=2299).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m)
}

/// Days since 1970-01-01 (proleptic Gregorian).
fn days_from_civil(y: i32, m: u8, d: u8) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

/// Serial of 1970-01-01 in Excel's 1900 date system (epoch 1899-12-30, which absorbs the
/// fictitious 1900-02-29 for every date after February 1900).
const UNIX_EPOCH_SERIAL: i64 = 25569;

/// Excel serial date for a calendar date (valid for dates after 1900-02-28).
pub fn ymd_to_serial(y: i32, m: u8, d: u8) -> f64 {
    (days_from_civil(y, m, d) + UNIX_EPOCH_SERIAL) as f64
}

/// Calendar date of an Excel serial (time part ignored). `None` outside Excel's range
/// (1 … 2958465, i.e. up to 9999-12-31).
pub fn serial_to_ymd(serial: f64) -> Option<(i32, u8, u8)> {
    if !serial.is_finite() || !(1.0..2958466.0).contains(&serial) {
        return None;
    }
    // Serials 1 … 59 are 1900-01-01 … 1900-02-28 (Excel counts the fictitious 1900-02-29 as 60
    // and its epoch 1899-12-30 only holds from March 1900 on); 60 is shown as 1900-02-29, read
    // here as 1900-02-28.
    let days = serial.floor() as i64;
    let days = if days < 60 { days + 1 } else { days };
    Some(civil_from_days(days - UNIX_EPOCH_SERIAL))
}

/// True for numbers that look like Excel serial dates in everyday data: 1954-10-03 (20000)
/// … 2064-04-10 (60000). Use on whole columns, not single cells — plain integers overlap.
pub fn is_plausible_serial(x: f64) -> bool {
    x.is_finite() && (20000.0..=60000.0).contains(&x)
}

/// True if most (≥ 90 %) of the numbers are plausible serial dates, and at least one value
/// is not a round number of thousands (to avoid columns of amounts like 25000, 30000).
pub fn looks_like_serial_column(values: &[f64]) -> bool {
    if values.is_empty() {
        return false;
    }
    let ok = values.iter().filter(|&&x| is_plausible_serial(x)).count();
    let non_round = values.iter().any(|&x| x % 1000.0 != 0.0);
    ok * 10 >= values.len() * 9 && non_round
}

// ---- tokens ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num { v: u32, len: u8 },
    Word(String),
    Sep(char),
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let mut toks = Vec::with_capacity(8);
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c.is_ascii_digit() {
            let mut end = i + 1;
            while let Some(&(j, d)) = chars.peek() {
                if !d.is_ascii_digit() {
                    break;
                }
                end = j + 1;
                chars.next();
            }
            let digits = &s[i..end];
            if digits.len() > 4 {
                return None;
            }
            toks.push(Tok::Num { v: digits.parse().ok()?, len: digits.len() as u8 });
        } else if c.is_alphabetic() {
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = chars.peek() {
                if !d.is_alphabetic() {
                    break;
                }
                end = j + d.len_utf8();
                chars.next();
            }
            toks.push(Tok::Word(norm_key(&s[i..end])));
        } else if c.is_whitespace() {
            if !matches!(toks.last(), Some(Tok::Sep(' '))) {
                toks.push(Tok::Sep(' '));
            }
        } else {
            // A space next to punctuation carries no meaning: "12. 03. 2024" == "12.03.2024".
            if matches!(toks.last(), Some(Tok::Sep(' '))) {
                toks.pop();
            }
            toks.push(Tok::Sep(c));
            while chars.peek().is_some_and(|&(_, d)| d.is_whitespace()) {
                chars.next();
            }
        }
        if toks.len() > 16 {
            return None;
        }
    }
    Some(toks)
}

/// Month number for an English or Serbian (Latin or transliterated Cyrillic) month word,
/// already normalized by `norm_key`.
fn month_word(w: &str) -> Option<u8> {
    // English, Serbian/Bosnian/Montenegrin, Croatian, Slovenian, German, French, Spanish,
    // Italian, Dutch, Portuguese, Polish — full, abbreviated and genitive forms, diacritics folded
    // (`März` → `marz`, `février` → `fevrier`, `août` → `aout`). Omitted on purpose:
    // `listopad(a)`, which is November in Polish/Czech but October in Croatian.
    Some(match w {
        "january" | "jan" | "januar" | "januara" | "januari" | "janner" | "janv" | "janvier" | "enero" | "ene"
        | "gennaio" | "gen" | "janeiro" | "styczen" | "stycznia" | "sty" | "sijecanj" | "sijecnja" => 1,
        "february" | "feb" | "febr" | "februar" | "februara" | "februari" | "feber" | "fevrier" | "fevr" | "fev"
        | "febrero" | "febbraio" | "fevereiro" | "luty" | "lutego" | "lut" | "veljaca" | "veljace" => 2,
        "march" | "mar" | "mart" | "marta" | "marz" | "mars" | "marzo" | "maart" | "mrt" | "marco" | "marzec"
        | "marca" | "marec" | "ozujak" | "ozujka" => 3,
        "april" | "apr" | "aprila" | "aprill" | "avril" | "avr" | "abril" | "abr" | "aprile" | "kwiecien"
        | "kwietnia" | "kwi" | "travanj" | "travnja" => 4,
        "may" | "maj" | "maja" | "mai" | "mayo" | "maggio" | "mag" | "mei" | "maio" | "svibanj" | "svibnja" => 5,
        "june" | "jun" | "juni" | "juna" | "junij" | "junija" | "juin" | "junio" | "giugno" | "giu" | "junho"
        | "czerwiec" | "czerwca" | "cze" | "lipanj" | "lipnja" => 6,
        "july" | "jul" | "juli" | "jula" | "julij" | "julija" | "juillet" | "juil" | "julio" | "luglio" | "lug"
        | "julho" | "lipiec" | "lipca" | "lip" | "srpanj" | "srpnja" => 7,
        "august" | "aug" | "avgust" | "avg" | "avgusta" | "aout" | "agosto" | "ago" | "augustus" | "sierpien"
        | "sierpnia" | "sie" | "kolovoz" | "kolovoza" => 8,
        "september" | "sep" | "sept" | "septembar" | "septembra" | "septemvri" | "septembre" | "septiembre"
        | "setiembre" | "set" | "settembre" | "setembro" | "wrzesien" | "wrzesnia" | "wrz" | "rujan" | "rujna" => 9,
        "october" | "oct" | "okt" | "oktobar" | "oktobra" | "oktober" | "octobre" | "octubre" | "ottobre" | "ott"
        | "outubro" | "out" | "pazdziernik" | "pazdziernika" | "paz" => 10,
        "november" | "nov" | "novembar" | "novembra" | "novembre" | "noviembre" | "novembro" | "lis" | "studeni"
        | "studenoga" | "studenog" => 11,
        "december" | "dec" | "decembar" | "decembra" | "dezember" | "dez" | "decembre" | "diciembre" | "dic"
        | "dicembre" | "dezembro" | "grudzien" | "grudnia" | "gru" | "prosinac" | "prosinca" => 12,
        _ => return None,
    })
}

/// Month abbreviations of other languages that are English words or abbreviations: `avg`
/// (average), `set`, `out`, `gen`, `mag`, `lip`, `lis`, `sie`, `ago`, `dic`, `paz`, `gru`, `lut`.
fn word_like_month(w: &str) -> bool {
    matches!(w, "avg" | "set" | "out" | "gen" | "mag" | "lip" | "lis" | "sie" | "ago" | "dic" | "paz" | "gru" | "lut" | "lug" | "giu" | "ene")
}

/// Month number of a month name or abbreviation in any of the parser's languages (`März`,
/// `Jän`, `févr.`, `Sept`), or `None`.
pub fn month_number(word: &str) -> Option<u8> {
    month_word(norm_key(word).trim_end_matches('.'))
}

fn weekday_word(w: &str) -> bool {
    matches!(
        w,
        // en
        "monday" | "mon" | "tuesday" | "tue" | "tues" | "wednesday" | "wed" | "thursday" | "thu" | "thur" | "thurs"
            | "friday" | "fri" | "saturday" | "sat" | "sunday" | "sun"
            // sr / hr / bs / sl
            | "ponedeljak" | "ponedjeljak" | "pon" | "utorak" | "uto" | "sreda" | "srijeda" | "sre" | "cetvrtak"
            | "cet" | "petak" | "pet" | "subota" | "sub" | "nedelja" | "nedjelja" | "ned"
            // de
            | "montag" | "mo" | "dienstag" | "di" | "mittwoch" | "mi" | "donnerstag" | "do" | "freitag" | "fr"
            | "samstag" | "sonnabend" | "sa" | "sonntag" | "so"
            // fr
            | "lundi" | "lun" | "mardi" | "mar" | "mercredi" | "mer" | "jeudi" | "jeu" | "vendredi" | "ven"
            | "samedi" | "sam" | "dimanche" | "dim"
            // es / it / pt
            | "lunes" | "martes" | "miercoles" | "mie" | "jueves" | "jue" | "viernes" | "vie" | "sabado" | "sab"
            | "domingo" | "dom" | "lunedi" | "martedi" | "mercoledi" | "giovedi" | "gio" | "venerdi" | "sabato"
            | "domenica" | "segunda" | "terca" | "quarta" | "quinta" | "sexta" | "seg" | "ter" | "qua" | "qui"
            | "sex" | "feira"
            // nl
            | "maandag" | "ma" | "dinsdag" | "woensdag" | "wo" | "donderdag" | "vrijdag" | "vr" | "zaterdag" | "za"
            | "zondag" | "zo"
            // pl
            | "poniedzialek" | "wtorek" | "wt" | "sroda" | "sr" | "czwartek" | "czw" | "piatek" | "pt" | "sobota"
            | "sob" | "niedziela" | "ndz" | "nd"
    )
}

/// Words that sit between date parts and carry nothing: "31 de diciembre de 2024",
/// "the 31st of December", "den 31. Dezember", "il 31 dicembre", "le 1er janvier".
fn filler_word(w: &str) -> bool {
    matches!(w, "de" | "del" | "of" | "the" | "le" | "el" | "den" | "am" | "il" | "lo")
}

/// Ordinal suffixes glued to a day or quarter number: 31st, 2nd, 3rd, 4th, 1er, 2e, 2eme, 1º.
fn ordinal_suffix(w: &str) -> bool {
    matches!(w, "st" | "nd" | "rd" | "th" | "er" | "re" | "e" | "eme" | "o" | "a" | "º" | "ª")
}

fn quarter_word(w: &str) -> bool {
    matches!(
        w,
        "q" | "kv" | "kvartal" | "kvartala" | "quarter" | "qtr" | "qrt" | "tromesecje" | "tromesecja" | "quartal"
            | "trimestre" | "trim" | "t" | "kwartaal" | "kwartal"
    )
}

fn roman_or_ordinal(w: &str) -> Option<u8> {
    Some(match w {
        "i" | "first" | "prvi" | "prvo" | "erstes" | "premier" | "primer" | "primero" | "primo" | "eerste" | "pierwszy" => 1,
        "ii" | "second" | "drugi" | "drugo" | "zweites" | "deuxieme" | "segundo" | "secondo" | "tweede" => 2,
        "iii" | "third" | "treci" | "trece" | "drittes" | "troisieme" | "tercer" | "tercero" | "terzo" | "derde" | "trzeci" => 3,
        "iv" | "fourth" | "cetvrti" | "cetvrto" | "viertes" | "quatrieme" | "cuarto" | "quarto" | "vierde" | "czwarty" => 4,
        _ => return None,
    })
}

fn year_of(v: u32, len: u8) -> Option<i32> {
    match len {
        4 => Some(v as i32),
        // Excel's two-digit-year rule: 00–29 → 2000s, 30–99 → 1900s.
        2 => Some(if v < 30 { 2000 + v as i32 } else { 1900 + v as i32 }),
        _ => None,
    }
}

/// Splits a trailing time (`10:30`, `10:30:15`, `10:30:15.5`, `2:05 PM`, `T10:30Z`,
/// `10:30+01:00`) off the text. Returns (date part, seconds after midnight).
fn split_time(s: &str) -> Option<(&str, Option<u32>)> {
    let Some(colon) = s.find(':') else {
        // French "31/12/2024 18h30".
        if let Some((date, last)) = s.rsplit_once(' ') {
            let b = last.as_bytes();
            let h = b.iter().position(|&c| c == b'h');
            let is_clock = |h: usize| {
                (1..=2).contains(&h) && b[..h].iter().all(u8::is_ascii_digit) && b[h + 1..].iter().all(u8::is_ascii_digit) && b.len() - h - 1 <= 2
            };
            if h.is_some_and(is_clock) {
                return Some((date.trim_end_matches([' ', ',']), Some(parse_clock(last)?)));
            }
        }
        return Some((s, None));
    };
    // Start of the time = last space, comma or 'T' before the first colon.
    let head = &s[..colon];
    let start = head.rfind([' ', ',', 'T', 't'])? + 1;
    let date = s[..start - 1].trim_end_matches([' ', ',']);
    Some((date, Some(parse_clock(&s[start..])?)))
}

/// Seconds after midnight for a clock time: `6:00`, `06:00:15`, `18:00:00.5`, `6:00 PM`,
/// `6 pm`, `6:00 a.m.`, `18:00 Uhr`, `18h30`, with an optional `Z` / `+01:00` suffix.
fn parse_clock(t: &str) -> Option<u32> {
    let t = trim(t);
    if t.is_empty() || t.len() > 24 {
        return None;
    }
    let mut l = t.to_ascii_lowercase();
    if let Some(x) = l.strip_suffix("uhr") {
        l = x.trim_end().to_string();
    }
    let mut pm = None;
    for (suf, is_pm) in [("p.m.", true), ("a.m.", false), ("pm", true), ("am", false)] {
        if let Some(x) = l.strip_suffix(suf) {
            l = x.trim_end().to_string();
            pm = Some(is_pm);
            break;
        }
    }
    // Drop a timezone suffix.
    let mut t: &str = l.trim_end_matches('z');
    if let Some(i) = t.rfind(['+', '-']).filter(|&i| i > 0) {
        t = &t[..i];
    }
    // French "18h30" / "18h".
    let fr;
    if let Some(i) = t.find('h').filter(|&i| i > 0 && !t.contains(':')) {
        let (a, b) = (&t[..i], &t[i + 1..]);
        fr = if b.is_empty() { format!("{a}:00") } else { format!("{a}:{b}") };
        t = &fr;
    }
    let mut parts = t.split(':');
    let h_s = parts.next()?;
    if h_s.is_empty() || h_s.len() > 2 || !h_s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: u32 = h_s.parse().ok()?;
    let (mi, sec) = match parts.next() {
        // "6 pm": hour only, needs am/pm.
        None if pm.is_some() => (0, 0.0),
        None => return None,
        Some(mi_s) => {
            if mi_s.len() != 2 || !mi_s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let sec: f64 = match parts.next() {
                Some(x) if !x.is_empty() && x.len() <= 9 && x.chars().all(|c| c.is_ascii_digit() || c == '.') => x.parse().ok()?,
                Some(_) => return None,
                None => 0.0,
            };
            (mi_s.parse::<u32>().ok()?, sec)
        }
    };
    if parts.next().is_some() || mi > 59 || sec >= 61.0 {
        return None;
    }
    let h = match pm {
        Some(true) if (1..=11).contains(&h) => h + 12,
        Some(true) if h == 12 => 12,
        Some(false) if h == 12 => 0,
        Some(_) if h > 12 || h == 0 => return None,
        _ => h,
    };
    if h > 23 {
        return None;
    }
    Some(h * 3600 + mi * 60 + sec as u32)
}

/// A clock time on its own (`6:00`, `6:00 PM`, `18:00:00`, `6 pm`, `18h30`, `18:00 Uhr`):
/// seconds after midnight. Text that also holds a date returns `None` (use [`parse_date`]).
pub fn parse_time(s: &str) -> Option<u32> {
    let t = trim(s);
    if t.len() > 16 || !t.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    parse_clock(t)
}

/// Parses a date written as text. `order` resolves ambiguous `dd/mm` vs `mm/dd`; without it
/// slash dates default to month/day (US) and dash dates to day/month, flagged `ambiguous`.
pub fn parse_date(s: &str, order: Option<DateOrder>) -> Option<ParsedDate> {
    let s = trim(s);
    if s.len() < 4 || s.len() > 48 || !s.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    let (date_part, time) = split_time(s)?;
    let toks = normalize_tokens(tokenize(date_part)?);
    let date = match parse_core(toks.clone(), order) {
        Some(d) => d,
        None => {
            // A leading weekday ("Monday, March 12, 2024", "pon, 12.03.2024", "Di 31.12.2024",
            // "segunda-feira, 1 de janeiro de 2024"). Tried second because some weekday
            // abbreviations are also month abbreviations (fr "mar" = mardi / March).
            let mut t: &[Tok] = &toks;
            let mut stripped = false;
            while let [first, rest @ ..] = t {
                match first {
                    Tok::Word(w) if weekday_word(w) => stripped = true,
                    Tok::Sep(_) if stripped => {}
                    _ => break,
                }
                t = rest;
            }
            if !stripped {
                return None;
            }
            parse_core(t.to_vec(), order)?
        }
    };
    if time.is_some() && date.precision != DatePrecision::Day {
        return None;
    }
    Some(ParsedDate { time, ..date })
}

/// Drops filler words and ordinal suffixes, merges repeated spaces.
fn normalize_tokens(toks: Vec<Tok>) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::with_capacity(toks.len());
    for t in toks {
        match &t {
            Tok::Word(w) if filler_word(w) => continue,
            Tok::Word(w) if ordinal_suffix(w) && matches!(out.last(), Some(Tok::Num { .. })) => continue,
            Tok::Sep(' ') if matches!(out.last(), None | Some(Tok::Sep(_))) => continue,
            Tok::Sep(c) if *c != ' ' && matches!(out.last(), Some(Tok::Sep(' '))) => {
                out.pop();
            }
            _ => {}
        }
        out.push(t);
    }
    out
}

fn parse_core(mut toks: Vec<Tok>, order: Option<DateOrder>) -> Option<ParsedDate> {
    // Trailing dot (Serbian "12.03.2024.") and a trailing comma are noise, and so is a trailing
    // year word: sr "godine"/"god."/"g.", pl "r." ("12. marta 2024. godine", "31 grudnia 2024 r.").
    let mut trailing_dot = false;
    loop {
        while matches!(toks.last(), Some(Tok::Sep('.' | ',' | ' '))) {
            trailing_dot |= matches!(toks.last(), Some(Tok::Sep('.')));
            toks.pop();
        }
        match toks.last() {
            Some(Tok::Word(w)) if matches!(w.as_str(), "godine" | "god" | "g" | "r" | "roku") && toks.len() > 1 => {
                toks.pop();
                trailing_dot = false;
            }
            _ => break,
        }
    }
    parse_tokens(&toks, order, trailing_dot)
}

fn day(y: i32, m: u8, d: u32, ambiguous: bool) -> Option<ParsedDate> {
    let d = u8::try_from(d).ok()?;
    valid_ymd(y, m, d).then_some(ParsedDate { year: y, month: m, day: d, precision: DatePrecision::Day, time: None, ambiguous })
}

fn month(y: i32, m: u32) -> Option<ParsedDate> {
    let m = u8::try_from(m).ok()?;
    valid_ymd(y, m, 1).then_some(ParsedDate { year: y, month: m, day: 1, precision: DatePrecision::Month, time: None, ambiguous: false })
}

fn quarter(y: i32, q: u8) -> Option<ParsedDate> {
    ((1..=4).contains(&q) && (1700..=2299).contains(&y)).then_some(ParsedDate {
        year: y,
        month: (q - 1) * 3 + 1,
        day: 1,
        precision: DatePrecision::Quarter(q),
        time: None,
        ambiguous: false,
    })
}

fn is_date_sep(c: char) -> bool {
    matches!(c, '.' | '/' | '-' | ' ' | ',' | '\'')
}

fn parse_tokens(t: &[Tok], order: Option<DateOrder>, trailing_dot: bool) -> Option<ParsedDate> {
    use Tok::*;
    if let Some(q) = parse_quarter(t) {
        return Some(q);
    }
    if let Some(w) = parse_week(t) {
        return Some(w);
    }
    match t {
        // ---- all-numeric, three fields -------------------------------------------------
        [Num { v: y, len: 4 }, Sep(a), Num { v: m, len: ml }, Sep(b), Num { v: d, len: dl }]
            if a == b && matches!(a, '-' | '/' | '.') && *ml <= 2 && *dl <= 2 =>
        {
            day(*y as i32, u8::try_from(*m).ok()?, *d, false)
        }
        [Num { v: a, len: al }, Sep(s1), Num { v: b, len: bl }, Sep(s2), Num { v: y, len: yl }]
            if s1 == s2 && matches!(s1, '.' | '/' | '-') && *al <= 2 && *bl <= 2 =>
        {
            let y = year_of(*y, *yl)?;
            let (a, b) = (*a, *b);
            if *s1 == '.' {
                return day(y, u8::try_from(b).ok()?, a, false);
            }
            let (ord, ambiguous) = if a > 12 && b <= 12 {
                (DateOrder::DayMonth, false)
            } else if b > 12 && a <= 12 {
                (DateOrder::MonthDay, false)
            } else if a == b {
                (DateOrder::DayMonth, false)
            } else {
                match order {
                    Some(o) => (o, false),
                    None if *s1 == '/' => (DateOrder::MonthDay, true),
                    None => (DateOrder::DayMonth, true),
                }
            };
            match ord {
                DateOrder::DayMonth => day(y, u8::try_from(b).ok()?, a, ambiguous),
                DateOrder::MonthDay => day(y, u8::try_from(a).ok()?, b, ambiguous),
            }
        }
        // ---- all-numeric month precision: 03/2024, 2024-03, 03.2024. -------------------
        [Num { v: m, len: ml }, Sep(s), Num { v: y, len: 4 }]
            if *ml <= 2 && (matches!(s, '/' | '-') || (*s == '.' && trailing_dot)) =>
        {
            month(*y as i32, *m)
        }
        [Num { v: y, len: 4 }, Sep('-' | '/'), Num { v: m, len: 2 }] => month(*y as i32, *m),
        // ---- month names ---------------------------------------------------------------
        // 12 March 2024 / 12. mart 2024. / 12-Mar-24
        [Num { v: d, len: dl }, rest @ ..] if *dl <= 2 => {
            let rest = skip_seps(rest);
            let [Word(w), rest @ ..] = rest else { return None };
            let m = month_word(w)?;
            let rest = skip_seps(rest);
            let [Num { v: y, len: yl }] = rest else { return None };
            day(year_of(*y, *yl)?, m, *d, false)
        }
        // March 12, 2024 / March 2024 / Mar-24 / mart 2024.
        [Word(w), rest @ ..] => {
            let m = month_word(w)?;
            // "March 25" (a space, no year) is a day of March, not March 2025: no date. A
            // two-digit year needs a mark: "Mar-24", "Mar/24", "Jul '99", "mart 24.".
            let spaced = matches!(rest, [Sep(' '), ..]) || matches!(rest, [Num { .. }, ..]);
            let rest = skip_seps(rest);
            match rest {
                [Num { v: y, len: 4 }] => month(*y as i32, m as u32),
                [Num { v: d, len: 2 }] if spaced && !trailing_dot && (1..=31).contains(d) => None,
                // A month abbreviation that is also an English word or abbreviation ("avg 00", an
                // average of 2000, not August 2000; "set 99", "out 01") needs a mark before a
                // two-digit year.
                [Num { len: 2, .. }] if spaced && !trailing_dot && word_like_month(w) => None,
                [Num { v: y, len: 2 }] => month(year_of(*y, 2)?, m as u32),
                [Num { v: d, len: dl }, rest @ ..] if *dl <= 2 => {
                    let rest = skip_seps(rest);
                    let [Num { v: y, len: 4 }] = rest else { return None };
                    day(*y as i32, m, *d, false)
                }
                _ => None,
            }
        }
        // 2024 March / 2024. mart
        [Num { v: y, len: 4 }, rest @ ..] => {
            let rest = skip_seps(rest);
            let [Word(w)] = rest else { return None };
            month(*y as i32, month_word(w)? as u32)
        }
        _ => None,
    }
}

fn skip_seps(mut t: &[Tok]) -> &[Tok] {
    let mut n = 0;
    while let [Tok::Sep(c), rest @ ..] = t {
        if !is_date_sep(*c) || n == 2 {
            break;
        }
        t = rest;
        n += 1;
    }
    t
}

/// ISO weeks: `2024-W05`, `2024W05`, `2024-W05-3`, `W05 2024`, `KW 5/2024`, `week 5 2024`.
/// The date is the Monday (or the given ISO weekday) of that week.
fn parse_week(t: &[Tok]) -> Option<ParsedDate> {
    use Tok::*;
    let is_week = |w: &str| matches!(w, "w" | "wk" | "kw" | "week" | "woche" | "semaine" | "semana" | "settimana" | "tydzien");
    let (y, w, d) = match t {
        [Num { v: y, len: 4 }, Word(k), Num { v: w, len: 1 | 2 }] if k == "w" => (*y, *w, 1),
        [Num { v: y, len: 4 }, Sep('-'), Word(k), Num { v: w, len: 2 }] if k == "w" => (*y, *w, 1),
        [Num { v: y, len: 4 }, Sep('-'), Word(k), Num { v: w, len: 2 }, Sep('-'), Num { v: d, len: 1 }] if k == "w" => (*y, *w, *d),
        [Word(k), rest @ ..] if is_week(k) => {
            let rest = skip_seps(rest);
            let [Num { v: w, len: 1 | 2 }, rest @ ..] = rest else { return None };
            let rest = skip_seps(rest);
            let [Num { v: y, len: 4 }] = rest else { return None };
            (*y, *w, 1)
        }
        _ => return None,
    };
    let y = y as i32;
    if !(1..=53).contains(&w) || !(1..=7).contains(&d) || !(1700..=2299).contains(&y) {
        return None;
    }
    // Monday of ISO week 1 = Monday of the week holding 4 January (1970-01-01 was a Thursday).
    let jan4 = days_from_civil(y, 1, 4);
    let week1 = jan4 - (jan4 + 3).rem_euclid(7);
    let day0 = week1 + (w as i64 - 1) * 7;
    if w == 53 && civil_from_days(day0 + 3).0 != y {
        return None; // year has only 52 weeks
    }
    let (yy, m, dd) = civil_from_days(day0 + d as i64 - 1);
    Some(ParsedDate { year: yy, month: m, day: dd, precision: DatePrecision::Week(w as u8), time: None, ambiguous: false })
}

/// Quarter strings: exactly one quarter keyword, one quarter number (digit 1–4, Roman numeral or
/// ordinal word) adjacent to it, and one year; separators in between.
fn parse_quarter(t: &[Tok]) -> Option<ParsedDate> {
    use Tok::*;
    let kw = t.iter().position(|x| matches!(x, Word(w) if quarter_word(w)))?;
    if t.iter().filter(|x| matches!(x, Word(w) if quarter_word(w))).count() != 1 {
        return None;
    }
    let mut q: Option<(usize, u8)> = None;
    let mut y: Option<(usize, i32)> = None;
    // Quarter number must touch the keyword (only separators / "rd"-style suffixes between).
    let adjacent = |i: usize| {
        let (lo, hi) = if i < kw { (i, kw) } else { (kw, i) };
        hi - lo <= 3 && t[lo + 1..hi].iter().all(|x| matches!(x, Sep(_)) || matches!(x, Word(w) if matches!(w.as_str(), "st" | "nd" | "rd" | "th")))
    };
    for (i, x) in t.iter().enumerate() {
        match x {
            Num { v, len: 1 } if (1..=4).contains(v) && q.is_none() && adjacent(i) => {
                q = Some((i, *v as u8));
            }
            Num { v, len: 4 } if y.is_none() => y = Some((i, *v as i32)),
            // "Q3 24", "3Q24", "Q3'24": two-digit year only right after the quarter number.
            Num { v, len: 2 } if y.is_none() && q.is_some_and(|(qi, _)| i == qi + 1 || i == qi + 2 || i == kw + 1) => {
                y = Some((i, year_of(*v, 2)?));
            }
            Word(w) if i != kw => {
                if let Some(n) = roman_or_ordinal(w).filter(|_| q.is_none() && adjacent(i)) {
                    q = Some((i, n));
                } else if !matches!(w.as_str(), "st" | "nd" | "rd" | "th") {
                    return None;
                }
            }
            Sep(c) if is_date_sep(*c) => {}
            _ if i == kw => {}
            _ => return None,
        }
    }
    quarter(y?.1, q?.1)
}

/// True for a period label with no year: a month name (`Jan`, `mart`, `Јануар`) or a quarter
/// (`Q3`, `III kvartal`, `3. kvartal`) — typical wide-layout column headers.
pub fn is_period_label(s: &str) -> bool {
    let s = trim(s);
    if s.is_empty() || s.len() > 24 {
        return false;
    }
    let Some(toks) = tokenize(s) else { return false };
    let mut toks: &[Tok] = &toks;
    while let [rest @ .., Tok::Sep('.' | ',' | ':')] = toks {
        toks = rest;
    }
    match toks {
        [Tok::Word(w)] => month_word(w).is_some(),
        [Tok::Word(k), Tok::Num { v, len: 1 }] | [Tok::Num { v, len: 1 }, Tok::Word(k)] => quarter_word(k) && (1..=4).contains(v),
        [Tok::Num { v, len: 1 }, Tok::Sep('.' | ' '), Tok::Word(k)] => quarter_word(k) && (1..=4).contains(v),
        [Tok::Word(r), Tok::Sep(' ' | '.'), Tok::Word(k)] => quarter_word(k) && roman_or_ordinal(r).is_some(),
        _ => false,
    }
}

/// Per-column vote on day/month order from all-numeric `a/b/yyyy` or `a-b-yyyy` values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DateOrderVotes {
    pub day_month: u32,
    pub month_day: u32,
}

impl DateOrderVotes {
    pub fn add(&mut self, s: &str) {
        let s = trim(s);
        if s.len() > 32 || !s.bytes().any(|b| b == b'/' || b == b'-') {
            return;
        }
        let Some(d) = parse_date(s, None) else { return };
        if d.precision != DatePrecision::Day || d.ambiguous {
            return;
        }
        // Unambiguous a/b/yyyy: tell which field held the day.
        let head: Vec<&str> = s.split(['/', '-']).take(2).collect();
        if head.len() < 2 || head[0].len() > 2 {
            return; // ISO yyyy-mm-dd says nothing about a/b order
        }
        let (Ok(a), Ok(b)) = (head[0].trim().parse::<u32>(), head[1].trim().parse::<u32>()) else { return };
        if a > 12 && b <= 12 {
            self.day_month += 1;
        } else if b > 12 && a <= 12 {
            self.month_day += 1;
        }
    }

    pub fn merge(&mut self, o: &DateOrderVotes) {
        self.day_month += o.day_month;
        self.month_day += o.month_day;
    }

    pub fn decision(&self) -> Option<DateOrder> {
        use std::cmp::Ordering::*;
        match self.day_month.cmp(&self.month_day) {
            Greater => Some(DateOrder::DayMonth),
            Less => Some(DateOrder::MonthDay),
            Equal => None,
        }
    }
}

/// Decides a column's day/month order: any value whose first field is > 12 votes day-first,
/// second field > 12 votes month-first; majority wins.
pub fn infer_date_order<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<DateOrder> {
    let mut v = DateOrderVotes::default();
    for s in values {
        v.add(s);
    }
    v.decision()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(s: &str) -> Option<(i32, u8, u8)> {
        parse_date(s, None).map(|d| (d.year, d.month, d.day))
    }

    #[test]
    fn serials() {
        assert_eq!(ymd_to_serial(1900, 3, 1), 61.0);
        assert_eq!(ymd_to_serial(2024, 1, 1), 45292.0);
        assert_eq!(ymd_to_serial(1970, 1, 1), 25569.0);
        assert_eq!(serial_to_ymd(45292.0), Some((2024, 1, 1)));
        assert_eq!(serial_to_ymd(45292.75), Some((2024, 1, 1)));
        assert_eq!(serial_to_ymd(61.0), Some((1900, 3, 1)));
        assert_eq!(serial_to_ymd(2958465.0), Some((9999, 12, 31)));
        assert_eq!(serial_to_ymd(0.0), None);
        // January and February 1900: Excel's serial 1 is 1900-01-01 (it was read as 1899-12-31)
        assert_eq!(serial_to_ymd(1.0), Some((1900, 1, 1)));
        assert_eq!(serial_to_ymd(1.5), Some((1900, 1, 1)));
        assert_eq!(serial_to_ymd(59.0), Some((1900, 2, 28)));
        assert_eq!(serial_to_ymd(60.0), Some((1900, 2, 28)));
        assert_eq!(serial_to_ymd(f64::NAN), None);
        for s in [61.0, 10000.0, 36526.0, 45292.0, 60000.0] {
            let (y, m, d) = serial_to_ymd(s).unwrap();
            assert_eq!(ymd_to_serial(y, m, d), s);
        }
        assert!(is_plausible_serial(45292.0));
        assert!(!is_plausible_serial(2024.0));
        assert!(looks_like_serial_column(&[45292.0, 45293.0, 45300.5]));
        assert!(!looks_like_serial_column(&[25000.0, 30000.0]));
        assert!(!looks_like_serial_column(&[1.0, 2.0, 45292.0]));
        assert!(!looks_like_serial_column(&[]));
        let d = parse_date("2024-01-01 18:00", None).unwrap();
        assert_eq!(d.serial(), 45292.75);
    }

    #[test]
    fn serbian_numeric() {
        assert_eq!(ymd("12.03.2024"), Some((2024, 3, 12)));
        assert_eq!(ymd("12.03.2024."), Some((2024, 3, 12)));
        assert_eq!(ymd("12. 3. 2024."), Some((2024, 3, 12)));
        assert_eq!(ymd("1.1.24"), Some((2024, 1, 1)));
        assert_eq!(ymd("31.12.1999"), Some((1999, 12, 31)));
        assert_eq!(ymd("29.02.2024"), Some((2024, 2, 29)));
        assert_eq!(ymd("29.02.2023"), None);
        assert_eq!(ymd("32.01.2024"), None);
        assert_eq!(ymd("12.13.2024"), None);
    }

    #[test]
    fn iso_and_time() {
        assert_eq!(ymd("2024-03-12"), Some((2024, 3, 12)));
        assert_eq!(ymd("2024/3/12"), Some((2024, 3, 12)));
        assert_eq!(ymd("2024.03.12."), Some((2024, 3, 12)));
        let d = parse_date("2024-03-12T10:30:00Z", None).unwrap();
        assert_eq!((d.year, d.month, d.day, d.time), (2024, 3, 12, Some(37800)));
        assert_eq!(parse_date("2024-03-12 10:30", None).unwrap().time, Some(37800));
        assert_eq!(parse_date("2024-03-12T10:30:00+01:00", None).unwrap().time, Some(37800));
        assert_eq!(parse_date("3/12/2024 2:05 PM", None).unwrap().time, Some(14 * 3600 + 300));
        assert_eq!(parse_date("3/12/2024 12:00 AM", None).unwrap().time, Some(0));
        assert_eq!(parse_date("12.03.2024. 23:59:59", None).unwrap().time, Some(86399));
        assert_eq!(parse_date("01.01.2019,14:42", None).unwrap().time, Some(14 * 3600 + 42 * 60));
        assert_eq!(parse_date("March 12, 2024, 10:30", None).unwrap().time, Some(37800));
        assert_eq!(parse_date("2024-03-12 25:00", None), None);
        assert_eq!(parse_date("2024-03-12 10:3", None), None);
        assert_eq!(parse_date("10:30", None), None);
        assert_eq!(parse_date("March 2024 10:30", None), None);
    }

    #[test]
    fn slash_order() {
        // Unambiguous by value.
        assert_eq!(ymd("25/12/2024"), Some((2024, 12, 25)));
        assert_eq!(ymd("12/25/2024"), Some((2024, 12, 25)));
        // Ambiguous: default US for '/', flagged.
        let d = parse_date("03/04/2024", None).unwrap();
        assert!(d.ambiguous);
        assert_eq!((d.month, d.day), (3, 4));
        let d = parse_date("03/04/2024", Some(DateOrder::DayMonth)).unwrap();
        assert!(!d.ambiguous);
        assert_eq!((d.month, d.day), (4, 3));
        let d = parse_date("03-04-24", None).unwrap();
        assert!(d.ambiguous);
        assert_eq!((d.year, d.month, d.day), (2024, 4, 3));
        assert!(!parse_date("05/05/2024", None).unwrap().ambiguous);
        assert_eq!(ymd("13/13/2024"), None);
        assert_eq!(ymd("12/25/99"), Some((1999, 12, 25)));
    }

    #[test]
    fn month_precision() {
        let d = parse_date("03/2024", None).unwrap();
        assert_eq!((d.year, d.month, d.precision), (2024, 3, DatePrecision::Month));
        assert_eq!(ymd("2024-03"), Some((2024, 3, 1)));
        assert_eq!(ymd("03.2024."), Some((2024, 3, 1)));
        assert_eq!(ymd("03.2024"), None); // could be a decimal number
        assert_eq!(ymd("13/2024"), None);
    }

    #[test]
    fn month_names() {
        let cases = [
            ("12 March 2024", (2024, 3, 12)),
            ("12-Mar-2024", (2024, 3, 12)),
            ("12-Mar-24", (2024, 3, 12)),
            ("March 12, 2024", (2024, 3, 12)),
            ("Mar 12 2024", (2024, 3, 12)),
            ("Monday, March 11, 2024", (2024, 3, 11)),
            ("12. mart 2024.", (2024, 3, 12)),
            ("12. marta 2024. godine", (2024, 3, 12)),
            ("12.03.2024. god.", (2024, 3, 12)),
            ("1. januar 2024", (2024, 1, 1)),
            ("1. јануар 2024.", (2024, 1, 1)),
            ("15. августа 2023.", (2023, 8, 15)),
            ("5. Septembra 2023", (2023, 9, 5)),
            ("3 Dec 2001", (2001, 12, 3)),
        ];
        for (s, want) in cases {
            if want.0 == 0 {
                assert_eq!(ymd(s), None, "{s:?}");
            } else {
                assert_eq!(ymd(s), Some(want), "{s:?}");
            }
        }
        for (s, y, m) in [("March 2024", 2024, 3), ("Mar-24", 2024, 3), ("mart 2024.", 2024, 3), ("okt 2026", 2026, 10), ("Јануар 2024", 2024, 1), ("2024 March", 2024, 3), ("Jul '99", 1999, 7)] {
            let d = parse_date(s, None).unwrap_or_else(|| panic!("{s:?}"));
            assert_eq!((d.year, d.month, d.precision), (y, m, DatePrecision::Month), "{s:?}");
        }
        assert_eq!(parse_date("12 March", None), None);
        // A day of a month without a year is no date (it was read as March 2025, 2026, …).
        for s in ["March 25", "March 26", "November 16", "Oct 15", "march 01"] {
            assert_eq!(parse_date(s, None), None, "{s:?}");
        }
        for (s, y) in [("Mar 99", 1999), ("Mar.24", 2024), ("Mar/24", 2024)] {
            let d = parse_date(s, None).unwrap_or_else(|| panic!("{s:?}"));
            assert_eq!((d.year, d.month, d.precision), (y, 3, DatePrecision::Month), "{s:?}");
        }
        assert_eq!(parse_date("Marchx 2024", None), None);
        // "avg 00" in an English sheet is an average for 2000, not August 2000; with a mark or a
        // four-digit year the Serbian month still reads.
        for s in ["avg 00", "avg 99", "set 01", "out 98"] {
            assert_eq!(parse_date(s, None), None, "{s:?}");
        }
        for (s, y) in [("avg-00", 2000), ("avg 2000", 2000), ("avg '99", 1999)] {
            let d = parse_date(s, None).unwrap_or_else(|| panic!("{s:?}"));
            assert_eq!((d.year, d.month), (y, 8), "{s:?}");
        }
        assert_eq!(parse_date("Total 2024", None), None);
    }

    #[test]
    fn quarters() {
        for (s, y, q) in [
            ("Q3 2024", 2024, 3),
            ("Q3-2024", 2024, 3),
            ("Q3/2024", 2024, 3),
            ("2024 Q3", 2024, 3),
            ("2024-Q3", 2024, 3),
            ("2024Q3", 2024, 3),
            ("3Q24", 2024, 3),
            ("Q1 24", 2024, 1),
            ("Q4'23", 2023, 4),
            ("III kvartal 2024", 2024, 3),
            ("III kvartal 2024.", 2024, 3),
            ("3. kvartal 2024.", 2024, 3),
            ("IV kv. 2023", 2023, 4),
            ("III квартал 2024", 2024, 3),
            ("3rd quarter 2024", 2024, 3),
            ("first quarter 2024", 2024, 1),
            ("2024 III kvartal", 2024, 3),
        ] {
            let d = parse_date(s, None).unwrap_or_else(|| panic!("{s:?}"));
            assert_eq!((d.year, d.precision, d.month), (y, DatePrecision::Quarter(q), (q - 1) * 3 + 1), "{s:?}");
        }
        for s in ["Q5 2024", "Q3", "Q3 Q4 2024", "V kvartal 2024", "Q3 2024 total"] {
            assert_eq!(parse_date(s, None), None, "{s:?}");
        }
    }

    #[test]
    fn not_dates() {
        for s in ["", "2024", "abc", "12", "1.5", "1,200.50", "12/03", "12.03.", "Total", "ABC-12-2024", "1-2-3-4", "+381 11 123 4567", "10:30", "12345678"] {
            assert_eq!(parse_date(s, None), None, "{s:?}");
        }
    }

    #[test]
    fn period_labels() {
        for s in ["Jan", "mart", "Јануар", "Q3", "3Q", "III kvartal", "3. kvartal", "December:"] {
            assert!(is_period_label(s), "{s:?}");
        }
        for s in ["Jan 2024", "Total", "Q5", "", "Marchx"] {
            assert!(!is_period_label(s), "{s:?}");
        }
    }

    #[test]
    fn order_votes() {
        assert_eq!(infer_date_order(["25/12/2024", "03/04/2024", "13/01/2024"]), Some(DateOrder::DayMonth));
        assert_eq!(infer_date_order(["12/25/2024", "03/04/2024"]), Some(DateOrder::MonthDay));
        assert_eq!(infer_date_order(["03/04/2024", "2024-12-25"]), None);
        assert_eq!(infer_date_order(["31-01-2024"]), Some(DateOrder::DayMonth));
        assert_eq!(infer_date_order(["x"]), None);
    }
}
