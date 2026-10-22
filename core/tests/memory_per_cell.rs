//! Peak heap per cell of `normalize_workbook` on generated sheets (B90): a merged area reaching
//! past the data (a title merged over `A2:XFD2`, a header label merged to the end of the row)
//! must not make a table cost its rows × the sheet's width. EQ08 (4M cells) went from ~1.2 GB
//! to an out-of-memory kill at 4 GB that way.
//!
//! A counting allocator measures the heap. It refuses allocations past `LIMIT`, so a
//! regression aborts the test binary ("memory allocation of … failed") instead of taking the
//! machine down. The fast test runs in debug builds; the full-size one is ignored:
//! `scripts/run_capped.sh 2500M mem.log cargo test --release -p hh_core --test memory_per_cell -- --ignored`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use hh_core::{NormalizeOptions, normalize_workbook};
use hh_core::{Cell, Rect, Sheet, Value, Workbook};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// Heap limit: far above any sane peak of the sizes tested, far below what the machine has.
static LIMIT: AtomicUsize = AtomicUsize::new(512 << 20);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        if now > LIMIT.load(Ordering::Relaxed) {
            CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
            return std::ptr::null_mut();
        }
        PEAK.fetch_max(now, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// One measurement at a time (the counters are process-wide).
static SERIAL: Mutex<()> = Mutex::new(());

const LAST_COL: u32 = 16_383;

fn text(row: u32, col: u32, s: &str) -> Cell {
    Cell { row, col, value: Some(Value::Text(s.to_string())), formula: None }
}

fn num(row: u32, col: u32, v: f64) -> Cell {
    Cell { row, col, value: Some(Value::Number(v)), formula: None }
}

#[derive(Clone, Copy, Debug)]
enum Shape {
    /// Title lines above the header, no merge past the data.
    Plain,
    /// EQ08: agency merged over A:L, the title over the whole row, the release line over A:L
    /// with the copyright in N, then a six-column header.
    FullRowTitle,
    /// The header's last label merged to the end of the row (`F4:XFD4`).
    HeaderMergedToRowEnd,
}

/// A data cube of `rows` records under a six-column header on row 4.
fn sheet(shape: Shape, rows: u32) -> Sheet {
    let mut cells = vec![
        text(0, 0, "Australian Bureau of Statistics"),
        text(1, 0, "6291.0.55.001 - EQ08 - Employed persons by Occupation unit group, Sex, State"),
        text(2, 0, "Released at 11.30 am (Canberra time) 23 December 2021"),
        text(2, 13, "© Commonwealth of Australia 2021"),
    ];
    let mut merges = Vec::new();
    if !matches!(shape, Shape::Plain) {
        merges.push(Rect { top: 0, left: 0, bottom: 0, right: 11 });
        merges.push(Rect { top: 1, left: 0, bottom: 1, right: LAST_COL });
        merges.push(Rect { top: 2, left: 0, bottom: 2, right: 11 });
    }
    for (j, h) in ["Mid-quarter month", "Sex", "State", "Occupation", "Employed total ('000)", "Hours ('000)"].iter().enumerate() {
        cells.push(text(3, j as u32, h));
    }
    if matches!(shape, Shape::HeaderMergedToRowEnd) {
        merges.push(Rect { top: 3, left: 5, bottom: 3, right: LAST_COL });
    }
    const STATES: [&str; 8] = ["New South Wales", "Victoria", "Queensland", "South Australia", "Western Australia", "Tasmania", "Northern Territory", "Australian Capital Territory"];
    for i in 0..rows {
        let r = 4 + i;
        cells.push(Cell { row: r, col: 0, value: Some(Value::DateTime(31_625.0 + 91.0 * (i / 800) as f64)), formula: None });
        cells.push(text(r, 1, if (i / 400) % 2 == 0 { "Males" } else { "Females" }));
        cells.push(text(r, 2, STATES[((i / 50) % 8) as usize]));
        cells.push(text(r, 3, &format!("{} Occupation {}", 1111 + i % 50, i % 50)));
        cells.push(num(r, 4, 0.25 + (i % 97) as f64 * 0.37));
        cells.push(num(r, 5, 12.5 + (i % 89) as f64 * 1.13));
    }
    Sheet { name: "Data 1".into(), cells, merges, formats: Vec::new() }
}

/// Peak heap bytes per cell of normalizing one sheet (the sheet itself excluded) and the
/// normalized tables' widest column count.
fn measure(shape: Shape, rows: u32) -> (f64, usize) {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let book = Workbook { sheets: vec![sheet(shape, rows)] };
    let cells = book.sheets[0].cells.len();
    let base = CURRENT.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let out = normalize_workbook(&book, &NormalizeOptions { rules_only: true, ..NormalizeOptions::default() });
    let peak = PEAK.load(Ordering::Relaxed) - base;
    let width = out.tables.iter().map(|t| t.columns.len()).max().unwrap_or(0);
    (peak as f64 / cells as f64, width)
}

/// Heap per cell a normalize may take on these sheets: five times the ~200 bytes they cost, far
/// below the ~43 kB a table as wide as the sheet costs (16,384 slots of 16 bytes per row of six
/// cells).
const MAX_BYTES_PER_CELL: f64 = 1_000.0;

fn check(rows: u32) {
    // Once to warm up what is built on first use (lazy tables), then the plain sheet's cost.
    measure(Shape::Plain, 100);
    let (plain, _) = measure(Shape::Plain, rows);
    for shape in [Shape::Plain, Shape::FullRowTitle, Shape::HeaderMergedToRowEnd] {
        let (per_cell, width) = measure(shape, rows);
        eprintln!("{shape:?} {rows} rows: {per_cell:.0} bytes per cell, {width} columns (plain {plain:.0})");
        assert!(per_cell <= MAX_BYTES_PER_CELL, "{shape:?}: {per_cell:.0} bytes per cell");
        // At most the columns A:N that hold cells, plus `_src_row`: never the merge's width.
        assert!(width <= 16, "{shape:?}: {width} columns");
        // A merge past the data costs nothing per cell beyond the plain sheet.
        assert!(per_cell <= plain * 1.5 + 200.0, "{shape:?}: {per_cell:.0} bytes per cell, plain {plain:.0}");
    }
}

#[test]
fn merges_past_the_data_cost_nothing_per_row() {
    check(if cfg!(debug_assertions) { 6_000 } else { 60_000 });
}

/// EQ08's size: 600k records (3.6M cells).
#[test]
#[ignore = "heavy: run in release under scripts/run_capped.sh (see the module docs)"]
fn merges_past_the_data_full_size() {
    LIMIT.store(2 << 30, Ordering::Relaxed);
    check(600_000);
}
