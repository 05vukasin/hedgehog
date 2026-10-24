# S12 / S13 accuracy batch

Evidence and validation for the bounded header and caption fixes started on 2026-10-24.

## Changes

- **S12, merged parent headers:** RealHiTBench's dev `business-table06.xlsx` has regional
  headings merged over repeated year columns, separated from them by a blank row. Recover
  these explicit parent headings only when multiple distinct merged parents have the same
  complete child-label sequence. Preserve their source coordinates and extend the table's
  source rectangle. Reject unrelated metadata and parent cells already owned by another table.
- **S13, shared parameter captions:** repeated numeric captions such as `Time = 75 min`
  apply to contained child tables. Accept an explicit merged caption span, or infer a span for
  unmerged captions only when each repeated block has the same two or more merged child titles
  immediately below it (matching labels and column geometry). Stop at intervening captions of
  the same parameter and source column, even if their geometry differs. Preserve the original
  text and coordinates as title/notes; show shared context once in the agent catalog with its
  table IDs. Bound the context output and disclose truncation.
  Lateral sections and unrelated formula notes must not inherit the caption.

Both root causes were reproduced by generated failing tests before the fixes. No scorer,
audit, holdout split or accepted quick baseline was changed. Numeric-only question focus
remains limited by the existing question-ranking algorithm; explicit caption-to-table
mapping still lets an agent choose the right tables.

## Speed investigation

The unchanged release baseline normalized the financial sample in 46 ms, the wide World Bank
CSV in 88 ms, and the large Enron NYISO workbook in 2.04 s (three-run medians, this laptop).
Peak resident memory was approximately 18, 24 and 270 MiB respectively. These are local
measurements, not portable performance guarantees. Existing trace timestamps include cumulative
stages; export time must be computed by subtracting the preceding timestamp.

Two candidates were tested and rejected:

- Borrowing table values in the XLSX writer removed text clones but produced no repeatable
  speed gain in alternating baseline/candidate runs. All exports remained identical.
- Caching context connected components did not share work with layout's separate DSU
  implementation and retained additional component vectors. Alternating runs showed no
  reliable latency benefit. The candidate was reverted before the accuracy gates.

Alternating five-run medians for the rejected connected-component cache were:

| Input | Baseline | Candidate | Result |
|---|---:|---:|---|
| Text-heavy synthetic CSV | 1.744 s | 1.738 s | within run variation; median RSS increased about 0.8 MiB |
| Large Enron NYISO XLSX | 2.234 s | 2.263 s | slower |
| Financial sample | 49 ms | 48 ms | no reliable improvement |
| Wide World Bank CSV | 98 ms | 97 ms | no reliable improvement |

All compared exports were identical. Local raw timing records are in
`../scratch-archive/codex-{writer,components}-comparison.json`.

There is **no shipped speed optimization** in this batch. A future unification of the two
connectivity implementations needs proof that merge-only components, ordering and malformed
overlapping merges retain their current semantics, followed by timing and peak-memory checks.

## Resource controls

All heavy commands used `../scripts/run_capped.sh`, one shared job slot, a 4 GiB aggregate
slice cap, no job swap, and two Cargo build jobs. The existing watchdog script was activated
with a 2,000 MiB available-RAM floor and a 50% swap threshold. Separate worktrees were used
for the two accuracy tasks; broad gates run on the combined candidate.

## Source checks

- RealHiTBench dev `business-table06.xlsx`: restored region/year paths, such as
  `North East / 2005`, instead of repeated ambiguous year labels. Catalog size increases from
  2,744 to 3,188 characters (+16.2%) because the missing region names are now included.
- Dev `science-table09.xlsx` and `science-table10.xlsx`: unmerged Time captions now map
  explicitly to every ROI table in their section. Lookup atoms improve from 28/33 to 33/33; complete question checks improve from 3/10 to 8/10.
  Catalog characters increase from 7,468 to 7,595 and 7,641 to 7,819 respectively.
- Independent review caught a skipped boundary when an intervening caption had a different
  width, and an avoidable full-sheet scan per caption. The final rule includes a mixed-width
  regression and restricts the sparse-caption check to its source row.

## Validation

The final release binary is `b147b92202c1` (SHA-256 prefix), from implementation commit
`865faa1`. All metrics below use the development split and unchanged scorers/audits.

| Measure | Before | After |
|---|---:|---:|
| L correct lookup atoms | 5,469 / 5,634 | 5,487 / 5,634 (+18) |
| L after audit | 97.2% | 97.5% |
| C proven computation atoms | 1,198 / 2,264 | 1,198 / 2,264 |
| C library misses | 42 | 42 |
| M violations | 1,589 in 767 files | 1,560 in 763 files |
| K RealHiTBench mean tokens/file | 769.8 | 770.7 (+0.12%) |
| K HiTab / MiMoTable mean tokens/file | 773.3 / 971.9 | unchanged |
| S HiTab table F1@0.8 | 98.85% | unchanged |
| S TableSense table F1@0.8 | 57.57% | unchanged |

The old ledger's approximately 99.1% HiTab score was stale: reproducing the original baseline
already yielded 98.85%. This batch does not change structure-detection scores.

- Focused generated regressions reproduce the original failures and cover both analyzers,
  exact source values/coordinates, owned parent cells, different child labels/geometry,
  lateral sections, mixed-width boundaries, and unrelated formula notes.
- `make quick`: all six parts pass, with accepted baselines unchanged.
- `make merge-check AREA=core`: 505 release tests passed, 17 ignored; workspace Clippy with
  warnings denied; 50 golden outputs identical; both analyzer benchmarks passed; full fidelity
  passed for 4,055 files with unchanged failure counts and no baseline updates.
- `make node-test`: 21 tests and TypeScript type checks passed.
- Exact per-item comparison: no new L/C misses; 18 L misses resolved. No new M violations;
  29 removed. Public DEV reachability improves by four checks in each RealHiTBench mode
  (catalog/cell/loose/strict); other public accuracy metrics are unchanged.
- Per-file comparison across all 4,055 fidelity files: no regression or new reader/normalizer error.

The exact per-item and public DEV comparison evidence is saved locally under
`../scratch-archive/codex-accuracy-batch/`; the original and final full scoreboards are under
`../scratch-archive/accuracy/work/9a6b06b06c23/` and `b147b92202c1/` respectively.
The summary scoreboard is [RESULTS.md](../eval/accuracy/RESULTS.md). Code and results are
integrated into local `master`; no remote push or installed-CLI replacement is part of this batch.
Temporary worker checkouts were removed after validation; detailed raw timing and comparison
artifacts remain in `../scratch-archive/`.

## See also

- [AGENT_PLAYBOOK.md](AGENT_PLAYBOOK.md)
- [development.md](development.md)
