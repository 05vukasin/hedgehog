# Concepts

The ideas behind hedgehog's output: how it finds tables, what a cell role is, how headers are
flattened, how totals are found and verified, and how every value stays traceable to its source cell.

Contents: [The pipeline](#the-pipeline) · [Tables](#tables) · [Cell roles](#cell-roles) ·
[Headers](#headers) · [Group-header rows](#group-header-rows) ·
[Totals and verification](#totals-and-verification) · [Value parsing](#value-parsing) ·
[Provenance](#provenance) · [Determinism](#determinism)

## The pipeline

```
 workbook ──▶ read ──▶ detect tables ──▶ parse values ──▶ verify totals ──▶ normalize ──▶ .normalized.xlsx
 (xlsx, xls,  (cells,   and cell roles    (per-column      (recompute       (flat header,   + manifest.json
  xlsb, ods)   formulas,                   locale)          from cached      typed columns,  + summary / facts
               merges)                                      values)          Totals, Notes)
```

Every stage is deterministic Rust code with no network access and no LLM. The same file always gives
the same output. Each stage runs once per sheet and its result is reused by the later ones.

## Tables

A sheet can hold several tables, side by side or stacked, with titles above and notes below. The
rule-based detector (`core/src/layout/`) works like this:

1. **Blocks.** Filled cells and merged areas are grouped into 8-connected components. Pieces
   separated by one or two blank spacer rows or one spacer column are merged when they look like one
   table (same columns, a header above, numbers continuing).
2. **Regions.** Within a block, rows at the top that look like a title and rows at the bottom that
   look like notes or sources are split off. Stacked tables are split where a header-like row
   appears in the middle.
3. **Header depth.** The number of header rows is found from text versus numbers, merged cells and
   the first data-like row, so multi-row headers (`2024` over `Q1 Q2 Q3 Q4`) stay together.
4. **Loose cells.** Filled cells outside every table become titles (text near the top), notes
   (marked text or text below a table) or other.

Each detected table gets a confidence between 0 and 1, reported in the manifest. The cost is linear in
the number of filled cells, never in the area of the used range, so a sheet with a few cells at
`A1` and `XFD1048576` is cheap.

Measured accuracy on real spreadsheets is in [evaluation.md](evaluation.md).

## Cell roles

Every non-empty cell gets one role. The set is harmonized with the public DECO, Strudel and
TableSense annotation schemes, so it can be scored against them.

| Role | Meaning | Example |
|---|---|---|
| `title` | A title above a table | `Sales report 2024` |
| `header` | A column header cell, possibly one of several header rows | `Revenue`, `Q1` |
| `data` | A value or row label in the table body | `Widgets`, `1200.5` |
| `group_header` | A label alone on its row that starts a section | `Region: North` |
| `total` | A total or subtotal, label or value | `Subtotal`, `1500.5` |
| `note` | A note, footnote or source line | `Source: internal ERP export` |
| `other` | Anything else | a stray cell far from any table |

In the normalized output, `data` cells go to the table sheets, `total` cells to `Totals`, and
`title`, `note` and `other` cells to `Notes`. `header` and `group_header` cells become column names
and the group column.

Roles come from the layout rules refined by a learned row model, which decides what kind of row
each row of a table is; see [learned-model.md](learned-model.md). `--rules-only` uses the rules
alone.

## Headers

Multi-row headers are flattened into one row with ` / ` between levels:

```
 Product │ Revenue        │ Units          Product │ Revenue / Q1 │ Revenue / Q2 │ Units
         │  Q1   │  Q2    │          ──▶
```

Before joining, a merged header cell fills its whole area, and a parent label written once over
several labelled columns is copied to the right. Empty, duplicate and over-long names are fixed so
the header is valid for an Excel table. The manifest keeps the original header cells of each column
(`original_header`) with their addresses. Details:
[normalized-output.md](normalized-output.md#header).

## Group-header rows

A row with a single label followed by data rows (`Region: North`, then the North products) is a
group header. In a flat table that structure would be lost, so it becomes a leading column whose
value repeats for every row of the group. A shared `Prefix: value` form names the column after the
prefix; otherwise the column is called `group`.

## Totals and verification

### Finding totals

A cell is taken for a total when there is evidence (`Workbook.totals()` reports it in `evidence`):

- **Formula:** `SUM(...)`, `SUBTOTAL(9|109, ...)`, `AGGREGATE(9, ...)` or a chain of references
  `C5+C6+C7`. These are recognized in any language because the formula says what it adds.
- **Keyword:** the row's label (or the column header, for a total column) is a total word such as
  `Total`, `Subtotal`, `Grand total`, `Sum`, `Ukupno`. See [locales.md](locales.md#total-and-note-keywords)
  for the languages covered.
- **Typed totals** (a plain number in a total row) need a source range. hedgehog tries the plausible
  ranges above (or to the left) and prefers one whose sum equals the typed value. If none matches,
  the guess is trusted only when most sibling totals in the same row or column did match
  (`siblings_match`); otherwise the result is `unconfirmed`, not `mismatch`.
- **Grand totals** that add the subtotals, or add everything except the subtotals, are recognized
  (`sums_totals`); the subtotal rows are listed in `excluded`.

The level (`total`, `subtotal`, `grand_total`) comes from the keyword and from nesting. The direction
is `vertical` for a total row and `horizontal` for a total column.

### Verifying totals

Each total is recomputed from the **cached values** of its source cells; no formula is evaluated.
Then `ok` is `True`, `False` or `None` and `reason` says why:

| reason | ok | Meaning |
|---|---|---|
| `match` | True | Equal within floating-point tolerance. |
| `rounded` | True | A typed total equals the sum rounded to its own number of decimals. |
| `mismatch` | False | The total disagrees with its source. For a formula this means a stale cached result. |
| `double_counted` | False | The total adds subtotals on top of their data; `expected` is the sum without them. |
| `matches_average` | None | A typed value equals the average of the source, so it is probably not a sum. |
| `no_cached_value` | None | The total is a formula saved without its result (common in files written by scripts). |
| `not_numeric` | None | The total cell holds text, a boolean or an error. |
| `error_in_source` | None | A source cell holds an error value. |
| `missing_source_value` | None | A source formula has no cached value. |
| `no_source` | None | Nothing numeric in the source, and the total is not zero. |
| `unconfirmed` | None | A typed total disagrees with its guessed source, and nothing confirmed the guess. |
| `not_verified` | None | Normalized output only: a row the layout marked as a total whose source range was not found. |

Recomputation follows Excel: numbers stored as text in the source range are not summed by `SUM`
(they are listed in `numbers_as_text`), but direct `+` operands are converted, as Excel does.

In the normalized output, `ok`/`False`/`None` appear as `status` `ok`/`mismatch`/`unknown`.

## Value parsing

Numbers, percentages, currencies, units, dates, booleans and missing-value tokens are parsed from
text with per-column conventions. Ambiguous values are never resolved silently. The rules and the
inference order are in [locales.md](locales.md).

A column's dtype is the type that at least 90% of its non-missing values have. Values that do not
fit stay as text and are reported, so nothing is turned into null without a trace.

## Provenance

Every value in the output can be traced back:

- **Rows:** `_src_row` in every table sheet is the 1-based row in the source sheet.
- **Columns:** `source_column` in the manifest is the source column letter, and `original_header`
  lists the header cells the name was built from.
- **Tables:** the manifest gives each table's source sheet, full range, header range, data range,
  title cell and attached notes.
- **Totals:** each row of `Totals` has the source sheet and cell, the label cell, the source ranges
  and the formula.
- **Everything else:** every other non-empty cell is on `Notes` with its address. The manifest's
  `cells` counts prove it: `total` equals `in_tables + in_totals + in_notes`.
- **The file itself:** the manifest records the source file's SHA-256 and size.

The same applies to the Python tools on the source workbook: `get_cell`, `get_range`, `find_text` and
`totals` always return addresses, and LLM output is checked against them with
[`check_citations`](python-api.md#hedgehogcheck_citations).

## Determinism

- No randomness, no network, no LLM in the core. The learned model is a fixed set of decision
  trees built into the library: the same file gives the same roles on every machine.
- The manifest has a fixed key order and no timestamps: the same input gives byte-identical output,
  so a diff between two runs shows real changes.
- The source file is opened read-only and never written.

## See also

- [normalized-output.md](normalized-output.md): the files these concepts produce
- [locales.md](locales.md): value parsing in detail
- [evaluation.md](evaluation.md): how well table detection and roles work today
- [architecture.md](architecture.md): where each stage lives in the code
