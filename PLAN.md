# Hedgehog — build plan

Goal: take any messy spreadsheet → find the tables, understand what every cell and column means,
normalize the data, add the formulas/metrics it obviously needs, and hand the next AI agent a short
summary plus tools it can call (MCP-style) to get exact, cell-traceable answers instantly.

Rule for every milestone: **it ships with a number.** Each step is measured on the eval harness
(M1) before and after, so we always know whether a change helped.

## Layout

```
hedgehog/
  core/   Rust: workbook reader, cell model, detection, normalization, summary
  py/     PyO3 bindings (`src/`) + Python package (`python/hedgehog/`, later: MCP server, connectors)
  cli/    `hedgehog describe|eval|bench <file>` for development
  tools/
    noise/           noise generator (Python, fast to iterate) + labels
    train/           model training (Python) → exports tree JSON loaded by Rust
  docs/              research, decisions (ADRs)
```
Folders are plain `core/`, `py/`, `cli/`. Cargo package names can't be just `core` (it clashes with
Rust's built-in `core` library), so inside `Cargo.toml` they are `hh_core`, `hh_py`, `hh_cli`;
users only ever see `import hedgehog` and the `hedgehog` command.
Test corpora stay outside the repo (`../testdata`, path via `HEDGEHOG_DATA`), they are GBs.

## Milestones

| # | Milestone | Delivers | Done when |
|---|---|---|---|
| M0 | Skeleton + cell model | Workspace above; `Workbook → Sheet → Cell{value, type, style id, formula, merge}`; reader via calamine behind a `Reader` trait | `hedgehog describe file.xlsx` prints sheets/cells; Python import works |
| M1 | Eval harness | One label schema (cell roles + table ranges); converter from DECO (local, Enron); metrics: table-detection P@IoU, cell-role macro F1; `hedgehog eval` | Baseline score of the current flood-fill printed for DECO |
| M2 | Layout rules v0 | Table detection with gap tolerance; titles/notes above & below; multi-row + merged headers (unmerge, fill, `Parent / Child` names); total & subtotal rows (keywords + `SUM(` + arithmetic check); group-header rows → new column | DECO table-detection & role F1 beat M1 baseline; published next to literature numbers (~0.8 P@IoU, ~0.9 role F1) |
| M3 | Value normalizer v0 | Locale-aware numbers (`1.200,50`, `(500)`), units/currency (`1.200 RSD`, "in thousands"), dates (mixed formats, serials, dd/mm inference), missing tokens (`-`, `n/a`, `#N/A`), whitespace/case/Cyrillic↔Latin | Unit tests generated per operator all pass; typed columns on DECO tables |
| M4 | Output v0 (first thing agents use) | `hedgehog.open(path)` → tables as Arrow/pandas; JSON **workbook map**; markdown **summary** (tables, columns, key numbers with cell refs, warnings like wrong totals) | Summary of a 10k-row Enron file < 1k tokens, generated in < 1 s |
| M5 | Noise generator v0 | ~15 operators (missing, categorical variants, numbers-as-text, locale, units, dates, whitespace; hierarchical headers, subtotals, group rows; offset, title, notes, multi-table; merges/styles) with full provenance labels; frequencies calibrated to Enron profile + DECO | 10k labelled synthetic sheets generated from `testdata/clean`; M2/M3 scored on them |
| M6 | Learned cell-role model | Gradient-boosted trees (XGBoost or LightGBM — whichever scores higher, both export trees) on ~65 features; trained in Python, inference in Rust with our own tree evaluator; tables derived from cell roles | Beats M2 rules on real DECO test split (train on synthetic+DECO train) |
| M7 | Semantic column types | City/country/municipality (GeoNames + Serbian list), currency amount, date/period, ID/tax ID (PIB/MB), person/company, category, percent…; value detectors + header embeddings + boosted-tree classifier | Column-type F1 measured on synthetic (known types) + small hand-labelled real set |
| M8 | Metrics & formulas | Metric definitions (one JSON def → Excel `SUMIFS` formulas + query); recipe catalogue (date+measure → quarterly/YoY, dimension+measure → group-by/share, totals → check); writes cleaned xlsx + "Review" sheet; profile embedded in xlsx + cache keyed by content hash | Re-opening the same/updated file reuses the profile; quarterly question answered from stored metric in ms |
| M9 | Agent tools / MCP server | `describe`, `get_cell`, `find`, `aggregate`, `metric`, `trace`, `sql` — every answer returns source cells | An agent answers "sum of invoices in D43"-type questions via tools only |
| M10 | Connectors & speed | Data sources (Google Sheets, OneDrive/SharePoint, S3, email attachments…) in Python plugin layer; own streaming xlsx reader (zip + quick-xml) replacing calamine if profiling shows it's the bottleneck; rayon parallelism | Benchmarks on the 1 GB Enron set |

Order rationale: M1 before any algorithm (no number → no progress). M2–M4 are rules only and already
useful to an agent. ML (M6, M7) comes once there is labelled data (M5) to train and measure it on —
the research shows rules reach ~90% and boosted trees close most of the remaining gap.

## Known issues
- calamine refuses ~2.4% of Enron xlsx (500-file sample): workbook lists a `veryHidden` sheet with
  empty `r:id=""` (leftover VBA module sheet from xls→xlsx conversion) → "Relationship not found".
  Fix: strip such `<sheet>` entries in memory before handing the zip to calamine, or our own reader (M10).
- calamine gives no cell styles (bold, borders, fills, number formats) — needed for M6 features.

## Not blocking yet (decide by M8–M10)
- Connectors: data sources, LLM providers, or both?
- Output: add sheets to the original file vs. new clean workbook (recommendation: add, never overwrite user data).
- May the pipeline call an LLM (cost/privacy)? Design works without one.
- Deployment: pip library, MCP server, hosted service.

## Engineering rules
- Heavy jobs run via `scripts/run_capped.sh` (memory cap); scripts are resumable and write incrementally.
- Downloaded corpora are untrusted: never executed, read only by parsers.
- Rust core has no network and no LLM calls — deterministic and testable.
