# Iteration 5: paired interval bands and measured performance work

Baseline: accepted local master `9838da0`, release `3c22b9e970fb`.
This iteration is accepted locally after all required gates pass.

## Accuracy

DEV HiTab StatCan 1521 has three explicit cohort merges in row 3, unit and
confidence-interval parents in row 4, and from/to children in row 5. A lone-stub
section in row 6 precedes numeric records in row 7. Raw source geometry and text
establish these headers; no inferred boundary or rescaling is introduced.

Regression `228d656` (main `a277b06`) fails on baseline: source rows 3/4 are emitted
as data alongside records 6–9 (zero-based). Implementation `b27b772` (main
`e3b3873`) extends the existing interval helper atomically for the two bands under
distinct disjoint cohort ownership. Each cohort must contain exactly one interval;
other labels must be admissible units/scales, and numeric bounds must follow at
most one lone-stub section. The section remains body context. Checked arithmetic
rejects coordinate-limit cases.

Independent source review passes. Three integration tests and four guard tests
pass, including the existing cases, 16 ownership/record variants and coordinate
limits. Both analyzers preserve all 12 numeric columns across four fixture rows,
source header coordinates and section context. Real-file checks pass both analyzers, and all four prior 46/504 checks pass.
The first checker run correctly flagged its own overly narrow group-role assumption:
rules-only output intentionally stores the section as a derived `parent` column.
Independent source/output review proves that representation. The corrected check
requires exact ancestor metadata, source A6 text, its own otherwise-empty row 6
and its row-7 relationship; original failure and checker remain saved.
Full gates pass. The same source-defined pattern also recovers StatCan 2728:
three explicit cohort merges in row 3, unit/CI parents in row 4 and from/to in
row 5, followed immediately by numeric records in row 6. Independent raw-XLSX
checks on the final release verify all 12 column paths/coordinates and unscaled
first-record values under both analyzers. Together these workbooks resolve one
L miss and six repeated-header M violations.

## Unsorted in-memory sheet bounds

A baseline performance fixture exposed an independent accuracy defect:
`Sheet::used_range` initializes its top bound from the first cell but never
minimizes it across later cells. Grid supports unsorted/duplicate cells, so this
can clamp real formula sources out of verification. Regression `dbd3a2c` (main
`b434eac`) reproduces both failures: top 9 instead of 2 and SUM verification
returning NoSource instead of confirming 10 + 20 = 30. Both red runs exit 101.

Reviewed one-line fix `bf36401` (main `4433f95`) adds the missing minimum update.
The bounds test covers 24 permutations, duplicate positions and merge expansion;
formula verification covers unsorted cells and last-duplicate ownership. The
bounds test, all 25 totals-detection tests and the corrected sibling semantic
baseline pass after the fix. Empty-sheet policy is unchanged.

## Performance

Temporary coarse instrumentation `ca92acb` (main `d70d58d`) attributes dominant
NYISO cold-open work. Its first build fails because a timer shadows the existing
`profile` function. Reviewed rename `67f6143` (main `a75e1ec`) corrects that; the
subsequent release build succeeds. Failed and successful logs are both retained.

One accepted and three instrumented cold opens return identical complete MCP
responses. Raw traces and stage aggregates are saved. Dependency parsing, index
marking, region planning, streaming and extraction slots/labels merit investigation.
Timers measure wall time; nested phases and shared-index waits cannot be added.
Instrumentation perturbs timing and is not evidence of a production speed gain.

The earlier handoff's single cached totals-detection assumption was incorrect:
model totals directly compute register mode false, while cached/prefetched totals
compute register mode true. Their shared dependency index does not make the two
detections interchangeable. Source review independently confirms this distinction.

Both temporary commits are completely reverted in `afeac55`/`e13e0d0`. No tracing
changes remain in production. The performance worker and reviewer are evaluating
a nine-line early guard from the measured sibling-support phase. It skips
verification/maps only when no typed vertical candidate can gain support. Separate
48-case tests preserve zero/missing/numeric-text, arithmetic evidence, source
orientation and full candidate state. The first fixture fails on baseline because
it exposes the unsorted-range bug; test-only correction `4f34340` (main `c0c6ee2`)
adds a verified peer premise and consistent above/below source geometry. The
corrected baseline passes. Guard `f91f694` (main `bc8fad9`) is applied and passes the same semantic test.
The accuracy-only release is `b3152e55d724`; combined release `8863194ea465`.
Both six- and twelve-pair comparisons preserve every export and complete MCP response.
Six-pair NYISO normalize improves 2,333.83 → 2,305.10 ms (5/6 wins), cold MCP
1,227.67 → 1,213.31 ms (6/6). Twelve-pair confirmation gives normalize
2,324.55 → 2,302.02 ms and cold MCP 1,239.78 → 1,218.21 ms (both 11/12 wins).
Existing totals-resolution traces drop about 40 ms, matching the guard's intended
skipped sibling work. The guard is retained after full correctness validation.

Small-case tradeoffs are retained explicitly. Twelve-pair financial normalize
51.67 → 51.11 ms, cold MCP 41.19 → 42.04 ms (+2.07%); wide normalize
105.40 → 104.84 ms, cold MCP 45.95 → 46.19 ms. Financial cold open was also
slower in the six-pair run. This is a targeted formula-heavy workbook improvement,
not a general latency gain. No extra optimization, cache or retained state is added.

## Validation and constraints

All required gates pass on final release `8863194ea465`:

- 538 release tests across 59 groups, 17 ignored, zero failures; doctests complete.
- All quick checks, Clippy, 50 identical goldens and both unchanged structure benchmarks.
- Full corpus fidelity; strict comparison of 4,055 per-file records finds no regression.
- Node 21 tests and TypeScript checks; fresh DEV score and strict per-item/public comparison.
- Real feature equivalence: six files, eight sheets, 41,358 segments, zero skips/differences.
- Final source checks: 1521 and 2728 plus prior 46/504 under both analyzers.

DEV L improves 5,496 → 5,497 / 5,634 (128 misses), M 1,560 → 1,554 in
763 files; C stays 1,198 / 2,264 (42 library misses). No new L/C misses or M
violations. Public HiTab col_exact improves 3,598 → 3,623; both_exact
3,325 → 3,344; lenient column/both each +1. No public accuracy metric declines.
HiTab catalogs add 106 total tokens for recovered source labels, mean
773.6585 → 773.7398 (+0.011%). Evaluator/audit/baseline definitions are unchanged.

WorkplaceGeneral's four ambiguous misses remain deferred; mixed-measure StatCan
1377 and source-conflicting 1667 are not forced into misleading numeric labels.
No broad speed claim is made beyond the measured formula-heavy workload.

Heavy jobs run serially under the 4 GiB aggregate cap, with two Cargo jobs and the
memory watchdog active. Source/review workers finish before heavy execution.
Accepted baseline artifacts contain 4,055 fidelity records and the frozen binary
hash; the strict verifier differs only in the accepted baseline tag and passes
independent source review. No evaluator, audit, split, gold or baseline definitions
change. No LLM/API calls, holdout inspection, remote push or installed CLI update.

Evidence: `../scratch-archive/codex-iteration-5/`.
