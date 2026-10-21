//! Debug aid on DECO (or `HH_GT_CORPUS=synthetic`) **dev** (never test): prints sheets with ground-truth vs predicted roles.
//! `HH_GT_FILE=substr` dumps whole matching sheets; `HH_GT_CONF=group_header>data` prints rows
//! that hold a cell with that (gt>pred) confusion, `HH_GT_LIMIT` rows max (default 60);
//! `HH_GT_TABLES=1` lists sheets whose tables do not match at IoU≥0.5.
//! `cargo test --release -p hh_core --test layout_gtdump -- --ignored --nocapture`

use std::collections::HashMap;

use hh_core::eval::{data_root, deco, split::Split, synthetic};
use hh_core::{a1, layout, open, Grid, Rect, Role, Value};

fn tag(r: Role) -> &'static str {
    match r {
        Role::Title => "T",
        Role::Header => "H",
        Role::Data => "d",
        Role::GroupHeader => "G",
        Role::Total => "S",
        Role::Note => "N",
        Role::Other => "o",
    }
}

fn short(v: &Option<Value>) -> String {
    let s = match v {
        Some(Value::Text(t)) => format!("{t:?}"),
        Some(Value::Number(n)) => format!("{n}"),
        Some(v) => format!("{v:?}"),
        None => "∅".into(),
    };
    s.chars().take(18).collect()
}

fn rs(r: &Rect) -> String {
    format!("{}:{}", a1(r.top, r.left), a1(r.bottom, r.right))
}

fn iou(a: &Rect, b: &Rect) -> f64 {
    let ih = (a.bottom.min(b.bottom) as i64 - a.top.max(b.top) as i64 + 1).max(0) as f64;
    let iw = (a.right.min(b.right) as i64 - a.left.max(b.left) as i64 + 1).max(0) as f64;
    let area = |r: &Rect| (r.bottom - r.top + 1) as f64 * (r.right - r.left + 1) as f64;
    let i = ih * iw;
    i / (area(a) + area(b) - i)
}

#[test]
#[ignore]
fn gtdump() {
    let file_filter = std::env::var("HH_GT_FILE").ok();
    let conf = std::env::var("HH_GT_CONF").ok().and_then(|s| {
        let (a, b) = s.split_once('>')?;
        let f = |x: &str| Role::ALL.into_iter().find(|r| r.as_str() == x);
        Some((f(a)?, f(b)?))
    });
    let tables = std::env::var("HH_GT_TABLES").is_ok();
    let limit: usize = std::env::var("HH_GT_LIMIT").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let corpus = match std::env::var("HH_GT_CORPUS").as_deref() {
        Ok("synthetic") => synthetic::load(&data_root().join("synthetic").join("v0")),
        _ => deco::load(&data_root()),
    }
    .unwrap();
    let mut printed = 0;
    for file in corpus.files.iter().filter(|f| Split::Dev.contains(&f.id)) {
        if file_filter.as_ref().is_some_and(|s| !file.id.contains(s.as_str())) {
            continue;
        }
        let Ok(wb) = open(&file.path) else { continue };
        for gs in &file.sheets {
            let Some(sheet) = wb.sheets.iter().find(|s| s.name == gs.name) else { continue };
            let grid = Grid::new(sheet);
            let l = layout::analyze(&grid);
            let gt: HashMap<(u32, u32), Role> = gs.cells.iter().map(|c| ((c.row, c.col), c.role)).collect();
            if tables {
                let pred: Vec<Rect> = l.tables.iter().map(|t| t.rect).collect();
                let ok = gs.tables.len() == pred.len()
                    && gs.tables.iter().all(|g| pred.iter().any(|p| iou(g, p) >= 0.5));
                if !ok {
                    let g: Vec<String> = gs.tables.iter().map(rs).collect();
                    let p: Vec<String> = pred.iter().map(rs).collect();
                    println!("{} | {} | gt {:?} | pred {:?}", file.id, gs.name, g, p);
                }
                continue;
            }
            let mut want_rows: Vec<u32> = Vec::new();
            if let Some((g, p)) = conf {
                for (i, c) in sheet.cells.iter().enumerate() {
                    if gt.get(&(c.row, c.col)) == Some(&g) && l.roles[i] == p && want_rows.last() != Some(&c.row) {
                        want_rows.push(c.row);
                    }
                }
                if want_rows.is_empty() {
                    continue;
                }
            } else if file_filter.is_none() {
                continue;
            }
            println!("== {} | {}", file.id, gs.name);
            println!("   gt  {:?}", gs.tables.iter().map(rs).collect::<Vec<_>>());
            println!("   pred{:?}", l.tables.iter().map(|t| rs(&t.rect)).collect::<Vec<_>>());
            let mut line = String::new();
            let mut row = u32::MAX;
            let show = |r: u32| conf.is_none() || want_rows.iter().any(|&w| w.abs_diff(r) <= 1);
            for (i, c) in sheet.cells.iter().enumerate() {
                if !show(c.row) {
                    continue;
                }
                if c.row != row {
                    if !line.is_empty() {
                        println!("{line}");
                        printed += 1;
                    }
                    row = c.row;
                    line = format!("{:>5}:", row + 1);
                }
                let g = gt.get(&(c.row, c.col)).map_or("-", |r| tag(*r));
                let p = tag(l.roles[i]);
                let mark = if g == p { g.to_string() } else { format!("{g}/{p}") };
                let f = if c.formula.is_some() { "=" } else { "" };
                line.push_str(&format!(" {}[{}]{}{}", a1(c.row, c.col), mark, f, short(&c.value)));
            }
            if !line.is_empty() {
                println!("{line}");
                printed += 1;
            }
            if printed >= limit {
                return;
            }
        }
    }
}
