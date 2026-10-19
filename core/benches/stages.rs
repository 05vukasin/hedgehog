//! Per-stage benchmarks on generated sheets and on real Enron files.
//!
//! `cargo bench -p hh_core --bench stages` (filter: `-- grid`, `-- enron/open` …).
//! Real files come from `core/benches/enron50.txt`, resolved against `$HEDGEHOG_DATA`
//! (default `/home/vukasin/projects/excell-lib-project/testdata`); missing files are skipped.
//! Set `HH_BENCH_BIG=1` to include the nine biggest files of the list (slow).

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use hh_core::eval::bench::{self, synth};
use hh_core::{context, eval, totals, values, Features, Grid, Sheet, Workbook};

fn stages(c: &mut Criterion, group: &str, name: &str, sheet: &Sheet) {
    let mut g = c.benchmark_group(group);
    g.sample_size(10).warm_up_time(Duration::from_millis(500)).measurement_time(Duration::from_secs(3));
    g.throughput(Throughput::Elements(sheet.cells.len() as u64));
    let grid = Grid::new(sheet);
    let n = sheet.cells.len();
    g.bench_function(BenchmarkId::new("grid", name), |b| b.iter(|| black_box(Grid::new(sheet))));
    g.bench_function(BenchmarkId::new("context", name), |b| {
        b.iter(|| {
            let mut f = Features::new(n);
            context::compute(&grid, &mut f);
            f
        })
    });
    g.bench_function(BenchmarkId::new("values", name), |b| {
        b.iter(|| {
            let mut f = Features::new(n);
            values::compute(&grid, &mut f);
            f
        })
    });
    g.bench_function(BenchmarkId::new("totals", name), |b| {
        b.iter(|| {
            let mut f = Features::new(n);
            totals::compute(&grid, &mut f);
            f
        })
    });
    g.bench_function(BenchmarkId::new("detect_verify", name), |b| {
        b.iter(|| {
            let cands = totals::detect(&grid);
            cands.iter().map(|c| totals::verify(&grid, c)).filter(|v| v.ok == Some(true)).count()
        })
    });
    g.bench_function(BenchmarkId::new("baseline", name), |b| b.iter(|| eval::baseline::analyze(&grid)));
    g.bench_function(BenchmarkId::new("all", name), |b| b.iter(|| bench::run_sheet(sheet, false)));
    g.finish();
}

fn synthetic(c: &mut Criterion) {
    for (name, sheet) in synth::all() {
        if name == "tall_1m" {
            continue; // one-off worst case, see `hedgehog bench --synthetic`
        }
        stages(c, "synth", name, &sheet);
    }
}

fn enron_files() -> Vec<PathBuf> {
    let list = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/enron50.txt");
    let files = bench::resolve_inputs(&list).unwrap_or_default();
    let big = std::env::var_os("HH_BENCH_BIG").is_some();
    // The list starts with the 9 biggest files.
    files.into_iter().enumerate().filter(|(i, _)| big || *i >= 9).map(|(_, p)| p).filter(|p| p.exists()).collect()
}

fn enron(c: &mut Criterion) {
    let files = enron_files();
    if files.is_empty() {
        eprintln!("enron benches skipped: no files under {}", eval::data_root().display());
        return;
    }
    let books: Vec<Workbook> = files.iter().filter_map(|p| hh_core::open(p).ok()).collect();
    let cells: usize = books.iter().flat_map(|b| &b.sheets).map(|s| s.cells.len()).sum();

    let mut g = c.benchmark_group("enron");
    g.sample_size(10).warm_up_time(Duration::from_millis(500)).measurement_time(Duration::from_secs(5));
    g.throughput(Throughput::Elements(cells as u64));
    g.bench_function("open", |b| b.iter(|| files.iter().filter_map(|p| hh_core::open(p).ok()).count()));
    g.bench_function("pipeline", |b| b.iter(|| books.iter().map(|wb| bench::run_workbook(wb, false).cells).sum::<usize>()));
    g.bench_function("end_to_end", |b| {
        b.iter(|| files.iter().filter_map(|p| bench::run_file(p, false).ok()).map(|r| r.cells).sum::<usize>())
    });
    g.finish();

    // Per-stage view on the largest sheet of the set.
    if let Some(s) = books.iter().flat_map(|b| &b.sheets).max_by_key(|s| s.cells.len()) {
        stages(c, "enron_largest_sheet", &s.name.clone(), s);
    }
}

criterion_group!(benches, synthetic, enron);
criterion_main!(benches);
