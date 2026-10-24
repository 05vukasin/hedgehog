# Iteration 4: interval child headers and critical-path performance

Baseline: local master `47be56d`, release `21ab0c6aed37`. This iteration is accepted locally. Source and independent review completed before every heavy run.

## Accuracy

DEV HiTab `clean/statcan/46.xlsx` and `504.xlsx` have explicit two-column merged
confidence-interval parents immediately above `from`/`to` child labels. The same
child band contains `'000` scale text under Number. Previously the band was
excluded from physical header rows: bounds received duplicate parent-only names,
and scale/child header source coordinates were absent. Source titles also name
the from/to units. These are source-defined headers, not inferred table boundaries.

Regression `5952956` (main `b7a9137`) fails on baseline semantics with the intended
missing `Number / '000` assertion; the ordinary mixed-record negative passes.
Implementation `f07cba8` (main `6aca97a`) extends one immediate band only when exact
merged-parent geometry, blank stub, admissible text units/scale, numeric following
bounds and unowned child cells establish ownership. No rescaling path is added.
It does not attempt broader two-band recovery in StatCan 1521.

Final review found that the width predicate could overflow on a single-point
merge at `u32::MAX`. The full queue was canceled before accepting its results;
regression `62732f6` reproduces the debug panic. Reviewed one-line fix `aa5a013`
uses checked subtraction, preserving valid two-column merges and rejecting invalid
widths. Both direct guard tests pass. The interrupted release-test run is not
counted as passed; the subsequent complete run passes all required gates.

The direct guard test and both integration tests pass; all 26 layout tests also
pass. Fixtures exercise both analyzers, two independent merged groups, exact
values across every numeric column/row, first-row retention and parent/child source
coordinates. Real-file checks independently parse XLSX merges/text and verify
normalized header provenance and unchanged first-record values under both analyzers;
all four executions pass on accuracy-only release `f24c8043ed6f`. Source CI
merges and parent/child/cohort coordinates match, with first-row values unchanged.

WorkplaceGeneral(45) remains deferred with four misses: unlabeled date-column
ownership is ambiguous, and merely renaming left headers would preserve unrelated
project associations and total-row contamination. StatCan 1377 legitimately mixes
counts, mean(SD) and ranges, so its homogeneous text typing is not changed just to
satisfy comma-number lookup checks. Source evidence remains in the scratch report.

## Performance

The duplicate row-classification candidate (`2001064`/`d5d6360`) passed three
baseline/optimized semantic tests and exact exports/MCP comparisons. Six- and
twelve-pair measurements did not establish a repeatable latency gain: twelve-pair
NYISO normalize 2,279.65 → 2,271.04 ms (7/12 wins), cold MCP 1,212.84 → 1,205.44 ms
(6/12), wide cold MCP 45.21 → 45.46 ms (5/12). Production was restored in `ee9954a`;
semantic regressions are retained. No speed claim is made for this experiment.

Alternative `4fe9112` moves the existing right-edge header predicate before
comment-column scans. A headed edge already prevents comment acceptance; skipping
three read-only body scans and a temporary pointer vector preserves output. The
unheaded path is unchanged. No helper, cache, API, retained field or allocation is
added. Its explicit/merged/absent-header regression passes on baseline behavior.
Independent source review and all 26 layout tests pass. Six-pair isolated timing
against accuracy-only `f24c8043ed6f` preserves every export/MCP response and all
four real-source checks. NYISO normalize improves 2,282.84 → 2,271.17 ms (6/6 wins),
but financial/wide cases worsen. Twelve-pair confirmation gives NYISO normalize
2,285.75 → 2,281.12 ms (4/12 wins), cold MCP 1,212.73 → 1,213.73 ms (4/12), and
financial normalize 49.12 → 49.84 ms (2/12). No useful repeatable overall gain is
established; production is restored in `670139c`. Both performance experiments
are rejected, and their semantic regressions remain. No speed improvement ships
from this iteration on the available evidence.

## Validation

Accuracy-only real-source checks and saved binary are complete. Both performance
measurement decisions are complete; production now contains only the accuracy fix.
All required correctness gates pass on final release `3c22b9e970fb`:
532 release tests across 59 groups (17 ignored), quick checks, Clippy, 50 identical
goldens, both structure benchmarks, full corpus fidelity, Node 21 tests/TypeScript,
fresh DEV score and strict per-item/public/fidelity comparison. Real feature
equivalence covers six files, eight sheets and 41,358 segments with zero skips or
differences. Final real-source header checks pass all four executions.

DEV L improves 5,493 → 5,496 / 5,634: exactly the three scale-header lookups in
46/504 resolve, with no new misses. C stays 1,198 / 2,264 (42 library misses),
M stays 1,560 in 763 files. HiTab exact/lenient column and both-path linking each
improve by three; no public accuracy metric declines. All 4,055 per-file fidelity
records have no regression. HiTab mean catalog tokens 773.6401 → 773.6585
(+24 total tokens) retain the recovered source labels. Fresh corpus costs are
slower than the older baseline run. Alternating six- and twelve-pair comparisons
of accepted/candidate binaries do not reproduce that broad slowdown; outputs and
complete MCP responses are exact. Twelve-pair medians (before → after, ms):
financial normalize 50.78 → 50.57, MCP 41.52 → 42.10; NYISO normalize
2,338.83 → 2,330.82, MCP 1,224.38 → 1,226.58; wide normalize 103.30 → 104.06,
MCP 45.12 → 45.13. Differences are small and mixed, not a speed improvement claim.
Both favorable and unfavorable measurements remain in the artifacts.

All heavy jobs are serialized through `run_capped.sh`, one slot, aggregate 4 GiB,
two Cargo jobs, with the memory watchdog active. No evaluator, audit, split, gold
or baseline definitions changed; no LLM/API calls, holdout scoring, remote push or
installed CLI replacement occurred. Evidence: `../scratch-archive/codex-iteration-4/`.
