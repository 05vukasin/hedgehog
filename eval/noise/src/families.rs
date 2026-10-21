//! Noise families. Each takes an intensity `t` in [0, 1] (0 = the clean table, 1 = the hardest
//! variant) and a seeded RNG and builds one sheet with its truth.
//!
//! Two kinds: *mutators* change one table's plan (headers, rows, values) and are rendered as a
//! single table; *arrangements* place several tables or reshape one (side by side, stacked,
//! blocks, transposed, long registers, wide period grids). `combo` stacks three mutators.
//!
//! Designed independently of `tools/noise` (different code, vocabulary and random design).

use crate::base::{self, round, BaseTable, Col, GenSpec, Kind, RealSource, V};
use crate::render::{self, Canvas, CellNoise, KHStyle, KeyStyle, Parent, Plan, PStyle, TotKind, CI, LOCS, RI};
use crate::rng::Rng;

pub struct Ctx {
    pub real: Vec<RealSource>,
}

pub const FAMILIES: &[&str] = &[
    "multirow_header",
    "side_by_side",
    "stacked",
    "label_blocks",
    "labels_once",
    "subtotals",
    "of_which",
    "average_rows",
    "unit_scale",
    "footnotes",
    "locale",
    "notes",
    "spacers",
    "separators",
    "transposed",
    "long_register",
    "wide_periods",
    "mixed_types",
    "total_labels",
    "multilingual",
    "combo",
];

pub fn describe(f: &str) -> &'static str {
    match f {
        "multirow_header" => "1–3 parent header rows over the measures (merged, written once, repeated or centred); years × measures with repeated child names",
        "side_by_side" => "2–4 tables side by side, gaps of 0–2 columns, later tables sharing the first table's row labels",
        "stacked" => "2–4 tables stacked, 0–3 blank rows apart, same or different schema, block titles, header repeated or not",
        "label_blocks" => "one wide table cut into 2–6 year blocks side by side, each block repeating the label columns",
        "labels_once" => "group label written once per block (blank below) or vertically merged",
        "subtotals" => "group subtotals at the bottom or top of each group, group label rows, grand total at the bottom or top, typed or formula",
        "of_which" => "'of which' / 'davon' / 'od toga' breakdown rows under data rows, plus a grand total",
        "average_rows" => "average / mean / Ø rows (group and overall) that must not be summed or flagged as wrong totals",
        "unit_scale" => "scale stated in the title, a unit row, the header text, a corner cell or a note ('in 1000 EUR', 'Mio. €')",
        "footnotes" => "footnote marks in numbers (123¹, 45*, 12 (r), 7 p) and headers, legend lines under the table",
        "locale" => "numbers and dates as text in US/EU/FR/CH conventions, negatives in parentheses, '-', '…', '..' for missing",
        "notes" => "notes above the header, inside the body, directly under the last row, and comments right of the table",
        "spacers" => "blank spacer rows every k rows and blank spacer columns",
        "separators" => "rows of ----- / ===== under the header and before totals, a | separator column",
        "transposed" => "periods across instead of down (records as columns), with a year row over quarters",
        "long_register" => "200–2000-row register whose descriptions contain total words ('Total knee replacement', 'Ukupni troškovi')",
        "wide_periods" => "6–60 period columns (years, 'Jan 2021', '2021-01', or a year row over month names)",
        "mixed_types" => "free text in numeric columns ('<5', 'confidential', 'n.a.', 'see note')",
        "total_labels" => "grand total rows with rare, foreign and misspelled labels (Totals, Insgesamt, Ukupno, Razem, Toal, ∑)",
        "multilingual" => "headers in de/fr/es/it/pl/sr/Cyrillic, bilingual two-row headers, Latin/Cyrillic mixed",
        "combo" => "three random mutators on one table at the same intensity",
        _ => "",
    }
}

#[derive(Clone, Copy)]
struct Need {
    grouped: Option<bool>,
    real_ok: bool,
    rows: (usize, usize),
    date: bool,
}

const ANY: Need = Need { grouped: None, real_ok: true, rows: (8, 24), date: false };

fn base_table(rng: &mut Rng, ctx: &Ctx, need: Need) -> BaseTable {
    let n = rng.int(need.rows.0 as i64, need.rows.1 as i64) as usize;
    if need.real_ok && need.grouped != Some(true) && !ctx.real.is_empty() && rng.chance(0.3) {
        let src = &ctx.real[rng.below(ctx.real.len())];
        return base::sample_real(rng, src, n);
    }
    let grouped = need.grouped.unwrap_or_else(|| rng.chance(0.6));
    let date_col = need.date && rng.chance(0.6);
    base::generate(rng, &GenSpec { rows: n, grouped, max_measures: 4, date_col })
}

fn place(cv: &mut Canvas, rng: &mut Rng, p: &Plan) -> u32 {
    let top = rng.below(3) as u32;
    let left = rng.below(2) as u32;
    render::render(cv, rng, p, top, left)
}

pub fn build(family: &str, t: f64, rng: &mut Rng, ctx: &Ctx) -> Canvas {
    let mut cv = Canvas::default();
    match family {
        "side_by_side" => side_by_side(&mut cv, rng, ctx, t),
        "stacked" => stacked(&mut cv, rng, ctx, t),
        "label_blocks" => label_blocks(&mut cv, rng, t),
        "transposed" => transposed(&mut cv, rng, t),
        "long_register" => long_register(&mut cv, rng, t),
        "wide_periods" => wide_periods(&mut cv, rng, t),
        "combo" => {
            let mut pool: Vec<&str> = MUTATORS.to_vec();
            rng.shuffle(&mut pool);
            // value noise that writes numbers as text conflicts with totals (Excel's SUM skips
            // text), so a combo takes at most one of each side
            let mut chosen: Vec<&str> = Vec::new();
            for m in pool {
                let totals = TOTALS_MUTATORS.contains(&m);
                let text = TEXT_MUTATORS.contains(&m);
                if (totals && chosen.iter().any(|c| TEXT_MUTATORS.contains(c)))
                    || (text && chosen.iter().any(|c| TOTALS_MUTATORS.contains(c)))
                    || (m == "multilingual" && chosen.iter().any(|c| *c == "multirow_header"))
                    || (m == "multirow_header" && chosen.contains(&"multilingual"))
                {
                    continue;
                }
                chosen.push(m);
                if chosen.len() == 3 {
                    break;
                }
            }
            chosen.sort_by_key(|m| MUTATORS.iter().position(|x| x == m));
            let grouped = chosen.iter().any(|m| matches!(*m, "subtotals" | "labels_once"));
            let generated = chosen.contains(&"multilingual");
            let need = Need { grouped: if grouped { Some(true) } else { None }, real_ok: !generated, ..ANY };
            let mut p = Plan::new(base_table(rng, ctx, need));
            cv.desc.push(format!("combo of {}; base {}", chosen.join(" + "), p.base.name));
            for m in chosen {
                mutate(m, &mut p, rng, t, &mut cv.desc);
            }
            place(&mut cv, rng, &p);
        }
        m => {
            let need = match m {
                "subtotals" | "labels_once" => Need { grouped: Some(true), real_ok: false, ..ANY },
                "multilingual" => Need { real_ok: false, ..ANY },
                "locale" => Need { real_ok: true, date: true, ..ANY },
                _ => ANY,
            };
            let mut p = Plan::new(base_table(rng, ctx, need));
            cv.desc.push(format!("base {} ({} rows × {} cols)", p.base.name, p.base.rows.len(), p.base.cols.len()));
            mutate(m, &mut p, rng, t, &mut cv.desc);
            place(&mut cv, rng, &p);
        }
    }
    cv
}

/// Mutators in the order a combo applies them (rows first, then headers, then values).
const MUTATORS: &[&str] = &[
    "subtotals",
    "labels_once",
    "of_which",
    "average_rows",
    "total_labels",
    "notes",
    "spacers",
    "separators",
    "multirow_header",
    "multilingual",
    "unit_scale",
    "footnotes",
    "locale",
    "mixed_types",
];
const TOTALS_MUTATORS: &[&str] = &["subtotals", "of_which", "average_rows", "total_labels"];
const TEXT_MUTATORS: &[&str] = &["footnotes", "locale", "mixed_types"];

fn mutate(m: &str, p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    match m {
        "multirow_header" => multirow_header(p, rng, t, desc),
        "labels_once" => labels_once(p, rng, t, desc),
        "subtotals" => subtotals(p, rng, t, desc),
        "of_which" => of_which(p, rng, t, desc),
        "average_rows" => average_rows(p, rng, t, desc),
        "unit_scale" => unit_scale(p, rng, t, desc),
        "footnotes" => footnotes(p, rng, t, desc),
        "locale" => locale(p, rng, t, desc),
        "notes" => notes(p, rng, t, desc),
        "spacers" => spacers(p, rng, t, desc),
        "separators" => separators(p, rng, t, desc),
        "mixed_types" => mixed_types(p, rng, t, desc),
        "total_labels" => total_labels(p, rng, t, desc),
        "multilingual" => multilingual(p, rng, t, desc),
        _ => panic!("unknown mutator {m}"),
    }
}

fn steps(t: f64, n: usize) -> usize {
    (t * n as f64).round() as usize
}

/// Data rows of the plan in order.
fn data_rows(p: &Plan) -> Vec<usize> {
    p.rows.iter().filter_map(|r| if let RI::Data(i) = r { Some(*i) } else { None }).collect()
}

/// Position in `p.rows` right after data row `i`.
fn after_data(p: &Plan, i: usize) -> usize {
    p.rows.iter().position(|r| matches!(r, RI::Data(x) if *x == i)).map_or(p.rows.len(), |k| k + 1)
}

// ---------------------------------------------------------------------------------------------
// Header mutators

const PARENT_WORDS: &[&str] =
    &["Volume", "Value", "Financials", "Key figures", "Indicators", "Measures", "Results", "Current period", "Activity"];
const TOP_WORDS: &[&str] = &["FY 2024", "Fiscal year 2023/24", "Reporting period", "Calendar year 2025", "All units", "Year to date"];

fn multirow_header(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    let levels = steps(t, 3);
    if levels == 0 {
        return;
    }
    let style = |rng: &mut Rng| {
        if rng.chance(1.0 - t * 0.7) {
            PStyle::Merged
        } else {
            *rng.pick(&[PStyle::Once, PStyle::Repeated, PStyle::Centered])
        }
    };
    p.key_header = *rng.pick(&[KHStyle::Merged, KHStyle::Merged, KHStyle::Bottom, KHStyle::Top]);
    let ms = p.base.measures();
    if ms.is_empty() {
        return;
    }
    // years × measures with repeated child names
    if t >= 0.5 && rng.chance(0.5) && ms.len() <= 3 {
        let years: Vec<String> = (0..rng.int(2, 3)).map(|k| format!("{}", 2022 + k)).collect();
        widen_years(p, rng, &years);
        let ms = p.base.measures();
        let per = ms.len() / years.len();
        let bottom = levels - 1;
        for (y, yr) in years.iter().enumerate() {
            let s = style(rng);
            p.parents.push(Parent { level: bottom, first: ms[y * per], last: ms[y * per + per - 1], text: yr.clone(), style: s });
        }
        for lv in 0..bottom {
            p.parents.push(Parent { level: lv, first: ms[0], last: *ms.last().unwrap(), text: rng.pick(TOP_WORDS).to_string(), style: style(rng) });
        }
        p.levels = levels;
        recompute_header_truth(p);
        desc.push(format!("multirow header: {levels} parent rows, years {} × measures (repeated child names)", years.join("/")));
        return;
    }
    let bottom = levels - 1;
    let split = if ms.len() >= 3 && rng.chance(0.6) { rng.int(1, ms.len() as i64 - 1) as usize } else { ms.len() };
    let mut words: Vec<&str> = PARENT_WORDS.to_vec();
    rng.shuffle(&mut words);
    p.parents.push(Parent { level: bottom, first: ms[0], last: ms[split - 1], text: words[0].to_string(), style: style(rng) });
    if split < ms.len() {
        p.parents.push(Parent { level: bottom, first: ms[split], last: *ms.last().unwrap(), text: words[1].to_string(), style: style(rng) });
    }
    for lv in 0..bottom {
        p.parents.push(Parent { level: lv, first: ms[0], last: *ms.last().unwrap(), text: rng.pick(TOP_WORDS).to_string(), style: style(rng) });
    }
    p.levels = levels;
    recompute_header_truth(p);
    let styles: Vec<String> = p.parents.iter().map(|x| format!("{:?}", x.style)).collect();
    desc.push(format!("multirow header: {levels} parent rows ({}), key headers {:?}", styles.join(","), p.key_header));
}

/// Replace the measures by one copy per year (values varied), so child names repeat.
fn widen_years(p: &mut Plan, rng: &mut Rng, years: &[String]) {
    let keys = p.base.keys();
    let ms = p.base.measures();
    let others: Vec<usize> = (0..p.base.cols.len()).filter(|j| !keys.contains(j) && !ms.contains(j)).collect();
    let mut cols: Vec<Col> = keys.iter().chain(others.iter()).map(|&j| p.base.cols[j].clone()).collect();
    for _ in years {
        for &m in &ms {
            cols.push(p.base.cols[m].clone());
        }
    }
    let rows = p
        .base
        .rows
        .iter()
        .map(|r| {
            let mut v: Vec<V> = keys.iter().chain(others.iter()).map(|&j| r[j].clone()).collect();
            for _ in years {
                for &m in &ms {
                    let x = match &r[m] {
                        V::Num(x) => *x,
                        _ => 0.0,
                    };
                    let d = render::decimals(p.base.cols[m].kind);
                    v.push(V::Num(round(x * (0.7 + rng.f() * 0.6), d)));
                }
            }
            v
        })
        .collect();
    let group = p.base.group.and_then(|g| keys.iter().position(|&k| k == g));
    p.base = BaseTable { cols, rows, group, ..p.base.clone() };
    p.headers = p.base.cols.iter().map(|c| c.header.clone()).collect();
    p.header_truth = p.headers.iter().map(|h| vec![h.clone()]).collect();
    p.cols = (0..p.base.cols.len()).map(CI::Col).collect();
}

fn recompute_header_truth(p: &mut Plan) {
    for j in 0..p.base.cols.len() {
        let mut path: Vec<(usize, String)> = p
            .parents
            .iter()
            .filter(|x| x.first <= j && j <= x.last && !p.is_key(j))
            .map(|x| (x.level, x.text.clone()))
            .collect();
        path.sort();
        let mut v: Vec<String> = path.into_iter().map(|x| x.1).collect();
        v.extend(p.header_truth[j].last().cloned());
        p.header_truth[j] = v;
    }
}

fn multilingual(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 {
        return;
    }
    let lang = 1 + rng.below(base::LANGS.len() - 1);
    let n = p.base.cols.len();
    if t >= 0.5 && rng.chance(0.5) && p.levels == 0 {
        // bilingual two-row header: local language over English, one parent per column
        let headers = p.headers.clone();
        for j in 0..n {
            let local = base::translate(&p.base.cols[j].header, lang);
            if p.is_key(j) {
                p.headers[j] = format!("{local} / {}", headers[j]);
                p.header_truth[j] = vec![local, headers[j].clone()];
            } else {
                p.parents.push(Parent { level: 0, first: j, last: j, text: local.clone(), style: PStyle::Merged });
                p.header_truth[j] = vec![local, headers[j].clone()];
            }
        }
        p.levels = 1;
        p.key_header = KHStyle::Bottom;
        desc.push(format!("bilingual header {} over en", base::LANGS[lang]));
        return;
    }
    let mut used = vec![base::LANGS[lang]];
    for j in 0..n {
        // at high intensity the columns mix Serbian Latin and Cyrillic (or two languages)
        let l = if t >= 0.75 && rng.chance(0.5) {
            let alt = if base::LANGS[lang] == "sr" { 7 } else if base::LANGS[lang] == "sr_cyr" { 6 } else { 1 + rng.below(base::LANGS.len() - 1) };
            used.push(base::LANGS[alt]);
            alt
        } else {
            lang
        };
        let mut h = base::translate(&p.base.cols[j].header, l);
        if t >= 1.0 && rng.chance(0.4) {
            h = h.to_uppercase();
        }
        p.headers[j] = h.clone();
        p.header_truth[j] = vec![h];
    }
    used.sort();
    used.dedup();
    desc.push(format!("headers in {}", used.join("+")));
}

// ---------------------------------------------------------------------------------------------
// Row mutators

fn labels_once(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 || p.base.group.is_none() {
        return;
    }
    p.key_style = if rng.chance(1.0 - t * 0.5) { KeyStyle::Once } else { KeyStyle::Merge };
    let mut extra = String::new();
    if t >= 0.75 && rng.chance(0.5) {
        // a blank row between the blocks
        let groups = p.base.groups();
        for &(_, end) in groups.iter().rev().skip(1) {
            let at = after_data(p, end - 1);
            p.rows.insert(at, RI::Blank);
        }
        extra = ", blank row between blocks".into();
    }
    desc.push(format!("group labels {:?}{extra}", p.key_style));
}

const SUB_EN: &[&str] = &["Total {g}", "{g} Total", "Subtotal", "Subtotal {g}", "{g} - total", "Sum {g}", "{g} total"];
const SUB_OTHER: &[&str] = &["Summe {g}", "{g} gesamt", "Zwischensumme", "Ukupno {g}", "Sous-total {g}", "Totale {g}", "Razem {g}"];
const GRAND: &[&str] = &["Total", "Grand total", "TOTAL", "Total all regions", "Overall total"];

fn subtotals(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if p.base.group.is_none() {
        return;
    }
    let groups = p.base.groups();
    let g = p.base.group.unwrap();
    let top = rng.chance(t * 0.5);
    let fmt = if rng.chance(t * 0.5) { *rng.pick(SUB_OTHER) } else { *rng.pick(SUB_EN) };
    p.formula_totals = (0.6 - 0.5 * t).max(0.0);
    let mut d = vec![format!("subtotals at the {} of each group as '{fmt}'", if top { "top" } else { "bottom" })];
    // insert from the last group so positions stay valid
    for &(a, b) in groups.iter().rev() {
        let label = fmt.replace("{g}", &p.base.text(a, g));
        let tot = RI::Total { label, rows: (a..b).collect(), kind: TotKind::Sum };
        let at = if top {
            p.rows.iter().position(|r| matches!(r, RI::Data(x) if *x == a)).unwrap()
        } else {
            after_data(p, b - 1)
        };
        p.rows.insert(at, tot);
    }
    if rng.chance(t * 0.6) {
        p.use_group_rows(&|s| s.to_string());
        d.push("group label rows".into());
    } else if rng.chance(t * 0.5) {
        p.key_style = KeyStyle::Once;
        d.push("group label written once".into());
    }
    if t > 0.0 && rng.chance(0.5 + t * 0.5) {
        let all: Vec<usize> = (0..p.base.rows.len()).collect();
        let label = rng.pick(GRAND).to_string();
        let gtop = rng.chance(t * 0.4);
        let tot = RI::Total { label: label.clone(), rows: all, kind: TotKind::Sum };
        if gtop {
            p.rows.insert(0, tot);
        } else {
            p.rows.push(tot);
        }
        d.push(format!("grand total '{label}' at the {}", if gtop { "top" } else { "bottom" }));
    }
    if t == 0.0 {
        // the clean variant: plain bottom subtotals only
        p.formula_totals = 0.6;
    }
    desc.push(d.join(", "));
}

const OF_WHICH: &[&str] = &[
    "of which: domestic",
    "  of which exports",
    "davon Inland",
    "davon: Export",
    "od toga: izvoz",
    "dont : exportations",
    "di cui: estero",
    "w tym: eksport",
    "thereof abroad",
    "- of which online",
];

fn of_which(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 {
        return;
    }
    let rows = data_rows(p);
    let k = ((t * rows.len() as f64 / 3.0).round() as usize).max(1);
    let mut pick: Vec<usize> = rows.clone();
    rng.shuffle(&mut pick);
    let mut pick: Vec<usize> = pick.into_iter().take(k).collect();
    pick.sort();
    let mut labels = Vec::new();
    for &i in pick.iter().rev() {
        let label = rng.pick(OF_WHICH).to_string();
        labels.push(label.clone());
        let at = after_data(p, i);
        p.rows.insert(at, RI::OfWhich { label, parent: i, frac: 0.2 + rng.f() * 0.6 });
    }
    let mut d = format!("{k} of-which rows ({})", labels.join(" | "));
    if rng.chance(0.6) {
        p.rows.push(RI::Total { label: rng.pick(GRAND).to_string(), rows: rows.clone(), kind: TotKind::Sum });
        d.push_str(", grand total");
    }
    desc.push(d);
}

const AVG: &[&str] = &["Average", "Mean", "Avg.", "Durchschnitt", "Ø", "Moyenne", "Promedio", "Prosek", "Media", "Średnia", "Median"];

fn average_rows(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 {
        return;
    }
    let all: Vec<usize> = (0..p.base.rows.len()).collect();
    let mut d = Vec::new();
    if t >= 0.5 && p.base.group.is_some() && rng.chance(0.6) {
        let g = p.base.group.unwrap();
        for &(a, b) in p.base.groups().iter().rev() {
            let label = format!("{} {}", rng.pick(AVG), p.base.text(a, g));
            let at = after_data(p, b - 1);
            p.rows.insert(at, RI::Total { label, rows: (a..b).collect(), kind: TotKind::Average });
        }
        d.push("group averages".to_string());
    }
    let avg = RI::Total { label: rng.pick(AVG).to_string(), rows: all.clone(), kind: TotKind::Average };
    let total = RI::Total { label: rng.pick(GRAND).to_string(), rows: all, kind: TotKind::Sum };
    if rng.chance(0.5) {
        if t >= 0.75 && rng.chance(0.5) {
            p.rows.push(avg);
            p.rows.push(total);
            d.push("average then total".into());
        } else {
            p.rows.push(total);
            p.rows.push(avg);
            d.push("total then average".into());
        }
    } else {
        p.rows.push(avg);
        d.push("average row only".into());
    }
    desc.push(d.join(", "));
}

const TOTAL_TIERS: &[&[&str]] = &[
    &["Total", "TOTAL", "Grand total"],
    &["Totals", "Grand Total", "Total:", "Sum", "Ukupno", "Insgesamt", "Summe", "Gesamt", "Total général", "Totale", "Totaal", "Suma", "Razem"],
    &["UKUPNO", "Svega", "Sveukupno", "Celkem", "Yhteensä", "I alt", "Totalt", "Összesen", "Ogółem", "Всего", "Укупно", "Итого", "Gesamtsumme", "Totaal generaal", "Totale complessivo"],
    &["Toal", "Totl", "Grand Totl", "T o t a l", "Tot.", "Totaal:", "Ukupno:", "SUMME", "total (all)", "∑", "Total general:"],
];

fn total_labels(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    let tier = rng.below(1 + steps(t, 3));
    let label = rng.pick(TOTAL_TIERS[tier]).to_string();
    let all: Vec<usize> = (0..p.base.rows.len()).collect();
    let top = t >= 0.75 && rng.chance(0.3);
    let tot = RI::Total { label: label.clone(), rows: all, kind: TotKind::Sum };
    if top {
        p.rows.insert(0, tot);
    } else {
        p.rows.push(tot);
    }
    desc.push(format!("typed grand total '{label}' (tier {tier}) at the {}", if top { "top" } else { "bottom" }));
}

const NOTE_TEXT: &[&str] = &[
    "Note: figures for the last period are provisional.",
    "* break in series",
    "Source: Statistical Office, annual survey 2024",
    "Quelle: eigene Berechnungen",
    "Izvor: Republički zavod za statistiku",
    "Data as reported by the departments; not audited.",
    "Remarque : données révisées",
    "Fuente: elaboración propia",
];
const COMMENTS: &[&str] = &["check!", "see email from JS", "revised", "?", "incl. late invoices", "per KL 3/4"];

fn notes(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    let k = steps(t, 4);
    let mut kinds: Vec<&str> = vec!["above", "below_gap0", "inside", "comment", "below_gap1"];
    rng.shuffle(&mut kinds);
    let mut d = Vec::new();
    for kind in kinds.into_iter().take(k) {
        let text = rng.pick(NOTE_TEXT).to_string();
        match kind {
            "above" => p.notes_above.push(text),
            "below_gap0" => {
                p.notes_below.push(text);
                p.notes_gap = 0;
            }
            "below_gap1" => p.notes_below.push(text),
            "inside" => {
                let rows = data_rows(p);
                let i = rows[rows.len() / 2];
                let at = after_data(p, i);
                p.rows.insert(at, RI::Note(text));
            }
            _ => {
                let rows = data_rows(p);
                let i = rows[rng.below(rows.len())];
                p.comments.push((i, rng.pick(COMMENTS).to_string()));
            }
        }
        d.push(kind);
    }
    if !d.is_empty() {
        desc.push(format!("notes: {}", d.join(", ")));
    }
}

fn spacers(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 {
        return;
    }
    let every = 6 - steps(t, 4); // 5..2
    let rows = data_rows(p);
    let double = t >= 0.75 && rng.chance(0.4);
    for (n, &i) in rows.iter().enumerate().rev() {
        if (n + 1) % every == 0 && n + 1 < rows.len() {
            let at = after_data(p, i);
            p.rows.insert(at, RI::Blank);
            if double {
                p.rows.insert(at, RI::Blank);
            }
        }
    }
    let mut d = format!("blank row every {every} rows{}", if double { " (double)" } else { "" });
    if rng.chance(t) {
        let k = p.cols.iter().position(|c| matches!(c, CI::Col(j) if !p.is_key(*j))).unwrap_or(1);
        p.cols.insert(k, CI::Blank);
        d.push_str(", blank column after the labels");
    }
    if rng.chance(t * 0.5) && p.cols.len() > 4 {
        let k = p.cols.len() - 1;
        p.cols.insert(k, CI::Blank);
        d.push_str(", blank column between measures");
    }
    desc.push(d);
}

const SEPS: &[&str] = &["-----", "=====", "_____", "*****", "~~~~~", "---------------"];

fn separators(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 {
        return;
    }
    let s = rng.pick(SEPS).to_string();
    let mut d = vec![format!("'{s}' rows")];
    if rng.chance(0.3 + 0.7 * t) {
        p.header_sep = Some(s.clone());
        d.push("under the header".into());
    }
    if rng.chance(0.5) {
        let all: Vec<usize> = (0..p.base.rows.len()).collect();
        p.rows.push(RI::Sep(s.clone()));
        p.rows.push(RI::Total { label: rng.pick(GRAND).to_string(), rows: all, kind: TotKind::Sum });
        d.push("before a grand total".into());
    }
    if t >= 0.5 {
        let rows = data_rows(p);
        let every = 4 + rng.below(3);
        for (n, &i) in rows.iter().enumerate().rev() {
            if (n + 1) % every == 0 && n + 1 < rows.len() {
                let at = after_data(p, i);
                p.rows.insert(at, RI::Sep(s.clone()));
            }
        }
        d.push(format!("every {every} rows"));
    }
    if t >= 0.75 && rng.chance(0.5) {
        let k = p.cols.iter().position(|c| matches!(c, CI::Col(j) if !p.is_key(*j))).unwrap_or(1);
        p.cols.insert(k, CI::Sep("|".into()));
        d.push("'|' column".into());
    }
    desc.push(d.join(", "));
}

// ---------------------------------------------------------------------------------------------
// Value mutators

const SCALES: &[(f64, &str, &[&str])] = &[
    (1000.0, "in 1000 EUR", &["1000"]),
    (1000.0, "(in thousands)", &["thousand"]),
    (1000.0, "Tsd. EUR", &["tsd"]),
    (1000.0, "USD '000", &["000"]),
    (1000.0, "u hiljadama dinara", &["hiljad"]),
    (1000.0, "en milliers d'euros", &["millier"]),
    (1_000_000.0, "Mio. €", &["mio"]),
    (1_000_000.0, "EUR million", &["million"]),
    (1_000_000.0, "in mln PLN", &["mln"]),
];

fn unit_scale(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 {
        return;
    }
    let (f, text, toks) = *rng.pick(SCALES);
    let tokens: Vec<String> = toks.iter().map(|s| s.to_string()).collect();
    p.scale = Some((f, tokens));
    // placements, easier first; intensity unlocks the harder ones
    let n = 1 + steps(t, 4);
    let at = rng.below(n);
    let ms = p.base.measures();
    let where_ = match at {
        0 => {
            if let Some(tl) = p.title.first_mut() {
                tl.push_str(&format!(" ({text})"));
            } else {
                p.title.push(text.to_string());
            }
            "title"
        }
        1 => {
            p.title.push(text.to_string());
            "own line under the title"
        }
        2 => {
            let mut units = vec![String::new(); p.base.cols.len()];
            for &m in &ms {
                units[m] = text.to_string();
            }
            p.unit_row = Some(units);
            "unit row under the header"
        }
        3 => {
            for &m in &ms {
                p.headers[m] = format!("{} ({text})", p.headers[m]);
            }
            "in every measure header"
        }
        _ => {
            p.notes_below.push(format!("All amounts {text}."));
            "note under the table"
        }
    };
    desc.push(format!("scale '{text}' ×{f} in the {where_}"));
}

fn footnotes(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 {
        return;
    }
    p.noise.footnote = t * 0.5;
    let ms = p.base.measures();
    let mut hm = 0;
    for &m in &ms {
        if rng.chance(t * 0.6) {
            p.headers[m].push_str(rng.pick(&["¹", "²", "*", " 1)", "(a)"]));
            hm += 1;
        }
    }
    let legends = ["¹ provisional", "* estimate", "(r) revised", "p = preliminary", "e = estimated", "1) without VAT"];
    let k = 1 + steps(t, 2);
    for l in legends.iter().take(k) {
        p.notes_below.push(l.to_string());
    }
    if rng.chance(t * 0.5) {
        p.notes_gap = 0;
    }
    desc.push(format!("footnote marks on {:.0}% of numbers, {hm} headers, {k} legend lines (gap {})", t * 50.0, p.notes_gap));
}

fn locale(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 {
        return;
    }
    let loc = *rng.pick(LOCS);
    p.noise = CellNoise {
        locale: Some(loc),
        text_share: t,
        paren_neg: rng.chance(t),
        missing: t * 0.12,
        // dates follow the numbers' convention (an EU file has EU dates), so the locale is
        // decidable from the sheet
        date_text: if rng.chance(t) { Some(if loc == render::Loc::Ch { render::Loc::Eu } else { loc }) } else { None },
        ..p.noise.clone()
    };
    desc.push(format!(
        "numbers as {loc:?} text on {:.0}% of cells, paren negatives {}, missing {:.0}%, dates as {:?}",
        t * 100.0,
        p.noise.paren_neg,
        t * 12.0,
        p.noise.date_text
    ));
}

fn mixed_types(p: &mut Plan, rng: &mut Rng, t: f64, desc: &mut Vec<String>) {
    if t == 0.0 {
        return;
    }
    let ms = p.base.measures();
    let mut cols = ms.clone();
    rng.shuffle(&mut cols);
    let k = if t >= 0.5 { 2.min(cols.len()) } else { 1 };
    p.noise.only_cols = cols.into_iter().take(k).collect();
    p.noise.mixed = t * 0.3;
    p.noise.missing = t * 0.05;
    desc.push(format!("free text in {k} numeric columns ({:.0}% of cells)", t * 30.0));
}

// ---------------------------------------------------------------------------------------------
// Arrangements

fn side_by_side(cv: &mut Canvas, rng: &mut Rng, ctx: &Ctx, t: f64) {
    let n = 1 + (t * 3.0).round() as usize;
    let first = base_table(rng, ctx, Need { grouped: Some(false), ..ANY });
    let nrows = first.rows.len();
    let mut plans = vec![Plan::new(first)];
    let mut d = vec![format!("{n} tables side by side")];
    for _ in 1..n {
        let shared = rng.chance(t * 0.6);
        let mut b = base_table(rng, ctx, Need { grouped: Some(false), real_ok: !shared, rows: (nrows, nrows), date: false });
        let mut p;
        if shared {
            // repeats the first table's rows without its own labels
            let keys0 = plans[0].base.keys();
            let k0 = keys0[0];
            b.rows.truncate(nrows);
            for (i, r) in b.rows.iter_mut().enumerate() {
                r[0] = plans[0].base.rows[i][k0].clone();
            }
            b.cols[0].header = plans[0].base.cols[k0].header.clone();
            p = Plan::new(b);
            p.cols.retain(|c| !matches!(c, CI::Col(0)));
            p.merge_group = Some(0);
            p.title.clear();
            d.push("shares the first table's labels".into());
        } else {
            p = Plan::new(b);
            if rng.chance(0.5) {
                p.title.clear();
            }
        }
        plans.push(p);
    }
    if plans.iter().any(|p| p.merge_group.is_some()) {
        plans[0].merge_group = Some(0);
    }
    let top0 = rng.below(2) as u32;
    let mut left = 0u32;
    let max_title = plans.iter().map(|p| p.title.len()).max().unwrap_or(0);
    for (k, p) in plans.iter_mut().enumerate() {
        let shift = if t > 0.5 && p.merge_group.is_none() { rng.below(3) as u32 } else { 0 };
        // align header rows across tables: pad titles
        while p.title.len() < max_title {
            p.title.insert(0, String::new());
        }
        let width = p.cols.len() as u32;
        render::render(cv, rng, p, top0 + shift, left);
        let gap = if rng.chance(t * 0.4) { 0 } else { 1 + rng.below(2) as u32 };
        if k + 1 < n {
            d.push(format!("gap {gap}"));
        }
        left += width + gap;
    }
    cv.desc.push(d.join(", "));
}

const BLOCK_TITLES: &[&str] = &["2022", "2023", "Region North", "Region South", "Quarter 1", "Quarter 2", "Plan", "Actual", "Men", "Women"];

fn stacked(cv: &mut Canvas, rng: &mut Rng, ctx: &Ctx, t: f64) {
    let n = 1 + (t * 3.0).round() as usize;
    let same = rng.chance(0.5);
    let mut d = vec![format!("{n} tables stacked, {} schema", if same { "same" } else { "different" })];
    let mut titles: Vec<&str> = BLOCK_TITLES.to_vec();
    rng.shuffle(&mut titles);
    let mut plans = Vec::new();
    if same {
        let big = base_table(rng, ctx, Need { grouped: Some(false), rows: (8 * n, 14 * n), ..ANY });
        let per = big.rows.len() / n;
        let hdr_truth: Vec<Vec<String>> = big.cols.iter().map(|c| vec![c.header.clone()]).collect();
        for k in 0..n {
            let mut b = big.clone();
            b.rows = big.rows[k * per..if k + 1 == n { big.rows.len() } else { (k + 1) * per }].to_vec();
            let mut p = Plan::new(b);
            p.header_truth = hdr_truth.clone();
            p.merge_group = Some(0);
            p.title.clear();
            if rng.chance(0.8) || n == 1 {
                p.title.push(titles[k].to_string());
                if n > 1 {
                    p.extra_labels.push(titles[k].to_string());
                }
            }
            if k > 0 && rng.chance(t * 0.5) {
                p.show_header = false;
                d.push(format!("block {} without header", k + 1));
            }
            plans.push(p);
        }
    } else {
        for _ in 0..n {
            let mut p = Plan::new(base_table(rng, ctx, ANY));
            if rng.chance(0.3) {
                p.title.clear();
            }
            plans.push(p);
        }
    }
    let mut top = rng.below(2) as u32;
    for (k, p) in plans.iter().enumerate() {
        let bottom = render::render(cv, rng, p, top, 0);
        let gap = if rng.chance(t * 0.5) { 0 } else { 1 + rng.below(3) as u32 };
        if k + 1 < n {
            d.push(format!("gap {gap}"));
        }
        top = bottom + 1 + gap;
    }
    cv.desc.push(d.join(", "));
}

fn label_blocks(cv: &mut Canvas, rng: &mut Rng, t: f64) {
    let blocks = 1 + steps(t, 5);
    let (rows, grouped) = (rng.int(6, 16) as usize, rng.chance(0.5));
    let b0 = base::generate(rng, &GenSpec { rows, grouped, max_measures: 3, date_col: false });
    let ms = b0.measures();
    let year0 = 2015 + rng.below(6);
    let mut left = 0u32;
    let top = rng.below(2) as u32;
    let gap0 = rng.chance(t * 0.5);
    for k in 0..blocks {
        let mut b = b0.clone();
        for r in b.rows.iter_mut() {
            for &m in &ms {
                if let V::Num(x) = r[m] {
                    r[m] = V::Num(round(x * (0.7 + rng.f() * 0.6), render::decimals(b0.cols[m].kind)));
                }
            }
        }
        let mut p = Plan::new(b);
        let year = format!("{}", year0 + k);
        p.parents.push(Parent { level: 0, first: ms[0], last: *ms.last().unwrap(), text: year, style: PStyle::Merged });
        p.levels = 1;
        p.key_header = KHStyle::Bottom;
        recompute_header_truth(&mut p);
        p.may_split = true;
        p.merge_group = Some(0);
        if k > 0 {
            p.title = vec![String::new()];
        }
        let width = p.cols.len() as u32;
        render::render(cv, rng, &p, top, left);
        left += width + if gap0 { 0 } else { 1 };
    }
    cv.desc.push(format!("{blocks} year blocks side by side, labels repeated per block, gap {}", if gap0 { 0 } else { 1 }));
}

fn transposed(cv: &mut Canvas, rng: &mut Rng, t: f64) {
    let d = &base::DOMAINS[rng.below(base::DOMAINS.len())];
    let quarterly = rng.chance(0.5);
    let y0 = 2014 + rng.below(6) as i64;
    let periods: Vec<(String, String)> = if quarterly {
        let n = rng.int(2, 3);
        (0..n).flat_map(|y| (1..=4).map(move |q| (format!("{}", y0 + y), format!("Q{q}")))).collect()
    } else {
        (0..rng.int(5, 10)).map(|y| (format!("{}", y0 + y), String::new())).collect()
    };
    let nm = rng.int(2, d.measures.len() as i64) as usize;
    let measures: Vec<(&str, Kind)> = d.measures.iter().take(nm).copied().collect();
    let mags: Vec<f64> = measures.iter().map(|_| 10f64.powi(rng.int(1, 4) as i32) * (1.0 + rng.f() * 4.0)).collect();
    let vals: Vec<Vec<f64>> = periods
        .iter()
        .map(|_| {
            measures
                .iter()
                .zip(&mags)
                .map(|((_, k), mag)| round(mag * (0.2 + rng.f() * 1.8), render::decimals(*k)))
                .collect()
        })
        .collect();
    let across = rng.chance(t);
    let tid = cv.new_table(Default::default());
    let (top, left) = (rng.below(2) as u32, 0u32);
    cv.text(top, left, d.title, Some("title"));
    let r0 = top + 1;
    if !across {
        // periods down: Year | (Quarter) | measures
        let mut c = left;
        cv.text(r0, c, "Year", None);
        c += 1;
        if quarterly {
            cv.text(r0, c, "Quarter", None);
            c += 1;
        }
        for (m, (h, _)) in measures.iter().enumerate() {
            cv.text(r0, c + m as u32, h, None);
        }
        for (i, (y, q)) in periods.iter().enumerate() {
            let r = r0 + 1 + i as u32;
            cv.text(r, left, y, None);
            if quarterly {
                cv.text(r, left + 1, q, None);
            }
            for (m, (h, _)) in measures.iter().enumerate() {
                cv.put(r, c + m as u32, render::W::Num(vals[i][m]));
                let mut labels = vec![y.clone()];
                if quarterly {
                    labels.push(q.clone());
                }
                cv.data.push(render::DataT { r, c: c + m as u32, table: tid, exp: render::Expect::Num(vals[i][m]), scale: None, labels, header: vec![h.to_string()] });
            }
        }
        cv.desc.push(format!("periods down ({} {})", periods.len(), if quarterly { "quarters" } else { "years" }));
        return;
    }
    // periods across: Indicator | p1 | p2 ...; quarterly → a year row over the quarters
    let two = quarterly && (t >= 0.5 || rng.chance(0.5));
    let hr = if quarterly { 2 } else { 1 };
    cv.text(r0 + hr - 1, left, rng.pick(&["Indicator", "", "Measure", "Item"]), None);
    for (i, (y, q)) in periods.iter().enumerate() {
        let c = left + 1 + i as u32;
        if quarterly {
            if two {
                if q == "Q1" {
                    cv.text(r0, c, y, None);
                    cv.merges.push((r0, c, r0, c + 3));
                }
                cv.text(r0 + 1, c, q, None);
            } else {
                cv.text(r0 + 1, c, &format!("{q} {y}"), None);
            }
        } else {
            cv.text(r0, c, y, None);
        }
    }
    for (m, (h, _)) in measures.iter().enumerate() {
        let r = r0 + hr + m as u32;
        cv.text(r, left, h, None);
        for (i, (y, q)) in periods.iter().enumerate() {
            let c = left + 1 + i as u32;
            cv.put(r, c, render::W::Num(vals[i][m]));
            let mut labels = vec![y.clone()];
            if quarterly {
                labels.push(q.clone());
            }
            cv.data.push(render::DataT { r, c, table: tid, exp: render::Expect::Num(vals[i][m]), scale: None, labels, header: vec![h.to_string()] });
        }
    }
    cv.desc.push(format!(
        "periods across ({} {}{})",
        periods.len(),
        if quarterly { "quarters" } else { "years" },
        if two { ", year row over quarters" } else { "" }
    ));
}

const DESCR: &[&str] = &[
    "Office supplies",
    "Maintenance of heating system",
    "Cleaning services, building B",
    "IT support contract",
    "Printer toner cartridges",
    "Legal advice, case 2024/17",
    "Road resurfacing, section 4",
    "Catering for staff event",
    "Software licences (annual)",
    "Fuel for service vehicles",
    "Medical consumables",
    "Translation of tender documents",
];
const DESCR_TOTAL: &[&str] = &[
    "Total knee replacement implant",
    "Prótese total anca",
    "Ukupni troškovi održavanja",
    "Gesamtsanierung Grundschule",
    "Total station survey equipment",
    "Sum insured adjustment",
    "Summe der Leistungen lt. Vertrag",
    "Grand total of works, phase 2",
    "Subtotal adjustment for Q3 invoices",
    "Totalrenovierung Turnhalle",
    "Ukupno: 3 komada stolica",
];
const SUPPLIERS: &[&str] = &["Acme d.o.o.", "Nordbau GmbH", "Iberia Servicios SL", "Medisupply Ltd", "Tecnofix SpA", "Kowalski i Syn", "Bureau Plus SARL"];

fn long_register(cv: &mut Canvas, rng: &mut Rng, t: f64) {
    let n = 200 + steps(t, 18) * 100;
    let share = t * 0.35;
    let cols = vec![
        Col { header: "Contract no.".into(), kind: Kind::Key },
        Col { header: "Description".into(), kind: Kind::Key },
        Col { header: "Supplier".into(), kind: Kind::Key },
        Col { header: "Signed".into(), kind: Kind::Date },
        Col { header: "Quantity".into(), kind: Kind::Int },
        Col { header: "Amount".into(), kind: Kind::Money },
    ];
    let mut rows = Vec::with_capacity(n);
    let mut with_total = 0;
    for i in 0..n {
        let d = if rng.chance(share) {
            with_total += 1;
            rng.pick(DESCR_TOTAL).to_string()
        } else {
            rng.pick(DESCR).to_string()
        };
        rows.push(vec![
            V::Text(format!("K-24-{:05}", 100 + i * 7)),
            V::Text(d),
            V::Text(rng.pick(SUPPLIERS).to_string()),
            V::Date((45292 + rng.below(365)) as f64),
            V::Num(rng.int(1, 500) as f64),
            V::Num(round(50.0 + rng.f() * 90_000.0, 2)),
        ]);
    }
    let b = BaseTable { name: "gen:register".into(), title: "Contracts register 2024".into(), cols, rows, group: None, real: false };
    let mut p = Plan::new(b);
    let mut d = format!("{n}-row register, {with_total} descriptions with total words");
    if t >= 0.5 && rng.chance(0.5) {
        p.rows.push(RI::Total { label: "Total".into(), rows: (0..n).collect(), kind: TotKind::Sum });
        d.push_str(", real total row at the end");
    }
    render::render(cv, rng, &p, 0, 0);
    cv.desc.push(d);
}

const MONTHS: &[&str] = &["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn wide_periods(cv: &mut Canvas, rng: &mut Rng, t: f64) {
    let np = 6 + steps(t, 9) * 6; // 6..60
    let style = rng.below(if t >= 0.5 { 4 } else { 3 });
    let y0 = 1990 + rng.below(25) as i64;
    let keys = ["Country", "Indicator"];
    let nk = 1 + rng.below(2);
    let mut cols: Vec<Col> = keys.iter().take(nk).map(|k| Col { header: k.to_string(), kind: Kind::Key }).collect();
    let mut paths: Vec<Vec<String>> = cols.iter().map(|c| vec![c.header.clone()]).collect();
    let mut parents: Vec<(usize, usize, String)> = Vec::new();
    for k in 0..np {
        let (h, path) = match style {
            0 => (format!("{}", y0 + k as i64), vec![format!("{}", y0 + k as i64)]),
            1 => {
                let s = format!("{} {}", MONTHS[k % 12], 2010 + (k / 12) as i64);
                (s.clone(), vec![s])
            }
            2 => {
                let s = format!("{}-{:02}", 2010 + (k / 12) as i64, k % 12 + 1);
                (s.clone(), vec![s])
            }
            _ => {
                let y = format!("{}", 2010 + (k / 12) as i64);
                if k % 12 == 0 {
                    parents.push((nk + k, (nk + k + 11).min(nk + np - 1), y.clone()));
                }
                (MONTHS[k % 12].to_string(), vec![y, MONTHS[k % 12].to_string()])
            }
        };
        cols.push(Col { header: h, kind: Kind::Dec(1) });
        paths.push(path);
    }
    let nrows = rng.int(5, 20) as usize;
    let mut rows = Vec::new();
    for i in 0..nrows {
        let mut r = vec![V::Text(base::DOMAINS[0].keys[0].1.get(i).map_or(format!("Area {i}"), |s| s.to_string()))];
        if nk == 2 {
            r.push(V::Text(rng.pick(&["GDP growth", "Unemployment rate", "Inflation", "Exports share"]).to_string()));
        }
        let mag = 1.0 + rng.f() * 50.0;
        for _ in 0..np {
            r.push(V::Num(round(mag * (0.5 + rng.f()), 1)));
        }
        rows.push(r);
    }
    if nk == 1 {
        for (i, r) in rows.iter_mut().enumerate() {
            r[0] = V::Text(format!("{} {}", ["Alder", "Birch", "Cedar", "Dune", "Elm"][i % 5], i / 5 + 1));
        }
    }
    let b = BaseTable { name: "gen:wide".into(), title: "Indicators by period".into(), cols, rows, group: None, real: false };
    let mut p = Plan::new(b);
    p.header_truth = paths;
    if style == 3 {
        for (a, b, y) in parents {
            p.parents.push(Parent { level: 0, first: a, last: b, text: y, style: if rng.chance(0.7) { PStyle::Merged } else { PStyle::Once } });
        }
        p.levels = 1;
        p.key_header = *rng.pick(&[KHStyle::Bottom, KHStyle::Merged]);
    }
    let top = rng.below(2) as u32;
    render::render(cv, rng, &p, top, 0);
    cv.desc.push(format!(
        "{np} period columns as {}",
        ["bare years", "'Jan 2010'", "'2010-01'", "a year row over month names"][style]
    ));
}
