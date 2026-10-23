//! Generic ground truth: a directory with `labels.tsv` (header line, then
//! `corpus  file  sheet  row  col  role`, 0-based row/col, role names as in [`Role::as_str`]) and
//! the workbooks it names, relative to the directory (`../x/y.xlsx` reaches files kept elsewhere,
//! as the European sample does). Written by converters such as
//! `tools/benchmarks/convert_strudel.py` and `tools/benchmarks/build_eu_labels.py`.
//!
//! File ids are `<corpus>/<file>` (the corpus part groups per-source scores). Without tables such
//! corpora score cell roles only; an optional `tables.tsv` beside it (header line, then
//! `corpus  file  sheet  top  left  bottom  right`, 0-based inclusive) adds table rectangles, so table
//! detection and header depth are scored too (written by `eval/public/convert_hitab.py`).

use std::collections::BTreeMap;
use std::path::Path;

use super::gt::{Corpus, GtCell, GtFile, GtSheet, LabelCount};
use super::EvalError;
use crate::analysis::Role;
use crate::model::Rect;

pub fn role_named(s: &str) -> Option<Role> {
    Role::ALL.iter().copied().find(|r| r.as_str() == s)
}

/// Loads `<dir>/labels.tsv`; `name` becomes the corpus name.
pub fn load(dir: &Path, name: &str) -> Result<Corpus, EvalError> {
    let path = dir.join("labels.tsv");
    let text = std::fs::read_to_string(&path).map_err(|e| EvalError(format!("{}: {e}", path.display())))?;
    // (id, file) → sheet name → cells
    let mut files: BTreeMap<(String, String), BTreeMap<String, Vec<GtCell>>> = BTreeMap::new();
    let mut labels: BTreeMap<String, LabelCount> = BTreeMap::new();
    let mut bad = 0u64;
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let (Some(corpus), Some(file), Some(sheet), Some(row), Some(col), Some(role)) =
            (f.first(), f.get(1), f.get(2), f.get(3), f.get(4), f.get(5))
        else {
            bad += 1;
            continue;
        };
        let (Ok(row), Ok(col)) = (row.parse::<u32>(), col.parse::<u32>()) else {
            bad += 1;
            continue;
        };
        let mapped = role_named(role);
        let lc = labels.entry(role.to_string()).or_insert(LabelCount { role: mapped, count: 0 });
        lc.count += 1;
        files
            .entry((format!("{corpus}/{file}"), file.to_string()))
            .or_default()
            .entry(sheet.to_string())
            .or_default()
            .push(GtCell { row, col, role: mapped.unwrap_or(Role::Other) });
    }
    // (id, sheet) → table rectangles, from the optional tables.tsv
    let mut rects: BTreeMap<(String, String), Vec<Rect>> = BTreeMap::new();
    let tpath = dir.join("tables.tsv");
    let has_tables = tpath.exists();
    if has_tables {
        let text = std::fs::read_to_string(&tpath).map_err(|e| EvalError(format!("{}: {e}", tpath.display())))?;
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split('\t').collect();
            let n: Vec<u32> = f.iter().skip(3).take(4).filter_map(|x| x.parse().ok()).collect();
            if f.len() < 7 || n.len() != 4 {
                bad += 1;
                continue;
            }
            let id = format!("{}/{}", f[0], f[1]);
            files.entry((id.clone(), f[1].to_string())).or_default().entry(f[2].to_string()).or_default();
            rects.entry((id, f[2].to_string())).or_default().push(Rect { top: n[0], left: n[1], bottom: n[2], right: n[3] });
        }
    }
    // A tables-only corpus (TableSense) has a labels.tsv with just its header line.
    let has_roles = !labels.is_empty();
    let mut corpus = Corpus { name: name.into(), has_tables, has_roles, labels, ..Corpus::default() };
    for ((id, file), sheets) in files {
        let sheets = sheets
            .into_iter()
            .map(|(name, mut cells)| {
                cells.sort_by_key(|c| (c.row, c.col));
                cells.dedup_by_key(|c| (c.row, c.col));
                let tables = rects.remove(&(id.clone(), name.clone())).unwrap_or_default();
                GtSheet { name, index: None, tables, cells }
            })
            .collect();
        corpus.files.push(GtFile { id, path: dir.join(&file), fallback_paths: Vec::new(), sheets });
    }
    if bad > 0 {
        corpus.notes.push(format!("{bad} malformed label lines skipped"));
    }
    Ok(corpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_tsv_adds_rectangles() {
        let dir = std::env::temp_dir().join(format!("hh_labelled_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("labels.tsv"), "corpus\tfile\tsheet\trow\tcol\trole\nc\ta.xlsx\tS\t0\t0\theader\nc\ta.xlsx\tS\t1\t0\tdata\n").unwrap();
        let c = load(&dir, "x").unwrap();
        assert!(!c.has_tables && c.has_roles);
        std::fs::write(dir.join("tables.tsv"), "corpus\tfile\tsheet\ttop\tleft\tbottom\tright\nc\ta.xlsx\tS\t0\t0\t1\t2\nc\tb.xlsx\tT\t3\t1\t9\t4\n").unwrap();
        let c = load(&dir, "x").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(c.has_tables);
        assert_eq!(c.files.len(), 2);
        assert_eq!(c.files[0].sheets[0].tables, vec![Rect { top: 0, left: 0, bottom: 1, right: 2 }]);
        assert_eq!(c.files[0].sheets[0].cells.len(), 2);
        assert_eq!(c.files[1].sheets[0].tables, vec![Rect { top: 3, left: 1, bottom: 9, right: 4 }]);
    }
}
