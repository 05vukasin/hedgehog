//! Clean tables with known content: generated (six domains, translatable headers) or sampled from
//! real clean tables under `$HEDGEHOG_DATA/clean/` (read with hedgehog's own CSV reader, parse
//! only). Every value is complete; noise comes later.

use crate::rng::Rng;

#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Num(f64),
    Text(String),
    /// Excel serial date.
    Date(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Label / key text column.
    Key,
    Int,
    /// Decimal with this many digits.
    Dec(u8),
    Money,
    /// Can be negative (variance, balance).
    Signed,
    Date,
}

impl Kind {
    pub fn numeric(self) -> bool {
        matches!(self, Kind::Int | Kind::Dec(_) | Kind::Money | Kind::Signed)
    }
}

#[derive(Clone, Debug)]
pub struct Col {
    pub header: String,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub struct BaseTable {
    /// `gen:<domain>` or `real:<file>`.
    pub name: String,
    pub title: String,
    pub cols: Vec<Col>,
    pub rows: Vec<Vec<V>>,
    /// Key column whose equal values are contiguous (groups of 2+ rows).
    pub group: Option<usize>,
    pub real: bool,
}

impl BaseTable {
    pub fn keys(&self) -> Vec<usize> {
        (0..self.cols.len()).filter(|&j| self.cols[j].kind == Kind::Key).collect()
    }
    pub fn measures(&self) -> Vec<usize> {
        (0..self.cols.len()).filter(|&j| self.cols[j].kind.numeric()).collect()
    }
    pub fn text(&self, i: usize, j: usize) -> String {
        match &self.rows[i][j] {
            V::Text(s) => s.clone(),
            V::Num(x) => fmt_plain(*x),
            V::Date(d) => format!("{d}"),
        }
    }
    pub fn num(&self, i: usize, j: usize) -> f64 {
        match &self.rows[i][j] {
            V::Num(x) => *x,
            _ => 0.0,
        }
    }
    /// Row index ranges of the groups (by `group`), in order.
    pub fn groups(&self) -> Vec<(usize, usize)> {
        let Some(g) = self.group else { return vec![(0, self.rows.len())] };
        let mut out = Vec::new();
        let mut start = 0;
        for i in 1..=self.rows.len() {
            if i == self.rows.len() || self.rows[i][g] != self.rows[start][g] {
                out.push((start, i));
                start = i;
            }
        }
        out
    }
}

/// Shortest plain text of a number (`12`, `12.5`).
pub fn fmt_plain(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 { format!("{}", x as i64) } else { format!("{x}") }
}

pub fn round(x: f64, d: u8) -> f64 {
    let p = 10f64.powi(d as i32);
    (x * p).round() / p
}

// ---------------------------------------------------------------------------------------------
// Generated domains

pub struct Domain {
    pub name: &'static str,
    pub title: &'static str,
    pub keys: &'static [(&'static str, &'static [&'static str])],
    pub measures: &'static [(&'static str, Kind)],
}

const REGIONS: &[&str] = &["North", "South", "East", "West", "Central", "Coastal", "Highlands"];
const PRODUCTS: &[&str] = &["Widgets", "Gadgets", "Gizmos", "Sprockets", "Brackets", "Valves", "Pumps", "Fittings", "Hinges"];
const DISTRICTS: &[&str] = &["Alder", "Birchwood", "Cedar Hill", "Dunmore", "Elmstead", "Fairview", "Glenrock", "Hollow Creek"];
const TOWNS: &[&str] = &["Ashford", "Brookside", "Carlton", "Dover Bay", "Eastwick", "Fernley", "Grantham", "Harrow", "Irwin", "Jasper"];
const DEPTS: &[&str] = &["Finance", "Human Resources", "IT Services", "Legal", "Operations", "Marketing", "Procurement"];
const CCS: &[&str] = &["CC-101", "CC-102", "CC-205", "CC-230", "CC-310", "CC-415", "CC-520", "CC-610"];
const PLANTS: &[&str] = &["Riverbend", "Stonegate", "Kestrel Point", "Oakridge", "Millbrook", "Northfield"];
const FUELS: &[&str] = &["Gas", "Coal", "Hydro", "Wind", "Solar", "Biomass", "Nuclear"];
const HOSPITALS: &[&str] = &["St. Mary", "General", "University Clinic", "Riverside", "Northgate"];
const WARDS: &[&str] = &["Cardiology", "Surgery", "Paediatrics", "Oncology", "Neurology", "Orthopaedics", "Maternity"];
const COUNTRIES: &[&str] = &["Austria", "Belgium", "Croatia", "Denmark", "Estonia", "France", "Germany", "Hungary", "Italy", "Serbia"];
const GOODS: &[&str] = &["Machinery", "Chemicals", "Food products", "Textiles", "Vehicles", "Metals", "Paper"];

pub const DOMAINS: &[Domain] = &[
    Domain {
        name: "sales",
        title: "Sales by region and product",
        keys: &[("Region", REGIONS), ("Product", PRODUCTS)],
        measures: &[("Units", Kind::Int), ("Revenue", Kind::Money), ("Cost", Kind::Money), ("Returns", Kind::Int)],
    },
    Domain {
        name: "population",
        title: "Population by district",
        keys: &[("District", DISTRICTS), ("Municipality", TOWNS)],
        measures: &[("Population", Kind::Int), ("Households", Kind::Int), ("Area", Kind::Dec(1)), ("Births", Kind::Int)],
    },
    Domain {
        name: "budget",
        title: "Budget and actuals",
        keys: &[("Department", DEPTS), ("Cost centre", CCS)],
        measures: &[("Budget", Kind::Money), ("Actual", Kind::Money), ("Variance", Kind::Signed)],
    },
    Domain {
        name: "energy",
        title: "Power generation by plant",
        keys: &[("Plant", PLANTS), ("Fuel", FUELS)],
        measures: &[("Capacity", Kind::Dec(1)), ("Generation", Kind::Int), ("Hours", Kind::Int)],
    },
    Domain {
        name: "health",
        title: "Hospital activity",
        keys: &[("Hospital", HOSPITALS), ("Ward", WARDS)],
        measures: &[("Beds", Kind::Int), ("Admissions", Kind::Int), ("Discharges", Kind::Int)],
    },
    Domain {
        name: "trade",
        title: "Foreign trade by country",
        keys: &[("Country", COUNTRIES), ("Product group", GOODS)],
        measures: &[("Exports", Kind::Money), ("Imports", Kind::Money), ("Balance", Kind::Signed)],
    },
];

/// Header translations for the multilingual family: en, de, fr, es, it, pl, sr (Latin), sr (Cyrillic).
pub const LANGS: &[&str] = &["en", "de", "fr", "es", "it", "pl", "sr", "sr_cyr"];
const TR: &[[&str; 8]] = &[
    ["Region", "Region", "Région", "Región", "Regione", "Region", "Region", "Регион"],
    ["Product", "Produkt", "Produit", "Producto", "Prodotto", "Produkt", "Proizvod", "Производ"],
    ["Units", "Stück", "Unités", "Unidades", "Unità", "Sztuki", "Komada", "Комада"],
    ["Revenue", "Umsatz", "Chiffre d'affaires", "Ingresos", "Ricavi", "Przychody", "Prihod", "Приход"],
    ["Cost", "Kosten", "Coût", "Coste", "Costo", "Koszt", "Trošak", "Трошак"],
    ["Returns", "Retouren", "Retours", "Devoluciones", "Resi", "Zwroty", "Povraćaj", "Повраћај"],
    ["District", "Bezirk", "Arrondissement", "Distrito", "Distretto", "Powiat", "Okrug", "Округ"],
    ["Municipality", "Gemeinde", "Commune", "Municipio", "Comune", "Gmina", "Opština", "Општина"],
    ["Population", "Einwohner", "Population", "Población", "Popolazione", "Ludność", "Stanovništvo", "Становништво"],
    ["Households", "Haushalte", "Ménages", "Hogares", "Famiglie", "Gospodarstwa", "Domaćinstva", "Домаћинства"],
    ["Area", "Fläche", "Superficie", "Superficie", "Superficie", "Powierzchnia", "Površina", "Површина"],
    ["Births", "Geburten", "Naissances", "Nacimientos", "Nascite", "Urodzenia", "Rođeni", "Рођени"],
    ["Department", "Abteilung", "Service", "Departamento", "Reparto", "Dział", "Sektor", "Сектор"],
    ["Cost centre", "Kostenstelle", "Centre de coût", "Centro de coste", "Centro di costo", "Centrum kosztów", "Mesto troška", "Место трошка"],
    ["Budget", "Budget", "Budget", "Presupuesto", "Bilancio", "Budżet", "Budžet", "Буџет"],
    ["Actual", "Ist", "Réalisé", "Real", "Effettivo", "Wykonanie", "Ostvareno", "Остварено"],
    ["Variance", "Abweichung", "Écart", "Desviación", "Scostamento", "Odchylenie", "Odstupanje", "Одступање"],
    ["Plant", "Kraftwerk", "Centrale", "Central", "Centrale", "Elektrownia", "Elektrana", "Електрана"],
    ["Fuel", "Brennstoff", "Combustible", "Combustible", "Combustibile", "Paliwo", "Gorivo", "Гориво"],
    ["Capacity", "Leistung", "Capacité", "Capacidad", "Capacità", "Moc", "Kapacitet", "Капацитет"],
    ["Generation", "Erzeugung", "Production", "Generación", "Produzione", "Produkcja", "Proizvodnja", "Производња"],
    ["Hours", "Stunden", "Heures", "Horas", "Ore", "Godziny", "Sati", "Сати"],
    ["Hospital", "Krankenhaus", "Hôpital", "Hospital", "Ospedale", "Szpital", "Bolnica", "Болница"],
    ["Ward", "Station", "Service", "Servicio", "Reparto", "Oddział", "Odeljenje", "Одељење"],
    ["Beds", "Betten", "Lits", "Camas", "Posti letto", "Łóżka", "Kreveti", "Кревети"],
    ["Admissions", "Aufnahmen", "Admissions", "Ingresos hospitalarios", "Ricoveri", "Przyjęcia", "Prijemi", "Пријеми"],
    ["Discharges", "Entlassungen", "Sorties", "Altas", "Dimissioni", "Wypisy", "Otpusti", "Отпусти"],
    ["Country", "Land", "Pays", "País", "Paese", "Kraj", "Zemlja", "Земља"],
    ["Product group", "Warengruppe", "Groupe de produits", "Grupo de productos", "Gruppo di prodotti", "Grupa towarów", "Grupa proizvoda", "Група производа"],
    ["Exports", "Ausfuhr", "Exportations", "Exportaciones", "Esportazioni", "Eksport", "Izvoz", "Извоз"],
    ["Imports", "Einfuhr", "Importations", "Importaciones", "Importazioni", "Import", "Uvoz", "Увоз"],
    ["Balance", "Saldo", "Solde", "Saldo", "Saldo", "Saldo", "Saldo", "Салдо"],
    ["Date", "Datum", "Date", "Fecha", "Data", "Data", "Datum", "Датум"],
];

/// `header` in language `lang` (index into [`LANGS`]); the English text when not in the table.
pub fn translate(header: &str, lang: usize) -> String {
    TR.iter().find(|r| r[0] == header).map_or(header.to_string(), |r| r[lang].to_string())
}

fn gen_value(rng: &mut Rng, kind: Kind, mag: f64) -> V {
    match kind {
        Kind::Int => V::Num((mag * (0.2 + rng.f() * 1.8)).round().max(1.0)),
        Kind::Dec(d) => V::Num(round(mag * (0.2 + rng.f() * 1.8), d)),
        Kind::Money => V::Num(round(mag * (0.2 + rng.f() * 1.8), 2)),
        Kind::Signed => V::Num(round(mag * (rng.f() * 2.0 - 1.0), 2)),
        Kind::Date => V::Date((44927 + rng.below(730)) as f64),
        Kind::Key => V::Text(String::new()),
    }
}

pub struct GenSpec {
    /// Wanted number of data rows (approximate when grouped).
    pub rows: usize,
    /// Two key columns with the first grouped (contiguous); otherwise one key column.
    pub grouped: bool,
    pub max_measures: usize,
    pub date_col: bool,
}

pub fn generate(rng: &mut Rng, spec: &GenSpec) -> BaseTable {
    let d = &DOMAINS[rng.below(DOMAINS.len())];
    let mut cols = Vec::new();
    let nkeys = if spec.grouped { 2 } else { 1 };
    let key_col = if spec.grouped { 0 } else { rng.below(2) };
    for k in 0..nkeys {
        let (h, _) = d.keys[if spec.grouped { k } else { key_col }];
        cols.push(Col { header: h.to_string(), kind: Kind::Key });
    }
    if spec.date_col {
        cols.push(Col { header: "Date".into(), kind: Kind::Date });
    }
    let nm = d.measures.len().min(spec.max_measures.max(1));
    let nm = rng.int(2.min(nm as i64), nm as i64) as usize;
    let mut ms: Vec<usize> = (0..d.measures.len()).collect();
    rng.shuffle(&mut ms);
    let mut ms: Vec<usize> = ms.into_iter().take(nm).collect();
    ms.sort();
    let mags: Vec<f64> = ms.iter().map(|_| 10f64.powi(rng.int(1, 4) as i32) * (1.0 + rng.f() * 4.0)).collect();
    for &m in &ms {
        cols.push(Col { header: d.measures[m].0.to_string(), kind: d.measures[m].1 });
    }
    let mut rows = Vec::new();
    if spec.grouped {
        let (_, gpool) = d.keys[0];
        let (_, ipool) = d.keys[1];
        let mut gs: Vec<&str> = gpool.to_vec();
        rng.shuffle(&mut gs);
        let per = (spec.rows / 3).clamp(2, 6);
        let ng = (spec.rows / per).clamp(2, gs.len());
        for g in gs.iter().take(ng) {
            let mut items: Vec<&str> = ipool.to_vec();
            rng.shuffle(&mut items);
            let n = rng.int(2, per as i64 + 1).min(items.len() as i64) as usize;
            for it in items.iter().take(n) {
                rows.push(vec![V::Text(g.to_string()), V::Text(it.to_string())]);
            }
        }
    } else {
        let (_, pool) = d.keys[key_col];
        let mut items: Vec<String> = pool.iter().map(|s| s.to_string()).collect();
        // Long single-key tables reuse names with a number so labels stay unique.
        let mut k = 2;
        while items.len() < spec.rows {
            items.extend(pool.iter().map(|s| format!("{s} {k}")));
            k += 1;
        }
        rng.shuffle(&mut items);
        for it in items.into_iter().take(spec.rows.max(3)) {
            rows.push(vec![V::Text(it)]);
        }
    }
    for r in rows.iter_mut() {
        if spec.date_col {
            r.push(gen_value(rng, Kind::Date, 0.0));
        }
        for (k, &m) in ms.iter().enumerate() {
            r.push(gen_value(rng, d.measures[m].1, mags[k]));
        }
    }
    BaseTable {
        name: format!("gen:{}", d.name),
        title: d.title.to_string(),
        cols,
        rows,
        group: if spec.grouped { Some(0) } else { None },
        real: false,
    }
}

// ---------------------------------------------------------------------------------------------
// Real clean tables

/// Small clean CSV tables (MANIFEST.csv licenses: CC0 / public domain / MIT / CC BY).
pub const REAL_FILES: &[&str] = &[
    "plotly_datasets/tips.csv",
    "fivethirtyeight/airline_safety.csv",
    "fivethirtyeight/bad_drivers.csv",
    "northwind/products.csv",
    "plotly_datasets/gapminder_five_year.csv",
    "data_gov_rs/rzs_zaposleni_cyr.csv",
    "fivethirtyeight/drug_use_by_age.csv",
    "vega_datasets/seattle_weather.csv",
];

pub struct RealSource {
    pub file: String,
    pub headers: Vec<String>,
    pub key: usize,
    pub measures: Vec<usize>,
    /// Rows where the key and every measure are filled.
    pub rows: Vec<Vec<V>>,
}

pub fn load_real(root: &std::path::Path) -> Vec<RealSource> {
    let mut out = Vec::new();
    for f in REAL_FILES {
        let path = root.join(f);
        let Ok(wb) = hh_core::open(&path) else { continue };
        let Some(sh) = wb.sheets.first() else { continue };
        let ncols = sh.cells.iter().map(|c| c.col + 1).max().unwrap_or(0) as usize;
        let nrows = sh.cells.iter().map(|c| c.row + 1).max().unwrap_or(0) as usize;
        if nrows < 12 || ncols < 2 {
            continue;
        }
        let mut grid: Vec<Vec<Option<V>>> = vec![vec![None; ncols]; nrows];
        for c in &sh.cells {
            let v = match &c.value {
                Some(hh_core::Value::Number(x)) => Some(V::Num(*x)),
                Some(hh_core::Value::Text(s)) if !s.trim().is_empty() => Some(V::Text(s.trim().to_string())),
                Some(hh_core::Value::DateTime(d)) => Some(V::Date(*d)),
                _ => None,
            };
            grid[c.row as usize][c.col as usize] = v;
        }
        let headers: Vec<String> = (0..ncols)
            .map(|j| match &grid[0][j] {
                Some(V::Text(s)) => s.clone(),
                _ => format!("col{j}"),
            })
            .collect();
        let body = &grid[1..];
        let share = |j: usize, f: &dyn Fn(&V) -> bool| {
            body.iter().filter(|r| r[j].as_ref().is_some_and(f)).count() as f64 / body.len() as f64
        };
        let mut key = None;
        let mut best = 0usize;
        for j in 0..ncols {
            if share(j, &|v| matches!(v, V::Text(s) if s.chars().count() <= 40 && s.chars().any(|c| c.is_alphabetic()))) >= 0.95 {
                let mut d: Vec<String> =
                    body.iter().filter_map(|r| if let Some(V::Text(s)) = &r[j] { Some(s.clone()) } else { None }).collect();
                d.sort();
                d.dedup();
                if d.len() > best {
                    best = d.len();
                    key = Some(j);
                }
            }
        }
        let Some(key) = key else { continue };
        let measures: Vec<usize> = (0..ncols)
            .filter(|&j| j != key && share(j, &|v| matches!(v, V::Num(_))) >= 0.98)
            .filter(|&j| !headers[j].to_lowercase().contains("id"))
            .take(4)
            .collect();
        if measures.len() < 2 {
            continue;
        }
        let rows: Vec<Vec<V>> = body
            .iter()
            .filter(|r| r[key].is_some() && measures.iter().all(|&m| r[m].is_some()))
            .map(|r| {
                let mut v = vec![r[key].clone().unwrap()];
                v.extend(measures.iter().map(|&m| r[m].clone().unwrap()));
                v
            })
            .collect();
        if rows.len() < 12 {
            continue;
        }
        out.push(RealSource { file: f.to_string(), headers, key, measures, rows });
    }
    out
}

/// A window of `n` consecutive rows of a real table as a clean base table.
pub fn sample_real(rng: &mut Rng, src: &RealSource, n: usize) -> BaseTable {
    let n = n.min(src.rows.len());
    let start = rng.below(src.rows.len() - n + 1);
    let mut cols = vec![Col { header: src.headers[src.key].clone(), kind: Kind::Key }];
    for (k, &m) in src.measures.iter().enumerate() {
        let decimals = src.rows[start..start + n]
            .iter()
            .map(|r| match &r[k + 1] {
                V::Num(x) => {
                    let s = format!("{x}");
                    s.split('.').nth(1).map_or(0, |f| f.len())
                }
                _ => 0,
            })
            .max()
            .unwrap_or(0);
        let neg = src.rows[start..start + n].iter().any(|r| matches!(r[k + 1], V::Num(x) if x < 0.0));
        let kind = if neg {
            Kind::Signed
        } else if decimals == 0 {
            Kind::Int
        } else {
            Kind::Dec(decimals.min(6) as u8)
        };
        cols.push(Col { header: src.headers[m].clone(), kind });
    }
    BaseTable {
        name: format!("real:{}", src.file),
        title: format!("Extract from {}", src.file.rsplit('/').next().unwrap_or(&src.file).trim_end_matches(".csv")),
        cols,
        rows: src.rows[start..start + n].to_vec(),
        group: None,
        real: true,
    }
}
