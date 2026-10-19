//! Synthetic-corpus loader, breakdown and value check on a tiny fixture in the `hh-noise-gt/1`
//! format written by `tools/noise` (built at test time; nothing committed).

use std::path::{Path, PathBuf};

use hh_core::eval::run::{evaluate, RunOptions};
use hh_core::eval::split::Split;
use hh_core::eval::synthetic::{breakdown, load, parse_json, BreakdownOptions, J};
use hh_core::eval::{baseline, metrics::role_index};
use hh_core::{Rect, Role};
use rust_xlsxwriter::{Formula, Workbook};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hh_synth_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("enron_like")).unwrap();
    d
}

/// Sheet "Sales": A1 title; A3:B3 header; A4:B5 data (B as European number text);
/// A6 "Total" + B6 =SUM(B4:B5); A8 note. Plus an empty sheet that the GT marks as `empty`.
fn write_fixture(dir: &Path) {
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Sales").unwrap();
    ws.write_string(0, 0, "Sales Report").unwrap();
    ws.write_string(2, 0, "Region").unwrap();
    ws.write_string(2, 1, "Amount").unwrap();
    ws.write_string(3, 0, "North").unwrap();
    ws.write_string(3, 1, "1.234,50").unwrap();
    ws.write_string(4, 0, "South").unwrap();
    ws.write_string(4, 1, "-").unwrap();
    ws.write_string(5, 0, "Total").unwrap();
    ws.write_formula(5, 1, Formula::new("=SUM(B4:B5)").set_result("1234.5")).unwrap();
    ws.write_string(7, 0, "Source: fixture").unwrap();
    let e = wb.add_worksheet();
    e.set_name("Sheet2").unwrap();
    wb.save(dir.join("enron_like/wb_00000.xlsx")).unwrap();

    let gt = r#"{"schema":"hh-noise-gt/1","file":"wb_00000.xlsx","profile":"enron_like","seed":1,"index":0,
 "generator":"hh_noise/0.1","datasets":["fixture/sales.csv"],"op_sets":[["title_block"],["locale_numbers"],["missing_tokens"],["grand_total"],["notes_block"]],
 "cell_fields":["row","col","role","table","ds","src_row","src_col","orig","ops","true"],
 "sheets":[{"name":"Sales","index":0,"kind":"tables","ops":[{"op":"title_block"}],"merges":[],"hidden_rows":[],"hidden_cols":[],
   "tables":[{"id":0,"rect":[2,0,5,1],"header_top":2,"header_rows":1,"header_cols":0,"orientation":"col","ds":0,
      "boundary":{"op":"sheet_start"},"scale":1,"row_kinds":["header","data","data","total"],"hidden_rows":[],"hidden_cols":[],
      "columns":[{"col":0,"header":"Region","header_path":["Region"],"semantic_type":"category","src_col":0},
                 {"col":1,"header":"Amount","header_path":["Amount"],"semantic_type":"money","src_col":1}],
      "groups":[],"implied":[],"ops":[{"op":"locale_numbers","style":"eu"},{"op":"grand_total"}]}],
   "cells":[[0,0,"title",-1,-1,null,null,null,0,null],
            [2,0,"header",0,0,-1,0,"Region",-1,null],[2,1,"header",0,0,-1,1,"Amount",-1,null],
            [3,0,"data",0,0,10,0,"North",-1,null],[3,1,"data",0,0,10,1,1234.5,1,{"num":1234.5,"style":"eu"}],
            [4,0,"data",0,0,11,0,"South",-1,null],[4,1,"data",0,0,11,1,7.0,2,{"missing":"-"}],
            [5,0,"total",0,-1,null,null,null,3,null],[5,1,"total",0,-1,null,null,null,3,{"num":1234.5,"formula":true}],
            [7,0,"note",-1,-1,null,null,null,4,null]]},
  {"name":"Sheet2","index":1,"kind":"empty","ops":[{"op":"empty_sheet"}],"merges":[],"hidden_rows":[],"hidden_cols":[],"tables":[],"cells":[]}]}"#;
    std::fs::write(dir.join("enron_like/wb_00000.gt.json"), gt).unwrap();
}

#[test]
fn json_parser_basics() {
    let j = parse_json(r#"{"a":[1,-2.5e1,true,null,"x\"yé😀"],"b":{}}"#).unwrap();
    let a = j.get("a").unwrap().arr();
    assert_eq!(a[0], J::Num(1.0));
    assert_eq!(a[1], J::Num(-25.0));
    assert_eq!(a[2], J::Bool(true));
    assert_eq!(a[3], J::Null);
    assert_eq!(a[4].str(), Some("x\"yé😀"));
    assert!(parse_json("{\"a\":1").is_err());
    assert!(parse_json("[1] x").is_err());
}

#[test]
fn loads_and_scores_fixture() {
    let dir = temp_dir("load");
    write_fixture(&dir);
    let corpus = load(&dir).unwrap();
    assert_eq!(corpus.files.len(), 1);
    let f = &corpus.files[0];
    assert_eq!(f.id, "enron_like/wb_00000.xlsx");
    assert_eq!(f.sheets.len(), 1, "empty sheet without tables is skipped");
    let s = &f.sheets[0];
    assert_eq!(s.name, "Sales");
    assert_eq!(s.tables, vec![Rect { top: 2, left: 0, bottom: 5, right: 1 }]);
    assert_eq!(s.cells.len(), 10);
    assert_eq!(s.cells[0].role, Role::Title);
    assert_eq!(s.cells.last().unwrap().role, Role::Note);
    assert_eq!(corpus.labels["total"].count, 2);

    let opts = RunOptions { split: Split::All, ..RunOptions::default() };
    let rep = evaluate(&corpus, &baseline::analyze, "baseline", &opts);
    assert_eq!(rep.files_evaluated, 1);
    assert_eq!(rep.missing_cells(), 0, "every GT cell exists in the xlsx");
    assert_eq!(rep.confusion.counts[role_index(Role::Data)].iter().sum::<u64>(), 4);

    let bd = breakdown(&dir, &baseline::analyze, &BreakdownOptions { limit: None, values: true }).unwrap();
    assert_eq!(bd.by_profile["enron_like"].sheets, 1);
    assert_eq!(bd.by_boundary["sheet_start"].gt_tables, 1);
    assert_eq!(bd.by_table_op["locale_numbers"].gt_tables, 1);
    let num = &bd.values["number"];
    assert_eq!((num.total, num.ok), (1, 1), "1.234,50 parses to 1234.5");
    let miss = &bd.values["missing"];
    assert_eq!((miss.total, miss.ok), (1, 1), "'-' is a missing token");
    assert!(bd.text().contains("value parsers"));
    assert!(bd.to_json().starts_with("{\"by_profile\""));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_dir_is_an_error() {
    let dir = temp_dir("empty");
    assert!(load(&dir).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
