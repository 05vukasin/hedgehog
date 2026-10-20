# Noise generator (`tools/noise`, PLAN.md M5)

Turns clean tables (`testdata/clean/`, typed by `MANIFEST.csv`) into messy, realistic workbooks with
exact ground truth: table rectangles, header rows, a role for every filled cell, column semantic
types and original headers, and provenance (source dataset, original row/column, original value,
operators applied) on every cell. Design follows `docs/research/irregularities.md` §3 (4 stages,
provenance instead of diffs, calibration against DECO / Enron).

## Usage

```bash
cd tools/noise
uv venv .venv && uv pip install --python .venv/bin/python xlsxwriter openpyxl pytest python-calamine
R=../../../scripts/run_capped.sh   # always run heavy jobs capped
$R 3G /tmp/src.log .venv/bin/python -m hh_noise sources          # cache clean tables → testdata/synthetic/_sources
$R 3G /tmp/gen.log .venv/bin/python -m hh_noise generate --out ../../../testdata/synthetic/v0 --n 2000 --seed 2026 --workers 2
.venv/bin/python -m hh_noise validate --dir ../../../testdata/synthetic/v0 --roundtrip   # schema + calamine round trip
.venv/bin/python -m hh_noise calibrate --dir ../../../testdata/synthetic/v0 --sample 150  # generated vs target stats
.venv/bin/python -m hh_noise show ../../../testdata/synthetic/v0/enron_like/wb_00001.xlsx  # text view with roles
.venv/bin/python -m hh_noise ops                                                          # operator list
.venv/bin/python -m pytest -q tests

# Rust: score analyzers on the corpus (+ per profile / boundary / operator breakdown, value parsers)
hedgehog eval synthetic --dir testdata/synthetic/v0 --analyzer layout --values
```

Generation is deterministic per (profile, seed, workbook index); it streams one workbook at a time
(≈ 10 ms each; 2,000 workbooks in ~15 s with 2 workers, ~60 MB) and is resumable via `index.jsonl`.
Generated files never go into git (`*.xlsx`, `*.jsonl` are ignored; keep corpora under `testdata/`).

`sources` keeps ≤ 4 contiguous chunks of 300 rows per dataset (contiguous so groups such as "all
rows of one country" survive), parses CSV (`,`/`;` with decimal commas), XLSX/XLS/ODS (python-calamine,
header row detected; `MANIFEST.csv` "real header in row2" honoured). Column semantic types come from
`MANIFEST.csv` (ground truth); they are mapped to a generator type that decides which operators apply
(`quantity`→count/int/float, `currency_amount`/`price`→money, `region`→state, …), with header/value
inference as fallback.

## Operators (51)

Stage order per table: B structure on clean values → A values → late B (spacers, repeated header,
transpose) → totals modifiers; then C layout per sheet and D presentation. Every operator tags the
cells it touches (`ops`), logs itself with parameters in the table/sheet `ops` program, and never
changes a cell's provenance.

**A value (18)**: `missing_tokens` ('', `-`, `n/a`, `..`, `#N/A`, …; rate ~Beta(1.2,10)) · `numbers_as_text` ·
`locale_numbers` (1,234.50 / 1.234,50 / 1 234,50 with space/NBSP/NNBSP / 1'234.50) · `currency_units`
($1,200 / 1.200 € / 1,200 USD / 12 kg) · `percent_text` · `negative_formats` ((1,234) / 1234- / −1234) ·
`dates_as_text` (ISO, US, EU dot/slash/dash, `5-Mar-24`, month names en/de/fr/es/it/nl/sr; mixed formats
sometimes) · `date_serials` · `whitespace_noise` · `case_noise` · `categorical_variants` (New York → NY /
N. York) · `typos` · `scale_thousands` (+ "in thousands" title line) · `value_drift` (value moved into the
neighbouring column, swapped if occupied; GT keeps the true column) · `footnote_markers` (+ note line) ·
`excel_errors` · `bool_variants` · `header_rename`.

**B structure (17)**: `group_header_rows` ("Region: North" rows replace the key column) ·
`group_label_column` (period label "Jul '04" / category alone on its row in a new far-left column; GT
`group_header`, and `groups[]` says which rows the value belongs to) · `subtotals` (per group, `=SUM()`
with cached value, sometimes unlabelled) · `grand_total` · `blank_repeats` · `vertical_merge_keys` ·
`total_column` · `wrong_total` (typed wrong value; GT has the true sum) · `totals_as_values` ·
`multirow_header` (1–2 parent rows merged over children: common prefixes, years, generic parents; key
headers merged vertically) · `wrapped_header` · `row_label_column` (text labels left of numbers with
`=`/`:` suffixes, header often blank, 0–2 blank columns before the numbers; role data) · `crosstab`
(key × years/months aggregated measure, optional parent header) · `transpose` · `spacer_rows` ·
`spacer_cols` · `repeat_header`.

**C layout (9)**: `offset` · `title_block` (org / title / scale / as-of / prepared-by; merged sometimes) ·
`table_caption` · `notes_block` (Source:/Note:/`*` lines, localized) · `multi_table` (stacked with gap 0–4,
side by side) · `stack_schema_change` (different datasets stacked, long first segment 100–300 rows:
(a) blank rows + own header, (b) own header no blank row, (c) no blank row and no header,
(d) wider/narrower and shifted ±1–2 columns, (e) second header made of years) · `stray_cells` ·
`multi_sheet` · `empty_sheet`.

**D presentation (7)**: `style_header` (unstyled ~25%, to avoid style leakage) · `style_totals` ·
`number_formats` · `column_widths` · `hidden_rows_cols` · `freeze_panes` · `autofilter`.

Totals: values are computed from what the referenced cells mean (numbers, or the true number behind
a text cell). A total stays a formula only if every referenced cell is a typed number (otherwise Excel's
`SUM` would skip text) — then it is written as a typed value and tagged `total_as_value_text_refs`.

## Profiles

Probabilities are per table (A/B), per sheet (C/D) or per workbook (`multi_sheet`, `empty_sheet`);
see `hh_noise/profiles.py`. Default mix: enron_like 50% · gov_stats 20% · eu_locale 20% · clean_ish 10%.

- `enron_like` — English, US numbers; calibrated to DECO §0 (below); stack_schema_change, row_label_column
  and group_label_column boosted.
- `gov_stats` — statistical-office style: titles 95%, notes 80%, hierarchical headers, footnote marks,
  scale words, crosstabs, group rows; mixed languages (en/sr/de/fr/es/it/nl).
- `eu_locale` — de/fr/es/it/nl/sr labels and dates; EU/FR/CH number formats, currency text.
- `clean_ish` — ~¼ of the base noise.

## Calibration (1,200 workbooks, `calibrate`; DECO-style counting from GT)

| metric (share of sheets with a table / of tables) | DECO target | enron_like | gov_stats | eu_locale | clean_ish |
|---|---|---|---|---|---|
| sheets with title | 54.8% | 52.5% | 96.9% | 56.2% | 13.1% |
| tables with ≥2 header rows | 34.7% | 35.7% | 51.6% | 22.1% | 6.2% |
| sheets with >1 table | 20.0% | 19.5% | 5.2% | 19.1% | 4.7% |
| sheets with totals | 42.7% | 39.9% | 39.7% | 29.9% | 5.5% |
| tables with a mid-table total | 12.3% | 11.3% | 6.1% | 2.4% | 0.7% |
| sheets with notes | 23.3% | 20.6% | 83.7% | 20.8% | 5.8% |
| sheets with group headers | 16.9% | 16.2% | 12.9% | 9.1% | 1.8% |

Enron profiler heuristics (`scripts/profile_corpus.py` on the generated xlsx vs the partial Enron
profile, 295 sheets): merged 48% vs 39%, multi-row header heuristic 60% vs 54%, total labels 37% vs 49%,
SUM formulas 27% vs 49%, whitespace 48% vs 52%, hidden rows/cols 10/9% vs 10/9%, notes 19% vs 13%;
**numbers stored as text 25% vs 5%** (mostly text code columns of the sources) and stray cells 39% vs 71%
(Enron regions heuristic also counts titles) are the largest gaps.

## Ground truth (`<name>.gt.json`, schema `hh-noise-gt/1`)

```
{schema, file, profile, seed, index, generator, lang, num_style,
 datasets: [clean source path …],            op_sets: [[operator …] …],
 cell_fields: [row, col, role, table, ds, src_row, src_col, orig, ops, true],
 sheets: [{name, index, kind: tables|empty, ops: [sheet program], merges, hidden_rows, hidden_cols,
   tables: [{id, rect: [top,left,bottom,right], header_top, header_rows, header_cols, orientation: col|row,
             ds, boundary: {op: sheet_start|multi_table_stacked|multi_table_side|stack_schema_change,
                            variant?, gap_rows?, gap_cols?, shift_cols?, caption?},
             scale, row_kinds, hidden_rows, hidden_cols,
             columns: [{col|row, header (original), header_path (rendered), semantic_type (MANIFEST),
                        gen_type, src_col, derived?}],
             groups: [{label: [r,c], value, rows: [r1,r2]}],
             implied: [{row, col, src_row, src_col, orig, ops, same_as?/merged_into?}],   # values removed
             ops: [table noise program]}],
   cells: [[row, col, role, table|-1, ds|-1, src_row|null, src_col|null, orig, opset|-1, true|null] …]}]}
```

0-based sheet coordinates; every non-empty written cell appears exactly once (merged areas at their
top-left). Roles: title/header/data/group_header/total/note/other. `src_row = -1` marks a source header.
`true` holds value-level truth for text cells: `{"num": x}` (with `style`, `unit`, `scale`, `footnote`),
`{"date": iso, "serial", "fmt", "lang", "ambiguous"?}`, `{"missing": token}`, `{"bool": b}`,
`{"canon": original}` (case/alias/typo), `{"true_col", "drift"}` (value_drift), and for totals
`{"num": true sum, "formula": bool, "wrong"?, "shown"?}`. `index.json` lists all workbooks with
counts and corpus-wide operator counts.

## Scores on synthetic v0 (2,000 workbooks, 3,100 sheets with tables, 3,828 tables, 584k cells)

| analyzer | tables F1 @IoU0.5 | EoB-0 F1 | roles macro F1 (pooled) | DECO dev (tables / roles) |
|---|---|---|---|---|
| baseline | 68.6 | 50.5 | 30.2 | 17.3 / 19.9 |
| layout | 98.0 | 80.1 | 76.5 | 75.1 / 54.7 |

Layout per class F1 (pooled): title 91.9, header 96.3, data 99.2, group_header 64.6, total 76.8, note 83.5,
other 23.3. Recall @IoU0.5 by profile (layout v2): side-by-side 93.4, stacked 93.7,
**stack_schema_change 86.8** — a 94.8, b 89.6, c 68.0, d 86.7, e 96.4 (layout v1: 48.4, c 4.0).
Value parsers (`--values`): numbers 95.5% (currency/unit text 83.5%, e.g. `3 units`; footnote marks like
`1994 p` fail), dates 100% (ambiguous dd/mm 78.4%), missing tokens 100%, booleans 90.9%.
The synthetic set is easier than DECO for whole tables (sources are clean relational tables) but
isolates the hard cases above per operator; see `scores/<analyzer>_synthetic.json` next to the corpus.

## Gaps / next steps

- Sim-to-real: numbers-as-text too frequent; few form/report sheets; no cross-sheet formulas, charts,
  mojibake, Cyrillic↔Latin variants, combined columns, duplicate rows, or ID damage yet.
- Thin semantic types (quarter, email, tax_id) are not synthesized; `canon` truth (case/alias/typo)
  is recorded but not scored yet (needs a categorical-normalization scorer).
- Adversarial validation (real-vs-synthetic classifier) and per-sheet Beta over-dispersion of p's.
