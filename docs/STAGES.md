# Stages

The ledger of work toward [ACCURACY_PLAN.md](ACCURACY_PLAN.md), following [AGENT_PLAYBOOK.md](AGENT_PLAYBOOK.md).
Every stage: goal, branch, merge commit, metrics before → after. Commits of a stage carry the trailer
`Stage: S<n>` (from S10 on; earlier stages are found by their merge commit). Metrics are on the **dev**
split unless noted. Update this file whenever a stage starts, merges or is dropped.

## Current state (master 4fd2909, 2026-10-24)

| Measure | Value | Target |
|---|---|---|
| F fidelity: values / dates / text changed (MiMoTable, RealHiTBench, HiTab) | 0/0/0 · 0/0/0 · 0/0/1 | 0 |
| F unplaced / misplaced (all corpora) | 1,913 / ~1,711 on master; S10 brings unplaced to ~9 | 0 (or documented) |
| L lookups | 5,442 / 5,634 = 96.6 % (96.7 % after audit) | 100 % after audit |
| C calculations | 1,198 proven; real library misses ≈ 58 (rest oracle limits / audited gold) | library misses 0 |
| M misleading-info violations | 1,515 (≈ 800 columns without header text) | 0 (or documented policy) |
| K catalog tokens / file | hitab ≈ 773, mimotable ≈ 966, realhitbench ≈ 770 | no growth |
| S HiTab table F1@0.8 | 99.1 % | — |

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

## Running

| Stage | Title | Branch / worktree | Status |
|---|---|---|---|
| S10 | Cell placement (Enron long tail), X-flag columns, small headerless blocks, 'N.B.' / 'avg 00' | placement / ../hh-placement | fixes committed (…b36f312); gates on merged master running |
| S11 | Side-by-side copies (values in wrong rows), lookups, C library misses, scorer fixes | round3 / ../hh-round3 | commits …d1d6931; side-by-side fix + gates in progress |

## Next (in order)

1. Merge S10, then S11 (gates between).
2. S12 — columns without header text (≈ 800 M): policy (generated name + catalog note when the source has
   no header text) + real lost headers.
3. S13 — remaining L misses (captions of stacked blocks, header paths, grounding) and C library misses.
4. S14 — misplaced long tail (side-by-side tables merged, forms, helper rows); document what stays.
5. Holdout scoring once (`make score-holdout`); new failure classes → S15…
6. Official agent runs (owner approves budget): MiMoTable end to end, SpreadsheetBench.

## Earlier (before the accuracy plan)

Agent-path work B136/B141–B162 (question-aware catalog, parallel reading, total codes, …) — see
eval/agents/BACKLOG.md and the merge commits ecdd4f6 … 02234a3.
