# hedgehog-sheets

Understand messy spreadsheets and hand AI agents clean, traceable data. Native Rust (napi-rs),
prebuilt for Linux (x64, arm64), macOS (Intel, Apple Silicon) and Windows (x64): **no Rust needed**.
Types included.

hedgehog reads xlsx, xlsm, xlsb, xls, ods, csv and tsv files with titles above tables, multi-row
merged headers, group rows, subtotals, numbers stored as text and footnotes, and writes a clean,
typed workbook plus a JSON manifest that points every value back to its source cell. Totals are
recomputed and checked.

```sh
npm install hedgehog-sheets
```

```ts
import { normalize, open, HedgehogSession } from 'hedgehog-sheets'

// One clean sheet per table + manifest (off the event loop)
const r = await normalize('sales.xlsx', 'out/')
console.log(r.xlsxPath, r.manifest.tables.map((t) => `${t.id} ${t.source.sheet}!${t.source.range}`))
console.log(r.manifest.time_coverage, r.manifest.totals.mismatch, 'wrong total(s)')

// Look inside the original, read-only
const wb = open('sales.xlsx')
wb.getCell('Sales', 'B13')            // { sheet, address, value, type, formula, merged }
wb.getRange('Sales', 'A6:C7').rows    // [['Widgets', 1200.5, 1350], ['Gadgets', 300, 410]]
wb.findText('grand total')            // case-, accent- and script-insensitive
wb.totals('Sales')                    // detected totals, recomputed: actual, expected, ok

// The agent tools of `hedgehog mcp`, in-process: compact text for an LLM
const s = new HedgehogSession({ root: '.' })
await s.openFile('sales.xlsx')        // catalog: tables, columns, time coverage, totals check
await s.query('sales.xlsx', 'SELECT Product, "Revenue / Q1" FROM T1')
HedgehogSession.toolSpecs()           // MCP tool specs to register with your model
```

| API | |
|---|---|
| `normalize(path, outDir, {keepTotals, csv, locale, rulesOnly, modelMaxCells}?)` | Promise of `{xlsxPath, manifestPath, csvPaths, manifest}` |
| `normalizeSync(...)` | The same, blocking |
| `open(path)`, `openAsync(path)` | `Workbook`: `sheetNames`, `sheets()`, `getCell`, `getRange`, `findText`, `totals`, `describe` |
| `new HedgehogSession({root, allow?})` | `openFile`, `describeTable`, `query`, `cells`, `totals`, `listFiles`, `call`, `allow` |

Full reference, error codes and the mapping to the Python package:
[docs/node-api.md](https://github.com/05vukasin/hedgehog/blob/master/docs/node-api.md). The manifest:
[docs/normalized-output.md](https://github.com/05vukasin/hedgehog/blob/master/docs/normalized-output.md).
The agent tools: [docs/mcp.md](https://github.com/05vukasin/hedgehog/blob/master/docs/mcp.md).

Python: `pip install hedgehog-sheets` (`import hedgehog`). Source and issues:
<https://github.com/05vukasin/hedgehog>. MIT license.
