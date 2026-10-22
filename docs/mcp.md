# MCP server: `hedgehog mcp`

`hedgehog mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) tool server. An AI
client (Claude Desktop, Claude Code, an IDE, or your own app) starts it as a child process and calls
its six tools to answer questions about spreadsheets: open a file, read its catalog, run SQL over
the normalized tables, quote source cells, check totals and find files.

Nothing is written to disk. Each file is normalized in memory (with total rows kept), loaded into its
own read-only in-memory SQLite database and cached for the session by its content hash (sha256).

- Transport: stdio, JSON-RPC 2.0, one message per line. Logs go to stderr only.
- Protocol version `2025-06-18`; clients asking for `2025-03-26` or `2024-11-05` get that version.
- Capabilities: `tools` only. Methods: `initialize`, `ping`, `tools/list`, `tools/call`; notifications
  (`notifications/initialized`, `notifications/cancelled`, …) are accepted and ignored.

The same tools are available without MCP from Rust through the `hh_agent` crate
(`hh_agent::Session`), which the server wraps.

## Run it

```sh
hedgehog mcp [--root DIR] [--allow PATH]... [--query-timeout-ms N] [--max-file-mb N] [--max-cells N] [--max-seconds N]
```

| Flag | Default | Meaning |
|---|---|---|
| `--root DIR` | current folder | Folder the tools may read. Paths are relative to it. |
| `--allow PATH` | none | One more file or folder outside the root (repeatable). |
| `--query-timeout-ms N` | `2000` | Time limit of one SQL query. |
| `--max-file-mb N` | `256` | Larger files are refused. |
| `--max-cells N` | `10000000` | Workbooks with more non-empty cells are refused. Normalizing takes roughly 200 bytes per cell at peak (8.5 million cells: 1.8 GB), so the default keeps one file near 2 GB; raise it on a large machine. |
| `--max-seconds N` | `120` | Time limit for reading and normalizing one file. |

Get the binary from a [GitHub release](https://github.com/05vukasin/hedgehog/releases)
(`hedgehog-<target>`, e.g. `hedgehog-x86_64-unknown-linux-gnu`, `hedgehog-aarch64-apple-darwin`,
`hedgehog-x86_64-pc-windows-msvc.exe`; checksums in `SHA256SUMS.txt`) or build it:
`cargo install --git https://github.com/05vukasin/hedgehog hh_cli`.

### Claude Desktop

`claude_desktop_config.json` (Settings → Developer → Edit config):

```json
{
  "mcpServers": {
    "hedgehog": {
      "command": "/usr/local/bin/hedgehog",
      "args": ["mcp", "--root", "/Users/me/Documents/spreadsheets"]
    }
  }
}
```

### Claude Code

**Plugin (tools + skill).** The repository is a Claude Code plugin marketplace. With the `hedgehog`
binary on your PATH, in Claude Code:

```text
/plugin marketplace add 05vukasin/hedgehog
/plugin install hedgehog@hedgehog
```

or from a shell: `claude plugin marketplace add 05vukasin/hedgehog` and
`claude plugin install hedgehog@hedgehog` (add `--scope project` to share it through the project's
`.claude/settings.json`). Restart Claude Code; `/mcp` then lists
`plugin:hedgehog:hedgehog`. The plugin ([integrations/claude-code](../integrations/claude-code)) has:

- the MCP server: `hedgehog mcp --root ${CLAUDE_PROJECT_DIR}`, so the tools read the project folder
  (tool names `mcp__plugin_hedgehog_hedgehog__open_file`, …);
- the `spreadsheets` skill (`/hedgehog:spreadsheets`, which Claude also loads by itself for a
  spreadsheet question): open the file first and trust the catalog's TIME, total-row, mixed-measure,
  quality and entity lines; numbers come from SQLite queries (group by `<col>_id` when the catalog
  says so); check what one row is before summing; verify a surprising result with a second query;
  cite source cells; write pandas only for what the tools cannot do.

To try it from a clone without installing: `claude --plugin-dir integrations/claude-code`.

**MCP server only.** For the tools without the skill:

```sh
claude mcp add hedgehog -- hedgehog mcp --root "$PWD"
```

Notes:

- With very many skills installed, Claude Code's skill listing drops descriptions past its
  character budget, and a skill listed without a description is not chosen by itself. Invoke it as
  `/hedgehog:spreadsheets`, or raise the budget (`SLASH_COMMAND_TOOL_CHAR_BUDGET=80000`). The MCP
  server's own instructions still tell Claude to open the file first and use SQL.
- `claude -p --strict-mcp-config` drops plugin MCP servers too; pass the server with
  `--mcp-config` (as the [agent comparison](../eval/agents/README.md) does) if you need both.
- What it buys: on the four cases in [eval/agents](../eval/agents/RESULTS.md), Claude Code with the
  plugin answered every check right without writing pandas, at roughly a third to two thirds less
  cost than Claude Code alone; time was about the same (opening a 4.5M-cell file takes ~45 s).

### Any MCP client

Start `hedgehog mcp --root <folder>` with stdin/stdout as the transport. A client may pass extra
allowed paths (for files the user referenced outside the root) in the `initialize` request:

```json
{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
  "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "my-app", "version": "1.0"},
  "_meta": {"hedgehog": {"allow": ["/home/me/Downloads/report.xlsx"]}}}}
```

By hand, for a quick test:

```console
$ printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"sh","version":"0"}}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"open_file","arguments":{"path":"sales.xlsx"}}}' \
  | hedgehog mcp
```

## Tools

Every result is `content: [{type: "text", text}]` with compact text of at most ~12,000 characters
(~3k tokens). When something was left out, the text ends with a line
`… cut: <what was omitted>; ask for <how to get more>`. Errors come back as `isError: true` with one
line the model can act on (an unknown column lists the closest names; an unknown table lists the
tables). Tables are named `T1`, `T2`, … per file, as in the [manifest](normalized-output.md).

| Tool | Arguments | Returns |
|---|---|---|
| `open_file` | `path`, `question?` | The catalog: file line, time coverage, tables with source range, rows, total rows, every column (type, unit; value ranges for up to 14 columns), all values of small label columns, mixed measures, group column, totals check, the rows of small tables as CSV, notes, warnings. Long catalogs list tables one line each. With the user's `question`, the tables it is about come first and in full, the others one line each ([Question-aware catalog](#question-aware-catalog)). See [The catalog](#the-catalog). |
| `describe_table` | `path`, `table` | Every column (SQL name, dtype with its semantic type, unit, nulls, distinct, min..max, example or all values (≤ 12), source column ← header cells), source and header range, group column, total rows, entity columns (`<col>_id`, `<col>_name`), first 5 rows as CSV. |
| `query` | `path`, `sql`, `limit?` | One read-only SQLite `SELECT`/`WITH` as CSV with a header, at most `limit` rows (default 50, max 200), then `(<n> rows)` or `(<n> rows, showing first <k>)`. Text values are returned whole up to 2,000 characters (a `group_concat` list can be pasted into the next query); a longer value ends in `…[+N chars]` and a `note:` says to read the rest with `substr(col, 2001, 2000)`. |
| `cells` | `path`, `sheet`, `range` | Original source cells (max 200) as `A1<TAB>value` lines, formulas after the value, merged areas marked; a formula saved without a value shows `(formula without a saved value)` and its formula. |
| `totals` | `path`, `table?`, `status?` | Counts per status (`ok`, `mismatch`, `unknown`), then up to 50 items, mismatches first: cell, label, value, recomputed, sources, reason. |
| `list_files` | `folder?`, `pattern?` | Spreadsheets under the folder (xlsx, xlsm, xlsb, xls, ods, csv, tsv) with size and modified date; max 200 lines. `pattern`: a glob (`*.csv`), words that must all appear (`linz 2022`) or alternatives (`preis\|price`). |

### SQL tables

For table *n*:

- `T<n>` — the data rows, one column per normalized column plus `_src_row` (the row number in the
  source sheet).
- `T<n>_all` — the same plus total and subtotal rows, with `_row_type` = `data`, `subtotal` or
  `total`, and the rows that are no data and no sums: `average` ("Average", "Ø 2024") and `of_which`
  ("of which: exports", "davon Inland"). Use it for overall figures the file states itself (a
  "Gesamt" or "Total" row, an annual average); never add those rows to the data rows.
- `T<n>_long` — only for a table with a run of 4 or more numeric columns named by periods of one
  granularity (`"2011".."2025"`, `Jan 2024`, `2024 Q1`, `Oktober / 2022`, `Thu / 2002-01-31`,
  `Rev 2024-03`): the period columns as rows, so a model filters `WHERE period = '2025'` instead of
  choosing a column. Columns: the table's key columns (group, label, ID and dimension columns, up
  to 8), `measure` when the period columns repeat for several measures (`Umsatz / 2023`,
  `Kosten / 2023` → `measure` = `Umsatz` | `Kosten`), `period` (normalized: `2025`, `2025-Q1`,
  `2025-03`, `2025-W05`, `2025-03-31`), `period_label` (the column name), `value`, `_src_row`
  and `_src_col` (the value's source cell is `<_src_col><_src_row>`). Like `T<n>`, it has the data
  rows only; `T<n>_long_all` also has the total rows, with `_row_type` (as `T<n>_all`). Empty
  cells are left out. Both are plain views (a `UNION ALL` over `T<n>` / `T<n>_all`), so nothing is
  copied at load; a filter on `period` reads only that column (a 20,000 × 120 month table:
  `WHERE period = '2019-03'` grouped by region in ~15 ms; a full `GROUP BY period` over all 2.4 M
  values ~1.4 s).

Types: integers `INTEGER`; numbers, currency and percentages `REAL` (a percentage as a fraction,
0.15 = 15 %); dates ISO text (`2024-03-31`, or `2024-03-31 10:30:00`); booleans 0/1; text `TEXT`.
Column names are the normalized headers; quote them with double quotes (`"Revenue / Q1"`, `"2024"`,
`"group"`). The source cell of a value is `<source column><_src_row>` on the table's source sheet;
query results that include `_src_row` say which column maps to which source column (`cite as:`).

The dialect is SQLite: date parts come from `strftime` (`strftime('%Y-%m', "Signed")` for the month,
`CAST(strftime('%m', d) AS INTEGER)` for a number), days between dates are
`julianday(b) - julianday(a)`, and `LIMIT n` replaces `TOP n`.

**Entity columns.** A text column whose values are mostly `"<id> - <name>"` with a code-like ID
(digits, or letters and digits: a tax number, a CPV or NUTS code), such as
`"503933813 - IP - Infraestruturas de Portugal, S.A."`, gets two more columns in `T<n>` and
`T<n>_all`, right after it: `<col>_id` (the code, as text) and `<col>_name` (the name). Registers
write one entity under several spellings with the same code, so group by `<col>_id`, not by the text;
`<col>_name` is one name per ID (the spelling most rows use), so grouping by it does not split an
entity either. A query result that lists one ID on several rows under different spellings (grouped
by the text) ends with a `note:` naming the rows and the query grouped by the ID.
A value that is only a code has an empty `<col>_name`; a value without a code (`"- - Name"`) has an
empty `<col>_id`; a cell naming several entities (`"a | b"`, `"a; b"`, one per line, or run together
as `"501… - A, Lda 502… - B, SA"`) is not split: its `<col>_id` lists every ID as `"id1 | id2"`. The
catalog and `describe_table` describe each such column (see [The catalog](#the-catalog)); `cite as:`
maps `<col>_id` to the source cell it comes from.

**Union views.** Tables with the same layout on several sheets (one per state, per day, per
month: the same column types in the same order and at least half of the names the same once
dates are dropped) get one view over all of them, `U<k>` (data rows) and `U<k>_all` (with the
total rows and `_row_type`, like `T<n>_all`). Columns: `_table` (`T8`), `sheet` (the source sheet),
`title`, `part` (the part of the title that differs between the tables, when the titles differ:
`'Tirol'` from "Ankünfte und Übernachtungen in Tirol – nach Herkunftsland Juli 2026"),
`variant` (the header words that differ, when the tables name a column differently:
`'Day-Ahead'` | `'Real-Time Transactions'`), the columns, and `_src_row`. The union's column
names are the tables' names with dates and periods dropped (`2001-05-09 / Day-Ahead / Qty` →
`Day-Ahead / Qty`); where the tables still differ, the shared trailing part (`Qty`), or the first
table's name. A catalog line per family names it:
`SQL union view U1 = T1..T10 (10 tables with the same columns; adds _table, sheet, title, part = 'Österreich' | 'Burgenland' | …)`.
So the state with the most overnight stays is one query:
`SELECT part, "Nächtigungen / Absolut" FROM U1_all WHERE "Herkunftsland oder -region" = 'Insgesamt' ORDER BY 2 DESC`.
(A name that collides with a column gets a leading `_`: `_sheet`.)

**Several files.** Every file opened in a session gets a number in the order it was opened, and
`f<k>.T1` names table T1 of file k, so one query joins tables of different files (`f2.T1_all`,
`f2.T1_long`, `f2.U1` work too). Plain `T1` stays the file in `path`, which can be any open
file. With two or more files open, the catalog ends with the session's files and the columns
that join them (same name in tables of different files, overlapping values, one side unique):

```text
FILES open in this session — one query can join them (any open file as path; plain T1 is the file in path): f1 = northwind/orders.csv (T1), f2 = northwind/order_details.csv (T1; this file); e.g. SELECT … FROM f1.T1 AS a JOIN f2.T1 AS b ON a.orderID = b.orderID
JOIN KEYS f1.T1.orderID = f2.T1.orderID (830 shared values; unique in f1.T1)
```

`SELECT o.employeeID, ROUND(SUM(d.unitPrice * d.quantity * (1 - d.discount)), 2) AS revenue FROM T1 o JOIN f2.T1 d ON o.orderID = d.orderID WHERE o.orderDate LIKE '1997%' GROUP BY 1 ORDER BY 2 DESC LIMIT 1`
on `path` = `northwind/orders.csv` gives `4,128809.79`. The query runs on a connection with the
files' databases attached read-only by the server (a user's `ATTACH` is still refused); the
number of a file stays the same for the session, and a second name of the same content is the
same file. In-process hosts list the files with `Session::open_files()` (Node:
`HedgehogSession.openFiles()`).

### Example

`eu_at_opendata/003_pen_02_02.xlsx` (Austrian pension supplements, one sheet):

```text
FILE eu_at_opendata/003_pen_02_02.xlsx · 1 sheet · 244 cells (97% in tables) · 1 table
TIME 2011–2025 (year, from column headers in T1)
T1  'jeweils Dezember' ← '2_2'!A3:R18 · 7 rows (+5 total rows only in T1_all) · 1 total row stays in T1 — exclude it before summing
    columns (19): "group"[group] Richtsatz "2011".."2025"[15 cols] "2020/ 2025" "2024/ 2025"
    rows mix measures by "group" — filter before aggregating
    T1_long: the 15 year columns "2011".."2025" as rows ("group", Richtsatz, period '2011'..'2025', value) · rows mix measures: filter "group" first · total rows only in T1_long_all
    do not sum: rows "group" = 'Durchschnittshöhe in Euro' (avg EUR) — use AVG or the file's figure
    group column: "group"
    total rows: SELECT … FROM T1_all WHERE _row_type IN ('total', 'of_which') — labels in Richtsatz: totals 'Gesamt'@9 (= rows 5–8), 'Gesamt'@15 [stays in T1]; of-which rows 'davon Männer'@6 (= rows 5), 'davon Frauen'@7 (= rows 5), 'davon Männer'@12 (= rows 11), 'davon Frauen'@13 (= rows 11)
    all 12 rows (CSV, as in T1_all; rows with _row_type other than data are not in T1):
    group,Richtsatz,2011,2012,2013,2014,2015,2016,2017,2018,2019,2020,2021,2022,2023,2024,2025,2020/ 2025,2024/ 2025,_row_type
    Anzahl Bezieher:innen,Alleinstehend,196489,192124,192524,188226,181418,178539,181292,179250,177133,168872,165943,162833,165364,163230,162375,-0.0384729,-0.00523801,data
    Anzahl Bezieher:innen,davon Männer,43628,43682,44709,43727,42306,42278,43796,43711,43766,42248,41872,41506,42453,42628,43488,0.0293505,0.0201745,of_which
    Anzahl Bezieher:innen,davon Frauen,152861,148442,147815,144499,139112,136261,137496,135539,133367,126624,124071,121327,122911,120602,118887,-0.0611022,-0.0142203,of_which
    …
TOTALS 51 ok, 0 mismatch, 47 unknown (details: totals)
NOTES 7 cells; e.g. '2_2'!A1: "AUSGLEICHSZULAGEN - alleinstehend/verheiratet"
SQL view T1 = data rows, T1_all = + total rows (_row_type); both with _src_row; quote names: SELECT "col name" FROM T1
SQL long view T1_long (and …_long_all with the total rows): period columns as rows (period, period_label, value, _src_row, _src_col: the value's cell)
(normalized in 0.1 s)
```

The trend of one series from the long view, with the cell of every value:
`SELECT period, value, _src_col, _src_row FROM T1_long WHERE Richtsatz = 'Alleinstehend' AND "group" LIKE 'Durchschnitt%'`
gives `2011,264.3,B,11` … `2025,403.19,P,11` (15 rows).

`query` with `SELECT "group", Richtsatz, "2024", "2025", _row_type, _src_row FROM T1_all WHERE Richtsatz IN ('Alleinstehend', 'Gesamt') ORDER BY _src_row`:

```text
group,Richtsatz,2024,2025,_row_type,_src_row
Anzahl Bezieher:innen,Alleinstehend,163230,162375,data,5
Anzahl Bezieher:innen,Gesamt,190423,189537,total,9
Durchschnittshöhe in Euro,Alleinstehend,386.9,403.19,data,11
Durchschnittshöhe in Euro,Gesamt,413.44,430.56,data,15
Richtsätze in Euro,Alleinstehend,1217.96,1273.99,data,17
cite as: Richtsatz = '2_2'!A<_src_row>; "2024" = '2_2'!O<_src_row>; "2025" = '2_2'!P<_src_row>
(5 rows)
```

An unquoted keyword, a guessed column name, a row label used as a column or another SQL dialect
gets a hint instead of a bare error:

```text
SQL error: near "group": syntax error (at character 7) — group is a column name that is also an SQL keyword; quote it: "group"
SQL error: no such column: "Dezember / 2021" (at character 7) — did you mean T1."De-zember / 20211)", T1."No-vember / 2021", …? (quote names with double quotes)
SQL error: no such column: "Total" (at character 7) — "Total" is a row label, not a column: total rows are in T1_all with _row_type = 'total' (label in col_C): SELECT * FROM T1_all WHERE _row_type = 'total'
SQL error: no such column: "SEA ROBIN" (at character 7) — "SEA ROBIN" is a value of column col_C, not a column: SELECT * FROM T1 WHERE "col_C" = 'SEA ROBIN'
SQL error: no such function: MONTH (at character 7) — SQLite has no MONTH(); dates are ISO text, use strftime('%m', col) for the month (text '01'..'12'; CAST(strftime('%m', col) AS INTEGER) for a number)
SQL error: near "5": syntax error (at character 11) — SQLite has no TOP n; put LIMIT n at the end: SELECT … ORDER BY x DESC LIMIT 5
```

A "no such column" for `_row_type` on `T1` says that `T1` has the data rows only and the total
rows (and `_row_type`) are in `T1_all`. A column the outer query takes from a subquery or CTE that
does not select it (an outer `SUM("price")` over `(SELECT buyer, SUM("price") AS total … GROUP BY buyer)`)
names the subquery's columns (`buyer, total`) instead of suggesting `T1."price"`.

A "no such column" whose name is the label of a total row (or a total word: Total, Gesamt, Summe,
Totale, Ukupno, Subtotal, …) says where the total rows are; one that is a value of a text column
says which column to filter. Both also run that query and return its rows (at most 10, ~3,000
characters) under `Those rows (the query above, run for you; pick the columns you need):`, so the
data is there even when the model does not retry; the call is still an error.

A total row takes the keys of the block it closes: its group (`"group"`) and empty text cells left
of its label that every data row of the block shares (`col_B = 'Receipts HH'`), so
`WHERE "group" = 'Anzahl Bezieher:innen' AND _row_type = 'total'` finds it. MySQL / Postgres / SQL Server functions and syntax SQLite lacks
(`YEAR`, `MONTH`, `DAY`, `DATE_FORMAT`, `TO_CHAR`, `EXTRACT(… FROM …)`, `DATEDIFF`, `DATE_ADD`, `NOW`,
`CONCAT`, `ISNULL`, `LEN`, `STRING_AGG`, `MEDIAN`/`PERCENTILE_CONT`, `STDDEV`, `TOP n`, `ILIKE`,
`::type`, `FETCH FIRST`, `INTERVAL`, …) get the SQLite replacement.

**Empty results explain themselves.** A result that reads as "nothing" (no rows, one row of
NULLs such as `SUM(…)` over no rows, or a count of 0) of a query with a filter `col = 'x'`,
`col IN (…)`, `col LIKE …` or `col = <number>` gets a `note:` per filter value, from small probe
queries:

```text
note: T1."Country Name" = 'European Union' is only on 1 total row (_row_type 'total'), which is in T1_all, not in T1
The same query on T1_all (the total rows included):
2024
43313.7668204
note: T1."Country Name" has no 'Somalia' — 0 rows here means the file has no such rows, not a value of 0; values that contain it: 'Somalia, Fed. Rep.' (1 row)
note: T1.agency_name has no 'NYPD' — 0 rows here means the file has no such rows, not a value of 0; its 2 values: 'ADMIN FOR CHILDREN''S SVCS' (95,179 rows), 'ADMIN TRIALS AND HEARINGS' (4,821 rows)
note: T1.fiscal_year has no 2030 — …; its values run from 2014 to 2025
note: T1.Returned has no 'TRUE' — …; Returned is a yes/no column stored as 1/0: use Returned = 1 (or = 0)
```

A value that is only on total rows also gets the rows of the same query run on `T<n>_all` (at
most 5). A value the column does not have gets the values that contain it (or that it contains as
a word, any case), else all the column's values (up to 20) or the closest / most frequent ones. A
non-empty result over `T<n>` whose filter value also labels total rows (only in `T<n>_all`) says
so in one line. A result with empty values from `a + b` over columns that are sometimes empty gets
`note: … a + b is NULL when any part is empty ("bonus" empty in 10 rows); use COALESCE(col, 0) …`.

### The catalog

Per table (when the whole catalog fits in ~12,000 characters):

- **Head line**: id, title, source range, data rows; how many total rows are taken out of the data
  (only in `T<n>_all`; their labels are on the table's `total rows:` line, or here when that line
  has none); total rows the totals check found that stay in the data (an "Insgesamt" row of a long
  table: exclude them before summing); the table's period when the file has several tables.
- **Columns**: every column, every name in full, with only what changes an answer in brackets: a
  currency or unit, `id` (not a quantity), `rate` / `avg` / `index` (do not add up; see *Do not
  sum*), `percent` (stored as a fraction), dates with their range. Up to 14 columns also get their
  value range (measures and dates, 4 significant digits; not IDs) or an example (text):
  `"Ankünfte / Absolut"[1169..6009964] "Ankünfte / Veränderung in %"[rate -28.75..73.98]`; a plain
  text or number column is just its name. Runs of 4+ columns named by consecutive periods or numbers
  are written as one item without hiding any (`"2011".."2025"[15 cols]`, `"Thu /
  2002-01-31".."Tue / 2002-01-01"[31 cols]`, `"1".."20"[20 cols]`). Wider tables group 3+
  neighbouring names by their prefix (the first header level, else the first word or `_` part)
  when that is a tenth shorter, every member named, a member of another type marked
  (`solar_{cons_change_pct[rate], consumption, electricity}`; the line says `a_{b, c} = a_b, a_c`).
  Past 6,000 characters the line gives names only, then a count.
- **Booleans** are shown as SQL stores them: `Returned[bool 1/0 in SQL (TRUE/yes = 1, FALSE/no = 0;
  filter = 1, not 'TRUE')]`, and `1` / `0` in inlined rows.
- **Units**: a scale in the table's title or a note above it (`(All values in 000s)`, `in Tausend`,
  `Mio. €`, `en milliers`, `(in millions)`, `in 1 000 EUR`): `units: amounts in thousands (× 1,000), says "(All
  values in 000s)" — multiply by 1,000 for full figures`.
- **Column notes**: a footnote of the table or its sheet that names a column's series code
  (`LF24`), or defines a quantity whose words are all in one column's header and match it best
  (`The headline employment rate is the number of people aged 16 to 64 …`): `column notes: "All
  aged 16 to 64 / Employment / rate (%) / LF24" ← "The headline employment rate is …" (A681)`.
- **Values**: text columns with at most 12 distinct values list all of them, so a model filters by
  the exact label. When room is left: a code column of a large table with at most 60 distinct short
  codes (2–12 capitals, digits, `_.-`; not period codes, not an ID with a name column) lists them all
  (Eurostat `geo values: "AL" | "AT" | …`), and a table of at most 30 rows that is too wide to
  inline lists every row label of its first text column (`Year values: "Equity" | … | "NPV @ 14%" |
  "IRR"`).
- **Column types**: each column gets a semantic type: `id`, `name` (a label per row), `dim` (a small
  category), `period`, a measure (`amount` with its currency when known from the values, the header
  or the title, `count`, `rate`, `avg`, `index`), `free text`, or `mixed` (numbers of a table whose
  rows mix measures). The catalog shows the ones that change an answer: the currency, `id`, and
  `rate` / `avg` / `index` (`"gleitender 7-Tage-Durchschnitt KSB"[avg 68..106.5]`,
  `Indexwert[index]`); `describe_table` shows every type (`number · avg — don't sum`). Numbers whose
  header, unit, currency and title say nothing are left untagged (plain quantities). Types come from header
  words in English, German, French, Spanish, Portuguese, Italian, Dutch and Serbian (`Anzahl`,
  `Durchschnitt`, `Prozent`, `taxa`, `moyenne`, `aantal`, `prosečna` …), the currency / unit
  normalization found, the table title for bare period headers (`"2025"` in an index table), a
  header naming two years (`"2020/ 2025"`: a change, so a rate) and value statistics (distinct
  values, text length, running numbers). A column the file itself adds up (a vertical total that
  checks out) is never flagged `don't sum`.
- **Do not sum**: `do not sum: unbereinigt (index), "gleitender 7-Tage-Durchschnitt KSB" (avg), …
  — use AVG, a weighted mean or the file's own total`; for a table whose rows mix measures, the
  labels whose rows are averages / rates / indexes: `do not sum: rows "group" = 'Durchschnittshöhe
  in Euro' (avg EUR)`.
- **Long view**: `T1_long: the 15 year columns "2011".."2025" as rows ("group", Richtsatz, period
  '2011'..'2025', value) · rows mix measures: filter "group" first · total rows only in
  T1_long_all` (see [SQL tables](#sql-tables)); tables listed
  briefly or as "same layout as" get ` · T4_long (by day)`, and the SQL line at the end lists the
  long views.
- **Mixed measures**: `rows mix measures by "group" — filter before aggregating` when one label
  column separates counts, amounts, rates (by units and words in the labels, such as `Anzahl` and
  `in Euro`, or by numbers of very different size under each label).
- **Entity columns**: one line per `"<id> - <name>"` column: an example, how many IDs, how many
  have several spellings, cells with several entities, values without an ID, and to group by
  `<col>_id`: `adjudicante = "<id> - <name>" (e.g. "600082440 - Direção-Geral de …"): 3,872 IDs, 808
  with several spellings (e.g. 508481287: 5 names; up to 52) — group by adjudicante_id, not by the
  text: SELECT "adjudicante_id", "adjudicante_name", count(*) FROM T1 GROUP BY "adjudicante_id";
  adjudicante_name holds one name per ID (its most frequent spelling) · …`. It comes right under
  the columns line (when some values have no ID, the query groups by
  `coalesce(<col>_id, <col>_name)`).
- **ID and name columns**: an ID column (codes such as `vr_012174`, or whole numbers under an
  ID-like header) that repeats over the rows, each ID going with one name (or a few spellings) in
  a column whose header says name, recipient, customer …: `empfaengerid is the ID of name: 9,873
  IDs, 1,506 with several spellings (e.g. hra_031152: 2 names) — group by empfaengerid, not by the
  name, and show a name: SELECT "empfaengerid", MAX("name") AS "name", count(*) FROM T1 GROUP BY
  "empfaengerid"`; a table's further pairs without the query (`… — group by "Функција_класе" like
  "Програм" above`).
- **Shares** (tables with ≥ 30 data rows and an additive amount column: money or a count, not a
  unit price with few values, a rate or an average, and not when a unit / currency column has
  several values): a ready query for a share of the total, ranked by the first entity column, an
  ID with names, or the dimension with the most values in 10..1,000 that is not a name or a
  period: `share of
  the total (e.g. top 10 by "adjudicante_id"): SELECT 100.0 * SUM(v) / (SELECT SUM("precoContratual")
  FROM T1) AS pct FROM (SELECT SUM("precoContratual") AS v FROM T1 GROUP BY "adjudicante_id" ORDER BY
  v DESC LIMIT 10)`.
- **Long forms**: when one table repeats another's values one row per key × category (checked on
  up to 40 sampled rows), both get `T5 is the long form of T3: one row per Datum × Saisonbereinigung,
  value in Indexwert (T3 has one column per Saisonbereinigung value)`.
- **Total rows**: how to select them, where their labels are and every label (up to 15) by level,
  the grand total named and the source rows each adds up (from the totals checks that confirmed
  it), e.g. `total rows: SELECT … FROM T1_all WHERE _row_type IN ('subtotal', 'total') — labels in
  col_A: grand total 'Total Expenses'@65 (= rows 7, 49, 57, 63); total 'Total O&M'@49 (= rows 41,
  47); subtotals (_row_type 'subtotal', not 'total') 'Subtotal Fixed O&M'@41 (= rows 10–39), …`.
  The grand total is the one total no other total includes (or, with several on top, the one the
  check calls the grand total); a total that stays in the data says `[stays in T1]`. A later table
  with the same labels says `labels as in T1`. Rows of one label without a scope of their own are
  listed once (`'T'@13/16/19`); many rows with few labels (one per period or measure) give each label
  once (`1080 rows labelled 'EU27_2020', 'EA20', …`). When the label column is constant (a dataset
  code), the labels come from the column whose values on the total rows are never a data row's
  (`geo`). With a grand total, totals whose label names a group next to the total word say that
  they total one group, not the table (`'Österreich gesamt', 'Deutschland gesamt' each total one
  group of "Herkunftsland oder -region" (e.g. Österreich), not the table`: guests from Austria, not
  Austria's total). Without a label column: their `_src_row` and sign
  (`_src_row 51 positive, 65 negative`).
- **Signs**: `signs: col_B 'Deliveries HH' rows are all negative (outflows?) — a net figure is the
  SUM over all rows, not a difference` when a block of rows holds only negative numbers next to
  positive ones: by a label column whose labels (or the number columns' names) name flows (receipts
  / deliveries, debit / credit, in / out, purchases / sales, payments, imports / exports …), else by
  `_src_row` ranges that hold at least a tenth of the signed rows. Never for price, rate or index
  columns; scattered negative values are counted on the quality line instead.
- **Small tables** (≤ 400 cells as rows × columns of `T<n>_all` with `_row_type`, ≤ 30 rows):
  every row as CSV under the table, total rows included and marked in a `_row_type` column (the
  values lines are then left out); fractions to 6 significant digits (every digit before the point,
  amounts with up to 2 decimals as they are); at most 3,500 characters of rows per catalog, the
  first tables first.
- **Amount columns** (tables with ≥ 30 data rows; not IDs, codes, years, period columns,
  percentages, averages or indexes): `numbers:` with sum (money and count columns), the fill rate
  when cells are missing, and the range (when the columns line has none), for columns with a sum or
  under 90 % filled; look-alike amount columns (both money, or similar names) whose sums differ get
  one line, e.g. `precoContratual (sum 11.16 bn) vs PrecoTotalEfetivo (sum 4.28 bn) — different
  measures; pick deliberately`.
- **Quality** (tables with ≥ 20 data rows): `quality:` with duplicates in an ID column that is
  otherwise unique (≤ 5 % repeats); an ID written on several lines (`"Order ID": 800 rows, 296
  distinct — count them with COUNT(DISTINCT "Order ID")`, up to two); empty cells of any column and
  the label that explains them (`annual_salary empty in 7,129 rows (22 %): salary_or_hourly =
  'HOURLY' (all its 7,129 rows)`, `dataCelebracaoContrato empty in 26,722 rows (15 %):
  tipoprocedimento = 'Ajuste direto simplificado' (all its 26,261 rows) … — counts by this date
  leave them out`; columns empty in the same rows for the same reason once: `nAnuncio, TipoAnuncio
  empty in …`), up to two text columns with ≥ 5 % empty cells and no such label (`"Запослен у ГО"
  empty in 1,261 rows (35 %) (a NULL group when grouping)`; not a block label with one value); a
  first or last year of the period column with missing months (`Date: 2013 has Sep–Dec only (4 of
  12 months)`); negative amounts except in columns of changes or differences (cells naming several
  entities are on the entity line) and formula cells saved without a value (`12 formula cells
  without a saved value (NULL, not evaluated)`, any table size).
- **Formulas without values** (whole file): `FORMULAS 1,479 cells without a saved value (e.g.
  May1!H13): NULL in the tables, not evaluated; cells shows the formula`, before `TOTALS`, when the
  file was saved by a tool that did not calculate. hedgehog never evaluates formulas.

The table lines come first; the optional parts are added while the catalog stays under its cap:
total-row and sign lines first, then small tables' rows, then amount and quality lines, each in
table order. They are computed in one pass per column (about 0.2 s on a 176k-row register).

**Runs of tables.** Three or more consecutive tables with an earlier table's columns and the same
facts (rows, total rows, period) get one line, with what tells them apart (the words their titles
do not share, else their sheets):
`T2..T10 same columns as T1 · 72 rows each (+3 total rows only in T<n>_all) · 1 total row stays in
each — exclude it before summing · total rows labelled as in T1 · titles 'Ankünfte und
Übernachtungen … – nach Herkunftsland Juli 2026': T2 Burgenland, T3 Kärnten, …, T8 Tirol, …`.

**Brief catalogs.** When the full catalog is too long (or the file has more than 40 tables) every
table gets one line, within 8,500 characters plus 25 per table (at most the ~12,000 cap): the
sheet (not repeated for the tables under it), data rows, the period when it is outside the TIME
line's span, total rows by level (a set repeated from an earlier table as `total rows as in T1`),
the scale of its numbers, and column notes on their own line. The column names come in order of
importance while room lasts: the first table of each sheet (its main table), then the others, then
tables of one or two rows; each gets every name if it fits, else its title and first names
(`Jahr +8 more`), else only its line. Shared header levels are written once (`"In jeweiligen
Preisen / {Ursprungswerte / {Mrd. EUR, %}, …}"`, `"{Einnahmen, Ausgaben} / % des BIP"`; members
separated by `; ` when one has a comma; the legend is on the first line). A table whose columns
are an earlier table's with one more header level says so by reference: `Jahr + T27's "Einnahmen /
insgesamt".."Finanzierungs-saldo", each + " / % des Bruttoinlandsprodukts"`. A table with an
earlier table's names says `same columns as T1`, and runs of them get one line as above. Union views
after the first share one line. If the lines still do not fit, the last line says which tables were
cut.

The totals summary, total-row labels and rows, and the time coverage are the ones the
[manifest](normalized-output.md) has (`tables[].totals`, `time_coverage`), from the same functions
(except a TIME line taken from text period codes over an update stamp, below).

### Question-aware catalog

`open_file` takes an optional `question`: the user's question, as asked. A host that opens a file
to answer a question passes it along (`{"path": "toll.xlsx", "question": "What was the highest
unadjusted daily value in 2023?"}`; Node: `session.openFile(path, { question })`; Rust:
`Session::open_file_for(path, Some(question))`). The catalog then lists the tables the question is
about first and in full, the other tables one line each, and says so at the top:

```text
QUESTION matches T3 (ksb, 2023), T5 (index, 2023): first, in full; then 3 other tables, one line each
T3  '42191-b01: Lkw-Maut-Fahrleistungsindex: Deutschland, Tage, …' ← '42191-b01'!A4:G6862 · 6,858 rows · …
    columns (7): Datum[date 2008-01-01..2026-10-10, 6858 distinct] … "Kalender- und saisonbereinigt (KSB)"[index 53.5..129.3] …
    do not sum: …
T5  ← 'csv-42191-b01'!A1:G13717 · 13,716 rows · …
    …
(other tables, one line each)
T1  'Daten zur Weiterverarbeitung ("CSV-Tabellen")' ← 'Inhaltsübersicht' · 7 rows · 3 cols: …
T2  ← 'GENESIS-Online' · 3 rows
T4  ← 'Erläuterung_zu_CSV-Tabellen' · 12 rows
```

How tables are matched (deterministic text matching, no model, no embeddings; `agent/src/question.rs`):

- The question's words are folded (lowercase, accents removed, Serbian Cyrillic in Latin letters),
  function words and question words of English, German, French, Spanish, Portuguese, Italian, Dutch
  and Serbian dropped, and stemmed with a few suffix rules shared by these languages (`salaries` ~
  `salary`, `Nächtigungen` ~ `Nächtigung`, `contratos` ~ `contrato`); a word of 5+ letters also
  matches a longer word it starts, one of 6+ letters a compound that contains it
  (`Bruttoinlandsprodukts`). A small list of statistical concepts links an English question to
  other languages' names (`GDP` → `Bruttoinlandsprodukt`, `PIB`; `unemployment` → `Erwerbslose`;
  `VAT` → `IVA`, …).
- Each table offers its title (weighs most), sheet name, column names, the distinct values of its
  text columns (label and code values), and its time coverage. A word counts by the best of these
  that has it, times how few tables have it: a word every table has tells none apart and counts
  nothing.
- Years count when a table's coverage (or a year in its title, sheet or column names) holds them,
  months (`January 2002`, `Juli 2026`) when its coverage holds the month, the narrowest table
  first; with years in the question, a table that holds none of them counts less. `Table 1`,
  `Tabelle 2.3` name a table by its title (`Table 1: …`, `1.1 Bruttoinlandsprodukt`) or sheet.
- A question often has several parts: the best-scoring table is taken, then, while one scores at
  least 40 % of that on the parts no chosen table answers well, the best of those; and the table
  that has a distinctive word (one at most √n tables have) best. Tables the question names by number
  come first, and then only tables for the other parts.

What changes, and what does not:

- Every table is still listed. When the catalog without a question lists every table in full, the
  matching tables (up to 8) keep their full block (columns, values, inline rows, total rows, signs,
  do-not-sum, mixed measures, quality facts) and the others get one line each (title, sheet, rows,
  how many total rows, as many column names as fit in 20 characters per table); 3+ consecutive
  other tables that show nothing more share one line (`T10..T26 · 17 tables, 34–35 rows ← sheets
  '1.4', '1.5', …`). When it is a brief catalog (too long, or more than 40 tables), every table
  stays one line and the matching ones come first with every column name (`first, with every
  column name; then 149 other tables`).
- The catalog stays as without a question when the question tells no table apart (no table, more
  than 16 or every table matches), for a file of one table, and when the focused catalog would not
  be shorter. Without `question` (or with an empty one) the output is byte-identical to before.
- With a question the catalog is computed for it (not taken from the disk cache), so a file
  reopened from the cache waits for its normalized form (a few ms to ~1 s on large files).
- `HEDGEHOG_TRACE=1` prints each table's score, matched words and the chosen tables to stderr.

Measured with [eval/info](../eval/info/QUESTION.md) (`score.py --question`, the 30 cases' prompts as
questions): 168 of 168 checks stay in the catalog or one query away, 42,360 → 34,573 catalog
tokens over all cases (−18 %), −29 % on the 12 multi-table files and −35 % on the 9 where the
question told tables apart; no trap warning of a matching table lost.

### The catalog's TIME line

The manifest's [time coverage](normalized-output.md#time-coverage): the period the data covers
(`TIME 2020-01–2025-12 (month, from columns T3.Berichtszeitraum)`), at the finest granularity found,
from date / period columns or period-like column headers. Dates that describe the row's entity
(start, hire, birth, expiry, valid-from: `agency_start_date`, `Hire Date`, `DOB`) count only when a
table has no other date or period column, so a payroll's `fiscal_year` decides its period, not the
employees' start dates. A table dated only by an update stamp (`LAST UPDATE`, `Stand`,
`extraction_date`) or not at all, with a text column of period codes (`2019-S1`, `2024-Q3`,
`2024M03`, `2024-03`, `2024`), takes its period from those codes (`TIME 2019-S1–2026-S1 (half-year,
from columns T1.TIME_PERIOD)`; the manifest keeps the core's coverage). When tables differ (a monthly and an
annual table) it says `mixed: year (T5, …), month (T3) — check what one row is before summing`.
Years that only titles, notes, sheet or file names mention are a secondary note: added when they lie
outside the data's period, or the whole line when the data has no dates
(`TIME 2023 (only mentioned in titles/notes/sheet or file names; no date or period columns)`). An
agent should check it before answering a question about a period the file may not cover.

## Security limits

The server only reads; it never writes, deletes or executes anything.

- **Files**: every path is resolved against `--root`, canonicalized (`..` and symlinks resolved) and
  must lie inside the root or be an allowed path (`--allow`, `_meta.hedgehog.allow`; an allowed file
  does not open its folder). `list_files` follows no symlinked folders and skips hidden entries. Only
  spreadsheet extensions are opened; files above `--max-file-mb` are refused.
- **Untrusted file contents**: every file is checked before and while it is read, and refused with
  a one-line tool error that starts with `file refused:`:
  - a zip package (xlsx, xlsm, xlsb, ods) whose parts decompress to more than 2 GB (zip bombs; the
    zip reader also refuses to inflate a part past its declared size);
  - more than `--max-cells` cells (10 million by default). An ODS file's repeated rows and columns
    are counted before it is parsed, so a few hundred bytes cannot ask for billions of cells;
  - more than 10 million shared strings;
  - reading plus normalizing longer than `--max-seconds` (120 s by default).

  Password-protected files (encrypted xlsx / xlsb, encrypted ODS, xls with a password) give
  `the file is password-protected (encrypted); …`; corrupt files give the reader's error on one
  line. The readers are fuzzed (`fuzz/`, see [development](development.md#fuzzing)), and the bugs
  found inside calamine are fixed in a vendored copy (`vendor/calamine`).
- **Crash isolation**: a panic inside one tool call (a bug hit by a malformed file) becomes that
  call's error (`internal error in open_file (…); the server is still running …`); the server keeps
  serving. Release builds unwind on panic (`panic = "unwind"`), which this needs; a binary built
  with `panic = "abort"` logs a warning at startup.
- **SQL**: each file has its own database (in memory, or the disk cache's file opened read-only);
  after loading, the connection is locked down:
  - an authorizer allows only `SELECT`, reads, recursive CTEs, functions (minus `load_extension` and
    similar) and the read-only pragmas `table_info`/`table_xinfo`/`table_list`; everything else —
    `INSERT`/`UPDATE`/`DELETE`, `CREATE`/`DROP`/`ALTER`, `ATTACH`/`DETACH`, other `PRAGMA`s,
    transactions, `VACUUM` — is denied at prepare time;
  - `PRAGMA query_only=ON`, defensive mode, no triggers, untrusted schema, no `ATTACH`
    (`SQLITE_LIMIT_ATTACHED` 0), one statement per call, only `SELECT`/`WITH`/`VALUES` accepted;
  - double-quoted strings are identifiers only, so a misspelled `"column"` is an error, not a string;
  - limits: 2 s per query (progress handler), 1 MB per value, 100 KB of SQL, 200 rows returned and
    counting stops after 1,000,000 rows, SQLite heap capped at the loaded data + 512 MB.
- **Output**: every result is capped at ~12,000 characters.

These limits are covered by tests (`agent/src/sql.rs`, `agent/src/files.rs`, `cli/tests/mcp.rs`,
`core/tests/hostile_files.rs`).

## Memory and speed

The first open of a file reads and normalizes it (what `hedgehog normalize` does, without writing
any output file: no xlsx, CSV or manifest). The SQL tables are built on another thread as soon as
the file is normalized, beside the catalog, and the first `query` waits for them if it comes sooner.
A large sheet also uses a few threads of its own (its total detection beside its layout, column
typing on up to four threads). After that the file is in the [disk cache](#disk-cache): any later
session, in any process, gets the catalog in a few milliseconds and query answers in milliseconds.

| File (new process each time, best of 3) | First open: catalog / first query | Reopen from the cache: catalog / first query | Peak memory first / reopen |
|---|---|---|---|
| `eu_pt_opendata/001_contratos2020.xlsx` (40 MB, 4.5M cells) | 9.0 s / 10.1 s | 1 ms / 20 ms | 1.17 GB / 0.23 GB |
| `eu_ch_bfs/036_36752511_master.xlsx` (7 MB, 1.5M cells) | 3.6 s / 3.9 s | 1 ms / 4 ms | 0.55 GB / 15 MB |
| `enron/andrea_ring__3__HHmonthlyavg.xlsx` | 29 ms / 31 ms | 2.3 ms / 2.5 ms | 13 MB / 6 MB |
| `eu_at_opendata/003_pen_02_02.xlsx` | 12 ms / 13 ms | 2.3 ms / 2.6 ms | 11 MB / 6 MB |

Times are from the request to the response (the two large files re-measured after the agent-path
speed-ups, B120–B135, with the disk cache on; with it off the first query comes ~0.5 s sooner on
the 4.5M-cell file, since no cache entry is written). First opens of the ten largest test files (1.7–7M cells) take 1.8–9.6 s with
the cache off, 25–55 % less than in 0.4.0, at the same or lower peak memory; see the
[changelog](../CHANGELOG.md). A reopened file's normalized form (which
`describe_table`, `totals` and SQL error hints need) is read on another thread right after
`open_file` (about 1 s for the 4.5M-cell file). At most 8 files stay in memory per session; the
least recently used is dropped. Run `hedgehog mcp` with stderr visible to see per-call time and
memory.

## Disk cache

Normalized files are kept on disk so a new session does not normalize them again. An entry holds a
file's SQLite database (opened read-only in place), its normalized form and its catalog; it is keyed
by the file's sha256, the hedgehog build (a hash of the sources and the row model, so an upgrade
never reads an older build's results) and the normalize options. A file whose path, size and
modification time are unchanged is not even hashed again (`hashes.tsv`).

| | |
|---|---|
| Location | `$HEDGEHOG_CACHE_DIR`, else `$XDG_CACHE_HOME/hedgehog` or `~/.cache/hedgehog` (Linux), `~/Library/Caches/hedgehog` (macOS), `%LOCALAPPDATA%\hedgehog\cache` (Windows) |
| Off | `HEDGEHOG_CACHE=0` |
| Size cap | 2 GB (`HEDGEHOG_CACHE_MAX_MB`); the least recently used entries are removed first. A 4.5M-cell file takes ~360 MB. |
| Safety | entries are written to a temporary folder and renamed into place, so readers never see half an entry; an entry that fails a check (size, header, a database that does not open) is removed and the file normalized again |
| Privacy | the cache holds your data (cell values, file paths): folders are created `0700`, files `0600` (Unix). Nothing leaves the machine. Delete the folder to clear it. |

The Node `HedgehogSession` uses the same cache.

## Cancelling a call

`hedgehog mcp` runs tool calls on a worker thread, one at a time in arrival order, and keeps reading
input: `ping` and `tools/list` are answered at once, and `notifications/cancelled` with a
`requestId` drops a queued call or stops the running one. A running query is interrupted; a running
normalization is left to finish in the background (it is written to the disk cache, so asking again
is fast) while the call ends. A cancelled call gets no response.
