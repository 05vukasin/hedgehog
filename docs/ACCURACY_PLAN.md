# Plan: a tool with 100 % accuracy

How the work is run: [AGENT_PLAYBOOK.md](AGENT_PLAYBOOK.md). What is done, running and next: [STAGES.md](STAGES.md).

Goal (user, 2026-10-23): hedgehog gives an agent everything it needs, exactly right, for every
question a spreadsheet can answer — proven with model-free tests first; an agent run is only the final,
official confirmation.

## What "100 % accurate" means (measurable, no LLM)

| # | Property | Measure | Target |
|---|---|---|---|
| F | **Fidelity** — every value hedgehog hands over equals the source | cell-fidelity gate (eval/fidelity): numbers, dates, text/IDs exact; every cell placed (table, label, total, key/value, note) | 100 % of cells on all corpora |
| S | **Structure** — tables, headers, row labels (full path), totals, units are right | public benchmarks with labels (HiTab, TableSense, DECO, StatSheets, Strudel) | 100 % of fixable misses fixed; the rest audited as label errors |
| L | **Lookup reachability** — a question's answer is a cell under the right row path and column header | gold cell + header path (HiTab), gold value + question words (MiMoTable, RealHiTBench) | 100 % of answerable lookups |
| C | **Computation reachability** — a calculated answer equals a simple query over clean tables | *oracle search*: enumerate small SQL programs (filter on question entities × sum/avg/count/min/max/diff/ratio/share/rank over clean columns) and test whether one yields the gold value | 100 % of answerable calculations |
| M | **Nothing misleading** — no total row in data, no mixed measures/units unflagged, no wrong date/sign/scale | invariants checked per table + the trap checks of eval/info | 0 violations |
| K | **Compact** — catalog stays small | tokens per file, per question | no regressions |

A miss is allowed only when it is **audited** as one of: the benchmark's gold answer is wrong; the file
does not contain the information; the question needs knowledge outside the file; the question is ambiguous
(two reasonable readings give different answers). Every audited miss is listed with its evidence (source
cells + computation) in `eval/accuracy/audit.tsv` (reviewed, never silent); candidates found not auditable
stay misses with their cause in `eval/accuracy/reviewed.tsv`.

## Honest data split

- **dev** (used while fixing): HiTab train + dev, 80 % of MiMoTable and RealHiTBench, TableSense, our cases.
- **holdout** (frozen, never looked at while fixing): HiTab test, a fixed random 20 % of MiMoTable and
  RealHiTBench (seeded, file-level split so no file is in both). Scored only at release time; published.

## Loop

1. `make score` — one model-free scoreboard over F, S, L, C, M, K on dev, with every miss grouped by cause.
2. Agents (≤ 2 at a time) take the largest miss cluster: reproduce as a unit test → fix in core → all gates
   green (make quick, merge-check, fidelity, public suite) → re-score. No regression anywhere, ever.
3. Repeat until dev is 100 % after audit.
4. Score holdout once. If holdout < dev, the gap is new failure classes: add tests for them, fix, re-freeze a
   new holdout for the next release (report both).
5. Official confirmation with an agent (user-approved budget): MiMoTable end to end, SpreadsheetBench.

## Status

- Done: public model-free suite (eval/public); five library fixes (row-label paths 66.7 → 77.1 % on HiTab).
- Running: cell-fidelity gate + fixes (branch `fidelity`), incl. the 9 library causes found in the MiMoTable
  agent run (e.g. "March 25" read as 2025-03-01).
- Done (branch `accuracy`): the measurement side in `eval/accuracy/` - seeded file-level dev/holdout split,
  computation-reachability oracle (validated against HiTab gold operands, a decoy test and a hand-checked
  sample), L with question keys, M invariants, `make score` (dev baseline in eval/accuracy/RESULTS.md),
  audit.tsv seeded with the verified wrong-gold MiMoTable items.
- Done (branch `oracle`): C oracle v2 - shares of a part in its whole (total rows / columns, arithmetic
  wholes), ranges, arg-extremes, adjacent-year changes, complements, tighter pair guards (decoy rates per
  answer type at or below v1); every C miss classified (library / oracle-limit / gold/question) with
  evidence in eval/accuracy/causes.py, source header grounding for generated column names.
- Done (branch `audit`): measurement hygiene - all 190 C audit candidates checked against the source cells
  (30 audited with evidence: gold_wrong 22, ambiguous_question 7, not_in_file 1; 160 reclassified in
  reviewed.tsv as oracle-limit 156 / library 4); clock times are typed values in the scorers; the M checker
  reviewed per class on a hand-checked sample, its false-positive classes fixed (tests in test_invariants.py).
- Next: the fix loop on the largest miss clusters.
