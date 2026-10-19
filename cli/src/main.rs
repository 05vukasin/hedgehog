use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use hh_core::eval::{self, run::RunOptions, split::Split, Analyzer};

mod bench;

fn report(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage:
  hedgehog describe <file>
  hedgehog eval deco [--split dev|test|all] [--limit N] [--analyzer NAME] [--json OUT.json] [--no-json] [--progress] [--threads N]
  hedgehog bench <dir|filelist> [--limit N] [--threads N] [--top K] [--open-only]
  hedgehog bench --synthetic [--case NAME]
  hedgehog golden <dir|filelist> [--limit N] [--check GOLDEN.txt]

eval: files are scored on --threads workers (default: min(4, CPUs); results identical to 1 thread);
      data root from $HEDGEHOG_DATA (default /home/vukasin/projects/excell-lib-project/testdata);
      JSON report defaults to $HEDGEHOG_SCORES or <data root>/../docs/research/scores/<analyzer>_deco_<split>.json
      analyzers: baseline";

/// Analyzer registry. The integrator adds `"layout" => Some(&|g: &Grid| layout::analyze(g))`.
fn analyzer(name: &str) -> Option<&'static dyn Analyzer> {
    match name {
        "baseline" => Some(&eval::baseline::analyze),
        _ => None,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["describe", path] => match hh_core::open(path) {
            Ok(wb) => {
                print!("{}", hh_core::describe(&wb));
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{path}: {e}");
                ExitCode::FAILURE
            }
        },
        ["bench", rest @ ..] => report(bench::bench(rest)),
        ["golden", rest @ ..] => report(bench::golden(rest)),
        ["eval", "deco", rest @ ..] => match eval_deco(rest) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}\n{USAGE}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn eval_deco(args: &[&str]) -> Result<(), String> {
    let mut opts = RunOptions::default();
    let mut analyzer_name = "baseline".to_string();
    let mut json: Option<PathBuf> = None;
    let mut write_json = true;
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(4);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match *a {
            "--split" => opts.split = val()?.parse::<Split>()?,
            "--limit" => opts.limit = Some(val()?.parse().map_err(|_| "--limit needs a number")?),
            "--analyzer" => analyzer_name = val()?.to_string(),
            "--json" => json = Some(PathBuf::from(val()?)),
            "--no-json" => write_json = false,
            "--progress" => opts.progress = true,
            "--threads" => threads = val()?.parse::<usize>().map_err(|_| "--threads needs a number")?.max(1),
            _ => return Err(format!("unknown argument `{a}`")),
        }
    }
    let an = analyzer(&analyzer_name).ok_or(format!("unknown analyzer `{analyzer_name}`"))?;

    let root = eval::data_root();
    let t = Instant::now();
    let corpus = eval::deco::load(&root).map_err(|e| e.to_string())?;
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;

    let mut report = eval::run::evaluate_parallel(&corpus, an, &analyzer_name, &opts, threads);
    drop(corpus);
    report.timing.load_ms = load_ms;
    print!("{}", report.scoreboard());

    if write_json {
        let path = json.unwrap_or_else(|| {
            let dir = std::env::var_os("HEDGEHOG_SCORES")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| root.parent().unwrap_or(&root).join("docs").join("research").join("scores"));
            let lim = opts.limit.map(|l| format!("_n{l}")).unwrap_or_default();
            dir.join(format!("{analyzer_name}_deco_{}{lim}.json", opts.split))
        });
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, report.to_json()).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("report: {}", path.display());
    }
    Ok(())
}
