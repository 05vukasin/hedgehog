# Agent recipes

Copy-paste pandas and openpyxl code for an AI agent (or a person) working in a code sandbox with the
normalized workbook and its manifest.

The usual setup: hedgehog runs where the files arrive, and the agent's sandbox receives
`<stem>.normalized.xlsx` and `<stem>.manifest.json`. The sandbox needs only `pandas` and `openpyxl`,
not hedgehog. Every recipe below was run against the output of
`hedgehog normalize sales.xlsx -o out/` for the [quickstart](quickstart.md#a-sample-messy-workbook)
sample.

Contents: [Tell the agent what it has](#tell-the-agent-what-it-has) ·
[Load every table](#load-every-table) · [Apply the manifest dtypes](#apply-the-manifest-dtypes) ·
[Aggregate without double counting](#aggregate-without-double-counting) ·
[Find wrong totals](#find-wrong-totals) · [Trace a value to its source cell](#trace-a-value-to-its-source-cell) ·
[Read the notes](#read-the-notes) · [Write changes back](#write-changes-back) ·
[Work with openpyxl](#work-with-openpyxl)

## Tell the agent what it has

A short paragraph in the agent's prompt saves it from exploring the files blindly:

```text
out/sales.normalized.xlsx is a cleaned copy of the user's workbook. Sheet README explains it.
Each table is on its own sheet with one header row; load it with pandas.read_excel(path, sheet_name=...).
Column _src_row is the row number in the original sheet. Total rows are not in the tables; they are on
the Totals sheet with a status of ok, mismatch or unknown. Titles and footnotes are on the Notes sheet.
out/sales.manifest.json describes every table: source sheet and range, column dtypes, original headers
and source column letters. Cite values as <source sheet>!<source column><_src_row>.
```

## Load every table

```python
import json
import pandas as pd

xlsx, manifest = "out/sales.normalized.xlsx", "out/sales.manifest.json"
m = json.load(open(manifest, encoding="utf-8"))
tables = {t["id"]: pd.read_excel(xlsx, sheet_name=t["sheet"]) for t in m["tables"]}
for t in m["tables"]:
    print(t["id"], t["sheet"], "from", f'{t["source"]["sheet"]}!{t["source"]["range"]}', "-", t["rows"], "rows")
```

```text
T1 T1_sales_report_2024 from Sales!A3:E13 - 4 rows
```

`pd.read_excel(xlsx, sheet_name=None)` loads every sheet at once, including `README`, `Totals` and
`Notes`.

## Apply the manifest dtypes

The manifest's `dtype` is authoritative. pandas usually agrees, but a column with missing values
becomes float, and a date column with no dates in the first rows may stay `object`. This makes the
types explicit:

```python
import json
import pandas as pd

m = json.load(open("out/sales.manifest.json", encoding="utf-8"))
t = m["tables"][0]
df = pd.read_excel("out/sales.normalized.xlsx", sheet_name=t["sheet"])

for c in t["columns"]:
    col = c["name"]
    if c["dtype"] == "integer":
        df[col] = pd.to_numeric(df[col], errors="coerce").astype("Int64")   # keeps missing values
    elif c["dtype"] in ("number", "percent", "currency"):
        df[col] = pd.to_numeric(df[col], errors="coerce")
    elif c["dtype"] in ("date", "datetime"):
        df[col] = pd.to_datetime(df[col], errors="coerce")
    elif c["dtype"] == "bool":
        df[col] = df[col].astype("boolean")
print(df.dtypes.to_string())
```

```text
Region              str
Product             str
Revenue / Q1    float64
Revenue / Q2      Int64
Units             Int64
Margin          float64
_src_row          Int64
```

pandas 3 prints `str` for text columns; pandas 2 prints `object`.

`errors="coerce"` turns values that were kept as text into missing values. Do this on purpose only:
the table's `warnings` list which source cells those are.

Percentages are fractions (`0.12` for 12%) and `currency` columns are plain numbers; the currency
code is in the column's `currency` field.

## Aggregate without double counting

By default the table sheets hold data rows only, so plain sums are correct:

```python
import pandas as pd

df = pd.read_excel("out/sales.normalized.xlsx", sheet_name="T1_sales_report_2024")
print(df.groupby("Region")[["Revenue / Q1", "Revenue / Q2"]].sum())
print("Q1 total:", df["Revenue / Q1"].sum())
```

```text
        Revenue / Q1  Revenue / Q2
Region
North         1500.5          1760
South         3025.0          2870
Q1 total: 4525.5
```

If the file was normalized with `--keep-totals`, filter first:
`df[df["_row_type"] == "data"]`. Otherwise every subtotal is counted twice.

## Find wrong totals

```python
import json

m = json.load(open("out/sales.manifest.json", encoding="utf-8"))
print(m["totals"]["ok"], "ok,", m["totals"]["mismatch"], "mismatch,", m["totals"]["unknown"], "unknown")
for t in m["totals"]["items"]:
    if t["status"] == "mismatch":
        print(f'{t["sheet"]}!{t["cell"]} ({t["label"]}): file says {t["value"]}, '
              f'{", ".join(t["sources"])} add up to {t["recomputed"]}')
```

```text
5 ok, 1 mismatch, 0 unknown
Sales!B13 (Grand total): file says 4600, B6:B12 add up to 4525.5
```

The `Totals` sheet has the same rows plus `label_cell` and `formula`; load it with
`pd.read_excel(xlsx, sheet_name="Totals")`. `unknown` means the total could not be checked (no
cached value, text in the source, an unconfirmed source range); the `reason` column says which.

## Trace a value to its source cell

Every data value came from `<source_column><_src_row>` on the table's source sheet:

```python
import json
import pandas as pd

m = json.load(open("out/sales.manifest.json", encoding="utf-8"))
t = m["tables"][0]
df = pd.read_excel("out/sales.normalized.xlsx", sheet_name=t["sheet"])
letter = {c["name"]: c["source_column"] for c in t["columns"]}

def source_cell(i, col):
    """Excel reference of the original cell behind df.loc[i, col]."""
    return f'{t["source"]["sheet"]}!{letter[col]}{int(df.loc[i, "_src_row"])}'

best = df["Revenue / Q1"].idxmax()
print(df.loc[best, "Product"], df.loc[best, "Revenue / Q1"], "from", source_cell(best, "Revenue / Q1"))
```

```text
Gizmos 2045.0 from Sales!B11
```

Columns whose `source_column` is `null` (the group column, `_src_row`, `_row_type`) have no single
source cell. For the group column, the original label row is above the data rows of its group.

The header text of a column, cell by cell, is in `original_header`:

```python
import json

m = json.load(open("out/sales.manifest.json", encoding="utf-8"))
cols = {c["name"]: c for c in m["tables"][0]["columns"]}
print(cols["Revenue / Q1"]["original_header"])
```

```text
[{'text': 'Revenue', 'cell': 'B3'}, {'text': 'Q1', 'cell': 'B4'}]
```

## Read the notes

Titles, footnotes and every cell outside a table are on the `Notes` sheet. Footnotes often say what
the numbers mean ("amounts in thousands", "preliminary"):

```python
import pandas as pd

notes = pd.read_excel("out/sales.normalized.xlsx", sheet_name="Notes")
print(notes[["cell", "kind", "table", "text"]].to_string(index=False))
```

```text
cell  kind table                                        text
  A1 title    T1                           Sales report 2024
 A15  note    T1 Source: internal ERP export, amounts in USD
```

## Write changes back

hedgehog never modifies the source file. To deliver a corrected workbook, write into a copy of the
original at the cells the manifest points to. Here the agent fixes the wrong grand total:

```python
import json
import openpyxl

m = json.load(open("out/sales.manifest.json", encoding="utf-8"))
wb = openpyxl.load_workbook("sales.xlsx")          # the original file
for t in m["totals"]["items"]:
    if t["status"] == "mismatch":
        wb[t["sheet"]][t["cell"]] = t["recomputed"]
        print("fixed", t["sheet"], t["cell"], t["value"], "->", t["recomputed"])
wb.save("sales.corrected.xlsx")                      # a new file; the original stays as it was
```

```text
fixed Sales B13 4600 -> 4525.5
```

To write a changed table into a copy of the normalized workbook, replace its sheet. The other sheets
are kept, but the Excel table definition (`ws.tables`) of the replaced sheet is not:

```python
import shutil
import pandas as pd

xlsx = shutil.copy("out/sales.normalized.xlsx", "out/sales.edited.xlsx")
df = pd.read_excel(xlsx, sheet_name="T1_sales_report_2024")
df["Revenue / H1"] = df["Revenue / Q1"] + df["Revenue / Q2"]
with pd.ExcelWriter(xlsx, engine="openpyxl", mode="a", if_sheet_exists="replace") as w:
    df.to_excel(w, sheet_name="T1_sales_report_2024", index=False)
print(pd.read_excel(xlsx, sheet_name="T1_sales_report_2024").columns.tolist())
```

```text
['Region', 'Product', 'Revenue / Q1', 'Revenue / Q2', 'Units', 'Margin', '_src_row', 'Revenue / H1']
```

## Work with openpyxl

Each table sheet holds an Excel table with the same name, so its range is one lookup away:

```python
import openpyxl

wb = openpyxl.load_workbook("out/sales.normalized.xlsx")
ws = wb["T1_sales_report_2024"]
tbl = ws.tables["T1_sales_report_2024"]
print(tbl.ref)
header = [c.value for c in ws[1]]
for row in ws.iter_rows(min_row=2, values_only=True):
    print(dict(zip(header, row)))
    break
```

```text
A1:G5
{'Region': 'North', 'Product': 'Widgets', 'Revenue / Q1': 1200.5, 'Revenue / Q2': 1350, 'Units': 1200, 'Margin': 0.12, '_src_row': 6}
```

## See also

- [normalized-output.md](normalized-output.md): every sheet, column and manifest field
- [concepts.md](concepts.md): tables, roles, totals and provenance
- [llm.md](llm.md#asking-questions-with-tools): letting a model query the source workbook through tools
