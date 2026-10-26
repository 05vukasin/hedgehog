# hedgehog documentation

hedgehog turns messy Excel workbooks into a clean, typed workbook plus a machine-readable manifest
that an AI agent with a code sandbox can load with pandas or openpyxl. This folder explains every
way to use it.

New here? Start with [OVERVIEW.md](OVERVIEW.md) (the whole picture: idea, how it works, where things stand, how to work in the code), then [installation](installation.md) and the [quickstart](quickstart.md).

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
| [node-api.md](node-api.md) | The npm package `hedgehog-sheets`: install, TypeScript API, agent tools in-process, mapping to Python |
| [normalized-output.md](normalized-output.md) | The normalized workbook's sheets and the manifest schema |

## Background

| Page | What it covers |
|---|---|
| [concepts.md](concepts.md) | Tables, cell roles, headers, totals and verification, provenance |
| [learned-model.md](learned-model.md) | The built-in learned row model: how it works with the rules, accuracy, speed, retraining |
| [evaluation.md](evaluation.md) | Benchmarks (DECO, StatSheets, Strudel, synthetic), metrics, current scores, how to run them |
| [architecture.md](architecture.md) | Rust core modules, PyO3 bindings, Python layer, data flow |
| [development.md](development.md) | Building, testing, test data, CI and the release process |
| [PERF.md](PERF.md) | Performance work: benchmarks, hotspots found, optimizations (engineering notes) |

[NORMALIZE.md](NORMALIZE.md) is an old path that points to normalized-output.md.

## See also

- [Project README](../README.md): overview, status and roadmap
- [CHANGELOG.md](../CHANGELOG.md): what changed in each release
- [PLAN.md](../PLAN.md): the milestone plan
