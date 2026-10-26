# Normalized output

What `normalize` writes: the layout of the clean workbook, every column of its `Totals` and `Notes`
sheets, and the manifest schema.

`normalize` turns a messy workbook into a clean, typed workbook plus a JSON manifest. The output is
meant for an agent with a code sandbox: it loads each table with one `pandas.read_excel` call and can
trace every value back to its source cell. The source file is only read, never modified.

Contents: [Running it](#running-it) · [Files](#files) · [Table sheets](#table-sheets) ·
[Totals sheet](#totals-sheet) · [Notes sheet](#notes-sheet) · [README sheet](#readme-sheet) ·
[CSV files](#csv-files) · [Manifest](#manifest) · [Transformations](#transformations) ·
[Known limits](#known-limits)

The examples on this page come from `sales.xlsx`, the sample workbook built in the
[quickstart](quickstart.md#a-sample-messy-workbook).

## Running it

```console
$ hedgehog normalize sales.xlsx -o out/
sales.xlsx: 1 tables, 39 cells (28 in tables, 9 totals, 2 notes) in 16 ms
  out/sales.normalized.xlsx
  out/sales.manifest.json
```

```python
import hedgehog

r = hedgehog.normalize("sales.xlsx", "out/", keep_totals=False, csv=False, locale=None)
r.xlsx_path        # 'out/sales.normalized.xlsx'
r.manifest_path    # 'out/sales.manifest.json'
r.manifest         # the manifest as a dict
r.tables           # r.manifest["tables"]
r.table("T1")      # one table's manifest entry, by id or output sheet name
df = r.read("T1")  # pandas DataFrame (needs pandas + openpyxl)
```

Options (CLI flag / Python keyword):

| Option | Default | Effect |
|---|---|---|
| `--keep-totals` / `keep_totals=True` | off | Total and subtotal rows stay in the table sheets, marked in a `_row_type` column. By default they are removed from the data and listed only on `Totals`. |
| `--csv` / `csv=True` | off | Also write one UTF-8 CSV per table. |
| `--locale TAG` / `locale="TAG"` | inferred | Fixes the decimal separator and day/month order for every column. See [locales.md](locales.md). |
| `--threads N` (CLI, `NormalizeOptions::threads` in Rust) | up to 4 | Threads that analyze sheets in parallel. The output does not depend on it. |

From Rust: `hh_core::normalize_file(path, out_dir, &hh_core::NormalizeOptions { keep_totals, csv, locale })`.
`hh_core::normalize_workbook(&wb, &opts)` gives the same result in memory without writing files.

## Files

| File | Content |
|---|---|
| `<stem>.normalized.xlsx` | Sheets `README`, one sheet per table, `Totals`, `Notes`, in this order |
| `<stem>.manifest.json` | Machine-readable description of all of the above ([schema](#manifest)) |
| `<stem>.<table sheet>.csv` | Only with `--csv`, for example `sales.T1_sales_report_2024.csv` |

`<stem>` is the input file name without its extension. The output directory is created if it does
not exist; existing files with the same names are overwritten.

## Table sheets

One sheet per detected table, named `T<n>_<slug>` and numbered in source order (sheet by sheet, top to
bottom, left to right). The slug comes from the table title, or from the source sheet name when the
table has no title. It is lowercase ASCII: Cyrillic is transliterated, diacritics are folded, and the
whole sheet name is at most 31 characters.

The data is registered as an Excel table (ListObject) with the **same name as the sheet**, so
`ws.tables["T1_sales_report_2024"]` works in openpyxl. The table has an autofilter, row 1 is frozen and
column widths are set.

The sample sheet becomes:

| Region | Product | Revenue / Q1 | Revenue / Q2 | Units | Margin | _src_row |
|---|---|---|---|---|---|---|
| North | Widgets | 1200.5 | 1350 | 1200 | 12.00% | 6 |
| North | Gadgets | 300 | 410 | 350 | 8.50% | 7 |
| South | Widgets | 980 | 1150 | | 10.00% | 10 |
| South | Gizmos | 2045 | 1720 | 2100 | 15.00% | 11 |

### Header

Row 1 is one flat header row. Every name in it is unique and non-empty.

- Multi-row headers are joined top to bottom with ` / ` (`Revenue / Q1`). Before joining, merged
  header cells fill their whole area, and a parent label written once over several labelled columns
  is copied to the right (`2024` over `Plan | Actual` gives `2024 / Plan`, `2024 / Actual`).
- A duplicate name gets ` (2)`, ` (3)`, and so on. Excel compares table column names
  case-insensitively, and so does this check.
- Each header part is cleaned for use as a name (the raw cells stay in `original_header`):
  - footnote marks are removed: `20221)` becomes `2022`, `Veränderungsrate3)` becomes
    `Veränderungsrate`, `Total²` becomes `Total`, `Einwohner*` becomes `Einwohner`. Only an
    unmatched `1)`/`12)`/`a)` at the end, or trailing superscripts, `*`, `†`, `‡` count as marks;
    balanced parentheses (`Gesamt-VPI (2020=100)`, `(in 1 000)`), numbers with three or more digits
    before `)` and a header that is only a mark (`1)`) are kept. A `²` or `³` right after a length
    unit is the unit's exponent, not a mark: `Fläche km²`, `Einwohner je km²`, `Wasser Mio. m³`,
    `Dichte kg/m³` keep it (`Fläche km²¹` loses only the `¹`); subscripts (`CO₂`) are never marks;
  - a word split by a hyphen at a line break is joined (`De-⏎zember` becomes `Dezember`,
    `Kalender-⏎faktoren` becomes `Kalenderfaktoren`, `Benützungs- gebühren` becomes
    `Benützungsgebühren`), except before a conjunction (`Ein- und Ausfuhr` stays); before an
    uppercase word only the break is removed (`Abgaben-⏎Ertragsanteile` becomes
    `Abgaben-Ertragsanteile`). Without a break, `X-y` is joined only when the result is a month
    name (`Sep-tember`) or the left part is a capitalized word and the pair is not a common
    compound (`Nachlassverbindlich-keiten` is joined; `Follow-up`, `Non-food`, `E-mail`, `EU-weit`
    are not);
  - whitespace and line breaks collapse to one space.

  `headers_cleaned` counts the columns whose name changed this way.
- A column without a header is named `col_<source letter>`, for example `col_F`.
- The label of a header row of series codes (`Dataset identifier code | MGSL | MGSF | …`) stays in
  `original_header` but does not name the column below it.
- Month-end dates across a header that step back from December to January of the same year
  (`2001-07-31` … `2001-12-31`, `2001-01-31` …) are read as the next year from the step on
  (`2002-01-31`), with a table warning.
- A block of a stacked statistical table without a header of its own (its columns `col_D` …, or
  one unit over all of them), under a block with the same source columns, takes that block's
  names followed by its own unit (`Mrd. EUR`, `%`), the one phrase written over the block, or
  its short title (`Finanzierungs-saldo / % des Bruttoinlandsprodukts`,
  `Bruttoinlandsprodukt / … / Veränderung gegenüber dem Vorjahr in %`); a table warning says so.
- When the header rows the layout found are sparse (`Ano: 2016`) and a fuller row of distinct
  header words follows before the first data row, that row is the header and the rows above are
  title rows. A numbered caption (`Quadro 1`, `Table 3:`, `Tabelle 2`, `Cuadro 4`) is preferred as
  the title, joined with the text beside it when it is only the number; a caption left as the
  last row of the table above is the title of the table below.
- Names longer than 255 characters are cut.

### Rows

- **Data rows only.** Header, title, note, blank and repeated-header rows are removed. Total and
  subtotal rows are removed too, unless `keep_totals` is set, and so are the rows that are no data
  and no sums: **average** rows (`Average`, `Mean`, `Median`, `Ø 2024`, `Durchschnitt`, `Moyenne`,
  `Prosek`, ...) and **of-which** rows (`of which: exports`, `davon Inland`, `dont : …`, `di cui`,
  `od toga`, `w tym`, ...), which repeat a part of the row above them. Both are listed on `Totals`
  (levels `average` and `of_which`). A **balance** row right under a total row (`SALDO (D - P)`,
  `Balance`, `Differenz`, `Razlika`, `Net`, ...) whose number is the difference of two of the
  total row's numbers (debit total − credit total) goes with the totals too (`_row_type` `total`,
  level `balance` on `Totals`).
- **Missing values are empty cells.** This covers missing tokens (`-`, `n/a`, `..`, `:`, `x`, `null`, ...),
  Excel errors (`#N/A`, `#REF!`, ...) and formulas without a cached value.
- **Group-header rows** (a label alone on its row, followed by data rows) become a **leading column**.
  If every label has the form `Prefix: value` with the same prefix, the column is named after the
  prefix (`Region: North` gives column `Region` with value `North`). Otherwise the column is named
  `group` and holds the full label. Nested group labels are joined with ` / `.
- **Sex breakdowns.** Blocks of three rows that split a row label by sex (`M` / `F` / `T`,
  `H` / `M` / `HM`, `Männer` / `Frauen` / `Insgesamt`, `Masculino` / `Feminino` / `Total`, ...) are
  data: the block's label (centred on the middle row or split over the rows) is written on all three
  rows, the both-sexes row is a `subtotal` of the two others when its numbers add up, and every row
  of a block labelled with a total word (`Total`) is a `total`. An unnamed code column is named
  `sex`.
- **`_src_row`** is the 1-based row number in the source sheet. It is always the last column, or the
  one before `_row_type`. A value in output column `c` came from source cell
  `<columns[c].source_column><_src_row>` on `source.sheet` of the table's manifest entry. Row
  labels copied from a table beside or above (`source_column` `null`, named in the table's
  `warnings`) come from that table's label cells in the matching row.
- **Derived columns** follow the column they are computed from; they have no source column
  (`source_column` `null`, `original_header` empty, `derived` in the manifest):
  - **`<name>_period`**: a text column whose labels name periods (at least three, 90% of the
    distinct ones: `Jän.21`, `März 2022`, `Ø 22` (an annual average: the year), `2025 Q1`,
    `Jan-24`, `Mar-May 1971` (a rolling range, the year is the last month's), stored dates among
    them, and labels whose trailing footnote digit continues the period above, `Apr.201` = April
    2020 + mark `1`) gets each label's period in a form that sorts and compares as text: `2022`,
    `2022-Q1`, `2022-01`, `2024-W05`, `2022-01-15`, `2025-11/2026-01`. An unnamed label column
    (`col_A`) gives `period`. Not added when every label is already written that way.
  - **`<name>_key`**: a name or category column (at most half of its values distinct, at most 40
    characters on average) whose values differ only in case, spaces, punctuation or diacritics
    (`Mc Kay` / `McKay`, `On-system` / `On-System`) holds one spelling per group, the most frequent;
    `GROUP BY "<name>_key"` counts the spellings together. The table's `warnings` name the groups.
  - **`<name>_parent`**: the row label column of an outline indented at two levels or more (the
    cells' indent style, or leading spaces when no label has one): each data row's parent, the
    nearest row above it whose label is indented less (`Mood disorder` under `Diagnosed mood or
    anxiety disorder`, both rows with values), empty for a top-level row. A group-header row
    (`group` column) starts afresh. Added when at least two rows, and a tenth of them, have a
    parent; an unnamed label column (`col_A`) gives `parent`. Files up to 2 MB are read with their
    cell formats for this.
- **Key/value tables**: labelled numbers outside every other table of a sheet (`Total Income |
  9666 | Total Expenditure | 7266` above a cash journal, an assumptions block beside a model) form
  one table per sheet, titled `key/value cells`, numbered after all other tables of the file:
  `key` (the one or two text cells left of the number), `value`, `cell` (the number's address).
  A number followed by another number (a row of data), a year, and text or date values stay in
  Notes; a sheet needs two pairs. Agents list these tables on one `KEY/VALUE` catalog line.
- **`_row_type`** (only with `keep_totals`): `data`, `subtotal`, `total`, `average` or `of_which`.
  Only `data` rows add up to the table's figures: a `total` or `subtotal` row sums data rows, an
  `average` row averages them, an `of_which` row is a part of the row above it (a balance under
  the totals is a `total` row).

### Values and dtypes

Values are written, not formulas. Each column gets one dtype:

| dtype | Written as | Number format |
|---|---|---|
| `integer`, `number` | number | General |
| `percent` | fraction (`12%` becomes `0.12`) | `0.00%` |
| `currency` | number, symbol or code stripped (see `currency` in the manifest) | `#,##0.00` |
| `date` / `datetime` | Excel date | `yyyy-mm-dd` / `yyyy-mm-dd hh:mm:ss` |
| `bool` | TRUE / FALSE (`yes`/`no`, `true`/`false`, `✓`/`✗` and a few other languages' words included) | |
| `text` | text, trimmed, runs of whitespace collapsed to one space | |
| `empty` | the column has a header but no values | |

Rules:

- A column gets a numeric, date or bool dtype when at least 90% of its non-missing values read that
  way, either as stored values or as text parsed with the column's locale.
- Censored, suppressed and see-note cells (`<5`, `c`, `[c]`, `confidential`, `see note`) stay text
  but do not count against a numeric column, and a few distinct words among many numbers
  (`approx.`) do not either; see [locales](locales.md#footnote-marks-censored-and-suppressed-values).
- Footnote marks on numbers (`123¹`, `12 (r)`, `7 p`, `20560.32e`) are removed from the value and
  kept in the column's `footnote_marks`.
- Footnote marks on text labels are removed the same way and kept in the label column's
  `footnote_marks`: `Delta²` becomes `Delta`, `Germany *` becomes `Germany`, `Norway (1)` becomes
  `Norway`. Marks are trailing superscript digits, `*`, `†`, `‡` and one or two digits in
  parentheses after a word; they are removed only from labels of at most 60 characters that keep
  a letter, and only when at most half of the column's labels carry one (a column of names that
  all end the same way is left alone). Parts of a name or unit stay: `Fläche km²`, `Mio. m³`,
  `CO₂`, `H₂O`, `Fund II`, `Area 51`, `Formula 1`, `Table 3 (2)`, `Category (A)`.
- Text numbers with leading zeros (`00123`) keep the column as `text`, because they are IDs.
- A value that does not fit the column's dtype is **kept as text**, and the table's `warnings` list
  its source cell. Nothing is turned into null silently.
- In a `text` column, stored numbers and dates become their text form (`2024`, `2024-03-31`).

## Totals sheet

One row per detected total or subtotal cell, registered as an Excel table named `Totals`.

| Column | Meaning |
|---|---|
| `table` | Table id (`T1`) whose range contains the cell; empty if none |
| `sheet`, `cell` | Source sheet and address of the total |
| `label`, `label_cell` | Text that labels it (`Grand total`) and where that text is |
| `level` | `total`, `subtotal` or `grand_total`; `average` (a mean of the rows above it), `of_which` (a part of the row above it) or `balance` (the difference of two totals of the total row above it) |
| `direction` | `vertical` (adds the cells above, or below for a subtotal over its group and a grand total at the top) or `horizontal` (adds the cells to its left) |
| `origin` | `formula` (SUM, SUBTOTAL, `A+B+C`), `typed` (a number in a total-labelled row), or `layout` (a number in a row the layout marked as a total, but whose source range was not found) |
| `value` | Value in the file (the cached result for formulas) |
| `recomputed` | Sum of the source cells, recomputed from the cached values (for `average`: their mean; for `of_which`: the value of the row above; for `balance`: the difference of the two total cells) |
| `status` | `ok`, `mismatch` or `unknown` |
| `reason` | `match`, `rounded`, `mismatch`, `double_counted`, `matches_average`, `no_cached_value`, `not_numeric`, `error_in_source`, `missing_source_value`, `no_source`, `unconfirmed`, `not_verified`, `not_additive`, `rounding_difference`, `average_like`, `known_aggregate`; for average rows `matches_mean`, `matches_median`, `unconfirmed`, `no_source`; for of-which rows `within_parent`, `exceeds_parent`, `no_parent` ([meanings](detection-rules.md#totals-and-verification)) |
| `sources` | Source ranges, for example `C6:C7` |
| `formula` | The formula, if any |
| `kept_in_data` | The total is also in the table data: `keep_totals` is set, or it is a total column (horizontal) |
| `count` | Number of totals the row stands for: 1, or the length of a run (below) |

A total column (a row total on every data row, such as `=SUM(H6:M6)` in `N6`, `N7`, ...) would list
one row per data row. Three or more consecutive row totals in one column that all check out (same
table, level, origin and label) are listed as **one run**: `cell` is a range such as `AB6:AB10494`,
`count` is the number of totals, `value` and `recomputed` are empty, and `sources` and `formula`
are those of the first one. Mismatched and unverified totals are always listed one by one, so
`status == "mismatch"` still finds every wrong cell. In the manifest, `totals.count`, `ok`,
`mismatch` and `unknown` count totals (runs counted by their `count`), not items.

For the sample, five totals check out and one does not:

| table | sheet | cell | label | level | origin | value | recomputed | status | sources |
|---|---|---|---|---|---|---|---|---|---|
| T1 | Sales | B8 | Subtotal | subtotal | typed | 1500.5 | 1500.5 | ok | B6:B7 |
| T1 | Sales | B13 | Grand total | grand_total | typed | 4600 | 4525.5 | mismatch | B6:B12 |

(Four more `ok` rows omitted.) The grand total in `B13` adds the data rows `B6:B12` without the two
subtotal rows, and the file says 4600 where the data add up to 4525.5.

Recomputation follows Excel's rules, so numbers stored as text in the source range are not summed.

## Notes sheet

Every source cell that is neither table data nor a total, with its address, so nothing is dropped
silently. Columns: `sheet`, `cell`, `kind`, `table`, `text`, `formula`. `kind` is one of:

| kind | Meaning |
|---|---|
| `title` | A title above a table |
| `note` | A note or footnote, such as `Source: ...` or `* preliminary` |
| `repeated_header` | A header row repeated inside the body |
| `total_row` | Text in a removed total row other than its label |
| `total` | An aggregate outside any table |
| `other` | A cell that belongs to no table, or a navigation link such as `zur Inhaltsübersicht` or `Back to contents` (never a title) |

`table` is set when a title or note could be attached to a table: the nearest table below a title, or
above a note, within 6 blank rows, with overlapping columns. For the sample:

| sheet | cell | kind | table | text |
|---|---|---|---|---|
| Sales | A1 | title | T1 | Sales report 2024 |
| Sales | A15 | note | T1 | Source: internal ERP export, amounts in USD |

## README sheet

A human-readable first sheet: the source file name and SHA-256, a short description of the layout,
one line per table (sheet, source sheet, source range, rows, columns, title) and copy-paste loading
code for pandas, openpyxl and the manifest.

## CSV files

With `--csv`, one file per table sheet: UTF-8, RFC 4180 quoting, ISO dates, percentages as fractions,
and an empty field for a missing value. The columns are the same as in the table sheet, including
`_src_row` (and `_row_type` with `--keep-totals`).

## Manifest

`<stem>.manifest.json`, `schema_version` 2. The manifest is deterministic: the same input gives the
same bytes. Keys have a fixed order and there are no timestamps, so a diff between two runs is
meaningful. Rows are 1-based and columns are letters, as in Excel.

The manifest of the sample, shortened (`…` marks omitted entries, `//` comments are not part of the
file):

```jsonc
{
  "schema_version": 2,
  "generator": {"name": "hedgehog", "version": "0.3.0"},
  "source": {
    "path": "sales.xlsx", "file_name": "sales.xlsx", "sha256": "…", "size_bytes": 5251,
    "sheets": [{"name": "Sales", "used_range": "A1:E15", "cells": 39, "formulas": 0, "merges": 4,
                "tables": ["T1"]}]
  },
  "output": {"xlsx": "sales.normalized.xlsx", "manifest": "sales.manifest.json", "csv": [],
             "sheets": ["README", "T1_sales_report_2024", "Totals", "Notes"]},
  "options": {"keep_totals": false, "csv": false, "locale": null, "cell_roles": "model"},
  "time_coverage": null,                      // the span over all tables, see below
  "tables": [{
    "id": "T1",
    "sheet": "T1_sales_report_2024",          // output sheet
    "excel_table": "T1_sales_report_2024",    // ListObject name (= sheet name)
    "source": {
      "sheet": "Sales", "range": "A3:E13",    // full table range incl. header and total rows
      "header_top": 3, "header_rows": 2,      // header = rows 3..4
      "header_range": "A3:E4", "data_range": "A5:E13",
      "title": "Sales report 2024", "title_cell": "A1",
      "notes": [{"text": "Source: internal ERP export, amounts in USD", "cell": "A15"}]
    },
    "rows": 4,
    "confidence": 0.95,                       // table-detection confidence, 0..1
    "time_coverage": null,                    // or {"from", "to", "granularity", "source", "columns"}, see below
    "columns": [
      {"name": "Region", "index": 0, "letter": "A", "role": "group", "dtype": "text", "source_column": null, …},
      {
        "name": "Revenue / Q1", "index": 2, "letter": "C",   // position in the output sheet
        "role": "data",                       // data | group | source_row | row_type
        "dtype": "number",                    // integer | number | percent | currency | date | datetime | text | bool | empty
        "number_format": null,                // e.g. "0.00%" for percent
        "source_column": "B",                 // null for the group, _src_row and _row_type columns
        "original_header": [{"text": "Revenue", "cell": "B3"}, {"text": "Q1", "cell": "B4"}],
        "unit": null, "currency": null,       // e.g. "kg", "EUR"
        "locale": {"decimal": null, "date_order": null, "ambiguous": 0},
        "nulls": 0, "distinct": 4, "min": 300, "max": 2045, "example": 1200.5   // dates as ISO strings
      },
      …
      {"name": "_src_row", "index": 6, "letter": "G", "role": "source_row", "dtype": "integer", …}
    ],
    "group_column": "Region",                 // or null
    "source_row_column": "_src_row",
    "row_type_column": null,                  // "_row_type" with keep_totals
    "totals": {"count": 6, "ok": 5, "mismatch": 1, "unknown": 0,   // this table's totals
               "rows": [{"row": 8, "label": "Subtotal", "level": "subtotal", "kept_in_data": false}, …,
                        {"row": 13, "label": "Grand total", "level": "grand_total", "kept_in_data": false}]},
    "transformations": {"numbers_from_text": 7, "percent_from_text": 4, "missing_to_null": 1,
                        "headers_flattened": 2, "merged_cells_filled": 4, "total_rows_removed": 3,
                        "group_header_rows_to_column": 2, …},   // every key is always present
    "warnings": []
  }],
  "totals": {"sheet": "Totals", "count": 6, "ok": 5, "mismatch": 1, "unknown": 0,
             "items": [ …,
               {"table": "T1", "sheet": "Sales", "cell": "B13", "row": 13, "count": 1, "label": "Grand total",
                "level": "grand_total", "direction": "vertical", "origin": "typed",
                "value": 4600, "recomputed": 4525.5, "status": "mismatch", "reason": "mismatch",
                "sources": ["B6:B12"], "kept_in_data": false}, …]},
  "notes": {"sheet": "Notes", "count": 2, "by_kind": {"note": 1, "title": 1}},
  "cells": {"total": 39, "in_tables": 28, "in_totals": 9, "in_notes": 2},  // every source cell counted once
  "unassigned_cells": 0,                      // Notes rows not attached to any table
  "warnings": [],
  "load_hints": {"pandas": "…", "openpyxl": "…", "manifest": "…", "trace": "…"}
}
```

Field reference:

| Path | Type | Meaning |
|---|---|---|
| `schema_version` | int | `2` (1 in hedgehog 0.1, before total runs). Changes when the structure changes incompatibly. |
| `generator` | object | `name` and `version` of hedgehog. |
| `source` | object | Input file: `path` as given, `file_name`, `sha256`, `size_bytes`, and per sheet `name`, `used_range`, `cells`, `formulas`, `merges`, `tables` (ids). |
| `output` | object | File names written (relative to the output directory) and the sheet order of the xlsx. |
| `time_coverage` | object or null | Period the workbook's tables cover: the span over every table's `time_coverage`, at the finest granularity found, with `source` `column` if any table's coverage comes from column values, `columns` as `T<n>.<name>` and `tables` (ids). See [Time coverage](#time-coverage). |
| `options` | object | Options used. `locale` is the preset name (`"de"`, `"us"`, ...) or `null` when inferred. `cell_roles` is `"model"` (rules + learned model, the default) or `"rules"` (`--rules-only`); see [learned-model.md](learned-model.md). |
| `tables[].id` | string | `T1`, `T2`, ... in source order. |
| `tables[].sheet`, `excel_table` | string | Output sheet and Excel table name (equal). |
| `tables[].source` | object | Source sheet and ranges, header position, `title` and `title_cell`, attached `notes`. |
| `tables[].rows` | int | Data rows written (with `keep_totals`: including total rows). |
| `tables[].confidence` | float | Table-detection confidence, 0 to 1. |
| `tables[].time_coverage` | object or null | Period the table's data covers. See [Time coverage](#time-coverage). |
| `tables[].columns[]` | object | One entry per output column, in order. See below. |
| `tables[].group_column` | string or null | Name of the column made from group-header rows. |
| `tables[].source_row_column` | string | Always `_src_row`. |
| `tables[].row_type_column` | string or null | `_row_type` with `keep_totals`, else `null`. |
| `tables[].totals` | object | This table's totals: `count`, `ok`, `mismatch`, `unknown` (counted like the workbook `totals`), and `rows`: one entry per total row (row of a vertical total) with `row` (1-based source row), `label`, `level`, `kept_in_data` (`false`: the row is not in the table data; with `keep_totals` it is, marked in `_row_type`), `period` (the row's value in the table's `<name>_period` column when the row is in the data: `Ø 22` → `"2022"`, the year's figure; else `null`) and `sources_outside` (source cells of the total outside its own table, e.g. `["D10"]` for a grand total under stacked blocks that adds the block above's subtotal). |
| `tables[].transformations` | object | Counts of every change made. See [Transformations](#transformations). |
| `tables[].warnings` | list of strings | Values kept as text in a typed column, values read with a guessed locale, and similar. |
| `totals` | object | Summary counts and one item per row of the `Totals` sheet (same fields plus `row`, the 1-based source row of `cell` (the first row of a run); `label_cell` and `formula` only on the sheet). |
| `notes` | object | Row count of the `Notes` sheet and counts by `kind`. |
| `cells` | object | Every non-empty source cell counted once: in a table, in a total, or in Notes. |
| `unassigned_cells` | int | Notes rows not attached to any table. |
| `warnings` | list of strings | Workbook-level warnings, for example a sheet above the model's size limit whose roles came from the rules alone. |
| `load_hints` | object | The loading snippets from the README sheet, as strings. |

Column fields:

| Field | Meaning |
|---|---|
| `name`, `index`, `letter` | Header text and position in the output sheet (0-based index, Excel letter). |
| `role` | `data` (from a source column), `group` (from group-header rows), `source_row` (`_src_row`), `row_type` (`_row_type`). |
| `dtype` | See [Values and dtypes](#values-and-dtypes). Authoritative: trust it over pandas' guess. |
| `number_format` | Excel number format written for the column, or `null`. |
| `source_column` | Source column letter, or `null` for non-data roles, derived columns and row labels copied from another table (`row_labels_copied`; see [concepts](detection-rules.md#tables)). |
| `derived` | `null`, or `{"from": "<column>", "kind": "period" \| "key"}` for a [derived column](#rows). |
| `original_header` | The header cells joined into `name`, top to bottom, with their addresses. |
| `unit`, `currency` | Unit or ISO currency code carried by more than half of the numeric values (the most common one), or `null`. Mixed currencies are also listed in `warnings`. |
| `locale` | `decimal` (`"."`, `","` or `null`), `date_order` (`"dmy"`, `"mdy"` or `null`), and `ambiguous`: how many values were read with a guess. `null` means no evidence. |
| `footnote_marks` | `null`, or one entry per footnote mark removed from the column's numbers or text labels: `{"mark": "¹", "count": 3, "cells": ["B5", "B9", "B12"]}` (source cells, at most 100 listed; `count` is complete). Marks are `*`, `†`, superscript digits, `(r)`-style parentheses and statistical flag letters (`p`, `e`, `bep`). |
| `nulls`, `distinct` | Missing values and distinct non-missing values. |
| `min`, `max`, `example` | For numbers and dates (dates as ISO strings); `example` is the first non-missing value. |

### Time coverage

`tables[].time_coverage` and the workbook-level `time_coverage` say which period the data covers,
so an agent can answer "the file has 2015–2017" (or refuse a question about 2025) without a query.
`null` when nothing period-like was found.

```jsonc
"time_coverage": {"from": "2020-01", "to": "2025-12", "granularity": "month",
                  "source": "column", "columns": ["Berichtszeitraum"]}
```

| Field | Meaning |
|---|---|
| `from`, `to` | First and last period, written in its granularity: `2017` (year), `2024-Q1` (quarter), `2024-03` (month), `2024-W05` (ISO week), `2024-03-31` (day). The workbook span may mix forms (`"from": "1950", "to": "2025-12"`). |
| `granularity` | `year`, `quarter`, `month`, `week` or `day`: the finest found. |
| `source` | `column`: values of date / period columns. `header`: period-like column headers of a wide table. |
| `columns` | Names of the columns the coverage was read from. |

Found, in this order (header periods only when no column gives one):

- **Date columns** (`date` / `datetime` dtype): the span of their values. Granularity from the
  values: all on the 1st of a month gives `month` (all in January: `year`; all in Jan/Apr/Jul/Oct:
  `quarter`), anything else `day`.
- **Year columns**: an `integer` column whose values are all 1900–2100 and whose name says year
  (`Year`, `Jahr`, `Berichtsjahr`, `année`, `año`, `anno`, `år`, `rok`, `jaar`, `godina`, `FY`, ...),
  or a text / group column of bare years with such a name (or years as group-header rows).
- **Period text columns**: at least 90% of the distinct values read as a period by the value parsers
  (`2024 Q1`, `Q3 2024`, `Jan 2024`, `März 2024`, `2024-03`, `2024-W05`, `KW 5/2024`; months in nine
  languages), or as the labels a `<name>_period` column is made from (`Ø 22`, `Mar-May 1971`,
  `Apr.201`). Days all on the 1st of a month count as months. The derived `<name>_period` column is
  listed in `columns` after its label column.
- **Period headers**: a header part that is a period (`Jan 2024`, `2024 Q1`), a month or quarter name
  with a year in another part (`Oktober / 2022`), or a year, alone or with one short word
  (`2019`, `RA 2023`, `2023 Ist`). Two or more such columns are needed when they are all bare years.

Month names without any year (`Januar`, `Februar`, ... as values or headers) give no coverage.

## Transformations

Every key below is present in every table's `transformations`, with a count (often 0).

| Key | Counts |
|---|---|
| `numbers_from_text` | Numbers parsed from text (`"1,200"`) |
| `percent_from_text` | Percentages parsed from text (`"12%"`) |
| `currency_stripped`, `unit_stripped` | Values whose currency symbol/code or unit was removed |
| `scale_applied` | A scale word multiplied in (`1,5 mil.` becomes 1500000) |
| `accounting_negatives` | `(500)` or `500-` read as -500 |
| `footnote_marks_stripped` | Numbers read from text with a footnote mark removed (`123¹`, `12 (r)`, `7 p`), and text labels with one removed (`Delta²`); marks per column in `footnote_marks` |
| `dates_from_text` | Dates parsed from text |
| `dates_from_serial_numbers` | A plain number in a date column read as an Excel serial date |
| `bools_from_text` | Booleans parsed from text |
| `missing_to_null`, `errors_to_null` | Missing tokens and Excel errors written as empty cells |
| `formulas_without_value` | Formula cells with no cached value (written empty) |
| `formulas_replaced_by_values` | Data cells that held a formula; their cached value was written |
| `whitespace_trimmed` | Text values trimmed or with whitespace collapsed |
| `values_to_text` | A stored number, date or bool in a text column written as text |
| `unparsed_kept_as_text` | Values that did not fit their column's dtype (also in `warnings`) |
| `text_truncated` | Text cut to Excel's cell limit |
| `headers_flattened` | Header rows joined into the flat header |
| `headers_cleaned` | Column names changed by header cleanup (footnote marks, line-break hyphenation; see [Header](#header)) |
| `header_cells_forward_filled` | Blank header cells that took the parent label on their left |
| `merged_cells_filled` | Header cells covered by a merge, plus body cells under a vertical merge (a body merge fills down its first column only) |
| `duplicate_headers_renamed`, `missing_headers_named` | Header fixes |
| `total_rows_removed`, `total_rows_kept` | Total rows moved out of (or kept in) the data |
| `repeated_header_rows_removed`, `blank_rows_removed` | Rows dropped from the body |
| `group_header_rows_to_column` | Group-header rows turned into the group column |
| `note_rows_moved`, `title_rows_moved` | Rows inside the table range moved to `Notes` |
| `empty_columns_removed` | Columns with neither header nor values |
| `row_labels_copied` | Label columns copied from a table beside or above whose rows this table repeats (a daily grid under a summary that names the rows once) |
| `period_columns_added`, `key_columns_added`, `parent_columns_added` | Derived `<name>_period`, `<name>_key` and `<name>_parent` columns |

## Repeatable output

The same input file, options and hedgehog version give the same bytes in every run: the normalized
xlsx, the manifest and the CSVs, on any number of threads (`--threads`, or a single CPU) and on any
day. Finance users can diff or hash two runs and get no difference.

- **Run metadata: none.** No output field holds a time, host name, user, process id or temp path,
  so nothing has to be masked before comparing. The xlsx document properties (`docProps/core.xml`,
  created / modified) carry a fixed date, 1980-01-01T00:00:00Z, the same as the zip entries'
  timestamps. The CLI's console line (`… in 16 ms`) and `HEDGEHOG_TRACE` timings go to the terminal
  only, never into a file.
- **Fields that follow the invocation, not the run:** `source.path` (the path as given; the same
  file opened through another path gives another value), the output file names (from the source
  file's stem) and `generator.version`. Everything else depends on the file's bytes and the options.
- **Time limits:** with a deadline (`NormalizeOptions::deadline`, `hedgehog mcp --max-seconds`) a
  run either finishes with the full output or fails with `file refused: … time limit`; a slow
  machine never yields a partial result.
- **Across versions** the output can change (that is what `generator.version` is for).

`fuzz/determinism.py` checks this over a folder or file list: it normalizes each file several times
(threaded and, with `--single N`, pinned to one CPU) and compares the manifest and every part of the
xlsx. `core/tests/determinism.rs` guards it in the test suite.

## Known limits

- Table detection, header depth and totals come from the rule-based `layout` and `totals` modules
  (see [evaluation.md](evaluation.md) for their measured accuracy). A table they miss ends up cell by
  cell on `Notes`, with kind `other` or `title`.
- A typed (non-formula) total row is recognized by its total label in one of the languages listed
  in [locales.md](locales.md#total-and-note-keywords), or, without one, only when its numbers add up
  to the rows above it in at least two columns. A total in another language, or an aggregate that is
  no plain sum of the rows around it (a region in a country list without World Bank codes), stays
  in the data as an ordinary row. Check the `Totals` sheet before aggregating.
- Totals are recomputed the way Excel computes them, so numbers stored as text in the source range
  are not added, and a total row over a column of text numbers is not detected as a total. A wrong
  source-range guess shows up as `unknown` / `unconfirmed`, not as `mismatch`. A typed total that does
  not add up but whose header or label names a rate, share or index, that is off by rounding only,
  or that lies between its smallest and largest source value is `unknown` (`not_additive`,
  `rounding_difference`, `average_like`), not `mismatch`; such rows are still moved out of the data
  like other total rows (use `keep_totals` to keep them).
- Values only: formulas are not recreated in the output. Each one is listed on `Totals` (for totals)
  and counted in `formulas_replaced_by_values`.
- Cell styles (xlsx / xlsm only) are read for the learned model's features, not used for typing:
  a percentage stored as a number, such as 0.12 shown as `12%` in Excel, stays a `number` unless
  the column also has `%` text.

## See also

- [agent-recipes.md](agent-recipes.md): pandas and openpyxl code that works with this output
- [concepts.md](concepts.md): tables, cell roles, totals and provenance explained
- [locales.md](locales.md): how numbers and dates are read
- [cli.md](cli.md#hedgehog-normalize) and [python-api.md](python-api.md#hedgehognormalize)
