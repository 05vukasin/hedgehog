//! Synthetic corpus loader (noise generator `tools/noise`, ground truth `hh-noise-gt/1`).
//!
//! Layout on disk: `<dir>/<profile>/<name>.xlsx` + `<name>.gt.json` (+ `index.json`). Each GT
//! file lists, per sheet, the table rectangles and every filled cell as
//! `[row, col, role, table, ds, src_row, src_col, orig, opset, true]` (see
//! `tools/noise/hh_noise/render.py`). Roles use our names directly (`title`, `header`, `data`,
//! `group_header`, `total`, `note`, `other`).
//!
//! Besides [`load`] (→ [`Corpus`], scored by [`super::run::evaluate`]) this module has
//! [`breakdown`]: a second pass that scores tables per profile, per table-boundary operator
//! (`stack_schema_change` variants a–e, `multi_table_stacked`, …) and per noise operator, and
//! optionally checks the value parsers ([`crate::values`]) against the true numbers, dates,
//! booleans and missing tokens behind text cells.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::gt::{Corpus, GtCell, GtFile, GtSheet, LabelCount};
use super::metrics::{iou, Confusion, Prf};
use super::{Analyzer, EvalError};
use crate::analysis::Role;
use crate::grid::Grid;
use crate::model::{Rect, Value};

// ---------------------------------------------------------------- minimal JSON

/// Parsed JSON value (core has no serde dependency).
#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn get(&self, k: &str) -> Option<&J> {
        match self {
            J::Obj(v) => v.iter().find(|(kk, _)| kk == k).map(|(_, x)| x),
            _ => None,
        }
    }
    pub fn arr(&self) -> &[J] {
        match self {
            J::Arr(v) => v,
            _ => &[],
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn num(&self) -> Option<f64> {
        match self {
            J::Num(x) => Some(*x),
            _ => None,
        }
    }
    pub fn u32(&self) -> Option<u32> {
        self.num().filter(|x| *x >= 0.0).map(|x| x as u32)
    }
    pub fn i64(&self) -> Option<i64> {
        self.num().map(|x| x as i64)
    }
}

pub fn parse_json(s: &str) -> Result<J, String> {
    let mut p = Parser { b: s.as_bytes(), i: 0 };
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(format!("trailing data at {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }
    fn err<T>(&self, m: &str) -> Result<T, String> {
        Err(format!("{m} at byte {}", self.i))
    }
    fn value(&mut self) -> Result<J, String> {
        self.ws();
        match self.b.get(self.i) {
            None => self.err("unexpected end"),
            Some(b'{') => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(J::Obj(v));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return self.err("expected ':'");
                    }
                    self.i += 1;
                    let x = self.value()?;
                    v.push((k, x));
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(J::Obj(v));
                        }
                        _ => return self.err("expected ',' or '}'"),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(J::Arr(v));
                }
                loop {
                    v.push(self.value()?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(J::Arr(v));
                        }
                        _ => return self.err("expected ',' or ']'"),
                    }
                }
            }
            Some(b'"') => Ok(J::Str(self.string()?)),
            Some(b't') if self.b[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(J::Bool(true))
            }
            Some(b'f') if self.b[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(J::Bool(false))
            }
            Some(b'n') if self.b[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(J::Null)
            }
            Some(b'N') if self.b[self.i..].starts_with(b"NaN") => {
                self.i += 3;
                Ok(J::Null)
            }
            Some(_) => {
                let st = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.b[st..self.i]).map_err(|e| e.to_string())?;
                t.parse::<f64>().map(J::Num).or_else(|_| self.err("bad number"))
            }
        }
    }
    fn string(&mut self) -> Result<String, String> {
        if self.b.get(self.i) != Some(&b'"') {
            return self.err("expected string");
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&c) = self.b.get(self.i) else { return self.err("unterminated string") };
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.b.get(self.i) else { return self.err("bad escape") };
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xD800..0xDC00).contains(&cp) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return self.err("bad escape"),
                    }
                }
                _ => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|e| e.to_string())
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.b.get(self.i..self.i + 4).ok_or("short \\u escape")?;
        self.i += 4;
        u32::from_str_radix(std::str::from_utf8(h).map_err(|e| e.to_string())?, 16).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------- loader

pub fn map_role(s: &str) -> Option<Role> {
    Some(match s {
        "title" => Role::Title,
        "header" => Role::Header,
        "data" => Role::Data,
        "group_header" => Role::GroupHeader,
        "total" => Role::Total,
        "note" => Role::Note,
        "other" => Role::Other,
        _ => return None,
    })
}

/// All `*.gt.json` under `dir` (the directory itself and one level of profile subfolders),
/// sorted by relative path.
pub fn gt_files(dir: &Path) -> Result<Vec<PathBuf>, EvalError> {
    let mut out = Vec::new();
    let rd = std::fs::read_dir(dir).map_err(|e| EvalError(format!("{}: {e}", dir.display())))?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&p) {
                out.extend(sub.flatten().map(|e| e.path()).filter(|p| is_gt(p)));
            }
        } else if is_gt(&p) {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

fn is_gt(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".gt.json"))
}

fn rect_of(j: &J) -> Option<Rect> {
    let a = j.arr();
    Some(Rect { top: a.first()?.u32()?, left: a.get(1)?.u32()?, bottom: a.get(2)?.u32()?, right: a.get(3)?.u32()? })
}

/// Reads one GT file: (file entry, parsed JSON).
pub fn read_gt(dir: &Path, gt_path: &Path) -> Result<(GtFile, J), EvalError> {
    let text = std::fs::read_to_string(gt_path).map_err(|e| EvalError(format!("{}: {e}", gt_path.display())))?;
    let j = parse_json(&text).map_err(|e| EvalError(format!("{}: {e}", gt_path.display())))?;
    let file = j.get("file").and_then(J::str).ok_or_else(|| EvalError(format!("{}: no `file`", gt_path.display())))?;
    let xlsx = gt_path.with_file_name(file);
    let id = xlsx.strip_prefix(dir).unwrap_or(&xlsx).to_string_lossy().replace('\\', "/");
    let mut sheets = Vec::new();
    for s in j.get("sheets").map(J::arr).unwrap_or(&[]) {
        let tables: Vec<Rect> = s.get("tables").map(J::arr).unwrap_or(&[]).iter().filter_map(|t| t.get("rect").and_then(rect_of)).collect();
        if tables.is_empty() && s.get("kind").and_then(J::str) == Some("empty") {
            continue;
        }
        let mut cells = Vec::new();
        for c in s.get("cells").map(J::arr).unwrap_or(&[]) {
            let a = c.arr();
            let (Some(row), Some(col), Some(role)) =
                (a.first().and_then(J::u32), a.get(1).and_then(J::u32), a.get(2).and_then(J::str).and_then(map_role))
            else {
                continue;
            };
            cells.push(GtCell { row, col, role });
        }
        cells.sort_by_key(|c| (c.row, c.col));
        sheets.push(GtSheet {
            name: s.get("name").and_then(J::str).unwrap_or_default().to_string(),
            index: s.get("index").and_then(J::u32).map(|x| x as usize),
            tables,
            cells,
        });
    }
    Ok((GtFile { id, path: xlsx, fallback_paths: Vec::new(), sheets }, j))
}

/// Loads a generated corpus into the common ground-truth schema.
pub fn load(dir: &Path) -> Result<Corpus, EvalError> {
    let mut corpus = Corpus { name: "synthetic".into(), has_tables: true, has_roles: true, ..Default::default() };
    for p in gt_files(dir)? {
        let (f, _) = read_gt(dir, &p)?;
        for s in &f.sheets {
            for c in &s.cells {
                corpus.labels.entry(c.role.as_str().to_string()).or_insert(LabelCount { role: Some(c.role), count: 0 }).count += 1;
            }
        }
        if !f.path.exists() {
            corpus.unmatched.push(super::gt::Unmatched { id: f.id.clone(), reason: "xlsx missing".into() });
            continue;
        }
        corpus.files.push(f);
    }
    corpus.files.sort_by(|a, b| a.id.cmp(&b.id));
    if corpus.files.is_empty() {
        return Err(EvalError(format!("no *.gt.json with an xlsx under {}", dir.display())));
    }
    Ok(corpus)
}

// ---------------------------------------------------------------- breakdown + value check

#[derive(Debug, Clone, Default)]
pub struct Bucket {
    pub tables: Prf,
    /// GT tables whose best predicted rectangle has IoU ≥ 0.5 / is exact.
    pub gt_tables: u64,
    pub hit_iou50: u64,
    pub hit_exact: u64,
    pub confusion: Confusion,
    pub sheets: u64,
}

impl Bucket {
    fn recall50(&self) -> f64 {
        self.hit_iou50 as f64 / self.gt_tables.max(1) as f64
    }
    fn recall_exact(&self) -> f64 {
        self.hit_exact as f64 / self.gt_tables.max(1) as f64
    }
}

#[derive(Debug, Clone, Default)]
pub struct ValueCount {
    pub total: u64,
    pub ok: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Breakdown {
    pub by_profile: BTreeMap<String, Bucket>,
    /// Key: `op` or `op:variant` of the table's `boundary`.
    pub by_boundary: BTreeMap<String, Bucket>,
    /// Key: table-level noise operator (tables carrying it).
    pub by_table_op: BTreeMap<String, Bucket>,
    pub values_checked: bool,
    /// Key: `kind` (number/date/missing/bool) and `kind/op`.
    pub values: BTreeMap<String, ValueCount>,
    pub value_failures: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BreakdownOptions {
    pub limit: Option<usize>,
    pub values: bool,
}

/// Second pass over the corpus: per-profile / per-boundary / per-operator table scores and,
/// with `opts.values`, the value-parser check.
pub fn breakdown(dir: &Path, analyzer: &dyn Analyzer, opts: &BreakdownOptions) -> Result<Breakdown, EvalError> {
    let mut out = Breakdown { values_checked: opts.values, ..Default::default() };
    for p in gt_files(dir)?.into_iter().take(opts.limit.unwrap_or(usize::MAX)) {
        let (f, j) = read_gt(dir, &p)?;
        let Ok(wb) = crate::reader::open(&f.path) else { continue };
        let profile = j.get("profile").and_then(J::str).unwrap_or("?").to_string();
        let op_sets: Vec<Vec<String>> = j
            .get("op_sets")
            .map(J::arr)
            .unwrap_or(&[])
            .iter()
            .map(|a| a.arr().iter().filter_map(|x| x.str().map(str::to_string)).collect())
            .collect();
        let jsheets: Vec<&J> = j.get("sheets").map(J::arr).unwrap_or(&[]).iter().collect();
        for gs in &f.sheets {
            let Some(sheet) = wb.sheets.iter().find(|s| s.name == gs.name) else { continue };
            let Some(js) = jsheets.iter().find(|s| s.get("name").and_then(J::str) == Some(&gs.name)) else { continue };
            let grid = Grid::new(sheet);
            let layout = analyzer.analyze(&grid);
            let pred: Vec<Rect> = layout.tables.iter().map(|t| t.rect).collect();
            let tp = super::metrics::table_matches(&gs.tables, &pred, super::metrics::TableCriterion::Iou(0.5));
            let mut conf = Confusion::default();
            if layout.roles.len() == sheet.cells.len() {
                for gc in &gs.cells {
                    if let Some(i) = grid.cell_index(gc.row, gc.col) {
                        conf.add(gc.role, layout.roles[i]);
                    }
                }
            }
            let pb = out.by_profile.entry(profile.clone()).or_default();
            pb.tables.add(Prf::from_matches(tp, gs.tables.len(), pred.len()));
            pb.confusion.merge(&conf);
            pb.sheets += 1;
            for t in js.get("tables").map(J::arr).unwrap_or(&[]) {
                let Some(r) = t.get("rect").and_then(rect_of) else { continue };
                let best = pred.iter().map(|p| iou(r, *p)).fold(0.0, f64::max);
                let hit50 = best + 1e-9 >= 0.5;
                let exact = pred.contains(&r);
                let mut keys = vec![format!("profile:{profile}")];
                if let Some(b) = t.get("boundary") {
                    let op = b.get("op").and_then(J::str).unwrap_or("?");
                    keys.clear();
                    keys.push(op.to_string());
                    if let Some(v) = b.get("variant").and_then(J::str) {
                        keys.push(format!("{op}:{v}"));
                    }
                    if let Some(g) = b.get("gap_rows").and_then(J::u32) {
                        keys.push(format!("{op}:gap{}", g.min(3)));
                    }
                }
                for k in keys {
                    let e = out.by_boundary.entry(k).or_default();
                    e.gt_tables += 1;
                    e.hit_iou50 += hit50 as u64;
                    e.hit_exact += exact as u64;
                }
                let mut ops: Vec<&str> = t.get("ops").map(J::arr).unwrap_or(&[]).iter().filter_map(|o| o.get("op").and_then(J::str)).collect();
                ops.sort_unstable();
                ops.dedup();
                for op in ops {
                    let e = out.by_table_op.entry(op.to_string()).or_default();
                    e.gt_tables += 1;
                    e.hit_iou50 += hit50 as u64;
                    e.hit_exact += exact as u64;
                }
            }
            if opts.values {
                check_values(&grid, js, &op_sets, &mut out, &f.id);
            }
        }
    }
    Ok(out)
}

fn text_at<'a>(grid: &Grid<'a>, r: u32, c: u32) -> Option<&'a str> {
    match grid.cell(r, c)?.value.as_ref()? {
        Value::Text(s) => Some(s.as_str()),
        _ => None,
    }
}

fn ymd_of_iso(s: &str) -> Option<(i32, u8, u8)> {
    let mut it = s.split('-');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.get(..2)?.parse().ok()?))
}

fn check_values(grid: &Grid, js: &J, op_sets: &[Vec<String>], out: &mut Breakdown, file: &str) {
    use crate::values::{infer_date_order, infer_decimal, missing_token, parse_bool, parse_date, parse_number, NumberHint};
    let cells = js.get("cells").map(J::arr).unwrap_or(&[]);
    // per-column text context (the parsers infer conventions per column)
    let mut col_texts: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
    for c in cells {
        let a = c.arr();
        let (Some(r), Some(col)) = (a.first().and_then(J::u32), a.get(1).and_then(J::u32)) else { continue };
        if a.get(2).and_then(J::str) == Some("data")
            && let Some(t) = text_at(grid, r, col)
        {
            col_texts.entry(col).or_default().push(t);
        }
    }
    let dec: BTreeMap<u32, _> = col_texts.iter().map(|(c, v)| (*c, infer_decimal(v.iter().copied()))).collect();
    let ord: BTreeMap<u32, _> = col_texts.iter().map(|(c, v)| (*c, infer_date_order(v.iter().copied()))).collect();
    for c in cells {
        let a = c.arr();
        let (Some(r), Some(col)) = (a.first().and_then(J::u32), a.get(1).and_then(J::u32)) else { continue };
        let Some(t) = a.get(9).filter(|t| matches!(t, J::Obj(_))) else { continue };
        if a.get(2).and_then(J::str) != Some("data") {
            continue;
        }
        let ops: &[String] = a.get(8).and_then(J::i64).filter(|i| *i >= 0).and_then(|i| op_sets.get(i as usize)).map(|v| v.as_slice()).unwrap_or(&[]);
        let cell = grid.cell(r, col);
        let (kind, ok, shown) = if let Some(m) = t.get("missing").and_then(J::str) {
            let ok = match cell.and_then(|c| c.value.as_ref()) {
                Some(Value::Error(_)) => true,
                Some(Value::Text(s)) => missing_token(s).is_some(),
                Some(Value::Number(x)) => *x == 0.0 && m == "0",
                None => true,
                _ => false,
            };
            ("missing", ok, m.to_string())
        } else if let Some(n) = t.get("num").and_then(J::num) {
            let Some(s) = text_at(grid, r, col) else { continue };
            let hint = NumberHint::with_decimal(dec.get(&col).copied().flatten());
            let got = parse_number(s, hint).map(|p| p.normalized());
            let ok = got.is_some_and(|g| (g - n).abs() <= 1e-6 * n.abs().max(1.0) + 1e-9);
            ("number", ok, s.to_string())
        } else if let Some(iso) = t.get("date").and_then(J::str) {
            let Some(s) = text_at(grid, r, col) else { continue };
            let got = parse_date(s, ord.get(&col).copied().flatten()).map(|d| (d.year, d.month, d.day));
            let kind = if matches!(t.get("ambiguous"), Some(J::Bool(true))) { "date_ambiguous" } else { "date" };
            (kind, got.is_some() && got == ymd_of_iso(iso), s.to_string())
        } else if let Some(J::Bool(bv)) = t.get("bool") {
            let Some(s) = text_at(grid, r, col) else { continue };
            ("bool", parse_bool(s) == Some(*bv), s.to_string())
        } else {
            continue;
        };
        let mut keys = vec![kind.to_string()];
        keys.extend(ops.iter().map(|o| format!("{kind}/{o}")));
        for k in keys {
            let e = out.values.entry(k).or_default();
            e.total += 1;
            e.ok += ok as u64;
        }
        if !ok && out.value_failures.len() < 40 {
            out.value_failures.push(format!("{file} {}: {kind} {shown:?} ops={ops:?}", crate::model::a1(r, col)));
        }
    }
}

fn pct(x: f64) -> String {
    format!("{:5.1}", x * 100.0)
}

impl Breakdown {
    pub fn text(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "\nby profile            sheets  tables P     R    F1@0.5  roles macro-F1");
        for (k, b) in &self.by_profile {
            let _ = writeln!(
                o,
                "  {:<20} {:>6}  {} {} {}   {}",
                k,
                b.sheets,
                pct(b.tables.precision()),
                pct(b.tables.recall()),
                b.tables.f1().map(pct).unwrap_or_else(|| "    -".into()),
                b.confusion.macro_f1().map(pct).unwrap_or_else(|| "    -".into())
            );
        }
        let _ = writeln!(o, "\nby table boundary (how the table is separated)   gt   recall@IoU0.5  exact");
        for (k, b) in &self.by_boundary {
            let _ = writeln!(o, "  {:<44} {:>6}   {}        {}", k, b.gt_tables, pct(b.recall50()), pct(b.recall_exact()));
        }
        let _ = writeln!(o, "\nby table operator                                gt   recall@IoU0.5  exact");
        for (k, b) in &self.by_table_op {
            let _ = writeln!(o, "  {:<44} {:>6}   {}        {}", k, b.gt_tables, pct(b.recall50()), pct(b.recall_exact()));
        }
        if self.values_checked {
            let _ = writeln!(o, "\nvalue parsers (text cells with a known true value)   n   accuracy");
            for (k, v) in &self.values {
                let ind = if k.contains('/') { "    " } else { "  " };
                let _ = writeln!(o, "{ind}{:<46} {:>7}   {}", k, v.total, pct(v.ok as f64 / v.total.max(1) as f64));
            }
            if !self.value_failures.is_empty() {
                let _ = writeln!(o, "\nfirst value failures:");
                for f in self.value_failures.iter().take(15) {
                    let _ = writeln!(o, "  {f}");
                }
            }
        }
        o
    }

    pub fn to_json(&self) -> String {
        use super::json;
        let mut o = String::from("{");
        let buckets = |o: &mut String, name: &str, m: &BTreeMap<String, Bucket>, first: bool| {
            json::key(o, name, first);
            o.push('{');
            for (i, (k, b)) in m.iter().enumerate() {
                json::key(o, k, i == 0);
                let _ = write!(o, "{{\"gt_tables\":{},\"recall_iou50\":", b.gt_tables);
                json::num(o, b.recall50());
                o.push_str(",\"recall_exact\":");
                json::num(o, b.recall_exact());
                o.push_str(",\"table_f1_iou50\":");
                json::opt_num(o, b.tables.f1());
                o.push_str(",\"role_macro_f1\":");
                json::opt_num(o, b.confusion.macro_f1());
                let _ = write!(o, ",\"sheets\":{}}}", b.sheets);
            }
            o.push('}');
        };
        buckets(&mut o, "by_profile", &self.by_profile, true);
        buckets(&mut o, "by_boundary", &self.by_boundary, false);
        buckets(&mut o, "by_table_op", &self.by_table_op, false);
        json::key(&mut o, "values", false);
        o.push('{');
        for (i, (k, v)) in self.values.iter().enumerate() {
            json::key(&mut o, k, i == 0);
            let _ = write!(o, "{{\"n\":{},\"ok\":{}}}", v.total, v.ok);
        }
        o.push_str("}}");
        o
    }
}
