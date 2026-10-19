//! DECO loader (Koci et al., ICDAR 2019): 1,165 Enron xlsx files annotated with tables and
//! cell roles.
//!
//! Inputs under `<data>/irregular/`:
//! - `deco/annotations/rangeAnnotations.csv`: `FileName,SheetName,SheetIndex,AnnotationName,
//!   AnnotationLabel,AnnotationParent,AnnotationAddress,FirstColumn,FirstRow,LastColumn,LastRow`.
//!   Rows labelled `Table` give the table rectangles (0-based, inclusive).
//! - `deco/annotations/cellAnnotations.csv`: `FileName,SheetName,SheetIndex,TableName,
//!   AnnotationLabel,AnnotationAddress,FirstColumn,FirstRow,LastColumn,LastRow`, one row per
//!   labelled non-empty cell (a merged cell is one row spanning its area → we keep its top-left).
//! - Spreadsheets: `enron/<FileName>` (the original Enron file, same name). Fallback:
//!   `deco/completed/<FileName>` — DECO's annotated copy, which has extra annotation sheets but
//!   identical content on the annotated sheets. Also tried when the Enron original fails to
//!   open (DECO re-saved its copies, which sidesteps calamine's `r:id=""` failure).
//!
//! Label mapping (DECO → [`Role`]), see [`map_label`]:
//! Data→Data, Header→Header, Derived→Total, GroupHead→GroupHeader, MetaTitle→Title,
//! Notes→Note, Other→Other. Anything else is counted as unmapped and scored as Other.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use super::csv::CsvReader;
use super::gt::{Corpus, GtCell, GtFile, GtSheet, LabelCount, Unmatched};
use super::EvalError;
use crate::analysis::Role;
use crate::model::Rect;

/// DECO cell label → harmonized role; `None` for labels outside the DECO set.
pub fn map_label(label: &str) -> Option<Role> {
    Some(match label {
        "Data" => Role::Data,
        "Header" => Role::Header,
        "Derived" => Role::Total,
        "GroupHead" => Role::GroupHeader,
        "MetaTitle" => Role::Title,
        "Notes" => Role::Note,
        "Other" => Role::Other,
        _ => return None,
    })
}

/// Hidden sheet in which the DECO tool stores its annotations; never ground truth.
pub const ANNOTATION_SHEET: &str = "Range_Annotations_Data";

type Sheets = BTreeMap<String, BTreeMap<String, GtSheet>>;

struct Columns {
    file: usize,
    sheet: usize,
    sheet_index: usize,
    label: usize,
    first_col: usize,
    first_row: usize,
    last_col: usize,
    last_row: usize,
}

impl Columns {
    fn from_header(h: &[String], path: &Path) -> Result<Columns, EvalError> {
        let find = |name: &str| {
            h.iter()
                .position(|x| x.trim_start_matches('\u{feff}') == name)
                .ok_or_else(|| EvalError(format!("{}: missing column `{name}`", path.display())))
        };
        Ok(Columns {
            file: find("FileName")?,
            sheet: find("SheetName")?,
            sheet_index: find("SheetIndex")?,
            label: find("AnnotationLabel")?,
            first_col: find("FirstColumn")?,
            first_row: find("FirstRow")?,
            last_col: find("LastColumn")?,
            last_row: find("LastRow")?,
        })
    }

    fn rect(&self, rec: &[String]) -> Option<Rect> {
        let n = |i: usize| rec.get(i)?.trim().parse::<u32>().ok();
        let (top, left, bottom, right) =
            (n(self.first_row)?, n(self.first_col)?, n(self.last_row)?, n(self.last_col)?);
        (top <= bottom && left <= right).then_some(Rect { top, left, bottom, right })
    }
}

/// Streams one annotation CSV, calling `f(columns, record, sheet)` for each well-formed row.
fn read_csv(
    path: &Path,
    sheets: &mut Sheets,
    bad_rows: &mut u64,
    mut f: impl FnMut(&Columns, &[String], &mut GtSheet),
) -> Result<(), EvalError> {
    let io = |e: std::io::Error| EvalError(format!("{}: {e}", path.display()));
    let mut r = CsvReader::new(BufReader::with_capacity(1 << 20, File::open(path).map_err(io)?));
    let mut rec = Vec::new();
    if !r.next_record(&mut rec).map_err(io)? {
        return Err(EvalError(format!("{}: empty file", path.display())));
    }
    let cols = Columns::from_header(&rec, path)?;
    let width = rec.len();
    while r.next_record(&mut rec).map_err(io)? {
        if rec.len() != width {
            *bad_rows += 1;
            continue;
        }
        let sheet = sheets
            .entry(rec[cols.file].clone())
            .or_default()
            .entry(rec[cols.sheet].clone())
            .or_insert_with(|| GtSheet {
                name: rec[cols.sheet].clone(),
                index: rec[cols.sheet_index].trim().parse::<usize>().ok().and_then(|i| i.checked_sub(1)),
                ..GtSheet::default()
            });
        f(&cols, &rec, sheet);
    }
    Ok(())
}

/// Loads DECO from `<data_root>/irregular/`. Only ground truth is held in memory; workbooks are
/// opened later, one at a time, by the runner.
pub fn load(data_root: &Path) -> Result<Corpus, EvalError> {
    let base = data_root.join("irregular");
    let ann = base.join("deco").join("annotations");
    let mut sheets: Sheets = BTreeMap::new();
    let mut labels: BTreeMap<String, LabelCount> = BTreeMap::new();
    let mut bad_rows = 0u64;
    let mut bad_rects = 0u64;

    read_csv(&ann.join("rangeAnnotations.csv"), &mut sheets, &mut bad_rows, |cols, rec, sheet| {
        if rec[cols.label] == "Table" {
            match cols.rect(rec) {
                Some(r) => sheet.tables.push(r),
                None => bad_rects += 1,
            }
        }
    })?;
    read_csv(&ann.join("cellAnnotations.csv"), &mut sheets, &mut bad_rows, |cols, rec, sheet| {
        let label = &rec[cols.label];
        let role = map_label(label);
        match labels.get_mut(label.as_str()) {
            Some(l) => l.count += 1,
            None => {
                labels.insert(label.clone(), LabelCount { role, count: 1 });
            }
        }
        match cols.rect(rec) {
            Some(r) => sheet.cells.push(GtCell { row: r.top, col: r.left, role: role.unwrap_or(Role::Other) }),
            None => bad_rects += 1,
        }
    })?;

    let mut corpus = Corpus { name: "deco".into(), has_tables: true, has_roles: true, labels, ..Corpus::default() };
    let mut duplicates = 0u64;
    let mut conflicts = 0u64;
    let mut fallback = 0usize;
    let mut tool_sheets = 0usize;
    for (id, by_name) in sheets {
        let mut file_sheets: Vec<GtSheet> = by_name.into_values().collect();
        // DECO's own bookkeeping sheet, present only in its annotated copies (one stray row).
        let before = file_sheets.len();
        file_sheets.retain(|s| s.name != ANNOTATION_SHEET);
        tool_sheets += before - file_sheets.len();
        if file_sheets.is_empty() {
            continue;
        }
        for s in &mut file_sheets {
            s.cells.sort_by_key(|c| (c.row, c.col));
            let before = s.cells.len();
            s.cells.dedup_by(|b, a| {
                let same = (a.row, a.col) == (b.row, b.col);
                if same && a.role != b.role {
                    conflicts += 1;
                }
                same
            });
            duplicates += (before - s.cells.len()) as u64;
            s.tables.sort_by_key(|r| (r.top, r.left, r.bottom, r.right));
            s.tables.dedup();
        }
        file_sheets.sort_by_key(|s| (s.index.unwrap_or(usize::MAX), s.name.clone()));

        let enron = base.join("enron").join(&id);
        let completed = base.join("deco").join("completed").join(&id);
        let (path, fallback_paths) = if enron.is_file() {
            let alt = if completed.is_file() { vec![completed] } else { vec![] };
            (enron, alt)
        } else if completed.is_file() {
            fallback += 1;
            (completed, vec![])
        } else {
            corpus.unmatched.push(Unmatched {
                id,
                reason: "no file with this name in enron/ or deco/completed/".into(),
            });
            continue;
        };
        corpus.files.push(GtFile { id, path, fallback_paths, sheets: file_sheets });
    }

    if bad_rows > 0 {
        corpus.notes.push(format!("{bad_rows} annotation rows with wrong field count skipped"));
    }
    if bad_rects > 0 {
        corpus.notes.push(format!("{bad_rects} annotation rows with unparsable coordinates skipped"));
    }
    if duplicates > 0 {
        corpus.notes.push(format!("{duplicates} duplicate cell labels dropped ({conflicts} with a different label; first kept)"));
    }
    if tool_sheets > 0 {
        corpus.notes.push(format!("{tool_sheets} annotated `{ANNOTATION_SHEET}` sheet(s) (DECO bookkeeping) ignored"));
    }
    if fallback > 0 {
        corpus.notes.push(format!("{fallback} files read from deco/completed/ (not found in enron/)"));
    }
    Ok(corpus)
}
