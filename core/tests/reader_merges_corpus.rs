//! The reader finds merged areas with a byte scan instead of calamine's second XML parse; this
//! checks both agree on a corpus. Ignored by default:
//! `cargo test --release -p hh_core --test reader_merges_corpus -- --ignored --nocapture`
//! Env: `HEDGEHOG_ENRON` (directory, default the Enron set), `MERGES_LIMIT` (files, default all),
//! `MERGES_THREADS` (default 8). Files are untrusted and only parsed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use calamine::{open_workbook_auto, Reader, Sheets};
use hh_core::Rect;

const DEFAULT_DIR: &str = "/home/vukasin/projects/excell-lib-project/testdata/irregular/enron";

fn calamine_merges(path: &PathBuf) -> Option<Vec<(String, Vec<Rect>)>> {
    let mut wb = open_workbook_auto(path).ok()?;
    let names = wb.sheet_names();
    let Sheets::Xlsx(x) = &mut wb else { return None };
    Some(
        names
            .into_iter()
            .map(|n| {
                let m = x
                    .merge_cells_by_sheet_name(&n)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|d| Rect { top: d.start.0, left: d.start.1, bottom: d.end.0, right: d.end.1 })
                    .collect();
                (n, m)
            })
            .collect(),
    )
}

#[test]
#[ignore]
fn merges_match_calamine() {
    let dir = std::env::var("HEDGEHOG_ENRON").unwrap_or_else(|_| DEFAULT_DIR.to_string());
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("skipped: {dir} not found");
        return;
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xlsx") || x.eq_ignore_ascii_case("xlsm")))
        .collect();
    files.sort();
    if let Some(l) = std::env::var("MERGES_LIMIT").ok().and_then(|s| s.parse().ok()) {
        files.truncate(l);
    }
    let threads: usize = std::env::var("MERGES_THREADS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let next = AtomicUsize::new(0);
    let checked = AtomicUsize::new(0);
    let with_merges = AtomicUsize::new(0);
    let diffs = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(i) else { break };
                // Only files calamine opens directly (repaired ones take the same code path).
                let Some(want) = calamine_merges(path) else { continue };
                let Ok(wb) = hh_core::open(path) else { continue };
                let got: Vec<(String, Vec<Rect>)> = wb.sheets.into_iter().map(|s| (s.name, s.merges)).collect();
                checked.fetch_add(1, Ordering::Relaxed);
                if want.iter().any(|(_, m)| !m.is_empty()) {
                    with_merges.fetch_add(1, Ordering::Relaxed);
                }
                if got != want {
                    diffs.lock().unwrap().push(path.display().to_string());
                }
            });
        }
    });
    let diffs = diffs.into_inner().unwrap();
    let (scanned, fallback) = hh_core::reader::merge_scan_stats();
    println!(
        "merges: {} files checked, {} with merges, {} differ; sheets scanned {scanned}, fell back {fallback}",
        checked.load(Ordering::Relaxed),
        with_merges.load(Ordering::Relaxed),
        diffs.len()
    );
    assert!(diffs.is_empty(), "merged areas differ from calamine in:\n{}", diffs.join("\n"));
}
