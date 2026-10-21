//! Cross-corpus benchmark: one [`run::Report`] per corpus, scored per source (publisher or
//! sub-corpus: the part of the file id before the first `/`, else the corpus name) in two role
//! spaces:
//!
//! - **full**: the seven hedgehog roles (only for corpora labelled with all of them: DECO,
//!   Strudel, synthetic);
//! - **coarse**: title / header / data / other, where group headers count as headers, totals as
//!   data and notes as other — the label set every corpus shares (StatSheets has only these), so
//!   scores are comparable across corpora.
//!
//! Generalization is judged on the coarse scores of corpora a model was not trained on.
//!
//! Corpora with table rectangles (DECO, StatSheets, synthetic) also get table-detection
//! precision / recall / F1 per source, at every [`TABLE_CRITERIA`] threshold.

use std::collections::BTreeMap;

use super::metrics::{Confusion, FileMacro, Prf, N_ROLES, TABLE_CRITERIA};
use super::run::{Report, N_CRIT};
use crate::analysis::Role;

/// The shared four-role space.
pub fn coarse(r: Role) -> Role {
    match r {
        Role::GroupHeader => Role::Header,
        Role::Total => Role::Data,
        Role::Note => Role::Other,
        r => r,
    }
}

pub fn coarse_confusion(c: &Confusion) -> Confusion {
    let mut out = Confusion::default();
    for g in 0..N_ROLES {
        for p in 0..N_ROLES {
            let n = c.counts[g][p];
            if n > 0 {
                out.counts[super::metrics::role_index(coarse(Role::ALL[g]))]
                    [super::metrics::role_index(coarse(Role::ALL[p]))] += n;
            }
        }
    }
    out
}

/// Scores of one source.
#[derive(Debug, Clone, Default)]
pub struct SourceScore {
    pub source: String,
    pub files: usize,
    pub sheets: usize,
    pub cells: u64,
    /// Pooled macro F1 and file-macro F1 over the seven roles (None for coarse-only corpora).
    pub full: Option<(f64, f64)>,
    /// Pooled macro F1 and file-macro F1 in the coarse space.
    pub coarse: (f64, f64),
    pub accuracy: f64,
    /// Coarse per-role pooled F1 ([`Role::ALL`] order; None for roles outside the coarse space).
    pub coarse_per_role: [Option<f64>; N_ROLES],
    pub coarse_confusion: Confusion,
    /// Full per-role pooled F1 (None for coarse-only corpora or absent roles).
    pub full_per_role: [Option<f64>; N_ROLES],
    /// Table detection per [`TABLE_CRITERIA`] entry (None for corpora without table ranges).
    pub tables: Option<[Prf; N_CRIT]>,
    /// Header depth over matched tables: (scored, exact).
    pub header_depth: (usize, usize),
}

fn source_of(file_id: &str, corpus: &str) -> String {
    file_id.split_once('/').map_or(corpus.to_string(), |(s, _)| s.to_string())
}

struct Acc {
    files: usize,
    sheets: usize,
    full: Confusion,
    coarse: Confusion,
    full_fm: FileMacro,
    coarse_fm: FileMacro,
    tables: [Prf; N_CRIT],
    header: (usize, usize),
}

/// Per-source scores plus an `ALL` row (last), from a finished report.
pub fn by_source(report: &Report, coarse_only: bool) -> Vec<SourceScore> {
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    let mut all = Acc {
        files: 0,
        sheets: 0,
        full: Confusion::default(),
        coarse: Confusion::default(),
        full_fm: FileMacro::default(),
        coarse_fm: FileMacro::default(),
        tables: [Prf::default(); N_CRIT],
        header: (0, 0),
    };
    let mut i = 0;
    let sheets = &report.sheets;
    while i < sheets.len() {
        let file = &sheets[i].file;
        let src = source_of(file, &report.corpus);
        let a = acc.entry(src).or_insert_with(|| Acc {
            files: 0,
            sheets: 0,
            full: Confusion::default(),
            coarse: Confusion::default(),
            full_fm: FileMacro::default(),
            coarse_fm: FileMacro::default(),
            tables: [Prf::default(); N_CRIT],
            header: (0, 0),
        });
        a.files += 1;
        all.files += 1;
        while i < sheets.len() && &sheets[i].file == file {
            let c = &sheets[i].confusion;
            let cc = coarse_confusion(c);
            for x in [&mut *a, &mut all] {
                x.sheets += 1;
                x.full.merge(c);
                x.coarse.merge(&cc);
                x.full_fm.add_sheet(c);
                x.coarse_fm.add_sheet(&cc);
                for (k, &tp) in sheets[i].table_tp.iter().enumerate() {
                    x.tables[k].add(Prf::from_matches(tp, sheets[i].gt_tables, sheets[i].pred_tables));
                }
                x.header.0 += sheets[i].header_n;
                x.header.1 += sheets[i].header_ok;
            }
            i += 1;
        }
        for x in [&mut *a, &mut all] {
            x.full_fm.end_file();
            x.coarse_fm.end_file();
        }
    }
    let score = |source: String, a: &Acc| SourceScore {
        source,
        files: a.files,
        sheets: a.sheets,
        cells: a.full.total(),
        full: (!coarse_only).then(|| (a.full.macro_f1().unwrap_or(0.0), a.full_fm.macro_f1().unwrap_or(0.0))),
        coarse: (a.coarse.macro_f1().unwrap_or(0.0), a.coarse_fm.macro_f1().unwrap_or(0.0)),
        accuracy: a.full.correct() as f64 / a.full.total().max(1) as f64,
        coarse_per_role: a.coarse.class_f1(),
        coarse_confusion: a.coarse.clone(),
        full_per_role: if coarse_only { [None; N_ROLES] } else { a.full.class_f1() },
        tables: report.has_tables.then_some(a.tables),
        header_depth: a.header,
    };
    let mut out: Vec<SourceScore> = acc.iter().map(|(s, a)| score(s.clone(), a)).collect();
    out.push(score("ALL".into(), &all));
    out
}

/// Plain-text table of [`by_source`] rows (F1 in percent; `—` where a corpus has no full roles).
pub fn table(corpus: &str, analyzer: &str, rows: &[SourceScore]) -> String {
    let mut s = format!(
        "{corpus} · {analyzer}\n  {:<24} {:>5} {:>6} {:>9}  {:>11} {:>11}  {:>11} {:>11}  {:>6}\n",
        "source", "files", "sheets", "cells", "full pooled", "full file", "coarse pool", "coarse file", "acc"
    );
    for r in rows {
        let roles: Vec<String> = Role::ALL
            .iter()
            .zip(r.coarse_per_role)
            .filter_map(|(role, f)| f.map(|f| format!("{} {:.1}", role.as_str(), 100.0 * f)))
            .collect();
        let (fp, ff) = r.full.map_or(("—".into(), "—".into()), |(a, b)| (format!("{:.1}", 100.0 * a), format!("{:.1}", 100.0 * b)));
        s.push_str(&format!(
            "  {:<24} {:>5} {:>6} {:>9}  {:>11} {:>11}  {:>11.1} {:>11.1}  {:>6.1}\n",
            r.source,
            r.files,
            r.sheets,
            r.cells,
            fp,
            ff,
            100.0 * r.coarse.0,
            100.0 * r.coarse.1,
            100.0 * r.accuracy
        ));
        if r.source == "ALL" {
            s.push_str(&format!("  coarse F1 per role: {}\n", roles.join(" · ")));
            let full: Vec<String> = Role::ALL
                .iter()
                .zip(r.full_per_role)
                .filter_map(|(role, f)| f.map(|f| format!("{} {:.1}", role.as_str(), 100.0 * f)))
                .collect();
            if !full.is_empty() {
                s.push_str(&format!("  full F1 per role:   {}\n", full.join(" · ")));
            }
            if let Some(t) = &r.tables {
                s.push_str(&format!("  tables:             {}\n", tables_line(t)));
                let (hn, hok) = r.header_depth;
                if hn > 0 {
                    s.push_str(&format!(
                        "  header rows exact:  {:.1} ({hok} of {hn} tables matched at IoU>=0.5)\n",
                        100.0 * hok as f64 / hn as f64
                    ));
                }
            }
            s.push_str("  coarse confusion (rows = truth, cols = predicted; title header data other):\n");
            let keep = [Role::Title, Role::Header, Role::Data, Role::Other].map(super::metrics::role_index);
            for &g in &keep {
                let line: Vec<String> = keep.iter().map(|&p| format!("{:>9}", r.coarse_confusion.counts[g][p])).collect();
                s.push_str(&format!("    {:<7}{}\n", Role::ALL[g].as_str(), line.join("")));
            }
        }
    }
    if rows.len() > 2 && rows.iter().any(|r| r.tables.is_some()) {
        s.push_str("  table F1 per source (IoU>=0.5 / 0.8 / 1.0):\n");
        for r in rows.iter().filter(|r| r.source != "ALL") {
            if let Some(t) = &r.tables {
                let f = |k: usize| t[k].f1().map_or("—".into(), |f| format!("{:.1}", 100.0 * f));
                s.push_str(&format!("    {:<24} {:>5} {:>5} {:>5}\n", r.source, f(0), f(1), f(2)));
            }
        }
    }
    s
}

/// `gt N pred M · P/R/F1 IoU>=0.5 p/r/f · …` for one source's table counts.
pub fn tables_line(t: &[Prf; N_CRIT]) -> String {
    let (gt, pred) = (t[0].tp + t[0].fn_, t[0].tp + t[0].fp);
    let parts: Vec<String> = TABLE_CRITERIA
        .iter()
        .zip(t)
        .map(|(c, p)| {
            format!(
                "{} {:.1}/{:.1}/{}",
                c.label(),
                100.0 * p.precision(),
                100.0 * p.recall(),
                p.f1().map_or("—".into(), |f| format!("{:.1}", 100.0 * f))
            )
        })
        .collect();
    format!("gt {gt} pred {pred} · P/R/F1 {}", parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_space_merges_roles() {
        let mut c = Confusion::default();
        c.add(Role::Total, Role::Data); // right in coarse space
        c.add(Role::GroupHeader, Role::Header); // right in coarse space
        c.add(Role::Note, Role::Title); // wrong in both
        let cc = coarse_confusion(&c);
        assert_eq!((c.correct(), cc.correct(), cc.total()), (0, 2, 3));
    }
}
