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

Run it before and after any change to `layout`, `values` or `totals` and compare the scoreboards.
Metrics and current scores: [evaluation.md](evaluation.md).

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

1. `cargo test --workspace --exclude hh_py`
2. `cargo clippy --workspace -- -D warnings`
3. build a wheel with `maturin build --profile dev`, install it, run `pytest`

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

To release:

1. Set the version in the workspace `Cargo.toml` (`[workspace.package] version`); the Python
   package takes its version from there.
2. Add the release notes to `CHANGELOG.md`.
3. Commit, then tag and push: `git tag v0.1.0 && git push origin v0.1.0`.
4. The workflow builds and tests all wheels, then creates a GitHub Release (marked pre-release) with
   the wheels and the sdist attached.
5. PyPI: the `pypi` job publishes through PyPI trusted publishing (no stored token) only when the
   repository variable `PUBLISH_PYPI` is `"true"` and a `pypi` environment is configured.

Running the workflow by hand (Actions → release → Run workflow) builds and tests the wheels without
publishing anything. Build artifacts are kept for one day; the release assets are permanent.

## See also

- [architecture.md](architecture.md): modules and data flow
- [evaluation.md](evaluation.md): metrics and how to read the scoreboard
- [installation.md](installation.md): installing as a user
- [`PLAN.md`](../PLAN.md): milestones
