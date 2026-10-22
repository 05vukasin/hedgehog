# Changelog

All notable changes to hedgehog are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/); before 1.0, minor versions may break the API.

## [Unreleased]

### Added

- **SQL joins across files** (B76). Every file opened in an agent session gets a number in the
  order it was opened; `f<k>.T1` (and `f<k>.T1_all`, `_long`, `U1`) names a table of file k in
  `query`, so one statement joins orders with order lines kept in two files. Plain `T1` stays the
  file in `path`. With two or more files open, the catalog ends with `FILES` (prefix = file) and
  `JOIN KEYS` (same-named columns with overlapping values, one side unique). The server attaches
  the databases read-only; users still cannot `ATTACH`. `Session::open_files()`, Node
  `HedgehogSession.openFiles()`, `eval/agents/sql.py --open`. See
  [docs/mcp.md](docs/mcp.md#sql-tables).
- **Union views of same-layout tables** (B79): tables with the same column types in the same order
  and mostly the same names (dates dropped) get `U<k>` / `U<k>_all` with `_table`, `sheet`,
  `title`, `part` (the part of the title that differs, e.g. the state) and `variant` columns, and a
  catalog line per family.
- **Empty query results explain themselves** (B75): a result with no rows, a NULL aggregate or a
  zero count of a filtered query gets notes — the value is only on total rows in `T<n>_all` (with
  that query's rows there), the column has no such value (values containing it, its values or the
  most frequent ones; "not a value of 0"), or a quoted value meets a 1/0 or number column. A filter
  value that also labels total rows only in `T<n>_all` gets a note on a non-empty result; `a + b`
  over sparse columns with NULL results suggests `COALESCE` / `total()` (B88).
- **Structure fixes from the 30-case agent run** (B70, B72, B78, B81, B84, B86, B87):
  - *Period labels* (B70): a text column whose labels name periods (`Jän.21`, `Ø 22`,
    `Mar-May 1971`, `2025 Q1`, `Jan-24`, stored dates among them) gets a derived
    `<name>_period` column right after it with a form that sorts as text (`2022`, `2022-01`,
    `2025-11/2026-01`); time coverage reads these labels; the label of a header row of series
    codes (ONS "Dataset identifier code") no longer names the period column. `Ø 22` average rows
    carry their year in `totals.rows[].period`.
  - *Sex breakdowns* (B72): blocks of M/F/T rows (H/M/HM, Männer/Frauen/Insgesamt,
    Masculino/Feminino/Total, ...) keep M and F as data with the block's label on every row, the
    both-sexes row as a subtotal, and a block labelled `Total` as total rows. A fuller header row
    under sparse title lines becomes the header; a numbered caption ("Quadro 1 …", "Table 3: …")
    is the table's title, also when it ends up as the last row of the table above.
  - *Spelling variants* (B78): a name or category column whose values differ only in case,
    spaces, punctuation or diacritics ('Mc Kay' / 'McKay') gets `<name>_key` (one spelling per
    group) and a table warning.
  - *Month headers* (B84) that step back from December to January of the same year are read as
    the next year; `totals.rows[].sources_outside` lists source cells in other tables (a grand
    total under stacked blocks that adds the block above).
  - *Names starting with a total word* (B81: 'TOTAL RECRUIT LTD', 'Zusammen unterwegs') in a
    register's name column (1,000+ distinct names, or a legal form) stay data and are not listed
    on Totals unless their numbers add up; a "Table N" caption above a table is preferred as its title.
  - *Stacked blocks without a header* (B86: Destatis "% change", "% of GDP") take the header of
    the block above with the same columns, plus their unit or title.
  - *Known aggregates* (B87): Eurostat `EU27_2020` / `EA20` / `EFTA` geo rows and OWID `World`,
    continents, income groups and `(EI)` / `(Ember)` regions are aggregates (total rows).
  - Manifest: `columns[].derived` (`{"from", "kind"}` for `_period` / `_key` columns),
    `totals.rows[].period` and `.sources_outside`, transformations `period_columns_added` and
    `key_columns_added`.

- **Robustness for untrusted files** (P4). Per-file limits, checked before and while a file is
  read (`hh_core::reader::Limits` in `ReadOptions`): parts of a zip package decompressing to more
  than 2 GB (zip bombs), more than 10 million cells (an ODS file's repeated rows and columns are
  counted before it is parsed), more than 10 million shared strings, and an optional deadline
  (`NormalizeOptions::deadline`, `try_normalize_workbook`). Refusals are one-line errors starting
  with `file refused:` (`Error::is_limit`). `hedgehog mcp` gets `--max-cells` (default 10,000,000)
  and `--max-seconds` (default 120). Password-protected files (encrypted xlsx / xlsb / ODS, xls
  with a password) now say so instead of reporting a broken zip or OLE file. See
  [docs/mcp.md](docs/mcp.md#security-limits).
- **Crash isolation.** The release profile unwinds on panic (`panic = "unwind"`, was `abort`), and
  `hedgehog mcp` runs each tool call under `catch_unwind`: a panic becomes that call's error and the
  server keeps running. The Python (`RuntimeError`) and Node bindings catch panics at their
  boundary too, so a hostile file can no longer take down the host process.
- **Fuzzing** (`fuzz/`, `hh_fuzz`): a mutation fuzzer on the stable toolchain for the CSV, xlsx,
  xls and ODS readers and the normalize pipeline. See [docs/development.md](docs/development.md#fuzzing).

- **Average and of-which rows** (B30, B31): `Average` / `Mean` / `Median` / `Ø 2024` /
  `Durchschnitt` / `Moyenne` / `Prosek` / `Średnia` ... rows and `of which` / `davon` / `darunter` /
  `dont` / `di cui` / `od toga` / `w tym` / `z toho` / `waarvan` ... rows are neither data nor sums:
  they leave the table data (`_row_type` `average` / `of_which` with `keep_totals`, so `T<n>` has
  data rows only and `T<n>_all` has them), are listed on `Totals` (levels `average` / `of_which`,
  checked: `matches_mean` / `matches_median`, `within_parent`; an unchecked or larger value is
  `unknown`, never `mismatch`) and are left out of every total's sum, so `Total` after them still
  checks out. Agent views, catalog (`total rows: … WHERE _row_type IN ('total', 'average')`) and
  disk cache know the new row types.
- **Totals above their rows** (B36): a subtotal at the top of its group and a grand total at the
  top of a table add the rows below them when those add up (typed), or by their formula (`SUM`
  over the cells below); `{g} gesamt`, `Razem {g}`, `{g} - total` close their groups; a group
  label written once per block is filled down across the subtotal rows; single spacer rows are
  stepped over when the sum then matches.
- **Total words in more languages** (B37): Czech/Slovak, Finnish, Danish, Swedish/Norwegian,
  Hungarian, Russian, Polish, Estonian, Icelandic, Turkish, Greek, German `X gesamt` / `X
  insgesamt`; one-letter typos of `total` in short labels (`Toal`, `Totl`, `Grand Totl`); spaced
  letters (`T o t a l`). The learned row model's features keep the keyword list they were trained
  on.
- **Aggregates without a total word** (B42): a row equal to the sum of the rows above it in at
  least two columns and two thirds of its numbers (`France métropolitaine`, `DOM`, `France
  métropolitaine et DOM`) is a total (evidence `unlabelled`); World Bank region and income-group
  rows (`WLD`, `EUU`, `HIC`, …, by their code column) are aggregates (`known_aggregate`). In the
  normalized output a row whose short total label (its first text) adds up, a `SUM` over the rows
  below, or such an aggregate is a total row even when the layout took it for data — in the table
  holding it only, not in a table beside it.

  Noise curves (model, t = 1, 60 seeds): `subtotals` 27 → 100 %, `of_which` 0 → 100 %,
  `average_rows` 22 → 100 %, `long_register` 0 → 100 %, `total_labels` 55 → 100 %, `combo` 8 →
  28 %, all families 61 → 81 % (rules 55 → 76 %); no family drops. Structure benchmarks: model
  identical to master, layout total F1 on the synthetic corpus 68.3 → 68.5. EU open data + Enron
  benchmark files: total cells found 728k → 764k (`ok` 611,692 → 646,280, mismatches 357 → 360).

- **Disk cache** of normalized files (agent tools: `hedgehog mcp`, Node `HedgehogSession`): a file
  opened before comes back in a new process in ~2 ms to the catalog and ~20 ms to the first query
  answer (Portugal's 4.5M-cell contracts register: 15 s → 2.3 ms / 19 ms; peak memory 1.2 GB →
  0.54 GB). Keyed by content hash, build and options; `HEDGEHOG_CACHE_DIR`, `HEDGEHOG_CACHE=0`,
  `HEDGEHOG_CACHE_MAX_MB` (default 2048, least recently used removed first); atomic writes, damaged
  entries rebuilt, `0700`/`0600` permissions. The (path, size, mtime) → sha256 shortcut is kept
  there too. Catalogs are byte-identical with and without the cache. See
  [docs/mcp.md](docs/mcp.md#disk-cache).
- **Catalog first** (`open_file`): the catalog returns as soon as the file is normalized; the SQL
  tables are built on another thread (into the cache, read-only from there).
- **`hedgehog mcp` cancels**: tool calls run on a worker thread; `ping` / `tools/list` answer while a
  call runs; `notifications/cancelled` drops a queued call or stops a running query (SQLite
  interrupt) or the wait for a normalization. See [docs/mcp.md](docs/mcp.md#cancelling-a-call).

- **Long views of wide period tables** (`hedgehog mcp`, Node, hedgehog-cli; B20): a table with 4 or
  more numeric columns named by periods (`"2011".."2025"`, `Jan 2024`, `2024 Q1`, `Thu / 2002-01-31`)
  gets a SQL view `T<n>_long` with its key columns, `period` (normalized: `2025`, `2025-Q1`,
  `2025-03`, `2025-03-31`), `period_label`, `value`, `_src_row` and `_src_col` (plus `measure`
  when the periods repeat for several measures), so a model filters `WHERE period = '2025'`
  instead of picking a column. As with `T<n>` / `T<n>_all`, `T<n>_long` has the data rows and
  `T<n>_long_all` also the total rows (`_row_type`). Plain views: nothing is copied at load. The catalog has one line per long view (`T1_long: one row per ("group",
  Richtsatz, period) — … use for trends/filters by year`, with the label to filter first when the
  rows mix measures); `describe_table T1_long` describes T1. `hh_core::normalize::header_period`
  exposes how the time coverage reads period headers. See [docs/mcp.md](docs/mcp.md#sql-tables).
- **Semantic column types** in the catalog and `describe_table` (B21): `id`, `name`, `dim`,
  `period`, `amount` (with currency), `count`, `rate`, `avg`, `index`, `free text`, `mixed`, from
  header words in eight languages, currency / unit, the table title and value statistics:
  `"2025"[number EUR · amount]`, `Indexwert[number · index — don't sum]`; a `do not sum:` line per
  table with average / rate / index columns (or rows, when a label column mixes measures). Columns
  the file adds up itself are never flagged. Catalog size on the four agent-eval files (with B15–B19):
  6,517 → 6,788 (pt-contracts), 3,396 → 4,068 (at-pension), 9,982 → 11,415 (enron-henry-hub, all
  rows of T1/T2 still inline), 4,782 → 5,310 characters (de-truck-toll). See [docs/mcp.md](docs/mcp.md#the-catalog).
- **Agent accuracy with small models (B15, B16, B18, B19)**, from the hedgehog-cli runs:
  - A `no such column` that names a total-row label (`"Total"`, `"Gesamt"`) or a value of a text
    column (`"SEA ROBIN"`) still explains where those rows are, and now also returns them: the
    suggested query is run and up to 10 rows (~3,000 characters) follow under `Those rows (the
    query above, run for you; pick the columns you need):`. The call stays an error.
  - Long forms: when one table repeats another's values one row per key × category (checked on up
    to 40 sampled rows), both get a catalog line, e.g. `T5 is the long form of T3: one row per Datum
    × Saisonbereinigung, value in Indexwert (T3 has one column per Saisonbereinigung value)`
    (de-truck-toll).
  - Entity columns: `<col>_name` is now one name per ID (the spelling most rows use), so `GROUP BY
    adjudicante_name` gives Infraestruturas de Portugal €737.7 M in one row like `GROUP BY
    adjudicante_id` (was €507.2 M + €229.0 M). The entity line comes right under the columns line
    and carries the ready query (`SELECT "adjudicante_id", "adjudicante_name", count(*) FROM T1
    GROUP BY "adjudicante_id"`; `coalesce(<col>_id, <col>_name)` when values without an ID exist). A
    result grouped by the raw text that lists one ID twice ends with `note: rows 1 and 3 are one
    adjudicante (ID 503933813) under different spellings — group by adjudicante_id instead, e.g. …`.
  - Shares: tables with ≥ 30 rows and a summable amount column get a ready query for a share of the
    total (`share of the total (e.g. top 10 by "adjudicante_id"): SELECT 100.0 * SUM(v) / (SELECT
    SUM("precoContratual") FROM T1) AS pct FROM (SELECT … GROUP BY … ORDER BY v DESC LIMIT 10)`); for
    the suppliers of pt-contracts it gives 12.9 %.

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
- `hedgehog normalize --threads N` / `NormalizeOptions::threads`: how many threads analyze sheets
  in parallel (default up to 4); the output is the same for every value.

### Changed

- SQL errors (B88): `_row_type` on `T<n>` says it is only in `T<n>_all`; a column the outer query
  takes from a subquery or CTE lists that subquery's columns instead of suggesting `T1."col"`.
  In-memory databases of an agent session are named shared-cache memory databases (so a
  multi-file query can attach them).

- **Catalog content from the 30-case run** (`open_file`, `describe_table`; B71, B73, B74, B80,
  B82, B83, B85, B89). Column names are never cut: wide tables list every name in full, grouped by
  prefix when the plain list is long (`solar_{consumption, electricity, share_elec[number · rate —
  don't sum]}[3 × number]`), and brief catalogs list each table's names in full (key and total
  columns first, `+N more (describe_table)` for the rest) and say `same as T1` for repeated
  layouts. Total rows are listed by level with the grand total named and the rows each adds up
  (`grand total 'Total Expenses'@65 (= rows 7, 49, 57, 63); total 'Total O&M'@49 (= rows 41, 47);
  subtotals (_row_type 'subtotal', not 'total') …`; up to 15, repeated sets once). The quality
  line covers every column: empty cells tied to the category that explains them (`annual_salary
  empty in 7,129 rows (22 %): salary_or_hourly = 'HOURLY' (all its 7,129 rows)`), text columns
  with many empty cells, IDs written on several lines (`"Order ID": 800 rows, 296 distinct`) and
  partial first / last years (`Date: 2013 has Sep–Dec only (4 of 12 months)`). New lines: an ID
  column with its names in another column (`empfaengerid is the ID of name: … SELECT
  "empfaengerid", MAX("name") …`), footnotes that define a column (`column notes: … LF24 ← "The
  headline employment rate is …"`) and a scale from the title (`units: amounts in thousands (×
  1,000), says "(All values in 000s)"`). Booleans show as SQL stores them (`1/0`). The share
  example is only built on an additive amount (no unit prices, rates or rows of mixed units),
  ranked by an entity, an ID with names or a breakdown of 10–1,000 values (not a name or a
  period); the fill-down hint only on label columns; period codes written as text (`TIME_PERIOD`
  '2019-S1') give the TIME line instead of an update stamp (`LAST UPDATE`). The 30 case catalogs
  stay under the 11,700-character cap (largest 11,143).
- **Faster, leaner model path, same output** (P7 "exact wins"; every output byte unchanged). The
  row model's input is streamed instead of materialized: a first pass holds the sheet-wide counts,
  sets and rectangles the features read, then the rows are walked with a three-row window (up to
  four chunks in parallel) and segments are scored in blocks, so neither the per-cell feature
  matrix (~440 bytes per cell) nor the segment matrix (~1.8 kB per segment) is built. Trees are
  walked eight rows at a time in lockstep with leaves that loop to themselves (no leaf test, same
  leaf, same sum order). On the largest model files (best of 3): bfs036 (1.5M cells) 14.3 s →
  7.5 s, pt008 (1.2M cells, 632k formulas) 19.6 s → 14.3 s, nl019 16.4 s → 13.1 s, uk069
  12.4 s → 10.8 s; peak memory during analysis 783 → 566 MB, 1443 → 1202 MB, 811 → 587 MB,
  912 → 642 MB. Also: the change-point search stops classifying a block's rows as soon as rows ×
  columns passes its cap (it used to classify every row first), `l_in_table` is a row sweep instead
  of a test of every table per cell, and the column-convention vote parses each text once (it
  parsed it twice). Rules-only files are unchanged within noise. `HEDGEHOG_TRACE=1` now prints
  sub-steps (`layout.blocks`, `layout.cp_signatures`, `M.stream`, `build_tables.typing`, …).
  Checked: `learned::equivalence` (every segment feature bit for bit, every role, every class
  score against the plain tree walk, the final layout; rows in 1 and 4 chunks) on 34,742 sheets /
  8.4M segments of the benchmark corpora and `testdata/irregular`; normalized output and manifest
  identical to master on 1,302 files, single-threaded and threaded; golden check and benchmark TSVs
  identical. See [docs/architecture.md](docs/architecture.md#performance-notes).
- **`query` keeps long values whole** (B47): text values come back exactly (CSV-quoted) up to 2,000
  characters instead of being cut at 80 with `…`, so a `group_concat` list can be reused in the next
  query; a longer value ends in `…[+N chars]` with a `note:` on reading the rest with `substr`.
- **Formulas without a saved value are reported** (B48): the catalog says `FORMULAS 1,479 cells
  without a saved value (e.g. May1!H13): NULL in the tables, not evaluated`, the quality line counts
  them per table, and `hedgehog cells` prints such a cell as `no_saved_value:=<formula>` (was
  `formula:<formula>`). Nothing is evaluated. Disk cache entries of earlier builds are not reused.
- **Sign note only for flows** (B49): the `signs: … (outflows?)` line needs flow labels (receipts /
  deliveries, debit / credit, in / out, …) or runs holding a tenth of the signed rows, and never
  appears for price, rate or index columns (Eurostat electricity prices lost a misleading note).
- **Time coverage ignores entity dates** (B49): start, hire, birth, expiry and valid-from date
  columns count only when a table has no other period column (NYC payroll: `1996-06-23–2025 (day)`
  → `2014–2025 (year, from fiscal_year)`; Chicago business licenses no longer run to 2030).
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

- **Out of memory on a sheet with a title merged over the whole row** (B90). `EQ08__13.xlsx` (an
  ABS data cube, 4.07M cells) was killed at a 4 GB cap in `build_tables`: since the parent header
  rows (B39/B53), its title merged over `A2:XFD2` and its release line (with the copyright beside
  it) were read as parent labels over the six-column header, so the table spanned 16,384 columns
  and every one of its 655,865 rows got 16,384 slots. A parent label row may no longer start right
  of the two rows below it, nor (a lone merged label) reach past them; and `build_table` leaves
  out the columns of a table's range left and right of all its header and body cells (a merge past
  the data, counted as empty columns removed), so a table costs its cells, not rows × the width
  of a merge. EQ08: out of memory → 1.2–1.26 GB peak, 16.0–16.3 s (before the regression 1.23 GB, 16.1 s),
  with the same table and columns as before it. Same-size work, less time: the B81 register-name
  check counted the whole label column once per keyword candidate (nl019 21.8 → 12.0 s, uk069
  17.4 → 12.2 s, pt008 16.6 → 13.5 s), and the sex-block and period-label checks now read each
  distinct text once. Both benchmark analyzers and the golden check are unchanged;
  `core/tests/memory_per_cell.rs` bounds the heap per cell of generated sheets with such merges.
- **Bit-for-bit repeatable output** (B68). The same file and options now give the same bytes in
  every run, on any thread count: the normalized xlsx carried the current time in its document
  properties (`docProps/core.xml` created / modified; now fixed at 1980-01-01, like the zip
  entries), and a table's `confidence` averaged its column purities in hash-map order, so its last
  bits, and sometimes the rounded manifest value, could change between runs (now in column order).
  The agent catalog's `<id> - <name>` separator broke a tie in hash-map order too. The manifest has
  no run metadata (no time, host or temp path); see
  [docs/normalized-output.md](docs/normalized-output.md#repeatable-output). Checked by
  `core/tests/determinism.rs` and `fuzz/determinism.py`: 308 files (Enron 50, US BEA, a US SEC
  EDGAR sample, 189 European files) x 3 threaded runs, and US BEA x 5 threaded + 5 single-CPU runs,
  all identical.

- **Register names taken for totals** (B50): in a register, a total word right of the record's own
  keys (`GROWTH WIRELESS LLC | TOTAL WIRELESS`, a counterparty `TOTAL` beside a deal id) needs its
  numbers to add up, and words only the rule layout reads as aggregates (`MAXIMUM MARKETING
  SERVICES INC`, `MORE THAN AVERAGE LLC`) no longer label a row that holds other text. Chicago
  business licenses: 87 records (1,112 cells on Totals, 1,639 on Notes) back in the data; the
  header `Customer | Resource | Total | 1 … 24` of an Enron sheet is no longer a total row.
- **ODS files with backslash paths** (B52): zip entries named `META-INF\manifest.xml` (written by
  some Windows tools) are found, so such files open instead of failing with `'META-INF/manifest.xml'
  file not found`; the xlsb workbook relationships part is looked up the same way.
- Crashes, hangs and runaway allocations on malformed files found by the fuzzer (each with a
  regression test in `core/tests/hostile_files.rs`). In calamine, now vendored with the fixes
  (`vendor/calamine`, each change marked `hedgehog:`): looping sector chains in xls / OLE files
  grew memory until the process aborted; a `sharedStrings.xml` `uniqueCount` reserved up to 51 GB;
  out-of-range reads in xls records and formulas, sector ids past the FAT, truncated files and
  overlong cell references panicked or wrapped around. In hedgehog: xls and xlsb sheets are read
  sparsely (two cells far apart no longer allocate the whole rectangle between them); merged areas
  written bottom-right first get their corners ordered; duplicate column names were made unique in
  quadratic time (a merged header over 16,000 columns took 22 s).
- `normalize` of a workbook with more than ~1,000 tables panicked with "Too many open files": the
  xlsx writer keeps a temp file open per output sheet. The open-file limit is raised when needed
  (a clear error if the hard limit is too low). CSV files are checked against the cell limit while
  they are read, so a huge CSV is refused before it fills memory.
- **Layout and headers on noisy and statistical sheets** (B33, B35, B38, B39, B43–B46; noise
  curves at t = 1, rules / model, before → after: `footnotes` 73 / 100 → 100 / 100, `wide_periods`
  68 → 100 at every intensity, `separators` 37 → 100, `notes` 17 / 32 → 100 / 100, `mixed_types`
  78 / 80 → 85 / 90, `multirow_header` 32 / 38 → 33 / 43, `side_by_side` 28 → 33 (t = 0.25:
  62 → 87), `combo` 8 → 27; all families 55 / 61 → 67 / 70; no family lower; structure
  benchmarks: table F1 up on every corpus, e.g. DECO model IoU≥0.8 56.0 → 57.8, StatSheets
  86.9 → 88.1):
  - numbers with footnote marks or flags (`28472.72*`, `1737³`, `6077.61 (p)`, `20560.32e`) count
    as numbers and censored / suppressed cells (`<5`, `confidential`, `[c]`) as blanks when the
    rules look for headers, so such rows no longer start a new table; whole numbers that fall in
    1900–2100 but jump (`Riverbend 2 | 1948 | 2043 | 338`) are measures, not a header of years;
  - a header row of three or more text periods (`2010-01`, `01/2010`, `2010-01-31`) is a header; so is a label
    over period numbers (`Year | 1 | 2 | … | 20`, Enron Newco);
  - ruling lines drawn with text (`-----`, `=====`, `_____`, `~~~~~`, `*****`, a `|` column) are
    neither data nor a table break: they go to `Notes` as `other`, and a total under one verifies;
  - parent labels centred over their columns without a merge name every child column;
  - a header-less column right of a table with a remark on a row or two (`check!`) is notes; a
    sentence alone on the last row and source lines in more languages (`Quelle:`, `Remarque :`,
    `Fuente:`, `Uwaga:`, …) are notes;
  - two row-aligned tables that each have their own row labels stay apart across 1–2 blank columns;
    numbers on a few rows across one spacer column inside a taller table's rows join it (MDEA's
    S:T profit columns on May 10–14 were 0-row header tables; every number is now in a table);
  - a unit row under the header (`Mrd. EUR | % | 2020 = 100 | 1 000 Personen`) joins the column
    names, and a column-numbering row no longer names them (Destatis: `In jeweiligen Preisen /
    Ursprungswerte / Mrd. EUR`, was `… / 1`);
  - key/value metadata rows above a table (`Date of publication: | …`) are never its header (ONS
    Table 1: the header is rows 5–8 again), and a table's loose title skips source lines and such
    metadata and prefers a `Table 1: …` line;
  - a row the model calls a group header that fills the label columns like the records around it
    (a WDI country without values) is a record: no invented `group` column;
  - an index named after its base year (`VPI 2000`, `HICP 2015`) is not a period for time coverage.
- **Layout regressions of the merge above, statistics-database exports and long TSVs** (B53–B57;
  every structure-benchmark number now at or above both the release before the layout work and
  that work itself; DECO model table F1 at IoU ≥ 0.5 / 0.8 / 1.0 69.4 / 57.9 / 38.4 → 75.0 / 62.8 /
  41.4, header rows exact 81.4 → 82.1 %, StatSheets 92.4 / 88.1 / 75.9 → 93.8 / 89.9 / 78.4 and
  71.6 → 73.5 %; 13 of 19 files without a table in the 1,093-file sweep now have one):
  - two row-aligned blocks with their own row labels are two tables side by side only when each
    has a header or a label and two value columns; a header-less two-column label/value block stays
    with its neighbour (gstorey `Totals_xx`: the `TOTAL $$` summary is a table again, not Notes);
  - a form's `Key: | value` rows are never its header, not even the next one past the metadata
    rows ("Brothers Production": header at row 2 again);
  - statistics-database exports (PX-Web, SCB, SURS: a header, one data row, then footnotes, source
    and contact lines) get their table: a block of single lines in the first column below a blank
    row is notes, up to two rows of parent labels over a header row are header rows, and period
    codes (`2016K3`, `2016Q4`, `2014M01`), split years (`1990-91`, `2011/12`) and year ranges
    (`2004-2008`) are header labels, not numbers;
  - Eurostat's `: m` (missing, with a flag letter) is a missing value, so a long-format TSV with
    such rows stays one table (`gov_10a_main.tsv`: 345 tables → 1; three more Eurostat TSVs
    66–87 → 1); NCES and Census marks (`†`, `‡`, `#`, `(X)`) are blanks and symbol legends
    (`---Not available.`) are notes, so they no longer split tables;
  - header-less blocks of numbers on exactly the same rows, up to two blank columns apart and with
    nothing above them, are column groups of one table (DECO `IPG INDEX`: 63 tables → 7); numbers
    inside a taller table's rows join it only when they carry no labels of their own;
  - a short label at the left edge right over the header row, below the title, is the header's top
    row (`Term` over `Delivery Pt | Q3-2002 | …`); a label merged over a group of columns above the
    header names them in the rules too (`2019 / Population`; noise `label_blocks` rules 50–82 → 100 %
    at every intensity);
  - two tables touching without a blank column are split where a second title stands right over the
    header above a text label column (noise `side_by_side` t = 0.25 / 0.5: 87 → 93 / 47 → 53 %);
  - CSV delimiter detection counts quotes out of place against a delimiter: a quoted header with
    `;` and line breaks inside no longer turns a comma-separated file into one column (Charity
    Commission workforce CSVs: 0 → 1 table).
- `eval/noise` builds again against the vendored calamine (its own workspace lacked the `[patch]`).

- Total words in names and descriptions (B32, B41): in a register (200+ rows) a description or
  name of more than three words that is its row's first text with more text right of it ("Total
  knee replacement implant", "BEECH TREE TOTAL CARE LIMITED") is a record unless more than half
  of its numbers add up; a recurring label is a category only when its row shares its keys with a
  neighbour (a record with its own id is not); `Somme` (département), `Sumas` (place), `SOM`
  (Somalia), `SUMA Kommunikation`, `Zusammen interreligiös` need arithmetic (`Suma`, `Zusammen`
  alone under a table stay total labels). The learned model's own post-step keeps its trained rule;
  the normalized output turns such rows back into data.

- **Numbers next to a column's other numbers** (B40): a CSV column of percentages with up to three
  decimals read `40.805` as 40805 (the UK gender pay gap file: 35 cells, the 500–999 band averaged
  42.06 % instead of 12.06 %). An ambiguous `x.xxx` / `x,xxx` is now decided by the column's
  numbers stored as numbers (typed cells; plain CSV fields like `12.3`) when its text values do not
  decide: among whole numbers it groups thousands (`4.405` next to `3178` is 4405), among fractions
  it takes the reading that matches their size. Integers with leading zeros (IDs) no longer support
  the thousands reading, and dates written `31.12.2024` count as decimal-comma evidence for the
  sheet. All 30 agent-eval cases' references still reproduce; the UK case's five `ref_py` answers
  are now also what `ref_sql` gives.
- **Footnote marks, `:` and censored values in numeric columns** (B34): numbers with statistical
  flag letters (`7 p`, `12.3 e`, `5 bep`, `20560.32e`) and the existing marks (`123¹`, `12 (r)`,
  `45*`) are read as numbers, and the mark is kept: the manifest column's new `footnote_marks`
  lists each mark with its count and cells, and tables count `footnote_marks_stripped`. `:` and
  `: c` (Eurostat), `k.A.`, `n.v.`, `b.d.`, `s.d.`, `n.c.`, `keine Angabe`, `brak danych`, ...
  are missing values, and in table columns also `(X)`, `[x]`, `(NA)`, `[z]`. Censored (`<5`,
  `> 1000`, `(Z)`), suppressed (`c`, `[c]`, `(D)`, `confidential`) and `see note` cells stay text
  but no longer turn a numeric column into text, nor do up to three distinct words among ≥ 75 %
  numbers (`approx.`); they are listed in the table's `warnings`. Noise robustness (model path,
  t = 1): `footnotes` 2 → 100 %, `locale` 43 → 100 %, `mixed_types` 5 → 80 %, `combo` 3 → 8 %,
  all families 50 → 61 % (no family lower at any intensity); structure benchmarks unchanged except
  the synthetic corpus (full F1 93.03 → 93.09); golden check identical.
- Total rows take the keys of the block they close (B17): the Austrian pension table's "Gesamt"
  row is in group `Anzahl Bezieher:innen` (its sums reached up into the year header, which made it
  look like a total over all groups, with an empty group), and an empty text cell left of a total
  row's label gets the value every data row of its block shares (Henry Hub: `col_B = 'Receipts HH'`
  / `'Deliveries HH'` on the two `Total` rows), so `WHERE key = '…' AND _row_type = 'total'` finds
  them. Counted as `group_keys_filled_down`. Grand totals over several groups keep no group.

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
