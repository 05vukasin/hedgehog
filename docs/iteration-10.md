# Iteration 10: preserve statistical records and reduce manifest allocations

Baseline: accepted master `aec8adf`, release `53fbb72abdae`.
Accepted release: `fccdb74be45e`. All validation gates, paired timing
comparisons and three independent final reviews pass. Local integration only;
iterative accuracy and performance work continues.

## Accuracy

StatCan 632 has Men/Women merged headers with Number/Percent leaves. Its age
and community-score records include literal mean/SD text, sometimes without a
closing parenthesis. Previously the age row started a second table and replaced
the cohort headers. Seven lines in the existing coarse numeric-layout predicate
recognize bounded finite mean/SD strings as measurement evidence. Values remain
text; numeric parsing, typing and SQL contracts are unchanged.

The complete fixture exposed a second defect: `Average 1991 CWB score`, alone
beside an empty merged measurement band, was retained as a total record. The
existing section detector now recognizes a full one-row empty measurement merge
followed by two physically consecutive labeled records with values. This supplies
section evidence even when its first record is named Total. Partial/vertical or
filled merges, formulas without cached values, gaps, missing labels and a single
Total child do not qualify. There is no new helper or public API.

Both failures were observed before their corrections. The final fixture passes
both analyzers: one table, two header rows, 42 records, 13 sections, complete
header provenance, exact count/SD text and numeric percentages. Group ownership
uses the explicit Group role, rather than the unrelated derived-data predicate.
Focused malformed/header SD cases and ten section cases pass. The independent
raw-workbook checker passes both analyzers on frozen accuracy-only release
`fc4668c962bc` and the combined candidate. It checks 144 scalar values, 12 exact
SD strings, 12 missing percentage markers and complete row/group ownership.

## Performance

Eight bounded diagnostic observations preserve complete exports and MCP
responses. Manifest construction takes a median 137.0 ms on NYISO; column
statistics take 128.3 ms, distinct-key destruction 5.6 ms and pretty printing
0.4 ms. These scopes are nested, not additive. All temporary instrumentation was
reverted before production testing and measurement.

The candidate partitions the original disjoint n/s/b-prefixed distinct keys:
Number/Date share a set of the exact existing formatted numeric strings; text
uses borrowed strings; booleans use a two-bit mask. Counts, bounds, nulls and
example order remain unchanged. No retained cache or public API is added.
Two tests pass before and after, including signed zero, nonfinite values,
formatted collisions, mixed types, Unicode, duplicates, empty and null columns.

Six alternating pairs against the same-accuracy frozen binary preserve all CSV,
manifest and decompressed XLSX members and complete MCP responses. NYISO export
falls 1938.37 to 1892.04 ms (-2.39%, six of six faster). Financial and wide CSV
exports change -1.40% and -1.14%. MCP changes are mixed; no MCP gain is claimed.
NYISO export median peak RSS rises 3580 KiB; other memory changes are mixed.
No memory reduction is claimed.

Final six-pair comparison against accepted iteration 9 preserves all exports and
MCP responses. Export medians change -1.19% financial, -1.52% NYISO (1915.79 to
1886.75 ms, five of six faster) and -1.56% wide CSV. MCP changes remain mixed
(-1.43%, +0.43%, -1.89% respectively). NYISO export median RSS falls 4572 KiB
in this final run while MCP rises 788 KiB. These mixed memory observations do
not establish a general reduction.

## Validation status

The targeted accuracy tests, independent real-source checker, before/after
manifest tests and exact-output paired measurements pass. Quick checks pass:
19 snapshots, 22 mini-goldens, 21 noise families under both analyzers, 94 info
and 94 question checks, 78 public lookups and the 27-file fidelity set.
Release tests pass: 562 tests in 65 groups, zero failures and 17 ignored;
doctest execution completed. Clippy, 50 identical goldens and both structure
benchmark gates, six-corpus fidelity gate, 21 Node tests and TypeScript checking
pass. Strict per-file comparison passes all 4055 files with no fidelity regression
and no new L/C misses or M violations. Lookup remains 5498/5634, computation
improves 1198 to 1201/2264, and M remains 1539. The three resolved computation
checks are percentage questions on StatCan 632. Feature equivalence passes
41358 segments across six files/eight sheets with zero differences or skipped
sheets. All eight final real-source checkers pass on the unchanged combined
CLI hash. Final timings and all three independent evidence reviews pass.

Workers finish before heavy execution. Heavy jobs run globally serialized under
memory caps; resources and the memory guard are monitored. Evaluator/audit/gold/
split definitions are unchanged. No holdout, LLM/API, push or installed CLI update.

Evidence: `../scratch-archive/codex-iteration-10/`.
