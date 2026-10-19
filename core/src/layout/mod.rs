//! Owned by the `layout` workstream — see PLAN.md.
//!
//! Rule-based table detection and cell roles (M2):
//! 1. blocks: 8-connected components of filled cells + merged areas, merged across 1–2 blank
//!    spacer rows / 1 spacer column when the pieces look like one table ([`blocks`]);
//! 2. per region: title rows above, note rows below, header depth (multi-row, merged), splits of
//!    stacked tables at header-like rows, totals (keywords / `SUM(` above), group-header rows ([`tables`]);
//! 3. filled cells outside tables become Title / Note / Other by position and wording.
//!
//! All thresholds live in [`params`]. Cost is O(cells + merge perimeters), never O(used range area).

mod blocks;
mod cells;
pub mod params;
mod tables;

use crate::analysis::{Role, SheetLayout};
use crate::grid::Grid;
use crate::model::Sheet;

use cells::{is_note_label, text_of, Kind};
use params::{NOTE_MAX_GAP, TITLE_MAX_GAP};
use tables::Out;

/// Detects tables and assigns one [`Role`] per entry of `grid.sheet.cells` (same order).
pub fn analyze(grid: &Grid) -> SheetLayout {
    let sheet = grid.sheet;
    let mut out = Out { roles: vec![Role::Other; sheet.cells.len()], tables: Vec::new(), loose: Vec::new() };
    for region in blocks::regions(grid) {
        tables::analyze_region(sheet, region, &mut out);
    }
    classify_loose(sheet, &mut out);
    out.tables.sort_by_key(|t| (t.rect.top, t.rect.left));
    SheetLayout { tables: out.tables, roles: out.roles }
}

/// Text near the top of a table → Title; marked or below a table → Note; the rest → Other.
fn classify_loose(sheet: &Sheet, out: &mut Out) {
    let first_top = out.tables.iter().map(|t| t.rect.top).min();
    for region in std::mem::take(&mut out.loose) {
        let r = region.rect;
        let mut above = first_top.is_some_and(|t| r.bottom < t);
        let mut below = false;
        for t in &out.tables {
            let t = t.rect;
            let cols = r.left <= t.right.saturating_add(1) && r.right.saturating_add(1) >= t.left;
            if !cols {
                continue;
            }
            if r.bottom < t.top && t.top - r.bottom - 1 <= TITLE_MAX_GAP {
                above = true;
            }
            if r.top > t.bottom && r.top - t.bottom - 1 <= NOTE_MAX_GAP {
                below = true;
            }
        }
        for row in &region.rows {
            for e in &row.entries {
                let Some(i) = e.cell else { continue };
                out.roles[i] = match e.kind {
                    Kind::Text => {
                        let t = text_of(&sheet.cells[i]).unwrap_or("");
                        if is_note_label(t) {
                            Role::Note
                        } else if above {
                            Role::Title
                        } else if below {
                            Role::Note
                        } else {
                            Role::Other
                        }
                    }
                    _ => Role::Other,
                };
            }
        }
    }
}
