//! Number parsing for numbers stored as text (irregularities V1, V2, V3, V12).
//!
//! [`parse_number`] accepts both separator conventions (`1,200.50` and `1.200,50`), space / NBSP /
//! apostrophe thousands, negatives (`-5`, `(500)`, `500-`), percent, currency symbols and codes,
//! units, scale words (`1,2 mil.`, `5k`) and trailing footnote markers (`1.234*`, `567 (a)`).
//! Everything that was stripped is recorded in [`ParsedNumber`] so the normalizer can report it.
//! Anything it does not understand makes the whole string "not a number" — the parser is strict
//! on purpose so that dates, phone numbers and codes are not misread.
//!
//! The ambiguous case — one separator followed by exactly three digits (`1.200`, `1,200`) — is
//! resolved, in order, by:
//! 1. [`NumberHint`]: an explicit [`super::Locale`] or the per-column result of
//!    [`infer_decimal`] (which itself falls back to the sheet-wide vote);
//! 2. the currency in the cell, when its area has one convention (`1.200 RSD` → 1200,
//!    `$1,200` → 1200; `€` is mixed and gives no evidence);
//! 3. a fixed tie-break with [`ParsedNumber::ambiguous`] set: `.` is read as the decimal point
//!    and `,` as a thousands separator (`1.200` → 1.2, `1,200` → 1200). This is the reading of
//!    spreadsheet "invariant" formatting; it is a guess, never silent — callers must treat
//!    `ambiguous` values as unresolved until column or sheet evidence arrives.
//!
//! Inference is symmetric: a value counts as evidence for a convention only when it cannot be
//! read the other way (`1.200,50`, `3,25`, `1.200.000` ⇒ comma; `1,200.50`, `3.25`,
//! `1,200,000` ⇒ dot).

use super::currency::{currency_match, decimal_convention, eq_fold};
use super::missing::FLAG_LETTERS;
use super::text::{is_space, trim};

pub use super::currency::currency;

/// Which character a column uses as the decimal separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecimalSep {
    /// `1,200.50` (English).
    Dot,
    /// `1.200,50` (Serbian / continental European).
    Comma,
}

impl DecimalSep {
    pub fn as_char(self) -> char {
        match self {
            DecimalSep::Dot => '.',
            DecimalSep::Comma => ',',
        }
    }
}

/// Context for resolving ambiguous strings; `Default` means "no idea".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NumberHint {
    pub decimal: Option<DecimalSep>,
    /// The column holds other currency amounts or mentions a currency; only then are ISO codes
    /// that double as English words (`ALL`, `TOP`, `PEN`…) read as currencies.
    pub currency_context: bool,
}

impl NumberHint {
    pub fn with_decimal(decimal: Option<DecimalSep>) -> Self {
        NumberHint { decimal, currency_context: false }
    }

    pub fn with_currency_context(self, currency_context: bool) -> Self {
        NumberHint { currency_context, ..self }
    }
}

/// How a negative number was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegativeStyle {
    /// `-5`, `−5`, `–5`.
    Minus,
    /// `(500)` — accounting style.
    Parens,
    /// `500-` — trailing minus (SAP / mainframe exports).
    TrailingMinus,
}

/// A number recognised in text, plus everything that was found around it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParsedNumber {
    /// The number as written with its sign, before `scale` and `percent` are applied:
    /// `"(1.200,50 RSD)"` → `-1200.5`, `"12%"` → `12.0`, `"1,5 mil."` → `1.5`.
    pub value: f64,
    pub negative: Option<NegativeStyle>,
    /// A `%` sign was present.
    pub percent: bool,
    /// ISO 4217 code of a currency symbol/code/word found (`€` → `EUR`, `din.` → `RSD`).
    pub currency: Option<&'static str>,
    /// The currency symbol is shared by several currencies (`kr`, `¥`, `Rs`); `currency` holds
    /// the most common reading.
    pub currency_ambiguous: bool,
    /// The currency was written before the number (`$5`, `CHF 1'234.50`) rather than after
    /// (`1 234,56 €`).
    pub currency_prefix: bool,
    /// Canonical unit found (`kom.` → `pcs`, `m²` → `m2`).
    pub unit: Option<&'static str>,
    /// Multiplier from a scale word (`k` → 1e3, `mil.` → 1e6, `mlrd` → 1e9); 1.0 when none.
    pub scale: f64,
    /// Thousands separator used, if any (`'.'`, `','`, `' '`, `'\''`).
    pub thousands_sep: Option<char>,
    /// Decimal separator used, if any.
    pub decimal_sep: Option<char>,
    /// The separator role was a guess: single `.` or `,` followed by exactly three digits and no
    /// hint was given (`"1.200"` → read as 1.2, `"1,200"` → read as 1200).
    pub ambiguous: bool,
    /// Scientific notation (`1.23E+15`).
    pub exponent: bool,
    /// Integer written with leading zeros (`00123`) — likely an ID, not a quantity.
    pub leading_zero: bool,
    /// A footnote marker was stripped (`*`, `†`, `¹`, `(a)`, `(r)`, ` p`, `e`).
    pub footnote: bool,
    /// Byte range of the stripped footnote marker(s) in the trimmed input; see [`ParsedNumber::footnote_mark`].
    pub mark: Option<(u8, u8)>,
}

impl ParsedNumber {
    /// Value with scale and percent applied: `"12%"` → 0.12, `"1,5 mil."` → 1_500_000.
    pub fn normalized(&self) -> f64 {
        let v = self.value * self.scale;
        if self.percent { v / 100.0 } else { v }
    }

    /// The footnote marker(s) as written (`"¹"`, `"(r)"`, `"e"`), given the text that was parsed.
    pub fn footnote_mark<'a>(&self, s: &'a str) -> Option<&'a str> {
        let (a, b) = self.mark?;
        trim(s).get(a as usize..b as usize).map(trim)
    }

    /// True when the text was more than optional sign + digits (anything had to be stripped or
    /// interpreted).
    pub fn is_decorated(&self) -> bool {
        self.negative.is_some_and(|n| n != NegativeStyle::Minus)
            || self.percent
            || self.currency.is_some()
            || self.unit.is_some()
            || self.scale != 1.0
            || self.thousands_sep.is_some()
            || self.footnote
    }
}

const MAX_GROUPS: usize = 24;
const MAX_DIGITS: usize = 48;

/// Parses a number written as text. `None` if the text is not a (decorated) number.
pub fn parse_number(s: &str, hint: NumberHint) -> Option<ParsedNumber> {
    let s = trim(s);
    if s.is_empty() || s.len() > 80 {
        return None;
    }
    let mut p = ParsedNumber {
        value: 0.0,
        negative: None,
        percent: false,
        currency: None,
        currency_ambiguous: false,
        currency_prefix: false,
        unit: None,
        scale: 1.0,
        thousands_sep: None,
        decimal_sep: None,
        ambiguous: false,
        exponent: false,
        leading_zero: false,
        footnote: false,
        mark: None,
    };
    let mut open_paren = false;
    let mut plus = false;
    let mut rest = s;

    // ---- prefix: sign, '(', currency, percent -------------------------------------------
    for _ in 0..6 {
        rest = rest.trim_start_matches(is_space);
        let c = rest.chars().next()?;
        if c.is_ascii_digit() || c == '.' || c == ',' {
            break;
        }
        match c {
            '-' | '−' | '–' if p.negative.is_none() && !plus => {
                p.negative = Some(NegativeStyle::Minus);
                rest = &rest[c.len_utf8()..];
            }
            '+' if p.negative.is_none() && !plus => {
                plus = true;
                rest = &rest[1..];
            }
            '(' if !open_paren => {
                open_paren = true;
                rest = &rest[1..];
            }
            '%' if !p.percent => {
                p.percent = true;
                rest = &rest[1..];
            }
            _ => {
                let end = rest
                    .char_indices()
                    .find(|&(_, c)| is_space(c) || c.is_ascii_digit() || matches!(c, '-' | '−' | '–' | '+' | '(' | ')'))
                    .map_or(rest.len(), |(i, _)| i);
                if end == 0 || p.currency.is_some() {
                    return None;
                }
                let word = &rest[..end];
                let m = currency_match(word)?;
                // A letter-only code glued to the digits is an ID ("AOA2182.06", "FT-1").
                let glued = rest[end..].chars().next().is_some_and(|c| !is_space(c));
                if (glued && word.chars().all(char::is_alphabetic)) || (m.wordlike && !hint.currency_context) {
                    return None;
                }
                p.currency = Some(m.code);
                p.currency_ambiguous = m.ambiguous;
                p.currency_prefix = true;
                rest = &rest[end..];
            }
        }
    }
    let c0 = rest.chars().next()?;
    if !(c0.is_ascii_digit() || c0 == '.' || c0 == ',') {
        return None;
    }

    // ---- core: digit groups separated by . , ' ’ and space-like characters ----------------
    let mut digits = [0u8; MAX_DIGITS];
    let mut nd = 0usize;
    let mut groups = [0u8; MAX_GROUPS]; // digit count per group
    let mut seps = [' '; MAX_GROUPS]; // seps[i] sits between group i and i+1
    let mut ng = 1usize;
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    let mut dash_sep: Option<char> = None;
    while i < rest.len() {
        let c = rest[i..].chars().next().unwrap();
        if c.is_ascii_digit() {
            if nd == MAX_DIGITS {
                return None;
            }
            digits[nd] = bytes[i];
            nd += 1;
            groups[ng - 1] += 1;
            i += 1;
            continue;
        }
        let is_sep = matches!(c, '.' | ',' | '\'' | '’') || (is_space(c) && c != '\t' && c != '\n' && c != '\r');
        if !is_sep {
            break;
        }
        // A run of up to three spaces counts as one thousands separator ("700   000").
        let mut sep_len = c.len_utf8();
        if is_space(c) {
            for _ in 0..2 {
                match rest[i + sep_len..].chars().next() {
                    Some(n) if is_space(n) && !matches!(n, '\t' | '\n' | '\r') => sep_len += n.len_utf8(),
                    _ => break,
                }
            }
        }
        let after = &rest[i + sep_len..];
        let next = after.chars().next();
        let next_digit = next.is_some_and(|n| n.is_ascii_digit());
        if !next_digit {
            // "1.200,-" (Serbian price notation: no decimals) — consume ",-".
            if (c == ',' || c == '.') && matches!(next, Some('-' | '–')) && nd > 0 {
                let after2 = &after[next.unwrap().len_utf8()..];
                if trim(after2).is_empty() || after2.starts_with(is_space) {
                    dash_sep = Some(c);
                    i += c.len_utf8() + next.unwrap().len_utf8();
                }
            }
            break;
        }
        if is_space(c) && groups[ng - 1] == 0 {
            break;
        }
        if ng == MAX_GROUPS {
            return None;
        }
        seps[ng - 1] = if is_space(c) { ' ' } else if c == '’' { '\'' } else { c };
        ng += 1;
        i += sep_len;
    }
    if nd == 0 {
        return None;
    }
    rest = &rest[i..];

    // ---- exponent ------------------------------------------------------------------------
    let mut exp: i32 = 0;
    if let Some(r) = rest.strip_prefix(['e', 'E']) {
        let (neg, r2) = match r.as_bytes().first() {
            Some(b'-') => (true, &r[1..]),
            Some(b'+') => (false, &r[1..]),
            _ => (false, r),
        };
        let n = r2.bytes().take_while(|b| b.is_ascii_digit()).count();
        if n > 0 && n <= 3 {
            exp = r2[..n].parse::<i32>().ok()?;
            if neg {
                exp = -exp;
            }
            p.exponent = true;
            rest = &r2[n..];
        }
    }

    // ---- suffix: ')', trailing minus, %, currency, unit, scale, footnotes ------------------
    let mut closed = false;
    let total = s.len();
    let note_mark = |p: &mut ParsedNumber, start: usize, end: usize| {
        p.footnote = true;
        let (a, b) = (start.min(255) as u8, end.min(255) as u8);
        p.mark = Some(p.mark.map_or((a, b), |(a0, _)| (a0, b)));
    };
    for _ in 0..8 {
        let before = rest.len();
        rest = rest.trim_start_matches(is_space);
        let spaced = rest.len() != before;
        let at = total - rest.len();
        let Some(c) = rest.chars().next() else { break };
        match c {
            ')' if open_paren && !closed => {
                closed = true;
                rest = &rest[1..];
            }
            '-' | '−' | '–' if p.negative.is_none() && !plus => {
                p.negative = Some(NegativeStyle::TrailingMinus);
                rest = &rest[c.len_utf8()..];
            }
            '%' if !p.percent => {
                p.percent = true;
                rest = &rest[1..];
            }
            '*' | '†' | '‡' | '⁰' | '¹' | '²' | '³' | '⁴' | '⁵' | '⁶' | '⁷' | '⁸' | '⁹' => {
                note_mark(&mut p, at, at + c.len_utf8());
                rest = &rest[c.len_utf8()..];
            }
            '(' => {
                // Footnote like "(a)" or "(1)".
                let close = rest.find(')')?;
                let inner = &rest[1..close];
                if inner.is_empty() || inner.chars().count() > 2 || !inner.chars().all(|c| c.is_alphanumeric()) {
                    return None;
                }
                note_mark(&mut p, at, at + close + 1);
                rest = &rest[close + 1..];
            }
            c if c.is_ascii_digit() => return None,
            _ => {
                let end = rest
                    .char_indices()
                    .find(|&(_, c)| is_space(c) || matches!(c, '(' | ')' | '%' | '*'))
                    .map_or(rest.len(), |(i, _)| i);
                let word = &rest[..end];
                if word.is_empty() {
                    return None;
                }
                if classify_suffix(word, &mut p, hint.currency_context).is_none() {
                    // Statistical flag letters: `7 p`, `12.3 e`, `5 bep` (Eurostat writes several
                    // in alphabetical order, so `pen` or `bed` are words, not flags), or one letter
                    // glued to the digits `20560.32e` (estimated / provisional / revised).
                    let w = word.as_bytes();
                    let flag = w.iter().all(|b| FLAG_LETTERS.contains(b))
                        && w.windows(2).all(|p| p[0] < p[1])
                        && if spaced { w.len() <= 3 } else { matches!(word, "e" | "p" | "r") };
                    if !flag {
                        return None;
                    }
                    note_mark(&mut p, at, at + end);
                }
                rest = &rest[end..];
            }
        }
    }
    if !rest.trim_start_matches(is_space).is_empty() {
        return None;
    }
    if open_paren != closed {
        return None;
    }
    if closed {
        if p.negative.is_some() {
            return None;
        }
        p.negative = Some(NegativeStyle::Parens);
    }

    // ---- decide separator roles ------------------------------------------------------------
    let groups = &groups[..ng];
    let seps = &seps[..ng - 1];
    let hint = match dash_sep {
        // "1.200,-": the separator before the dash is the decimal one.
        Some(c) => NumberHint::with_decimal(Some(if c == ',' { DecimalSep::Comma } else { DecimalSep::Dot })),
        None => hint,
    };
    // Without a column hint, the currency's customary convention breaks the tie
    // ("1.200 RSD" → 1200, "$1,200" → 1200, "1.200 €" stays a guess: the euro area is mixed).
    let currency_hint = if p.currency_ambiguous { None } else { p.currency.and_then(decimal_convention) };
    let hint_used = match hint.decimal {
        None if currency_hint.is_some() => NumberHint::with_decimal(currency_hint),
        _ => hint,
    };
    let (thousands, decimal_at, mut ambiguous) = resolve_separators(groups, seps, digits[0] == b'0', hint_used)?;
    if hint_used != hint {
        ambiguous = resolve_separators(groups, seps, digits[0] == b'0', hint).is_some_and(|r| r.2);
    }
    if dash_sep.is_some() && decimal_at.is_some() {
        return None;
    }
    p.thousands_sep = thousands;
    p.ambiguous = ambiguous;
    if dash_sep.is_some() {
        p.decimal_sep = dash_sep;
    }

    // ---- assemble ASCII and parse ----------------------------------------------------------
    let mut buf = [0u8; MAX_DIGITS + 4];
    let mut n = 0usize;
    if p.negative.is_some() {
        buf[n] = b'-';
        n += 1;
    }
    let int_digits: usize = match decimal_at {
        Some(k) => groups[..=k].iter().map(|&g| g as usize).sum(),
        None => nd,
    };
    if int_digits == 0 {
        buf[n] = b'0';
        n += 1;
    }
    buf[n..n + int_digits].copy_from_slice(&digits[..int_digits]);
    n += int_digits;
    if let Some(k) = decimal_at {
        p.decimal_sep = Some(seps[k]);
        buf[n] = b'.';
        n += 1;
        let frac = nd - int_digits;
        buf[n..n + frac].copy_from_slice(&digits[int_digits..nd]);
        n += frac;
    }
    p.leading_zero = decimal_at.is_none() && int_digits > 1 && digits[0] == b'0';
    // "($000)", "(000$)", "000 €": a scale marker in a header ("in thousands"), not a value.
    if p.leading_zero && digits[..nd].iter().all(|&d| d == b'0') && (p.currency.is_some() || p.negative == Some(NegativeStyle::Parens)) {
        return None;
    }
    let text = std::str::from_utf8(&buf[..n]).ok()?;
    let mut v: f64 = text.parse().ok()?;
    if exp != 0 {
        v *= 10f64.powi(exp);
    }
    if !v.is_finite() {
        return None;
    }
    p.value = v;
    Some(p)
}

/// Decides which separators are thousands and which (if any) is the decimal point.
/// Returns (thousands separator, index into `seps` of the decimal separator, ambiguous).
fn resolve_separators(groups: &[u8], seps: &[char], lead_zero: bool, hint: NumberHint) -> Option<(Option<char>, Option<usize>, bool)> {
    if seps.is_empty() {
        return Some((None, None, false));
    }
    let dots = seps.iter().filter(|&&c| c == '.').count();
    let commas = seps.iter().filter(|&&c| c == ',').count();
    let others = seps.len() - dots - commas;
    let last = seps.len() - 1;

    let decimal_at: Option<usize>;
    let mut ambiguous = false;
    if dots > 0 && commas > 0 {
        // The last separator is the decimal point and must be the only one of its kind.
        let d = seps[last];
        let count = if d == '.' { dots } else { commas };
        if count != 1 || others > 0 {
            return None;
        }
        decimal_at = Some(last);
    } else if dots + commas == 0 {
        decimal_at = None;
    } else if dots + commas >= 2 {
        // "1.200.300": a repeated separator is a thousands separator.
        if others > 0 {
            return None;
        }
        decimal_at = None;
    } else {
        // Exactly one '.' or ','.
        let k = seps.iter().position(|&c| c == '.' || c == ',').unwrap();
        let c = seps[k];
        if others > 0 {
            if k != last {
                return None;
            }
            decimal_at = Some(k);
        } else {
            let int = groups[0];
            let frac = groups[1];
            let could_be_thousands = (1..=3).contains(&int) && frac == 3 && !lead_zero;
            if !could_be_thousands {
                decimal_at = Some(k);
            } else {
                match hint.decimal {
                    Some(d) if d.as_char() == c => decimal_at = Some(k),
                    Some(_) => decimal_at = None,
                    None => {
                        ambiguous = true;
                        decimal_at = if c == '.' { Some(k) } else { None };
                    }
                }
            }
        }
    }

    // Validate grouping of the integer part.
    let int_groups = match decimal_at {
        Some(k) => &groups[..=k],
        None => groups,
    };
    if decimal_at.is_some_and(|k| groups[k + 1] == 0) {
        return None;
    }
    let thousands = if int_groups.len() > 1 {
        if !(1..=3).contains(&int_groups[0]) || int_groups[1..].iter().any(|&g| g != 3) {
            return None;
        }
        let t = seps[0];
        if seps[..int_groups.len() - 1].iter().any(|&c| c != t) {
            return None;
        }
        Some(t)
    } else {
        if decimal_at.is_none() && int_groups[0] == 0 {
            return None;
        }
        None
    };
    Some((thousands, decimal_at, ambiguous))
}

fn lookup(word: &str, table: &[(&'static str, &[&str])]) -> Option<&'static str> {
    table.iter().find(|(_, aliases)| aliases.iter().any(|a| eq_fold(word, a))).map(|(code, _)| *code)
}

const UNITS: &[(&str, &[&str])] = &[
    ("kg", &["kg", "kgs", "kilogram", "kilograma", "кг"]),
    ("g", &["g", "gr", "gram", "grama", "гр"]),
    ("mg", &["mg"]),
    ("t", &["t", "tona", "tone", "ton", "tons", "tonnes"]),
    ("km", &["km", "км"]),
    ("m", &["m", "metar", "metara", "м"]),
    ("cm", &["cm", "цм"]),
    ("mm", &["mm"]),
    ("m2", &["m2", "m²", "kvm", "sqm", "м2", "м²"]),
    ("m3", &["m3", "m³", "м3", "м³"]),
    ("km2", &["km2", "km²", "км2", "км²"]),
    ("ha", &["ha", "hektar", "hektara", "ха"]),
    ("l", &["l", "lit", "lit.", "litar", "litara", "liter", "liters", "litre", "litres", "л"]),
    ("ml", &["ml"]),
    ("pcs", &[
        "kom", "kom.", "komada", "komad", "pcs", "pc", "pcs.", "pieces", "kos", "ком", "ком.", "stk", "stk.", "stück",
        "uds", "uds.", "ud", "ud.", "unidades", "pz", "pz.", "pezzi", "szt", "szt.", "st.", "stuks", "pièces",
    ]),
    ("lb", &["lb", "lbs", "lb."]),
    ("oz", &["oz"]),
    ("ft", &["ft", "ft."]),
    ("mi", &["mi"]),
    ("gal", &["gal", "gal."]),
    ("h", &["h", "hr", "hrs", "hours", "sat", "sati", "сати"]),
    ("min", &["min", "min.", "minuta"]),
    ("s", &["s", "sec", "sek"]),
    ("days", &["days", "dana", "дана"]),
    ("kwh", &["kwh"]),
    ("mwh", &["mwh"]),
    ("kw", &["kw"]),
    ("mw", &["mw"]),
    ("km/h", &["km/h", "kmh"]),
    ("°C", &["°c", "°"]),
    ("bbl", &["bbl", "bbls"]),
    ("mmbtu", &["mmbtu"]),
    ("mcf", &["mcf"]),
    ("gb", &["gb"]),
    ("mb", &["mb"]),
];

/// Canonical unit for a unit word (case-insensitive), e.g. `kom.` → `pcs`, `m²` → `m2`.
pub fn unit(word: &str) -> Option<&'static str> {
    lookup(word, UNITS)
}

const SCALES: &[(f64, &[&str])] = &[
    (1e3, &[
        "k", "th", "thousand", "thousands", "hilj", "hilj.", "hiljada", "hiljade", "hiljadu", "tis", "tis.", "хиљ", "хиљ.",
        "хиљада", "хиљаде", "tsd", "tsd.", "tausend", "mille", "tys", "tys.",
    ]),
    (1e6, &[
        "mil", "mil.", "mln", "mln.", "mn", "mio", "mio.", "milion", "miliona", "milioni", "million", "millions", "мил",
        "мил.", "милион", "милиона", "millionen", "millón", "millones", "milione", "miljoen", "milhão",
        "milhões", "milionów",
    ]),
    (1e9, &[
        "bn", "bln", "mlrd", "mlrd.", "milijarda", "milijarde", "milijardi", "billion", "billions", "млрд", "млрд.",
        "милијарди", "милијарда", "mrd", "mrd.", "milliarde", "milliarden", "milliard", "milliards", "md", "mld", "mld.",
        "miliardi", "miljard", "mds",
    ]),
];

/// Multiplier for a scale word (`k`, `mil.`, `mln`, `hiljada`, `million`, `mlrd`, …).
/// Case matters only for one-letter/short forms: `M`/`MM` = million, `B` = billion, while
/// lowercase `m`/`mm` are length units.
pub fn scale(word: &str) -> Option<f64> {
    match word {
        "M" | "MM" => return Some(1e6),
        "B" => return Some(1e9),
        "m" | "mm" | "b" => return None,
        _ => {}
    }
    SCALES.iter().find(|(_, words)| words.iter().any(|w| eq_fold(word, w))).map(|(f, _)| *f)
}

fn classify_suffix(word: &str, p: &mut ParsedNumber, currency_context: bool) -> Option<()> {
    // Each kind of suffix may appear once.
    // After a currency, lowercase "m" is million, not metres: "£2.5m", "$3m".
    let as_scale = if word == "m" && p.currency.is_some() { Some(1e6) } else { scale(word) };
    if let Some(f) = as_scale {
        (p.scale == 1.0).then(|| p.scale = f)
    } else if let Some(m) = currency_match(word).filter(|m| !m.wordlike || currency_context) {
        p.currency.is_none().then(|| {
            p.currency = Some(m.code);
            p.currency_ambiguous = m.ambiguous;
        })
    } else {
        let u = unit(word)?;
        p.unit.is_none().then(|| p.unit = Some(u))
    }
}

/// Per-column vote on the decimal convention, from the column's text values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecimalVotes {
    /// Values that can only be read with `.` as decimal (`1,200.50`, `3.14`, `1,200,000`).
    pub dot: u32,
    /// Values that can only be read with `,` as decimal (`1.200,50`, `3,14`, `1.200.000`).
    pub comma: u32,
    /// `x.xxx` values that could be either.
    pub ambiguous_dot: u32,
    /// `x,xxx` values that could be either.
    pub ambiguous_comma: u32,
    /// Plain integers (`500`) — no evidence, but support a thousands reading of ambiguous values.
    /// Integers with leading zeros (`00123`, IDs) are not counted.
    pub plain: u32,
    /// Sum of log10 of the ambiguous values read with a decimal separator (`1.200` → 1.2), in
    /// thousandths: their size, compared with the column's other numbers.
    pub ambiguous_log_milli: i64,
    /// Day-first dates written with dots (`31.12.2024`, `1.2.2024.`): continental-European
    /// evidence for a decimal comma, used only when no number decides.
    pub dotted_dates: u32,
}

/// Numbers in the same column that were not text (cells typed as numbers; in a CSV, fields that
/// were plain `12.3` / `500`). They cannot vote on the separator, but their shape tells how an
/// ambiguous `x.xxx` next to them reads: among whole numbers it groups thousands (`4.405` next to
/// `3178` is 4405), among fractions of the same size it is a decimal point (`40.805` next to
/// `12.3` is 40.805).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeNumbers {
    pub count: u32,
    /// Values with a fractional part.
    pub fractional: u32,
    /// Sum of log10(|x|) over the non-zero values, in thousandths, and their number.
    pub log_milli: i64,
    pub log_n: u32,
}

impl NativeNumbers {
    pub fn add(&mut self, x: f64) {
        if !x.is_finite() {
            return;
        }
        self.count += 1;
        self.fractional += u32::from(x.fract() != 0.0);
        if x != 0.0 {
            self.log_milli += (x.abs().log10() * 1000.0).round() as i64;
            self.log_n += 1;
        }
    }

    pub fn merge(&mut self, o: &NativeNumbers) {
        self.count += o.count;
        self.fractional += o.fractional;
        self.log_milli += o.log_milli;
        self.log_n += o.log_n;
    }
}

/// `31.12.2024`, `1.2.24`, `31.12.2024.`: day, month and year separated by dots.
fn dotted_date(s: &str) -> bool {
    let t = s.strip_suffix('.').unwrap_or(s);
    let mut parts = t.split('.');
    let (Some(d), Some(m), Some(y), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let digits = |p: &str, lo: usize, hi: usize| (lo..=hi).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit());
    if !(digits(d, 1, 2) && digits(m, 1, 2) && (y.len() == 2 || y.len() == 4) && digits(y, 2, 4)) {
        return false;
    }
    let (d, m): (u32, u32) = (d.parse().unwrap_or(0), m.parse().unwrap_or(0));
    (1..=31).contains(&d) && (1..=12).contains(&m)
}

impl DecimalVotes {
    /// Adds one value's evidence.
    pub fn add(&mut self, s: &str) {
        self.add_parsed(s, parse_number(s, NumberHint::default()).as_ref());
    }

    /// [`DecimalVotes::add`] with `parse_number(s, NumberHint::default())` already done (`p`), so
    /// a caller that needs that reading for something else parses once.
    pub fn add_parsed(&mut self, s: &str, p: Option<&ParsedNumber>) {
        let Some(p) = p else {
            if dotted_date(trim(s)) {
                self.dotted_dates += 1;
            }
            return;
        };
        if p.ambiguous {
            let small = if p.decimal_sep == Some('.') {
                self.ambiguous_dot += 1;
                p.value
            } else {
                self.ambiguous_comma += 1;
                p.value / 1000.0
            };
            if small != 0.0 {
                self.ambiguous_log_milli += (small.abs().log10() * 1000.0).round() as i64;
            }
            return;
        }
        match (p.decimal_sep, p.thousands_sep) {
            (Some('.'), _) | (None, Some(',')) => self.dot += 1,
            (Some(','), _) | (None, Some('.')) => self.comma += 1,
            (None, _) if !p.exponent && !p.leading_zero => self.plain += 1,
            _ => {}
        }
    }

    pub fn merge(&mut self, o: &DecimalVotes) {
        self.dot += o.dot;
        self.comma += o.comma;
        self.ambiguous_dot += o.ambiguous_dot;
        self.ambiguous_comma += o.ambiguous_comma;
        self.plain += o.plain;
        self.ambiguous_log_milli += o.ambiguous_log_milli;
        self.dotted_dates += o.dotted_dates;
    }

    /// Majority convention; ties and no evidence → `None`. With no decisive values, ambiguous
    /// `x.xxx` values next to plain integers (`500`, `1.200`, `12.500`) are read as thousands,
    /// and day-first dates with dots (`31.12.2024`) mean a decimal comma.
    pub fn decision(&self) -> Option<DecimalSep> {
        self.decision_with(&NativeNumbers::default())
    }

    /// [`DecimalVotes::decision`] that also weighs the column's non-text numbers (see
    /// [`NativeNumbers`]) when its text values alone do not decide.
    pub fn decision_with(&self, native: &NativeNumbers) -> Option<DecimalSep> {
        if self.dot > self.comma {
            return Some(DecimalSep::Dot);
        }
        if self.comma > self.dot {
            return Some(DecimalSep::Comma);
        }
        // Only ambiguous values of one kind: does their separator group thousands?
        let amb = self.ambiguous_dot + self.ambiguous_comma;
        let one_kind = amb > 0 && (self.ambiguous_dot == 0 || self.ambiguous_comma == 0);
        let convention = |thousands: bool| {
            let dot_sep = self.ambiguous_dot > 0;
            Some(if dot_sep == thousands { DecimalSep::Comma } else { DecimalSep::Dot })
        };
        if one_kind && native.count >= 2 && native.log_n > 0 {
            let m_amb = self.ambiguous_log_milli as f64 / f64::from(amb) / 1000.0;
            let m_nat = native.log_milli as f64 / f64::from(native.log_n) / 1000.0;
            let (d_dec, d_th) = ((m_amb - m_nat).abs(), (m_amb + 3.0 - m_nat).abs());
            if native.fractional == 0 {
                // Among whole numbers: thousands, unless the sizes clearly say otherwise.
                return convention(d_th <= d_dec + 2.0);
            }
            if d_dec + 0.5 < d_th {
                return convention(false);
            }
            if d_th + 0.5 < d_dec {
                return convention(true);
            }
        }
        if self.dot == 0 && self.plain > 0 {
            if self.ambiguous_dot > 0 && self.ambiguous_comma == 0 {
                return Some(DecimalSep::Comma);
            }
            if self.ambiguous_comma > 0 && self.ambiguous_dot == 0 {
                return Some(DecimalSep::Dot);
            }
        }
        if self.dot == 0 && self.dotted_dates > 0 {
            return Some(DecimalSep::Comma);
        }
        None
    }
}

/// Decides a column's decimal convention from its text values (`None` when undecided).
pub fn infer_decimal<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<DecimalSep> {
    let mut v = DecimalVotes::default();
    for s in values {
        v.add(s);
    }
    v.decision()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto(s: &str) -> Option<ParsedNumber> {
        parse_number(s, NumberHint::default())
    }
    fn val(s: &str) -> Option<f64> {
        auto(s).map(|p| p.normalized())
    }
    fn with(s: &str, d: DecimalSep) -> Option<f64> {
        parse_number(s, NumberHint::with_decimal(Some(d))).map(|p| p.normalized())
    }

    #[test]
    fn plain() {
        assert_eq!(val("0"), Some(0.0));
        assert_eq!(val("42"), Some(42.0));
        assert_eq!(val(" 42 "), Some(42.0));
        assert_eq!(val("\u{A0}42\u{200B}"), Some(42.0));
        assert_eq!(val("3.25"), Some(3.25));
        assert_eq!(val("3,25"), Some(3.25));
        assert_eq!(val(".5"), Some(0.5));
        assert_eq!(val(",5"), Some(0.5));
        assert_eq!(val("+7"), Some(7.0));
        let p = auto("42").unwrap();
        assert!(!p.is_decorated());
    }

    #[test]
    fn separators_both_conventions() {
        assert_eq!(val("1,200.50"), Some(1200.5));
        assert_eq!(val("1.200,50"), Some(1200.5));
        assert_eq!(val("1,234,567.89"), Some(1234567.89));
        assert_eq!(val("1.234.567,89"), Some(1234567.89));
        assert_eq!(val("1.234.567"), Some(1234567.0));
        assert_eq!(val("1,234,567"), Some(1234567.0));
        assert_eq!(val("1 234 567,5"), Some(1234567.5));
        assert_eq!(val("1\u{A0}234,5"), Some(1234.5));
        assert_eq!(val("700   000"), Some(700000.0));
        assert_eq!(val("700    000"), None);
        assert_eq!(val("1\u{202F}234.5"), Some(1234.5));
        assert_eq!(val("1'234'567.50"), Some(1234567.5));
        assert_eq!(val("1’234.50"), Some(1234.5));
        let p = auto("1.200,50").unwrap();
        assert_eq!((p.thousands_sep, p.decimal_sep, p.ambiguous), (Some('.'), Some(','), false));
        let p = auto("1 200").unwrap();
        assert_eq!(p.thousands_sep, Some(' '));
        // A regional currency resolves the ambiguous case towards the comma convention.
        let p = auto("1.200 RSD").unwrap();
        assert_eq!((p.value, p.ambiguous), (1200.0, true));
        assert_eq!(auto("1.200 EUR").unwrap().value, 1.2);
        assert_eq!(parse_number("1.200 RSD", NumberHint::with_decimal(Some(DecimalSep::Dot))).unwrap().value, 1.2);
    }

    #[test]
    fn ambiguous_uses_hint() {
        let p = auto("1.200").unwrap();
        assert!(p.ambiguous);
        assert_eq!(p.value, 1.2);
        let p = auto("1,200").unwrap();
        assert!(p.ambiguous);
        assert_eq!(p.value, 1200.0);
        assert_eq!(with("1.200", DecimalSep::Comma), Some(1200.0));
        assert_eq!(with("1.200", DecimalSep::Dot), Some(1.2));
        assert_eq!(with("1,200", DecimalSep::Comma), Some(1.2));
        assert_eq!(with("1,200", DecimalSep::Dot), Some(1200.0));
        assert!(!parse_number("1.200", NumberHint::with_decimal(Some(DecimalSep::Comma))).unwrap().ambiguous);
        // Structure beats the hint.
        assert_eq!(with("1,5", DecimalSep::Dot), Some(1.5));
        assert_eq!(with("1.200,50", DecimalSep::Dot), Some(1200.5));
        // Not ambiguous: leading zero, 4+ integer digits, 2 decimals.
        assert!(!auto("0.125").unwrap().ambiguous);
        assert_eq!(val("1234.567"), Some(1234.567));
        assert_eq!(val("12,50"), Some(12.5));
    }

    #[test]
    fn bad_grouping_rejected() {
        for s in ["1,00,000", "12.03.2024", "1.2.3", "1,2,3.4.5", "1.200.300,5,6", "1 2", "1..2", "1.", "1,", "12 34", "1.200 300"] {
            assert_eq!(val(s), None, "{s:?}");
        }
    }

    #[test]
    fn negatives() {
        assert_eq!(val("-5"), Some(-5.0));
        assert_eq!(val("−5"), Some(-5.0));
        assert_eq!(val("– 5"), Some(-5.0));
        assert_eq!(val("(500)"), Some(-500.0));
        assert_eq!(val("( 1.200,50 )"), Some(-1200.5));
        assert_eq!(val("500-"), Some(-500.0));
        assert_eq!(auto("(500)").unwrap().negative, Some(NegativeStyle::Parens));
        assert_eq!(auto("500-").unwrap().negative, Some(NegativeStyle::TrailingMinus));
        assert_eq!(auto("-5").unwrap().negative, Some(NegativeStyle::Minus));
        for s in ["-(5)", "(5", "5)", "--5", "-5-", "+-5"] {
            assert_eq!(val(s), None, "{s:?}");
        }
    }

    #[test]
    fn percent() {
        let p = auto("12%").unwrap();
        assert!(p.percent);
        assert_eq!(p.value, 12.0);
        assert_eq!(p.normalized(), 0.12);
        assert_eq!(val("12,5 %"), Some(0.125));
        assert_eq!(val("-3.5%"), Some(-0.035));
        assert_eq!(val("% 5"), Some(0.05));
        assert_eq!(val("5%%"), None);
    }

    #[test]
    fn currencies() {
        let cases = [
            ("1.200 RSD", 1200.0, "RSD"),
            ("1.200,00 din.", 1200.0, "RSD"),
            ("1.200,00 дин.", 1200.0, "RSD"),
            ("500 Din", 500.0, "RSD"),
            ("€ 15", 15.0, "EUR"),
            ("15€", 15.0, "EUR"),
            ("15,50 EUR", 15.5, "EUR"),
            ("$1,234.56", 1234.56, "USD"),
            ("-$5", -5.0, "USD"),
            ("$-5", -5.0, "USD"),
            ("($500)", -500.0, "USD"),
            ("USD 1,000", 1000.0, "USD"),
            ("£3", 3.0, "GBP"),
            ("10 KM", 10.0, "BAM"),
            ("1.200,-", 1200.0, ""),
            ("1.200,- din", 1200.0, "RSD"),
        ];
        for (s, v, c) in cases {
            let p = auto(s).unwrap_or_else(|| panic!("{s:?}"));
            assert_eq!(p.normalized(), v, "{s:?}");
            assert_eq!(p.currency.unwrap_or(""), c, "{s:?}");
        }
        assert_eq!(val("$$5"), None);
        // Audit of the Enron sample: IDs and header markers are not amounts.
        for s in ["AOA2182.06", "FT-1", "NGN2", "SDG1", "JPY4512201", "($000)", "(000$)", "000 $", "5 ft"] {
            assert_eq!(auto(s).and_then(|p| p.currency), None, "{s:?}");
        }
        assert_eq!(val("$2M"), Some(2e6));
        assert_eq!(val("$0.50 MM"), Some(5e5));
        assert_eq!(val("$             0.00"), Some(0.0));
        // Word-like codes need column evidence.
        for s in ["TOP 10", "ALL 5", "CUP 3", "5 PEN", "BOB 2", "MOP 1", "GEL 4"] {
            assert_eq!(auto(s), None, "{s:?}");
        }
        let ctx = NumberHint::default().with_currency_context(true);
        assert_eq!(parse_number("ALL 5", ctx).unwrap().currency, Some("ALL"));
        assert_eq!(parse_number("5 PEN", ctx).unwrap().currency, Some("PEN"));
        assert_eq!(auto("USD 5").unwrap().currency, Some("USD"));
        assert_eq!(auto("100 USD").unwrap().currency, Some("USD"));
        assert_eq!(val("5 EUR USD"), None);
    }

    #[test]
    fn units() {
        let cases = [
            ("15 kg", "kg"),
            ("15kg", "kg"),
            ("3 kom", "pcs"),
            ("3 kom.", "pcs"),
            ("3 pcs", "pcs"),
            ("120 m²", "m2"),
            ("120 m2", "m2"),
            ("5 km", "km"),
            ("2,5 l", "l"),
            ("2 m", "m"),
            ("100 mm", "mm"),
            ("8 h", "h"),
            ("3 кг", "kg"),
        ];
        for (s, u) in cases {
            let p = auto(s).unwrap_or_else(|| panic!("{s:?}"));
            assert_eq!(p.unit, Some(u), "{s:?}");
            assert_eq!(p.scale, 1.0, "{s:?}");
        }
        assert_eq!(val("5 kg kg"), None);
        assert_eq!(val("5 apples"), None);
    }

    #[test]
    fn scales() {
        assert_eq!(val("5k"), Some(5000.0));
        assert_eq!(val("5 K"), Some(5000.0));
        assert_eq!(val("1.5M"), Some(1_500_000.0));
        assert_eq!(val("1,5 mil."), Some(1_500_000.0));
        assert_eq!(val("2 mln"), Some(2_000_000.0));
        assert_eq!(val("3 hiljade"), Some(3000.0));
        assert_eq!(val("3 thousand"), Some(3000.0));
        assert_eq!(val("2 million"), Some(2e6));
        assert_eq!(val("1,2 mlrd"), Some(1.2e9));
        assert_eq!(val("4bn"), Some(4e9));
        assert_eq!(val("1,5 милиона"), Some(1.5e6));
        assert_eq!(val("5MM"), Some(5e6));
        let p = auto("1,5 mil. RSD").unwrap();
        assert_eq!((p.scale, p.currency), (1e6, Some("RSD")));
        let p = auto("$1.2bn").unwrap();
        assert_eq!((p.normalized(), p.currency), (1.2e9, Some("USD")));
        // lowercase m is metres, not million
        assert_eq!(auto("5m").unwrap().unit, Some("m"));
        assert_eq!(val("5 k k"), None);
    }

    #[test]
    fn exponent() {
        let p = auto("1.23E+15").unwrap();
        assert!(p.exponent);
        assert_eq!(p.value, 1.23e15);
        assert_eq!(val("5e-3"), Some(0.005));
        assert_eq!(val("2E3"), Some(2000.0));
        assert_eq!(val("5 EUR"), Some(5.0));
    }

    #[test]
    fn footnotes_and_leading_zero() {
        let p = auto("1.234*").unwrap();
        assert!(p.footnote);
        assert_eq!(val("1.234*"), Some(1.234));
        assert_eq!(val("567 (a)"), Some(567.0));
        assert_eq!(val("567(1)"), Some(567.0));
        assert_eq!(val("12³"), Some(12.0));
        assert_eq!(val("12 (note)"), None);
        let p = auto("00123").unwrap();
        assert!(p.leading_zero);
        assert_eq!(p.value, 123.0);
        assert!(!auto("0.5").unwrap().leading_zero);
        assert!(!auto("0").unwrap().leading_zero);
    }

    #[test]
    fn statistical_footnote_marks() {
        let cases = [
            ("123¹", 123.0, "¹"),
            ("45*", 45.0, "*"),
            ("12 (r)", 12.0, "(r)"),
            ("6077.61 (p)", 6077.61, "(p)"),
            ("7 p", 7.0, "p"),
            ("20560.32e", 20560.32, "e"),
            ("84e", 84.0, "e"),
            ("1e", 1.0, "e"),
            ("12.3 bep", 12.3, "bep"),
            ("1 234*", 1234.0, "*"),
            ("1737³", 1737.0, "³"),
            ("28472.72*", 28472.72, "*"),
            ("12¹²", 12.0, "¹²"),
        ];
        for (s, v, m) in cases {
            let p = auto(s).unwrap_or_else(|| panic!("{s:?}"));
            assert_eq!(p.normalized(), v, "{s:?}");
            assert!(p.footnote, "{s:?}");
            assert_eq!(p.footnote_mark(s), Some(m), "{s:?}");
        }
        // Not marks: exponents, units, other letters, letters glued that are not e / p / r.
        let p = auto("1e5").unwrap();
        assert_eq!((p.value, p.footnote, p.footnote_mark("1e5")), (100000.0, false, None));
        assert_eq!(auto("5 s").unwrap().unit, Some("s"));
        assert!(!auto("5 s").unwrap().footnote);
        for s in ["12b", "12 x", "12 pq", "12 bepr", "e", "12 e5", "100 pen", "3 bed", "5 pp"] {
            assert_eq!(auto(s), None, "{s:?}");
        }
    }

    #[test]
    fn not_numbers() {
        for s in [
            "", " ", "-", "abc", "1a", "a1", "2024-03-12", "12/03/2024", "10:30", "+381 11 123 4567", "1-2", "Q3 2024",
            "1.2.3.4", "ABC-123", "e5", "1e+", "N/A", "5 - 6", "(a)", "12 abc", "x", "$", "%", "1,2,3",
        ] {
            assert_eq!(auto(s), None, "{s:?}");
        }
        assert_eq!(val(&"9".repeat(200)), None);
    }

    #[test]
    fn native_numbers_decide_ambiguous_values() {
        let votes = |xs: &[&str]| {
            let mut v = DecimalVotes::default();
            xs.iter().for_each(|s| v.add(s));
            v
        };
        let natives = |xs: &[f64]| {
            let mut n = NativeNumbers::default();
            xs.iter().for_each(|x| n.add(*x));
            n
        };
        // Percentages with up to 3 decimals (CSV: 12.3 is a number, 40.805 stays text).
        let v = votes(&["40.805", "12.125"]);
        assert_eq!(v.decision(), None);
        assert_eq!(v.decision_with(&natives(&[12.3, 8.4, 0.5, -3.25, 20.1])), Some(DecimalSep::Dot));
        // EU thousands next to typed integers.
        assert_eq!(votes(&["4.405"]).decision_with(&natives(&[3178.0, 2411.0, 512.0])), Some(DecimalSep::Comma));
        assert_eq!(votes(&["1,200"]).decision_with(&natives(&[3178.0, 2411.0])), Some(DecimalSep::Dot));
        // Small whole numbers next to x.xxx: sizes say decimal.
        assert_eq!(votes(&["1.500"]).decision_with(&natives(&[1.0, 2.0, 3.0])), Some(DecimalSep::Dot));
        // Fractions of the thousands size: thousands.
        assert_eq!(votes(&["4.405"]).decision_with(&natives(&[3178.5, 2411.25])), Some(DecimalSep::Comma));
        // One native number is not enough; unambiguous text still wins.
        assert_eq!(votes(&["4.405"]).decision_with(&natives(&[3178.0])), None);
        assert_eq!(votes(&["4.405", "1,5"]).decision_with(&natives(&[3178.0, 1.0])), Some(DecimalSep::Comma));
    }

    #[test]
    fn decimal_votes() {
        assert_eq!(infer_decimal(["1.200,50", "3,5", "1.200"]), Some(DecimalSep::Comma));
        assert_eq!(infer_decimal(["1,200.50", "3.5", "1,200"]), Some(DecimalSep::Dot));
        assert_eq!(infer_decimal(["1.200", "500", "12.500"]), Some(DecimalSep::Comma));
        assert_eq!(infer_decimal(["1,200", "500"]), Some(DecimalSep::Dot));
        assert_eq!(infer_decimal(["1.200", "2.500"]), None);
        assert_eq!(infer_decimal(["abc", "500"]), None);
        assert_eq!(infer_decimal(["1.200.000", "1.500"]), Some(DecimalSep::Comma));
        // Integers with leading zeros (IDs) do not support a thousands reading.
        assert_eq!(infer_decimal(["01234567", "1.200"]), None);
        // Day-first dates with dots mean a decimal comma when no number decides.
        assert_eq!(infer_decimal(["31.12.2024", "1.200"]), Some(DecimalSep::Comma));
        assert_eq!(infer_decimal(["31.12.2024", "1.5"]), Some(DecimalSep::Dot));
        let mut a = DecimalVotes::default();
        a.add("3,14");
        let mut b = DecimalVotes::default();
        b.merge(&a);
        assert_eq!(b.decision(), Some(DecimalSep::Comma));
    }
}
