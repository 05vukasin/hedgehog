//! Structure repairs of the normalized output found by the agent comparisons: sex breakdown
//! blocks (M / F / T rows) are data with a sex column and a subtotal per block, a numbered caption
//! is the title (B72); period labels get a sortable `<name>_period` column (B70).

use std::path::PathBuf;

use hh_core::normalize::{normalize_workbook, Normalized, Options, Out, RowType, Table};
use hh_core::open;
use rust_xlsxwriter::Workbook;

fn normalize(name: &str, build: impl FnOnce(&mut rust_xlsxwriter::Worksheet)) -> Normalized {
    let path: PathBuf = std::env::temp_dir().join(format!("hh_structure_{name}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    build(wb.add_worksheet());
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    normalize_workbook(&book, &Options { keep_totals: true, ..Options::default() })
}

fn col<'a>(t: &'a Table, name: &str) -> &'a [Out] {
    &t.columns.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no column {name:?} in {:?}", names(t))).values
}

fn names(t: &Table) -> Vec<&str> {
    t.columns.iter().map(|c| c.name.as_str()).collect()
}

fn text(v: &Out) -> &str {
    match v {
        Out::Text(s) => s,
        _ => "",
    }
}

#[test]
fn sex_breakdown_blocks_are_data_with_a_subtotal_per_block() {
    let n = normalize("sex", |ws| {
        ws.write_string(0, 0, "BALANÇO SOCIAL").unwrap();
        ws.write_string(2, 4, "Ano: 2016").unwrap();
        ws.write_string(3, 0, "Quadro 1").unwrap();
        ws.write_string(3, 1, "Contagem dos trabalhadores por cargo e género").unwrap();
        for (c, h) in ["Relação Jurídica", "Sexo", "Técnico", "Assistente", "Bombeiros", "TOTAL"].iter().enumerate() {
            ws.write_string(5, c as u16, *h).unwrap();
        }
        // (label rows, M values, F values): the first block is the total of the others.
        let blocks: [(&[&str], [f64; 3], [f64; 3]); 4] = [
            (&["", "Total", ""], [13.0, 26.0, 9.0], [11.0, 7.0, 2.0]),
            (&["", "Comissão de serviço", ""], [3.0, 20.0, 0.0], [5.0, 1.0, 0.0]),
            (&["CTFP", "Por tempo", "indeterminado"], [10.0, 5.0, 9.0], [6.0, 6.0, 2.0]),
            (&["", "Outras", ""], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]),
        ];
        let mut row = 7;
        for (labels, m, f) in blocks {
            for (k, sex) in ["M", "F", "T"].iter().enumerate() {
                if !labels[k].is_empty() {
                    ws.write_string(row, 0, labels[k]).unwrap();
                }
                ws.write_string(row, 1, *sex).unwrap();
                let vals: Vec<f64> = (0..3).map(|j| [m[j], f[j], m[j] + f[j]][k]).collect();
                for (j, v) in vals.iter().enumerate() {
                    ws.write_number(row, 2 + j as u16, *v).unwrap();
                }
                ws.write_number(row, 5, vals.iter().sum::<f64>()).unwrap();
                row += 1;
            }
        }
    });
    let t = &n.tables[0];
    assert!(t.title.as_ref().is_some_and(|x| x.0.starts_with("Quadro 1 Contagem")), "{:?} {:?} {} {:?} {:?}", t.title, names(t), t.header_top, t.rect, n.notes);
    let label = col(t, "Relação Jurídica");
    let sex = col(t, "Sexo");
    let total = col(t, "TOTAL");
    let rows: Vec<(String, String, RowType)> =
        (0..t.rows()).map(|i| (text(&label[i]).to_string(), text(&sex[i]).to_string(), t.row_types[i])).collect();
    assert!(rows.contains(&("CTFP Por tempo indeterminado".into(), "M".into(), RowType::Data)), "{rows:?}");
    assert!(rows.contains(&("Comissão de serviço".into(), "T".into(), RowType::Subtotal)), "{rows:?}");
    assert!(rows.contains(&("Total".into(), "F".into(), RowType::Total)), "{rows:?}");
    // The data rows (M and F of each group) add up to the file's total.
    let data_sum: f64 = (0..t.rows())
        .filter(|&i| t.row_types[i] == RowType::Data)
        .map(|i| if let Out::Num(x) = total[i] { x } else { 0.0 })
        .sum();
    assert_eq!(data_sum, 68.0);
}

#[test]
fn register_names_that_start_with_a_total_word_are_no_totals() {
    let n = normalize("names", |ws| {
        for (c, h) in ["EmployerName", "Town", "Employees", "Gap"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        for r in 1..=260u32 {
            let name = match r {
                100 => "TOTAL RECRUIT LTD".to_string(),
                150 => "Zusammen unterwegs".to_string(),
                _ => format!("Employer {r} Ltd"),
            };
            ws.write_string(r, 0, name).unwrap();
            ws.write_string(r, 1, ["Leeds", "York", "Hull"][r as usize % 3]).unwrap();
            ws.write_number(r, 2, (250 + r * 7 % 900) as f64).unwrap();
            ws.write_number(r, 3, (r % 23) as f64 / 2.0).unwrap();
        }
    });
    assert_eq!(n.tables[0].rows(), 260);
    assert!(n.totals.is_empty(), "{:?}", n.totals.iter().map(|x| (x.label.clone(), x.status, x.reason.clone(), x.kept_in_data, x.cell_a1())).collect::<Vec<_>>());
}

#[test]
fn owid_and_eurostat_aggregates_are_total_rows() {
    let owid = normalize("owid", |ws| {
        for (c, h) in ["country", "year", "solar_electricity"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        let mut names: Vec<String> = (0..40).map(|k| format!("Country {k}")).collect();
        names.extend(["World", "Africa", "Asia (EI)", "High-income countries"].map(String::from));
        let mut r = 1;
        for (k, name) in names.iter().enumerate() {
            for y in 2019..2025 {
                ws.write_string(r, 0, name).unwrap();
                ws.write_number(r, 1, y as f64).unwrap();
                ws.write_number(r, 2, (k * 7 + y as usize % 5) as f64 + 0.5).unwrap();
                r += 1;
            }
        }
    });
    let t = &owid.tables[0];
    let country = col(t, "country");
    let totals: Vec<&str> = (0..t.rows()).filter(|&i| t.row_types[i] != RowType::Data).map(|i| text(&country[i])).collect();
    assert_eq!(totals.len(), 24, "{totals:?}");
    assert!(totals.contains(&"World") && totals.contains(&"Asia (EI)") && !totals.contains(&"Country 3"));

    let eurostat = normalize("eurostat", |ws| {
        for (c, h) in ["geo", "TIME_PERIOD", "OBS_VALUE"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        let geos = ["AT", "BE", "BG", "CY", "CZ", "DE", "DK", "EE", "EL", "ES", "FI", "FR", "HR", "HU", "IE", "IT", "EU27_2020", "EA20"];
        let mut r = 1;
        for (k, g) in geos.iter().enumerate() {
            for half in ["2024-S1", "2024-S2"] {
                ws.write_string(r, 0, *g).unwrap();
                ws.write_string(r, 1, half).unwrap();
                ws.write_number(r, 2, 0.2 + k as f64 / 100.0).unwrap();
                r += 1;
            }
        }
        // A register: more rows below.
        for k in 0..200u32 {
            ws.write_string(r, 0, ["LT", "LU", "LV", "MT", "NL"][k as usize % 5]).unwrap();
            ws.write_string(r, 1, "2023-S1").unwrap();
            ws.write_number(r, 2, 0.3 + k as f64 / 1000.0).unwrap();
            r += 1;
        }
    });
    let t = &eurostat.tables[0];
    let geo = col(t, "geo");
    let totals: Vec<&str> = (0..t.rows()).filter(|&i| t.row_types[i] != RowType::Data).map(|i| text(&geo[i])).collect();
    assert_eq!(totals, ["EU27_2020", "EU27_2020", "EA20", "EA20"]);
}

#[test]
fn month_end_headers_roll_over_into_the_next_year() {
    let n = normalize("rollover", |ws| {
        let date = rust_xlsxwriter::Format::new().set_num_format("mm/dd/yy");
        ws.write_string(0, 0, "Pipeline").unwrap();
        // Jul–Dec 2001, then Jan–Mar typed as 2001 (serials 37103.. and 36922..).
        let serials = [37103.0, 37134.0, 37164.0, 37195.0, 37225.0, 37256.0, 36922.0, 36950.0, 36981.0];
        for (k, s) in serials.iter().enumerate() {
            ws.write_number_with_format(0, 1 + k as u16, *s, &date).unwrap();
        }
        for r in 1..5u32 {
            ws.write_string(r, 0, format!("P{r}")).unwrap();
            for k in 0..serials.len() {
                ws.write_number(r, 1 + k as u16, (r * 100 + k as u32) as f64).unwrap();
            }
        }
    });
    let t = &n.tables[0];
    let names = names(t);
    assert_eq!(names[6..10], ["2001-12-31", "2002-01-31", "2002-02-28", "2002-03-31"], "{names:?}");
    assert!(t.warnings.iter().any(|w| w.contains("next year")), "{:?}", t.warnings);
}

#[test]
fn stacked_blocks_without_a_header_borrow_the_one_above() {
    let n = normalize("borrow", |ws| {
        for (c, h) in ["Jahr", "Einnahmen", "Ausgaben", "Finanzierungssaldo"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        for (k, y) in (1991..2026).enumerate() {
            let r = 1 + k as u32;
            ws.write_number(r, 0, y as f64).unwrap();
            for c in 1..4u16 {
                ws.write_number(r, c, 1000.0 + (k as f64) * 13.5 + c as f64 * 7.25).unwrap();
            }
        }
        ws.write_string(40, 1, "% des Bruttoinlandsprodukts").unwrap();
        for (k, y) in (1991..2026).enumerate() {
            let r = 41 + k as u32;
            ws.write_number(r, 0, y as f64).unwrap();
            for c in 1..4u16 {
                ws.write_number(r, c, 40.0 + (k as f64) * 0.37 - c as f64 * 3.1).unwrap();
            }
        }
    });
    let low = n.tables.iter().find(|t| t.rect.top >= 38).unwrap_or_else(|| panic!("{:?}", n.tables.iter().map(|t| t.rect).collect::<Vec<_>>()));
    let names = names(low);
    assert!(names.contains(&"Finanzierungssaldo / % des Bruttoinlandsprodukts") && names.contains(&"Jahr"), "{names:?}");
}

#[test]
fn period_labels_get_a_sortable_period_column() {
    let n = normalize("periods", |ws| {
        ws.write_string(0, 0, "Jahr / Monat").unwrap();
        ws.write_string(0, 1, "% zu Vorjahr").unwrap();
        let labels = ["Ø 2019", "Jän.20", "Feb.20", "Mär.20", "Apr.201", "Mai.201", "Ø 20", "Jän.21", "Feb.21"];
        for (k, l) in labels.iter().enumerate() {
            ws.write_string(1 + k as u32, 0, *l).unwrap();
            ws.write_number(1 + k as u32, 1, 1.0 + k as f64 / 10.0).unwrap();
        }
    });
    let t = &n.tables[0];
    assert_eq!(names(t)[..3], ["Jahr / Monat", "Jahr / Monat_period", "% zu Vorjahr"]);
    let p: Vec<&str> = col(t, "Jahr / Monat_period").iter().map(text).collect();
    assert_eq!(p, ["2019", "2020-01", "2020-02", "2020-03", "2020-04", "2020-05", "2020", "2021-01", "2021-02"]);
    let cov = hh_core::normalize::table_time_coverage(t).unwrap();
    assert_eq!((cov.from.as_str(), cov.to.as_str(), cov.granularity), ("2019", "2021-02", "month"));
}

/// Balanço Social 2016, A349:A351: a leading space on the middle fragment
/// belongs to one label reconstructed across M/F/T rows, not an outline child.
#[test]
fn split_sex_block_label_is_not_a_sparse_outline() {
    let n = normalize("sex_fragment_indent", |ws| {
        for (c, h) in ["Motivo da Alteração", "Sexo", "Técnico", "Assistente", "TOTAL"].iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        let blocks = [
            ["Promoções", "(carreiras não revistas e", "carreiras subsistentes)"],
            ["Procedimento", "Concursal", ""],
            ["Consolidação da", "mobilidade", "na categoria"],
            ["Alteração obrigatória", " do posicionamento", "remuneratório"],
            ["Alte.do posicionamento", "remuneratório por ", "opção gestionária (regra)"],
            ["Alte.do posicionamento", "remuneratório por ", "opção gestionária (excepção)"],
        ];
        for (b, labels) in blocks.iter().enumerate() {
            for (k, sex) in ["M", "F", "T"].iter().enumerate() {
                let r = 1 + (3 * b + k) as u32;
                if !labels[k].is_empty() { ws.write_string(r, 0, labels[k]).unwrap(); }
                ws.write_string(r, 1, *sex).unwrap();
                for c in 2..=3u16 {
                    ws.write_number(r, c, if k == 2 { 3.0 } else { 1.0 + k as f64 }).unwrap();
                }
                ws.write_number(r, 4, if k == 2 { 6.0 } else { 2.0 + 2.0 * k as f64 }).unwrap();
            }
        }
    });
    let t = n.tables.iter().find(|t| t.columns.iter().any(|c| c.name == "Sexo")).unwrap();
    let label = col(t, "Motivo da Alteração");
    assert!(label.iter().any(|v| text(v) == "Alteração obrigatória do posicionamento remuneratório"));
    assert!(t.columns.iter().all(|c| c.derived_kind() != Some("parent")), "fragment whitespace became hierarchy: {:?}", names(t));
}
