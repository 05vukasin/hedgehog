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
| `--max-cells N` | `20000000` | Workbooks with more non-empty cells are refused (memory grows with cells). |
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
| `open_file` | `path` | The catalog: file line, time coverage, tables with source range, rows, total rows, every column (type, unit; value ranges for up to 14 columns), all values of small label columns, mixed measures, group column, totals check, the rows of small tables as CSV, notes, warnings. Long catalogs list tables one line each. See [The catalog](#the-catalog). |
| `describe_table` | `path`, `table` | Every column (SQL name, dtype, unit, nulls, distinct, min..max, example or all values (≤ 12), source column ← header cells), source and header range, group column, total rows, entity columns (`<col>_id`, `<col>_name`), first 5 rows as CSV. |
| `query` | `path`, `sql`, `limit?` | One read-only SQLite `SELECT`/`WITH` as CSV with a header, at most `limit` rows (default 50, max 200), then `(<n> rows)` or `(<n> rows, showing first <k>)`. |
| `cells` | `path`, `sheet`, `range` | Original source cells (max 200) as `A1<TAB>value` lines, formulas after the value, merged areas marked. |
| `totals` | `path`, `table?`, `status?` | Counts per status (`ok`, `mismatch`, `unknown`), then up to 50 items, mismatches first: cell, label, value, recomputed, sources, reason. |
| `list_files` | `folder?`, `pattern?` | Spreadsheets under the folder (xlsx, xlsm, xlsb, xls, ods, csv, tsv) with size and modified date; max 200 lines. `pattern`: a glob (`*.csv`), words that must all appear (`linz 2022`) or alternatives (`preis\|price`). |

### SQL tables

For table *n*:

- `T<n>` — the data rows, one column per normalized column plus `_src_row` (the row number in the
  source sheet).
- `T<n>_all` — the same plus total and subtotal rows, with `_row_type` = `data`, `subtotal` or
  `total`. Use it for overall figures the file states itself (a "Gesamt" or "Total" row); never add
  those rows to the data rows.

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
`T<n>_all`, right after it: `<col>_id` (the code, as text) and `<col>_name` (the rest). Registers
write one entity under several spellings with the same code, so group by `<col>_id`, not by the text.
A value that is only a code has an empty `<col>_name`; a value without a code (`"- - Name"`) has an
empty `<col>_id`; a cell naming several entities (`"a | b"`, `"a; b"`, one per line, or run together
as `"501… - A, Lda 502… - B, SA"`) is not split: its `<col>_id` lists every ID as `"id1 | id2"`. The
catalog and `describe_table` describe each such column (see [The catalog](#the-catalog)); `cite as:`
maps `<col>_id` to the source cell it comes from.

### Example

`eu_at_opendata/003_pen_02_02.xlsx` (Austrian pension supplements, one sheet):

```text
FILE eu_at_opendata/003_pen_02_02.xlsx · 1 sheet · 244 cells (97% in tables) · 1 table · roles: model
TIME 2011–2025 (year, from column headers in T1)
T1  'AUSGLEICHSZULAGEN - alleinstehend/verheiratet' ← '2_2'!A1:R18 · 11 rows (+1 total row: 'Gesamt'@9 — only in T1_all) · 1 total row stays in T1 ('Gesamt'@15) — exclude it before summing
    columns (19): "group"[text group] Richtsatz[text] "2011".."2025"[15 × number] "2020/ 2025"[number] "2024/ 2025"[number]
    "group" values: "Anzahl Bezieher:innen" | "Durchschnittshöhe in Euro" | "Richtsätze in Euro"
    Richtsatz values: "Alleinstehend" | "davon Männer" | "davon Frauen" | "Verheiratet ***" | "Gesamt"
    rows mix measures by "group" — filter before aggregating
    group column: "group" · totals: 0 ok, 0 mismatch, 30 unknown
TOTALS 0 ok, 0 mismatch, 30 unknown (call totals for details)
NOTES 7 cells (note 5, title 2); e.g. '2_2'!A1: "AUSGLEICHSZULAGEN - alleinstehend/verheiratet"
WARNINGS none
SQL view T1 (data rows) and T1_all (+ total rows, _row_type); both have _src_row; quote names: SELECT "col name" FROM T1
(normalized in 0.0 s)
```

`query` with `SELECT "group", Richtsatz, "2024", "2025", _row_type, _src_row FROM T1_all WHERE Richtsatz IN ('Alleinstehend', 'Gesamt') ORDER BY _src_row`:

```text
group,Richtsatz,2024,2025,_row_type,_src_row
Anzahl Bezieher:innen,Alleinstehend,163230,162375,data,5
,Gesamt,190423,189537,total,9
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

A "no such column" whose name is the label of a total row (or a total word: Total, Gesamt, Summe,
Totale, Ukupno, Subtotal, …) says where the total rows are; one that is a value of a text column
says which column to filter. MySQL / Postgres / SQL Server functions and syntax SQLite lacks
(`YEAR`, `MONTH`, `DAY`, `DATE_FORMAT`, `TO_CHAR`, `EXTRACT(… FROM …)`, `DATEDIFF`, `DATE_ADD`, `NOW`,
`CONCAT`, `ISNULL`, `LEN`, `STRING_AGG`, `MEDIAN`/`PERCENTILE_CONT`, `STDDEV`, `TOP n`, `ILIKE`,
`::type`, `FETCH FIRST`, `INTERVAL`, …) get the SQLite replacement.

### The catalog

Per table (when the whole catalog fits in ~12,000 characters):

- **Head line**: id, title, source range, data rows; total rows taken out of the data (only in
  `T<n>_all`); total rows the totals check found that stay in the data (an "Insgesamt" row of a long
  table: exclude them before summing); the table's period when the file has several tables.
- **Columns**: every column. Up to 14 columns with their value range (numbers, dates) or an example;
  up to 40 as `name[dtype unit]`; runs of 4+ columns named by consecutive periods are written as one
  item without hiding any (`"2011".."2025"[15 × number]`, `"Thu / 2002-01-31".."Tue / 2002-01-01"[31
  × integer]`); wider tables also group runs of 3+ same-type columns (`describe_table` lists them).
- **Values**: text columns with at most 12 distinct values list all of them, so a model filters by
  the exact label.
- **Mixed measures**: `rows mix measures by "group" — filter before aggregating` when one label
  column separates counts, amounts, rates (by units and words in the labels, such as `Anzahl` and
  `in Euro`, or by numbers of very different size under each label).
- **Entity columns**: one line per `"<id> - <name>"` column: an example, how many IDs, how many
  have several spellings, cells with several entities, values without an ID, and to group by
  `<col>_id`: `adjudicante = "<id> - <name>" (e.g. "600082440 - Direção-Geral de …"): 3,872 IDs, 808
  with several spellings (e.g. 508481287: 5 names; up to 52) — group by adjudicante_id, not by the
  text; adjudicante_name holds the name · …`.
- **Total rows**: how to select them and where their label is, e.g.
  `total rows: SELECT … FROM T1_all WHERE _row_type = 'total' (label in col_C: 'Total')`; without a
  label column, their `_src_row` and sign (`_src_row 51 positive, 65 negative`).
- **Signs**: `signs: col_B 'Deliveries HH' rows are all negative (outflows?) — a net figure is the
  SUM over all rows, not a difference` when a block of rows holds only negative numbers next to
  positive ones (by a label column, else by `_src_row` ranges).
- **Small tables** (≤ 400 cells as rows × columns of `T<n>_all` with `_row_type`, ≤ 30 rows):
  every row as CSV under the table, total rows included and marked in a `_row_type` column (the
  values lines are then left out).
- **Amount columns** (tables with ≥ 30 data rows; not IDs, codes, years, period columns,
  percentages, averages or indexes): `numbers:` with sum (money and count columns), fill rate, share
  of zeros and range (when the columns line has no range); look-alike amount columns (both money, or
  similar names) whose sums differ get one line, e.g. `precoContratual (sum 11.16 bn, 100 % filled)
  vs PrecoTotalEfetivo (sum 4.28 bn, 100 % filled, 55 % zero) — different measures; pick
  deliberately`.
- **Quality** (tables with ≥ 20 data rows): `quality:` with duplicates in an ID column that is
  otherwise unique (≤ 5 % repeats), empty date cells and the label that explains them
  (`dataCelebracaoContrato empty in 26,722 rows (15 %): tipoprocedimento = 'Ajuste direto
  simplificado' (all its 26,261 rows) … — counts by this date leave them out`) and negative
  amounts (cells naming several entities are on the entity line).

The table lines come first; the optional parts are added while the catalog stays under its cap:
total-row and sign lines first, then small tables' rows, then amount and quality lines, each in
table order. They are computed in one pass per column (about 0.2 s on a 176k-row register).

When the full catalog is too long it lists every table on one line (`describe_table` gives the
columns); with very many tables the last line says which were cut.

The totals summary, total-row labels and rows, and the time coverage are the ones the
[manifest](normalized-output.md) has (`tables[].totals`, `time_coverage`), from the same functions.

### The catalog's TIME line

The manifest's [time coverage](normalized-output.md#time-coverage): the period the data covers
(`TIME 2020-01–2025-12 (month, from columns T3.Berichtszeitraum)`), at the finest granularity found,
from date / period columns or period-like column headers. When tables differ (a monthly and an
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
  - more than `--max-cells` cells (20 million by default). An ODS file's repeated rows and columns
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
- **SQL**: each file has its own in-memory database; after loading, the connection is locked down:
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

Opening a file costs what `hedgehog normalize` costs (without writing the output files), plus the
SQLite copy of the tables. The largest file in the test corpus (`eu_ch_bfs/036_36752511_master.xlsx`,
7.2 MB, 1.5 million cells, 5 tables) opens in about 20 s with a peak of ~790 MB (`normalize` alone:
~820 MB peak); a second `open_file` of the same content takes milliseconds. Small files open in
10–300 ms. At most 8 files stay in memory; the least recently used is dropped. Run
`hedgehog mcp` with stderr visible to see per-call time and memory.
