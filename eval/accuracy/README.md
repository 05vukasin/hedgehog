# Accuracy scoreboard (model-free)

The measurement side of [docs/ACCURACY_PLAN.md](../../docs/ACCURACY_PLAN.md): does hedgehog give an agent
everything a public benchmark question needs, exactly right? No LLM anywhere; same numbers on every run.

```sh
make score CARGO_TARGET_DIR=../hedgehog/target-master LOGS=../scratch-archive/accuracy/logs   # dev split
make score-holdout ...                                                                         # release only
```

`make score` runs `eval/public/run_hedgehog.py` for the three QA datasets with the current build (outputs per
build under `../scratch-archive/accuracy/work/<sha256[:12] of the binary>/`), then `scoreboard.py`, which writes
[RESULTS.md](RESULTS.md) and `results.json`. Data: `eval/public` (fetch + convert scripts, never in git).

| File | What it does |
|---|---|
| `split.py`, `split/*.txt` | dev / holdout file lists (below) |
| `acc.py` | items of a split, their gold atoms (L or C), hedgehog's tables (eval/public `hh_load` + dtypes) |
| `lookup.py` | L: lookup reachability |
| `oracle.py` | C: computation-reachability oracle, its validation (`--validate`, `--spot`) |
| `invariants.py` | M: misleading-output invariants |
| `scoreboard.py` | F, S, L, C, M, K, miss clusters, audit, RESULTS.md |
| `audit.tsv` | audited misses with evidence |
| `k_baseline.json` | K baseline (catalog tokens per file), written with `scoreboard.py --update-k` |

## Split

`split.py` (seed `hedgehog-accuracy-holdout-2026-10-23`, recorded in the script): HiTab's official split
(train + dev → dev, test → holdout; HiTab's splits are file-disjoint). MiMoTable and RealHiTBench: file-level
80 / 20; files that share a question are one unit; strata MiMoTable `table_type` × language, RealHiTBench
`structure`; within a stratum units are ordered by sha256(seed + unit) and the first round(20 %) are holdout.
`python3 -I eval/accuracy/split.py --check` recomputes and compares with the committed lists.

| Dataset | dev files / questions | holdout files / questions |
|---|---|---|
| HiTab | 1,303 / 6,309 | 216 / 1,015 |
| MiMoTable | 402 / 1,307 | 100 / 329 |
| RealHiTBench | 537 / 3,006 | 135 / 746 |

Every scorer takes `--holdout` to score the holdout; without it only dev items are loaded, so holdout
questions never appear in failure lists. `scoreboard.py --holdout` writes aggregates only.

## Gold atoms (acc.py)

An item's answer is split into atoms; each is measured by L (find a cell) or C (compute a value):
HiTab lookups → L with the gold cell; HiTab computed items with numeric answers → C (gold operand cells kept for
validation); RealHiTBench Numerical Reasoning numbers → C, other answers → L (Data Analysis, Visualization: not
attempted); MiMoTable English → the gold key of `eval/public/agent_mimotable.py` (`gold_key`, `KEY_KEEP`;
`AUDIT_EXCLUDE` items not attempted), numbers of Compare / Calculate / Reasoning questions → C, the rest → L;
Chinese narratives: not attempted (no deterministic key extractor). An L number that is no cell of the source
file moves to C; an L text that is no source cell is a paraphrase and not attempted.

## L: lookup reachability

HiTab: the gold answer cell is a cell of a normalized table, its number equals the source, the column name holds
the gold top-header path (whole ` / ` segments, in order) and every element of the gold left-header path is a
label of the row. HiTab's own normalisations are allowed: case and punctuation, a bracketed unit or qualifier the
annotators dropped, a glued footnote letter, a year row label kept as a number, a gold root that is the label
column's header or a total row (HiTab roots its left tree there), placeholder elements (`-`).
MiMoTable / RealHiTBench: the atom is a table cell, a column header or a title; a number must also share a
content word with the question through its row labels or column header (else a bare number is found by chance);
a date atom matches a normalized date (or kept text) with the same month and day.
A miss follows the atom's source cell into the tables: cell not in any table / value changed (how: date text
parsed with another day, Excel serial, number kept as text, ...) / column header path differs / label path
missing parent / row label differs / generic column header / number not grounded.

## C: computation-reachability oracle

For every C atom, the oracle enumerates small programs over hedgehog's normalized tables and tests whether
one gives the gold at the gold's shown precision (`oracle.py` docstring has the full program space):

- **filters** grounded in the question: label values (text, group and `_parent` columns) named in the question
  (phrase at word boundaries, plurals folded, or all / most content words), years, year ranges ("from 2012 to
  2015"), months, row ids ("serial number 5", "the 8th"), "outside X" (complement); conjunctions of two;
  IN-lists of one column; thresholds with a question number (counts);
- **aggregates** on data rows only (`_row_type` = data; total rows only through single-row lookups): value,
  sum, avg, min, max, range, count, count rows, count distinct, top / bottom N, row-wise sum / avg / min / max
  across the question's columns (or a year range of columns), rank;
- **binary programs** of like operands in the shapes questions ask for: two named row sets in one column
  (difference, entity / whole share), one named row set in two columns that differ in one header segment
  (change between years), the same column in two tables: a−b, a+b, a/b, 100·a/b, (a−b)/b, 100·(a−b)/b;
  date differences in days; hypotheticals ("increases by 10 %", "doubled").

**Grounding guard.** A witness counts only when it is *tight*: the operation is named by the question (a
generous lexicon: "difference", "more than", "ratio", "share", "total", "average", ...) and the row set and the
column are named (an aggregate over all data rows needs a fully named column; a count needs its row set). Items
where only loosely grounded or ungrounded programs match are misses with that cause (they are listed: a
library cause often hides there, e.g. a column named `Yuan (2)` instead of "Depreciation (Yuan)").
**Matching** (agent_mimotable's rules): absolute values; |x − gold| ≤ half a unit of the gold's last shown
digit, or the gold is x truncated (when it shows decimals); percent ↔ fraction; scale words.

**Validation** (numbers in RESULTS.md, "C oracle: validation"):
1. HiTab computed items give the gold operand cells. Among the atoms whose gold those cells reproduce with a
   formula of the program space, the oracle must reach them with a tight witness that uses only gold operand
   cells (agreement).
2. Decoy test: the gold × 1.1 and × 0.9 at the same precision; the share of decoys the oracle "reaches"
   estimates the false-reachable rate per gold magnitude. Small integers (≤ 10: counts) are weak evidence by
   nature; integers > 100 and decimals are not.
3. A seeded sample of 30 reachable MiMoTable items (`oracle.py --spot`), checked by hand (2026-10-23):
   29 / 30 verdicts right. In 23 the first witness is the intended program, in 3 the intended program is a
   later witness (en-0324, en-0553, en-0501), in 3 the value is right but the witness is another program
   (argmax lookups and a filter outside the program space: en-0638, en-0250, en-0135); 1 is a coincidence
   (en-0549: "6 records with Repayment OK" matched a row count, while hedgehog took that row as the header -
   M catches it).

## M: nothing misleading

Per normalized table (`invariants.py` docstring): M1 a total-labelled row among data rows whose number is the
sum of its block (arithmetic, 0.1 %); M2 percent and plain numbers mixed in a column, percent scale mixed;
M3 percent/rate rows and counts mixed in a column without a unit; M4 dates out of 1900-2100 (unless the source
writes that year), dates collapsed, a source date text stored with another day, Excel serials left as numbers
in a date column; M5 `col_X` columns, duplicate names whose header above was lost, 3+ adjacent columns with one
name (a data row taken as the header), a plain number as a header; M6 an outline number "(4)" stored as −4.

## Clusters and audit

A C or L miss on a file that has an M violation is clustered under that violation (most specific first), else
under the measure's own cause. `audit.tsv` lists misses that are not hedgehog's: `gold_wrong`, `not_in_file`,
`outside_knowledge`, each with evidence checked against the source cells. Scores are reported raw and after
audit (audited misses leave the denominator). Never audit a miss without evidence.
