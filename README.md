<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/logo-dark.png">
    <img src="assets/logo/logo.png" alt="hedgehog logo: a hedgehog whose back is a gear" width="160">
  </picture>
</p>

<h1 align="center">hedgehog</h1>

<p align="center">
  <b>Turns messy business spreadsheets into clean, understood data that AI agents can trust.</b><br>
  Rust core · Python bindings · pre-alpha
</p>

---

> **Status: pre-alpha.** Today hedgehog reads xlsx / xlsm / xlsb / xls / ods into one cell model
> and prints a `describe` overview. Everything else below is being built right now or is on the
> roadmap, and is marked as such. The API will change.

## The problem

Real spreadsheets are not tables. A typical finance or operations workbook has:

- a title, a date and a note above the data, and footnotes below it
- several tables on one sheet, side by side or stacked
- multi-row headers built from merged cells (`2024` over `Q1 Q2 Q3 Q4`)
- subtotal and total rows mixed into the data, group-header rows like `Region: North`
- numbers stored as text, `(500)` for negatives, `1.200 RSD`, "amounts in thousands"
- US and European conventions mixed across files: `1,200.50` vs `1.200,50` vs `1 200,50`, `12/31/2024` vs
  `31.12.2024`, month names and "Total" labels in many languages, Latin and Cyrillic in one column

Give that to an LLM as CSV or a screenshot and it guesses. It miscounts columns, sums the
subtotals twice and reads `1.200` as one point two.

## What hedgehog does for an agent

Planned pipeline, run once per workbook, deterministic, no LLM calls inside:

1. **Find the tables**: table ranges on every sheet, including titles and notes around them.
2. **Label every cell**: title, header, data, group header, total, note.
3. **Normalize values**: locale-aware numbers, units and currency, dates, missing-value tokens.
4. **Verify totals**: recompute every total/subtotal row and flag the ones that don't add up.
5. **Summarize**: a short markdown summary (tables, columns, key numbers, warnings) where
   every number carries its cell reference, e.g. `Revenue 2024 = 1.234.000 (Sheet1!F43)`.
6. **Answer through tools**: `describe`, `get_cell`, `find`, `aggregate`, `metric`, `trace`,
   `sql`. Every answer returns the source cells it came from, so the agent can cite them.

The agent reads a summary of under 1k tokens instead of the whole file, then calls tools for
exact answers.

## What works today

```sh
$ hedgehog describe finansijski-plan-rashoda-2026.xlsx    # a municipal budget (Serbian open data)
PLAN RASHODA: A1:I745 · 3650 cells · 0 formulas · 255 merged ranges
```

```python
import hedgehog

wb = hedgehog.open("finansijski-plan-rashoda-2026.xlsx")
wb.sheet_names          # ['PLAN RASHODA']
print(wb.describe())
```

Measured on a random sample of 500 Enron spreadsheets, the reader opens **500 / 500** at about
**17 ms per file** (including process start). 12 of them list a `veryHidden` sheet with an empty
`r:id` (a leftover VBA module sheet from xls to xlsx conversion) that calamine rejects; hedgehog
strips that entry in memory and reads the rest of the workbook.

## Roadmap

Every milestone ships with a number measured on the eval harness, before and after.

- [x] **M0 Skeleton + cell model**: `Workbook → Sheet → Cell{value, type, formula, merge}`, reader via calamine, `describe`, Python import
- [ ] **M1 Eval harness** (in progress): one label schema for cell roles and table ranges, DECO converter, table-detection P@IoU and cell-role F1, `hedgehog eval`
- [ ] **M2 Layout rules** (in progress): table detection with gap tolerance, titles/notes, multi-row and merged headers, total and subtotal rows, group-header rows
- [ ] **M3 Value normalizer** (in progress): `1.200,50`, `(500)`, units and currency, mixed date formats, `-` / `n/a` / `#N/A`, Cyrillic ↔ Latin
- [ ] **M4 Output**: tables as Arrow / pandas, JSON workbook map, markdown summary with cell refs and warnings (e.g. wrong totals)
- [ ] **M5 Noise generator**: ~15 operators that turn clean datasets into realistically messy sheets, with full labels
- [ ] **M6 Learned cell-role model**: gradient-boosted trees trained in Python, evaluated in Rust by our own tree evaluator
- [ ] **M7 Semantic column types**: city / country, currency amount, date / period, tax ID (PIB / MB), person / company, category, percent
- [ ] **M8 Metrics & formulas**: metric definitions that become Excel formulas and queries, cleaned xlsx with a Review sheet, cached profiles
- [ ] **M9 Agent tools / MCP server**: `describe`, `get_cell`, `find`, `aggregate`, `metric`, `trace`, `sql`
- [ ] **M10 Connectors & speed**: Google Sheets, OneDrive / SharePoint, S3, email attachments; streaming reader and parallelism if profiling asks for it

Details and "done when" criteria: [`PLAN.md`](PLAN.md).

## Architecture

```
            ┌────────────── hh_core (Rust) ──────────────┐
 xlsx/xls/  │ reader → cell model → layout → values →    │   PyO3    ┌── hedgehog (Python) ──┐
 xlsb/ods ─▶│ totals → summary          (+ eval)         │ ────────▶ │ open(), describe(),   │──▶ agent
            └────────────────────────────────────────────┘           │ later: MCP server     │
                          ▲                                          └───────────────────────┘
                     hh_cli: `hedgehog describe` (dev tool)
```

| Folder | Crate | What |
|---|---|---|
| `core/` | `hh_core` | Rust library: reader, workbook model, feature table, and the `layout`, `values`, `totals`, `eval`, `context` workstreams |
| `py/` | `hh_py` | PyO3 bindings (`src/`) and the `hedgehog` Python package (`python/hedgehog/`) |
| `cli/` | `hh_cli` | the `hedgehog` command, for development |

The Rust core has no network access and makes no LLM calls, so every result is
deterministic and testable. Connectors and the MCP server live in the Python layer.

## How it is tested

- **Unit tests on generated fixtures.** Tests build their xlsx files with `rust_xlsxwriter`
  at test time, so no spreadsheets are committed.
- **Accuracy on real sheets.** Enron spreadsheets with DECO annotations (cell roles and table
  ranges) are the ground truth for table detection and cell-role scores.
- **Synthetic messy data.** A noise generator takes clean datasets and adds titles, merged
  headers, subtotals, locale formats and so on, keeping a label for every change. This gives
  unlimited labelled data, covering US and European number/date conventions and languages that public corpora lack.
- **Corpus robustness runs.** The reader and pipeline run over large random samples of real
  files to catch crashes and measure speed (the 488/500 number above comes from one).
- **Agent Q&A end to end.** An agent answers questions about a workbook using only hedgehog's
  tools, and the answers are checked against the known cells.

Test corpora are several GB, live outside the repo in `../testdata`, and are never committed.

## Why Rust

Parsing and analysing large workbooks cell by cell is CPU-bound work that should take
milliseconds, not seconds. Rust gives that speed with predictable memory, one core that can be
shipped as a Python wheel today and to other languages later, and a type system that keeps a
pipeline with many small rules honest. Python stays where it is best: training models,
generating test data, connectors and the agent-facing layer.

## Develop

```sh
cargo test --workspace --exclude hh_py      # Rust tests (fixtures are generated, no xlsx in git)
cargo run -p hh_cli -- describe file.xlsx

uv venv .venv && uv pip install --python .venv/bin/python maturin
(cd py && ../.venv/bin/maturin develop)     # add --release for the optimized build
.venv/bin/python -c "import hedgehog; print(hedgehog.open('file.xlsx').describe())"
```

Test corpora live outside the repo in `../testdata` and are never committed.
