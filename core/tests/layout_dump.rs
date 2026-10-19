//! Debug aid: prints every cell with its layout role and the detected tables.
//! `HH_DUMP=file.xlsx cargo test --release -p hh_core --test layout_dump -- --ignored --nocapture`
//! (`HH_DUMP_SHEET=name` limits it to one sheet).

use hh_core::{a1, layout, open, Grid, Role, Value};

fn short(v: &Option<Value>) -> String {
    let s = match v {
        Some(Value::Text(t)) => format!("{t:?}"),
        Some(v) => format!("{v:?}"),
        None => "∅".into(),
    };
    s.chars().take(24).collect()
}

fn tag(r: Role) -> &'static str {
    match r {
        Role::Title => "T",
        Role::Header => "H",
        Role::Data => "d",
        Role::GroupHeader => "G",
        Role::Total => "Σ",
        Role::Note => "N",
        Role::Other => "o",
    }
}

#[test]
#[ignore]
fn dump() {
    let Ok(path) = std::env::var("HH_DUMP") else { return };
    let only = std::env::var("HH_DUMP_SHEET").ok();
    let book = open(&path).unwrap();
    for sheet in &book.sheets {
        if only.as_ref().is_some_and(|n| *n != sheet.name) {
            continue;
        }
        let l = layout::analyze(&Grid::new(sheet));
        println!("== {} ({} cells)", sheet.name, sheet.cells.len());
        for t in &l.tables {
            println!(
                "  table {}:{} header_top={} rows={} conf={:.2}",
                a1(t.rect.top, t.rect.left),
                a1(t.rect.bottom, t.rect.right),
                t.header_top + 1,
                t.header_rows,
                t.confidence
            );
        }
        let mut row = u32::MAX;
        let mut line = String::new();
        for (i, c) in sheet.cells.iter().enumerate() {
            if c.row != row {
                if !line.is_empty() {
                    println!("{line}");
                }
                row = c.row;
                line = format!("{:>5}:", row + 1);
            }
            let f = if c.formula.is_some() { "=" } else { "" };
            line.push_str(&format!(" {}[{}]{}{}", a1(c.row, c.col), tag(l.roles[i]), f, short(&c.value)));
        }
        if !line.is_empty() {
            println!("{line}");
        }
    }
}
