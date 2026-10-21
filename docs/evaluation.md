# Evaluation

How hedgehog's table detection and cell roles are measured, how to run the evaluation, and the
current scores.

Every change to detection is measured before and after on the same annotated data. This page lists
only numbers that were measured with `hedgehog eval`.

Contents: [The DECO corpus](#the-deco-corpus) · [Metrics](#metrics) · [Current scores](#current-scores) ·
[Running the evaluation](#running-the-evaluation) · [Reading the scoreboard](#reading-the-scoreboard) ·
[Other measurements](#other-measurements) · [What is not measured yet](#what-is-not-measured-yet)

## The DECO corpus

DECO (Koci et al., ICDAR 2019)
annotates real Enron spreadsheets with table ranges and a label for every non-empty cell. hedgehog's
loader matches 854 annotated sheets to their files and maps the DECO labels to its seven
[cell roles](concepts.md#cell-roles):

| DECO label | hedgehog role |
|---|---|
| Data | `data` |
| Header | `header` |
| Derived | `total` |
| GroupHead | `group_header` |
| MetaTitle | `title` |
| Notes | `note` |
| Other | `other` |

The files are split 70/30 into **dev** (596 sheets, used while developing) and **test** (258 sheets,
held out for milestone reports). The split is fixed by an FNV-1a hash of the file name, so it never
changes between runs or machines.

## Metrics

**Table detection.** Predicted and annotated table rectangles are matched one to one, best pairs
first; precision, recall and F1 are counted over all tables of all sheets. A predicted table counts
as found when it overlaps the annotated one with:

- IoU ≥ 0.5, IoU ≥ 0.8, IoU = 1.0 (intersection over union, as in StatSheets);
- EoB-0, EoB-2: every edge within 0 or 2 cells of the annotation (as in TableSense and
  SpreadsheetLLM).

**Header depth.** For every table found at IoU ≥ 0.5, the ground truth's header is the run of
rows labelled header (by the majority of their labelled cells) after any title, note or other rows
at the top of its rectangle; the table's header is exact when `header_top` and `header_rows` cover
the same labelled rows. Reported as "header rows exact" (share of found tables).

**Cell roles.** A 7×7 confusion matrix per sheet. Per role, F1 = 2TP / (2TP + FP + FN). Two
averages:

- *pooled*: all cells of all sheets counted together; dominated by `data`, which is most cells;
- *file-macro*: per role, the mean over the sheets of a file, then over files; the macro score is
  the mean over roles. Every file and every role weigh the same, so rare roles (titles, notes,
  group headers) matter. This is the headline role number.

## Current scores

Scores in percent. `baseline` is an 8-connected flood fill (each component of two or more cells is a
table, its first row the header), the number every change must beat. `layout` is the rule-based
detector that `normalize` uses.

| Split | Analyzer | Tables F1 @IoU≥0.5 | @IoU≥0.8 | @IoU=1.0 | EoB-2 | Roles file-macro F1 | Roles pooled F1 | Cell accuracy |
|---|---|---|---|---|---|---|---|---|
| dev | baseline | 17.3 | 13.0 | 7.1 | 12.6 | 19.9 | 17.3 | 92.6 |
| dev | layout | **75.1** | 62.3 | 29.8 | 57.7 | **54.7** | 63.2 | 96.9 |
| test | layout | 65.5 | 51.6 | 34.2 | 50.9 | 53.6 | 57.1 | 95.0 |

Per role, `layout` on dev (pooled F1 / file-macro F1): header 89.7 / 84.6, data 98.5 / 91.3,
total 51.7 / 63.3, title 44.6 / 53.6, note 29.3 / 26.5, group header 54.8 / 41.4, other 73.8 / 22.3.
(0.1.0: tables 74.1, roles 50.4; layout v2 mainly fixed group headers, 11.0 → 54.8 pooled.)

What this means in practice:

- Headers and data are found reliably; the table boundaries are usually right within a row or two
  (IoU ≥ 0.5), less often exact.
- Titles, notes and especially group-header rows are the weak spots. The normalized output keeps
  every cell (misplaced ones end up on `Notes`), so these errors lose structure, not data.
- The dev/test gap on tables (75.1 vs 65.5) shows the rules are tuned on dev. Published systems
  report roughly 0.8 table precision at IoU 0.5 and 0.9 role F1 on comparable data; the learned cell
  model on the [roadmap](../README.md#status-and-roadmap) is meant to close that gap.

## Running the evaluation

The corpus is not in the repository (it is several GB). Put the DECO annotations and the Enron files
under one data root:

```
$HEDGEHOG_DATA/
  irregular/
    deco/annotations/rangeAnnotations.csv
    deco/annotations/cellAnnotations.csv
    deco/completed/           # DECO's annotated copies, used as a fallback
    enron/                    # the original Enron xlsx files
```

Then build the CLI ([development.md](development.md)) and run:

```sh
export HEDGEHOG_DATA=/path/to/testdata
hedgehog eval deco --split dev --analyzer layout --no-json     # scoreboard only
hedgehog eval deco --split dev --analyzer layout                # also writes a JSON report
hedgehog eval deco --split dev --analyzer layout --limit 50     # quick check on 50 files
```

All flags are listed in [cli.md](cli.md#hedgehog-eval). The JSON report holds every number of the
scoreboard plus one row per sheet, so two runs can be compared sheet by sheet. Files are scored on
`--threads` workers (default: up to 4), each holding one workbook at a time; the report is the same
for any thread count. A full dev run took 25 s with a debug build on the development machine; the
`open` and `analyze` times on the `time:` line are summed over the workers.

## Reading the scoreboard

Output of `hedgehog eval deco --split dev --analyzer layout --no-json` (release build, 2 threads):

```text
hedgehog eval · deco · split=dev · analyzer=layout
corpus: 854 files matched (0 unmatched), 854 annotated sheets, 0 unmapped labels
  note: 1 annotated `Range_Annotations_Data` sheet(s) (DECO bookkeeping) ignored
run: 596/596 files opened (0 failed, 0 via fallback copy) · 596 sheets (0 not found) · 1431296 cells scored (1 missing in reader)
time: load 1.3s · open 3.6s · analyze 0.79s (1.33 ms/sheet) · eval total 2.3s

tables (gt 1087, pred 1194)     P      R     F1
  IoU>=0.5                71.8  78.8  75.1
  IoU>=0.8                59.5  65.3  62.3
  IoU>=1.0                28.5  31.3  29.8
  EoB-0                   28.5  31.3  29.8
  EoB-2                   55.1  60.5  57.7

cell roles        support      P      R  F1(pooled)  F1(file-macro)
  title                730  36.1  58.2        44.6            53.6
  header             24580  88.1  91.3        89.7            84.6
  data             1330754  98.2  98.8        98.5            91.3
  group_header        1338  68.6  45.6        54.8            41.4
  total              13895  63.4  43.6        51.7            63.3
  note                 621  43.8  22.1        29.3            26.5
  other              59378  76.5  71.4        73.8            22.3
  macro            1431296                63.2            54.7   (accuracy 96.9)
```

- `corpus`: how many annotated files and sheets were found; unmatched files or unmapped labels would
  mean a broken data root.
- `run`: files that failed to open, or opened only through DECO's re-saved copy (`via fallback`).
- `tables`: `gt` annotated tables, `pred` predicted tables; P/R/F1 per criterion.
- `cell roles`: `support` is the number of annotated cells per role.

## Cross-corpus benchmark

DECO is one company's spreadsheets. To check that improvements generalize, `hedgehog benchmark`
scores every labelled corpus under `$HEDGEHOG_DATA`, on its test files:

| Corpus | Source | Labelled cells (test) | Labels |
|---|---|---:|---|
| DECO | Enron business spreadsheets | 355,815 | seven roles |
| StatSheets | 14 statistics publishers (ABS, BEA, INSEE, OECD, WHO, World Bank, Japan, Saudi Arabia, ...), 6 languages, xlsx/xls/ods/csv | 1,804,833 | title, header, data, other |
| Strudel | SAUS, CIUS, DeEx and Troy tables, converted to xlsx by `tools/benchmarks/convert_strudel.py` | 261,467 | seven roles |
| Synthetic | `tools/noise` | 169,127 | seven roles |

Scores are given in the full seven-role space where a corpus has it, and in the four roles all
corpora share (group headers count as headers, totals as data, notes as other). Two label
conventions are harmonized: StatSheets marks row labels (stub columns) as headers, which DECO and
hedgehog treat as data, so a StatSheets header cell with data to its right in the same row is
scored as data; its grids are stored compressed and are mapped back with hedgehog's reader (652 of
737 sheets align, all 65 CSV sheets among them).

Rules (`layout`) vs rules + learned model (`model`), test splits:

| | 7 roles pooled | 7 roles per file | 4 roles pooled | 4 roles per file |
|---|---|---|---|---|
| DECO rules / model | 57.1 / **67.3** | 53.6 / **59.5** | 65.7 / **80.0** | 64.4 / **70.9** |
| StatSheets rules / model | — | — | 48.1 / **72.3** | 65.6 / **89.6** |
| Strudel rules / model | 50.6 / **63.2** | 48.6 / **61.6** | 73.9 / **87.5** | 77.1 / **90.5** |
| Synthetic rules / model | 75.9 / **93.0** | 79.5 / **91.3** | 86.3 / **97.1** | 91.5 / **98.0** |

For the corpora with table ranges (DECO, StatSheets, synthetic) the benchmark also prints table
precision / recall / F1 at every criterion, the header-depth score, and table F1 per source; the
`--out` TSV has `table_f1_iou50/80/100` per source. The conventions differ: DECO counts title rows
touching a table as part of its range, StatSheets and the synthetic set start a table at its
header. Rules' tables vs the model path's (which refines them with the row roles, see
[learned-model.md](learned-model.md#how-it-works)), test splits, F1 at IoU ≥ 0.5 / 0.8 / 1.0:

| | Rules' tables | Refined by the model | Header rows exact |
|---|---|---|---|
| DECO | 65.5 / 51.6 / 34.2 | **68.2 / 56.0 / 36.7** | 75.8 → **81.5** |
| StatSheets | 83.6 / 72.5 / 36.8 | **91.5 / 86.9 / 74.6** | 60.2 → **72.2** |
| Synthetic | 98.1 / 95.7 / 82.4 | **98.0 / 97.0 / 94.8** | 89.7 → **96.3** |

To see where tables go wrong, `HH_GT_CORPUS=deco|statsheets|synthetic cargo test --release -p hh_core
--test tables_gtdump -- --ignored --nocapture` lists dev sheets whose tables miss the ground truth,
row by row (ground-truth, model and rule roles); `HH_GT_DIFF=1` only those the model path changed.

Leave-one-corpus-out (`tools/train/rows.py`: train on three corpora, test on the fourth; four
roles per file): DECO 73.6 → 74.8, StatSheets 66.7 → 78.0, Strudel 79.1 → 86.6, synthetic
93.7 → 94.5. More in [learned-model.md](learned-model.md).

**Formatting features** (bold, fills, borders, font size, number format, ... from xlsx styles;
[learned-model.md](learned-model.md#formatting-features)), leave-one-corpus-out on the current
rows (StatSheets with its CSV sheets), four roles per file, mean of three seeds, without → with:
DECO 75.2 → 75.6, StatSheets 79.9 → 80.1, Strudel 86.6 → 87.3, synthetic 94.3 → 94.7. Retrained
on everything, the best candidate (`--style-dropout 0.3`, master's training rows) against the
shipped model, test splits:

| | 7 roles pooled / per file | 4 roles pooled / per file |
|---|---|---|
| DECO shipped / with formatting | 67.3 / 59.6 → 67.5 / 59.5 | 80.0 / 70.9 → 81.3 / 70.8 |
| StatSheets | — | 72.3 / 89.6 → 72.7 / 89.9 |
| Strudel | 63.2 / 61.6 → 64.1 / 62.1 | 87.5 / 90.5 → 88.6 / 91.3 |
| Synthetic | 93.0 / 91.3 → 93.7 / 92.4 | 97.1 / 98.0 → 97.4 / 98.2 |
| European sample | 61.3 / 65.4 → 61.4 / 63.6 | 87.2 / 84.7 → 88.1 / 85.1 |

It is not shipped: the European sample loses 1.8 points per file in seven roles (group headers
33 → 25, 36 cells) and DECO totals 78.7 → 72.7; a retrain without formatting loses as much on the
European sample (65.4 → 64.5), so this is mostly retraining variance.

## European open-data sample

A check on files none of the corpora above come from: 31 sheets from 20 European open-data
sources (statistics offices and city/national portals in AT, BE, CH, DE, ES, FR, IT, NL, NO, PL,
PT, SE), 61,874 non-empty cells, every one labelled with one of the seven roles. No model trains on
them, so `hedgehog benchmark --corpora eu` always scores the whole sample, whatever `--split`
says. The corpus is opt-in (not in the default `--corpora`).

**The labels were written by an AI assistant (Claude) reading the sheets, not by the people who
made them or by domain experts, and have not been reviewed by a person yet.** They were finished
before hedgehog was run on these sheets, so they are not tuned to its output, but they are a check,
not ground truth. Corrections go into the spec and are rebuilt:

```bash
python -I tools/benchmarks/select_eu_sheets.py --data $HEDGEHOG_DATA --bin target/release/hedgehog
python -I tools/benchmarks/build_eu_labels.py --data $HEDGEHOG_DATA --bin target/release/hedgehog
hedgehog benchmark --corpora eu --analyzer model
```

- **Selection** (`select_eu_sheets.py`, deterministic): per source folder in
  `testdata/irregular/eu_*`, sheets of 40 to 20,000 non-empty cells, ranked by merged ranges
  (merged titles, multi-row headers) first, then 150 to 6,000 cells, then a hash of file and sheet
  name; one sheet per workbook, two per source with 30 or more candidates. Two sheets were looked at
  and skipped (listed with the reason in the script): a leftover link sheet without headers, and a
  form with forty small M/F/T sub-tables that would have dominated the sample. The sample covers
  merged titles with table codes, multi-row and bilingual headers, section rows, national and
  regional aggregate rows, footnote and source blocks, side-by-side tables, stacked blocks, long
  flat lists and a free-text table.
- **Labels** (`tools/benchmarks/eu_labels.json`): per sheet, A1 ranges with a role applied in
  order, plus why the sheet was picked and what was uncertain. `build_eu_labels.py` expands them
  against the cells hedgehog's reader sees (`hedgehog cells <file> --sheet NAME`) into
  `testdata/irregular/eu_labelled/labels.tsv`, and fails if any cell is left unlabelled or a range
  matches nothing. Labels: data 60,248 · total 788 · header 627 · note 116 · title 54 ·
  group header 36 · other 5. One 17,294-cell list (PT) holds 28% of the cells, so the per-file
  scores are the fairer summary.
- **Conventions:** row labels and stub columns are data; every row of a column header is header
  (including stub-head cells, unit rows and column-number rows); a label alone on its row that
  starts a section is group header; titles, subtitles, year and unit lines above a table are title;
  footnotes, sources, as-of dates, contacts, URLs and metadata blocks are note; navigation links
  (`zur Inhaltsübersicht`) and stray cells are other. A row is total (label and values) when it is
  marked as one (Total, Insgesamt, Summe, Ogółem, i alt, ...) or aggregates the listed units (the
  country above its cantons, macro-regions after Italian regions, the EU row, annual averages after
  twelve months). Parent rows of an indented hierarchy without total wording stay data, and total
  columns stay data. These last two choices are the least certain.

Rules (`layout`) vs rules + learned model (`model`), whole sample:

| | 7 roles pooled | 7 roles per file | 4 roles pooled | 4 roles per file |
|---|---|---|---|---|
| European sample rules / model | 46.4 / **61.4** | 48.0 / **65.4** | 82.2 / **87.2** | 73.2 / **84.8** |

Per role (7 roles pooled F1, rules / model): title 62.4 / 77.1 · header 86.7 / 94.5 · data
99.3 / 99.3 · group header 26.3 / 33.5 · total 29.0 / 29.7 · note 16.9 / 80.7 · other 4.0 / 14.8.
Totals and group headers are the weak spots.

Per source (7 roles pooled · 4 roles pooled, rules → model; one or two sheets each, so single
labels move these a lot):

| Source | Sheets | Cells | 7 roles | 4 roles |
|---|---:|---:|---|---|
| eu_at_opendata | 2 | 2,786 | 100.0 → 100.0 | 100.0 → 100.0 |
| eu_at_statistik_austria | 2 | 4,105 | 52.7 → 81.5 | 66.7 → 100.0 |
| eu_be_opendata | 2 | 1,071 | 55.6 → 82.8 | 91.6 → 96.4 |
| eu_ch_bfs | 2 | 558 | 21.1 → 74.9 | 49.3 → 94.1 |
| eu_ch_opendata | 2 | 2,977 | 47.5 → 57.5 | 47.4 → 74.0 |
| eu_de_destatis | 2 | 2,891 | 34.5 → 62.8 | 79.0 → 86.5 |
| eu_de_opendata | 1 | 1,447 | 74.9 → 59.8 | 77.1 → 57.8 |
| eu_es_ine | 1 | 5,934 | 49.9 → 74.9 | 28.2 → 91.4 |
| eu_es_opendata | 1 | 232 | 78.9 → 100.0 | 100.0 → 100.0 |
| eu_fr_insee | 2 | 2,246 | 79.7 → 79.7 | 100.0 → 100.0 |
| eu_fr_opendata | 1 | 3,348 | 34.9 → 33.8 | 38.8 → 34.7 |
| eu_it_istat | 2 | 637 | 46.5 → 69.4 | 81.8 → 91.8 |
| eu_it_opendata | 2 | 7,769 | 75.0 → 91.1 | 100.0 → 88.1 |
| eu_nl_opendata | 1 | 2,517 | 46.4 → 63.5 | 46.4 → 63.5 |
| eu_no_ssb | 1 | 1,421 | 49.3 → 64.7 | 100.0 → 100.0 |
| eu_pl_dane_gov_pl | 2 | 1,245 | 38.9 → 40.5 | 66.9 → 64.8 |
| eu_pl_gus | 2 | 493 | 64.5 → 68.7 | 81.8 → 80.4 |
| eu_pl_opendata | 1 | 153 | 100.0 → 100.0 | 100.0 → 100.0 |
| eu_pt_opendata | 1 | 17,294 | 96.2 → 100.0 | 94.9 → 100.0 |
| eu_se_opendata | 1 | 2,750 | 100.0 → 100.0 | 100.0 → 100.0 |

## Other measurements

- **Reader robustness:** on a random sample of 500 Enron spreadsheets, the reader opened 500/500,
  at about 17 ms per file including process start. Twelve of them list a `veryHidden` sheet with an
  empty relationship id, which the underlying `calamine` reader rejects; hedgehog removes that entry
  in memory and reads the rest of the workbook.
- **Normalization:** covered by tests on generated messy workbooks (`core/tests/normalize_messy.rs`,
  `py/tests/test_normalize.py`); there is no corpus-level score for it yet.

## What is not measured yet

- Value parsing and column typing on real data (unit tests only).
- Totals detection against annotations (DECO's `Derived` label covers more than totals).
- End-to-end agent question answering on a fixed question set (`hedgehog.llm.bench` exists, a
  shared benchmark does not).

## See also

- [concepts.md](concepts.md): what tables and roles are
- [development.md](development.md): building the CLI and running the tests
- [`PLAN.md`](../PLAN.md): milestones and their "done when" criteria
