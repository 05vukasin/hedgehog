// Tests of the Node bindings against the locally built addon (`npm run build:debug` first).
// Spreadsheets are never committed: every input is a CSV written here (the reader detects CSV
// from content), and the xlsx case opens hedgehog's own normalized output.
import { test, describe, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync, realpathSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { basename, join } from 'node:path'
import { pathToFileURL } from 'node:url'

import { open, openAsync, normalize, normalizeSync, version, Workbook, HedgehogSession, toolSpecs } from '../index.js'

const require = createRequire(import.meta.url)
let dir

const SALES = [
  'Region,City,Budget',
  'North,Beograd,1200.5',
  'North,Čačak,300',
  'South,Niš,450',
  'Total,,1950.5',
].join('\n') + '\n'

before(() => {
  dir = mkdtempSync(join(tmpdir(), 'hedgehog-node-test-'))
  writeFileSync(join(dir, 'sales.csv'), SALES)
  writeFileSync(join(dir, 'garbage.xlsx'), 'PK\u0003\u0004 not really a zip')
})

after(() => rmSync(dir, { recursive: true, force: true }))

test('version matches package.json', () => {
  assert.equal(version(), require('../package.json').version)
})

describe('open', () => {
  test('sheets and sheetNames', () => {
    const wb = open(join(dir, 'sales.csv'))
    assert.ok(wb instanceof Workbook)
    assert.deepEqual(wb.sheetNames, ['sales'])
    assert.deepEqual(wb.sheets(), [
      { name: 'sales', usedRange: 'A1:C5', rows: 5, cols: 3, cells: 14, formulas: 0, merges: 0 },
    ])
    assert.match(wb.describe(), /^sales: A1:C5 · 14 cells/)
    assert.equal(String(wb), 'Workbook(sheets=["sales"])')
  })

  test('getCell: sheet argument, sheet in the reference, first sheet, empty cell', () => {
    const wb = open(join(dir, 'sales.csv'))
    const want = { sheet: 'sales', address: 'C2', value: 1200.5, type: 'number', formula: null, merged: null }
    assert.deepEqual(wb.getCell('sales', 'C2'), want)
    assert.deepEqual(wb.getCell(null, 'sales!$C$2'), want)
    assert.deepEqual(wb.getCell(undefined, 'C2'), want)
    assert.deepEqual(wb.getCell('SALES', 'B3').value, 'Čačak')
    assert.deepEqual(wb.getCell('sales', 'Z99'), {
      sheet: 'sales', address: 'Z99', value: null, type: 'empty', formula: null, merged: null,
    })
  })

  test('getRange', () => {
    const wb = open(join(dir, 'sales.csv'))
    assert.deepEqual(wb.getRange('sales', 'A4:C5'), {
      sheet: 'sales',
      range: 'A4:C5',
      rows: [['South', 'Niš', 450], ['Total', null, 1950.5]],
    })
    assert.throws(() => wb.getRange(null, 'A1:Z1000'), /has 26000 cells, more than maxCells=5000/)
    assert.equal(wb.getRange(null, 'A1:Z1000', 26000).rows.length, 1000)
  })

  test('findText is case- and diacritic-insensitive, with sheet and limit', () => {
    const wb = open(join(dir, 'sales.csv'))
    assert.deepEqual(wb.findText('cacak').map((c) => c.address), ['B3'])
    assert.deepEqual(wb.findText('NIS', { sheet: 'sales' }).map((c) => c.value), ['Niš'])
    assert.deepEqual(wb.findText('1200').map((c) => c.address), ['C2'])
    assert.equal(wb.findText('o', { limit: 2 }).length, 2)
    assert.deepEqual(wb.findText('   '), [])
  })

  test('totals: detected and verified, one sheet or all', () => {
    const wb = open(join(dir, 'sales.csv'))
    const all = wb.totals()
    assert.equal(all.length, 1)
    const t = all[0]
    assert.equal(t.address, 'C5')
    assert.equal(t.level, 'total')
    assert.equal(t.direction, 'vertical')
    assert.deepEqual(t.sources, ['C2:C4'])
    assert.equal(t.label, 'Total')
    assert.equal(t.labelAddress, 'A5')
    assert.equal(t.actual, 1950.5)
    assert.equal(t.expected, 1950.5)
    assert.equal(t.ok, true)
    assert.equal(t.reason, 'match')
    assert.ok(t.evidence.includes('keyword:total'))
    assert.deepEqual(wb.totals('sales'), all)
  })

  test('errors: missing file, not a spreadsheet, bad reference, unknown sheet', () => {
    const missing = join(dir, 'missing.xlsx')
    assert.throws(() => open(missing), (e) => e instanceof Error && e.message.startsWith(`${missing}: `) && /No such file|cannot find/i.test(e.message))
    assert.throws(() => open(join(dir, 'garbage.xlsx')), /garbage\.xlsx: /)
    assert.throws(() => open(42), TypeError)
    const wb = open(join(dir, 'sales.csv'))
    assert.throws(() => wb.getCell('sales', 'not a ref'), /not a cell reference: "not a ref"/)
    assert.throws(() => wb.getCell('sales', 'A1:B2'), /not a cell reference/)
    assert.throws(() => wb.getCell('Nope', 'A1'), /no sheet named "Nope"; sheets: \["sales"\]/)
    assert.throws(() => wb.totals('Nope'), /no sheet named "Nope"/)
  })

  test('openAsync and file: URLs', async () => {
    const wb = await openAsync(pathToFileURL(join(dir, 'sales.csv')))
    assert.ok(wb instanceof Workbook)
    assert.equal(wb.getCell(null, 'C4').value, 450)
    assert.equal(open(pathToFileURL(join(dir, 'sales.csv'))).sheetNames[0], 'sales')
    await assert.rejects(openAsync(join(dir, 'missing.csv')), /missing\.csv: /)
  })
})

describe('normalize', () => {
  test('writes xlsx + manifest and returns the parsed manifest', async () => {
    const out = join(dir, 'out')
    const r = await normalize(join(dir, 'sales.csv'), out)
    assert.equal(r.xlsxPath, join(out, 'sales.normalized.xlsx'))
    assert.equal(r.manifestPath, join(out, 'sales.manifest.json'))
    assert.deepEqual(r.csvPaths, [])
    assert.ok(existsSync(r.xlsxPath))
    assert.equal(r.manifest.schema_version, 2)
    assert.equal(r.manifest.tables.length, 1)
    const t = r.manifest.tables[0]
    assert.equal(t.id, 'T1')
    assert.equal(t.rows, 3)
    assert.ok(t.columns.some((c) => c.name === 'Budget' && c.dtype === 'number'))
    assert.equal(r.manifest.totals.ok, 1)
    // same object as the file, keys in the file's order
    const onDisk = JSON.parse(readFileSync(r.manifestPath, 'utf8'))
    assert.deepEqual(r.manifest, onDisk)
    assert.deepEqual(Object.keys(r.manifest), Object.keys(onDisk))
    assert.equal(Object.keys(r.manifest)[0], 'schema_version')
  })

  test('options: csv, keepTotals, locale, rulesOnly, modelMaxCells', async () => {
    const out = join(dir, 'out-opts')
    const r = await normalize(join(dir, 'sales.csv'), out, {
      csv: true, keepTotals: true, locale: 'en-US', rulesOnly: true, modelMaxCells: 10,
    })
    assert.equal(r.csvPaths.length, 1)
    assert.match(readFileSync(r.csvPaths[0], 'utf8'), /Beograd/)
    assert.equal(r.manifest.options.keep_totals, true)
    assert.equal(r.manifest.options.csv, true)
    assert.equal(r.manifest.options.cell_roles, 'rules')
    assert.equal(r.manifest.tables[0].row_type_column, '_row_type')
  })

  test('the normalized xlsx opens again', async () => {
    const r = await normalize(join(dir, 'sales.csv'), join(dir, 'out-xlsx'))
    const wb = open(r.xlsxPath)
    assert.deepEqual(wb.sheetNames, r.manifest.output.sheets)
    assert.ok(wb.findText('Beograd').length >= 1)
  })

  test('normalizeSync gives the same result', async () => {
    const a = await normalize(join(dir, 'sales.csv'), join(dir, 'out-a'))
    const b = normalizeSync(join(dir, 'sales.csv'), join(dir, 'out-b'))
    assert.deepEqual(b.manifest, a.manifest)
  })

  test('errors reject the Promise', async () => {
    const missing = join(dir, 'missing.xlsx')
    await assert.rejects(normalize(missing, join(dir, 'out-err')), (e) => e.message.startsWith(`${missing}: `))
    await assert.rejects(normalize(join(dir, 'sales.csv'), join(dir, 'out-err'), { locale: 'xx-nope' }), /unknown locale "xx-nope"/)
    await assert.rejects(normalize(join(dir, 'garbage.xlsx'), join(dir, 'out-err')), /garbage\.xlsx/)
    await assert.rejects(normalize(null, join(dir, 'out-err')), TypeError)
    assert.throws(() => normalizeSync(missing, join(dir, 'out-err')), /missing\.xlsx/)
  })

  test('does not block the event loop', async () => {
    const rows = ['Region,Month,Units,Revenue']
    for (let i = 0; i < 5000; i++) rows.push(`R${i % 7},2024-${String((i % 12) + 1).padStart(2, '0')},${i % 97},${(i * 1.5).toFixed(2)}`)
    const big = join(dir, 'big.csv')
    writeFileSync(big, rows.join('\n') + '\n')
    let ticks = 0
    const timer = setInterval(() => ticks++, 5)
    const t0 = performance.now()
    const r = await normalize(big, join(dir, 'out-big'))
    const ms = performance.now() - t0
    clearInterval(timer)
    assert.equal(r.manifest.tables[0].rows, 5000)
    // a blocked loop would fire the interval at most once
    assert.ok(ticks >= Math.min(3, Math.floor(ms / 5) - 1), `${ticks} ticks in ${ms.toFixed(0)} ms`)
  })
})

describe('HedgehogSession (agent tools)', () => {
  test('is available and lists the six MCP tools', () => {
    assert.equal(HedgehogSession.available, true)
    assert.deepEqual(HedgehogSession.toolSpecs().map((t) => t.name), toolSpecs().map((t) => t.name))
    assert.deepEqual(toolSpecs().map((t) => t.name), ['open_file', 'describe_table', 'query', 'cells', 'totals', 'list_files'])
    assert.equal(toolSpecs()[0].inputSchema.type, 'object')
  })

  test('tools return the contract texts', async () => {
    const s = new HedgehogSession({ root: dir })
    // canonical root; on Windows a \\?\ path, so only the last part is compared
    assert.equal(basename(s.root), basename(realpathSync(dir)))
    const catalog = await s.openFile('sales.csv')
    assert.match(catalog, /^FILE sales\.csv · 1 sheet/)
    assert.match(catalog, /^T1 /m)
    assert.match(await s.describeTable('sales.csv', 'T1'), /Budget \| number/)
    assert.equal(await s.query('sales.csv', 'SELECT SUM(Budget) AS b FROM T1'), 'b\n1950.5\n(1 row)')
    assert.match(await s.query('sales.csv', 'SELECT City FROM T1 ORDER BY City', 2), /\(3 rows, showing first 2\)$/)
    assert.match(await s.cells('sales.csv', 'sales', 'B3'), /B3\tČačak/)
    assert.match(await s.totals('sales.csv', { status: 'ok' }), /^ok,T1,sales!C5,Total/m)
    assert.match(await s.listFiles(undefined, '*.csv'), /sales\.csv/)
    assert.equal(await s.call('query', { path: 'sales.csv', sql: 'SELECT 1 AS x' }), 'x\n1\n(1 row)')
  })

  test('several files: f<k>.T1 joins tables of different files', async () => {
    const d = mkdtempSync(join(tmpdir(), 'hedgehog-node-multi-'))
    try {
      writeFileSync(join(d, 'orders.csv'), 'orderID,employeeID\n1,7\n2,8\n3,7\n')
      writeFileSync(join(d, 'lines.csv'), 'orderID,amount\n1,10\n1,5\n2,20\n3,1\n')
      const s = new HedgehogSession({ root: d })
      await s.openFile('orders.csv')
      const cat = await s.openFile('lines.csv')
      assert.match(cat, /^FILES open in this session .*f1 = orders\.csv \(T1\), f2 = lines\.csv \(T1; this file\)/m)
      assert.match(cat, /^JOIN KEYS f1\.T1\.orderID = f2\.T1\.orderID/m)
      assert.deepEqual(s.openFiles(), [{ prefix: 'f1', path: 'orders.csv' }, { prefix: 'f2', path: 'lines.csv' }])
      const sql = 'SELECT o.employeeID, SUM(l.amount) AS s FROM T1 o JOIN f2.T1 l ON o.orderID = l.orderID GROUP BY 1 ORDER BY 1'
      assert.equal(await s.query('orders.csv', sql), 'employeeID,s\n7,16\n8,20\n(2 rows)')
    } finally {
      rmSync(d, { recursive: true, force: true })
    }
  })

  test('openFile with a question lists the tables it matches first (B150)', async () => {
    const d = mkdtempSync(join(tmpdir(), 'hedgehog-node-question-'))
    try {
      writeFileSync(join(d, 'book.csv'), 'City,Amount\nGraz,1\nLinz,2\nWien,3\n')
      const s = new HedgehogSession({ root: d })
      // one table: the question changes nothing (multi-table catalogs: agent/tests/snapshots.rs)
      const plain = await s.openFile('book.csv')
      const asked = await s.openFile('book.csv', { question: 'Which city has the largest amount?' })
      const body = (t) => t.replace(/\n\((cached|normalized in [\d.]+ s)\)$/, '')
      assert.match(plain, /^T1 /m)
      assert.equal(body(asked), body(plain))
      assert.equal(toolSpecs()[0].inputSchema.properties.question.type, 'string')
      const viaCall = await s.call('open_file', { path: 'book.csv', question: 'What is x?' })
      assert.match(viaCall, /^FILE book\.csv/)
    } finally {
      rmSync(d, { recursive: true, force: true })
    }
  })

  test('calls run concurrently without blocking and take turns on the session', async () => {
    const s = new HedgehogSession({ root: dir })
    const results = await Promise.all(Array.from({ length: 8 }, (_, i) => s.query('sales.csv', `SELECT ${i} AS n`)))
    assert.deepEqual(results, Array.from({ length: 8 }, (_, i) => `n\n${i}\n(1 row)`))
  })

  test('tool errors reject with ERR_HEDGEHOG_TOOL and a one-line message', async () => {
    const s = new HedgehogSession({ root: dir })
    const isTool = (re) => (e) => e.code === 'ERR_HEDGEHOG_TOOL' && re.test(e.message) && !e.message.includes('\n')
    await assert.rejects(s.query('sales.csv', 'SELECT nope FROM T1'), isTool(/no such column: nope.*columns are T1: Region, City, Budget/))
    await assert.rejects(s.query('sales.csv', 'DELETE FROM T1'), isTool(/./))
    await assert.rejects(s.describeTable('sales.csv', 'T9'), isTool(/no table "T9"; tables are T1/))
    await assert.rejects(s.openFile('../outside.csv'), isTool(/no such file: \.\.\/outside\.csv/))
    await assert.rejects(s.openFile('missing.xlsx'), isTool(/missing\.xlsx/))
    await assert.rejects(s.call('bogus', {}), isTool(/unknown tool "bogus"/))
    assert.throws(() => new HedgehogSession({ root: join(dir, 'no-such-dir') }), /no-such-dir/)
    assert.throws(() => new HedgehogSession({}), TypeError)
  })

  test('allow: files outside the root', async () => {
    const outside = mkdtempSync(join(tmpdir(), 'hedgehog-node-allow-'))
    try {
      writeFileSync(join(outside, 'q3.csv'), SALES)
      const s = new HedgehogSession({ root: dir })
      await assert.rejects(s.openFile(join(outside, 'q3.csv')), (e) => e.code === 'ERR_HEDGEHOG_TOOL')
      s.allow(join(outside, 'q3.csv'))
      assert.match(await s.openFile(join(outside, 'q3.csv')), /^FILE /)
      const s2 = new HedgehogSession({ root: dir, allow: [pathToFileURL(outside)] })
      assert.match(await s2.openFile(join(outside, 'q3.csv')), /^FILE /)
    } finally {
      rmSync(outside, { recursive: true, force: true })
    }
  })
})
