# Node.js / TypeScript API

hedgehog for JavaScript and TypeScript: the npm package **`hedgehog-sheets`**, a native addon built
with [napi-rs](https://napi.rs) from the same Rust core as the Python package and the `hedgehog`
command. Prebuilt for Linux, macOS and Windows, so **no Rust is needed**; types are included.

```ts
import { normalize, open, HedgehogSession } from 'hedgehog-sheets'
```

- [Install](#install)
- [Example](#example)
- [normalize](#normalize), [normalizeSync](#normalizesync)
- [open, openAsync and Workbook](#open-openasync-and-workbook)
- [HedgehogSession: the agent tools](#hedgehogsession-the-agent-tools)
- [Errors](#errors)
- [Threads and the event loop](#threads-and-the-event-loop)
- [How it maps to Python](#how-it-maps-to-python)
- [Building from source](#building-from-source)

## Install

Node.js 18 or newer on Linux (x64, arm64; glibc), macOS (Intel, Apple Silicon) or Windows (x64).

```sh
npm install hedgehog-sheets
```

The package pulls in one small platform package with the native addon
(`hedgehog-sheets-linux-x64-gnu`, `hedgehog-sheets-darwin-arm64`, ...) through
`optionalDependencies`; npm installs only the one for your machine. Works the same with pnpm, yarn
and bun.

Until it is on npm, every [GitHub Release](https://github.com/05vukasin/hedgehog/releases) has the
tarballs. Install the main package together with the one for your platform:

```sh
v=0.5.0   # a release with the npm tarballs
npm install \
  https://github.com/05vukasin/hedgehog/releases/download/v$v/hedgehog-sheets-$v.tgz \
  https://github.com/05vukasin/hedgehog/releases/download/v$v/hedgehog-sheets-linux-x64-gnu-$v.tgz
```

Platform packages: `linux-x64-gnu`, `linux-arm64-gnu`, `darwin-x64`, `darwin-arm64`,
`win32-x64-msvc`. Alpine (musl) and 32-bit systems are not built yet.

Check the install:

```sh
node -e "console.log(require('hedgehog-sheets').version())"
```

The package is CommonJS with named exports, so both `import { open } from 'hedgehog-sheets'` and
`const { open } = require('hedgehog-sheets')` work.

## Example

With the `sales.xlsx` of the [quickstart](quickstart.md#a-sample-messy-workbook):

```ts
import { normalize, open, HedgehogSession } from 'hedgehog-sheets'

const r = await normalize('sales.xlsx', 'out/')          // same files as the command line
const t = r.manifest.tables[0]
console.log(t.sheet, t.rows, 'rows,', r.manifest.totals.mismatch, 'wrong total(s)')

const wb = open('sales.xlsx')                            // look inside the original, read-only
console.log(wb.getCell('Sales', 'B13').value)            // one cell
console.log(wb.getRange('Sales', 'A6:C7').rows)          // a range, row by row
console.log(wb.findText('grand total')[0].address)       // search, ignoring case and accents
console.log(wb.totals('Sales').slice(-2).map((x) => [x.address, x.ok]))

const s = new HedgehogSession({ root: '.' })             // the MCP tools, in-process
console.log(await s.query('sales.xlsx', 'SELECT Product, "Revenue / Q1" FROM T1 ORDER BY 2 DESC LIMIT 2'))
```

```text
T1_sales_report_2024 4 rows, 1 wrong total(s)
4600
[ [ 'Widgets', 1200.5, 1350 ], [ 'Gadgets', 300, 410 ] ]
A13
[ [ 'B13', false ], [ 'C13', true ] ]
Product,Revenue / Q1
Gizmos,2045
Widgets,1200.5
(2 rows)
```

## Naming

Function, method, option and result keys are camelCase (`getCell`, `keepTotals`, `xlsxPath`,
`usedRange`, `labelAddress`). Two things keep their original spelling:

- **The manifest** (`result.manifest`) is the parsed `<stem>.manifest.json`, keys snake_case exactly
  as in the file and in [normalized-output.md](normalized-output.md#manifest) (`schema_version`,
  `time_coverage`, `source_column`). Its keys keep the file's order.
- **Enum-like values** are the same strings as in Python and the manifest: `grand_total`,
  `formula:sum`, `keyword:total`, `arithmetic_match`.

Paths can be strings or `file:` URLs (`new URL('./data.xlsx', import.meta.url)`).

## normalize

```ts
normalize(path: PathLike, outDir: PathLike, opts?: NormalizeOptions): Promise<NormalizeResult>
```

Writes `<stem>.normalized.xlsx` (README, one clean typed sheet per table, Totals, Notes) and
`<stem>.manifest.json` into `outDir` (created if missing); the source file is never modified. Runs on
the libuv thread pool, so the event loop keeps running. Reads xlsx, xlsm, xlsb, xls, ods, csv and tsv
(format detected from content). The output is described in [normalized-output.md](normalized-output.md).

| Option | Type | Default | Meaning |
|---|---|---|---|
| `keepTotals` | boolean | false | Keep total/subtotal rows in the table sheets, marked in a `_row_type` column |
| `csv` | boolean | false | Also write one UTF-8 CSV per table |
| `locale` | string | inferred | `"de-DE"`, `"en-US"`, `"sr"`, ... for every column; see [locales.md](locales.md) |
| `rulesOnly` | boolean | false | Cell roles from the layout rules alone, without the learned model |
| `modelMaxCells` | number | 1,000,000 | Sheets with more cells use the rules alone |

`NormalizeResult`:

| Field | Type | Meaning |
|---|---|---|
| `xlsxPath` | string | The normalized workbook |
| `manifestPath` | string | The manifest file |
| `csvPaths` | string[] | One per table with `csv: true`, else empty |
| `manifest` | `Manifest` | The manifest, parsed; typed in `index.d.ts` (`ManifestTable`, `ManifestColumn`, `TimeCoverage`) |

```ts
const r = await normalize('report.xlsx', 'out', { csv: true, locale: 'de-DE' })
for (const t of r.manifest.tables) {
  console.log(t.id, `${t.source.sheet}!${t.source.range}`, t.columns.map((c) => `${c.name}:${c.dtype}`))
}
console.log(r.manifest.time_coverage)   // { from: '2011', to: '2025', granularity: 'year', ... } or null
```

## normalizeSync

```ts
normalizeSync(path: PathLike, outDir: PathLike, opts?: NormalizeOptions): NormalizeResult
```

The same on the calling thread. It blocks the event loop for the whole run; use it in scripts and
tests, not in servers or UIs.

## open, openAsync and Workbook

```ts
open(path: PathLike): Workbook
openAsync(path: PathLike): Promise<Workbook>
```

Read a workbook into memory, read-only. `open` reads on the calling thread; `openAsync` on the thread
pool (worth it for large files). The `Workbook` methods are fast lookups and run synchronously.

| Member | Returns |
|---|---|
| `sheetNames` | `string[]`, in workbook order |
| `sheets()` | `SheetInfo[]`: `{name, usedRange, rows, cols, cells, formulas, merges}`; `usedRange` is `"A1:F20"` or null |
| `describe()` | One line per sheet, plain text |
| `getCell(sheet, address)` | `CellInfo` |
| `getRange(sheet, range, maxCells = 5000)` | `{sheet, range, rows}`: values row by row, null for empty cells |
| `findText(query, {sheet?, limit = 50}?)` | `CellInfo[]` whose text contains `query`, ignoring case, accents and script (Latin/Cyrillic); numbers match on their digits |
| `totals(sheet?)` | `TotalInfo[]` of one sheet, or of every sheet without `sheet` |
| `toString()` | `Workbook(sheets=[...])` |

`sheet` may be null when the reference names the sheet (`getCell(null, "Sales!B13")`); with neither,
the first sheet is used. Sheet names match exactly first, then ignoring case.

`CellInfo` is `{sheet, address, value, type, formula, merged}`:

| Field | Type | Meaning |
|---|---|---|
| `value` | number, string, boolean or null | Dates as ISO strings (`2024-03-31`), errors as their text (`#DIV/0!`), empty cells null |
| `type` | string | `empty`, `bool`, `number`, `text`, `date` or `error` |
| `formula` | string or null | With a leading `=` |
| `merged` | string or null | The merged range the cell belongs to (`B3:C3`) |

`TotalInfo` is a detected total and its check: `address`, `origin` (`formula`/`typed`), `level`
(`total`, `subtotal`, `grand_total`), `direction`, `sources` (summed ranges), `excluded`,
`doubleCounted`, `label`, `labelAddress`, `formula`, `evidence`, `actual` (the value in the cell),
`expected` (the recomputed sum), `diff`, `ok` (true, false, or null when it could not be checked),
`reason`, `summed` and `numbersAsText`. Same meaning as in [python-api.md](python-api.md#totals).

## HedgehogSession: the agent tools

The six tools of [`hedgehog mcp`](mcp.md), in-process, for a TypeScript agent that registers its own
tools (no MCP server process to spawn). Every method resolves to the same compact text an LLM gets
from the MCP server: at most ~12,000 characters, and a cut ends with a `… cut:` line saying how to
get more.

```ts
const s = new HedgehogSession({ root: '/path/to/spreadsheets', allow: ['/home/me/q3.xlsx'] })
console.log(await s.openFile('budget/2024.xlsx'))          // catalog: tables, columns, time coverage, totals
console.log(await s.query('budget/2024.xlsx', 'SELECT SUM("Amount") FROM T1'))
```

| Method | MCP tool | Returns |
|---|---|---|
| `new HedgehogSession({root, allow?})` | | File access is bounded by `root` (relative paths resolve against it, no `..` escape) plus the `allow` files and folders |
| `openFile(path)` | `open_file` | The catalog; call first for every file |
| `describeTable(path, table)` | `describe_table` | All columns with stats, first rows, header cells, total rows |
| `query(path, sql, limit?)` | `query` | Read-only SQLite SQL over `T1`, `T2`, ... (`T1_all` with total rows) as CSV; default 50, max 200 rows |
| `cells(path, sheet, range)` | `cells` | Source cells (max 200) as `A1<TAB>value` lines, for citations |
| `totals(path, {table?, status?}?)` | `totals` | The totals check, mismatches first |
| `listFiles(folder?, pattern?)` | `list_files` | Spreadsheet files under the folder |
| `call(name, args)` | any | A tool by its MCP name with JSON arguments, as an MCP host would call it |
| `allow(path)` | | Allows one more file or folder (e.g. one the user referenced with `@`) |
| `root` | | The canonical root folder |
| `HedgehogSession.toolSpecs()` | `tools/list` | `[{name, title, description, inputSchema, annotations}]`, to register the tools with an LLM |

Files are normalized once per content hash and kept for the session (the 8 most recently used).
A tool error rejects with an `Error` whose `message` is the one-line text for the model (unknown
column → the column names, unknown table → the table ids) and whose `code` is `ERR_HEDGEHOG_TOOL`;
pass the message back to the model as the tool result with an error flag. The texts, limits and SQL
rules are documented in [mcp.md](mcp.md).

Wiring the tools into an agent loop with their MCP specs:

```ts
const tools = HedgehogSession.toolSpecs()                 // name, description, inputSchema
const s = new HedgehogSession({ root: process.cwd() })

async function runTool(name: string, args: Record<string, unknown>) {
  try {
    return { text: await s.call(name, args), isError: false }
  } catch (e) {
    if ((e as { code?: string }).code === 'ERR_HEDGEHOG_TOOL') return { text: (e as Error).message, isError: true }
    throw e
  }
}
```

## Errors

| Situation | Thrown or rejected with |
|---|---|
| Missing or unreadable file | `Error`, message `<path>: <reason>` (`report.xlsx: No such file or directory (os error 2)`) |
| Not a spreadsheet | `Error`, message `<path>: <reader error>` |
| Unknown sheet | `Error`: `no sheet named "Nope"; sheets: ["Sales"]` |
| Bad cell reference | `Error`: `not a cell reference: "B"` |
| Range above `maxCells` | `Error`: `range A1:Z1000 has 26000 cells, more than maxCells=5000` |
| Unknown locale | rejected: `unknown locale "xx"` |
| Path that is not a string or URL | `TypeError` |
| Agent tool error | rejected, `code: 'ERR_HEDGEHOG_TOOL'` |

Errors from `open` and the `Workbook` methods carry `code: 'InvalidArg'`, normalize failures
`code: 'GenericFailure'` (napi-rs status names). The async functions never throw synchronously: every
error rejects the Promise.

## Threads and the event loop

`normalize`, `openAsync` and every `HedgehogSession` method run on the libuv thread pool (4 threads
by default, `UV_THREADPOOL_SIZE` to change); the JS thread only converts the result. Measured on a
debug build: normalizing a 244-cell open-data workbook took 193 ms while a 5 ms interval timer kept
firing (53 times, longest gap 6 ms). Calls on one `HedgehogSession` take turns (a session owns SQLite
connections); use several sessions for parallel work on different folders.

A panic in the Rust code becomes a JS exception (the addon is built with unwinding), so a bug in
hedgehog cannot abort the host process.

## How it maps to Python

| Python (`import hedgehog`) | Node (`hedgehog-sheets`) |
|---|---|
| `hedgehog.normalize(path, out_dir, keep_totals=, csv=, locale=, rules_only=, model_max_cells=)` | `await normalize(path, outDir, {keepTotals, csv, locale, rulesOnly, modelMaxCells})` |
| `NormalizeResult.xlsx_path`, `.manifest_path`, `.csv_paths`, `.manifest` | `.xlsxPath`, `.manifestPath`, `.csvPaths`, `.manifest` (same snake_case manifest) |
| `NormalizeResult.tables`, `.table("T1")` | `r.manifest.tables`, `r.manifest.tables.find((t) => t.id === 'T1')` |
| `NormalizeResult.read("T1")` (pandas) | not included; read the xlsx or the CSVs with any JS library, or use `HedgehogSession.query` |
| `hedgehog.open(path)` | `open(path)`, `await openAsync(path)` |
| `wb.sheet_names`, `wb.sheets()`, `wb.describe()` | `wb.sheetNames`, `wb.sheets()`, `wb.describe()` |
| `wb.get_cell(sheet, a1)` | `wb.getCell(sheet, a1)` |
| `wb.get_range(sheet, range, max_cells=5000)` | `wb.getRange(sheet, range, maxCells)` |
| `wb.find_text(q, sheet=None, limit=50)` | `wb.findText(q, {sheet, limit})` |
| `wb.totals(sheet)` | `wb.totals(sheet)`, or `wb.totals()` for every sheet |
| dict keys `used_range`, `label_address`, `double_counted`, `numbers_as_text` | `usedRange`, `labelAddress`, `doubleCounted`, `numbersAsText` |
| `None` | `null` |
| `hedgehog.summarize`, `facts`, `check_citations`, `hedgehog.llm` | not in the Node package (Python layer) |
| `hedgehog mcp` (server) | `HedgehogSession` (same tools, in-process) |

## Building from source

You need Rust (the version in `rust-toolchain.toml`) and Node.js 18 or newer.

```sh
git clone https://github.com/05vukasin/hedgehog
cd hedgehog/node
npm ci
npm run build          # release addon: hedgehog.<platform>.node next to index.js
npm run build:debug    # faster to compile, slower to run
npm test               # node:test, on spreadsheets generated by the tests
npm run typecheck      # tsc over test/types.ts against index.d.ts
```

`npm run build` regenerates `binding.js` (the napi-rs loader that picks the platform package or the
local `.node` file) and `binding.d.ts` (the generated types); both are committed and the release
workflow checks that they are current. `index.js` and `index.d.ts` are hand-written on top: URL paths,
the parsed and typed manifest, and `HedgehogSession`. The Rust side is `node/src/lib.rs` (crate
`hh_node`), a thin layer over `hh_core` and `hh_agent` like the Python bindings in `py/`.

Releases: see [development.md](development.md#releases); the `node` and `npm` jobs of
`.github/workflows/release.yml` build the five addons, pack the npm tarballs and publish them when
the repository variable `PUBLISH_NPM` is `true`.

## See also

- [mcp.md](mcp.md): the agent tools' texts and limits
- [normalized-output.md](normalized-output.md): the normalized workbook and manifest
- [python-api.md](python-api.md): the Python package
