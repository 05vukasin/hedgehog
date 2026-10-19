//! Corpus sanity run (ignored by default):
//! `HH_LAYOUT_CORPUS=<dir> HH_LAYOUT_N=300 cargo test --release -p hh_core --test layout_corpus -- --ignored --nocapture`
//! Files are untrusted and only read through `hh_core::open`. Reports panics, timing, and
//! distributions to compare with the DECO numbers in docs/research/irregularities.md §0.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use hh_core::{layout, open, Grid, Role};

#[test]
#[ignore]
fn corpus_sanity() {
    let Ok(dir) = std::env::var("HH_LAYOUT_CORPUS") else {
        eprintln!("HH_LAYOUT_CORPUS not set; skipping");
        return;
    };
    let n: usize = std::env::var("HH_LAYOUT_N").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "xlsx" || x == "xls"))
        .map(|p| {
            let mut h = DefaultHasher::new();
            p.file_name().hash(&mut h);
            (h.finish(), p)
        })
        .collect();
    files.sort();
    files.truncate(n);

    let (mut ok, mut failed, mut panics) = (0, 0, Vec::new());
    let (mut sheets, mut nonempty) = (0usize, 0usize);
    let mut per_sheet: BTreeMap<usize, usize> = BTreeMap::new();
    let (mut tables, mut with_header, mut multi_header, mut with_total, mut mid_total) = (0, 0, 0, 0, 0);
    let (mut s_title, mut s_note, mut s_group, mut s_total, mut s_multi) = (0, 0, 0, 0, 0);
    let mut many: Vec<(usize, String)> = Vec::new();
    let mut role_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut t_open, mut t_layout, mut worst) = (0f64, 0f64, (0f64, String::new(), 0usize));
    for (_, path) in &files {
        let t0 = Instant::now();
        let wb = match open(path) {
            Ok(wb) => wb,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        t_open += t0.elapsed().as_secs_f64();
        ok += 1;
        for sheet in &wb.sheets {
            sheets += 1;
            if sheet.cells.is_empty() {
                continue;
            }
            nonempty += 1;
            let t1 = Instant::now();
            let res = catch_unwind(AssertUnwindSafe(|| layout::analyze(&Grid::new(sheet))));
            let dt = t1.elapsed().as_secs_f64();
            t_layout += dt;
            if dt > worst.0 {
                worst = (dt, format!("{} [{}]", path.display(), sheet.name), sheet.cells.len());
            }
            let Ok(l) = res else {
                panics.push(format!("{} [{}]", path.display(), sheet.name));
                continue;
            };
            assert_eq!(l.roles.len(), sheet.cells.len());
            *per_sheet.entry(l.tables.len().min(6)).or_default() += 1;
            if l.tables.len() >= 6 {
                many.push((l.tables.len(), format!("{} [{}]", path.display(), sheet.name)));
            }
            s_multi += (l.tables.len() > 1) as usize;
            for r in &l.roles {
                *role_counts.entry(r.as_str()).or_default() += 1;
            }
            s_title += l.roles.contains(&Role::Title) as usize;
            s_note += l.roles.contains(&Role::Note) as usize;
            s_group += l.roles.contains(&Role::GroupHeader) as usize;
            s_total += l.roles.contains(&Role::Total) as usize;
            for t in &l.tables {
                tables += 1;
                with_header += (t.header_rows > 0) as usize;
                multi_header += (t.header_rows >= 2) as usize;
                let total_rows: Vec<u32> = sheet
                    .cells
                    .iter()
                    .zip(&l.roles)
                    .filter(|(c, r)| {
                        **r == Role::Total
                            && (t.rect.top..=t.rect.bottom).contains(&c.row)
                            && (t.rect.left..=t.rect.right).contains(&c.col)
                    })
                    .map(|(c, _)| c.row)
                    .collect();
                with_total += (!total_rows.is_empty()) as usize;
                mid_total += total_rows.iter().any(|&r| r + 1 < t.rect.bottom) as usize;
            }
        }
    }
    let pct = |a: usize, b: usize| 100.0 * a as f64 / b.max(1) as f64;
    println!("files: {} sampled, {ok} opened, {failed} reader errors, {} panics", files.len(), panics.len());
    for p in &panics {
        println!("  PANIC {p}");
    }
    println!("sheets: {sheets} ({nonempty} non-empty)");
    println!(
        "time: open {:.1} ms/file, layout {:.2} ms/sheet, worst layout {:.1} ms ({} cells) {}",
        1e3 * t_open / ok.max(1) as f64,
        1e3 * t_layout / nonempty.max(1) as f64,
        1e3 * worst.0,
        worst.2,
        worst.1
    );
    println!("tables per non-empty sheet (6 = 6+): {per_sheet:?}; sheets with >1 table {:.1}%", pct(s_multi, nonempty));
    println!(
        "tables {tables}: header found {:.1}%, ≥2 header rows {:.1}%, with total {:.1}%, mid-table total {:.1}%",
        pct(with_header, tables),
        pct(multi_header, tables),
        pct(with_total, tables),
        pct(mid_total, tables)
    );
    println!(
        "sheets with title {:.1}%, notes {:.1}%, group headers {:.1}%, totals {:.1}%",
        pct(s_title, nonempty),
        pct(s_note, nonempty),
        pct(s_group, nonempty),
        pct(s_total, nonempty)
    );
    println!("cell roles: {role_counts:?}");
    many.sort();
    for (k, name) in many.iter().rev().take(15) {
        println!("  {k} tables: {name}");
    }
    assert!(panics.is_empty());
}

/// Debug view: `HH_LAYOUT_DUMP=<file> [HH_LAYOUT_SHEET=<name>]` prints a role map
/// (T title, H header, D data, G group, S total, N note, o other; lowercase-free = in no table).
#[test]
#[ignore]
fn dump_sheet() {
    let Ok(path) = std::env::var("HH_LAYOUT_DUMP") else { return };
    let want = std::env::var("HH_LAYOUT_SHEET").ok().filter(|w| !w.is_empty());
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let (r0, c0) = (env("HH_LAYOUT_R0"), env("HH_LAYOUT_C0"));
    let max_rows: u32 = std::env::var("HH_LAYOUT_ROWS").ok().and_then(|v| v.parse().ok()).unwrap_or(80);
    let wb = open(&path).unwrap();
    for sheet in &wb.sheets {
        if want.as_ref().is_some_and(|w| *w != sheet.name) || sheet.cells.is_empty() {
            continue;
        }
        let l = layout::analyze(&Grid::new(sheet));
        println!("== {} : {} tables", sheet.name, l.tables.len());
        for t in &l.tables {
            println!(
                "  {}:{} header_rows={} conf={:.2}",
                hh_core::a1(t.rect.top, t.rect.left),
                hh_core::a1(t.rect.bottom, t.rect.right),
                t.header_rows,
                t.confidence
            );
        }
        let mut grid: BTreeMap<u32, BTreeMap<u32, (char, String)>> = BTreeMap::new();
        for (c, r) in sheet.cells.iter().zip(&l.roles) {
            if c.row < r0 || c.row >= r0 + max_rows || c.col < c0 || c.col >= c0 + 30 {
                continue;
            }
            let ch = match r {
                Role::Title => 'T',
                Role::Header => 'H',
                Role::Data => 'D',
                Role::GroupHeader => 'G',
                Role::Total => 'S',
                Role::Note => 'N',
                Role::Other => 'o',
            };
            let v = match (&c.value, &c.formula) {
                (_, Some(f)) => format!("={f}"),
                (Some(hh_core::Value::Text(s)), _) => s.clone(),
                (Some(v), _) => format!("{v:?}"),
                _ => String::new(),
            };
            grid.entry(c.row).or_default().insert(c.col, (ch, v.chars().take(10).collect()));
        }
        for (r, cols) in grid {
            let line: Vec<String> =
                cols.iter().map(|(c, (ch, v))| format!("{}{ch}:{v}", hh_core::a1(r, *c))).collect();
            println!("{:>4} {}", r + 1, line.join(" | "));
        }
    }
}

fn csv_fields(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut q) = (Vec::new(), String::new(), false);
    let mut it = line.chars().peekable();
    while let Some(ch) = it.next() {
        match (ch, q) {
            ('"', true) if it.peek() == Some(&'"') => {
                cur.push('"');
                it.next();
            }
            ('"', _) => q = !q,
            (',', false) => out.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

/// Quick local DECO check for tuning (the eval workstream owns the real harness):
/// `HH_DECO=<deco dir> cargo test --release -p hh_core --test layout_corpus deco -- --ignored --nocapture`
#[test]
#[ignore]
fn deco_quick_eval() {
    let Ok(dir) = std::env::var("HH_DECO") else { return };
    let dir = std::path::PathBuf::from(dir);
    let csv = std::fs::read_to_string(dir.join("annotations/rangeAnnotations.csv")).unwrap();
    // (file, sheet) -> [(label, rect)]
    let mut gt: BTreeMap<(String, String), Vec<(String, hh_core::Rect)>> = BTreeMap::new();
    for line in csv.lines().skip(1) {
        let f = csv_fields(line);
        if f.len() < 11 {
            continue;
        }
        let p = |i: usize| f[i].trim().parse::<u32>().ok();
        let (Some(c0), Some(r0), Some(c1), Some(r1)) = (p(7), p(8), p(9), p(10)) else { continue };
        let rect = hh_core::Rect { top: r0, left: c0, bottom: r1, right: c1 };
        gt.entry((f[0].clone(), f[1].clone())).or_default().push((f[4].clone(), rect));
    }
    let mut by_file: BTreeMap<String, Vec<(String, Vec<(String, hh_core::Rect)>)>> = BTreeMap::new();
    for ((file, sheet), v) in gt {
        by_file.entry(file).or_default().push((sheet, v));
    }
    let iou = |a: &hh_core::Rect, b: &hh_core::Rect| {
        let ih = (a.bottom.min(b.bottom) as i64 - a.top.max(b.top) as i64 + 1).max(0);
        let iw = (a.right.min(b.right) as i64 - a.left.max(b.left) as i64 + 1).max(0);
        let area = |r: &hh_core::Rect| (r.bottom - r.top + 1) as i64 * (r.right - r.left + 1) as i64;
        let inter = ih * iw;
        inter as f64 / (area(a) + area(b) - inter) as f64
    };
    let names = ["title", "header", "data", "group_header", "total", "note", "other"];
    let map = |l: &str| match l {
        "MetaTitle" => Some("title"),
        "Header" => Some("header"),
        "Data" => Some("data"),
        "GroupHead" => Some("group_header"),
        "Derived" => Some("total"),
        "Notes" => Some("note"),
        "Other" => Some("other"),
        _ => None,
    };
    let (mut n_gt, mut n_pred, mut tp5, mut tp9, mut tp10) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut conf: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut sheets_eval = 0;
    let mut cats: BTreeMap<&str, usize> = BTreeMap::new();
    let mut misses: Vec<String> = Vec::new();
    let top_pair = std::env::var("HH_TOP").ok();
    let mut fp: BTreeMap<String, usize> = BTreeMap::new();
    let mut fp_list: Vec<String> = Vec::new();
    let mut presence: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    let mut top: BTreeMap<String, usize> = BTreeMap::new();
    for (file, sheets) in &by_file {
        if hh_core::eval::split::split_of(file) != hh_core::eval::split::Split::Dev {
            continue;
        }
        let Ok(wb) = open(dir.join("completed").join(file)) else { continue };
        for (sname, anns) in sheets {
            let Some(sheet) = wb.sheets.iter().find(|s| &s.name == sname) else { continue };
            sheets_eval += 1;
            let l = layout::analyze(&Grid::new(sheet));
            let gtt: Vec<_> = anns.iter().filter(|(lb, _)| lb == "Table").map(|(_, r)| *r).collect();
            n_gt += gtt.len();
            n_pred += l.tables.len();
            let mut used = vec![false; l.tables.len()];
            for g in &gtt {
                let best = l
                    .tables
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !used[*i])
                    .map(|(i, t)| (iou(g, &t.rect), i))
                    .max_by(|a, b| a.0.total_cmp(&b.0));
                let fmt = |r: &hh_core::Rect| format!("{}:{}", hh_core::a1(r.top, r.left), hh_core::a1(r.bottom, r.right));
                let bv = best.map_or(0.0, |b| b.0);
                if bv < 0.999 {
                    let cat = match best {
                        None => "no pred",
                        Some((_, i)) => {
                            let p = l.tables[i].rect;
                            let contains = p.top <= g.top && p.left <= g.left && p.bottom >= g.bottom && p.right >= g.right;
                            let inside = p.top >= g.top && p.left >= g.left && p.bottom <= g.bottom && p.right <= g.right;
                            match (contains, inside) {
                                (true, _) => "pred too big",
                                (_, true) => "pred too small",
                                _ => "shifted",
                            }
                        }
                    };
                    *cats.entry(cat).or_default() += 1;
                    misses.push(format!(
                        "{cat:>14} iou {bv:.2} gt {} pred {} | {file} [{sname}] ({} pred)",
                        fmt(g),
                        best.map_or("-".into(), |b| fmt(&l.tables[b.1].rect)),
                        l.tables.len()
                    ));
                }
                if let Some((v, i)) = best
                    && v >= 0.5
                {
                    used[i] = true;
                    tp5 += 1;
                    tp9 += (v >= 0.9) as usize;
                    tp10 += (v >= 0.999) as usize;
                }
            }
            for (i, t) in l.tables.iter().enumerate() {
                if used[i] {
                    continue;
                }
                let p = t.rect;
                let inside_gt = gtt.iter().any(|g| p.top >= g.top && p.bottom <= g.bottom && p.left >= g.left && p.right <= g.right);
                let overl = gtt.iter().any(|g| iou(g, &p) > 0.0);
                let kind = if inside_gt { "fragment" } else if overl { "overlap" } else { "spurious" };
                let h = (p.bottom - p.top + 1).min(10);
                let w = (p.right - p.left + 1).min(10);
                *fp.entry(format!("{kind} h{h:02} hdr{}", (t.header_rows > 0) as u8)).or_default() += 1;
                if std::env::var("HH_FP").is_ok_and(|k| k == kind) {
                    let g = gtt.iter().find(|g| iou(g, &p) > 0.0).map_or("-".to_string(), |g| format!("{}:{}", hh_core::a1(g.top, g.left), hh_core::a1(g.bottom, g.right)));
                    fp_list.push(format!("{kind} pred {}:{} hdr {} gt {g} | {file} [{sname}]", hh_core::a1(p.top, p.left), hh_core::a1(p.bottom, p.right), t.header_rows));
                }
                let _ = w;
            }
            let mut sheet_gold: BTreeMap<&str, usize> = BTreeMap::new();
            let mut sheet_pred: BTreeMap<&str, usize> = BTreeMap::new();
            for (c, r) in sheet.cells.iter().zip(&l.roles) {
                let lab = anns.iter().rev().find_map(|(lb, rr)| {
                    let inside = (rr.top..=rr.bottom).contains(&c.row) && (rr.left..=rr.right).contains(&c.col);
                    if inside { map(lb) } else { None }
                });
                if let Some(g) = lab {
                    *sheet_gold.entry(g).or_default() += 1;
                    *sheet_pred.entry(r.as_str()).or_default() += 1;
                    *conf.entry((g, r.as_str())).or_default() += 1;
                    if top_pair.as_deref() == Some(&format!("{g}:{}", r.as_str())) {
                        *top.entry(format!("{file} [{sname}]")).or_default() += 1;
                    }
                }
            }
            for k in names {
                let (g, p) = (sheet_gold.get(k).copied().unwrap_or(0), sheet_pred.get(k).copied().unwrap_or(0));
                let e = presence.entry(k).or_default();
                if std::env::var("HH_GOLDONLY").is_ok_and(|x| x == k) && g > 0 && p == 0 {
                    println!("GOLDONLY {k} {g} | {file} [{sname}]");
                }
                if std::env::var("HH_PREDONLY").is_ok_and(|x| x == k) && g == 0 && p > 0 {
                    println!("PREDONLY {k} {p} | {file} [{sname}]");
                }
                match (g > 0, p > 0) {
                    (true, false) => e.0 += 1,
                    (false, true) => e.1 += 1,
                    (true, true) => e.2 += 1,
                    _ => {}
                }
            }
        }
    }
    let pr = |tp: usize| {
        let p = tp as f64 / n_pred.max(1) as f64;
        let r = tp as f64 / n_gt.max(1) as f64;
        (p, r, 2.0 * p * r / (p + r).max(1e-9))
    };
    for (k, v) in &presence {
        println!("presence {k}: gold-only {} pred-only {} both {}", v.0, v.1, v.2);
    }
    let mut tv: Vec<_> = top.into_iter().collect();
    tv.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (k, v) in tv.iter().take(15) {
        println!("TOP {v:>6} {k}");
    }
    println!("unmatched predictions: {fp:?}");
    let step = (fp_list.len() / 40).max(1);
    for m in fp_list.iter().step_by(step) {
        println!("FP {m}");
    }
    println!("non-exact GT tables by category: {cats:?}");
    let step = (misses.len() / 50).max(1);
    for m in misses.iter().step_by(step) {
        println!("  {m}");
    }
    println!("sheets {sheets_eval}, gt tables {n_gt}, predicted {n_pred}");
    for (name, tp) in [("IoU>=0.5", tp5), ("IoU>=0.9", tp9), ("IoU=1", tp10)] {
        let (p, r, f) = pr(tp);
        println!("tables {name}: P {p:.3} R {r:.3} F1 {f:.3}");
    }
    let mut f1s = Vec::new();
    for g in names {
        let tp = *conf.get(&(g, g)).unwrap_or(&0);
        let gold: usize = conf.iter().filter(|((a, _), _)| *a == g).map(|(_, v)| v).sum();
        let pred: usize = conf.iter().filter(|((_, b), _)| *b == g).map(|(_, v)| v).sum();
        let p = tp as f64 / pred.max(1) as f64;
        let r = tp as f64 / gold.max(1) as f64;
        let f = 2.0 * p * r / (p + r).max(1e-9);
        f1s.push(f);
        let row: Vec<String> = names
            .iter()
            .map(|b| format!("{}", conf.get(&(g, *b)).unwrap_or(&0)))
            .collect();
        println!("{g:>13}: gold {gold:>7} P {p:.3} R {r:.3} F1 {f:.3} | pred {}", row.join(" "));
    }
    println!("role macro F1 {:.3}  (pred columns: {})", f1s.iter().sum::<f64>() / f1s.len() as f64, names.join(" "));
}
