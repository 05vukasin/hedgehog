//! Golden check for performance work: every output of every stage (cells, merges, all feature
//! columns, totals detect + verify, baseline layout) on the 50 files of `core/benches/enron50.txt`
//! must hash to the values in `core/benches/golden_enron50.txt`.
//!
//! `cargo test --release -p hh_core --test perf_golden -- --ignored --nocapture`
//! (same check: `hedgehog golden core/benches/enron50.txt --check core/benches/golden_enron50.txt`).
//! Skipped when the corpus is missing. When a workstream changes outputs on purpose, regenerate
//! the golden file with `hedgehog golden core/benches/enron50.txt > core/benches/golden_enron50.txt`.

use std::path::PathBuf;

use hh_core::eval::{bench, data_root};

#[test]
#[ignore]
fn outputs_match_golden() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches");
    let files = bench::resolve_inputs(&dir.join("enron50.txt")).unwrap();
    if !files.iter().all(|p| p.exists()) {
        eprintln!("skipped: corpus not found under {}", data_root().display());
        return;
    }
    let golden = std::fs::read_to_string(dir.join("golden_enron50.txt")).unwrap();
    let root = data_root();
    let mut bad = Vec::new();
    for (path, want) in files.iter().zip(golden.lines()) {
        let name = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        let got = match bench::run_file(path, true) {
            Ok(r) => format!("{:016x}\t{}\t{name}", r.hash, r.cells),
            Err(e) => format!("error\t0\t{name}\t{}", e.to_string().replace(['\t', '\n'], " ")),
        };
        if got != want {
            bad.push(format!("got  {got}\nwant {want}"));
        }
    }
    assert_eq!(golden.lines().count(), files.len(), "golden file and list differ in length");
    assert!(bad.is_empty(), "{} files differ:\n{}", bad.len(), bad.join("\n"));
}
