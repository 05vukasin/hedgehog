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
/// match (the other phrases need non-ASCII letters). The last group covers the other languages
/// handled by `values::total_score` (gesamt/insgesamt, zusammen, somme/soma/som, razem, ogolem,
/// lacznie, skupaj/skupno).
#[cfg(test)]
const NEEDLES: [&[u8]; 12] = [
    b"tot",
    b"sum",
    b"ukup",
    b"svega",
    b"zbir",
    b"gesamt",
    b"zusammen",
    b"som",
    b"razem",
    b"ogolem",
    b"lacznie",
    b"skup",
];

/// True if `b` contains one of [`NEEDLES`] (ASCII case-insensitive), in one pass dispatched on the
/// first letter.
fn ascii_may_hold_keyword(b: &[u8]) -> bool {
    let at = |rest: &[u8], n: &[u8]| rest.len() >= n.len() && rest[..n.len()].eq_ignore_ascii_case(n);
    (0..b.len()).any(|i| {
        let rest = &b[i..];
        match b[i].to_ascii_lowercase() {
            b't' => at(rest, b"tot"),
            b's' => at(rest, b"sum") || at(rest, b"svega") || at(rest, b"som") || at(rest, b"skup"),
            b'u' => at(rest, b"ukup"),
            b'z' => at(rest, b"zbir") || at(rest, b"zusammen"),
            b'g' => at(rest, b"gesamt"),
            b'r' => at(rest, b"razem"),
            b'o' => at(rest, b"ogolem"),
            b'l' => at(rest, b"lacznie"),
            _ => false,
        }
    })
}

/// [`ascii_may_hold_keyword`] for any text: non-ASCII text is checked on its diacritic- and
/// script-folded form ("Ogółem" → "ogolem", "Укупно" → "ukupno"), built in a reused buffer.
fn may_hold_keyword(text: &str) -> bool {
    if text.is_ascii() {
        return ascii_may_hold_keyword(text.as_bytes());
    }
    if text.contains(['Σ', 'σ', '∑']) {
        return true;
    }
    thread_local! {
        static FOLDED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    FOLDED.with_borrow_mut(|buf| {
        buf.clear();
        crate::values::text::fold_key_into(text, buf);
        ascii_may_hold_keyword(buf.as_bytes())
    })
}

/// True if `text` has more than `max` [`tokens`], counted without allocating.
fn more_tokens_than(text: &str, max: usize) -> bool {
    let mut n = 0;
    let mut in_token = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == 'σ' {
            n += !in_token as usize;
            in_token = true;
        } else {
            n += (ch == '∑') as usize;
            in_token = false;
        }
        if n > max {
            return true;
        }
    }
    false
}

fn contains_phrase(toks: &[String], phrase: &[&str]) -> bool {
    toks.windows(phrase.len()).any(|w| w.iter().zip(phrase).all(|(a, b)| a == b))
}

/// Returns the total level a short label announces ("Ukupno Vojvodina" → Total,
/// "Sub-total" → Subtotal, "GRAND TOTAL:" → GrandTotal), or `None`.
pub fn total_keyword(text: &str) -> Option<KeywordLevel> {
    if !may_hold_keyword(text) {
        return None; // fast path for the vast majority of labels: no allocation
    }
    if more_tokens_than(text, MAX_TOKENS) {
        return None; // a sentence: skip tokenizing it
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
    use super::{KeywordLevel::*, NEEDLES, ascii_may_hold_keyword, total_keyword};

    #[test]
    fn prefilter_matches_every_needle() {
        for n in NEEDLES {
            let mut up = b"x ".to_vec();
            up.extend(n.to_ascii_uppercase());
            assert!(ascii_may_hold_keyword(n) && ascii_may_hold_keyword(&up), "{}", String::from_utf8_lossy(n));
        }
        assert!(!ascii_may_hold_keyword(b"Revenue / Q1 to"));
    }

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
        assert_eq!(total_keyword("The total is computed from all rows above minus the returns of last year"), None);
    }
}
