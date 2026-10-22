//! Semantic column types (B21): what a column *is*, beyond its storage type, so a small model
//! knows which columns to group by, which to filter on and which it must not add up.
//!
//! Each data / group column of a table gets one [`Sem`]: `id`, `name` (a label per row),
//! `dimension` (a small category), `period`, a measure (`amount` with its currency when known,
//! `count`, `rate`, `avg`, `index`) or free `text`; numeric columns of a table whose rows mix
//! measures (a label column separates counts from amounts) are `mixed`. The type comes from the
//! header words (EN/DE/FR/ES/PT/IT/NL/SR), the column's currency / unit from normalization, the
//! table title for period-named columns (`"2025"` in an index table), and value statistics
//! (distinct values, text length, integers). Pure functions; a few cheap passes over a sample of
//! the rows (at most [`SAMPLE`] per column).

use hh_core::normalize::{table_time_coverage, ColRole, Column, Dtype, Normalized, Out, Table};

use crate::catalog::{data_cols, data_rows, is_date, is_numeric};
use crate::text::{ident, short};

/// Rows looked at per column for distinct values and text length.
const SAMPLE: usize = 4000;
/// A text column with at most this many distinct values (or ≤ 5 % of the rows, up to 200) is a
/// dimension.
const DIM_VALUES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sem {
    Id,
    Name,
    Dimension,
    Period,
    Amount,
    Count,
    Rate,
    Average,
    Index,
    Text,
    /// Numbers of a table whose rows mix measures (by a label column).
    Mixed,
}

/// A column's semantic type and currency (from the column's values, its header or the title).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColType {
    pub sem: Sem,
    pub currency: Option<String>,
    /// A guess without evidence (a number column with no words, unit or currency: `amount`);
    /// the catalog leaves the tag out, `describe_table` shows it.
    pub weak: bool,
}

impl ColType {
    fn new(sem: Sem) -> Self {
        ColType { sem, currency: None, weak: false }
    }

    /// Averages, rates and indexes: adding them up gives nonsense.
    pub fn no_sum(&self) -> bool {
        matches!(self.sem, Sem::Rate | Sem::Average | Sem::Index)
    }

    /// Short name: `id`, `name`, `dim`, `period`, `amount`, `count`, `rate`, `avg`, `index`, `text`, `mixed`.
    pub fn short(&self) -> &'static str {
        match self.sem {
            Sem::Id => "id",
            Sem::Name => "name",
            Sem::Dimension => "dim",
            Sem::Period => "period",
            Sem::Amount => "amount",
            Sem::Count => "count",
            Sem::Rate => "rate",
            Sem::Average => "avg",
            Sem::Index => "index",
            Sem::Text => "free text",
            Sem::Mixed => "mixed",
        }
    }

    /// The tag after the storage type in a column item: `amount`, `avg — don't sum`.
    pub fn tag(&self) -> String {
        if self.no_sum() { format!("{} — don't sum", self.short()) } else { self.short().to_string() }
    }
}

// ------------------------------------------------------------------------------------------------
// words

/// Lowercase words of a header: split at non-alphanumerics (keeping `%`, `€`, `$`, `£`, `ø`) and at
/// camelCase humps.
fn words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in name.chars() {
        if "%€$£".contains(ch) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(ch.to_string());
            prev_lower = false;
            continue;
        }
        if !ch.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        cur.extend(ch.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn any_word(ws: &[String], exact: &[&str], stems: &[&str]) -> bool {
    ws.iter().any(|w| exact.contains(&w.as_str()) || stems.iter().any(|s| w.starts_with(s)))
}

/// Averages, medians, per-head figures.
fn avg_words(ws: &[String], low: &str) -> bool {
    const EXACT: &[&str] = &[
        "average", "averages", "avg", "mean", "median", "ø", "ø-", "moyenne", "moyen", "moyens", "media", "medie",
        "promedio", "medio", "médio", "média", "gemiddeld", "gemiddelde", "prosek", "mittel", "mittelwert",
    ];
    const STEMS: &[&str] = &["durchschnitt", "prosečn", "prosecn", "просеч", "gemiddeld", "mittelwert"];
    any_word(ws, EXACT, STEMS)
        || low.contains("durchschnitt")
        || low.contains("per capita")
        || low.contains("pro kopf")
        || low.contains("je einwohner")
        || low.contains("per person")
        || low.contains("par habitant")
        || low.contains("por habitante")
        || low.contains("ø ")
}

/// Index numbers (`Fahrleistungsindex`, `Indexwert`, `2015=100`).
fn index_words(ws: &[String], low: &str) -> bool {
    const EXACT: &[&str] = &["index", "indices", "indexes", "indizes", "indice", "índice", "indeks", "indexwert", "messzahl", "indexzahl"];
    any_word(ws, EXACT, &["messzahl"]) || low.contains("index") || low.contains("=100") || low.contains("= 100")
}

/// Percentages, shares, ratios, rates, changes in percent.
fn rate_words(ws: &[String]) -> bool {
    const EXACT: &[&str] = &[
        "%", "percent", "percentage", "pct", "prozent", "prozentual", "anteil", "anteile", "share", "rate", "rates",
        "ratio", "quote", "quota", "quotient", "pourcentage", "taux", "part", "tasa", "porcentaje", "proporción",
        "proporcion", "taxa", "percentagem", "porcentagem", "tasso", "percentuale", "quota", "procent", "percentage",
        "aandeel", "procenat", "stopa", "udeo", "udio", "učešće", "ucesce", "proc",
    ];
    const STEMS: &[&str] = &["prozent", "percent", "procent", "porcent", "percentual", "quote", "anteil"];
    any_word(ws, EXACT, STEMS)
}

/// Counts of things or people.
fn count_words(ws: &[String], low: &str) -> bool {
    const EXACT: &[&str] = &[
        "count", "counts", "qty", "quantity", "quantities", "pieces", "persons", "people", "cases",
        "employees", "households", "visitors", "anzahl", "zahl", "menge", "stück", "stk", "personen", "fälle", "faelle",
        "fall", "einwohner", "bevölkerung", "beschäftigte", "haushalte", "besucher", "bezieher", "quantité",
        "effectif", "effectifs", "habitants", "personnes", "cantidad", "personas", "habitantes", "casos",
        "quantidade", "pessoas", "quantità", "quantita", "persone", "abitanti", "aantal", "personen",
        "inwoners", "broj", "količina", "kolicina", "osoba", "lica", "stanovnika", "stanovnici", "population",
        "pop", "n",
    ];
    const STEMS: &[&str] = &["anzahl", "bezieher", "einwohner", "beschäftigt", "teilnehmer", "fälle"];
    any_word(ws, EXACT, STEMS) || low.contains("number of") || low.contains("nombre de") || low.contains("número de")
}

/// Identifiers and codes.
fn id_words(ws: &[String]) -> bool {
    const EXACT: &[&str] = &[
        "id", "ids", "nr", "no", "nº", "n°", "num", "code", "codes", "codigo", "código", "cod", "key", "ref", "uuid",
        "plz", "zip", "cpv", "nuts", "isin", "nif", "nipc", "vat", "pib", "jmbg", "mb", "sku", "ean", "iban", "kennung",
        "kennziffer", "schlüssel", "nummer", "numéro", "identifiant", "identificador", "identificação", "šifra", "sifra",
        "oznaka", "postcode", "postleitzahl", "kod", "kode", "codice", "number", "numero", "número",
    ];
    const STEMS: &[&str] = &["schlüssel", "schluessel", "kennziffer", "identif", "nummer"];
    ws.first().is_some_and(|w| w.starts_with("id") && w.len() > 3 && !w.starts_with("ide") && !w.starts_with("ida"))
        || any_word(ws, EXACT, STEMS)
}

/// Year / month / week / day / quarter as a column of numbers.
fn calendar_words(ws: &[String]) -> bool {
    const EXACT: &[&str] = &[
        "year", "jahr", "ano", "año", "anno", "année", "annee", "jaar", "godina", "month", "monat", "mes", "mês", "mois",
        "mese", "maand", "mesec", "week", "woche", "kalenderwoche", "semana", "semaine", "settimana", "quarter",
        "quartal", "trimestre", "kwartaal", "kvartal", "fy", "berichtsjahr", "jahrgang",
    ];
    any_word(ws, EXACT, &["kalenderwoche", "berichtsjahr"])
}

/// Names of things (a label per row).
fn name_words(ws: &[String]) -> bool {
    const EXACT: &[&str] = &[
        "name", "names", "title", "label", "bezeichnung", "benennung", "titel", "nom", "libellé", "libelle", "intitulé",
        "nombre", "descripción", "nome", "denominação", "designação", "denominazione", "naam", "omschrijving", "naziv",
        "ime", "firma", "company", "unternehmen", "empresa", "entidade", "entidad",
    ];
    any_word(ws, EXACT, &["bezeichn"])
}

/// A currency named in the text (a symbol, an ISO code or a currency word), not an ambiguous
/// short symbol (`kr`, `Fr`) or a code that is also an English word.
pub fn header_currency(text: &str) -> Option<&'static str> {
    for ch in text.chars() {
        let code = match ch {
            '€' => "EUR",
            '£' => "GBP",
            '$' => "USD",
            _ => continue,
        };
        return Some(code);
    }
    text.split(|c: char| !c.is_alphanumeric() && c != '.' && c != '$')
        .filter(|w| !w.is_empty())
        .find_map(|w| {
            let m = hh_core::values::currency_match(w)?;
            (!m.wordlike && !m.ambiguous && (w.chars().count() >= 3 || !w.is_ascii()) && !w.contains('.')).then_some(m.code)
        })
}

/// What a header or row label says a number measures (`Durchschnittshöhe in Euro` → avg EUR),
/// or None when its words say nothing.
pub fn measure_of_label(label: &str) -> Option<ColType> {
    let low = label.to_lowercase();
    let ws = words(label);
    let currency = header_currency(label).map(String::from);
    let sem = if avg_words(&ws, &low) {
        Sem::Average
    } else if index_words(&ws, &low) {
        Sem::Index
    } else if rate_words(&ws) {
        Sem::Rate
    } else if currency.is_some() || money_words(&ws, &low) {
        Sem::Amount
    } else if count_words(&ws, &low) {
        Sem::Count
    } else {
        return None;
    };
    Some(ColType { sem, currency, weak: false })
}

fn money_words(ws: &[String], low: &str) -> bool {
    const STEMS: &[&str] = &[
        "preco", "preço", "price", "precio", "prezzo", "prix", "valor", "value", "amount", "betrag", "kosten", "costo",
        "custo", "cost", "revenue", "umsatz", "erlös", "erloes", "sales", "salary", "gehalt", "lohn", "budget", "payment",
        "zahlung", "iznos", "cijena", "cena", "montant", "importe", "ausgaben", "einnahmen", "expense", "income", "profit",
        "gewinn", "spend", "turnover", "bedrag", "prijs", "kosten", "omzet", "importo", "ricavi", "spesa", "receita",
        "despesa", "ingresos", "gastos", "chiffre d'affaires", "dépenses", "recettes", "prihod", "rashod", "trošk",
        "plata", "zarada", "vermögen", "schulden", "debt", "assets",
    ];
    let _ = ws;
    STEMS.iter().any(|s| low.contains(s))
}

// ------------------------------------------------------------------------------------------------
// classification

/// Evenly spread sample of the data rows.
fn sample(rows: &[usize]) -> Vec<usize> {
    if rows.len() <= SAMPLE {
        return rows.to_vec();
    }
    let step = rows.len() as f64 / SAMPLE as f64;
    (0..SAMPLE).map(|k| rows[(k as f64 * step) as usize]).collect()
}

struct TextStats {
    filled: usize,
    distinct: usize,
    avg_len: f64,
    code_like: usize,
}

fn text_stats(c: &Column, rows: &[usize]) -> TextStats {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let (mut filled, mut len, mut code_like) = (0usize, 0usize, 0usize);
    for &r in rows {
        if let Some(Out::Text(s)) = c.values.get(r) {
            filled += 1;
            len += s.chars().count();
            seen.insert(s.as_str());
            let s = s.trim();
            if !s.is_empty() && s.len() <= 24 && !s.contains(' ') && s.chars().any(|ch| ch.is_ascii_digit()) {
                code_like += 1;
            }
        }
    }
    TextStats { filled, distinct: seen.len(), avg_len: if filled > 0 { len as f64 / filled as f64 } else { 0.0 }, code_like }
}

/// Integers in 1900..=2100 only (years), or None when the column has a non-integer.
fn all_years(c: &Column, rows: &[usize]) -> bool {
    let mut n = 0;
    for &r in rows {
        if let Some(Out::Num(x)) = c.values.get(r) {
            if x.fract() != 0.0 || !(1900.0..=2100.0).contains(x) {
                return false;
            }
            n += 1;
        }
    }
    n > 0
}

fn distinct_nums(c: &Column, rows: &[usize]) -> (usize, usize) {
    let mut seen = std::collections::HashSet::new();
    let mut filled = 0;
    for &r in rows {
        if let Some(Out::Num(x)) = c.values.get(r) {
            filled += 1;
            seen.insert(x.to_bits());
        }
    }
    (filled, seen.len())
}

/// Source columns of a table that the file itself adds up: a vertical total that checks out.
/// An average or rate column the file sums (average daily flows per pipeline, shares that make
/// 100 %) is not flagged "don't sum".
pub fn summed_cols(norm: &Normalized, t: &Table) -> Vec<u32> {
    let mut cols: Vec<u32> = norm
        .totals
        .iter()
        .filter(|x| x.table == Some(t.n) && x.direction == "vertical" && x.status == "ok")
        .map(|x| x.col)
        .collect();
    cols.sort_unstable();
    cols.dedup();
    cols
}

/// Types of a table's columns (indexed like `t.columns`; None for `_src_row` / `_row_type`).
/// `mixed`: the label column by which the table's rows mix measures, if any; `summed`: source
/// columns the file adds up itself ([`summed_cols`]).
pub fn classify(t: &Table, names: &[String], mixed: Option<usize>, summed: &[u32]) -> Vec<Option<ColType>> {
    let rows = sample(&data_rows(t));
    let title = t.title.as_ref().map(|(s, _)| s.as_str()).unwrap_or("");
    let title_measure = measure_of_label(title).filter(|m| m.sem != Sem::Count);
    let period_cols: Vec<String> = table_time_coverage(t)
        .filter(|c| c.source == "column")
        .map(|c| c.columns)
        .unwrap_or_default();
    let mut out: Vec<Option<ColType>> = vec![None; t.columns.len()];
    for (i, c) in data_cols(t) {
        let name = &names[i];
        let mut ty = classify_col(c, name, &rows, mixed.is_some() && mixed != Some(i), title_measure.as_ref(), &period_cols);
        if ty.no_sum() && c.source_col.is_some_and(|s| summed.binary_search(&s).is_ok()) {
            ty.sem = Sem::Amount;
        }
        out[i] = Some(ty);
    }
    out
}

fn classify_col(
    c: &Column,
    name: &str,
    rows: &[usize],
    mixed: bool,
    title: Option<&ColType>,
    period_cols: &[String],
) -> ColType {
    let ws = words(name);
    if c.role == ColRole::Group {
        return ColType::new(Sem::Dimension);
    }
    if is_date(c.dtype) || period_cols.iter().any(|p| p == &c.name) {
        return ColType::new(Sem::Period);
    }
    match c.dtype {
        Dtype::Empty => ColType::new(Sem::Text),
        Dtype::Bool => ColType::new(Sem::Dimension),
        Dtype::Text => {
            let st = text_stats(c, rows);
            if st.filled == 0 {
                return ColType::new(Sem::Text);
            }
            // B89: period codes written as text ('2019-S1').
            if crate::views::text_period_column(c, rows).is_some() {
                return ColType::new(Sem::Period);
            }
            let unique = st.distinct * 10 >= st.filled * 9;
            if id_words(&ws) && !name_words(&ws) || (unique && st.filled >= 20 && st.code_like * 10 >= st.filled * 9) {
                return ColType::new(Sem::Id);
            }
            if st.distinct <= DIM_VALUES || (st.distinct * 20 <= st.filled && st.distinct <= 200) {
                return ColType::new(Sem::Dimension);
            }
            if st.avg_len > 60.0 {
                return ColType::new(Sem::Text);
            }
            ColType::new(Sem::Name)
        }
        _ => {
            // Numbers.
            let periodic = crate::views::period_of(name).is_some();
            if !periodic && calendar_words(&ws) && all_years_or_small(c, rows) {
                return ColType::new(Sem::Period);
            }
            if !periodic && id_words(&ws) && measure_of_label(name).is_none() {
                return ColType::new(Sem::Id);
            }

            // Two periods in one header ("2020/ 2025", "2024 vs 2023"): a change between them.
            if two_periods(name) {
                return ColType::new(Sem::Rate);
            }
            if mixed {
                return ColType::new(Sem::Mixed);
            }
            let currency = c.currency.clone().or_else(|| header_currency(name).map(String::from));
            if c.dtype == Dtype::Percent || c.unit.as_deref() == Some("%") {
                return ColType::new(Sem::Rate);
            }
            if let Some(mut m) = measure_of_label(name) {
                if m.currency.is_none() {
                    m.currency = currency.clone();
                }
                if m.sem == Sem::Count && currency.is_some() {
                    m.sem = Sem::Amount;
                }
                return m;
            }

            if currency.is_some() || c.dtype == Dtype::Currency {
                return ColType { sem: Sem::Amount, currency, weak: false };
            }
            // A bare period header ("2025"): the table title says what; an index / average / rate
            // title also for headers without measure words ("unbereinigt" in an index table).
            if let Some(m) = title.filter(|m| periodic || m.no_sum()) {
                return ColType { sem: m.sem, currency: m.currency.clone(), weak: false };
            }
            let (filled, distinct) = distinct_nums(c, rows);
            if c.dtype == Dtype::Integer && filled >= 20 && distinct == filled && sequential(c, rows) {
                return ColType::new(Sem::Id);
            }
            // No words to go by: a quantity that adds up.
            ColType { sem: Sem::Amount, currency: None, weak: true }
        }
    }
}

fn all_years_or_small(c: &Column, rows: &[usize]) -> bool {
    all_years(c, rows)
        || rows.iter().all(|&r| match c.values.get(r) {
            Some(Out::Num(x)) => x.fract() == 0.0 && (0.0..=53.0).contains(x),
            _ => true,
        })
}

/// Consecutive integers in row order (a running number).
fn sequential(c: &Column, rows: &[usize]) -> bool {
    let vals: Vec<f64> = rows
        .iter()
        .filter_map(|&r| if let Some(Out::Num(x)) = c.values.get(r) { Some(*x) } else { None })
        .collect();
    vals.windows(2).filter(|w| (w[1] - w[0]).abs() == 1.0).count() * 10 >= vals.len().saturating_sub(1) * 9
}

/// A header naming two years (`2020/ 2025`, `2024 vs. 2023`, `Veränderung 2023-2024`).
fn two_periods(name: &str) -> bool {
    let years = crate::text::years_in(name);
    years.len() >= 2 && years.iter().collect::<std::collections::HashSet<_>>().len() >= 2
}

// ------------------------------------------------------------------------------------------------
// text for the catalog and describe_table

/// The text inside a column item's brackets with the semantic type: `number EUR · amount`,
/// `number · avg — don't sum` (`kind` is the storage type with unit / currency as shown so far).
pub fn kind_with(kind: &str, c: &Column, ty: Option<&ColType>) -> String {
    let Some(ty) = ty.filter(|ty| !ty.weak) else { return kind.to_string() };
    if ty.sem == Sem::Text && c.dtype == Dtype::Text && c.role != ColRole::Group {
        return kind.replacen("text", "free text", 1);
    }
    let mut s = kind.to_string();
    if let Some(cur) = ty.currency.as_ref().filter(|_| c.currency.is_none() && c.unit.is_none()) {
        s.push(' ');
        s.push_str(cur);
    }
    // A text dimension that is the group column says "group" already.
    if !(c.role == ColRole::Group && ty.sem == Sem::Dimension) {
        s.push_str(" · ");
        s.push_str(&ty.tag());
    }
    s
}

/// `do not sum:` line of a table: its average / rate / index columns, and for a table whose rows
/// mix measures, the labels whose rows are averages / rates / indexes. None when there are none.
pub fn no_sum_line(t: &Table, names: &[String], types: &[Option<ColType>], mixed: Option<usize>) -> Option<String> {
    let mut items: Vec<String> = Vec::new();
    for (i, ty) in types.iter().enumerate() {
        if let Some(ty) = ty.as_ref().filter(|ty| ty.no_sum() && is_numeric(t.columns[i].dtype)) {
            items.push(format!("{} ({})", ident(&short(&names[i], 40)), ty.short()));
        }
    }
    let cols = items.len();
    if let Some(m) = mixed {
        let mut labels: Vec<&str> = Vec::new();
        for r in data_rows(t) {
            if let Some(Out::Text(s)) = t.columns[m].values.get(r)
                && !labels.contains(&s.as_str())
            {
                labels.push(s);
                if labels.len() > 12 {
                    break;
                }
            }
        }
        for l in labels {
            if let Some(ty) = measure_of_label(l).filter(ColType::no_sum) {
                let cur = ty.currency.as_ref().map(|c| format!(" {c}")).unwrap_or_default();
                items.push(format!("rows {} = '{}' ({}{cur})", ident(&names[m]), short(l, 40), ty.short()));
            }
        }
    }
    if items.is_empty() {
        return None;
    }
    let more = items.len().saturating_sub(6);
    items.truncate(6);
    let mut s = format!("    do not sum: {}", items.join(", "));
    if more > 0 {
        s.push_str(&format!(" +{more} more"));
    }
    s.push_str(if cols > 0 { " — use AVG, a weighted mean or the file's own total" } else { " — use AVG or the file's own figure" });
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sem(label: &str) -> Option<Sem> {
        measure_of_label(label).map(|m| m.sem)
    }

    #[test]
    fn label_measures() {
        assert_eq!(sem("Durchschnittshöhe in Euro"), Some(Sem::Average));
        assert_eq!(measure_of_label("Durchschnittshöhe in Euro").unwrap().currency.as_deref(), Some("EUR"));
        assert_eq!(sem("Anzahl Bezieher:innen"), Some(Sem::Count));
        assert_eq!(sem("Richtsätze in Euro"), Some(Sem::Amount));
        assert_eq!(sem("Veränderung KSB gegenüber Vortag in Prozent"), Some(Sem::Rate));
        assert_eq!(sem("gleitender 7-Tage-Durchschnitt KSB"), Some(Sem::Average));
        assert_eq!(sem("Lkw-Maut-Fahrleistungsindex"), Some(Sem::Index));
        assert_eq!(sem("Indexwert"), Some(Sem::Index));
        assert_eq!(sem("precoContratual"), Some(Sem::Amount));
        assert_eq!(sem("Taxa de desemprego"), Some(Sem::Rate));
        assert_eq!(sem("Prix moyen"), Some(Sem::Average));
        assert_eq!(sem("Prosečna zarada (RSD)"), Some(Sem::Average));
        assert_eq!(sem("Aantal inwoners"), Some(Sem::Count));
        assert_eq!(sem("Revenue USD"), Some(Sem::Amount));
        assert_eq!(sem("Richtsatz"), None);
        assert_eq!(sem("Wochentag"), None);
    }

    #[test]
    fn currencies_in_headers() {
        assert_eq!(header_currency("Betrag in €"), Some("EUR"));
        assert_eq!(header_currency("Amount (USD)"), Some("USD"));
        assert_eq!(header_currency("Iznos u dinarima"), None);
        assert_eq!(header_currency("Iznos (din)"), Some("RSD"));
        assert_eq!(header_currency("Fr / 2002-01-04"), None);
        assert_eq!(header_currency("Total all"), None);
        assert_eq!(header_currency("Plata RSD"), Some("RSD"));
    }

    #[test]
    fn changes_and_words() {
        assert!(two_periods("2020/ 2025"));
        assert!(!two_periods("2025"));
        assert_eq!(words("precoContratual €"), vec!["preco", "contratual", "€"]);
    }
}
