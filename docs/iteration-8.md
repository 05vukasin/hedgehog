# Iteration 8: independently owned stacked ranking tables

Baseline: accepted master `c6638e7`, release `d119f31186d5`.
Accepted release: `33bf81151fab`. Full validation and final independent review
pass. Integration is local; the iterative accuracy/performance work continues.

## Accuracy

RealHiTBench health-table01 contains four metric captions merged across A:D
at rows 5, 14, 22 and 31. Each section holds independent Highest and Least
state/measure lists with five records. The accepted binary returns four tables,
mixes section headers into earlier tables and loses copy/metric ownership.
The correction returns eight tables, preserving all 40 native numeric values,
including zeros and the literal numeric value 99.96.

The splitter partitions at actual full-width merged captions only when every
piece proves the same independent horizontal copy ranges, distinct rank labels,
complete ordered leaf headers and contiguous point-cell records. Failed new
plans fall back to the established copy splitter. Legacy cell ownership and
body handling remain unchanged. Only validated direct children receive the
shared metric caption and a transient private proof flag. The existing table
emitter retains metric, rank and leaf as three header rows. No public API,
persistent field or cache is added.

The whole-layout fixture fails before correction and passes under both
analyzers. Additional regressions cover shared titles, units and footnotes,
unranked blocks, crossing body merges, header geometry and false provenance.
An unequal later header case requires exact accepted fallback geometry and
source ownership, with every source cell retained once.

The first candidate regressed NSF 163, manufacture-table13 and one TableSense
workbook, Raptor Asset List. Explicitly scoping the new behavior resolves those
regressions. Their complete CSVs and manifests now match the accepted binary
under both analyzers. A reviewed diagnostic copy preserves every gold coordinate
and reports per-file scores: all 1,258 readable TableSense files match accepted
F1 at .5, .8 and 1.0 IoU. Original evaluator and gold files are unchanged.

## Performance investigation

Temporary parse/dependency instrumentation identified 18.4 million NYISO
reference visits, 17.8 million already marked. Instrumentation was removed
before production validation. A bounded interval experiment preserved marking
semantics, budget order, self exclusions and bit upgrades, but six alternating
pairs established no useful end-to-end gain. NYISO normalize/MCP medians were
2212.56/1151.54 ms before and 2227.29/1151.59 ms after; wide CSV normalization
increased 2.22%, with mixed RSS. Production was reverted in `b96c4c0`; two
full-mark regression tests remain. No speed improvement is claimed.

Final six-pair comparisons against the accepted release preserve every normalize
export and complete MCP response. Median latency changes range from -1.59% to
+0.39%; NYISO normalize/MCP change -0.13%/-0.39%. NYISO MCP median RSS rises
2,976 KiB (about 2.9 MiB); normalize falls 2,014 KiB. Other RSS changes are small.
These measurements establish no general speed gain.

## Validation

All source/review workers finish before heavy jobs. Jobs run globally serialized
under a 4 GiB aggregate cap with two Cargo jobs and the memory guard.

- Quick: 19 snapshots, 22 mini-goldens, 21 noise families under both analyzers,
  94 info and 94 question checks, 78 public lookups and a 27-file fidelity slice.
- Release: 551 tests across 62 groups, zero failures, 17 ignored; doctests complete.
- Clippy with warnings denied, 50 identical golden files, both structure gates.
- Full six-corpus fidelity with accepted counts unchanged; strict per-file
  comparison covers 4,055 files with zero regressions.
- Node: 21 tests and TypeScript checking pass.
- Fresh DEV and strict comparison: no new L/C misses or M violations. L remains
  5498/5634; C remains 1198/2264. M falls 1548 to 1542 in 760 files; all six
  removals concern Health01. Public strict/loose reach rises 492/857 to 493/858.
- RealHiTBench catalogs gain 222 tokens from independently owned Health01 tables.
- Final feature equivalence: six files, eight sheets, 41,358 segments, zero
  differences or skipped sheets. All current and prior raw-source checks pass.
- Final CLI hash matches the frozen candidate release.

No evaluator, audit, gold, split or baseline definitions change. No holdout reads,
LLM/API calls, remote push or installed CLI update.
Evidence: `../scratch-archive/codex-iteration-8/`.
