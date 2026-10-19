//! Cell content: value parsing and per-cell content features (workstream `values`, see PLAN.md).
//!
//! * [`number`] — numbers stored as text in any locale, with currency / units / % / scale.
//! * [`date`] — dates, months and quarters written as text; Excel serial conversion.
//! * [`missing`] — missing-value tokens, Excel errors, boolean codes.
//! * [`text`] — whitespace cleanup, Cyrillic→Latin, case/diacritic-insensitive keys.
//! * [`keywords`] — total / note / enumerator cues.
//! * [`features`] — `v_*` feature columns for every cell ([`compute`]).
//!
//! Everything here is pure and deterministic.

pub mod date;
pub mod features;
pub mod keywords;
pub mod missing;
pub mod number;
pub mod text;

pub use date::{infer_date_order, parse_date, serial_to_ymd, ymd_to_serial, DateOrder, DatePrecision, ParsedDate};
pub use features::{cell_features, compute, infer_column, sheet_column_hints, ColumnHints, FEATURES};
pub use missing::{is_excel_error, missing_token, parse_bool, MissingKind};
pub use number::{infer_decimal, parse_number, DecimalSep, NegativeStyle, NumberHint, ParsedNumber};
pub use text::{clean, cyrillic_to_latin, fingerprint, norm_key};
