//! Corpus check for the totals module on a random sample of real files (Enron by default).
//! Ignored by default; run with
//! `cargo test -p hh_core --test totals_corpus -- --ignored --nocapture`
//! Env: `HEDGEHOG_ENRON` (directory), `TOTALS_SAMPLE` (number of files, default 500), `TOTALS_SEED`.
//! Files are untrusted and only read through `hh_core::open`.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use hh_core::totals::{self, detect_with, verify, DependencyIndex, FormulaKind};
use hh_core::{a1, open, Features, Grid, Value};

const DEFAULT_DIR: &str = "/home/vukasin/projects/excell-lib-project/testdata/irregular/enron";

#[derive(Default)]
struct Stats {
    files: usize,
    open_failed: usize,
    our_panics: usize,
    sheets: usize,
    sheets_with_totals: usize,
    formulas: usize,
    formulas_with_refs: usize,
    formulas_with_error: usize,
    kinds: BTreeMap<String, usize>,
    totals: usize,
    by_origin: BTreeMap<String, usize>,
    by_level: BTreeMap<String, usize>,
    by_direction: BTreeMap<String, usize>,
    ok: usize,
    wrong: usize,
    unknown: usize,
    reasons: BTreeMap<String, usize>,
    examples: Vec<String>,
    /// file → [typed vertical, typed horizontal, formula] × [ok, wrong, unknown]
    per_file: BTreeMap<String, [[usize; 3]; 3]>,
}

fn label_text(sheet: &hh_core::Sheet, pos: Option<(u32, u32)>) -> String {
    let Some((r, c)) = pos else { return String::new() };
    sheet
        .cells
        .iter()
        .find(|x| (x.row, x.col) == (r, c))
        .and_then(|x| match &x.value {
            Some(Value::Text(s)) => Some(s.chars().take(30).collect()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
#[ignore]
fn corpus_totals() {
    let dir = std::env::var("HEDGEHOG_ENRON").unwrap_or_else(|_| DEFAULT_DIR.to_string());
    let n: usize = std::env::var("TOTALS_SAMPLE").ok().and_then(|s| s.parse().ok()).unwrap_or(500);
    let mut seed: u64 = std::env::var("TOTALS_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(42);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("corpus dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            matches!(ext.as_str(), "xlsx" | "xlsm" | "xls") && p.metadata().map(|m| m.len() < 30_000_000).unwrap_or(false)
        })
        .collect();
    files.sort();
    // Fisher–Yates with xorshift, deterministic per seed.
    for i in (1..files.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        files.swap(i, (seed % (i as u64 + 1)) as usize);
    }
    files.truncate(n);

    let start = Instant::now();
    let mut st = Stats::default();
    for path in &files {
        st.files += 1;
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let wb = match catch_unwind(|| open(path)) {
            Ok(Ok(wb)) => wb,
            _ => {
                st.open_failed += 1;
                continue;
            }
        };
        for sheet in &wb.sheets {
            st.sheets += 1;
            let res = catch_unwind(AssertUnwindSafe(|| {
                let grid = Grid::new(sheet);
                let deps = DependencyIndex::new(&grid);
                for f in deps.formulas() {
                    st.formulas += 1;
                    st.formulas_with_refs += !f.parsed.refs.is_empty() as usize;
                    st.formulas_with_error += f.parsed.has_error as usize;
                    *st.kinds.entry(format!("{:?}", f.parsed.kind)).or_default() += 1;
                }
                let cands = detect_with(&grid, &deps);
                st.sheets_with_totals += !cands.is_empty() as usize;
                for c in &cands {
                    st.totals += 1;
                    *st.by_origin.entry(format!("{:?}", c.origin)).or_default() += 1;
                    *st.by_level.entry(format!("{:?}", c.level)).or_default() += 1;
                    *st.by_direction.entry(format!("{:?}", c.direction)).or_default() += 1;
                    let chk = verify(&grid, c);
                    let kind = match (c.origin, c.direction) {
                        (hh_core::totals::TotalOrigin::Typed, hh_core::totals::Direction::Vertical) => 0,
                        (hh_core::totals::TotalOrigin::Typed, _) => 1,
                        _ => 2,
                    };
                    let res = match chk.ok { Some(true) => 0, Some(false) => 1, None => 2 };
                    st.per_file.entry(name.clone()).or_default()[kind][res] += 1;
                    *st.reasons.entry(format!("{:?}", chk.reason)).or_default() += 1;
                    match chk.ok {
                        Some(true) => st.ok += 1,
                        Some(false) => {
                            st.wrong += 1;
                            let typed = c.origin == hh_core::totals::TotalOrigin::Typed;
                            if st.examples.iter().filter(|e| e.contains("| Typed ") == typed).count() < 20 && !st.examples.iter().any(|e| e.starts_with(&format!("{name} | {} |", sheet.name))) {
                                let formula = grid.cell(c.row, c.col).and_then(|x| x.formula.clone()).unwrap_or_default();
                                st.examples.push(format!(
                                    "{name} | {} | {} | {:?} {:?} | label {:?} | expected {:?} actual {:?} | {:?} | {}",
                                    sheet.name,
                                    a1(c.row, c.col),
                                    c.origin,
                                    c.level,
                                    label_text(sheet, c.label),
                                    chk.expected,
                                    chk.actual,
                                    chk.reason,
                                    formula
                                ));
                            }
                        }
                        None => st.unknown += 1,
                    }
                }
                let mut feats = Features::new(sheet.cells.len());
                totals::compute(&grid, &mut feats);
            }));
            if res.is_err() {
                st.our_panics += 1;
                eprintln!("PANIC in totals on {name} / {}", sheet.name);
            }
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let pct = |a: usize, b: usize| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
    println!("== totals corpus check: {dir}");
    println!("files {} (open failed {}), sheets {}, our panics {}, {:.1}s", st.files, st.open_failed, st.sheets, st.our_panics, secs);
    println!(
        "formulas {}: with refs {:.1}%, with error literal {:.1}%, recognised aggregate/ref {:.1}%",
        st.formulas,
        pct(st.formulas_with_refs, st.formulas),
        pct(st.formulas_with_error, st.formulas),
        pct(st.formulas - st.kinds.get(&format!("{:?}", FormulaKind::Other)).copied().unwrap_or(0), st.formulas)
    );
    println!("formula kinds {:?}", st.kinds);
    println!(
        "totals {} on {} sheets ({:.1}% of sheets; {:.1} per sheet with totals)",
        st.totals,
        st.sheets_with_totals,
        pct(st.sheets_with_totals, st.sheets),
        st.totals as f64 / st.sheets_with_totals.max(1) as f64
    );
    println!("origin {:?} level {:?} direction {:?}", st.by_origin, st.by_level, st.by_direction);
    println!(
        "verified ok {} ({:.1}%), wrong {} ({:.1}%), unknown {} ({:.1}%)",
        st.ok,
        pct(st.ok, st.totals),
        st.wrong,
        pct(st.wrong, st.totals),
        st.unknown,
        pct(st.unknown, st.totals)
    );
    println!("reasons {:?}", st.reasons);
    let mut top: Vec<_> = st.per_file.iter().collect();
    top.sort_by_key(|(_, v)| std::cmp::Reverse(v.iter().map(|k| k[1]).sum::<usize>()));
    println!("-- files with most mismatches ([typed-vertical, typed-horizontal, formula] x [ok, wrong, unknown]):");
    for (f, v) in top.iter().take(15) {
        println!("{f}: {v:?}");
    }
    let mut kinds = [[0usize; 3]; 3];
    for v in st.per_file.values() {
        for k in 0..3 {
            for r in 0..3 {
                kinds[k][r] += v[k][r];
            }
        }
    }
    println!("by kind [typed-vertical, typed-horizontal, formula] x [ok, wrong, unknown]: {kinds:?}");
    println!("-- mismatch examples:");
    for e in &st.examples {
        println!("{e}");
    }
    assert_eq!(st.our_panics, 0);
}
