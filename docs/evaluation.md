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
- Speed on very large files.

## See also

- [concepts.md](concepts.md): what tables and roles are
- [development.md](development.md): building the CLI and running the tests
- [`PLAN.md`](../PLAN.md): milestones and their "done when" criteria
