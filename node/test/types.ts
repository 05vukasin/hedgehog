// Compile-only check of the public types (`npm run typecheck`); not executed.
import {
  open,
  openAsync,
  normalize,
  normalizeSync,
  version,
  Workbook,
  HedgehogSession,
  type CellInfo,
  type CellValue,
  type Manifest,
  type NormalizeOptions,
  type NormalizeResult,
  type TotalInfo,
} from '../index'

async function main(): Promise<void> {
  const v: string = version()
  const wb: Workbook = open('book.xlsx')
  const wb2: Workbook = await openAsync(new URL('file:///tmp/book.xlsx'))
  const names: string[] = wb.sheetNames
  const cell: CellInfo = wb.getCell(names[0], 'B2')
  const value: CellValue = cell.value
  const kind: 'empty' | 'bool' | 'number' | 'text' | 'date' | 'error' = cell.type
  const rows: CellValue[][] = wb2.getRange(null, 'Sheet1!A1:C3', 100).rows
  const hits: CellInfo[] = wb.findText('total', { sheet: 'Sheet1', limit: 5 })
  const totals: TotalInfo[] = wb.totals()
  const ok: boolean | null = totals[0].ok

  const opts: NormalizeOptions = { keepTotals: true, csv: true, locale: 'de-DE', rulesOnly: false, modelMaxCells: 1_000_000 }
  const r: NormalizeResult = await normalize('book.xlsx', 'out', opts)
  const m: Manifest = r.manifest
  const coverage = m.time_coverage?.from
  const dtypes = m.tables.flatMap((t) => t.columns.map((c) => c.dtype))
  const sync: NormalizeResult = normalizeSync('book.xlsx', 'out')
  const paths: string[] = [r.xlsxPath, r.manifestPath, ...r.csvPaths, sync.xlsxPath]

  if (HedgehogSession.available) {
    const s = new HedgehogSession({ root: '.', allow: ['/data/q3.xlsx'] })
    const catalog: string = await s.openFile('q3.xlsx')
    const text: string[] = await Promise.all([
      s.describeTable('q3.xlsx', 'T1'),
      s.query('q3.xlsx', 'SELECT * FROM T1', 10),
      s.cells('q3.xlsx', 'Sheet1', 'A1:C3'),
      s.totals('q3.xlsx', { table: 'T1', status: 'mismatch' }),
      s.listFiles('.', '*.xlsx'),
    ])
    const specs: string[] = HedgehogSession.toolSpecs().map((t) => t.name)
    const viaCall: string = await s.call('query', { path: 'q3.xlsx', sql: 'SELECT 1' })
    s.allow(new URL('file:///data/'))
    const root: string = s.root
    void [catalog, text, specs, viaCall, root]
  }
  void [v, value, kind, rows, hits, ok, coverage, dtypes, paths]
}

void main
