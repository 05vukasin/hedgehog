// hedgehog-sheets: public entry point. The native addon is loaded by the napi-rs generated
// `binding.js` (prebuilt `hedgehog-sheets-<platform>` package, or a local `hedgehog.<platform>.node`).
// This file only accepts `URL` paths on top of strings and holds the agent-tools session.
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

// TODO(agent-tools): back this with the Rust `Session` of `hedgehog mcp` (open_file,
// describe_table, query, cells, totals, list_files returning compact text, see
// docs/research/hedgehog_mcp_contract.md) once that crate is merged to master. The API and its
// types (index.d.ts) are final; only the native half is missing.
class HedgehogSession {
  /** false until the agent-tools session ships in the native addon. */
  static get available() {
    return false
  }

  constructor(options) {
    const err = new Error(
      'HedgehogSession is not available in this version of hedgehog-sheets yet; ' +
        'use open() / normalize(), or the `hedgehog mcp` server, meanwhile',
    )
    err.code = 'ERR_HEDGEHOG_NOT_IMPLEMENTED'
    void options
    throw err
  }
}

exports.HedgehogSession = HedgehogSession
