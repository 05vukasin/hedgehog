<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/logo-dark.png">
    <img src="assets/logo/logo.png" alt="hedgehog logo: a hedgehog whose back is a gear" width="160">
  </picture>
</p>

<h1 align="center">hedgehog</h1>

<p align="center">
  <b>Give it a messy Excel file. Get back clean tables that any program or AI agent can use right away.</b>
</p>

<p align="center">
  <a href="https://github.com/05vukasin/hedgehog/actions/workflows/ci.yml"><img src="https://github.com/05vukasin/hedgehog/actions/workflows/ci.yml/badge.svg?branch=master" alt="CI status"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

---

## The problem

Real spreadsheets are made for people to look at, not for programs to read. They have titles above
the data, headers spread over two or three merged rows, "Region: North" rows that split a table
into groups, subtotal rows in the middle, numbers typed as text (`"1.200,50"`, `"12%"`, `"(500)"`),
dates in every format, and notes at the bottom. Load such a file with `pandas.read_excel` and you
get a broken table: the title becomes the header, subtotals are added twice, numbers stay text.

**hedgehog fixes this automatically.** It reads the file, works out where every table is and what
every cell means, and writes a **clean copy**: one tidy sheet per table with a single header row and
real numbers and dates, plus a **manifest** (a JSON file) that explains what it found and what it
changed. The original file is never modified.

## What it fixes

Measured on **365 real files**: 315 public spreadsheets from European government open-data portals
and statistics offices (Austria, Switzerland, Germany, France, the Netherlands, Portugal, Spain and
others) and 50 Enron business spreadsheets. Together: 3,952 sheets and 43.5 million cells.

| Problem in the original | What hedgehog does | How many | Files |
|---|---|---:|---:|
| Numbers stored as text: `"1,200.50"`, `"1.200,50"`, `"1 200"`, `"(500)"` | Reads them as real numbers, in US or European style | 1,153,826 cells | 70 |
| Extra spaces around values | Trims them | 1,075,912 cells | 214 |
| Formulas, which other programs may not be able to compute | Keeps the calculated value | 993,834 cells | 73 |
| Placeholders for "nothing": `-`, `n/a`, `#N/A` | Turns them into proper empty values | 681,098 cells | 154 |
| Dates written as text: `31.12.2024`, `12/31/24`, `March 2024` | Turns them into real dates | 141,986 cells | 28 |
| Merged cells (the value is only in the first one) | Fills the value into every cell it covers | 57,213 cells | 120 |
| A header spread over two or three rows ("Revenue" above "Q1", "Q2") | One header row: `Revenue / Q1`, `Revenue / Q2` | 20,805 headers | 149 |
| The same header twice, or no header | Renames to `Code (2)`, names the empty ones | 26,975 headers | 187 |
| "Region: North" rows splitting a table into groups | Turns them into a `Region` column and fills it down | 7,094 group rows | 123 |
| Titles, notes and sources mixed in with the data | Moves them to a `Notes` sheet, with their cell address | 5,957 rows | 184 |
| Blank rows and empty columns inside tables | Removes them | 7,472 | 156 |
| Total and subtotal rows inside the data | Moves them to a `Totals` sheet and **re-calculates** them | 668,029 totals | 184 |

Of those 668,029 totals, 604,939 add up and **4,132 do not** (in 30 files); the rest cannot be
checked (for example a total without the cells it adds). Each one that does not
add up is listed with its cell, the value in the file and the value the cells add up to, so a person
or an agent can check it. 98.3% of all cells ended up in clean tables. The rest (titles, notes,
totals) is kept on separate sheets with the original cell address, so **nothing is thrown away**.

## How it works

```
 your file ──▶ 1. read ──▶ 2. find the tables ──▶ 3. read the values ──▶ 4. check totals ──▶ 5. write
               every cell,   where each table       numbers, dates and     re-add the cells      clean .xlsx
               merged cell   starts and ends,       percentages, in US     a total covers and    + manifest
               and formula   what each cell is      or European format     compare               + summary
```

1. **Read** the workbook: `.xlsx`, `.xlsm`, `.xlsb`, `.xls` or `.ods`, or a CSV / TSV file (delimiter
   and encoding detected, numbers left for step 3 to read in the column's format).
2. **Find the tables.** A sheet can hold several tables, side by side or stacked, even with
   different columns. Every cell is labelled as a title, header, data, group header, total, note
   or other: rules find the tables, then a small built-in model decides what kind of row each row
   is ([how](docs/learned-model.md)).
3. **Read the values** the way a person would. Each column is judged as a whole, so `1.200` is one
   thousand two hundred in a German column and one point two in a US column. When hedgehog cannot be
   sure, it says so in the manifest instead of guessing silently.
4. **Check the totals.** Every `SUM` formula and every row labelled "Total", "Summe", "Totaal",
   "Ukupno" and similar is re-calculated from the cells it covers.
5. **Write** the clean workbook, the manifest and, if asked, a short summary.

hedgehog is written in Rust, so it is fast: a typical file takes about 30 milliseconds,
and the largest, with 4.5 million cells, takes about 20 seconds. Same file in, same result out; it
needs no internet and no GPU: the learned model is a set of decision trees built into the library.
A large language model is optional and only used to phrase a summary.

## What you get

For `sales.xlsx`, hedgehog writes `sales.normalized.xlsx` and `sales.manifest.json`:

```
 Original sheet "Sales"                          sales.normalized.xlsx, sheet "T1_sales_report_2024"
 ───────────────────────────────────────         ──────────────────────────────────────────────────────────────
 Sales report 2024                               Region  Product  Revenue / Q1  Revenue / Q2  Units  Margin  _src_row
 Product   Revenue          Units   Margin       North   Widgets        1200.5          1350   1200    0.12         6
           Q1       Q2                           North   Gadgets         300.0           410    350   0.085         7
 Region: North                                   South   Widgets         980.0          1150            0.10        10
 Widgets   1200.5   1350    1,200   12%          South   Gizmos         2045.0          1720   2100    0.15        11
 Gadgets   300      410     350     8.5%
 Subtotal  1500.5   1760                         Totals sheet: B13 "Grand total" = 4600,
 Region: South                                   but B6:B12 add up to 4525.5  → mismatch
 Widgets   980      1150    n/a     10%
 Gizmos    2045     1720    2,100   15%          Notes sheet: A1 title, A15 source note
 Subtotal  3025     2870
 Grand total 4600   4630
 Source: internal ERP export, amounts in USD
```

- **One sheet per table**, with one header row, numbers as numbers and dates as dates.
- **`_src_row`** on every row: its row number in the original, so every value can be traced back
  to the original cell.
- **`Totals` sheet:** every total, whether it adds up, and what it should be.
- **`Notes` sheet:** titles, notes and every other cell that is not table data.
- **`README` sheet:** what each sheet is and how to load it.
- **Manifest:** the same as JSON, plus each column's type, its original header cells, the number
  format used and a count of every change made.

Now `df["Revenue / Q1"].sum()` is simply right.

## Installation

Prebuilt for Linux, macOS and Windows (Python 3.9 or newer). **No Rust needed.** The package is
called **`hedgehog-sheets`**; in Python you `import hedgehog`.

```sh
pip install hedgehog-sheets --find-links https://github.com/05vukasin/hedgehog/releases/expanded_assets/v0.3.1

# With an optional AI provider for summaries: [openai], [anthropic], [bedrock], or [llm] for all three
pip install "hedgehog-sheets[openai]" --find-links https://github.com/05vukasin/hedgehog/releases/expanded_assets/v0.3.1
```

It is not on PyPI yet; once it is, `pip install hedgehog-sheets` will be enough.

The command-line tool: download `hedgehog-<target>` from a
[release](https://github.com/05vukasin/hedgehog/releases) (from the next release on), or
`cargo install --git https://github.com/05vukasin/hedgehog hh_cli` (needs Rust). From a clone: `pip install ./py`. Details: [docs/installation.md](docs/installation.md).

For Node.js and TypeScript (Node 18 or newer), the same package name on npm, with prebuilt native
binaries and types (from the next release on; until it is on npm, the release has the tarballs):

```sh
npm install hedgehog-sheets
```

## Use it from the command line

```console
$ hedgehog normalize sales.xlsx -o out/
sales.xlsx: 1 tables, 39 cells (28 in tables, 9 totals, 2 notes) in 16 ms
  out/sales.normalized.xlsx
  out/sales.manifest.json
```

Useful options: `--locale de-DE` (the file is German; also `fr-FR`, `en-US`, …), `--keep-totals`
(keep total rows in the tables, marked in a `_row_type` column), `--csv` (also one CSV per table).
`hedgehog describe sales.xlsx` prints a one-line overview per sheet. All commands:
[docs/cli.md](docs/cli.md).

## Use it from Python

```python
import hedgehog

r = hedgehog.normalize("sales.xlsx", "out/")         # same files as the command line
df = r.read("T1")                                      # the first table as a pandas DataFrame
print(df["Revenue / Q1"].sum(), r.manifest["totals"]["mismatch"], "wrong total(s)")

wb = hedgehog.open("sales.xlsx")                       # look inside the original, read-only
print(wb.get_cell("Sales", "B13")["value"])            # one cell
print(wb.get_range("Sales", "A6:C7")["rows"])          # a range, row by row
print(wb.find_text("grand total")[0]["address"])       # search, ignoring case and accents
print([(t["address"], t["ok"]) for t in wb.totals("Sales")][-2:])   # totals, re-calculated
```

```text
4525.5 1 wrong total(s)
4600.0
[['Widgets', 1200.5, 1350.0], ['Gadgets', 300.0, 410.0]]
A13
[('B13', False), ('C13', True)]
```

A short summary for a person or an AI agent, made without AI, or phrased by an AI model in a single
call (every cell it mentions is checked):

```python
import hedgehog

summary = hedgehog.summarize("sales.xlsx").text          # markdown: problems first
print(next(line for line in summary.splitlines() if "B13" in line))
```

```text
- Sales!B13 "Grand total": 4,600, but B6:B12 adds up to 4,525.5 (diff +74.50; mismatch)
```

```python
import hedgehog
from hedgehog import llm

client = llm.connect("openai", api_key="sk-...")       # or "anthropic", "ollama", "vllm", "bedrock", ...
s = hedgehog.summarize("sales.xlsx", llm=client)       # one call; s.text, s.usage, s.warnings
```

More: [docs/python-api.md](docs/python-api.md), [docs/llm.md](docs/llm.md).

## Use it from TypeScript

The npm package `hedgehog-sheets` has the same core, with camelCase names; slow work runs off the
event loop and returns a Promise. It also has the agent tools of the [MCP server](#as-an-mcp-tool-server)
in-process, for a TypeScript agent that registers its own tools:

```ts
import { normalize, open, HedgehogSession } from 'hedgehog-sheets'

const r = await normalize('sales.xlsx', 'out/')          // same files as the command line
console.log(r.manifest.tables[0].sheet, r.manifest.totals.mismatch, 'wrong total(s)')

const wb = open('sales.xlsx')                            // look inside the original, read-only
console.log(wb.getCell('Sales', 'B13').value, wb.totals('Sales').at(-2)?.ok)

const s = new HedgehogSession({ root: '.' })             // open_file, query, cells, totals, ...
console.log(await s.query('sales.xlsx', 'SELECT Product, "Revenue / Q1" FROM T1 ORDER BY 2 DESC LIMIT 2'))
```

```text
T1_sales_report_2024 1 wrong total(s)
4600 false
Product,Revenue / Q1
Gizmos,2045
Widgets,1200.5
(2 rows)
```

More: [docs/node-api.md](docs/node-api.md).

## Use it with an AI agent

This is what hedgehog was built for: an agent that can run Python gets clean tables and a map of the
file instead of a raw spreadsheet. Give it `out/sales.normalized.xlsx` and
`out/sales.manifest.json`; it only needs pandas:

```python
import json
import pandas as pd

m = json.load(open("out/sales.manifest.json", encoding="utf-8"))
t = m["tables"][0]                                     # source sheet, ranges, columns, types
df = pd.read_excel("out/sales.normalized.xlsx", sheet_name=t["sheet"])
col = {c["name"]: c["source_column"] for c in t["columns"]}

i = df["Revenue / Q1"].idxmax()                        # point back to the original cell via _src_row
print(df.loc[i, "Product"], f'{t["source"]["sheet"]}!{col["Revenue / Q1"]}{df.loc[i, "_src_row"]}')
print([x["cell"] for x in m["totals"]["items"] if x["status"] == "mismatch"])
```

```text
Gizmos Sales!B11
['B13']
```

More recipes (types, totals, notes, writing corrections back): [docs/agent-recipes.md](docs/agent-recipes.md).

### As an MCP tool server

An agent that cannot run Python can use hedgehog through the
[Model Context Protocol](https://modelcontextprotocol.io): `hedgehog mcp --root <folder>` serves six
tools over stdio — `open_file` (a compact catalog of the tables, time coverage and totals check),
`describe_table`, `query` (read-only SQL over the normalized tables, `T1_all` with the total rows),
`cells` (source cells to cite), `totals` and `list_files`. Every answer is at most ~3k tokens, so a
model can work with files that would never fit in its context. Claude Desktop:

```json
{ "mcpServers": { "hedgehog": { "command": "hedgehog", "args": ["mcp", "--root", "/path/to/spreadsheets"] } } }
```

Standalone `hedgehog` binaries for Linux, macOS and Windows are attached to each
[release](https://github.com/05vukasin/hedgehog/releases). Tools, limits and examples:
[docs/mcp.md](docs/mcp.md).

**Claude Code plugin:** with `hedgehog` on your PATH, run `/plugin marketplace add 05vukasin/hedgehog`
and `/plugin install hedgehog@hedgehog` in Claude Code. It adds the tools for the project folder and a
`spreadsheets` skill that tells Claude to answer from the catalog and SQL instead of writing pandas.
On the four cases in [eval/agents](eval/agents/RESULTS.md), that took a third to two thirds less cost
for the same answers. See [docs/mcp.md](docs/mcp.md#claude-code).

## How good is it?

Accuracy is measured, not guessed: hedgehog is scored against spreadsheets that people labelled by
hand, cell by cell, on files it never saw while it was built.

| Benchmark | What it is | Cell types, rules alone | **Cell types, hedgehog 0.4** |
|---|---|---:|---:|
| DECO | Enron business spreadsheets | 65.5 | **71.5** |
| StatSheets | 14 statistics offices (US, France, Japan, Australia, OECD, WHO, ...), 6 languages | 66.9 | **89.6** |
| Strudel | US government and scientific tables | 78.5 | **90.8** |
| Synthetic | 2,000 generated messy workbooks with known answers | 93.7 | **98.1** |

Scores are F1 from 0 to 100 (finding the right things without false alarms), averaged per file over
title, header, data and other. Table detection on DECO: 75.0 (a simple "connected cells" approach:
17.3). The learned model also beats the rules on each benchmark when that benchmark is left out of
its training entirely, so the gain carries over to new kinds of files. A misplaced cell still ends
up on the `Notes` sheet, so a mistake costs structure, never data. Details:
[docs/learned-model.md](docs/learned-model.md), [docs/evaluation.md](docs/evaluation.md).

**Messy layouts.** An independent noise generator (21 kinds of mess: "of which" rows, average rows,
footnote marks, locale formats, separators, side-by-side tables, ...) builds workbooks whose right
answer is known. hedgehog recovers every value in its place in 100 % of the clean ones and 89–91 %
at the highest noise level, with 0 crashes ([eval/noise/REPORT.md](eval/noise/REPORT.md)). A fuzzer
and a sweep over 24,000 real files found 0 crashes; output is byte-identical from run to run.

**With an AI agent: the tool does the work.** On 30 real analyst questions (finance, sales, HR,
public statistics, registers; 200 cells to 4.5M cells; 4 questions that need two files), a simple
chat loop on a small model with only hedgehog's tools passes **86 %** of the automatic checks; Claude
Code (a frontier model writing its own Python) passes 97 %. hedgehog does it **2.9× faster and 51×
cheaper** ($0.21 for all 30 cases vs $10.45) and reads 5× fewer tokens. With the same small model and
the same minimal loop, hedgehog's tools score 85 % on the 30 cases; on the first 4 cases, the other
tool sets scored 27 % (a generic Excel MCP server), 45 % (a pandas tool) and 55 % (the sheet as
markdown in the prompt). Every run, answer and check: [eval/agents](eval/agents/RESULTS.md);
an earlier walk-through: [docs/case-study-contracts.md](docs/case-study-contracts.md).

## Documentation

| | |
|---|---|
| [Installation](docs/installation.md) | wheels, extras, command line, from source |
| [Quickstart](docs/quickstart.md) | a messy sample workbook, end to end |
| [Agent recipes](docs/agent-recipes.md) | pandas/openpyxl code for AI agents |
| [MCP server](docs/mcp.md) | `hedgehog mcp`: tools for AI clients over stdio |
| [Claude Code plugin](docs/mcp.md#claude-code) | `/plugin install hedgehog@hedgehog`: the tools plus a `spreadsheets` skill |
| [Normalized output](docs/normalized-output.md) | every sheet and manifest field |
| [CLI](docs/cli.md) · [Python API](docs/python-api.md) · [Node API](docs/node-api.md) | full reference |
| [LLMs](docs/llm.md) · [Locales](docs/locales.md) | AI providers and keys; numbers, dates, currencies |
| [Concepts](docs/concepts.md) · [Evaluation](docs/evaluation.md) | how it works; how well it works |
| [Case study: 176k contracts](docs/case-study-contracts.md) | hedgehog tools + gpt-4o-mini vs Claude Code on one real file |
| [Architecture](docs/architecture.md) · [Development](docs/development.md) · [Troubleshooting](docs/troubleshooting.md) | for contributors and when something goes wrong |

## Roadmap

Done (0.1 to 0.3):

- [x] Reading every common spreadsheet format; finding tables and labelling every cell
- [x] Numbers, dates, percentages and currencies in US and European formats
- [x] Finding and re-calculating totals
- [x] Clean workbook, manifest and summary; optional AI summaries
- [x] Stacked tables with different columns; row-label and group columns
- [x] A generator of messy test workbooks (51 kinds of mess)
- [x] Multi-million-cell files with modest memory
- [x] CSV and TSV input
- [x] A learned row model (gradient-boosted trees in Rust, no GPU) combined with the rules, tested on
      four independent benchmarks (0.3)

Next:

- [ ] Formatting features (bold, fills, borders, number formats) for the model
- [ ] Tables rebuilt from the model's cell roles
- [ ] Column meaning: city, country, tax ID, amount, period, …
- [ ] Ready-made formulas and metrics (quarterly totals, year over year)
- [ ] An MCP server, so agents can call hedgehog as a tool
- [ ] PyPI release

Milestones: [PLAN.md](PLAN.md). Changes: [CHANGELOG.md](CHANGELOG.md).

## Contributing

Issues and pull requests are welcome. Please:

- when a file is read wrongly, include a small (anonymized) workbook or the code that builds one;
  never commit real spreadsheets;
- run `cargo test --workspace --exclude hh_py`, `cargo clippy --workspace -- -D warnings` and the
  Python tests before opening a pull request;
- for changes to detection, parsing or totals, include `hedgehog eval` scores before and after.

Setup and conventions: [docs/development.md](docs/development.md).

## License

MIT, see [LICENSE](LICENSE).
