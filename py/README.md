# hedgehog-sheets

Understand and normalize messy Excel spreadsheets for AI agents: find the tables, label every cell,
parse US and European numbers and dates, verify totals, and write a clean typed workbook plus a
manifest that an agent with a code sandbox can load directly. Rust core, Python API.

```sh
pip install hedgehog-sheets            # prebuilt wheels: no Rust toolchain needed
```

```python
import hedgehog
wb = hedgehog.open("report.xlsx")
print(wb.describe())
```

Documentation, roadmap and source: https://github.com/05vukasin/hedgehog
