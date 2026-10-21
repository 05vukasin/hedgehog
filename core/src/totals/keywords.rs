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
/// match (the other phrases need non-ASCII letters). The next group covers the other languages
/// handled by `values::total_score` (gesamt/insgesamt, zusammen, somme/soma/som, razem, ogolem,
/// lacznie, skupaj/skupno); the last one [`FOLDED_TOTAL`] (Czech, Finnish, Danish, Hungarian,
/// Russian, Estonian, Icelandic, Turkish) and the typos of "total" ([`TYPOS`]).
#[cfg(test)]
const NEEDLES: [&[u8]; 24] = [
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
    b"celk",
    b"yhteens",
    b"alt",
    b"osszes",
    b"vsego",
    b"itog",
    b"kokku",
    b"samtals",
    b"toplam",
    b"toal",
    b"ttal",
    b"otal",
];

/// True if `b` contains one of [`NEEDLES`] (ASCII case-insensitive), in one pass dispatched on the
/// first letter.
fn ascii_may_hold_keyword(b: &[u8]) -> bool {
    let at = |rest: &[u8], n: &[u8]| rest.len() >= n.len() && rest[..n.len()].eq_ignore_ascii_case(n);
    (0..b.len()).any(|i| {
        let rest = &b[i..];
        match b[i].to_ascii_lowercase() {
            b't' => at(rest, b"tot") || at(rest, b"toplam") || at(rest, b"toal") || at(rest, b"ttal"),
            b's' => {
                at(rest, b"sum") || at(rest, b"svega") || at(rest, b"som") || at(rest, b"skup") || at(rest, b"samtals")
            }
            b'u' => at(rest, b"ukup"),
            b'z' => at(rest, b"zbir") || at(rest, b"zusammen"),
            b'g' => at(rest, b"gesamt"),
            b'r' => at(rest, b"razem"),
            b'o' => at(rest, b"ogolem") || at(rest, b"osszes") || at(rest, b"otal"),
            b'l' => at(rest, b"lacznie"),
            b'c' => at(rest, b"celk"),
            b'y' => at(rest, b"yhteens"),
            b'a' => at(rest, b"alt"),
            b'v' => at(rest, b"vsego"),
            b'i' => at(rest, b"itog"),
            b'k' => at(rest, b"kokku"),
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

/// Total words in more languages, matched on the diacritic- and script-folded tokens
/// (`values::norm_key`) as the first or the last word of a label: Czech/Slovak `celkem`/`celkom`,
/// Finnish `yhteensä`, Hungarian `összesen`, Russian `всего`/`итого`, Polish `ogółem`/`łącznie`/
/// `razem`, Estonian `kokku`, Icelandic `samtals`, Turkish `toplam`, Swedish/Norwegian `totalt`,
/// Greek `σύνολο`, German `gesamt`/`insgesamt` (also after the group: "Deutschland gesamt",
/// "EU 27 insgesamt"), Serbian `ukupno`.
const FOLDED_TOTAL: &[&str] = &[
    "celkem", "celkom", "yhteensa", "osszesen", "vsego", "itogo", "itog", "ogolem", "lacznie", "razem", "kokku",
    "samtals", "toplam", "totalt", "gesamt", "insgesamt", "ukupno", "σύνολο", "συνολο", "σύνολα",
];

/// Folded grand-total phrases: "Total général", "Totaal generaal", "Totale complessivo".
const FOLDED_GRAND: &[&[&str]] = &[
    &["total", "general"],
    &["totaal", "generaal"],
    &["totale", "generale"],
    &["totale", "complessivo"],
    &["gran", "total"],
    &["celkem", "celkem"],
];

/// "total" with one letter dropped or two neighbours swapped ("Toal", "Totl", "Grand Totl"); not
/// "tota" (a word in several languages). Only in short labels.
const TYPOS: &[&str] = &["toal", "totl", "otal", "ttal", "totla", "toatl", "ottal", "ttoal"];

/// Labels with at most this many words may carry a typo of "total".
const TYPO_LABEL_MAX_WORDS: usize = 3;

/// Labels with at most this many words may carry a [`FOLDED_TOTAL`] word.
const FOLDED_LABEL_MAX_WORDS: usize = 5;

/// "T o t a l": three or more single letters separated by spaces, joined ("total").
fn spaced_letters(text: &str) -> Option<String> {
    let t = text.trim();
    if t.len() > 40 || !t.contains(' ') {
        return None;
    }
    let mut out = String::new();
    let mut n = 0;
    for w in t.split_whitespace() {
        let w = w.trim_end_matches([':', '.']);
        let mut cs = w.chars();
        match (cs.next(), cs.next()) {
            (Some(c), None) if c.is_alphabetic() => out.push(c),
            _ => return None,
        }
        n += 1;
    }
    (n >= 3).then_some(out)
}

/// Folded tokens of `text` without trailing footnote numbers ("Summe aller Produkte 2").
fn folded_tokens(text: &str) -> Vec<String> {
    let key = crate::values::norm_key(text);
    let mut toks: Vec<String> =
        key.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect();
    while toks.len() > 1 && toks.last().is_some_and(|w| w.len() <= 2 && w.bytes().all(|b| b.is_ascii_digit())) {
        toks.pop();
    }
    toks
}

/// [`FOLDED_TOTAL`], [`FOLDED_GRAND`] and [`TYPOS`] for a label that matched none of the English
/// and Serbian phrases.
fn more_languages(toks: &[String], text: &str) -> Option<KeywordLevel> {
    let folded = folded_tokens(text);
    if folded.is_empty() || folded.len() > MAX_TOKENS {
        return None;
    }
    if FOLDED_GRAND.iter().any(|p| contains_phrase(&folded, p)) {
        return Some(KeywordLevel::GrandTotal);
    }
    let edge = |w: &String| FOLDED_TOTAL.contains(&w.as_str());
    if folded.len() <= FOLDED_LABEL_MAX_WORDS {
        let (first, last) = (&folded[0], &folded[folded.len() - 1]);
        if edge(first) || edge(last) || contains_phrase(&folded, &["i", "alt"]) && (folded.len() == 2 || last == "alt")
        {
            return Some(KeywordLevel::Total);
        }
    }
    if toks.len() <= TYPO_LABEL_MAX_WORDS
        && let (Some(first), Some(last)) = (toks.first(), toks.last())
        && (TYPOS.contains(&first.as_str()) || TYPOS.contains(&last.as_str()))
    {
        return Some(if toks.iter().any(|w| w == "grand") { KeywordLevel::GrandTotal } else { KeywordLevel::Total });
    }
    None
}

/// Returns the total level a short label announces ("Ukupno Vojvodina" → Total,
/// "Sub-total" → Subtotal, "GRAND TOTAL:" → GrandTotal, "Deutschland gesamt", "Всего", "I alt",
/// "Toal" → Total), or `None`.
pub fn total_keyword(text: &str) -> Option<KeywordLevel> {
    keyword(text, true)
}

/// [`total_keyword`] without the words added for the normalized output ([`FOLDED_TOTAL`],
/// [`FOLDED_GRAND`], [`TYPOS`], spaced letters): the keywords the learned row model's total
/// features were trained on.
pub fn total_keyword_base(text: &str) -> Option<KeywordLevel> {
    keyword(text, false)
}

fn keyword(text: &str, more: bool) -> Option<KeywordLevel> {
    if !may_hold_keyword(text) {
        // fast path for the vast majority of labels: no allocation
        return if more { spaced_letters(text).and_then(|t| keyword(&t, true)) } else { None };
    }
    if more_tokens_than(text, MAX_TOKENS) {
        return None; // a sentence: skip tokenizing it
    }
    let toks = tokens(text);
    if toks.is_empty() || toks.len() > MAX_TOKENS {
        return None;
    }
    let any = |list: &[&[&str]]| list.iter().any(|p| contains_phrase(&toks, p));
    let folded_grand = || {
        more && toks.len() >= 2 && {
            let folded = folded_tokens(text);
            FOLDED_GRAND.iter().any(|p| contains_phrase(&folded, p))
        }
    };
    if any(GRAND) || folded_grand() {
        Some(KeywordLevel::GrandTotal)
    } else if any(SUBTOTAL) {
        Some(KeywordLevel::Subtotal)
    } else if any(TOTAL) {
        Some(KeywordLevel::Total)
    } else if more {
        more_languages(&toks, text).or_else(|| other_language_total(text))
    } else {
        other_language_total(text)
    }
}

/// Total words that are also ordinary words, names or codes: "Somme" (a French département),
/// "Sumas" (a place), "SOM" (Somalia), "SUMA Kommunikation", "Zusammen interreligiös".
const AMBIGUOUS: &[&str] = &["som", "suma", "sumas", "somme", "soma", "summa", "zusammen"];

/// True if the only total word of `text` is one of [`AMBIGUOUS`] (its first word): the label
/// names a total only if the row's numbers add up (see `totals::detect`).
pub fn ambiguous_keyword(text: &str) -> bool {
    let folded = folded_tokens(text);
    let Some((first, rest)) = folded.split_first() else { return false };
    AMBIGUOUS.contains(&first.as_str()) && rest.iter().all(|w| total_keyword(w).is_none())
}

/// A row that is neither data nor a sum of other rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NonSumKind {
    /// "of which: exports", "davon Inland", "dont : …": a part of the row above.
    OfWhich,
    /// "Average", "Mean", "Median", "Ø", "Durchschnitt", "Moyenne", "Prosek": a mean of the rows above.
    Average,
}

/// "of which" phrases (folded tokens) at the start of a label, in English, German, French,
/// Italian, Spanish, Portuguese, Serbian/Croatian/Slovenian, Polish, Czech, Dutch, Nordic,
/// Finnish, Hungarian and Russian.
const OF_WHICH: &[&[&str]] = &[
    &["of", "which"],
    &["of", "whom"],
    &["thereof"],
    &["whereof"],
    &["davon"],
    &["darunter"],
    &["hiervon"],
    &["dont"],
    &["di", "cui"],
    &["de", "los", "cuales"],
    &["de", "las", "cuales"],
    &["del", "cual"],
    &["de", "la", "cual"],
    &["dos", "quais"],
    &["das", "quais"],
    &["od", "toga"],
    &["od", "tega"],
    &["od", "cega"],
    &["od", "kojih"],
    &["w", "tym"],
    &["z", "toho"],
    &["z", "tego"],
    &["waarvan"],
    &["hiervan"],
    &["varav"],
    &["heraf"],
    &["hvorav"],
    &["herav"],
    &["josta"],
    &["ebbol"],
    &["iz", "nih"],
    &["v", "tom", "cisle"],
    &["u", "tomu", "cisli"],
];

/// Average words (folded), as the first word of a label.
const AVERAGE: &[&str] = &[
    "average", "avg", "mean", "median", "durchschnitt", "mittelwert", "moyenne", "mediane", "media", "mediana",
    "promedio", "prosek", "prosecno", "prosjek", "prosjecno", "sredina", "srednee", "srednia", "gemiddelde",
    "gemiddeld", "gennemsnit", "gjennomsnitt", "genomsnitt", "keskiarvo", "atlag", "prumer", "priemer", "povprecje",
];

/// Average labels have at most this many words ("Average", "Ø 2012", "Average North region").
const AVERAGE_LABEL_MAX_WORDS: usize = 4;

/// Of-which labels have at most this many words.
const OF_WHICH_LABEL_MAX_WORDS: usize = 8;

/// How plainly a label announces an average row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NonSumLabel {
    /// The average sign, alone or before a period: "Ø", "Ø 2012", "⌀ 24".
    Sign,
    /// Only the word, maybe before a year: "Average", "Mean:", "Durchschnitt 2023".
    Bare,
    /// The word and more: "Average North", "of which: exports".
    Phrase,
}

/// First words (folded) of [`AVERAGE`] and [`OF_WHICH`]: everything else is rejected after
/// folding one word.
fn nonsum_first_word(w: &str) -> bool {
    AVERAGE.contains(&w) || OF_WHICH.iter().any(|p| p[0] == w)
}

/// The kind of non-sum row a label announces ("of which: …", "Average", "Ø 2012") and how plainly
/// ([`NonSumLabel`]), or `None`.
pub fn nonsum_keyword(text: &str) -> Option<(NonSumKind, NonSumLabel)> {
    let t = text.trim().trim_start_matches(['-', '–', '—', '•', '·', '(', ' ', '\u{a0}']);
    if t.is_empty() || t.len() > 160 {
        return None;
    }
    // The average sign, not a word starting with Ø ("Østfold").
    let mut cs = t.chars();
    if matches!(cs.next(), Some('Ø' | 'ø' | '⌀')) {
        let rest = cs.as_str();
        if rest.chars().next().is_some_and(char::is_alphabetic) {
            return None;
        }
        let words = rest.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).count();
        let sign = rest.chars().all(|n| !n.is_alphabetic());
        return (words < AVERAGE_LABEL_MAX_WORDS)
            .then_some((NonSumKind::Average, if sign { NonSumLabel::Sign } else { NonSumLabel::Phrase }));
    }
    if more_tokens_than(t, OF_WHICH_LABEL_MAX_WORDS) {
        return None;
    }
    // Fold the first word only (no allocation for most labels).
    let first_end = t.find(|c: char| !c.is_alphanumeric()).unwrap_or(t.len());
    if first_end == 0 {
        return None;
    }
    thread_local! {
        static FIRST: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    let candidate = FIRST.with_borrow_mut(|buf| {
        buf.clear();
        crate::values::text::fold_key_into(&t[..first_end], buf);
        nonsum_first_word(buf)
    });
    if !candidate {
        return None;
    }
    let toks = folded_tokens(t);
    let first = toks.first()?;
    if AVERAGE.contains(&first.as_str()) && toks.len() <= AVERAGE_LABEL_MAX_WORDS {
        let bare = toks[1..].iter().all(|w| w.bytes().all(|b| b.is_ascii_digit()));
        return Some((NonSumKind::Average, if bare { NonSumLabel::Bare } else { NonSumLabel::Phrase }));
    }
    if OF_WHICH.iter().any(|p| toks.len() >= p.len() && toks.iter().zip(p.iter()).all(|(a, b)| a == b)) {
        return Some((NonSumKind::OfWhich, NonSumLabel::Phrase));
    }
    None
}

/// Sheets spanning at least this many rows are long enough to be registers (one record per row),
/// where a total word inside a free-text cell is part of the record ("Prótese total anca").
pub const REGISTER_MIN_ROWS: u32 = 200;

/// Words in a short total label ("Total", "Grand total", "Ukupno Vojvodina", "Summe:"); longer
/// texts with a total word are descriptions or names when they sit right of other text.
const SHORT_LABEL_MAX_WORDS: usize = 2;

/// True if `text` has at most [`SHORT_LABEL_MAX_WORDS`] words.
fn short_label(text: &str) -> bool {
    !more_tokens_than(text, SHORT_LABEL_MAX_WORDS)
}

/// A total word that does not, by itself, make its row a total row: in a long sheet (a register,
/// at least [`REGISTER_MIN_ROWS`] rows from its first to its last cell), a text longer than a
/// short label, with other text left of it in the row, is a free-text value (a description, a
/// supplier's name: "AQUISIÇÃO DE UMA ESTAÇÃO TOTAL", "GLOBAL 2 SEGURANÇA TOTAL LDA"), not the row's
/// label. Such a row is a total only if its numbers add up (see `totals::detect`).
///
/// The same holds for a description or name that is the row's first text but has other text right
/// of it, once it is longer than [`LABEL_WITH_TEXT_RIGHT_MAX_WORDS`] words: "Total knee replacement
/// implant | Medisupply Ltd", "BEECH TREE TOTAL CARE LIMITED | 22 Church Road, …". A total label
/// rarely has more text beside it.
///
/// `text_left` / `text_right`: the row holds a non-numeric text cell left / right of this one;
/// `sheet_rows`: rows from the sheet's first to its last cell.
pub fn incidental_keyword(text: &str, text_left: bool, text_right: bool, sheet_rows: u32) -> bool {
    sheet_rows >= REGISTER_MIN_ROWS
        && ((text_left && !short_label(text))
            || (text_right && more_tokens_than(text, LABEL_WITH_TEXT_RIGHT_MAX_WORDS)))
}

/// Words a register's first text may have and still be a total label with other text right of it
/// ("Total Region North | Contracts 2020").
const LABEL_WITH_TEXT_RIGHT_MAX_WORDS: usize = 3;

/// The text starts or ends with a total word ("Summe der bezogenen Rohstoffe", "Totaal van aantal
/// eenheden", "Umsatz insgesamt"), as a total label does, rather than holding one in the middle.
/// Trailing footnote numbers ("Summe aller Produkte 2") are ignored.
pub fn keyword_at_edge(text: &str) -> bool {
    let mut toks = tokens(text);
    while toks.last().is_some_and(|w| w.len() <= 2 && w.bytes().all(|b| b.is_ascii_digit())) {
        toks.pop();
    }
    let edge = |w: Option<&String>| w.is_some_and(|w| total_keyword(w).is_some());
    edge(toks.first()) || edge(toks.last())
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
    fn incidental_total_words() {
        use super::{incidental_keyword as inc, REGISTER_MIN_ROWS as N};
        // Free text right of other text in a register: descriptions, suppliers' names.
        assert!(inc("AQUISIÇÃO DE UMA ESTAÇÃO TOTAL", true, false, N));
        assert!(inc("Prótese total anca", true, false, 176_513));
        assert!(inc("507210360 - GLOBAL 2 SEGURANÇA TOTAL LDA", true, false, N));
        // The first text with more text right of it, longer than a label: a description or name.
        assert!(inc("BEECH TREE TOTAL CARE LIMITED", false, true, N));
        assert!(inc("Total knee replacement implant", false, true, N));
        assert!(!inc("Total Region North", false, true, N));
        // Short labels, the row's first text, and short sheets (reports) keep their keyword.
        for s in ["Total", "TOTAL:", "Grand total", "Ukupno Vojvodina", "Gesamt", "Summe", "Sub-total"] {
            assert!(!inc(s, true, true, N), "{s}");
        }
        assert!(!inc("AQUISIÇÃO DE UMA ESTAÇÃO TOTAL", false, false, N));
        assert!(!inc("Prótese total anca", true, true, N - 1));
    }

    #[test]
    fn more_languages_typos_and_ambiguous_words() {
        use super::{ambiguous_keyword as amb, total_keyword_base as base};
        for s in ["Всего", "ИТОГО", "Celkem", "Razem", "I alt", "Yhteensä", "Összesen", "Σύνολο", "∑", "Toal", "Totl",
            "T o t a l", "Totalt", "Kokku", "Ogółem", "Deutschland gesamt", "EU 27 insgesamt", "Nord gesamt",
            "Razem Belgia", "Danmark i alt", "Praha celkem"]
        {
            assert!(total_keyword(s).is_some(), "{s}");
        }
        assert_eq!(total_keyword("Grand Totl"), Some(GrandTotal));
        assert_eq!(total_keyword("Total général"), Some(GrandTotal));
        assert_eq!(total_keyword("Totaal generaal"), Some(GrandTotal));
        for s in ["Tota", "Tonal", "Totalitarian regimes", "Alternative fuels", "Gesamtschule Nord", "Altona"] {
            assert_eq!(total_keyword(s), None, "{s}");
        }
        // The row model's keywords stay as they were trained.
        assert_eq!(base("Всего"), None);
        assert_eq!(base("Deutschland gesamt"), None);
        assert_eq!(base("Total"), Some(Total));
        // Names and codes that hold a total word.
        for s in ["Somme", "Sumas", "SOM", "SUMA Kommunikation", "Zusammen interreligiös"] {
            assert!(amb(s), "{s}");
        }
        for s in ["Suma total", "Total", "Summe", "Gesamt"] {
            assert!(!amb(s), "{s}");
        }
    }

    #[test]
    fn of_which_and_average_labels() {
        use super::{nonsum_keyword as ns, NonSumKind::*, NonSumLabel::*};
        for s in ["of which: domestic", "  of which exports", "davon Inland", "davon: Export", "od toga: izvoz",
            "dont : exportations", "di cui: estero", "w tym: eksport", "thereof abroad", "- of which online", "из них"]
        {
            assert_eq!(ns(s), Some((OfWhich, Phrase)), "{s}");
        }
        assert_eq!(ns("Ø 2012"), Some((Average, Sign)));
        assert_eq!(ns("Ø"), Some((Average, Sign)));
        assert_eq!(ns("Average"), Some((Average, Bare)));
        assert_eq!(ns("Avg."), Some((Average, Bare)));
        assert_eq!(ns("Średnia"), Some((Average, Bare)));
        assert_eq!(ns("Average North"), Some((Average, Phrase)));
        assert_eq!(ns("Median"), Some((Average, Bare)));
        for s in ["Østfold", "Durchschnittshöhe in Euro", "Average hourly earnings of all full-time employees", "Dontworry",
            "Of course", "Total"]
        {
            assert_eq!(ns(s), None, "{s}");
        }
    }

    #[test]
    fn total_word_at_the_edge() {
        use super::keyword_at_edge as edge;
        assert!(edge("Summe der bezogenen Rohstoffe"));
        assert!(edge("Totaal van aantal eenheden"));
        assert!(edge("ANDERSON PETRO Total"));
        assert!(edge("Summe aller Produkte einschl. besondere Finanzierungsvorgänge 2"));
        assert!(!edge("Volumendring. Sum tre måneder, rullerende"));
        assert!(!edge("Prótese total anca"));
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
