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
`pip install "hedgehog-sheets[openai]"` ([llm.md](llm.md#install)).

## A file does not open

| Message | Cause | What to do |
|---|---|---|
| `Xlsx error: I/O error: No such file or directory` | Wrong path. | Check the path and the working directory. |
| `Cannot detect file format` | The file is a zip or OLE container that is not an xlsx, xlsm, xlsb, xls or ods workbook (a Word document, for example). The format is detected from the file's content, so a wrong extension alone is not a problem. | Export the data from the program that made the file. |
| `not a spreadsheet or delimited text file (binary content)` | The file is neither a workbook nor text: it was read as CSV (a text extension, or a name hedgehog does not know) and holds NUL bytes. | Check that it is the file you meant; export it as xlsx or CSV. |
| `Xlsx error: Zip error: invalid Zip archive` | The file is not really an xlsx: truncated, or an HTML or CSV export renamed to `.xlsx`. | Re-export the file from Excel or LibreOffice. |
| `the file is password-protected (encrypted); …` | The workbook is encrypted with a password (Excel's *Encrypt with Password*, an encrypted ODS, or an xls with a password). | Save a copy without the password and open that. |
| `file refused: the file decompresses to … (a zip bomb?)` | The package's parts add up to more than 2 GB uncompressed. | A real file this large is rare; split it, or read it with `ReadOptions::limits` raised (Rust). |
| `file refused: the workbook has more than 10,000,000 cells` | Over the cell limit (an ODS file also counts the empty cells it repeats between filled ones). | `hedgehog mcp --max-cells N` raises it for the server; Rust: `ReadOptions::limits.max_cells`. |
| `file refused: reading took longer than the time limit` / `normalizing took longer …` | The MCP server's per-file time limit (`--max-seconds`, 120 s). | Raise `--max-seconds`, or normalize the file once with `hedgehog normalize` (no time limit). |
| `Corrupt compound file: …`, `Xls error: …`, `Xml error: …` | A damaged xls, xlsx or ods file. | Open and re-save it in Excel or LibreOffice. |
| `internal error in … ; the file may be malformed` | A bug in hedgehog hit by an unusual file. The server or your program keeps running. | Please report it with the file if you can share it. |
| `TypeError: ... is not an instance of 'str'` | `hedgehog.open` was given a `pathlib.Path`. | Pass `str(path)`. `normalize` and `summarize` accept both. |

Workbooks that list a hidden sheet with an empty relationship id (a leftover of xls-to-xlsx
conversion) are repaired in memory and open normally.

## CSV and TSV files

A `.csv`, `.tsv` or `.txt` file, or a file with any other name whose content is not a workbook, is
read as delimited text: one sheet named after the file stem, one row per record (a quoted field
may span lines).

**The columns are split wrongly.** The delimiter is detected from the first 200 records: of `,`
`;` tab and `|`, the one that splits the most of them into the same number of fields wins (quotes
respected; ties go to tab, then `;`, then `|`). A `.tsv` file always uses tab. A file with very few
lines and fields that contain the other delimiters can be detected wrongly; re-export it with a
`.tsv` extension and tabs, or as xlsx.

**Accented letters look wrong (`CafÃ©`, `Caf�`).** The encoding is UTF-8 (with or without a BOM)
or UTF-16 with a BOM; a file that is not valid UTF-8 is read as Windows-1252 (Western European),
the default of Excel's "CSV" export on Western Windows. Other legacy encodings (Shift_JIS, Windows-1250
for Central European, Cyrillic code pages) come out garbled: the table structure is still right,
the text is not. Re-save the file as "CSV UTF-8" (Excel) or convert it, for example
`iconv -f CP1250 -t UTF-8 in.csv > out.csv`.

**Numbers stay text in `hedgehog.open`.** Only plain, unambiguous numbers (`42`, `-3.5`, `1e6`) are
stored as numbers when the file is read. `1.200,50`, `1,200.50`, `12%`, `007`, dates and `1.200`
(1200 or 1.2?) are kept as text, and `normalize` reads them per column like numbers stored as text
in a workbook ([above](#numbers-or-dates-are-read-the-wrong-way)); a leading-zero code such as
`007` stays text. Integers with more than 15 digits (IDs, account numbers) stay text so that no
digit is lost.

**A total row is not checked.** The totals check reads stored numbers only, as it does for numbers
stored as text in a workbook. In a CSV with decimal commas or thousands separators (`4.471,66`), total
rows are still recognized and removed from the table (Notes lists them), but they are not
re-calculated.

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
