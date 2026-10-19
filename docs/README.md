# hedgehog documentation

hedgehog turns messy Excel workbooks into a clean, typed workbook plus a machine-readable manifest
that an AI agent with a code sandbox can load with pandas or openpyxl. This folder explains every
way to use it.

New here? Read [installation](installation.md), then the [quickstart](quickstart.md).

## Guides

| Page | What it covers |
|---|---|
| [installation.md](installation.md) | Prebuilt wheels, optional LLM extras, the `hedgehog` command, building from source |
| [quickstart.md](quickstart.md) | From a messy sample workbook to a clean pandas DataFrame, CLI and Python |
| [agent-recipes.md](agent-recipes.md) | pandas and openpyxl code for sandbox agents: load tables, apply dtypes, find wrong totals, trace values, write changes back |
| [llm.md](llm.md) | Optional LLM layer: providers, API keys, summaries, the tool loop, model benchmarks, cost |
| [locales.md](locales.md) | US and European numbers, dates, currencies, missing values; ambiguity and locale presets |
| [troubleshooting.md](troubleshooting.md) | Files that do not open, misread values, missing tables, LLM errors |

## Reference

| Page | What it covers |
|---|---|
| [cli.md](cli.md) | Every `hedgehog` subcommand, flag, environment variable and exit code |
| [python-api.md](python-api.md) | Every public Python function, class, method and result field |
| [normalized-output.md](normalized-output.md) | The normalized workbook's sheets and the manifest schema |

## Background

| Page | What it covers |
|---|---|
| [concepts.md](concepts.md) | Tables, cell roles, headers, totals and verification, provenance |
| [evaluation.md](evaluation.md) | The DECO benchmark, metrics, current scores, how to run `hedgehog eval` |
| [architecture.md](architecture.md) | Rust core modules, PyO3 bindings, Python layer, data flow |
| [development.md](development.md) | Building, testing, test data, CI and the release process |
| [PERF.md](PERF.md) | Performance work: benchmarks, hotspots found, optimizations (engineering notes) |

[NORMALIZE.md](NORMALIZE.md) is an old path that points to normalized-output.md.

## See also

- [Project README](../README.md): overview, status and roadmap
- [CHANGELOG.md](../CHANGELOG.md): what changed in each release
- [PLAN.md](../PLAN.md): the milestone plan
