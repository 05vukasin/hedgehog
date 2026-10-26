# Command line

Reference for the `hedgehog` command: every subcommand, flag, environment variable and exit code.

The CLI is a thin wrapper over the Rust core. It is built from the `cli/` crate
([installation.md](installation.md#the-hedgehog-command)); the Python wheel does not install it.

```
hedgehog describe <file>
hedgehog cells <file> [--sheet NAME]
hedgehog normalize <file> -o <out_dir> [--keep-totals] [--csv] [--locale TAG]
hedgehog mcp [--root DIR] [--allow PATH]... [--query-timeout-ms N] [--max-file-mb N]
hedgehog eval deco [--split dev|test|all] [--limit N] [--analyzer NAME] [--json OUT.json] [--no-json] [--progress] [--threads N]
hedgehog bench <dir|filelist> [--limit N] [--threads N] [--top K] [--open-only]
hedgehog bench --synthetic [--case NAME]
hedgehog golden <dir|filelist> [--limit N] [--check GOLDEN.txt]
```

`describe`, `normalize` and `mcp` are for users; `cells`, `eval`, `bench` and `golden` are development tools.

Input files can be `.xlsx`, `.xlsm`, `.xlsb`, `.xls` or `.ods`, or delimited text: `.csv`, `.tsv`, `.txt`
or any other name whose content is not a workbook. A CSV is one sheet named after the file stem; the
delimiter (`,` `;` tab `|`) and the encoding (UTF-8, UTF-16 with a BOM, else Windows-1252) are
detected ([troubleshooting](troubleshooting.md#csv-and-tsv-files)). Examples on this page use `sales.xlsx`
from the [quickstart](quickstart.md#a-sample-messy-workbook).

## `hedgehog describe`

One line per sheet: name, used range, number of non-empty cells, formulas and merged ranges.

```console
$ hedgehog describe sales.xlsx
Sales: A1:E15 · 39 cells · 0 formulas · 4 merged ranges
```

Use it to check that a file opens and to see how big it is before normalizing. An empty sheet shows
`empty` instead of a range.

## `hedgehog normalize`

Writes the normalized workbook and manifest for one input file.

```console
$ hedgehog normalize sales.xlsx -o out/
sales.xlsx: 1 tables, 39 cells (28 in tables, 9 totals, 2 notes) in 16 ms
  out/sales.normalized.xlsx
  out/sales.manifest.json
```

| Argument | Required | Meaning |
|---|---|---|
| `<file>` | yes | Input workbook. Only read, never modified. |
| `-o <dir>`, `--out <dir>` | yes | Output directory, created if missing. Files with the same names are overwritten. |
| `--keep-totals` | no | Keep total and subtotal rows in the table sheets, marked in a `_row_type` column (`data`, `subtotal`, `total`; `average` and `of_which` for average and "of which" rows). Default: they are removed from the data and listed only on the `Totals` sheet. |
| `--csv` | no | Also write `<stem>.<table sheet>.csv` per table (UTF-8, ISO dates, empty field = missing). |
| `--locale TAG` | no | Fix the decimal separator and day/month order for every column instead of inferring them per column. `TAG` is a preset or a language tag: `us`, `en-US`, `uk`, `en-GB`, `de`, `de-AT`, `fr`, `es`, `it`, `nl`, `pt`, `pt-BR`, `pl`, `ch`, `de-CH`, `fr-CH`, `sr`, `hr`, `bs`, `iso`, ... See [locales.md](locales.md#locale-presets). |
| `--rules-only` | no | Label cells with the layout rules alone, without the built-in learned model ([learned-model.md](learned-model.md)). |
| `--model-max-cells N` | no | Sheets with more than `N` cells use the rules alone (default 1,000,000; the model's features take about 440 bytes per cell). |

The summary line counts every non-empty source cell once: in a table, in a total row (`Totals`
sheet), or elsewhere (`Notes` sheet). With `--keep-totals` the total rows stay in the table and
are counted under "in tables". The time includes reading, analysis and writing.

With all options:

```console
$ hedgehog normalize sales.xlsx -o out_keep/ --keep-totals --csv --locale en-US
sales.xlsx: 1 tables, 39 cells (37 in tables, 0 totals, 2 notes) in 18 ms
  out_keep/sales.normalized.xlsx
  out_keep/sales.manifest.json
  out_keep/sales.T1_sales_report_2024.csv
$ head -4 out_keep/sales.T1_sales_report_2024.csv
Region,Product,Revenue / Q1,Revenue / Q2,Units,Margin,_src_row,_row_type
North,Widgets,1200.5,1350,1200,0.12,6,data
North,Gadgets,300,410,350,0.085,7,data
North,Subtotal,1500.5,1760,,,8,subtotal
```

The output layout and manifest schema are described in [normalized-output.md](normalized-output.md).

Normalizing many files is a shell loop; each run is independent:

```console
$ for f in *.xlsx; do hedgehog normalize "$f" -o normalized/ > /dev/null || echo "failed: $f"; done
$ ls normalized/ | head -3
budget.manifest.json
budget.normalized.xlsx
orders.manifest.json
```

## `hedgehog mcp`

Runs an MCP (Model Context Protocol) tool server over stdio for AI clients: `open_file`,
`describe_table`, `query` (read-only SQL), `cells`, `totals` and `list_files`. It reads files under
`--root` (default: the current folder) plus `--allow` paths, normalizes them in memory and writes
nothing. JSON-RPC goes to stdout, one log line per call (time, memory) to stderr.

```console
$ hedgehog mcp --root ~/spreadsheets
[hedgehog mcp] serving tools over stdio; root /home/me/spreadsheets
```

| Flag | Default | Meaning |
|---|---|---|
| `--root DIR` | current folder | Folder the tools may read; paths are relative to it. |
| `--allow PATH` | none | Another file or folder outside the root (repeatable). |
| `--query-timeout-ms N` | `2000` | Time limit of one SQL query. |
| `--max-file-mb N` | `256` | Larger files are refused. |

Tools, output format, security limits and client configuration: [mcp.md](mcp.md).

## `hedgehog-inspect` (Python)

Shows, file by file, what hedgehog makes of a spreadsheet and what an AI agent would receive:
overview, every table (columns, types, first rows), the totals check, notes, warnings, the files
and loading code an agent gets (with the manifest's size in tokens), and the summary. Installed
with the Python package (`python -m hedgehog.inspect` works too).

```console
$ hedgehog-inspect sales.xlsx                       # everything; free deterministic summary
$ hedgehog-inspect sales.xlsx --llm                 # summary phrased by the LLM configured in .env
$ hedgehog-inspect sales.xlsx --ask "Total Q2 revenue?"   # an LLM agent answers with the workbook tools
$ hedgehog-inspect sales.xlsx --show tables,totals --rows 10
$ hedgehog-inspect sales.xlsx --compare             # rules + model vs rules alone
$ hedgehog-inspect sales.xlsx --show manifest       # the full manifest JSON
$ hedgehog-inspect reports/                         # one line per file
```

Options: `--show` (sections `overview,tables,totals,notes,warnings,agent,summary,manifest` or
`all`), `--rows N`, `--tables N|all`, `--llm`, `--ask Q`, `--provider`, `--model`, `--env PATH`
(default: the nearest `.env`; without `AI_PROVIDER` the first provider with a key is used),
`--rules-only`, `--compare`, `--keep-totals`, `--locale`, `--model-max-cells`, `--out DIR`,
`--open`, `--full` (several files in full), `--no-color`. In a clone, the `Makefile` wraps it:
`make inspect FILE=…`, `make llm FILE=…`, `make ask FILE=… Q="…"`, `make compare FILE=…`,
`make batch DIR=…` (`make help` lists them).

## `hedgehog benchmark`

Scores an analyzer on every labelled corpus under `$HEDGEHOG_DATA` (DECO, StatSheets, Strudel,
synthetic), per source and overall, in the full seven-role space and in the four roles all corpora
share. Used to check that the model stays better than the rules everywhere.

```console
$ hedgehog benchmark --analyzer model --split test
$ hedgehog benchmark --analyzer layout --corpora deco,strudel --out scores.tsv
```

| Argument | Meaning |
|---|---|
| `--analyzer NAME` | `baseline`, `layout` (rules) or `model` (rules + learned model; `$HEDGEHOG_MODEL` = a folder with `coarse.json` and `fine.json` replaces the built-in one). |
| `--corpora LIST` | Comma-separated subset of `deco,statsheets,strudel,synthetic` (the default), plus `eu`: the hand-labelled [European open-data sample](evaluation.md#european-open-data-sample), always scored whole. |
| `--split S` | `test` (default; files the model never trained on), `dev` or `all`. |
| `--threads N`, `--out FILE.tsv` | Worker threads; also write the per-source scores as TSV. |

`hedgehog cells <file> [--sheet NAME]` prints every non-empty cell the reader sees as TSV
(`sheet row col a1 value`, 0-based row and column), the coordinates labelled corpora use.
`hedgehog features` exports training data (`--level cell` or `--level row`), and
`hedgehog pack-model <dir>` turns `row_coarse.json` + `row_fine.json` into the embedded
`row_model.bin`; see [learned-model.md](learned-model.md#retraining).

## `hedgehog eval`

Scores table detection and cell-role labelling against the annotated DECO corpus. This is a
development command: it needs the corpus on disk (see [evaluation.md](evaluation.md)).

```sh
HEDGEHOG_DATA=/path/to/testdata hedgehog eval deco --split dev --analyzer layout
```

| Flag | Default | Meaning |
|---|---|---|
| `--split dev\|test\|all` | `dev` | Which part of the corpus to score. The 70/30 dev/test split is fixed by a hash of the file name. |
| `--limit N` | all | Score only the first `N` files of the split. |
| `--analyzer NAME` | `baseline` | `baseline` (8-connected flood fill, the number to beat) or `layout` (the rule-based detector used by `normalize`). |
| `--json OUT.json` | see below | Where to write the full JSON report. |
| `--no-json` | off | Print the scoreboard only. |
| `--progress` | off | Print progress while scoring. |
| `--threads N` | min(4, CPUs) | Files are scored on `N` worker threads. The report is identical to a single-threaded run. |

Environment variables:

| Variable | Meaning |
|---|---|
| `HEDGEHOG_DATA` | Root of the test corpora. DECO is read from `$HEDGEHOG_DATA/irregular/deco/` and `$HEDGEHOG_DATA/irregular/enron/`. Set it explicitly: the built-in default is a path on the original development machine. |
| `HEDGEHOG_SCORES` | Directory for the JSON report when `--json` is not given. Default: `<data root>/../docs/research/scores/`. The file is named `<analyzer>_deco_<split>[_n<limit>].json`. |

The scoreboard format is shown in [evaluation.md](evaluation.md#reading-the-scoreboard).

## `hedgehog bench`

Throughput and per-stage timing of the analysis pipeline (read, grid, context, values, totals,
total detection and verification, baseline layout) over a directory of workbooks or a text file
with one path per line. A development tool for performance work.

| Flag | Default | Meaning |
|---|---|---|
| `<dir\|filelist>` | | Directory (all workbooks in it) or a file listing paths. |
| `--limit N` | all | Only the first `N` files. |
| `--threads N` | 1 | Process files on `N` threads. |
| `--top K` | 5 | How many of the slowest files to list. |
| `--open-only` | off | Time only reading the files. |
| `--synthetic [--case NAME]` | | Run on generated worst-case sheets (dense, sparse, giant merges, a million rows, ...) instead of files. |

Output for a directory with the three sample workbooks (debug build; timings vary):

```
$ hedgehog bench . --top 2
hedgehog bench · 3 files (3 ok, 0 failed) · 1 threads · 3 sheets · 63 cells · 4 merges · 0 formulas
wall 0.01s · 330.61 files/s · 6943 cells/s · peak RSS 11 MB
stage            cpu s   share
  open             0.004   48.2%
  grid             0.000    0.3%
  context          0.001    6.4%
  values           0.003   36.5%
  totals           0.000    4.5%
  detect_verify    0.000    2.7%
  baseline         0.000    1.5%
  sum              0.009
slowest files:
    0.004s       39 cells  sales.xlsx
    0.003s       12 cells  budget.xlsx
```

Files that fail to open are listed at the end and do not stop the run.

## `hedgehog golden`

Prints one hash per file over every output of every analysis stage (cells, merges, feature columns,
totals and the baseline layout), or with `--check GOLDEN.txt` compares against a saved list. Used to
prove that a performance change did not change any result.

## Exit codes

| Code | When |
|---|---|
| `0` | Success. |
| `1` | The command ran but failed: the file could not be opened or read, an unknown flag or locale, a missing `-o`, an unwritable output directory, or a missing corpus for `eval`. The error is printed to stderr, followed by the usage text for every command except `describe`. |
| `2` | No or unknown subcommand. The usage text is printed to stderr. |

```console
$ hedgehog normalize sales.xlsx -o out/ --locale xx-YY 2> /dev/null; echo "exit $?"
exit 1
$ hedgehog describe missing.xlsx; echo "exit $?"
missing.xlsx: Xlsx error: I/O error: No such file or directory (os error 2)
exit 1
$ hedgehog frobnicate 2> /dev/null; echo "exit $?"
exit 2
```

## See also

- [quickstart.md](quickstart.md): a walkthrough with the sample workbook
- [normalized-output.md](normalized-output.md): what `normalize` writes
- [python-api.md](python-api.md): the same features from Python
- [troubleshooting.md](troubleshooting.md): common errors
