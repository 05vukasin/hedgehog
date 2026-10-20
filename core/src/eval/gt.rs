//! Corpus-independent ground truth. Every loader (DECO now; TableSense, Strudel SAUS/CIUS/DeEx,
//! StatSheets and synthetic data later) converts its annotations into a [`Corpus`].
//!
//! Conventions: coordinates are 0-based sheet coordinates like [`crate::model`]; a labelled merged
//! cell is recorded once, at its top-left (the position that holds the value). A corpus that only
//! has tables (TableSense) leaves `cells` empty and sets `has_roles = false`; one with only line
//! labels (Strudel) expands them to the non-empty cells of the line and sets `has_tables = false`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::analysis::Role;
use crate::model::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GtCell {
    pub row: u32,
    pub col: u32,
    pub role: Role,
}

#[derive(Debug, Clone, Default)]
pub struct GtSheet {
    pub name: String,
    /// 0-based sheet position, if the corpus records it; used only when the name doesn't match.
    pub index: Option<usize>,
    pub tables: Vec<Rect>,
    /// Labelled cells, sorted row-major, one per position.
    pub cells: Vec<GtCell>,
}

#[derive(Debug, Clone)]
pub struct GtFile {
    /// Stable id (file name relative to the corpus); the split hashes it.
    pub id: String,
    pub path: PathBuf,
    /// Other copies to try if `path` fails to open (e.g. DECO's annotated copy of an Enron file).
    pub fallback_paths: Vec<PathBuf>,
    pub sheets: Vec<GtSheet>,
}

/// An annotated file that could not be mapped to a spreadsheet on disk.
#[derive(Debug, Clone)]
pub struct Unmatched {
    pub id: String,
    pub reason: String,
}

/// How often a source label occurred and what it maps to. `role == None` means unmapped
/// (scored as [`Role::Other`]).
#[derive(Debug, Clone, Default)]
pub struct LabelCount {
    pub role: Option<Role>,
    pub count: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Corpus {
    pub name: String,
    /// Files whose spreadsheet was found, sorted by id.
    pub files: Vec<GtFile>,
    pub unmatched: Vec<Unmatched>,
    /// Source label → mapping + count (cell-level labels).
    pub labels: BTreeMap<String, LabelCount>,
    pub has_tables: bool,
    pub has_roles: bool,
    /// Labels use only title / header / data / other (StatSheets): score in the coarse role space
    /// (see [`super::benchmark::coarse`]).
    pub coarse_roles: bool,
    /// Free-form loader notes (fallback paths used, duplicate labels, ...), shown in the report.
    pub notes: Vec<String>,
}

impl Corpus {
    pub fn sheet_count(&self) -> usize {
        self.files.iter().map(|f| f.sheets.len()).sum()
    }

    pub fn unmapped_count(&self) -> u64 {
        self.labels.values().filter(|l| l.role.is_none()).map(|l| l.count).sum()
    }
}
