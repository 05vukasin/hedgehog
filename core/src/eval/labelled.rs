//! Generic ground truth: a directory with `labels.tsv` (header line, then
//! `corpus  file  sheet  row  col  role`, 0-based row/col, role names as in [`Role::as_str`]) and
//! the workbooks it names, relative to the directory (`../x/y.xlsx` reaches files kept elsewhere,
//! as the European sample does). Written by converters such as
//! `tools/benchmarks/convert_strudel.py` and `tools/benchmarks/build_eu_labels.py`.
//!
//! File ids are `<corpus>/<file>` (the corpus part groups per-source scores). No tables: such
//! corpora score cell roles only.

use std::collections::BTreeMap;
use std::path::Path;

use super::gt::{Corpus, GtCell, GtFile, GtSheet, LabelCount};
use super::EvalError;
use crate::analysis::Role;

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
    let mut corpus = Corpus { name: name.into(), has_tables: false, has_roles: true, labels, ..Corpus::default() };
    for ((id, file), sheets) in files {
        let sheets = sheets
            .into_iter()
            .map(|(name, mut cells)| {
                cells.sort_by_key(|c| (c.row, c.col));
                cells.dedup_by_key(|c| (c.row, c.col));
                GtSheet { name, index: None, tables: Vec::new(), cells }
            })
            .collect();
        corpus.files.push(GtFile { id, path: dir.join(&file), fallback_paths: Vec::new(), sheets });
    }
    if bad > 0 {
        corpus.notes.push(format!("{bad} malformed label lines skipped"));
    }
    Ok(corpus)
}
