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

pub mod baseline;
pub mod bench;
pub mod csv;
pub mod deco;
pub mod gt;
pub mod json;
pub mod metrics;
pub mod run;
pub mod split;

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

/// Default location of the test corpora (`HEDGEHOG_DATA` overrides it).
pub const DEFAULT_DATA_ROOT: &str = "/home/vukasin/projects/excell-lib-project/testdata";

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
