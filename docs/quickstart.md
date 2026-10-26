# Quickstart

Ten minutes from a messy workbook to a clean table you can load with pandas, first from the command
line and then from Python.

You need hedgehog installed ([installation.md](installation.md)) and, for this walkthrough, `openpyxl`
and `pandas` (`pip install openpyxl pandas`).

## A sample messy workbook

This script writes `sales.xlsx`, a small report with the usual problems: a title above the table, a
two-row header with merged cells, group rows (`Region: North`), subtotal rows, a grand total that
does not add up, numbers and percentages stored as text, a missing-value token and a footnote.

```python
import openpyxl

wb = openpyxl.Workbook()
ws = wb.active
ws.title = "Sales"
ws["A1"] = "Sales report 2024"
ws.append([])
ws.append(["Product", "Revenue", None, "Units", "Margin"])
ws.append([None, "Q1", "Q2", None, None])
for rng in ("B3:C3", "A3:A4", "D3:D4", "E3:E4"):
    ws.merge_cells(rng)
for row in [
    ["Region: North"],
    ["Widgets", 1200.5, 1350, "1,200", "12%"],
    ["Gadgets", 300, 410, "350", "8.5%"],
    ["Subtotal", 1500.5, 1760, None, None],
    ["Region: South"],
    ["Widgets", 980, 1150, "n/a", "10%"],
    ["Gizmos", 2045, 1720, "2,100", "15%"],
    ["Subtotal", 3025, 2870, None, None],
    ["Grand total", 4600, 4630, None, None],
]:
    ws.append(row)
ws.append([])
ws.append(["Source: internal ERP export, amounts in USD"])
wb.save("sales.xlsx")
```

In Excel it looks like this:

```
     A                 B         C       D       E
 1   Sales report 2024
 2
 3   Product           Revenue           Units   Margin
 4                     Q1        Q2
 5   Region: North
 6   Widgets           1200.5    1350    1,200   12%
 7   Gadgets           300       410     350     8.5%
 8   Subtotal          1500.5    1760
 9   Region: South
10   Widgets           980       1150    n/a     10%
11   Gizmos            2045      1720    2,100   15%
12   Subtotal          3025      2870
13   Grand total       4600      4630
14
15   Source: internal ERP export, amounts in USD
```

`pd.read_excel("sales.xlsx")` on this sheet gives a frame with `Unnamed:` columns, the title as a
header, the group and total rows mixed into the data, and `Units` and `Margin` as text.

## 1. From the command line

Get an overview of the file:

```console
$ hedgehog describe sales.xlsx
Sales: A1:E15 · 39 cells · 0 formulas · 4 merged ranges
```

Normalize it:

```console
$ hedgehog normalize sales.xlsx -o out/
sales.xlsx: 1 tables, 39 cells (28 in tables, 9 totals, 2 notes) in 16 ms
  out/sales.normalized.xlsx
  out/sales.manifest.json
```

Every one of the 39 source cells is accounted for: 28 went into the table, 9 belong to total rows
(listed on the `Totals` sheet), and 2 (the title and the footnote) are on the `Notes` sheet.

## 2. Load the result with pandas

```python
import pandas as pd

df = pd.read_excel("out/sales.normalized.xlsx", sheet_name="T1_sales_report_2024")
print(df)
```

```text
  Region  Product  Revenue / Q1  Revenue / Q2   Units  Margin  _src_row
0  North  Widgets        1200.5          1350  1200.0   0.120         6
1  North  Gadgets         300.0           410   350.0   0.085         7
2  South  Widgets         980.0          1150     NaN   0.100        10
3  South   Gizmos        2045.0          1720  2100.0   0.150        11
```

What happened:

- The two header rows became one: `Revenue / Q1`, `Revenue / Q2`.
- `Region: North` and `Region: South` became a `Region` column.
- The subtotal and grand-total rows left the data, so `df["Revenue / Q1"].sum()` is correct (4525.5).
- `"1,200"` became 1200, `"12%"` became 0.12, `n/a` became a missing value.
- `_src_row` is the row in the original sheet: `Gizmos` came from row 11.

## 3. Check the totals

The `Totals` sheet lists every total and subtotal with its recomputed value. One of them is wrong:

```python
import json

m = json.load(open("out/sales.manifest.json", encoding="utf-8"))
for t in m["totals"]["items"]:
    if t["status"] == "mismatch":
        print(t["sheet"], t["cell"], t["label"], t["value"], "but the data add up to", t["recomputed"])
```

```text
Sales B13 Grand total 4600 but the data add up to 4525.5
```

## 4. The same from Python

```python
import hedgehog

r = hedgehog.normalize("sales.xlsx", "out/")
print(r.tables[0]["sheet"], r.tables[0]["rows"], "rows")
for c in r.table("T1")["columns"]:
    print(f'{c["name"]:14} {c["dtype"]:8} from column {c["source_column"]}')
```

```text
T1_sales_report_2024 4 rows
Region         text     from column None
Product        text     from column A
Revenue / Q1   number   from column B
Revenue / Q2   integer  from column C
Units          integer  from column D
Margin         percent  from column E
_src_row       integer  from column None
```

Read the source workbook directly, without normalizing:

```python
import hedgehog

wb = hedgehog.open("sales.xlsx")
print(wb.get_cell("Sales", "B13"))
print(hedgehog.summarize(wb).text)
```

```text
{'sheet': 'Sales', 'address': 'B13', 'value': 4600.0, 'type': 'number', 'formula': None}
# Workbook summary

1 sheet(s), 39 filled cells, 0 formulas.

| Sheet | Used range | Rows × cols | Cells | Formulas | Merged |
|---|---|---|---|---|---|
| Sales | A1:E15 | 15 × 5 | 39 | 0 | 4 |

## Totals

6 total(s) found: 5 verified, 1 mismatched, 0 could not be checked.

### Mismatched

- Sales!B13 "Grand total": 4,600, but B6:B12 adds up to 4,525.5 (diff +74.50; mismatch)

### Verified

- Sales!B8 "Subtotal" = 1,500.5 = sum of B6:B7
- Sales!C8 "Subtotal" = 1,760 = sum of C6:C7
- Sales!B12 "Subtotal" = 3,025 = sum of B10:B11
- Sales!C12 "Subtotal" = 2,870 = sum of C10:C11
- Sales!C13 "Grand total" = 4,630 = sum of C6:C12
```

## 5. Let an AI agent ask the questions

Agents don't need the clean workbook: they call hedgehog's tools and ask with SQL. With the `hedgehog`
command installed, add it to Claude Code as a plugin:

```text
/plugin marketplace add 05vukasin/hedgehog
/plugin install hedgehog@hedgehog
```

Then ask Claude about `sales.xlsx` in that folder. It opens the file with `open_file`, reads the
catalog, and answers with a query such as:

```text
SELECT SUM("Revenue / Q1") FROM T1 WHERE Region = 'North'
1500.5
```

Any other MCP client works the same way with `hedgehog mcp --root <folder>`; see
[mcp.md](mcp.md).

## Next steps

- Understand what you just saw: [concepts.md](concepts.md)
- Hand the clean workbook to an agent with a code sandbox: [agent-recipes.md](agent-recipes.md)
- Every output sheet and manifest field: [normalized-output.md](normalized-output.md)
- European files, ambiguous `1.200` and `03/04/2024`: [locales.md](locales.md)
- Have an LLM phrase the summary: [llm.md](llm.md)

## See also

- [cli.md](cli.md): all commands and flags
- [python-api.md](python-api.md): every public function and result field
