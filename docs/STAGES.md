# Stages

The ledger of work toward [ACCURACY_PLAN.md](ACCURACY_PLAN.md), following [AGENT_PLAYBOOK.md](AGENT_PLAYBOOK.md).
Every stage: goal, branch, merge commit, metrics before → after. Commits of a stage carry the trailer
`Stage: S<n>` (from S10 on; earlier stages are found by their merge commit). Metrics are on the **dev**
split unless noted. Update this file whenever a stage starts, merges or is dropped.

## Current state (master 134d648, 2026-10-24)

| Measure | Value | Target |
|---|---|---|
| F fidelity: values / dates / text changed (MiMoTable · RealHiTBench · HiTab) | 0/0/0 · 0/0/0 · 0/0/0 | 0 |
| F unplaced / misplaced (mimotable · realhitbench · hitab · tablesense · golden · agents) | 0/134 · 0/398 · 0/0 · 0/2,512 · 0/163 · 0/6 (tablesense text_changed 5) | 0 (or documented) |
| L lookups | 5,469 / 5,634 = 97.1 % raw, 97.2 % after audit (≈ 156 misses) | 100 % after audit |
| C calculations | 1,198 / 2,264 proven; library misses 42; oracle-limit ≈ 988; gold/question 2 unreviewed | library misses 0 |
| M misleading-info violations | 1,589 in 767 files (most: columns without header text incl. ≈ 80 new col_<letter> value-block tables) | 0 (or documented policy) |
| K catalog tokens / file | hitab 773, mimotable 972, realhitbench 770 | no growth |
| S HiTab table F1@0.8 | ≈ 99.1 % | — |
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

## Running

Nothing (milestone reached 2026-10-24 ~13:00; no agents, capped jobs or stage worktrees).

## Next (in order)

1. S12 — columns without header text (M ≈ 1,589, mostly "header missing"): policy for source columns that
   truly have no header (generated name + catalog note) vs real lost headers; incl. the col_<letter> tables
   S10 recovered from Notes.
2. S13 — remaining L misses (≈ 156: stacked-block captions "Time = 75 min" 40, not grounded 27, header path 24,
   col_X 13, parent 12 incl. 1535 mid-table header, cell in no table 18) and C library misses (42).
3. S14 — misplaced long tail (≈ 3,200 cells: header rows sent to Notes, data rows read as headers, side-by-side +
   stacked tables like manufacture-table13 / health-table01, forms like en-0549); document what stays.
4. Review the 2 new gold/question candidates (a04c5358, 55e60981) with source evidence.
5. Holdout scoring once (`make score-holdout`); new failure classes → S15…
6. Official agent runs (owner approves budget): MiMoTable end to end, SpreadsheetBench.

## Earlier (before the accuracy plan)

Agent-path work B136/B141–B162 (question-aware catalog, parallel reading, total codes, …) — see
eval/agents/BACKLOG.md and the merge commits ecdd4f6 … 02234a3.
