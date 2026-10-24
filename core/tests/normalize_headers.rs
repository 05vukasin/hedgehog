//! Column headers that the normalized output used to lose (columns named `col_<letter>`, or a header
//! row read as data), found by the model-free accuracy scoreboard (eval/accuracy, clusters "answer
//! column has no header" and "question key: cell not in any table"). Each case is a small generated
//! sheet shaped like the public-benchmark file it came from.

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, Normalized, Options, Table};
use hh_core::open;
use rust_xlsxwriter::{ExcelDateTime, Format, Workbook, Worksheet};

fn normalize(name: &str, build: impl FnOnce(&mut Worksheet)) -> Normalized {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_headers_{name}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    build(wb.add_worksheet());
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    normalize_workbook(&book, &Options { keep_totals: true, ..Options::default() })
}

fn names(t: &Table) -> Vec<&str> {
    t.columns.iter().map(|c| c.name.as_str()).filter(|n| !n.starts_with('_')).collect()
}

fn unnamed(t: &Table) -> Vec<&str> {
    names(t).into_iter().filter(|n| n.starts_with("col_")).collect()
}

/// The table whose columns include `name`.
fn with_column<'a>(n: &'a Normalized, name: &str) -> &'a Table {
    n.tables
        .iter()
        .find(|t| t.columns.iter().any(|c| c.name == name))
        .unwrap_or_else(|| panic!("no table has a column {name:?}: {:?}", n.tables.iter().map(names).collect::<Vec<_>>()))
}

/// RealHiTBench military-table07…19: a row of dates over a `Number | %` row, separated from the body
/// by a blank row. The pair is the header (it used to be a loose title block, and every column of
/// the table was `col_<letter>`).
#[test]
fn date_header_over_a_blank_row() {
    let n = normalize("dates_blank", |ws| {
        let date = Format::new().set_num_format("mmm yyyy");
        ws.write_string(0, 0, "Table 16 - Strength by age").unwrap();
        ws.write_string(2, 3, "Glossary of terms").unwrap();
        ws.write_string(3, 3, "Return to Contents").unwrap();
        for k in 0..5u16 {
            let d = ExcelDateTime::from_ymd(2012 + k, 4, 1).unwrap();
            ws.write_datetime_with_format(4, 4 + 2 * k, &d, &date).unwrap();
            ws.write_string(5, 4 + 2 * k, "Number").unwrap();
            ws.write_string(5, 5 + 2 * k, "%").unwrap();
        }
        // row 6 blank
        let labels = ["All Services", "Under 18", "18 and 19", "20-24", "25-29", "30-34", "35-39", "40-44"];
        for (i, l) in labels.iter().enumerate() {
            let r = 7 + i as u32;
            ws.write_string(r, if i == 0 { 0 } else { 2 }, *l).unwrap();
            for k in 0..5u16 {
                ws.write_number(r, 4 + 2 * k, 1000.0 + (37 * i + 11 * k as usize) as f64).unwrap();
                ws.write_number(r, 5 + 2 * k, 1.5 + (i + k as usize) as f64).unwrap();
            }
        }
    });
    let t = with_column(&n, "2012-04-01 / Number");
    assert!(t.columns.iter().any(|c| c.name == "2016-04-01 / %"), "{:?}", names(t));
    assert_eq!(unnamed(t), ["col_A", "col_C"], "only the label columns (no header text) stay unnamed");
}

/// RealHiTBench education-table26…29: `WP ID | Teaching Group | Condition | 1 | 2 | …`, labels then
/// a column numbering, is the header (it used to be the first data row of a headerless table).
#[test]
fn labels_then_column_numbers_are_a_header() {
    let n = normalize("numbering", |ws| {
        for (c, h) in ["WP ID", "Teaching Group", "Condition"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        for k in 1..=8u16 {
            ws.write_number(0, 2 + k, k as f64).unwrap();
        }
        for i in 0..12u32 {
            ws.write_number(1 + i, 0, 92.0 + i as f64).unwrap();
            ws.write_number(1 + i, 1, 4.0).unwrap();
            ws.write_string(1 + i, 2, if i % 2 == 0 { "CAI" } else { "Lecture" }).unwrap();
            for k in 1..=8u16 {
                ws.write_number(1 + i, 2 + k, ((i * 7 + k as u32 * 3) % 5) as f64 - 1.0).unwrap();
            }
        }
    });
    let t = with_column(&n, "WP ID");
    assert_eq!(names(t)[..5], ["WP ID", "Teaching Group", "Condition", "1", "2"]);
    assert!(unnamed(t).is_empty(), "{:?}", names(t));
}

/// MiMoTable PriceAnalysisTable1: `Name | 1 | … | 12 | Annual Average`, a column numbering between
/// labels (the trailing label once made the row a total row of a headerless table).
#[test]
fn column_numbers_between_labels_are_a_header() {
    let n = normalize("numbering_tail", |ws| {
        ws.write_string(0, 1, "Price Analysis Table").unwrap();
        ws.write_string(2, 1, "Name").unwrap();
        for k in 1..=12u16 {
            ws.write_number(2, 1 + k, k as f64).unwrap();
        }
        ws.write_string(2, 14, "Annual Average").unwrap();
        for i in 0..6u32 {
            ws.write_string(3 + i, 1, format!("Material {}", i + 1)).unwrap();
            for k in 1..=13u16 {
                ws.write_number(3 + i, 1 + k, 1.0 + ((i * 13 + k as u32) % 7) as f64 / 10.0).unwrap();
            }
        }
    });
    let t = with_column(&n, "Annual Average");
    assert_eq!(names(t)[..3], ["Name", "1", "2"]);
    assert!(unnamed(t).is_empty(), "{:?}", names(t));
}

/// MiMoTable ProjectStatisticsTable1 (en-0516, en-0519): a `Key: | … | Key: | value` line over a wide
/// header row. The header row is the header (it used to be dropped: the table had no header, the
/// question's key columns `Contract Number`, `Client Unit` were in no table).
#[test]
fn key_value_line_over_the_header() {
    let n = normalize("kv_over_header", |ws| {
        ws.write_string(0, 1, "Project Statistics Table").unwrap();
        ws.write_string(2, 2, "Project Name:").unwrap();
        ws.write_string(2, 6, "Number of Clients:").unwrap();
        ws.write_number(2, 7, 7.0).unwrap();
        let header = [
            "No.", "Contract Number", "Client Name", "Client Unit", "Contact Information", "Project Name", "Quantity",
            "Proposed Duration", "Status", "Statistician",
        ];
        for (c, h) in header.iter().enumerate() {
            ws.write_string(3, 1 + c as u16, *h).unwrap();
        }
        for i in 0..7u32 {
            let r = 4 + i;
            ws.write_number(r, 1, (i + 1) as f64).unwrap();
            ws.write_string(r, 2, format!("DH102933{}", i + 1)).unwrap();
            ws.write_string(r, 3, format!("DaoKe{}", i + 1)).unwrap();
            ws.write_string(r, 4, ["ZhongHang", "Cheng Wei", "BaBaiJiu", "Xiao Lian", "DongNan", "Henan", "Wu Jiang"][i as usize]).unwrap();
            ws.write_string(r, 5, format!("180xxxx102{}", i + 1)).unwrap();
            ws.write_string(r, 6, ["Robot Trolley", "Medical Trolley", "Handle", "Machine", "Robot", "Vehicle", "Lathe"][i as usize]).unwrap();
            ws.write_string(r, 7, format!("{} sets", 10 * (i + 1))).unwrap();
            ws.write_number(r, 8, 40.0 + 9.0 * i as f64).unwrap();
            ws.write_string(r, 9, if i < 4 { "Completed" } else { "In Progress" }).unwrap();
            ws.write_string(r, 10, "Zhang Xiaomei").unwrap();
        }
    });
    let t = with_column(&n, "Contract Number");
    assert!(t.columns.iter().any(|c| c.name == "Client Unit"), "{:?}", names(t));
    assert!(unnamed(t).is_empty(), "{:?}", names(t));
    assert_eq!(t.src_rows.len(), 7, "the header row is no data row");
}

/// MiMoTable 11UndergroundEngineeringMachinery (en-0008): a four-row header with vertical merges down
/// to a unit row (`Quota | Yuan | … | kg`). The rules end the header at a row of fuel prices and
/// start a table at the unit row, but the merges hang into it; the row model calls the upper rows
/// header, so they are the top of that table's header (they used to stay a table of their own, and
/// `Machinery Name`, `Model` were `col_B`, `col_D`).
#[test]
fn header_block_with_merges_down_to_the_unit_row() {
    let n = normalize("merged_unit_row", |ws| {
        let m = Format::new();
        ws.write_string(0, 0, "Section Eleven: Underground Engineering Machinery").unwrap();
        ws.merge_range(1, 0, 5, 0, "Number", &m).unwrap();
        ws.merge_range(1, 1, 5, 2, "Machinery Name", &m).unwrap();
        ws.merge_range(1, 3, 5, 3, "Model", &m).unwrap();
        ws.merge_range(1, 4, 5, 5, "Specification Model", &m).unwrap();
        ws.merge_range(1, 6, 4, 6, "Depreciation Period", &m).unwrap();
        ws.merge_range(1, 7, 4, 7, "Shift Base Price", &m).unwrap();
        ws.merge_range(1, 8, 1, 13, "Cost Composition", &m).unwrap();
        ws.merge_range(2, 8, 3, 8, "Depreciation Cost", &m).unwrap();
        ws.merge_range(2, 9, 3, 9, "Major Repair Cost", &m).unwrap();
        ws.merge_range(2, 10, 3, 10, "Fuel and Power Cost", &m).unwrap();
        ws.merge_range(2, 11, 2, 13, "Including", &m).unwrap();
        for (k, f) in ["Gasoline", "Diesel", "Electricity"].iter().enumerate() {
            ws.write_string(3, 11 + k as u16, *f).unwrap();
        }
        ws.merge_range(4, 8, 4, 10, "Quota", &m).unwrap();
        for (k, p) in [5.65, 6.85, 0.9].iter().enumerate() {
            ws.write_number(4, 11 + k as u16, *p).unwrap();
        }
        for (k, u) in ["Quota", "Yuan", "Yuan", "Yuan", "Yuan", "kg", "kg", "kw·h"].iter().enumerate() {
            ws.write_string(5, 6 + k as u16, *u).unwrap();
        }
        let names = ["Dry Excavation Shield", "Hydraulic Cabinet", "Air Pressure Balance"];
        for i in 0..12u32 {
            let r = 6 + i;
            ws.write_string(r, 0, format!("11-{}", i + 1)).unwrap();
            ws.write_string(r, 1, names[(i / 4) as usize]).unwrap();
            ws.write_string(r, 3, "Special").unwrap();
            ws.write_string(r, 4, "Diameter (mm)").unwrap();
            ws.write_string(r, 5, format!("φ{}", 3500 + 1500 * (i % 4))).unwrap();
            ws.write_number(r, 6, 8.0 + (i / 4) as f64).unwrap();
            for c in 7..=13u16 {
                ws.write_number(r, c, 100.0 * c as f64 + 13.7 * i as f64).unwrap();
            }
        }
    });
    let t = with_column(&n, "Number");
    assert!(t.columns.iter().any(|c| c.name == "Machinery Name"), "{:?}", names(t));
    assert!(t.columns.iter().any(|c| c.name == "Model"), "{:?}", names(t));
    assert_eq!(t.src_rows.len(), 12, "{:?}", names(t));
}

/// MiMoTable FinancialProfitAnalysisTable (en-0508, en-0509): section labels merged over only two of
/// their four columns (`Main Income` over F:G of F:I). The repeated sub-headers take the nearest
/// section label to their left as parent (`Main Income / This Year`) instead of a counter (`This
/// Year (2)`), so a question's `This Year` of `Main Income` names one column.
#[test]
fn repeated_sub_headers_take_their_section_as_parent() {
    let n = normalize("partial_merge_parents", |ws| {
        let m = Format::new();
        ws.write_string(0, 1, "Profit Statistics").unwrap();
        for (g, section) in ["Main Income", "Main Cost", "Tax"].iter().enumerate() {
            let c = 1 + 4 * g as u16;
            ws.merge_range(2, c, 2, c + 1, section, &m).unwrap();
            for (k, h) in ["Month", "Last Year", "This Year", "Growth"].iter().enumerate() {
                ws.write_string(3, c + k as u16, *h).unwrap();
            }
            for i in 0..8u32 {
                ws.write_number(4 + i, c, (i + 1) as f64).unwrap();
                ws.write_number(4 + i, c + 1, 100.0 + (17 * i + 5 * g as u32) as f64).unwrap();
                ws.write_number(4 + i, c + 2, 110.0 + (19 * i + 3 * g as u32) as f64).unwrap();
                ws.write_number(4 + i, c + 3, 0.05 + 0.01 * i as f64).unwrap();
            }
        }
    });
    let t = with_column(&n, "Main Income / This Year");
    for name in ["Main Income / Month", "Main Cost / This Year", "Tax / This Year", "Tax / Growth"] {
        assert!(t.columns.iter().any(|c| c.name == name), "no {name:?} in {:?}", names(t));
    }
    assert!(!names(t).iter().any(|n| n.ends_with(')')), "{:?}", names(t));
}

/// HiTab statcan/861: a body row whose values are all StatCan's suppression mark `F` (too
/// unreliable to publish) reads like a header row over the numbers below it, and split the table
/// there (the lower part's columns were `F`, `F (2)`). The mark already stands in those columns
/// above: it is a value, not a new header.
#[test]
fn a_row_of_suppression_marks_is_no_new_header() {
    let n = normalize("suppressed_row", |ws| {
        ws.write_string(0, 0, "Table 1: Reasons for not reporting, by type of incident").unwrap();
        ws.write_string(2, 0, "Reason for not reporting").unwrap();
        ws.write_string(2, 1, "Hate crime").unwrap();
        ws.write_string(2, 2, "Not a hate crime").unwrap();
        ws.write_string(3, 1, "percent").unwrap();
        let reasons = [
            "Fear of revenge", "Lack of evidence", "Unsatisfactory service", "Tried to report", "Could have caused trouble",
            "Did not want the hassle", "Reporting would not help", "Family prevented the victim", "Incident was a private matter",
            "Crime was minor", "No one was harmed", "Offender would not be convicted", "Police would be biased",
        ];
        for (i, r) in reasons.iter().enumerate() {
            let row = 4 + i as u32;
            ws.write_string(row, 0, *r).unwrap();
            for c in 1..=2u16 {
                // `F` on a few rows, and on both columns of the "Family prevented" row.
                if i == 7 || (c == 1 && (i == 3 || i == 4)) {
                    ws.write_string(row, c, "F").unwrap();
                } else {
                    ws.write_number(row, c, (11 + 7 * i + 13 * c as usize) as f64).unwrap();
                }
            }
        }
    });
    let t = with_column(&n, "Hate crime");
    assert_eq!(t.src_rows.len(), 13, "one table: {:?}", n.tables.iter().map(names).collect::<Vec<_>>());
    assert!(!n.tables.iter().any(|t| t.columns.iter().any(|c| c.name == "F")));
}

/// DECO peter_keavey (Pipe Options): `TIME BUCKET/KEY: | REPORT REGION | RISK TYPE | 1 | 2 | …` over
/// two rows of dates (from / to). Labels then column numbers are no header of their own when a
/// header row follows: the dates still name the columns.
#[test]
fn numbered_row_over_a_date_header_keeps_the_dates() {
    let n = normalize("numbers_over_dates", |ws| {
        let date = Format::new().set_num_format("mm/dd/yy");
        ws.write_string(5, 0, "TIME BUCKET/KEY:").unwrap();
        ws.write_string(5, 2, "REPORT REGION").unwrap();
        ws.write_string(5, 3, "RISK TYPE").unwrap();
        for k in 0..6u16 {
            ws.write_number(5, 4 + k, (k + 1) as f64).unwrap();
            let d = ExcelDateTime::from_ymd(2000, 4 + k as u8, 1).unwrap();
            ws.write_datetime_with_format(6, 4 + k, &d, &date).unwrap();
            ws.write_datetime_with_format(7, 4 + k, &d, &date).unwrap();
        }
        ws.write_string(6, 0, "(Positions in Cont. Equiv.) From:").unwrap();
        ws.write_string(7, 0, "To:").unwrap();
        for i in 0..10u32 {
            let r = 9 + i;
            ws.write_string(r, 0, format!("Gas Daily {i}")).unwrap();
            ws.write_string(r, 2, "MAWEST").unwrap();
            ws.write_string(r, 3, ["G", "I", "D", "P"][(i % 4) as usize]).unwrap();
            for k in 0..6u16 {
                ws.write_number(r, 4 + k, 10.5 * (i as f64 + 1.0) - 3.25 * k as f64).unwrap();
            }
        }
    });
    let t = n.tables.iter().max_by_key(|t| t.src_rows.len()).unwrap();
    assert!(names(t).iter().any(|n| n.contains("2000-04-01")), "{:?}", names(t));
}

/// RealHiTBench science-table02, geography-table03 (OECD data explorer exports): `Reference area`
/// alone right above a `label | value` list (or above its first group label) is the header of the
/// label column, not a title (the column was `col_B`, so `Malta`, `Victoria` had no column name).
#[test]
fn a_lone_label_over_the_label_column_is_its_header() {
    let n = normalize("reference_area", |ws| {
        ws.write_string(0, 1, "Green areas - Local areas").unwrap();
        ws.write_string(1, 1, "Combined unit of measure: Square kilometres").unwrap();
        ws.write_string(3, 1, "Reference area").unwrap();
        ws.write_string(4, 1, "Australia").unwrap();
        let states = ["New South Wales", "Victoria", "Queensland", "South Australia", "Western Australia", "Tasmania"];
        for (i, s) in states.iter().enumerate() {
            ws.write_string(5 + i as u32, 1, format!("·\u{2007}\u{2007}{s}")).unwrap();
            ws.write_number(5 + i as u32, 3, 1000.0 * (i as f64 + 1.5)).unwrap();
        }
        ws.write_string(11, 1, "Austria").unwrap();
        for (i, s) in ["Burgenland", "Lower Austria", "Vienna", "Carinthia"].iter().enumerate() {
            ws.write_string(12 + i as u32, 1, format!("·\u{2007}\u{2007}{s}")).unwrap();
            ws.write_number(12 + i as u32, 3, 300.0 * (i as f64 + 2.5)).unwrap();
        }
    });
    let t = with_column(&n, "Reference area");
    assert_eq!(t.src_rows.len(), 10, "{:?}", names(t));
}

/// RealHiTBench labor-table50: a text-only table under three header rows, `2012 | 2013 | 2014`
/// over `Fitch | S&P | Moody's` (merged over their groups) over `Company | Type of Audit | Month |
/// …`. All three are the header (the table had none: `Moody's`, `Company` were in no table).
#[test]
fn parent_label_rows_over_a_text_only_table() {
    let n = normalize("text_only_parents", |ws| {
        let m = Format::new();
        for (y, year) in [2012, 2013].iter().enumerate() {
            let c0 = 1 + 9 * y as u16;
            ws.merge_range(0, c0, 0, c0 + 8, &year.to_string(), &m).unwrap();
            for (a, agency) in ["Fitch", "S&P", "Moody's"].iter().enumerate() {
                let c = c0 + 3 * a as u16;
                ws.merge_range(1, c, 1, c + 2, agency, &m).unwrap();
                for (k, h) in ["Type of Audit", "Month", "Company Rating"].iter().enumerate() {
                    ws.write_string(2, c + k as u16, *h).unwrap();
                }
            }
        }
        ws.write_string(2, 0, "Company").unwrap();
        let months = ["MARCH", "JULY", "MAY", "AUGUST", "JUNE"];
        for i in 0..6u32 {
            ws.write_string(3 + i, 0, format!("Company {}", i + 1)).unwrap();
            for c in 1..=18u16 {
                let v = match c % 3 {
                    1 => if (i + c as u32) % 4 == 0 { "-" } else { "FULL" },
                    2 => months[((i + c as u32) % 5) as usize],
                    _ => ["GOOD", "NORMAL", "BAD"][((i + c as u32) % 3) as usize],
                };
                ws.write_string(3 + i, c, v).unwrap();
            }
        }
    });
    let t = with_column(&n, "Company");
    assert!(t.columns.iter().any(|c| c.name == "2013 / Moody's / Company Rating"), "{:?}", names(t));
    assert_eq!(t.src_rows.len(), 6);
}

/// RealHiTBench labor-table19: stacked blocks, each with its label merged down its own rows
/// (`Bay 1` over A2:A13) and three header rows. A merge that ends inside its block is no header
/// span: the blocks stay tables of their own (they became one table with 28 header rows, and
/// cells were lost).
#[test]
fn a_label_merged_down_its_block_is_no_header_span() {
    let n = normalize("merged_block_labels", |ws| {
        let m = Format::new();
        for (b, bay) in ["Bay 1", "Bay 2", "Bay 3"].iter().enumerate() {
            let top = 12 * b as u32;
            ws.merge_range(top, 0, top + 11, 0, bay, &m).unwrap();
            for (k, h) in ["Customer", "Model", "Work type"].iter().enumerate() {
                ws.write_string(top + k as u32, 1, *h).unwrap();
                for c in 2..8u16 {
                    let v = [["Company A", "Company B"][b / 2], "N/A", ["Check", "Overhaul"][b / 2]][k];
                    ws.write_string(top + k as u32, c, v).unwrap();
                }
            }
            for i in 0..9u32 {
                ws.write_string(top + 3 + i, 1, format!("Data {}", i + 1)).unwrap();
                for c in 2..8u16 {
                    ws.write_number(top + 3 + i, c, (i * 7 + c as u32 * 3 + b as u32) as f64 / 2.0).unwrap();
                }
            }
        }
    });
    for t in &n.tables {
        assert!(t.header_rows <= 4, "{} header rows: {:?}", t.header_rows, names(t));
    }
    let data: usize = n.tables.iter().map(|t| t.src_rows.len()).sum();
    assert!(data >= 27, "{data} data rows: {:?}", n.tables.iter().map(names).collect::<Vec<_>>());
}

/// DECO-style supply sheet (TableSense VEnron2 `June 12 Proposal`): a label alone above rows whose
/// labels read like it (`From Anadarko Pool` over `From Bondad Station | …`) is an empty data row,
/// not the header of the label column.
#[test]
fn a_label_like_the_rows_below_is_no_header() {
    let n = normalize("sibling_label", |ws| {
        ws.write_string(0, 0, "Receipt Right Assignments").unwrap();
        ws.write_string(2, 0, "From Anadarko Pool").unwrap();
        let from = [
            "From Bondad Station", "From Bondad Mainline", "From Blanco", "From Plains", "From Keystone", "From Waha",
            "From Permian", "From Ignacio", "From Kutz", "From Chaco",
        ];
        for (i, s) in from.iter().enumerate() {
            ws.write_string(3 + i as u32, 0, *s).unwrap();
            for c in 1..=4u16 {
                ws.write_number(3 + i as u32, c, 100.0 * (i as f64 + 1.25) + 7.0 * c as f64).unwrap();
            }
        }
    });
    assert!(!n.tables.iter().any(|t| t.columns.iter().any(|c| c.name == "From Anadarko Pool")));
}

/// The title of a table under a `Key: | value` line: never the line's keys or values
/// (`Number of Clients: | 7` made it "7"), but other text on the line is (MiMoTable
/// StockSituation4: `Stock D | Code: | 656839` over the trades; en-0162 asks for Stock D).
#[test]
fn key_value_line_titles() {
    let n = normalize("kv_titles", |ws| {
        ws.write_string(0, 0, "Stock D").unwrap();
        ws.write_string(0, 1, "Code:").unwrap();
        ws.write_number(0, 2, 656839.0).unwrap();
        for (c, h) in ["Serial Number", "Date", "Direction", "Price", "Shares", "Fee", "Total Amount", "Account Cost", "Account Shares"]
            .iter()
            .enumerate()
        {
            ws.write_string(1, c as u16, *h).unwrap();
        }
        for i in 0..5u32 {
            ws.write_string(2 + i, 0, format!("0{}", i + 1)).unwrap();
            ws.write_number(2 + i, 1, 44198.0 + 9.0 * i as f64).unwrap();
            ws.write_string(2 + i, 2, if i % 2 == 0 { "Buy" } else { "Sell" }).unwrap();
            for c in 3..9u16 {
                ws.write_number(2 + i, c, 20.0 + 5.0 * i as f64 + c as f64).unwrap();
            }
        }
    });
    let t = with_column(&n, "Serial Number");
    assert_eq!(t.title.as_ref().map(|x| x.0.as_str()), Some("Stock D"));
    let n = normalize("kv_title_value", |ws| {
        ws.write_string(2, 2, "Project Name:").unwrap();
        ws.write_string(2, 6, "Number of Clients:").unwrap();
        ws.write_number(2, 7, 7.0).unwrap();
        let header = ["No.", "Contract Number", "Client Name", "Client Unit", "Contact", "Project Name", "Quantity", "Days", "Status", "Statistician"];
        for (c, h) in header.iter().enumerate() {
            ws.write_string(3, 1 + c as u16, *h).unwrap();
        }
        for i in 0..7u32 {
            ws.write_number(4 + i, 1, (i + 1) as f64).unwrap();
            for c in 2..=10u16 {
                ws.write_string(4 + i, c, format!("v{i}{c}")).unwrap();
            }
            ws.write_number(4 + i, 7, 10.0 * (i + 1) as f64).unwrap();
        }
    });
    let t = with_column(&n, "Contract Number");
    let title = t.title.as_ref().map(|x| x.0.clone()).unwrap_or_default();
    assert!(!title.ends_with(':') && title.parse::<f64>().is_err(), "title {title:?}");
}

/// HiTab statcan/2797: rows of `F` (too unreliable) also in columns that had none above. A value
/// annotation is no header label: one table, its columns named by the real header.
#[test]
fn suppression_marks_in_new_columns_are_no_header() {
    let n = normalize("suppressed_new_cols", |ws| {
        ws.write_string(0, 0, "Table 2: Victims by selected characteristics").unwrap();
        ws.write_string(2, 0, "Characteristic").unwrap();
        for (k, h) in ["Male", "Female", "Total number", "Total percent"].iter().enumerate() {
            ws.write_string(2, 1 + k as u16, *h).unwrap();
        }
        let rows: [(&str, [Option<f64>; 4]); 10] = [
            ("Army", [Some(1.2), Some(9.3), Some(430.0), Some(2.2)]),
            ("Navy", [Some(2.4), Some(6.0), Some(120.0), Some(3.6)]),
            ("Air Force", [None, None, Some(20.0), Some(1.1)]),
            ("Personnel Command", [None, Some(7.8), Some(70.0), Some(3.7)]),
            ("Other", [None, None, None, None]),
            ("Indigenous", [None, None, Some(70.0), Some(6.1)]),
            ("Non-Indigenous", [Some(1.1), Some(6.7), Some(540.0), Some(2.0)]),
            ("Junior officer", [Some(1.5), Some(8.2), Some(480.0), Some(2.6)]),
            ("Senior officer", [Some(0.9), None, Some(50.0), Some(1.3)]),
            ("Recruit", [Some(1.9), Some(6.1), Some(60.0), Some(2.9)]),
        ];
        for (i, (label, vals)) in rows.iter().enumerate() {
            let r = 3 + i as u32;
            ws.write_string(r, 0, *label).unwrap();
            for (k, v) in vals.iter().enumerate() {
                match v {
                    Some(x) => ws.write_number(r, 1 + k as u16, *x).unwrap(),
                    None => ws.write_string(r, 1 + k as u16, "F").unwrap(),
                };
            }
        }
    });
    let t = with_column(&n, "Total percent");
    assert_eq!(t.src_rows.len(), 10, "one table: {:?}", n.tables.iter().map(names).collect::<Vec<_>>());
}
