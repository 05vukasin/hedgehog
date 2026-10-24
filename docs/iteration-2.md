# Iteration 2: sparse outline ancestry and analysis work

Baseline: `d1a2386`, release SHA prefix `b147b92202c1`. The owner's ongoing instruction is
paired accuracy/performance iterations, minimal production changes, and resource checks before
expanding the swarm. Source/review work finishes before heavy validation begins.

## Accuracy evidence

HiTab DEV `clean/statcan/521.xlsx` has WHO and IRCC-specific explicitly indented below
High (WHO+IRCC-specific), but only two of 25 output rows use this hierarchy. In DEV
`clean/statcan/2674.xlsx`, three drug categories are indented under Yes, only three of 46 rows.
The existing minimum-two-children and 10% coverage guard discarded both parent columns.

Retain sparse ancestry only where each ancestor is explicitly less indented than its child.
Keep the existing density guard for bold-only inference. Reuse the existing stack and paths;
add no public API, helper function or retained row index. An independent review caught the
distinction between an explicit edge and merely a row with some explicit ancestor: a valid
outer ancestor must not preserve an unsupported same-indent bold ancestor in that chain.

Generated regressions cover styles/leading spaces, one/two children, nested ancestry, resets
at sections, flat rows, separate bold-only families and mixed explicit/bold ancestry chains.
The first regression failed before the fix because `Characteristics_parent` was missing.
After the fix all five focused tests pass, exercising both analyzers in the workbook cases.
The two real-source files improve from 5/9 to 9/9 correct DEV lookup checks.

## Performance evidence

Two small changes remove work already proved unnecessary by existing state:

- Strictly sorted, unique cell positions need no duplicate-position lookup while building
  row/column lists. Unsorted/duplicate inputs retain the last-wins path.
- An unsplit row's segment mean reuses the existing row mean, with identical accumulation
  order and denominator. Split rows keep their existing summation.

No new cache or public API is introduced. Independent review and generated equivalence tests
cover sparse/extreme positions, duplicates, gaps, merges, empty sheets and threaded chunks.
Real-file equivalence covers 41,358 segments across eight sheets, with zero feature/model/layout
differences and no skipped sheets.

Alternating release comparisons use six initial pairs and twelve confirmation pairs per
case/mode. Disk cache is off; MCP latency runs from request flush to its matching response,
excluding initialization/shutdown. All normalized files and decompressed XLSX members match;
complete MCP responses match. The original Python wait4 RSS measurements inherited Python's
high-water mark; confirmation uses the existing tiny C launcher to measure the command itself.

| Confirmation medians | Baseline | Candidate | Interpretation |
|---|---:|---:|---|
| Financial sample, cold MCP open | 38.57 ms | 37.59 ms | 2.5% faster; 10/12 paired wins |
| NYISO, cold MCP open | 1,089.64 ms | 1,076.49 ms | 1.2% faster; 11/12 paired wins |
| Wide CSV, cold MCP open | 42.13 ms | 42.06 ms | no reliable gain |
| NYISO, normalize/export | 2,090.71 ms | 2,080.68 ms | modest 0.5% improvement |
| Financial / wide CSV normalize | 45.98 / 93.04 ms | 45.74 / 92.96 ms | within run variation |

Cold-open median peak RSS: financial 19.04 → 19.10 MiB, NYISO 267.70 → 265.08 MiB,
wide CSV 24.58 → 24.54 MiB. These local measurements support a modest gain on two tested
agent workloads, not a general speedup guarantee. Repeated trace names in these completed
reports retain the last sheet's occurrence; whole-process latency and RSS are independent.

Raw evidence and bounded scripts: `../scratch-archive/codex-iteration-2/`, including
`performance-confirm.json`, `real-feature-equivalence.log`, `accuracy-red.log`,
`accuracy-green.log`, `source-before.json` and `source-after.json`.

## Integration status

The first combined quick run caught two genuine false-parent regressions before any accepted
baseline changed. Portuguese social-report checks fell 7/7 → 5/7 because outline detection read
whitespace on a split M/F/T label fragment instead of the existing reconstructed label. That
changed a table schema and union grouping, hiding two needed column names in the catalog.
A generated regression failed before its four-line fix, which reuses the reconstructed label.

The Enron mini-golden change is also unsupported: `spin reserve log sheet!B20` has one incidental
leading space on an independent dated/operator log entry. It acquired a false parent column in
T18. Before/after exports are saved under the iteration evidence folder. The generated padding regression also failed before its fix. Sparse text edges now require
a two-space step; style indentation remains explicit and the old dense one-space policy stays
unchanged. All thirteen focused outline/structure tests pass after these follow-ups.

Source/review work reopened after these failures and completed with independent review of
both follow-ups. Heavy jobs paused during worker investigation. The combined candidate
`817dfef` is rerunning the unchanged quick gates, followed by full DEV scoring and release
checks. It is not accepted yet. No scorer, audit, baseline or holdout definitions were changed.
