# Python API

Reference for the `hedgehog` Python package: every public function, class, method and result field,
with signatures, types and examples.

```python
import hedgehog
print(hedgehog.__all__)
```

```text
['NormalizeResult', 'Summary', 'Workbook', 'check_citations', 'facts', 'facts_json', 'normalize', 'open', 'summarize']
```

The optional LLM layer lives in `hedgehog.llm` and is documented in [llm.md](llm.md). Examples use
`sales.xlsx` from the [quickstart](quickstart.md#a-sample-messy-workbook).

Contents: [`normalize`](#hedgehognormalize) · [`NormalizeResult`](#normalizeresult) ·
[`open`](#hedgehogopen) · [`Workbook`](#workbook) · [`summarize`](#hedgehogsummarize) ·
[`Summary`](#summary) · [`facts`](#hedgehogfacts) · [`check_citations`](#hedgehogcheck_citations) ·
[Errors](#errors) · [Conventions](#conventions)

## `hedgehog.normalize`

```
normalize(path, out_dir, *, keep_totals=False, csv=False, locale=None) -> NormalizeResult
```

Writes `<stem>.normalized.xlsx` and `<stem>.manifest.json` (and CSVs with `csv=True`) into `out_dir`,
which is created if missing. The source file is never modified. The work runs in Rust with the GIL
released, so several files can be normalized from threads.

| Parameter | Type | Meaning |
|---|---|---|
| `path` | `str` or `os.PathLike` | Input workbook (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.ods`). |
| `out_dir` | `str` or `os.PathLike` | Output directory. |
| `keep_totals` | `bool` | Keep total rows in the table sheets with a `_row_type` column. |
| `csv` | `bool` | Also write one CSV per table. |
| `locale` | `str` or `None` | Locale preset or tag such as `"de-DE"`, `"en-US"`, `"fr"`, `"iso"`. `None` infers per column. See [locales.md](locales.md). |

```python
import hedgehog

r = hedgehog.normalize("sales.xlsx", "out/")
print(r.xlsx_path)
print(r.manifest_path)
print(r.manifest["totals"]["mismatch"], "mismatched total(s)")
```

```text
out/sales.normalized.xlsx
out/sales.manifest.json
1 mismatched total(s)
```

### `NormalizeResult`

A dataclass returned by `normalize`.

| Member | Type | Meaning |
|---|---|---|
| `xlsx_path` | `str` | Path of the normalized workbook. |
| `manifest_path` | `str` | Path of the manifest JSON. |
| `manifest` | `dict` | The parsed manifest ([schema](normalized-output.md#manifest)). |
| `csv_paths` | `list[str]` | CSV files written (empty unless `csv=True`). |
| `tables` | `list[dict]` (property) | `manifest["tables"]`: one entry per table sheet. |
| `table(key)` | `dict` | One table entry by id (`"T1"`) or output sheet name (`"T1_sales_report_2024"`). Raises `KeyError` with the list of sheets if there is no such table. |
| `read(key)` | `pandas.DataFrame` | The table sheet loaded with `pandas.read_excel`. Needs `pandas` and `openpyxl`. |

```python
import hedgehog

r = hedgehog.normalize("sales.xlsx", "out/", keep_totals=True, csv=True)
print(r.csv_paths)
df = r.read("T1")
print(df[["Product", "Revenue / Q1", "_src_row", "_row_type"]])
```

```text
['out/sales.T1_sales_report_2024.csv']
       Product  Revenue / Q1  _src_row _row_type
0      Widgets        1200.5         6      data
1      Gadgets         300.0         7      data
2     Subtotal        1500.5         8  subtotal
3      Widgets         980.0        10      data
4       Gizmos        2045.0        11      data
5     Subtotal        3025.0        12  subtotal
6  Grand total        4600.0        13     total
```

## `hedgehog.open`

```
open(path: str) -> Workbook
```

Reads a workbook into memory: values, formulas (with their cached results) and merged ranges of every
sheet. Raises `ValueError` if the file cannot be read. `path` must be a `str`; pass `str(p)` for a
`pathlib.Path`.

## `Workbook`

Returned by `open`. Read-only: nothing is ever written back to the file. Every method that takes a
`sheet` matches the name exactly first, then case-insensitively, and raises `KeyError` (listing the
sheet names) if neither matches.

| Member | Returns | Meaning |
|---|---|---|
| `sheet_names` | `list[str]` (property) | Sheet names in workbook order. |
| `describe()` | `str` | The `hedgehog describe` text: one line per sheet. |
| `sheets()` | `list[dict]` | Per sheet: `name`, `used_range` (`"A1:E15"` or `None`), `rows`, `cols`, `cells`, `formulas`, `merges`. |
| `get_cell(sheet, address)` | `dict` | One cell, see below. |
| `get_range(sheet, range, max_cells=5000)` | `dict` | `{sheet, range, rows}`: values row by row, `None` for empty cells. |
| `find_text(query, sheet=None, limit=50)` | `list[dict]` | Cells whose text contains `query`, as cell dicts. |
| `totals(sheet)` | `list[dict]` | Detected totals of one sheet with their verification, see below. |

```python
import hedgehog

wb = hedgehog.open("sales.xlsx")
print(wb)
print(wb.sheets())
```

```text
Workbook(sheets=["Sales"])
[{'name': 'Sales', 'used_range': 'A1:E15', 'rows': 15, 'cols': 5, 'cells': 39, 'formulas': 0, 'merges': 4}]
```

### `get_cell`

```
get_cell(sheet: str | None, address: str) -> dict
```

`address` is an A1 reference (`"D43"`, `"$D$43"`) and may name the sheet itself (`"Sales!B13"`,
`"'My sheet'!B2"`); a sheet in the address wins over the `sheet` argument. With `sheet=None` and no
sheet in the address, the first sheet is used. The result:

| Key | Meaning |
|---|---|
| `sheet`, `address` | Where the cell is. |
| `value` | `float`, `str`, `bool`, an ISO date string (`"2024-03-31"`) for dates, an error string such as `"#N/A"`, or `None` for an empty cell or a formula without a cached value. |
| `type` | `"number"`, `"text"`, `"bool"`, `"date"`, `"error"` or `"empty"`. |
| `formula` | The formula with a leading `=`, or `None`. |

```python
import hedgehog

wb = hedgehog.open("sales.xlsx")
print(wb.get_cell("Sales", "B13"))
print(wb.get_cell(None, "Sales!E6"))
```

```text
{'sheet': 'Sales', 'address': 'B13', 'value': 4600.0, 'type': 'number', 'formula': None}
{'sheet': 'Sales', 'address': 'E6', 'value': '12%', 'type': 'text', 'formula': None}
```

`Workbook` returns values as stored in the source. Text such as `"12%"` is parsed only by
`normalize`.

### `get_range`

```
get_range(sheet: str | None, range: str, max_cells: int = 5000) -> dict
```

`range` is `"A1:F20"` or a single cell, optionally with a sheet name. Ranges larger than `max_cells`
raise `ValueError`, so a caller cannot dump a huge sheet by accident.

```python
import hedgehog

wb = hedgehog.open("sales.xlsx")
print(wb.get_range("Sales", "A5:C8"))
```

```text
{'sheet': 'Sales', 'range': 'A5:C8', 'rows': [['Region: North', None, None], ['Widgets', 1200.5, 1350.0], ['Gadgets', 300.0, 410.0], ['Subtotal', 1500.5, 1760.0]]}
```

### `find_text`

```
find_text(query: str, sheet: str | None = None, limit: int = 50) -> list[dict]
```

Case-, accent- and script-insensitive substring search over text cells (`"region"` finds
`Region: North`, `"beograd"` finds `Београд`). A numeric query also matches number cells whose
value contains it. Hits come in sheet order, row by row, at most `limit`.

```python
import hedgehog

wb = hedgehog.open("sales.xlsx")
for hit in wb.find_text("REGION"):
    print(hit["address"], hit["value"])
```

```text
A5 Region: North
A9 Region: South
```

### `totals`

```
totals(sheet: str) -> list[dict]
```

Every total and subtotal cell found on the sheet, recomputed from the cached values of its source
range. See [concepts.md](concepts.md#totals-and-verification) for how totals are found.

| Key | Meaning |
|---|---|
| `sheet`, `address` | The total cell. |
| `origin` | `"formula"` or `"typed"`. |
| `level` | `"total"`, `"subtotal"` or `"grand_total"`. |
| `direction` | `"vertical"` (adds cells above) or `"horizontal"` (adds cells to the left). |
| `sources` | Source ranges, such as `["B6:B12"]`. |
| `excluded` | Cells inside `sources` that are not added (subtotal rows under a grand total, nested `SUBTOTAL`s). |
| `double_counted` | Subtotals inside the source that the total adds again on top of their data. |
| `label`, `label_address` | The label text and its cell, if any. |
| `formula` | The total's formula, or `None`. |
| `evidence` | Why it was taken for a total, such as `"keyword:grand_total"`, `"formula:sum"`, `"sums_totals"`, `"siblings_match"`. |
| `actual` | The value in the file. |
| `expected` | The recomputed sum (without double-counted totals), or `None`. |
| `diff` | `actual - expected`, or `None`. |
| `ok` | `True` (verified), `False` (wrong) or `None` (could not be checked). |
| `reason` | `"match"`, `"rounded"`, `"mismatch"`, `"double_counted"`, `"matches_average"`, `"no_cached_value"`, `"not_numeric"`, `"error_in_source"`, `"missing_source_value"`, `"no_source"`, `"unconfirmed"`. |
| `summed` | How many numeric source cells were added. |
| `numbers_as_text` | Source cells holding numbers stored as text, which Excel's `SUM` skips. |

```python
import hedgehog

wb = hedgehog.open("sales.xlsx")
for t in wb.totals("Sales"):
    print(t["address"], t["label"], t["actual"], t["expected"], t["ok"], t["reason"])
```

```text
B8 Subtotal 1500.5 1500.5 True match
C8 Subtotal 1760.0 1760.0 True match
B12 Subtotal 3025.0 3025.0 True match
C12 Subtotal 2870.0 2870.0 True match
B13 Grand total 4600.0 4525.5 False mismatch
C13 Grand total 4630.0 4630.0 True match
```

## `hedgehog.summarize`

```
summarize(wb_or_path, llm=None, *, max_tokens=800, budget_tokens=3000, language="en",
          words=200, temperature=0.2, **chat_options) -> Summary
```

A short summary of a workbook. All numbers are computed deterministically; an LLM, if given, only
phrases them.

- **Without `llm`:** a markdown summary of the [facts](#hedgehogfacts): sheets, sizes, and every
  total grouped into mismatched, verified and not checked, with cell references. Free, instant, no
  network.
- **With `llm`** (a client from [`hedgehog.llm.connect`](llm.md#connecting)): exactly one chat call
  with no tools. The model gets the facts trimmed to about `budget_tokens`, a short system prompt
  asking for at most `words` words in `language`, and `max_tokens` for the reply. The cells it cites
  are checked against the workbook. If the call fails, the deterministic summary is returned with
  `llm_error` set; `summarize` does not raise because the LLM did. See [llm.md](llm.md#summaries).

`wb_or_path` is a `Workbook`, a `str` or an `os.PathLike`. Extra `chat_options` are passed to the
client's `chat` call.

```python
import hedgehog

s = hedgehog.summarize("sales.xlsx")
print(s.source, s.usage, s.warnings)
print(next(line for line in s.text.splitlines() if "B13" in line))
```

```text
deterministic None []
- Sales!B13 "Grand total": 4,600, but B6:B12 adds up to 4,525.5 (diff +74.50; mismatch)
```

### `Summary`

| Field | Type | Meaning |
|---|---|---|
| `text` | `str` | The summary (markdown). `str(summary)` returns it too. |
| `facts` | `dict` | The full facts it was built from. |
| `source` | `str` | `"deterministic"` or `"llm"`. |
| `deterministic` | `str` | The deterministic summary, always computed (equals `text` without an LLM). |
| `usage` | `Usage` or `None` | Tokens of the LLM call: `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `calls`, `total_tokens`. |
| `model` | `str` or `None` | Model that wrote the text. |
| `citations` | `CitationReport` or `None` | Check of every cell reference in the LLM text. |
| `warnings` | `list[str]` | Bad or unknown citations, numbers without a citation, refusals, truncation. |
| `payload` | `dict` or `None` | Exactly what was sent to the LLM, for auditing cost. |
| `llm_error` | `str` or `None` | Set when the LLM call failed and `text` fell back to the deterministic summary. |

## `hedgehog.facts`

```
facts(wb, *, small_sheet_cells=150, top_rows=3, max_cols=12, max_totals=40) -> dict
facts_json(wb, **kwargs) -> str
```

The compact, deterministic description of a workbook that summaries and the agent tool
`describe_workbook` are built from. `wb` is a `Workbook` or a path.

- `sheets`: per sheet `name`, `used_range`, `rows`, `cols`, `cells`, `formulas`, `merges`, and
  either `values` (all values of a sheet with at most `small_sheet_cells` filled cells) or `top_rows`
  (the first `top_rows` rows and `max_cols` columns of a larger sheet). Both are
  `{"range": "A1:E3", "rows": [[...], ...]}`.
- `totals`: counts `found`, `verified`, `mismatched`, `unchecked`, and `items` (mismatches first, at
  most `max_totals`, with `omitted` if more). Each item has `cell` (`"Sales!B13"`), `label`, `value`,
  `expected`, `ok`, `reason`, `sources`, `level`, `origin`, and when present `formula`, `label_cell`,
  `double_counted`, `numbers_as_text`.

`facts_json` returns the same as compact JSON (no spaces, UTF-8 kept).

```python
import hedgehog

f = hedgehog.facts("sales.xlsx")
print({k: v for k, v in f["totals"].items() if k != "items"})
print(f["totals"]["items"][0])
```

```text
{'found': 6, 'verified': 5, 'mismatched': 1, 'unchecked': 0}
{'cell': 'Sales!B13', 'label': 'Grand total', 'value': 4600, 'expected': 4525.5, 'ok': False, 'reason': 'mismatch', 'sources': ['B6:B12'], 'level': 'grand_total', 'origin': 'typed', 'label_cell': 'Sales!A13'}
```

## `hedgehog.check_citations`

```
check_citations(text, wb, facts=None) -> CitationReport
```

Finds cell references in text written by a model (`Sales!B13`, `'My sheet'!B2:B9`, `(B13)`) and
checks each one against the workbook: the sheet exists, the cell is inside the used range and not
empty, and a number written right next to the reference equals the cell's value, allowing for the
rounding the text shows. With `facts`, a cell that exists but was not in the facts given to the model
is flagged `not_in_facts`.

`CitationReport`:

| Member | Meaning |
|---|---|
| `citations` | List of `Citation(ref, sheet, rect, start, end, status, written, actual)`. `status` is `ok`, `unknown_sheet`, `empty_cell`, `out_of_range`, `mismatch` or `not_in_facts`. |
| `uncited_numbers` | Numbers that look like data values but have no citation within 40 characters. |
| `cited` | Distinct references, in order. |
| `invalid` | Citations whose status is not `ok`. |
| `ok` | `True` if there are no invalid citations. |
| `problems()` | One readable line per invalid citation. |

```python
import hedgehog

wb = hedgehog.open("sales.xlsx")
rep = hedgehog.check_citations("Grand total 4,600 (Sales!B13); Widgets Q1 1,300 (Sales!B6).", wb)
print(rep.ok, rep.cited)
print(rep.problems())
```

```text
False ['Sales!B13', 'Sales!B6']
['Sales!B6: text gives 1,300.00 for this cell, but it holds 1,200.50']
```

## Errors

| Exception | Raised by | When |
|---|---|---|
| `ValueError` | `open`, `normalize`, `summarize`, `facts` | The file cannot be opened or read; unknown `locale`. |
| `ValueError` | `Workbook.get_cell`, `get_range` | Not a cell reference; range larger than `max_cells`. |
| `KeyError` | `Workbook` methods | No sheet with that name. |
| `KeyError` | `NormalizeResult.table`, `read` | No table with that id or sheet name. |
| `TypeError` | `open` | `path` is not a `str`. |
| `ImportError` | `NormalizeResult.read` | `pandas` or `openpyxl` is not installed. |

LLM-layer errors (`ConfigError`, `MissingDependency`) are described in [llm.md](llm.md#errors).

## Conventions

- Addresses are Excel A1 references; rows are 1-based in every result.
- Sheet names in references are quoted Excel-style when they contain spaces or punctuation:
  `'Plan 2026'!B7`.
- Dates read from the source workbook are ISO strings (`"2024-03-31"`); in the normalized workbook
  they are real Excel dates.
- Nothing in the core package calls the network.

## See also

- [llm.md](llm.md): `hedgehog.llm` (providers, `connect`, `ask`, `bench`)
- [normalized-output.md](normalized-output.md): the files `normalize` writes
- [agent-recipes.md](agent-recipes.md): code for agents working with the output
- [cli.md](cli.md): the same features from the command line
