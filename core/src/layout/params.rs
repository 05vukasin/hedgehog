//! Every tunable threshold of the layout rules, in one place, so DECO scores can drive tuning.

// --- Blocks & gap tolerance -------------------------------------------------------------------

/// Max blank rows between two blocks that may still be one table (spacer rows, L5).
pub const MAX_ROW_GAP: u32 = 3;
/// Max blank rows between a header-only block and the body below it.
pub const HEADER_ONLY_MAX_GAP: u32 = 4;
/// Max blank columns between two blocks that may still be one table (spacer columns, L5).
pub const MAX_COL_GAP: u32 = 1;
/// Max blank columns between exactly row-aligned blocks (or a header-only block on the header
/// row) that are still one table.
pub const MAX_COL_GAP_ALIGNED: u32 = 2;
/// Numbers on a few rows inside a taller block's rows, across one spacer column, join it when
/// they have no header of their own and are at most this many columns wide (sparse columns).
pub const INSIDE_MAX_WIDTH: u32 = 4;
/// Row-aligned blocks that each have row labels and numbers are separate tables side by side
/// only when both are at least this many columns wide (a label and two values, B35/B55).
pub const SBS_MIN_COLS: u32 = 3;
/// Tables side by side without a blank column: split at a second title over a text label column
/// right of a numeric column (B35), when at least this share of the body rows agree.
pub const SPLIT_ADJACENT_BY_TITLES: bool = true;
pub const ADJACENT_SPLIT_SHARE: f32 = 0.8;
/// Vertical gap merge: column overlap / wider block width must reach this.
pub const GAP_MIN_COL_OVERLAP: f32 = 0.6;
/// Horizontal gap merge: row overlap / taller block height must reach this.
pub const GAP_MIN_ROW_OVERLAP: f32 = 0.8;
/// Vertical gap merge: share of shared columns whose majority type (text/number) agrees.
pub const GAP_MIN_TYPE_AGREE: f32 = 0.6;
/// Max blank columns between a narrow text-only block and a numeric block it shares a row with
/// (row labels / group labels left of the numbers); 0 turns the rule off.
pub const LABEL_MAX_GAP: u32 = 5;
/// … and the label block is at most this many columns wide.
pub const LABEL_MAX_WIDTH: u32 = 2;
/// Smallest blank-row gap for the vertical gap merge (0: rows touch without being 8-connected).
pub const VERTICAL_MIN_GAP: i64 = 0;
/// Merge blocks lying inside another group's bounding box into it.
pub const ABSORB_CONTAINED: bool = true;
pub const ABSORB_MAX_GROUPS: usize = 2000;
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
/// A sparser table is kept when it has a header, at least [`SPARSE_MIN_BODY_ROWS`] body rows and
/// [`SPARSE_KEY_COLS`] key columns (ID, name…) filled on at least [`SPARSE_KEY_FILL`] of them:
/// wide registers where most measure columns are blank on most rows.
pub const SPARSE_MIN_BODY_ROWS: usize = 20;
pub const SPARSE_KEY_COLS: usize = 2;
pub const SPARSE_KEY_FILL: f32 = 0.9;
/// Two-column blocks whose first column is mostly `Label:` text are forms, not tables (L12).
pub const FORM_COLON_FRAC: f32 = 0.6;

/// Trailing body rows that together use at most this share of the table's reference width (header
/// / first body rows), in a run of at least `TAIL_MIN_ROWS`, are cut off the table (→ Other).
pub const TAIL_MAX_FILL_FRAC: f32 = 0.5;
pub const TAIL_MIN_ROWS: usize = 3;

/// Cut columns at the right edge of a table that have no header cell above them (→ Other).
/// Tried: worse on DECO dev (tables -1.1, roles -0.2), so off.
pub const RIGHT_TRIM_UNHEADED: bool = false;
/// A last column without a header cell, holding only text on at most this many body rows (or
/// this share of them), is a comment column: its cells are notes (B38).
pub const COMMENT_MAX_CELLS: usize = 2;
pub const COMMENT_MAX_FRAC: f32 = 0.15;

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
/// A single text cell of at least this many words and characters ending a block is a note.
pub const NOTE_SENTENCE_WORDS: usize = 5;
pub const NOTE_SENTENCE_LEN: usize = 30;
/// At least this many single text lines below a blank row at the end of a region are a notes
/// block, not table rows (B53).
pub const NOTE_BLOCK_MIN_ROWS: usize = 2;
/// One line of such a notes block is at least this long.
pub const NOTE_BLOCK_LONG: usize = 20;

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
/// A column-numbering row (`1 2 3 …`) directly under the header belongs to the header.
pub const COLUMN_NUMBER_ROWS: bool = true;
/// A unit row (`Mrd. EUR | % | 2020 = 100`) directly under the header belongs to the header.
pub const UNIT_ROWS: bool = true;
/// A row of parent labels (fewer cells, labels only) right over a header row is part of the
/// header even when its own type votes do not make it one (B53).
pub const PARENT_HEADER_ROWS: bool = true;
/// A short label at the left edge right over a header row, below a title, is the header's top
/// row (`Term` over `Delivery Pt | Q3-2002 | …`, B55).
pub const LABEL_OVER_HEADER: bool = true;
/// A header-like row inside a body starts a new table when it has text/years in at least this many
/// (and this share of) the body's numeric columns (numeric in ≥ half the rows so far).
pub const SPLIT_MIN_HITS: usize = 2;
pub const SPLIT_MIN_HIT_FRAC: f32 = 0.5;
/// Years in this range are header-like labels ("2023") rather than measures.
pub const YEAR_MIN: f64 = 1900.0;
pub const YEAR_MAX: f64 = 2100.0;
/// Two or more years on one row are labels only when neighbours differ by at most this much (a
/// data row whose values happen to lie in the year range jumps further).
pub const YEAR_MAX_STEP: f64 = 30.0;
/// Periods written as text (`2010-01`) are labels only when at least this many share a row.
pub const TEXT_PERIOD_MIN_RUN: usize = 3;

/// Outline key column (label on the first row of each group only) → GroupHeader cells.
pub const KEY_COLUMN_GROUPS: bool = true;
/// Data rows needed, max share of them with a key, share of keys that are text/dates, and share
/// of keys that start a run (next data row has no key).
pub const KEY_MIN_ROWS: usize = 4;
pub const KEY_MAX_FILL: f32 = 0.5;
pub const KEY_MIN_LABEL_FRAC: f32 = 0.8;
pub const KEY_MIN_START_FRAC: f32 = 0.5;
/// Leftmost columns of a table checked for outline keys (the first two before multi-column keys).
pub const KEY_MAX_COLS: usize = 3;
/// … past the first one, only left of a column with text on this share of the data rows.
pub const KEY_DENSE_FRAC: f32 = 0.8;
/// … and keys on at most this share of the data rows (blocks of several rows).
pub const KEY_MAX_FILL_INNER: f32 = 0.25;

/// A group-header row must be followed by at least this many multi-cell rows.
pub const GROUP_MIN_RUN: usize = 2;
/// … or, for a label of at most this many characters (≤ 3 words), one multi-cell row.
pub const GROUP_SHORT_LABEL: usize = 24;
/// A short label in the group-header column on a data row after a blank row opens a group too.
pub const INLINE_GROUP_LABELS: bool = true;
/// … unless more than this share of the data rows has a short label in that column (item names).
pub const INLINE_GROUP_MAX_LABEL_FRAC: f32 = 0.25;

// --- Change points (stacked tables with different schemas) ------------------------------------

/// Each side of a cut needs at least this many data rows (not header-like, not totals).
pub const CP_MIN_ROWS: usize = 3;
/// Columns at least this filled on both sides count for the shared-content distance.
pub const CP_SHARED_MIN_FILL: f32 = 0.2;
/// Text at least this long is "long text" (notes, descriptions) in row signatures.
pub const CP_LONG_TEXT: usize = 25;
/// Skip blocks with more distinct columns / more rows × columns than this.
pub const CP_MAX_COLS: usize = 256;
pub const CP_MAX_WORK: usize = 4_000_000;
pub const CP_MAX_CUTS: usize = 16;
/// Column distance = this × coarse (empty/text/date/number) + rest × fine class distance.
pub const CP_COARSE_WEIGHT: f32 = 0.8;
/// Distance shrinkage for small sides: × m / (m + CP_SHRINK), m = smaller side's rows.
pub const CP_SHRINK: f32 = 2.0;
/// … only while the smaller side has fewer rows than this (so it never moves the cut).
pub const CP_SHRINK_ROWS: f32 = 8.0;
/// A cut needs this profile distance with a cue (header row, gap, total above, column range
/// change), or `CP_STRONG` without one.
pub const CP_WEAK: f32 = 0.3;
/// A cue (header row, gap) at most this many rows from the largest change takes the cut.
pub const CP_SNAP: usize = 2;
pub const CP_STRONG: f32 = 0.55;

// --- Totals -----------------------------------------------------------------------------------

/// A row is a total when at least this share of its numeric cells aggregate cells above (`SUM(`).
pub const TOTAL_FORMULA_FRAC: f32 = 0.5;
/// If more than this share of a table's body rows are formula-only totals, they are a running
/// or computed column instead, and stay Data.
pub const TOTAL_MAX_ROW_FRAC: f32 = 0.4;
/// Mark cells with a `SUM(` over cells to their left (total columns, R3) as Total.
pub const MARK_ROW_SUM_CELLS_AS_TOTAL: bool = false;
/// Cells outside tables with an aggregate formula (SUM/AVERAGE/MAX… over ≥2 cells) or a total
/// label are Total (summary blocks above/beside a table).
pub const LOOSE_AGGREGATES_AS_TOTAL: bool = true;
/// Labels with more tokens than this only count as total when the keyword is the first token.
pub const TOTAL_LABEL_MAX_TOKENS: usize = 6;

// --- Confidence -------------------------------------------------------------------------------

pub const CONF_BASE: f32 = 0.35;
pub const CONF_HEADER: f32 = 0.25;
pub const CONF_TYPE: f32 = 0.25;
pub const CONF_SIZE: f32 = 0.15;
/// Body cells at which the size bonus saturates.
pub const CONF_SIZE_CELLS: f32 = 30.0;
