# hedgehog: the whole picture

Read this first if you are new to the project, or coming back after a break. It explains the idea,
how the pieces fit together, where the work stands, and how to find your way around the code.
Other docs go deeper; this one connects them.

## The idea

People ask AI agents questions about spreadsheets: "how much did we spend on health in 2019?",
"which supplier won the most contracts?". Real spreadsheets make that hard. A typical government
or finance workbook has a title above the table, headers spread over two rows, subtotal rows mixed
into the data, numbers stored as text, notes at the bottom, and several tables on one sheet.

An agent that reads raw cells spends its effort working out the layout, and it often gets it wrong:
it adds the total row to the data, reads the wrong header, or picks a value from the wrong row.
Dumping the whole file into the prompt costs a fortune on big files and still leaves the model to
do arithmetic it is bad at.

hedgehog does the layout work first, in fast deterministic Rust code, with no AI involved. It finds
every table, its headers (including multi-row ones), row labels with their parent sections, total
rows, units and measures. Then it gives the agent two things:

1. **A catalog**: a short text description of the workbook, a few hundred tokens for a typical file.
   It lists every table, its columns, what one row is, which rows are totals, and any traps
   ("rows mix measures by Indicator: filter first").
2. **Exact access**: read-only SQL over the cleaned tables. Every number in an answer comes from a
   query, so the model never has to remember or retype values.

The goal is to be measurably the best spreadsheet tool an agent can use: more accurate, faster and
cheaper than giving a model raw cells, pandas or a markdown dump. The long-term ambition is a tool
good enough that a big AI lab would adopt it.

## Two projects

| Project | Where | What it is | License |
|---|---|---|---|
| **hedgehog** | `excell-lib-project/hedgehog`, public on GitHub (05vukasin/hedgehog) | The library. Rust core, plus ways to call it: a CLI, an MCP server, Python and Node packages, a Claude Code plugin. | MIT |
| **hedgehog-cli** | `excell-lib-project/hedgehog-cli`, private (05vukasin/hedgehog-cli) | A terminal chat app for asking questions about spreadsheets. A fork of Qwen Code with all coding features removed and hedgehog's tools built in. | Apache-2.0 (fork) |

hedgehog-cli uses the library in-process through the Node package. You type `hhc` in a folder with
spreadsheets and ask questions; a model (by default an OpenAI model you choose with `/model`) calls
hedgehog's tools and answers.

Test data lives outside both repos in `excell-lib-project/testdata`. See [DATA.md](DATA.md).

## How a question gets answered

Follow one question through the system, say "What were total 2019 expenses by area?" about a budget
file.

1. **The agent opens the file.** It calls the `open_file` tool with the path and, if the host passes
   it, the user's question.
2. **hedgehog reads the workbook** (`core/src/reader`). It opens xlsx, xls, xlsb, ods and csv,
   with cell formats, merged cells and number formats. Large sheets are read in parallel.
3. **It finds the tables** (`core/src/layout`, plus the learned row model in `core/src/learned`).
   It decides which cells are titles, headers, data, totals and notes, and where each table starts
   and ends. Side-by-side tables and stacked tables get split apart.
4. **It cleans each table** (`core/src/normalize`, `core/src/values`). It builds column names from
   multi-row headers ("2024 / Yuan"), keeps the full row-label path (`Region › Africa`), parses
   numbers and dates in any common locale, keeps IDs as text, and moves total rows to a separate
   copy of the table (`T1_all`) so a plain `SUM` never double counts.
5. **It loads the tables into SQLite** and writes the catalog (`agent/src`). With the question, the
   catalog puts the matching tables first and shortens the rest.
6. **The agent queries.** It sends `SELECT ...` through the `query` tool. Results come back as CSV
   with a "cite as" line pointing to source cells.
7. **hedgehog-cli checks the answer.** Before the answer reaches you, the answer check compares
   every number in it with what the tools printed. A number that appears nowhere, and that isn't
   exact arithmetic on numbers the answer states, triggers one correction round.
8. **You see the answer**, with charts drawn in the terminal if the agent made any, and a stats line
   under the input (time, tokens, cost, check result).

Steps 2 to 5 take about 13 ms for a typical file and a few seconds for a million-cell workbook.
Results are cached on disk by file hash.

## Inside the library

The Rust workspace has these packages (`Cargo.toml`):

| Folder | Package | Role |
|---|---|---|
| `core/` | `hh_core` | Reading, layout, value parsing, normalization. No network, no AI. |
| `agent/` | `hh_agent` | The agent tools as an API: `Session` holds opened files, builds catalogs, runs read-only SQL. |
| `cli/` | `hh_cli` | The `hedgehog` command: `describe`, `normalize`, `mcp` (MCP server), `benchmark`, `eval`. |
| `py/` | `hh_py` | Python package `hedgehog-sheets` (PyO3). |
| `node/` | `hh_node` | Node package `hedgehog-sheets` (napi-rs). hedgehog-cli uses this. |
| `fuzz/` | | Fuzz targets for hostile files. |
| `vendor/calamine/` | | A patched copy of the calamine Excel reader (fixes for broken files). |

The core, in reading order:

| Module | What it does |
|---|---|
| `reader/` | Opens files; resolves styles and merges; streaming CSV reader. |
| `model.rs`, `grid.rs` | Workbook, sheet and cell types; fast lookups over a sheet. |
| `layout/` | Rule-based table detection: blocks of cells, titles and notes, header depth (`tables.rs`, `blocks.rs`), stacked tables (`changepoint.rs`), side-by-side copies (`repeat.rs`). Thresholds in `params.rs`. |
| `learned/` | A small XGBoost row classifier (title, header, data, other) trained on public corpora, embedded as `core/models/row_model.bin`. It refines the rules; it never runs without them. See [learned-model.md](learned-model.md). |
| `values/` | Parsing numbers, currencies, dates, percentages and missing-value marks in many locales (`locale.rs`, `number.rs`, `date.rs`, `missing.rs`). |
| `totals/` | Finding total and subtotal rows and checking that they add up. |
| `normalize/` | Turning a detected table into a clean one: headers (`headers.rs`), rows and labels (`extract.rs`, the biggest file), column types (`typing.rs`), the manifest (`manifest.rs`) and output files (`write.rs`). |
| `eval/` | Scoring detection against labelled corpora (`hedgehog benchmark`). |

The agent layer (`agent/src`) turns normalized tables into what a model sees. Start with `lib.rs`
(the `Session`), then `catalog.rs` (the catalog text), `sql.rs` (SQLite and query hints),
`question.rs` (question-aware catalogs) and `cache.rs` (disk cache).

## Ways to use the library

- **CLI**: `hedgehog describe file.xlsx` prints what hedgehog sees; `hedgehog normalize file.xlsx -o out/ --csv`
  writes clean tables and a manifest. Reference: [cli.md](cli.md).
- **MCP server**: `hedgehog mcp --root <folder>` gives any MCP client six tools: `open_file`,
  `describe_table`, `query`, `cells`, `totals`, `list_files`. Reference: [mcp.md](mcp.md).
- **Python / Node**: same tools and normalize in-process. [python-api.md](python-api.md), [node-api.md](node-api.md).
- **Claude Code plugin**: `integrations/claude-code` wires the MCP server and a short skill into
  Claude Code. On 30 test cases Claude Code with the plugin passed 168/168 checks against 164/168
  without it, 28 % faster and 26 % cheaper.

## Inside hedgehog-cli

hedgehog-cli is an npm workspace with three packages under `packages/`:

| Package | What it holds |
|---|---|
| `analyst/` | The portable analyst harness, designed to be copied into other chat products. It imports only `zod`. Chart format and chart logic (`charts/`), the system prompt in sections (`prompt/`), the answer check (`verify/`), the SQL-backed chart tool and the Excel workbook spec (`tools/`), HTML reports (`report/`) and a reference agent loop (`loop.ts`). Read `PORTING.md` there to reuse it elsewhere. |
| `core/` | The agent engine from Qwen Code, cut down. `src/hedgehog/` has everything specific: the tools (`tools.ts`), how they reach the library (`nativeBackend.ts`, or `mcpBackend.ts` as a fallback), the chart tool, the optional Python tool (`pythonTool.ts`, `localPython.ts`), the Excel writer (`workbookTool.ts`, `xlsxWriter.ts`), the answer check wiring (`answerCheck.ts`) and the system prompt (`systemPrompt.ts`). |
| `cli/` | The terminal UI (Ink/React). Full-screen mode (`ui/layouts/FullScreenAppLayout.tsx`, `ui/components/TranscriptViewport.tsx`), the start screen with the animated logo (`ui/components/EmptyStateSplash.tsx`, `AsciiArt.ts`), terminal charts (`ui/charts/`), the stats line (`ui/hooks/usePromptStats.ts`) and slash commands such as `/copy` and `/export` (`ui/commands/`). |

Settings live in `~/.hedgehog-cli/settings.json` (it holds an API key: never print it). Useful ones:
`ui.fullScreen`, `ui.animations`, `analyst.verify`, `analyst.python` (off by default),
`analyst.files` (Excel output, asks by default).

Build and install: `npm run compile` produces `dist-bin/hedgehog-cli-linux-x64`, a single binary
with the hedgehog library embedded. Installed copies live in `~/.hedgehog-cli/bin` (`hhc` is a link).

## How we know it works

Development uses tests that need no AI model, so they are cheap, repeatable and point at the exact
cell that went wrong. Model runs happen only for official benchmark numbers, with the owner's budget.

| Suite | Command | What it checks |
|---|---|---|
| Unit tests | `cargo test`, `npm test` | each change |
| Quick tier | `make quick` (about a minute) | snapshots, golden outputs on 22 files, noise, catalog information, small public and fidelity slices |
| Cell fidelity | `make fidelity` | every source cell in about 4,000 files ends up in hedgehog's output with the same value, in a sensible place |
| Accuracy scoreboard | `make score` | on public benchmarks (HiTab, MiMoTable, RealHiTBench): can a lookup answer be found under the right labels (L), can a calculated answer be reached with a simple query (C), is anything misleading (M), how big is the catalog (K) |
| Public suite | `eval/public/score.py` | structure and reachability against pandas and MarkItDown baselines |
| Structure benchmarks | `hedgehog benchmark` | table and cell-role detection on DECO, StatSheets, Strudel, synthetic data |
| Merge check | `make merge-check AREA=core` | all of the above plus release tests and clippy, before anything merges |
| Agent evals | `eval/agents/run.py` | real model runs (costs money; official numbers only) |

The benchmarks have a frozen **holdout** part that nobody looks at while fixing. It gets scored once
per release so published numbers stay honest. See [ACCURACY_PLAN.md](ACCURACY_PLAN.md).

## Where things stand (26 Oct 2026)

On the development split of the public benchmarks, with no model:

| Measure | Value |
|---|---|
| Wrong values, dates or text handed to the agent | 0 on MiMoTable, RealHiTBench and HiTab |
| Source cells left out of every table | 0 |
| Lookups answerable under the right labels | 97.7 % after audit (127 misses left) |
| Calculations: misses caused by the library | 37 |
| Misleading-info warnings | 1,539 (1,357 are columns with no header text in the source) |
| Catalog size | about 770 to 970 tokens per file |

With a model: hedgehog-cli with gpt-4.1-mini answered 81.2 % of 149 MiMoTable questions (85.8 %
leaving out wrong answer keys), at $0.0024 and 4.5 s per question. The paper reports 79.0 % for
Claude 3.5 Sonnet. The two are not directly comparable: different subset, matcher and model.

Work is organized in numbered stages and iterations. [STAGES.md](STAGES.md) has the full ledger,
what is running now and what comes next. As of today, iterations 11 to 16 are validated but not
accepted, waiting on two decisions about the gate (a header that is blank in the source, and a
checker rule that flags age headers as data).

## Working in the code

**First time:**

1. Read [STAGES.md](STAGES.md) and [AGENT_PLAYBOOK.md](AGENT_PLAYBOOK.md). The playbook has the rules
   and the gates.
2. Run `make quick` to see the current state.
3. Open a file and look at what hedgehog does with it: `make inspect FILE=path/to/file.xlsx`.

**This laptop has limits.** It has 14 GB of RAM, and the system kills the whole terminal when swap
fills up. Run anything heavy through `../scripts/run_capped.sh <memory> <log> <command>`. The disk
is btrfs and its metadata area fills up from many small files: delete scratch output when you are
done.

**To fix something:**

1. Find the miss in `make score` or `make fidelity` output (they name the file and cell).
2. Write a small test that fails, usually a sheet built in code in `core/tests/`.
3. Fix it, run the test, then `make quick`, `make fidelity` and `make score`. Nothing that passed
   before may fail.
4. Work on a branch in its own worktree; finish with `make merge-check AREA=core`.

**Where to look for a problem:**

| Symptom | Likely place |
|---|---|
| a number or date read wrongly | `core/src/values/` |
| a table cut in the wrong place, or two tables merged | `core/src/layout/` |
| wrong column names or row labels | `core/src/normalize/headers.rs`, `extract.rs` |
| a total row counted as data | `core/src/totals/`, `normalize/extract.rs` |
| the catalog says something misleading or too long | `agent/src/catalog.rs` |
| a query fails or gives a confusing error | `agent/src/sql.rs` |
| the chat app shows something wrong | hedgehog-cli `packages/cli/src/ui/` |
| the model gets the right data but answers badly | hedgehog-cli `packages/core/src/hedgehog/systemPrompt.ts`, `packages/analyst/src/prompt/` |

## Other docs

| Doc | Read it for |
|---|---|
| [architecture.md](architecture.md) | module-by-module reference of the Rust code |
| [concepts.md](concepts.md) | the vocabulary: roles, tables, totals, measures |
| [normalized-output.md](normalized-output.md) | the clean workbook and manifest format |
| [development.md](development.md) | building, testing tiers, CI and releases |
| [ACCURACY_PLAN.md](ACCURACY_PLAN.md) | what "100 % accurate" means and how it is measured |
| [STAGES.md](STAGES.md) | what has been done, what is running, what is next |
| [AGENT_PLAYBOOK.md](AGENT_PLAYBOOK.md) | rules and gates for anyone, human or agent, changing the code |
| [DATA.md](DATA.md) | where the test data is and where it came from |
