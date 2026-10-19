//! Metric functions (pure, unit-tested in `core/tests/eval_metrics.rs`).
//!
//! Table detection: greedy one-to-one matching of predicted to ground-truth rectangles (best
//! pairs first), then precision/recall/F1 over tables, micro-summed across sheets. Criteria:
//! IoU ≥ 0.5 / 0.8 / 1.0 (StatSheets) and EoB-0 / EoB-2 (TableSense, SpreadsheetLLM:
//! max edge offset ≤ k cells).
//!
//! Cell roles: a 7×7 confusion matrix per sheet. Per-class F1 = 2TP / (2TP + FP + FN), undefined
//! when the class occurs neither in ground truth nor prediction on that sheet. File-macro F1:
//! per class, mean over sheets within a file, then mean over files; macro F1 = mean over classes.

use crate::analysis::Role;
use crate::model::Rect;

pub const N_ROLES: usize = Role::ALL.len();

pub fn role_index(r: Role) -> usize {
    Role::ALL.iter().position(|x| *x == r).unwrap()
}

fn area(r: Rect) -> u64 {
    (r.bottom - r.top + 1) as u64 * (r.right - r.left + 1) as u64
}

/// Intersection over union of two inclusive rectangles, in cells.
pub fn iou(a: Rect, b: Rect) -> f64 {
    let top = a.top.max(b.top);
    let bottom = a.bottom.min(b.bottom);
    let left = a.left.max(b.left);
    let right = a.right.min(b.right);
    if top > bottom || left > right {
        return 0.0;
    }
    let inter = (bottom - top + 1) as u64 * (right - left + 1) as u64;
    inter as f64 / (area(a) + area(b) - inter) as f64
}

/// Error-of-boundary: the largest offset among the four edges.
pub fn eob(a: Rect, b: Rect) -> u32 {
    a.top
        .abs_diff(b.top)
        .max(a.bottom.abs_diff(b.bottom))
        .max(a.left.abs_diff(b.left))
        .max(a.right.abs_diff(b.right))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TableCriterion {
    /// IoU ≥ threshold.
    Iou(f64),
    /// EoB ≤ k.
    Eob(u32),
}

impl TableCriterion {
    pub fn label(self) -> String {
        match self {
            TableCriterion::Iou(t) => format!("IoU>={t:.1}"),
            TableCriterion::Eob(k) => format!("EoB-{k}"),
        }
    }

    /// Match score (higher is better) if the pair qualifies.
    fn score(self, gt: Rect, pred: Rect) -> Option<f64> {
        let i = iou(gt, pred);
        match self {
            TableCriterion::Iou(t) => (i > 0.0 && i + 1e-9 >= t).then_some(i),
            TableCriterion::Eob(k) => {
                let e = eob(gt, pred);
                (e <= k).then_some(-(e as f64) + i * 0.5)
            }
        }
    }
}

/// The criteria every report shows, in this order.
pub const TABLE_CRITERIA: [TableCriterion; 5] = [
    TableCriterion::Iou(0.5),
    TableCriterion::Iou(0.8),
    TableCriterion::Iou(1.0),
    TableCriterion::Eob(0),
    TableCriterion::Eob(2),
];

/// Number of one-to-one matches (true positives) between `gt` and `pred` under `crit`,
/// greedy by best score, ties broken by index order.
pub fn table_matches(gt: &[Rect], pred: &[Rect], crit: TableCriterion) -> usize {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, g) in gt.iter().enumerate() {
        for (j, p) in pred.iter().enumerate() {
            if let Some(s) = crit.score(*g, *p) {
                pairs.push((s, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut used_g = vec![false; gt.len()];
    let mut used_p = vec![false; pred.len()];
    let mut n = 0;
    for (_, i, j) in pairs {
        if !used_g[i] && !used_p[j] {
            used_g[i] = true;
            used_p[j] = true;
            n += 1;
        }
    }
    n
}

/// True positive / false positive / false negative counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Prf {
    pub tp: u64,
    pub fp: u64,
    pub fn_: u64,
}

impl Prf {
    /// From a match count and the sizes of both sides.
    pub fn from_matches(tp: usize, n_gt: usize, n_pred: usize) -> Prf {
        Prf { tp: tp as u64, fp: (n_pred - tp) as u64, fn_: (n_gt - tp) as u64 }
    }

    pub fn add(&mut self, o: Prf) {
        self.tp += o.tp;
        self.fp += o.fp;
        self.fn_ += o.fn_;
    }

    /// 0 when nothing was predicted.
    pub fn precision(&self) -> f64 {
        ratio(self.tp, self.tp + self.fp)
    }

    /// 0 when there is no ground truth.
    pub fn recall(&self) -> f64 {
        ratio(self.tp, self.tp + self.fn_)
    }

    /// 2TP / (2TP + FP + FN); `None` when all counts are zero.
    pub fn f1(&self) -> Option<f64> {
        let d = 2 * self.tp + self.fp + self.fn_;
        (d > 0).then(|| (2 * self.tp) as f64 / d as f64)
    }
}

fn ratio(a: u64, b: u64) -> f64 {
    if b == 0 { 0.0 } else { a as f64 / b as f64 }
}

/// Confusion matrix over [`Role::ALL`]: `counts[gt][pred]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Confusion {
    pub counts: [[u64; N_ROLES]; N_ROLES],
}

impl Confusion {
    pub fn add(&mut self, gt: Role, pred: Role) {
        self.counts[role_index(gt)][role_index(pred)] += 1;
    }

    pub fn merge(&mut self, o: &Confusion) {
        for g in 0..N_ROLES {
            for p in 0..N_ROLES {
                self.counts[g][p] += o.counts[g][p];
            }
        }
    }

    pub fn total(&self) -> u64 {
        self.counts.iter().flatten().sum()
    }

    pub fn correct(&self) -> u64 {
        (0..N_ROLES).map(|i| self.counts[i][i]).sum()
    }

    pub fn class(&self, r: Role) -> Prf {
        let k = role_index(r);
        let tp = self.counts[k][k];
        let gt: u64 = self.counts[k].iter().sum();
        let pred: u64 = (0..N_ROLES).map(|g| self.counts[g][k]).sum();
        Prf { tp, fp: pred - tp, fn_: gt - tp }
    }

    /// Per-class F1 (`None` = class absent on both sides).
    pub fn class_f1(&self) -> [Option<f64>; N_ROLES] {
        Role::ALL.map(|r| self.class(r).f1())
    }

    /// Mean of the defined per-class F1 values.
    pub fn macro_f1(&self) -> Option<f64> {
        mean_defined(&self.class_f1())
    }
}

pub fn mean_defined(xs: &[Option<f64>]) -> Option<f64> {
    let v: Vec<f64> = xs.iter().flatten().copied().collect();
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

/// File-macro F1: per class, average sheet F1 within a file, then average files.
/// Call [`FileMacro::add_sheet`] for each sheet and [`FileMacro::end_file`] after each file.
#[derive(Debug, Clone, Default)]
pub struct FileMacro {
    sheet_sum: [f64; N_ROLES],
    sheet_n: [u32; N_ROLES],
    file_sum: [f64; N_ROLES],
    file_n: [u32; N_ROLES],
}

impl FileMacro {
    pub fn add_sheet(&mut self, sheet: &Confusion) {
        for (k, f) in sheet.class_f1().into_iter().enumerate() {
            if let Some(f) = f {
                self.sheet_sum[k] += f;
                self.sheet_n[k] += 1;
            }
        }
    }

    pub fn end_file(&mut self) {
        for k in 0..N_ROLES {
            if self.sheet_n[k] > 0 {
                self.file_sum[k] += self.sheet_sum[k] / self.sheet_n[k] as f64;
                self.file_n[k] += 1;
            }
        }
        self.sheet_sum = [0.0; N_ROLES];
        self.sheet_n = [0; N_ROLES];
    }

    pub fn per_class(&self) -> [Option<f64>; N_ROLES] {
        std::array::from_fn(|k| (self.file_n[k] > 0).then(|| self.file_sum[k] / self.file_n[k] as f64))
    }

    /// Number of files in which each class was defined.
    pub fn support(&self) -> [u32; N_ROLES] {
        self.file_n
    }

    pub fn macro_f1(&self) -> Option<f64> {
        mean_defined(&self.per_class())
    }
}
