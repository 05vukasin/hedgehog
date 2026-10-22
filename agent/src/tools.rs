//! Tool definitions (name, description, JSON Schema of the arguments) and a dispatcher that takes
//! JSON arguments, for MCP and other JSON-speaking hosts.

use serde_json::{json, Value};

use crate::{Session, ToolError, ToolResult, QUERY_DEFAULT_LIMIT, QUERY_MAX_LIMIT};

pub const TOOL_NAMES: [&str; 6] = ["open_file", "describe_table", "query", "cells", "totals", "list_files"];

const PATH_DESC: &str = "Spreadsheet path relative to the working folder, as the user referenced it (xlsx, xlsm, xlsb, xls, ods, csv, tsv).";

/// The tools as MCP `tools/list` entries: `{name, title, description, inputSchema, annotations}`.
pub fn tool_specs() -> Value {
    let path = json!({"type": "string", "description": PATH_DESC});
    let ro = json!({"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false});
    json!([
        {
            "name": "open_file",
            "title": "Open spreadsheet",
            "description": "Normalize a spreadsheet and return its catalog: time coverage (which years/periods the file holds), \
                the tables T1, T2, … with source sheet!range, title, row counts, total rows, columns (type, unit, value range), \
                group column and totals check, plus notes and warnings. Call this first for every file. \
                Fast; cached per file content. Pass the user's question to get the tables it is about first and in full, \
                the other tables one line each (fewer tokens on files with many tables).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": path,
                    "question": {"type": "string", "description": "Optional: the user's question, as asked. The catalog then lists \
                        the tables it matches (by words, column names, values, years) first and in full, the others one line each; \
                        every table is still listed."}
                },
                "required": ["path"]
            },
            "annotations": ro,
        },
        {
            "name": "describe_table",
            "title": "Describe table",
            "description": "All columns of one table (exact SQL name, dtype, unit, nulls, distinct, min..max or example, source column and \
                header cells), its source range, group column, total rows and the first 5 rows. Use it when the catalog lists a \
                table briefly or before writing SQL against long column names.",
            "inputSchema": {
                "type": "object",
                "properties": {"path": path, "table": {"type": "string", "description": "Table id from the catalog, e.g. T2."}},
                "required": ["path", "table"]
            },
            "annotations": ro,
        },
        {
            "name": "query",
            "title": "SQL query",
            "description": format!("Run one read-only SQL SELECT (SQLite dialect) over the file's tables and get CSV back. \
                Views: T1, T2, … hold the data rows; T1_all, … also hold the total/subtotal rows with _row_type \
                ('data'|'subtotal'|'total'; also 'average' and 'of_which' rows, which are no sums) — use those for overall figures, never add them to data rows. Every view has _src_row \
                (row number in the source sheet) for citing cells. A table with one column per year/month/day also has \
                T1_long (its period columns as rows: period, period_label, value; T1_long_all with total rows): filter WHERE period = '2025'. \
                Tables with the same columns on several sheets also have a union view U1 (U1_all with total rows) with _table, sheet and title columns. \
                With several files open, f1.T1, f2.T1, … name the tables of each open file (the catalog lists them and the join keys), \
                so one query joins files; plain T1 is the file in path. Quote column names with double quotes: \
                SELECT \"Revenue / Q1\" FROM T1. Dates are ISO text 'YYYY-MM-DD'; percentages are fractions (0.15 = 15%). \
                This is SQLite, not MySQL/Postgres: month strftime('%m', d), year strftime('%Y', d), \
                year-month strftime('%Y-%m', d), days between julianday(b) - julianday(a); LIMIT n, not TOP n. \
                Use SQL for sums, counts, top-N and filters instead of reading rows. At most {QUERY_MAX_LIMIT} rows are returned, \
                followed by the total row count."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": path,
                    "sql": {"type": "string", "description": "One SELECT or WITH … SELECT statement."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": QUERY_MAX_LIMIT, "default": QUERY_DEFAULT_LIMIT,
                              "description": "Rows to return."}
                },
                "required": ["path", "sql"]
            },
            "annotations": ro,
        },
        {
            "name": "cells",
            "title": "Source cells",
            "description": "Original cells of the source sheet (as stored in the file, before normalization), max 200 cells per call, \
                as A1<TAB>value lines with formulas and merged areas marked. Use it to cite or check exact values (a total, a header) \
                — cite as Sheet!B13.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": path,
                    "sheet": {"type": "string", "description": "Source sheet name (from the catalog's ← Sheet!range)."},
                    "range": {"type": "string", "description": "A1 range such as B3:F12, or one cell such as B13."}
                },
                "required": ["path", "sheet", "range"]
            },
            "annotations": ro,
        },
        {
            "name": "totals",
            "title": "Totals check",
            "description": "The file's total and subtotal cells recomputed from their parts: counts per status, then up to 50 items \
                (mismatches first) with cell, label, value, recomputed value, source ranges and reason.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": path,
                    "table": {"type": "string", "description": "Only totals of this table, e.g. T1."},
                    "status": {"type": "string", "enum": ["mismatch", "ok", "unknown"], "description": "Only items with this status."}
                },
                "required": ["path"]
            },
            "annotations": ro,
        },
        {
            "name": "list_files",
            "title": "List spreadsheets",
            "description": "Spreadsheet files under a folder of the working folder (recursive), with size and modified date; at most 200 lines.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "folder": {"type": "string", "description": "Folder relative to the working folder; default '.'."},
                    "pattern": {"type": "string", "description": "Case-insensitive filter on the path: a glob (*.csv), words that must all \
                        appear ('linz 2022'), or alternatives ('preis|price|cpi'). File names are often in the data's language."}
                }
            },
            "annotations": ro,
        }
    ])
}

fn arg<'a>(args: &'a Value, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|n| args.get(*n).and_then(Value::as_str))
}

fn req<'a>(args: &'a Value, names: &[&str]) -> Result<&'a str, ToolError> {
    arg(args, names).ok_or_else(|| ToolError(format!("missing argument \"{}\" (a string)", names[0])))
}

fn limit_arg(args: &Value) -> Result<Option<usize>, ToolError> {
    match args.get("limit") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(Some(n.as_f64().unwrap_or(0.0).max(1.0) as usize)),
        Some(Value::String(s)) => s.trim().parse().map(Some).map_err(|_| ToolError("limit must be a number".into())),
        Some(_) => Err(ToolError("limit must be a number".into())),
    }
}

impl Session {
    /// Runs tool `name` with JSON `args` (an object). Unknown tools are an error too; check
    /// [`TOOL_NAMES`] first when that must be told apart.
    pub fn call(&mut self, name: &str, args: &Value) -> ToolResult {
        let empty = json!({});
        let args = if args.is_object() { args } else { &empty };
        match name {
            "open_file" => self.open_file_for(req(args, &["path", "file"])?, arg(args, &["question"])),
            "describe_table" => self.describe_table(req(args, &["path", "file"])?, req(args, &["table"])?),
            "query" => self.query(req(args, &["path", "file"])?, req(args, &["sql"])?, limit_arg(args)?),
            "cells" => self.cells(req(args, &["path", "file"])?, arg(args, &["sheet"]).unwrap_or(""), req(args, &["range"])?),
            "totals" => self.totals(
                req(args, &["path", "file"])?,
                arg(args, &["table"]).filter(|s| !s.is_empty()),
                arg(args, &["status"]).filter(|s| !s.is_empty()),
            ),
            "list_files" => self.list_files(arg(args, &["folder"]), arg(args, &["pattern", "match"])),
            other => Err(ToolError(format!("unknown tool {other:?}; tools: {}", TOOL_NAMES.join(", ")))),
        }
    }
}
