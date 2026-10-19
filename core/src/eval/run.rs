//! Runner: opens each ground-truth file in turn (never more than one workbook in memory), runs
//! the analyzer on every annotated sheet, and aggregates a [`Report`].

use std::fmt::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use super::gt::{Corpus, GtFile};
use super::json;
use super::metrics::{Confusion, FileMacro, Prf, TABLE_CRITERIA};
use super::split::Split;
use super::Analyzer;
use crate::analysis::Role;
use crate::grid::Grid;
use crate::model::{Rect, Workbook};

const N_CRIT: usize = TABLE_CRITERIA.len();

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub split: Split,
    /// Evaluate only the first N files of the split (sorted by id).
    pub limit: Option<usize>,
    /// Print one line per file to stderr.
    pub progress: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions { split: Split::Dev, limit: None, progress: false }
    }
}

#[derive(Debug, Clone)]
pub struct SheetResult {
    pub file: String,
    pub sheet: String,
    /// How the annotated sheet was found in the workbook: `name`, `name_ci` or `index`.
    pub matched_by: &'static str,
    pub gt_tables: usize,
    pub pred_tables: usize,
    /// True positives per [`TABLE_CRITERIA`] entry.
    pub table_tp: [usize; N_CRIT],
    pub gt_cells: usize,
    /// Ground-truth cells that have a cell in the sheet as read (the ones scored).
    pub scored_cells: usize,
    /// Ground-truth cells the reader did not produce (not scored).
    pub missing_cells: usize,
    pub confusion: Confusion,
    pub analyze_ms: f64,
    /// Analyzer contract violation (e.g. wrong `roles` length); roles then count as Other.
    pub error: Option<String>,
}

impl SheetResult {
    pub fn role_macro_f1(&self) -> Option<f64> {
        self.confusion.macro_f1()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Timing {
    pub load_ms: f64,
    pub open_ms: f64,
    pub analyze_ms: f64,
    pub total_ms: f64,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub corpus: String,
    pub analyzer: String,
    pub split: Split,
    pub limit: Option<usize>,
    pub corpus_files: usize,
    pub corpus_sheets: usize,
    pub unmatched_files: Vec<(String, String)>,
    pub unmapped_labels: u64,
    pub labels: Vec<(String, Option<Role>, u64)>,
    pub notes: Vec<String>,
    pub files_in_split: usize,
    pub files_evaluated: usize,
    pub files_failed: Vec<(String, String)>,
    /// Files whose primary path failed but a fallback copy opened (id, primary error).
    pub files_via_fallback: Vec<(String, String)>,
    pub sheets_not_found: Vec<(String, String)>,
    pub sheets: Vec<SheetResult>,
    pub tables: [Prf; N_CRIT],
    pub confusion: Confusion,
    pub file_macro: FileMacro,
    pub timing: Timing,
}

/// Finds the annotated sheet in the workbook: exact name, then trimmed case-insensitive name,
/// then position.
fn find_sheet(wb: &Workbook, name: &str, index: Option<usize>) -> Option<(usize, &'static str)> {
    if let Some(i) = wb.sheets.iter().position(|s| s.name == name) {
        return Some((i, "name"));
    }
    let lname = name.trim().to_lowercase();
    if let Some(i) = wb.sheets.iter().position(|s| s.name.trim().to_lowercase() == lname) {
        return Some((i, "name_ci"));
    }
    index.filter(|&i| i < wb.sheets.len()).map(|i| (i, "index"))
}

pub fn evaluate(corpus: &Corpus, analyzer: &dyn Analyzer, analyzer_name: &str, opts: &RunOptions) -> Report {
    evaluate_parallel(corpus, analyzer, analyzer_name, opts, 1)
}

/// What one file contributes to the report (computed on a worker, merged in file order).
struct FileOutcome {
    via_fallback: Option<String>,
    failed: Option<String>,
    open_ms: f64,
    /// Annotated sheets in ground-truth order: a result, or `None` if the sheet was not found.
    sheets: Vec<(String, Option<SheetResult>)>,
}

fn evaluate_file(file: &GtFile, analyzer: &dyn Analyzer) -> FileOutcome {
    let t = Instant::now();
    let mut out = FileOutcome { via_fallback: None, failed: None, open_ms: 0.0, sheets: Vec::new() };
    let mut opened = crate::reader::open(&file.path);
    if opened.is_err() {
        for alt in &file.fallback_paths {
            if let Ok(wb) = crate::reader::open(alt) {
                out.via_fallback = Some(opened.err().map(|e| e.to_string()).unwrap_or_default());
                opened = Ok(wb);
                break;
            }
        }
    }
    let wb = match opened {
        Ok(wb) => wb,
        Err(e) => {
            out.failed = Some(e.to_string());
            out.open_ms = ms(t);
            return out;
        }
    };
    out.open_ms = ms(t);

    for gs in &file.sheets {
        let Some((si, matched_by)) = find_sheet(&wb, &gs.name, gs.index) else {
            out.sheets.push((gs.name.clone(), None));
            continue;
        };
        let sheet = &wb.sheets[si];
        let grid = Grid::new(sheet);
        let t = Instant::now();
        let layout = analyzer.analyze(&grid);
        let analyze_ms = ms(t);

        let pred: Vec<Rect> = layout.tables.iter().map(|t| t.rect).collect();
        let table_tp = TABLE_CRITERIA.map(|c| super::metrics::table_matches(&gs.tables, &pred, c));

        let error = (layout.roles.len() != sheet.cells.len()).then(|| {
            format!("analyzer returned {} roles for {} cells", layout.roles.len(), sheet.cells.len())
        });
        let mut confusion = Confusion::default();
        let mut missing = 0;
        for gc in &gs.cells {
            let idx = grid.cell_index(gc.row, gc.col).or_else(|| {
                let m = grid.merge_at(gc.row, gc.col)?;
                grid.cell_index(m.top, m.left)
            });
            match idx {
                Some(i) => {
                    let pred = if error.is_none() { layout.roles[i] } else { Role::Other };
                    confusion.add(gc.role, pred);
                }
                None => missing += 1,
            }
        }
        out.sheets.push((
            gs.name.clone(),
            Some(SheetResult {
                file: file.id.clone(),
                sheet: gs.name.clone(),
                matched_by,
                gt_tables: gs.tables.len(),
                pred_tables: pred.len(),
                table_tp,
                gt_cells: gs.cells.len(),
                scored_cells: gs.cells.len() - missing,
                missing_cells: missing,
                confusion,
                analyze_ms,
                error,
            }),
        ));
    }
    out
}

/// [`evaluate`] on up to `threads` worker threads (each holds one workbook at a time). Files are
/// merged into the report in split order, so everything except the timings is identical to the
/// single-threaded run; `timing.open_ms` / `analyze_ms` are then summed over threads.
pub fn evaluate_parallel(
    corpus: &Corpus,
    analyzer: &dyn Analyzer,
    analyzer_name: &str,
    opts: &RunOptions,
    threads: usize,
) -> Report {
    let start = Instant::now();
    let in_split: Vec<_> = corpus.files.iter().filter(|f| opts.split.contains(&f.id)).collect();
    let files_in_split = in_split.len();
    let take = opts.limit.unwrap_or(usize::MAX);

    let mut rep = Report {
        corpus: corpus.name.clone(),
        analyzer: analyzer_name.to_string(),
        split: opts.split,
        limit: opts.limit,
        corpus_files: corpus.files.len(),
        corpus_sheets: corpus.sheet_count(),
        unmatched_files: corpus.unmatched.iter().map(|u| (u.id.clone(), u.reason.clone())).collect(),
        unmapped_labels: corpus.unmapped_count(),
        labels: corpus.labels.iter().map(|(k, v)| (k.clone(), v.role, v.count)).collect(),
        notes: corpus.notes.clone(),
        files_in_split,
        files_evaluated: 0,
        files_failed: Vec::new(),
        files_via_fallback: Vec::new(),
        sheets_not_found: Vec::new(),
        sheets: Vec::new(),
        tables: [Prf::default(); N_CRIT],
        confusion: Confusion::default(),
        file_macro: FileMacro::default(),
        timing: Timing::default(),
    };

    let files: Vec<&GtFile> = in_split.into_iter().take(take).collect();
    let total = files.len();
    let merge = |rep: &mut Report, n: usize, file: &GtFile, o: FileOutcome| {
        rep.timing.open_ms += o.open_ms;
        if let Some(e) = o.via_fallback {
            rep.files_via_fallback.push((file.id.clone(), e));
        }
        if let Some(e) = o.failed {
            rep.files_failed.push((file.id.clone(), e));
            return;
        }
        rep.files_evaluated += 1;
        for (name, res) in o.sheets {
            let Some(res) = res else {
                rep.sheets_not_found.push((file.id.clone(), name));
                continue;
            };
            rep.timing.analyze_ms += res.analyze_ms;
            if corpus.has_tables {
                for (k, tp) in res.table_tp.iter().enumerate() {
                    rep.tables[k].add(Prf::from_matches(*tp, res.gt_tables, res.pred_tables));
                }
            }
            if corpus.has_roles {
                rep.confusion.merge(&res.confusion);
                rep.file_macro.add_sheet(&res.confusion);
            }
            rep.sheets.push(res);
        }
        rep.file_macro.end_file();
        if opts.progress {
            eprintln!("[{}/{}] {}", n + 1, files_in_split.min(take), file.id);
        }
    };

    let threads = threads.clamp(1, total.max(1));
    if threads == 1 {
        for (n, file) in files.iter().enumerate() {
            let o = evaluate_file(file, analyzer);
            merge(&mut rep, n, file, o);
        }
    } else {
        // Workers take files in order; the merger consumes outcomes in order as they arrive, so
        // at most a few small outcomes wait in memory.
        let next = AtomicUsize::new(0);
        let slots: Vec<Mutex<Option<FileOutcome>>> = (0..total).map(|_| Mutex::new(None)).collect();
        let ready = Condvar::new();
        let lock = Mutex::new(());
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(i) else { break };
                    let o = evaluate_file(file, analyzer);
                    *slots[i].lock().unwrap() = Some(o);
                    let _g = lock.lock().unwrap();
                    ready.notify_all();
                });
            }
            for (n, file) in files.iter().enumerate() {
                let o = {
                    let mut g = lock.lock().unwrap();
                    loop {
                        if let Some(o) = slots[n].lock().unwrap().take() {
                            break o;
                        }
                        g = ready.wait(g).unwrap();
                    }
                };
                merge(&mut rep, n, file, o);
            }
        });
    }
    rep.timing.total_ms = ms(start);
    rep
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn pct(x: f64) -> String {
    format!("{:5.1}", x * 100.0)
}

fn opct(x: Option<f64>) -> String {
    x.map(pct).unwrap_or_else(|| "    -".into())
}

impl Report {
    pub fn scored_cells(&self) -> usize {
        self.sheets.iter().map(|s| s.scored_cells).sum()
    }

    pub fn missing_cells(&self) -> usize {
        self.sheets.iter().map(|s| s.missing_cells).sum()
    }

    /// Compact human-readable scoreboard.
    pub fn scoreboard(&self) -> String {
        let mut o = String::new();
        let lim = self.limit.map(|l| format!(" · limit={l}")).unwrap_or_default();
        let _ = writeln!(o, "hedgehog eval · {} · split={}{} · analyzer={}", self.corpus, self.split, lim, self.analyzer);
        let _ = writeln!(
            o,
            "corpus: {} files matched ({} unmatched), {} annotated sheets, {} unmapped labels",
            self.corpus_files,
            self.unmatched_files.len(),
            self.corpus_sheets,
            self.unmapped_labels
        );
        for n in &self.notes {
            let _ = writeln!(o, "  note: {n}");
        }
        let _ = writeln!(
            o,
            "run: {}/{} files opened ({} failed, {} via fallback copy) · {} sheets ({} not found) · {} cells scored ({} missing in reader)",
            self.files_evaluated,
            self.files_in_split.min(self.limit.unwrap_or(usize::MAX)),
            self.files_failed.len(),
            self.files_via_fallback.len(),
            self.sheets.len(),
            self.sheets_not_found.len(),
            self.scored_cells(),
            self.missing_cells()
        );
        let t = &self.timing;
        let _ = writeln!(
            o,
            "time: load {:.1}s · open {:.1}s · analyze {:.2}s ({:.2} ms/sheet) · eval total {:.1}s",
            t.load_ms / 1e3,
            t.open_ms / 1e3,
            t.analyze_ms / 1e3,
            t.analyze_ms / self.sheets.len().max(1) as f64,
            t.total_ms / 1e3
        );
        let gt: usize = self.sheets.iter().map(|s| s.gt_tables).sum();
        let pred: usize = self.sheets.iter().map(|s| s.pred_tables).sum();
        let _ = writeln!(o, "\ntables (gt {gt}, pred {pred})     P      R     F1");
        for (c, p) in TABLE_CRITERIA.iter().zip(&self.tables) {
            let _ = writeln!(o, "  {:<22} {} {} {}", c.label(), pct(p.precision()), pct(p.recall()), opct(p.f1()));
        }
        let fm = self.file_macro.per_class();
        let _ = writeln!(o, "\ncell roles        support      P      R  F1(pooled)  F1(file-macro)");
        for (k, r) in Role::ALL.iter().enumerate() {
            let p = self.confusion.class(*r);
            let _ = writeln!(
                o,
                "  {:<14} {:>9} {} {}       {}           {}",
                r.as_str(),
                p.tp + p.fn_,
                pct(p.precision()),
                pct(p.recall()),
                opct(p.f1()),
                opct(fm[k])
            );
        }
        let acc = self.confusion.correct() as f64 / self.confusion.total().max(1) as f64;
        let _ = writeln!(
            o,
            "  {:<14} {:>9}               {}           {}   (accuracy {})",
            "macro",
            self.confusion.total(),
            opct(self.confusion.macro_f1()),
            opct(self.file_macro.macro_f1()),
            pct(acc).trim()
        );
        o
    }

    pub fn to_json(&self) -> String {
        let mut o = String::with_capacity(1 << 16);
        o.push('{');
        json::key(&mut o, "corpus", true);
        json::string(&mut o, &self.corpus);
        json::key(&mut o, "analyzer", false);
        json::string(&mut o, &self.analyzer);
        json::key(&mut o, "split", false);
        json::string(&mut o, self.split.as_str());
        json::key(&mut o, "limit", false);
        match self.limit {
            Some(l) => {
                let _ = write!(o, "{l}");
            }
            None => o.push_str("null"),
        }
        let _ = write!(
            o,
            ",\"counts\":{{\"corpus_files\":{},\"corpus_sheets\":{},\"unmatched_files\":{},\"unmapped_labels\":{},\
             \"files_in_split\":{},\"files_evaluated\":{},\"files_failed\":{},\"files_via_fallback\":{},\"sheets_evaluated\":{},\
             \"sheets_not_found\":{},\"cells_scored\":{},\"cells_missing\":{}}}",
            self.corpus_files,
            self.corpus_sheets,
            self.unmatched_files.len(),
            self.unmapped_labels,
            self.files_in_split,
            self.files_evaluated,
            self.files_failed.len(),
            self.files_via_fallback.len(),
            self.sheets.len(),
            self.sheets_not_found.len(),
            self.scored_cells(),
            self.missing_cells()
        );
        let t = &self.timing;
        o.push_str(",\"timing_ms\":{");
        for (i, (k, v)) in
            [("load", t.load_ms), ("open", t.open_ms), ("analyze", t.analyze_ms), ("total", t.total_ms)].iter().enumerate()
        {
            json::key(&mut o, k, i == 0);
            json::num(&mut o, *v);
        }
        o.push('}');

        o.push_str(",\"tables\":{");
        for (i, (c, p)) in TABLE_CRITERIA.iter().zip(&self.tables).enumerate() {
            json::key(&mut o, &c.label(), i == 0);
            prf_json(&mut o, p);
        }
        o.push('}');

        let fm = self.file_macro.per_class();
        let support = self.file_macro.support();
        o.push_str(",\"roles\":{");
        for (k, r) in Role::ALL.iter().enumerate() {
            json::key(&mut o, r.as_str(), k == 0);
            let p = self.confusion.class(*r);
            o.push('{');
            json::key(&mut o, "pooled", true);
            prf_json(&mut o, &p);
            json::key(&mut o, "file_macro_f1", false);
            json::opt_num(&mut o, fm[k]);
            let _ = write!(o, ",\"files_defined\":{}", support[k]);
            o.push('}');
        }
        o.push('}');
        o.push_str(",\"role_macro\":{");
        json::key(&mut o, "pooled_f1", true);
        json::opt_num(&mut o, self.confusion.macro_f1());
        json::key(&mut o, "file_macro_f1", false);
        json::opt_num(&mut o, self.file_macro.macro_f1());
        json::key(&mut o, "accuracy", false);
        json::num(&mut o, self.confusion.correct() as f64 / self.confusion.total().max(1) as f64);
        o.push('}');
        o.push_str(",\"confusion\":{\"order\":[");
        for (k, r) in Role::ALL.iter().enumerate() {
            if k > 0 {
                o.push(',');
            }
            json::string(&mut o, r.as_str());
        }
        o.push_str("],\"gt_by_pred\":");
        matrix_json(&mut o, &self.confusion);
        o.push('}');

        o.push_str(",\"labels\":[");
        for (i, (l, r, n)) in self.labels.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push('{');
            json::key(&mut o, "label", true);
            json::string(&mut o, l);
            json::key(&mut o, "role", false);
            match r {
                Some(r) => json::string(&mut o, r.as_str()),
                None => o.push_str("null"),
            }
            let _ = write!(o, ",\"count\":{n}}}");
        }
        o.push(']');
        pairs_json(&mut o, "unmatched_files", &self.unmatched_files);
        pairs_json(&mut o, "files_failed", &self.files_failed);
        pairs_json(&mut o, "files_via_fallback", &self.files_via_fallback);
        pairs_json(&mut o, "sheets_not_found", &self.sheets_not_found);
        o.push_str(",\"notes\":[");
        for (i, n) in self.notes.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            json::string(&mut o, n);
        }
        o.push(']');

        o.push_str(",\"sheets\":[");
        for (i, s) in self.sheets.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push_str("\n{");
            json::key(&mut o, "file", true);
            json::string(&mut o, &s.file);
            json::key(&mut o, "sheet", false);
            json::string(&mut o, &s.sheet);
            json::key(&mut o, "matched_by", false);
            json::string(&mut o, s.matched_by);
            let _ = write!(o, ",\"gt_tables\":{},\"pred_tables\":{},\"table_tp\":{{", s.gt_tables, s.pred_tables);
            for (k, c) in TABLE_CRITERIA.iter().enumerate() {
                json::key(&mut o, &c.label(), k == 0);
                let _ = write!(o, "{}", s.table_tp[k]);
            }
            let _ = write!(
                o,
                "}},\"gt_cells\":{},\"scored_cells\":{},\"missing_cells\":{},\"correct_cells\":{}",
                s.gt_cells,
                s.scored_cells,
                s.missing_cells,
                s.confusion.correct()
            );
            json::key(&mut o, "role_macro_f1", false);
            json::opt_num(&mut o, s.role_macro_f1());
            o.push_str(",\"class_f1\":{");
            let mut first = true;
            for (r, f) in Role::ALL.iter().zip(s.confusion.class_f1()) {
                if let Some(f) = f {
                    json::key(&mut o, r.as_str(), first);
                    json::num(&mut o, f);
                    first = false;
                }
            }
            o.push('}');
            json::key(&mut o, "analyze_ms", false);
            json::num(&mut o, s.analyze_ms);
            json::key(&mut o, "error", false);
            match &s.error {
                Some(e) => json::string(&mut o, e),
                None => o.push_str("null"),
            }
            o.push('}');
        }
        o.push_str("\n]}\n");
        o
    }
}

fn prf_json(o: &mut String, p: &Prf) {
    let _ = write!(o, "{{\"tp\":{},\"fp\":{},\"fn\":{},\"precision\":", p.tp, p.fp, p.fn_);
    json::num(o, p.precision());
    o.push_str(",\"recall\":");
    json::num(o, p.recall());
    o.push_str(",\"f1\":");
    json::opt_num(o, p.f1());
    o.push('}');
}

fn matrix_json(o: &mut String, c: &Confusion) {
    o.push('[');
    for (g, row) in c.counts.iter().enumerate() {
        if g > 0 {
            o.push(',');
        }
        o.push('[');
        for (p, n) in row.iter().enumerate() {
            if p > 0 {
                o.push(',');
            }
            let _ = write!(o, "{n}");
        }
        o.push(']');
    }
    o.push(']');
}

fn pairs_json(o: &mut String, key: &str, pairs: &[(String, String)]) {
    json::key(o, key, false);
    o.push('[');
    for (i, (a, b)) in pairs.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push('[');
        json::string(o, a);
        o.push(',');
        json::string(o, b);
        o.push(']');
    }
    o.push(']');
}
