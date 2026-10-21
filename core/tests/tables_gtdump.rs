//! Debug aid for table boundaries on the **dev** split (never test) of a corpus with table ranges:
//! lists sheets whose predicted tables do not match the ground truth at IoU ≥ `HH_GT_IOU` (default
//! 0.8), with one line per row: ground-truth role, model role and rule role (majority over the
//! row's labelled cells; `T H d G S N o`, `-` unlabelled), and whether the row lies in a ground-truth
//! (`g`) / predicted (`p`) table, then the first cells.
//!
//! `HH_GT_CORPUS=deco|statsheets|synthetic` (default deco), `HH_GT_FILE=substr`,
//! `HH_GT_LIMIT=sheets` (default 40), `HH_GT_RULES=1` scores the rules' tables instead of the
//! model path's. `HH_GT_DIFF=1` lists only sheets where the model path's tables differ from the
//! rules' (what `learned::tables` changed), with the matches at `HH_GT_IOU` before -> after.
//! `cargo test --release -p hh_core --test tables_gtdump -- --ignored --nocapture`

use std::collections::HashMap;

use hh_core::eval::metrics::{iou, table_matches, TableCriterion};
use hh_core::eval::{data_root, deco, split::Split, statsheets, synthetic};
use hh_core::{a1, layout, learned, open, Grid, Rect, Role, Value};

fn tag(r: Role) -> char {
    match r {
        Role::Title => 'T',
        Role::Header => 'H',
        Role::Data => 'd',
        Role::GroupHeader => 'G',
        Role::Total => 'S',
        Role::Note => 'N',
        Role::Other => 'o',
    }
}

fn short(v: &Option<Value>) -> String {
    let s = match v {
        Some(Value::Text(t)) => format!("{t:?}"),
        Some(Value::Number(n)) => format!("{n}"),
        Some(v) => format!("{v:?}"),
        None => "∅".into(),
    };
    s.chars().take(16).collect()
}

fn rs(r: &Rect) -> String {
    format!("{}:{}", a1(r.top, r.left), a1(r.bottom, r.right))
}

fn majority(roles: impl Iterator<Item = Role>) -> char {
    let mut n: HashMap<char, usize> = HashMap::new();
    for r in roles {
        *n.entry(tag(r)).or_default() += 1;
    }
    n.into_iter().max_by_key(|&(c, k)| (k, c)).map_or('-', |(c, _)| c)
}

#[test]
#[ignore]
fn tables_gtdump() {
    let root = data_root();
    let corpus = match std::env::var("HH_GT_CORPUS").as_deref() {
        Ok("statsheets") => statsheets::load(&root),
        Ok("synthetic") => synthetic::load(&root.join("synthetic").join("v0")),
        _ => deco::load(&root),
    }
    .unwrap();
    let file_filter = std::env::var("HH_GT_FILE").ok();
    let limit: usize = std::env::var("HH_GT_LIMIT").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let thr: f64 = std::env::var("HH_GT_IOU").ok().and_then(|s| s.parse().ok()).unwrap_or(0.8);
    let rules_only = std::env::var_os("HH_GT_RULES").is_some();
    let diff = std::env::var_os("HH_GT_DIFF").is_some();
    let model = learned::embedded().unwrap();
    let mut shown = 0;
    for file in corpus.files.iter().filter(|f| Split::Dev.contains(&f.id)) {
        if file_filter.as_ref().is_some_and(|s| !file.id.contains(s.as_str())) {
            continue;
        }
        let Ok(wb) = open(&file.path) else { continue };
        for gs in &file.sheets {
            let sheet = wb
                .sheets
                .iter()
                .find(|s| s.name == gs.name)
                .or_else(|| gs.index.and_then(|i| wb.sheets.get(i)));
            let Some(sheet) = sheet else { continue };
            let grid = Grid::new(sheet);
            let rules = layout::analyze(&grid);
            let l = if rules_only { rules.clone() } else { learned::analyze(&grid, model) };
            let pred: Vec<Rect> = l.tables.iter().map(|t| t.rect).collect();
            if diff {
                // Only sheets where the refined tables differ from the rules' tables.
                let other = learned::analyze_with(&grid, model, learned::RefineSteps::NONE);
                let crit = TableCriterion::Iou(thr);
                let po: Vec<Rect> = other.tables.iter().map(|t| t.rect).collect();
                let (a, b) = (table_matches(&gs.tables, &po, crit), table_matches(&gs.tables, &pred, crit));
                if po == pred {
                    continue;
                }
                println!("   DIFF tp {a} -> {b}, pred {} -> {}", po.len(), pred.len());
            } else {
                let ok = gs.tables.len() == pred.len() && gs.tables.iter().all(|g| pred.iter().any(|p| iou(*g, *p) >= thr));
                if ok {
                    continue;
                }
            }
            println!("== {} | {}", file.id, gs.name);
            println!("   gt   {:?}", gs.tables.iter().map(rs).collect::<Vec<_>>());
            let p: Vec<String> =
                l.tables.iter().map(|t| format!("{} h{}+{}", rs(&t.rect), t.header_top + 1, t.header_rows)).collect();
            println!("   pred {p:?}");
            let gt: HashMap<(u32, u32), Role> = gs.cells.iter().map(|c| ((c.row, c.col), c.role)).collect();
            let span = gs.tables.iter().chain(&pred).fold((u32::MAX, 0), |(a, b), r| (a.min(r.top), b.max(r.bottom)));
            let mut by_row: Vec<(u32, Vec<usize>)> = Vec::new();
            for (i, c) in sheet.cells.iter().enumerate() {
                if c.row + 3 < span.0 || c.row > span.1 + 3 {
                    continue;
                }
                match by_row.last_mut() {
                    Some((r, v)) if *r == c.row => v.push(i),
                    _ => by_row.push((c.row, vec![i])),
                }
            }
            let n = by_row.len();
            for (k, (row, idx)) in by_row.iter().enumerate() {
                if n > 40 && k == 25 {
                    println!("   ... {} rows", n - 33);
                }
                if n > 40 && (25..n - 8).contains(&k) {
                    continue;
                }
                let cells = || idx.iter().map(|&i| &sheet.cells[i]);
                let g = majority(cells().filter_map(|c| gt.get(&(c.row, c.col)).copied()));
                let m = majority(idx.iter().map(|&i| l.roles[i]));
                let r = majority(idx.iter().map(|&i| rules.roles[i]));
                let ing = if gs.tables.iter().any(|t| t.top <= *row && *row <= t.bottom) { 'g' } else { ' ' };
                let inp = if pred.iter().any(|t| t.top <= *row && *row <= t.bottom) { 'p' } else { ' ' };
                let text: Vec<String> = cells().take(5).map(|c| format!("{}={}", a1(c.row, c.col), short(&c.value))).collect();
                println!("  {:>5} {g}{m}{r} {ing}{inp} {:>3}  {}", row + 1, idx.len(), text.join(" "));
            }
            shown += 1;
            if shown >= limit {
                return;
            }
        }
    }
}
