# The learned model

Since 0.3, hedgehog labels cells with **the layout rules plus a small learned model**. The rules
find the tables and give every cell a first role; the model looks at each row of a table as a
whole and decides what kind of row it is (title, header, data, group header, total, note or other).
The model's row roles then correct the tables themselves: where they start and end, how deep the
header is, and where stacked tables split.
On four independent hand-labelled benchmarks it is more accurate than the rules alone, it runs on
any CPU without a GPU or extra dependencies, and it is on by default.

## How it works

```
 sheet ──▶ rules (layout) ──▶ cell features ──▶ row segments ──▶ two-stage model ──▶ safeguards ──▶ roles ──▶ tables
            tables, a role     110 numbers        cells of one     coarse family,      keep rule roles         edges, header
            for every cell     per cell           row, ~450        then the exact      where they are          depth, splits
                                                  numbers each     role inside it      the safer call          from row roles
```

1. **Rules first.** The rule layout finds the tables and labels every cell. Its tables are the
   starting point; step 6 corrects them.
2. **Cell features.** About 110 numbers per cell: what the value is (number, text, date, keyword),
   its neighbours, its row and column statistics, whether it is a total, and the role the rules gave
   it and its neighbours.
3. **Row segments.** The filled cells of one sheet row, split where more than two empty columns
   separate them. 97% of rows in the labelled corpora have a single role, and studies of
   spreadsheet layout (Koci et al. 2016) found that classifying cells one by one makes more mistakes
   than working on rows. Each segment is described by the average of its cells' features, its first
   cell (usually the row label), and the rows above and below: about 450 numbers.
4. **Two-stage model.** A *coarse* model picks the family (title, header, data or other); a *fine*
   model then picks the exact role inside that family (header or group header, data or total, note
   or other). Both are gradient-boosted decision trees (XGBoost), 176 + 476 trees, built into the
   library (`core/models/row_model.bin`, 2 MB).
5. **Safeguards.**
   - Cells the rules set apart from the rest of their row keep the rules' role (a group label in
     the key column of a data row).
   - Deep inside a table (ten or more rows into its body), a row the rules call data, with data
     rows above and below, stays data if the model says title or note, or header while the row is
     at least half as full as its neighbours. Titles and notes live at the edges of tables; a real
     sub-header between data rows is much sparser than the data around it. Without this, one
     176,000-row register lost 780,000 cells to `Notes`.
   - Sheets with more than 1,000,000 cells use the rules alone (the features take about 440
     bytes per cell); the manifest lists a warning for each.

6. **Tables from row roles** (`core/src/learned/tables.rs`). Per table, each row's role is the
   majority role of its cells inside the rectangle, and:
   - a rules table the model calls header from top to bottom, directly above a table over the
     same columns, becomes the top of that table's header (ABS time series: the ten metadata rows
     `Unit`, `Series Type`, … `Series ID` that the rules cut into three tables);
   - a table directly below another over the same columns, whose "header" the model calls data,
     continues it (the rules cut a list at a change of content and took its first data row, such
     as `Inactivation | Koch Gateway Pipeline …`, for a header);
   - rows just above the header that the model calls header extend it; trailing header rows it
     calls data (80% of the cells or more) move into the body; a column-numbering row
     (`1 2 3 …`) under the header stays in it;
   - a wide run of rows the model calls header, after a blank row and between data rows, starts a
     new table, unless it repeats the table's header or the table would fall into more than ten
     pieces (a long list with recurring section headers);
   - trailing rows the model calls note or title (or other, where the rules do not call them data)
     leave the table, and so do title, note and other rows above the header — the table range is
     header plus body, as in StatSheets and the synthetic set (DECO counts titles touching a table
     as part of it, and still scores better this way);
   - a table without a row the model calls header, data or total (a key/value block such as
     `Data Source | World Development Indicators` above a World Bank table) is dropped; rows below
     a table that the model calls data, inside its columns and in no other table, join it;
   - no two tables overlap.

   This only runs in the model path; `--rules-only` keeps the rules' tables.

The model never moves a cell out of the workbook: whatever role a cell gets, it ends up in a table,
on `Totals` or on `Notes`, with its address.

## Accuracy

Scored on the test files of each benchmark, which the model never saw in training. F1 runs from 0
to 100. "Per file" averages over files, so every spreadsheet counts the same; "pooled" counts every
cell once.

| Benchmark | What it is | Rules (7 roles, pooled / per file) | **Rules + model** | 4 shared roles per file: rules → model |
|---|---|---|---|---|
| DECO | Enron business spreadsheets | 57.1 / 53.6 | **67.3 / 59.5** | 64.4 → **70.9** |
| StatSheets | 14 statistics publishers, 6 languages | — | — | 65.6 → **89.6** |
| Strudel | US government and scientific tables | 50.6 / 48.6 | **63.2 / 61.6** | 77.1 → **90.5** |
| Synthetic | generated messy workbooks | 75.9 / 79.5 | **93.0 / 91.3** | 91.5 → **98.0** |

No role is worse with the model than with the rules by more than 0.1 point on DECO, Strudel or the
synthetic set; most improve by 10 to 40 points (Strudel notes 50 → 89, synthetic totals 66 → 95,
DECO headers 56 → 84). StatSheets labels only four roles, so it is scored in those.

**Tables.** Table detection (F1 in percent, test splits; a table counts as found at IoU ≥ 0.5,
0.8 or exactly) and how often the header rows of a found table are exactly right:

| Benchmark | Rules' tables @0.5 / @0.8 / exact | **Refined by the model** | Header rows exact: rules → model |
|---|---|---|---|
| DECO | 65.5 / 51.6 / 34.2 | **68.2 / 56.0 / 36.7** | 75.8 → **81.5** |
| StatSheets | 83.6 / 72.5 / 36.8 | **91.5 / 86.9 / 74.6** | 60.2 → **72.2** |
| Synthetic | 98.1 / 95.7 / 82.4 | **98.0 / 97.0 / 94.8** | 89.7 → **96.3** |

Cell roles stay where they were (DECO 67.4 / 59.8, StatSheets 89.6, Strudel 63.2 / 61.6, synthetic
93.0 / 91.3); on the European sample the per-file score drops 0.5 (65.4 → 64.9), from one Destatis
unit row (`Mrd. EUR`) that the model now calls header instead of group header.

**Generalization.** Before training on all four corpora, each corpus was held out in turn and the
model trained on the other three (leave-one-corpus-out). It still beat the rules on every held-out
corpus (per file, four roles): DECO 73.6 → 74.8, StatSheets 66.7 → 78.0, Strudel 79.1 → 86.6,
synthetic 93.7 → 94.5. The gain is not a memorized corpus. Details and how to reproduce them:
[evaluation.md](evaluation.md#cross-corpus-benchmark).

## Speed and memory

On 365 real files (315 European government spreadsheets and 50 Enron files):

| | Rules only | Rules + model |
|---|---|---|
| Typical file (median) | 19 ms | 29 ms |
| 95% of files under | 1.5 s | 2.5 s |
| Slowest file (4.5M cells, above the model limit) | 16.8 s | 16.6 s |
| All 365 files | 126 s | 186 s |
| Peak memory, 4.5M-cell file | 1.23 GB | 1.24 GB |
| Peak memory, file with a 947k-cell sheet (just under the limit) | 0.75 GB | 1.48 GB |
| Peak memory, 5 sheets / 1.5M cells | 0.78 GB | 0.90 GB |
| Peak memory, 23 sheets / 3.1M cells | 0.51 GB | 0.81 GB |

Measured on v0.3.1, 16-core laptop.

The model's features take about 440 bytes per cell of the sheet being analyzed, so a sheet near
the 1,000,000-cell limit needs up to about 0.75 GB more than the rules alone; `--model-max-cells`
lowers the limit on machines with little memory.

What keeps it fast: the model labels rows, not cells (about 25 times fewer predictions); the fine
model only evaluates the trees of roles inside the family the coarse model chose; trees are
packed into 16-byte nodes and evaluated a block of rows at a time; repeated texts and numbers are
parsed once per sheet; on sheets of 20,000+ cells the feature groups run on parallel threads, and
the sheets of a workbook are analyzed in parallel when they hold at most 300,000 cells together
(larger workbooks go one sheet at a time, so peak memory is set by the largest sheet). A single-core machine runs the same code
sequentially and gets the same result.

## Using it

It is on by default. To label with the rules alone:

```console
$ hedgehog normalize sales.xlsx -o out/ --rules-only
```

```python
hedgehog.normalize("sales.xlsx", "out/", rules_only=True)
hedgehog.normalize("big.xlsx", "out/", model_max_cells=5_000_000)   # raise the size limit
```

The manifest records which was used: `options.cell_roles` is `"model"` or `"rules"`.

## Retraining

Training runs on a laptop CPU in about two minutes (Python, `tools/train/`, needs `pandas`,
`pyarrow`, `xgboost`):

```sh
export HEDGEHOG_DATA=/path/to/testdata
for c in deco statsheets strudel synthetic; do
  hedgehog features --level row --corpus $c --out rows_$c.csv     # features + labels per row segment
done
python tools/train/rows.py rows_*.csv          # leave-one-corpus-out check
python tools/train/train_rows.py rows_*.csv --out models/      # train, size sweep, export
cp models/coarse.json core/models/row_coarse.json
cp models/fine.json core/models/row_fine.json
hedgehog pack-model core/models                # → core/models/row_model.bin (embedded)
hedgehog benchmark --analyzer model            # scores on every test split
```

`hedgehog features --level row` and the library compute the features with the same code, so a
trained model sees in production exactly what it saw in training. A unit test checks that the
embedded binary decodes to the same trees as the JSON files.

## See also

- [evaluation.md](evaluation.md): benchmarks, metrics, how to run them
- [concepts.md](concepts.md#cell-roles): the seven cell roles
- [architecture.md](architecture.md): where the code lives
