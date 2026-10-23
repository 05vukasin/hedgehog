# Development

How to build hedgehog from source, run the tests and the evaluation, where test data lives, and how
releases and CI work.

## Prerequisites

- Rust, stable toolchain (the workspace uses edition 2024), with `clippy`.
- Python 3.9 or newer, and [uv](https://docs.astral.sh/uv/) or pip.
- [maturin](https://www.maturin.rs/) 1.7 or newer for the Python extension.

## Build

```sh
git clone https://github.com/05vukasin/hedgehog
cd hedgehog

cargo build -p hh_cli                       # the CLI: target/debug/hedgehog
cargo run -p hh_cli -- describe file.xlsx   # or run it through cargo

uv venv .venv
uv pip install --python .venv/bin/python maturin openpyxl pandas pytest
(cd py && ../.venv/bin/maturin develop --uv)          # editable install into .venv; add --release for speed
.venv/bin/python -c "import hedgehog; print(hedgehog.open('file.xlsx').describe())"
```

`maturin develop` builds an unoptimized extension, which is fine for tests but several times slower
on large files; use `--release` for real workloads. The release profile uses fat LTO and one codegen
unit, so a release build takes a few minutes.

On a machine with little memory, limit parallel compile jobs with `CARGO_BUILD_JOBS=2`.

## Tests

```sh
cargo test --workspace --exclude hh_py      # Rust unit and integration tests
cargo clippy --workspace -- -D warnings     # lints, warnings are errors (as in CI)
(cd py && ../.venv/bin/python -m pytest -q tests)   # Python tests (needs the extension built)
```

- **No spreadsheets in git.** Rust tests build their xlsx files with `rust_xlsxwriter` at test time;
  Python tests build them with `openpyxl` in a temporary directory. `.gitignore` excludes `*.xlsx`,
  `*.xls`, `*.csv` and similar.
- **No real API calls.** LLM tests use scripted clients and mocked HTTP transports, and an autouse
  fixture removes every provider variable from the environment. Provider tests are skipped when the
  SDK is not installed (`pip install openai anthropic boto3` to run them all).
- **Corpus tests are opt-in.** Tests that read the real corpora are marked `#[ignore]` and need the
  data on disk plus environment variables (for example `HH_LAYOUT_CORPUS`). Run them with
  `cargo test -p hh_core --test layout_corpus -- --ignored --nocapture`.

### Testing in tiers

The full gate takes 1–2 hours. Don't run it for every small change: run tier 0 after every edit,
test what the change can affect, and run the full gate once at the end of a batch. Every tier has a
`make` target.

| Tier | When | What | Time |
|---|---|---|---|
| 0. Quick — `make quick` | after every edit | feature snapshots, mini golden on 22 real files, noise smoke, info scoreboard on 17 small cases without and with the question (`eval/quick/info.py`: no case may lose a check that's in the catalog or one query away, no trap check may fail, no catalog may grow > 5 %) (below); no LLM, no network, same output on every run | ~5 s with nothing to rebuild, ~15 s after an edit in `core/` (up to ~30 s on a busy machine), ~2–3 min cold |
| 1. Targeted | after each small change | `cargo test -p <crate> <filter>` for the touched module; the specific files, eval cases (`check_cases.py --cases …`) or noise family (`noise_eval --families …`, `--repro`) the change is about | seconds to ~2 min |
| 2. Merge — `make merge-check AREA=core\|agent\|docs` | when merging a branch | tests of the affected crates; clippy; golden check; only the benchmarks/noise families the changed area can affect (layout/totals/values → both benchmark analyzers; agent/ → agent tests + Node; docs/eval only → nothing heavy) | ~5–10 min |
| 3. Full verify — `make full` | once per batch, before a release or a published comparison | everything: `cargo test --workspace`, clippy, golden, both benchmark analyzers on all corpora, all noise families, the file sweep, `check_cases.py`, Node, Python, then the agent eval (by hand) | ~1–2 h |

If tier 2 turns up a surprise, widen the test for that area only. Heavy jobs go through
`scripts/run_capped.sh` (see the memory notes in the repo's parent folder); the `make` targets do
that for every step and keep each step's output in `target-logs/<step>.log`.

#### Tier 0: `make quick`

`scripts/quick.sh` runs five parts and prints one result line each (timings go to stderr, so the
rest of the output is identical on every run):

| Part | What it checks | Stored result | Needs |
|---|---|---|---|
| snapshots | `agent/tests/snapshots.rs`: one small workbook per agent-facing feature, built in the test (total, of-which and average rows, subtotals above groups; wide years and text periods; `<id> - <name>` and ID + name columns; several files with `f1`/`f2` and JOIN KEYS; union views; the quality line; signs; units from titles; booleans; notes on empty results; SQL dialect and total-label hints; footnote marks; EU/US/Latin-1 CSV numbers; side-by-side and stacked tables) — its `open_file` catalog and 1–3 query results | `agent/tests/snapshots/<feature>.txt` | nothing (also runs in CI) |
| golden | the `open_file` catalog (one `hedgehog mcp` session) and the `hedgehog normalize` manifest of 22 small real files: finance, statistics offices, registers, xlsx/xls/ods/CSV, EU and US | `eval/quick/files.txt`, `golden.tsv` (table count, catalog and manifest hashes) | `$HEDGEHOG_DATA` |
| noise | `noise_eval --seeds 1 --intensities 0.5`: one workbook per noise family (21), rules and model path; fails on a crash or a family that passed before | `eval/quick/noise.tsv` | `$HEDGEHOG_DATA` (its `clean/` tables) |
| public | the public-benchmark slice (`eval/public/quick.py`): 14 trimmed HiTab tables through `hedgehog` (cells, normalize, `open_file`), their 78 lookup questions scored for value-label linking (cell found, number equal, column header exact, row labels exact / leaf); fails when a count drops or a catalog grows > 5 % | `eval/public/quick.tsv` | `$HEDGEHOG_DATA/public/hitab` ([eval/public/README.md](../eval/public/README.md)) |
| info | the info scoreboard (`eval/info/score.py --question`) on 17 small cases (with the naive-query traps of pt-social-report, uk-ons-labour and enron-henry-hub), two lines: the catalog as usual, and with each case's prompt as `open_file`'s `question` (B150; the line gives the size change). Fails when a case loses a check that is in the catalog or one query away, a trap check fails, or a catalog grows > 5 % | `eval/quick/info.tsv`, `eval/quick/info_question.tsv` | `$HEDGEHOG_DATA` |

A difference fails with a readable line diff: the snapshot lines that changed, or for the golden
files the catalog lines (against the texts in `eval/quick/catalogs/`, kept locally and gitignored
because they are derived from the private test data). If the change is intended, accept it and
review what changed:

```sh
make quick-update                 # = scripts/quick.sh --update; or UPDATE_SNAPSHOTS=1 cargo test -p hh_agent --test snapshots
git diff agent/tests/snapshots/ eval/quick/
scripts/quick.sh --only snapshots # one part; --only golden,noise for several
```

Builds use the `quick` Cargo profile in `target-quick/`: dependencies optimized once, hedgehog's
own crates unoptimized and incremental, so a rebuild after an edit takes seconds and does not touch
the release builds in `target-master/`. Without `$HEDGEHOG_DATA` (or `../testdata`) the golden and
noise parts are skipped. To add a feature snapshot, add a `#[test]` to `snapshots.rs` that writes
its workbook, calls `s.open(…)` / `s.query(…)`, ends with `s.check()`, and run `make quick-update`;
to add a real file, add it to `eval/quick/files.txt` (small, < 1 MB) and run `make quick-update`.

## Fuzzing

`fuzz/` holds `hh_fuzz`, a mutation fuzzer that runs on the stable toolchain (cargo-fuzz needs
nightly). Targets: `csv`, `xlsx`, `xls`, `ods` (the readers, then normalize, the agent catalog and
the SQL load for small results) and `normalize` (generated grids straight into the pipeline). Zip
packages are mutated part by part and re-zipped, so the mutations reach the XML parsers. Panics are
caught and saved once per location; hangs (`--timeout`, 10 s) and aborts (the heap limit
`--rss-mb`, 1500 MB, stack overflows) end the process, and `fuzz/run.sh` restarts it and keeps the
input.

```sh
S=../scripts/run_capped.sh   # always under a memory cap
$S 2500M build.log cargo build --profile fuzz -p hh_fuzz
target/fuzz/hh_fuzz seeds /tmp/seeds          # generated seed workbooks; add small real files
$S 2500M fuzz.log fuzz/run.sh xlsx 600 /tmp/seeds/xlsx /tmp/fuzz-xlsx target/fuzz/hh_fuzz
target/fuzz/hh_fuzz run /tmp/fuzz-xlsx/crashes/<file>   # reproduce (prints a backtrace)
```

`fuzz/sweep.py` runs `hedgehog normalize` over every spreadsheet file under a folder, one process
per file (heap capped at 2.3 GB, 300 s timeout), and writes a resumable TSV of outcomes (`ok`,
`error`, `limit`, `crash`, `timeout`):
`$S 2500M sweep.log python3 -I fuzz/sweep.py target/release/hedgehog $HEDGEHOG_DATA /tmp/sweep.tsv`.

`fuzz/determinism.py` checks that the output is repeatable (B68): it normalizes every file of a
folder or file list several times (`--runs N` threaded, `--single N` pinned to one CPU) and compares
the manifest and every part of the normalized xlsx byte for byte; any difference is a bug (see
[normalized-output.md](normalized-output.md#repeatable-output)):
`$S 2500M det.log python3 -I fuzz/determinism.py target/release/hedgehog <dir|list> /tmp/det.tsv --runs 3`.
A `HashMap` / `HashSet` from `std` iterates in a different order in every run: never let its order
reach an output (sort, use a `BTreeMap`, or an `FxHashMap`, whose order is the same in every run
for the same inserts), and break ties of `max_by_key` / `min_by_key` over one explicitly.

The `fuzz` profile keeps overflow checks and debug assertions on, so silent wraparounds show up.
Bugs found in calamine are fixed in `vendor/calamine` (each change marked `hedgehog:`); every fix
gets a regression test in `core/tests/hostile_files.rs`, built from bytes in the test.

## Test data

Corpora are several GB and never committed. They live next to the repository, by default in
`../testdata`, and are located through `HEDGEHOG_DATA`:

| Folder | Contents |
|---|---|
| `clean/` | Well-structured datasets with known column meanings; input for the planned noise generator. |
| `irregular/` | Real messy spreadsheets: Enron, DECO annotations, government statistics releases. Used for evaluation and robustness runs. |
| `synthetic/` | Generated messy sheets with ground-truth labels (planned noise generator output). |
| `reference/` | Dictionaries for semantic typing (cities, countries, currencies). |

Downloaded files are untrusted: they are only read by parsers, never executed.

## Evaluation

```sh
export HEDGEHOG_DATA=/path/to/testdata
cargo run -p hh_cli --release -- eval deco --split dev --analyzer layout
```

Run it before and after any change to `layout`, `values` or `totals` and compare the scoreboards,
and `hedgehog benchmark --analyzer model` for the default path across all corpora. Changes to the
features (`context`, `values`, `totals`, `eval/export.rs`, `learned.rs`) change what the learned
model sees: retrain it ([learned-model.md](learned-model.md#retraining)) or check that the
benchmark does not drop. Metrics and current scores: [evaluation.md](evaluation.md).

## Code layout and conventions

- All analysis lives in `hh_core`. The core has no network access and makes no LLM calls; keep it
  that way so results stay deterministic and testable.
- Bindings in `py/src/lib.rs` stay thin: look up, convert to dicts, nothing else.
- Each workstream owns its folder in `core/src/` (`layout`, `values`, `totals`, `context`, `eval`,
  `normalize`); see [architecture.md](architecture.md).
- Thresholds of the layout rules are in `core/src/layout/params.rs`.

## Documentation

- The README stays short; details go to `docs/`. Every page starts with a one-line purpose and ends
  with "See also".
- Every example must run. Code samples use the sample workbook from the
  [quickstart](quickstart.md#a-sample-messy-workbook), and outputs are copied from real runs.
- Only measured numbers. A score in the docs must come from `hedgehog eval` or a recorded run.

## Continuous integration

`.github/workflows/ci.yml` runs on pushes to `master` and on pull requests:

1. the tier 0 feature snapshots, `cargo test -p hh_agent --test snapshots` (the parts of
   `make quick` that need the private test data run only locally)
2. `cargo test --workspace --exclude hh_py`
3. `cargo clippy --workspace -- -D warnings`
4. build a wheel with `maturin build --profile dev`, install it, run `pytest`

It is tuned for the GitHub Actions free tier:

- pushes that only touch Markdown, `docs/`, `assets/`, `LICENSE` or `.env.example` do not run CI;
- a newer push cancels the running job of the same branch;
- the Rust cache is saved only from `master`; pull requests reuse it without adding entries;
- the job has a 30-minute timeout.

## Releases

`.github/workflows/release.yml` builds prebuilt wheels so users do not need Rust:

| Wheel | Runner | Smoke test |
|---|---|---|
| Linux x86_64 (manylinux) | ubuntu-latest | yes |
| Linux aarch64 (manylinux, cross-compiled) | ubuntu-latest | no |
| macOS universal2 (Intel and Apple Silicon) | macos-latest | yes |
| Windows x64 | windows-latest | yes |
| source distribution | ubuntu-latest | |

All wheels are `abi3`: one wheel per platform works on CPython 3.9 and newer. The smoke test
installs the wheel without Rust, generates a small workbook and opens it.

The `node` job builds the Node addon (`npm run build -- --target <triple>`, Cargo profile
`node-release`: the release profile with unwinding panics) for `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu` (native arm runner), `aarch64-apple-darwin`, `x86_64-apple-darwin`
(cross-compiled on the Apple Silicon runner, not tested) and `x86_64-pc-windows-msvc`, checks that the
committed `node/binding.js` / `binding.d.ts` are current, and runs `npm test` and `npm run typecheck`.
On a tag, the `npm` job makes the platform packages (`hedgehog-sheets-<platform>`, napi-rs
`create-npm-dirs` / `artifacts` / `prepublish`), packs every package into a `.tgz`, and publishes
them to npm only when the repository variable `PUBLISH_NPM` is `"true"` (secret `NPM_TOKEN`, with
provenance). See [node-api.md](node-api.md#building-from-source) for local builds.

To release:

1. Set the version in the workspace `Cargo.toml` (`[workspace.package] version`) and in
   `node/package.json` (the `npm` job refuses a tag that does not match it); the Python package
   takes its version from `Cargo.toml`.
2. Add the release notes to `CHANGELOG.md`.
3. Commit, then tag and push: `git tag v0.1.0 && git push origin v0.1.0`.
4. The workflow builds and tests all wheels, CLI binaries and Node addons, then creates a GitHub
   Release (marked pre-release) with the wheels, the sdist, the CLI binaries and the npm tarballs
   attached.
5. PyPI: the `pypi` job publishes through PyPI trusted publishing (no stored token) only when the
   repository variable `PUBLISH_PYPI` is `"true"` and a `pypi` environment is configured.
6. npm: the `npm` job publishes the platform packages, then `hedgehog-sheets`, only when the
   repository variable `PUBLISH_NPM` is `"true"`.

Running the workflow by hand (Actions → release → Run workflow) builds and tests everything without
publishing anything. Build artifacts are kept for one day; the release assets are permanent.

## See also

- [architecture.md](architecture.md): modules and data flow
- [evaluation.md](evaluation.md): metrics and how to read the scoreboard
- [installation.md](installation.md): installing as a user
- [`PLAN.md`](../PLAN.md): milestones
