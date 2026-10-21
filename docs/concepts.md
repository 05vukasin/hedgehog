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
   table (same columns, a header above, numbers continuing). Numbers on a few rows across one
   spacer column, inside a taller table's rows, are that table's sparse columns (profit columns
   filled only on the hours that traded). Two row-aligned blocks that each have their own row-label
   column and numbers stay two tables side by side, even one or two blank columns apart.
2. **Regions.** Within a block, rows at the top that look like a title and rows at the bottom that
   look like notes or sources are split off, including a sentence alone on the last row ("Data as
   reported by the departments; not audited.") and source lines in many languages (`Quelle:`,
   `Remarque :`, `Fuente:`, `Uwaga:`). Stacked tables are split where a header-like row appears in
   the middle. A data row does not look like a header just because of its values: numbers with
   footnote marks or flags (`28472.72*`, `1737³`, `6077.61 (p)`, `20560.32e`) count as numbers,
   censored and suppressed values (`<5`, `confidential`, `[c]`) as blanks, and whole numbers that
   fall in 1900–2100 but jump (`1948 | 2043`) are measures, not a row of years. Ruling lines drawn
   with text (`-----`, `=====`, `_____`, `~~~~~`, a column of `|`) are neither data nor a table
   break: they go to `Notes` as `other`, and a total under one is still checked against the rows
   above. A column right of the table with no header and a remark on a row or two (`check!`,
   `per KL 3/4`) is notes, not a data column.
3. **Header depth.** The number of header rows is found from text versus numbers, merged cells and
   the first data-like row, so multi-row headers (`2024` over `Q1 Q2 Q3 Q4`) stay together. A
   column-numbering row right under the header (`1 2 3 4 5`, as in Destatis tables whose notes say
   "Sp. 3–4") is part of the header, not the first data row, and so is a unit row (`Mrd. EUR | % |
   2020 = 100 | 1 000 Personen`). A header row of periods written as text (`2010-01`, `01/2010`)
   is a header like a row of years, and so is a label over period numbers (`Year | 1 | 2 | … | 20`).
   Key/value metadata above a table (`Date of publication: | 15 Sep 2026 | Date of next
   publication: | …`) is never its header.
4. **Loose cells.** Filled cells outside every table become titles (text near the top), notes
   (marked text or text below a table) or other.
5. **Label columns.** A label written once per block in one of the leftmost columns (`Receipts HH`
   on the first row of the receipts, `Deliveries HH` on the first row of the deliveries, a pipeline
   name on every row next to it) is a group key, and normalize fills it down, so every row says
   which block it belongs to.
6. **Repeated headers.** A body row that repeats the header (a page break in a long list) is
   dropped from the data. Rows with labels are compared by their labels; a header of dates or years
   only repeats with the same dates, so a row of numbers under it (all zeros, say) stays data.

In the default path the learned row model then corrects the tables: it trims title and note rows
at the edges (the table range is the header plus the body), deepens or shortens the header, merges
pieces the rules cut apart (a header-only block above a table, a list cut at a change of content)
and splits stacked tables at a new header, and no two tables overlap (where two rectangles would,
the one that keeps its header and loses the fewest positions gives up the shared part). See
[learned-model.md](learned-model.md#how-it-works); `--rules-only` keeps the rules' tables.

**Shared row labels.** Reports often name their rows once: a summary with flow and pipeline
columns, then a daily grid beside it and one grid per month below it, numbers only, in the same
rows. Normalize gives such a grid copies of the summary's label columns, so every row says what it
is (`SELECT col_C, "Wed / 2002-01-23" FROM T3 WHERE col_B = 'Receipts HH'`). It does so only when
the grid has no labels of its own (no text, date or group column, no column of distinct whole
numbers in order such as years) and its body repeats the summary's row for row: the same data and
total rows with the same blank rows between them, at least 4 data rows, either in the same sheet
rows (beside it, the summary on the left) or shifted down (below it, overlapping columns; the
nearest such summary). The summary needs a label column that names at least 80% of its rows. The
copies keep the summary's column names, have no `source_column` (their cells belong to the
summary), are counted as `row_labels_copied` and named in a table warning.

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
several labelled columns is copied to the right. A unit row under the header ends the names
(`In jeweiligen Preisen / Ursprungswerte / Mrd. EUR`); a column-numbering row does not name the
columns (it stays in `original_header`). Empty, duplicate and over-long names are fixed so
the header is valid for an Excel table. Names are cleaned for agents: footnote marks glued to a word
or year are dropped (`20221)` becomes `2022`) and words split by a hyphen at a line break are joined
(`De-⏎zember` becomes `Dezember`). The manifest keeps the original header cells of each column
(`original_header`) with their addresses. Details:
[normalized-output.md](normalized-output.md#header).

A table's title is the title row above it; among loose lines above a table, source lines and
key/value metadata are skipped and a `Table 1: …` line comes first. Navigation links such as `zur Inhaltsübersicht`,
`Back to contents`, `Retour au sommaire` or `Powrót do spisu tablic` are never a title: the next
title-like line takes their place, and the link goes to `Notes` as `other`.

## Group-header rows

A row with a single label followed by data rows (`Region: North`, then the North products) is a
group header. A row that fills the label columns like the records around it but has no values
(a WDI country without data) is a record, not a group header. In a flat table that structure would be lost, so it becomes a leading column whose
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
- **Totals above their rows:** a subtotal at the top of its group (`Total Belgium` over the Belgian
  rows) or a grand total at the top of a table (`Insgesamt` over `Österreich gesamt`, its Länder
  and `Ausland`) adds the rows below it. A `SUM` over the cells below is read as such; a typed
  total is taken this way only when the rows below it add up (`source_below`): down to the next
  total, or the first run of rows from the row below that does, the totals inside skipped.
- **Totals without a total word:** a row whose numbers equal the sum of the rows above it in at
  least two columns and two thirds of its numbers (at least three non-zero numbers each, not only small
  whole numbers; five in a one-column table), counted since the
  previous total or over everything above without the totals, is a total (`unlabelled`):
  `France métropolitaine` under the départements, `DOM`, `France métropolitaine et DOM`.
- **Known aggregates:** in a country list with World Bank codes, the rows of region and
  income-group codes (`WLD`, `EUU`, `HIC`, `SSF`, ...) are aggregates. Their values are weighted or
  partial, so they are no checkable sums; they are listed on `Totals` with reason
  `known_aggregate`, so a sum over the countries does not count them again.

In the normalized output a row whose total word is its label (its first text, at most four words,
not a value recurring in its column as in long-format data) and whose numbers add up is a total row
even when the layout took it for data; a row the totals took for a total without a total word, or
with its sum below it, likewise. These decisions apply to the table holding the total's label or
numbers only, not to a table beside it.

#### Words that are not total labels

- **In registers** (sheets of at least 200 rows), a total word inside a longer free text is part
  of the record: a description or a name right of other text (`Prótese total anca`), or the row's
  first text when more text follows it and it has more than three words (`Total knee replacement
  implant`, `BEECH TREE TOTAL CARE LIMITED`). Such a row is a total only if more than half of its
  numbers add up (or it closes its group).
- **Names and codes** that start with a total word in another language are total labels only if
  more than half of the row's numbers add up: a longer name starting with one (`SUMA Kommunikation`,
  `Zusammen interreligiös`), or `Somme` (a département), `Sumas` (a place) and `SOM` (Somalia's
  code) right of other text or with numbers on the row below (inside a list). Alone as the label
  under a table's numbers they stay total labels, as `Suma` and `Zusammen` always do.
- A label recurring as a category of its column (`Summe der bezogenen Rohstoffe` once per industry
  in long-format data) stays a total label when the row shares its other keys with a neighbouring
  row; a record with an id of its own (`K-24-00758 | Total station survey equipment`) does not.

#### Average and of-which rows

Rows that are neither data nor sums are listed on `Totals` with their own level and check, and
left out of every sum:

- **Average** rows (`Average`, `Mean`, `Median`, `Ø`, `Durchschnitt`, `Mittelwert`, `Moyenne`,
  `Media`, `Promedio`, `Prosek`, `Średnia`, `Gemiddelde`, ...; at most four words): the value is
  compared with the mean (for `Median`, the median or the mean) of the numbers above it, since the
  previous total or average row, or of all numbers above it without the totals and averages. A row
  is an average row when that matches in some column, or, unchecked, when its label is the average
  sign (`Ø 2024`) or the bare word at the end of its block, over at least two numbers. Reasons:
  `matches_mean`, `matches_median` (`ok`), `unconfirmed` (no match: a weighted or published
  average), `no_source`.
- **Of-which** rows (`of which`, `thereof`, `davon`, `darunter`, `dont`, `di cui`, `de los cuales`,
  `od toga`, `w tym`, `z toho`, `waarvan`, `varav`, `heraf`, `josta`, `ebből`, `из них`, ...): a
  part of the row directly above it (past other of-which rows and a spacer row). The part must not
  be larger than the whole: `within_parent` (`ok`); `exceeds_parent` (`unknown`, not `mismatch`: a rate,
  share or change column is no part of its row above) or `no_parent` otherwise.

A total under such rows adds the data rows only, so `Total` after `of which: exports` or after
`Average` still checks out.

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
| `known_aggregate` | None | Normalized output only: a known aggregate row (World Bank region or income-group code), not a checkable sum. |
| `not_additive` | None | Normalized output only: a typed total that does not add up, in a column whose header (or a row whose label) names a rate, share, percentage, index, average or per-capita value: `%`, `Anteil`, `Quote`, `Rate`, `Index`, `je Einwohner`, `j.E.`, `pro Kopf`, `per capita`, `Durchschnitt`, `average`, `Gesamt-VPI (2020=100)`. Also given to `not_verified` total rows with such a header or label. |
| `rounding_difference` | None | Normalized output only: a typed total off by no more than the rounding of its n summed values (√n units of the last decimal shown, at least one) and by under 1%: typical of statistics published "in 1 000". |
| `average_like` | None | Normalized output only: a typed total that lies between the smallest and the largest of its (non-negative) source values, so it cannot be their sum: an average, a weighted index or a wrong source range. |

The last three only re-judge typed totals that would otherwise be a `mismatch`; a formula total is
never re-judged, because its formula says what it adds. They keep a weighted average or an index
row (`Gesamt-VPI`) from being reported as a wrong total.

Recomputation follows Excel: numbers stored as text in the source range are not summed by `SUM`
(they are listed in `numbers_as_text`), but direct `+` operands are converted, as Excel does.

In the normalized output, `ok`/`False`/`None` appear as `status` `ok`/`mismatch`/`unknown`. The
manifest counts them for the workbook (`totals`) and per table (`tables[].totals`, which also lists
each total row's `row` and `label`).

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
