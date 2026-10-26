//! Row-level training data: one CSV row per **row segment** — the filled
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

pub use crate::learned::MAX_GAP;

fn quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Writes one CSV row per labelled row segment of every file in the split. The features are
/// exactly what [`crate::learned::segment_features`] computes at prediction time.
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
        let Some(wb) = std::iter::once(&file.path).chain(&file.fallback_paths).find_map(|p| crate::reader::open_with(p, crate::reader::ReadOptions { formats: true, ..Default::default() }).ok())
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
            let (segs, sf) = crate::learned::segment_features(&grid, &f);
            if !wrote_header {
                let mut line = String::from("source,file,sheet,row,left,right,split,label,n_labelled");
                for r in Role::ALL {
                    line.push_str(&format!(",n_{}", r.as_str()));
                }
                line.push_str(",pairs");
                for n in sf.names() {
                    line.push(',');
                    line.push_str(n);
                }
                writeln!(out, "{line}")?;
                wrote_header = true;
            }
            match &names {
                None => names = Some(sf.names().to_vec()),
                Some(n) => assert_eq!(n.as_slice(), sf.names(), "feature columns differ between sheets"),
            }
            stats.sheets += 1;
            let cols: Vec<&[f32]> = (0..sf.names().len()).map(|i| sf.column(i)).collect();
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
            let prefix = format!("{},{},{}", quote(&opts.source), quote(&file.id), quote(&gs.name));
            for (s, seg) in segs.iter().enumerate() {
                let mut counts = [0u32; Role::ALL.len()];
                let mut pairs: HashMap<(i32, i32), u32> = HashMap::new();
                for &i in &seg.members {
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
                use std::fmt::Write as _;
                let mut line = String::with_capacity(8 * cols.len());
                let _ = write!(line, "{prefix},{},{},{},{split},{label},{labelled}", seg.row, seg.left, seg.right);
                for c in counts {
                    let _ = write!(line, ",{c}");
                }
                let mut p: Vec<_> = pairs.into_iter().collect();
                p.sort_unstable();
                let ps: Vec<String> = p.iter().map(|((g, q), n)| format!("{g}.{q}.{n}")).collect();
                let _ = write!(line, ",{}", ps.join("|"));
                for c in &cols {
                    let _ = write!(line, ",{}", c[s]);
                }
                writeln!(out, "{line}")?;
                stats.rows += 1;
                stats.per_role[label] += 1;
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
        let sheet = crate::model::Sheet { name: "s".into(), cells, merges: vec![], formats: vec![] };
        let grid = Grid::new(&sheet);
        let f = sheet_features(&grid);
        let (segs, sf) = crate::learned::segment_features(&grid, &f);
        let spans: Vec<(u32, u32)> = segs.iter().map(|s| (s.left, s.right)).collect();
        assert_eq!(spans, vec![(0, 3), (7, 8)]);
        assert_eq!(sf.rows(), 2);
        assert_eq!(sf.get("g_cells"), Some(&[3.0, 2.0][..]));
        assert_eq!(sf.get("g_segments_in_row"), Some(&[2.0, 2.0][..]));
    }
}
