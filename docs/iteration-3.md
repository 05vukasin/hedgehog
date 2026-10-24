# Iteration 3: repeated cohort headers and scoped distinct values

Baseline: local master `225b9da`, release `541500dbeb36`. Candidate source contains
cohort fixes through `429de74`, distinct-value patch `4c826db`, and parallel fixture
isolation `0ebe8f6`. All workers completed source/review work before heavy testing.

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
release/core gate, Node/TypeScript checks, fresh DEV score and exact per-item/public/
fidelity comparison are running. This iteration is not integrated or accepted yet.
No scorer, audit, split, holdout or fidelity baseline definitions changed. No API/LLM
calls, remote push or installed CLI replacement occurred.

Evidence and bounded scripts: `../scratch-archive/codex-iteration-3/`, including
`numeric-text-red.log`, `numeric-context-green-final.log`, `source-context-release.json`,
`real-feature-context.log`, `performance-context.json`,
`performance-context-confirm.json` and `performance-context-isolated.json`.
