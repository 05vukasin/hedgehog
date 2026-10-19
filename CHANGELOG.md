# Changelog

All notable changes to hedgehog are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/); before 1.0, minor versions may break the API.

## [Unreleased]

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
