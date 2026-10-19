# Troubleshooting

Common errors and surprising results, what causes them, and what to do.

Contents: [Installation](#installation) · [A file does not open](#a-file-does-not-open) ·
[Numbers or dates are read the wrong way](#numbers-or-dates-are-read-the-wrong-way) ·
[A table is missing or split](#a-table-is-missing-or-split) ·
[A total row stayed in the data](#a-total-row-stayed-in-the-data) ·
[Totals show unknown](#totals-show-unknown) · [LLM calls fail](#llm-calls-fail) ·
[Getting help](#getting-help)

## Installation

**`pip install hedgehog` installs something else.** The distribution is called `hedgehog-sheets`;
the import name is `hedgehog`. Run `pip uninstall hedgehog` if you installed the wrong one.

**pip tries to compile Rust.** No prebuilt wheel matched your platform or Python, so pip fell back
to the source distribution. Wheels exist for Linux x86_64 and aarch64, macOS (Intel and Apple
Silicon) and Windows x64, for CPython 3.9 and newer. On other platforms install a Rust toolchain
(see [installation.md](installation.md#from-source)), or use a supported Python.

**`hedgehog: command not found`.** The Python package does not install the CLI. Build it from
source: [installation.md](installation.md#the-hedgehog-command).

**`ImportError` from `NormalizeResult.read`.** It needs `pandas` and `openpyxl`:
`pip install pandas openpyxl`.

**`MissingDependency: The ... provider needs ...`.** Install the provider extra, for example
`pip install "hedgehog-sheets[openai]"` ([llm.md](llm.md#install)). The message may say
`hedgehog[openai]`; the right package name is `hedgehog-sheets[openai]`.

## A file does not open

| Message | Cause | What to do |
|---|---|---|
| `Xlsx error: I/O error: No such file or directory` | Wrong path. | Check the path and the working directory. |
| `Cannot detect file format` | The extension is not `.xlsx`, `.xlsm`, `.xlsb`, `.xls` or `.ods` (for example a `.csv`). | Convert CSV files to xlsx first, or load them with pandas directly. |
| `Xlsx error: Zip error: invalid Zip archive` | The file is not really an xlsx: truncated, an HTML or CSV export renamed to `.xlsx`, or password-protected (encrypted files are not zip archives). | Re-export the file from Excel or LibreOffice; remove the password. |
| `TypeError: ... is not an instance of 'str'` | `hedgehog.open` was given a `pathlib.Path`. | Pass `str(path)`. `normalize` and `summarize` accept both. |

Workbooks that list a hidden sheet with an empty relationship id (a leftover of xls-to-xlsx
conversion) are repaired in memory and open normally.

## Numbers or dates are read the wrong way

Look at the column in the manifest: `locale` shows the decimal separator and date order used, and
`locale.ambiguous` counts values that were read by a guess. The table's `warnings` say so too:

```text
column 'Weight': 3 value(s) read with a guessed decimal separator or day/month order (pass a locale to override)
```

This happens when every value in a column can be read two ways (`1.200`, `03/04/2024`) and the rest of
the sheet gives no evidence. Pass the file's locale:

```sh
hedgehog normalize orders.xlsx -o out/ --locale de-DE
```

or `hedgehog.normalize(path, out, locale="de-DE")`. Details and the list of presets:
[locales.md](locales.md#ambiguous-values).

A percentage stored as a number (0.12 shown as `12%` by a cell format) stays a `number`, because cell
formats are not read yet.

## A table is missing or split

Table detection is rule-based and not perfect ([evaluation.md](evaluation.md) has the numbers). If a
table is missing, its cells are on the `Notes` sheet with kind `other` or `title`, so nothing is lost.
Check `manifest["cells"]`: `in_notes` much larger than expected means a table was not recognized.
Common causes: a table without a header row, a table made of text only, or a layout where one table
continues across a wide gap. Please report such files (see below).

## A total row stayed in the data

A total row is moved to `Totals` when its value is a `SUM`, `SUBTOTAL` or `A+B` formula, or when its
label is a total word the totals module knows (`Total`, `Subtotal`, `Grand total`, `Sum`, `Ukupno`,
...). A typed number in a row labelled in another language (`Summe`, `Gesamt`, `Totale`) is not yet
recognized and stays as a data row; so does a total over a column of numbers stored as text. Before
summing a column, check that no row label looks like a total:

```python
import json
import re
import pandas as pd

m = json.load(open("out/sales.manifest.json", encoding="utf-8"))
t = m["tables"][0]
df = pd.read_excel("out/sales.normalized.xlsx", sheet_name=t["sheet"])
words = r"(?i)\b(?:total|subtotal|sum|summe|gesamt|totale|totaal|razem)\b"
text_cols = [c["name"] for c in t["columns"] if c["dtype"] == "text"]
suspects = df[df[text_cols].apply(lambda row: row.astype(str).str.contains(words).any(), axis=1)]
print(len(suspects), "rows look like totals")
```

```text
0 rows look like totals
```

If a row shows up, drop it before aggregating and check the `Totals` sheet.

## Totals show unknown

`status` `unknown` means the total could not be checked, not that it is wrong. The `reason` column
says why:

| reason | Typical cause |
|---|---|
| `no_cached_value` | The file was written by a script (openpyxl, pandas) and never opened in Excel, so formulas have no saved results. Open and save it in Excel or LibreOffice (`soffice --headless --convert-to xlsx`) to store them. |
| `missing_source_value` | Same, for a formula inside the source range. |
| `unconfirmed` | A typed total disagrees with the guessed source range, and nothing confirmed the guess. |
| `not_numeric`, `error_in_source` | The total or a source cell holds text or an error. |
| `matches_average` | The value equals the average of the range: probably an average row, not a sum. |

## LLM calls fail

| Symptom | Cause | What to do |
|---|---|---|
| `ConfigError: openai needs an API key` | No key in the argument, environment or `.env`. | `llm.connect("openai", api_key=...)`, set `OPENAI_API_KEY`, or pass `env_file=".env"`. A `.env` file is never read unless you ask. |
| `ConfigError: no LLM provider given` | `connect()` without a provider, no `AI_PROVIDER`, and zero or several presets configured. | Name the provider: `llm.connect("anthropic")`. |
| `ConfigError: ... needs a model` | OpenAI-compatible servers, OpenRouter, Groq and Bedrock have no default model. | Pass `model=` or set the preset's `*_MODEL` variable. |
| HTTP 429 with `insufficient_quota` / `credit_balance_exhausted` | The account is out of credits. Not retried, because waiting does not help. | Add credits or switch provider. `summarize` returns the deterministic summary with `llm_error` set. |
| HTTP 429 rate limit, 5xx, timeouts | Temporary. Retried up to `max_retries` (3) with backoff and `Retry-After`. | Lower concurrency: `max_concurrency=1` or `AI_MAX_CONCURRENT_LLM=1`; raise `max_retries`. |
| Summary has `warnings` about citations | The model cited a wrong cell or wrote numbers without a citation. | Expected with small models; the warnings are the point. Use `s.deterministic` when you need exact text. |
| `summarize` result has `source == "deterministic"` although `llm=` was given | The call failed or returned nothing; see `s.llm_error`. | Fix the cause above. |

Details: [llm.md](llm.md#retries-rate-limits-and-quotas).

## Getting help

Open an issue at <https://github.com/05vukasin/hedgehog/issues> with the hedgehog version
(`pip show hedgehog-sheets`), the command or code you ran, and the full error. If a file is
misread, a small anonymized workbook that shows the problem helps most.

## See also

- [installation.md](installation.md)
- [locales.md](locales.md)
- [normalized-output.md](normalized-output.md#known-limits): known limits of the output
- [llm.md](llm.md)
