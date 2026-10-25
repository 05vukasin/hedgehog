//! Which formula cells reference which cells, within one sheet — the backbone of `trace(cell)`.

use std::collections::HashMap;

use crate::grid::Grid;
use crate::totals::formula::{parse, ParsedFormula, Ref};

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
        for c in &sheet.cells {
            if let Some(f) = &c.formula {
                by_pos.insert((c.row, c.col), formulas.len());
                formulas.push(FormulaEntry { row: c.row, col: c.col, parsed: parse(f) });
            }
        }

        // Column → [(row, cell index)] sorted by row (cells are row-major).
        let mut columns: HashMap<u32, Vec<(u32, usize)>> = HashMap::new();
        for (i, c) in sheet.cells.iter().enumerate() {
            columns.entry(c.col).or_default().push((c.row, i));
        }
        let mut col_keys: Vec<u32> = columns.keys().copied().collect();
        col_keys.sort_unstable();

        let mut marks = vec![0u8; sheet.cells.len()];
        let mut budget = MARK_BUDGET;
        'outer: for f in &formulas {
            let bits = if f.parsed.kind.is_aggregate() { REFERENCED | BY_AGGREGATE } else { REFERENCED };
            for r in f.parsed.refs.iter().filter(|r| r.is_local(&sheet.name)) {
                let lo = col_keys.partition_point(|&c| c < r.rect.left);
                let hi = col_keys.partition_point(|&c| c <= r.rect.right);
                for col in &col_keys[lo..hi] {
                    let cells = &columns[col];
                    let a = cells.partition_point(|&(row, _)| row < r.rect.top);
                    let b = cells.partition_point(|&(row, _)| row <= r.rect.bottom);
                    for &(row, idx) in &cells[a..b] {
                        if (row, *col) != (f.row, f.col) {
                            marks[idx] |= bits;
                        }
                    }
                    budget = budget.saturating_sub(b - a + 1);
                    if budget == 0 {
                        break 'outer;
                    }
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, Sheet, Value};

    // The original traversal, including its per-column budget charge and self exclusion.
    fn reference_marks(sheet: &Sheet, formulas: &[FormulaEntry]) -> Vec<u8> {
        let mut columns: HashMap<u32, Vec<(u32, usize)>> = HashMap::new();
        for (i, c) in sheet.cells.iter().enumerate() {
            columns.entry(c.col).or_default().push((c.row, i));
        }
        let mut keys: Vec<_> = columns.keys().copied().collect();
        keys.sort_unstable();
        let mut marks = vec![0; sheet.cells.len()];
        let mut budget = MARK_BUDGET;
        'outer: for f in formulas {
            let bits = if f.parsed.kind.is_aggregate() { REFERENCED | BY_AGGREGATE } else { REFERENCED };
            for r in f.parsed.refs.iter().filter(|r| r.is_local(&sheet.name)) {
                let lo = keys.partition_point(|&c| c < r.rect.left);
                let hi = keys.partition_point(|&c| c <= r.rect.right);
                for col in &keys[lo..hi] {
                    let cells = &columns[col];
                    let a = cells.partition_point(|&(row, _)| row < r.rect.top);
                    let b = cells.partition_point(|&(row, _)| row <= r.rect.bottom);
                    for &(row, idx) in &cells[a..b] {
                        if (row, *col) != (f.row, f.col) { marks[idx] |= bits; }
                    }
                    budget = budget.saturating_sub(b - a + 1);
                    if budget == 0 { break 'outer; }
                }
            }
        }
        marks
    }

    fn formula(row: u32, col: u32, f: &str) -> Cell {
        Cell { row, col, value: Some(Value::Number(1.0)), formula: Some(f.into()) }
    }

    #[test]
    fn dependency_marks_match_reference_for_overlaps_upgrades_and_self_holes() {
        let mut sheet = Sheet { name: "Data".into(), ..Sheet::default() };
        for row in 0..9 {
            for col in 0..3 {
                sheet.cells.push(Cell { row, col, value: Some(Value::Number(1.0)), formula: None });
            }
        }
        for (i, f) in ["A1:A9", "SUM(A3:A6)", "A1:A9", "SUM(A1:A9)", "A1:C9+A1:C9",
            "SUM(A20:A30)", "Other!A1", "SUM(A:A)", "B1:B2", "B5:B6", "B2:B5",
            "SUM(C1:C2)", "SUM(C5:C6)", "SUM(C2:C5)"].iter().enumerate()
        {
            sheet.cells.push(formula(10 + i as u32, 4, f));
        }
        // Two formulas cover one another, so neither self hole may certify a full slice.
        sheet.cells.push(formula(0, 5, "SUM(F1:F6)"));
        sheet.cells.push(formula(1, 5, "SUM(F1:F6)"));
        sheet.cells.push(formula(2, 5, "SUM(F1:F6)"));
        // A later self-containing range may use coverage certified by a different formula.
        sheet.cells.push(formula(9, 4, "SUM(F:F)"));
        sheet.cells.push(formula(21, 5, "SUM(F:F)"));
        // Duplicate source positions remain separate mark entries, including a formula at one.
        sheet.cells.push(formula(4, 0, "A1:A9"));
        sheet.cells.sort_by_key(|c| (c.row, c.col));
        for reverse in [false, true] {
            let mut input = sheet.clone();
            if reverse { input.cells.reverse(); }
            let actual = DependencyIndex::new(&Grid::new(&input));
            assert_eq!(actual.marks, reference_marks(&input, &actual.formulas), "reverse={reverse}");
        }
    }

    #[test]
    fn dependency_mark_budget_is_charged_even_for_repeated_full_columns() {
        let mut sheet = Sheet { name: "Data".into(), ..Sheet::default() };
        for row in 0..5000 {
            sheet.cells.push(Cell { row, col: 0, value: Some(Value::Number(1.0)), formula: None });
            sheet.cells.push(formula(row, 1, if row % 100 == 0 { "SUM(A:A)" } else { "A:A" }));
        }
        sheet.cells.push(formula(5001, 2, "B:B"));
        let actual = DependencyIndex::new(&Grid::new(&sheet));
        assert_eq!(actual.marks, reference_marks(&sheet, &actual.formulas));
        assert!(sheet.cells.iter().enumerate().filter(|(_, c)| c.col == 0).all(|(i, _)| actual.marks[i] == 3));
        assert!(sheet.cells.iter().enumerate().filter(|(_, c)| c.col == 1).all(|(i, _)| actual.marks[i] == 0));
    }
}
