use hh_core::eval::metrics::{eob, iou, table_matches, Confusion, FileMacro, Prf, TableCriterion};
use hh_core::eval::split::{split_of, Split};
use hh_core::{Rect, Role};

fn r(top: u32, left: u32, bottom: u32, right: u32) -> Rect {
    Rect { top, left, bottom, right }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn iou_and_eob() {
    let a = r(0, 0, 9, 9); // 100 cells
    assert!(close(iou(a, a), 1.0));
    assert!(close(iou(a, r(20, 20, 21, 21)), 0.0));
    // 10x5 overlap: inter 50, union 100 + 100 - 50.
    assert!(close(iou(a, r(0, 5, 9, 14)), 50.0 / 150.0));
    // Contained: 25 / 100.
    assert!(close(iou(a, r(0, 0, 4, 4)), 0.25));
    assert_eq!(eob(a, a), 0);
    assert_eq!(eob(a, r(1, 0, 9, 11)), 2);
    assert_eq!(eob(a, r(0, 0, 4, 4)), 5);
}

#[test]
fn table_matching_is_one_to_one_and_thresholded() {
    let gt = [r(0, 0, 9, 9), r(20, 0, 29, 9)];
    // Exact copy of table 1, a slightly short table 2 (one row missing → IoU 0.9, EoB 1),
    // a duplicate of table 1 (must not match twice), and junk.
    let pred = [r(0, 0, 9, 9), r(20, 0, 28, 9), r(0, 0, 9, 9), r(50, 50, 50, 51)];
    assert_eq!(table_matches(&gt, &pred, TableCriterion::Iou(0.5)), 2);
    assert_eq!(table_matches(&gt, &pred, TableCriterion::Iou(0.8)), 2);
    assert_eq!(table_matches(&gt, &pred, TableCriterion::Iou(1.0)), 1);
    assert_eq!(table_matches(&gt, &pred, TableCriterion::Eob(0)), 1);
    assert_eq!(table_matches(&gt, &pred, TableCriterion::Eob(2)), 2);
    let p = Prf::from_matches(2, gt.len(), pred.len());
    assert_eq!(p, Prf { tp: 2, fp: 2, fn_: 0 });
    assert!(close(p.precision(), 0.5));
    assert!(close(p.recall(), 1.0));
    assert!(close(p.f1().unwrap(), 4.0 / 6.0));
    assert_eq!(table_matches(&gt, &[], TableCriterion::Iou(0.5)), 0);
    assert_eq!(Prf::default().f1(), None);
}

#[test]
fn greedy_prefers_best_pair() {
    // pred[0] overlaps both GT tables; best pairing gives 2 matches only if pred[0]→gt[0].
    let gt = [r(0, 0, 9, 9), r(0, 0, 9, 4)];
    let pred = [r(0, 0, 9, 9), r(0, 0, 9, 4)];
    assert_eq!(table_matches(&gt, &pred, TableCriterion::Iou(0.5)), 2);
}

#[test]
fn confusion_per_class_and_macro() {
    let mut c = Confusion::default();
    // 3 header cells: 2 right, 1 called data. 5 data cells: 4 right, 1 called header.
    for _ in 0..2 {
        c.add(Role::Header, Role::Header);
    }
    c.add(Role::Header, Role::Data);
    for _ in 0..4 {
        c.add(Role::Data, Role::Data);
    }
    c.add(Role::Data, Role::Header);
    let h = c.class(Role::Header);
    assert_eq!(h, Prf { tp: 2, fp: 1, fn_: 1 });
    let d = c.class(Role::Data);
    assert_eq!(d, Prf { tp: 4, fp: 1, fn_: 1 });
    let f1 = c.class_f1();
    assert!(close(f1[1].unwrap(), 4.0 / 6.0));
    assert!(close(f1[2].unwrap(), 8.0 / 10.0));
    assert_eq!(f1[0], None, "title absent on both sides → undefined");
    assert!(close(c.macro_f1().unwrap(), (4.0 / 6.0 + 0.8) / 2.0));
    assert_eq!(c.total(), 8);
    assert_eq!(c.correct(), 6);

    // A class only predicted counts as F1 = 0.
    let mut c2 = Confusion::default();
    c2.add(Role::Data, Role::Total);
    assert_eq!(c2.class(Role::Total).f1(), Some(0.0));
    assert_eq!(c2.class(Role::Data).f1(), Some(0.0));
}

#[test]
fn file_macro_averages_sheets_then_files() {
    let perfect = {
        let mut c = Confusion::default();
        c.add(Role::Data, Role::Data);
        c
    };
    let wrong = {
        let mut c = Confusion::default();
        c.add(Role::Data, Role::Other);
        c
    };
    let mut fm = FileMacro::default();
    // File A: two sheets, F1(data) = 1 and 0 → 0.5.
    fm.add_sheet(&perfect);
    fm.add_sheet(&wrong);
    fm.end_file();
    // File B: one perfect sheet → 1.0.
    fm.add_sheet(&perfect);
    fm.end_file();
    let pc = fm.per_class();
    assert!(close(pc[2].unwrap(), 0.75));
    // Other: only defined in file A (predicted, never right) → 0.
    assert!(close(pc[6].unwrap(), 0.0));
    assert_eq!(fm.support()[2], 2);
    assert_eq!(fm.support()[6], 1);
    assert!(close(fm.macro_f1().unwrap(), 0.375));
}

#[test]
fn split_is_deterministic_and_roughly_70_30() {
    assert_eq!(split_of("andrea_ring__30__ENRONGAS(1200).xlsx"), split_of("andrea_ring__30__ENRONGAS(1200).xlsx"));
    let dev = (0..2000).filter(|i| split_of(&format!("file_{i}.xlsx")) == Split::Dev).count();
    assert!((1300..1500).contains(&dev), "dev share {dev}/2000");
    assert!(Split::All.contains("x"));
    assert_eq!("test".parse::<Split>().unwrap(), Split::Test);
}
