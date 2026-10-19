//! `hedgehog bench` and `hedgehog golden`: throughput / per-stage timing and output hashes over a
//! directory or a file list (see `hh_core::eval::bench`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use hh_core::eval::bench::{self, synth, FileRun, StageTimes, STAGES};

struct Opts {
    input: Option<PathBuf>,
    limit: Option<usize>,
    threads: usize,
    top: usize,
    check: Option<PathBuf>,
    synthetic: bool,
    case: Option<String>,
    open_only: bool,
}

fn parse(args: &[&str]) -> Result<Opts, String> {
    let mut o = Opts { input: None, limit: None, threads: 1, top: 5, check: None, synthetic: false, case: None, open_only: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match *a {
            "--limit" => o.limit = Some(val()?.parse().map_err(|_| "--limit needs a number")?),
            "--threads" => o.threads = val()?.parse::<usize>().map_err(|_| "--threads needs a number")?.max(1),
            "--top" => o.top = val()?.parse().map_err(|_| "--top needs a number")?,
            "--check" => o.check = Some(PathBuf::from(val()?)),
            "--synthetic" => o.synthetic = true,
            "--open-only" => o.open_only = true,
            "--case" => o.case = Some(val()?.to_string()),
            s if !s.starts_with("--") && o.input.is_none() => o.input = Some(PathBuf::from(s)),
            _ => return Err(format!("unknown argument `{a}`")),
        }
    }
    Ok(o)
}

fn inputs(o: &Opts) -> Result<Vec<PathBuf>, String> {
    let input = o.input.as_ref().ok_or("missing <dir|filelist>")?;
    let mut files = bench::resolve_inputs(input).map_err(|e| format!("{}: {e}", input.display()))?;
    if let Some(l) = o.limit {
        files.truncate(l);
    }
    Ok(files)
}

/// Opens `path` only (the `open` stage).
fn open_only(path: &Path) -> Result<FileRun, String> {
    let t = Instant::now();
    let wb = hh_core::open(path).map_err(|e| e.to_string())?;
    let mut r = FileRun { sheets: wb.sheets.len(), ..Default::default() };
    for s in &wb.sheets {
        r.cells += s.cells.len();
        r.merges += s.merges.len();
        r.formulas += s.formula_count();
    }
    drop(wb);
    r.times.0[0] = t.elapsed().as_secs_f64();
    Ok(r)
}

/// Runs the pipeline (or only `open`) over `files` on `threads` workers; results in input order.
fn run_all(files: &[PathBuf], threads: usize, golden: bool, open: bool) -> Vec<Result<FileRun, String>> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<Result<FileRun, String>>>> = Mutex::new(vec![None; files.len()].into_iter().collect());
    std::thread::scope(|s| {
        for _ in 0..threads.min(files.len().max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let r = if open { open_only(path) } else { bench::run_file(path, golden).map_err(|e| e.to_string()) };
                    out.lock().unwrap()[i] = Some(r);
                }
            });
        }
    });
    out.into_inner().unwrap().into_iter().map(|r| r.unwrap_or_else(|| Err("not run".into()))).collect()
}

fn mb(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

pub fn bench(args: &[&str]) -> Result<(), String> {
    let o = parse(args)?;
    if o.synthetic {
        return synthetic(&o);
    }
    let files = inputs(&o)?;
    let wall = Instant::now();
    let results = run_all(&files, o.threads, false, o.open_only);
    let wall = wall.elapsed().as_secs_f64();

    let mut times = StageTimes::default();
    let (mut ok, mut sheets, mut cells, mut merges, mut formulas) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut per_file: Vec<(f64, usize, &Path)> = Vec::new();
    let mut failed = Vec::new();
    for (path, r) in files.iter().zip(&results) {
        match r {
            Ok(r) => {
                ok += 1;
                sheets += r.sheets;
                cells += r.cells;
                merges += r.merges;
                formulas += r.formulas;
                times.add(&r.times);
                per_file.push((r.times.total(), r.cells, path));
            }
            Err(e) => failed.push((path, e)),
        }
    }
    println!(
        "hedgehog bench · {} files ({} ok, {} failed) · {} threads · {sheets} sheets · {cells} cells · {merges} merges · {formulas} formulas",
        files.len(),
        ok,
        failed.len(),
        o.threads
    );
    println!(
        "wall {:.2}s · {:.2} files/s · {:.0} cells/s · peak RSS {:.0} MB",
        wall,
        ok as f64 / wall,
        cells as f64 / wall,
        bench::peak_rss().map_or(f64::NAN, mb)
    );
    let total = times.total();
    println!("stage            cpu s   share");
    for (name, t) in STAGES.iter().zip(times.0) {
        println!("  {name:<14} {t:7.3}  {:5.1}%", 100.0 * t / total.max(1e-12));
    }
    println!("  {:<14} {total:7.3}", "sum");
    per_file.sort_by(|a, b| b.0.total_cmp(&a.0));
    if o.top > 0 {
        println!("slowest files:");
        for (t, c, p) in per_file.iter().take(o.top) {
            println!("  {t:7.3}s {c:>8} cells  {}", p.file_name().map(|f| f.to_string_lossy()).unwrap_or_default());
        }
    }
    for (p, e) in failed {
        println!("failed: {}: {e}", p.display());
    }
    Ok(())
}

fn synthetic(o: &Opts) -> Result<(), String> {
    println!("case                    cells  merges       grid    context     values     totals  det+ver   baseline   peak RSS");
    for (name, sheet) in synth::all() {
        if o.case.as_deref().is_some_and(|c| c != name) || (o.case.is_none() && name == "giant_merge") {
            continue; // TEMP: giant_merge explodes the pre-optimization Grid
        }
        let (t, _) = bench::run_sheet(&sheet, false);
        let ms = |i: usize| t.0[i] * 1e3;
        println!(
            "{name:<20} {:>8} {:>7} {:8.1}ms {:8.1}ms {:8.1}ms {:8.1}ms {:6.1}ms {:8.1}ms {:7.0} MB",
            sheet.cells.len(),
            sheet.merges.len(),
            ms(1),
            ms(2),
            ms(3),
            ms(4),
            ms(5),
            ms(6),
            bench::peak_rss().map_or(f64::NAN, mb)
        );
    }
    Ok(())
}

pub fn golden(args: &[&str]) -> Result<(), String> {
    let o = parse(args)?;
    let files = inputs(&o)?;
    let results = run_all(&files, o.threads, true, false);
    let root = hh_core::eval::data_root();
    let mut lines = Vec::new();
    for (path, r) in files.iter().zip(&results) {
        let name = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        lines.push(match r {
            Ok(r) => format!("{:016x}\t{}\t{name}", r.hash, r.cells),
            Err(e) => format!("error\t0\t{name}\t{}", e.replace(['\t', '\n'], " ")),
        });
    }
    match &o.check {
        None => {
            for l in &lines {
                println!("{l}");
            }
            Ok(())
        }
        Some(g) => {
            let want = std::fs::read_to_string(g).map_err(|e| format!("{}: {e}", g.display()))?;
            let want: Vec<&str> = want.lines().filter(|l| !l.is_empty()).collect();
            let mut bad = 0;
            for (i, l) in lines.iter().enumerate() {
                if want.get(i) != Some(&l.as_str()) {
                    bad += 1;
                    println!("DIFF: got  {l}\n      want {}", want.get(i).unwrap_or(&"<missing>"));
                }
            }
            if want.len() != lines.len() {
                bad += 1;
                println!("DIFF: {} lines, golden has {}", lines.len(), want.len());
            }
            if bad == 0 {
                println!("golden OK: {} files identical", lines.len());
                Ok(())
            } else {
                Err(format!("golden check failed: {bad} differences"))
            }
        }
    }
}
