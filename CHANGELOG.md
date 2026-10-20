# Changelog

All notable changes to hedgehog are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/); before 1.0, minor versions may break the API.

## [Unreleased]

### Added

- `hedgehog-inspect` (Python entry point, `python -m hedgehog.inspect`) and a `Makefile`: see, file by
  file, the tables, totals check, notes, warnings, what an agent receives and the summary, with or
  without an LLM from `.env`, and ask the agent a question.
- European open-data sample: 31 sheets from 20 sources, every cell labelled (by an AI assistant,
  not yet reviewed by a person), scored with `hedgehog benchmark --corpora eu` (always the whole
  sample; rules → model, 7 roles per file 48.0 → 65.4, 4 roles per file 73.2 → 84.8). See
  [docs/evaluation.md](docs/evaluation.md#european-open-data-sample).
- `hedgehog cells <file> [--sheet NAME]`: every non-empty cell the reader sees, as TSV.
- **CSV and TSV input** for `open`, `normalize`, `describe` and the Python API. A `.csv`, `.tsv` or
  `.txt` file, or any file whose content is not a workbook, is read as one sheet named after the
  file stem. The delimiter (`,` `;` tab `|`) is detected with quotes respected (`.tsv` is always
  tab); RFC 4180 quoting; UTF-8 (BOM stripped), UTF-16 with a BOM, otherwise Windows-1252. Only
  unambiguous plain numbers (`42`, `-3.5e2`) are stored as numbers; `1.200,50`, `1,200.50`,
  `1.200`, `12%`, `007` and dates stay text and are read per column by the value normalizer, as
  for numbers stored as text in a workbook. Streamed: a 5M-cell, 41 MB CSV reads in about 1 s with
  0.4 GB peak memory. No new dependencies.
- StatSheets benchmark includes its 65 CSV sheets (all align; 21 in the test split). Test split,
  4 roles per file, rules / model: 65.2 / 89.6 → 65.6 / 89.6; pooled 48.0 / 72.2 → 48.1 / 72.3.

## [0.3.1] - pre-alpha

### Fixed

- Peak memory with the learned model on multi-sheet workbooks: sheets are analyzed in parallel
  only when they hold at most 300,000 cells together (was 2,000,000), so peak memory is set by the
  largest sheet. A 5-sheet, 1.5M-cell workbook: 1.76 GB → 0.90 GB (rules alone: 0.78 GB).
- Documented speed and memory figures re-measured on this build (365 files, rules → model: 0
  failures, median 19 → 29 ms, 95% under 1.5 → 2.5 s, slowest 16.8 → 16.6 s; a sheet near the
  1M-cell model limit needs up to ~0.75 GB more than the rules alone).

## [0.3.0] - pre-alpha

### Added

- **Learned row model, on by default** ([docs/learned-model.md](docs/learned-model.md)). The rules
  find the tables; a two-stage gradient-boosted tree model (176 + 476 trees, built into the
  library, no GPU or extra dependency) decides what kind of row each table row is, with safeguards
  that keep the rules' answer where it is the safer call. Test splits, rules → model, cell roles
  per file: DECO 64.4 → 70.9 (all seven roles pooled 57.1 → 67.3), StatSheets 65.2 → 89.6,
  Strudel 77.1 → 90.5, synthetic 91.5 → 98.0; no role more than 0.1 below the rules. It also beats
  the rules on each benchmark when that benchmark is left out of its training.
- `--rules-only` / `rules_only=True`, and `--model-max-cells` / `model_max_cells` (default
  1,000,000 cells per sheet; larger sheets use the rules alone and get a manifest warning);
  manifest `options.cell_roles`.
- **Cross-corpus benchmark** `hedgehog benchmark`: DECO, StatSheets (14 statistics publishers, 6
  languages), Strudel (converted with `tools/benchmarks/convert_strudel.py`) and synthetic, per
  source, in seven roles and in the four roles all corpora share.
- Training tooling: `hedgehog features [--level row]` (training data, computed by the same code as
  the library), `hedgehog pack-model`, `tools/train/` (leave-one-corpus-out, two-stage training,
  tree export verified against XGBoost).

### Performance

- The model adds about 11 ms to a typical file (median 19 → 30 ms on 365 real files); feature
  groups and the sheets of a workbook run on parallel threads within a memory budget; the largest
  file (4.5M cells) stays at 17 s and 1.2 GB (see 0.3.1 for multi-sheet memory).
- Faster value features for the rules path too: repeated texts and numbers are parsed once per
  sheet (golden output identical).

## [0.2.0] - pre-alpha

### Added

- **Layout v2**: change-point segmentation of stacked tables whose schema changes down the sheet;
  row-label and group-label columns separated from the numbers by blank columns join their table;
  outline key columns become group headers and are filled down. DECO dev split: table F1 at
  IoU ≥ 0.5 75.1 (0.1.0: 74.1), cell-role file-macro F1 54.7 (0.1.0: 50.4); held-out test split
  65.5 / 53.6. Synthetic corpus: tables 98.0, stacked schema changes found 86.8%.
- **Noise generator** (`tools/noise`, M5): 51 operators in 4 profiles turn clean tables into messy
  workbooks with exact ground truth; `hedgehog eval synthetic` scores the analyzers on them.
- `HEDGEHOG_TRACE=1` prints the time and peak memory of every normalize stage to stderr.

### Changed

- **Manifest schema version 2**: three or more consecutive row totals in one column that all check
  out are listed as one run (`cell` like `AB6:AB10494`, new `count` field; also a trailing `count`
  column on the `Totals` sheet). Mismatched and unverified totals are still listed one by one.
  Summary counts (`totals.count`, `ok`, `mismatch`, `unknown`) count totals, not items.
- A wide, sparse table (header, key columns filled on at least 90% of 20+ rows) is kept as a table
  instead of being dropped by the density rule.
- Identical total labels on three or more adjacent rows ("Totaal van aantal eenheden" in a
  long-format register) are a category value, not total rows.

### Performance

- The normalized xlsx is streamed to disk row by row (low-memory worksheets, temp file next to the
  output) instead of being built in memory: peak memory on a workbook with 632k formulas went from
  3.5 GB to 750 MB, and its manifest from 192 MB to 0.8 MB.
- Totals detection is near-linear on huge sheets (column views, bitsets, one keyword check per
  distinct text) and layout block merging only compares blocks that share rows. On the 315 European
  open-data files: all 315 now normalize (0.1.0: one timed out after 180 s), the slowest in 20 s, median 20 ms.

## [0.1.0] - pre-alpha

First public version. Python distribution `hedgehog-sheets` (import `hedgehog`), Rust crates
`hh_core` and `hh_cli` (not published on crates.io).

### Added

- **Normalized output** (`hedgehog normalize`, `hedgehog.normalize`): writes
  `<stem>.normalized.xlsx` with a README sheet, one typed sheet per detected table (flat header,
  group column, `_src_row`, registered as an Excel table), a `Totals` sheet and a `Notes` sheet, plus
  a deterministic `<stem>.manifest.json` (schema version 1). Options: `--keep-totals`, `--csv`,
  `--locale`.
- **Reader** for xlsx, xlsm, xlsb, xls and ods via calamine, with the format detected from the file's
  content; repairs workbooks that list a hidden sheet with an empty relationship id. Faster streaming
  read of cells and merged ranges.
- **Table detection and cell roles** (rule-based `layout` module): several tables per sheet, titles,
  notes, multi-row and merged headers, stacked tables, total and group-header rows. DECO dev split:
  table F1 at IoU ≥ 0.5 of 74.1 (baseline 17.3), cell-role file-macro F1 of 50.4 (baseline 19.9).
- **Value parsing** (`values` module): numbers in US, European, Swiss and space-separated formats,
  accounting negatives, percentages, ISO 4217 currencies and symbols, units, scale words; dates,
  months, quarters and ISO weeks with month names in nine European languages; missing-value tokens,
  Excel errors and booleans; per-column inference of decimal separator and day/month order; locale
  presets (`us`, `uk`, `de`, `fr`, `es`, `it`, `nl`, `pt`, `pl`, `ch`, `rs`, `iso`).
- **Totals** (`totals` module): detection of formula and typed totals, subtotals and grand totals;
  verification against cached values, including double-counted subtotals.
- **Python API**: `open` and `Workbook` (`sheet_names`, `describe`, `sheets`, `get_cell`,
  `get_range`, `find_text`, `totals`), `normalize` and `NormalizeResult`, `summarize` and `Summary`,
  `facts`, `facts_json`, `check_citations`.
- **Optional LLM layer** (`hedgehog.llm`): one `ChatClient` for OpenAI, OpenAI-compatible servers
  (vLLM, Ollama, LM Studio, OpenRouter, Groq), Anthropic and AWS Bedrock; API keys as arguments,
  environment variables or an explicit `.env` file; retries with backoff; LLM-phrased summaries with
  citation checking; a tool loop (`ask`) and model comparison (`bench`). Extras `[openai]`,
  `[anthropic]`, `[bedrock]`, `[llm]`.
- **CLI**: `hedgehog describe`, `normalize`, `eval deco` (with `--threads`), `bench`, `golden`.
- **Evaluation harness**: DECO loader, fixed 70/30 dev/test split, table P/R/F1 at IoU and EoB
  thresholds, per-role and file-macro F1, JSON reports.
- **Packaging**: `abi3` wheels for Linux x86_64 and aarch64, macOS universal2 and Windows x64, built
  and smoke-tested by the release workflow and attached to GitHub Releases; CI with tests and
  clippy.
- **Documentation** in `docs/`: installation, quickstart, CLI and Python reference, normalized
  output, agent recipes, LLMs, locales, concepts, evaluation, architecture, development,
  troubleshooting.

[Unreleased]: https://github.com/05vukasin/hedgehog/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/05vukasin/hedgehog/releases/tag/v0.1.0
