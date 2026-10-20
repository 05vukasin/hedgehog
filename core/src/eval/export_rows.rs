//! Row-level training data (PLAN.md M6, phase 1): one CSV row per **row segment** — the filled
//! cells of one sheet row, split where more than [`MAX_GAP`] empty columns separate them — that
//! contains labelled cells.
//!
//! Features (`g_*` geometry, then for every cell feature of [`super::export::sheet_features`]):
//! `m_*` mean over the segment's cells, `f_*` the first (leftmost) cell, `u_*` / `d_*` mean over
//! the whole sheet row directly above / below (0 when empty).
//!
//! Labels: `label` = majority ground-truth role of the segment's labelled cells; `n_<role>` = how
//! many labelled cells have each role; `pairs` = `truth.rules.count` triples joined by `|` (role
//! ids, [`super::export::role_id`]) so cell-level scores of the rules and of any row prediction
//! are exact.

use std::collections::HashMap;
use std::io::Write;

use crate::analysis::Role;
use crate::grid::Grid;

use super::export::{role_id, sheet_features, ExportOptions, ExportStats};
use super::gt::{Corpus, GtFile};

/// Empty columns allowed inside one segment.
pub const MAX_GAP: u32 = 2;

const GEOMETRY: [&str; 9] = [
    "g_cells",
    "g_width",
    "g_left",
    "g_row",
    "g_gap_above",
    "g_gap_below",
    "g_share_of_row",
    "g_segment_index",
    "g_segments_in_row",
];

fn quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Splits one row's cell indices (sorted by column) into segments.
fn segments(cells: &[crate::model::Cell], row: &[u32]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for k in 1..=row.len() {
        if k == row.len() || cells[row[k] as usize].col > cells[row[k - 1] as usize].col + MAX_GAP + 1 {
            out.push(start..k);
            start = k;
        }
    }
    out
}

/// Writes one CSV row per labelled row segment of every file in the split.
pub fn export(corpus: &Corpus, opts: &ExportOptions, out: &mut impl Write, header: bool) -> std::io::Result<ExportStats> {
    let mut stats = ExportStats::default();
    let files: Vec<&GtFile> =
        corpus.files.iter().filter(|f| opts.split.contains(&f.id)).take(opts.limit.unwrap_or(usize::MAX)).collect();
    let mut wrote_header = !header;
    let mut names: Option<Vec<&'static str>> = None;
    for (k, file) in files.iter().enumerate() {
        if opts.progress && k % 50 == 0 {
            eprintln!("  {}/{} files, {} segments", k, files.len(), stats.rows);
        }
        let Some(wb) = std::iter::once(&file.path).chain(&file.fallback_paths).find_map(|p| crate::reader::open(p).ok())
        else {
            stats.failed += 1;
            continue;
        };
        stats.files += 1;
        let split = super::split::split_of(&file.id).as_str();
        for gs in &file.sheets {
            let Some((si, _)) = super::run::find_sheet(&wb, &gs.name, gs.index) else { continue };
            let sheet = &wb.sheets[si];
            let grid = Grid::new(sheet);
            let f = sheet_features(&grid);
            let nf = f.names().len();
            if !wrote_header {
                let mut line = String::from("source,file,sheet,row,left,right,split,label,n_labelled");
                for r in Role::ALL {
                    line.push_str(&format!(",n_{}", r.as_str()));
                }
                line.push_str(",pairs");
                for g in GEOMETRY {
                    line.push_str(&format!(",{g}"));
                }
                for p in ["m", "f", "u", "d"] {
                    for n in f.names() {
                        line.push_str(&format!(",{p}_{n}"));
                    }
                }
                writeln!(out, "{line}")?;
                wrote_header = true;
            }
            match &names {
                None => names = Some(f.names().to_vec()),
                Some(n) => assert_eq!(n.as_slice(), f.names(), "feature columns differ between sheets"),
            }
            stats.sheets += 1;
            let cols: Vec<&[f32]> = (0..nf).map(|i| f.column(i)).collect();
            let rules = f.get("l_role").expect("l_role");
            // Ground truth by cell index (a label inside a merged area goes to its anchor cell).
            let mut truth: HashMap<usize, Role> = HashMap::new();
            for gc in &gs.cells {
                let idx = grid.cell_index(gc.row, gc.col).or_else(|| {
                    let m = grid.merge_at(gc.row, gc.col)?;
                    grid.cell_index(m.top, m.left)
                });
                if let Some(i) = idx {
                    truth.entry(i).or_insert(gc.role);
                }
            }
            // Mean features of every non-empty sheet row.
            let mut row_mean: HashMap<u32, Vec<f32>> = HashMap::new();
            let rows_with_cells: Vec<u32> = {
                let mut r: Vec<u32> = sheet.cells.iter().map(|c| c.row).collect();
                r.sort_unstable();
                r.dedup();
                r
            };
            for &r in &rows_with_cells {
                let idx = grid.row_cells(r);
                let mut m = vec![0f32; nf];
                for &i in idx {
                    for (k, c) in cols.iter().enumerate() {
                        m[k] += c[i as usize];
                    }
                }
                let n = idx.len().max(1) as f32;
                m.iter_mut().for_each(|x| *x /= n);
                row_mean.insert(r, m);
            }
            let zeros = vec![0f32; nf];
            let prefix = format!("{},{},{}", quote(&opts.source), quote(&file.id), quote(&gs.name));
            for (ri, &r) in rows_with_cells.iter().enumerate() {
                let idx = grid.row_cells(r);
                let segs = segments(&sheet.cells, idx);
                let gap_above = if ri > 0 { (r - rows_with_cells[ri - 1]).min(50) } else { 50 };
                let gap_below = rows_with_cells.get(ri + 1).map_or(50, |&n| (n - r).min(50));
                for (si_, seg) in segs.iter().enumerate() {
                    let members = &idx[seg.clone()];
                    let mut counts = [0u32; Role::ALL.len()];
                    let mut pairs: HashMap<(i32, i32), u32> = HashMap::new();
                    for &i in members {
                        if let Some(&role) = truth.get(&(i as usize)) {
                            counts[role_id(role) as usize] += 1;
                            *pairs.entry((role_id(role), rules[i as usize] as i32)).or_default() += 1;
                        }
                    }
                    let labelled: u32 = counts.iter().sum();
                    if labelled == 0 {
                        continue;
                    }
                    let label = (0..counts.len()).max_by_key(|&k| (counts[k], std::cmp::Reverse(k))).unwrap();
                    let first = members[0] as usize;
                    let left = sheet.cells[first].col;
                    let right = sheet.cells[*members.last().unwrap() as usize].col;
                    let mut line = String::with_capacity(8 * (9 + 4 * nf));
                    use std::fmt::Write as _;
                    let _ = write!(line, "{prefix},{r},{left},{right},{split},{label},{labelled}");
                    for c in counts {
                        let _ = write!(line, ",{c}");
                    }
                    let mut p: Vec<_> = pairs.into_iter().collect();
                    p.sort_unstable();
                    let ps: Vec<String> = p.iter().map(|((g, q), n)| format!("{g}.{q}.{n}")).collect();
                    let _ = write!(line, ",{}", ps.join("|"));
                    let geom = [
                        members.len() as f32,
                        (right - left + 1) as f32,
                        left as f32,
                        r as f32,
                        gap_above as f32,
                        gap_below as f32,
                        members.len() as f32 / idx.len() as f32,
                        si_ as f32,
                        segs.len() as f32,
                    ];
                    for g in geom {
                        let _ = write!(line, ",{g}");
                    }
                    for c in &cols {
                        let s: f32 = members.iter().map(|&i| c[i as usize]).sum();
                        let _ = write!(line, ",{}", s / members.len() as f32);
                    }
                    for c in &cols {
                        let _ = write!(line, ",{}", c[first]);
                    }
                    let up = r.checked_sub(1).and_then(|u| row_mean.get(&u)).unwrap_or(&zeros);
                    let down = row_mean.get(&(r + 1)).unwrap_or(&zeros);
                    for v in up.iter().chain(down) {
                        let _ = write!(line, ",{v}");
                    }
                    writeln!(out, "{line}")?;
                    stats.rows += 1;
                    stats.per_role[label] += 1;
                }
            }
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, Value};

    #[test]
    fn segments_split_on_wide_gaps() {
        let cells: Vec<Cell> = [0u32, 1, 3, 7, 8]
            .iter()
            .map(|&c| Cell { row: 0, col: c, value: Some(Value::Number(1.0)), formula: None })
            .collect();
        let idx: Vec<u32> = (0..5).collect();
        assert_eq!(segments(&cells, &idx), vec![0..3, 3..5]);
    }
}
