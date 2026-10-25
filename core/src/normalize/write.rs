//! Writers: the normalized xlsx (README, one sheet per table, Totals, Notes) and per-table CSVs.

use std::io::Write as _;
use std::path::Path;

use rust_xlsxwriter::{
    DocProperties, ExcelDateTime, Format, Table as XTable, TableColumn, Workbook, Worksheet, XlsxError,
};

use super::manifest::{Source, load_hints};
use super::typing::{fmt_num, iso};
use super::{Error, Normalized, Out, Table};
use crate::model::a1;

const MAX_ROWS: usize = 1_048_575;
const MAX_COLS: usize = 16_384;
const WIDTH_SAMPLE: usize = 300;

fn xerr(e: XlsxError) -> Error {
    Error(format!("writing xlsx: {e}"))
}

fn range_a1(r: crate::model::Rect) -> String {
    format!("{}:{}", a1(r.top, r.left), a1(r.bottom, r.right))
}

fn width_of(s: &str) -> f64 {
    s.chars().count().min(60) as f64
}

/// Writes a header row + `nrows` rows as an Excel table named `name` (when there is data). Rows
/// come from `fill(row, buf)` one at a time, in order, so a low-memory worksheet streams them to
/// disk instead of holding the whole sheet.
fn write_grid(
    ws: &mut Worksheet,
    name: &str,
    headers: &[String],
    formats: &[Option<Format>],
    nrows: usize,
    mut fill: impl FnMut(usize, &mut Vec<Out>),
    add_table_time: &mut std::time::Duration,
) -> Result<(), Error> {
    // Column widths from the header and the first rows, set before any data is written.
    let sample: Vec<Vec<Out>> = (0..nrows.min(WIDTH_SAMPLE))
        .map(|r| {
            let mut row = Vec::with_capacity(headers.len());
            fill(r, &mut row);
            row
        })
        .collect();
    let mut widths: Vec<f64> = headers.iter().map(|h| width_of(h) + 2.0).collect();
    for row in &sample {
        for (c, v) in row.iter().enumerate().take(widths.len()) {
            let w = match v {
                Out::Text(s) => width_of(s),
                Out::Date(_) => 19.0,
                Out::Num(x) => fmt_num(*x).len() as f64,
                _ => 0.0,
            };
            if w + 2.0 > widths[c] {
                widths[c] = w + 2.0;
            }
        }
    }
    for (c, w) in widths.iter().enumerate() {
        ws.set_column_width(c as u16, w.clamp(8.0, 62.0)).map_err(xerr)?;
    }
    ws.set_freeze_panes(1, 0).map_err(xerr)?;
    if nrows == 0 || headers.is_empty() {
        let bold = Format::new().set_bold();
        for (c, h) in headers.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, h, &bold).map_err(xerr)?;
        }
        return Ok(());
    }
    // The table writes the header row; data rows follow in order.
    let cols: Vec<TableColumn> = headers.iter().map(|h| TableColumn::new().set_header(h)).collect();
    let table = XTable::new().set_name(name).set_columns(&cols);
    let table_start = std::time::Instant::now();
    let added = ws.add_table(0, 0, nrows as u32, (headers.len() - 1) as u16, &table).map_err(xerr);
    *add_table_time += table_start.elapsed();
    added?;
    let mut buf = Vec::with_capacity(headers.len());
    for r in 0..nrows {
        let row = match sample.get(r) {
            Some(row) => row,
            None => {
                buf.clear();
                fill(r, &mut buf);
                &buf
            }
        };
        let xr = (r + 1) as u32;
        for (c, v) in row.iter().enumerate() {
            let xc = c as u16;
            match (v, formats.get(c).and_then(|f| f.as_ref())) {
                (Out::Null, _) => {}
                (Out::Num(x) | Out::Date(x), Some(f)) if x.is_finite() => {
                    ws.write_number_with_format(xr, xc, *x, f).map_err(xerr)?;
                }
                (Out::Num(x) | Out::Date(x), _) if x.is_finite() => {
                    ws.write_number(xr, xc, *x).map_err(xerr)?;
                }
                (Out::Num(_) | Out::Date(_), _) => {}
                (Out::Bool(b), _) => {
                    ws.write_boolean(xr, xc, *b).map_err(xerr)?;
                }
                (Out::Text(s), _) => {
                    ws.write_string(xr, xc, s).map_err(xerr)?;
                }
            }
        }
    }
    Ok(())
}

fn table_sheet(wb: &mut Workbook, t: &Table, add_table_time: &mut std::time::Duration) -> Result<(), Error> {
    let ws = wb.add_worksheet_with_low_memory();
    ws.set_name(&t.sheet).map_err(xerr)?;
    let ncols = t.columns.len().min(MAX_COLS);
    let cols = &t.columns[..ncols];
    let headers: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
    let formats: Vec<Option<Format>> =
        cols.iter().map(|c| c.dtype.num_format().map(|f| Format::new().set_num_format(f))).collect();
    // Column-major storage, written row by row.
    write_grid(ws, &t.sheet, &headers, &formats, t.rows().min(MAX_ROWS), |i, row| {
        row.extend(cols.iter().map(|c| c.values[i].clone()))
    }, add_table_time)
}

fn text(s: impl Into<String>) -> Out {
    Out::Text(s.into())
}

fn opt_num(v: Option<f64>) -> Out {
    v.filter(|x| x.is_finite()).map_or(Out::Null, Out::Num)
}

fn opt_text(v: Option<String>) -> Out {
    v.filter(|s| !s.is_empty()).map_or(Out::Null, Out::Text)
}

/// rust_xlsxwriter (with `constant_memory`) opens a temp file for every worksheet and keeps it open
/// until the workbook is saved; past the process's open-file limit it panicked ("Too many open
/// files": 1,500 tables against Linux's usual soft limit of 1024, 250 against macOS's 256). The soft
/// limit is raised towards the hard limit when needed, and a workbook that still does not fit is a
/// clear error.
#[allow(clippy::unnecessary_cast)] // rlim_t is u64 on most, not all, unix targets
fn ensure_open_files(sheets: u64) -> Result<(), Error> {
    // Files the process may already hold (stdio, the source, libraries) plus the output.
    let needed = sheets + 64;
    #[cfg(unix)]
    {
        let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: getrlimit / setrlimit only read / write the struct passed.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
            return Ok(());
        }
        let cur = lim.rlim_cur as u64;
        if cur >= needed || lim.rlim_cur == libc::RLIM_INFINITY {
            return Ok(());
        }
        let max = if lim.rlim_max == libc::RLIM_INFINITY { u64::MAX } else { lim.rlim_max as u64 };
        let want = needed.saturating_mul(2).min(max);
        if want > cur {
            lim.rlim_cur = want as libc::rlim_t;
            // SAFETY: as above.
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) };
        }
        if want < needed {
            return Err(Error(format!(
                "cannot write {} sheets: the xlsx writer keeps a file open per sheet and this process may open only {max} files (raise `ulimit -n`)",
                sheets
            )));
        }
    }
    #[cfg(not(unix))]
    let _ = needed;
    Ok(())
}

fn table_id(n: Option<usize>) -> Out {
    n.map_or(Out::Null, |n| text(format!("T{n}")))
}

pub(super) fn xlsx(
    norm: &Normalized,
    src: &Source,
    xlsx_name: &str,
    manifest_name: &str,
    path: &Path,
) -> Result<(), Error> {
    // One sheet per table plus README, Totals and Notes, each holding a temp file open until saved.
    ensure_open_files(norm.tables.len() as u64 + 3)?;
    let mut wb = Workbook::new();
    // The same input gives the same bytes (B68): rust_xlsxwriter stamps the document's created /
    // modified properties with the current time unless one is set, so set a fixed one (the zip
    // entries' own timestamp, 1980-01-01).
    let epoch = ExcelDateTime::from_ymd(1980, 1, 1).map_err(xerr)?;
    wb.set_properties(&DocProperties::new().set_creation_datetime(&epoch));
    // Low-memory sheets stream rows to temp files: put them next to the output, not in a /tmp that
    // may live in RAM.
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        wb.set_tempdir(dir).map_err(xerr)?;
    }

    // README.
    {
        let ws = wb.add_worksheet();
        ws.set_name("README").map_err(xerr)?;
        let bold = Format::new().set_bold();
        let mut lines: Vec<(String, bool)> = vec![
            ("hedgehog normalized workbook".into(), true),
            (format!("Source: {} (sha256 {})", src.file_name, src.sha256), false),
            (format!("Generated by hedgehog {}. The source file is not modified.", env!("CARGO_PKG_VERSION")), false),
            (format!("Manifest (column types, original headers, locale, transformations): {manifest_name}"), false),
            (String::new(), false),
            ("Layout".into(), true),
            ("- One sheet per detected table, named T<n>_<slug>; the Excel table (ListObject) has the same name.".into(), false),
            ("- Row 1 is a single flat header; multi-row headers are joined with ' / '.".into(), false),
            ("- Values only: formulas are replaced by their cached values. Numbers, dates and booleans are typed.".into(), false),
            ("- _src_row = 1-based row number of the value in the source sheet (see 'Source range' below for the sheet).".into(), false),
            ("- Group-header rows ('Region: North') become a leading column. _row_type (with --keep-totals) = data|subtotal|total|average|of_which.".into(), false),
            ("- Totals: every total/subtotal with its recomputed value and ok/mismatch/unknown status.".into(), false),
            ("- Notes: titles, notes and every other source cell that is not table data, with its address.".into(), false),
            (String::new(), false),
            ("Tables".into(), true),
        ];
        let mut rows: Vec<Vec<String>> = Vec::new();
        rows.push(
            ["Sheet", "Source sheet", "Source range", "Rows", "Columns", "Title"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
        for t in &norm.tables {
            rows.push(vec![
                t.sheet.clone(),
                t.source_sheet.clone(),
                range_a1(t.rect),
                t.rows().to_string(),
                t.columns.len().to_string(),
                t.title.as_ref().map(|x| x.0.clone()).unwrap_or_default(),
            ]);
        }
        let mut r = 0u32;
        for (l, b) in lines.drain(..) {
            if b {
                ws.write_string_with_format(r, 0, &l, &bold).map_err(xerr)?;
            } else if !l.is_empty() {
                ws.write_string(r, 0, &l).map_err(xerr)?;
            }
            r += 1;
        }
        for (k, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                if k == 0 {
                    ws.write_string_with_format(r, c as u16, v, &bold).map_err(xerr)?;
                } else {
                    ws.write_string(r, c as u16, v).map_err(xerr)?;
                }
            }
            r += 1;
        }
        if norm.tables.is_empty() {
            ws.write_string(r, 0, "(no tables detected; see Notes)").map_err(xerr)?;
            r += 1;
        }
        r += 1;
        ws.write_string_with_format(r, 0, "How to load (Python)", &bold).map_err(xerr)?;
        r += 1;
        for l in
            load_hints(norm, xlsx_name, manifest_name).iter().flat_map(|(_, code)| code.lines().map(str::to_string))
        {
            ws.write_string(r, 0, &l).map_err(xerr)?;
            r += 1;
        }
        ws.set_column_width(0, 28).map_err(xerr)?;
        for c in 1..6u16 {
            ws.set_column_width(c, 16).map_err(xerr)?;
        }
    }

    let mut add_table_time = std::time::Duration::ZERO;
    for t in &norm.tables {
        table_sheet(&mut wb, t, &mut add_table_time)?;
    }

    // Totals.
    {
        let ws = wb.add_worksheet_with_low_memory();
        ws.set_name("Totals").map_err(xerr)?;
        let headers: Vec<String> = [
            "table",
            "sheet",
            "cell",
            "label",
            "label_cell",
            "level",
            "direction",
            "origin",
            "value",
            "recomputed",
            "status",
            "reason",
            "sources",
            "formula",
            "kept_in_data",
            "count",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let totals = &norm.totals;
        write_grid(ws, "Totals", &headers, &[], totals.len().min(MAX_ROWS), |i, row| {
            let x = &totals[i];
            row.extend([
                table_id(x.table),
                text(&x.sheet),
                text(x.cell_a1()),
                opt_text(x.label.clone()),
                x.label_cell.map_or(Out::Null, |(r, c)| text(a1(r, c))),
                text(x.level),
                text(x.direction),
                text(x.origin),
                opt_num(x.value),
                opt_num(x.recomputed),
                text(x.status),
                text(&x.reason),
                opt_text(Some(x.sources.join(", "))),
                opt_text(x.formula.clone()),
                Out::Bool(x.kept_in_data),
                Out::Num(x.count as f64),
            ])
        }, &mut add_table_time)?;
    }

    // Notes.
    {
        let ws = wb.add_worksheet_with_low_memory();
        ws.set_name("Notes").map_err(xerr)?;
        let headers: Vec<String> =
            ["sheet", "cell", "kind", "table", "text", "formula"].iter().map(|s| s.to_string()).collect();
        let notes = &norm.notes;
        write_grid(ws, "Notes", &headers, &[], notes.len().min(MAX_ROWS), |i, row| {
            let x = &notes[i];
            row.extend([
                text(&x.sheet),
                text(a1(x.row, x.col)),
                text(x.kind),
                table_id(x.table),
                opt_text(Some(super::typing::fit(x.text.clone(), &mut Default::default()))),
                opt_text(x.formula.clone()),
            ])
        }, &mut add_table_time)?;
    }

    super::trace_ms("prof9.xlsx.add_table", add_table_time);
    wb.save(path).map_err(xerr)
}

fn csv_field(s: &str, out: &mut String) {
    if s.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&s.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(s);
    }
}

/// One UTF-8 CSV (RFC 4180, `\n` line ends): numbers in shortest form (percent as fractions),
/// dates ISO, booleans `true`/`false`, missing values empty.
pub(super) fn csv(t: &Table, path: &Path) -> Result<(), Error> {
    let f = std::fs::File::create(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    let mut w = std::io::BufWriter::new(f);
    let mut line = String::new();
    let io = |e: std::io::Error| Error(format!("{}: {e}", path.display()));
    for (k, c) in t.columns.iter().enumerate() {
        if k > 0 {
            line.push(',');
        }
        csv_field(&c.name, &mut line);
    }
    line.push('\n');
    w.write_all(line.as_bytes()).map_err(io)?;
    for i in 0..t.rows() {
        line.clear();
        for (k, c) in t.columns.iter().enumerate() {
            if k > 0 {
                line.push(',');
            }
            match &c.values[i] {
                Out::Null => {}
                Out::Num(x) => line.push_str(&fmt_num(*x)),
                Out::Date(x) => line.push_str(&iso(*x)),
                Out::Bool(b) => line.push_str(if *b { "true" } else { "false" }),
                Out::Text(s) => csv_field(s, &mut line),
            }
        }
        line.push('\n');
        w.write_all(line.as_bytes()).map_err(io)?;
    }
    w.flush().map_err(io)
}
