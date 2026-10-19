<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/logo-dark.png">
    <img src="assets/logo/logo.png" alt="hedgehog logo: a hedgehog whose back is a gear" width="160">
  </picture>
</p>

<h1 align="center">hedgehog</h1>

<p align="center">
  <b>Turns messy Excel workbooks into clean, typed tables and a manifest that AI agents can work with.</b>
</p>

<p align="center">
  <a href="https://github.com/05vukasin/hedgehog/actions/workflows/ci.yml"><img src="https://github.com/05vukasin/hedgehog/actions/workflows/ci.yml/badge.svg?branch=master" alt="CI status"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

---

hedgehog reads a real-world spreadsheet (titles above the data, multi-row merged headers, group rows,
subtotals, numbers stored as text in US or European format) and writes a **normalized copy**: one
clean sheet per table with a flat header and typed columns, every total listed and recomputed, every
other cell kept with its address, plus a **JSON manifest** describing it all. An agent with a code
sandbox loads it with one `pandas.read_excel` call and can trace every value back to its source cell.

The analysis is deterministic Rust with Python bindings. LLMs are optional and only phrase summaries.

> **Status: pre-alpha (0.1.0).** Normalization, table detection, value parsing and totals
> verification work and are tested; accuracy is measured and [published](docs/evaluation.md). The
> API may still change.

## Why

A typical report, and what `pd.read_excel` makes of it versus what hedgehog writes:

```
 Source sheet "Sales"                              Normalized sheet "T1_sales_report_2024"
 ───────────────────────────────────────────       ──────────────────────────────────────────────────────────────
 Sales report 2024                                 Region  Product  Revenue / Q1  Revenue / Q2  Units  Margin  _src_row
 Product   Revenue          Units   Margin         North   Widgets        1200.5          1350   1200    0.12         6
           Q1       Q2                             North   Gadgets         300.0           410    350   0.085         7
 Region: North                                     South   Widgets         980.0          1150            0.10        10
 Widgets   1200.5   1350    1,200   12%            South   Gizmos         2045.0          1720   2100    0.15        11
 Gadgets   300      410     350     8.5%
 Subtotal  1500.5   1760                           Totals sheet: B13 "Grand total" = 4600,
 Region: South                                     but B6:B12 add up to 4525.5  → mismatch
 Widgets   980      1150    n/a     10%
 Gizmos    2045     1720    2,100   15%            Notes sheet: A1 title, A15 source note
 Subtotal  3025     2870
 Grand total 4600   4630
 Source: internal ERP export, amounts in USD
```

Read naively, the title becomes the header, subtotals get summed twice, `"1,200"` and `"12%"` stay
text, and nobody notices that the grand total is wrong. After normalizing, `df["Revenue / Q1"].sum()`
is simply right.

## Installation

Prebuilt wheels for Linux, macOS and Windows (CPython 3.9+), no Rust toolchain needed. The package is
**`hedgehog-sheets`**; you `import hedgehog`.

```sh
# From GitHub Releases (available from the first release, v0.1.0)
pip install hedgehog-sheets --find-links https://github.com/05vukasin/hedgehog/releases/expanded_assets/v0.1.0

# From PyPI: coming soon
pip install hedgehog-sheets

# Optional LLM providers: [openai], [anthropic], [bedrock] or [llm] for all three
pip install "hedgehog-sheets[openai]" --find-links https://github.com/05vukasin/hedgehog/releases/expanded_assets/v0.1.0
```

From source (needs Rust): `pip install ./py` in a clone. The `hedgehog` command-line tool is built
separately: `cargo install --git https://github.com/05vukasin/hedgehog hh_cli`.
Details: [docs/installation.md](docs/installation.md).

## Quickstart: command line

```console
$ hedgehog describe sales.xlsx
Sales: A1:E15 · 39 cells · 0 formulas · 4 merged ranges
$ hedgehog normalize sales.xlsx -o out/
sales.xlsx: 1 tables, 39 cells (28 in tables, 9 totals, 2 notes) in 16 ms
  out/sales.normalized.xlsx
  out/sales.manifest.json
```

Options: `--keep-totals` (keep total rows, marked in `_row_type`), `--csv` (one CSV per table),
`--locale de-DE` (fix the decimal separator and date order). Development commands: `hedgehog eval`
(accuracy on an annotated corpus) and `hedgehog bench` (speed). See [docs/cli.md](docs/cli.md).

## Quickstart: Python

```python
import hedgehog

r = hedgehog.normalize("sales.xlsx", "out/")         # same files as the CLI
df = r.read("T1")                                      # pandas DataFrame of the first table
print(df["Revenue / Q1"].sum(), r.manifest["totals"]["mismatch"], "wrong total(s)")

wb = hedgehog.open("sales.xlsx")                       # the source workbook, read-only
print(wb.get_cell("Sales", "B13")["value"])            # one cell
print(wb.get_range("Sales", "A6:C7")["rows"])          # a range, row by row
print(wb.find_text("grand total")[0]["address"])       # case- and accent-insensitive search
print([(t["address"], t["ok"]) for t in wb.totals("Sales")][-2:])   # totals, recomputed
```

```text
4525.5 1 wrong total(s)
4600.0
[['Widgets', 1200.5, 1350.0], ['Gadgets', 300.0, 410.0]]
A13
[('B13', False), ('C13', True)]
```

A summary, deterministic and free, or phrased by an LLM in one call with every cited cell checked:

```python
import hedgehog

summary = hedgehog.summarize("sales.xlsx").text          # markdown: sheets, totals, problems first
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

See [docs/python-api.md](docs/python-api.md) and [docs/llm.md](docs/llm.md).

## Using it from an agent sandbox

Give the agent `out/sales.normalized.xlsx` and `out/sales.manifest.json`. It needs only pandas:

```python
import json
import pandas as pd

m = json.load(open("out/sales.manifest.json", encoding="utf-8"))
t = m["tables"][0]                                     # source sheet, ranges, columns, dtypes
df = pd.read_excel("out/sales.normalized.xlsx", sheet_name=t["sheet"])
col = {c["name"]: c["source_column"] for c in t["columns"]}

i = df["Revenue / Q1"].idxmax()                        # cite the original cell via _src_row
print(df.loc[i, "Product"], f'{t["source"]["sheet"]}!{col["Revenue / Q1"]}{df.loc[i, "_src_row"]}')
print([x["cell"] for x in m["totals"]["items"] if x["status"] == "mismatch"])
```

```text
Gizmos Sales!B11
['B13']
```

More recipes (dtypes, totals, notes, writing corrections back): [docs/agent-recipes.md](docs/agent-recipes.md).

## Features

- **Reads** xlsx, xlsm, xlsb, xls and ods (format detected from content); values, formulas with
  cached results, merged cells.
- **Finds tables** on every sheet, several per sheet, with titles above and notes below, and labels
  every cell: title, header, data, group header, total, note, other.
- **Flattens headers**: multi-row and merged headers become `Revenue / Q1`; names are made unique.
- **Parses values** stored as text: `1,200.50`, `1.200,50`, `1 200,50`, `(500)`, `12%`, `$1,200`,
  `1.200 €`, units, `5k`; dates in numeric forms and with month names in nine European languages, quarters, ISO weeks;
  missing tokens (`-`, `n/a`, `#N/A`); booleans. US and European conventions are equal citizens;
  ambiguous values (`1.200`, `03/04/2024`) are decided per column and flagged, never silently.
- **Verifies totals**: SUM/SUBTOTAL formulas and typed total rows are recomputed from the data;
  double-counted subtotals and wrong grand totals are reported.
- **Keeps provenance**: `_src_row`, source column letters and original header cells for every value;
  every other cell on a `Notes` sheet; the source file's SHA-256 in a deterministic manifest.
- **Agent tools**: `get_cell`, `get_range`, `find_text`, `totals`, and an optional LLM tool loop with
  citation checking (OpenAI, OpenAI-compatible servers, Anthropic, Bedrock).

## How it works

```
 workbook ─▶ read ─▶ detect tables ─▶ parse values ─▶ verify totals ─▶ normalize ─▶ .normalized.xlsx
            cells,   & cell roles     per-column      recompute from   flat header,  + manifest.json
            merges,                   locale          cached values    typed cols,   + summary
            formulas                                                   Totals, Notes
```

Everything above runs in the Rust core: no network, no LLM, same input gives the same bytes.
[docs/concepts.md](docs/concepts.md) explains each stage; [docs/architecture.md](docs/architecture.md)
the code.

## Documentation

| | |
|---|---|
| [Installation](docs/installation.md) | wheels, extras, CLI, from source |
| [Quickstart](docs/quickstart.md) | a messy sample workbook, end to end |
| [Agent recipes](docs/agent-recipes.md) | pandas/openpyxl code for sandbox agents |
| [Normalized output](docs/normalized-output.md) | sheets and manifest schema |
| [CLI](docs/cli.md) · [Python API](docs/python-api.md) | full reference |
| [LLMs](docs/llm.md) · [Locales](docs/locales.md) | providers and keys; numbers, dates, currencies |
| [Concepts](docs/concepts.md) · [Evaluation](docs/evaluation.md) | how it works; how well it works |
| [Architecture](docs/architecture.md) · [Development](docs/development.md) · [Troubleshooting](docs/troubleshooting.md) | for contributors and when things go wrong |

## Status and roadmap

Measured on the annotated DECO corpus of real Enron spreadsheets ([details](docs/evaluation.md)):
table detection F1 at IoU ≥ 0.5 is **74.1** on the dev split (flood-fill baseline: 17.3; held-out
test split: 64.1), cell-role macro F1 **50.4** (baseline 19.9). The reader opened 500 of 500 randomly
sampled Enron files.

Done in 0.1.0:

- [x] Reader and cell model; `describe`
- [x] Evaluation harness (DECO, table and role metrics, `hedgehog eval`)
- [x] Rule-based table detection and cell roles
- [x] Locale-aware value parsing (numbers, currencies, dates, missing values)
- [x] Totals detection and verification
- [x] Normalized workbook + manifest (`normalize`), deterministic summary
- [x] Optional LLM layer: summaries, tool loop, model benchmark

Planned:

- [ ] Noise generator for labelled synthetic messy sheets (in progress)
- [ ] Learned cell-role model (gradient-boosted trees, XGBoost or LightGBM, evaluated in Rust)
- [ ] Semantic column types (city, country, tax ID, currency amount, period, ...)
- [ ] Metrics and formulas (metric definitions that become Excel formulas)
- [ ] MCP server exposing the agent tools
- [ ] PyPI release

Milestones and their "done when" criteria: [PLAN.md](PLAN.md). Changes: [CHANGELOG.md](CHANGELOG.md).

## Contributing

Issues and pull requests are welcome. Please:

- include a small (anonymized) workbook or the openpyxl code that builds one when reporting a misread
  file; never commit spreadsheets (tests generate them);
- run `cargo test --workspace --exclude hh_py`, `cargo clippy --workspace -- -D warnings` and the
  Python tests before opening a PR;
- for changes to detection, parsing or totals, include `hedgehog eval` scores before and after.

Setup and conventions: [docs/development.md](docs/development.md).

## License

MIT, see [LICENSE](LICENSE).
