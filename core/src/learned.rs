//! Learned cell-role model (PLAN.md M6): gradient-boosted trees trained in Python
//! (`tools/train/train_cell_roles.py`) and exported to the compact `hh-trees-1` JSON format
//! (`tools/train/export_trees.py`), evaluated here without any ML dependency.
//!
//! A node goes left when `x < threshold` (XGBoost's rule, in f32 like the exported model). Each
//! class score is its base margin plus the leaf values of its trees; the role with the highest
//! score wins. Feature columns are looked up by name in the sheet's [`Features`].

use crate::analysis::{Role, SheetLayout};
use crate::eval::synthetic::{parse_json, J};
use crate::features::Features;
use crate::grid::Grid;

mod stream;
pub use stream::equivalence;
mod tables;
pub use tables::Steps as RefineSteps;

/// One tree node, 16 bytes: a split (`x[feat] < thr` → left) or, with `feat == LEAF`, a leaf whose
/// value is `thr`. Child indices are absolute.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Node {
    feat: u32,
    thr: f32,
    left: u32,
    right: u32,
}

const LEAF: u32 = u32::MAX;

/// Rows evaluated together: every tree walks a whole block while its nodes are in cache.
const BLOCK: usize = 256;

/// A multi-class tree ensemble.
#[derive(Debug, Clone)]
pub struct TreeModel {
    pub features: Vec<String>,
    pub classes: Vec<Role>,
    base: Vec<f32>,
    /// Every tree's nodes, concatenated; `feat` indexes `used`.
    nodes: Vec<Node>,
    /// (root node, class) per tree.
    roots: Vec<(u32, u8)>,
    /// Positions in `features` of the features the trees actually test.
    used: Vec<usize>,
    /// Evaluation form of `nodes` (see [`TreeModel::prepare`]): a leaf points to itself on both
    /// sides with a NaN threshold (`x < NaN` is false, so a walk that reached it stays there), so
    /// every walk takes exactly its tree's depth in steps, without a leaf test.
    walk: Vec<Node>,
    /// Leaf value per node (0 for splits).
    leaf: Vec<f32>,
    /// Depth of each tree (longest root-to-leaf path), in `roots` order.
    depth: Vec<u8>,
}

/// Equal models: the same features, classes, base scores and trees (the evaluation form is derived
/// from them; its NaN thresholds never compare equal).
impl PartialEq for TreeModel {
    fn eq(&self, o: &TreeModel) -> bool {
        (&self.features, &self.classes, &self.base, &self.nodes, &self.roots, &self.used)
            == (&o.features, &o.classes, &o.base, &o.nodes, &o.roots, &o.used)
    }
}

fn role_named(s: &str) -> Option<Role> {
    Role::ALL.iter().copied().find(|r| r.as_str() == s)
}

impl TreeModel {
    /// Parses an `hh-trees-1` model.
    pub fn from_json(text: &str) -> Result<TreeModel, String> {
        let j = parse_json(text)?;
        if j.get("format").and_then(J::str) != Some("hh-trees-1") {
            return Err("not an hh-trees-1 model".into());
        }
        let strings = |k: &str| -> Vec<String> {
            j.get(k).map(J::arr).unwrap_or(&[]).iter().filter_map(|x| x.str().map(str::to_string)).collect()
        };
        let features = strings("features");
        let classes = strings("classes")
            .iter()
            .map(|c| role_named(c).ok_or(format!("unknown class `{c}`")))
            .collect::<Result<Vec<_>, _>>()?;
        let base: Vec<f32> =
            j.get("base").map(J::arr).unwrap_or(&[]).iter().map(|x| x.num().unwrap_or(0.0) as f32).collect();
        if classes.is_empty() || base.len() != classes.len() {
            return Err("model needs one base score per class".into());
        }
        let mut m = TreeModel { features, classes, base, nodes: Vec::new(), roots: Vec::new(), used: Vec::new(), walk: Vec::new(), leaf: Vec::new(), depth: Vec::new() };
        let mut compact: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
        for t in j.get("trees").map(J::arr).unwrap_or(&[]) {
            let nums = |k: &str| -> Vec<f64> { t.get(k).map(J::arr).unwrap_or(&[]).iter().filter_map(J::num).collect() };
            let (f, th, l, r, v) = (nums("f"), nums("t"), nums("l"), nums("r"), nums("v"));
            let n = l.len();
            if n == 0 || [f.len(), th.len(), r.len(), v.len()].iter().any(|&x| x != n) {
                return Err("tree arrays have different lengths".into());
            }
            let class = t.get("class").and_then(J::u32).ok_or("tree without class")? as usize;
            if class >= m.classes.len() {
                return Err(format!("tree class {class} out of range"));
            }
            let off = m.nodes.len() as u32;
            for k in 0..n {
                let (lk, rk) = (l[k] as i64, r[k] as i64);
                if lk < 0 {
                    m.nodes.push(Node { feat: LEAF, thr: v[k] as f32, left: 0, right: 0 });
                    continue;
                }
                let fk = f[k] as usize;
                if lk as usize >= n || rk < 0 || rk as usize >= n || fk >= m.features.len() {
                    return Err("tree node out of range".into());
                }
                let next = m.used.len() as u32;
                let cf = *compact.entry(fk).or_insert_with(|| {
                    m.used.push(fk);
                    next
                });
                m.nodes.push(Node { feat: cf, thr: th[k] as f32, left: off + lk as u32, right: off + rk as u32 });
            }
            m.roots.push((off, class as u8));
        }
        m.prepare()?;
        Ok(m)
    }

    /// Builds the evaluation form (`walk`, `leaf`, `depth`) from `nodes` and `roots`. Errors on
    /// trees that are not trees (a node reached twice, a path deeper than 255).
    fn prepare(&mut self) -> Result<(), String> {
        let mut seen = vec![false; self.nodes.len()];
        self.walk = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if n.feat == LEAF {
                    Node { feat: 0, thr: f32::NAN, left: i as u32, right: i as u32 }
                } else {
                    *n
                }
            })
            .collect();
        self.leaf = self.nodes.iter().map(|n| if n.feat == LEAF { n.thr } else { 0.0 }).collect();
        self.depth.clear();
        for &(root, _) in &self.roots {
            let mut depth = 0u8;
            let mut stack = vec![(root, 0u8)];
            while let Some((i, d)) = stack.pop() {
                if std::mem::replace(&mut seen[i as usize], true) {
                    return Err("tree nodes shared or cyclic".into());
                }
                let n = self.nodes[i as usize];
                if n.feat == LEAF {
                    depth = depth.max(d);
                    continue;
                }
                if d == u8::MAX {
                    return Err("tree deeper than 255 levels".into());
                }
                stack.push((n.left, d + 1));
                stack.push((n.right, d + 1));
            }
            self.depth.push(depth);
        }
        Ok(())
    }

    /// Adds every tree's leaf value to `out` (`n` rows × classes, holding the base scores) for the
    /// `n` rows of `x` (row-major, one value per entry of `used`), tree by tree in model order, so
    /// each score is the same sum in the same order whatever the block size. With `mask`
    /// (`n` × classes), a row only evaluates the trees of the classes it allows.
    ///
    /// The rows of a block walk each tree [`LANES`] at a time in lockstep, for exactly the tree's
    /// depth (leaves loop to themselves, see `walk`): independent loads instead of one dependent
    /// chain per row, and no leaf test. The leaf each row reaches is the one the plain walk
    /// (`x < threshold` → left, until a leaf) reaches.
    fn add_block(&self, x: &[f32], n: usize, mask: Option<&[bool]>, out: &mut [f32]) {
        let (k, nu) = (self.classes.len(), self.used.len());
        if n == 0 {
            return;
        }
        for (&(root, class), &depth) in self.roots.iter().zip(&self.depth) {
            let c = class as usize;
            let mut b = 0;
            while b < n {
                let lanes = LANES.min(n - b);
                let mut active = [false; LANES];
                let mut any = false;
                // Lanes past the last row walk the last row; their result is not used.
                let mut off = [0usize; LANES];
                for l in 0..LANES {
                    off[l] = (b + l).min(n - 1) * nu;
                    if l < lanes {
                        active[l] = mask.is_none_or(|m| m[(b + l) * k + c]);
                        any |= active[l];
                    }
                }
                if any {
                    let mut idx = [root; LANES];
                    for _ in 0..depth {
                        for l in 0..LANES {
                            let node = self.walk[idx[l] as usize];
                            idx[l] = if x[off[l] + node.feat as usize] < node.thr { node.left } else { node.right };
                        }
                    }
                    for l in 0..lanes {
                        if active[l] {
                            out[(b + l) * k + c] += self.leaf[idx[l] as usize];
                        }
                    }
                }
                b += LANES;
            }
        }
    }

    /// The columns of `f` the trees read, in `used` order. Errors if one is missing.
    fn columns<'f>(&self, f: &'f Features) -> Result<Vec<&'f [f32]>, String> {
        self.used
            .iter()
            .map(|&k| f.get(&self.features[k]).ok_or(format!("feature `{}` not computed", self.features[k])))
            .collect()
    }

    pub fn trees(&self) -> usize {
        self.roots.len()
    }

    /// Compact binary form (little endian), read back by [`TreeModel::from_bytes`]: loading is a
    /// copy instead of a JSON parse.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = b"HHTREE01".to_vec();
        let u32le = |b: &mut Vec<u8>, x: u32| b.extend(x.to_le_bytes());
        u32le(&mut b, self.features.len() as u32);
        for f in &self.features {
            u32le(&mut b, f.len() as u32);
            b.extend(f.as_bytes());
        }
        u32le(&mut b, self.classes.len() as u32);
        for (c, base) in self.classes.iter().zip(&self.base) {
            b.push(crate::eval::export::role_id(*c) as u8);
            b.extend(base.to_le_bytes());
        }
        u32le(&mut b, self.used.len() as u32);
        for &u in &self.used {
            u32le(&mut b, u as u32);
        }
        u32le(&mut b, self.nodes.len() as u32);
        for n in &self.nodes {
            for x in [n.feat, n.thr.to_bits(), n.left, n.right] {
                u32le(&mut b, x);
            }
        }
        u32le(&mut b, self.roots.len() as u32);
        for &(root, class) in &self.roots {
            u32le(&mut b, root);
            b.push(class);
        }
        b
    }

    /// Reads [`TreeModel::to_bytes`]; returns the model and the bytes after it.
    pub fn from_bytes(b: &[u8]) -> Result<(TreeModel, &[u8]), String> {
        struct R<'a>(&'a [u8]);
        impl<'a> R<'a> {
            fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
                if self.0.len() < n {
                    return Err("model file is truncated".into());
                }
                let (a, rest) = self.0.split_at(n);
                self.0 = rest;
                Ok(a)
            }
            fn u32(&mut self) -> Result<u32, String> {
                Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
            }
        }
        let mut r = R(b);
        if r.take(8)? != b"HHTREE01" {
            return Err("not an hh tree model".into());
        }
        let nf = r.u32()? as usize;
        let mut features = Vec::with_capacity(nf);
        for _ in 0..nf {
            let len = r.u32()? as usize;
            features.push(String::from_utf8(r.take(len)?.to_vec()).map_err(|e| e.to_string())?);
        }
        let nc = r.u32()? as usize;
        let (mut classes, mut base) = (Vec::with_capacity(nc), Vec::with_capacity(nc));
        for _ in 0..nc {
            let id = r.take(1)?[0] as usize;
            classes.push(*Role::ALL.get(id).ok_or("bad class id")?);
            base.push(f32::from_bits(r.u32()?));
        }
        let nu = r.u32()? as usize;
        let used = (0..nu).map(|_| r.u32().map(|x| x as usize)).collect::<Result<Vec<_>, _>>()?;
        if used.iter().any(|&u| u >= features.len()) {
            return Err("bad feature index".into());
        }
        let nn = r.u32()? as usize;
        let mut nodes = Vec::with_capacity(nn);
        for _ in 0..nn {
            let (feat, thr, left, right) = (r.u32()?, f32::from_bits(r.u32()?), r.u32()?, r.u32()?);
            if feat != LEAF && (feat as usize >= nu || left as usize >= nn || right as usize >= nn) {
                return Err("bad tree node".into());
            }
            nodes.push(Node { feat, thr, left, right });
        }
        let nt = r.u32()? as usize;
        let mut roots = Vec::with_capacity(nt);
        for _ in 0..nt {
            let root = r.u32()?;
            let class = r.take(1)?[0];
            if root as usize >= nn || class as usize >= nc {
                return Err("bad tree root".into());
            }
            roots.push((root, class));
        }
        let mut m = TreeModel { features, classes, base, nodes, roots, used, walk: Vec::new(), leaf: Vec::new(), depth: Vec::new() };
        m.prepare()?;
        Ok((m, r.0))
    }

    /// Raw class scores, row-major (`rows × classes`). Errors if a model feature is missing.
    pub fn scores(&self, f: &Features) -> Result<Vec<f32>, String> {
        self.scores_masked(f, None::<fn(usize, usize) -> bool>)
    }

    /// Like [`TreeModel::scores`], but row `i` only evaluates the trees of classes `c` with
    /// `allowed(i, c)`; the scores of other classes stay at their base value.
    pub fn scores_where(&self, f: &Features, allowed: impl Fn(usize, usize) -> bool) -> Result<Vec<f32>, String> {
        self.scores_masked(f, Some(allowed))
    }

    fn scores_masked(&self, f: &Features, allowed: Option<impl Fn(usize, usize) -> bool>) -> Result<Vec<f32>, String> {
        let cols = self.columns(f)?;
        let (k, nu, rows) = (self.classes.len(), cols.len(), f.rows());
        let mut out = vec![0f32; rows * k];
        for s in out.chunks_exact_mut(k) {
            s.copy_from_slice(&self.base);
        }
        let mut x = vec![0f32; BLOCK * nu];
        let mut mask = vec![false; BLOCK * k];
        for start in (0..rows).step_by(BLOCK) {
            let n = BLOCK.min(rows - start);
            for (c, col) in cols.iter().enumerate() {
                for b in 0..n {
                    x[b * nu + c] = col[start + b];
                }
            }
            if let Some(allowed) = &allowed {
                for b in 0..n {
                    for c in 0..k {
                        mask[b * k + c] = allowed(start + b, c);
                    }
                }
            }
            let m = allowed.as_ref().map(|_| &mask[..n * k]);
            self.add_block(&x[..n * nu], n, m, &mut out[start * k..(start + n) * k]);
        }
        Ok(out)
    }

    /// [`TreeModel::scores`] by the plain walk, one row and one tree at a time (`x < threshold` →
    /// left, until a leaf): the reference the lockstep evaluation is checked against, bit for bit.
    pub fn scores_reference(&self, f: &Features) -> Result<Vec<f32>, String> {
        let cols = self.columns(f)?;
        let (k, rows) = (self.classes.len(), f.rows());
        let mut out = vec![0f32; rows * k];
        for (i, s) in out.chunks_exact_mut(k).enumerate() {
            s.copy_from_slice(&self.base);
            for &(root, class) in &self.roots {
                let mut node = self.nodes[root as usize];
                while node.feat != LEAF {
                    let next = if cols[node.feat as usize][i] < node.thr { node.left } else { node.right };
                    node = self.nodes[next as usize];
                }
                s[class as usize] += node.thr;
            }
        }
        Ok(out)
    }

    /// The winning class of a row's scores (the first of equal maxima).
    fn argmax(&self, s: &[f32]) -> usize {
        (0..self.classes.len()).fold(0, |b, c| if s[c] > s[b] { c } else { b })
    }

    /// Role per row of `f` (the class with the highest score). Errors if a model feature is missing.
    pub fn predict(&self, f: &Features) -> Result<Vec<Role>, String> {
        let k = self.classes.len();
        Ok(self.scores(f)?.chunks_exact(k).map(|s| self.classes[self.argmax(s)]).collect())
    }
}

/// Rows of one tree walked together in lockstep by [`TreeModel::add_block`].
const LANES: usize = 8;

/// Rows into a table body from which the mid-table guard of [`analyze`] applies.
const GUARD_DEPTH: u32 = 10;

/// Segment features the mid-table guard of [`analyze`] reads.
const GUARD_FEATURES: [&str; 6] = ["m_l_role", "u_l_role", "d_l_role", "m_c_row_cells", "u_c_row_cells", "d_c_row_cells"];

/// Empty columns allowed inside one row segment.
pub const MAX_GAP: u32 = 2;

/// Geometry features of a row segment, in column order.
pub const GEOMETRY: [&str; 9] = [
    "g_cells",
    "g_width",
    "g_left",
    "g_row",
    "g_gap_above",
    "g_gap_below",
    "g_share_of_row",
    "g_segment_index",
    "g_segments_in_row",
];

/// The filled cells of one sheet row, split where more than [`MAX_GAP`] empty columns separate
/// them: the unit the row model labels.
#[derive(Debug, Clone)]
pub struct Segment {
    pub row: u32,
    pub left: u32,
    pub right: u32,
    /// Indices into `sheet.cells`, by column.
    pub members: Vec<u32>,
}

/// Prefixed feature names (`m_`, `f_`, `u_`, `d_` + cell feature), built once per process.
fn prefixed(prefix: &str, base: &[&'static str]) -> Vec<&'static str> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static NAMES: Mutex<Option<HashMap<String, &'static str>>> = Mutex::new(None);
    let mut guard = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    base.iter()
        .map(|b| {
            let k = format!("{prefix}_{b}");
            *map.entry(k.clone()).or_insert_with(|| Box::leak(k.into_boxed_str()))
        })
        .collect()
}

/// Row segments of a sheet and their features: [`GEOMETRY`], then for every cell feature in `f`
/// its mean over the segment (`m_`), its value in the first cell (`f_`), and its mean over the whole
/// sheet row above (`u_`) and below (`d_`; 0 when that row is empty). One feature row per segment.
pub fn segment_features(grid: &Grid, f: &Features) -> (Vec<Segment>, Features) {
    segment_features_where(grid, f, |_| true)
}

/// [`segment_features`] with only the segment features `keep` accepts (geometry always): what a
/// model needs, without the memory of the columns it never reads.
pub fn segment_features_where(grid: &Grid, f: &Features, keep: impl Fn(&str) -> bool) -> (Vec<Segment>, Features) {
    let sheet = grid.sheet;
    let names = f.names();
    let nf = names.len();
    let cols: Vec<&[f32]> = (0..nf).map(|i| f.column(i)).collect();
    let (m_names, f_names, u_names, d_names) =
        (prefixed("m", names), prefixed("f", names), prefixed("u", names), prefixed("d", names));
    let mut rows: Vec<u32> = sheet.cells.iter().map(|c| c.row).collect();
    rows.sort_unstable();
    rows.dedup();
    // Mean features of every non-empty row, row-major, for the features needed above / below.
    let row_feats: Vec<usize> = (0..nf).filter(|&k| keep(u_names[k]) || keep(d_names[k])).collect();
    let nr = row_feats.len();
    let mut slot = vec![usize::MAX; nf];
    for (j, &k) in row_feats.iter().enumerate() {
        slot[k] = j;
    }
    let mut means = vec![0f32; rows.len() * nr];
    for (ri, &r) in rows.iter().enumerate() {
        let idx = grid.row_cells(r);
        let m = &mut means[ri * nr..(ri + 1) * nr];
        for &i in idx {
            for (j, &k) in row_feats.iter().enumerate() {
                m[j] += cols[k][i as usize];
            }
        }
        let n = idx.len().max(1) as f32;
        m.iter_mut().for_each(|x| *x /= n);
    }
    let mean_of = |r: u32| rows.binary_search(&r).ok().map(|ri| &means[ri * nr..(ri + 1) * nr]);
    let mut segs = Vec::new();
    let mut geom: Vec<[f32; 9]> = Vec::new();
    for (ri, &r) in rows.iter().enumerate() {
        let idx = grid.row_cells(r);
        let gap_above = if ri > 0 { (r - rows[ri - 1]).min(50) } else { 50 };
        let gap_below = rows.get(ri + 1).map_or(50, |&n| (n - r).min(50));
        let mut start = 0;
        let mut parts = Vec::new();
        for k in 1..=idx.len() {
            if k == idx.len() || sheet.cells[idx[k] as usize].col > sheet.cells[idx[k - 1] as usize].col + MAX_GAP + 1 {
                parts.push(start..k);
                start = k;
            }
        }
        for (si, part) in parts.iter().enumerate() {
            let members = idx[part.clone()].to_vec();
            let left = sheet.cells[members[0] as usize].col;
            let right = sheet.cells[*members.last().unwrap() as usize].col;
            geom.push([
                members.len() as f32,
                (right - left + 1) as f32,
                left as f32,
                r as f32,
                gap_above as f32,
                gap_below as f32,
                members.len() as f32 / idx.len() as f32,
                si as f32,
                parts.len() as f32,
            ]);
            segs.push(Segment { row: r, left, right, members });
        }
    }
    let n = segs.len();
    let mut out = Features::new(n);
    for (g, name) in GEOMETRY.iter().enumerate() {
        out.add(name, geom.iter().map(|x| x[g]).collect());
    }
    for (k, c) in cols.iter().enumerate().filter(|(k, _)| keep(m_names[*k])) {
        out.add(m_names[k], segs.iter().map(|s| s.members.iter().map(|&i| c[i as usize]).sum::<f32>() / s.members.len() as f32).collect());
    }
    for (k, c) in cols.iter().enumerate().filter(|(k, _)| keep(f_names[*k])) {
        out.add(f_names[k], segs.iter().map(|s| c[s.members[0] as usize]).collect());
    }
    for k in (0..nf).filter(|&k| keep(u_names[k])) {
        let j = slot[k];
        out.add(u_names[k], segs.iter().map(|s| s.row.checked_sub(1).and_then(mean_of).map_or(0.0, |m| m[j])).collect());
    }
    for k in (0..nf).filter(|&k| keep(d_names[k])) {
        let j = slot[k];
        out.add(d_names[k], segs.iter().map(|s| mean_of(s.row + 1).map_or(0.0, |m| m[j])).collect());
    }
    (segs, out)
}

/// The row model shipped with hedgehog (`core/models/row_model.bin`, packed from the
/// `row_coarse.json` / `row_fine.json` that `tools/train/train_rows.py` exports; trained on DECO,
/// StatSheets, Strudel and synthetic sheets), loaded on first use. `None` only if the
/// embedded files are broken, which the tests rule out.
pub fn embedded() -> Option<&'static RowModel> {
    static MODEL: std::sync::OnceLock<Option<RowModel>> = std::sync::OnceLock::new();
    MODEL
        .get_or_init(|| RowModel::from_bytes(include_bytes!("../models/row_model.bin")).ok())
        .as_ref()
}

/// The two-stage row model: `coarse` (title / header / data / other) chooses the family, `fine`
/// (the seven roles) chooses inside it.
#[derive(Debug, Clone, PartialEq)]
pub struct RowModel {
    pub coarse: TreeModel,
    pub fine: TreeModel,
}

impl RowModel {
    pub fn from_json(coarse: &str, fine: &str) -> Result<RowModel, String> {
        let (coarse, fine) = (TreeModel::from_json(coarse)?, TreeModel::from_json(fine)?);
        if fine.classes.len() != Role::ALL.len() {
            return Err("the fine model needs all seven roles".into());
        }
        Ok(RowModel { coarse, fine })
    }

    /// Binary form: the coarse model's bytes, then the fine model's ([`TreeModel::to_bytes`]).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = self.coarse.to_bytes();
        b.extend(self.fine.to_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<RowModel, String> {
        let (coarse, rest) = TreeModel::from_bytes(b)?;
        let (fine, rest) = TreeModel::from_bytes(rest)?;
        if !rest.is_empty() || fine.classes.len() != Role::ALL.len() {
            return Err("not a row model".into());
        }
        Ok(RowModel { coarse, fine })
    }

    /// Segment features [`analyze`] reads: those the trees test, and the mid-table guard's.
    pub fn needed_features(&self) -> std::collections::HashSet<&str> {
        let mut need: std::collections::HashSet<&str> = GUARD_FEATURES.into_iter().collect();
        for m in [&self.coarse, &self.fine] {
            need.extend(m.used.iter().map(|&k| m.features[k].as_str()));
        }
        need
    }

    /// Whether the trees test any formatting feature (`?_s_*`): then the reader must read cell
    /// formats for this model.
    pub fn reads_formats(&self) -> bool {
        self.needed_features().iter().any(|n| n.get(1..4) == Some("_s_"))
    }

    /// Role per row of `f` (one per segment).
    pub fn predict(&self, f: &Features) -> Result<Vec<Role>, String> {
        let (cc, cf) = (self.coarse.columns(f)?, self.fine.columns(f)?);
        let rows = f.rows();
        let mut s = Scratch::new(self);
        let mut out = Vec::with_capacity(rows);
        for start in (0..rows).step_by(BLOCK) {
            let n = BLOCK.min(rows - start);
            for (x, cols) in [(&mut s.xc, &cc), (&mut s.xf, &cf)] {
                let nu = cols.len();
                for (c, col) in cols.iter().enumerate() {
                    for b in 0..n {
                        x[b * nu + c] = col[start + b];
                    }
                }
            }
            self.predict_block(&mut s, n, &mut out);
        }
        Ok(out)
    }

    /// Roles of the `n` rows whose features are in `s.xc` (coarse model's `used` order) and
    /// `s.xf` (fine model's), appended to `out`. Every row is scored on its own, so the roles do
    /// not depend on how rows are grouped into blocks.
    fn predict_block(&self, s: &mut Scratch, n: usize, out: &mut Vec<Role>) {
        let coarse_of = |r: Role| crate::eval::benchmark::coarse(r);
        let (kc, kf) = (self.coarse.classes.len(), self.fine.classes.len());
        let (nuc, nuf) = (self.coarse.used.len(), self.fine.used.len());
        for row in s.sc[..n * kc].chunks_exact_mut(kc) {
            row.copy_from_slice(&self.coarse.base);
        }
        self.coarse.add_block(&s.xc[..n * nuc], n, None, &mut s.sc[..n * kc]);
        // Only the fine roles inside a row's family can win: skip the other roles' trees, and a
        // family with a single fine role (title) needs no fine trees at all.
        let fine_classes = &self.fine.classes;
        let mut families = [Role::Other; BLOCK];
        for b in 0..n {
            let fam = self.coarse.classes[self.coarse.argmax(&s.sc[b * kc..(b + 1) * kc])];
            families[b] = fam;
            let single = fine_classes.iter().filter(|&&c| coarse_of(c) == fam).count() <= 1;
            for c in 0..kf {
                s.mask[b * kf + c] = !single && coarse_of(fine_classes[c]) == fam;
            }
        }
        for row in s.sf[..n * kf].chunks_exact_mut(kf) {
            row.copy_from_slice(&self.fine.base);
        }
        self.fine.add_block(&s.xf[..n * nuf], n, Some(&s.mask[..n * kf]), &mut s.sf[..n * kf]);
        for (b, &family) in families.iter().enumerate().take(n) {
            let sf = &s.sf[b * kf..(b + 1) * kf];
            let mut best: Option<usize> = None;
            for c in 0..kf {
                if coarse_of(fine_classes[c]) == family && best.is_none_or(|x| sf[c] > sf[x]) {
                    best = Some(c);
                }
            }
            out.push(best.map_or(family, |x| fine_classes[x]));
        }
    }
}

/// Buffers for one block of [`RowModel::predict_block`].
struct Scratch {
    xc: Vec<f32>,
    xf: Vec<f32>,
    sc: Vec<f32>,
    sf: Vec<f32>,
    mask: Vec<bool>,
}

impl Scratch {
    fn new(m: &RowModel) -> Scratch {
        Scratch {
            xc: vec![0.0; BLOCK * m.coarse.used.len()],
            xf: vec![0.0; BLOCK * m.fine.used.len()],
            sc: vec![0.0; BLOCK * m.coarse.classes.len()],
            sf: vec![0.0; BLOCK * m.fine.classes.len()],
            mask: vec![false; BLOCK * m.fine.classes.len()],
        }
    }
}

/// Cell roles from the row model; tables from the rule layout, refined with the model's row roles
/// ([`tables`]: header-only tables merged, continuations joined, header depth, stacked tables,
/// title / note rows at the edges). Every cell of a row segment gets the segment's role, except
/// cells the rules set apart from the rest of their segment (a group label in the key column of a
/// data row): those keep the rules' role.
pub fn analyze(grid: &Grid, model: &RowModel) -> SheetLayout {
    analyze_with(grid, model, RefineSteps::ALL)
}

/// [`analyze`] with a chosen set of table refinements (`RefineSteps::NONE`: the rules' tables).
pub fn analyze_with(grid: &Grid, model: &RowModel, steps: RefineSteps) -> SheetLayout {
    analyze_on(grid, model, steps, Input::Auto)
}

/// How [`analyze_on`] builds the model's input: streamed where possible (see [`stream`]), or
/// always through the materialized feature matrix. Both give the same result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Auto,
    Materialized,
}

/// [`analyze_with`] with a chosen input path (the equivalence tests compare the two).
pub fn analyze_on(grid: &Grid, model: &RowModel, steps: RefineSteps, input: Input) -> SheetLayout {
    let t0 = std::time::Instant::now();
    let styles = model.reads_formats();
    let (mut lay, segs, mut pred, guard) = if input == Input::Auto && !styles && stream::eligible(grid) {
        let (lay, s) = stream::run(grid, model);
        let Some(s) = s else { return lay };
        crate::normalize::trace("M:features+trees", t0);
        (lay, s.segs, s.pred, Some(s.guard))
    } else {
        let need = model.needed_features();
        let (lay, f) = crate::eval::export::layout_and_features(grid, styles);
        crate::normalize::trace("M:layout+features", t0);
        let (segs, sf) = segment_features_where(grid, &f, |n| need.contains(n));
        drop(f);
        crate::normalize::trace("M:segment_features", t0);
        let Ok(pred) = model.predict(&sf) else { return lay };
        crate::normalize::trace("M:trees", t0);
        let [m, u, d, mc, uc, dc] = GUARD_FEATURES.map(|n| sf.get(n));
        let guard = match (m, u, d, mc, uc, dc) {
            (Some(m), Some(u), Some(d), Some(mc), Some(uc), Some(dc)) => {
                Some((0..sf.rows()).map(|k| [m[k], u[k], d[k], mc[k], uc[k], dc[k]]).collect::<Vec<_>>())
            }
            _ => None,
        };
        (lay, segs, pred, guard)
    };
    let raw_pred = pred.clone();
    // Titles and notes sit at the edges of a table, not between its data rows; and a header row
    // between data rows (a sub-header such as "Males") is much sparser than the data around it. So a row
    // the rules call data, with data rows directly above and below, stays data when the model calls
    // it a title or note, or a header while holding at least half as many cells as its neighbours.
    if let Some(g) = &guard {
        let data = crate::eval::export::role_id(Role::Data) as f32;
        // Deep inside a rules table: at least GUARD_DEPTH rows below the start of its body (the
        // first rows may be a header block the rules missed).
        let deep = |row: u32, col: u32| {
            lay.tables.iter().any(|t| {
                let body = t.header_top + t.header_rows;
                t.rect.left <= col && col <= t.rect.right && row <= t.rect.bottom && row >= body + GUARD_DEPTH
            })
        };
        for (k, p) in pred.iter_mut().enumerate() {
            let [m, u, d, mc, uc, dc] = g[k];
            if !(m == data && u == data && d == data && deep(segs[k].row, segs[k].left)) {
                continue;
            }
            let full = 2.0 * mc >= uc.min(dc);
            if matches!(*p, Role::Title | Role::Note) || (*p == Role::Header && full) {
                *p = Role::Data;
            }
        }
    }
    let rules = lay.roles.clone();
    let mut raw = rules.clone();
    for (seg, &role) in segs.iter().zip(&raw_pred) {
        for &i in &seg.members {
            raw[i as usize] = role;
        }
    }
    for (seg, role) in segs.iter().zip(pred) {
        let mut counts = [0u32; Role::ALL.len()];
        for &i in &seg.members {
            counts[crate::eval::export::role_id(rules[i as usize]) as usize] += 1;
        }
        let main = (0..counts.len()).max_by_key(|&k| (counts[k], std::cmp::Reverse(k))).unwrap();
        for &i in &seg.members {
            let r = rules[i as usize];
            lay.roles[i as usize] = if crate::eval::export::role_id(r) as usize != main { r } else { role };
        }
    }
    tables::refine(grid, &mut lay, &raw, &rules, steps);
    crate::normalize::trace("M:tables", t0);
    // The model's features count every total word (as in training); a row whose only one is
    // incidental (free text in a register) and whose numbers do not add up is a record.
    let incidental = crate::totals::model_records(grid);
    if !incidental.is_empty() {
        for (i, c) in grid.sheet.cells.iter().enumerate() {
            if lay.roles[i] == Role::Total && incidental.binary_search(&c.row).is_ok() {
                lay.roles[i] = Role::Data;
            }
        }
        crate::normalize::trace("M:incidental_totals", t0);
    }
    lay
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_binary_matches_the_json_models() {
        let json = RowModel::from_json(include_str!("../models/row_coarse.json"), include_str!("../models/row_fine.json"))
            .unwrap();
        assert_eq!(embedded(), Some(&json), "core/models/row_model.bin is stale: run `hedgehog pack-model core/models`");
        assert_eq!(RowModel::from_bytes(&json.to_bytes()).unwrap(), json);
        assert!(RowModel::from_bytes(&json.to_bytes()[..100]).is_err());
    }

    #[test]
    fn embedded_model_loads_and_labels_a_table() {
        let m = embedded().expect("embedded model parses");
        assert_eq!((m.coarse.classes.len(), m.fine.classes.len()), (4, 7));
        use crate::model::{Cell, Sheet, Value};
        let cell = |row: u32, col: u32, v: Value| Cell { row, col, value: Some(v), formula: None };
        let mut cells = vec![
            cell(0, 0, Value::Text("Sales report 2024".into())),
            cell(2, 0, Value::Text("Region".into())),
            cell(2, 1, Value::Text("Revenue".into())),
            cell(2, 2, Value::Text("Units".into())),
        ];
        for (k, r) in ["North", "South", "East", "West", "Central"].iter().enumerate() {
            let row = 3 + k as u32;
            cells.push(cell(row, 0, Value::Text(r.to_string())));
            cells.push(cell(row, 1, Value::Number(100.0 + k as f64 * 17.5)));
            cells.push(cell(row, 2, Value::Number(3.0 + k as f64)));
        }
        let sheet = Sheet { name: "s".into(), cells, merges: vec![], formats: vec![] };
        let grid = Grid::new(&sheet);
        let lay = analyze(&grid, m);
        let role_at = |row: u32, col: u32| lay.roles[grid.cell_index(row, col).unwrap()];
        assert_eq!(role_at(0, 0), Role::Title);
        assert_eq!(role_at(2, 1), Role::Header);
        assert_eq!(role_at(4, 1), Role::Data);
    }

    #[test]
    fn filtered_segment_features_match_the_full_ones() {
        use crate::model::{Cell, Sheet, Value};
        let mut cells = Vec::new();
        for r in 0..6u32 {
            for c in [0u32, 1, 2, 6] {
                let v = if r == 0 { Value::Text(format!("h{c}")) } else { Value::Number((r * 10 + c) as f64) };
                cells.push(Cell { row: r * 2, col: c, value: Some(v), formula: None });
            }
        }
        let sheet = Sheet { name: "s".into(), cells, merges: vec![], formats: vec![] };
        let grid = Grid::new(&sheet);
        let f = crate::eval::export::sheet_features(&grid);
        let (_, all) = segment_features(&grid, &f);
        let want = ["g_cells", "m_l_role", "f_c_kind", "u_l_role", "d_c_row_cells"];
        let (_, some) = segment_features_where(&grid, &f, |n| want.contains(&n));
        assert_eq!(some.names().len(), want.len() + GEOMETRY.len() - 1);
        for n in want {
            assert_eq!(some.get(n), all.get(n), "{n}");
        }
    }

    /// A sheet with a bit of everything the features look at: a title, headers, numbers stored as
    /// numbers and as text, dates, formulas and totals, blank rows, side-by-side blocks, segments
    /// split by wide gaps, merges, negative zero, and a note.
    fn mixed_sheet(seed: u64, rows: u32) -> crate::model::Sheet {
        use crate::model::{Cell, Rect, Sheet, Value};
        let mut s = seed;
        let mut rnd = move |n: u64| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 33) % n
        };
        let mut cells = Vec::new();
        let cell = |row: u32, col: u32, v: Value, f: Option<&str>| Cell { row, col, value: Some(v), formula: f.map(str::to_string) };
        cells.push(cell(0, 0, Value::Text("Quarterly report".into()), None));
        for (c, h) in ["Region", "Q1", "Q2", "Q3", "", "", "Code", "Note"].iter().enumerate() {
            if !h.is_empty() {
                cells.push(cell(2, c as u32, Value::Text(h.to_string()), None));
            }
        }
        let mut r = 3;
        while r < rows {
            if rnd(17) == 0 {
                r += 1 + rnd(3) as u32; // blank rows
                continue;
            }
            let total = rnd(23) == 0;
            cells.push(cell(r, 0, Value::Text(if total { "Total".into() } else { format!("Region {}", rnd(9)) }), None));
            for c in 1..4u32 {
                let v = match rnd(7) {
                    0 => Value::Text(format!("{},{}", rnd(900), rnd(99))),
                    1 => Value::Number(-0.0),
                    2 => Value::DateTime(40000.0 + rnd(3000) as f64),
                    _ => Value::Number(rnd(10_000) as f64 / 4.0),
                };
                let f = if total { Some("SUM(B4:B9)") } else { None };
                cells.push(cell(r, c, v, f));
            }
            if rnd(3) > 0 {
                cells.push(cell(r, 6, Value::Text(format!("{:05}", rnd(500))), None));
            }
            if rnd(5) == 0 {
                cells.push(cell(r, 7, Value::Text("see note 1".into()), None));
            }
            if rnd(11) == 0 {
                // A small block to the right.
                cells.push(cell(r, 12, Value::Number(rnd(50) as f64), None));
                cells.push(cell(r, 13, Value::Bool(rnd(2) == 0), None));
            }
            r += 1;
        }
        cells.push(cell(r + 2, 0, Value::Text("Source: statistical office, provisional figures".into()), None));
        let merges = vec![Rect { top: 0, left: 0, bottom: 0, right: 3 }];
        Sheet { name: "s".into(), cells, merges, formats: vec![] }
    }

    #[test]
    fn streamed_input_matches_the_materialized_one() {
        let m = embedded().unwrap();
        for (seed, rows) in [(1, 12), (2, 60), (3, 400), (4, 7)] {
            let sheet = mixed_sheet(seed, rows);
            let grid = Grid::new(&sheet);
            let n = equivalence(&grid, m, &[1, 2, 3, 5]).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
            assert!(n > 0);
        }
        // Not row-major: the materialized path, same layout either way.
        let mut sheet = mixed_sheet(5, 40);
        sheet.cells.reverse();
        let grid = Grid::new(&sheet);
        assert_eq!(equivalence(&grid, m, &[2]), Ok(0));
    }

    #[test]
    fn lockstep_trees_match_the_plain_walk() {
        let m = embedded().unwrap();
        let sheet = mixed_sheet(9, 300);
        let grid = Grid::new(&sheet);
        let f = crate::eval::export::sheet_features(&grid);
        let (_, mut sf) = segment_features(&grid, &f);
        // Values on and around thresholds, NaN and infinities.
        let n = sf.rows();
        let mut extra = Features::new(n);
        let names: Vec<&'static str> = sf.names().to_vec();
        for (j, name) in names.iter().enumerate() {
            let col: Vec<f32> = (0..n)
                .map(|i| match (i + j) % 11 {
                    0 => f32::NAN,
                    1 => f32::INFINITY,
                    2 => f32::NEG_INFINITY,
                    3 => -0.0,
                    _ => sf.column(j)[i],
                })
                .collect();
            extra.add(name, col);
        }
        for f in [&sf, &extra] {
            for t in [&m.coarse, &m.fine] {
                let (a, b) = (t.scores(f).unwrap(), t.scores_reference(f).unwrap());
                assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
            }
        }
        sf = extra;
        assert_eq!(m.predict(&sf).unwrap().len(), n);
    }

    #[test]
    fn broken_trees_are_refused() {
        // A node pointing back at the root would loop forever.
        let text = r#"{"format":"hh-trees-1","classes":["title"],"features":["a"],"base":[0.0],
            "trees":[{"class":0,"f":[0,0,0],"t":[0.5,0,0],"l":[1,-1,-1],"r":[0,-1,-1],"v":[0,1.0,-1.0]}]}"#;
        assert!(TreeModel::from_json(text).is_err());
    }

    #[test]
    fn walks_trees_and_picks_the_best_class() {
        // Two classes; tree 0 (title) splits on `a < 0.5`; tree 1 (data) is a single leaf.
        let text = r#"{"format":"hh-trees-1","classes":["title","data"],"features":["a"],"base":[0.0,0.25],
            "trees":[{"class":0,"f":[0,0,0],"t":[0.5,0,0],"l":[1,-1,-1],"r":[2,-1,-1],"v":[0,1.0,-1.0]},
                     {"class":1,"f":[0],"t":[0],"l":[-1],"r":[-1],"v":[0.5]}]}"#;
        let m = TreeModel::from_json(text).unwrap();
        let mut f = Features::new(2);
        f.add("a", vec![0.0, 1.0]);
        assert_eq!(m.predict(&f).unwrap(), vec![Role::Title, Role::Data]);
        let mut g = Features::new(1);
        g.add("b", vec![0.0]);
        assert!(m.predict(&g).is_err());
        assert!(TreeModel::from_json(r#"{"format":"other"}"#).is_err());
    }
}
