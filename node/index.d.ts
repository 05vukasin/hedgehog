// Public types of hedgehog-sheets. The native classes and result shapes come from the napi-rs
// generated `binding.d.ts`; this file adds `URL` paths, a typed manifest and the agent-tools session.

import type {
  ToolSpec,
  NormalizeOptions,
  NormalizeResult as NativeNormalizeResult,
  Workbook,
} from './binding'

export type {
  CellInfo,
  FindTextOptions,
  NormalizeOptions,
  RangeValues,
  SheetInfo,
  TotalInfo,
  ToolSpec,
} from './binding'
export { Workbook, version, toolSpecs } from './binding'

/** A file path, or a `file:` URL (`new URL('./data.xlsx', import.meta.url)`). */
export type PathLike = string | URL

/** A cell value: number, text, boolean, ISO date string (`2024-03-31`), `#ERR` text, or null. */
export type CellValue = number | string | boolean | null

/** Period covered by a table or the workbook (see docs/normalized-output.md#time-coverage). */
export interface TimeCoverage {
  /** `2017`, `2024-Q1`, `2024-03`, `2024-W05` or `2024-03-31`. */
  from: string
  to: string
  granularity: 'year' | 'quarter' | 'month' | 'week' | 'day'
  source: 'column' | 'header'
  columns: string[]
  /** Workbook level only: ids of the tables it spans. */
  tables?: string[]
  [key: string]: unknown
}

/** One output column of a table (`manifest.tables[].columns[]`). */
export interface ManifestColumn {
  name: string
  index: number
  letter: string
  role: 'data' | 'group' | 'source_row' | 'row_type'
  dtype: 'integer' | 'number' | 'percent' | 'currency' | 'date' | 'datetime' | 'text' | 'bool' | 'empty'
  number_format: string | null
  source_column: string | null
  original_header?: Array<{ text: string; cell: string }>
  unit?: string | null
  currency?: string | null
  nulls?: number
  distinct?: number
  min?: CellValue
  max?: CellValue
  example?: CellValue
  [key: string]: unknown
}

/** One table of the normalized workbook (`manifest.tables[]`). */
export interface ManifestTable {
  /** `T1`, `T2`, ... */
  id: string
  /** Output sheet in the normalized xlsx (`T1_sales`). */
  sheet: string
  excel_table: string
  source: {
    sheet: string
    range: string
    header_top: number
    header_rows: number
    header_range: string
    data_range: string
    title: string | null
    title_cell: string | null
    notes: Array<{ text: string; cell: string }>
    [key: string]: unknown
  }
  rows: number
  confidence: number
  time_coverage: TimeCoverage | null
  columns: ManifestColumn[]
  group_column: string | null
  source_row_column: string
  row_type_column: string | null
  totals: { count: number; ok: number; mismatch: number; unknown: number; rows: unknown[] }
  transformations: Record<string, number>
  warnings: string[]
  [key: string]: unknown
}

/**
 * The manifest (`<stem>.manifest.json`, schema_version 2), as parsed JSON. Keys are snake_case,
 * exactly as in the file. Field reference: docs/normalized-output.md#manifest.
 */
export interface Manifest {
  schema_version: number
  generator: { name: string; version: string }
  source: { path: string; file_name: string; sha256: string; size_bytes: number; sheets: unknown[] }
  output: { xlsx: string; manifest: string; csv: string[]; sheets: string[] }
  options: Record<string, unknown>
  time_coverage: TimeCoverage | null
  tables: ManifestTable[]
  totals: { sheet: string; count: number; ok: number; mismatch: number; unknown: number; items: unknown[] }
  notes: Record<string, unknown>
  cells: { total: number; in_tables: number; in_totals: number; in_notes: number }
  unassigned_cells: number
  warnings: string[]
  load_hints: Record<string, string>
  [key: string]: unknown
}

/** What `normalize` wrote, and the parsed manifest. */
export interface NormalizeResult extends Omit<NativeNormalizeResult, 'manifestJson'> {
  manifest: Manifest
}

/** Reads an xlsx / xlsm / xlsb / xls / ods workbook or a CSV / TSV file on the calling thread. */
export declare function open(path: PathLike): Workbook

/** Like `open`, on the libuv thread pool (does not block the event loop). */
export declare function openAsync(path: PathLike): Promise<Workbook>

/**
 * Writes `<stem>.normalized.xlsx` + `<stem>.manifest.json` (+ one CSV per table with `csv`) into
 * `outDir` (created if missing), on the libuv thread pool. The source file is never modified.
 * Errors (missing or unreadable file, unknown locale) reject the Promise.
 */
export declare function normalize(path: PathLike, outDir: PathLike, opts?: NormalizeOptions): Promise<NormalizeResult>

/** `normalize` on the calling thread (blocks the event loop). */
export declare function normalizeSync(path: PathLike, outDir: PathLike, opts?: NormalizeOptions): NormalizeResult

/** Options of `new HedgehogSession(...)`. */
export interface SessionOptions {
  /** Folder that bounds file access: relative paths resolve against it and must stay inside it. */
  root: PathLike
  /** Extra files or folders outside `root` the user referenced explicitly (e.g. with `@`). */
  allow?: PathLike[]
}

/** Options of `HedgehogSession.totals`. */
export interface SessionTotalsOptions {
  /** Only this table (`T1`). */
  table?: string
  status?: 'mismatch' | 'ok' | 'unknown'
}

/** MCP tool names, for `HedgehogSession.call`. */
export type ToolName = 'open_file' | 'describe_table' | 'query' | 'cells' | 'totals' | 'list_files'

/**
 * The agent tools of `hedgehog mcp`, in-process: every method resolves to the same compact text
 * an LLM gets from the MCP server (at most ~12,000 characters; a cut ends with a `… cut:` line).
 * A tool error rejects with an Error whose `message` is the one-line text an LLM can act on and
 * whose `code` is `ERR_HEDGEHOG_TOOL`. Files are normalized once per content hash and kept for
 * the session. Calls run off the main thread; calls of one session take turns.
 * Paths are relative to `root` (or absolute and allowed). See docs/mcp.md for the texts.
 */
export declare class HedgehogSession {
  /** Whether this build of the addon contains the agent-tools session. */
  static readonly available: boolean
  /** The tools as MCP `tools/list` entries, for hosts that register them with an LLM. */
  static toolSpecs(): ToolSpec[]
  /** Throws (code `InvalidArg`) when `root` does not exist or an `allow` path is invalid. */
  constructor(options: SessionOptions)
  /** The canonical root folder. */
  readonly root: string
  /** Allows one more file or folder outside the root. */
  allow(path: PathLike): void
  /** Normalizes the file and returns its catalog (tables, columns, time coverage, totals, notes). */
  openFile(path: string): Promise<string>
  /** All columns of one table (dtype, unit, nulls, min/max), the first rows, header cells, totals. */
  describeTable(path: string, table: string): Promise<string>
  /** Read-only SQL (SQLite dialect) over the tables `T1`, `T2`, ... as CSV (default 50, max 200 rows). */
  query(path: string, sql: string, limit?: number): Promise<string>
  /** Original source cells of an A1 range (max 200) as `A1<TAB>value` lines, for citations. */
  cells(path: string, sheet: string, range: string): Promise<string>
  /** The totals check: counts per status, then items, mismatches first. */
  totals(path: string, opts?: SessionTotalsOptions): Promise<string>
  /** Spreadsheet files under `folder` (relative to root) with size and modified date. */
  listFiles(folder?: string, pattern?: string): Promise<string>
  /** Runs a tool by its MCP name with JSON arguments, as an MCP host would. */
  call(name: ToolName | (string & {}), args?: Record<string, unknown>): Promise<string>
}
