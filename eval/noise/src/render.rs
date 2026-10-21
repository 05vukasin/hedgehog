//! A table `Plan` (clean content + how to lay it out) rendered onto a `Canvas` (cells, merges) with
//! the truth of every written cell: what a data value must become, which labels must stay with it,
//! which cells are totals, notes or separators. Canvas → xlsx via rust_xlsxwriter.

use std::collections::BTreeMap;

use crate::base::{fmt_plain, round, BaseTable, Kind, V};
use crate::rng::Rng;

#[derive(Clone, Debug)]
pub enum W {
    Num(f64),
    Text(String),
    Date(f64),
    /// Formula (without `=`) and its cached value.
    Formula(String, f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    Num(f64),
    Text(String),
    Date(f64),
    Null,
    /// Free text in a numeric column: kept as this text, or null when it is a missing word.
    TextOrNull(String),
}

#[derive(Clone, Debug)]
pub struct DataT {
    pub r: u32,
    pub c: u32,
    pub table: usize,
    pub exp: Expect,
    /// The value may also come out multiplied by this (a scale line such as "in 1000 EUR").
    pub scale: Option<f64>,
    /// Row key labels that must stay attached to the value (row texts, group column, table title).
    pub labels: Vec<String>,
    /// Header path parts the value's column name must carry.
    pub header: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TotKind {
    Sum,
    Average,
    OfWhich,
}

#[derive(Clone, Debug)]
pub struct TotT {
    pub r: u32,
    pub c: u32,
    pub table: usize,
    pub kind: TotKind,
}

#[derive(Clone, Debug)]
pub struct NonData {
    pub r: u32,
    pub c: u32,
    pub what: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct TableT {
    /// The table may come out as several output tables (blocks that repeat their labels).
    pub may_split: bool,
    /// Tables with the same group may be merged into one output table.
    pub merge_group: Option<usize>,
    /// Scale factor and the words that state it.
    pub scale: Option<(f64, Vec<String>)>,
}

#[derive(Default)]
pub struct Canvas {
    pub cells: BTreeMap<(u32, u32), W>,
    pub merges: Vec<(u32, u32, u32, u32)>,
    pub data: Vec<DataT>,
    pub totals: Vec<TotT>,
    pub nondata: Vec<NonData>,
    pub tables: Vec<TableT>,
    /// What the generator did, one phrase per step (for the report and reproduction).
    pub desc: Vec<String>,
}

impl Canvas {
    pub fn put(&mut self, r: u32, c: u32, w: W) {
        self.cells.insert((r, c), w);
    }
    pub fn text(&mut self, r: u32, c: u32, s: &str, what: Option<&'static str>) {
        if s.is_empty() {
            return;
        }
        self.put(r, c, W::Text(s.to_string()));
        if let Some(w) = what {
            self.nondata.push(NonData { r, c, what: w });
        }
    }
    pub fn bottom(&self) -> u32 {
        self.cells.keys().map(|k| k.0).max().unwrap_or(0)
    }
    pub fn right(&self) -> u32 {
        self.cells.keys().map(|k| k.1).max().unwrap_or(0)
    }
    pub fn new_table(&mut self, t: TableT) -> usize {
        self.tables.push(t);
        self.tables.len() - 1
    }

    pub fn write_xlsx(&self, path: &std::path::Path) -> Result<(), String> {
        use rust_xlsxwriter::{Format, Formula, Workbook};
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        ws.set_name("Data").map_err(|e| e.to_string())?;
        let date = Format::new().set_num_format("yyyy-mm-dd");
        let plain = Format::new();
        let mut merged_tl: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
        for &(r1, c1, r2, c2) in &self.merges {
            if (r1, c1) != (r2, c2) {
                merged_tl.insert((r1, c1), (r2, c2));
            }
        }
        let inside_merge = |r: u32, c: u32| {
            self.merges.iter().any(|&(r1, c1, r2, c2)| r >= r1 && r <= r2 && c >= c1 && c <= c2 && (r, c) != (r1, c1))
        };
        for (&(r, c), w) in &self.cells {
            if inside_merge(r, c) {
                continue;
            }
            let cc = c as u16;
            if let Some(&(r2, c2)) = merged_tl.get(&(r, c)) {
                let s = match w {
                    W::Text(s) => s.clone(),
                    W::Num(x) | W::Formula(_, x) => fmt_plain(*x),
                    W::Date(d) => fmt_plain(*d),
                };
                ws.merge_range(r, cc, r2, c2 as u16, &s, &plain).map_err(|e| e.to_string())?;
                continue;
            }
            match w {
                W::Num(x) => ws.write_number(r, cc, *x).map(|_| ()),
                W::Text(s) => ws.write_string(r, cc, s).map(|_| ()),
                W::Date(d) => ws.write_number_with_format(r, cc, *d, &date).map(|_| ()),
                W::Formula(f, v) => {
                    ws.write_formula(r, cc, Formula::new(format!("={f}")).set_result(fmt_plain(*v))).map(|_| ())
                }
            }
            .map_err(|e| e.to_string())?;
        }
        // merges whose top-left holds no value (blank merged header areas)
        for &(r1, c1, r2, c2) in &self.merges {
            if (r1, c1) != (r2, c2) && !self.cells.contains_key(&(r1, c1)) {
                ws.merge_range(r1, c1 as u16, r2, c2 as u16, "", &plain).map_err(|e| e.to_string())?;
            }
        }
        wb.save(path).map_err(|e| e.to_string())
    }

    /// Text view of the canvas (for `--dump`).
    pub fn show(&self) -> String {
        let mut out = String::new();
        let (b, rt) = (self.bottom(), self.right());
        for r in 0..=b {
            let mut line = format!("{:>4} ", r + 1);
            for c in 0..=rt.min(25) {
                let s = match self.cells.get(&(r, c)) {
                    None => String::new(),
                    Some(W::Text(s)) => s.clone(),
                    Some(W::Num(x)) => fmt_plain(*x),
                    Some(W::Date(d)) => format!("d{}", fmt_plain(*d)),
                    Some(W::Formula(f, _)) => format!("={f}"),
                };
                let s: String = s.chars().take(14).collect();
                line.push_str(&format!("|{s:<14}"));
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Number and date text in several conventions

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Loc {
    /// 1,234.56
    Us,
    /// 1.234,56
    Eu,
    /// 1 234,56 (no-break space)
    Fr,
    /// 1'234.56
    Ch,
    /// 1234,56
    EuPlain,
}

pub const LOCS: &[Loc] = &[Loc::Us, Loc::Eu, Loc::Fr, Loc::Ch, Loc::EuPlain];

pub fn fmt_num(x: f64, decimals: u8, loc: Loc, paren_neg: bool) -> String {
    let neg = x < 0.0;
    let s = format!("{:.*}", decimals as usize, x.abs());
    let (int, frac) = match s.split_once('.') {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (s.clone(), None),
    };
    let (group, dec) = match loc {
        Loc::Us => (",", "."),
        Loc::Eu => (".", ","),
        Loc::Fr => ("\u{a0}", ","),
        Loc::Ch => ("'", "."),
        Loc::EuPlain => ("", ","),
    };
    let digits: Vec<char> = int.chars().collect();
    let mut g = String::new();
    for (k, ch) in digits.iter().enumerate() {
        if k > 0 && (digits.len() - k) % 3 == 0 {
            g.push_str(group);
        }
        g.push(*ch);
    }
    let body = match frac {
        Some(f) => format!("{g}{dec}{f}"),
        None => g,
    };
    if neg {
        if paren_neg { format!("({body})") } else { format!("-{body}") }
    } else {
        body
    }
}

/// (year, month, day) of an Excel serial date.
pub fn ymd(serial: f64) -> (i64, u32, u32) {
    let z = serial as i64 - 25569 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn fmt_date(serial: f64, loc: Loc) -> String {
    let (y, m, d) = ymd(serial);
    match loc {
        Loc::Us => format!("{m}/{d}/{y}"),
        Loc::Eu => format!("{d:02}.{m:02}.{y}"),
        Loc::Fr => format!("{d:02}/{m:02}/{y}"),
        Loc::Ch => format!("{y}-{m:02}-{d:02}"),
        Loc::EuPlain => format!("{d}.{m}.{y}."),
    }
}

pub fn decimals(kind: Kind) -> u8 {
    match kind {
        Kind::Int => 0,
        Kind::Dec(d) => d,
        Kind::Money | Kind::Signed => 2,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------------------------
// Plans

#[derive(Clone, Debug)]
pub enum CI {
    Col(usize),
    Blank,
    /// A separator column (`|`) through the body.
    Sep(String),
}

#[derive(Clone, Debug)]
pub enum RI {
    Data(usize),
    Blank,
    /// A note line inside or next to the body (label column only).
    Note(String),
    /// A separator row (`-----`) across the table.
    Sep(String),
    /// A group label alone on its row; the rows below carry it as a label.
    Group(String),
    Total { label: String, rows: Vec<usize>, kind: TotKind },
    OfWhich { label: String, parent: usize, frac: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PStyle {
    Merged,
    /// Text in the first column of the span only.
    Once,
    /// Text in every column of the span.
    Repeated,
    /// Text in the middle column, no merge ("centre across selection").
    Centered,
}

#[derive(Clone, Debug)]
pub struct Parent {
    pub level: usize,
    pub first: usize,
    pub last: usize,
    pub text: String,
    pub style: PStyle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyStyle {
    Repeat,
    /// Written on the first row of a run only.
    Once,
    /// Vertically merged over its run.
    Merge,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KHStyle {
    Bottom,
    Top,
    Merged,
}

#[derive(Clone, Debug, Default)]
pub struct CellNoise {
    /// Numbers written as text in this convention, for this share of cells.
    pub locale: Option<Loc>,
    pub text_share: f64,
    pub paren_neg: bool,
    pub missing: f64,
    pub footnote: f64,
    pub mixed: f64,
    pub date_text: Option<Loc>,
    /// Columns (base index) the value noise touches; empty = all measures.
    pub only_cols: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub base: BaseTable,
    /// Header text as written per base column.
    pub headers: Vec<String>,
    /// Expected header path per base column.
    pub header_truth: Vec<Vec<String>>,
    pub parents: Vec<Parent>,
    pub levels: usize,
    pub key_header: KHStyle,
    pub unit_row: Option<Vec<String>>,
    pub cols: Vec<CI>,
    pub rows: Vec<RI>,
    pub key_style: KeyStyle,
    /// Group key column replaced by group label rows.
    pub group_rows: bool,
    pub title: Vec<String>,
    pub notes_above: Vec<String>,
    pub notes_below: Vec<String>,
    pub notes_gap: u32,
    pub header_sep: Option<String>,
    pub noise: CellNoise,
    pub formula_totals: f64,
    pub scale: Option<(f64, Vec<String>)>,
    /// Labels every value of this table must keep (a block title that tells stacked blocks apart).
    pub extra_labels: Vec<String>,
    pub show_header: bool,
    pub may_split: bool,
    pub merge_group: Option<usize>,
    /// Comments right of the table, by data row index.
    pub comments: Vec<(usize, String)>,
    /// Total rows' label column (base column index); None = the first key column shown.
    pub total_label_col: Option<usize>,
}

impl Plan {
    pub fn new(base: BaseTable) -> Plan {
        let headers: Vec<String> = base.cols.iter().map(|c| c.header.clone()).collect();
        let header_truth = headers.iter().map(|h| vec![h.clone()]).collect();
        let cols = (0..base.cols.len()).map(CI::Col).collect();
        let rows = (0..base.rows.len()).map(RI::Data).collect();
        Plan {
            title: vec![base.title.clone()],
            base,
            headers,
            header_truth,
            parents: Vec::new(),
            levels: 0,
            key_header: KHStyle::Bottom,
            unit_row: None,
            cols,
            rows,
            key_style: KeyStyle::Repeat,
            group_rows: false,
            notes_above: Vec::new(),
            notes_below: Vec::new(),
            notes_gap: 1,
            header_sep: None,
            noise: CellNoise::default(),
            formula_totals: 0.0,
            scale: None,
            extra_labels: Vec::new(),
            show_header: true,
            may_split: false,
            merge_group: None,
            comments: Vec::new(),
            total_label_col: None,
        }
    }

    pub fn is_key(&self, j: usize) -> bool {
        self.base.cols[j].kind == Kind::Key
    }

    /// Row labels of data row `i`: every key value.
    pub fn row_labels(&self, i: usize) -> Vec<String> {
        let mut out: Vec<String> = self.base.keys().iter().map(|&k| self.base.text(i, k)).collect();
        out.extend(self.extra_labels.iter().cloned());
        out
    }

    /// Drop the group key column and put group label rows before each group.
    pub fn use_group_rows(&mut self, fmt: &dyn Fn(&str) -> String) {
        let Some(g) = self.base.group else { return };
        self.group_rows = true;
        self.cols.retain(|c| !matches!(c, CI::Col(j) if *j == g));
        let mut rows = Vec::new();
        let mut last: Option<String> = None;
        for ri in std::mem::take(&mut self.rows) {
            if let RI::Data(i) = ri {
                let v = self.base.text(i, g);
                if last.as_deref() != Some(v.as_str()) {
                    rows.push(RI::Group(fmt(&v)));
                    last = Some(v);
                }
            }
            rows.push(ri);
        }
        self.rows = rows;
    }

    fn header_rows(&self) -> usize {
        if self.show_header { self.levels + 1 } else { 0 }
    }
}

/// Renders `p` with its top-left corner at (top, left). Returns the bottom row used.
pub fn render(cv: &mut Canvas, rng: &mut Rng, p: &Plan, top: u32, left: u32) -> u32 {
    let tid = cv.new_table(TableT { may_split: p.may_split, merge_group: p.merge_group, scale: p.scale.clone() });
    let mut r = top;
    for t in &p.title {
        cv.text(r, left, t, Some("title"));
        r += 1;
    }
    for n in &p.notes_above {
        cv.text(r, left, n, Some("note"));
        r += 1;
    }
    // column positions
    let mut pos: Vec<u32> = Vec::new(); // per CI
    for (k, _) in p.cols.iter().enumerate() {
        pos.push(left + k as u32);
    }
    let col_at = |j: usize| -> Option<u32> {
        p.cols.iter().position(|c| matches!(c, CI::Col(x) if *x == j)).map(|k| pos[k])
    };
    // header
    let hn = p.header_rows() as u32;
    if hn > 0 {
        for (k, ci) in p.cols.iter().enumerate() {
            let CI::Col(j) = ci else { continue };
            let c = pos[k];
            if p.is_key(*j) && hn > 1 {
                match p.key_header {
                    KHStyle::Bottom => cv.text(r + hn - 1, c, &p.headers[*j], None),
                    KHStyle::Top => cv.text(r, c, &p.headers[*j], None),
                    KHStyle::Merged => {
                        cv.text(r, c, &p.headers[*j], None);
                        cv.merges.push((r, c, r + hn - 1, c));
                    }
                }
            } else {
                cv.text(r + hn - 1, c, &p.headers[*j], None);
            }
        }
        for par in &p.parents {
            let hr = r + par.level as u32;
            let (Some(a), Some(b)) = (col_at(par.first), col_at(par.last)) else { continue };
            match par.style {
                PStyle::Merged => {
                    cv.text(hr, a, &par.text, None);
                    if b > a {
                        cv.merges.push((hr, a, hr, b));
                    }
                }
                PStyle::Once => cv.text(hr, a, &par.text, None),
                PStyle::Repeated => {
                    for k in 0..p.cols.len() {
                        if matches!(p.cols[k], CI::Col(_)) && pos[k] >= a && pos[k] <= b {
                            cv.text(hr, pos[k], &par.text, None);
                        }
                    }
                }
                PStyle::Centered => cv.text(hr, a + (b - a) / 2, &par.text, None),
            }
        }
        r += hn;
    }
    if let Some(units) = &p.unit_row {
        for (k, ci) in p.cols.iter().enumerate() {
            if let CI::Col(j) = ci {
                cv.text(r, pos[k], &units[*j], Some("unit"));
            }
        }
        r += 1;
    }
    if let Some(s) = &p.header_sep {
        for k in 0..p.cols.len() {
            cv.text(r, pos[k], s, Some("separator"));
        }
        r += 1;
    }
    // body: assign rows first (totals may reference rows below them)
    let mut row_of: Vec<u32> = Vec::with_capacity(p.rows.len());
    let mut data_row: BTreeMap<usize, u32> = BTreeMap::new();
    for (k, ri) in p.rows.iter().enumerate() {
        row_of.push(r + k as u32);
        if let RI::Data(i) = ri {
            data_row.insert(*i, r + k as u32);
        }
    }
    let label_col = p
        .total_label_col
        .and_then(col_at)
        .or_else(|| p.cols.iter().enumerate().find(|(_, c)| matches!(c, CI::Col(j) if p.is_key(*j))).map(|(k, _)| pos[k]))
        .unwrap_or(pos[0]);
    let measure_cis: Vec<(u32, usize)> = p
        .cols
        .iter()
        .enumerate()
        .filter_map(|(k, c)| match c {
            CI::Col(j) if p.base.cols[*j].kind.numeric() => Some((pos[k], *j)),
            _ => None,
        })
        .collect();
    let group_labels: BTreeMap<usize, String> = {
        // data row → label of the group row above it (group rows mode)
        let mut m = BTreeMap::new();
        let mut cur: Option<String> = None;
        for ri in &p.rows {
            match ri {
                RI::Group(s) => cur = Some(s.clone()),
                RI::Data(i) => {
                    if let Some(s) = &cur {
                        m.insert(*i, s.clone());
                    }
                }
                _ => {}
            }
        }
        m
    };
    let mut prev_key: BTreeMap<usize, (String, u32)> = BTreeMap::new(); // key col → (value, first row of run)
    let mut runs: Vec<(u32, u32, u32)> = Vec::new(); // (col, first row, last row) for merged keys
    for (k, ri) in p.rows.iter().enumerate() {
        let rr = row_of[k];
        match ri {
            RI::Data(i) => {
                let i = *i;
                let mut labels = p.row_labels(i);
                if let Some(g) = group_labels.get(&i) {
                    labels.push(g.clone());
                }
                for (q, ci) in p.cols.iter().enumerate() {
                    let c = pos[q];
                    match ci {
                        CI::Blank => {}
                        CI::Sep(s) => cv.text(rr, c, s, Some("separator")),
                        CI::Col(j) => {
                            let j = *j;
                            let kind = p.base.cols[j].kind;
                            if kind == Kind::Key {
                                let v = p.base.text(i, j);
                                let is_group = p.base.group == Some(j);
                                let continues = prev_key.get(&j).is_some_and(|(pv, _)| *pv == v)
                                    && k > 0
                                    && matches!(p.rows[k - 1], RI::Data(_));
                                if is_group && p.key_style != KeyStyle::Repeat && continues {
                                    if p.key_style == KeyStyle::Merge {
                                        if let Some(run) = runs.last_mut().filter(|x| x.0 == c) {
                                            run.2 = rr;
                                        }
                                    }
                                    continue;
                                }
                                prev_key.insert(j, (v.clone(), rr));
                                if is_group && p.key_style == KeyStyle::Merge {
                                    runs.push((c, rr, rr));
                                }
                                cv.put(rr, c, W::Text(v.clone()));
                                cv.data.push(DataT {
                                    r: rr,
                                    c,
                                    table: tid,
                                    exp: Expect::Text(v),
                                    scale: None,
                                    labels: Vec::new(),
                                    header: p.header_truth[j].clone(),
                                });
                            } else {
                                let (w, exp) = render_value(rng, p, i, j, kind);
                                cv.put(rr, c, w);
                                let scale = if kind.numeric() { p.scale.as_ref().map(|s| s.0) } else { None };
                                cv.data.push(DataT {
                                    r: rr,
                                    c,
                                    table: tid,
                                    exp,
                                    scale,
                                    labels: labels.clone(),
                                    header: p.header_truth[j].clone(),
                                });
                            }
                        }
                    }
                }
                if let Some((_, s)) = p.comments.iter().find(|(x, _)| *x == i) {
                    let c = pos[pos.len() - 1] + 1;
                    cv.text(rr, c, s, Some("comment"));
                }
            }
            RI::Blank => {}
            RI::Note(s) => cv.text(rr, pos[0], s, Some("note")),
            RI::Sep(s) => {
                for &c in &pos {
                    cv.text(rr, c, s, Some("separator"));
                }
            }
            RI::Group(s) => cv.text(rr, pos[0], s, Some("group")),
            RI::Total { label, rows, kind } => {
                cv.text(rr, label_col, label, Some("total_label"));
                for &(c, j) in &measure_cis {
                    let vals: Vec<f64> = rows.iter().map(|&i| p.base.num(i, j)).collect();
                    let d = decimals(p.base.cols[j].kind).max(if *kind == TotKind::Average { 2 } else { 0 });
                    let v = match kind {
                        TotKind::Average => round(vals.iter().sum::<f64>() / vals.len().max(1) as f64, d),
                        _ => round(vals.iter().sum::<f64>(), d.min(6)),
                    };
                    let srows: Vec<u32> = rows.iter().filter_map(|i| data_row.get(i).copied()).collect();
                    let w = if *kind == TotKind::Sum && rng.chance(p.formula_totals) && !srows.is_empty() {
                        let contiguous = srows.windows(2).all(|w| w[1] == w[0] + 1)
                            && srows.iter().all(|&x| {
                                // no other row kind inside the range
                                row_of.iter().position(|&y| y == x).is_some()
                            });
                        let a1 = |row: u32| hh_core::a1(row, c);
                        let f = if contiguous && !srows.is_empty() {
                            format!("SUM({}:{})", a1(srows[0]), a1(*srows.last().unwrap()))
                        } else {
                            srows.iter().map(|&x| a1(x)).collect::<Vec<_>>().join("+")
                        };
                        W::Formula(f, v)
                    } else {
                        W::Num(v)
                    };
                    cv.put(rr, c, w);
                    cv.totals.push(TotT { r: rr, c, table: tid, kind: *kind });
                }
            }
            RI::OfWhich { label, parent, frac } => {
                cv.text(rr, label_col, label, Some("total_label"));
                for &(c, j) in &measure_cis {
                    let v = round(p.base.num(*parent, j) * frac, decimals(p.base.cols[j].kind));
                    cv.put(rr, c, W::Num(v));
                    cv.totals.push(TotT { r: rr, c, table: tid, kind: TotKind::OfWhich });
                }
            }
        }
    }
    for (c, a, b) in runs {
        if b > a {
            cv.merges.push((a, c, b, c));
        }
    }
    let mut bottom = row_of.last().copied().unwrap_or(r.saturating_sub(1));
    if !p.notes_below.is_empty() {
        let mut nr = bottom + 1 + p.notes_gap;
        for n in &p.notes_below {
            cv.text(nr, left, n, Some("note"));
            nr += 1;
        }
        bottom = nr - 1;
    }
    bottom
}

const MISSING: &[&str] = &["-", "…", "..", "–", "n/a", ":", "—"];
const MIXED: &[&str] = &["confidential", "<5", "n.a.", "see note", ">1000", "not available", "c", "approx."];
const MARKS: &[&str] = &["¹", "²", "*", " (r)", " p", "e", " (p)", "³"];

fn render_value(rng: &mut Rng, p: &Plan, i: usize, j: usize, kind: Kind) -> (W, Expect) {
    let n = &p.noise;
    let touched = n.only_cols.is_empty() || n.only_cols.contains(&j);
    match &p.base.rows[i][j] {
        V::Date(d) => {
            if let Some(loc) = n.date_text.filter(|_| touched) {
                (W::Text(fmt_date(*d, loc)), Expect::Date(*d))
            } else {
                (W::Date(*d), Expect::Date(*d))
            }
        }
        V::Text(s) => (W::Text(s.clone()), Expect::Text(s.clone())),
        V::Num(x) => {
            let x = *x;
            if !touched {
                return (W::Num(x), Expect::Num(x));
            }
            if rng.chance(n.missing) {
                return (W::Text(rng.pick(MISSING).to_string()), Expect::Null);
            }
            if rng.chance(n.mixed) {
                let s = rng.pick(MIXED).to_string();
                return (W::Text(s.clone()), Expect::TextOrNull(s));
            }
            let d = decimals(kind);
            let as_text = n.locale.is_some() && rng.chance(n.text_share);
            let mark = rng.chance(n.footnote);
            if !as_text && !mark {
                return (W::Num(x), Expect::Num(x));
            }
            let loc = n.locale.unwrap_or(Loc::Us);
            let mut s = if as_text { fmt_num(x, d, loc, n.paren_neg) } else { fmt_num(x, d, Loc::Us, false).replace(',', "") };
            if mark {
                let m = rng.pick(MARKS);
                s.push_str(m);
            }
            (W::Text(s), Expect::Num(x))
        }
    }
}
