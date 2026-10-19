//! Corpus check for the value parsers (ignored by default; needs the local test corpora).
//!
//! Run with:
//!   HEDGEHOG_DATA=/path/to/testdata cargo test -p hh_core --test values_corpus -- --ignored --nocapture
//! Optional: HH_CORPUS_SAMPLE (Enron files, default 500), HH_CORPUS_MAX_MB (skip larger files, default 8).
//!
//! Corpus files are untrusted: they are only read through the workbook reader.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Instant;

use hh_core::values::{self, missing_token, parse_date, parse_number};
use hh_core::{open, Features, Grid, Value};

fn data_dir() -> PathBuf {
    std::env::var_os("HEDGEHOG_DATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/vukasin/projects/excell-lib-project/testdata"))
}

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn spreadsheet_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
        if matches!(ext.as_str(), "xlsx" | "xlsm" | "xls" | "xlsb" | "ods") {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Character-class shape: digits → 9, letters → a, whitespace → _, rest kept. Groups lookalikes.
fn shape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().take(24) {
        let k = if c.is_ascii_digit() {
            '9'
        } else if c.is_alphabetic() {
            'a'
        } else if c.is_whitespace() {
            '_'
        } else {
            c
        };
        // Collapse runs of the same class.
        if !(out.ends_with(k) && matches!(k, '9' | 'a' | '_')) {
            out.push(k);
        }
    }
    out
}

/// Text that a person would read as a number but the parsers rejected.
fn number_looking(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() || t.chars().count() > 30 {
        return false;
    }
    let non_space = t.chars().filter(|c| !c.is_whitespace()).count();
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    digits > 0 && digits * 2 >= non_space
}

#[derive(Default)]
struct Stats {
    files: usize,
    read_errors: usize,
    skipped_large: usize,
    panics: usize,
    sheets: usize,
    cells: usize,
    text_cells: usize,
    numeric_text: usize,
    date_text: usize,
    missing: usize,
    with_currency: usize,
    with_unit: usize,
    percent: usize,
    ambiguous_numbers: usize,
    unparsed_number_looking: usize,
    unparsed: HashMap<String, (usize, String)>,
    millis_compute: u128,
}

fn run_file(path: &Path, st: &mut Stats, max_bytes: u64) {
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > max_bytes {
        st.skipped_large += 1;
        return;
    }
    st.files += 1;
    let wb = match catch_unwind(|| open(path)) {
        Ok(Ok(wb)) => wb,
        _ => {
            st.read_errors += 1;
            return;
        }
    };
    for sheet in &wb.sheets {
        st.sheets += 1;
        st.cells += sheet.cells.len();
        let t0 = Instant::now();
        let res = catch_unwind(AssertUnwindSafe(|| {
            let grid = Grid::new(sheet);
            let mut f = Features::new(sheet.cells.len());
            values::compute(&grid, &mut f);
            (values::sheet_column_hints(&grid), f)
        }));
        st.millis_compute += t0.elapsed().as_millis();
        let Ok((hints, f)) = res else {
            st.panics += 1;
            eprintln!("PANIC in values::compute: {} / {}", path.display(), sheet.name);
            continue;
        };
        let num = f.get("v_numeric_text").unwrap();
        let date = f.get("v_date_text").unwrap();
        for (i, c) in sheet.cells.iter().enumerate() {
            let Some(Value::Text(s)) = &c.value else { continue };
            st.text_cells += 1;
            let h = hints.get(&c.col).copied().unwrap_or_default();
            if num[i] == 1.0 {
                st.numeric_text += 1;
                if let Some(p) = parse_number(s, h.number_hint()) {
                    st.with_currency += p.currency.is_some() as usize;
                    st.with_unit += p.unit.is_some() as usize;
                    st.percent += p.percent as usize;
                    st.ambiguous_numbers += p.ambiguous as usize;
                }
                continue;
            }
            if date[i] == 1.0 {
                st.date_text += 1;
                continue;
            }
            if missing_token(s).is_some() {
                st.missing += 1;
                continue;
            }
            if number_looking(s) && parse_date(s, h.date_order).is_none() {
                st.unparsed_number_looking += 1;
                let e = st.unparsed.entry(shape(s.trim())).or_insert((0, s.trim().chars().take(40).collect()));
                e.0 += 1;
            }
        }
    }
}

fn report(name: &str, st: &Stats) {
    let pct = |a: usize| if st.text_cells == 0 { 0.0 } else { 100.0 * a as f64 / st.text_cells as f64 };
    println!("== {name}");
    println!(
        "files {} (read errors {}, skipped >max size {}), sheets {}, cells {}, compute {} ms, panics {}",
        st.files, st.read_errors, st.skipped_large, st.sheets, st.cells, st.millis_compute, st.panics
    );
    println!(
        "text cells {}: numeric_text {} ({:.2}%), date_text {} ({:.2}%), missing tokens {} ({:.2}%), unparsed number-looking {} ({:.2}%)",
        st.text_cells,
        st.numeric_text,
        pct(st.numeric_text),
        st.date_text,
        pct(st.date_text),
        st.missing,
        pct(st.missing),
        st.unparsed_number_looking,
        pct(st.unparsed_number_looking)
    );
    println!(
        "  numeric text with currency {}, unit {}, percent {}, ambiguous separator {}",
        st.with_currency, st.with_unit, st.percent, st.ambiguous_numbers
    );
    let mut top: Vec<_> = st.unparsed.iter().collect();
    top.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
    println!("  top unparsed number-looking shapes (shape, count, example):");
    for (shape, (n, ex)) in top.into_iter().take(25) {
        println!("    {shape:<20} {n:>7}  {ex:?}");
    }
}

#[test]
#[ignore = "needs local corpora; run with --ignored --nocapture"]
fn corpus_values() {
    let data = data_dir();
    let sample = env_num("HH_CORPUS_SAMPLE", 500);
    let max_bytes = env_num("HH_CORPUS_MAX_MB", 8) as u64 * 1024 * 1024;

    let enron = spreadsheet_files(&data.join("irregular/enron"));
    let step = (enron.len() / sample.max(1)).max(1);
    let mut st = Stats::default();
    // Deterministic sample: every `step`-th file of the sorted listing, offset by 7.
    for p in enron.iter().skip(7.min(enron.len())).step_by(step).take(sample) {
        run_file(p, &mut st, max_bytes);
    }
    report(&format!("enron sample ({} of {})", sample.min(enron.len()), enron.len()), &st);
    let mut total_panics = st.panics;

    let mut sr = Stats::default();
    for dir in ["irregular/serbia_data_gov_rs", "clean/data_gov_rs"] {
        for p in spreadsheet_files(&data.join(dir)) {
            run_file(&p, &mut sr, max_bytes);
        }
    }
    report("serbian (serbia_data_gov_rs + data_gov_rs)", &sr);
    total_panics += sr.panics;
    assert_eq!(total_panics, 0, "values::compute panicked");
}
