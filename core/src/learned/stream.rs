//! The row model's input without the per-cell feature matrix (P7, "streamed segment features").
//!
//! The materialized path ([`crate::eval::export::layout_and_features`], then
//! [`super::segment_features_where`], then [`super::RowModel::predict`]) holds ~110 f32 per cell
//! and ~450 per row segment. Every global quantity those features read is a count, a set or a
//! rectangle that a first pass computes ([`Context::new`], [`Values::new`], [`Totals::new`],
//! [`LayoutTotals::new`], the rules layout); after it, the features of a segment need only its own
//! row and the rows directly above and below. So this module walks the sheet row by row with a
//! three-row window, builds each segment's feature vector, and scores segments in blocks of
//! [`BLOCK`] as soon as they are complete. Memory: the first-pass state (a few bytes per cell)
//! instead of ~440 bytes per cell plus ~1.8 kB per segment.
//!
//! Exact by construction: the same functions produce the same per-cell values in the same order
//! (the context's running column counters advance cell by cell in sheet order, as in
//! [`crate::context::compute`]); row means and segment means are the same f32 sums in the same
//! order; and every segment is scored on its own ([`super::RowModel::predict_block`]). Rows can be
//! split into chunks walked on parallel threads: a chunk starts one row early and ends one row
//! late (those rows only provide the window), and its context cursor starts where a single walk
//! would stand. Only row-major sheets (cells sorted by row, then column, no duplicate positions;
//! what the readers produce) take this path; others use the materialized one.

use crate::analysis::{Role, SheetLayout};
use crate::context::{Context, Cursor};
use crate::eval::export::{LayoutTotals, LAYOUT_TOTALS};
use crate::grid::Grid;
use crate::totals::features::Totals;
use crate::values::features::Values;

use super::{RowModel, Scratch, Segment, BLOCK, GEOMETRY, GUARD_FEATURES, MAX_GAP};

/// Cell features, in the order the materialized path adds them: context, values, totals, then
/// layout and total context.
const NB: usize = crate::context::N + crate::values::FEATURES.len() + crate::totals::features::NAMES.len() + LAYOUT_TOTALS.len();
/// Segment features: geometry, then the mean (`m_`), first cell (`f_`), row above (`u_`) and row
/// below (`d_`) of every cell feature.
const W: usize = GEOMETRY.len() + 4 * NB;

/// Cell feature names in [`NB`] order.
fn base_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = crate::context::FEATURES.to_vec();
    v.extend(crate::values::FEATURES.iter().map(|(n, _)| *n));
    v.extend(crate::totals::features::NAMES);
    v.extend(LAYOUT_TOTALS);
    v
}

/// Position of segment feature `name` in a [`W`]-wide segment vector.
fn position(name: &str, base: &rustc_hash::FxHashMap<&str, usize>) -> Option<usize> {
    if let Some(g) = GEOMETRY.iter().position(|g| *g == name) {
        return Some(g);
    }
    let p = ["m_", "f_", "u_", "d_"].iter().position(|p| name.starts_with(p))?;
    Some(GEOMETRY.len() + p * NB + base.get(&name[2..])?)
}

/// Whether [`run`] can take `grid`'s sheet: cells strictly in (row, column) order.
pub(super) fn eligible(grid: &Grid) -> bool {
    grid.sheet.cells.windows(2).all(|w| (w[0].row, w[0].col) < (w[1].row, w[1].col))
}

/// Row segments, the model's raw role per segment and the mid-table guard's inputs per segment
/// ([`GUARD_FEATURES`]); `None` if the model reads a feature this path does not produce (as the
/// materialized path would fail on it).
pub(super) struct Streamed {
    pub segs: Vec<Segment>,
    pub pred: Vec<Role>,
    pub guard: Vec<[f32; 6]>,
}

/// Rules layout and the streamed model input of an [`eligible`] sheet, the first passes on
/// parallel threads for larger sheets (as the materialized path does).
pub(super) fn run(grid: &Grid, model: &RowModel) -> (SheetLayout, Option<Streamed>) {
    let trace = crate::normalize::trace_ms;
    let n = grid.sheet.cells.len();
    let profile = std::time::Instant::now();
    let (lay, ctx, values, (totals, cands)) = if n < crate::eval::export::PARALLEL_MIN_CELLS {
        let t = std::time::Instant::now();
        let lay = crate::layout::analyze_for_model(grid);
        trace("M.layout", t.elapsed());
        let t = std::time::Instant::now();
        let ctx = Context::new(grid);
        let values = Values::new(grid, None);
        let totals = Totals::new(grid);
        trace("M.pass1", t.elapsed());
        (lay, ctx, values, totals)
    } else {
        std::thread::scope(|s| {
            let lay = s.spawn(|| {
                let t = std::time::Instant::now();
                let lay = crate::layout::analyze_for_model(grid);
                (lay, t.elapsed())
            });
            let values = s.spawn(|| {
                let t = std::time::Instant::now();
                (Values::new(grid, None), t.elapsed())
            });
            let totals = s.spawn(|| {
                let t = std::time::Instant::now();
                (Totals::new(grid), t.elapsed())
            });
            let t = std::time::Instant::now();
            let ctx = Context::new(grid);
            let tc = t.elapsed();
            let wait = std::time::Instant::now();
            let (lay, tl) = join(lay.join());
            let (values, tv) = join(values.join());
            let (totals, tt) = join(totals.join());
            trace("prof.pass1.remaining_join_wait", wait.elapsed());
            trace("M.layout", tl);
            trace("M.context", tc);
            trace("M.values", tv);
            trace("M.totals", tt);
            (lay, ctx, values, totals)
        })
    };
    trace("prof.pass1.wall", profile.elapsed());
    let t = std::time::Instant::now();
    let lt = LayoutTotals::new(grid, &lay, &cands);
    drop(cands);
    trace("M.layout_totals", t.elapsed());
    let t = std::time::Instant::now();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_THREADS);
    let out = stream(grid, model, &ctx, &values, &totals, &lt, threads, CHUNK_MIN_CELLS, false).map(|(s, _)| s);
    trace("M.stream", t.elapsed());
    drop(lt);
    (lay, out)
}

/// A scoped thread's result, re-raising its panic.
fn join<T>(r: std::thread::Result<T>) -> T {
    r.unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// The first-pass state every chunk reads.
struct Sources<'a, 'g> {
    grid: &'a Grid<'g>,
    ctx: &'a Context<'g>,
    totals: &'a Totals<'g>,
    lt: &'a LayoutTotals<'g>,
    /// Model feature positions in a segment vector, in each model's `used` order, and the
    /// guard features'.
    coarse: Vec<usize>,
    fine: Vec<usize>,
    guard: [usize; 6],
}

#[allow(clippy::too_many_arguments)]
fn stream<'g>(
    grid: &Grid<'g>,
    model: &RowModel,
    ctx: &Context<'g>,
    values: &Values<'g>,
    totals: &Totals<'g>,
    lt: &LayoutTotals<'g>,
    threads: usize,
    min_chunk_cells: usize,
    keep: bool,
) -> Option<(Streamed, Vec<u64>)> {
    let profile = std::time::Instant::now();
    let names = base_names();
    debug_assert_eq!(names.len(), NB);
    let base: rustc_hash::FxHashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let pos = |m: &super::TreeModel| m.used.iter().map(|&k| position(&m.features[k], &base)).collect::<Option<Vec<_>>>();
    let (coarse, fine) = (pos(&model.coarse)?, pos(&model.fine)?);
    let mut guard = [0usize; 6];
    for (g, name) in guard.iter_mut().zip(GUARD_FEATURES) {
        *g = position(name, &base)?;
    }
    let src = Sources { grid, ctx, totals, lt, coarse, fine, guard };

    // Rows as ranges of cell indices (the sheet is row-major).
    let cells = &grid.sheet.cells;
    let mut rows: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let mut j = i + 1;
        while j < cells.len() && cells[j].row == cells[i].row {
            j += 1;
        }
        rows.push((i, j));
        i = j;
    }
    // Chunks of about equal cell counts, cut at row starts.
    let chunks = threads.min(cells.len() / min_chunk_cells.max(1)).max(1);
    let mut cuts = vec![0usize];
    for k in 1..chunks {
        let target = cells.len() * k / chunks;
        let r = rows.partition_point(|&(a, _)| a < target);
        if r > *cuts.last().unwrap() && r < rows.len() {
            cuts.push(r);
        }
    }
    cuts.push(rows.len());
    // Each chunk walks one row before and after the rows it emits.
    let spans: Vec<(usize, usize, usize, usize)> = cuts
        .windows(2)
        .map(|w| (w[0].saturating_sub(1), w[0], w[1], (w[1] + 1).min(rows.len())))
        .collect();
    let starts: Vec<usize> = spans.iter().map(|s| rows.get(s.0).map_or(cells.len(), |r| r.0)).collect();
    let cursors = ctx.cursors(&starts);
    let rows = &rows;
    let src = &src;
    crate::normalize::trace("prof.stream.setup", profile);
    let profile = std::time::Instant::now();
    let outs: Vec<(Streamed, Vec<u64>)> = if spans.len() == 1 {
        let cur = cursors.into_iter().next().expect("one cursor");
        vec![walk(src, model, values.fork(), cur, rows, spans[0], keep)]
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = spans
                .iter()
                .zip(cursors)
                .map(|(&span, cur)| {
                    let v = values.fork();
                    s.spawn(move || walk(src, model, v, cur, rows, span, keep))
                })
                .collect();
            let wait = std::time::Instant::now();
            let out = handles.into_iter().map(|h| join(h.join())).collect();
            crate::normalize::trace("prof.stream.join_wait", wait);
            out
        })
    };
    crate::normalize::trace("prof.stream.execute_wall", profile);
    let profile = std::time::Instant::now();
    let mut all = Streamed { segs: Vec::new(), pred: Vec::new(), guard: Vec::new() };
    let mut kept = Vec::new();
    for (o, k) in outs {
        all.segs.extend(o.segs);
        all.pred.extend(o.pred);
        all.guard.extend(o.guard);
        kept.extend(k);
    }
    crate::normalize::trace("prof.stream.assemble", profile);
    Some((all, kept))
}

/// Threads for one sheet's stream (as the materialized path's feature groups).
const MAX_THREADS: usize = 4;
/// Fewest cells per chunk.
const CHUNK_MIN_CELLS: usize = 20_000;

/// One row's cell features ([`NB`] per cell, in cell order) and their mean.
struct RowFeats {
    row: u32,
    start: usize,
    end: usize,
    feats: Vec<f32>,
    mean: Vec<f32>,
}

fn row_feats<'g>(src: &Sources<'_, 'g>, values: &mut Values<'g>, cur: &mut Cursor, (start, end): (usize, usize)) -> RowFeats {
    let cells = &src.grid.sheet.cells;
    let mut feats = vec![0f32; (end - start) * NB];
    let mut c = [0f32; crate::context::N];
    for (k, i) in (start..end).enumerate() {
        let f = &mut feats[k * NB..(k + 1) * NB];
        src.ctx.cell(cur, i, &mut c);
        let (a, rest) = f.split_at_mut(crate::context::N);
        a.copy_from_slice(&c);
        let (b, rest) = rest.split_at_mut(crate::values::FEATURES.len());
        b.copy_from_slice(&values.cell(&cells[i]));
        let (t, l) = rest.split_at_mut(crate::totals::features::NAMES.len());
        t.copy_from_slice(&src.totals.cell(i));
        l.copy_from_slice(&src.lt.cell(i));
    }
    // As `segment_features_where`: per feature, the sum over the row's cells in order, then / n.
    let mut mean = vec![0f32; NB];
    for k in 0..end - start {
        for (m, &v) in mean.iter_mut().zip(&feats[k * NB..(k + 1) * NB]) {
            *m += v;
        }
    }
    let n = (end - start).max(1) as f32;
    mean.iter_mut().for_each(|x| *x /= n);
    RowFeats { row: cells[start].row, start, end, feats, mean }
}

/// Walks rows `a..d` of `rows` (span `(a, b, c, d)`), emitting and scoring the segments of rows
/// `b..c`; rows `a` and `c` (when they differ from `b` and `d`) only give the window.
fn walk<'g>(
    src: &Sources<'_, 'g>,
    model: &RowModel,
    mut values: Values<'g>,
    mut cur: Cursor,
    rows: &[(usize, usize)],
    (a, b, c, d): (usize, usize, usize, usize),
    keep: bool,
) -> (Streamed, Vec<u64>) {
    let mut sink = Sink::new(model, src);
    sink.keep = keep.then(Vec::new);
    let mut prev: Option<RowFeats> = None;
    let mut here: Option<RowFeats> = None;
    for ri in a..=d {
        let next = rows.get(ri).filter(|_| ri < d).map(|&r| row_feats(src, &mut values, &mut cur, r));
        // `here` is row ri - 1.
        if let Some(h) = &here
            && ri >= 1
            && (b..c).contains(&(ri - 1))
        {
            emit(src, h, prev.as_ref(), next.as_ref(), &mut sink);
        }
        prev = here.take();
        here = next;
    }
    sink.flush();
    (sink.out, sink.keep.unwrap_or_default())
}

/// The segments of `row` (as `segment_features_where` splits and describes them) into `sink`.
fn emit(src: &Sources<'_, '_>, row: &RowFeats, prev: Option<&RowFeats>, next: Option<&RowFeats>, sink: &mut Sink) {
    let cells = &src.grid.sheet.cells;
    let r = row.row;
    let gap_above = prev.map_or(50, |p| (r - p.row).min(50));
    let gap_below = next.map_or(50, |q| (q.row - r).min(50));
    let up = prev.filter(|p| r.checked_sub(1) == Some(p.row)).map(|p| &p.mean[..]);
    let down = next.filter(|q| r.checked_add(1) == Some(q.row)).map(|q| &q.mean[..]);
    let len = row.end - row.start;
    let mut parts = Vec::new();
    let mut start = 0;
    for k in 1..=len {
        if k == len || cells[row.start + k].col > cells[row.start + k - 1].col + MAX_GAP + 1 {
            parts.push(start..k);
            start = k;
        }
    }
    let feats = &row.feats;
    for (si, part) in parts.iter().enumerate() {
        let v = sink.next_vec();
        let members: Vec<u32> = part.clone().map(|k| (row.start + k) as u32).collect();
        let left = cells[members[0] as usize].col;
        let right = cells[*members.last().unwrap() as usize].col;
        let geom = [
            members.len() as f32,
            (right - left + 1) as f32,
            left as f32,
            r as f32,
            gap_above as f32,
            gap_below as f32,
            members.len() as f32 / len as f32,
            si as f32,
            parts.len() as f32,
        ];
        v[..GEOMETRY.len()].copy_from_slice(&geom);
        let g = GEOMETRY.len();
        let nm = members.len() as f32;
        for k in 0..NB {
            // The whole-row mean was already accumulated in cell order for the neighbours'
            // features. Most rows have one segment, so do not sum all their features twice.
            v[g + k] = if parts.len() == 1 {
                row.mean[k]
            } else {
                part.clone().map(|j| feats[j * NB + k]).sum::<f32>() / nm
            };
            v[g + NB + k] = feats[part.start * NB + k];
            v[g + 2 * NB + k] = up.map_or(0.0, |m| m[k]);
            v[g + 3 * NB + k] = down.map_or(0.0, |m| m[k]);
        }
        sink.push(Segment { row: r, left, right, members });
    }
}

/// Segment vectors waiting to be scored, and the results so far.
struct Sink<'m, 's> {
    model: &'m RowModel,
    pos: (&'s [usize], &'s [usize], [usize; 6]),
    scratch: Scratch,
    /// Up to [`BLOCK`] segment vectors of [`W`] values.
    vecs: Vec<f32>,
    pending: usize,
    out: Streamed,
    /// A hash of every segment vector, when asked to keep them (equivalence checks).
    keep: Option<Vec<u64>>,
}

impl<'m, 's> Sink<'m, 's> {
    fn new(model: &'m RowModel, src: &'s Sources<'_, '_>) -> Sink<'m, 's> {
        Sink {
            model,
            pos: (&src.coarse, &src.fine, src.guard),
            scratch: Scratch::new(model),
            vecs: vec![0.0; BLOCK * W],
            pending: 0,
            out: Streamed { segs: Vec::new(), pred: Vec::new(), guard: Vec::new() },
            keep: None,
        }
    }

    /// The vector of the next segment (filled by the caller, then [`Sink::push`]).
    fn next_vec(&mut self) -> &mut [f32] {
        &mut self.vecs[self.pending * W..(self.pending + 1) * W]
    }

    fn push(&mut self, seg: Segment) {
        let v = &self.vecs[self.pending * W..(self.pending + 1) * W];
        self.out.guard.push(self.pos.2.map(|p| v[p]));
        if let Some(k) = &mut self.keep {
            k.push(vector_hash(v));
        }
        self.out.segs.push(seg);
        self.pending += 1;
        if self.pending == BLOCK {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let n = self.pending;
        if n == 0 {
            return;
        }
        let (pc, pf, _) = self.pos;
        for b in 0..n {
            let v = &self.vecs[b * W..(b + 1) * W];
            for (c, &p) in pc.iter().enumerate() {
                self.scratch.xc[b * pc.len() + c] = v[p];
            }
            for (c, &p) in pf.iter().enumerate() {
                self.scratch.xf[b * pf.len() + c] = v[p];
            }
        }
        self.model.predict_block(&mut self.scratch, n, &mut self.out.pred);
        self.pending = 0;
    }
}

/// FNV-1a over the bits of a segment vector.
fn vector_hash(v: &[f32]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for x in v {
        for b in x.to_bits().to_le_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// Equivalence check of this path against the materialized one on one sheet (a test aid for the
/// P7 gates): for every entry of `threads` (rows split into that many chunks), every segment's
/// vector of all segment features must have the same bits as [`super::segment_features`] gives
/// (compared by a hash per segment), and every role the same as [`RowModel::predict`] gives;
/// every class score of the lockstep tree walk must equal the plain walk's; and the final layout
/// must equal [`super::analyze_on`] with [`super::Input::Materialized`]. Returns the number of
/// segments compared, or the first difference. Sheets the stream does not take (not row-major)
/// only compare the layouts.
pub fn equivalence(grid: &Grid, model: &RowModel, threads: &[usize]) -> Result<usize, String> {
    use super::{analyze_on, segment_features, Input, RefineSteps};
    // The rules' table confidence averages column purities in hash-map order, so its last bits
    // vary from run to run whatever the path (the manifest rounds it to three decimals): it is
    // compared with a tolerance, everything else exactly.
    let shape = |l: &SheetLayout| {
        let t: Vec<_> = l.tables.iter().map(|t| (t.rect, t.header_top, t.header_rows)).collect();
        format!("{t:?} {:?}", l.roles)
    };
    let full = analyze_on(grid, model, RefineSteps::ALL, Input::Materialized);
    let fast = analyze_on(grid, model, RefineSteps::ALL, Input::Auto);
    if shape(&full) != shape(&fast) {
        return Err("layouts differ".into());
    }
    if let Some((a, b)) =
        full.tables.iter().zip(&fast.tables).find(|(a, b)| (a.confidence - b.confidence).abs() > 1e-5)
    {
        return Err(format!("table confidence differs: {} materialized, {} streamed", a.confidence, b.confidence));
    }
    drop((full, fast));
    if !eligible(grid) || model.reads_formats() {
        return Ok(0);
    }
    let (lay, f) = crate::eval::export::layout_and_features(grid, false);
    let (segs, sf) = segment_features(grid, &f);
    drop(f);
    // The materialized segment vectors, hashed in segment-vector order.
    let names = base_names();
    let base: rustc_hash::FxHashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut at = vec![usize::MAX; W];
    for (j, name) in sf.names().iter().enumerate() {
        at[position(name, &base).ok_or(format!("no position for `{name}`"))?] = j;
    }
    if at.contains(&usize::MAX) {
        return Err("the materialized features do not cover every segment feature".into());
    }
    let mut v = vec![0f32; W];
    let want_hash: Vec<u64> = (0..segs.len())
        .map(|k| {
            for (p, &j) in at.iter().enumerate() {
                v[p] = sf.column(j)[k];
            }
            vector_hash(&v)
        })
        .collect();
    let want = model.predict(&sf)?;
    // The lockstep tree evaluation against the plain walk, every class score bit for bit.
    for (name, m) in [("coarse", &model.coarse), ("fine", &model.fine)] {
        let (a, b) = (m.scores(&sf)?, m.scores_reference(&sf)?);
        if let Some(k) = a.iter().zip(&b).position(|(x, y)| x.to_bits() != y.to_bits()) {
            return Err(format!("{name} score {k}: {} lockstep, {} plain walk", a[k], b[k]));
        }
    }
    drop(sf);
    let ctx = Context::new(grid);
    let values = Values::new(grid, None);
    let (totals, cands) = Totals::new(grid);
    let lt = LayoutTotals::new(grid, &lay, &cands);
    for &t in threads {
        let Some((s, hashes)) = stream(grid, model, &ctx, &values, &totals, &lt, t, 1, true) else {
            return Err("stream refused the model".into());
        };
        if s.segs.len() != segs.len() {
            return Err(format!("{t} chunks: {} segments streamed, {} materialized", s.segs.len(), segs.len()));
        }
        for (k, (a, b)) in s.segs.iter().zip(&segs).enumerate() {
            if (a.row, a.left, a.right, &a.members) != (b.row, b.left, b.right, &b.members) {
                return Err(format!("{t} chunks: segment {k} differs"));
            }
        }
        if let Some(k) = (0..segs.len()).find(|&k| hashes[k] != want_hash[k]) {
            return Err(format!("{t} chunks: features of segment {k} (row {}, col {}) differ", segs[k].row, segs[k].left));
        }
        if want != s.pred {
            return Err(format!("{t} chunks: roles differ"));
        }
    }
    Ok(segs.len())
}
