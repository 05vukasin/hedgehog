# hedgehog documentation

hedgehog reads messy spreadsheets and gives AI agents and programs clean, exact tables. These pages
take you from "what is this" to every detail. Read the first three in order; after that, pick what you
need.

## Start here

1. [What hedgehog is](introduction.md): the problem, what you get back, how a question gets answered.
2. [Installation](installation.md): the Python package, the Node package or the `hedgehog` command.
3. [Quickstart](quickstart.md): a messy sample workbook, cleaned and loaded with pandas.
4. [Concepts](concepts.md): tables, cell roles, headers, row labels, values, totals, the catalog,
   one idea at a time.

## Use it

| Page | When you want to |
|---|---|
| [MCP server and Claude Code plugin](mcp.md) | let an AI agent ask questions about spreadsheets: the tools, the catalog, the SQL views |
| [Agent recipes](agent-recipes.md) | give an agent with a code sandbox the clean workbook: pandas and openpyxl code |
| [Normalized output](normalized-output.md) | know every sheet, column and manifest field of the clean workbook |
| [Locales](locales.md) | read US and European numbers, dates and currencies, and see how doubtful values are handled |
| [LLMs](llm.md) | have a language model write a summary (optional) |
| [Troubleshooting](troubleshooting.md) | fix a file that doesn't open, a misread value, a missing table |

## Reference

| Page | What it covers |
|---|---|
| [Command line](cli.md) | every `hedgehog` command, flag and exit code |
| [Python API](python-api.md) | every public function, class and result field |
| [Node.js API](node-api.md) | the npm package `hedgehog-sheets`, with the agent tools in-process |
| [Detection rules](detection-rules.md) | the exact rules behind tables, roles, headers and totals |

## How it works and how well

| Page | What it covers |
|---|---|
| [Architecture](architecture.md) | the Rust modules, the bindings and how data flows through them |
| [The learned model](learned-model.md) | the built-in row model: how it works with the rules, accuracy, retraining |
| [Evaluation](evaluation.md) | the benchmarks, the accuracy measurements and how to run them |

## Contributing

[Development](development.md): building, testing, test data, CI and releases.

Also: [project README](../README.md) · [CHANGELOG](../CHANGELOG.md)
