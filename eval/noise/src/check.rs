//! Metamorphic invariants: the normalized output (in memory, `hh_core::normalize_workbook`)
//! against the truth the generator recorded.

use std::collections::{BTreeMap, BTreeSet};

use hh_core::normalize::{ColRole, Normalized, Out};

use crate::render::{Canvas, Expect, TotKind};

pub const INVARIANTS: &[(&str, &str)] = &[
    ("placed", "every data value is in exactly one table cell (not lost, not duplicated, not on Totals/Notes)"),
    ("value", "the value is typed and equal to the original (missing tokens → empty; scale applied or not)"),
    ("labels", "the row's key labels (row texts, group column, block title) stay with the value"),
    ("header", "the column name carries the header path (parents + child, normalized match)"),
    ("tables", "no output table mixes two source tables; a source table is not split (unless it is made of blocks)"),
    ("notes_out", "titles, notes, unit rows, separators, comments and group label cells are not table data"),
    ("totals_found", "every inserted sum total is on Totals"),
    ("totals_ok", "every inserted sum total that was found verifies (status ok)"),
    ("totals_out", "total, average and of-which rows are not table data"),
    ("nonsum", "average / of-which rows on Totals are not reported as mismatches"),
    ("scale", "a stated scale is applied to the values or kept in the column name / unit / title / notes"),
];

#[derive(Clone, Debug, Default)]
pub struct Inv {
    pub applicable: bool,
    pub items: usize,
    pub failed: usize,
    pub first: Option<String>,
}

impl Inv {
    fn add(&mut self, ok: bool, detail: impl FnOnce() -> String) {
        self.applicable = true;
        self.items += 1;
        if !ok {
            self.failed += 1;
            if self.first.is_none() {
                self.first = Some(detail());
            }
        }
    }
    pub fn pass(&self) -> bool {
        self.failed == 0
    }
}

const SUPERS: &[char] = &['¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹', '⁰'];

pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| !SUPERS.contains(c))
        .flat_map(|c| c.to_lowercase())
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn out_text(v: &Out) -> Option<String> {
    match v {
        Out::Null => None,
        Out::Num(x) => Some(crate::base::fmt_plain(*x)),
        Out::Date(d) => Some(hh_core::normalize::iso(*d)),
        Out::Bool(b) => Some(b.to_string()),
        Out::Text(s) => Some(s.clone()),
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * b.abs().max(1.0)
}

fn ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn show(v: &Out) -> String {
    match v {
        Out::Null => "null".into(),
        Out::Num(x) => format!("{x}"),
        Out::Date(d) => format!("date {}", hh_core::normalize::iso(*d)),
        Out::Bool(b) => format!("{b}"),
        Out::Text(s) => format!("text {s:?}"),
    }
}

pub fn check(cv: &Canvas, norm_out: &Normalized) -> BTreeMap<&'static str, Inv> {
    let mut res: BTreeMap<&'static str, Inv> = INVARIANTS.iter().map(|(k, _)| (*k, Inv::default())).collect();
    // where each source cell went
    let mut occ: BTreeMap<(u32, u32), Vec<(usize, usize, usize)>> = BTreeMap::new();
    for (ti, t) in norm_out.tables.iter().enumerate() {
        for (ci, col) in t.columns.iter().enumerate() {
            if col.role != ColRole::Data {
                continue;
            }
            let Some(c) = col.source_col else { continue };
            for (k, &r) in t.src_rows.iter().enumerate() {
                occ.entry((r, c)).or_default().push((ti, k, ci));
            }
        }
    }
    let mut tot: BTreeMap<(u32, u32), &'static str> = BTreeMap::new();
    for x in &norm_out.totals {
        for r in x.row..=x.row_end.max(x.row) {
            tot.insert((r, x.col), x.status);
        }
    }
    let notes: BTreeMap<(u32, u32), &'static str> = norm_out.notes.iter().map(|n| ((n.row, n.col), n.kind)).collect();
    let a1 = |r: u32, c: u32| hh_core::a1(r, c);

    // row text cache: (table, row) → normalized texts of the row + table title
    let row_texts = |ti: usize, k: usize| -> String {
        let t = &norm_out.tables[ti];
        let mut parts: Vec<String> = t.columns.iter().filter(|c| c.role != ColRole::SrcRow).filter_map(|c| out_text(&c.values[k])).map(|s| norm(&s)).collect();
        if let Some((title, _)) = &t.title {
            parts.push(norm(title));
        }
        parts.join("|")
    };

    let mut src_to_out: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    let mut out_to_src: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for d in &cv.data {
        let place = occ.get(&(d.r, d.c)).map_or(&[][..], |v| v.as_slice());
        let on_totals = tot.get(&(d.r, d.c));
        res.get_mut("placed").unwrap().add(place.len() == 1 && on_totals.is_none(), || {
            if let (1, Some(s)) = (place.len(), on_totals) {
                format!("{} {:?} is table data but also listed on Totals ({s})", a1(d.r, d.c), d.exp)
            } else if place.is_empty() {
                let wh = if let Some(s) = on_totals {
                    format!("on Totals ({s})")
                } else if let Some(k) = notes.get(&(d.r, d.c)) {
                    format!("on Notes as {k}")
                } else if let Some(x) = norm_out.totals.iter().find(|x| x.row <= d.r && d.r <= x.row_end.max(x.row) && x.direction == "vertical") {
                    format!("dropped: its row was taken as a total row ({} '{}')", x.level, x.label.clone().unwrap_or_default())
                } else if let Some(t) = norm_out.tables.iter().find(|t| {
                    t.header_top <= d.r && d.r < t.header_top + t.header_rows && t.rect.left <= d.c && d.c <= t.rect.right
                }) {
                    format!("taken as a header row of T{}", t.n)
                } else {
                    "nowhere".into()
                };
                format!("{} {:?} {wh}", a1(d.r, d.c), d.exp)
            } else {
                format!("{} in {} table cells", a1(d.r, d.c), place.len())
            }
        });
        let Some(&(ti, k, ci)) = place.first() else { continue };
        src_to_out.entry(d.table).or_default().insert(ti);
        out_to_src.entry(ti).or_default().insert(d.table);
        let t = &norm_out.tables[ti];
        let col = &t.columns[ci];
        let got = &col.values[k];
        let ok = match (&d.exp, got) {
            (Expect::Num(x), Out::Num(y)) => close(*y, *x) || d.scale.is_some_and(|f| close(*y, *x * f)),
            (Expect::Date(x), Out::Date(y)) => close(*y, *x),
            (Expect::Text(s), Out::Text(y)) => ws(s) == ws(y),
            (Expect::Text(s), v @ (Out::Num(_) | Out::Date(_))) => out_text(v).is_some_and(|y| norm(&y) == norm(s)),
            (Expect::Null, Out::Null) => true,
            (Expect::TextOrNull(_), Out::Null) => true,
            (Expect::TextOrNull(s), Out::Text(y)) => ws(s) == ws(y),
            _ => false,
        };
        res.get_mut("value").unwrap().add(ok, || format!("{} expected {:?}, got {} (column '{}' {})", a1(d.r, d.c), d.exp, show(got), col.name, col.dtype.as_str()));
        if !d.labels.is_empty() {
            let found = format!("{}|{}", row_texts(ti, k), norm(&col.name));
            let missing: Vec<&String> = d.labels.iter().filter(|l| !norm(l).is_empty() && !found.contains(&norm(l))).collect();
            res.get_mut("labels").unwrap().add(missing.is_empty(), || {
                format!("{} lost labels {:?} (row has: {})", a1(d.r, d.c), missing, found.replace('|', ", "))
            });
        }
        if !d.header.is_empty() {
            let found = format!("{}|{}", norm(&col.name), row_texts(ti, k));
            let missing: Vec<&String> = d.header.iter().filter(|h| !norm(h).is_empty() && !found.contains(&norm(h))).collect();
            res.get_mut("header").unwrap().add(missing.is_empty(), || {
                format!("{} column '{}' lacks {:?}", a1(d.r, d.c), col.name, missing)
            });
        }
    }
    // tables
    for (ti, srcs) in &out_to_src {
        let groups: BTreeSet<Option<usize>> = srcs.iter().map(|&s| cv.tables[s].merge_group.map(|g| g + 1000).or(Some(s))).collect();
        res.get_mut("tables").unwrap().add(groups.len() <= 1, || format!("T{} mixes {} source tables", norm_out.tables[*ti].n, srcs.len()));
    }
    for (s, outs) in &src_to_out {
        if cv.tables[*s].may_split {
            continue;
        }
        res.get_mut("tables").unwrap().add(outs.len() <= 1, || {
            let ids: Vec<String> = outs.iter().map(|&o| format!("T{}", norm_out.tables[o].n)).collect();
            format!("source table {} split into {}", s + 1, ids.join("+"))
        });
    }
    for n in &cv.nondata {
        let inside = occ.contains_key(&(n.r, n.c));
        res.get_mut("notes_out").unwrap().add(!inside, || format!("{} {} became table data", a1(n.r, n.c), n.what));
    }
    for x in &cv.totals {
        let in_data = occ.contains_key(&(x.r, x.c));
        let st = tot.get(&(x.r, x.c)).copied();
        match x.kind {
            TotKind::Sum => {
                res.get_mut("totals_found").unwrap().add(st.is_some(), || {
                    format!("{} total not on Totals{}", a1(x.r, x.c), if in_data { " (stayed in the data)" } else { "" })
                });
                if let Some(s) = st {
                    res.get_mut("totals_ok").unwrap().add(s == "ok", || {
                        let reason = norm_out.totals.iter().find(|t| t.row <= x.r && x.r <= t.row_end.max(t.row) && t.col == x.c).map(|t| t.reason.clone()).unwrap_or_default();
                        format!("{} total status {s} ({reason})", a1(x.r, x.c))
                    });
                }
                res.get_mut("totals_out").unwrap().add(!in_data, || format!("{} total row cell is table data", a1(x.r, x.c)));
            }
            TotKind::Average | TotKind::OfWhich => {
                let kind = if x.kind == TotKind::Average { "average" } else { "of-which" };
                res.get_mut("totals_out").unwrap().add(!in_data, || format!("{} {kind} row cell is table data", a1(x.r, x.c)));
                res.get_mut("nonsum").unwrap().add(!in_data && st != Some("mismatch"), || {
                    format!("{} {kind} row: {}", a1(x.r, x.c), if in_data { "summable table data".to_string() } else { format!("on Totals as {}", st.unwrap_or("-")) })
                });
            }
        }
    }
    // scale
    for (s, t) in cv.tables.iter().enumerate() {
        let Some((f, tokens)) = &t.scale else { continue };
        let cells: Vec<_> = cv.data.iter().filter(|d| d.table == s && matches!(d.exp, Expect::Num(_))).collect();
        let mut scaled = 0;
        let mut outs = BTreeSet::new();
        for d in &cells {
            if let Some(&(ti, k, ci)) = occ.get(&(d.r, d.c)).and_then(|v| v.first()) {
                outs.insert((ti, ci));
                if let (Expect::Num(x), Out::Num(y)) = (&d.exp, &norm_out.tables[ti].columns[ci].values[k]) {
                    if close(*y, x * f) && !close(*y, *x) {
                        scaled += 1;
                    }
                }
            }
        }
        let applied = scaled * 2 > cells.len();
        let mut words = String::new();
        for &(ti, ci) in &outs {
            let tb = &norm_out.tables[ti];
            let c = &tb.columns[ci];
            words.push_str(&norm(&c.name));
            words.push('|');
            if let Some(u) = &c.unit {
                words.push_str(&norm(u));
                words.push('|');
            }
            if let Some((tt, _)) = &tb.title {
                words.push_str(&norm(tt));
                words.push('|');
            }
            for (n, _) in &tb.notes {
                words.push_str(&norm(n));
                words.push('|');
            }
        }
        let kept = tokens.iter().any(|tk| words.contains(&norm(tk)));
        res.get_mut("scale").unwrap().add(applied || kept, || format!("scale ×{f} ({}) neither applied nor kept with the table", tokens.join("/")));
    }
    res
}
