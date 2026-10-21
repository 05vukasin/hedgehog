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
hedgehog mcp [--root DIR] [--allow PATH]... [--query-timeout-ms N] [--max-file-mb N]
```

| Flag | Default | Meaning |
|---|---|---|
| `--root DIR` | current folder | Folder the tools may read. Paths are relative to it. |
| `--allow PATH` | none | One more file or folder outside the root (repeatable). |
| `--query-timeout-ms N` | `2000` | Time limit of one SQL query. |
| `--max-file-mb N` | `256` | Larger files are refused. |

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
| `describe_table` | `path`, `table` | Every column (SQL name, dtype, unit, nulls, distinct, min..max, example or all values (≤ 12), source column ← header cells), source and header range, group column, total rows, first 5 rows as CSV. |
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

An unquoted keyword or a guessed column name gets a hint instead of a bare error:

```text
SQL error: near "group": syntax error (at character 7) — group is a column name that is also an SQL keyword; quote it: "group"
SQL error: no such column: "Dezember / 2021" (at character 7) — did you mean T1."De-zember / 20211)", T1."No-vember / 2021", …? (quote names with double quotes)
```

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
- **Small tables** (≤ 15 data rows, ≤ 12 columns): all data rows as CSV under the table, added in
  table order while the catalog stays under its cap (the values lines are then left out).

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

These limits are covered by tests (`agent/src/sql.rs`, `agent/src/files.rs`, `cli/tests/mcp.rs`).

## Memory and speed

Opening a file costs what `hedgehog normalize` costs (without writing the output files), plus the
SQLite copy of the tables. The largest file in the test corpus (`eu_ch_bfs/036_36752511_master.xlsx`,
7.2 MB, 1.5 million cells, 5 tables) opens in about 20 s with a peak of ~790 MB (`normalize` alone:
~820 MB peak); a second `open_file` of the same content takes milliseconds. Small files open in
10–300 ms. At most 8 files stay in memory; the least recently used is dropped. Run
`hedgehog mcp` with stderr visible to see per-call time and memory.
