use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use hh_core::eval::{self, run::RunOptions, split::Split, Analyzer};

const USAGE: &str = "usage:
  hedgehog describe <file>
  hedgehog normalize <file> -o <out_dir> [--keep-totals] [--csv] [--locale TAG]
  hedgehog eval deco [--split dev|test|all] [--limit N] [--analyzer NAME] [--json OUT.json] [--no-json] [--progress]

eval: data root from $HEDGEHOG_DATA (default /home/vukasin/projects/excell-lib-project/testdata);
      JSON report defaults to $HEDGEHOG_SCORES or <data root>/../docs/research/scores/<analyzer>_deco_<split>.json
      analyzers: baseline";

/// Analyzer registry. The integrator adds `"layout" => Some(&|g: &Grid| layout::analyze(g))`.
fn analyzer(name: &str) -> Option<&'static dyn Analyzer> {
    match name {
        "baseline" => Some(&eval::baseline::analyze),
        "layout" => Some(&hh_core::layout::analyze),
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
        ["normalize", rest @ ..] => match normalize(rest) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}\n{USAGE}");
                ExitCode::FAILURE
            }
        },
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
            _ => return Err(format!("unknown argument `{a}`")),
        }
    }
    let an = analyzer(&analyzer_name).ok_or(format!("unknown analyzer `{analyzer_name}`"))?;

    let root = eval::data_root();
    let t = Instant::now();
    let corpus = eval::deco::load(&root).map_err(|e| e.to_string())?;
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;

    let mut report = eval::run::evaluate(&corpus, an, &analyzer_name, &opts);
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

/// `hedgehog normalize in.xlsx -o out/`: one clean typed sheet per table + manifest JSON.
fn normalize(args: &[&str]) -> Result<(), String> {
    let mut opts = hh_core::normalize::Options::default();
    let mut input: Option<&str> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match *a {
            "-o" | "--out" => out_dir = Some(PathBuf::from(val()?)),
            "--keep-totals" => opts.keep_totals = true,
            "--csv" => opts.csv = true,
            "--locale" => {
                let tag = val()?;
                opts.locale = Some(hh_core::values::Locale::parse(tag).ok_or(format!("unknown locale `{tag}`"))?);
            }
            s if s.starts_with('-') => return Err(format!("unknown argument `{s}`")),
            s if input.is_none() => input = Some(s),
            s => return Err(format!("unexpected argument `{s}`")),
        }
    }
    let input = input.ok_or("normalize needs an input file")?;
    let out_dir = out_dir.ok_or("normalize needs -o <out_dir>")?;
    let t = Instant::now();
    let out = hh_core::normalize_file(input, &out_dir, &opts).map_err(|e| e.to_string())?;
    let c = out.cells;
    println!(
        "{input}: {} tables, {} cells ({} in tables, {} totals, {} notes) in {:.0} ms",
        out.tables,
        c.total,
        c.in_tables,
        c.in_totals,
        c.in_notes,
        t.elapsed().as_secs_f64() * 1000.0
    );
    println!("  {}", out.xlsx_path.display());
    println!("  {}", out.manifest_path.display());
    for p in &out.csv_paths {
        println!("  {}", p.display());
    }
    Ok(())
}
