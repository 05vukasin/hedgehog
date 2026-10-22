// hedgehog-sheets: public entry point. The native addon is loaded by the napi-rs generated
// `binding.js` (prebuilt `hedgehog-sheets-<platform>` package, or a local `hedgehog.<platform>.node`).
// This file accepts `URL` paths on top of strings, parses the manifest and wraps the agent-tools
// session.
'use strict'

const { fileURLToPath } = require('node:url')
const native = require('./binding.js')

/** @param {string | URL} p */
function toPath(p) {
  if (p instanceof URL) return fileURLToPath(p)
  if (typeof p !== 'string') throw new TypeError(`path must be a string or a file: URL, got ${typeof p}`)
  return p
}

/** Native result → public result: the manifest text parsed (key order as in the file). */
function withManifest(r) {
  const { manifestJson, ...rest } = r
  return { ...rest, manifest: JSON.parse(manifestJson) }
}

exports.Workbook = native.Workbook
exports.version = native.version

/** Reads a workbook (format detected from content) on the calling thread. */
exports.open = function open(path) {
  return native.open(toPath(path))
}

/** Reads a workbook on the libuv thread pool. */
exports.openAsync = function openAsync(path) {
  try {
    return native.openAsync(toPath(path))
  } catch (e) {
    return Promise.reject(e)
  }
}

/** Normalizes `path` into `outDir` on the libuv thread pool; resolves to paths + manifest. */
exports.normalize = function normalize(path, outDir, opts) {
  try {
    return native.normalize(toPath(path), toPath(outDir), opts).then(withManifest)
  } catch (e) {
    return Promise.reject(e)
  }
}

/** `normalize` on the calling thread. */
exports.normalizeSync = function normalizeSync(path, outDir, opts) {
  return withManifest(native.normalizeSync(toPath(path), toPath(outDir), opts))
}

/** A tool error (one line an LLM can act on) gets code ERR_HEDGEHOG_TOOL. */
function toolError(e) {
  if (e && e.code === 'GenericFailure') e.code = 'ERR_HEDGEHOG_TOOL'
  throw e
}

/** Promise of a native tool call; argument errors reject too instead of throwing. */
function tool(fn) {
  try {
    return fn().catch(toolError)
  } catch (e) {
    return Promise.reject(e)
  }
}

/**
 * The agent tools of `hedgehog mcp` in-process (`hh_agent::Session`): each method resolves to the
 * compact text the MCP server returns. Calls run on the libuv thread pool; calls of one session
 * take turns (it owns SQLite connections).
 */
class HedgehogSession {
  /** Whether this build of the addon contains the agent-tools session. */
  static get available() {
    return typeof native.NativeSession === 'function'
  }

  /** The tools as MCP `tools/list` entries (name, title, description, inputSchema, annotations). */
  static toolSpecs() {
    return native.toolSpecs()
  }

  #s

  constructor(options) {
    if (!options || options.root === undefined) throw new TypeError('HedgehogSession needs { root }')
    const allow = (options.allow || []).map(toPath)
    this.#s = new native.NativeSession({ root: toPath(options.root), allow })
  }

  /** The canonical root folder. */
  get root() {
    return this.#s.root
  }

  /** Allows one more file or folder outside the root (e.g. one the user referenced with `@`). */
  allow(path) {
    this.#s.allow(toPath(path))
  }

  openFile(path, opts) {
    const o = opts || {}
    return tool(() => this.#s.openFile(path, o.question))
  }

  describeTable(path, table) {
    return tool(() => this.#s.describeTable(path, table))
  }

  query(path, sql, limit) {
    return tool(() => this.#s.query(path, sql, limit))
  }

  cells(path, sheet, range) {
    return tool(() => this.#s.cells(path, sheet, range))
  }

  totals(path, opts) {
    const o = opts || {}
    return tool(() => this.#s.totals(path, o.table, o.status))
  }

  listFiles(folder, pattern) {
    return tool(() => this.#s.listFiles(folder, pattern))
  }

  /** The files opened so far, in order, with their SQL prefix (`f2` in `SELECT … FROM f2.T1`). */
  openFiles() {
    return this.#s.openFiles()
  }

  /** Runs a tool by its MCP name (`open_file`, `query`, ...) with JSON arguments. */
  call(name, args) {
    return tool(() => this.#s.call(name, args))
  }
}

exports.HedgehogSession = HedgehogSession
exports.toolSpecs = native.toolSpecs
