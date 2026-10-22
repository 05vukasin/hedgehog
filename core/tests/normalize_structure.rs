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
