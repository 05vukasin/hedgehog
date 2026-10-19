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
//! resolved by [`NumberHint`], normally the per-column result of [`infer_decimal`].

use super::text::{is_space, trim};

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
}

impl NumberHint {
    pub fn with_decimal(decimal: Option<DecimalSep>) -> Self {
        NumberHint { decimal }
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
    /// A footnote marker was stripped (`*`, `†`, `¹`, `(a)`).
    pub footnote: bool,
}

impl ParsedNumber {
    /// Value with scale and percent applied: `"12%"` → 0.12, `"1,5 mil."` → 1_500_000.
    pub fn normalized(&self) -> f64 {
        let v = self.value * self.scale;
        if self.percent { v / 100.0 } else { v }
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
        unit: None,
        scale: 1.0,
        thousands_sep: None,
        decimal_sep: None,
        ambiguous: false,
        exponent: false,
        leading_zero: false,
        footnote: false,
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
                p.currency = Some(currency(&rest[..end])?);
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
    for _ in 0..8 {
        rest = rest.trim_start_matches(is_space);
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
            '*' | '†' | '‡' | '¹' | '²' | '³' | '⁴' | '⁵' | '⁶' | '⁷' | '⁸' | '⁹' => {
                p.footnote = true;
                rest = &rest[c.len_utf8()..];
            }
            '(' => {
                // Footnote like "(a)" or "(1)".
                let close = rest.find(')')?;
                let inner = &rest[1..close];
                if inner.is_empty() || inner.chars().count() > 2 || !inner.chars().all(|c| c.is_alphanumeric()) {
                    return None;
                }
                p.footnote = true;
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
                classify_suffix(word, &mut p)?;
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
    // Without a column hint, a regional currency implies the comma convention ("1.200 RSD").
    let comma_currency = matches!(p.currency, Some("RSD" | "BAM" | "MKD" | "HRK"));
    let hint_used = match hint.decimal {
        None if comma_currency => NumberHint::with_decimal(Some(DecimalSep::Comma)),
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

/// Case-insensitive comparison of `word` against a lowercase dictionary entry.
fn eq_fold(word: &str, lower: &str) -> bool {
    if word.is_ascii() && lower.is_ascii() {
        return word.eq_ignore_ascii_case(lower);
    }
    let mut a = word.chars().flat_map(char::to_lowercase);
    let mut b = lower.chars();
    loop {
        match (a.next(), b.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if x == y => {}
            _ => return false,
        }
    }
}

fn lookup(word: &str, table: &[(&'static str, &[&str])]) -> Option<&'static str> {
    table.iter().find(|(_, aliases)| aliases.iter().any(|a| eq_fold(word, a))).map(|(code, _)| *code)
}

const CURRENCIES: &[(&str, &[&str])] = &[
    ("RSD", &["rsd", "din", "din.", "dinar", "dinara", "dinari", "дин", "дин.", "динар", "динара"]),
    ("EUR", &["eur", "€", "euro", "euros", "evra", "evro", "eura", "евра", "евро"]),
    ("USD", &["usd", "$", "us$", "dollars", "dolara", "долара"]),
    ("GBP", &["gbp", "£"]),
    ("CHF", &["chf"]),
    ("JPY", &["jpy", "¥"]),
    ("RUB", &["rub", "₽", "руб", "руб."]),
    ("HRK", &["hrk", "kn"]),
    ("BAM", &["bam"]),
    ("MKD", &["mkd", "ден", "ден."]),
    ("HUF", &["huf", "ft"]),
    ("CAD", &["cad", "c$"]),
    ("AUD", &["aud", "a$"]),
    ("CNY", &["cny", "rmb"]),
    ("INR", &["inr", "₹"]),
    ("SEK", &["sek"]),
    ("NOK", &["nok"]),
    ("DKK", &["dkk"]),
    ("PLN", &["pln", "zł"]),
    ("CZK", &["czk", "kč"]),
    ("TRY", &["try", "₺"]),
];

/// ISO code for a currency symbol, code or word (case-insensitive), e.g. `din.` → `RSD`.
pub fn currency(word: &str) -> Option<&'static str> {
    if word == "KM" {
        return Some("BAM"); // konvertibilna marka; lowercase "km" is kilometres
    }
    lookup(word, CURRENCIES)
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
    ("pcs", &["kom", "kom.", "komada", "komad", "pcs", "pc", "pcs.", "pieces", "kos", "ком", "ком."]),
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
    (1e3, &["k", "th", "thousand", "thousands", "hilj", "hilj.", "hiljada", "hiljade", "hiljadu", "tis", "tis.", "хиљ", "хиљ.", "хиљада", "хиљаде"]),
    (1e6, &["mil", "mil.", "mln", "mln.", "mn", "mio", "mio.", "milion", "miliona", "milioni", "million", "millions", "мил", "мил.", "милион", "милиона"]),
    (1e9, &["bn", "bln", "mlrd", "mlrd.", "milijarda", "milijarde", "milijardi", "billion", "billions", "млрд", "млрд.", "милијарди", "милијарда"]),
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

fn classify_suffix(word: &str, p: &mut ParsedNumber) -> Option<()> {
    // Each kind of suffix may appear once.
    if let Some(f) = scale(word) {
        (p.scale == 1.0).then(|| p.scale = f)
    } else if let Some(c) = currency(word) {
        p.currency.is_none().then(|| p.currency = Some(c))
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
    pub plain: u32,
}

impl DecimalVotes {
    /// Adds one value's evidence.
    pub fn add(&mut self, s: &str) {
        let Some(p) = parse_number(s, NumberHint::default()) else { return };
        if p.ambiguous {
            if p.decimal_sep == Some('.') { self.ambiguous_dot += 1 } else { self.ambiguous_comma += 1 }
            return;
        }
        match (p.decimal_sep, p.thousands_sep) {
            (Some('.'), _) | (None, Some(',')) => self.dot += 1,
            (Some(','), _) | (None, Some('.')) => self.comma += 1,
            (None, _) if !p.exponent => self.plain += 1,
            _ => {}
        }
    }

    pub fn merge(&mut self, o: &DecimalVotes) {
        self.dot += o.dot;
        self.comma += o.comma;
        self.ambiguous_dot += o.ambiguous_dot;
        self.ambiguous_comma += o.ambiguous_comma;
        self.plain += o.plain;
    }

    /// Majority convention; ties and no evidence → `None`. With no decisive values, ambiguous
    /// `x.xxx` values next to plain integers (`500`, `1.200`, `12.500`) are read as thousands.
    pub fn decision(&self) -> Option<DecimalSep> {
        if self.dot > self.comma {
            return Some(DecimalSep::Dot);
        }
        if self.comma > self.dot {
            return Some(DecimalSep::Comma);
        }
        if self.dot == 0 && self.plain > 0 {
            if self.ambiguous_dot > 0 && self.ambiguous_comma == 0 {
                return Some(DecimalSep::Comma);
            }
            if self.ambiguous_comma > 0 && self.ambiguous_dot == 0 {
                return Some(DecimalSep::Dot);
            }
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
    fn not_numbers() {
        for s in [
            "", " ", "-", "abc", "1a", "a1", "2024-03-12", "12/03/2024", "10:30", "+381 11 123 4567", "1-2", "Q3 2024",
            "1.2.3.4", "ABC-123", "e5", "1e", "N/A", "5 - 6", "(a)", "12 abc", "x", "$", "%", "1,2,3",
        ] {
            assert_eq!(auto(s), None, "{s:?}");
        }
        assert_eq!(val(&"9".repeat(200)), None);
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
        let mut a = DecimalVotes::default();
        a.add("3,14");
        let mut b = DecimalVotes::default();
        b.merge(&a);
        assert_eq!(b.decision(), Some(DecimalSep::Comma));
    }
}
