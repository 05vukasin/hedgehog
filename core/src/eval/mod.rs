//! Scoring harness: ground truth from annotated corpora, metrics, and a runner that scores any
//! analyzer (`Fn(&Grid) -> SheetLayout`) against it. Owned by the `eval` workstream (PLAN.md M1).
//!
//! Pieces:
//! - [`gt`]: one corpus-independent ground-truth schema (files → sheets → table rects + cell roles).
//! - [`deco`]: DECO loader (Enron sheets, Koci et al. 2019). More loaders (TableSense, Strudel,
//!   StatSheets, synthetic) only need to produce a [`gt::Corpus`].
//!   Already on disk under `testdata/irregular/` for future loaders: `tablesense_annotations/
//!   Table range annotations.txt` (tables only; sources in `venron_spreadcluster/`),
//!   `strudel/{saus,cius,deex,troy}.jl.gz` (line+cell roles), `statsheets/dataset/` (manifest.csv,
//!   spreadsheets/, per-sheet `.npz` annotations: cell types + tables), `mondrian/res/{deco,fuste}`.
//! - [`split`]: deterministic dev/test split by file id hash (70/30).
//! - [`metrics`]: table detection P/R/F1 at IoU and EoB thresholds, cell-role confusion,
//!   per-class F1, and file-macro F1 (StatSheets-style).
//! - [`baseline`]: 8-connected flood fill, the number to beat.
//! - [`run`]: streams files one at a time, runs the analyzer, aggregates a [`run::Report`]
//!   (scoreboard text + JSON with per-sheet rows).
//!
//! Plugging in a detector:
//! ```ignore
//! let corpus = hh_core::eval::deco::load(&hh_core::eval::data_root())?;
//! let opts = hh_core::eval::run::RunOptions::default();
//! let report = hh_core::eval::run::evaluate(&corpus, &|g: &Grid| layout::analyze(g), "layout", &opts);
//! println!("{}", report.scoreboard());
//! ```

/// Read cell formats when evaluation opens a file (see [`set_read_formats`]).
static READ_FORMATS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Makes the evaluation harness read cell formats (for a model that is not the embedded one but
/// reads `s_*` features). The embedded model's needs are covered by [`crate::reader::ReadOptions`].
pub fn set_read_formats(on: bool) {
    READ_FORMATS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Opens a corpus file for scoring: [`crate::reader::ReadOptions::default`], plus formats when
/// [`set_read_formats`] asked for them.
pub fn open(path: &std::path::Path) -> Result<crate::model::Workbook, crate::reader::Error> {
    let mut o = crate::reader::ReadOptions::default();
    o.formats |= READ_FORMATS.load(std::sync::atomic::Ordering::Relaxed);
    crate::reader::open_with(path, o)
}

pub mod baseline;
pub mod bench;
pub mod benchmark;
pub mod csv;
pub mod deco;
pub mod export;
pub mod export_rows;
pub mod gt;
pub mod labelled;
pub mod json;
pub mod metrics;
pub mod run;
pub mod split;
pub mod statsheets;
pub mod synthetic;

use std::fmt;
use std::path::PathBuf;

use crate::analysis::SheetLayout;
use crate::grid::Grid;

/// Anything that turns a sheet into a layout. Implemented for every `Fn(&Grid) -> SheetLayout`
/// that is `Sync` (so [`run::evaluate_parallel`] can share it between worker threads).
pub trait Analyzer: Sync {
    fn analyze(&self, grid: &Grid) -> SheetLayout;
}

impl<F: Fn(&Grid) -> SheetLayout + Sync> Analyzer for F {
    fn analyze(&self, grid: &Grid) -> SheetLayout {
        self(grid)
    }
}

/// Default location of the test corpora, relative to the working directory (`HEDGEHOG_DATA`
/// overrides it). The corpora are not part of the repository; see `docs/evaluation.md`.
pub const DEFAULT_DATA_ROOT: &str = "../testdata";

/// Root of the test corpora: `$HEDGEHOG_DATA` or [`DEFAULT_DATA_ROOT`].
pub fn data_root() -> PathBuf {
    match std::env::var_os("HEDGEHOG_DATA") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from(DEFAULT_DATA_ROOT),
    }
}

#[derive(Debug)]
pub struct EvalError(pub String);

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EvalError {}
