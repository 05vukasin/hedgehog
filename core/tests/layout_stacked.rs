//! Layout v2 fixtures (generated at test time, no spreadsheets in git): stacked tables with
//! different schemas (change-point segmentation), row-label columns, far-left group labels, and
//! the long sparse "true-up" sheet that used to be over-split.

use std::path::PathBuf;

use hh_core::{a1, layout, open, Grid, Rect, Role, Sheet, SheetLayout};
use rust_xlsxwriter::{ExcelDateTime, Format, Formula, Workbook, Worksheet};

fn fixture(name: &str, build: impl FnOnce(&mut Worksheet)) -> Sheet {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_stacked_{}_{name}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    build(wb.add_worksheet());
    wb.save(&path).unwrap();
    let mut book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    book.sheets.remove(0)
}

fn run(sheet: &Sheet) -> SheetLayout {
    let layout = layout::analyze(&Grid::new(sheet));
    assert_eq!(layout.roles.len(), sheet.cells.len());
    layout
}

fn role(sheet: &Sheet, l: &SheetLayout, row: u32, col: u32) -> Role {
    let i = sheet.cells.iter().position(|c| c.row == row && c.col == col).expect("cell exists");
    l.roles[i]
}

fn rect(top: u32, left: u32, bottom: u32, right: u32) -> Rect {
    Rect { top, left, bottom, right }
}

fn rects(l: &SheetLayout) -> Vec<String> {
    l.tables.iter().map(|t| format!("{}:{}", a1(t.rect.top, t.rect.left), a1(t.rect.bottom, t.rect.right))).collect()
}

/// Tiny deterministic generator.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / (1u64 << 31) as f64
    }
}

#[derive(Clone, Copy)]
enum Col {
    Name,
    Int,
    Dec,
    Big,
    Date,
    /// Number stored as text, European style ("1.234,50 EUR").
    TextNum,
    Code,
}

fn write_rows(ws: &mut Worksheet, row: u32, col: u16, cols: &[Col], n: u32, seed: u64) {
    let mut r = Rng(seed);
    let date = Format::new().set_num_format("yyyy-mm-dd");
    let names = ["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliet"];
    for i in 0..n {
        for (j, c) in cols.iter().enumerate() {
            let (rr, cc) = (row + i, col + j as u16);
            match c {
                Col::Name => ws.write_string(rr, cc, format!("{} {}", names[(r.next() * 10.0) as usize % 10], i)),
                Col::Int => ws.write_number(rr, cc, (r.next() * 90.0).floor() + 1.0),
                Col::Dec => ws.write_number(rr, cc, (r.next() * 1000.0).round() / 100.0),
                Col::Big => ws.write_number(rr, cc, (r.next() * 1e7).round() / 100.0),
                Col::Date => {
                    let d = ExcelDateTime::from_ymd(2023, 1 + (i % 12) as u8, 1 + (i % 27) as u8).unwrap();
                    ws.write_datetime_with_format(rr, cc, &d, &date)
                }
                Col::TextNum => {
                    let v = (r.next() * 1e6).round() / 100.0;
                    let s = format!("{:.2}", v).replace('.', ",");
                    ws.write_string(rr, cc, format!("{s} EUR"))
                }
                Col::Code => ws.write_string(rr, cc, format!("X-{}", (r.next() * 900.0) as u32 + 100)),
            }
            .unwrap();
        }
    }
}

fn header(ws: &mut Worksheet, row: u32, col: u16, names: &[&str]) {
    for (j, h) in names.iter().enumerate() {
        ws.write_string(row, col + j as u16, *h).unwrap();
    }
}

const T1: &[Col] = &[Col::Name, Col::Int, Col::Dec, Col::Date];

/// Table 1: header row 0 + 30 rows A1:D31.
fn first(ws: &mut Worksheet) {
    header(ws, 0, 0, &["Trader", "Deals", "Price", "Trade date"]);
    write_rows(ws, 1, 0, T1, 30, 1);
}

fn assert_two(l: &SheetLayout, a: Rect, b: Rect) {
    assert_eq!(l.tables.len(), 2, "{:?}", rects(l));
    assert_eq!(l.tables[0].rect, a, "{:?}", rects(l));
    assert_eq!(l.tables[1].rect, b, "{:?}", rects(l));
}

/// (a) No blank row and no second header: the schema changes from row 32.
#[test]
fn stack_a_no_gap_no_header() {
    let s = fixture("a", |ws| {
        first(ws);
        write_rows(ws, 31, 0, &[Col::Date, Col::Code, Col::Big, Col::Name], 25, 2);
    });
    let l = run(&s);
    assert_two(&l, rect(0, 0, 30, 3), rect(31, 0, 55, 3));
}

/// (b) Second table wider, with its own header, no blank row.
#[test]
fn stack_b_wider() {
    let s = fixture("b", |ws| {
        first(ws);
        header(ws, 31, 0, &["Region", "Jan", "Feb", "Mar", "Apr", "May"]);
        write_rows(ws, 32, 0, &[Col::Name, Col::Big, Col::Big, Col::Big, Col::Big, Col::Big], 20, 3);
    });
    let l = run(&s);
    assert_two(&l, rect(0, 0, 30, 3), rect(31, 0, 51, 5));
    assert_eq!(role(&s, &l, 31, 3), Role::Header);
}

/// (c) Second table narrower, no header, no blank row.
#[test]
fn stack_c_narrower() {
    let s = fixture("c", |ws| {
        first(ws);
        write_rows(ws, 31, 0, &[Col::Code, Col::Big], 20, 4);
    });
    let l = run(&s);
    assert_two(&l, rect(0, 0, 30, 3), rect(31, 0, 50, 1));
}

/// (d) Second table shifted right, with header.
#[test]
fn stack_d_shifted() {
    let s = fixture("d", |ws| {
        first(ws);
        header(ws, 31, 2, &["Book", "Volume", "Value", "Fee"]);
        write_rows(ws, 32, 2, &[Col::Code, Col::Big, Col::Big, Col::Dec], 20, 5);
    });
    let l = run(&s);
    assert_two(&l, rect(0, 0, 30, 3), rect(31, 2, 51, 5));
}

/// (e) Second header looks like data (years), no blank row.
#[test]
fn stack_e_year_header() {
    let s = fixture("e", |ws| {
        first(ws);
        ws.write_string(31, 0, "Region").unwrap();
        for (j, y) in [2023.0, 2024.0, 2025.0].iter().enumerate() {
            ws.write_number(31, 1 + j as u16, *y).unwrap();
        }
        write_rows(ws, 32, 0, &[Col::Name, Col::Big, Col::Big, Col::Big], 20, 6);
    });
    let l = run(&s);
    assert_two(&l, rect(0, 0, 30, 3), rect(31, 0, 51, 3));
    assert_eq!(role(&s, &l, 31, 2), Role::Header);
}

/// Repeated identical header rows (page breaks) stay one table.
#[test]
fn page_break_headers_do_not_split() {
    let s = fixture("pagebreak", |ws| {
        let h = ["Trader", "Deals", "Price", "Trade date"];
        let mut row = 0;
        for p in 0..3 {
            header(ws, row, 0, &h);
            write_rows(ws, row + 1, 0, T1, 25, 10 + p);
            row += 26;
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", rects(&l));
    assert_eq!(l.tables[0].rect, rect(0, 0, 77, 3));
    assert_eq!(role(&s, &l, 26, 1), Role::Header);
}

/// Numbers stored as text ("1.234,50 EUR") in the second half of a column are still numbers.
#[test]
fn text_numbers_do_not_split() {
    let s = fixture("textnum", |ws| {
        header(ws, 0, 0, &["Trader", "Deals", "Amount", "Trade date"]);
        write_rows(ws, 1, 0, &[Col::Name, Col::Int, Col::Big, Col::Date], 25, 20);
        write_rows(ws, 26, 0, &[Col::Name, Col::Int, Col::TextNum, Col::Date], 25, 21);
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", rects(&l));
    assert_eq!(l.tables[0].rect, rect(0, 0, 50, 3));
}

/// A long table whose optional column fills in only later must not split.
#[test]
fn optional_column_does_not_split() {
    let s = fixture("optional", |ws| {
        header(ws, 0, 0, &["Trader", "Deals", "Price", "Actual", "Trade date"]);
        write_rows(ws, 1, 0, &[Col::Name, Col::Int, Col::Dec], 40, 30);
        let mut r = Rng(31);
        let date = Format::new().set_num_format("yyyy-mm-dd");
        for i in 0..40u32 {
            if i >= 25 {
                ws.write_number(1 + i, 3, (r.next() * 100.0).round()).unwrap();
            }
            let d = ExcelDateTime::from_ymd(2023, 1 + (i % 12) as u8, 1).unwrap();
            ws.write_datetime_with_format(1 + i, 4, &d, &date).unwrap();
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", rects(&l));
}

/// Row labels in E ("Gas Annuity =") beside numbers in G:L, with a blank spacer column F,
/// belong to the table.
#[test]
fn row_label_column_with_spacer() {
    let s = fixture("labels", |ws| {
        header(ws, 0, 6, &["Day Ahead", "ISO", "Services", "Total", "Payments", "Net"]);
        let labels = ["Gas Annuity =", "Services Desk Annuity =", "Power Annuity:", "Other ="];
        let mut r = Rng(40);
        for i in 0..12u32 {
            ws.write_string(2 + i, 4, labels[i as usize % 4]).unwrap();
            for c in 6..12u16 {
                ws.write_number(2 + i, c, (r.next() * 1e6).round() / 100.0).unwrap();
            }
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", rects(&l));
    assert_eq!(l.tables[0].rect, rect(0, 4, 13, 11), "{:?}", rects(&l));
    assert_eq!(role(&s, &l, 3, 4), Role::Data);
}

/// Month labels alone on a row in column A, data in E:H below: group headers inside the table.
#[test]
fn far_left_group_labels() {
    let s = fixture("groups", |ws| {
        header(ws, 0, 4, &["Item", "Volume", "Price", "Value"]);
        let mut r = Rng(50);
        let mut row = 2;
        for m in ["July '00", "Aug", "Sep", "Oct"] {
            ws.write_string(row, 0, m).unwrap();
            row += 1;
            for i in 0..3 {
                ws.write_string(row, 4, format!("Desk {i}")).unwrap();
                for c in 5..8u16 {
                    ws.write_number(row, c, (r.next() * 1e5).round() / 100.0).unwrap();
                }
                row += 1;
            }
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", rects(&l));
    assert_eq!(l.tables[0].rect.left, 0, "{:?}", rects(&l));
    assert_eq!(role(&s, &l, 2, 0), Role::GroupHeader);
    assert_eq!(role(&s, &l, 6, 0), Role::GroupHeader);
    assert_eq!(role(&s, &l, 3, 5), Role::Data);
}

/// Rebuild of `chris_stokley__3980__Enron True-Upcorrect.xlsx` Sheet1 (values only, no real
/// data): one long sparse table G3:L54 with month groups in A, row labels in E, task notes in A,
/// subtotal formulas and a grand-total row.
fn true_up(ws: &mut Worksheet) {
    ws.write_string(0, 0, "Harbor True-Up Action Items").unwrap();
    header(ws, 2, 6, &["Day Ahead less Gas", "ISO", "Services Desk Profit", "Total", "Payments", "Net"]);
    let mut r = Rng(60);
    let mut nums = |ws: &mut Worksheet, row: u32| {
        for c in 6..12u16 {
            ws.write_number(row, c, (r.next() * 1e8).round() / 100.0).unwrap();
        }
    };
    let sub = |ws: &mut Worksheet, row: u32| {
        let f = format!("=SUM(G{}:G{})", row - 1, row);
        ws.write_formula(row, 6, Formula::new(f).set_result("1349839.5")).unwrap();
    };
    let labels = |ws: &mut Worksheet, row: u32| {
        ws.write_string(row, 4, "Gas Annuity =").unwrap();
        ws.write_number(row, 6, 1336995.67).unwrap();
        ws.write_string(row + 1, 4, "Services Desk Annuity =").unwrap();
        ws.write_number(row + 1, 6, 12843.83).unwrap();
    };
    // July: label row, data row, two labelled rows, subtotal.
    ws.write_string(4, 0, "July '00").unwrap();
    nums(ws, 5);
    labels(ws, 6);
    sub(ws, 8);
    // Aug
    ws.write_string(9, 0, "Aug").unwrap();
    ws.write_string(10, 0, "1.  Enter annuity deal for services desk").unwrap();
    nums(ws, 10);
    labels(ws, 11);
    sub(ws, 13);
    // Sep
    ws.write_string(14, 0, "Sep").unwrap();
    ws.write_string(15, 0, "1  Wait on ISO completion").unwrap();
    nums(ws, 15);
    ws.write_string(16, 0, "    ISO deal").unwrap();
    labels(ws, 16);
    ws.write_string(17, 0, "2.  Recalc Services Profit").unwrap();
    ws.write_string(18, 0, "2.  Reverse annuity in October").unwrap();
    sub(ws, 18);
    ws.write_string(19, 0, "3.  Enter annuity deal").unwrap();
    // Oct .. Feb: month label on the data row or just above, then task notes.
    let mut row = 22;
    for (k, m) in ["Oct", "Nov", "Dec", "Jan", "Feb"].iter().enumerate() {
        ws.write_string(row, 0, *m).unwrap();
        let data = if k == 2 { row + 1 } else { row };
        nums(ws, data);
        for t in 0..4 {
            let rr = row + 1 + t;
            ws.write_string(rr, 0, format!("{}.  Pay Day Ahead Amount step {t}", t + 1)).unwrap();
        }
        row += 6 + (k as u32 % 2);
    }
    for (c, col) in ["G", "H", "I", "J", "K", "L"].iter().enumerate() {
        let f = format!("=SUM({col}6:{col}52)");
        ws.write_formula(53, 6 + c as u16, Formula::new(f).set_result("100")).unwrap();
    }
}

#[test]
fn true_up_is_one_table() {
    let s = fixture("trueup", true_up);
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", rects(&l));
    let t = &l.tables[0];
    assert_eq!(t.rect.left, 0, "{:?}", rects(&l));
    assert_eq!(t.rect.right, 11);
    assert_eq!(t.rect.bottom, 53, "{:?}", rects(&l));
    assert!(t.rect.top <= 2, "{:?}", rects(&l));
    assert_eq!(role(&s, &l, 2, 6), Role::Header);
    assert_eq!(role(&s, &l, 4, 0), Role::GroupHeader);
    assert_eq!(role(&s, &l, 9, 0), Role::GroupHeader);
    assert_eq!(role(&s, &l, 6, 4), Role::Data);
    assert_eq!(role(&s, &l, 53, 6), Role::Total);
}
