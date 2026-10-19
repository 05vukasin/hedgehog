//! Keyword cues (en, sr/hr/bs/sl, de, fr, es, it, nl, pt, pl) for row roles: totals (R1–R3), notes/sources (L7), enumerated labels (H5).
//! Matching is on whole words of [`norm_key`], so Cyrillic and diacritic variants match too.

use super::text::{norm_key, trim};

/// Total keywords: en, sr/hr/bs/sl, de, fr, es, it, nl, pt, pl, nordic (diacritics folded).
const TOTAL_WORDS: &[&str] = &[
    // en
    "total", "totals", "subtotal", "subtotals", "grandtotal", "sum", "tot", "ttl", "all",
    // sr / hr / bs / sl
    "ukupno", "ukupna", "ukupan", "ukupni", "ukupne", "sveukupno", "svega", "zbir", "zbirno", "medjuzbir", "skupaj", "skupno",
    // de
    "summe", "gesamt", "insgesamt", "gesamtsumme", "zwischensumme", "endsumme", "gesamtbetrag", "zusammen",
    // fr / es / it / pt
    "somme", "totaux", "suma", "sumas", "totales", "totale", "totali", "subtotale", "soma",
    // nl / nordic / pl
    "totaal", "subtotaal", "som", "totalt", "razem", "ogolem", "lacznie",
    "sigma", "σ",
];

/// Note / source keywords in the same languages.
const NOTE_WORDS: &[&str] = &[
    // en
    "note", "notes", "nb", "source", "sources", "footnote", "footnotes", "remark", "remarks", "comment", "comments",
    "legend", "disclaimer",
    // sr / hr / bs / sl
    "napomena", "napomene", "nap", "izvor", "izvori", "fusnota", "fusnote", "komentar", "primedba", "primedbe",
    "objasnjenje", "legenda", "pojasnjenje", "opomba", "opombe", "vir", "viri",
    // de
    "anmerkung", "anmerkungen", "hinweis", "hinweise", "quelle", "quellen", "fussnote", "fussnoten", "erlauterung",
    "erlauterungen", "bemerkung", "bemerkungen",
    // fr / es / it / pt
    "remarque", "remarques", "avertissement", "nota", "notas", "fuente", "fuentes", "observacion", "observaciones",
    "aclaracion", "aclaraciones", "fonte", "fontes", "fonti", "osservazioni", "avvertenza", "observacao",
    "observacoes",
    // nl / pl
    "bron", "bronnen", "opmerking", "opmerkingen", "toelichting", "zrodlo", "zrodla", "uwaga", "uwagi", "przypis",
];

fn words(key: &str) -> impl Iterator<Item = &str> {
    key.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty())
}

/// Total/subtotal keyword score: 1.0 if the first word is a total keyword (`Total`, `Ukupno:`,
/// `Укупно`, `Sub-total`, `Σ`), 0.5 if one appears later (`Revenue total`), 0.0 otherwise.
/// `all` counts only as `All …` at the start of a short label ("All regions").
pub fn total_score(s: &str) -> f32 {
    let t = trim(s);
    if t.is_empty() || t.len() > 120 {
        return 0.0;
    }
    if t.starts_with('Σ') || t.starts_with('∑') {
        return 1.0;
    }
    let key = norm_key(t);
    let mut ws = words(&key);
    let Some(first) = ws.next() else { return 0.0 };
    // "Sub-total", "Grand total", "sous-total", "gran total".
    let first_word = if matches!(first, "sub" | "grand" | "sous" | "gran") { ws.next().unwrap_or("") } else { first };
    if TOTAL_WORDS.contains(&first_word) && (first_word != "all" || key.len() < 30) {
        return 1.0;
    }
    if ws.any(|w| w != "all" && TOTAL_WORDS.contains(&w)) {
        return 0.5;
    }
    0.0
}

/// True if the text starts like a note or source line: a note keyword as first word
/// (`Note:`, `Izvor:`, `Napomena`, `Source - RZS`), or a footnote marker (`*`, `**`, `†`, `¹`,
/// `(1)` / `1)` followed by text).
pub fn is_note(s: &str) -> bool {
    let t = trim(s);
    if t.is_empty() {
        return false;
    }
    let first = t.chars().next().unwrap();
    if matches!(first, '*' | '†' | '‡' | '¹' | '²' | '³' | '⁴' | '⁵' | '⁶' | '⁷' | '⁸' | '⁹') {
        return t.chars().any(|c| c.is_alphabetic());
    }
    if t.len() > 400 {
        return false;
    }
    let key = norm_key(t);
    words(&key).next().is_some_and(|w| NOTE_WORDS.contains(&w))
}

/// True if a label starts with an enumerator followed by text: `1. Prihodi`, `1.2 Plate`,
/// `2) Troškovi`, `(3) Ostalo`, `a) item`, `IV. Rashodi`, `- item`, `• item`. Numbers such as
/// `1.200,50` or `2024` do not count.
pub fn starts_with_enumerator(s: &str) -> bool {
    let t = trim(s);
    let b = t.as_bytes();
    if b.is_empty() {
        return false;
    }
    let rest: &str;
    let first = t.chars().next().unwrap();
    if matches!(first, '-' | '–' | '—' | '•' | '·' | '▪' | '►' | '‣' | '◦' | '○' | '■') {
        rest = &t[first.len_utf8()..];
        return rest.starts_with(char::is_whitespace) && rest.chars().any(char::is_alphabetic);
    }
    let paren_open = b[0] == b'(';
    let i0 = paren_open as usize;
    // Marker body: dotted digits (1, 1.2, 1.2.3), a single letter, or a Roman numeral.
    let body_end = {
        let mut i = i0;
        if b.get(i).is_some_and(|c| c.is_ascii_digit()) {
            loop {
                let start = i;
                while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
                    i += 1;
                }
                if i - start > 3 {
                    return false;
                }
                // continue "1.2" only if a digit follows the dot
                if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
                    i += 1;
                    continue;
                }
                break;
            }
            i
        } else {
            let romans = b[i..].iter().take_while(|c| matches!(c, b'I' | b'V' | b'X' | b'i' | b'v' | b'x')).count();
            if (1..=5).contains(&romans) {
                i + romans
            } else if b.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
                i + 1
            } else {
                return false;
            }
        }
    };
    let is_digit_body = b[i0].is_ascii_digit();
    let mut j = body_end;
    match b.get(j) {
        Some(b')') => j += 1,
        Some(b'.') if !paren_open => j += 1,
        // "1.2 Plate": multi-level number without a trailing dot
        Some(b' ') if is_digit_body && !paren_open && t[i0..body_end].contains('.') => {}
        _ => return false,
    }
    if paren_open && b[j - 1] != b')' {
        return false;
    }
    let after = &t[j..];
    after.starts_with(char::is_whitespace) && after.trim_start().chars().next().is_some_and(char::is_alphabetic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals() {
        for s in ["Total", "TOTAL:", "Grand Total", "Sub-total", "Subtotal Q1", "Ukupno", "UKUPNO:", "Укупно", "СВЕГА", "Svega", "Sveukupno", "Σ", "Sum", "All regions", "Međuzbir"] {
            assert_eq!(total_score(s), 1.0, "{s:?}");
        }
        for s in ["Revenue total", "Prihodi ukupno", "Rashodi - svega"] {
            assert_eq!(total_score(s), 0.5, "{s:?}");
        }
        for s in ["Totally", "Summary", "Beograd", "", "123", "Tottenham", "Call all"] {
            assert_eq!(total_score(s), 0.0, "{s:?}");
        }
    }

    #[test]
    fn notes() {
        for s in ["Note: values in RSD", "Izvor: RZS", "Napomena", "NAPOMENA:", "Напомена: подаци", "Source - World Bank", "* preliminary data", "¹ estimate", "Izvori podataka"] {
            assert!(is_note(s), "{s:?}");
        }
        for s in ["Notebook", "Beograd", "*", "", "Total", "Sources of revenue are listed".replace("Sources", "Many").as_str()] {
            assert!(!is_note(s), "{s:?}");
        }
    }

    #[test]
    fn european_keywords() {
        for s in [
            "Gesamt", "Summe", "Insgesamt", "Zwischensumme", "Totaal", "Totale", "Totali", "Suma", "Somme", "Total général",
            "Sous-total", "Gran total", "Razem", "Ogółem", "Łącznie", "Skupaj", "Totalt", "Soma",
        ] {
            assert_eq!(total_score(s), 1.0, "{s:?}");
        }
        assert_eq!(total_score("Umsatz gesamt"), 0.5);
        assert_eq!(total_score("Montant total"), 0.5);
        for s in [
            "Anmerkung: vorläufig", "Quelle: Destatis", "Hinweis", "Remarque : provisoire", "Source : INSEE", "Nota: datos",
            "Fuente: INE", "Fonte: ISTAT", "Bron: CBS", "Opmerking", "Źródło: GUS", "Uwaga", "Observação", "Fußnote 1",
        ] {
            assert!(is_note(s), "{s:?}");
        }
    }

    #[test]
    fn enumerators() {
        for s in ["1. Prihodi", "12) Troškovi", "(3) Ostalo", "a) item", "B. Section", "IV. Rashodi", "iv) foo", "1.2 Plate", "1.2.3. Sub", "- item", "• bullet", "(a) note"] {
            assert!(starts_with_enumerator(s), "{s:?}");
        }
        for s in ["1.200,50", "2024", "2024 Budget", "1 kg", "-5", "Beograd", "1.", "a)", "(3 Ostalo", "1234. foo", "12.03.2024", "(1)", "ab) x", "1.2"] {
            assert!(!starts_with_enumerator(s), "{s:?}");
        }
    }
}
