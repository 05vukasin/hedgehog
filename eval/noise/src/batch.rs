//! Raw results as TSV, so a sweep can run as many small capped jobs (≤ 200 workbooks each) and
//! the report is built from all of them afterwards.
//!
//! Lines: `S family t seed ms_rules ms_model desc` · `E family t seed path message` ·
//! `I family t seed path invariant items failed first` · `M secs real_files`.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use crate::check::{Inv, INVARIANTS};
use crate::families::FAMILIES;
use crate::Outcome;

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

pub fn dump(rs: &[Outcome], ts: &[f64], secs: f64, real: &[String]) -> String {
    let mut s = String::new();
    writeln!(s, "M\t{secs}\t{}", real.join(",")).unwrap();
    for o in rs {
        let (f, t) = (FAMILIES[o.family], ts[o.ti]);
        writeln!(s, "S\t{f}\t{t}\t{}\t{}\t{}\t{}", o.seed, o.ms[0], o.ms[1], clean(&o.desc)).unwrap();
        for (p, r) in o.paths.iter().enumerate() {
            match r {
                Err(e) => writeln!(s, "E\t{f}\t{t}\t{}\t{p}\t{}", o.seed, clean(e)).unwrap(),
                Ok(m) => {
                    for (k, i) in m.iter().filter(|(_, i)| i.applicable) {
                        writeln!(s, "I\t{f}\t{t}\t{}\t{p}\t{k}\t{}\t{}\t{}", o.seed, i.items, i.failed, clean(&i.first.clone().unwrap_or_default()))
                            .unwrap();
                    }
                }
            }
        }
    }
    s
}

/// (outcomes, intensities, seeds per cell, summed seconds, real files) from every `*.tsv` in `dir`.
pub fn load(dir: &Path) -> (Vec<Outcome>, Vec<f64>, u64, f64, Vec<String>) {
    let mut secs = 0.0;
    let mut real: Vec<String> = Vec::new();
    // (family, t bits, seed) → outcome under construction
    let mut map: BTreeMap<(usize, u64, u64), (f64, Outcome)> = BTreeMap::new();
    let mut paths: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    let empty = || -> BTreeMap<&'static str, Inv> { INVARIANTS.iter().map(|(k, _)| (*k, Inv::default())).collect() };
    for p in paths.iter().filter(|p| p.extension().is_some_and(|e| e == "tsv")) {
        let text = std::fs::read_to_string(p).unwrap();
        for line in text.lines() {
            let c: Vec<&str> = line.split('\t').collect();
            if c[0] == "M" {
                secs += c[1].parse::<f64>().unwrap_or(0.0);
                for r in c.get(2).unwrap_or(&"").split(',').filter(|x| !x.is_empty()) {
                    if !real.iter().any(|x| x == r) {
                        real.push(r.to_string());
                    }
                }
                continue;
            }
            let fam = FAMILIES.iter().position(|f| *f == c[1]).expect("family");
            let t: f64 = c[2].parse().unwrap();
            let seed: u64 = c[3].parse().unwrap();
            let e = map.entry((fam, t.to_bits(), seed)).or_insert_with(|| {
                (t, Outcome { family: fam, ti: 0, seed, paths: [Ok(empty()), Ok(empty())], desc: String::new(), ms: [0.0; 2] })
            });
            match c[0] {
                "S" => {
                    e.1.ms = [c[4].parse().unwrap_or(0.0), c[5].parse().unwrap_or(0.0)];
                    e.1.desc = c.get(6).unwrap_or(&"").to_string();
                }
                "E" => {
                    let path: usize = c[4].parse().unwrap();
                    e.1.paths[path] = Err(c.get(5).unwrap_or(&"").to_string());
                }
                "I" => {
                    let path: usize = c[4].parse().unwrap();
                    if let Ok(m) = &mut e.1.paths[path] {
                        let k = INVARIANTS.iter().find(|(k, _)| *k == c[5]).expect("invariant").0;
                        let i = m.get_mut(k).unwrap();
                        i.applicable = true;
                        i.items = c[6].parse().unwrap();
                        i.failed = c[7].parse().unwrap();
                        let first = c.get(8).unwrap_or(&"").to_string();
                        i.first = if first.is_empty() { None } else { Some(first) };
                    }
                }
                _ => {}
            }
        }
    }
    let mut ts: Vec<f64> = map.values().map(|(t, _)| *t).collect();
    ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ts.dedup();
    let mut seeds_per: BTreeMap<(usize, u64), u64> = BTreeMap::new();
    let mut out: Vec<Outcome> = Vec::new();
    for ((f, tb, _), (t, mut o)) in map {
        o.ti = ts.iter().position(|x| *x == t).unwrap();
        *seeds_per.entry((f, tb)).or_default() += 1;
        out.push(o);
    }
    out.sort_by_key(|o| (o.family, o.ti, o.seed));
    let seeds = seeds_per.values().copied().max().unwrap_or(0);
    (out, ts, seeds, secs, real)
}
