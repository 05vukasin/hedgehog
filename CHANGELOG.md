# Changelog

All notable changes to hedgehog are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/); before 1.0, minor versions may break the API.

## [Unreleased]

### Added

- **Catalog facts** (`open_file`): small tables in full by a cell budget (≤ 400 cells, ≤ 30 rows)
  from `T<n>_all` with a `_row_type` column, so total rows are visible (a 12 × 19 pension table is
  now inlined); per table with total rows, how to select them (`total rows: SELECT … FROM T1_all
  WHERE _row_type = 'total' (label in col_C: 'Total')`); sign conventions (`signs: col_B
  'Deliveries HH' rows are all negative …`); for registers, sums / fill / range of amount columns
  with a line for look-alike money columns (`precoContratual (sum 11.16 bn …) vs PrecoTotalEfetivo
  (sum 4.28 bn …) — different measures; pick deliberately`) and a `quality:` line (duplicate IDs,
  empty date cells and the category they belong to, negative amounts). Added
  within the catalog's cap, total-row and sign lines first. See [docs/mcp.md](docs/mcp.md#the-catalog).

- **Claude Code plugin** (`integrations/claude-code`, marketplace in `.claude-plugin/marketplace.json`):
  `/plugin marketplace add 05vukasin/hedgehog` then `/plugin install hedgehog@hedgehog` registers
  `hedgehog mcp` for the project folder and adds a `spreadsheets` skill (open the file first, trust
  the catalog, SQL for numbers, check what one row is, verify, cite cells, pandas only as a
  fallback). The agent comparison's `claude-hh` arm now runs Claude Code with this plugin
  (`--skill-budget` sets the skill-listing budget). See [docs/mcp.md](docs/mcp.md#claude-code).
- Agent comparison: amounts written as `€2.41B` are read as billions.

- **Entity columns in the agent tools' SQL** (`hedgehog mcp`, Node, hedgehog-cli): a text column
  whose values are `"<id> - <name>"` (a tax number, a CPV / NUTS code before the name) gets
  `<col>_id` and `<col>_name` in `T<n>` and `T<n>_all`, and a catalog / `describe_table` line with
  the number of IDs, how many have several spellings and to group by `<col>_id`. On the Portuguese
  2020 contracts register (176k rows) the top buyer, written 6 ways, is one group of €737.7 M
  instead of €507.2 M + €229.0 M; multi-winner cells (943) keep every ID (`"id1 | id2"`); a
  `GROUP BY adjudicante_id` takes ~0.2 s, like grouping by the text.
- **SQL error hints for other dialects and row labels**: `no such function: MONTH` (and `YEAR`,
  `DATE_FORMAT`, `TO_CHAR`, `EXTRACT`, `DATEDIFF`, `NOW`, `CONCAT`, `ISNULL`, `LEN`, `STRING_AGG`,
  `MEDIAN`, …), `TOP n` and `ILIKE` errors name the SQLite way (`strftime('%m', col)`, `LIMIT n`,
  `LIKE`); the `query` tool description says it is SQLite with date examples. A column name that is a
  total-row label (`"Total"`, `"Gesamt"`, …) says the total rows are in `T<n>_all` with
  `_row_type = 'total'` and which column holds the label; a value of a text column used as a column
  name says which column to filter.
- **Shared row labels**: a grid of numbers with no labels of its own, beside or below a table
  whose rows it repeats one for one (same data rows, total rows and blank rows; at least 4 data
  rows), gets copies of that table's label columns, named like the originals, with no
  `source_column`, counted as `row_labels_copied` and named in a table warning. The Enron Henry Hub
  report's daily grids (`T2`–`T7`) now say which pipeline and flow each row is, so "total receipts
  per day in January" (23 January, 606,356) and "the pipeline with the highest single-day receipts"
  are SQL queries. On the 315 EU files and the 50 Enron benchmark files, 2 tables get labels (an
  Istat `segue` block beside its region names, a block of measurements beside its sample ids); on
  a sample of 399 other Enron files, 5 (price, quote and position grids beside their names).
- **Tables from the model's row roles** (default path; `--rules-only` unchanged): title and note
  rows at the edges leave the table, header-only blocks join the table below as its header (ABS
  time series), pieces the rules cut off a list rejoin it instead of taking a data row for a
  header, headers deepen or shorten where the model disagrees with the rules, stacked tables split
  at a new header after a blank row, key/value metadata blocks stop being tables and data rows
  left below a table join it. Table F1 on the test splits at IoU ≥ 0.5 / 0.8 / 1.0: DECO
  65.5 / 51.6 / 34.2 → 68.2 / 56.0 / 36.7, StatSheets 83.6 / 72.5 / 36.8 → 91.5 / 86.9 / 74.6,
  synthetic 98.1 / 95.7 / 82.4 → 98.1 / 97.2 / 94.8; cell roles unchanged. See
  [docs/learned-model.md](docs/learned-model.md#how-it-works).
- Column-numbering rows (`1 2 3 4 …` under a Destatis header) are part of the header, not the
  first data row (rules and model path).
- Block labels in a second or third label column (`Receipts HH` / `Deliveries HH` once per block,
  next to a column of names) are group keys and are filled down, so every row says which block it
  belongs to.
- `hedgehog benchmark` reports table precision / recall / F1 (IoU 0.5 / 0.8 / 1.0, EoB) per corpus
  and per source for DECO, StatSheets and synthetic, and how often a found table's header rows are
  exactly right; the TSV gets `table_f1_iou50/80/100`. Debug aid: `core/tests/tables_gtdump.rs`.
- **Cell formatting from xlsx / xlsm styles**: `Sheet::formats` holds one packed `Format` per cell
  (bold, italic, underline, font colour, size, fill, borders, alignment, indent, wrap, number
  format kind), read from `xl/styles.xml` and the cells' `s` attributes in the pass that already
  scans each sheet for merged areas (4 bytes per cell; other formats: empty, never guessed).
  Opt-in: `reader::open_with(path, ReadOptions { formats: true })`; plain `open` reads them only
  when the embedded model uses them (not yet), so read time and memory are unchanged. 14
  `s_*` features for the learned model, relative to the sheet where that matters. In
  leave-one-corpus-out training they help every held-out corpus (four roles per file, mean of three
  seeds: DECO 75.2 → 75.6, StatSheets 79.9 → 80.1, Strudel 86.6 → 87.3, synthetic 94.3 → 94.7), but
  the retrained model did not beat the shipped one on the European sample, so the shipped model is
  unchanged and does not read them yet. See
  [docs/learned-model.md](docs/learned-model.md#formatting-features).
- Training scripts: `--style-dropout`, `--exclude`, `--seed`; features are loaded into one float32
  matrix file by file and XGBoost gets `QuantileDMatrix`es, so training on the four corpora fits in
  4 GB (it needed more).
- **`hedgehog mcp`**, an MCP tool server over stdio (JSON-RPC 2.0, protocol `2025-06-18`, older
  versions accepted) with six tools: `open_file` (compact catalog with time coverage, tables, total
  rows, columns and totals check), `describe_table`, `query` (read-only SQLite SQL over `T1`… and
  `T1_all`… with total rows), `cells`, `totals` and `list_files`. Files are normalized in memory and
  cached per sha256; file access is bounded by `--root` plus allowed paths; SQL runs under an
  authorizer, `query_only`, a 2 s time limit and row/size caps; every result is at most ~12,000
  characters. See [docs/mcp.md](docs/mcp.md).
- New crate `hh_agent` (`agent/`): the same tools as a plain Rust API (`Session::open_file`,
  `query`, …, or `Session::call(name, json_args)`), for the MCP server and future bindings.
  SQLite is compiled in (`rusqlite`, bundled); `hh_core` stays free of it.
- Releases attach standalone `hedgehog` CLI binaries (`hedgehog-<target>[.exe]` for x86_64/aarch64
  Linux, x86_64/aarch64 macOS and x86_64 Windows) with `SHA256SUMS.txt`.
- **Node.js / TypeScript package `hedgehog-sheets`** (new crate `hh_node`, `node/`, napi-rs 3), with
  types and prebuilt addons for Linux x64/arm64 (glibc), macOS x64/arm64 and Windows x64 as
  `hedgehog-sheets-<platform>` optional dependencies: `normalize` (libuv thread pool, returns a
  Promise with the paths and the parsed, typed manifest) and `normalizeSync`; `open` / `openAsync`
  and `Workbook` (`sheetNames`, `sheets`, `getCell`, `getRange`, `findText`, `totals`), the Python
  API in camelCase; `HedgehogSession`, the six `hedgehog mcp` tools in-process over `hh_agent`
  (`openFile`, `describeTable`, `query`, `cells`, `totals`, `listFiles`, `call`, `toolSpecs`). The
  release workflow builds and tests the addons, attaches the npm tarballs to the GitHub Release and
  publishes to npm when the repository variable `PUBLISH_NPM` is `true`. See
  [docs/node-api.md](docs/node-api.md).

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
- Manifest **`time_coverage`**, per table and for the workbook: `{from, to, granularity, source,
  columns}` from date columns, year columns (`Jahr`, `année`, `año`, ...), period text (`2024 Q1`,
  `Jan 2024`, `2024-03`) or period-like column headers (`2019 | 2020 | 2021`, `Oktober / 2022`).
  Additive; `schema_version` stays 2. See
  [normalized-output.md](docs/normalized-output.md#time-coverage).
- Manifest **per-table `totals`**: counts by status and `rows` (one per total row: `row`, `label`,
  `level`, `kept_in_data`), so an agent sees which rows are not in a table's data. Every
  `totals.items[]` entry also has `row`, the 1-based source row.
- Column names are cleaned for agents (`original_header` unchanged): footnote marks dropped
  (`20221)` → `2022`, `Rate3)` → `Rate`, `Total²` → `Total`), line-break hyphenation joined
  (`De-⏎zember` → `Dezember`, `Sep-tember` → `September`, `Benützungs- gebühren` →
  `Benützungsgebühren`). New transformation count `headers_cleaned`.
- `check_citations` pairs a cited cell with the number in the neighbouring Markdown table cell, in
  the same sentence, or in the previous sentence when the text refers back ("478,737. This value is
  from cell D7"), and flags a cell whose value differs (`Citation.pairing` says how it was paired).
  A bare table id like `(T1)` / `[T12]` is no longer read as a cell.
- `hh_core::normalize::{table_time_coverage, workbook_time_coverage, table_totals}`: the manifest's
  `time_coverage` and `tables[].totals` as Rust values (`TimeCoverage`, `TableTotals`), so other
  views agree with the manifest.

### Changed

- The model path computes only the row-segment features the trees test (and the guard's), not all
  ~450: same predictions, less memory and time per sheet.
- `open_file` catalog (MCP, `hh_agent`, Node `HedgehogSession`), after the gpt-4o-mini chat
  pilots: every column is listed for tables up to 40 columns (`name[dtype unit]`; ranges up to 14),
  runs of columns named by consecutive periods as one item (`"2011".."2025"[15 × number]`), wider
  tables group same-type runs instead of `… 6 more …`; text columns with ≤ 12 distinct values list
  them all (`"group" values: "Anzahl Bezieher:innen" | …`); `rows mix measures by "group" — filter
  before aggregating` when a label column separates counts, amounts and rates; tables of ≤ 15 rows ×
  ≤ 12 columns get their rows as CSV while the catalog stays under its cap. The TIME line, per-table
  periods, totals counts and total-row labels now come from the manifest's `time_coverage` and
  `tables[].totals` (years only in titles / sheet / file names are a secondary note), and total
  rows that stay in the data ("Insgesamt" rows of long tables) are named. `describe_table` lists all
  values of small label columns and the same mixed-measures line. `hh_agent::TimeCoverage` is now
  the manifest's; `hh_agent::mentioned_years` gives the weak hints.
- The Node addon is stripped (`strip = true` in the `node-release` profile; `panic = "unwind"`
  kept): `hedgehog.linux-x64-gnu.node` 11.8 MB → 9.1 MB here (89.5 MB → 15.8 MB in the
  hedgehog-cli pilot build).

- Totals in the normalized output: a typed total that does not add up is `unknown`, not
  `mismatch`, when its column header or row label names a rate, share, index, average or
  per-capita value (`not_additive`: `%`, `Anteil`, `Quote`, `je Einwohner`, `j.E.`, `per capita`,
  `Gesamt-VPI (2020=100)`), when it is off by rounding only (`rounding_difference`: at most √n units
  of the last decimal and under 1%, as in tables "in 1 000"), or when it lies between its smallest
  and largest source value (`average_like`). Formula totals are not re-judged. Unverified total rows
  with such a header or label get reason `not_additive` instead of `not_verified`. On the 315 EU
  open-data files plus the 50 Enron benchmark files: mismatches 2,701 → 355 (EU 2,696 → 352,
  Enron 5 → 3); `ok` unchanged. The docs sample's wrong grand total (`B13`) is still a
  mismatch. Detection and `Workbook.totals()` are unchanged.
- A navigation link (`zur Inhaltsübersicht`, `Back to contents`, `Retour au sommaire`, `Powrót do
  spisu tablic`, `Inhalt`, ...) is never a table title; the next title-like line is, and the link is
  a Notes row of kind `other` (15 of the 31 tables of a destatis migration report were titled
  "zur Inhaltsübersicht").

### Fixed

- `open_file` on large registers: the catalog took ~48 s on a 176k-row file (a quadratic search
  for measure labels over text columns with ~100k distinct values, and the time coverage computed
  twice); now ~0.45 s. Text columns stop being checked for period labels as soon as a tenth of
  their values fail (same result).

- A row of numbers under a header of dates or years only (all zeros in the first row of a daily
  grid) is data, not a repeated header: repeated headers are compared by value where the header has
  no text. The Enron Henry Hub report kept 163 data cells in `Notes`; DECO header F1 (rules)
  56.3 → 81.7, model path 84.1 → 85.4. A labelled row still repeats by its labels.
- **Records whose description says "total" are no longer total rows.** In a register (a sheet of
  200 rows or more), a total word inside a longer text that has other text to its left (a
  description, a supplier's name: "AQUISIÇÃO DE UMA ESTAÇÃO TOTAL", "Prótese total anca",
  "GLOBAL 2 SEGURANÇA TOTAL LDA") no longer makes its row a total row. Such a row is still a total
  if one of its numbers is the sum of at least two numbers above it, if it closes a group ("X
  Total" under the row labelled "X"), or if the text starts or ends with the total word and recurs
  as a category of its column (long-format statistics: "Summe der bezogenen Rohstoffe" in 1 % of
  the rows, or "Summe aller Produkte" closing each block of identical keys). Short labels
  ("Total", "Gesamt", "Summe", "Ukupno Vojvodina"), labels in a row's first text cell and short
  sheets are unchanged. Portugal's 2020 procurement register now keeps all 176,512 contracts in
  `T1` (38 were removed as totals, 63 more were flagged "exclude before summing"). The rule
  applies in the totals, the rule layout and to the learned model's output (its features are
  unchanged, as it was trained on them). Benchmarks: rule layout total F1 66.0 → 68.3 on
  synthetic, statsheets accuracy 86.2 → 86.7 (model 87.3 → 87.4); nothing drops.

- Tables on one sheet no longer overlap in the default (model) path (Finanzstatistik T2
  `A17:IB43` over T3 `A31:HO43`, from a vertical merge hanging below a table).

- Typed totals over a column with missing-value tokens (`n/a`, `-`, `*`, `#N/A`): the tokens were
  read as header text, so the source range stopped at them and the total could be reported as a
  false mismatch. They now count as blanks. A typed total also counts numbers stored as text
  (`"1,200.50"`) in its source, since it was added up by hand, not by Excel's `SUM`.
- `--keep-totals`: a grand-total row no longer takes the last group's label in the group column
  (a subtotal keeps its group; a total over several groups belongs to none).
- A group label written on the first data row of its group after a blank row (`Oct | … | 370,494`
  under month groups that otherwise have a row of their own) starts that group instead of staying
  a value under the previous one (rules; the learned model can still call such a row a total).
- Bedrock: `connect("bedrock", api_key=…)` no longer sets `AWS_BEARER_TOKEN_BEDROCK` in
  `os.environ`; the key is the bearer token of the client's own botocore session.
- `llm.ask` tools: `check_totals` and `describe_workbook` items carry the normalize manifest's
  `status` (`ok` / `mismatch` / `unknown`), `check_totals` reports counts per status and lists
  mismatches first, and an unverified total's recomputed sum is `recomputed`, not `expected`, so
  an agent no longer counts unconfirmed totals (averages, percentages) as wrong. `get_cell` on a
  merged cell gives the merge range and the value it shows.

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
