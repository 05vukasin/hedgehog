# Iteration 7: triangular matrix axes and statistical annotations

Baseline: accepted local master `897d06b`, release `7de31e0ebc5e`.
Accepted after all required gates and final timing checks. Production head: `2fee835`.

## Accuracy

DEV RealHiTBench environment-table03 contains two lower-triangular matrices.
Merged B1:F1 and B10:F10 name the measures; B7:F7 and B16:F16 name their
column axes below the data. Each five-row staircase has shifted, matching
row labels. Baseline output retains the footer as a sixth record and repeats
the measure caption across the numeric columns.

The fix recovers those footer labels as header parts with their original source
coordinates, excluding the footer from data while retaining its physical ownership.
It requires complete staircase occupancy, unique matching axes, a merged measure
title, a blank footer stub, and no body formulas, merges or ownership conflicts.
Fourteen guard cases reject invalid shapes. Physical header height stays one.
The private matrix helper is needed to keep the ownership checks together;
no cache, public API or retained field is added.

The exact fixture fails on baseline (exit 101) because source rows include the
footer. Initial implementation then exposes existing numeric inference: two native
numbers plus unrecognized N.S. become text. The additional correction recognizes
exact trimmed N.S. through existing numeric-annotation handling. Native numbers
remain numeric and the marker stays literal text. General numeric thresholds,
leading-zero safeguards and missing-value rules do not change. The warning
mentions non-significance only when N.S. is present.

Independent review also caught footer row accounting: the nonblank footer must
count as physically seen to avoid reporting it as BlankRowsRemoved. A separate
assertion covers this. The original matrix fixture is unchanged throughout.

Commits: matrix regression `5f45fad` → `b3793bb`; additive accounting assertion
`0fa511e` → `915aa26`; runtime `a8c3001` → `fbb128f`; accounting follow-up
`8bfe7af` → `1e611ca`; typing regression `2b823a0` → `86b33fd`; typing fix
`daa3d9a` → `ef8a1b5`. All final source reviews pass.

Seven focused tests pass: matrix integration under both analyzers, the 14-case
guard, three interval tests, prior unmerged-parent integration, and statistical
annotation typing with four negative cases. The dedicated typing regression
also fails before its fix (exit 101, Text instead of Number).

Accuracy-only release SHA-256:
`d119f31186d5bf347975a2aea1faedede99b7e66f64149ab93b8e19608e5cc26`.
Its real-source checker passes under both analyzers: all 28 numeric cells,
numeric dtypes, two N.S. entries, blank triangle cells, row labels, source rows,
measure captions and footer coordinates. All prior 1521/2728/46/504/environment09
real-source checks pass too. The checker was independently reviewed against raw XML.

## Performance

Candidate `6fafed8` (main `38e5e70`) removes a temporary Vec<Term> in formula
classification, retaining the split list and full token scan. No helper, retained
state, cache or API is added. Regression `9cc8000` (main `1eec367`) asserts six
complete ParsedFormula results, including references/functions/errors after an
unsupported term. All six formula tests pass before and after the implementation;
independent source review confirms ordered, pure classification semantics.

The prior dependency-parse profile measured 77.7 ms on NYISO, but this allocation's
share was unknown. Six paired measurements compare the same accuracy changes in
both binaries. Combined release `4899c2dc8e32` preserves every export byte
(XLSX members decompressed) and complete MCP response and passes the matrix check.

| Workload | Mode | Before ms | After ms | After wins |
|---|---|---:|---:|---:|
| Financial | Normalize | 58.20 | 60.45 | 1/6 |
| Financial | MCP | 46.52 | 46.43 | 4/6 |
| NYISO | Normalize | 2,776.77 | 2,774.31 | 5/6 |
| NYISO | MCP | 1,497.28 | 1,520.33 | 1/6 |
| Wide CSV | Normalize | 117.17 | 115.35 | 5/6 |
| Wide CSV | MCP | 49.99 | 50.26 | 3/6 |

Memory varies across runs; there is no useful target latency gain, and results
include regressions. Production is reverted in `2fee835`; the semantic regression
remains. No speed improvement ships from this experiment.

## Validation

All source/review workers complete before heavy checks. Follow-up failures return
to source review before testing resumes. Jobs are globally serialized under a
4 GiB aggregate cap with two Cargo jobs and the memory guard active.
Independent review passes the accepted baseline artifacts: frozen binary hash,
4,055 fidelity records and strict verifier with only its baseline tag changed.

All quick checks pass: 19 snapshots, 22 mini-goldens, 21 noise families under
both analyzers, 94 info and 94 question checks, 78 public lookups, and 27-file
fidelity slice. Full release suite passes: 545 tests across 61 groups, 17 ignored, zero failures;
Doc-test completion confirmed. Clippy with warnings denied passes. All 50 golden outputs are identical and
both structure benchmark gates pass. Full six-corpus fidelity passes with unchanged accepted counts. All 21 Node tests and TypeScript checks pass.
Fresh DEV score and strict per-item comparisons pass. L remains 5,498/5,634
(127 audited misses), C remains 1,198/2,264 (41 library misses), and M falls
1,552 → 1,548 in 761 files. All four removed violations are duplicate p-value
headers in environment-table03 T2. No new L/C misses, no added M, no public
accuracy decline and no per-file fidelity regression across all 4,055 files.
RealHiTBench clean operation reach increases 521 → 523; other non-cost public
metrics remain unchanged. Catalog cost decreases by 72 characters / 16 tokens
in total (mean 771.054 → 771.024 tokens/file). Final real-file/source checks pass under both analyzers. Real feature
equivalence passes six files, eight sheets and 41,358 segments with zero
skipped sheets or differences. Final binary hash matches the accuracy-only
release above. Final six-pair comparison against accepted iteration 6 preserves every export
and complete MCP response. Financial normalize 47.95 → 47.88 ms, MCP
40.45 → 40.08 ms; NYISO normalize 2,187.23 → 2,189.17 ms, MCP
1,145.63 → 1,142.70 ms; wide normalize 99.51 → 99.50 ms, MCP
43.06 → 43.39 ms. All median changes are within 1%, and median RSS does
not increase. This verifies no material sampled overhead; no speed gain is claimed.

No evaluator, audit, gold, split or baseline definitions change. No holdout
inspection, LLM/API calls, remote push or installed CLI update.
Evidence: `../scratch-archive/codex-iteration-7/`.
