//! `ReadOptions::threads`: the parallel reads (large sheets, large shared string tables) keep
//! within the host's budget, `Some(1)` starts no worker threads, and the workbook is the same
//! for every thread count. The workbooks are generated here.
//!
//! The ignored `measure_read` reads one file for a benchmark (one process per run, so the peak
//! RSS is the read's): `HH_READ_FILE=big.xlsx HH_READ_THREADS=1|none cargo test --release -p
//! hh_core --test reader_threads -- --ignored --nocapture measure_read`; `HH_READ_WRITE=big.xlsx`
//! with `write_big` first writes a 1,000,000-cell workbook there.

use hh_core::reader::{ReadOptions, open_bytes_with, open_with};

/// `rows` rows of 4 distinct strings and 4 numbers: with `rows` in the thousands, the sheet part
/// and the shared string table both take the parallel read (with a part threshold of 0).
fn workbook(rows: u32, cols: u16) -> rust_xlsxwriter::Workbook {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet().set_name("Data").unwrap();
    for r in 0..rows {
        for c in 0..cols {
            if c % 2 == 0 {
                ws.write_string(r, c, format!("item {r}-{c}")).unwrap();
            } else {
                ws.write_number(r, c, f64::from(r) * 1.5 + f64::from(c))
                    .unwrap();
            }
        }
    }
    wb
}

fn read(bytes: &[u8], threads: Option<usize>) -> (String, usize) {
    let before = calamine::parallel_reads_started();
    let wb = open_bytes_with(
        bytes,
        Some("t.xlsx"),
        ReadOptions {
            threads,
            ..ReadOptions::default()
        },
    )
    .unwrap();
    let text = format!("{:?}", wb.sheets);
    (text, calamine::parallel_reads_started() - before)
}

#[test]
fn threads_bound_the_parallel_read() {
    let bytes = workbook(5_000, 8).save_to_buffer().unwrap();
    // Every part read in parallel, in chunks of 4 kB, on up to 4 threads.
    calamine::set_parallel_read_for_tests(Some((4, 4096, 0)));
    let (all, started) = read(&bytes, None);
    assert_eq!(
        started, 2,
        "the shared strings and the sheet read in parallel"
    );
    for threads in [Some(0), Some(1)] {
        let (one, started) = read(&bytes, threads);
        assert_eq!(started, 0, "{threads:?}: no worker threads");
        assert_eq!(one, all, "{threads:?}");
    }
    let (two, started) = read(&bytes, Some(2));
    assert_eq!(started, 2);
    assert_eq!(two, all);
    calamine::set_parallel_read_for_tests(None);
    // The budget is scoped to the call.
    assert_eq!(
        calamine::with_parallel_read_threads(Some(1), calamine::parallel_read_threads),
        1
    );
    assert_eq!(
        calamine::parallel_read_threads(),
        std::thread::available_parallelism().map_or(1, |n| n.get().min(4))
    );
}

#[test]
#[ignore]
fn write_big() {
    let path = std::env::var("HH_READ_WRITE").expect("HH_READ_WRITE=path.xlsx");
    // By default 100,000 rows x 10 columns (1,000,000 cells).
    let rows = std::env::var("HH_READ_ROWS").map_or(100_000, |r| r.parse().unwrap());
    workbook(rows, 10).save(&path).unwrap();
}

#[test]
#[ignore]
fn measure_read() {
    let path = std::env::var("HH_READ_FILE").expect("HH_READ_FILE=path.xlsx");
    let threads = match std::env::var("HH_READ_THREADS").as_deref() {
        Ok("none") | Err(_) => None,
        Ok(n) => Some(n.parse().expect("HH_READ_THREADS: a number or none")),
    };
    let env = |k: &str| std::env::var(k).map_or(0, |v| v.parse::<usize>().unwrap());
    let repeat = env("HH_READ_REPEAT").max(1);
    // A heap of other work (another host's): HH_READ_BALLAST_MB of 4 kB blocks allocated on 4
    // threads, every other block freed again, so the trim has arenas and free pages to walk.
    let ballast: Vec<Vec<Vec<u8>>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(|| {
                    let mut v: Vec<Vec<u8>> = (0..env("HH_READ_BALLAST_MB") * 64)
                        .map(|i| vec![u8::try_from(i % 251).unwrap(); 4096])
                        .collect();
                    for (i, b) in v.iter_mut().enumerate() {
                        if i % 2 == 1 {
                            *b = Vec::new();
                        }
                    }
                    v
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let before = calamine::parallel_reads_started();
    let t = std::time::Instant::now();
    let mut cells = 0;
    for _ in 0..repeat {
        let wb = open_with(
            &path,
            ReadOptions {
                threads,
                ..ReadOptions::default()
            },
        )
        .unwrap();
        cells = wb.sheets.iter().map(|s| s.cells.len()).sum();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "read threads={threads:?}: {cells} cells, {repeat} opens in {ms:.1} ms, parallel reads {}",
        calamine::parallel_reads_started() - before
    );
    drop(ballast);
}
