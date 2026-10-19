//! Coarse cell type, derived only from [`Value`] (no parsing of text — that is the `values` stage).

use crate::model::{Cell, Value};

/// Coarse type of a sheet position. The `u8` value is the code stored in `c_kind` / `c_n_*` features,
/// so the numbering is part of the feature contract — append, never renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    /// No cell and not covered by a merged area (also: positions outside the sheet).
    Empty = 0,
    Number = 1,
    Text = 2,
    Date = 3,
    Bool = 4,
    Error = 5,
    /// Formula whose cached value was not saved.
    FormulaOnly = 6,
    /// Position without its own cell but covered by a merged area (non-anchor part of a merge).
    MergedEmpty = 7,
}

impl Kind {
    pub const COUNT: usize = 8;

    pub fn code(self) -> f32 {
        self as u8 as f32
    }

    /// Number or date — both are numeric to a reader of the table.
    pub fn is_numeric(self) -> bool {
        matches!(self, Kind::Number | Kind::Date)
    }
}

/// Coarse type of a cell.
pub fn kind_of(cell: &Cell) -> Kind {
    match &cell.value {
        Some(Value::Number(_)) => Kind::Number,
        Some(Value::Text(_)) => Kind::Text,
        Some(Value::DateTime(_)) => Kind::Date,
        Some(Value::Bool(_)) => Kind::Bool,
        Some(Value::Error(_)) => Kind::Error,
        None if cell.formula.is_some() => Kind::FormulaOnly,
        None => Kind::Empty,
    }
}

/// Numeric value of a number or date cell.
pub(crate) fn numeric_value(cell: &Cell) -> Option<f64> {
    match cell.value {
        Some(Value::Number(v)) | Some(Value::DateTime(v)) => Some(v),
        _ => None,
    }
}
