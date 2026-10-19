//! Every tunable threshold of the layout rules, in one place, so DECO scores can drive tuning.

// --- Blocks & gap tolerance -------------------------------------------------------------------

/// Max blank rows between two blocks that may still be one table (spacer rows, L5).
pub const MAX_ROW_GAP: u32 = 2;
/// Max blank rows between a header-only block and the body below it.
pub const HEADER_ONLY_MAX_GAP: u32 = 4;
/// Max blank columns between two blocks that may still be one table (spacer columns, L5).
pub const MAX_COL_GAP: u32 = 1;
/// Max blank columns between exactly row-aligned blocks (or a header-only block on the header
/// row) that are still one table.
pub const MAX_COL_GAP_ALIGNED: u32 = 2;
/// Vertical gap merge: column overlap / wider block width must reach this.
pub const GAP_MIN_COL_OVERLAP: f32 = 0.6;
/// Horizontal gap merge: row overlap / taller block height must reach this.
pub const GAP_MIN_ROW_OVERLAP: f32 = 0.8;
/// Vertical gap merge: share of shared columns whose majority type (text/number) agrees.
pub const GAP_MIN_TYPE_AGREE: f32 = 0.6;
/// A column counts as a row-label column when this share of its body entries is text.
pub const LABEL_COL_TEXT_FRAC: f32 = 0.5;

// --- Tables -----------------------------------------------------------------------------------

/// Minimum rows (header + body) and columns for a region to be a table.
pub const MIN_TABLE_ROWS: usize = 2;
pub const MIN_TABLE_COLS: u32 = 2;
/// Tables without a header need at least this many columns and rows.
pub const HEADERLESS_MIN_COLS: u32 = 3;
pub const HEADERLESS_MIN_ROWS: usize = 8;
/// Tables with a header need at least this many body rows.
pub const MIN_BODY_ROWS: usize = 1;
/// Minimum filled share of the table rectangle.
pub const MIN_TABLE_DENSITY: f32 = 0.2;
/// Two-column blocks whose first column is mostly `Label:` text are forms, not tables (L12).
pub const FORM_COLON_FRAC: f32 = 0.6;

/// Trailing body rows that together use at most this share of the table's reference width (header
/// / first body rows), in a run of at least `TAIL_MIN_ROWS`, are cut off the table (→ Other).
pub const TAIL_MAX_FILL_FRAC: f32 = 0.5;
pub const TAIL_MIN_ROWS: usize = 3;

// --- Titles & notes ---------------------------------------------------------------------------

/// Max leading single-cell rows inside a block treated as titles.
pub const MAX_TITLE_ROWS: usize = 6;
/// A single merged cell spanning at least this share of the block width is a title, wherever it starts.
pub const TITLE_SPAN_FRAC: f32 = 0.8;
/// Extend a table's range over title rows directly above it (no blank row), as DECO does.
pub const TABLE_INCLUDES_ADJACENT_TITLES: bool = true;
/// A lone label off the left edge directly over a number is a header cell, not a title
/// (tried: slightly worse on DECO dev, so off).
pub const TITLE_HEADER_OVER_NUMBER: bool = false;
/// A loose text block at most this many blank rows above a table is its title/metadata.
pub const TITLE_MAX_GAP: u32 = 5;
/// A loose text block at most this many blank rows below a table is a note.
pub const NOTE_MAX_GAP: u32 = 6;
/// Loose blocks with more rows than this are never titles/notes by position (only by wording).
pub const LOOSE_MAX_ROWS: usize = 4;
/// A single text cell at least this long below a table is a note even without a marker.
pub const NOTE_MIN_LEN: usize = 50;

// --- Headers ----------------------------------------------------------------------------------

/// Max header depth (multi-row / hierarchical headers, H2).
pub const MAX_HEADER_ROWS: usize = 4;
/// A header row may hold at most this share of plain numbers (years and dates don't count).
pub const HEADER_MAX_NUM_FRAC: f32 = 0.25;
/// Rows below a candidate header row used for its type votes.
pub const HEADER_VOTE_WINDOW: usize = 12;
/// A cell "repeats" its column's body type when that type has at least this share below it.
pub const HEADER_VOTE_SHARE: f32 = 0.3;
/// With no type-vote header, accept a wide all-text first row as the header.
pub const HEADER_PRIOR: bool = true;
/// Sparse metadata rows (≤2 cells) that may sit between the titles and the header.
pub const MAX_PRE_HEADER_ROWS: usize = 3;
/// Text-only tables: the first row is a header if it fills at least this share of the width.
pub const TEXT_HEADER_MIN_FILL: f32 = 0.6;
/// A header-like row inside a body starts a new table when it has text/years in at least this many
/// (and this share of) the body's numeric columns (numeric in ≥ half the rows so far).
pub const SPLIT_MIN_HITS: usize = 2;
pub const SPLIT_MIN_HIT_FRAC: f32 = 0.5;
/// Years in this range are header-like labels ("2023") rather than measures.
pub const YEAR_MIN: f64 = 1900.0;
pub const YEAR_MAX: f64 = 2100.0;

/// A group-header row must be followed by at least this many multi-cell rows.
pub const GROUP_MIN_RUN: usize = 2;

// --- Totals -----------------------------------------------------------------------------------

/// A row is a total when at least this share of its numeric cells aggregate cells above (`SUM(`).
pub const TOTAL_FORMULA_FRAC: f32 = 0.5;
/// Mark cells with a `SUM(` over cells to their left (total columns, R3) as Total.
pub const MARK_ROW_SUM_CELLS_AS_TOTAL: bool = false;
/// Labels with more tokens than this only count as total when the keyword is the first token.
pub const TOTAL_LABEL_MAX_TOKENS: usize = 6;

// --- Confidence -------------------------------------------------------------------------------

pub const CONF_BASE: f32 = 0.35;
pub const CONF_HEADER: f32 = 0.25;
pub const CONF_TYPE: f32 = 0.25;
pub const CONF_SIZE: f32 = 0.15;
/// Body cells at which the size bonus saturates.
pub const CONF_SIZE_CELLS: f32 = 30.0;
