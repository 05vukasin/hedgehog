//! Which formula cells reference which cells, within one sheet — the backbone of `trace(cell)`.

use std::collections::HashMap;

use crate::grid::Grid;
use crate::totals::formula::{parse_profiled, ParsedFormula, Ref};

#[derive(Debug, Clone)]
pub struct FormulaEntry {
    pub row: u32,
    pub col: u32,
    pub parsed: ParsedFormula,
}

/// Parsed formulas of one sheet plus, for every non-empty cell, whether a formula on the same
/// sheet references it.
#[derive(Debug, Clone)]
pub struct DependencyIndex {
    sheet_name: String,
    formulas: Vec<FormulaEntry>,
    by_pos: HashMap<(u32, u32), usize>,
    /// Per `sheet.cells` index: bit 0 = referenced, bit 1 = referenced by an aggregate formula.
    marks: Vec<u8>,
}

const REFERENCED: u8 = 1;
const BY_AGGREGATE: u8 = 2;
/// Upper bound on cell visits while marking referenced cells (pathological sheets with thousands
/// of whole-column references); beyond it the marks are incomplete, never wrong.
const MARK_BUDGET: usize = 20_000_000;

impl DependencyIndex {
    pub fn new(grid: &Grid) -> Self {
        let sheet = grid.sheet;
        let mut formulas = Vec::new();
        let mut by_pos = HashMap::new();
        let started = std::time::Instant::now();
        let mut parse_times = [std::time::Duration::ZERO; 3];
        for c in &sheet.cells {
            if let Some(f) = &c.formula {
                by_pos.insert((c.row, c.col), formulas.len());
                formulas.push(FormulaEntry { row: c.row, col: c.col, parsed: parse_profiled(f, &mut parse_times) });
            }
        }
        crate::normalize::trace("prof8.deps.parse_wall", started);
        for (name, took) in ["prof8.parse.tokenize", "prof8.parse.classify", "prof8.parse.collect"].into_iter().zip(parse_times) {
            crate::normalize::trace_ms(name, took);
        }
        let started = std::time::Instant::now();

        // Column → [(row, cell index)] sorted by row (cells are row-major).
        let mut columns: HashMap<u32, Vec<(u32, usize)>> = HashMap::new();
        for (i, c) in sheet.cells.iter().enumerate() {
            columns.entry(c.col).or_default().push((c.row, i));
        }
        let mut col_keys: Vec<u32> = columns.keys().copied().collect();
        col_keys.sort_unstable();
        crate::normalize::trace("prof8.deps.columns", started);
        let started = std::time::Instant::now();

        let mut marks = vec![0u8; sheet.cells.len()];
        let mut budget = MARK_BUDGET;
        let mut counts = [0u64; 7];
        'outer: for f in &formulas {
            let bits = if f.parsed.kind.is_aggregate() { REFERENCED | BY_AGGREGATE } else { REFERENCED };
            for r in f.parsed.refs.iter().filter(|r| r.is_local(&sheet.name)) {
                counts[0] += 1;
                counts[1] += (r.rect.top == r.rect.bottom && r.rect.left == r.rect.right) as u64;
                let lo = col_keys.partition_point(|&c| c < r.rect.left);
                let hi = col_keys.partition_point(|&c| c <= r.rect.right);
                for col in &col_keys[lo..hi] {
                    counts[2] += 1;
                    let cells = &columns[col];
                    let a = cells.partition_point(|&(row, _)| row < r.rect.top);
                    let b = cells.partition_point(|&(row, _)| row <= r.rect.bottom);
                    counts[3] += (b - a) as u64;
                    counts[4] += (b - a + 1) as u64;
                    for &(row, idx) in &cells[a..b] {
                        if (row, *col) != (f.row, f.col) {
                            counts[5] += (marks[idx] & bits == bits) as u64;
                            marks[idx] |= bits;
                        } else {
                            counts[6] += 1;
                        }
                    }
                    budget = budget.saturating_sub(b - a + 1);
                    if budget == 0 {
                        break 'outer;
                    }
                }
            }
        }
        crate::normalize::trace("prof8.deps.mark_with_counters", started);
        if crate::normalize::tracing() {
            eprintln!("[profile-counts] deps cells={} formulas={} local_refs={} point_refs={} column_probes={} visits={} charged={} already_marked={} self_skips={} budget_left={}",
                sheet.cells.len(), formulas.len(), counts[0], counts[1], counts[2], counts[3], counts[4], counts[5], counts[6], budget);
        }
        DependencyIndex { sheet_name: sheet.name.clone(), formulas, by_pos, marks }
    }

    pub fn sheet_name(&self) -> &str {
        &self.sheet_name
    }

    /// All formula cells of the sheet, row-major.
    pub fn formulas(&self) -> &[FormulaEntry] {
        &self.formulas
    }

    pub fn formula(&self, row: u32, col: u32) -> Option<&ParsedFormula> {
        self.by_pos.get(&(row, col)).map(|&i| &self.formulas[i].parsed)
    }

    /// References the formula at (row, col) reads, including other sheets / workbooks.
    pub fn precedents(&self, row: u32, col: u32) -> &[Ref] {
        self.formula(row, col).map(|p| p.refs.as_slice()).unwrap_or(&[])
    }

    /// Formula cells on this sheet whose references cover (row, col), row-major.
    /// The cell may be empty — a blank inside `SUM(C5:C20)` still has that SUM as dependent.
    pub fn dependents(&self, row: u32, col: u32) -> Vec<(u32, u32)> {
        self.formulas
            .iter()
            .filter(|f| (f.row, f.col) != (row, col))
            .filter(|f| f.parsed.refs.iter().any(|r| r.is_local(&self.sheet_name) && r.contains(row, col)))
            .map(|f| (f.row, f.col))
            .collect()
    }

    /// Whether the cell at `sheet.cells[index]` is referenced by a formula on the same sheet.
    pub fn is_referenced(&self, index: usize) -> bool {
        self.marks.get(index).is_some_and(|m| m & REFERENCED != 0)
    }

    /// Whether the cell at `sheet.cells[index]` is inside an aggregate's (SUM, AVERAGE, …) reference.
    pub fn referenced_by_aggregate(&self, index: usize) -> bool {
        self.marks.get(index).is_some_and(|m| m & BY_AGGREGATE != 0)
    }
}
