//! Currency recognition: all active ISO 4217 codes (embedded, uppercase only), plus symbols and
//! words in prefix or suffix position (`$`, `€`, `£`, `¥`, `₹`, `₽`, `₺`, `₩`, `₪`, `zł`, `Kč`,
//! `kr`, `Fr.`, `R$`, `C$`, `A$`, `US$`, `HK$`, `din.`, `дин.` …).
//!
//! Ambiguity: a bare `$` is read as USD (prefixed dollars such as `C$`, `A$`, `HK$` are
//! distinct). Symbols shared by several currencies resolve to the most common one and are flagged
//! [`CurrencyMatch::ambiguous`]: `kr` (SEK/NOK/DKK/ISK → SEK), `¥` (JPY/CNY → JPY), `Rs`
//! (INR/PKR/LKR/NPR → INR).

use super::number::DecimalSep;

/// Active ISO 4217 alphabetic codes (source: ISO 4217 list one; test codes XTS/XXX omitted).
const ISO_CODES: [&str; 176] = [
    "AED", "AFN", "ALL", "AMD", "AOA", "ARS", "AUD", "AWG", "AZN", "BAM", "BBD", "BDT", "BHD", "BIF", "BMD", "BND",
    "BOB", "BOV", "BRL", "BSD", "BTN", "BWP", "BYN", "BZD", "CAD", "CDF", "CHE", "CHF", "CHW", "CLF", "CLP", "CNY",
    "COP", "COU", "CRC", "CUP", "CVE", "CZK", "DJF", "DKK", "DOP", "DZD", "EGP", "ERN", "ETB", "EUR", "FJD", "FKP",
    "GBP", "GEL", "GHS", "GIP", "GMD", "GNF", "GTQ", "GYD", "HKD", "HNL", "HTG", "HUF", "IDR", "ILS", "INR", "IQD",
    "IRR", "ISK", "JMD", "JOD", "JPY", "KES", "KGS", "KHR", "KMF", "KPW", "KRW", "KWD", "KYD", "KZT", "LAK", "LBP",
    "LKR", "LRD", "LSL", "LYD", "MAD", "MDL", "MGA", "MKD", "MMK", "MNT", "MOP", "MRU", "MUR", "MVR", "MWK", "MXN",
    "MXV", "MYR", "MZN", "NAD", "NGN", "NIO", "NOK", "NPR", "NZD", "OMR", "PAB", "PEN", "PGK", "PHP", "PKR", "PLN",
    "PYG", "QAR", "RON", "RSD", "RUB", "RWF", "SAR", "SBD", "SCR", "SDG", "SEK", "SGD", "SHP", "SLE", "SOS", "SRD",
    "SSP", "STN", "SVC", "SYP", "SZL", "THB", "TJS", "TMT", "TND", "TOP", "TRY", "TTD", "TWD", "TZS", "UAH", "UGX",
    "USD", "USN", "UYI", "UYU", "UYW", "UZS", "VED", "VES", "VND", "VUV", "WST", "XAD", "XAF", "XAG", "XAU", "XBA",
    "XBB", "XBC", "XBD", "XCD", "XCG", "XDR", "XOF", "XPD", "XPF", "XPT", "XSU", "XUA", "YER", "ZAR", "ZMW", "ZWG",
];

/// (ISO code, ambiguous, case-insensitive aliases in lowercase).
const ALIASES: &[(&str, bool, &[&str])] = &[
    ("USD", false, &["$", "usd", "us$", "u$s", "dollar", "dollars", "dolara", "долара"]),
    ("EUR", false, &["€", "eur", "euro", "euros", "evra", "evro", "eura", "евра", "евро"]),
    ("GBP", false, &["£", "gbp", "pounds", "stg"]),
    ("RSD", false, &["rsd", "din", "din.", "dinar", "dinara", "dinari", "дин", "дин.", "динар", "динара"]),
    ("CHF", false, &["chf", "fr.", "fr", "sfr", "sfr.", "chf."]),
    ("JPY", true, &["¥", "円"]),
    ("JPY", false, &["jpy", "yen"]),
    ("CNY", false, &["cny", "rmb", "元", "yuan"]),
    ("INR", true, &["rs", "rs."]),
    ("INR", false, &["₹", "inr"]),
    ("SEK", true, &["kr", "kr.", ":-"]),
    ("RUB", false, &["₽", "rub", "руб", "руб.", "rbl"]),
    ("TRY", false, &["₺", "try", "tl"]),
    ("KRW", false, &["₩", "krw", "won"]),
    ("ILS", false, &["₪", "ils", "nis"]),
    ("PLN", false, &["zł", "pln"]),
    ("CZK", false, &["kč", "czk"]),
    ("HUF", false, &["ft", "huf"]),
    ("RON", false, &["lei", "ron"]),
    ("BGN", false, &["лв", "лв.", "bgn"]),
    ("UAH", false, &["₴", "грн", "uah"]),
    ("VND", false, &["₫", "vnd"]),
    ("THB", false, &["฿", "thb"]),
    ("PHP", false, &["₱", "php"]),
    ("NGN", false, &["₦", "ngn"]),
    ("KZT", false, &["₸", "kzt"]),
    ("BRL", false, &["r$", "brl"]),
    ("CAD", false, &["c$", "ca$", "cad"]),
    ("AUD", false, &["a$", "au$", "aud"]),
    ("NZD", false, &["nz$", "nzd"]),
    ("HKD", false, &["hk$", "hkd"]),
    ("SGD", false, &["s$", "sgd"]),
    ("MXN", false, &["mx$", "mxn"]),
    ("HRK", false, &["kn", "hrk"]),
    ("MKD", false, &["ден", "ден.", "mkd"]),
    ("BAM", false, &["bam"]),
    ("SEK", false, &["sek"]),
    ("NOK", false, &["nok"]),
    ("DKK", false, &["dkk"]),
    ("ISK", false, &["isk"]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrencyMatch {
    /// ISO 4217 code (for an ambiguous symbol, the most common reading).
    pub code: &'static str,
    /// The symbol is shared by several currencies (`kr`, `¥`, `Rs`).
    pub ambiguous: bool,
}

/// Case-insensitive comparison of `word` against a lowercase entry.
pub(crate) fn eq_fold(word: &str, lower: &str) -> bool {
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

/// True for an active ISO 4217 code written in uppercase (`USD`, `PEN`, `ALL`). Lowercase
/// spellings are accepted only for the common codes listed as aliases, so words like `all` or
/// `top` are not read as currencies.
pub fn is_iso_code(word: &str) -> bool {
    word.len() == 3 && word.bytes().all(|b| b.is_ascii_uppercase()) && ISO_CODES.binary_search(&word).is_ok()
}

/// Recognizes a currency symbol, code or word.
pub fn currency_match(word: &str) -> Option<CurrencyMatch> {
    if word == "KM" {
        // konvertibilna marka; lowercase "km" is kilometres
        return Some(CurrencyMatch { code: "BAM", ambiguous: false });
    }
    if let Some((code, ambiguous, _)) = ALIASES.iter().find(|(_, _, a)| a.iter().any(|x| eq_fold(word, x))) {
        return Some(CurrencyMatch { code, ambiguous: *ambiguous });
    }
    if is_iso_code(word) {
        let i = ISO_CODES.binary_search(&word).ok()?;
        return Some(CurrencyMatch { code: ISO_CODES[i], ambiguous: false });
    }
    None
}

/// ISO code for a currency symbol, code or word, e.g. `din.` → `RSD`, `R$` → `BRL`.
pub fn currency(word: &str) -> Option<&'static str> {
    currency_match(word).map(|m| m.code)
}

/// The decimal convention customary for amounts in this currency, when the currency area
/// agrees on one; `None` for mixed areas (EUR, where Ireland/Malta use `.`), or unknown codes.
/// Used only as a fallback when the column gives no evidence.
pub fn decimal_convention(code: &str) -> Option<DecimalSep> {
    match code {
        "USD" | "GBP" | "JPY" | "CNY" | "INR" | "AUD" | "CAD" | "NZD" | "HKD" | "SGD" | "MXN" | "KRW" | "ILS" | "THB"
        | "PHP" | "CHF" | "NGN" | "MYR" | "PKR" | "TWD" | "AED" | "SAR" => Some(DecimalSep::Dot),
        "RSD" | "BAM" | "MKD" | "HRK" | "BRL" | "PLN" | "CZK" | "HUF" | "RON" | "BGN" | "TRY" | "RUB" | "UAH" | "DKK"
        | "NOK" | "SEK" | "ISK" | "ARS" | "CLP" | "COP" | "IDR" | "VND" | "ZAR" | "KZT" | "UYU" => Some(DecimalSep::Comma),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_table_sorted_and_complete() {
        assert!(ISO_CODES.windows(2).all(|w| w[0] < w[1]));
        for c in ["USD", "EUR", "RSD", "PEN", "ALL", "XAU", "ZWG", "VES"] {
            assert!(is_iso_code(c), "{c}");
        }
        assert!(!is_iso_code("all"));
        assert!(!is_iso_code("XXX"));
        assert!(!is_iso_code("ABC"));
        assert_eq!(currency("PEN"), Some("PEN"));
        assert_eq!(currency("pen"), None);
    }

    #[test]
    fn symbols() {
        let cases = [
            ("$", "USD"), ("US$", "USD"), ("€", "EUR"), ("£", "GBP"), ("¥", "JPY"), ("₹", "INR"), ("₽", "RUB"),
            ("₺", "TRY"), ("₩", "KRW"), ("₪", "ILS"), ("zł", "PLN"), ("Kč", "CZK"), ("kr", "SEK"), ("kr.", "SEK"),
            ("Fr.", "CHF"), ("CHF", "CHF"), ("R$", "BRL"), ("C$", "CAD"), ("A$", "AUD"), ("HK$", "HKD"),
            ("din.", "RSD"), ("дин.", "RSD"), ("KM", "BAM"), ("Ft", "HUF"), ("lei", "RON"), ("лв.", "BGN"),
        ];
        for (s, c) in cases {
            assert_eq!(currency(s), Some(c), "{s:?}");
        }
        assert!(currency_match("kr").unwrap().ambiguous);
        assert!(currency_match("¥").unwrap().ambiguous);
        assert!(!currency_match("$").unwrap().ambiguous);
        assert!(!currency_match("SEK").unwrap().ambiguous);
        assert_eq!(currency("km"), None);
        assert_eq!(currency("kg"), None);
    }

    #[test]
    fn conventions() {
        assert_eq!(decimal_convention("USD"), Some(DecimalSep::Dot));
        assert_eq!(decimal_convention("RSD"), Some(DecimalSep::Comma));
        assert_eq!(decimal_convention("EUR"), None);
    }
}
