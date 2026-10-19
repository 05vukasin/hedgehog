# Normalize

`normalize` turns a messy workbook into a clean, typed workbook plus a JSON manifest. The output is
meant for an agent that has a code sandbox: it loads each table with one `pandas.read_excel` call
and can trace every value back to its source cell. The source file is only read, never modified.

## Normalized output

### Running it

```sh
hedgehog normalize in.xlsx -o out/ [--keep-totals] [--csv] [--locale de-DE]
```

```python
import hedgehog

r = hedgehog.normalize("in.xlsx", "out/", keep_totals=False, csv=False, locale=None)
r.xlsx_path        # out/in.normalized.xlsx
r.manifest_path    # out/in.manifest.json
r.manifest         # the manifest as a dict
r.tables           # r.manifest["tables"]
r.table("T1")      # one table's manifest entry, by id or sheet name
df = r.read("T1")  # pandas DataFrame (needs pandas + openpyxl)
```

Rust: `hh_core::normalize_file(path, out_dir, &hh_core::normalize::Options { keep_totals, csv, locale })`.
`hh_core::normalize_workbook(&wb, &opts)` gives the same result in memory without writing files.

- `--keep-totals` / `keep_totals=True`: total and subtotal rows stay in the table sheets, marked
  in a `_row_type` column. By default they are removed from the data and listed only on `Totals`.
- `--csv`: also write `out/<stem>.<sheet>.csv` for every table.
- `--locale`: `en-US`, `en-GB`, `de-DE`, `fr`, `es`, `it`, `nl`, `pt-BR`, `pl`, `de-CH`, `sr`, `hr`,
  `iso`, ... This sets the decimal separator and day/month order for every column. Without it,
  both are inferred per column, falling back to the sheet-wide vote. The locale only settles
  ambiguous text such as `1.200` or `03/04/2024`; unambiguous text like `1,200.50` is always read
  as written.

### Files

| File | Content |
|---|---|
| `<stem>.normalized.xlsx` | `README`, one sheet per table, `Totals`, `Notes` (in this order) |
| `<stem>.manifest.json` | machine-readable description of all of the above (schema below) |
| `<stem>.<sheet>.csv` | only with `--csv`: UTF-8, RFC 4180, ISO dates, percentages as fractions, empty = missing |

### Sheets of the normalized workbook

**`README`**: what the file is, the source file name and SHA-256, one line per table (sheet, source
sheet, source range, rows, columns, title), and copy-paste loading code.

**`T<n>_<slug>`**: one sheet per detected table, numbered in source order (sheet by sheet, top to
bottom, left to right). The slug comes from the table title, or from the source sheet name when
there is no title. It is lowercase ASCII: Cyrillic is transliterated, diacritics are folded, and
the whole sheet name is at most 31 characters. The data is registered as an Excel table
(ListObject) with the **same name as the sheet**, so `ws.tables["T1_sales"]` works in openpyxl.
The table has an autofilter, row 1 is frozen and column widths are set.

- **Row 1** is one flat header row, and every name in it is unique and non-empty:
  - Multi-row headers are joined top to bottom with ` / ` (`Revenue / Q1`). Before joining,
    merged header cells fill their whole area, and a parent label written once over several
    labelled columns is copied right (`2024` over `Plan | Actual` gives `2024 / Plan`,
    `2024 / Actual`).
  - A duplicate name gets ` (2)`, ` (3)`, ... (Excel compares names case-insensitively).
  - A column without a header is named `col_<source letter>`, e.g. `col_F`.
  - Names longer than 255 characters are cut.
- **Data rows only.** Header, title, note, blank and repeated-header rows are removed. Total and
  subtotal rows are removed too, unless `keep_totals` is set.
- **Typed values.** The source values are written, not the source formulas.

  | dtype | written as | number format |
  |---|---|---|
  | `integer`, `number` | number | General |
  | `percent` | fraction (`12%` → `0.12`) | `0.00%` |
  | `currency` | number (symbol or code stripped, see `currency` in the manifest) | `#,##0.00` |
  | `date` / `datetime` | Excel date | `yyyy-mm-dd` / `yyyy-mm-dd hh:mm:ss` |
  | `bool` | TRUE / FALSE (`yes/no`, `da/ne`, `✓/✗` included) | |
  | `text` | text, trimmed, whitespace runs collapsed to one space | |
  | `empty` | (column has a header but no values) | |

  A column gets a numeric, date or bool dtype when at least 90 % of its non-missing values read
  that way, either as stored values or as text parsed with the column's locale. Text numbers
  with leading zeros (`00123`) keep the column as text, because they are IDs. A value that does
  not fit the column's dtype is **kept as text**, and the table's warnings list its source cell.
  Nothing is turned into null silently. In a text column, stored numbers and dates become their
  text form (`2024`, `2024-03-31`).
- **Missing values are empty cells.** This covers missing tokens (`-`, `n/a`, `..`, `nema`, `x`,
  ...), Excel errors (`#N/A`, `#REF!`, ...) and formulas without a cached value.
- **Group-header rows** (one label on a row by itself, followed by data rows) become a **leading
  column**. If every label has the form `Prefix: value` with the same prefix, the column is named
  after the prefix (`Region: North` → column `Region`, value `North`). Otherwise the column is
  named `group` and holds the full label. Nested group labels are joined with ` / `.
- **`_src_row`** (always the last column, or the one before `_row_type`) is the **1-based row
  number in the source sheet**. A value in output column `c` came from source cell
  `<columns[c].source_column><_src_row>` on `source.sheet` of the manifest entry.
- **`_row_type`** (only with `keep_totals`): `data`, `subtotal` or `total`.

**`Totals`**: one row per total or subtotal cell, also registered as an Excel table named `Totals`.
Columns:

| column | meaning |
|---|---|
| `table` | table id (`T1`) whose range contains the cell, empty if none |
| `sheet`, `cell` | source sheet and address of the total |
| `label`, `label_cell` | text that labels it (`Grand total`) and where it is |
| `level` | `total`, `subtotal` or `grand_total` |
| `direction` | `vertical` (adds the cells above) or `horizontal` (adds the cells to its left) |
| `origin` | `formula` (SUM / SUBTOTAL / `A+B+C`), `typed` (a number in a total-labelled row), or `layout` (a number in a row the layout marked as a total, but whose source range was not found) |
| `value` | value in the file (cached result for formulas) |
| `recomputed` | sum of the source cells, recomputed from the cached values |
| `status` | `ok`, `mismatch` or `unknown` |
| `reason` | `match`, `rounded`, `mismatch`, `double_counted`, `matches_average`, `no_cached_value`, `not_numeric`, `error_in_source`, `missing_source_value`, `no_source`, `unconfirmed`, `not_verified` |
| `sources` | source ranges, e.g. `C7:C12` |
| `formula` | the formula, if any |
| `kept_in_data` | the total is also in the table data: `keep_totals` is set, or it is a total column (horizontal) |

Recomputation follows Excel's rules, so numbers stored as text in the source range are not summed.

**`Notes`**: every source cell that is neither table data nor a total, with its address. Nothing
is dropped silently. Columns are `sheet`, `cell`, `kind`, `table`, `text` and `formula`. `kind` is
one of:

- `title`: a title above a table
- `note`: a note or footnote, such as `Source: ...` or `* preliminary`
- `repeated_header`: a header row repeated inside the body
- `total_row`: text in a removed total row other than its label
- `total`: an aggregate outside any table
- `other`: a cell that belongs to no table

`table` is set when a title or note could be attached to a table: the nearest table below a
title, or above a note, within 6 blank rows, with overlapping columns.

### Manifest (`<stem>.manifest.json`, `schema_version` 1)

The manifest is deterministic: the same input gives the same bytes. Keys have a fixed order and
there are no timestamps, so diffs between two runs are meaningful. Rows are 1-based and columns
are letters, as in Excel.

```jsonc
{
  "schema_version": 1,
  "generator": {"name": "hedgehog", "version": "0.1.0"},
  "source": {
    "path": "in.xlsx", "file_name": "in.xlsx", "sha256": "…", "size_bytes": 6904,
    "sheets": [{"name": "Sales", "used_range": "A1:E15", "cells": 39, "formulas": 0, "merges": 4, "tables": ["T1"]}]
  },
  "output": {"xlsx": "in.normalized.xlsx", "manifest": "in.manifest.json", "csv": [],
             "sheets": ["README", "T1_sales_report_2024", "Totals", "Notes"]},
  "options": {"keep_totals": false, "csv": false, "locale": null},
  "tables": [{
    "id": "T1",
    "sheet": "T1_sales_report_2024",          // output sheet
    "excel_table": "T1_sales_report_2024",    // ListObject name (= sheet)
    "source": {
      "sheet": "Sales", "range": "A3:E13",    // full table range incl. header and total rows
      "header_top": 3, "header_rows": 2,      // header = rows 3..4
      "header_range": "A3:E4", "data_range": "A5:E13",
      "title": "Sales report 2024", "title_cell": "A1",
      "notes": [{"text": "Source: internal ERP export", "cell": "A15"}]
    },
    "rows": 4,
    "confidence": 0.95,                       // table-detection confidence 0..1
    "columns": [{
      "name": "Revenue / Q1", "index": 2, "letter": "C",   // position in the output sheet
      "role": "data",                         // data | group | source_row | row_type
      "dtype": "number",                      // integer|number|percent|currency|date|datetime|text|bool|empty
      "number_format": null,
      "source_column": "B",                   // null for group/_src_row/_row_type
      "original_header": [{"text": "Revenue", "cell": "B3"}, {"text": "Q1", "cell": "B4"}],
      "unit": null, "currency": null,         // e.g. "kg", "EUR"
      "locale": {"decimal": ".", "date_order": "mdy", "ambiguous": 0},  // "," / "dmy"; null = no evidence
                                              // (inferred for number/date columns only; `ambiguous` = values read by a guess)
      "nulls": 1, "distinct": 3, "min": 50, "max": 1200.5, "example": 1200.5   // dates as ISO strings
    }],
    "group_column": "Region",                 // or null
    "source_row_column": "_src_row",
    "row_type_column": null,                  // "_row_type" with keep_totals
    "transformations": {                      // counts; every key is always present
      "numbers_from_text": 5, "percent_from_text": 3, "currency_stripped": 0, "unit_stripped": 0,
      "scale_applied": 0, "accounting_negatives": 0, "dates_from_text": 3,
      "dates_from_serial_numbers": 0, "bools_from_text": 0, "missing_to_null": 2,
      "errors_to_null": 0, "formulas_without_value": 0, "formulas_replaced_by_values": 0,
      "whitespace_trimmed": 1, "values_to_text": 0, "unparsed_kept_as_text": 0, "text_truncated": 0,
      "headers_flattened": 2, "header_cells_forward_filled": 0, "merged_cells_filled": 4,
      "duplicate_headers_renamed": 0, "missing_headers_named": 0, "total_rows_removed": 3,
      "total_rows_kept": 0, "repeated_header_rows_removed": 0, "blank_rows_removed": 0,
      "group_header_rows_to_column": 2, "note_rows_moved": 0, "title_rows_moved": 0,
      "empty_columns_removed": 0
    },
    "warnings": ["column 'Amount' (number): 1 value(s) are not numbers and were kept as text: B7"]
  }],
  "totals": {"sheet": "Totals", "count": 6, "ok": 1, "mismatch": 1, "unknown": 4,
             "items": [{"table": "T1", "sheet": "Sales", "cell": "C13", "label": "Grand total",
                        "level": "grand_total", "direction": "vertical", "origin": "typed",
                        "value": 1999, "recomputed": 330, "status": "mismatch", "reason": "mismatch",
                        "sources": ["C7:C12"], "kept_in_data": false}]},
  "notes": {"sheet": "Notes", "count": 3, "by_kind": {"note": 1, "other": 1, "title": 1}},
  "cells": {"total": 75, "in_tables": 63, "in_totals": 9, "in_notes": 3},  // every source cell counted once
  "unassigned_cells": 1,                      // Notes rows not attached to any table
  "warnings": [],
  "load_hints": {"pandas": "…", "openpyxl": "…", "manifest": "…", "trace": "…"}
}
```

Transformation meanings, where the name is not self-explanatory:

- `scale_applied`: a scale word was multiplied in (`1,5 mil.` → 1500000).
- `accounting_negatives`: `(500)` or `500-` was read as -500.
- `dates_from_serial_numbers`: a plain number in a date column was read as an Excel serial.
- `formulas_replaced_by_values`: data cells that held a formula. Their cached value was written.
- `values_to_text`: a stored number, date or bool in a text column was written as text.
- `header_cells_forward_filled`: blank header cells that took the parent label on their left.
- `merged_cells_filled`: header cells covered by a merge, plus body cells under a vertical merge.
  A body merge fills down its first column only.

### Recipes for agents

```python
import json, pandas as pd
m = json.load(open("out/in.manifest.json", encoding="utf-8"))
for t in m["tables"]:
    df = pd.read_excel("out/in.normalized.xlsx", sheet_name=t["sheet"])
    src = t["source"]["sheet"]
    letters = {c["name"]: c["source_column"] for c in t["columns"]}
    # cite a value: f"{src}!{letters[col]}{df.loc[i, '_src_row']}"
bad = [x for x in m["totals"]["items"] if x["status"] == "mismatch"]
```

- With `keep_totals`, filter with `df[df["_row_type"] == "data"]` before aggregating. This avoids
  summing the subtotals twice.
- `dtype` in the manifest is authoritative. If a numeric column holds some text, the table's
  `warnings` lists those cells; use `pd.to_numeric(df[col], errors="coerce")` to drop them on
  purpose.

### Known limits

- Table detection, header depth and totals come from the rule-based `layout` and `totals`
  modules. A table they miss ends up cell by cell on `Notes`, with kind `other` or `title`.
- Totals are recomputed the way Excel computes them, so numbers stored as text in the source
  range are not added. A wrong source-range guess shows up as `unknown` / `unconfirmed`, not as
  `mismatch`.
- Values only: formulas are not recreated in the output. Each one is listed in `Totals` (for
  totals) and counted in `formulas_replaced_by_values`.
- Cell styles are not read (calamine gives none). A percentage stored as a number, such as 0.12
  shown as `12%` in Excel, stays a `number` unless the column also has `%` text.
