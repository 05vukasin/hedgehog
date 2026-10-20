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

pub use analysis::{DetectedTable, Role, SheetLayout};
pub use features::Features;
pub use grid::Grid;
pub use model::{a1, Cell, Rect, Sheet, Value, Workbook};
pub use normalize::{normalize_file, normalize_workbook, Normalized, Options as NormalizeOptions, Output as NormalizeOutput};
pub use reader::{open, Error};

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
