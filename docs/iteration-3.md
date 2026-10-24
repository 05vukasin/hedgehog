# Iteration 3: repeated cohort headers and scoped distinct values

Baseline: local master `225b9da`, release `541500dbeb36`. Candidate source contains
cohort fixes through `429de74`, distinct-value patch `4c826db`, capacity guard
`0e86da4`, and parallel fixture isolation `0ebe8f6`. Final release: `21ab0c6aed37`.
All workers completed source/review work before heavy testing.

## Source-backed accuracy

HiTab DEV `clean/statcan/1535.xlsx` has Men followed by Women. The second block
repeats merged disease parents, including Circulatory diseases → Cancer, but not
Crude/ASHR/confidence-interval child rows. Previously Women records inherited Men
and the first block's disease names. Two of four lookup checks failed.

The normalizer splits proven cohort blocks before building/typing each table.
It requires complete matching merged-parent geometry, a cohort stub, inherited
child-header coverage and numeric records on both sides. Each block retains its
own parents and cohort; only donor child cells are reused, with their original
source coordinates. Existing table building handles ownership, totals and notes
independently. Physical header bounds remain physical; inherited cells are explicit
in header provenance and a warning. Repeated and returning parent names compare
against the preceding accepted block, including when the cohort label is unchanged.

Initial generated fixtures used native numbers and passed, while the real file
still failed: its numeric values are stored as text. A new regression reproduced
the gap. The guard now reuses the existing number parser. Actual source checks
improve **2/4 → 4/4**, with Cancer present and no old Men label on female rows.

Generated coverage includes exact male/female values, every female source value,
row partitions, parent/child coordinates, changed merge geometry, metadata gaps,
changed unit text, returning/unchanged parents and numeric-text storage. Concurrent
fixture calls now use unique temporary paths. Thirty-two focused tests pass; one
corpus equivalence test is separately run rather than silently counted as passed.

## Performance experiments

The row-feature mean-fusion experiment passed bitwise/export equivalence but showed
no measured latency improvement; it was removed before final validation.

The retained candidate clears the existing distinct-value set after each physical
column. Stable column order makes earlier columns irrelevant; component identity
and value-key semantics remain unchanged. Existing last-column state and set
allocation are reused. Live entries are bounded by one column, and retained capacity
by the largest column, rather than all columns together. No cache, helper, public
API or retained field is added.

Tests cover interleaved components, repeated values across columns, duplicate
coordinates, reversed input, signed zero and both column-order implementations.
Real bitwise feature/model/layout comparison passes across **41,358 segments**, eight
sheets and six files: zero differences, skips or unreadable files.

Six-pair and twelve-pair comparisons against the previous release preserve exact
CSV/manifest exports, decompressed XLSX members and complete MCP responses. Combined
latency results are mixed, so a separate twelve-pair run isolates the performance
patch with the same accuracy fix in both binaries (`c8480f84f54f` → `ff06f1daf0f1`).

| Isolated medians | Accuracy only | With scoped set |
|---|---:|---:|
| Financial cold MCP open | 37.96 ms | 38.08 ms |
| NYISO cold MCP open | 1,082.50 ms | 1,082.06 ms |
| Wide CSV cold MCP open | 42.17 ms | 41.64 ms |
| Wide CSV normalize/export | 92.89 ms | 92.52 ms |
| Wide CSV normalize peak RSS | 24.02 MiB | 23.04 MiB |
| NYISO cold-open peak RSS | 263.91 MiB | 268.74 MiB |

The wide-CSV opening/normalize direction repeats across runs; the isolated open
median improves 1.3%, with 8/12 paired wins. Other latency differences do not support
a speed claim. NYISO's internal context stage improves but parallel work dominates
total time; a faster internal stage is not reported as overall analysis speed.
Peak process memory is workload dependent despite the smaller scoped set.

## Validation status

All focused tests, actual source checks and bitwise comparisons pass. The full
core gate passes after the capacity guard: 524 release tests (17 ignored), Clippy,
50 golden outputs, both
structure benchmarks and fresh 4,055-file fidelity. Node's 21 tests and TypeScript
checks pass. Fresh DEV scoring gives L 5,493/5,634 (97.7% audited, 132 misses),
C 1,198/2,264 and M 1,560 in 763 files.

The strict public comparison reports HiTab exact column matches 3,597 → 3,595.
Independent source review identifies exactly two annotation omissions: DEV items
`26efe0336e86b2b0c388cc55b52a270b` (C13=310) and
`22c640ecce3db7cedf7b98d38a297852` (C12=668). Their gold path is All-cause / ASHR;
the source explicitly says All-cause (excluding pregnancy) in merged B11:E11,
with ASHR at C4. The candidate preserves that qualifier and correct Women context;
values and lenient column matches remain correct. The raw strict metric is retained,
and no source qualifier, evaluator or annotation is changed to restore its count.
The separate artifact-only reviewed comparison has passed independent source review.
Its final execution passes: exactly these two explained changes, no new L/C misses
or M violations, and zero per-file fidelity regressions across 4,055 files. The
original strict failure remains saved; the reviewed artifact is separate from the
project evaluator. HiTab row_exact and row_lenient both increase by two. Only the
source workbook's catalog changes (3,377 → 4,286 characters); corpus mean tokens
increase 773.44 → 773.64 (+0.026%) to preserve the recovered cohort/header context.

A further bounded performance probe found a skewed-column cost before acceptance.
With 500,000 values in the first column, adding 8,192 singleton columns (1.6% more
cells) raises Context::new median 81.35 → 164.04 ms. The retained-capacity clearing
control takes about 85 ms. The source-reviewed capacity guard now releases disproportionate retained storage
while preserving normal reuse and amortized linear work (commit `0e86da4`).
Nineteen focused tests pass. On the same generated 508,192-cell input, its first
three-repeat probe gives median 81.10 ms, versus 164.04 ms before the guard.
This is a workload-specific measurement, not a general end-to-end speed claim.
The refreshed full core gate, Node tests/TypeScript and DEV comparison pass.
Final release bitwise equivalence also passes across all 41,358 segments, with no
skips or differences. All source/review work finished before heavy validation.

The final six-pair comparison isolates the capacity guard (`ff06f1daf0f1` →
`21ab0c6aed37`) and preserves every CSV/manifest/XLSX member and complete MCP response.
Financial cold MCP is 37.91 → 38.04 ms, NYISO cold MCP 1,075.06 → 1,071.87 ms,
and wide CSV cold MCP 41.02 → 40.13 ms. These small mixed changes do not establish
a general speedup. Wide normalization is 92.20 → 91.76 ms (5/6 paired wins).

NYISO normalization initially measures 2,069.70 → 2,118.28 ms (3/6 wins), with
variable export timing. The targeted twelve-pair confirmation reverses that median
to 2,079.10 → 2,062.97 ms (4/12 wins), preserving every output. Analysis-stage traces
are effectively unchanged; NYISO overall latency is treated as neutral/inconclusive.
The device shutdown interrupts only the optional MCP extension after six pairs;
that incomplete extension is excluded from the twelve-pair claim. Its saved report
contains the complete normalization case; the earlier complete six-pair MCP run
remains the evidence for MCP. No further timing repetition is justified by these
results. The supported performance gains are the scoped-set wide workload direction
and removal of the demonstrated skewed-column clearing cost.

This iteration is accepted for local integration with the explicit source-backed
strict-header limitation above. Remaining accuracy work includes 132 audited lookup
misses, 42 computation library misses and 1,560 misleading-output violations.
No scorer, audit, split, holdout or fidelity baseline definitions changed. No API/LLM
calls, remote push or installed CLI replacement occurred.

Evidence and bounded scripts: `../scratch-archive/codex-iteration-3/`, including
`numeric-text-red.log`, `numeric-context-green-final.log`, `source-context-release.json`,
`real-feature-context.log`, `performance-context.json`,
`performance-context-confirm.json`, `performance-context-isolated.json`,
`performance-capacity.json`, `performance-capacity-nyiso-confirm.json`,
`capacity-real-equivalence.log` and `verification-reviewed.json`.
