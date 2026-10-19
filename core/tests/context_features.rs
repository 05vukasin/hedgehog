//! `context` features on small xlsx fixtures generated at test time (nothing committed).

use std::path::PathBuf;

use hh_core::context::{self, kind_of, Kind, FEATURES};
use hh_core::{open, Cell, Features, Grid, Rect, Sheet, Value};
use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};

fn fixture(name: &str, build: impl FnOnce(&mut rust_xlsxwriter::Worksheet)) -> Sheet {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_ctx_{}_{name}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    build(wb.add_worksheet());
    wb.save(&path).unwrap();
    let mut book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    book.sheets.remove(0)
}

struct Ctx<'a> {
    grid: Grid<'a>,
    f: Features,
}

impl<'a> Ctx<'a> {
    fn new(sheet: &'a Sheet) -> Self {
        let grid = Grid::new(sheet);
        let mut f = Features::new(sheet.cells.len());
        context::compute(&grid, &mut f);
        Ctx { grid, f }
    }

    /// Feature `name` at A1-ish 0-based (row, col).
    fn at(&self, name: &str, r: u32, c: u32) -> f32 {
        let i = self.grid.cell_index(r, c).unwrap_or_else(|| panic!("no cell at ({r},{c})"));
        self.f.value(name, i).unwrap_or_else(|| panic!("no feature {name}"))
    }
}

/// Title merged over B1:E1, empty row, header row 3, four data rows, total row with SUM formulas.
fn report(ws: &mut rust_xlsxwriter::Worksheet) {
    ws.merge_range(0, 1, 0, 4, "Sales report", &Format::new()).unwrap();
    for (c, h) in ["ID", "City", "Q1", "Q2"].iter().enumerate() {
        ws.write_string(2, 1 + c as u16, *h).unwrap();
    }
    let rows = [(1, "Beograd", 10.0, 12.0), (2, "Niš", 7.0, 9.5), (3, "Novi Sad", 8.0, 8.0), (4, "Niš", 3.0, 1.0)];
    for (i, (id, city, q1, q2)) in rows.iter().enumerate() {
        let r = 3 + i as u32;
        ws.write_number(r, 1, *id as f64).unwrap();
        ws.write_string(r, 2, *city).unwrap();
        ws.write_number(r, 3, *q1).unwrap();
        ws.write_number(r, 4, *q2).unwrap();
    }
    ws.write_string(7, 2, "Total").unwrap();
    ws.write_formula(7, 3, "=SUM(D4:D7)").unwrap();
    ws.write_formula(7, 4, "=SUM(E4:E7)").unwrap();
}

#[test]
fn adds_every_documented_column() {
    let sheet = fixture("cols", report);
    let ctx = Ctx::new(&sheet);
    assert_eq!(ctx.f.names(), FEATURES);
    for name in FEATURES {
        assert!(name.starts_with("c_"));
        assert!(ctx.f.get(name).unwrap().iter().all(|v| v.is_finite()), "{name} has non-finite values");
    }
}

#[test]
fn empty_sheet_still_has_columns() {
    let sheet = Sheet::default();
    let ctx = Ctx::new(&sheet);
    assert_eq!(ctx.f.names().len(), FEATURES.len());
    assert!(context::components(&ctx.grid).is_empty());
}

#[test]
fn kinds() {
    let cell = |value: Option<Value>, formula: Option<&str>| Cell { row: 0, col: 0, value, formula: formula.map(String::from) };
    assert_eq!(kind_of(&cell(Some(Value::Number(1.0)), None)), Kind::Number);
    assert_eq!(kind_of(&cell(Some(Value::Text("a".into())), None)), Kind::Text);
    assert_eq!(kind_of(&cell(Some(Value::DateTime(45000.0)), None)), Kind::Date);
    assert_eq!(kind_of(&cell(Some(Value::Bool(true)), None)), Kind::Bool);
    assert_eq!(kind_of(&cell(Some(Value::Error("#REF!".into())), None)), Kind::Error);
    assert_eq!(kind_of(&cell(None, Some("A1+1"))), Kind::FormulaOnly);
    assert_eq!(kind_of(&cell(Some(Value::Number(2.0)), Some("A1+1"))), Kind::Number);

    let sheet = fixture("kinds", |ws| {
        ws.write_number(0, 0, 1.0).unwrap();
        ws.write_string(0, 1, "x").unwrap();
        ws.write_datetime_with_format(0, 2, ExcelDateTime::from_ymd(2024, 1, 31).unwrap(), &Format::new().set_num_format("yyyy-mm-dd")).unwrap();
        ws.write_boolean(1, 0, true).unwrap();
        ws.merge_range(1, 1, 1, 2, "m", &Format::new()).unwrap();
    });
    let ctx = Ctx::new(&sheet);
    assert_eq!(ctx.at("c_kind", 0, 0), Kind::Number.code());
    assert_eq!(ctx.at("c_kind", 0, 2), Kind::Date.code());
    assert_eq!(ctx.at("c_kind", 1, 0), Kind::Bool.code());
    // Neighbours of B1: left number, right date, down = merge anchor (text), down-right covered by merge.
    assert_eq!(ctx.at("c_n_l", 0, 1), Kind::Number.code());
    assert_eq!(ctx.at("c_n_r", 0, 1), Kind::Date.code());
    assert_eq!(ctx.at("c_n_d", 0, 1), Kind::Text.code());
    assert_eq!(ctx.at("c_n_dr", 0, 1), Kind::MergedEmpty.code());
    assert_eq!(ctx.at("c_n_u", 0, 1), Kind::Empty.code());
    // B1 neighbours: up row (3) off-sheet, right-of-date… only C1-row: A1,C1,A2,B2,C2 filled -> 3 empty.
    assert_eq!(ctx.at("c_empty_neighbours", 0, 1), 3.0);
    assert_eq!(ctx.at("c_same_down", 0, 1), 1.0); // text over text
    assert_eq!(ctx.at("c_same_left", 0, 1), 0.0);
}

#[test]
fn merges_and_neighbours() {
    let sheet = fixture("merge", report);
    let ctx = Ctx::new(&sheet);
    // Title anchor B1, merged B1:E1.
    assert_eq!(ctx.at("c_in_merge", 0, 1), 1.0);
    assert_eq!(ctx.at("c_merge_width", 0, 1), 4.0);
    assert_eq!(ctx.at("c_merge_height", 0, 1), 1.0);
    assert_eq!(ctx.at("c_merge_top_left", 0, 1), 1.0);
    assert_eq!(ctx.at("c_n_r", 0, 1), Kind::MergedEmpty.code());
    assert_eq!(ctx.at("c_in_merge", 3, 1), 0.0);
    assert_eq!(ctx.at("c_merge_width", 3, 1), 0.0);
    // Inner data cell C5 ("Niš"): 8 filled neighbours, same type above/below.
    assert_eq!(ctx.at("c_empty_neighbours", 4, 2), 0.0);
    assert_eq!(ctx.at("c_same_up", 4, 2), 1.0);
    assert_eq!(ctx.at("c_same_down", 4, 2), 1.0);
    assert_eq!(ctx.at("c_same_left", 4, 2), 0.0);
    assert_eq!(ctx.at("c_same_right", 3, 3), 1.0);
    // Formula cells.
    assert_eq!(ctx.at("c_has_formula", 7, 3), 1.0);
    assert_eq!(ctx.at("c_row_formula_ratio", 7, 2), 2.0 / 3.0);
}

#[test]
fn row_aggregates() {
    let sheet = fixture("rows", report);
    let ctx = Ctx::new(&sheet);
    // Title row: single cell, sheet row has 1 cell.
    assert_eq!(ctx.at("c_row_single", 0, 1), 1.0);
    assert_eq!(ctx.at("c_row_filled", 0, 1), 1.0);
    // Header row: all text, next row numeric.
    assert_eq!(ctx.at("c_row_text_ratio", 2, 3), 1.0);
    assert_eq!(ctx.at("c_row_text_next_num", 2, 3), 1.0);
    assert_eq!(ctx.at("c_row_fill_ratio", 2, 3), 1.0);
    assert_eq!(ctx.at("c_row_first", 2, 1), 1.0);
    assert_eq!(ctx.at("c_row_first", 2, 2), 0.0);
    // Data row: 3 numbers + 1 text.
    assert_eq!(ctx.at("c_row_num_ratio", 3, 2), 0.75);
    assert_eq!(ctx.at("c_row_text_ratio", 3, 2), 0.25);
    assert_eq!(ctx.at("c_row_text_next_num", 3, 2), 0.0);
    // Total row: 3 cells of a 4-wide block.
    assert_eq!(ctx.at("c_row_cells", 7, 2), 3.0);
    assert_eq!(ctx.at("c_row_fill_ratio", 7, 2), 0.75);
    assert_eq!(ctx.at("c_row_first", 7, 2), 1.0);
}

#[test]
fn column_aggregates() {
    let sheet = fixture("cols2", report);
    let ctx = Ctx::new(&sheet);
    // ID column B3:B7 (block = rows 3..8): header text + 4 increasing numbers.
    assert_eq!(ctx.at("c_col_cells", 3, 1), 5.0);
    assert_eq!(ctx.at("c_col_consistency", 3, 1), 0.8);
    assert_eq!(ctx.at("c_col_type_match", 3, 1), 1.0);
    assert_eq!(ctx.at("c_col_type_match", 2, 1), 0.0);
    assert_eq!(ctx.at("c_col_num_ratio", 3, 1), 0.8);
    assert_eq!(ctx.at("c_col_monotonic", 3, 1), 1.0);
    // Q2: 12, 9.5, 8, 1, then SUM cached value (rust_xlsxwriter writes 0) -> no increasing steps.
    assert_eq!(ctx.at("c_col_monotonic", 3, 4), 0.0);
    // City column: City, Beograd, Niš, Novi Sad, Niš, Total -> 5 distinct of 6.
    assert!((ctx.at("c_col_distinct_ratio", 3, 2) - 5.0 / 6.0).abs() < 1e-6);
    // num below / text above.
    assert_eq!(ctx.at("c_col_num_below", 2, 1), 1.0);
    assert_eq!(ctx.at("c_col_text_above", 2, 1), 0.0);
    assert_eq!(ctx.at("c_col_text_above", 4, 1), 0.5);
    assert_eq!(ctx.at("c_col_num_below", 6, 1), 0.0); // last cell of the column
}

#[test]
fn header_helpers() {
    let sheet = fixture("hdr", report);
    let ctx = Ctx::new(&sheet);
    assert_eq!(ctx.at("c_text_over_numbers", 2, 3), 1.0); // "Q1"
    assert_eq!(ctx.at("c_text_over_numbers", 3, 3), 0.0); // a number
    assert_eq!(ctx.at("c_text_over_numbers", 2, 2), 0.0); // "City" is over text
    // City is the leftmost text-majority column, numbers to its right.
    assert_eq!(ctx.at("c_label_col", 4, 2), 1.0);
    assert_eq!(ctx.at("c_row_label", 4, 2), 1.0);
    assert_eq!(ctx.at("c_row_label", 4, 1), 0.0);
    assert_eq!(ctx.at("c_label_col", 4, 3), 0.0);
}

#[test]
fn block_position() {
    let sheet = fixture("pos", report);
    let ctx = Ctx::new(&sheet);
    // Title is its own block (row 2 is empty); table B3:E8 is the other.
    assert_eq!(ctx.at("c_sheet_components", 0, 1), 2.0);
    assert_eq!(ctx.at("c_comp_cells", 0, 1), 1.0);
    assert_eq!(ctx.at("c_comp_cols", 0, 1), 4.0); // merge counts toward the block's extent
    assert_eq!(ctx.at("c_comp_cells", 4, 2), 23.0);
    assert_eq!(ctx.at("c_comp_rows", 4, 2), 6.0);
    assert_eq!(ctx.at("c_comp_cols", 4, 2), 4.0);
    assert_eq!(ctx.at("c_comp_row_off", 4, 2), 2.0);
    assert_eq!(ctx.at("c_comp_col_off", 4, 2), 1.0);
    assert_eq!(ctx.at("c_comp_row_rel", 7, 2), 1.0);
    assert!((ctx.at("c_comp_col_rel", 4, 2) - 1.0 / 3.0).abs() < 1e-6);
    assert_eq!(ctx.at("c_comp_first_row", 2, 3), 1.0);
    assert_eq!(ctx.at("c_comp_last_row", 7, 3), 1.0);
    assert_eq!(ctx.at("c_comp_first_col", 3, 1), 1.0);
    assert_eq!(ctx.at("c_comp_last_col", 3, 4), 1.0);
    assert_eq!(ctx.at("c_comp_last_col", 3, 3), 0.0);
    // Gaps: rows 3..8 form a run.
    assert_eq!(ctx.at("c_gap_above", 2, 1), 1.0);
    assert_eq!(ctx.at("c_gap_above", 4, 1), 3.0);
    assert_eq!(ctx.at("c_gap_below", 7, 2), 1.0);
    assert_eq!(ctx.at("c_gap_below", 2, 1), 6.0);
    assert_eq!(ctx.at("c_gap_above", 0, 1), 1.0);
    // Used range starts at B1.
    assert_eq!(ctx.at("c_rows_from_top", 4, 2), 4.0);
    assert_eq!(ctx.at("c_cols_from_left", 4, 2), 1.0);
}

#[test]
fn multi_table_components() {
    // Table A at B2:C4, table B at E2:F5 (one empty column between), table C at B8:C9.
    let sheet = fixture("multi", |ws| {
        for r in 1..4 {
            ws.write_string(r, 1, "a").unwrap();
            ws.write_number(r, 2, r as f64).unwrap();
        }
        for r in 1..5 {
            ws.write_string(r, 4, "b").unwrap();
            ws.write_number(r, 5, r as f64).unwrap();
        }
        ws.write_string(7, 1, "c").unwrap();
        ws.write_string(8, 2, "d").unwrap(); // diagonal touch only
    });
    let ctx = Ctx::new(&sheet);
    let comps = context::components(&ctx.grid);
    let rects: Vec<Rect> = comps.iter().map(|c| c.rect).collect();
    assert_eq!(
        rects,
        vec![
            Rect { top: 1, left: 1, bottom: 3, right: 2 },
            Rect { top: 1, left: 4, bottom: 4, right: 5 },
            Rect { top: 7, left: 1, bottom: 8, right: 2 },
        ]
    );
    assert_eq!(comps.iter().map(|c| c.cells.len()).collect::<Vec<_>>(), vec![6, 8, 2]);
    assert_eq!(ctx.at("c_sheet_components", 1, 1), 3.0);
    // Row aggregates are per block: row 2 has 4 cells on the sheet but 2 per block.
    assert_eq!(ctx.at("c_row_filled", 1, 1), 4.0);
    assert_eq!(ctx.at("c_row_cells", 1, 1), 2.0);
    assert_eq!(ctx.at("c_row_first", 1, 4), 1.0);
    assert_eq!(ctx.at("c_comp_col_off", 1, 5), 1.0);
    // Both side-by-side tables have their own label column.
    assert_eq!(ctx.at("c_row_label", 2, 1), 1.0);
    assert_eq!(ctx.at("c_row_label", 2, 4), 1.0);
    // Row 5 (index 4) only exists in block B: gap below of rows 2..5 run.
    assert_eq!(ctx.at("c_gap_below", 1, 1), 4.0);

    let lab = context::label(&ctx.grid);
    assert_eq!(lab.cell_component[ctx.grid.cell_index(8, 2).unwrap()], 2);
}

#[test]
fn merged_areas_bridge_components() {
    // A1 and E1 are joined only through the merged (non-empty) B1:D1.
    let sheet = fixture("bridge", |ws| {
        ws.write_string(0, 0, "a").unwrap();
        ws.merge_range(0, 1, 0, 3, "m", &Format::new()).unwrap();
        ws.write_string(0, 4, "b").unwrap();
        // An empty merged area G3:I3 joins F2 and J4 diagonally; the empty merge M1:M5 stands alone.
        ws.write_string(1, 5, "c").unwrap();
        ws.merge_range(2, 6, 2, 8, "", &Format::new()).unwrap();
        ws.write_string(3, 9, "d").unwrap();
        ws.merge_range(0, 12, 4, 12, "", &Format::new()).unwrap();
    });
    let ctx = Ctx::new(&sheet);
    let comps = context::components(&ctx.grid);
    // Empty merge cells are dropped by the reader unless an anchor value exists; the cell-less
    // M1:M5 merge forms no component.
    assert_eq!(comps.len(), 1, "{comps:?}");
    assert_eq!(comps[0].rect, Rect { top: 0, left: 0, bottom: 3, right: 9 });
    assert_eq!(comps[0].merges.len(), 2);
    assert_eq!(ctx.at("c_sheet_components", 0, 0), 1.0);
    assert_eq!(ctx.at("c_comp_cells", 3, 9), 5.0);
    assert_eq!(ctx.at("c_gap_below", 0, 0), 5.0); // M1:M5 merge keeps rows 1..5 non-empty
}

#[test]
fn tall_merge_counts_for_gaps_and_neighbours() {
    let sheet = fixture("tall", |ws| {
        ws.merge_range(0, 0, 3, 0, "Group", &Format::new()).unwrap();
        ws.write_number(3, 1, 5.0).unwrap();
    });
    let ctx = Ctx::new(&sheet);
    assert_eq!(ctx.at("c_merge_height", 0, 0), 4.0);
    assert_eq!(ctx.at("c_n_l", 3, 1), Kind::MergedEmpty.code());
    assert_eq!(ctx.at("c_gap_above", 3, 1), 4.0);
    assert_eq!(context::components(&ctx.grid).len(), 1);
}
