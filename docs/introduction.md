# What hedgehog is

hedgehog reads messy spreadsheets so that programs and AI agents can use them. This page explains the
problem it solves, what it gives you back and how a question about a spreadsheet gets answered. It
takes about ten minutes to read; after it, [concepts.md](concepts.md) explains the ideas one by one.

## The problem

Spreadsheets are made for people to look at. A typical report from a finance team or a statistics
office looks like this:

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

A person sees one table at a glance. A program sees 39 loose cells, and it has to work out a lot
before it can answer anything:

- the title on row 1 is not a column name;
- the header takes two rows, so the second column is "Revenue, Q1";
- "Region: North" is not a product, it says which region the rows below belong to;
- the subtotal and grand total rows must not be added to the data, or every sum comes out double;
- `1,200` and `12%` are numbers stored as text, and `n/a` means "no value";
- the grand total in B13 is wrong: B6, B7, B10 and B11 add up to 4,525.5, not 4,600.

Load this file with `pandas.read_excel` and you get a broken table. Give the cells to a language
model and it often guesses wrong: it adds the subtotal rows, reads a value from the wrong row, or
does arithmetic it is bad at. On a big file, the cells alone cost more tokens than a model can take.

## What hedgehog does

hedgehog does the reading work before anyone asks a question. It is written in Rust, needs no
internet and uses no AI model for this: the same file always gives the same result. For each
workbook it works out:

- where every table is, even when a sheet holds several, side by side or stacked;
- which rows are the header, and what each column is called (`Revenue / Q1`);
- which label each row belongs to (`Region` = `North`);
- which rows are totals, and whether they add up;
- what every value is: a number, a percentage, a date, a code, an empty value.

Then it gives you the result in one of two forms.

**For AI agents: a catalog and SQL.** The agent calls `open_file` and gets a short description of
the workbook, usually a few hundred tokens, that lists every table, its columns, what one row is,
which rows are totals and what to watch out for. Then it asks for numbers with SQL queries over the
cleaned tables. The model never has to copy numbers out of a long dump, and every result says which
source cells it came from. This is the main way hedgehog is used. You run it as an
[MCP server](mcp.md), as a [Claude Code plugin](mcp.md#claude-code), or call the same tools from
[Python](python-api.md) and [Node.js](node-api.md).

**For programs: a clean workbook.** `hedgehog normalize` writes a copy of the file with one tidy
sheet per table, plus a JSON manifest that says what was found and changed. Load it with pandas and
`df["Revenue / Q1"].sum()` is right. See the [quickstart](quickstart.md).

Both forms come from the same reading step, so they always agree.

## How a question gets answered

Follow one question through the system: "What was the North region's revenue in Q1?", asked of the
file above.

1. **The agent opens the file.** It calls `open_file` with the path and, if its host passes it on,
   the question.
2. **hedgehog reads the workbook.** It reads xlsx, xlsm, xlsb, xls, ods and CSV files, with merged
   cells and number formats.
3. **It finds the table** and decides what each row is: title, header, data, group label, total,
   note.
4. **It cleans the table.** The two header rows become `Revenue / Q1`, `Revenue / Q2`; the region
   rows become a `Region` column; the text numbers become numbers; the total rows move out of the
   data into a second view of the table, so a plain `SUM` never counts them twice.
5. **It loads the table into a small in-memory database** and writes the catalog. With the question,
   the tables it is about come first.
6. **The agent sends a query:**
   `SELECT SUM("Revenue / Q1") FROM T1 WHERE Region = 'North'`. The answer comes back: 1500.5.
   A query that also selects `_src_row` gets a line saying which source cells each value comes
   from (`"Revenue / Q1" = Sales!B<_src_row>`).
7. **The agent answers** with that number and can point to the cells.

Steps 2 to 5 take a few milliseconds for a file like this and a few seconds for a workbook with a
million cells. The result is cached, so the next question on the same file starts at step 6.

## What hedgehog does not do

- It never changes your file. It opens it read-only.
- It reads the values Excel saved with the file. It does not recalculate formulas, so a file saved by
  a script without calculated values has no numbers to read in those cells (hedgehog says so).
- It does not answer questions by itself. An agent or a program asks; hedgehog makes the asking easy
  and exact. A language model is optional, and only used if you ask for a written summary.

## Where to go next

1. [installation.md](installation.md): install the Python package, the Node package or the
   `hedgehog` command.
2. [quickstart.md](quickstart.md): run it on the sample file above, then connect it to an agent.
3. [concepts.md](concepts.md): the ideas behind the output, one at a time.
