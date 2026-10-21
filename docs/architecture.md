# Architecture

How the code is organized: the Rust core and its modules, the PyO3 bindings, the Python layer, the
CLI, and how data flows between them.

## Overview

```
                ┌──────────────────────── hh_core (Rust, core/) ────────────────────────┐
 .xlsx .xlsm    │                                                                        │
 .xlsb .xls ───▶│ reader ─▶ model ─▶ grid ─▶ layout ─▶ values ─▶ totals ─▶ normalize ──▶ │──▶ .normalized.xlsx
 .ods .csv      │                     │                                       │          │    .manifest.json
                │                     └─▶ features (context, values, totals)  └─ write   │    .csv
                │                         eval (scores any analyzer)                     │
                └───────────────┬───────────────────────────────────┬────────────────────┘
                                │ PyO3 (py/src/lib.rs)              │
                ┌───────────────▼──────────────┐       ┌────────────▼──────────┐
                │ hedgehog (Python, py/python/) │       │ hh_cli (cli/)          │
                │ open, normalize, summarize,   │       │ hedgehog describe |    │
                │ facts, check_citations, llm   │       │ normalize | eval       │
                └───────────────────────────────┘       └───────────────────────┘
```

Design rules:

- **All analysis is in Rust.** The core is deterministic: no network, no LLM, no randomness. It
  reads only cached values and never evaluates formulas.
- **The bindings are thin.** `py/src/lib.rs` only looks things up and converts results to Python
  dicts and lists.
- **Python holds the optional and fast-changing parts:** LLM providers, prompts, citation checks,
  and later an MCP server.
- **Measured.** Every detection change is scored with `eval` before and after
  ([evaluation.md](evaluation.md)).

## Repository layout

| Folder | Cargo package | What |
|---|---|---|
| `core/` | `hh_core` | The Rust library. Everything below. |
| `py/` | `hh_py` | PyO3 bindings (`py/src/`, compiled to `hedgehog._native`) and the Python package (`py/python/hedgehog/`). Published as `hedgehog-sheets`. |
| `cli/` | `hh_cli` | The `hedgehog` command (`cli/src/main.rs`), including the MCP server `hedgehog mcp` (`cli/src/mcp.rs`). |
| `agent/` | `hh_agent` | The agent tools as a Rust API (`Session`: catalog, describe_table, read-only SQL over SQLite, cells, totals, list_files, path bounds); used by `hedgehog mcp` and the Node package. Keeps SQLite out of `hh_core`. |
| `node/` | `hh_node` | napi-rs bindings over `hh_core` and `hh_agent` (`node/src/lib.rs`), the JS entry point and types (`index.js`, `index.d.ts`) and Node tests. Published on npm as `hedgehog-sheets` ([node-api.md](node-api.md)). |
| `docs/` | | This documentation. |
| `assets/` | | Logo. |
| `.github/workflows/` | | CI and release workflows ([development.md](development.md#continuous-integration)). |

The Cargo package names start with `hh_` because a crate named `core` would clash with Rust's
built-in `core` library. Users only see `import hedgehog` and the `hedgehog` command.

## Rust core modules

| Module | Role |
|---|---|
| `reader` | Opens xlsx, xlsm, xlsb, xls and ods through [calamine](https://crates.io/crates/calamine) into the model. Repairs workbooks that list a hidden sheet with an empty relationship id. calamine gives no styles: for xlsx / xlsm, `reader/styles.rs` resolves `xl/styles.xml` into one packed `Format` per cell (bold, borders, fills, font size, alignment, number format kind) during the byte scan of each sheet that also finds the merged areas (`reader/merges.rs`). CSV / TSV is read by its own streaming reader (`reader/csv.rs`) as one sheet: only unambiguous plain numbers become numbers, the rest stays text for `values`. |
| `model` | `Workbook → Sheet → Cell { row, col, value, formula }` plus merged ranges. `Value` is number, text, bool, date (Excel serial) or error. Coordinates are 0-based. |
| `grid` | Fast lookups over one sheet: cell at (row, col), merges, used range. |
| `analysis` | The output types of layout: `Role` (seven cell roles), `DetectedTable`, `SheetLayout`. |
| `layout` | Rule-based table detection and cell roles: blocks, titles and notes, header depth, stacked tables, totals and group headers. Thresholds in `layout/params.rs`. |
| `values` | Value parsing: numbers in any convention, currencies (ISO 4217), dates and periods, missing tokens, booleans, text cleanup and Cyrillic transliteration, keyword lists, `Locale` presets, per-column convention inference. |
| `totals` | A small formula reference parser, total detection (formula, keyword, typed), verification against cached values, and a dependency index for tracing. |
| `context` | Spatial features of every cell (neighbours, row and column statistics, block geometry). |
| `features` | Named per-cell feature columns (`c_*` context, `v_*` values, `t_*` totals, `l_*` rule layout, `x_*` total context). |
| `learned` | The learned row model: row segments and their features, the tree evaluator (packed nodes, block evaluation), the two-stage `RowModel` embedded from `core/models/row_model.bin`, and `analyze()` (rules + model + safeguards). See [learned-model.md](learned-model.md). |
| `normalize` | Builds the normalized workbook: per table a flat header, typed columns, group column, `_src_row`; the `Totals` and `Notes` sheets; the manifest. Writes xlsx with [rust_xlsxwriter](https://crates.io/crates/rust_xlsxwriter). |
| `eval` | Ground-truth schema; DECO, StatSheets, Strudel/labelled and synthetic loaders; dev/test split; metrics; baseline analyzer; the runners behind `hedgehog eval` and `hedgehog benchmark`; training-data export (`export`, `export_rows`). |

Public entry points (re-exported in `core/src/lib.rs`):

```rust
let wb = hh_core::open("sales.xlsx")?;                         // Workbook
print!("{}", hh_core::describe(&wb));                          // one line per sheet
let opts = hh_core::NormalizeOptions::default();             // rules + learned model
let out = hh_core::normalize_file("sales.xlsx", "out/", &opts)?;   // writes files
let mem = hh_core::normalize_workbook(&wb, &opts);             // same result in memory
let layout = hh_core::layout::analyze(&hh_core::Grid::new(&wb.sheets[0]));
```

The Rust API is not published on crates.io and may change with every release.

## Data flow inside `normalize`

Per sheet, each step computed once and reused:

1. `Grid::new(sheet)`: index the cells.
2. `learned::analyze(&grid, model)`: tables from `layout::analyze`, a role for every cell from the
   rules refined by the row model (`layout::analyze` alone with `rules_only` or above
   `model_max_cells`). Sheets of a workbook are analyzed on parallel threads when their cells
   total at most 300,000.
3. `totals::detect` and `totals::verify`: total candidates and their checks.
4. `values::sheet_column_hints_with(&grid, locale)`: decimal separator and day/month order per
   column, or the explicit locale.
5. Per table (`normalize/extract.rs`, `typing.rs`): flatten the header, type each column, turn group
   rows into a column, move total rows to `Totals` and everything else to `Notes`, count every
   transformation.
6. `normalize/write.rs` and `manifest.rs`: write the xlsx (README, tables as Excel tables, Totals,
   Notes), optional CSVs and the manifest JSON (fixed key order, no timestamps).

## Python layer

| Module | Contents |
|---|---|
| `hedgehog._native` | The compiled extension: `Workbook`, `open`, `normalize`. |
| `hedgehog.normalize` | `normalize()` and `NormalizeResult` (parses the manifest, loads tables with pandas). |
| `hedgehog.facts` | Compact facts about a workbook, A1 helpers. |
| `hedgehog.summary` | `summarize()`, `Summary`, the deterministic markdown summary, facts trimming for LLMs. |
| `hedgehog.citations` | `check_citations()`: finds and verifies cell references in model text. |
| `hedgehog.llm` | Optional providers (OpenAI and compatible servers, Anthropic, Bedrock) behind one `ChatClient`, `.env` handling, the workbook tools, `ask` and `bench`. |

The wheel uses the stable ABI (`abi3`, CPython 3.9 and newer), so one wheel per platform covers every
Python version. `normalize` releases the GIL while it runs.

## Planned components

Not built yet; see the roadmap in the [README](../README.md#status-and-roadmap):

- semantic column types (city, country, tax ID, currency amount, period, ...);
- metric definitions that become Excel formulas;
- an MCP server exposing the agent tools;
- styles from xls and ods files (xlsx / xlsm styles are read).

## See also

- [concepts.md](concepts.md): what each stage does, without the code
- [development.md](development.md): building and testing
- [`PLAN.md`](../PLAN.md): the milestone plan
