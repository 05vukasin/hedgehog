//! Hedgehog core: read messy spreadsheets into one model, then understand and normalize them.

pub mod analysis;
pub mod features;
pub mod grid;
pub mod model;
pub mod reader;

// Workstreams; each owns its folder.
pub mod context;
pub mod eval;
pub mod layout;
pub mod learned;
pub mod normalize;
pub mod totals;
pub mod values;

mod threads;
#[doc(hidden)]
pub use threads::spawned as inner_threads_spawned;

pub use analysis::{DetectedTable, Role, SheetLayout};
pub use features::Features;
pub use grid::Grid;
pub use model::{Cell, Rect, Sheet, Value, Workbook, a1};
pub use normalize::{
    Normalized, Options as NormalizeOptions, Output as NormalizeOutput, normalize_file,
    normalize_workbook, try_normalize_workbook,
};
pub use reader::{Error, open, open_bytes};

/// Short plain-text overview of a workbook, one line per sheet.
pub fn describe(wb: &Workbook) -> String {
    let mut out = String::new();
    for s in &wb.sheets {
        let range = match s.used_range() {
            Some(r) => format!("{}:{}", a1(r.top, r.left), a1(r.bottom, r.right)),
            None => "empty".to_string(),
        };
        out.push_str(&format!(
            "{}: {} · {} cells · {} formulas · {} merged ranges\n",
            s.name,
            range,
            s.cells.len(),
            s.formula_count(),
            s.merges.len()
        ));
    }
    out
}
