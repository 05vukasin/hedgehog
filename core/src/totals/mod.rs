//! Formulas, total rows and their verification (owned by the `totals` workstream).
//!
//! * [`formula`] — small reference parser + formula classification (SUM, SUBTOTAL, `C5+C6+C7`, …).
//! * [`deps`] — per-sheet dependency index (`precedents` / `dependents`) for `trace(cell)`.
//! * [`detect`] — total / subtotal / grand-total candidates (formula and typed-in).
//! * [`verify`] — recompute a candidate from cached values and compare.
//! * [`compute`] — per-cell `t_*` feature columns.
//!
//! Everything works on the sheet's cached values only; nothing is evaluated.

pub mod deps;
pub mod detect;
pub mod features;
pub mod formula;
pub mod keywords;
pub mod verify;

pub use deps::DependencyIndex;
pub use detect::{
    Detection, Direction, Evidence, IncidentalRows, NonSumCheck, NonSumRow, TotalCandidate,
    TotalLevel, TotalOrigin, detect, detect_rows, detect_with, detection, incidental_rows,
    keyword_rows, labelled_rows, model_records,
};
pub use features::compute;
pub use formula::{FormulaKind, ParsedFormula, Ref, RefKind, parse};
pub use keywords::{
    KeywordLevel, NonSumKind, NonSumLabel, ambiguous_keyword, incidental_keyword, nonsum_keyword,
    total_keyword, total_keyword_base,
};
pub use verify::{CheckReason, TotalCheck, numeric_text, verify};
