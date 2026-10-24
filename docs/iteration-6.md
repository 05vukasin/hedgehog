# Iteration 6: unmerged price parents and row scratch storage

Baseline: accepted local master `16b7d55`, release `8863194ea465`.
Accepted after all gates and the final timing confirmation. Production head: `e7a34a4`.

## Accuracy

DEV RealHiTBench environment-table09 has no merged cells. Source B4 and D4 name
Contemporary prices and 2009 prices above identical complete B:C and D:E child
header blocks in row 6. Row 5 is blank. F4 independently names the percentage
column, whose row-6 header is empty; A6 names Year. Numeric records start at row 7.

Regression `aeb73db` (main `2254638`) fails on baseline because the price parent
is absent. Existing-helper extension `af9a399` (main `1c7a6b8`) recognizes distinct,
aligned, complete repeated text blocks with ownership, gap and metadata guards.
It introduces no fake merges, retained state, API or additional helper.

Its first green run fails: a later centered-header heuristic attaches the recovered
price parent to Year. The unchanged regression exposes that error. Follow-up
`b496ea3` (main `c477519`) preserves the recovered parent-row result and skips
centering for that row; validated rightward filling follows the source-caption
boundaries. Existing explicit merged behavior already skips that pass.

Independent source review passes both commits. Six focused tests pass: the new
both-analyzer fixture, merged ownership guard, 16 unmerged boundary cases, and
three prior interval-header integrations. The fixture preserves exact names,
coordinates and values, including separate Year and F columns. The strengthened
real-source checker passes on accuracy-only release `7de31e0ebc5e` under both
analyzers, with exact Year/price/percentage names, provenance and unchanged first
record. All prior 1521/2728/46/504 real-source checks pass too. Full DEV metrics and final comparison gates remain pending.

## Performance

Regression `9bdfda6` (main `3d27bbf`) covers scratch reset across sparse, skipped,
uncached-formula, error and merged-label rows. It passes on baseline behavior.
Candidate `dde977d` reuses one local row vector within table extraction: resize
at the existing allocation point, drain into output columns, explicitly drop at
the end of the body loop. Early continues precede initialization, so rows cannot
retain stale slot values. Empty output remains allocation-free. No persistent
cache, field, helper or public API is added.

Source review passes. The earlier profile measured slots/labels at about 124 ms
on NYISO, but the share due to this allocation is unknown. Production is applied as `fdb0d44`, and the same semantic regression passes.
The combined release `220529f4341e` passes its real-source check. Six- and
twelve-pair comparisons preserve every export and complete MCP response, but do
not establish a useful repeatable gain. Six-pair financial normalize improves
58.60 → 57.32 ms, then confirmation worsens 57.76 → 59.14 ms. Twelve-pair NYISO
normalize 2,777.02 → 2,772.47 ms (8/12 wins), MCP 1,500.80 → 1,495.64 ms (9/12);
wide normalize worsens 115.27 → 117.51 ms (2/12 wins). Other small cold-open
results are mixed across runs. NYISO confirmation median RSS increases about
5 MiB. Production is reverted in `e7a34a4`; the semantic regression remains.
No speed improvement ships from this experiment.
Local scratch capacity lasts until the table body loop ends and may affect peak
memory; exact outputs and RSS must be checked alongside latency.

## Validation

All source/review workers finish before heavy execution. Jobs are serialized under
the 4 GiB aggregate cap with two Cargo jobs and the memory watchdog active.
The strict baseline comparison artifacts are independently reviewed: accepted
binary hash, 4,055 fidelity records, and verifier baseline-tag change only.

Full required validation is now running serially after the performance decision.
541 release tests across 60 groups (17 ignored, zero failures), quick checks and
Clippy pass. All 50 goldens are identical, both structure benchmarks are unchanged,
and full six-corpus fidelity passes with accepted baseline counts. All 21 Node
tests and TypeScript checking pass. Fresh DEV score and strict comparisons pass: L 5,497 → 5,498/5,634
(128 → 127 audited misses), C remains 1,198/2,264, and M 1,554 → 1,552
in 762 files. Lookup F540 now finds “2009 prices”; the duplicate All goods
header and missing percentage header violations disappear. No new L/C misses,
no added M, no public accuracy decline and no per-file fidelity regression
across all 4,055 files. Prior science cases remain 8/10 questions and 33/33 atoms.
Real feature equivalence passes on six files, eight sheets and 41,358 segments
with zero skipped sheets or differences. Final current/prior real-source checks
pass under both analyzers. Final CLI SHA-256 is
`7de31e0ebc5ed848df1a7d6d4ceebe82c39bf7a60dd21e0ad1ec3f5db1e031e9`.

Catalog cost rises by 167 characters / 30 tokens across RealHiTBench DEV
(mean 770.998 → 771.054 tokens/file), reflecting the recovered source captions.
Historical public-run timings are slower across all corpora; these are not
paired measurements. A final alternating comparison against the accepted
iteration-5 binary is required to assess current runtime overhead. No evaluator, audit,
split, gold or baseline definitions change. No LLM/API calls, holdout inspection,
remote push or installed CLI update.

Evidence: `../scratch-archive/codex-iteration-6/`.

The final six-pair accepted-baseline comparison preserves all export/MCP bytes.
Financial normalize 59.74 → 59.71 ms; NYISO normalize 2,822.16 → 2,837.45 ms,
MCP 1,527.48 → 1,538.02 ms; wide normalize 121.47 → 123.65 ms, MCP
50.53 → 52.64 ms. RSS is essentially unchanged. A twelve-pair confirmation
is running to determine whether the small latency increases repeat.

Twelve-pair confirmation: financial normalize 58.73 → 58.94 ms, MCP
47.77 → 48.17 ms; NYISO normalize 2,764.20 → 2,790.91 ms (+0.97%,
4/12 wins), MCP 1,493.28 → 1,496.43 ms; wide normalize
115.66 → 116.14 ms, MCP 49.98 → 49.75 ms. The larger six-pair
wide-file changes do not repeat. Memory remains broadly unchanged and every
export/MCP byte matches. Accept the source-backed accuracy gain with this
small documented latency cost; no speed improvement is claimed.
