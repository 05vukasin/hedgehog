//! Finds total, subtotal and grand-total cells: aggregate formulas over the cells above / to the
//! left, and typed-in numbers in rows (or columns) labelled with a total keyword.

use std::collections::{HashMap, HashSet};

use crate::grid::Grid;
use crate::model::{Cell, Rect, Value};
use crate::totals::deps::DependencyIndex;
use crate::totals::formula::FormulaKind;
use crate::totals::keywords::{total_keyword, KeywordLevel};
use crate::totals::verify::{close, numeric_text, verify};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TotalOrigin {
    /// The cell holds an aggregate formula.
    Formula,
    /// A typed-in number in a row / column labelled as total.
    Typed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TotalLevel {
    Total,
    /// Total of one group inside a table, followed by more groups or a grand total.
    Subtotal,
    /// Total of totals (or of everything above, excluding subtotals).
    GrandTotal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Column total: adds cells above, in the same column.
    Vertical,
    /// Row total: adds cells to the left, in the same row.
    Horizontal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    /// The cell's formula is an aggregate of this kind.
    Formula(FormulaKind),
    /// The row (vertical) or column header (horizontal) carries a total keyword.
    Keyword(KeywordLevel),
    /// Its source contains other totals that it adds once more (double counting).
    ContainsTotals,
    /// It adds up other totals (or everything except the subtotals) — a grand total.
    SumsTotals,
    /// The typed value equals the sum of the inferred source (used to choose the source).
    ArithmeticMatch,
    /// "X Total" whose group starts at the row (row, col) labelled "X".
    GroupLabel(u32, u32),
    /// Typed total without an arithmetic match, but most other totals in its row / column match,
    /// so the inferred source is trusted and a mismatch is reported as wrong.
    SiblingsMatch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TotalCandidate {
    pub row: u32,
    pub col: u32,
    pub origin: TotalOrigin,
    pub level: TotalLevel,
    pub direction: Direction,
    /// Source ranges on the same sheet, in the order the total adds them.
    pub ranges: Vec<Rect>,
    /// Cells inside `ranges` the total does not add (nested SUBTOTALs, subtotal rows of a typed
    /// grand total).
    pub exclude: Vec<(u32, u32)>,
    /// Other totals inside `ranges` that this total adds again on top of their data.
    pub double_counted: Vec<(u32, u32)>,
    /// Label cell: the keyword cell, or the nearest text to the left (vertical) / above (horizontal).
    pub label: Option<(u32, u32)>,
    pub evidence: Vec<Evidence>,
}

impl TotalCandidate {
    fn new(row: u32, col: u32, origin: TotalOrigin, direction: Direction) -> Self {
        TotalCandidate {
            row,
            col,
            origin,
            level: TotalLevel::Total,
            direction,
            ranges: Vec::new(),
            exclude: Vec::new(),
            double_counted: Vec::new(),
            label: None,
            evidence: Vec::new(),
        }
    }

    /// True if (row, col) is one of the cells this total adds.
    pub fn covers(&self, row: u32, col: u32) -> bool {
        self.ranges.iter().any(|r| r.top <= row && row <= r.bottom && r.left <= col && col <= r.right)
            && !self.exclude.contains(&(row, col))
    }
}

fn text(cell: Option<&Cell>) -> Option<&str> {
    match cell?.value.as_ref()? {
        Value::Text(s) => Some(s),
        _ => None,
    }
}

fn number(cell: Option<&Cell>) -> Option<f64> {
    match cell?.value.as_ref()? {
        Value::Number(v) | Value::DateTime(v) => Some(*v),
        _ => None,
    }
}

/// Row → (label column, level) for rows whose leftmost keyword text cell names a total and that
/// hold at least one number or formula right of it (a header row "Name | Q1 | Total" does not).
pub fn keyword_rows(grid: &Grid) -> HashMap<u32, (u32, KeywordLevel)> {
    let mut out = HashMap::new();
    let mut label: Option<(u32, u32, KeywordLevel)> = None; // (row, col, level) of the current row
    for c in &grid.sheet.cells {
        if label.is_some_and(|(r, _, _)| r != c.row) {
            label = None;
        }
        if out.contains_key(&c.row) {
            continue;
        }
        match (&label, &c.value) {
            (None, Some(Value::Text(s))) => {
                if let Some(level) = total_keyword(s) {
                    label = Some((c.row, c.col, level));
                }
            }
            (Some((r, lc, lvl)), v) => {
                if c.formula.is_some() || matches!(v, Some(Value::Number(_))) {
                    out.insert(*r, (*lc, *lvl));
                }
            }
            _ => {}
        }
    }
    out
}

/// Nearest text cell left of (row, col) in the same row.
fn text_left(grid: &Grid, row: u32, col: u32) -> Option<u32> {
    // Row cells are contiguous in the row-major cell list; scan them via the grid instead of
    // assuming an order.
    let used = grid.used_range()?;
    (used.left..col).rev().find(|&c| text(grid.cell(row, c)).is_some_and(|s| numeric_text(s).is_none()))
}

/// Nearest text cell above (row, col) in the same column.
fn text_above(grid: &Grid, row: u32, col: u32) -> Option<u32> {
    let used = grid.used_range()?;
    (used.top..row).rev().find(|&r| text(grid.cell(r, col)).is_some_and(|s| numeric_text(s).is_none()))
}

pub fn detect(grid: &Grid) -> Vec<TotalCandidate> {
    detect_with(grid, &DependencyIndex::new(grid))
}

/// [`detect`] reusing an already built dependency index.
pub fn detect_with(grid: &Grid, deps: &DependencyIndex) -> Vec<TotalCandidate> {
    let sheet = grid.sheet;
    let kw_rows = keyword_rows(grid);
    let mut out: Vec<TotalCandidate> = Vec::new();

    // (a) Aggregate formulas over cells above / to the left.
    for f in deps.formulas() {
        let p = &f.parsed;
        if !p.kind.is_sum() || p.has_error || p.sum_refs.is_empty() {
            continue;
        }
        if p.sum_refs.iter().any(|r| !r.is_local(&sheet.name)) {
            continue;
        }
        let vertical = p.sum_refs.iter().all(|r| r.rect.left == f.col && r.rect.right == f.col && r.rect.bottom < f.row);
        let horizontal =
            !vertical && p.sum_refs.iter().all(|r| r.rect.top == f.row && r.rect.bottom == f.row && r.rect.right < f.col);
        if !vertical && !horizontal {
            continue;
        }
        let dir = if vertical { Direction::Vertical } else { Direction::Horizontal };
        let mut c = TotalCandidate::new(f.row, f.col, TotalOrigin::Formula, dir);
        c.evidence.push(Evidence::Formula(p.kind));
        c.ranges = p.sum_refs.iter().map(|r| r.rect).collect();
        let keyword = if vertical {
            kw_rows.get(&f.row).filter(|(lc, _)| *lc < f.col).map(|&(lc, lvl)| ((f.row, lc), lvl))
        } else {
            text_above(grid, f.row, f.col)
                .and_then(|r| total_keyword(text(grid.cell(r, f.col))?).map(|lvl| ((r, f.col), lvl)))
        };
        let area: u64 = p.sum_refs.iter().map(|r| r.area()).sum();
        if area < 2 && keyword.is_none() {
            continue; // `=SUM(C5)` is a link, not a total
        }
        match keyword {
            Some((cell, lvl)) => {
                c.label = Some(cell);
                c.evidence.push(Evidence::Keyword(lvl));
                c.level = level_of(lvl);
            }
            None => {
                c.label = if vertical {
                    text_left(grid, f.row, f.col).map(|lc| (f.row, lc))
                } else {
                    text_above(grid, f.row, f.col).map(|r| (r, f.col))
                };
            }
        }
        if p.kind == FormulaKind::Subtotal {
            // SUBTOTAL skips cells that themselves hold SUBTOTAL formulas.
            for r in &c.ranges {
                for g in deps.formulas() {
                    if r.top <= g.row && g.row <= r.bottom && r.left <= g.col && g.col <= r.right {
                        if g.parsed.functions.iter().any(|n| n == "SUBTOTAL") {
                            c.exclude.push((g.row, g.col));
                        }
                    }
                }
            }
        }
        out.push(c);
    }

    let formula_total: HashSet<(u32, u32)> = out.iter().map(|c| (c.row, c.col)).collect();
    let rows_with_cells: HashSet<u32> = sheet.cells.iter().map(|c| c.row).collect();

    // (b) Typed numbers in keyword rows → column totals over the cells above.
    let mut kw_sorted: Vec<_> = kw_rows.iter().map(|(&r, &v)| (r, v)).collect();
    kw_sorted.sort_unstable_by_key(|x| x.0);
    for &(row, (lc, lvl)) in &kw_sorted {
        let cols: Vec<u32> = sheet
            .cells
            .iter()
            .filter(|c| c.row == row && c.col > lc && c.formula.is_none())
            .filter(|c| matches!(c.value, Some(Value::Number(_))))
            .map(|c| c.col)
            .collect();
        for col in cols {
            let value = number(grid.cell(row, col)).unwrap_or(0.0);
            if let Some(c) = typed_vertical(grid, &kw_rows, &formula_total, &rows_with_cells, row, col, lc, lvl, value)
            {
                out.push(c);
            }
        }
    }

    // (b') Typed numbers under a keyword column header → row totals over the cells to the left.
    let taken: HashSet<(u32, u32)> = out.iter().map(|c| (c.row, c.col)).collect();
    for h in &sheet.cells {
        let Some(Value::Text(s)) = &h.value else { continue };
        let Some(lvl) = total_keyword(s) else { continue };
        if kw_rows.get(&h.row).is_some_and(|&(lc, _)| lc != h.col) {
            continue; // a second keyword in a total row ("Total" … "Total %") is not a header
        }
        out.extend(typed_horizontal(grid, &taken, h.row, h.col, lvl));
    }

    resolve_nesting(grid, &mut out);
    mark_sibling_support(grid, &mut out);
    out.sort_by_key(|c| (c.row, c.col));
    out
}

fn level_of(k: KeywordLevel) -> TotalLevel {
    match k {
        KeywordLevel::Total => TotalLevel::Total,
        KeywordLevel::Subtotal => TotalLevel::Subtotal,
        KeywordLevel::GrandTotal => TotalLevel::GrandTotal,
    }
}

/// Group name announced by a total label: "Condor - Energy Total" → "condor - energy",
/// "Total East" / "Ukupno Vojvodina" → "east" / "vojvodina" (pivot-table and report style).
fn group_name(label: &str) -> Option<String> {
    let l = label.trim().trim_end_matches(':').trim().to_lowercase();
    const WORDS: [&str; 6] = ["total", "subtotal", "ukupno", "svega", "укупно", "свега"];
    for w in WORDS {
        if let Some(rest) = l.strip_suffix(w).filter(|r| r.ends_with(' ')) {
            return Some(rest.trim().to_string()).filter(|s| !s.is_empty());
        }
        if let Some(rest) = l.strip_prefix(w).filter(|r| r.starts_with(' ')) {
            return Some(rest.trim().to_string()).filter(|s| !s.is_empty());
        }
    }
    None
}

/// "X Total" whose group starts at the row labelled "X" in the same label column (Excel pivot
/// tables and outline reports): the source is that block, minus the deeper totals inside it.
#[allow(clippy::too_many_arguments)]
fn typed_group(
    grid: &Grid,
    kw_rows: &HashMap<u32, (u32, KeywordLevel)>,
    rows_with_cells: &HashSet<u32>,
    row: u32,
    col: u32,
    label_col: u32,
    lvl: KeywordLevel,
    value: f64,
) -> Option<TotalCandidate> {
    let name = group_name(text(grid.cell(row, label_col))?)?;
    let mut start = None;
    let mut r = row;
    while r > 0 && row - r < 10_000 {
        r -= 1;
        if !rows_with_cells.contains(&r) {
            break;
        }
        if let Some(&(lc, _)) = kw_rows.get(&r) {
            if lc <= label_col {
                break; // a sibling or a higher-level total: the group would have started below it
            }
            continue;
        }
        if text(grid.cell(r, label_col)).is_some_and(|t| t.trim().to_lowercase() == name) {
            start = Some(r);
            break;
        }
    }
    let start = start?;
    let mut exclude = Vec::new();
    let mut sum = 0.0;
    let mut n = 0;
    for r in start..row {
        let cell = grid.cell(r, col);
        if kw_rows.contains_key(&r) {
            if cell.is_some() {
                exclude.push((r, col));
            }
            continue;
        }
        if let Some(v) = number(cell) {
            sum += v;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    let mut c = TotalCandidate::new(row, col, TotalOrigin::Typed, Direction::Vertical);
    c.label = Some((row, label_col));
    c.evidence.push(Evidence::Keyword(lvl));
    c.evidence.push(Evidence::GroupLabel(start, label_col));
    c.level = level_of(lvl);
    c.ranges = vec![Rect { top: start, left: col, bottom: row - 1, right: col }];
    c.exclude = exclude;
    if close(value, sum, true) {
        c.evidence.push(Evidence::ArithmeticMatch);
    }
    Some(c)
}

#[allow(clippy::too_many_arguments)]
fn typed_vertical(
    grid: &Grid,
    kw_rows: &HashMap<u32, (u32, KeywordLevel)>,
    formula_total: &HashSet<(u32, u32)>,
    rows_with_cells: &HashSet<u32>,
    row: u32,
    col: u32,
    label_col: u32,
    lvl: KeywordLevel,
    value: f64,
) -> Option<TotalCandidate> {
    if let Some(c) = typed_group(grid, kw_rows, rows_with_cells, row, col, label_col, lvl, value) {
        return Some(c);
    }
    // Walk up: data rows until a header text, an empty row or the top; remember earlier totals.
    let mut top = row;
    let mut first_total: Option<u32> = None;
    let mut totals: Vec<u32> = Vec::new();
    let mut r = row;
    while r > 0 {
        r -= 1;
        if !rows_with_cells.contains(&r) {
            break;
        }
        let cell = grid.cell(r, col);
        if kw_rows.contains_key(&r) || formula_total.contains(&(r, col)) {
            if cell.is_some() {
                totals.push(r);
            }
            first_total.get_or_insert(r);
            top = r;
            continue;
        }
        if text(cell).is_some_and(|s| numeric_text(s).is_none()) {
            break;
        }
        top = r;
    }
    // Leading blanks and text (a year header "2023" stored as text) are not part of the source.
    while top < row && grid.cell(top, col).is_none_or(|c| c.formula.is_none() && number(Some(c)).is_none()) {
        top += 1;
    }
    if top == row {
        return None;
    }
    let sum_rows = |from: u32, skip: &[u32]| -> (f64, usize) {
        let mut s = 0.0;
        let mut n = 0;
        for r in from..row {
            if skip.contains(&r) {
                continue;
            }
            if let Some(v) = number(grid.cell(r, col)) {
                s += v;
                n += 1;
            }
        }
        (s, n)
    };
    // Section since the previous total, and everything above except the earlier totals.
    let mut section_top = first_total.map_or(top, |t| t + 1);
    while section_top < row && grid.cell(section_top, col).is_none_or(|c| c.formula.is_none() && number(Some(c)).is_none()) {
        section_top += 1;
    }
    let (s_section, n_section) = if section_top < row { sum_rows(section_top, &[]) } else { (0.0, 0) };
    let (s_all, n_all) = sum_rows(top, &totals);
    if n_all == 0 {
        return None;
    }
    let mut c = TotalCandidate::new(row, col, TotalOrigin::Typed, Direction::Vertical);
    c.label = Some((row, label_col));
    c.evidence.push(Evidence::Keyword(lvl));
    c.level = level_of(lvl);
    let section = |c: &mut TotalCandidate| {
        c.ranges = vec![Rect { top: section_top, left: col, bottom: row - 1, right: col }];
    };
    // Everything above; earlier totals excluded (a grand total when there are any).
    let whole = |c: &mut TotalCandidate| {
        c.ranges = vec![Rect { top, left: col, bottom: row - 1, right: col }];
        if !totals.is_empty() {
            c.exclude = totals.iter().map(|&r| (r, col)).collect();
            c.level = TotalLevel::GrandTotal;
            c.evidence.push(Evidence::SumsTotals);
        }
    };
    let has_section = n_section > 0;
    if has_section && close(value, s_section, true) {
        section(&mut c);
        c.evidence.push(Evidence::ArithmeticMatch);
    } else if close(value, s_all, true) {
        whole(&mut c);
        c.evidence.push(Evidence::ArithmeticMatch);
    } else if has_section && (totals.is_empty() || lvl != KeywordLevel::GrandTotal) {
        section(&mut c);
    } else if lvl != KeywordLevel::Subtotal {
        whole(&mut c);
    } else {
        return None;
    }
    Some(c)
}

/// Typed vertical totals whose source was not confirmed by arithmetic get
/// [`Evidence::SiblingsMatch`] when most other totals in the same row or column check out —
/// then a mismatch is a real error rather than a wrongly inferred source.
fn mark_sibling_support(grid: &Grid, cands: &mut [TotalCandidate]) {
    let matched: Vec<bool> = cands
        .iter()
        .map(|c| match c.origin {
            TotalOrigin::Typed => c.evidence.contains(&Evidence::ArithmeticMatch),
            TotalOrigin::Formula => verify(grid, c).ok == Some(true),
        })
        .collect();
    let mut by_row: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut by_col: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        if c.direction == Direction::Vertical {
            by_row.entry(c.row).or_default().push(i);
            by_col.entry(c.col).or_default().push(i);
        }
    }
    let supported: Vec<bool> = (0..cands.len())
        .map(|i| {
            let c = &cands[i];
            if c.origin != TotalOrigin::Typed || c.direction != Direction::Vertical || matched[i] {
                return false;
            }
            let others = by_row[&c.row].iter().chain(&by_col[&c.col]).filter(|&&j| j != i);
            let (mut n, mut ok) = (0, 0);
            for &j in others {
                n += 1;
                ok += matched[j] as usize;
            }
            ok >= 1 && 2 * ok >= n
        })
        .collect();
    for (c, s) in cands.iter_mut().zip(supported) {
        if s {
            c.evidence.push(Evidence::SiblingsMatch);
        }
    }
}

fn typed_horizontal(grid: &Grid, taken: &HashSet<(u32, u32)>, hrow: u32, hcol: u32, lvl: KeywordLevel) -> Vec<TotalCandidate> {
    // Typed numbers directly below the header.
    let mut rows = Vec::new();
    let mut r = hrow + 1;
    while let Some(cell) = grid.cell(r, hcol) {
        if cell.formula.is_some() || !matches!(cell.value, Some(Value::Number(_))) {
            break;
        }
        if !taken.contains(&(r, hcol)) {
            rows.push(r);
        }
        r += 1;
    }
    if rows.is_empty() || hcol == 0 {
        return Vec::new();
    }
    // Per row: the numbers to the left, nearest first, up to a text cell or two blanks.
    let lefts: Vec<Vec<(u32, f64)>> = rows
        .iter()
        .map(|&r| {
            let mut v = Vec::new();
            let mut blanks = 0;
            let mut c = hcol;
            while c > 0 && blanks < 2 {
                c -= 1;
                match grid.cell(r, c) {
                    None => blanks += 1,
                    Some(cell) => {
                        blanks = 0;
                        match &cell.value {
                            Some(Value::Number(x)) => v.push((c, *x)),
                            Some(Value::Text(s)) if numeric_text(s).is_none() => break,
                            _ => {}
                        }
                    }
                }
            }
            v
        })
        .collect();
    // The number of addends k must match arithmetically in at least half of the rows.
    let mut votes: HashMap<usize, usize> = HashMap::new();
    for (i, &r) in rows.iter().enumerate() {
        let value = number(grid.cell(r, hcol)).unwrap_or(0.0);
        let mut s = 0.0;
        for (k, &(_, x)) in lefts[i].iter().enumerate() {
            s += x;
            if k >= 1 && close(value, s, true) {
                *votes.entry(k + 1).or_default() += 1;
            }
        }
    }
    let Some((&k, &n)) = votes.iter().max_by_key(|(k, n)| (**n, **k)) else { return Vec::new() };
    if n < 2.max(rows.len().div_ceil(2)) {
        return Vec::new();
    }
    rows.iter()
        .zip(&lefts)
        .filter(|(_, l)| !l.is_empty())
        .map(|(&r, l)| {
            let left = l[(k - 1).min(l.len() - 1)].0;
            let mut c = TotalCandidate::new(r, hcol, TotalOrigin::Typed, Direction::Horizontal);
            c.ranges = vec![Rect { top: r, left, bottom: r, right: hcol - 1 }];
            c.label = Some((hrow, hcol));
            c.level = level_of(lvl);
            c.evidence.push(Evidence::Keyword(lvl));
            let value = number(grid.cell(r, hcol)).unwrap_or(0.0);
            let sum: f64 = l.iter().take(k).map(|x| x.1).sum();
            c.evidence.push(if close(value, sum, true) { Evidence::ArithmeticMatch } else { Evidence::SiblingsMatch });
            c
        })
        .collect()
}

/// Totals inside other totals' sources (same direction): an outer total that adds only totals is a
/// grand total over subtotals; one that adds inner totals *and* the data they summarise
/// double-counts. Inner totals become subtotals.
fn resolve_nesting(grid: &Grid, cands: &mut [TotalCandidate]) {
    let pos: HashMap<(u32, u32), usize> = cands.iter().enumerate().map(|(i, c)| ((c.row, c.col), i)).collect();
    let mut inner_all: Vec<usize> = Vec::new();
    let mut updates: Vec<(usize, bool, bool, Vec<usize>)> = Vec::new();
    for i in 0..cands.len() {
        let c = &cands[i];
        let mut inside: Vec<usize> = Vec::new();
        let mut data_cells = 0usize;
        let area: u64 = c.ranges.iter().map(|r| (r.bottom - r.top + 1) as u64 * (r.right - r.left + 1) as u64).sum();
        if area > 200_000 {
            continue;
        }
        for r in &c.ranges {
            for row in r.top..=r.bottom {
                for col in r.left..=r.right {
                    if (row, col) == (c.row, c.col) || c.exclude.contains(&(row, col)) {
                        continue;
                    }
                    match pos.get(&(row, col)) {
                        Some(&j) if cands[j].direction == c.direction => {
                            if !inside.contains(&j) {
                                inside.push(j);
                            }
                        }
                        _ => data_cells += number(grid.cell(row, col)).is_some() as usize,
                    }
                }
            }
        }
        let excluded_totals: Vec<usize> = c.exclude.iter().filter_map(|p| pos.get(p).copied()).collect();
        let within = |inner: &TotalCandidate| {
            inner.ranges.iter().all(|ir| {
                c.ranges.iter().any(|r| r.top <= ir.top && ir.bottom <= r.bottom && r.left <= ir.left && ir.right <= r.right)
            })
        };
        let nested: Vec<usize> = inside.iter().copied().filter(|&j| within(&cands[j])).collect();
        let sums_totals = !inside.is_empty() && data_cells == 0;
        inner_all.extend(excluded_totals.iter().copied());
        let grand = !excluded_totals.is_empty() || sums_totals || !nested.is_empty();
        if sums_totals {
            inner_all.extend(inside.iter().copied());
        } else if !nested.is_empty() {
            inner_all.extend(nested.iter().copied());
        }
        updates.push((i, grand, sums_totals, if sums_totals { Vec::new() } else { nested }));
    }
    let inner_set: HashSet<usize> = inner_all.iter().copied().collect();
    for (i, grand, sums_totals, nested) in updates {
        let mut cells: Vec<(u32, u32)> = nested.iter().map(|&j| (cands[j].row, cands[j].col)).collect();
        cells.sort_unstable();
        let c = &mut cands[i];
        if sums_totals && !c.evidence.contains(&Evidence::SumsTotals) {
            c.evidence.push(Evidence::SumsTotals);
        }
        if !cells.is_empty() {
            c.evidence.push(Evidence::ContainsTotals);
            c.double_counted = cells;
        }
        // A total that is itself inside a bigger one stays a (sub)total.
        if grand && c.level == TotalLevel::Total && !inner_set.contains(&i) {
            c.level = TotalLevel::GrandTotal;
        }
    }
    for i in inner_all {
        if cands[i].level == TotalLevel::Total {
            cands[i].level = TotalLevel::Subtotal;
        }
    }
}
