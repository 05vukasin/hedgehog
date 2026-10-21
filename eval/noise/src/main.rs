//! `noise_eval`: robustness curves for hedgehog's `normalize` under independently designed noise.
//!
//! For every family × intensity × seed: build a sheet from a clean table with known content
//! (`families`), write it as xlsx, read it back with `hh_core::open`, normalize it in memory with
//! the rules only and with the learned model, and check the metamorphic invariants (`check`).
//! Writes a markdown report (default `eval/noise/REPORT.md`).
//!
//! ```text
//! noise_eval [--seeds 30] [--seed-from 0] [--families a,b] [--intensities 0,0.25,0.5,0.75,1] [--threads 2]
//!            [--batch-dir DIR]                         # write raw results (TSV) instead of a report
//! noise_eval --report-from DIR [--out eval/noise/REPORT.md]   # report from all batches in DIR
//!            [--out eval/noise/REPORT.md] [--work /tmp/claude-1000/w1noise] [--data $HEDGEHOG_DATA]
//! noise_eval --repro <family> <intensity> <seed>     # one scenario: sheet, truth, both paths
//! ```

mod base;
mod batch;
mod check;
mod families;
mod render;
mod report;
mod rng;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use check::Inv;
use families::Ctx;

thread_local! {
    static PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub struct Outcome {
    pub family: usize,
    pub ti: usize,
    pub seed: u64,
    /// Per path (0 rules, 1 model): Err(panic message) or the invariants.
    pub paths: [Result<BTreeMap<&'static str, Inv>, String>; 2],
    pub desc: String,
    pub ms: [f64; 2],
}

impl Outcome {
    pub fn pass(&self, path: usize) -> bool {
        matches!(&self.paths[path], Ok(m) if m.values().all(|i| i.pass()))
    }
}

struct Args {
    seeds: u64,
    seed_from: u64,
    batch_dir: Option<PathBuf>,
    report_from: Option<PathBuf>,
    families: Vec<String>,
    intensities: Vec<f64>,
    threads: usize,
    out: PathBuf,
    work: PathBuf,
    data: Option<PathBuf>,
    repro: Option<(String, f64, u64)>,
    keep_failures: usize,
}

fn parse_args() -> Args {
    let mut a = Args {
        seeds: 30,
        seed_from: 0,
        batch_dir: None,
        report_from: None,
        families: families::FAMILIES.iter().map(|s| s.to_string()).collect(),
        intensities: vec![0.0, 0.25, 0.5, 0.75, 1.0],
        threads: 2,
        out: PathBuf::from("eval/noise/REPORT.md"),
        work: PathBuf::from("/tmp/claude-1000/w1noise"),
        data: std::env::var_os("HEDGEHOG_DATA").map(PathBuf::from),
        repro: None,
        keep_failures: 2,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let v = |k: usize| args.get(i + k).cloned().unwrap_or_else(|| panic!("{} needs a value", args[i]));
        match args[i].as_str() {
            "--seeds" => a.seeds = v(1).parse().expect("--seeds N"),
            "--list" => {
                println!("{}", families::FAMILIES.join(" "));
                std::process::exit(0);
            }
            "--seed-from" => a.seed_from = v(1).parse().expect("--seed-from N"),
            "--batch-dir" => a.batch_dir = Some(PathBuf::from(v(1))),
            "--report-from" => a.report_from = Some(PathBuf::from(v(1))),
            "--families" => a.families = v(1).split(',').map(|s| s.to_string()).collect(),
            "--intensities" => a.intensities = v(1).split(',').map(|s| s.parse().expect("intensity")).collect(),
            "--threads" => a.threads = v(1).parse().expect("--threads N"),
            "--out" => a.out = PathBuf::from(v(1)),
            "--work" => a.work = PathBuf::from(v(1)),
            "--data" => a.data = Some(PathBuf::from(v(1))),
            "--keep-failures" => a.keep_failures = v(1).parse().expect("N"),
            "--repro" => {
                a.repro = Some((v(1), v(2).parse().expect("intensity"), v(3).parse().expect("seed")));
                i += 2;
            }
            x => panic!("unknown argument {x}"),
        }
        i += 2;
    }
    for f in &a.families {
        assert!(families::FAMILIES.contains(&f.as_str()), "unknown family {f}");
    }
    a
}

fn scenario_key(family: &str, t: f64, seed: u64) -> String {
    format!("{family}/{t:.2}/{seed}")
}

fn panicked<T>(r: std::thread::Result<T>) -> Result<T, String> {
    r.map_err(|e| {
        let loc = PANIC.with(|p| p.borrow_mut().take()).unwrap_or_default();
        let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
        format!("panic: {msg} {loc}")
    })
}

/// Builds, writes, reads and normalizes one scenario.
fn run_one(family: &str, t: f64, seed: u64, ctx: &Ctx, xlsx: &Path) -> (render::Canvas, Outcome, Option<hh_core::normalize::Normalized>) {
    let mut rng = rng::Rng::keyed(&scenario_key(family, t, seed));
    let fam = families::FAMILIES.iter().position(|f| *f == family).unwrap();
    let cv = match panicked(catch_unwind(AssertUnwindSafe(|| families::build(family, t, &mut rng, ctx)))) {
        Ok(cv) => cv,
        Err(e) => {
            let err = format!("generator {e}");
            return (
                render::Canvas::default(),
                Outcome { family: fam, ti: 0, seed, paths: [Err(err.clone()), Err(err)], desc: String::new(), ms: [0.0; 2] },
                None,
            );
        }
    };
    let desc = cv.desc.join("; ");
    let mut out = Outcome { family: fam, ti: 0, seed, paths: [Err(String::new()), Err(String::new())], desc, ms: [0.0; 2] };
    if let Err(e) = cv.write_xlsx(xlsx) {
        let err = format!("xlsx write failed: {e}");
        out.paths = [Err(err.clone()), Err(err)];
        return (cv, out, None);
    }
    let wb = match panicked(catch_unwind(|| hh_core::open(xlsx))) {
        Ok(Ok(wb)) => wb,
        Ok(Err(e)) => {
            let err = format!("read error: {e}");
            out.paths = [Err(err.clone()), Err(err)];
            return (cv, out, None);
        }
        Err(e) => {
            let err = format!("reader {e}");
            out.paths = [Err(err.clone()), Err(err)];
            return (cv, out, None);
        }
    };
    let mut last = None;
    for (p, rules_only) in [true, false].into_iter().enumerate() {
        let opts = hh_core::NormalizeOptions { rules_only, ..Default::default() };
        let t0 = Instant::now();
        let r = panicked(catch_unwind(AssertUnwindSafe(|| hh_core::normalize_workbook(&wb, &opts))));
        out.ms[p] = t0.elapsed().as_secs_f64() * 1000.0;
        out.paths[p] = match r {
            Ok(n) => {
                let inv = panicked(catch_unwind(AssertUnwindSafe(|| check::check(&cv, &n)))).map_err(|e| format!("checker {e}"));
                last = Some(n);
                inv
            }
            Err(e) => Err(format!("normalize {e}")),
        };
    }
    (cv, out, last)
}

fn repro(a: &Args, ctx: &Ctx) {
    let (family, t, seed) = a.repro.clone().unwrap();
    std::fs::create_dir_all(&a.work).unwrap();
    let xlsx = a.work.join(format!("{family}_{t:.2}_{seed}.xlsx"));
    let (cv, out, _) = run_one(&family, t, seed, ctx, &xlsx);
    println!("scenario {} → {}", scenario_key(&family, t, seed), xlsx.display());
    println!("{}\n", out.desc);
    print!("{}", cv.show());
    println!("\ntruth: {} data cells, {} total cells, {} non-data cells, {} source tables", cv.data.len(), cv.totals.len(), cv.nondata.len(), cv.tables.len());
    let wb = hh_core::open(&xlsx).ok();
    for (p, name) in ["rules", "model"].iter().enumerate() {
        println!("\n== {name} ({:.1} ms)", out.ms[p]);
        if let Some(wb) = &wb {
            let n = hh_core::normalize_workbook(wb, &hh_core::NormalizeOptions { rules_only: p == 0, ..Default::default() });
            for t in &n.tables {
                let cols: Vec<String> = t.columns.iter().map(|c| format!("{}:{}", c.name, c.dtype.as_str())).collect();
                println!(
                    "  T{} {}:{} header_top {} rows {} · {} rows · [{}]",
                    t.n,
                    hh_core::a1(t.rect.top, t.rect.left),
                    hh_core::a1(t.rect.bottom, t.rect.right),
                    t.header_top + 1,
                    t.header_rows,
                    t.rows(),
                    cols.join(", ")
                );
            }
            println!("  totals: {} · notes: {}", n.totals.len(), n.notes.len());
            for x in n.totals.iter().take(8) {
                println!(
                    "    total {} '{}' {} {} {} {} kept_in_data={}",
                    x.cell_a1(),
                    x.label.clone().unwrap_or_default(),
                    x.level,
                    x.direction,
                    x.status,
                    x.reason,
                    x.kept_in_data
                );
            }
            for x in n.notes.iter().take(8) {
                println!("    note {} {} {:?}", hh_core::a1(x.row, x.col), x.kind, x.text.chars().take(40).collect::<String>());
            }
        }
        match &out.paths[p] {
            Err(e) => println!("  CRASH {e}"),
            Ok(m) => {
                for (k, inv) in m {
                    if inv.applicable {
                        println!(
                            "  {:<13} {}  {}/{} failed  {}",
                            k,
                            if inv.pass() { "ok  " } else { "FAIL" },
                            inv.failed,
                            inv.items,
                            inv.first.clone().unwrap_or_default()
                        );
                    }
                }
            }
        }
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map(|l| format!("at {}:{}", l.file(), l.line())).unwrap_or_default();
        PANIC.with(|p| *p.borrow_mut() = Some(loc));
    }));
    let a = parse_args();
    if let Some(dir) = &a.report_from {
        let (results, ts, seeds, secs, real) = batch::load(dir);
        let md = report::render(&results, &ts, seeds, secs, &real, &a.out);
        std::fs::write(&a.out, md).unwrap();
        eprintln!("wrote {} ({} scenarios from {})", a.out.display(), results.len(), dir.display());
        return;
    }
    let real = match &a.data {
        Some(d) => base::load_real(&d.join("clean")),
        None => Vec::new(),
    };
    let ctx = Ctx { real };
    if a.repro.is_some() {
        repro(&a, &ctx);
        return;
    }
    let fail_dir = a.work.join("failures");
    std::fs::create_dir_all(&fail_dir).unwrap();
    let mut tasks: Vec<(usize, usize, u64)> = Vec::new();
    for f in &a.families {
        let fi = families::FAMILIES.iter().position(|x| x == f).unwrap();
        for ti in 0..a.intensities.len() {
            for s in a.seed_from..a.seed_from + a.seeds {
                tasks.push((fi, ti, s));
            }
        }
    }
    let t0 = Instant::now();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());
    let kept: Mutex<BTreeMap<(usize, usize), usize>> = Mutex::new(BTreeMap::new());
    std::thread::scope(|s| {
        for w in 0..a.threads.max(1) {
            let (tasks, next, results, kept, ctx, a, fail_dir) = (&tasks, &next, &results, &kept, &ctx, &a, &fail_dir);
            s.spawn(move || {
                let xlsx = a.work.join(format!("work_{w}.xlsx"));
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(fi, ti, seed)) = tasks.get(i) else { break };
                    let family = families::FAMILIES[fi];
                    let t = a.intensities[ti];
                    let (_, mut out, _) = run_one(family, t, seed, ctx, &xlsx);
                    out.ti = ti;
                    if !(out.pass(0) && out.pass(1)) {
                        let mut k = kept.lock().unwrap();
                        let n = k.entry((fi, ti)).or_default();
                        if *n < a.keep_failures {
                            *n += 1;
                            let _ = std::fs::copy(&xlsx, fail_dir.join(format!("{family}_{t:.2}_{seed}.xlsx")));
                        }
                    }
                    if i % 200 == 0 {
                        eprintln!("{i}/{} {:.0}s", tasks.len(), t0.elapsed().as_secs_f64());
                    }
                    results.lock().unwrap().push(out);
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|o| (o.family, o.ti, o.seed));
    let secs = t0.elapsed().as_secs_f64();
    let real_names: Vec<String> = ctx.real.iter().map(|r| r.file.clone()).collect();
    if let Some(dir) = &a.batch_dir {
        std::fs::create_dir_all(dir).unwrap();
        let name = format!("{}_{}_{}.tsv", a.families.join("+"), a.intensities.iter().map(|t| format!("{t}")).collect::<Vec<_>>().join("+"), a.seed_from);
        std::fs::write(dir.join(&name), batch::dump(&results, &a.intensities, secs, &real_names)).unwrap();
        eprintln!("wrote {} ({} scenarios in {secs:.1} s)", dir.join(name).display(), results.len());
        return;
    }
    let md = report::render(&results, &a.intensities, a.seeds, secs, &real_names, &a.out);
    std::fs::write(&a.out, md).unwrap();
    eprintln!("wrote {} ({} scenarios in {secs:.0} s)", a.out.display(), results.len());
}
