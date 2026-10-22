//! Bit-for-bit repeatable output (B68): the same input gives the same layout, manifest and
//! normalized xlsx bytes in every run, on any number of threads.
//!
//! The corpus check needs `$HEDGEHOG_DATA` and is ignored by default:
//! `cargo test --release -p hh_core --test determinism -- --ignored --nocapture`
//! (`fuzz/determinism.py` runs the same comparison over a whole folder through the CLI).

use std::path::{Path, PathBuf};

use hh_core::{layout, open, Grid, NormalizeOptions};
use rust_xlsxwriter::{Workbook, Worksheet};

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hh_determinism_{}_{name}", std::process::id()))
}

/// Title, header and 30 rows over 14 columns; column `j` holds a text ("rev 7") on every
/// `j + 2`-th row, so the columns have many different type purities, then a total row.
fn mixed_table(ws: &mut Worksheet, seed: u32) {
    ws.write_string(0, 0, "Quarterly figures by region").unwrap();
    ws.write_string(2, 0, "Region").unwrap();
    for j in 1..15u16 {
        ws.write_string(2, j, format!("M{j}")).unwrap();
    }
    for i in 0..30u32 {
        let r = 3 + i;
        ws.write_string(r, 0, format!("Region {i}")).unwrap();
        for j in 1..15u16 {
            if (i + seed).is_multiple_of(j as u32 + 2) {
                ws.write_string(r, j, format!("rev {i}")).unwrap();
            } else {
                ws.write_number(r, j, ((i * 37 + j as u32 * 11 + seed) % 997) as f64 + 0.25).unwrap();
            }
        }
    }
    ws.write_string(33, 0, "Total").unwrap();
}

fn workbook(path: &Path, sheets: u32) {
    let mut wb = Workbook::new();
    for s in 0..sheets {
        let ws = wb.add_worksheet();
        ws.set_name(format!("Sheet{s}")).unwrap();
        mixed_table(ws, s);
    }
    wb.save(path).unwrap();
}

/// The table confidence averages per-column purities; it was summed in hash-map order, so its
/// last bits (and the rounded manifest value) changed between runs. Every `HashMap` gets new
/// random keys, so repeated analyses in one process see many different orders.
#[test]
fn table_confidence_does_not_depend_on_hash_order() {
    let path = tmp("conf.xlsx");
    workbook(&path, 1);
    let mut book = open(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let sheet = book.sheets.remove(0);
    let first: Vec<u32> = layout::analyze(&Grid::new(&sheet)).tables.iter().map(|t| t.confidence.to_bits()).collect();
    assert!(!first.is_empty());
    for _ in 0..60 {
        let again: Vec<u32> =
            layout::analyze(&Grid::new(&sheet)).tables.iter().map(|t| t.confidence.to_bits()).collect();
        assert_eq!(again, first);
    }
}

fn normalize_bytes(src: &Path, out: &Path, threads: Option<usize>) -> (String, Vec<u8>) {
    let opts = NormalizeOptions { threads, ..Default::default() };
    let o = hh_core::normalize_file(src, out, &opts).unwrap();
    let xlsx = std::fs::read(&o.xlsx_path).unwrap();
    std::fs::remove_dir_all(out).ok();
    (o.manifest_json, xlsx)
}

fn zip_part(xlsx: &[u8], name: &str) -> String {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(xlsx)).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

/// Manifest and xlsx bytes are identical across runs, across a second boundary (no timestamps)
/// and between parallel and single-threaded sheet analysis.
#[test]
fn normalized_output_is_byte_identical() {
    let src = tmp("norm.xlsx");
    workbook(&src, 4);
    let out = tmp("norm_out");
    let first = normalize_bytes(&src, &out, None);
    let core = zip_part(&first.1, "docProps/core.xml");
    assert!(core.contains("1980-01-01T00:00:00Z"), "{core}");
    for (k, threads) in [None, Some(1), None, Some(1), Some(3), None, Some(1)].into_iter().enumerate() {
        if k == 3 {
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        let again = normalize_bytes(&src, &out, threads);
        assert!(again.0 == first.0, "manifest differs (threads {threads:?})");
        assert!(again.1 == first.1, "xlsx bytes differ (threads {threads:?})");
    }
    std::fs::remove_file(&src).ok();
}

/// Real files whose output used to change between runs; 5 threaded and 5 single-threaded runs
/// each must give the same manifest and xlsx bytes.
#[test]
#[ignore]
fn corpus_files_are_repeatable() {
    let Some(data) = std::env::var_os("HEDGEHOG_DATA").map(PathBuf::from) else {
        eprintln!("HEDGEHOG_DATA not set; skipped");
        return;
    };
    let files = ["irregular/us_bea/007_trad0826.xlsx", "irregular/us_bea/001_gdp-gdi-vintage-history.xlsx"];
    for f in files {
        let src = data.join(f);
        if !src.exists() {
            eprintln!("{f}: missing, skipped");
            continue;
        }
        let out = tmp("corpus_out");
        let first = normalize_bytes(&src, &out, None);
        for k in 0..9 {
            let threads = if k < 4 { None } else { Some(1) };
            let again = normalize_bytes(&src, &out, threads);
            assert!(again.0 == first.0, "{f}: manifest differs in run {}", k + 2);
            assert!(again.1 == first.1, "{f}: xlsx bytes differ in run {}", k + 2);
        }
        eprintln!("{f}: 10 runs identical");
    }
}
