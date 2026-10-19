//! Cell content: value parsing and per-cell content features (workstream `values`, see PLAN.md).
//!
//! * [`number`] — numbers stored as text in any locale, with currency / units / % / scale.
//! * [`currency`] — ISO 4217 codes and currency symbols.
//! * [`locale`] — explicit locale presets (US, UK, DE, FR, ES, IT, NL, PT, PL, CH, RS, ISO).
//! * [`date`] — dates, months and quarters written as text; Excel serial conversion.
//! * [`missing`] — missing-value tokens, Excel errors, boolean codes.
//! * [`text`] — whitespace cleanup, Cyrillic→Latin, case/diacritic-insensitive keys.
//! * [`keywords`] — total / note / enumerator cues.
//! * [`features`] — `v_*` feature columns for every cell ([`compute`]).
//!
//! Everything here is pure and deterministic.

pub mod currency;
pub mod date;
pub mod features;
pub mod keywords;
pub mod locale;
pub mod missing;
pub mod number;
pub mod text;

pub use currency::{currency_match, is_iso_code, CurrencyMatch};
pub use date::{infer_date_order, parse_date, parse_time, serial_to_ymd, ymd_to_serial, DateOrder, DatePrecision, ParsedDate};
pub use features::{
    cell_features, compute, compute_with, infer_column, sheet_column_hints, sheet_column_hints_with, ColumnHints, FEATURES,
};
pub use locale::Locale;
pub use missing::{is_excel_error, missing_token, parse_bool, MissingKind};
pub use number::{currency, infer_decimal, parse_number, DecimalSep, NegativeStyle, NumberHint, ParsedNumber};
pub use text::{clean, cyrillic_to_latin, fingerprint, norm_key};
