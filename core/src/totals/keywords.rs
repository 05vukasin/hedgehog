//! Total keywords in English and Serbian (Latin, Latin without diacritics, Cyrillic).

/// Which kind of total a label announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordLevel {
    Total,
    Subtotal,
    GrandTotal,
}

/// Phrases as token sequences, checked in this order (most specific first).
const GRAND: &[&[&str]] = &[
    &["grand", "total"],
    &["grandtotal"],
    &["overall", "total"],
    &["total", "general"],
    &["sveukupno"],
    &["sve", "ukupno"],
    &["ukupno", "svega"],
    &["свеукупно"],
    &["све", "укупно"],
    &["укупно", "свега"],
];

const SUBTOTAL: &[&[&str]] = &[
    &["subtotal"],
    &["subtotals"],
    &["sub", "total"],
    &["s", "total"],
    &["međuzbir"],
    &["medjuzbir"],
    &["meduzbir"],
    &["међузбир"],
];

const TOTAL: &[&[&str]] = &[
    &["total"],
    &["totals"],
    &["tot"],
    &["sum"],
    &["σ"],
    &["ukupno"],
    &["ukupan"],
    &["ukupna"],
    &["svega"],
    &["zbir"],
    &["укупно"],
    &["укупан"],
    &["укупна"],
    &["свега"],
    &["збир"],
];

/// Labels longer than this many tokens are sentences / notes, not total labels.
const MAX_TOKENS: usize = 8;

fn tokens(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in lower.chars() {
        if ch.is_alphanumeric() || ch == 'σ' {
            cur.push(ch);
        } else if ch == '∑' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push("σ".to_string());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Every ASCII phrase above contains one of these (lower-case) substrings: `total`, `totals`,
/// `tot`, `grandtotal`, `subtotal` → `tot`; `sum`; `ukupno`, `ukupan`, `ukupna`, `sveukupno` →
/// `ukup`; `svega`; `zbir`, `medjuzbir`, `meduzbir` → `zbir`. Text without any of them cannot
/// match (the other phrases need non-ASCII letters).
fn ascii_may_hold_keyword(b: &[u8]) -> bool {
    // The last group covers the other languages handled by `values::total_score`
    // (gesamt/insgesamt, zusammen, somme/soma/som, razem, ogolem, lacznie, skupaj/skupno).
    const NEEDLES: [&[u8]; 12] = [
        b"tot", b"sum", b"ukup", b"svega", b"zbir", b"gesamt", b"zusammen", b"som", b"razem", b"ogolem", b"lacznie",
        b"skup",
    ];
    NEEDLES.iter().any(|n| b.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n)))
}

fn contains_phrase(toks: &[String], phrase: &[&str]) -> bool {
    toks.windows(phrase.len()).any(|w| w.iter().zip(phrase).all(|(a, b)| a == b))
}

/// Returns the total level a short label announces ("Ukupno Vojvodina" → Total,
/// "Sub-total" → Subtotal, "GRAND TOTAL:" → GrandTotal), or `None`.
pub fn total_keyword(text: &str) -> Option<KeywordLevel> {
    if text.is_ascii() && !ascii_may_hold_keyword(text.as_bytes()) {
        return None; // fast path for the vast majority of labels: no allocation
    }
    let toks = tokens(text);
    if toks.is_empty() || toks.len() > MAX_TOKENS {
        return None;
    }
    let any = |list: &[&[&str]]| list.iter().any(|p| contains_phrase(&toks, p));
    if any(GRAND) {
        Some(KeywordLevel::GrandTotal)
    } else if any(SUBTOTAL) {
        Some(KeywordLevel::Subtotal)
    } else if any(TOTAL) {
        Some(KeywordLevel::Total)
    } else {
        other_language_total(text)
    }
}

/// German, French, Spanish, Italian, Dutch, Portuguese, Polish and Nordic labels via the shared
/// multilingual list in `values` ("Summe", "Insgesamt", "Somme", "Totale", "Razem", "Zwischensumme").
/// Only a keyword at the start of the label counts, and "All …" is left out (too often a category).
fn other_language_total(text: &str) -> Option<KeywordLevel> {
    if crate::values::keywords::total_score(text) < 1.0 {
        return None;
    }
    let key = crate::values::norm_key(text);
    let first = key.split(|c: char| !c.is_alphanumeric()).find(|w| !w.is_empty()).unwrap_or("");
    if first == "all" {
        return None;
    }
    let sub = ["zwischensumme", "subtotale", "subtotaal", "sous", "subtotales", "medjuzbir"];
    if sub.contains(&first) || first.starts_with("zwischen") {
        Some(KeywordLevel::Subtotal)
    } else {
        Some(KeywordLevel::Total)
    }
}

#[cfg(test)]
mod tests {
    use super::{total_keyword, KeywordLevel::*};

    #[test]
    fn keywords() {
        assert_eq!(total_keyword("Total"), Some(Total));
        assert_eq!(total_keyword("TOTAL:"), Some(Total));
        assert_eq!(total_keyword("Total revenue"), Some(Total));
        assert_eq!(total_keyword("Ukupno Vojvodina"), Some(Total));
        assert_eq!(total_keyword("Укупно"), Some(Total));
        assert_eq!(total_keyword("Svega"), Some(Total));
        assert_eq!(total_keyword("Σ"), Some(Total));
        assert_eq!(total_keyword("Sub-total"), Some(Subtotal));
        assert_eq!(total_keyword("Subtotal East"), Some(Subtotal));
        assert_eq!(total_keyword("Međuzbir"), Some(Subtotal));
        assert_eq!(total_keyword("Grand Total"), Some(GrandTotal));
        assert_eq!(total_keyword("Summe"), Some(Total));
        assert_eq!(total_keyword("Insgesamt"), Some(Total));
        assert_eq!(total_keyword("Gesamt:"), Some(Total));
        assert_eq!(total_keyword("Somme"), Some(Total));
        assert_eq!(total_keyword("Totale"), Some(Total));
        assert_eq!(total_keyword("Totaal"), Some(Total));
        assert_eq!(total_keyword("Razem"), Some(Total));
        assert_eq!(total_keyword("Ogółem"), Some(Total));
        assert_eq!(total_keyword("Zwischensumme"), Some(Subtotal));
        assert_eq!(total_keyword("All regions"), None);
        assert_eq!(total_keyword("Gesamtschule Nord"), None);
        assert_eq!(total_keyword("Sveukupno"), Some(GrandTotal));
        assert_eq!(total_keyword("СВЕУКУПНО"), Some(GrandTotal));
        assert_eq!(total_keyword("Summary"), None);
        assert_eq!(total_keyword("MEDJUZBIR"), Some(Subtotal));
        assert_eq!(total_keyword("Overall TOTAL"), Some(GrandTotal));
        assert_eq!(total_keyword("Sve ukupno:"), Some(GrandTotal));
        assert_eq!(total_keyword("SUM"), Some(Total));
        assert_eq!(total_keyword("Price"), None);
        assert_eq!(total_keyword("Totally unrelated sentence"), None);
        assert_eq!(total_keyword("Beograd"), None);
        assert_eq!(
            total_keyword("The total is computed from all rows above minus the returns of last year"),
            None
        );
    }
}
