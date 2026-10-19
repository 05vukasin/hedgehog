# Performance

Status of the `ws/perf` workstream: how fast the Rust pipeline is, where the time goes, what
was changed and what is left for the other workstreams.

All numbers: release profile (fat LTO), 16-core laptop shared with other agents' builds, so
expect ±10% noise between runs (key comparisons were repeated, alternating binaries).

## How to measure

```bash
export CARGO_TARGET_DIR=/home/vukasin/projects/excell-lib-project/hedgehog/target
S=/home/vukasin/projects/excell-lib-project/scripts/run_capped.sh

# throughput + per-stage share + peak RSS over a directory or a file list
$S 4G bench.log cargo run --release -p hh_cli -- bench core/benches/enron50.txt [--threads 8] [--open-only] [--limit N]
# generated worst cases (dense, sparse over the whole sheet, merges, formulas, 1M rows, 16k cols, giant merges)
$S 4G bench.log cargo run --release -p hh_cli -- bench --synthetic [--case tall_1m]
# criterion, per stage, synthetic + real files (HH_BENCH_BIG=1 adds the 9 biggest files)
$S 4G bench.log cargo bench -p hh_core --bench stages
# golden check: every output of every stage must hash as before
$S 4G gold.log cargo run --release -p hh_cli -- golden core/benches/enron50.txt --threads 4 --check core/benches/golden_enron50.txt
$S 4G gold.log cargo test --release -p hh_core --test perf_golden -- --ignored --nocapture
# merged areas of the byte scanner vs calamine on the whole Enron set
$S 5G m.log cargo test --profile profiling -p hh_core --test reader_merges_corpus -- --ignored --nocapture
```

* Stages (`hh_core::eval::bench`): `open` (reader), `grid` (`Grid::new`), `context`
  (`context::compute`), `values` (`values::compute`), `totals` (`totals::compute`, the `t_*`
  features), `detect_verify` (`totals::detect` + `verify` of every candidate), `baseline`
  (`eval::baseline::analyze`).
* `core/benches/enron50.txt`: the 9 largest distinct Enron files, `richard_ring__31626`
  (582k cells) and every ~400th file by name — 50 files, 281 sheets, 7.8M cells, 161k formulas.
* `core/benches/golden_enron50.txt`: FNV hash per file of cells, merges, every feature column,
  totals detect + verify results and the baseline layout. **Regenerate it** (`hedgehog golden
  core/benches/enron50.txt > core/benches/golden_enron50.txt`) when a workstream changes outputs
  on purpose (values/layout merges will).
* `[profile.profiling]` = release + debug symbols + thin LTO. No sampling profiler is usable
  here (`perf_event_paranoid=2`, samply refuses), so hotspots were found with `Instant`
  instrumentation of sub-steps (probe tests, not committed).

## Results

enron50, 1 thread, end to end (open + all stages):

| | baseline | final |
|---|---|---|
| wall | 304.0 s | 17.3–19.5 s (**~16×**) |
| files/s | 0.16 | 2.6–2.9 |
| cells/s | 25.7k | 400–450k |
| peak RSS | 587 MB | 506 MB |
| with `--threads 8` | – | 6.5 s · 7.7 files/s · 1.2M cells/s · 1.86 GB |

Per stage (CPU s, 1 thread):

| stage | baseline | final | notes |
|---|---|---|---|
| open | 12.5 | 5.5 | one XML pass, merge byte-scan |
| grid | 0.60 | 0.19 | |
| context | 5.7 | 3.3–3.8 | |
| values | 4.9 | 4.6–5.0 | not touched (other workstream) |
| totals | 159.8 | 1.6–1.8 | quadratic scans removed |
| detect_verify | 115.5 | 1.0–1.2 | |
| baseline | 4.7 | 1.6–2.2 | |

Worst file: NYISO Price Information (890k cells, 38k SUM/+ formulas) 273.7 s → 2.0 s.
Open only (reader): 12.2 s → 5.6 s, peak 358 → 177 MB.
`hedgehog eval deco --split all` (854 files): 7.0 s → 1.4 s with `--threads 8` (identical report).

Synthetic (`hedgehog bench --synthetic`, ms):

| case | stage | baseline | final |
|---|---|---|---|
| giant_merge (A1:XFD1048576) | grid | killed at 2 GB cap (17.2G map inserts) | 0.0 ms, whole pipeline 36 ms |
| giant_merge_sparse (cells in opposite corners + giant merge) | baseline | 17.2G positions | 0.0 ms, pipeline 6 ms, 4 MB |
| xlsx using A1 and XFD1048576 | open | calamine builds two dense rows×cols ranges (17G × 32 B) | 2 cells, instant |
| formula_heavy_50k | totals / det+ver | 545 / 556 | 46 / 26 |
| tall_1m (1M rows + SUM(A:A)) | context | 1601 | 600–710 |
| tall_1m | baseline | 754 | 270–360 |
| dense_100k | grid / context | 3.6 / 75 | 1.8 / 44–49 |

## Hotspots found (measured, not guessed)

1. **totals was quadratic** (52% + 38% of baseline time, all of it on two files):
   `Grid::used_range()` recomputed the bounding box over all cells on every call and was
   called per candidate (`text_left`/`text_above`, `verify`); `text_left/above` then walked every
   position back to the used range's edge; typed totals scanned all sheet cells per keyword
   row; `verify` probed every position of a range and did O(k) `Vec::contains` per cell.
2. **reader**: `worksheet_range` + `worksheet_formula` parse every sheet XML twice and build two
   dense `rows × cols` grids; `merge_cells_by_sheet_name` parses it a third time just to reach
   `<mergeCells>` at the end (~40% of the reader). Shared strings were copied twice per cell.
3. **Grid** expanded each merged area into one map entry per position (A1:XFD1048576 → 17.2G);
   SipHash on `(u32, u32)` keys everywhere.
4. **context**: hash lookups per cell for row/column aggregates (row aggregates alone 270 ms on
   1M rows); SipHash digests of every value for the distinct ratio. Neighbour lookups are only
   15–25% of context — not worth a row-walk rewrite yet.
5. **values** (not changed, see Deferred): ~1 µs per text cell; only 9.2% of the 2.4M text cells
   in enron50 are distinct per column.
6. Things checked and *not* hot: `Grid::new` (1%), `DependencyIndex` (≤100 ms on 38k formulas),
   formula parsing.

## Optimizations (one commit each, golden check identical after every one)

| commit | change | effect |
|---|---|---|
| `perf: benchmark harness…` | bench/golden CLI, criterion, synth sheets, profiling profile | – |
| `reader: … one streaming pass` | `next_cell_with_formula` straight into sparse cells; sort/fold dups exactly like the dense ranges; other formats keep the old path | open 12.2 → 7.6 s, RSS 358 → 176 MB |
| `grid: FxHash index, interval merge index, cached used range…` | packed u64 keys + FxHash; merges indexed per row (short) / per column (tall narrow) / scanned (big); `used_range` cached; new `row_cells`, `col_cells`, `cells_in`, `merge_index_at` | 304 → 41 s end to end; giant merge OK |
| `totals: no per-candidate scans…` | nearest label via per-line prefix tables + binary search; row cells via grid; `verify` over `cells_in` (same row-major order → identical float sums) + hash sets | 41 → 25 s |
| `context, eval baseline: FxHash…` | | 25 → 22 s |
| `totals: allocation-free keyword pre-check` | ASCII text without tot/sum/ukup/svega/zbir can't be a keyword | totals 2.65 → 1.58 s, det+ver 1.73 → 0.97 s |
| `context: per-cell slots…` | row-major sheets need no maps for row aggregates; columns via stable counting sort | tall_1m context 1.40 → 0.60 s |
| `eval: … bounded worker threads` | `evaluate_parallel`, `hedgehog eval --threads N` (default min(4, CPUs)), ordered merge | DECO all 7.0 → 1.4 s (8 threads), report identical |
| `reader: … byte scan instead of a second XML parse` | own mergeCells scanner with calamine's rules, verified by `<dimension>`; falls back to calamine | open 8.4 → 5.9 s; 15,471 files / 63,720 sheets identical to calamine, 0 fallbacks |
| `features: hash index…` | `get`/`value` O(1); `index_of` + `column` | 15 lookups/cell: 47.5 → 12.6 ms per 100k cells |
| `eval baseline: bounded on giant merged areas` | union-find path when clipped merges cover > 4·cells + 1M positions; randomized equivalence test vs the flood fill | giant_merge_sparse: 17.2G positions → 0 ms |
| `context: distinct values by exact key` | FxHashSet of borrowed values instead of SipHash digests | context −6% |
| `reader: choose the format from the file's content` | magic bytes / zip entries pick xls/xlsx/xlsb/ods (integrator request) | EU corpus 292/315 → 315/315 open |

Tried and rejected: **mimalloc** as the CLI's global allocator — measured 10–25% *slower*
end to end and +40 MB RSS (two alternating runs each), so the system allocator stays.

## API changes callers must know

* `eval::Analyzer` now has `Sync` as a supertrait (needed by `evaluate_parallel`). `fn` items and
  closures over `Sync` data (e.g. `&|g: &Grid| layout::analyze(g)`) are unaffected.
* Additive only: `Grid::{row_cells, col_cells, cells_in, merge_index_at}`,
  `Features::{index_of, column}`, `eval::run::evaluate_parallel`, `eval::bench`,
  `reader::merge_scan_stats` (doc-hidden). `Cell`, `Value`, `Sheet`, `Grid`, `Features`
  signatures are unchanged.
* `Grid::used_range()` is now a cached field (same value).
* Duplicate positions in `sheet.cells` (never produced by the reader): `Grid` lookups still
  resolve to the last one; `row_cells`/`col_cells`/`cells_in` list only that one.

## Deferred (for other workstreams — measured, not changed here)

**values** (`core/src/values/`, now the largest stage after `open`: ~24%):
1. `features.rs:105-115` `compute` + `cell_features` text branch: memoize per `(column hints,
   text)` within a sheet. Only 9.2% of text cells are distinct per column on enron50, and text
   cells cost ~0.8–1.2 µs each (ERMS NG_02: 400k text cells = 308 ms of 597 ms) — expected
   values −60…−70%.
2. `features.rs:84-91` `sheet_column_hints`: std `HashMap` (SipHash) keyed by column, entered per
   text cell, then `hints.get(&c.col)` per cell at :110. Use a `Vec<ColumnHints>` indexed by
   column (≤ 16,384) or FxHashMap — hints are 20–25% of values (123 ms of 500 ms on NG_01).
3. `DecimalVotes::add` (number.rs:604) and `DateOrderVotes::add` (date.rs:527) parse the same
   text that `cell_features` parses again; with (1) the votes can be taken from cached parses.

**layout** (`core/src/layout/`, still a stub):
1. Never call `Features::value(name, row)` inside per-cell loops; resolve columns once
   (`get` / `index_of` + `column`) — now O(1) anyway, but a slice index is ~10× cheaper.
2. Use `Grid::row_cells`, `col_cells` and `cells_in` instead of scanning positions over the used
   range (that pattern made totals quadratic). Don't call `sheet.used_range()` in loops —
   `Grid::used_range()` is cached.
3. Don't expand merged areas into positions; use `Grid::merge_at` / `merge_index_at`.
4. The 8 `kind_at` neighbour probes in `context` are 15–25% of context; if layout needs
   neighbourhood features too, consider sharing one row-walk pass (cells are row-major).

**py** (`py/src/lib.rs`):
1. Release the GIL around `hh_core::open` and any compute (`py.detach`/`allow_threads`) so
   Python can open files on threads; bindings should never hold it during Rust work.
2. Sheets are independent after `open`: compute features per sheet on scoped threads
   (bounded, e.g. min(4, CPUs)); per-file parallelism like `hedgehog bench --threads` gives
   ~3× on 8 threads (memory grows with threads: 0.5 GB → 1.9 GB peak on enron50).
3. Expose `Features` columns as NumPy arrays without copying (one `Vec<f32>` per column).
4. Don't switch the extension module to mimalloc (measured slower here).

**reader (future, PLAN M10)**: `open` is now ~28% of time; the rest is calamine's shared-string
parse and cell XML events. An own quick-xml reader (styles are needed anyway) could also skip
expanding shared formulas per cell and intern repeated strings.

## Worst cases

`core/tests/perf_worst_cases.rs` runs in every `cargo test`: giant merges (with and without far
cells), 20k cells over the whole sheet, one row × 16,384 columns, 50k rows, 2k merges, formula
grids, and an xlsx using A1 and XFD1048576. Memory now scales with cells (+ merge count ×
min(height, width, 64)), never with used-range or merge area. Known remaining super-linear
spots, all bounded or pathological-only: `verify` sums each candidate's range (inherent);
`totals::detect` SUBTOTAL exclusion is O(subtotals × formulas); `DependencyIndex` marking is
capped at 20M visits; the baseline's union-find merge sweep is quadratic only for many
row-overlapping merges.
