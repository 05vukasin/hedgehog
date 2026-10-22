//! Layout rules on fixtures generated at test time (no spreadsheets in git).

use std::path::PathBuf;

use hh_core::{layout, open, Grid, Rect, Role, Sheet, SheetLayout};
use rust_xlsxwriter::{Format, Workbook, Worksheet};

fn fixture(name: &str, build: impl FnOnce(&mut Worksheet)) -> Sheet {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_layout_{}_{name}.xlsx", std::process::id()));
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

/// Header row at `row` (texts), then `n` data rows: label + numbers.
fn table(ws: &mut Worksheet, row: u32, col: u16, headers: &[&str], n: u32, label: &str) {
    for (j, h) in headers.iter().enumerate() {
        ws.write_string(row, col + j as u16, *h).unwrap();
    }
    for i in 0..n {
        ws.write_string(row + 1 + i, col, format!("{label} {i}")).unwrap();
        for j in 1..headers.len() {
            ws.write_number(row + 1 + i, col + j as u16, (i * 10 + j as u32) as f64 + 0.5).unwrap();
        }
    }
}

#[test]
fn table_at_a1() {
    let s = fixture("a1", |ws| table(ws, 0, 0, &["Name", "Q1", "Q2"], 4, "Item"));
    let l = run(&s);
    assert_eq!(l.tables.len(), 1);
    assert_eq!(l.tables[0].rect, rect(0, 0, 4, 2));
    assert_eq!(l.tables[0].header_rows, 1);
    assert!(l.tables[0].confidence > 0.8, "{}", l.tables[0].confidence);
    assert_eq!(role(&s, &l, 0, 1), Role::Header);
    assert_eq!(role(&s, &l, 3, 0), Role::Data);
    assert_eq!(role(&s, &l, 3, 2), Role::Data);
}

#[test]
fn offset_table_with_title_and_notes() {
    let s = fixture("offset", |ws| {
        ws.write_string(1, 2, "Sales report 2001").unwrap(); // title, separated by a blank row
        ws.write_string(2, 2, "in thousands USD").unwrap();
        table(ws, 4, 2, &["Region", "Jan", "Feb", "Mar"], 5, "Region");
        ws.write_string(10, 2, "Source: internal ledger").unwrap(); // note directly below
        ws.write_string(12, 2, "* preliminary figures").unwrap(); // note after a blank row
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1);
    assert_eq!(l.tables[0].rect, rect(4, 2, 9, 5));
    assert_eq!(l.tables[0].header_rows, 1);
    assert_eq!(role(&s, &l, 1, 2), Role::Title);
    assert_eq!(role(&s, &l, 2, 2), Role::Title);
    assert_eq!(role(&s, &l, 4, 3), Role::Header);
    assert_eq!(role(&s, &l, 10, 2), Role::Note);
    assert_eq!(role(&s, &l, 12, 2), Role::Note);
}

#[test]
fn title_directly_above_header() {
    let s = fixture("title_adjacent", |ws| {
        ws.merge_range(0, 0, 0, 3, "Quarterly volumes", &Format::new()).unwrap();
        table(ws, 1, 0, &["Desk", "Q1", "Q2", "Q3"], 3, "Desk");
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1);
    // DECO convention: a title touching the table is inside its range (header_rows counts from it).
    assert_eq!(l.tables[0].rect, rect(0, 0, 4, 3));
    assert_eq!(role(&s, &l, 0, 0), Role::Title);
    assert_eq!(role(&s, &l, 1, 0), Role::Header);
}

#[test]
fn side_by_side_tables() {
    let s = fixture("side", |ws| {
        table(ws, 0, 0, &["Name", "Vol", "Price"], 4, "A");
        table(ws, 0, 4, &["Name", "Vol", "Price"], 6, "B");
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 2, "{:?}", l.tables);
    assert_eq!(l.tables[0].rect, rect(0, 0, 4, 2));
    assert_eq!(l.tables[1].rect, rect(0, 4, 6, 6));
}

#[test]
fn stacked_tables_with_own_headers_split() {
    let s = fixture("stacked", |ws| {
        table(ws, 0, 0, &["Name", "Vol", "Price"], 4, "A");
        table(ws, 6, 0, &["Trader", "Deals", "PnL"], 3, "T"); // one blank row (5)
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 2, "{:?}", l.tables);
    assert_eq!(l.tables[0].rect, rect(0, 0, 4, 2));
    assert_eq!(l.tables[1].rect, rect(6, 0, 9, 2));
    assert_eq!(role(&s, &l, 6, 1), Role::Header);
}

#[test]
fn stacked_tables_without_blank_row_split() {
    let s = fixture("stacked_tight", |ws| {
        table(ws, 0, 0, &["Name", "Vol", "Price"], 4, "A");
        ws.write_string(5, 0, "Second book").unwrap();
        table(ws, 6, 0, &["Trader", "Deals", "PnL"], 3, "T");
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 2, "{:?}", l.tables);
    assert_eq!(l.tables[0].rect, rect(0, 0, 4, 2));
    assert_eq!(l.tables[1].rect, rect(5, 0, 9, 2));
    assert_eq!(role(&s, &l, 5, 0), Role::Title);
}

#[test]
fn blank_spacer_row_does_not_split() {
    let s = fixture("spacer", |ws| {
        table(ws, 0, 0, &["Name", "Vol", "Price"], 3, "A");
        for i in 0..3u32 {
            ws.write_string(5 + i, 0, format!("B {i}")).unwrap();
            ws.write_number(5 + i, 1, 7.0 + i as f64).unwrap();
            ws.write_number(5 + i, 2, 1.5).unwrap();
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", l.tables);
    assert_eq!(l.tables[0].rect, rect(0, 0, 7, 2));
    assert_eq!(role(&s, &l, 6, 1), Role::Data);
}

#[test]
fn blank_spacer_column_does_not_split() {
    let s = fixture("spacer_col", |ws| {
        table(ws, 0, 0, &["Name", "2000", "2001"], 4, "A");
        ws.write_string(0, 4, "Change").unwrap();
        for i in 0..4u32 {
            ws.write_number(1 + i, 4, 0.1 * i as f64 + 0.05).unwrap();
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", l.tables);
    assert_eq!(l.tables[0].rect, rect(0, 0, 4, 4));
}

#[test]
fn two_row_merged_header() {
    let s = fixture("hdr2", |ws| {
        let f = Format::new();
        ws.merge_range(0, 0, 1, 0, "Region", &f).unwrap();
        ws.merge_range(0, 1, 0, 2, "Revenue", &f).unwrap();
        ws.merge_range(0, 3, 0, 4, "Cost", &f).unwrap();
        for (j, y) in ["2000", "2001", "2000", "2001"].iter().enumerate() {
            ws.write_string(1, 1 + j as u16, *y).unwrap();
        }
        for i in 0..5u32 {
            ws.write_string(2 + i, 0, format!("R{i}")).unwrap();
            for j in 1..5u16 {
                ws.write_number(2 + i, j, 100.0 * i as f64 + j as f64 + 0.25).unwrap();
            }
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", l.tables);
    assert_eq!(l.tables[0].rect, rect(0, 0, 6, 4));
    assert_eq!(l.tables[0].header_rows, 2);
    assert_eq!(role(&s, &l, 0, 1), Role::Header);
    assert_eq!(role(&s, &l, 1, 2), Role::Header);
    assert_eq!(role(&s, &l, 2, 1), Role::Data);
}

#[test]
fn numeric_year_header() {
    let s = fixture("years", |ws| {
        for (j, y) in [2000.0, 2001.0, 2002.0].iter().enumerate() {
            ws.write_number(0, 1 + j as u16, *y).unwrap();
        }
        for i in 0..4u32 {
            ws.write_string(1 + i, 0, format!("Line {i}")).unwrap();
            for j in 1..4u16 {
                ws.write_number(1 + i, j, 3.5 * (i + 1) as f64 * j as f64).unwrap();
            }
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1);
    assert_eq!(l.tables[0].header_rows, 1);
    assert_eq!(role(&s, &l, 0, 2), Role::Header);
}

#[test]
fn subtotals_and_grand_total() {
    let s = fixture("totals", |ws| {
        ws.write_string(0, 0, "Desk").unwrap();
        ws.write_string(0, 1, "Volume").unwrap();
        ws.write_string(0, 2, "Margin").unwrap();
        // group A rows 1-3, subtotal 4 (keyword-less, SUM formulas); group B rows 5-6, subtotal 7
        for (r, name) in [(1, "a1"), (2, "a2"), (3, "a3"), (5, "b1"), (6, "b2")] {
            ws.write_string(r, 0, name).unwrap();
            ws.write_number(r, 1, 10.0 + r as f64).unwrap();
            ws.write_number(r, 2, 1.5 * r as f64).unwrap();
        }
        ws.write_formula(4, 1, "=SUM(B2:B4)").unwrap();
        ws.write_formula(4, 2, "=SUM(C2:C4)").unwrap();
        ws.write_string(7, 0, "Subtotal B").unwrap();
        ws.write_formula(7, 1, "=SUM(B6:B7)").unwrap();
        ws.write_formula(7, 2, "=SUM(C6:C7)").unwrap();
        ws.write_string(8, 0, "Grand Total").unwrap();
        ws.write_formula(8, 1, "=B5+B8").unwrap();
        ws.write_formula(8, 2, "=C5+C8").unwrap();
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", l.tables);
    assert_eq!(l.tables[0].rect, rect(0, 0, 8, 2));
    assert_eq!(role(&s, &l, 4, 1), Role::Total);
    assert_eq!(role(&s, &l, 7, 0), Role::Total);
    assert_eq!(role(&s, &l, 7, 2), Role::Total);
    assert_eq!(role(&s, &l, 8, 0), Role::Total);
    assert_eq!(role(&s, &l, 8, 1), Role::Total);
    assert_eq!(role(&s, &l, 5, 1), Role::Data);
}

#[test]
fn serbian_total_keywords() {
    let s = fixture("ukupno", |ws| {
        table(ws, 0, 0, &["Grad", "Iznos"], 3, "Grad");
        ws.write_string(4, 0, "Ukupno").unwrap();
        ws.write_number(4, 1, 99.0).unwrap();
        ws.write_string(5, 0, "УКУПНО").unwrap();
        ws.write_number(5, 1, 99.0).unwrap();
    });
    let l = run(&s);
    assert_eq!(role(&s, &l, 4, 0), Role::Total);
    assert_eq!(role(&s, &l, 5, 1), Role::Total);
    assert_eq!(role(&s, &l, 3, 1), Role::Data);
}

#[test]
fn group_header_rows() {
    let s = fixture("groups", |ws| {
        ws.write_string(0, 0, "Item").unwrap();
        ws.write_string(0, 1, "Qty").unwrap();
        ws.write_string(0, 2, "Price").unwrap();
        ws.write_string(1, 0, "Region: North").unwrap();
        for r in 2..5u32 {
            ws.write_string(r, 0, format!("n{r}")).unwrap();
            ws.write_number(r, 1, r as f64).unwrap();
            ws.write_number(r, 2, r as f64 * 1.5).unwrap();
        }
        ws.merge_range(5, 0, 5, 2, "Region: South", &Format::new()).unwrap();
        for r in 6..9u32 {
            ws.write_string(r, 0, format!("s{r}")).unwrap();
            ws.write_number(r, 1, r as f64).unwrap();
            ws.write_number(r, 2, r as f64 * 1.5).unwrap();
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", l.tables);
    assert_eq!(l.tables[0].header_rows, 1);
    assert_eq!(l.tables[0].rect, rect(0, 0, 8, 2));
    assert_eq!(role(&s, &l, 1, 0), Role::GroupHeader);
    assert_eq!(role(&s, &l, 5, 0), Role::GroupHeader);
    assert_eq!(role(&s, &l, 6, 1), Role::Data);
}

#[test]
fn sparse_form_has_no_table() {
    let s = fixture("form", |ws| {
        ws.write_string(0, 0, "INVOICE").unwrap();
        ws.write_string(2, 0, "Customer:").unwrap();
        ws.write_string(2, 1, "Enron North America").unwrap();
        ws.write_string(3, 0, "Date:").unwrap();
        ws.write_string(3, 1, "2001-05-01").unwrap();
        ws.write_string(4, 0, "Amount:").unwrap();
        ws.write_number(4, 1, 1200.0).unwrap();
        ws.write_string(7, 3, "Approved by").unwrap();
        ws.write_string(9, 5, "Signature").unwrap();
    });
    let l = run(&s);
    assert!(l.tables.is_empty(), "{:?}", l.tables);
    assert!(l.roles.iter().all(|r| *r != Role::Data && *r != Role::Header));
}

#[test]
fn empty_sheet() {
    let s = Sheet::default();
    let l = run(&s);
    assert!(l.tables.is_empty());
}

#[test]
fn text_only_table_gets_header() {
    let s = fixture("text_table", |ws| {
        for (j, h) in ["Name", "City", "Role"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        for i in 1..5u32 {
            for j in 0..3u16 {
                ws.write_string(i, j, format!("v{i}{j}")).unwrap();
            }
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1);
    assert_eq!(l.tables[0].header_rows, 1);
}

#[test]
fn column_numbering_row_is_part_of_the_header() {
    // Destatis: "Jahr | BIP | …" then "1 2 3 4" under the header, then the years.
    let s = fixture("colnum", |ws| {
        for (j, h) in ["Jahr", "BIP", "BNE", "Saldo", "Netto"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        for j in 1..5u16 {
            ws.write_number(1, j, j as f64).unwrap();
        }
        for i in 0..6u32 {
            ws.write_number(2 + i, 0, 1991.0 + i as f64).unwrap();
            for j in 1..5u16 {
                ws.write_number(2 + i, j, 1500.25 + (i * 7 + j as u32) as f64).unwrap();
            }
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1);
    assert_eq!((l.tables[0].header_top, l.tables[0].header_rows), (0, 2));
    assert_eq!(role(&s, &l, 1, 2), Role::Header);
    assert_eq!(role(&s, &l, 2, 2), Role::Data);
}

#[test]
fn block_labels_in_a_second_column_are_group_keys() {
    // Henry Hub flows: "Henry Hub" once in A, "Receipts HH" / "Deliveries HH" once per block in B,
    // a pipeline name on every row in C.
    let s = fixture("blockkeys", |ws| {
        for (j, h) in ["Hub", "Flow", "Pipeline", "Capacity", "MTD"].iter().enumerate() {
            ws.write_string(0, j as u16, *h).unwrap();
        }
        ws.write_string(1, 0, "Henry Hub").unwrap();
        let mut row = 1u32;
        for (block, label) in ["Receipts HH", "Deliveries HH"].iter().enumerate() {
            ws.write_string(row, 1, *label).unwrap();
            for (k, p) in ["ACADIAN", "BRIDGELINE", "COLUMBIA GULF", "NGPL", "SONAT"].iter().enumerate() {
                ws.write_string(row, 2, *p).unwrap();
                ws.write_number(row, 3, 1000.0 * (k + 1) as f64).unwrap();
                ws.write_number(row, 4, 17.5 * (k + block + 1) as f64).unwrap();
                row += 1;
            }
            ws.write_string(row, 2, "Total").unwrap();
            ws.write_number(row, 3, 15000.0).unwrap();
            ws.write_number(row, 4, 300.0).unwrap();
            row += 1;
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", l.tables);
    assert_eq!(role(&s, &l, 1, 1), Role::GroupHeader);
    assert_eq!(role(&s, &l, 7, 1), Role::GroupHeader);
    assert_eq!(role(&s, &l, 2, 2), Role::Data);
}

#[test]
fn statistics_database_export_with_one_data_row() {
    // PX-Web export (SCB, SURS; B53): title, parent labels over period codes, one data row, a
    // blank row, then footnotes, sources and contacts in column A.
    let s = fixture("pxweb", |ws| {
        ws.write_string(0, 0, "Industrial production index by quarter").unwrap();
        ws.write_string(2, 0, "Calendar adjusted").unwrap();
        ws.write_string(2, 4, "Trend").unwrap();
        for (j, p) in ["2016K3", "2016K4", "2017K1", "2017K2", "2016K3", "2016K4", "2017K1", "2017K2"].iter().enumerate() {
            ws.write_string(3, j as u16, *p).unwrap();
        }
        for j in 0..8u16 {
            ws.write_number(4, j, 97.0 + j as f64 * 1.3).unwrap();
        }
        ws.write_string(6, 0, "2016-12-01: revised figures for the whole series").unwrap();
        ws.write_string(7, 0, "Source:").unwrap();
        ws.write_string(8, 0, "Statistics Sweden").unwrap();
        ws.write_string(9, 0, "+46 010-479 50 00").unwrap();
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", l.tables);
    assert_eq!(l.tables[0].rect, rect(2, 0, 4, 7));
    assert_eq!((l.tables[0].header_top, l.tables[0].header_rows), (2, 2));
    assert_eq!(role(&s, &l, 3, 0), Role::Header);
    assert_eq!(role(&s, &l, 4, 3), Role::Data);
    assert_eq!(role(&s, &l, 6, 0), Role::Note);
    assert_eq!(role(&s, &l, 8, 0), Role::Note);
}

#[test]
fn two_column_summary_beside_a_table_stays_in_a_table() {
    // gstorey Totals_xx (B56): a header-less `TOTAL $$` label/value block in A:B next to a table
    // in D:G on the same rows; it must not fall out to Notes.
    let s = fixture("summary_beside", |ws| {
        ws.write_string(0, 0, "TOTAL $$").unwrap();
        ws.write_number(0, 1, -481355295.0).unwrap();
        for (j, h) in ["Positions", "Nov", "Dec", "Total"].iter().enumerate() {
            ws.write_string(0, 3 + j as u16, *h).unwrap();
        }
        for (i, name) in ["ngi socal", "rocky", "san juan", "waha", "permian"].iter().enumerate() {
            let r = 1 + i as u32;
            ws.write_string(r, 0, *name).unwrap();
            ws.write_number(r, 1, 1000.0 * (i + 1) as f64).unwrap();
            ws.write_string(r, 3, *name).unwrap();
            for j in 0..3u16 {
                ws.write_number(r, 4 + j, (i as f64 + 1.0) * (j as f64 + 2.5)).unwrap();
            }
        }
    });
    let l = run(&s);
    assert!(l.tables.iter().any(|t| t.rect.left == 0 && t.rect.bottom == 5), "{:?}", l.tables);
    assert_eq!(role(&s, &l, 2, 0), Role::Data);
    assert_eq!(role(&s, &l, 2, 1), Role::Data);
}

/// ABS data cube (EQ08, B90): the agency merged over A:L, the title merged over the whole row
/// (`A2:XFD2`), the release line merged over A:L with the copyright in N, then a six-column
/// header and records. The lines are titles, not parent labels over the header (they reach past
/// its columns); as parents they made the table 16,384 columns wide.
#[test]
fn full_row_merged_title_lines_stay_titles() {
    let s = fixture("full_row_merge", |ws| {
        let f = Format::new();
        ws.merge_range(0, 0, 0, 11, "Australian Bureau of Statistics", &f).unwrap();
        ws.merge_range(1, 0, 1, 16_383, "6291.0.55.001 - EQ08 - Employed persons by Occupation, Sex, State", &f).unwrap();
        ws.merge_range(2, 0, 2, 11, "Released at 11.30 am (Canberra time) 23 December 2021", &f).unwrap();
        ws.write_string(2, 13, "© Commonwealth of Australia 2021").unwrap();
        for (j, h) in ["Midquarter month", "Sex", "State", "Occupation", "Employed total ('000)", "Hours ('000)"].iter().enumerate() {
            ws.write_string(3, j as u16, *h).unwrap();
        }
        for i in 0..30u32 {
            let r = 4 + i;
            ws.write_number(r, 0, 31_000.0 + 91.0 * (i / 6) as f64).unwrap();
            ws.write_string(r, 1, if i % 2 == 0 { "Males" } else { "Females" }).unwrap();
            ws.write_string(r, 2, ["New South Wales", "Victoria", "Queensland"][(i % 3) as usize]).unwrap();
            ws.write_string(r, 3, format!("Occupation {}", i % 5)).unwrap();
            ws.write_number(r, 4, 1.5 + i as f64 * 0.37).unwrap();
            ws.write_number(r, 5, 20.25 + i as f64 * 1.13).unwrap();
        }
    });
    let l = run(&s);
    assert_eq!(l.tables.len(), 1, "{:?}", l.tables);
    let t = &l.tables[0];
    assert_eq!((t.header_top, t.header_rows), (3, 1), "{t:?}");
    assert_eq!(role(&s, &l, 3, 0), Role::Header);
    assert_eq!(role(&s, &l, 5, 4), Role::Data);
}
