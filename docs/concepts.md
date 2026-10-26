# Concepts

The ideas behind hedgehog's output, in the order hedgehog meets them while reading a file. Each
section says what the idea is, shows it on the sample report from the
[introduction](introduction.md), and says where it appears in the output. The exact rules behind each
step are in [detection-rules.md](detection-rules.md); you don't need them to use hedgehog.

The sample file, `sales.xlsx` (the [quickstart](quickstart.md) has the script that writes it):

```
     A                 B         C       D       E
 1   Sales report 2024
 2
 3   Product           Revenue           Units   Margin
 4                     Q1        Q2
 5   Region: North
 6   Widgets           1200.5    1350    1,200   12%
 7   Gadgets           300       410     350     8.5%
 8   Subtotal          1500.5    1760
 9   Region: South
10   Widgets           980       1150    n/a     10%
11   Gizmos            2045      1720    2,100   15%
12   Subtotal          3025      2870
13   Grand total       4600      4630
14
15   Source: internal ERP export, amounts in USD
```

## 1. Tables

A sheet is a grid of cells, but the data in it comes in tables. hedgehog's first job is to find
them: where each table starts and ends, and which cells around it are titles or notes rather than
data.

One sheet can hold several tables: side by side, stacked one under another, or a small summary
next to a long list. hedgehog splits them, and numbers them through the whole file: `T1`, `T2`,
and so on. In `sales.xlsx` there is one table, `T1`, covering `A3:E13`. Row 1 is its title and
row 15 a note.

Every table gets a confidence between 0 and 1. A cell hedgehog cannot place in any table is never
dropped. It goes to the notes with its address (see [Nothing is lost](#7-nothing-is-lost)).

## 2. Cell roles

Inside and around a table, every filled cell gets one role:

| Role | Meaning | In `sales.xlsx` |
|---|---|---|
| `title` | A title above a table | A1 `Sales report 2024` |
| `header` | A column name, possibly one of several header rows | A3 `Product`, B4 `Q1` |
| `data` | A value or row label in the table body | A6 `Widgets`, B6 `1200.5` |
| `group_header` | A label alone on its row that starts a section | A5 `Region: North` |
| `total` | A total or subtotal, its label or its value | A8 `Subtotal`, B13 `4600` |
| `note` | A note, footnote or source line | A15 `Source: …` |
| `other` | Anything else | a stray cell far from any table |

Two things decide the roles. Hand-written rules look at the layout: blank rows, merged cells, text
above numbers, words like "Total" or "Source". A small learned model then checks each row of each
table and corrects the rules where they are unsure. The model is a set of decision trees built into
the library; it needs no GPU and gives the same answer every time. See
[learned-model.md](learned-model.md).

## 3. Headers

A column name is often spread over several rows. In the sample, "Revenue" sits over two columns and
"Q1", "Q2" sit below it. hedgehog joins the levels with ` / `:

```
 Product │ Revenue        │ Units          Product │ Revenue / Q1 │ Revenue / Q2 │ Units
         │  Q1   │  Q2    │          ──▶
```

So every column has one name you can use in code or SQL. Empty, repeated and very long names are
fixed (`Code (2)`, `col_C`), and the original header cells of every column are kept with their
addresses, so you can always see what the file said.

## 4. Row labels and groups

A row of numbers only means something together with its labels. Spreadsheets often write a label
once and let the reader carry it down:

- **Group rows.** `Region: North` on its own row means "the rows below are North". hedgehog turns
  it into a column, `Region`, filled in on every row of the group.
- **Outlines.** An indented list (`Europe`, then `Germany` and `France` indented under it) gets a
  `<label>_parent` column, so each row knows what it belongs to.
- **Labels written once per block.** A name in the first row of a block of rows is copied down to
  the other rows of that block.

After this step, every row can be read on its own: `North | Widgets | 1200.5 | 1350 | 1200 | 0.12`.

## 5. Values

Cells often hold numbers as text, in a format that depends on the country:

| In the file | hedgehog reads |
|---|---|
| `1,200` in a US-style column | 1200 |
| `1.200,50` in a German-style column | 1200.5 |
| `12%` | 0.12 (stored as a fraction) |
| `(500)` | -500 |
| `31.12.2024`, `12/31/24`, `March 2024` | a date |
| `n/a`, `-`, `#N/A` | empty |
| `00123` in an ID column | the text `00123`, leading zeros kept |

hedgehog judges each column as a whole: `1.200` means one thousand two hundred in a column of
German numbers and one point two in a column of US numbers. When the column does not settle it,
hedgehog reports the doubt instead of guessing. Each column gets one type (integer, number, date,
text, boolean), and values that don't fit it stay as text and are reported. The details are in
[locales.md](locales.md).

## 6. Totals

Total rows are the most common trap in spreadsheet data. If they stay in the table, every sum counts
the same numbers twice: in the sample, adding all of column B with the subtotals and the grand total
gives 13,651 instead of 4,525.5.

hedgehog finds total rows by their formulas (`SUM(B6:B7)`), by their labels (`Total`, `Subtotal`,
`Summe`, `Ukupno`, `Totaal` and many more), and by their numbers (a row that equals the sum of the
rows above it). It takes them out of the data, keeps them separately, and recalculates each one:

- `ok`: the total adds up (B8, 1500.5 = B6 + B7);
- `mismatch`: it doesn't (B13 says 4600, the cells add up to 4525.5);
- `unknown`: there is nothing to check it against.

Rows that look like totals but are not sums (an "Average" row, an "of which: exports" row, a
weighted index) are recognized and kept apart too, so nobody adds them.

## 7. Nothing is lost

Every filled cell of the source ends up in exactly one place: in a table's data, among its totals,
or in the notes (titles, notes, sources and loose cells, with their addresses). The output counts
them, and the counts always add up to the number of cells in the file.

Every value can be traced back to its source cell. Each table row carries `_src_row`, its row
number in the source sheet, and each column knows its source column. In SQL results this becomes a
`cite as` line:

```text
cite as: Product = Sales!A<_src_row>; "Revenue / Q1" = Sales!B<_src_row>
```

## 8. What an agent sees: the catalog and SQL

An agent that opens a file gets the **catalog**: a short text that describes every table. For
`sales.xlsx` it starts like this:

```text
FILE sales.xlsx · 1 sheet · 39 cells (94% in tables) · 1 table
TIME 2024 (only mentioned in titles/notes/sheet or file names; no date or period columns)
T1  'Sales report 2024' ← Sales!A3:E13 · 4 rows (+3 total rows only in T1_all)
    columns (6): Region[group] Product "Revenue / Q1"[300..4600] "Revenue / Q2"[410..4630] Units[350..2100] Margin[percent rate 0.085..0.15]
    do not sum: Margin (rate) — use AVG, a weighted mean or the file's total
    group column: Region · totals: 1 mismatch (call totals)
```

The catalog tells the agent what one row is, which columns it can add up and which it can't, where
the totals are and what time the data covers. Small tables are included in full. On a large file
the catalog stays short: it lists columns and value ranges, not rows. When the host passes the
user's question, the tables the question is about come first.

Then the agent asks for numbers with **SQL**. Each table is a read-only database table:

- `T1`: the data rows only. A plain `SUM` over it is right.
- `T1_all`: the data rows plus the total rows, with a `_row_type` column (`data`, `subtotal`,
  `total`, …), for when the agent wants the file's own totals.
- `T1_long`: for a table with many period columns (`2011` … `2025`), the same data with one row per
  period, so the agent filters `WHERE period = '2025'` instead of picking a column.

```sql
SELECT SUM("Revenue / Q1") FROM T1 WHERE Region = 'North'   -- 1500.5
```

When a query goes wrong, the error says how to fix it: a misspelled column gets the closest
names, a value that isn't in the column gets the values that are. The tools and every catalog line
are described in [mcp.md](mcp.md).

## 9. What a program sees: the clean workbook

`hedgehog normalize` writes the same result as files: a workbook with one sheet per table (one
header row, typed values, `_src_row`), a `Totals` sheet with every total and its check, a `Notes`
sheet with every other cell, and a JSON manifest that describes all of it: each column's type, its
original header cells, what was changed and how many cells went where. See
[normalized-output.md](normalized-output.md).

## 10. Same file, same result

hedgehog uses no randomness, no network and no AI model while reading. The same file gives
byte-identical output on every machine, so a difference between two runs is always a real change.
The source file is opened read-only and never written.

## See also

- [detection-rules.md](detection-rules.md): every rule behind tables, roles, headers and totals
- [mcp.md](mcp.md): the agent tools, the catalog and the SQL views in full
- [normalized-output.md](normalized-output.md): every output sheet and manifest field
- [evaluation.md](evaluation.md): how accurate all of this is, measured
