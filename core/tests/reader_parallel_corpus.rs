//! Large xlsx sheets and shared string tables, and the tables of a large ODS `content.xml`, are
//! read on several threads (vendor/calamine/src/xlsx/parallel.rs, `parallel_content` in
//! vendor/calamine/src/ods.rs); this checks that every file
//! of a list reads to exactly the same workbook (cells, merged areas, formats, or the same error)
//! on one thread and in parallel with tiny chunks, so that every sheet is cut at many rows.
//! Ignored by default:
//! `READ_LIST=files.txt cargo test --release -p hh_core --test reader_parallel_corpus -- --ignored --nocapture`
//! Env: `READ_LIST` (one path per line; default: the xlsx files under `$HEDGEHOG_DATA/irregular/enron`),
//! `READ_CHUNK` (bytes per chunk, default 4096), `READ_THREADS` (files at once, default 2).
//! Files are untrusted and only parsed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use hh_core::reader::{open_with, ReadOptions};

fn files() -> Vec<PathBuf> {
    if let Ok(list) = std::env::var("READ_LIST") {
        return std::fs::read_to_string(list).unwrap().lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).collect();
    }
    let data = std::env::var("HEDGEHOG_DATA").unwrap_or_else(|_| "../../testdata".into());
    let Ok(rd) = std::fs::read_dir(PathBuf::from(data).join("irregular/enron")) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xlsx") || x.eq_ignore_ascii_case("xlsm")))
        .collect();
    v.sort();
    v
}

/// The workbook as comparable text: per sheet its name, cells, merges and formats; or the error.
fn read(path: &PathBuf, formats: bool) -> Result<Vec<(String, Vec<hh_core::Cell>, Vec<hh_core::Rect>, Vec<hh_core::model::Format>)>, String> {
    let wb = open_with(path, ReadOptions { formats, ..Default::default() }).map_err(|e| e.to_string())?;
    Ok(wb.sheets.into_iter().map(|s| (s.name, s.cells, s.merges, s.formats)).collect())
}

#[test]
#[ignore]
fn parallel_read_matches_one_thread() {
    let files = files();
    let chunk: usize = std::env::var("READ_CHUNK").ok().and_then(|s| s.parse().ok()).unwrap_or(4096);
    let threads: usize = std::env::var("READ_THREADS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let next = AtomicUsize::new(0);
    let checked = AtomicUsize::new(0);
    let diffs = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(i) else { break };
                for formats in [false, true] {
                    calamine::set_parallel_read_for_tests(Some((1, chunk, u64::MAX)));
                    let want = read(path, formats);
                    calamine::set_parallel_read_for_tests(Some((4, chunk, 0)));
                    let got = read(path, formats);
                    calamine::set_parallel_read_for_tests(None);
                    // as text: NaN equals NaN, -0 differs from 0
                    if format!("{got:?}") != format!("{want:?}") {
                        let detail = match (&want, &got) {
                            (Ok(a), Ok(b)) => a
                                .iter()
                                .zip(b)
                                .find(|(x, y)| x != y)
                                .map(|(x, y)| {
                                    let cell = x.1.iter().zip(&y.1).find(|(c, d)| c != d);
                                    format!("sheet {:?}: {} vs {} cells, first differing {:?}, merges {}, formats {}", x.0, x.1.len(), y.1.len(), cell, x.2 == y.2, x.3 == y.3)
                                })
                                .unwrap_or_else(|| format!("{} vs {} sheets", a.len(), b.len())),
                            (a, b) => format!("{:?} vs {:?}", a.as_ref().err(), b.as_ref().err()),
                        };
                        diffs.lock().unwrap().push(format!("{} (formats {formats}): {detail}", path.display()));
                    }
                }
                checked.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    let diffs = diffs.into_inner().unwrap();
    let (parts, gave_up, again) = calamine::parallel_read_stats();
    println!(
        "parallel read: {} files checked, {} differ; parts read in parallel {parts}, gave up {gave_up}, chunks read again {again}",
        checked.load(Ordering::Relaxed),
        diffs.len()
    );
    for d in &diffs {
        println!("  differs: {d}");
    }
    assert!(diffs.is_empty());
}
