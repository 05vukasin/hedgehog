//! The JSON manifest: source fingerprint, one entry per table (columns with original headers,
//! dtypes, locale, stats), transformation counts, totals, notes summary and loading snippets.
//! Keys are written in a fixed order and nothing time-dependent is included, so the same input
//! always gives the same bytes. All row numbers are 1-based, columns are letters, like Excel.

use std::collections::{BTreeMap, HashSet};

use super::extract::{ROW_TYPE, SRC_ROW};
use super::json::Json;
use super::typing::{fmt_num, iso};
use super::{ColRole, Column, Dtype, Normalized, Options, Out, T, Table, TotalItem, coverage};
use crate::model::{Rect, a1};
use crate::values::{DateOrder, DecimalSep};

/// Bumped on any incompatible change of the manifest layout.
pub const SCHEMA_VERSION: u32 = 2;

pub(super) struct Source {
    pub path: String,
    pub file_name: String,
    pub sha256: String,
    pub size: usize,
}

fn range_a1(r: Rect) -> String {
    format!("{}:{}", a1(r.top, r.left), a1(r.bottom, r.right))
}

fn letter(col: u32) -> String {
    a1(0, col).trim_end_matches('1').to_string()
}

fn py_str(s: &str) -> String {
    format!("{:?}", s)
}

/// (name, python code) pairs, also printed on the README sheet.
pub(super) fn load_hints(norm: &Normalized, xlsx_name: &str, manifest_name: &str) -> Vec<(&'static str, String)> {
    let example = norm.tables.first().map_or("T1_table".to_string(), |t| t.sheet.clone());
    let x = py_str(xlsx_name);
    let e = py_str(&example);
    vec![
        (
            "pandas",
            format!(
                "import pandas as pd\ndf = pd.read_excel({x}, sheet_name={e})  # one sheet per table, header in row 1\ntables = pd.read_excel({x}, sheet_name=None)  # dict of all sheets"
            ),
        ),
        (
            "openpyxl",
            format!(
                "import openpyxl\nwb = openpyxl.load_workbook({x})\nws = wb[{e}]\ntbl = ws.tables[{e}]  # Excel table, same name as the sheet\nrows = list(ws.iter_rows(min_row=2, values_only=True))"
            ),
        ),
        (
            "manifest",
            format!(
                "import json\nm = json.load(open({}, encoding=\"utf-8\"))\ncols = {{c[\"name\"]: c for c in m[\"tables\"][0][\"columns\"]}}  # dtype, original_header, source_column",
                py_str(manifest_name)
            ),
        ),
        (
            "trace",
            "# value at df.loc[i, col] came from source cell <source_column><_src_row> on the table's source sheet"
                .to_string(),
        ),
    ]
}

fn out_json(v: &Out) -> Json {
    match v {
        Out::Null => Json::Null,
        Out::Num(x) => Json::Num(*x),
        Out::Date(x) => Json::Str(iso(*x)),
        Out::Bool(b) => Json::Bool(*b),
        Out::Text(s) => {
            let s: String =
                if s.chars().count() > 80 { s.chars().take(80).collect::<String>() + "…" } else { s.clone() };
            Json::Str(s)
        }
    }
}

/// Cells listed per footnote mark in the manifest (the count is always complete).
const MAX_MARK_CELLS: usize = 100;

/// `null`, or one entry per footnote mark stripped from the column's numbers:
/// `{"mark": "¹", "count": 3, "cells": ["B5", "B9", "B12"]}`.
fn footnote_marks_json(c: &Column) -> Json {
    if c.footnote_marks.is_empty() {
        return Json::Null;
    }
    Json::Arr(
        c.footnote_marks
            .iter()
            .map(|(m, cells)| {
                let list: Vec<Json> = cells.iter().take(MAX_MARK_CELLS).map(|(r, col)| Json::Str(a1(*r, *col))).collect();
                Json::obj().with("mark", m.as_str()).with("count", cells.len()).with("cells", Json::Arr(list))
            })
            .collect(),
    )
}

fn column_json(c: &Column, index: usize, left: Option<&Column>) -> Json {
    let mut nulls = 0usize;
    let mut distinct: HashSet<String> = HashSet::new();
    let (mut min, mut max): (Option<f64>, Option<f64>) = (None, None);
    let mut example: Option<&Out> = None;
    for v in &c.values {
        let key = match v {
            Out::Null => {
                nulls += 1;
                continue;
            }
            Out::Num(x) | Out::Date(x) => {
                min = Some(min.map_or(*x, |m| m.min(*x)));
                max = Some(max.map_or(*x, |m| m.max(*x)));
                format!("n{}", fmt_num(*x))
            }
            Out::Bool(b) => format!("b{b}"),
            Out::Text(s) => format!("s{s}"),
        };
        example.get_or_insert(v);
        distinct.insert(key);
    }
    let numeric = matches!(c.dtype, Dtype::Integer | Dtype::Number | Dtype::Percent | Dtype::Currency);
    let dated = matches!(c.dtype, Dtype::Date | Dtype::Datetime);
    let bound = |v: Option<f64>| -> Json {
        match v {
            Some(x) if dated => Json::Str(iso(x)),
            Some(x) if numeric => Json::Num(x),
            _ => Json::Null,
        }
    };
    let header: Vec<Json> = c
        .original_header
        .iter()
        .map(|(t, (r, col))| Json::obj().with("text", t.as_str()).with("cell", a1(*r, *col)))
        .collect();
    let locale = Json::obj()
        .with(
            "decimal",
            c.decimal.map(|d| match d {
                DecimalSep::Dot => ".",
                DecimalSep::Comma => ",",
            }),
        )
        .with(
            "date_order",
            c.date_order.map(|d| match d {
                DateOrder::DayMonth => "dmy",
                DateOrder::MonthDay => "mdy",
            }),
        )
        .with("ambiguous", c.ambiguous);
    // A column computed from the one on its left (`<name>_period`, `<name>_key`).
    let derived = match (c.derived_kind(), left) {
        (Some(kind), Some(base)) => Json::obj().with("from", base.name.as_str()).with("kind", kind),
        _ => Json::Null,
    };
    Json::obj()
        .with("name", c.name.as_str())
        .with("index", index)
        .with("letter", letter(index as u32))
        .with("role", c.role.as_str())
        .with("dtype", c.dtype.as_str())
        .with("number_format", c.dtype.num_format())
        .with("source_column", c.source_col.map(letter))
        .with("original_header", Json::Arr(header))
        .with("derived", derived)
        .with("unit", c.unit.clone())
        .with("currency", c.currency.clone())
        .with("locale", locale)
        .with("footnote_marks", footnote_marks_json(c))
        .with("nulls", nulls)
        .with("distinct", distinct.len())
        .with("min", bound(min))
        .with("max", bound(max))
        .with("example", example.map_or(Json::Null, out_json))
}

/// Per-table totals: counts by status and one entry per total row (vertical totals), so an agent
/// can tell which rows are not in the table's data.
fn table_totals(t: &Table, totals: &[TotalItem]) -> Json {
    let tt = super::table_totals(t, totals);
    let rows: Vec<Json> = tt
        .rows
        .into_iter()
        .map(|r| {
            Json::obj().with("row", r.row).with("label", r.label).with("level", r.level).with("kept_in_data", r.kept_in_data).with("period", r.period).with("sources_outside", r.sources_outside)
        })
        .collect();
    Json::obj()
        .with("count", tt.count)
        .with("ok", tt.ok)
        .with("mismatch", tt.mismatch)
        .with("unknown", tt.unknown)
        .with("rows", Json::Arr(rows))
}

fn table_json(t: &Table, totals: &[TotalItem], cov: Option<&coverage::Coverage>) -> Json {
    let body_top = t.header_top + t.header_rows;
    let header_range = (t.header_rows > 0)
        .then(|| range_a1(Rect { top: t.header_top, left: t.rect.left, bottom: body_top - 1, right: t.rect.right }));
    let data_range = (body_top <= t.rect.bottom)
        .then(|| range_a1(Rect { top: body_top, left: t.rect.left, bottom: t.rect.bottom, right: t.rect.right }));
    let notes: Vec<Json> =
        t.notes.iter().map(|(s, (r, c))| Json::obj().with("text", s.as_str()).with("cell", a1(*r, *c))).collect();
    let source = Json::obj()
        .with("sheet", t.source_sheet.as_str())
        .with("range", range_a1(t.rect))
        .with("header_top", t.header_top + 1)
        .with("header_rows", t.header_rows)
        .with("header_range", header_range)
        .with("data_range", data_range)
        .with("title", t.title.as_ref().map(|x| x.0.clone()))
        .with("title_cell", t.title.as_ref().map(|x| a1(x.1.0, x.1.1)))
        .with("notes", Json::Arr(notes));
    let mut tr = Json::obj();
    for k in T::ALL {
        tr.push(k.as_str(), t.transforms.get(k));
    }
    let group = t.columns.iter().find(|c| c.role == ColRole::Group).map(|c| c.name.clone());
    let row_type = t.columns.iter().any(|c| c.role == ColRole::RowType).then_some(ROW_TYPE);
    Json::obj()
        .with("id", t.id())
        .with("sheet", t.sheet.as_str())
        .with("excel_table", t.sheet.as_str())
        .with("source", source)
        .with("rows", t.rows())
        .with("confidence", t.confidence)
        .with("time_coverage", cov.map_or(Json::Null, |c| c.json(false)))
        .with("columns", Json::Arr(t.columns.iter().enumerate().map(|(i, c)| column_json(c, i, i.checked_sub(1).map(|j| &t.columns[j]))).collect()))
        .with("group_column", group)
        .with("source_row_column", SRC_ROW)
        .with("row_type_column", row_type)
        .with("totals", table_totals(t, totals))
        .with("transformations", tr)
        .with("warnings", t.warnings.clone())
}

pub(super) fn build(
    norm: &Normalized,
    src: &Source,
    opts: &Options,
    xlsx_name: &str,
    manifest_name: &str,
    csv_names: &[String],
) -> Json {
    let sheets: Vec<Json> = norm
        .sheets
        .iter()
        .map(|s| {
            Json::obj()
                .with("name", s.name.as_str())
                .with("used_range", s.used_range.map(range_a1))
                .with("cells", s.cells)
                .with("formulas", s.formulas)
                .with("merges", s.merges)
                .with("tables", s.tables.iter().map(|n| format!("T{n}")).collect::<Vec<_>>())
        })
        .collect();
    let mut out_sheets = vec!["README".to_string()];
    out_sheets.extend(norm.tables.iter().map(|t| t.sheet.clone()));
    out_sheets.push("Totals".into());
    out_sheets.push("Notes".into());

    let (mut ok, mut bad, mut unknown) = (0usize, 0usize, 0usize);
    let items: Vec<Json> = norm
        .totals
        .iter()
        .map(|x| {
            match x.status {
                "ok" => ok += x.count,
                "mismatch" => bad += x.count,
                _ => unknown += x.count,
            }
            Json::obj()
                .with("table", x.table.map(|n| format!("T{n}")))
                .with("sheet", x.sheet.as_str())
                .with("cell", x.cell_a1())
                .with("row", x.row + 1)
                .with("count", x.count)
                .with("label", x.label.clone())
                .with("level", x.level)
                .with("direction", x.direction)
                .with("origin", x.origin)
                .with("value", x.value)
                .with("recomputed", x.recomputed)
                .with("status", x.status)
                .with("reason", x.reason.as_str())
                .with("sources", x.sources.clone())
                .with("kept_in_data", x.kept_in_data)
        })
        .collect();
    let totals = Json::obj()
        .with("sheet", "Totals")
        .with("count", ok + bad + unknown)
        .with("ok", ok)
        .with("mismatch", bad)
        .with("unknown", unknown)
        .with("items", Json::Arr(items));

    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for n in &norm.notes {
        *by_kind.entry(n.kind).or_default() += 1;
    }
    let mut kinds = Json::obj();
    for (k, v) in &by_kind {
        kinds.push(k, *v);
    }
    let unassigned = norm.notes.iter().filter(|n| n.table.is_none()).count();
    let notes = Json::obj().with("sheet", "Notes").with("count", norm.notes.len()).with("by_kind", kinds);
    let cells = Json::obj()
        .with("total", norm.cells.total)
        .with("in_tables", norm.cells.in_tables)
        .with("in_totals", norm.cells.in_totals)
        .with("in_notes", norm.cells.in_notes);
    let mut hints = Json::obj();
    for (k, v) in load_hints(norm, xlsx_name, manifest_name) {
        hints.push(k, v);
    }

    let covs: Vec<Option<coverage::Coverage>> = norm.tables.iter().map(coverage::table).collect();
    let with_cov: Vec<(String, coverage::Coverage)> =
        norm.tables.iter().zip(&covs).filter_map(|(t, c)| c.clone().map(|c| (t.id(), c))).collect();
    let tables: Vec<Json> =
        norm.tables.iter().zip(&covs).map(|(t, c)| table_json(t, &norm.totals, c.as_ref())).collect();

    Json::obj()
        .with("schema_version", SCHEMA_VERSION)
        .with("generator", Json::obj().with("name", "hedgehog").with("version", env!("CARGO_PKG_VERSION")))
        .with(
            "source",
            Json::obj()
                .with("path", src.path.as_str())
                .with("file_name", src.file_name.as_str())
                .with("sha256", src.sha256.as_str())
                .with("size_bytes", src.size)
                .with("sheets", Json::Arr(sheets)),
        )
        .with(
            "output",
            Json::obj()
                .with("xlsx", xlsx_name)
                .with("manifest", manifest_name)
                .with("csv", csv_names.to_vec())
                .with("sheets", out_sheets),
        )
        .with(
            "options",
            Json::obj()
                .with("keep_totals", opts.keep_totals)
                .with("csv", opts.csv)
                .with("locale", opts.locale.map(|l| l.as_str()))
                .with("cell_roles", if opts.rules_only || crate::learned::embedded().is_none() { "rules" } else { "model" }),
        )
        .with("time_coverage", coverage::workbook(&with_cov).map_or(Json::Null, |c| c.json(true)))
        .with("tables", Json::Arr(tables))
        .with("totals", totals)
        .with("notes", notes)
        .with("cells", cells)
        .with("unassigned_cells", unassigned)
        .with("warnings", norm.warnings.clone())
        .with("load_hints", hints)
}
