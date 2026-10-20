//! Header and title text for agents: flattened column names without footnote marks and
//! line-break hyphenation, navigation links that must not become a table title, and header words
//! that mark a column as not additive (rates, shares, per-capita values, indices).
//!
//! The original header cells stay untouched in the manifest (`original_header`); only the
//! flattened `name` is cleaned.

use crate::values::date::is_period_label;
use crate::values::{clean, norm_key};

/// Words after a line-break hyphen that are a separate word, not the rest of a split one
/// ("Ein-\nund Ausfuhr", "Import-\nor export").
const CONJUNCTIONS: [&str; 18] = [
    "und",
    "u",
    "oder",
    "bzw",
    "beziehungsweise",
    "sowie",
    "resp",
    "respektive",
    "and",
    "or",
    "et",
    "ou",
    "y",
    "e",
    "i",
    "o",
    "en",
    "of",
];

/// Right halves of real hyphenated compounds written without a line break ("Follow-up",
/// "E-mail", "Pre-tax"): never joined.
const COMPOUND_TAILS: [&str; 24] = [
    "up", "in", "on", "off", "out", "to", "by", "end", "based", "free", "level", "term", "time",
    "year", "date", "type", "related", "specific", "wide", "owned", "made", "mail", "tax", "food",
];

/// Prefixes of real hyphenated compounds ("Non-food", "Self-employed", "Co-op").
const COMPOUND_HEADS: [&str; 12] = [
    "non", "pre", "post", "self", "co", "re", "ex", "anti", "sub", "semi", "multi", "inter",
];

/// A flattened column-name part: whitespace collapsed, line-break hyphenation joined
/// (`De-\nzember` → `Dezember`, `Kalender-\nfaktoren` → `Kalenderfaktoren`, `Sep-tember` →
/// `September`), and footnote marks removed (`20221)` → `2022`, `Rate3)` → `Rate`, `Total²` →
/// `Total`, `Einwohner*` → `Einwohner`). A part that is nothing but a mark is kept as it is.
pub(super) fn clean_header(raw: &str) -> String {
    let joined = join_hyphenation(raw);
    let text = clean(&joined).into_owned();
    let stripped = strip_footnote(&text);
    if stripped.is_empty() { text } else { stripped }
}

/// Joins words split by a hyphen at a line break, and words split by a hyphen without a break
/// when the right half starts lowercase and the result is not a real compound.
fn join_hyphenation(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '-' && i > 0 && chars[i - 1].is_alphabetic() {
            // Whitespace after the hyphen, and whether it holds a line break.
            let mut j = i + 1;
            let mut newline = false;
            while j < chars.len() && (chars[j].is_whitespace()) {
                newline |= matches!(chars[j], '\n' | '\r' | '\u{2028}');
                j += 1;
            }
            let gap = j > i + 1;
            if j < chars.len() && chars[j].is_alphabetic() {
                let left = word_before(&chars, i);
                let right = word_after(&chars, j);
                let lower = chars[j].is_lowercase();
                let right_key = norm_key(&right);
                let left_key = norm_key(&left);
                let separate = CONJUNCTIONS.contains(&right_key.as_str());
                // A hyphen at a line break (or before a space, where the break was): the word goes on.
                if (newline || gap) && lower && !separate {
                    // "Kalender-\nfaktoren" → "Kalenderfaktoren", "Benützungs- gebühren"
                    i = j;
                    continue;
                }
                if (newline || gap) && !separate {
                    // "Abgaben-\nErtragsanteile" → "Abgaben-Ertragsanteile"
                    out.push('-');
                    i = j;
                    continue;
                }
                if !gap && lower && !separate {
                    let month = is_period_label(&format!("{left}{right}"));
                    let compound = COMPOUND_TAILS.contains(&right_key.as_str())
                        || COMPOUND_HEADS.contains(&left_key.as_str())
                        || right.chars().count() <= 2
                        || left.chars().all(|c| !c.is_lowercase());
                    if month || (!compound && left.chars().next().is_some_and(char::is_uppercase)) {
                        // "Sep-tember", "Nachlassverbindlich-keiten"
                        i = j;
                        continue;
                    }
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn word_before(chars: &[char], end: usize) -> String {
    let mut k = end;
    while k > 0 && chars[k - 1].is_alphabetic() {
        k -= 1;
    }
    chars[k..end].iter().collect()
}

fn word_after(chars: &[char], start: usize) -> String {
    chars[start..]
        .iter()
        .take_while(|c| c.is_alphabetic())
        .collect()
}

fn is_superscript_mark(c: char) -> bool {
    matches!(
        c,
        '¹' | '²'
            | '³'
            | '⁴'
            | '⁵'
            | '⁶'
            | '⁷'
            | '⁸'
            | '⁹'
            | '⁰'
            | '⁾'
            | '⁽'
            | '*'
            | '†'
            | '‡'
    )
}

/// Removes one trailing footnote mark: superscript digits / `*` / `†`, or an unmatched `1)` /
/// `12)` / `a)` glued to a word, after a space, or glued to a year (`20221)` → `2022`).
fn strip_footnote(s: &str) -> String {
    let t = s.trim_end_matches(is_superscript_mark).trim_end();
    if t.len() != s.len() {
        return t.to_string();
    }
    let Some(body) = s.strip_suffix(')') else {
        return s.to_string();
    };
    if s.matches('(').count() >= s.matches(')').count() {
        return s.to_string(); // "(2020=100)", "(in 1 000)"
    }
    let chars: Vec<char> = body.chars().collect();
    let mut k = chars.len();
    while k > 0 && chars[k - 1].is_ascii_digit() {
        k -= 1;
    }
    let digits: String = chars[k..].iter().collect();
    let head: String = chars[..k].iter().collect();
    if digits.is_empty() {
        // " a)": one letter after a space.
        let n = chars.len();
        if n >= 3 && chars[n - 1].is_ascii_lowercase() && chars[n - 2] == ' ' {
            return chars[..n - 2]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_string();
        }
        return s.to_string();
    }
    let before = head.chars().last();
    match before {
        // "Rate3)", "Einwohner 1)"
        Some(c) if (c.is_alphabetic() || c == ' ' || c == '.') && digits.len() <= 2 => {
            head.trim_end().to_string()
        }
        // "20221)", "201912)": a year with the mark glued on.
        None | Some(' ' | '/' | '-') if (5..=6).contains(&digits.len()) => {
            let year: u32 = digits[..4].parse().unwrap_or(0);
            if (1900..=2100).contains(&year) {
                format!("{head}{}", &digits[..4])
            } else {
                s.to_string()
            }
        }
        _ => s.to_string(),
    }
}

/// Navigation links that sit above many statistical tables ("zur Inhaltsübersicht", "Back to
/// contents", "Retour au sommaire", "Powrót do spisu tablic"). Never a table title.
pub(super) fn is_nav_link(text: &str) -> bool {
    if text.is_empty() || text.len() > 120 || !text.chars().any(char::is_alphabetic) {
        return false;
    }
    let key = norm_key(text);
    let key: String = key
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if key.is_empty() || key.len() > 60 {
        return false;
    }
    const EXACT: [&str; 22] = [
        "inhalt",
        "overzicht",
        "contents",
        "content",
        "sommaire",
        "inhoud",
        "indice",
        "index",
        "spis tablic",
        "spis tresci",
        "innhold",
        "innehall",
        "sadrzaj",
        "home",
        "back",
        "zuruck",
        "retour",
        "terug",
        "torna",
        "volver",
        "powrot",
        "menu",
    ];
    if EXACT.contains(&key.as_str()) {
        return true;
    }
    const LEADS: [&str; 22] = [
        "zur ",
        "zum ",
        "zuruck zu",
        "back to ",
        "go to ",
        "return to ",
        "retour au ",
        "retour a ",
        "retour vers ",
        "powrot do ",
        "terug naar ",
        "naar ",
        "torna a",
        "torna all",
        "volver a",
        "voltar a",
        "tilbake til ",
        "tillbaka till ",
        "nazad na ",
        "povratak na ",
        "ritorno a",
        "regresar a",
    ];
    const TARGETS: [&str; 22] = [
        "inhalt",
        "ubersicht",
        "verzeichnis",
        "contents",
        "index",
        "overview",
        "menu",
        "sommaire",
        "table des",
        "spisu",
        "overzicht",
        "inhoud",
        "indice",
        "innhold",
        "innehall",
        "sadrzaj",
        "pocetak",
        "start",
        "home",
        "top",
        "tabellen",
        "tables",
    ];
    LEADS.iter().any(|l| key.starts_with(l)) && TARGETS.iter().any(|t| key.contains(t))
}

/// Header or label words that mark values as not additive: rates, shares, percentages, indices,
/// averages and per-capita values ("je Einwohner", "j.E.", "per capita", "Anteil", "Quote", "%").
pub(super) fn not_additive(text: &str) -> bool {
    if text.contains('%') || text.contains('‰') {
        return true;
    }
    let key = norm_key(text);
    // "(2020=100)": an index base.
    let compact: String = key.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(i) = compact.find("=100") {
        let lead: String = compact[..i].chars().rev().take(4).collect();
        if lead.len() == 4 && lead.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    let tokens: Vec<&str> = key
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    const WORDS: [&str; 36] = [
        "anteil",
        "quote",
        "rate",
        "index",
        "indeks",
        "indice",
        "share",
        "ratio",
        "percent",
        "percentage",
        "prozent",
        "pourcentage",
        "porcentaje",
        "percentuale",
        "procent",
        "odsetek",
        "wskaznik",
        "durchschnitt",
        "average",
        "avg",
        "mean",
        "mittelwert",
        "moyenne",
        "media",
        "promedio",
        "gemiddelde",
        "srednia",
        "prosek",
        "density",
        "dichte",
        "vpi",
        "hvpi",
        "cpi",
        "hicp",
        "capita",
        "kopf",
    ];
    for (k, t) in tokens.iter().enumerate() {
        if WORDS.contains(t) {
            return true;
        }
        if t.len() >= 8
            && [
                "srate",
                "nrate",
                "squote",
                "nquote",
                "index",
                "durchschnitt",
            ]
            .iter()
            .any(|s| t.ends_with(s))
        {
            return true;
        }
        let next = tokens.get(k + 1).copied();
        let per = match (*t, next) {
            // German "je" / "pro" always mean "per": "je Einwohner", "je 1 000", "pro Kopf".
            ("je" | "pro", Some(_)) => true,
            // "j.E.", "j. E."
            ("j", Some("e")) => true,
            // Elsewhere "per" / "par" / "por" also mean "by" ("per sesso", "par âge"): only with a
            // per-capita or per-thousand word.
            ("per" | "par" | "por" | "na", Some(n)) => [
                "capita",
                "head",
                "person",
                "persona",
                "inhabitant",
                "inhabitants",
                "resident",
                "habitant",
                "habitante",
                "habitantes",
                "abitante",
                "tete",
                "mieszkanca",
                "inwoner",
                "1000",
                "100",
                "cent",
                "mille",
                "km",
                "km2",
            ]
            .contains(&n),
            _ => false,
        };
        if per {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footnotes() {
        assert_eq!(clean_header("20221)"), "2022");
        assert_eq!(clean_header("Veränderungsrate3)"), "Veränderungsrate");
        assert_eq!(clean_header("Sonstige Steuern 1)"), "Sonstige Steuern");
        assert_eq!(clean_header("Total²"), "Total");
        assert_eq!(
            clean_header("Liczba beneficjentów*"),
            "Liczba beneficjentów"
        );
        assert_eq!(clean_header("Einwohner a)"), "Einwohner");
        // real numbers and balanced parentheses stay
        assert_eq!(
            clean_header("Gesamt-VPI (2020=100)"),
            "Gesamt-VPI (2020=100)"
        );
        assert_eq!(clean_header("in 1 000"), "in 1 000");
        assert_eq!(clean_header("2022"), "2022");
        assert_eq!(clean_header("1)"), "1)");
        assert_eq!(clean_header("*"), "*");
        assert_eq!(clean_header("Wert (in Mio. EUR)"), "Wert (in Mio. EUR)");
        assert_eq!(clean_header("Code 123)"), "Code 123)");
    }

    #[test]
    fn hyphenation() {
        assert_eq!(clean_header("De-\nzember"), "Dezember");
        assert_eq!(clean_header("Kalender-\nfaktoren"), "Kalenderfaktoren");
        assert_eq!(clean_header("Sep-tember"), "September");
        assert_eq!(clean_header("De-zember / 20211)"), "Dezember / 2021");
        assert_eq!(
            clean_header("Nachlassverbindlich-keiten"),
            "Nachlassverbindlichkeiten"
        );
        assert_eq!(
            clean_header("Abgaben-\nErtragsanteile 2)"),
            "Abgaben-Ertragsanteile"
        );
        assert_eq!(clean_header("Ein-\nund Ausfuhr"), "Ein- und Ausfuhr");
        assert_eq!(clean_header("Ein- und Ausfuhr"), "Ein- und Ausfuhr");
        assert_eq!(clean_header("Follow-up"), "Follow-up");
        assert_eq!(clean_header("Non-food"), "Non-food");
        assert_eq!(clean_header("E-mail"), "E-mail");
        assert_eq!(clean_header("EU-weit"), "EU-weit");
        assert_eq!(clean_header("Gesamt-VPI"), "Gesamt-VPI");
        assert_eq!(clean_header("Nord-Süd"), "Nord-Süd");
        assert_eq!(clean_header("  Umsatz \n  2024 "), "Umsatz 2024");
        assert_eq!(
            clean_header("Benützungs- gebühren nach FAG"),
            "Benützungsgebühren nach FAG"
        );
        assert_eq!(
            clean_header("Interessenten- Beiträge"),
            "Interessenten-Beiträge"
        );
        assert_eq!(
            clean_header("Lohn- bzw. Gehaltssumme"),
            "Lohn- bzw. Gehaltssumme"
        );
        assert_eq!(clean_header("Revenue - Q1"), "Revenue - Q1");
    }

    #[test]
    fn nav_links() {
        for s in [
            "zur Inhaltsübersicht",
            "Back to contents",
            "Retour au sommaire",
            "Powrót do spisu tablic",
            "Overzicht",
            "Inhalt",
            "<< zurück zum Inhaltsverzeichnis",
            "← Back to index",
        ] {
            assert!(is_nav_link(s), "{s}");
        }
        for s in [
            "Inhaltsübersicht",
            "Wanderungen 2024",
            "Tabelle 1: Inhalt der Lieferungen",
            "Index of prices 2024",
        ] {
            assert!(!is_nav_link(s), "{s}");
        }
    }

    #[test]
    fn additivity() {
        for s in [
            "Grundsteuer A / j.E.",
            "Einwohner je km²",
            "Ausgaben je Einwohner",
            "per capita",
            "Pro Kopf",
            "Anteil in %",
            "Arbeitslosenquote",
            "Veränderungsrate",
            "Gesamt-VPI (2020=100)",
            "Index",
            "Durchschnitt",
        ] {
            assert!(not_additive(s), "{s}");
        }
        for s in [
            "Umsatz",
            "Abgaben- Ertragsanteile / EURO",
            "Corporate",
            "Einwohner",
            "Betrag in 1 000 EUR",
            "Insgesamt",
            "Popolazione per sesso",
            "Population par âge",
            "Mittel des Bundes",
        ] {
            assert!(!not_additive(s), "{s}");
        }
    }
}
