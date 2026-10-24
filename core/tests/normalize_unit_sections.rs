//! Unit rows inside a table body (HiTab, StatCan): a row whose only content is a unit in the value
//! columns (`mean`, `number`, `percentage points`, `dollars`) heads the rows below it - their values
//! are in that unit, not in the unit the column header names. It becomes a section (a group label of
//! the rows below), never a data row with the unit as a value, and never a repeated header.

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, ColRole, Options, Out, Table};
use hh_core::open;
use rust_xlsxwriter::{Workbook, Worksheet};

fn table(name: &str, build: impl FnOnce(&mut Worksheet)) -> Table {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_unit_sections_{name}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    build(wb.add_worksheet());
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let n = normalize_workbook(&book, &Options { keep_totals: true, ..Options::default() });
    n.tables.into_iter().next().expect("a table")
}

fn text(v: &Out) -> String {
    match v {
        Out::Text(s) => s.clone(),
        Out::Null => String::new(),
        v => format!("{v:?}"),
    }
}

/// Every text label of the row whose label (in any text column) is `label`: group, parent and label columns.
fn labels(t: &Table, label: &str) -> Vec<String> {
    let texts: Vec<usize> = (0..t.columns.len())
        .filter(|&c| t.columns[c].role == ColRole::Group || t.columns[c].values.iter().any(|v| matches!(v, Out::Text(_))))
        .collect();
    let r = (0..t.rows())
        .find(|&r| texts.iter().any(|&c| text(&t.columns[c].values[r]) == label))
        .unwrap_or_else(|| panic!("no row {label}: {:?}", t.columns.iter().map(|c| &c.name).collect::<Vec<_>>()));
    texts.iter().filter(|&&c| t.columns[c].role != ColRole::RowType).map(|&c| text(&t.columns[c].values[r])).filter(|s| !s.is_empty()).collect()
}

/// StatCan 2200: `proportion` under the header, then `mean` and `number` rows in the body.
#[test]
fn unit_rows_in_the_body_head_their_rows() {
    let t = table("mean", |ws| {
        ws.write_string(0, 0, "Table 1: Sample characteristics by country").unwrap();
        ws.write_string(2, 1, "Canada").unwrap();
        ws.write_string(2, 2, "United States").unwrap();
        ws.write_string(3, 1, "proportion").unwrap();
        let mut r = 4;
        let row = |ws: &mut Worksheet, r: &mut u32, label: &str, a: f64, b: f64| {
            ws.write_string(*r, 0, label).unwrap();
            ws.write_number(*r, 1, a).unwrap();
            ws.write_number(*r, 2, b).unwrap();
            *r += 1;
        };
        row(ws, &mut r, "Female", 0.47, 0.484);
        row(ws, &mut r, "Immigrant", 0.265, 0.162);
        row(ws, &mut r, "Holds a university credential", 0.323, 0.342);
        ws.write_string(r, 1, "mean").unwrap();
        r += 1;
        row(ws, &mut r, "Literacy", 278.3, 274.4);
        row(ws, &mut r, "Numeracy", 271.7, 260.6);
        ws.write_string(r, 1, "number").unwrap();
        r += 1;
        row(ws, &mut r, "Sample size", 16589.0, 2958.0);
    });
    assert_eq!(t.rows(), 6, "unit rows are no data rows: {:?}", t.src_rows);
    assert!(labels(&t, "Literacy").contains(&"mean".to_string()), "{:?}", labels(&t, "Literacy"));
    assert!(labels(&t, "Numeracy").contains(&"mean".to_string()), "{:?}", labels(&t, "Numeracy"));
    let n = labels(&t, "Sample size");
    assert!(n.contains(&"number".to_string()) && !n.contains(&"mean".to_string()), "{n:?}");
    assert!(!labels(&t, "Female").contains(&"mean".to_string()));
}

/// StatCan 175: `percentage points` over a section `Contribution` and its rows.
#[test]
fn unit_row_over_a_section() {
    let t = table("points", |ws| {
        ws.write_string(0, 0, "Table 4: Contribution of cohorts").unwrap();
        ws.write_string(2, 1, "2003 to 2009").unwrap();
        ws.write_string(2, 2, "2009 to 2014").unwrap();
        ws.write_string(3, 1, "percent").unwrap();
        ws.write_string(4, 0, "Change in technological frontier").unwrap();
        ws.write_number(4, 1, -7.0).unwrap();
        ws.write_number(4, 2, 11.9).unwrap();
        ws.write_string(5, 1, "percentage points").unwrap();
        ws.write_string(6, 0, "Contribution").unwrap();
        for (k, (l, a, b)) in [("High-productivity cohort", -8.4, 7.0), ("Low-productivity cohort", 4.9, 4.2), ("Entrants after 2002", -3.5, 0.7)]
            .iter()
            .enumerate()
        {
            ws.write_string(7 + k as u32, 0, *l).unwrap();
            ws.write_number(7 + k as u32, 1, *a).unwrap();
            ws.write_number(7 + k as u32, 2, *b).unwrap();
        }
    });
    let l = labels(&t, "High-productivity cohort");
    assert!(l.contains(&"percentage points".to_string()) && l.contains(&"Contribution".to_string()), "{l:?}");
    assert!(!labels(&t, "Change in technological frontier").contains(&"percentage points".to_string()));
}

/// A text column of codes is no unit row: a row with only `kg` in a text column of units stays data.
#[test]
fn a_unit_column_is_no_unit_row() {
    let t = table("unitcol", |ws| {
        for (c, h) in ["Item", "Unit", "Quantity"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        for (k, (i, u, q)) in [("Flour", "kg", 12.0), ("Milk", "l", 30.0), ("Eggs", "number", 120.0), ("Sugar", "kg", 5.0)].iter().enumerate() {
            ws.write_string(1 + k as u32, 0, *i).unwrap();
            ws.write_string(1 + k as u32, 1, *u).unwrap();
            ws.write_number(1 + k as u32, 2, *q).unwrap();
        }
    });
    assert_eq!(t.rows(), 4);
}

/// StatCan 132: a bold section (`Inequality decomposition …`, merged over the empty value cells)
/// right after the indented rows of the section above, over a bold indented sub-section: it
/// replaces the section above (`Summary of parameter estimates`), it is no note.
#[test]
fn section_after_indented_rows_replaces_the_one_above() {
    use rust_xlsxwriter::Format;
    let t = table("sections", |ws| {
        let bold = Format::new().set_bold();
        let ind = |n: u8| Format::new().set_indent(n);
        ws.write_string(0, 0, "Table 4: Summary of the estimated two-way fixed effects model for male and female workers").unwrap();
        ws.write_string_with_format(2, 1, "Male workers", &bold).unwrap();
        ws.write_string_with_format(2, 2, "Female workers", &bold).unwrap();
        ws.write_string_with_format(3, 1, "Column 1", &bold).unwrap();
        ws.write_string_with_format(3, 2, "Column 2", &bold).unwrap();
        ws.write_string(4, 0, "Standard deviation of log earnings").unwrap();
        ws.write_string(4, 1, "0.575").unwrap();
        ws.write_string(4, 2, "0.499").unwrap();
        ws.write_string_with_format(5, 0, "Summary of parameter estimates", &bold).unwrap();
        ws.merge_range(5, 1, 5, 2, "", &Format::new()).unwrap();
        for (k, l) in ["Number of person effects", "Number of firm effects", "Standard deviation of person effects"].iter().enumerate() {
            ws.write_string_with_format(6 + k as u32, 0, *l, &ind(1)).unwrap();
            ws.write_string(6 + k as u32, 1, &format!("0.{}", 438 + k)).unwrap();
            ws.write_string(6 + k as u32, 2, &format!("0.{}", 413 + k)).unwrap();
        }
        ws.write_string_with_format(9, 0, "Inequality decomposition of two-way fixed effects model", &bold).unwrap();
        ws.merge_range(9, 1, 9, 2, "", &Format::new()).unwrap();
        ws.write_string_with_format(10, 0, "Share of variance of log earnings attributable to", &bold.clone().set_indent(1)).unwrap();
        ws.merge_range(10, 1, 10, 2, "", &Format::new()).unwrap();
        for (k, l) in ["Person effects", "Firm effects", "Residual"].iter().enumerate() {
            ws.write_string_with_format(11 + k as u32, 0, *l, &ind(2)).unwrap();
            ws.write_string(11 + k as u32, 1, &format!("{}.1", 58 - 20 * k)).unwrap();
            ws.write_string(11 + k as u32, 2, &format!("{}.5", 68 - 20 * k)).unwrap();
        }
    });
    let l = labels(&t, "Person effects");
    assert!(l.contains(&"Inequality decomposition of two-way fixed effects model".to_string()), "{l:?}");
    assert!(!l.contains(&"Summary of parameter estimates".to_string()), "{l:?}");
}

/// NSF 128 table 2: sections `Current $` and `Constant 2012 $` (merged across the row), each over a
/// total row `Total R&D, all performers` and its indented parts: the second total row belongs to
/// the second section.
#[test]
fn total_row_right_under_a_section_belongs_to_it() {
    use rust_xlsxwriter::Format;
    let t = table("nsf", |ws| {
        let ind = Format::new().set_indent(1);
        ws.write_string(0, 0, "TABLE 2. Annual rates of growth in U.S. R&D expenditures").unwrap();
        ws.write_string(2, 0, "Expenditures and gross domestic product").unwrap();
        for (c, h) in ["1990–2000", "2000–08", "2008–16", "2008–09"].iter().enumerate() {
            ws.write_string(2, 1 + c as u16, *h).unwrap();
        }
        let mut r = 3u32;
        for (s, name) in ["Current $", "Constant 2012 $"].iter().enumerate() {
            ws.merge_range(r, 0, r, 4, name, &Format::new()).unwrap();
            r += 1;
            for (k, (l, f)) in [("Total R&D, all performers", None), ("Business", Some(&ind)), ("Federal government", Some(&ind)), ("Higher education", Some(&ind)), ("Gross domestic product", None)]
                .iter()
                .enumerate()
            {
                match f {
                    Some(f) => ws.write_string_with_format(r, 0, *l, f).unwrap(),
                    None => ws.write_string(r, 0, *l).unwrap(),
                };
                for c in 0..4u16 {
                    ws.write_string(r, 1 + c, &format!("{:.1}", 5.8 - 2.1 * s as f64 + 0.3 * k as f64 - 0.7 * c as f64)).unwrap();
                }
                r += 1;
            }
        }
    });
    // the second 'Total R&D' row: values 3.7 ... in the first value column
    let lab = t.columns.iter().position(|c| c.name.starts_with("Expenditures")).unwrap();
    let first = t.columns.iter().position(|c| c.name == "1990–2000").unwrap();
    let r = (0..t.rows())
        .find(|&r| text(&t.columns[lab].values[r]) == "Total R&D, all performers" && matches!(t.columns[first].values[r], Out::Num(x) if (x - 3.7).abs() < 1e-9))
        .expect("second total row");
    let g = t.columns.iter().position(|c| c.role == ColRole::Group).expect("group");
    assert_eq!(text(&t.columns[g].values[r]), "Constant 2012 $");
}

/// StatCan 2433: the first section `Individual health` sits over empty cells merged down into the
/// next section row (B5:F6). It is a section (the parent of `Self-reported general health`), not a
/// repeated header.
#[test]
fn first_section_over_a_merge_down_is_no_repeated_header() {
    use rust_xlsxwriter::Format;
    let t = table("mergedown", |ws| {
        let bold = Format::new().set_bold();
        let ind = |n: u8| Format::new().set_indent(n);
        ws.write_string(0, 0, "Table 2: Association between health and income, men").unwrap();
        for (c, h) in ["Spouse-pair income", "Individual income", "Employment", "Hours worked"].iter().enumerate() {
            ws.write_string_with_format(2, 1 + c as u16, *h, &bold).unwrap();
        }
        ws.merge_range(3, 1, 3, 2, "dollars", &Format::new()).unwrap();
        ws.write_string(3, 3, "percentage points").unwrap();
        ws.write_string(3, 4, "hours").unwrap();
        let mut r = 4u32;
        for (s, top) in ["Individual health", "Spousal health"].iter().enumerate() {
            ws.write_string_with_format(r, 0, *top, &bold).unwrap();
            ws.merge_range(r, 1, r + 1, 4, "", &Format::new()).unwrap();
            r += 1;
            for (k, sub) in ["Self-reported general health", "Self-reported mental health"].iter().enumerate() {
                ws.write_string_with_format(r, 0, *sub, &bold.clone().set_indent(1)).unwrap();
                if k > 0 {
                    ws.merge_range(r, 1, r, 4, "", &Format::new()).unwrap();
                }
                r += 1;
                for (j, l) in ["Good/very good/excellent (ref.)", "Fair/poor"].iter().enumerate() {
                    ws.write_string_with_format(r, 0, *l, &ind(2)).unwrap();
                    for c in 0..4u16 {
                        if j == 0 {
                            ws.write_string(r, 1 + c, "...").unwrap();
                        } else {
                            ws.write_string(r, 1 + c, &format!("{:.1}", -11.8 - 3.0 * s as f64 - 1.1 * k as f64 + 0.4 * c as f64)).unwrap();
                        }
                    }
                    r += 1;
                }
            }
        }
    });
    let first = (0..t.rows()).find(|&r| t.columns.iter().any(|c| text(&c.values[r]) == "Fair/poor")).unwrap();
    let l: Vec<String> = t.columns.iter().map(|c| text(&c.values[first])).collect();
    assert!(l.contains(&"Individual health".to_string()) && l.contains(&"Self-reported general health".to_string()), "{l:?}");
}

/// StatCan 108: bold section labels over plain rows; one of them starts with a total word
/// (`Total household income`) and has no number: a section, not a total row.
#[test]
fn total_worded_section_without_numbers_is_a_section() {
    use rust_xlsxwriter::Format;
    let t = table("totalsection", |ws| {
        let bold = Format::new().set_bold();
        ws.write_string(0, 0, "Table 2: Predicted probabilities of food insecurity").unwrap();
        ws.write_string(2, 1, "predicted probabilities").unwrap();
        let mut r = 3u32;
        for (sec, rows) in [
            ("Sex", &["Men (ref.)", "Women"][..]),
            ("Labour force status", &["Employed (ref.)", "Unemployed", "Not in the labour force"][..]),
            ("Total household income", &["Below median income", "Above or equal to median income (ref.)"][..]),
            ("Strength of extended family ties", &["Moderate to very weak", "Strong"][..]),
        ] {
            ws.write_string_with_format(r, 0, sec, &bold).unwrap();
            r += 1;
            for (k, l) in rows.iter().enumerate() {
                ws.write_string(r, 0, *l).unwrap();
                ws.write_string(r, 1, &format!("0.{}", 401 + 37 * k + r as usize)).unwrap();
                r += 1;
            }
        }
    });
    let l = labels(&t, "Below median income");
    assert!(l.contains(&"Total household income".to_string()) && !l.contains(&"Labour force status".to_string()), "{l:?}");
}

/// StatCan 1856: a data row (`Recent immigrant owners`, with values) heads sections indented below
/// it (`From China`, `From India`): it is their parent. A later row back at its indent leaves it.
#[test]
fn data_row_heads_the_sections_indented_below_it() {
    use rust_xlsxwriter::Format;
    let t = table("anchor", |ws| {
        let bold1 = Format::new().set_bold().set_indent(1);
        let ind2 = Format::new().set_indent(2);
        ws.write_string(0, 0, "Table 3: Value of single-detached properties by immigration category").unwrap();
        for (c, h) in ["Share", "Number", "Average value"].iter().enumerate() {
            ws.write_string(2, 1 + c as u16, *h).unwrap();
        }
        let mut r = 3u32;
        let vals = |ws: &mut Worksheet, r: u32, k: f64| {
            for c in 0..3u16 {
                ws.write_number(r, 1 + c, 1.5 + k * 3.25 + c as f64 * 1000.0).unwrap();
            }
        };
        ws.write_string(r, 0, "Canadian-born owners").unwrap();
        vals(ws, r, 1.0);
        r += 1;
        ws.write_string(r, 0, "Recent immigrant owners").unwrap();
        vals(ws, r, 2.0);
        r += 1;
        for (s, sec) in ["From China", "From India"].iter().enumerate() {
            ws.write_string_with_format(r, 0, *sec, &bold1).unwrap();
            r += 1;
            for (k, l) in ["Federal investors", "Provincial nominees", "Other"].iter().enumerate() {
                ws.write_string_with_format(r, 0, *l, &ind2).unwrap();
                vals(ws, r, 3.0 + s as f64 * 3.0 + k as f64);
                r += 1;
            }
        }
        ws.write_string(r, 0, "Non-permanent residents").unwrap();
        vals(ws, r, 20.0);
    });
    let find = |label: &str, nth: usize| -> Vec<String> {
        let rows: Vec<usize> = (0..t.rows()).filter(|&r| t.columns.iter().any(|c| text(&c.values[r]) == label)).collect();
        let r = rows[nth];
        t.columns.iter().filter(|c| c.role != ColRole::RowType).map(|c| text(&c.values[r])).filter(|s| !s.is_empty()).collect()
    };
    let l = find("Provincial nominees", 1);
    assert!(l.contains(&"Recent immigrant owners".to_string()) && l.contains(&"From India".to_string()), "{l:?}");
    let top = find("Non-permanent residents", 0);
    assert!(!top.iter().any(|x| x == "From India" || x == "Recent immigrant owners"), "{top:?}");
    let first = find("Canadian-born owners", 0);
    assert!(!first.iter().any(|x| x == "Recent immigrant owners"), "{first:?}");
}
