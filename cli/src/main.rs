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
  hedgehog normalize <file> -o <out_dir> [--keep-totals] [--csv] [--locale TAG]
  hedgehog eval deco [--split dev|test|all] [--limit N] [--analyzer NAME] [--json OUT.json] [--no-json] [--progress] [--threads N]
  hedgehog eval synthetic --dir DIR [--analyzer NAME] [--limit N] [--values] [--json OUT.json] [--no-json] [--progress]
  hedgehog bench <dir|filelist> [--limit N] [--threads N] [--top K] [--open-only]
  hedgehog bench --synthetic [--case NAME]
  hedgehog golden <dir|filelist> [--limit N] [--check GOLDEN.txt]
  hedgehog benchmark [--corpora deco,statsheets,strudel,synthetic] [--analyzer NAME] [--split test|dev|all] [--threads N] [--out FILE.tsv]
  hedgehog features --out FILE.csv [--corpus deco,synthetic,statsheets,strudel] [--dir DIR] [--split dev|test|all] [--data-keep F] [--limit N] [--level cell|row]

eval: files are scored on --threads workers (default: min(4, CPUs); results identical to 1 thread);
      data root from $HEDGEHOG_DATA (default ../testdata);
      JSON report defaults to $HEDGEHOG_SCORES or <data root>/../docs/research/scores/<analyzer>_deco_<split>.json
      analyzers: baseline, layout, model (cell roles from the trees in $HEDGEHOG_MODEL)";

/// Analyzer registry. The integrator adds `"layout" => Some(&|g: &Grid| layout::analyze(g))`.
fn analyzer(name: &str) -> Option<&'static dyn Analyzer> {
    match name {
        "baseline" => Some(&eval::baseline::analyze),
        "layout" => Some(&hh_core::layout::analyze),
        "model" => Some(&model_analyze),
        _ => None,
    }
}

/// The learned model named by `$HEDGEHOG_MODEL` (an `hh-trees-1` JSON file), loaded once.
fn model() -> &'static hh_core::learned::TreeModel {
    static MODEL: std::sync::OnceLock<hh_core::learned::TreeModel> = std::sync::OnceLock::new();
    MODEL.get_or_init(|| {
        let path = std::env::var("HEDGEHOG_MODEL").unwrap_or_else(|_| {
            eprintln!("the `model` analyzer needs HEDGEHOG_MODEL=<model.json>");
            std::process::exit(2)
        });
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(2)
        });
        hh_core::learned::TreeModel::from_json(&text).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(2)
        })
    })
}

fn model_analyze(g: &hh_core::Grid) -> hh_core::SheetLayout {
    hh_core::learned::analyze(g, model())
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
        ["normalize", rest @ ..] => report(normalize(rest)),
        ["bench", rest @ ..] => report(bench::bench(rest)),
        ["golden", rest @ ..] => report(bench::golden(rest)),
        ["eval", "synthetic", rest @ ..] => report(eval_synthetic(rest)),
        ["features", rest @ ..] => report(features(rest)),
        ["benchmark", rest @ ..] => report(benchmark(rest)),
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

/// Scores an analyzer on a generated corpus (`tools/noise`): the standard report plus a breakdown
/// per profile, table-boundary operator and noise operator, and optionally the value parsers.
fn eval_synthetic(args: &[&str]) -> Result<(), String> {
    let mut opts = RunOptions { split: Split::All, ..RunOptions::default() };
    let mut analyzer_name = "baseline".to_string();
    let mut dir: Option<PathBuf> = None;
    let mut json: Option<PathBuf> = None;
    let mut write_json = true;
    let mut values = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match *a {
            "--dir" => dir = Some(PathBuf::from(val()?)),
            "--split" => opts.split = val()?.parse::<Split>()?,
            "--limit" => opts.limit = Some(val()?.parse().map_err(|_| "--limit needs a number")?),
            "--analyzer" => analyzer_name = val()?.to_string(),
            "--values" => values = true,
            "--json" => json = Some(PathBuf::from(val()?)),
            "--no-json" => write_json = false,
            "--progress" => opts.progress = true,
            _ => return Err(format!("unknown argument `{a}`")),
        }
    }
    let dir = dir.unwrap_or_else(|| eval::data_root().join("synthetic").join("v0"));
    let an = analyzer(&analyzer_name).ok_or(format!("unknown analyzer `{analyzer_name}`"))?;
    let t = Instant::now();
    let corpus = eval::synthetic::load(&dir).map_err(|e| e.to_string())?;
    let load_ms = t.elapsed().as_secs_f64() * 1000.0;
    let mut report = eval::run::evaluate(&corpus, an, &analyzer_name, &opts);
    drop(corpus);
    report.timing.load_ms = load_ms;
    print!("{}", report.scoreboard());
    let bopts = eval::synthetic::BreakdownOptions { limit: opts.limit, values };
    let bd = eval::synthetic::breakdown(&dir, an, &bopts).map_err(|e| e.to_string())?;
    print!("{}", bd.text());
    if write_json {
        let lim = opts.limit.map(|l| format!("_n{l}")).unwrap_or_default();
        let path = json.unwrap_or_else(|| dir.join("scores").join(format!("{analyzer_name}_synthetic{lim}.json")));
        if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let body = report.to_json();
        let merged = format!("{},\"breakdown\":{}}}", body.trim_end().trim_end_matches('}'), bd.to_json());
        std::fs::write(&path, merged).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("report: {}", path.display());
    }
    Ok(())
}

/// Training data for the learned cell-role model: one CSV row per labelled cell (see
/// `hh_core::eval::export`).
fn features(args: &[&str]) -> Result<(), String> {
    use std::io::Write as _;
    let mut out: Option<PathBuf> = None;
    let mut corpora = vec!["deco".to_string(), "synthetic".to_string()];
    let mut dir: Option<PathBuf> = None;
    let mut rows = false;
    let mut opts = eval::export::ExportOptions {
        source: String::new(),
        split: Split::All,
        data_keep: 0.1,
        limit: None,
        progress: true,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match *a {
            "--out" | "-o" => out = Some(PathBuf::from(val()?)),
            "--corpus" => corpora = val()?.split(',').map(str::to_string).collect(),
            "--dir" => dir = Some(PathBuf::from(val()?)),
            "--split" => opts.split = val()?.parse::<Split>()?,
            "--data-keep" => opts.data_keep = val()?.parse().map_err(|_| "--data-keep needs a number")?,
            "--limit" => opts.limit = Some(val()?.parse().map_err(|_| "--limit needs a number")?),
            "--level" => rows = val()? == "row",
            _ => return Err(format!("unknown argument `{a}`")),
        }
    }
    let out = out.ok_or("features needs --out FILE.csv")?;
    let file = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut w = std::io::BufWriter::new(file);
    let root = eval::data_root();
    let t = Instant::now();
    for (k, name) in corpora.iter().enumerate() {
        let corpus = match name.as_str() {
            "deco" => eval::deco::load(&root),
            "synthetic" => eval::synthetic::load(&dir.clone().unwrap_or_else(|| root.join("synthetic").join("v0"))),
            "statsheets" => eval::statsheets::load(&root),
            "strudel" => eval::labelled::load(&root.join("irregular").join("strudel_xlsx"), "strudel"),
            other => return Err(format!("unknown corpus `{other}` (deco, synthetic, statsheets, strudel)")),
        }
        .map_err(|e| e.to_string())?;
        opts.source = name.clone();
        eprintln!("{name}: {} files", corpus.files.len());
        let st = if rows {
            eval::export_rows::export(&corpus, &opts, &mut w, k == 0)
        } else {
            eval::export::export(&corpus, &opts, &mut w, k == 0)
        }
        .map_err(|e| e.to_string())?;
        let roles: Vec<String> =
            hh_core::Role::ALL.iter().zip(st.per_role).map(|(r, n)| format!("{}={n}", r.as_str())).collect();
        println!(
            "{name}: {} files ({} failed), {} sheets, {} rows [{}]",
            st.files,
            st.failed,
            st.sheets,
            st.rows,
            roles.join(" ")
        );
    }
    w.flush().map_err(|e| e.to_string())?;
    println!("wrote {} in {:.1} s", out.display(), t.elapsed().as_secs_f64());
    Ok(())
}

/// Cross-corpus scores per source (`hh_core::eval::benchmark`). DECO and synthetic use `--split`
/// (the model trains on their dev split); StatSheets and Strudel are external and scored whole.
fn benchmark(args: &[&str]) -> Result<(), String> {
    let mut corpora = vec!["deco".to_string(), "statsheets".into(), "strudel".into(), "synthetic".into()];
    let mut analyzer_name = "layout".to_string();
    let mut split = Split::Test;
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(4);
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().copied().ok_or(format!("{a} needs a value"));
        match *a {
            "--corpora" => corpora = val()?.split(',').map(str::to_string).collect(),
            "--analyzer" => analyzer_name = val()?.to_string(),
            "--split" => split = val()?.parse::<Split>()?,
            "--threads" => threads = val()?.parse::<usize>().map_err(|_| "--threads needs a number")?.max(1),
            "--out" => out = Some(PathBuf::from(val()?)),
            _ => return Err(format!("unknown argument `{a}`")),
        }
    }
    let an = analyzer(&analyzer_name).ok_or(format!("unknown analyzer `{analyzer_name}`"))?;
    let root = eval::data_root();
    let mut tsv = String::from("corpus\tsource\tfiles\tsheets\tcells\tfull_pooled\tfull_file\tcoarse_pooled\tcoarse_file\taccuracy\n");
    for name in &corpora {
        let t = Instant::now();
        let (corpus, sp) = match name.as_str() {
            "deco" => (eval::deco::load(&root), split),
            "synthetic" => (eval::synthetic::load(&root.join("synthetic").join("v0")), split),
            "statsheets" => (eval::statsheets::load(&root), Split::All),
            "strudel" => (eval::labelled::load(&root.join("irregular").join("strudel_xlsx"), "strudel"), Split::All),
            other => return Err(format!("unknown corpus `{other}`")),
        };
        let corpus = corpus.map_err(|e| e.to_string())?;
        for n in &corpus.notes {
            eprintln!("{name}: {n}");
        }
        let opts = RunOptions { split: sp, ..RunOptions::default() };
        let report = eval::run::evaluate_parallel(&corpus, an, &analyzer_name, &opts, threads);
        let rows = eval::benchmark::by_source(&report, corpus.coarse_roles);
        print!("{}", eval::benchmark::table(&format!("{name} ({sp})"), &analyzer_name, &rows));
        println!("  time {:.1} s (analyze {:.1} s)\n", t.elapsed().as_secs_f64(), report.timing.analyze_ms / 1000.0);
        for r in &rows {
            let (fp, ff) = r.full.map_or((String::new(), String::new()), |(a, b)| (format!("{:.4}", a), format!("{:.4}", b)));
            tsv.push_str(&format!(
                "{name}\t{}\t{}\t{}\t{}\t{fp}\t{ff}\t{:.4}\t{:.4}\t{:.4}\n",
                r.source, r.files, r.sheets, r.cells, r.coarse.0, r.coarse.1, r.accuracy
            ));
        }
    }
    if let Some(p) = out {
        std::fs::write(&p, tsv).map_err(|e| format!("{}: {e}", p.display()))?;
        println!("wrote {}", p.display());
    }
    Ok(())
}
