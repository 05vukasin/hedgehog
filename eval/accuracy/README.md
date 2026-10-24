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
| `causes.py` | C: one family + cause + evidence for every C miss (library / oracle-limit / gold/question) |
| `invariants.py` | M: misleading-output invariants |
| `scoreboard.py` | F, S, L, C, M, K, miss clusters, audit, RESULTS.md |
| `audit.tsv` | audited misses with evidence |
| `reviewed.tsv` | reviewed audit candidates that are not auditable: their oracle-limit / library cause with evidence |
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
  (change between years), the same column in two tables: a−b, a+b, a/b, 100·a/b, (a−b)/b, 100·(a−b)/b,
  1 − a/b (complement share); date differences in days; hypotheticals ("increases by 10 %", "doubled");
- **wholes** (v2): hedgehog's total rows, total-labelled rows and columns ("Total", "All agencies"),
  arithmetic wholes (a row = the sum of the block of 2+ rows below / above it, a column = the sum of the 2+
  columns next to it), "All X" rows named by "X": the denominator of a share of a named part inside its block,
  also when the question does not name the whole ("of the $75.2 billion spent on R&D, how many percent ...");
- **v2 shapes**: numeric range labels tiling a question range ("aged 18 to 34" = 18-24 + 25-34); argmax /
  argmin (the value of a named column, or the year label, in the row where a named column is extreme; the
  year of a row's extreme year column); k-th largest; counts of rows where one named column exceeds another;
  the change to the year next to the one named ("up from FY 2016"); 100 − x of a percentage whose row label
  negates what the question asks ("No prior experience" for "some prior experience").

**Grounding guard.** Names are matched after folding "R&D"-style acronyms, plurals, men/women ~ male/female
and label acronyms ("NASA"); a value named by most of its words names nothing when 3+ values of its column are
named that way. A witness counts only when it is *tight*: the operation is named by the question (a
generous lexicon: "difference", "more than", "ratio", "share", "total", "average", ...) and the row set and the
column are named (an aggregate over all data rows needs a fully named column; a count needs its row set). Items
where only loosely grounded or ungrounded programs match are misses with that cause (they are listed: a
library cause often hides there, e.g. a column named `Yuan (2)` instead of "Depreciation (Yuan)").
Pair guards (v2, each from a decoy or spot-check finding): a share divides a part by a whole unless the
question asks for a ratio ("times", "ratio", "as a percentage of"); growth compares two periods, or two
entities with "than" / "compared" / "from A to B", and never a column that already holds changes; "how many
times" is answered only by a ratio; "+" never sums overlapping row sets and is not named by a bare "and";
'Mean' / 'S.D.', 'number' / 'percent' columns are no pair; IN-lists only as one set in two columns or the
part of a share.
**Matching** (agent_mimotable's rules): absolute values; |x − gold| ≤ half a unit of the gold's last shown
digit, or the gold is x truncated (when it shows decimals); percent ↔ fraction; scale words.

**Validation** (numbers in RESULTS.md, "C oracle: validation"):
1. HiTab computed items give the gold operand cells. Among the atoms whose gold those cells reproduce with a
   formula of the program space, the oracle must reach them with a tight witness that uses only gold operand
   cells (agreement).
2. Decoy test: the gold × 1.1 and × 0.9 at the same precision; the share of decoys the oracle "reaches"
   estimates the false-reachable rate per gold magnitude. Small integers (≤ 10: counts) are weak evidence by
   nature; integers > 100 and decimals are not.
3. v2: two seeded samples of 30 witnesses that v1 did not reach, checked by hand (2026-10-23): 24 / 30 right
   on the first draft (false: a lookup through a range member, a complement cued by "non-", growth between
   two entities, a sum over mixed number / percent rows, a value lookup for a "how many times" question) -
   each tightened; 28 / 30 on the final space (false: a column named only by its leaf segment, 2645.xlsx
   'Males / SHeS / N'; one uncertain growth between two mice for two time points, science-table35).
   v1: a seeded sample of 30 reachable MiMoTable items (`oracle.py --spot`), checked by hand (2026-10-23):
   29 / 30 verdicts right. In 23 the first witness is the intended program, in 3 the intended program is a
   later witness (en-0324, en-0553, en-0501), in 3 the value is right but the witness is another program
   (argmax lookups and a filter outside the program space: en-0638, en-0250, en-0135); 1 is a coincidence
   (en-0549: "6 records with Repayment OK" matched a row count, while hedgehog took that row as the header -
   M catches it).

**Miss causes** (`causes.py`, v2): every C miss has exactly one family and cause with an evidence string;
RESULTS.md lists them ("C misses by family and cause") and the audit candidates. In order:
- *library*: an operand cell in no table / a value changed / a number kept as text / a total row among the
  data rows (reachable when total-labelled rows are dropped) / **hedgehog header naming** (generated column
  names - `col_X`, `Yuan (2)`, a bare unit - replaced by the source header cells above the column make a tight
  witness: the witness is recorded, the miss stays hedgehog's) / header missing (the best match uses a
  generated column name) / label path lost or header misnamed (a HiTab gold header-path element the question
  names is not in hedgehog's row labels or column name) / operands split into two tables;
- *gold/question* (HiTab, from the gold operand cells): the gold is not reproduced from its own operand cells
  by any reading of its aggregation, or the question names no word of an operand's header path; MiMoTable
  lookups whose answer is no cell. Audit candidates: listed for review, never audited automatically;
- *oracle-limit*: grounding (the question paraphrases an operand, names the column or the operation only in
  part, an implicit period or national total) or a program shape the oracle does not enumerate (from HiTab's
  aggregation and operand geometry, else the question's wording: forecast, group-by aggregate, median, ...).

## M: nothing misleading

Per normalized table (`invariants.py` docstring): M1 a total-labelled row among data rows whose number is the
sum of its block (arithmetic, 0.1 %); M2 percent and plain numbers mixed in a column, percent scale mixed;
M3 percent/rate rows and counts mixed in a column without a unit (a row is percent/rate only when all its numbers
stay within 105: a '%' in a name is none); M4 dates out of 1900-2100 (unless the source
writes that year), dates collapsed, a source date text stored with another day, Excel serials left as numbers
in a date column; M5 `col_X` columns, duplicate names whose header above was lost, 3+ adjacent columns with one
name (a data row taken as the header), a plain number as a header; M6 an outline number "(4)" stored as −4. The checker was reviewed by hand per class on a seeded sample against
the source cells (2026-10-24; precision table in RESULTS.md, "M classes"); its false-positive classes are fixed
and tested (`test_invariants.py`: `python3 -I -m unittest discover -s eval/accuracy -p 'test_*.py'`).

## Clusters and audit

A C or L miss on a file that has an M violation is clustered under that violation (most specific first), else
under the measure's own cause. `audit.tsv` lists misses that are not hedgehog's: `gold_wrong`, `not_in_file`,
`outside_knowledge`, `ambiguous_question` (two reasonable readings give different answers; both shown), each with
evidence checked against the source cells (independent reader, eval/fidelity/source.py). Scores are reported raw
and after audit (audited items leave the denominator; a pass on an audited item does not count). Never audit a
miss without evidence. `reviewed.tsv` lists gold/question candidates checked the same way and found NOT auditable
(the gold is right and in the file): they stay misses under the oracle-limit / library cause recorded there.
