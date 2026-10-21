//! `hedgehog mcp`: an MCP tool server over stdio (JSON-RPC 2.0, one message per line).
//!
//! A thin wrapper over [`hh_agent::Session`]: `initialize`, `ping`, `tools/list`, `tools/call`.
//! Nothing but protocol messages goes to stdout; logs go to stderr.
//!
//! Requests run one at a time, in arrival order, on a worker thread that owns the session; the
//! main thread keeps reading, answers `ping` and `tools/list` at once, and handles
//! `notifications/cancelled`: a queued request is dropped, a running one is stopped (a query is
//! interrupted; waiting for a normalization ends and the normalization finishes in the background
//! into the disk cache). A cancelled request gets no response. Other notifications are ignored.

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use hh_agent::{Options, Session, TOOL_NAMES};
use serde_json::{json, Value};

/// Newest protocol version first; an older version a client asks for is answered with itself.
const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Lines longer than this are rejected (a request is a few KB at most).
const MAX_LINE: usize = 4 << 20;

const INSTRUCTIONS: &str = "hedgehog reads messy spreadsheets (xlsx, xls, xlsb, ods, csv) and normalizes them into clean tables. \
Open a file first (open_file) and read its catalog: check TIME before answering questions about a period the file may not cover. \
Use query (SQLite SQL over T1, T2, …; T<n>_all includes total rows) for numbers instead of reading rows. \
Cite source cells as Sheet!B13 (use _src_row and the source column) or check them with cells. \
If the files cannot answer the question, say so and say what they contain.";

pub fn run(args: &[&str]) -> Result<(), String> {
    let mut root: Option<PathBuf> = None;
    let mut allow: Vec<PathBuf> = Vec::new();
    let mut opts = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match *a {
            "--root" => root = Some(PathBuf::from(val()?)),
            "--allow" => allow.push(PathBuf::from(val()?)),
            "--query-timeout-ms" => {
                opts.query_timeout = Duration::from_millis(val()?.parse().map_err(|_| "--query-timeout-ms needs a number")?)
            }
            "--max-file-mb" => {
                opts.max_file_bytes = val()?.parse::<u64>().map_err(|_| "--max-file-mb needs a number")? << 20
            }
            "--max-cells" => opts.max_cells = val()?.parse().map_err(|_| "--max-cells needs a number")?,
            "--max-seconds" => {
                opts.max_seconds = Duration::from_secs(val()?.parse().map_err(|_| "--max-seconds needs a number")?)
            }
            _ => return Err(format!("unknown argument `{a}`")),
        }
    }
    let root = match root {
        Some(r) => r,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let session = Session::with_options(&root, &allow, opts).map_err(|e| e.0)?;
    log(&format!("serving tools over stdio; root {}", session.bounds().root().display()));
    if cfg!(panic = "abort") {
        log("warning: built with panic = \"abort\": a crash in one tool call ends the server (build with the release profile, which unwinds)");
    }
    let stdin = std::io::stdin();
    serve(session, stdin.lock(), std::io::stdout()).map_err(|e| e.to_string())
}

fn log(msg: &str) {
    eprintln!("[hedgehog mcp] {msg}");
}

/// Requests waiting for or running on the worker, by id (JSON text), for cancellation.
#[derive(Default)]
struct Calls {
    queued: HashSet<String>,
    running: Option<String>,
    /// The running request was cancelled: its response is dropped.
    running_cancelled: bool,
}

/// Work for the worker thread: one message, or a batch answered as one array.
enum Job {
    One(Value),
    Batch(Vec<Value>),
}

/// The id of a request as a key (`None` for notifications and non-objects).
fn id_key(msg: &Value) -> Option<String> {
    let obj = msg.as_object()?;
    obj.get("method")?;
    obj.get("id").map(Value::to_string)
}

/// Reads messages from `input` until EOF and writes responses to `out`.
pub fn serve(session: Session, mut input: impl BufRead, out: impl Write + Send + 'static) -> std::io::Result<()> {
    let out = Arc::new(Mutex::new(out));
    let calls = Arc::new(Mutex::new(Calls::default()));
    let cancel = session.cancel_flag();
    let (tx, rx) = mpsc::channel::<Job>();
    let worker = {
        let (out, calls, cancel) = (out.clone(), calls.clone(), cancel.clone());
        std::thread::Builder::new().name("hedgehog-mcp".into()).spawn(move || work(session, rx, &out, &calls, &cancel))?
    };
    let send = |v: &Value| -> std::io::Result<()> { write(&mut *out.lock().unwrap_or_else(|e| e.into_inner()), v) };
    let mut buf = Vec::new();
    let res = loop {
        buf.clear();
        let n = match std::io::Read::take(input.by_ref(), MAX_LINE as u64 + 1).read_until(b'\n', &mut buf) {
            Ok(n) => n,
            Err(e) => break Err(e),
        };
        if n == 0 {
            break Ok(());
        }
        if buf.len() > MAX_LINE && !buf.ends_with(b"\n") {
            // Skip the rest of the oversized line.
            let mut sink = Vec::new();
            if let Err(e) = input.read_until(b'\n', &mut sink) {
                break Err(e);
            }
            send(&error(Value::Null, -32600, "request too large"))?;
            continue;
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let job = match serde_json::from_str::<Value>(line) {
            Err(e) => {
                send(&error(Value::Null, -32700, &format!("parse error: {e}")))?;
                continue;
            }
            Ok(Value::Array(batch)) => {
                for m in &batch {
                    note_cancel(m, &calls, &cancel);
                }
                Job::Batch(batch)
            }
            Ok(msg) => {
                if note_cancel(&msg, &calls, &cancel) {
                    continue;
                }
                // Answered at once, also while a call runs.
                let method = msg.get("method").and_then(Value::as_str);
                if let (Some("ping" | "tools/list"), Some(id)) = (method, msg.get("id")) {
                    let r = if method == Some("ping") { json!({}) } else { json!({"tools": hh_agent::tool_specs()}) };
                    send(&ok(id.clone(), r))?;
                    continue;
                }
                Job::One(msg)
            }
        };
        if let Job::One(m) = &job
            && let Some(k) = id_key(m)
        {
            calls.lock().unwrap_or_else(|e| e.into_inner()).queued.insert(k);
        }
        if tx.send(job).is_err() {
            break Ok(());
        }
    };
    drop(tx);
    let _ = worker.join();
    res
}

/// Handles `notifications/cancelled` (true when `msg` is one).
fn note_cancel(msg: &Value, calls: &Mutex<Calls>, cancel: &AtomicBool) -> bool {
    if msg.get("method").and_then(Value::as_str) != Some("notifications/cancelled") || msg.get("id").is_some() {
        return false;
    }
    let Some(id) = msg.pointer("/params/requestId") else { return true };
    let k = id.to_string();
    let mut c = calls.lock().unwrap_or_else(|e| e.into_inner());
    if c.running.as_ref() == Some(&k) {
        c.running_cancelled = true;
        cancel.store(true, Ordering::Relaxed);
        log(&format!("cancel {k}: stopping"));
    } else if c.queued.remove(&k) {
        log(&format!("cancel {k}: dropped before it ran"));
    }
    true
}

/// The worker: runs requests in order and writes their responses.
fn work(mut session: Session, rx: mpsc::Receiver<Job>, out: &Mutex<impl Write>, calls: &Mutex<Calls>, cancel: &AtomicBool) {
    let send = |v: &Value| {
        let _ = write(&mut *out.lock().unwrap_or_else(|e| e.into_inner()), v);
    };
    for job in rx {
        match job {
            Job::Batch(batch) => {
                let replies: Vec<Value> = batch.into_iter().filter_map(|m| handle(&mut session, m)).collect();
                if !replies.is_empty() {
                    send(&Value::Array(replies));
                }
            }
            Job::One(msg) => {
                let key = id_key(&msg);
                if let Some(k) = &key {
                    let mut c = calls.lock().unwrap_or_else(|e| e.into_inner());
                    if !c.queued.remove(k) {
                        continue; // cancelled while queued
                    }
                    c.running = Some(k.clone());
                    c.running_cancelled = false;
                    cancel.store(false, Ordering::Relaxed);
                }
                let reply = handle(&mut session, msg);
                let dropped = {
                    let mut c = calls.lock().unwrap_or_else(|e| e.into_inner());
                    c.running = None;
                    std::mem::take(&mut c.running_cancelled)
                };
                cancel.store(false, Ordering::Relaxed);
                match reply {
                    Some(r) if !dropped => send(&r),
                    _ => {}
                }
            }
        }
    }
}

fn write(out: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let mut s = serde_json::to_string(v).unwrap_or_else(|_| "{}".into());
    s.push('\n');
    out.write_all(s.as_bytes())?;
    out.flush()
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// One message; `None` for notifications and responses.
fn handle(session: &mut Session, msg: Value) -> Option<Value> {
    let Value::Object(obj) = &msg else {
        return Some(error(Value::Null, -32600, "invalid request: expected a JSON object"));
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response to a request we never sent, or garbage.
        return match id {
            Some(id) if !obj.contains_key("result") && !obj.contains_key("error") => {
                Some(error(id, -32600, "invalid request: no method"))
            }
            _ => None,
        };
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        // Notification: notifications/initialized, notifications/cancelled, … — nothing to do.
        return None;
    };
    Some(match method {
        "initialize" => ok(id, initialize(session, &params)),
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({"tools": hh_agent::tool_specs()})),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "invalid params: tools/call needs a tool name"));
            };
            if !TOOL_NAMES.contains(&name) {
                return Some(error(id, -32602, &format!("unknown tool: {name}")));
            }
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let t0 = Instant::now();
            let res = call_isolated(session, name, &args);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            let (text, is_error) = match res {
                Ok(t) => (t, false),
                Err(e) => (e.0, true),
            };
            log(&format!(
                "{name} {:.0} ms{}{}",
                ms,
                peak_memory().map(|p| format!(", RSS {p}")).unwrap_or_default(),
                if is_error { format!(" error: {text}") } else { String::new() }
            ));
            ok(id, json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        other => error(id, -32601, &format!("method not found: {other}")),
    })
}

/// One tool call; a panic in it (a bug hit by a malformed file) becomes this call's one-line
/// error instead of ending the server. Needs a build with `panic = "unwind"` (the release
/// profile); see [`run`] for the warning otherwise.
fn call_isolated(session: &mut Session, name: &str, args: &Value) -> Result<String, hh_agent::ToolError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.call(name, args))) {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(hh_agent::ToolError(format!(
                "internal error in {name} ({}); the file may be malformed — the server is still running, try another file or tool",
                msg.split_whitespace().collect::<Vec<_>>().join(" ")
            )))
        }
    }
}

/// Resident and peak resident memory of this process (Linux only).
fn peak_memory() -> Option<String> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let mb = |key: &str| -> Option<u64> {
        let kb: u64 = s.lines().find_map(|l| l.strip_prefix(key))?.trim().trim_end_matches("kB").trim().parse().ok()?;
        Some(kb / 1024)
    };
    Some(format!("{} MB (peak {} MB)", mb("VmRSS:")?, mb("VmHWM:")?))
}

fn initialize(session: &mut Session, params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOLS[0]);
    let version = if PROTOCOLS.contains(&asked) { asked } else { PROTOCOLS[0] };
    if let Some(list) = params.pointer("/_meta/hedgehog/allow").and_then(Value::as_array) {
        for p in list.iter().filter_map(Value::as_str) {
            if let Err(e) = session.allow(p) {
                log(&format!("allow {p}: {e}"));
            }
        }
    }
    let client = params.pointer("/clientInfo/name").and_then(Value::as_str).unwrap_or("?");
    log(&format!("initialize from {client}, protocol {version}"));
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "hedgehog", "title": "hedgehog spreadsheets", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}
