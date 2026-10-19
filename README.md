# hedgehog

Understand messy spreadsheets: find the tables, work out what every cell and column means,
normalize the data and hand AI agents a short summary plus tools for exact, cell-traceable answers.
Rust core, Python bindings. See `PLAN.md`.

| Folder | What |
|---|---|
| `core/` | Rust library: reader, workbook model, (later) detection, normalization, summary |
| `py/` | PyO3 bindings + the `hedgehog` Python package |
| `cli/` | `hedgehog` command for development |

## Develop

```sh
cargo test --workspace --exclude hh_py      # Rust tests (fixtures are generated, no xlsx in git)
cargo run -p hh_cli -- describe file.xlsx

uv venv .venv && uv pip install --python .venv/bin/python maturin
(cd py && ../.venv/bin/maturin develop)     # add --release for the optimized build
.venv/bin/python -c "import hedgehog; print(hedgehog.open('file.xlsx').describe())"
```

Test corpora live outside the repo in `../testdata` and are never committed.
