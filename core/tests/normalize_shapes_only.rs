//! `Options::shapes_only`: every column's values are left empty, and every other field of
//! `Normalized` is the same as without it. The workbooks are generated here.
//!
//! The ignored `measure_normalize` normalizes one file for a benchmark (one process per run, so
//! the peak RSS is the run's): `HH_NORM_FILE=big.xlsx HH_NORM_SHAPES=0|1 HH_NORM_THREADS=1|none
//! cargo test --release -p hh_core --test normalize_shapes_only -- --ignored --nocapture
//! measure_normalize`. `HH_NORM_WRITE=big.xlsx` with `write_big` first writes a workbook there:
//! `HH_NORM_SHEETS` sheets (default 4) of `HH_NORM_ROWS` rows (default 25,000) x 10 columns,
//! 1,000,000 cells by default.

use hh_core::normalize::{Normalized, Options, normalize_workbook};
use hh_core::{Workbook, open, open_bytes};
use rust_xlsxwriter::Worksheet;

/// A title, a header, group rows, a column of period labels (a derived `_period` column), a
/// column of name spellings (a derived `_key` column), text and numeric columns, a total row.
fn table(ws: &mut Worksheet, rows: u32, seed: u32) {
    ws.write_string(0, 0, format!("Sales by region, sheet {seed}"))
        .unwrap();
    let header = [
        "Period", "Customer", "Product", "Note", "Units", "Price", "Revenue", "Cost", "Margin",
        "Share",
    ];
    for (c, h) in (0u16..).zip(header) {
        ws.write_string(2, c, h).unwrap();
    }
    let names = [
        "Mc Kay", "McKay", "MCKAY", "Smith", "Jones", "O'Neil", "ONeil",
    ];
    let mut r = 3;
    let mut sum = 0.0;
    for i in 0..rows {
        if i % 500 == 0 {
            ws.write_string(r, 0, format!("Region {}", i / 500 + seed))
                .unwrap();
            r += 1;
        }
        let k = i + seed;
        ws.write_string(r, 0, format!("{} Q{}", 2000 + k % 20, k % 4 + 1))
            .unwrap();
        ws.write_string(r, 1, names[(k % 7) as usize]).unwrap();
        ws.write_string(r, 2, format!("product {}", k % 97))
            .unwrap();
        ws.write_string(r, 3, format!("order {k} shipped from depot {}", k % 13))
            .unwrap();
        let units = f64::from(k % 50 + 1);
        let price = f64::from(k % 300) + 0.99;
        ws.write_number(r, 4, units).unwrap();
        ws.write_number(r, 5, price).unwrap();
        ws.write_number(r, 6, units * price).unwrap();
        ws.write_number(r, 7, units * price * 0.6).unwrap();
        ws.write_number(r, 8, units * price * 0.4).unwrap();
        ws.write_number(r, 9, f64::from(k % 100) / 100.0).unwrap();
        sum += units;
        r += 1;
    }
    ws.write_string(r, 0, "Total").unwrap();
    ws.write_number(r, 4, sum).unwrap();
    ws.write_string(r + 2, 0, "Source: generated for the test.")
        .unwrap();
    // Key/value cells beside the table: a table of their own.
    ws.write_string(0, 12, "Total Income").unwrap();
    ws.write_number(0, 13, 9666.0).unwrap();
    ws.write_string(1, 12, "Total Expenditure").unwrap();
    ws.write_number(1, 13, 7266.0).unwrap();
}

fn workbook(sheets: u32, rows: u32) -> rust_xlsxwriter::Workbook {
    let mut wb = rust_xlsxwriter::Workbook::new();
    for s in 0..sheets {
        let ws = wb.add_worksheet().set_name(format!("S{s}")).unwrap();
        table(ws, rows, s);
    }
    wb
}

fn book(sheets: u32, rows: u32) -> Workbook {
    let bytes = workbook(sheets, rows).save_to_buffer().unwrap();
    open_bytes(&bytes, Some("t.xlsx")).unwrap()
}

fn without_values(mut n: Normalized) -> Normalized {
    for t in &mut n.tables {
        for c in &mut t.columns {
            c.values = Vec::new();
        }
    }
    n
}

#[test]
fn shapes_only_keeps_everything_but_the_values() {
    for (sheets, rows) in [(1, 40), (3, 120), (5, 30)] {
        let wb = book(sheets, rows);
        for threads in [None, Some(1)] {
            let base = Options {
                threads,
                ..Options::default()
            };
            let full = normalize_workbook(&wb, &base);
            assert!(full.tables.len() > sheets as usize, "{sheets} sheets");
            let derived = full
                .tables
                .iter()
                .flat_map(|t| &t.columns)
                .filter(|c| c.derived_kind().is_some())
                .count();
            assert!(derived > 0, "the test covers derived columns");
            assert!(
                full.tables
                    .iter()
                    .flat_map(|t| &t.columns)
                    .any(|c| !c.values.is_empty())
            );
            for keep_totals in [false, true] {
                let full = normalize_workbook(
                    &wb,
                    &Options {
                        keep_totals,
                        ..base
                    },
                );
                let shapes = normalize_workbook(
                    &wb,
                    &Options {
                        keep_totals,
                        shapes_only: true,
                        ..base
                    },
                );
                assert!(
                    shapes
                        .tables
                        .iter()
                        .flat_map(|t| &t.columns)
                        .all(|c| c.values.is_empty())
                );
                assert_eq!(
                    shapes,
                    without_values(full),
                    "{sheets} sheets, threads {threads:?}, keep_totals {keep_totals}"
                );
            }
        }
    }
}

/// `normalize_file` writes the values, so it ignores the switch: the same files.
#[test]
fn normalize_file_ignores_shapes_only() {
    let dir = std::env::temp_dir().join(format!("hh_shapes_only_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("t.xlsx");
    workbook(2, 50).save(&src).unwrap();
    let run = |shapes_only: bool| {
        let out = dir.join(format!("out_{shapes_only}"));
        let o = hh_core::normalize_file(
            &src,
            &out,
            &Options {
                shapes_only,
                ..Options::default()
            },
        )
        .unwrap();
        (o.manifest_json, std::fs::read(&o.xlsx_path).unwrap())
    };
    let (a, b) = (run(false), run(true));
    std::fs::remove_dir_all(&dir).ok();
    assert!(a == b, "same manifest and xlsx bytes");
}

#[test]
#[ignore]
fn write_big() {
    let path = std::env::var("HH_NORM_WRITE").expect("HH_NORM_WRITE=path.xlsx");
    let env = |k: &str, d: u32| std::env::var(k).map_or(d, |v| v.parse().unwrap());
    workbook(env("HH_NORM_SHEETS", 4), env("HH_NORM_ROWS", 25_000))
        .save(&path)
        .unwrap();
}

fn peak_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l[6..].trim().trim_end_matches(" kB").parse().ok())
        })
        .unwrap_or(0)
}

#[test]
#[ignore]
fn measure_normalize() {
    let path = std::env::var("HH_NORM_FILE").expect("HH_NORM_FILE=path.xlsx");
    let shapes_only = std::env::var("HH_NORM_SHAPES").is_ok_and(|v| v == "1");
    let threads = match std::env::var("HH_NORM_THREADS").as_deref() {
        Ok("none") | Err(_) => None,
        Ok(n) => Some(n.parse().expect("HH_NORM_THREADS: a number or none")),
    };
    let wb = open(&path).unwrap();
    let cells: usize = wb.sheets.iter().map(|s| s.cells.len()).sum();
    let read_peak = peak_kb();
    let t = std::time::Instant::now();
    let norm = hh_core::try_normalize_workbook(
        &wb,
        &Options {
            threads,
            shapes_only,
            ..Options::default()
        },
    )
    .unwrap();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "normalize shapes_only={shapes_only} threads={threads:?}: {cells} cells, {} tables, \
         {ms:.1} ms, peak after read {} MB, peak {} MB",
        norm.tables.len(),
        read_peak / 1024,
        peak_kb() / 1024
    );
}
