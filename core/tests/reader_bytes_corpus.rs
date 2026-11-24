//! `open_bytes_with` must read every file of a list exactly as `open_with` reads the file (cells,
//! merged areas, formats, or the same error), with the file name as the hint and, for
//! workbooks, without one. Ignored by default:
//! `READ_LIST=files.txt cargo test --release -p hh_core --test reader_bytes_corpus -- --ignored --nocapture`
//! Env: `READ_LIST` (one path per line; default: every file under `$HEDGEHOG_DATA/irregular`
//! the reader takes), `READ_THREADS` (files at once, default 2), `READ_MAX_MB` (larger files
//! are skipped, default 64). Files are untrusted and only parsed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use hh_core::reader::{open_bytes_with, open_with, ReadOptions};

const EXTENSIONS: [&str; 9] = ["xlsx", "xlsm", "xlsb", "xls", "ods", "csv", "tsv", "txt", "asp"];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| EXTENSIONS.contains(&x.to_ascii_lowercase().as_str())) {
            out.push(p);
        }
    }
}

fn files() -> Vec<PathBuf> {
    if let Ok(list) = std::env::var("READ_LIST") {
        return std::fs::read_to_string(list).unwrap().lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).collect();
    }
    let data = std::env::var("HEDGEHOG_DATA").unwrap_or_else(|_| "../../testdata".into());
    let mut v = Vec::new();
    walk(&PathBuf::from(data).join("irregular"), &mut v);
    v.sort();
    v
}

/// A read result as comparable text (NaN equals NaN, -0 differs from 0).
fn outcome(r: Result<hh_core::Workbook, hh_core::Error>) -> String {
    match r {
        Ok(wb) => format!("{:?}", wb.sheets),
        Err(e) => format!("error (limit {}): {e}", e.is_limit()),
    }
}

#[test]
#[ignore]
fn bytes_read_like_files() {
    let files = files();
    let threads: usize = std::env::var("READ_THREADS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let max = std::env::var("READ_MAX_MB").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(64) << 20;
    let next = AtomicUsize::new(0);
    let (checked, skipped, errors) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    let diffs = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(i) else { break };
                let Some(bytes) = std::fs::metadata(path).ok().filter(|m| m.len() <= max).and_then(|_| std::fs::read(path).ok()) else {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let name = path.file_name().and_then(|n| n.to_str());
                let workbook = bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]);
                for formats in [false, true] {
                    let opts = ReadOptions { formats, ..Default::default() };
                    let want = outcome(open_with(path, opts));
                    if formats && want.starts_with("error") {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                    let mut hints = vec![name];
                    if workbook {
                        hints.push(None);
                    }
                    for hint in hints {
                        let got = outcome(open_bytes_with(&bytes, hint, opts));
                        if got != want {
                            let at = got.bytes().zip(want.bytes()).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
                            let ctx = |s: &str| s.get(at.saturating_sub(40)..(at + 80).min(s.len())).unwrap_or("").to_string();
                            diffs.lock().unwrap().push(format!("{} (formats {formats}, hint {hint:?}): bytes {:?} vs file {:?}", path.display(), ctx(&got), ctx(&want)));
                        }
                    }
                }
                checked.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    let diffs = diffs.into_inner().unwrap();
    eprintln!(
        "{} files checked ({} errors, the same both ways), {} skipped, {} differences",
        checked.load(Ordering::Relaxed),
        errors.load(Ordering::Relaxed),
        skipped.load(Ordering::Relaxed),
        diffs.len()
    );
    for d in diffs.iter().take(20) {
        eprintln!("  {d}");
    }
    assert!(diffs.is_empty(), "{} files read differently from memory", diffs.len());
}
