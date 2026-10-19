//! Text cleanup and normalization keys (irregularities V9, S1, S4, S5).
//!
//! * [`trim`] / [`clean`] — strip NBSP, zero-width and other invisible characters, collapse runs
//!   of whitespace (tabs, line breaks) into one space.
//! * [`cyrillic_to_latin`] — deterministic Serbian Cyrillic → Latin (`Београд` → `Beograd`,
//!   `Љубовија` → `Ljubovija`); a few Russian/Macedonian letters are mapped too.
//! * [`norm_key`] — case-, script- and diacritic-insensitive key used to cluster variants:
//!   `Beograd`, ` BEOGRAD `, `Београд` all map to `beograd`; `Đorđević` → `djordjevic`.
//! * [`fingerprint`] — OpenRefine-style key: [`norm_key`] with punctuation removed and tokens
//!   sorted and de-duplicated (`"Novi Sad, grad"` == `"grad Novi-Sad"`).

use std::borrow::Cow;

/// Characters that render as nothing: zero-width space/joiners, word joiner, BOM, soft hyphen,
/// Mongolian vowel separator.
pub fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}' | '\u{180E}')
}

/// Unicode whitespace (includes NBSP U+00A0, thin/narrow spaces) or an invisible character.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || is_invisible(c)
}

/// Trims whitespace, NBSP and invisible characters from both ends. Never allocates.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// True if the text starts or ends with whitespace/invisible characters (but is not blank).
pub fn is_padded(s: &str) -> bool {
    let t = trim(s);
    !t.is_empty() && t.len() != s.len()
}

/// True if the text is empty or consists only of whitespace/invisible characters.
pub fn is_blank(s: &str) -> bool {
    trim(s).is_empty()
}

/// Trimmed text with invisible characters removed and every whitespace run (spaces, NBSP, tabs,
/// line breaks) collapsed to one ASCII space. Borrows when the input is already clean.
pub fn clean(s: &str) -> Cow<'_, str> {
    let t = trim(s);
    let mut prev_space = false;
    let needs_work = t.chars().any(|c| {
        let bad = is_invisible(c) || (c.is_whitespace() && (c != ' ' || prev_space));
        prev_space = c == ' ';
        bad
    });
    if !needs_work {
        return Cow::Borrowed(t);
    }
    let mut out = String::with_capacity(t.len());
    let mut pending_space = false;
    for c in t.chars() {
        if is_invisible(c) {
            continue;
        }
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(c);
    }
    Cow::Owned(out)
}

/// True if any character is in the Cyrillic block.
pub fn has_cyrillic(s: &str) -> bool {
    s.chars().any(is_cyrillic)
}

pub fn is_cyrillic(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{04FF}')
}

/// Latin transliteration of one lowercase Cyrillic letter (Serbian standard, plus common
/// Russian/Macedonian/Ukrainian letters). `None` for non-Cyrillic characters.
fn cyr_lower(c: char) -> Option<&'static str> {
    Some(match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'ђ' => "đ",
        'е' => "e",
        'ж' => "ž",
        'з' => "z",
        'и' => "i",
        'ј' => "j",
        'к' => "k",
        'л' => "l",
        'љ' => "lj",
        'м' => "m",
        'н' => "n",
        'њ' => "nj",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'ћ' => "ć",
        'у' => "u",
        'ф' => "f",
        'х' => "h",
        'ц' => "c",
        'ч' => "č",
        'џ' => "dž",
        'ш' => "š",
        // Non-Serbian letters seen in regional data.
        'й' => "j",
        'ы' => "y",
        'э' => "e",
        'ю' => "ju",
        'я' => "ja",
        'ё' => "e",
        'щ' => "šč",
        'ъ' | 'ь' => "",
        'ѓ' => "đ",
        'ќ' => "ć",
        'ѕ' => "dz",
        'є' => "je",
        'і' => "i",
        'ї' => "ji",
        'ґ' => "g",
        _ => return None,
    })
}

/// Transliterates Cyrillic letters to Serbian Latin, preserving case (`Љ` → `Lj`). Borrows when
/// the text has no Cyrillic.
pub fn cyrillic_to_latin(s: &str) -> Cow<'_, str> {
    if !has_cyrillic(s) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if !is_cyrillic(c) {
            out.push(c);
            continue;
        }
        let lower = c.to_lowercase().next().unwrap_or(c);
        match cyr_lower(lower) {
            Some(lat) if lower != c => {
                // Uppercase: capitalize the first output letter only (Љ → Lj).
                let mut it = lat.chars();
                if let Some(f) = it.next() {
                    out.extend(f.to_uppercase());
                    out.push_str(it.as_str());
                }
            }
            Some(lat) => out.push_str(lat),
            None => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Appends the diacritic-free form of a lowercase Latin character (`š`→`s`, `đ`→`dj`,
/// `č`/`ć`→`c`, `ž`→`z`, `é`→`e`, `ß`→`ss`, …). Combining marks are dropped.
fn push_folded(c: char, out: &mut String) {
    let s: &str = match c {
        'š' | 'ś' | 'ş' | 'ș' => "s",
        'đ' => "dj",
        'č' | 'ć' | 'ç' => "c",
        'ž' | 'ź' | 'ż' => "z",
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ą' | 'ă' | 'ā' => "a",
        'æ' => "ae",
        'è' | 'é' | 'ê' | 'ë' | 'ę' | 'ě' | 'ē' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ı' | 'ī' => "i",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ő' | 'ō' => "o",
        'œ' => "oe",
        'ù' | 'ú' | 'û' | 'ü' | 'ů' | 'ű' | 'ū' => "u",
        'ý' | 'ÿ' => "y",
        'ß' => "ss",
        'ł' | 'ľ' | 'ĺ' => "l",
        'ř' | 'ŕ' => "r",
        'ť' | 'ț' => "t",
        'ď' => "d",
        'ğ' => "g",
        'ǆ' => "dz",
        'ǉ' => "lj",
        'ǌ' => "nj",
        '\u{0300}'..='\u{036F}' => "",
        _ => {
            out.push(c);
            return;
        }
    };
    out.push_str(s);
}

fn push_key_char(c: char, out: &mut String) {
    for l in c.to_lowercase() {
        match cyr_lower(l) {
            Some(lat) => {
                for lc in lat.chars() {
                    push_folded(lc, out);
                }
            }
            None => push_folded(l, out),
        }
    }
}

/// Case-, script- and diacritic-insensitive key: trimmed, invisible characters removed,
/// whitespace collapsed, Cyrillic transliterated, lowercased, diacritics folded.
/// `"  Београд "`, `"BEOGRAD"`, `"beograd"` → `"beograd"`; `"Đorđević"` → `"djordjevic"`.
pub fn norm_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in trim(s).chars() {
        if is_invisible(c) {
            continue;
        }
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        push_key_char(c, &mut out);
    }
    out
}

/// OpenRefine-style fingerprint: [`norm_key`] split on anything that is not a letter or digit,
/// tokens sorted and de-duplicated, joined with one space.
pub fn fingerprint(s: &str) -> String {
    let key = norm_key(s);
    let mut tokens: Vec<&str> = key.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    tokens.sort_unstable();
    tokens.dedup();
    tokens.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_invisible_and_nbsp() {
        assert_eq!(trim("\u{A0} abc\u{200B}\t"), "abc");
        assert_eq!(trim("\u{FEFF}"), "");
        assert!(is_blank(" \u{A0}\u{200B} "));
        assert!(!is_blank(" a "));
        assert!(is_padded(" a"));
        assert!(is_padded("a\u{A0}"));
        assert!(!is_padded("a b"));
        assert!(!is_padded("   "));
    }

    #[test]
    fn clean_collapses_and_borrows() {
        assert!(matches!(clean("already clean"), Cow::Borrowed(_)));
        assert!(matches!(clean("  padded  "), Cow::Borrowed("padded")));
        assert_eq!(clean("a \u{A0}\t b\nc\u{200B}d"), "a b cd");
        assert_eq!(clean("a  b"), "a b");
    }

    #[test]
    fn cyrillic_transliteration() {
        assert!(has_cyrillic("Београд"));
        assert!(!has_cyrillic("Beograd"));
        assert_eq!(cyrillic_to_latin("Београд"), "Beograd");
        assert_eq!(cyrillic_to_latin("Љубовија Њива Џеп"), "Ljubovija Njiva Džep");
        assert_eq!(cyrillic_to_latin("Ђорђевић Чачак Шабац Жабаљ Ћуприја"), "Đorđević Čačak Šabac Žabalj Ćuprija");
        assert_eq!(cyrillic_to_latin("укупно 2024"), "ukupno 2024");
        assert!(matches!(cyrillic_to_latin("latin"), Cow::Borrowed(_)));
    }

    #[test]
    fn norm_key_clusters_variants() {
        for v in ["Beograd", "BEOGRAD", " beograd ", "Београд", "БЕОГРАД", "Beo\u{200B}grad"] {
            assert_eq!(norm_key(v), "beograd", "{v:?}");
        }
        assert_eq!(norm_key("Đorđević"), "djordjevic");
        assert_eq!(norm_key("Ђорђевић"), "djordjevic");
        assert_eq!(norm_key("Čačak"), "cacak");
        assert_eq!(norm_key("Чачак"), "cacak");
        assert_eq!(norm_key("Novi  \u{A0}Sad"), "novi sad");
        assert_eq!(norm_key("Žabalj Šid Ćuprija"), "zabalj sid cuprija");
        assert_eq!(norm_key("Ljig"), norm_key("Љиг"));
        assert_eq!(norm_key("Crème Brûlée"), "creme brulee");
        assert_eq!(norm_key("Cafe\u{301}"), "cafe");
        assert_eq!(norm_key(""), "");
    }

    #[test]
    fn fingerprint_orders_tokens() {
        assert_eq!(fingerprint("Novi Sad, grad"), fingerprint("grad Novi-Sad"));
        assert_eq!(fingerprint("d.o.o. Petrović"), "d o petrovic");
    }
}
