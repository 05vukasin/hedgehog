//! B150: which tables a question is about, by cheap deterministic text matching (no model, no
//! embeddings). The question's words are folded (lowercase, accents removed, Serbian Cyrillic
//! transliterated), function words dropped and the rest stemmed with a few suffix rules shared by
//! English, German, French, Spanish, Portuguese, Italian, Dutch and Serbian. A table scores by the
//! words it shares with the question (title and sheet name weigh most, then column names, then
//! label and code values), each word weighted by how few tables have it (IDF), plus the years and
//! months of the question that its time coverage holds. A small lexicon of statistical concepts
//! (GDP, population, revenue, …) links an English question to German, French, … names.

use std::collections::HashSet;

/// Lowercase, accents folded (`Nächtigungen` → `nachtigungen`), `ß` → `ss`, Serbian (and Russian)
/// Cyrillic written in Latin letters (`Порези` → `porezi`).
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars().flat_map(char::to_lowercase) {
        let rep: &str = match ch {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' | 'ă' => "a",
            'ç' | 'ć' | 'č' => "c",
            'ď' | 'đ' => "d",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
            'ł' | 'ľ' | 'ĺ' => "l",
            'ñ' | 'ń' | 'ň' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
            'ŕ' | 'ř' => "r",
            'ś' | 'š' | 'ş' | 'ș' => "s",
            'ť' | 'ţ' | 'ț' => "t",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
            'ý' | 'ÿ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            'ß' => "ss",
            'æ' => "ae",
            'œ' => "oe",
            'а' => "a",
            'б' => "b",
            'в' => "v",
            'г' => "g",
            'д' => "d",
            'ђ' => "dj",
            'е' | 'э' | 'ё' => "e",
            'ж' | 'з' => "z",
            'и' => "i",
            'ј' | 'й' => "j",
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
            'ћ' | 'ч' | 'ц' => "c",
            'у' => "u",
            'ф' => "f",
            'х' => "h",
            'џ' => "dz",
            'ш' | 'щ' => "s",
            'ы' => "y",
            'ю' => "ju",
            'я' => "ja",
            'ь' | 'ъ' => "",
            _ => {
                out.push(ch);
                continue;
            }
        };
        out.push_str(rep);
    }
    out
}

/// The words of a text, folded: runs of letters and digits, camelCase split
/// (`precoContratual` → `preco`, `contratual`).
pub fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && prev_lower && !cur.is_empty() {
                out.push(fold(&std::mem::take(&mut cur)));
            }
            prev_lower = ch.is_lowercase();
            cur.push(ch);
        } else {
            prev_lower = false;
            if !cur.is_empty() {
                out.push(fold(&std::mem::take(&mut cur)));
            }
        }
    }
    if !cur.is_empty() {
        out.push(fold(&cur));
    }
    out
}

/// Suffixes stripped by [`stem`], longest first: plural and inflection endings of the eight
/// languages (`-ungen`, `-ations`, `-ies`, `-es`, `-en`, `-s`, `-a`, `-o`, `-i`, …).
const SUFFIXES: &[&str] = &[
    "ations", "ements", "ungen", "ation", "ments", "ement", "ment", "ings", "ions", "ing", "ion", "ies", "oes", "ung", "es", "en",
    "er", "ed", "os", "as", "e", "s", "a", "o", "i", "u", "y",
];

/// A folded word without one inflection ending, keeping at least 3 letters (`salaries` → `salar`,
/// `nachtigungen` → `nachtig`, `porezi` → `porez`). Numbers and words of up to 3 letters stay.
pub fn stem(w: &str) -> String {
    if w.chars().count() <= 3 || w.chars().all(|c| c.is_ascii_digit()) {
        return w.to_string();
    }
    for s in SUFFIXES {
        if let Some(base) = w.strip_suffix(s)
            && base.chars().count() >= 3
            // `preis`, `basis`, `bus`, `class` keep their s.
            && !(*s == "s" && base.ends_with(['i', 'u', 's']))
        {
            return base.to_string();
        }
    }
    w.to_string()
}

/// Function words and question words of the eight languages (folded), never matched.
const STOP: &[&str] = &[
    // en
    "a", "an", "the", "and", "or", "of", "in", "on", "at", "to", "for", "from", "by", "with", "per", "each", "this",
    "these", "that", "those", "there", "their", "its", "it", "as", "than", "then", "into", "over", "between", "both",
    "any", "all", "only", "also", "not", "no", "yes", "what", "which", "who", "whom", "how", "many", "much", "when",
    "where", "why", "did", "does", "do", "was", "were", "is", "are", "be", "been", "has", "have", "had", "most", "more",
    "less", "least", "highest", "lowest", "largest", "smallest", "biggest", "top", "give", "show", "list", "compare",
    "compared", "versus", "vs", "previous", "latest", "first", "last", "same", "other", "one", "two", "three", "file",
    "files", "table", "tables", "sheet", "sheets", "data", "value", "values", "number", "amount", "including", "excluding",
    "together", "overall", "across", "against", "after", "before", "during", "about", "if", "so", "up", "down", "go",
    "went", "rise", "rose", "fall", "fell", "change", "changed", "did", "get", "got", "let", "say", "each", "every",
    "they", "them", "we", "our", "you", "your", "he", "she", "his", "her", "out", "money", "much", "whole",
    // total words (every table with total rows has them)
    "total", "totals", "sum", "gesamt", "insgesamt", "summe", "ukupno", "totale", "totaal",
    // de
    "der", "die", "das", "den", "dem", "des", "ein", "eine", "einer", "einen", "und", "oder", "von", "vom", "im", "am",
    "an", "auf", "aus", "bei", "mit", "nach", "fur", "zu", "zum", "zur", "ist", "sind", "war", "waren", "wie", "viel",
    "viele", "welche", "welcher", "welches", "wann", "wo", "was", "wer", "nicht", "auch", "je", "pro",
    // fr
    "le", "la", "les", "un", "une", "du", "de", "et", "ou", "en", "au", "aux", "pour", "par", "sur", "dans", "est",
    "sont", "quel", "quelle", "quels", "quelles", "combien", "ce", "cette", "ces", "qui", "que",
    // es / pt / it
    "el", "los", "las", "una", "del", "y", "o", "por", "para", "con", "es", "son", "cual", "cuales", "cuanto", "cuantos",
    "os", "as", "um", "uma", "do", "da", "dos", "das", "e", "em", "no", "na", "nos", "nas", "com", "qual", "quais",
    "quanto", "quantos", "il", "lo", "gli", "di", "della", "dei", "delle", "che", "quale", "quanti",
    // nl
    "het", "een", "en", "op", "van", "voor", "met", "zijn", "wat", "welke", "hoeveel",
    // sr (Latin, folded)
    "i", "u", "na", "za", "od", "sa", "je", "su", "koji", "koja", "koje", "koliko", "sto", "iz", "ili",
];

fn is_stop(w: &str) -> bool {
    STOP.contains(&w)
}

/// Statistical concepts and their names in the eight languages (folded); a question word that is
/// one of them also looks for the others (`GDP` → `Bruttoinlandsprodukt`, `PIB`, …).
const CONCEPTS: &[&[&str]] = &[
    &["gdp", "bip", "bruttoinlandsprodukt", "inlandsprodukt", "pib", "bdp"],
    &["population", "inhabitants", "residents", "bevolkerung", "einwohner", "populacao", "poblacion", "popolazione", "bevolking", "stanovnistvo", "stanovnika"],
    &["unemployment", "unemployed", "arbeitslos", "arbeitslosenquote", "erwerbslos", "erwerbslosenquote", "chomage", "desemprego", "desempleo", "disoccupazione", "werkloosheid", "nezaposlenost", "nezaposleni"],
    &["employment", "employed", "erwerbstatig", "erwerbstatige", "beschaftigung", "emploi", "emprego", "empleo", "occupazione", "werkgelegenheid", "zaposlenost"],
    &["employees", "workers", "staff", "personnel", "pessoal", "personal", "beschaftigte", "mitarbeiter", "trabalhadores", "funcionarios", "empleados", "dipendenti", "zaposleni", "radnici", "personeel", "effectifs"],
    &["deficit", "surplus", "defizit", "uberschuss", "finanzierungssaldo", "saldo", "excedente", "excedent", "suficit", "manjak", "visak"],
    &["revenue", "revenues", "income", "receipts", "einnahmen", "ertrag", "ertrage", "erlose", "recettes", "receitas", "ingresos", "entrate", "prihodi", "primanja", "ontvangsten"],
    &["expenses", "expenditure", "expenditures", "spending", "costs", "charges", "encargos", "ausgaben", "aufwand", "aufwendungen", "kosten", "depenses", "despesas", "gastos", "spese", "rashodi", "izdaci", "uitgaven"],
    &["tax", "taxes", "steuer", "steuern", "impot", "impots", "imposto", "impostos", "impuesto", "impuestos", "imposta", "imposte", "porez", "porezi", "belasting"],
    &["vat", "mwst", "umsatzsteuer", "mehrwertsteuer", "tva", "iva", "pdv", "btw"],
    &["price", "prices", "preis", "preise", "prix", "preco", "precos", "precio", "precios", "prezzo", "prezzi", "cena", "cene", "prijs", "prijzen"],
    &["inflation", "cpi", "vpi", "hicp", "hvpi", "verbraucherpreisindex", "ipc", "ipca", "inflacija", "inflatie", "inflacao", "inflacion", "inflazione"],
    &["salary", "salaries", "wage", "wages", "pay", "earnings", "remuneracao", "remuneracoes", "vencimento", "retribucion", "retribuzione", "lohn", "lohne", "gehalt", "gehalter", "verdienst", "salaire", "salaires", "salario", "salarios", "stipendio", "plata", "zarada", "loon"],
    &["women", "woman", "female", "females", "frauen", "weiblich", "femmes", "mulheres", "mujeres", "donne", "zene", "vrouwen"],
    &["men", "man", "male", "males", "manner", "mannlich", "hommes", "homens", "hombres", "uomini", "muskarci", "mannen"],
    &["profit", "profits", "gewinn", "benefice", "lucro", "beneficio", "utile", "dobit", "winst"],
    &["sales", "umsatz", "verkauf", "ventes", "vendas", "ventas", "vendite", "prodaja", "verkoop"],
    &["budget", "haushalt", "voranschlag", "budzet", "orcamento", "presupuesto", "bilancio", "begroting"],
    &["age", "alter", "idade", "edad", "eta", "starost", "leeftijd", "ans"],
    &["overnight", "nachtigungen", "ubernachtungen", "nuitees", "dormidas", "pernoctaciones", "pernottamenti", "nocenja", "overnachtingen"],
    &["arrivals", "ankunfte", "arrivees", "chegadas", "llegadas", "arrivi", "dolasci", "aankomsten"],
    &["daily", "day", "days", "taglich", "tag", "tage", "datum", "journalier", "jour", "diario", "dia", "giornaliero", "giorno", "dnevni", "dan", "dagelijks", "dag", "date"],
    &["monthly", "month", "months", "monatlich", "monat", "mensuel", "mois", "mensal", "mes", "mensual", "mensile", "mese", "mesecni", "mesec", "maandelijks", "maand"],
    &["annual", "yearly", "year", "years", "jahrlich", "jahr", "annuel", "annee", "anual", "ano", "annuale", "anno", "godisnji", "godina", "jaarlijks", "jaar"],
    &["country", "countries", "land", "lander", "pays", "pais", "paises", "paese", "zemlja", "drzava"],
    &["region", "regions", "state", "states", "bundesland", "bundeslander", "regiao", "regione", "oblast", "provincie"],
    &["supplier", "suppliers", "vendor", "lieferant", "fournisseur", "fornecedor", "proveedor", "fornitore", "dobavljac", "leverancier", "adjudicatario", "adjudicatarios"],
    &["people", "persons", "personen", "personnes", "pessoas", "personas", "persone", "osobe", "lica", "mensen"],
    &["government", "staat", "staates", "etat", "governo", "gobierno", "vlada", "overheid"],
    &["contract", "contracts", "vertrag", "vertrage", "contrat", "contrato", "contratos", "contratto", "ugovor", "contract"],
];

/// Month names (folded) of the eight languages, by month number.
const MONTHS: &[(&str, u32)] = &[
    ("january", 1), ("jan", 1), ("januar", 1), ("janvier", 1), ("janeiro", 1), ("enero", 1), ("gennaio", 1), ("januari", 1), ("januar", 1),
    ("february", 2), ("feb", 2), ("februar", 2), ("fevrier", 2), ("fevereiro", 2), ("febrero", 2), ("febbraio", 2), ("februari", 2),
    ("march", 3), ("mar", 3), ("marz", 3), ("mars", 3), ("marco", 3), ("marzo", 3), ("maart", 3), ("mart", 3),
    ("april", 4), ("apr", 4), ("avril", 4), ("abril", 4), ("aprile", 4),
    ("may", 5), ("mai", 5), ("maio", 5), ("mayo", 5), ("maggio", 5), ("mei", 5), ("maj", 5),
    ("june", 6), ("jun", 6), ("juni", 6), ("juin", 6), ("junho", 6), ("junio", 6), ("giugno", 6),
    ("july", 7), ("jul", 7), ("juli", 7), ("juillet", 7), ("julho", 7), ("julio", 7), ("luglio", 7),
    ("august", 8), ("aug", 8), ("aout", 8), ("agosto", 8), ("augustus", 8), ("avgust", 8),
    ("september", 9), ("sep", 9), ("sept", 9), ("septembre", 9), ("setembro", 9), ("septiembre", 9), ("settembre", 9), ("septembar", 9),
    ("october", 10), ("oct", 10), ("oktober", 10), ("octobre", 10), ("outubro", 10), ("octubre", 10), ("ottobre", 10), ("oktobar", 10),
    ("november", 11), ("nov", 11), ("novembre", 11), ("novembro", 11), ("noviembre", 11), ("novembar", 11),
    ("december", 12), ("dec", 12), ("dezember", 12), ("decembre", 12), ("dezembro", 12), ("diciembre", 12), ("dicembre", 12), ("decembar", 12),
];

fn month_of(w: &str) -> Option<u32> {
    MONTHS.iter().find(|(m, _)| *m == w).map(|(_, k)| *k)
}

/// One word to look for: its stem, how much a match counts, the word as the question has it, and
/// its group (a question word and the other names of its concept count once per table).
#[derive(Debug, Clone)]
pub struct Term {
    pub key: String,
    pub weight: f64,
    pub shown: String,
    pub group: usize,
}

/// A question, parsed: the words to look for, the years and the (year, month) periods it names.
#[derive(Debug, Clone, Default)]
pub struct Question {
    pub terms: Vec<Term>,
    pub years: Vec<i32>,
    pub months: Vec<(i32, u32)>,
    /// Tables the question names by number: `Table 1`, `Tabelle 2.3` → `1`, `2.3`.
    pub numbers: Vec<String>,
}

/// "Table" in the eight languages (folded).
const TABLE_WORDS: &[&str] = &["table", "tabelle", "tableau", "tabla", "tabela", "tabella", "tabel", "tab"];

/// `Table 1`, `Tabelle 2.3`, `tableau 4` in a text: the numbers.
fn table_numbers(text: &str) -> Vec<String> {
    let low = fold(text);
    let toks: Vec<&str> = low.split_whitespace().map(|t| t.trim_matches(|c: char| !c.is_alphanumeric() && c != '.')).collect();
    let mut out: Vec<String> = Vec::new();
    for w in toks.windows(2) {
        let num = w[1].trim_end_matches('.');
        if TABLE_WORDS.contains(&w[0]) && !num.is_empty() && num.chars().all(|c| c.is_ascii_digit() || c == '.') && !out.iter().any(|x| x == num) {
            out.push(num.to_string());
        }
    }
    out
}

/// A term matches a table word when the stems are equal, or (stems of 5+ letters) one starts with
/// the other (`employ` / `employe`), or a term of 6+ letters is part of a compound
/// (`inlandsprodukt` in `bruttoinlandsprodukt`). Words of up to 3 letters (codes such as `VAT`,
/// `KSB`) match only exactly.
fn matches(key: &str, word: &str, word_stem: &str) -> f64 {
    if key == word_stem || key == word {
        return 1.0;
    }
    let (kl, sl) = (key.len(), word_stem.len());
    if kl >= 5 && sl >= 5 && (word_stem.starts_with(key) || key.starts_with(word_stem)) {
        return 0.9;
    }
    if kl >= 6 && word.len() > kl && word.contains(key) {
        return 0.5;
    }
    0.0
}

impl Question {
    pub fn parse(text: &str) -> Question {
        let mut q = Question { numbers: table_numbers(text), ..Question::default() };
        // Acronyms of 2 letters ("EU", "UK") count only when written in capitals.
        let raw: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        let caps: HashSet<String> =
            raw.iter().filter(|w| w.chars().count() == 2 && w.chars().all(char::is_uppercase)).map(|w| fold(w)).collect();
        let ws = words(text);
        let mut seen: HashSet<String> = HashSet::new();
        let mut group = 0;
        for (i, w) in ws.iter().enumerate() {
            if w.len() == 4 && w.chars().all(|c| c.is_ascii_digit()) {
                let y: i32 = w.parse().unwrap_or(0);
                if (1900..=2100).contains(&y) && !q.years.contains(&y) {
                    q.years.push(y);
                }
                continue;
            }
            // A month with a year right after it: `January 2002`, `Juli 2026`.
            if let Some(m) = month_of(w)
                && let Some(y) = ws.get(i + 1).and_then(|n| n.parse::<i32>().ok()).filter(|y| (1900..=2100).contains(y))
            {
                if !q.months.contains(&(y, m)) {
                    q.months.push((y, m));
                }
                continue;
            }
            if w.chars().all(|c| c.is_ascii_digit()) || is_stop(w) || (w.chars().count() < 3 && !caps.contains(w)) {
                continue;
            }
            let key = stem(w);
            if !seen.insert(key.clone()) {
                continue;
            }
            group += 1;
            q.terms.push(Term { key, weight: 1.0, shown: w.clone(), group });
            // The concept's other names, weighed a little less.
            for names in CONCEPTS {
                if names.iter().any(|g| matches(&stem(w), g, &stem(g)) >= 0.9) {
                    for g in names.iter() {
                        let k = stem(g);
                        if seen.insert(k.clone()) {
                            q.terms.push(Term { key: k, weight: 0.8, shown: w.clone(), group });
                        }
                    }
                }
            }
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.years.is_empty() && self.months.is_empty() && self.numbers.is_empty()
    }
}

/// What a table offers to be matched: its texts by weight, and its time coverage.
#[derive(Debug, Clone, Default)]
pub struct TableText {
    /// (weight, text): title 3, sheet name 2.5, column names 2, label / code values 1.
    pub texts: Vec<(f64, String)>,
    /// First and last period of its time coverage (`2002-01-01`, `2026-S1`, `2019`).
    pub from: Option<String>,
    pub to: Option<String>,
}

/// One table's relevance: the score and the question words it matched (as the question has them).
#[derive(Debug, Clone, Default)]
pub struct Relevance {
    pub score: f64,
    pub matched: Vec<String>,
    /// The table that matches one of the question's distinctive words, years or months best: shown
    /// with the best-scoring table even when it scores lower (a question of several parts).
    pub champion: bool,
    /// The question names this table by its number (`Table 1`).
    pub named: bool,
    /// What each part of the question (a word with its concept, a table number, a year, a month)
    /// adds to the score; the same parts in the same order for every table.
    pub parts: Vec<f64>,
}

/// A table's words, each with its stem and the weight of the best field it is in.
struct Words {
    list: Vec<(String, String, f64)>,
    years: HashSet<i32>,
}

fn table_words(t: &TableText) -> Words {
    let mut best: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut years = HashSet::new();
    for (w, text) in &t.texts {
        if *w >= 2.0 {
            years.extend(crate::text::years_in(text));
        }
        for word in words(text) {
            let e = best.entry(word).or_insert(0.0);
            *e = e.max(*w);
        }
    }
    let mut list: Vec<(String, String, f64)> = best.into_iter().map(|(w, x)| (stem(&w), w, x)).collect();
    list.sort_by(|a, b| a.1.cmp(&b.1));
    Words { list, years }
}

fn year_of(p: &str) -> Option<i32> {
    p.get(..4).and_then(|y| y.parse().ok())
}

/// The months a table's coverage spans (12 per year when it is coarser than months; unknown: max).
fn span_months(t: &TableText) -> i64 {
    let (Some(from), Some(to)) = (t.from.as_deref(), t.to.as_deref()) else { return i64::MAX };
    let month = |s: &str| s.get(5..7).and_then(|m| m.parse::<i64>().ok()).filter(|m| (1..=12).contains(m));
    match (year_of(from), year_of(to), month(from), month(to)) {
        (Some(a), Some(b), Some(m1), Some(m2)) => (b as i64 * 12 + m2) - (a as i64 * 12 + m1) + 1,
        (Some(a), Some(b), _, _) => (b as i64 - a as i64 + 1) * 12,
        _ => i64::MAX,
    }
}

/// Whether a table's coverage (or a year in its title, sheet or column names) holds year `y`.
fn covers_year(t: &TableText, w: &Words, y: i32) -> bool {
    if w.years.contains(&y) {
        return true;
    }
    match (t.from.as_deref().and_then(year_of), t.to.as_deref().and_then(year_of)) {
        (Some(a), Some(b)) => a <= y && y <= b,
        _ => false,
    }
}

/// Whether a table's coverage holds the month (`2002-01` inside `2002-01-01`..`2002-01-31`).
fn covers_month(t: &TableText, y: i32, m: u32) -> bool {
    let (Some(from), Some(to)) = (t.from.as_deref(), t.to.as_deref()) else { return false };
    let p = format!("{y:04}-{m:02}");
    let cut = |s: &str| s.get(..7).map(str::to_string).unwrap_or_else(|| s.to_string());
    // Coarser coverage (years, quarters, halves): by year.
    if from.len() < 7 || to.len() < 7 || !from[5..7].chars().all(|c| c.is_ascii_digit()) {
        return year_of(from).is_some_and(|a| a <= y) && year_of(to).is_some_and(|b| y <= b);
    }
    cut(from) <= p && p <= cut(to)
}

/// Scores every table against the question. A word (with its concept's other names, once per
/// table) counts by the weight of the table's best field that has it times its inverse document
/// frequency, ln(n / df): a word every table has tells none apart and counts nothing. Years and
/// months the table's coverage holds count the same way. With years in the question, a table with
/// time facts that holds none of them keeps 40 % of its score.
///
/// A word, year or month that at most a third of the tables have names a champion: the table with
/// it in the best field (ties: the first), for a period the one with the narrowest coverage.
pub fn rank(q: &Question, tables: &[TableText]) -> Vec<Relevance> {
    let n = tables.len();
    let ws: Vec<Words> = tables.iter().map(table_words).collect();
    let mut out: Vec<Relevance> = vec![Relevance::default(); n];
    let idf = |df: usize| (n as f64 / df.max(1) as f64).ln();
    let distinctive = |df: usize| df > 0 && df < n && df as f64 <= (n as f64).sqrt().max(2.0);
    let groups: Vec<usize> = {
        let mut g: Vec<usize> = q.terms.iter().map(|t| t.group).collect();
        g.dedup();
        g
    };
    for g in groups {
        let terms: Vec<&Term> = q.terms.iter().filter(|t| t.group == g).collect();
        // The best match of the group in each table.
        let hits: Vec<f64> = ws
            .iter()
            .map(|w| {
                terms
                    .iter()
                    .map(|term| w.list.iter().map(|(s, raw, x)| matches(&term.key, raw, s) * x * term.weight).fold(0.0, f64::max))
                    .fold(0.0, f64::max)
            })
            .collect();
        let df = hits.iter().filter(|h| **h > 0.0).count();
        if df == 0 || df == n {
            continue;
        }
        // (of up to 3 tables with it equally well, the first: a workbook's main table comes first;
        // more tell none apart)
        let top = hits.iter().copied().fold(0.0, f64::max);
        let tied: Vec<usize> = (0..n).filter(|&k| hits[k] == top).collect();
        if distinctive(df) && top >= 1.4 && tied.len() <= 3 {
            out[tied[0]].champion = true;
        }
        let f = idf(df);
        for (k, h) in hits.iter().enumerate() {
            out[k].parts.push(f * h);
            if *h > 0.0 {
                out[k].score += f * h;
                out[k].matched.push(terms[0].shown.clone());
            }
        }
    }
    // Tables named by number: `Table 1` in the title (`Table 1: Labour Force Survey …`), a title
    // that starts with the number (`1.1 Bruttoinlandsprodukt`) or a sheet named so (`1`, `Tab 1`).
    for num in &q.numbers {
        let hits: Vec<bool> = tables
            .iter()
            .map(|t| {
                t.texts.iter().any(|(w, text)| {
                    let low = fold(text);
                    let low = low.trim();
                    let named = |rest: &str| rest.strip_prefix(num.as_str()).is_some_and(|r| !r.starts_with(|c: char| c.is_ascii_digit() || c == '.'));
                    (*w >= 2.5 && (named(low) || TABLE_WORDS.iter().any(|tw| low.match_indices(tw).any(|(i, _)| named(low[i + tw.len()..].trim_start())))))
                })
            })
            .collect();
        let df = hits.iter().filter(|h| **h).count();
        if df == 0 || df == n {
            continue;
        }
        for (k, h) in hits.iter().enumerate() {
            out[k].parts.push(if *h { 3.0 * idf(df) } else { 0.0 });
            if *h {
                out[k].score += 3.0 * idf(df);
                out[k].matched.push(format!("table {num}"));
                if df <= 2 {
                    out[k].champion = true;
                    out[k].named = true;
                }
            }
        }
    }
    // Years and months: the narrowest coverage that holds a distinctive one is its champion.
    let mut periods: Vec<(String, Vec<bool>)> = Vec::new();
    for &y in &q.years {
        periods.push((y.to_string(), tables.iter().zip(&ws).map(|(t, w)| covers_year(t, w, y)).collect()));
    }
    for &(y, m) in &q.months {
        periods.push((format!("{y}-{m:02}"), tables.iter().map(|t| covers_month(t, y, m)).collect()));
    }
    for (label, hits) in periods {
        let df = hits.iter().filter(|h| **h).count();
        if df == 0 || df == n {
            continue;
        }
        // (only when one table is the narrowest: equal ones tell none apart)
        let spans: Vec<i64> = (0..n).filter(|&k| hits[k]).map(|k| span_months(&tables[k])).collect();
        let narrowest = spans.iter().copied().min().unwrap_or(i64::MAX);
        if distinctive(df)
            && spans.iter().filter(|s| **s == narrowest).count() == 1
            && let Some(k) = (0..n).find(|&k| hits[k] && span_months(&tables[k]) == narrowest)
        {
            out[k].champion = true;
        }
        for (k, h) in hits.iter().enumerate() {
            out[k].parts.push(if *h { 1.5 * idf(df) } else { 0.0 });
            if *h {
                out[k].score += 1.5 * idf(df);
                out[k].matched.push(label.clone());
            }
        }
    }
    if !q.years.is_empty() {
        for (k, (t, w)) in tables.iter().zip(&ws).enumerate() {
            let timed = t.from.is_some() || !w.years.is_empty();
            if timed && !q.years.iter().any(|&y| covers_year(t, w, y)) {
                out[k].score *= 0.4;
                out[k].parts.iter_mut().for_each(|p| *p *= 0.4);
            }
        }
    }
    out
}

/// The tables a question is about, best score first; none when no table matches. A question often
/// has several parts, each about another table: the best-scoring table first, then, while one
/// scores at least a third of that on the parts no chosen table answers well (70 % of the best
/// table for that part), the best of those; and the champions of distinctive words, years and
/// months. Tables the question names by number (`Table 1`) are chosen first and answer every part
/// they match; then only tables for the other parts follow (no champions).
pub fn relevant(rel: &[Relevance]) -> Vec<usize> {
    let n = rel.len();
    let parts = rel.first().map_or(0, |r| r.parts.len());
    let top: Vec<f64> = (0..parts).map(|g| rel.iter().map(|r| r.parts[g]).fold(0.0, f64::max)).collect();
    let mut covered = vec![false; parts];
    let mut chosen: Vec<usize> = (0..n).filter(|&k| rel[k].named).collect();
    let named = !chosen.is_empty();
    let mut first = chosen.iter().map(|&k| rel[k].score).fold(0.0, f64::max);
    for &k in &chosen {
        for g in 0..parts {
            covered[g] |= rel[k].parts[g] > 0.0;
        }
    }
    while chosen.len() < n {
        let residual = |k: usize| (0..parts).filter(|&g| !covered[g]).map(|g| rel[k].parts[g]).sum::<f64>();
        let Some(k) = (0..n).filter(|k| !chosen.contains(k)).max_by(|a, b| residual(*a).total_cmp(&residual(*b)).then(b.cmp(a)))
        else {
            break;
        };
        let r = residual(k);
        if r <= 0.0 || (!chosen.is_empty() && r < first * 0.4) {
            break;
        }
        if chosen.is_empty() {
            first = r;
        }
        chosen.push(k);
        // The parts it answers well, and at least its strongest one.
        let strongest = (0..parts).filter(|&g| !covered[g]).max_by(|a, b| rel[k].parts[*a].total_cmp(&rel[k].parts[*b]));
        for g in 0..parts {
            if rel[k].parts[g] > 0.0 && rel[k].parts[g] >= 0.7 * top[g] {
                covered[g] = true;
            }
        }
        if let Some(g) = strongest {
            covered[g] = true;
        }
    }
    let mut ks: Vec<usize> =
        (0..n).filter(|&k| chosen.contains(&k) || (!named && rel[k].champion && rel[k].score > 0.0)).collect();
    ks.sort_by(|a, b| rel[*b].named.cmp(&rel[*a].named).then(rel[*b].score.total_cmp(&rel[*a].score)).then(a.cmp(b)));
    ks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_accents_and_cyrillic() {
        assert_eq!(fold("Nächtigungen"), "nachtigungen");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("Restituição"), "restituicao");
        assert_eq!(fold("Порези"), "porezi");
        assert_eq!(fold("Друмски саобраћај"), "drumski saobracaj");
        assert_eq!(fold("Ensemble"), "ensemble");
    }

    #[test]
    fn splits_words_and_camel_case() {
        assert_eq!(words("precoContratual, Ø 22"), vec!["preco", "contratual", "o", "22"]);
        assert_eq!(words("renewables_share_elec"), vec!["renewables", "share", "elec"]);
        assert_eq!(words("NPV @ 14%"), vec!["npv", "14"]);
    }

    #[test]
    fn stems_plurals_and_endings() {
        // en
        assert!(matches(&stem("salaries"), "salary", &stem("salary")) > 0.0);
        assert_eq!(stem("receipts"), "receipt");
        assert_eq!(stem("unemployment"), stem("unemployed"));
        // de: `preis` keeps its s
        assert_eq!(stem("nachtigungen"), "nachtig");
        assert_eq!(stem("einnahmen"), "einnahm");
        assert_eq!(stem("preise"), "preis");
        assert_eq!(stem("preis"), "preis");
        // fr / es / pt / it
        assert_eq!(stem("departements"), stem("departement"));
        assert_eq!(stem("contratos"), stem("contrato"));
        assert_eq!(stem("prezzi"), stem("prezzo"));
        // nl / sr
        assert_eq!(stem("prijzen"), "prijz");
        assert_eq!(stem("porezi"), stem("porez"));
        // short words and numbers stay
        assert_eq!(stem("vat"), "vat");
        assert_eq!(stem("2023"), "2023");
        // prefixes and compounds
        assert!(matches(&stem("employment"), "employees", &stem("employees")) > 0.0);
        assert!(matches(&stem("inlandsprodukt"), "bruttoinlandsprodukts", &stem("bruttoinlandsprodukts")) > 0.0);
        assert_eq!(matches("vat", "vatican", &stem("vatican")), 0.0);
    }

    #[test]
    fn parses_years_months_and_drops_function_words() {
        let q = Question::parse("What was the GDP in 2025, and on which day in January 2002 were receipts highest? EU average");
        assert_eq!(q.years, vec![2025, 2002]);
        assert_eq!(q.months, vec![(2002, 1)]);
        let keys: Vec<&str> = q.terms.iter().filter(|t| t.weight == 1.0).map(|t| t.key.as_str()).collect();
        assert!(keys.contains(&"gdp") && keys.contains(&"receipt") && keys.contains(&"eu"), "{keys:?}");
        assert!(!keys.contains(&"what") && !keys.contains(&"the") && !keys.contains(&"which"));
        // the concept's other names, in the same group
        let gdp = q.terms.iter().find(|t| t.key == "gdp").unwrap().group;
        assert!(q.terms.iter().any(|t| t.key == stem("bruttoinlandsprodukt") && t.group == gdp));
        // two-letter words only in capitals
        assert!(Question::parse("is it in the eu").terms.is_empty());
        // tables by number
        assert_eq!(Question::parse("For the latest period in Table 1: what …? See Tabelle 2.3.").numbers, vec!["1", "2.3"]);
    }

    fn table(title: &str, cols: &str, from: Option<&str>, to: Option<&str>) -> TableText {
        TableText {
            texts: vec![(3.0, title.into()), (2.0, cols.into())],
            from: from.map(String::from),
            to: to.map(String::from),
        }
    }

    #[test]
    fn ranks_by_words_years_and_months() {
        let tables = vec![
            table("Bruttoinlandsprodukt", "Jahr In jeweiligen Preisen", Some("1991"), Some("2025")),
            table("Bevölkerung und Erwerbstätigkeit", "Jahr Einwohner Erwerbslose", Some("1991"), Some("2025")),
            table("Steuern", "Jahr Einnahmen", Some("1991"), Some("2010")),
        ];
        let q = Question::parse("What was Germany's GDP in 2025?");
        let r = rank(&q, &tables);
        assert_eq!(relevant(&r), vec![0], "{r:?}");
        let q = Question::parse("How many inhabitants did Germany have in 2025, and what was the unemployment rate?");
        assert_eq!(relevant(&rank(&q, &tables)), vec![1]);
        // two parts, two tables
        let q = Question::parse("What was GDP in 2025, and what was the unemployment rate?");
        assert_eq!(relevant(&rank(&q, &tables)), vec![0, 1]);
        // a month: the narrowest table that holds it
        let tables = vec![
            table("Flows", "Pipeline", Some("2001-09-05"), Some("2002-02-05")),
            table("Flows", "Pipeline", Some("2002-01-01"), Some("2002-01-31")),
            table("Flows", "Pipeline", Some("2001-12-01"), Some("2001-12-31")),
            table("Flows", "Pipeline", Some("2001-11-01"), Some("2001-11-30")),
        ];
        let q = Question::parse("On which day in January 2002 were pipeline flows highest?");
        let r = rank(&q, &tables);
        assert_eq!(relevant(&r), vec![0, 1], "{r:?}");
        assert!(r[1].champion && r[1].matched.contains(&"2002-01".to_string()));
        // no match: nothing relevant
        assert!(relevant(&rank(&Question::parse("zebra giraffe"), &tables)).is_empty());
        // a table by its number
        let tables = vec![
            table("Introduction", "Day Month Year", None, None),
            table("Table 1: Labour Force Survey Summary", "period rate", None, None),
            table("Table 11: Claimant count", "period rate", None, None),
        ];
        let r = rank(&Question::parse("For the latest period in Table 1, what is the rate?"), &tables);
        assert_eq!(relevant(&r), vec![1], "{r:?}");
    }
}
