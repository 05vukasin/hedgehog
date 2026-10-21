//! Cell formats read from xlsx styles, and the `s_*` features built from them.

use hh_core::model::{Align, NumFmt};
use rust_xlsxwriter::{Format, FormatAlign, FormatBorder, Workbook};

#[test]
fn xlsx_formats_reach_cells_and_features() {
    let path = std::env::temp_dir().join(format!("hh_test_{}_styles.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let title = Format::new().set_bold().set_font_size(16);
    let header = Format::new().set_bold().set_background_color("C6E0B4").set_border_bottom(FormatBorder::Thin).set_align(FormatAlign::Center);
    let money = Format::new().set_num_format("#,##0.00");
    let pct = Format::new().set_num_format("0.0%");
    let label = Format::new().set_indent(2);
    ws.write_with_format(0, 0, "Sales 2024", &title).unwrap();
    ws.write_with_format(2, 0, "Region", &header).unwrap();
    ws.write_with_format(2, 1, "Revenue", &header).unwrap();
    ws.write_with_format(2, 2, "Share", &header).unwrap();
    for r in 3..8u32 {
        ws.write_with_format(r, 0, format!("R{r}"), &label).unwrap();
        ws.write_with_format(r, 1, 1000.0 * r as f64, &money).unwrap();
        ws.write_with_format(r, 2, 0.1 * r as f64, &pct).unwrap();
    }
    ws.write(8, 0, "plain").unwrap();
    // A styled empty cell between filled cells must not shift the others.
    ws.write_blank(8, 1, &header).unwrap();
    ws.write(8, 2, 3.0).unwrap();
    wb.save(&path).unwrap();

    let book = hh_core::reader::open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let sheet = &book.sheets[0];
    assert_eq!(sheet.formats.len(), sheet.cells.len());
    let at = |row: u32, col: u32| {
        let i = sheet.cells.iter().position(|c| (c.row, c.col) == (row, col)).unwrap();
        sheet.formats[i].unpack().unwrap()
    };
    let t = at(0, 0);
    assert!(t.bold && t.size_half_pt == 32);
    let h = at(2, 1);
    assert!(h.bold && h.fill && h.border_bottom && !h.border_top && h.align == Align::Center);
    assert_eq!(at(4, 1).num_fmt, NumFmt::Thousands);
    assert_eq!(at(4, 2).num_fmt, NumFmt::Percent);
    assert_eq!(at(4, 0).indent, 2);
    let p = at(8, 2);
    assert!(!p.bold && !p.fill && p.num_fmt == NumFmt::General && p.size_half_pt == 22);

    let grid = hh_core::grid::Grid::new(sheet);
    let f = hh_core::eval::export::sheet_features(&grid);
    let i = grid.cell_index(0, 0).unwrap();
    assert_eq!(f.value("s_known", i), Some(1.0));
    assert!(f.value("s_bold", i).unwrap() > 0.5); // bold, minus the sheet's bold share
    assert!((f.value("s_font_rel", i).unwrap() - 16.0 / 11.0).abs() < 1e-6);
    let j = grid.cell_index(2, 2).unwrap();
    assert!(f.value("s_fill", j).unwrap() > 0.5 && f.value("s_border_bottom", j).unwrap() > 0.5);
    assert_eq!(f.value("s_align", j), Some(2.0));
    // The model path computes them only when asked (a model that reads them).
    let (_, with) = hh_core::eval::export::layout_and_features(&grid, true);
    let (_, without) = hh_core::eval::export::layout_and_features(&grid, false);
    assert_eq!(with.get("s_bold"), f.get("s_bold"));
    assert!(without.get("s_bold").is_none() && without.get("l_role").is_some());
}

#[test]
fn csv_has_no_formats() {
    let path = std::env::temp_dir().join(format!("hh_test_{}_styles.csv", std::process::id()));
    std::fs::write(&path, "a,b\n1,2\n").unwrap();
    let book = hh_core::reader::open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let sheet = &book.sheets[0];
    assert!(sheet.formats.is_empty());
    let grid = hh_core::grid::Grid::new(sheet);
    let f = hh_core::eval::export::sheet_features(&grid);
    assert!(f.get("s_known").unwrap().iter().all(|&x| x == 0.0));
    assert!(f.get("s_bold").unwrap().iter().all(|&x| x == 0.0));
}
