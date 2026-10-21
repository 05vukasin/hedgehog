---
name: spreadsheets
description: Use FIRST, before open_file or query, when a question needs numbers or facts from a spreadsheet (xlsx, xls, ods, csv). Rules the tools leave out - entity IDs, total rows, what one row is, verifying, citing cells.
---

# Spreadsheets with hedgehog

hedgehog's tools turn a messy sheet into clean SQL tables in seconds and describe them in a short
catalog. Use them before anything else; they are faster, cheaper and safer than reading the file
with Python.

## Steps

1. **`open_file` first** (path relative to the project folder; `list_files` finds files). Read the
   whole catalog before querying. Trust its lines, they were computed over every row:
   - `TIME`: the period the file covers. A question outside it cannot be answered from this file.
   - table head line: what the table is, how many rows, which **total rows** were taken out (only in
     `T<n>_all`) and which stay in the data ("exclude before summing").
   - column ranges and value lists: use the exact labels shown in `WHERE`.
   - `rows mix measures by "<col>"`: filter on that column before any `SUM`/`AVG`.
   - quality / entity / ID lines (duplicates, empty dates, negative amounts, one entity under
     several spellings): follow them. When it says to group by `<col>_id`, group by the ID column
     and show one name per ID.
   - small tables come with all their rows as CSV: answer from those rows directly.
2. **Numbers come from `query`** (SQLite: `strftime('%m', d)` not `MONTH(d)`, `LIMIT` not `TOP`,
   double-quoted column names: `SELECT "Revenue / Q1" FROM T1`). One query per sub-question; let
   SQL do sums, counts, top-N and shares. `describe_table` gives exact column names of a wide
   table.
3. **Know what one row is before summing**: a contract, a day, a month, a subtotal? Count by
   `COUNT(*)`, value by `SUM(<amount>)`; "most contracts" means by count. Overall figures the file
   states itself are rows of `T<n>_all` with `_row_type = 'total'`, never added to the data rows.
4. **Verify a surprising result** (a total that is far off, a sign flip, a missing month) with a
   second query from another angle (row count, the file's own total row, `totals`, or `cells`).
5. **Cite** the source cell of key numbers: `Sheet!<source column><_src_row>` (query results with
   `_src_row` print a `cite as:` line), or quote cells with `cells`.
6. **Fall back to Python/pandas only** for what the tools cannot do (charts, writing files, very
   custom parsing), and say why.

Answer every sub-question, give the numbers with units, and say when the file cannot answer
something.
