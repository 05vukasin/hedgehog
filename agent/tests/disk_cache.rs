//! The disk cache through a `Session`: a new session reopens a file from the cache with the same
//! catalog and answers; damaged entries are rebuilt; a cancel stops a running query.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use hh_agent::cache::Cache;
use hh_agent::{Options, Session};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hh_agent_disk_cache_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("data/sub")).unwrap();
    d
}

fn csv() -> String {
    let mut s = String::from("Region,Product,Revenue,Date\n");
    for i in 0..400 {
        s.push_str(&format!("R{},\"{} - Product {}\",{}.5,2021-{:02}-01\n", i % 5, 1000 + i % 7, i % 7, 100 + i, 1 + i % 12));
    }
    s.push_str("Total,,,\n");
    s
}

fn session(root: &Path, cache: Option<&Path>) -> Session {
    let opts = Options { cache: cache.map(|c| Cache { dir: c.to_path_buf(), max_bytes: 1 << 30 }), ..Options::default() };
    Session::with_options(root, &[], opts).unwrap()
}

/// The catalog without its last line (the timing line differs between a fresh and a cached open).
fn body(cat: &str) -> &str {
    cat.rsplit_once('\n').map_or(cat, |x| x.0)
}

fn entries(cache: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(cache.join("v1")) else { return Vec::new() };
    rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()
}

const Q: &str = "SELECT Region, round(sum(Revenue), 1) AS r, count(DISTINCT Product_id) AS ids FROM T1 GROUP BY Region ORDER BY Region";

#[test]
fn reopen_from_cache_matches() {
    let d = dir("reopen");
    let (root, cache) = (d.join("data"), d.join("cache"));
    std::fs::write(root.join("sales.csv"), csv()).unwrap();
    std::fs::write(root.join("sub/sales copy.csv"), csv()).unwrap();

    let mut plain = session(&root, None);
    let cat0 = plain.open_file("sales.csv").unwrap();
    let q0 = plain.query("sales.csv", Q, None).unwrap();
    let desc0 = plain.describe_table("sales.csv", "T1").unwrap();
    let copy0 = plain.open_file("sub/sales copy.csv").unwrap();
    assert!(entries(&cache).is_empty());

    let mut cold = session(&root, Some(&cache));
    let cat1 = cold.open_file("sales.csv").unwrap();
    assert!(cat1.ends_with(" s)"), "{cat1}");
    assert_eq!(cold.query("sales.csv", Q, None).unwrap(), q0);
    assert_eq!(entries(&cache).len(), 1, "the entry is written once the database is built");

    let mut warm = session(&root, Some(&cache));
    let cat2 = warm.open_file("sales.csv").unwrap();
    assert!(cat2.ends_with("\n(cached)"), "{cat2}");
    assert_eq!(body(&cat0), body(&cat1));
    assert_eq!(body(&cat0), body(&cat2));
    assert_eq!(warm.query("sales.csv", Q, None).unwrap(), q0);
    assert_eq!(warm.describe_table("sales.csv", "T1").unwrap(), desc0);
    assert_eq!(warm.totals("sales.csv", None, None).unwrap(), plain.totals("sales.csv", None, None).unwrap());
    // Same content under another name: one entry, the catalog made for that name.
    let copy2 = warm.open_file("sub/sales copy.csv").unwrap();
    assert_eq!(body(&copy2), body(&copy0));
    assert_eq!(entries(&cache).len(), 1);
    let mut again = session(&root, Some(&cache));
    assert_eq!(body(&again.open_file("sub/sales copy.csv").unwrap()), body(&copy0));

    // A changed file is a new entry.
    std::fs::write(root.join("sales.csv"), csv().replace("R1,", "R9,")).unwrap();
    let mut changed = session(&root, Some(&cache));
    assert!(!changed.open_file("sales.csv").unwrap().ends_with("(cached)"));
    assert!(changed.query("sales.csv", "SELECT count(*) FROM T1 WHERE Region = 'R9'", None).unwrap().starts_with("count(*)\n80"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn damaged_entries_are_rebuilt() {
    let d = dir("damaged");
    let (root, cache) = (d.join("data"), d.join("cache"));
    std::fs::write(root.join("sales.csv"), csv()).unwrap();
    let mut s = session(&root, Some(&cache));
    s.open_file("sales.csv").unwrap();
    let q0 = s.query("sales.csv", Q, None).unwrap();
    let entry = entries(&cache).pop().unwrap();

    // norm.bin damaged (same size): the catalog still comes from the entry; the query rebuilds.
    let n = std::fs::metadata(entry.join("norm.bin")).unwrap().len() as usize;
    std::fs::write(entry.join("norm.bin"), vec![7u8; n]).unwrap();
    let mut s = session(&root, Some(&cache));
    assert!(s.open_file("sales.csv").unwrap().ends_with("(cached)"));
    assert_eq!(s.query("sales.csv", Q, None).unwrap(), q0);

    // db.sqlite damaged (same size): not a database, so the file is normalized again.
    let entry = entries(&cache).pop().unwrap();
    let n = std::fs::metadata(entry.join("db.sqlite")).unwrap().len() as usize;
    std::fs::write(entry.join("db.sqlite"), vec![0u8; n]).unwrap();
    let mut s = session(&root, Some(&cache));
    assert!(!s.open_file("sales.csv").unwrap().ends_with("(cached)"));
    assert_eq!(s.query("sales.csv", Q, None).unwrap(), q0);

    // A partial entry (no meta.json) is ignored and replaced.
    let entry = entries(&cache).pop().unwrap();
    std::fs::remove_file(entry.join("meta.json")).unwrap();
    let mut s = session(&root, Some(&cache));
    assert!(!s.open_file("sales.csv").unwrap().ends_with("(cached)"));
    assert_eq!(s.query("sales.csv", Q, None).unwrap(), q0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn cancel_stops_a_query() {
    let d = dir("cancel");
    let root = d.join("data");
    std::fs::write(root.join("sales.csv"), csv()).unwrap();
    let opts = Options { cache: None, query_timeout: Duration::from_secs(30), ..Options::default() };
    let mut s = Session::with_options(&root, &[], opts).unwrap();
    s.open_file("sales.csv").unwrap();
    let flag = s.cancel_flag();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        flag.store(true, Ordering::Relaxed);
    });
    let t0 = std::time::Instant::now();
    let e = s
        .query("sales.csv", "WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM r) SELECT count(*) FROM r", None)
        .unwrap_err();
    t.join().unwrap();
    assert_eq!(e.0, "cancelled");
    assert!(t0.elapsed() < Duration::from_secs(10));
    s.cancel_flag().store(false, Ordering::Relaxed);
    assert!(s.query("sales.csv", "SELECT count(*) FROM T1", None).unwrap().starts_with("count(*)\n400"));
    let _ = std::fs::remove_dir_all(&d);
}
