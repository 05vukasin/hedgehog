//! Per-sheet extraction: layout tables → flat header + typed columns; totals → `TotalItem`s;
//! every other filled cell → `NoteItem`s. Each source cell ends up in exactly one place
//! (table, Totals or Notes), which `CellCounts` reports.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use super::typing::{display, fit, type_column, Slot};
use super::{
    CellCounts, ColRole, Column, Dtype, NoteItem, Normalized, Options, Out, RowType, SheetInfo, Table, TotalItem, Transforms,
    T,
};
use crate::analysis::{DetectedTable, Role, SheetLayout};
use crate::grid::Grid;
use crate::layout;
use crate::model::{a1, Rect, Sheet, Value};
use crate::totals::{self, CheckReason, Direction, KeywordLevel, TotalLevel, TotalOrigin};
use crate::values::{infer_column, norm_key, sheet_column_hints_with, ColumnHints};

const OWN_NONE: u8 = 0;
const OWN_TABLE: u8 = 1;
const OWN_TOTALS: u8 = 2;
const OWN_NOTES: u8 = 3;

/// Max blank rows between a loose title (note) and the table below (above) it.
const ASSOC_MAX_GAP: u32 = 6;
/// Excel's limit for a table column name.
const MAX_HEADER_LEN: usize = 255;
pub(super) const SRC_ROW: &str = "_src_row";
pub(super) const ROW_TYPE: &str = "_row_type";

fn col_letter(col: u32) -> String {
    let s = a1(0, col);
    s.trim_end_matches('1').to_string()
}

fn range_a1(r: Rect) -> String {
    if r.top == r.bottom && r.left == r.right {
        a1(r.top, r.left)
    } else {
        format!("{}:{}", a1(r.top, r.left), a1(r.bottom, r.right))
    }
}

fn level_str(l: TotalLevel) -> &'static str {
    match l {
        TotalLevel::Total => "total",
        TotalLevel::Subtotal => "subtotal",
        TotalLevel::GrandTotal => "grand_total",
    }
}

/// `CheckReason::DoubleCounted` → `double_counted`.
fn snake(debug: impl std::fmt::Debug) -> String {
    let s = format!("{debug:?}");
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// What a body row of a table is, from its cells' layout roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Data,
    Total,
    Group,
    RepeatedHeader,
    Note,
    Title,
}

fn row_kind(roles: impl Iterator<Item = Role>) -> RowKind {
    let (mut total, mut group, mut header, mut note, mut title, mut data) = (false, false, false, false, false, false);
    for r in roles {
        match r {
            Role::Total => total = true,
            Role::GroupHeader => group = true,
            Role::Header => header = true,
            Role::Note => note = true,
            Role::Title => title = true,
            Role::Data | Role::Other => data = true,
        }
    }
    if total {
        RowKind::Total
    } else if group {
        RowKind::Group
    } else if header {
        RowKind::RepeatedHeader
    } else if data {
        RowKind::Data
    } else if note {
        RowKind::Note
    } else if title {
        RowKind::Title
    } else {
        RowKind::Data
    }
}

/// Header + group column name for "Region: North"-style labels, if all labels share the prefix.
fn group_prefix(labels: &[&str]) -> Option<String> {
    let mut prefix: Option<(String, String)> = None;
    for l in labels {
        let (p, rest) = l.split_once(':')?;
        let (p, rest) = (p.trim(), rest.trim());
        if p.is_empty() || rest.is_empty() || p.chars().count() > 40 {
            return None;
        }
        let key = norm_key(p);
        match &prefix {
            None => prefix = Some((p.to_string(), key)),
            Some((_, k)) if *k == key => {}
            Some(_) => return None,
        }
    }
    prefix.map(|p| p.0)
}

/// Lowercase ASCII `[a-z0-9_]` slug (Cyrillic transliterated, diacritics folded).
pub(super) fn slug(s: &str, max: usize) -> String {
    let key = norm_key(s);
    let mut out = String::new();
    for ch in key.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
        if out.len() >= max {
            break;
        }
    }
    out.truncate(max);
    out.trim_matches('_').to_string()
}

/// Makes `name` unique (case-insensitive, like Excel) by appending ` (2)`, ` (3)`…
fn unique(name: String, seen: &mut HashSet<String>, tr: &mut Transforms) -> String {
    if seen.insert(name.to_lowercase()) {
        return name;
    }
    tr.add(T::DuplicateHeadersRenamed, 1);
    let mut k = 2;
    loop {
        let cand = format!("{name} ({k})");
        if seen.insert(cand.to_lowercase()) {
            return cand;
        }
        k += 1;
    }
}

struct Built {
    table: Table,
    /// Source rows that stayed in the table's data.
    kept: HashSet<u32>,
    /// Removed total rows: (row, row type, cell indices inside the table).
    removed_totals: Vec<(u32, RowType, Vec<usize>)>,
    notes: Vec<NoteItem>,
}

type Head = Option<(String, (u32, u32))>;

#[allow(clippy::too_many_arguments)]
fn build_table(
    grid: &Grid,
    lay: &SheetLayout,
    t: &DetectedTable,
    n: usize,
    sheet_hints: &OnceCell<HashMap<u32, ColumnHints>>,
    row_level: &HashMap<u32, TotalLevel>,
    opts: &Options,
    owner: &mut [u8],
) -> Built {
    let sheet = grid.sheet;
    let cells = &sheet.cells;
    let r = t.rect;
    let ncols = (r.right - r.left + 1) as usize;
    let mut tr = Transforms::default();
    let mut notes: Vec<NoteItem> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let note = |i: usize, kind: &'static str| {
        let c = &cells[i];
        NoteItem {
            sheet: sheet.name.clone(),
            row: c.row,
            col: c.col,
            kind,
            table: Some(n),
            text: c.value.as_ref().map(display).unwrap_or_default(),
            formula: c.formula.as_ref().map(|f| format!("={f}")),
        }
    };

    // Cells of the table, grouped by row (sheet.cells is row-major).
    let lo = cells.partition_point(|c| c.row < r.top);
    let hi = cells.partition_point(|c| c.row <= r.bottom);
    let mut rows: Vec<(u32, Vec<usize>)> = Vec::new();
    for (i, c) in cells.iter().enumerate().take(hi).skip(lo) {
        if c.col < r.left || c.col > r.right {
            continue;
        }
        match rows.last_mut() {
            Some((row, v)) if *row == c.row => v.push(i),
            _ => rows.push((c.row, vec![i])),
        }
    }
    let htop = t.header_top.clamp(r.top, r.bottom);
    let body_top = (htop + t.header_rows).min(r.bottom + 1);
    let nh = (body_top - htop) as usize;

    // Title rows inside the rect (above the header).
    let mut title: Option<(String, (u32, u32))> = None;
    let mut title_rows = 0;
    for (_, idxs) in rows.iter().filter(|(row, _)| *row < htop) {
        title_rows += 1;
        for &i in idxs {
            owner[i] = OWN_NOTES;
            let it = note(i, "title");
            if title.is_none() && !it.text.is_empty() {
                title = Some((it.text.clone(), (it.row, it.col)));
            }
            notes.push(it);
        }
    }
    tr.add(T::TitleRowsMoved, title_rows);

    // --- Header ----------------------------------------------------------------------------
    let mut head: Vec<Vec<Head>> = vec![vec![None; ncols]; nh];
    for (row, idxs) in rows.iter().filter(|(row, _)| *row >= htop && *row < body_top) {
        for &i in idxs {
            owner[i] = OWN_TABLE;
            let c = &cells[i];
            if let Some(v) = &c.value {
                let s = display(v);
                if !s.is_empty() {
                    head[(row - htop) as usize][(c.col - r.left) as usize] = Some((s, (c.row, c.col)));
                }
            }
        }
    }
    // Merged header cells: every covered position takes the top-left text.
    let mut merged_in_header: HashSet<(usize, usize)> = HashSet::new();
    if nh > 0 {
        for m in &sheet.merges {
            if m.bottom < htop || m.top >= body_top || m.right < r.left || m.left > r.right || m.top < htop {
                continue;
            }
            let Some(text) = grid.cell(m.top, m.left).and_then(|c| c.value.as_ref()).map(display).filter(|s| !s.is_empty())
            else {
                continue;
            };
            for rr in m.top..=m.bottom.min(body_top - 1) {
                for cc in m.left.max(r.left)..=m.right.min(r.right) {
                    let (hr, hc) = ((rr - htop) as usize, (cc - r.left) as usize);
                    merged_in_header.insert((hr, hc));
                    if head[hr][hc].is_none() {
                        head[hr][hc] = Some((text.clone(), (m.top, m.left)));
                        tr.add(T::MergedCellsFilled, 1);
                    }
                }
            }
        }
    }
    // Blank cells right of a parent label (not merged) take it, while columns below are labelled
    // and the parent's own parent does not change.
    for hr in 0..nh.saturating_sub(1) {
        let mut carry: Option<(Head, usize)> = None;
        for hc in 0..ncols {
            let below = (hr + 1..nh).any(|k| head[k][hc].is_some());
            if head[hr][hc].is_some() {
                carry = (below && !merged_in_header.contains(&(hr, hc))).then(|| (head[hr][hc].clone(), hc));
                continue;
            }
            let Some((val, from)) = &carry else { continue };
            let same_parent = hr == 0 || head[hr - 1][hc] == head[hr - 1][*from];
            if below && same_parent {
                head[hr][hc] = val.clone();
                tr.add(T::HeaderCellsForwardFilled, 1);
            } else {
                carry = None;
            }
        }
    }

    // --- Body --------------------------------------------------------------------------------
    // Vertically merged body cells (e.g. a label over 3 rows): fill down in the label's column.
    let mut fill: HashMap<(u32, u32), &Value> = HashMap::new();
    for m in &sheet.merges {
        if m.top < body_top || m.top > r.bottom || m.left < r.left || m.left > r.right || m.bottom == m.top {
            continue;
        }
        if let Some(v) = grid.cell(m.top, m.left).and_then(|c| c.value.as_ref()) {
            for rr in m.top + 1..=m.bottom.min(r.bottom) {
                fill.insert((rr, m.left), v);
            }
        }
    }
    let mut slots: Vec<Vec<Slot>> = vec![Vec::new(); ncols];
    let mut src_rows: Vec<u32> = Vec::new();
    let mut row_types: Vec<RowType> = Vec::new();
    let mut row_group: Vec<Option<usize>> = Vec::new();
    let mut paths: Vec<Vec<String>> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let mut group_labels: Vec<String> = Vec::new();
    let mut removed_totals = Vec::new();
    let mut body_rows_seen = 0u32;
    for (row, idxs) in rows.iter().filter(|(row, _)| *row >= body_top) {
        body_rows_seen += 1;
        let kind = row_kind(idxs.iter().map(|&i| lay.roles[i]));
        match kind {
            RowKind::Group => {
                let label = idxs
                    .iter()
                    .filter_map(|&i| cells[i].value.as_ref())
                    .map(display)
                    .find(|s| !s.is_empty())
                    .unwrap_or_default();
                for &i in idxs {
                    owner[i] = OWN_TABLE;
                }
                tr.add(T::GroupHeaderRowsToColumn, 1);
                group_labels.push(label.clone());
                run.push(label);
                continue;
            }
            RowKind::RepeatedHeader => {
                tr.add(T::RepeatedHeaderRowsRemoved, 1);
                for &i in idxs {
                    owner[i] = OWN_NOTES;
                    notes.push(note(i, "repeated_header"));
                }
                continue;
            }
            RowKind::Note | RowKind::Title => {
                tr.add(T::NoteRowsMoved, 1);
                for &i in idxs {
                    owner[i] = OWN_NOTES;
                    notes.push(note(i, if kind == RowKind::Note { "note" } else { "title" }));
                }
                continue;
            }
            RowKind::Total | RowKind::Data => {}
        }
        let rt = if kind == RowKind::Total {
            let level = row_level.get(row).copied().or_else(|| {
                idxs.iter()
                    .filter_map(|&i| match &cells[i].value {
                        Some(Value::Text(s)) => totals::total_keyword(s),
                        _ => None,
                    })
                    .next()
                    .map(|k| match k {
                        KeywordLevel::Subtotal => TotalLevel::Subtotal,
                        KeywordLevel::GrandTotal => TotalLevel::GrandTotal,
                        KeywordLevel::Total => TotalLevel::Total,
                    })
            });
            if level == Some(TotalLevel::Subtotal) { RowType::Subtotal } else { RowType::Total }
        } else {
            RowType::Data
        };
        if rt != RowType::Data && !opts.keep_totals {
            tr.add(T::TotalRowsRemoved, 1);
            removed_totals.push((*row, rt, idxs.clone()));
            continue;
        }
        if rt != RowType::Data {
            tr.add(T::TotalRowsKept, 1);
        }
        if !run.is_empty() {
            let keep = path.len().saturating_sub(run.len());
            path.truncate(keep);
            path.append(&mut run);
            paths.push(path.clone());
        }
        row_group.push((!path.is_empty()).then(|| paths.len() - 1));
        let mut row_slots: Vec<Slot> = vec![Slot::Empty; ncols];
        for &i in idxs {
            owner[i] = OWN_TABLE;
            let c = &cells[i];
            let k = (c.col - r.left) as usize;
            row_slots[k] = match &c.value {
                Some(v) => {
                    tr.add(T::FormulasReplacedByValues, c.formula.is_some() as usize);
                    Slot::Val(v)
                }
                None if c.formula.is_some() => Slot::NoCache,
                None => Slot::Empty,
            };
        }
        for (k, s) in row_slots.iter_mut().enumerate() {
            if matches!(s, Slot::Empty)
                && let Some(v) = fill.get(&(*row, r.left + k as u32))
            {
                *s = Slot::Val(v);
                tr.add(T::MergedCellsFilled, 1);
            }
        }
        for (k, s) in row_slots.into_iter().enumerate() {
            slots[k].push(s);
        }
        src_rows.push(*row);
        row_types.push(rt);
    }
    if r.bottom + 1 > body_top {
        tr.add(T::BlankRowsRemoved, (r.bottom + 1 - body_top).saturating_sub(body_rows_seen) as usize);
    }

    // --- Columns -----------------------------------------------------------------------------
    let mut data_cols: Vec<Column> = Vec::new();
    let mut col_warnings = Vec::new();
    for k in 0..ncols {
        let col = r.left + k as u32;
        let mut parts: Vec<(String, (u32, u32))> = Vec::new();
        for h in head.iter().flat_map(|row| &row[k]) {
            if parts.last().is_none_or(|p| p.0 != h.0) {
                parts.push(h.clone());
            }
        }
        let mut name = parts.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join(" / ");
        if name.chars().count() > MAX_HEADER_LEN {
            name = name.chars().take(MAX_HEADER_LEN).collect();
        }
        let label = if name.is_empty() { format!("col_{}", col_letter(col)) } else { name.clone() };
        let first = opts.locale.map(|l| l.hints()).unwrap_or_default();
        let mut refine = |sample: &[&str], ambiguous: bool| -> ColumnHints {
            let inferred = infer_column(sample.iter().copied());
            if let Some(l) = opts.locale {
                return ColumnHints { currency_context: inferred.currency_context, ..l.hints() };
            }
            let mut h = inferred;
            // The sheet-wide vote is only worth its cost when a guess is actually needed.
            if ambiguous && (h.decimal.is_none() || h.date_order.is_none()) {
                let sh = sheet_hints
                    .get_or_init(|| sheet_column_hints_with(grid, None))
                    .get(&col)
                    .copied()
                    .unwrap_or_default();
                h.decimal = h.decimal.or(sh.decimal);
                h.date_order = h.date_order.or(sh.date_order);
                h.currency_context |= sh.currency_context;
            }
            h
        };
        let typed = type_column(&label, col, &slots[k], &src_rows, first, &mut refine, &mut tr);
        if parts.is_empty() && typed.dtype == Dtype::Empty {
            tr.add(T::EmptyColumnsRemoved, 1);
            continue;
        }
        if parts.is_empty() {
            tr.add(T::MissingHeadersNamed, 1);
        }
        if parts.len() > 1 {
            tr.add(T::HeadersFlattened, 1);
        }
        col_warnings.extend(typed.warnings);
        data_cols.push(Column {
            name: label,
            role: ColRole::Data,
            source_col: Some(col),
            original_header: parts,
            dtype: typed.dtype,
            unit: typed.unit,
            currency: typed.currency,
            decimal: typed.decimal,
            date_order: typed.date_order,
            ambiguous: typed.ambiguous,
            values: typed.values,
        });
    }

    // Group column from group-header rows.
    let mut group_col: Option<Column> = None;
    if !group_labels.is_empty() {
        let refs: Vec<&str> = group_labels.iter().map(String::as_str).collect();
        let prefix = group_prefix(&refs);
        let strip = |l: &str| -> String {
            match &prefix {
                Some(_) => l.split_once(':').map(|(_, rest)| rest.trim().to_string()).unwrap_or_else(|| l.to_string()),
                None => l.to_string(),
            }
        };
        let joined: Vec<String> =
            paths.iter().map(|p| p.iter().map(|l| strip(l)).collect::<Vec<_>>().join(" / ")).collect();
        let values: Vec<Out> = row_group
            .iter()
            .map(|g| g.map_or(Out::Null, |i| Out::Text(fit(joined[i].clone(), &mut tr))))
            .collect();
        group_col = Some(Column {
            name: prefix.unwrap_or_else(|| "group".to_string()),
            role: ColRole::Group,
            source_col: None,
            original_header: Vec::new(),
            dtype: if values.iter().all(|v| *v == Out::Null) { Dtype::Empty } else { Dtype::Text },
            unit: None,
            currency: None,
            decimal: None,
            date_order: None,
            ambiguous: 0,
            values,
        });
    }

    // Unique names: reserved columns claim theirs first.
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(SRC_ROW.to_string());
    if opts.keep_totals {
        seen.insert(ROW_TYPE.to_string());
    }
    let mut columns: Vec<Column> = Vec::new();
    for mut c in group_col.into_iter().chain(data_cols) {
        c.name = unique(std::mem::take(&mut c.name), &mut seen, &mut tr);
        columns.push(c);
    }
    let plain = |name: &str, role: ColRole, dtype: Dtype, values: Vec<Out>| Column {
        name: name.to_string(),
        role,
        source_col: None,
        original_header: Vec::new(),
        dtype,
        unit: None,
        currency: None,
        decimal: None,
        date_order: None,
        ambiguous: 0,
        values,
    };
    columns.push(plain(SRC_ROW, ColRole::SrcRow, Dtype::Integer, src_rows.iter().map(|&r| Out::Num(r as f64 + 1.0)).collect()));
    if opts.keep_totals {
        let values = row_types.iter().map(|t| Out::Text(t.as_str().to_string())).collect();
        columns.push(plain(ROW_TYPE, ColRole::RowType, Dtype::Text, values));
    }

    if t.header_rows == 0 {
        warnings.push("no header row detected; columns are named col_<letter> after their source column".to_string());
    }
    if src_rows.is_empty() {
        warnings.push("no data rows".to_string());
    }
    if t.confidence < 0.5 {
        warnings.push(format!("low table-detection confidence ({:.2})", t.confidence));
    }
    warnings.extend(col_warnings);

    let kept: HashSet<u32> = src_rows.iter().copied().collect();
    Built {
        table: Table {
            n,
            sheet: String::new(),
            source_sheet: sheet.name.clone(),
            rect: r,
            header_top: htop,
            header_rows: t.header_rows,
            title,
            notes: Vec::new(),
            confidence: t.confidence,
            columns,
            src_rows,
            row_types,
            transforms: tr,
            warnings,
        },
        kept,
        removed_totals,
        notes,
    }
}

/// Nearest table below (title) or above (note) a loose cell, within `ASSOC_MAX_GAP` blank rows
/// and overlapping its columns (with a little slack on the left).
fn associate(tables: &[Table], row: u32, col: u32, title: bool) -> Option<usize> {
    let mut best: Option<(u32, usize)> = None;
    for (k, t) in tables.iter().enumerate() {
        let r = t.rect;
        if col > r.right || col + 2 < r.left {
            continue;
        }
        let dist = if title {
            if r.top <= row {
                continue;
            }
            r.top - row - 1
        } else {
            if row <= r.bottom {
                continue;
            }
            row - r.bottom - 1
        };
        if dist <= ASSOC_MAX_GAP && best.is_none_or(|b| dist < b.0) {
            best = Some((dist, k));
        }
    }
    best.map(|b| b.1)
}

pub(super) fn process_sheet(sheet: &Sheet, opts: &Options, out: &mut Normalized) {
    let grid = Grid::new(sheet);
    let cells = &sheet.cells;
    let mut info = SheetInfo {
        name: sheet.name.clone(),
        used_range: sheet.used_range(),
        cells: cells.len(),
        formulas: sheet.formula_count(),
        merges: sheet.merges.len(),
        tables: Vec::new(),
    };
    if cells.is_empty() {
        out.sheets.push(info);
        return;
    }
    let lay = layout::analyze(&grid);
    let mut owner = vec![OWN_NONE; cells.len()];
    let sheet_hints = OnceCell::new();
    let cands = totals::detect(&grid);
    let checks: Vec<_> = cands.iter().map(|c| totals::verify(&grid, c)).collect();
    let mut row_level: HashMap<u32, TotalLevel> = HashMap::new();
    for c in cands.iter().filter(|c| c.direction == Direction::Vertical) {
        let e = row_level.entry(c.row).or_insert(c.level);
        if c.level == TotalLevel::Subtotal {
            *e = TotalLevel::Subtotal;
        }
    }

    // Tables.
    let first = out.tables.len();
    let mut kept: Vec<HashSet<u32>> = Vec::new();
    let mut removed: Vec<(usize, u32, RowType, Vec<usize>)> = Vec::new();
    let mut notes: Vec<NoteItem> = Vec::new();
    for t in &lay.tables {
        let n = out.tables.len() + 1;
        let b = build_table(&grid, &lay, t, n, &sheet_hints, &row_level, opts, &mut owner);
        info.tables.push(n);
        kept.push(b.kept);
        removed.extend(b.removed_totals.into_iter().map(|(row, rt, idxs)| (n, row, rt, idxs)));
        notes.extend(b.notes);
        out.tables.push(b.table);
    }
    let tables = &out.tables[first..];
    let table_at = |row: u32, col: u32| -> Option<usize> {
        tables.iter().position(|t| t.rect.top <= row && row <= t.rect.bottom && t.rect.left <= col && col <= t.rect.right)
    };

    // Totals: detected candidates with their verification.
    let mut items: Vec<TotalItem> = Vec::new();
    let mut listed: HashSet<(u32, u32)> = HashSet::new();
    for (cand, check) in cands.iter().zip(&checks) {
        let k = table_at(cand.row, cand.col);
        let kept_in_data = k.is_some_and(|k| kept[k].contains(&cand.row));
        if let Some(i) = grid.cell_index(cand.row, cand.col)
            && owner[i] != OWN_TABLE
        {
            owner[i] = OWN_TOTALS;
        }
        let label = cand.label.and_then(|(lr, lc)| {
            let i = grid.cell_index(lr, lc)?;
            if owner[i] == OWN_NONE {
                owner[i] = OWN_TOTALS;
            }
            cells[i].value.as_ref().map(display)
        });
        listed.insert((cand.row, cand.col));
        items.push(TotalItem {
            table: k.map(|k| first + k + 1),
            sheet: sheet.name.clone(),
            row: cand.row,
            col: cand.col,
            label,
            label_cell: cand.label,
            level: level_str(cand.level),
            direction: match cand.direction {
                Direction::Vertical => "vertical",
                Direction::Horizontal => "horizontal",
            },
            origin: match cand.origin {
                TotalOrigin::Formula => "formula",
                TotalOrigin::Typed => "typed",
            },
            value: check.actual,
            recomputed: check.expected,
            status: match check.ok {
                Some(true) => "ok",
                Some(false) => "mismatch",
                None => "unknown",
            },
            reason: if check.reason == CheckReason::Match { "match".into() } else { snake(check.reason) },
            sources: cand.ranges.iter().map(|r| range_a1(*r)).collect(),
            formula: grid.cell(cand.row, cand.col).and_then(|c| c.formula.as_ref()).map(|f| format!("={f}")),
            kept_in_data,
        });
    }
    // Removed total rows: numbers not found by `totals::detect` are listed unverified; the row's
    // label text is attached; other text goes to Notes.
    for (n, row, rt, idxs) in &removed {
        let label_idx = idxs.iter().copied().find(|&i| matches!(cells[i].value, Some(Value::Text(_))));
        let label = label_idx.and_then(|i| cells[i].value.as_ref()).map(display);
        for &i in idxs {
            let c = &cells[i];
            if owner[i] != OWN_NONE {
                continue;
            }
            let num = match &c.value {
                Some(Value::Number(v)) => Some(*v),
                _ => None,
            };
            if Some(i) == label_idx {
                owner[i] = OWN_TOTALS;
            } else if num.is_some() || (c.value.is_none() && c.formula.is_some()) {
                owner[i] = OWN_TOTALS;
                if listed.insert((c.row, c.col)) {
                    items.push(TotalItem {
                        table: Some(*n),
                        sheet: sheet.name.clone(),
                        row: c.row,
                        col: c.col,
                        label: label.clone(),
                        label_cell: label_idx.map(|j| (cells[j].row, cells[j].col)),
                        level: rt.as_str(),
                        direction: "vertical",
                        origin: "layout",
                        value: num,
                        recomputed: None,
                        status: "unknown",
                        reason: "not_verified".into(),
                        sources: Vec::new(),
                        formula: c.formula.as_ref().map(|f| format!("={f}")),
                        kept_in_data: false,
                    });
                }
            } else {
                owner[i] = OWN_NOTES;
                notes.push(NoteItem {
                    sheet: sheet.name.clone(),
                    row: *row,
                    col: c.col,
                    kind: "total_row",
                    table: Some(*n),
                    text: c.value.as_ref().map(display).unwrap_or_default(),
                    formula: c.formula.as_ref().map(|f| format!("={f}")),
                });
            }
        }
    }
    items.sort_by_key(|t| (t.row, t.col));

    // Everything not yet placed → Notes; titles / notes are attached to the nearest table.
    for (i, c) in cells.iter().enumerate() {
        if owner[i] != OWN_NONE {
            continue;
        }
        owner[i] = OWN_NOTES;
        let kind = match lay.roles[i] {
            Role::Title => "title",
            Role::Note => "note",
            Role::Total => "total",
            _ => "other",
        };
        let table = match kind {
            "title" => associate(tables, c.row, c.col, true),
            "note" => associate(tables, c.row, c.col, false),
            _ => None,
        };
        notes.push(NoteItem {
            sheet: sheet.name.clone(),
            row: c.row,
            col: c.col,
            kind,
            table: table.map(|k| first + k + 1),
            text: c.value.as_ref().map(display).unwrap_or_default(),
            formula: c.formula.as_ref().map(|f| format!("={f}")),
        });
    }
    notes.sort_by_key(|n| (n.row, n.col));

    // Titles / notes per table, then output names.
    let tables = &mut out.tables[first..];
    for t in tables.iter_mut() {
        let mine: Vec<&NoteItem> =
            notes.iter().filter(|x| x.table == Some(t.n) && x.kind != "repeated_header" && !x.text.is_empty()).collect();
        if t.title.is_none() {
            // Nearest loose title above the table.
            t.title = mine
                .iter()
                .filter(|x| x.kind == "title" && x.row < t.rect.top)
                .max_by_key(|x| x.row)
                .map(|x| (x.text.clone(), (x.row, x.col)));
        }
        let title_cell = t.title.as_ref().map(|x| x.1);
        t.notes = mine
            .iter()
            .filter(|x| matches!(x.kind, "title" | "note") && Some((x.row, x.col)) != title_cell)
            .map(|x| (x.text.clone(), (x.row, x.col)))
            .collect();
        let prefix = format!("T{}_", t.n);
        let base = t.title.as_ref().map(|x| slug(&x.0, 31 - prefix.len())).filter(|s| !s.is_empty());
        let base = base
            .or_else(|| Some(slug(&t.source_sheet, 31 - prefix.len())).filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "table".to_string());
        t.sheet = format!("{prefix}{base}");
    }

    let mut counts = CellCounts { total: cells.len(), ..Default::default() };
    for o in &owner {
        match *o {
            OWN_TABLE => counts.in_tables += 1,
            OWN_TOTALS => counts.in_totals += 1,
            _ => counts.in_notes += 1,
        }
    }
    out.cells.total += counts.total;
    out.cells.in_tables += counts.in_tables;
    out.cells.in_totals += counts.in_totals;
    out.cells.in_notes += counts.in_notes;
    out.totals.extend(items);
    out.notes.extend(notes);
    out.sheets.push(info);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Prodaja po regionima — 2024", 26), "prodaja_po_regionima_2024");
        assert_eq!(slug("Укупно Београд", 26), "ukupno_beograd");
        assert_eq!(slug("  ***  ", 26), "");
        assert!(slug("a very long title that goes on and on and on", 10).len() <= 10);
    }

    #[test]
    fn group_prefixes() {
        assert_eq!(group_prefix(&["Region: North", "region: South"]), Some("Region".into()));
        assert_eq!(group_prefix(&["Region: North", "Area: South"]), None);
        assert_eq!(group_prefix(&["North"]), None);
    }
}
