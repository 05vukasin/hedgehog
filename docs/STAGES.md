# Stages

The ledger of work toward [ACCURACY_PLAN.md](ACCURACY_PLAN.md), following [AGENT_PLAYBOOK.md](AGENT_PLAYBOOK.md).
Every stage: goal, branch, merge commit, metrics before → after. Commits of a stage carry the trailer
`Stage: S<n>` (from S10 on; earlier stages are found by their merge commit). Metrics are on the **dev**
split unless noted. Update this file whenever a stage starts, merges or is dropped.

## Current state (iteration 5, implementation bc8fad9, 2026-10-24)

| Measure | Value | Target |
|---|---|---|
| F fidelity: values / dates / text changed (MiMoTable · RealHiTBench · HiTab) | 0/0/0 · 0/0/0 · 0/0/0 | 0 |
| F unplaced / misplaced (mimotable · realhitbench · hitab · tablesense · golden · agents) | 0/134 · 0/398 · 0/0 · 0/2,512 · 0/163 · 0/6 (tablesense text_changed 5) | 0 (or documented) |
| L lookups | 5,497 / 5,634 = 97.6 % raw, 97.7 % after audit (128 misses) | 100 % after audit |
| C calculations | 1,198 / 2,264 proven; library misses 42; oracle-limit ≈ 988; gold/question 2 unreviewed | library misses 0 |
| M misleading-info violations | 1,554 in 763 files (1,358 are columns without header text) | 0 (or documented policy) |
| K catalog tokens / file | hitab 774, mimotable 972, realhitbench 771 | no growth |
| S HiTab table F1@0.8 | 98.9 % (unchanged reproduced baseline; old ledger was stale) | — |
| Installed hhc / hedgehog (~/.hedgehog-cli/bin) | built from master 134d648 on 2026-10-24 12:57 (backup of previous in scratch-archive/backup-hhc-20261024-1257) | — |

## Done

| Stage | Title | Branch → merge | Result |
|---|---|---|---|
| S1 | Public model-free benchmark suite (MiMoTable, RealHiTBench, HiTab, TableSense) | public-bench → e612c71 | eval/public; 0 / 2,691 files fail to open |
| S2 | Five library fixes found by the public suite | fix-public → a2bfcf7 | HiTab row labels exact 66.7 → 77.1 %, column header 91.2 → 93.7 % |
| S3 | Accuracy scoreboard: dev/holdout split, C oracle, L/M checks, audit, `make score` | accuracy → 0ad591d | dev baseline L 88.0 %, C 49.6 % proven |
| S4 | Cell-fidelity gate (`make fidelity`) + 10 reading fixes | fidelity → af67eb6 | changed values/dates on MiMoTable/RealHiTBench → 0 |
| S5 | C oracle v2: wider program space; every C miss classified | oracle → a4f047e | library 62 / oracle-limit 820 / gold 190 |
| S6 | Lost header rows, section-named duplicates, suppression rows | headers → beadb37, fixes cf7d972 | L +74 atoms, M 2,806 → 1,993 |
| S7 | Audit of 190 gold candidates; scorer fixes (clock times, CSV catalog numbers, year labels); M recall | audit → 461a375 | 30 audited with evidence, 160 kept as misses |
| S8 | Full outline paths of row labels; '(4)' footnotes; serial dates under date headers | labels → 08db712 | L 4,994 → 5,240 (93.0 %) |
| S9 | Totals inside data; mixed measures | totals → f3fa706 | M1 73 → 23, M3 310 → 72, M 1,805 → 1,517 |
| S9b | Remaining lookups; F-mark annotation rule; unit/lone-label sections | lookup → 4fd2909 | L 5,266 → 5,442 (96.6 %) |
| S10 | Cell placement: value blocks from Notes → tables, X-flag columns kept, 'N.B.'/'avg 00', checker blind spots closed (flag_lost, value_block_in_notes) | placement → 2e773e1 | tablesense misplaced 98,733 → 2,512, golden 35,850 → 163, unplaced → 0 everywhere |
| S11 | Side-by-side copies of one list split (values were in other states' rows), header labels over numbers, bold sections, scorer fixes | round3 → 7e2cb73 (+134d648 scorer fix) | L 97.2 % after audit; C library family 60 → 42; per-item no regression vs master |
| S12 (bounded slice) | Merged parent headers over repeated child labels, with blank-row and ownership guards | codex/accuracy-batch → cda69df | Region/year paths restored; combined batch M 1,589 → 1,560 |
| S13 (bounded slice) | Shared numeric captions, including unmerged Time sections inferred from repeated merged child titles; raw-source boundary guards | codex/accuracy-batch → 865faa1 | Combined batch L 5,469 → 5,487; real science cases 3/10 → 8/10 questions, 28/33 → 33/33 atoms |

## Completed batch validation

See [the batch evidence](accuracy-batch-2026-10-24.md) and [the scoreboard](../eval/accuracy/RESULTS.md).
Final release SHA-256 prefix: `b147b92202c1`. Baseline/audit/split/scorer definitions unchanged.

- 505 release tests passed, 17 ignored; all quick checks passed; Clippy with warnings denied.
- 50 golden outputs identical; both analyzer benchmarks passed; full fidelity passed across 4,055 files.
- Per-item comparison: 18 L misses resolved, no new L/C misses; 29 M violations removed, none added.
  Public DEV metrics did not decline; no per-file fidelity regression.
- Node: 21 tests passed and TypeScript checks passed.
- K: RealHiTBench mean 769.8 → 770.7 tokens (+0.12%); other corpus means unchanged.
  Source-specific catalog growth is justified by recovered labels/context in the batch report.
- Both speed experiments rejected: no repeatable improvement. No speed optimization shipped.
- Heavy jobs serialized, aggregate cap 4G, Cargo jobs 2, memory watchdog active. No LLM or holdout scoring.

## Iteration 2 accepted

See [iteration evidence](iteration-2.md). Baseline `d1a2386` / release `b147b92202c1`;
final release `541500dbeb36`, alignment implementation `bec3146`.

- Sparse explicit hierarchy recovered; split-label and incidental/centered padding guarded.
  Four L misses resolved, no new L/C misses or M violations. Public coverage preserved;
  HiTab exact row/both linking each +2, lenient row/both each +4.
- 516 release tests passed, 17 ignored; quick, Clippy, 50 Enron goldens, both structure
  benchmarks, Node 21 tests/TypeScript and full 4,055-file fidelity passed.
- Only Census mini-golden hashes changed, after independent raw-source/output proof:
  49 correct ancestors added, all rows/source values/existing ancestors retained.
- Two small optimizations remove duplicate position lookup and repeated whole-row means.
  Final cold MCP open: financial 38.56 → 37.05 ms, NYISO 1,085.31 → 1,072.80 ms;
  exact exports/responses preserved. Wide CSV gain remains inconclusive across runs.
- Final exact per-item/public/fidelity verification passed. Scorer/audit/split/fidelity
  baselines unchanged. No LLM calls, holdout scoring, remote push or installed CLI update.

## Iteration 3 accepted

See [iteration evidence](iteration-3.md). Baseline `225b9da` / release `541500dbeb36`;
final implementation `0e86da4`, release `21ab0c6aed37`.

- Repeated cohort blocks keep their own parent headers and row context, including
  numbers stored as text. Two L misses resolved, no new L/C misses or M violations.
- Raw strict HiTab col_exact 3,597 → 3,595 is retained: exactly two gold paths omit
  the source's "excluding pregnancy" qualifier. Independent source review and a
  separate artifact guard prove those cases. Row exact/lenient linking each +2.
- Only that workbook's catalog changes; HiTab mean tokens +0.026% for recovered
  cohort/header context. Evaluators, audits, splits and baselines are unchanged.
- Scoped distinct storage and the minimal capacity guard preserve exact features,
  exports and MCP responses. Skewed-column Context median 164.04 → 81.10 ms;
  general end-to-end gains are not claimed. Wide workload direction is favorable;
  NYISO timing remains neutral/inconclusive across six- and twelve-pair runs.
- 524 release tests pass (17 ignored), quick, Clippy, 50 goldens, both structure
  benchmarks, 4,055-file fidelity, Node 21 tests/TypeScript, fresh DEV score and
  reviewed per-item/public/fidelity comparison all pass. Real bitwise equivalence:
  41,358 segments/eight sheets, zero differences or skips.
- Device shutdown interrupted only an optional MCP timing extension; its incomplete
  result is excluded. All required correctness and normalization evidence is saved.
- Local integration only; no remote push or installed CLI replacement.

## Iteration 4 accepted

See [iteration evidence](iteration-4.md). Baseline `47be56d` / release `21ab0c6aed37`;
final implementation `aa5a013`, release `3c22b9e970fb`.

- Explicit confidence-interval child bands retain scale/from/to headers and source
  coordinates in HiTab 46/504. Three L misses resolved; no new L/C misses or M violations.
  Invalid merge widths at extreme coordinates are rejected without overflow.
- Strict public/per-item comparison passes: HiTab column/both-path linking each +3;
  no public accuracy declines or per-file fidelity regressions across 4,055 records.
  Mean HiTab catalog growth is 0.0184 tokens/file for recovered source labels.
- 532 release tests (17 ignored), quick checks, Clippy, 50 goldens, both structure
  benchmarks, full fidelity, Node 21 tests/TypeScript and fresh DEV score all pass.
  Real feature equivalence: 41,358 segments/eight sheets, zero differences/skips;
  final real-source checks pass on both analyzers for both workbooks.
- Both performance candidates rejected after six-/twelve-pair comparisons; production
  changes reverted, semantic regressions retained. Final accepted/candidate timing
  confirms small mixed differences, without a repeatable speed gain or broad slowdown.
- Local integration only; no remote push, installed CLI replacement, LLM or holdout use.

## Iteration 5 accepted

See [iteration evidence](iteration-5.md). Baseline `9838da0` / release `3c22b9e970fb`;
final implementation `bc8fad9` with range fix `4433f95`, release `8863194ea465`.

- Explicit cohort/interval bands recovered in StatCan 1521/2728: one L miss
  resolved, six repeated-header M violations removed, no new L/C/M failures.
  Source values, coordinates and first records verified under both analyzers.
- Unsorted in-memory sheet bounds now include the minimum cell row; permutation,
  duplicate and actual formula-verification regressions reproduce and verify the fix.
- Nine-line sibling-support guard avoids unused formula verification/indexing.
  Exact exports/MCP preserved; twelve-pair NYISO normalize/cold-open improve
  about 1.0%/1.7% (11/12 wins each). Financial cold open is 0.85 ms slower;
  small-workload results are mixed. This is a targeted gain, not general acceleration.
- 538 release tests (17 ignored), quick, Clippy, 50 goldens, both structure benchmarks,
  full fidelity, Node 21 tests/TypeScript, fresh DEV and strict regression gates pass.
  Per-file fidelity: 4,055 records, no regressions. Real feature equivalence:
  41,358 segments/eight sheets, zero differences/skips. Final source checks pass.
- Public HiTab exact column linking +25, exact both +19, lenient column/both +1.
  Catalog growth +106 total tokens (+0.011% mean) retains recovered source labels.
- Temporary instrumentation fully removed. Local integration only; no remote push,
  installed CLI replacement, evaluator changes, LLM calls or holdout use.

## Running

Iteration 6 baseline: local master `16b7d55`, release `8863194ea465`.

- Lead `codex/iteration-6`; accuracy `codex/accuracy-iter6` in `../hh-accuracy-iter2`;
  performance `codex/performance-iter6` in `../hh-performance-iter2`; independent reviewer.
- Accuracy: discover remaining source-backed DEV lost-header/label or library-contract
  issues; preserve ambiguous boundaries and source-conflicting gold as unresolved.
- Performance: one candidate from measured remaining dominant scopes; no speculative
  caches or unmeasured cleanup. Regression and implementation commits remain separate.
- Before expansion: 10 GiB RAM available, 4.3 MiB swap used, 121 GiB free disk,
  btrfs metadata 17.37/18.50 GiB; three free worker slots and no heavy jobs.
  All source/review workers finish before lead-only serialized capped testing.

## Next (in order)

1. S12 remainder — columns without header text (1,358 of 1,554 M violations): policy for source columns that
   truly have no header (generated name + catalog note) vs real lost headers; incl. the col_<letter> tables
   S10 recovered from Notes.
2. S13 remainder — 128 audited L misses: largest raw clusters are question keys outside tables (28),
   numbers not grounded by labels (27), header paths (24), col_X (13), and missing parent paths (12).
   Numeric-only question ranking still needs work. C library misses remain 42.
3. S14 — misplaced long tail (≈ 3,200 cells: header rows sent to Notes, data rows read as headers, side-by-side +
   stacked tables like manufacture-table13 / health-table01, forms like en-0549); document what stays.
4. Review the 2 new gold/question candidates (a04c5358, 55e60981) with source evidence.
5. Holdout scoring once (`make score-holdout`); new failure classes → S15…
6. Official agent runs (owner approves budget): MiMoTable end to end, SpreadsheetBench.

## Earlier (before the accuracy plan)

Agent-path work B136/B141–B162 (question-aware catalog, parallel reading, total codes, …) — see
eval/agents/BACKLOG.md and the merge commits ecdd4f6 … 02234a3.
