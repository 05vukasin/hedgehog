//! DECO loader + runner + baseline on fixtures generated at test time (nothing committed).

use std::path::{Path, PathBuf};

use hh_core::eval::deco::{load, map_label};
use hh_core::eval::run::{evaluate, RunOptions};
use hh_core::eval::split::Split;
use hh_core::eval::{baseline, metrics::role_index};
use hh_core::{Grid, Rect, Role};
use rust_xlsxwriter::Workbook;

#[test]
fn deco_label_mapping() {
    assert_eq!(map_label("Data"), Some(Role::Data));
    assert_eq!(map_label("Header"), Some(Role::Header));
    assert_eq!(map_label("Derived"), Some(Role::Total));
    assert_eq!(map_label("GroupHead"), Some(Role::GroupHeader));
    assert_eq!(map_label("MetaTitle"), Some(Role::Title));
    assert_eq!(map_label("Notes"), Some(Role::Note));
    assert_eq!(map_label("Other"), Some(Role::Other));
    assert_eq!(map_label("Table"), None);
    assert_eq!(map_label("data"), None);
}

fn temp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hh_eval_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("irregular/deco/annotations")).unwrap();
    std::fs::create_dir_all(d.join("irregular/enron")).unwrap();
    d
}

/// Sheet "Data, sheet" (comma in the name → quoted CSV field):
///   A1 title "Report" (alone); A3:C3 header; A4:C5 data; A6 "Total" + C6 = derived.
/// The baseline sees component {A3:C6} → one table; A1 alone → Other.
fn write_fixture(root: &Path) {
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Data, sheet").unwrap();
    ws.write_string(0, 0, "Report").unwrap();
    for (c, h) in ["Name", "Qty", "Price"].iter().enumerate() {
        ws.write_string(2, c as u16, *h).unwrap();
    }
    ws.write_string(3, 0, "a").unwrap();
    ws.write_number(3, 1, 1.0).unwrap();
    ws.write_number(3, 2, 2.0).unwrap();
    ws.write_string(4, 0, "b").unwrap();
    ws.write_number(4, 1, 3.0).unwrap();
    ws.write_number(4, 2, 4.0).unwrap();
    ws.write_string(5, 0, "Total").unwrap();
    ws.write_number(5, 2, 6.0).unwrap();
    wb.save(root.join("irregular/enron/u__1__fix.xlsx")).unwrap();

    let ranges = "FileName,SheetName,SheetIndex,AnnotationName,AnnotationLabel,AnnotationParent,AnnotationAddress,FirstColumn,FirstRow,LastColumn,LastRow\n\
\"u__1__fix.xlsx\",\"Data, sheet\",1,\"T\",Table,\"Data, sheet\",$A$3:$C$6,0,2,2,5\n\
\"u__1__fix.xlsx\",\"Data, sheet\",1,\"M\",MetaTitle,\"Data, sheet\",$A$1,0,0,0,0\n\
\"u__2__gone.xlsx\",\"Sheet1\",1,\"T\",Table,\"Sheet1\",$A$1:$B$2,0,0,1,1\n";
    std::fs::write(root.join("irregular/deco/annotations/rangeAnnotations.csv"), ranges).unwrap();

    let mut cells = String::from(
        "FileName,SheetName,SheetIndex,TableName,AnnotationLabel,AnnotationAddress,FirstColumn,FirstRow,LastColumn,LastRow\n",
    );
    let mut add = |label: &str, row: u32, col: u32| {
        cells.push_str(&format!("\"u__1__fix.xlsx\",\"Data, sheet\",1,\"T\",{label},X,{col},{row},{col},{row}\n"));
    };
    add("MetaTitle", 0, 0);
    for c in 0..3 {
        add("Header", 2, c);
    }
    for r in 3..5 {
        for c in 0..3 {
            add("Data", r, c);
        }
    }
    add("Derived", 5, 0);
    add("Derived", 5, 2);
    add("Mystery", 9, 9); // unmapped, and no such cell in the sheet → missing
    std::fs::write(root.join("irregular/deco/annotations/cellAnnotations.csv"), cells).unwrap();
}

#[test]
fn deco_loader_and_baseline_run() {
    let root = temp_root("deco");
    write_fixture(&root);

    let corpus = load(&root).unwrap();
    assert_eq!(corpus.files.len(), 1);
    assert_eq!(corpus.unmatched.len(), 1);
    assert_eq!(corpus.unmatched[0].id, "u__2__gone.xlsx");
    assert_eq!(corpus.unmapped_count(), 1);
    let s = &corpus.files[0].sheets[0];
    assert_eq!(s.name, "Data, sheet");
    assert_eq!(s.index, Some(0));
    assert_eq!(s.tables, vec![Rect { top: 2, left: 0, bottom: 5, right: 2 }]);
    assert_eq!(s.cells.len(), 13);

    let opts = RunOptions { split: Split::All, limit: None, progress: false };
    let rep = evaluate(&corpus, &baseline::analyze, "baseline", &opts);
    assert_eq!(rep.files_evaluated, 1);
    assert!(rep.files_failed.is_empty());
    assert_eq!(rep.sheets.len(), 1);
    let sr = &rep.sheets[0];
    assert_eq!(sr.matched_by, "name");
    assert_eq!(sr.missing_cells, 1);
    assert_eq!(sr.scored_cells, 12);
    // Baseline finds the table exactly.
    assert_eq!(sr.pred_tables, 1);
    assert_eq!(sr.table_tp, [1, 1, 1, 1, 1]);
    assert_eq!(rep.tables[2].tp, 1);
    // Roles: header 3/3, data 6/6, title→other, derived (2) → data.
    let c = &rep.confusion;
    assert_eq!(c.counts[role_index(Role::Header)][role_index(Role::Header)], 3);
    assert_eq!(c.counts[role_index(Role::Data)][role_index(Role::Data)], 6);
    assert_eq!(c.counts[role_index(Role::Title)][role_index(Role::Other)], 1);
    assert_eq!(c.counts[role_index(Role::Total)][role_index(Role::Data)], 2);
    assert_eq!(c.total(), 12);

    let json = rep.to_json();
    assert!(json.contains("\"sheet\":\"Data, sheet\""));
    assert!(json.contains("\"unmatched_files\":[[\"u__2__gone.xlsx\""));
    assert!(rep.scoreboard().contains("IoU>=1.0"));

    // Corrupt original + DECO copy present → opened via fallback.
    std::fs::create_dir_all(root.join("irregular/deco/completed")).unwrap();
    std::fs::copy(root.join("irregular/enron/u__1__fix.xlsx"), root.join("irregular/deco/completed/u__1__fix.xlsx"))
        .unwrap();
    std::fs::write(root.join("irregular/enron/u__1__fix.xlsx"), b"not a zip").unwrap();
    let corpus = load(&root).unwrap();
    let rep = evaluate(&corpus, &baseline::analyze, "baseline", &opts);
    assert_eq!(rep.files_via_fallback.len(), 1);
    assert_eq!(rep.sheets.len(), 1);

    // No usable copy → counted as failed to open, not a crash.
    std::fs::write(root.join("irregular/deco/completed/u__1__fix.xlsx"), b"not a zip").unwrap();
    let rep = evaluate(&corpus, &baseline::analyze, "baseline", &opts);
    assert_eq!(rep.files_failed.len(), 1);
    assert_eq!(rep.sheets.len(), 0);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn baseline_components() {
    use hh_core::{Cell, Sheet, Value};
    let cell = |row, col| Cell { row, col, value: Some(Value::Number(1.0)), formula: None };
    // Diagonal neighbours join (8-connectivity); an isolated cell is Other; a merged area bridges.
    let sheet = Sheet {
        name: "s".into(),
        cells: vec![cell(0, 0), cell(1, 1), cell(5, 5), cell(10, 0), cell(10, 4)],
        merges: vec![Rect { top: 10, left: 1, bottom: 10, right: 3 }],
    };
    let layout = baseline::analyze(&Grid::new(&sheet));
    let rects: Vec<Rect> = layout.tables.iter().map(|t| t.rect).collect();
    assert_eq!(rects, vec![Rect { top: 0, left: 0, bottom: 1, right: 1 }, Rect { top: 10, left: 0, bottom: 10, right: 4 }]);
    assert_eq!(layout.roles, vec![Role::Header, Role::Data, Role::Other, Role::Header, Role::Header]);
}
