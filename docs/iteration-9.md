# Iteration 9: preserve child headers and investigate XLSX registration

Baseline: accepted master `b9d2bb9`, release `33bf81151fab`.
Accepted release: `53fbb72abdae`. All validation gates, independent reviews and
final paired comparisons pass. Integration is local; iterative work continues.

## Accuracy

StatCan 550 has a 402-character merged parent at B3:K3, ten province labels
at B4:K4 and a shared percent unit at B5:K5. Prefix-only truncation at 255
characters removes every province/unit label and leaves duplicate column names.
The correction uses actual source header parts to retain the complete descendant
path when it fits, shortening the ancestor with an explicit ellipsis. A literal
slash within a source cell is not interpreted as a hierarchy boundary. Full
source text and coordinates remain in original_header; no layout, typing or
value ownership changes.

The whole-layout regression fails before correction (exit 101). Two focused
integration tests pass afterward under both analyzers, covering ten provinces,
all 40 values, exact header provenance, Unicode, literal slashes, short headers,
long flat headers and oversized descendants. The raw-source checker passes on
release `549af4ea758c`, with all earlier raw-source header checks also passing.
All integration gates and independent evidence reviews pass.

StatCan 632 remains a separate real segmentation defect: an age/SD text row is
misread as a later table header. Its mixed statistical strings require a complete
layout/ownership solution; it is not treated as a simple parent-label repair.

## Performance investigation

Coarse writer profiling used one reference plus three instrumented observations
under each normalize/MCP mode, with exact exports and responses. NYISO normalize
medians attribute 864.6 ms to XLSX output, including 460.4 ms table population and
398.5 ms saving. Grid setup/body aggregates are nested within population, not
additional wall time. CSV takes 103.1 ms and manifest construction 134.3 ms.
MCP does not enter this writer path. These are attribution measurements.

A second reviewed diagnostic isolates aggregate table registration: three
observations of 292.6, 295.3 and 272.2 ms, median 292.6 ms. All eight normalize/MCP
outputs match the frozen accuracy-only release. Both temporary diagnostics are
fully reverted before candidate work.

The pinned writer dependency builds a per-cell overlap map even for the first
and only table on a fresh worksheet. A controlled local dependency patch defers first-table occupancy as one private
rectangle/index pair. Later additions materialize the original map before the
unchanged overlap loop. Independent source review and four public-API regressions
pass before and after, including original failed-call state and error ordering.
The pristine 76 source files match the pinned package; licenses, features and
transitive versions are retained. The roughly 4 MB vendor footprint is explicit.

Six alternating pairs preserve every normalize export and complete MCP response.
NYISO normalization falls 2209.91 to 1947.37 ms (-11.88%, six of six wins); wide
CSV falls 98.67 to 95.43 ms (-3.28%, six of six). Financial normalization changes
-0.35%. MCP changes are mixed (+1.34% financial, +0.28% NYISO, -0.65% wide), so
no MCP acceleration is claimed. NYISO normalize/MCP median RSS rises 346/1524 KiB;
financial normalize rises 532 KiB and other changes are small. The candidate is
accepted after full integration gates and independent reviews pass.

Final six-pair comparison against accepted iteration 8 preserves all exports and
MCP responses. NYISO normalization falls 2155.29 to 1895.61 ms (-12.05%); wide CSV
falls 98.54 to 95.56 ms (-3.03%), each winning six of six pairs. Financial
normalization changes -2.15%. MCP changes remain mixed (-0.85% financial, +0.30%
NYISO, -0.33% wide). NYISO normalization median RSS rises 3,596 KiB (about 3.5 MiB),
while MCP falls 1,942 KiB. No general MCP or memory reduction is claimed.

## Full validation

- Quick checks pass: 19 snapshots, 22 mini-goldens, 21 noise families under both
  analyzers, 94 info and 94 question checks, 78 public lookups and 27-file fidelity.
- Release: 557 tests in 64 groups, zero failures, 17 ignored; doctests completed.
- Clippy with warnings denied, 50 identical goldens, both structure benchmark gates.
- Six-corpus fidelity and strict 4,055-file comparison: no new regressions.
- All 21 Node tests and TypeScript checking pass.
- Fresh DEV/strict comparison: no new L/C misses or M violations. L remains
  5498/5634 and C 1198/2264; M falls 1542 to 1539 in 758 files. Removed cases
  concern StatCan 550, StatCan 597 and society-table45. HiTab lenient column and
  both-axis linking each improve by two.
- Final feature equivalence: six files, eight sheets, 41,358 segments, zero
  differences or skipped sheets. All seven final raw-source checkers pass.
- Final CLI hash matches frozen combined release `53fbb72abdae`.

## Execution controls

Workers finish source work before heavy commands. Builds, tests and benchmarks
run globally serialized through run_capped.sh under a 4 GiB aggregate cap and
with two Cargo jobs. RAM, swap, free disk and btrfs metadata are checked before
worker expansion. Strict verifiers retain the accepted baseline and all previous
source checks. No evaluator, audit, gold or split definitions change; no holdout
reads, remote push, installed CLI update or LLM/API calls.

Evidence: `../scratch-archive/codex-iteration-9/`.
