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

/// One tree node, 16 bytes: a split (`x[feat] < thr` → left) or, with `feat == LEAF`, a leaf whose
/// value is `thr`. Child indices are absolute.
#[derive(Debug, Clone, Copy)]
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
        let mut m = TreeModel { features, classes, base, nodes: Vec::new(), roots: Vec::new(), used: Vec::new() };
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
        Ok(m)
    }

    pub fn trees(&self) -> usize {
        self.roots.len()
    }

    /// Raw class scores, row-major (`rows × classes`). Errors if a model feature is missing.
    pub fn scores(&self, f: &Features) -> Result<Vec<f32>, String> {
        let cols: Vec<&[f32]> = self
            .used
            .iter()
            .map(|&k| f.get(&self.features[k]).ok_or(format!("feature `{}` not computed", self.features[k])))
            .collect::<Result<_, _>>()?;
        let (k, nu, rows) = (self.classes.len(), cols.len(), f.rows());
        let mut out = vec![0f32; rows * k];
        for s in out.chunks_exact_mut(k) {
            s.copy_from_slice(&self.base);
        }
        let mut x = vec![0f32; BLOCK * nu];
        for start in (0..rows).step_by(BLOCK) {
            let n = BLOCK.min(rows - start);
            for (c, col) in cols.iter().enumerate() {
                for b in 0..n {
                    x[b * nu + c] = col[start + b];
                }
            }
            for &(root, class) in &self.roots {
                for b in 0..n {
                    let xb = &x[b * nu..(b + 1) * nu];
                    let mut node = self.nodes[root as usize];
                    while node.feat != LEAF {
                        let next = if xb[node.feat as usize] < node.thr { node.left } else { node.right };
                        node = self.nodes[next as usize];
                    }
                    out[(start + b) * k + class as usize] += node.thr;
                }
            }
        }
        Ok(out)
    }

    /// Like [`TreeModel::scores`], but row `i` only evaluates the trees of classes `c` with
    /// `allowed(i, c)`; the scores of other classes stay at their base value.
    pub fn scores_where(&self, f: &Features, allowed: impl Fn(usize, usize) -> bool) -> Result<Vec<f32>, String> {
        let cols: Vec<&[f32]> = self
            .used
            .iter()
            .map(|&k| f.get(&self.features[k]).ok_or(format!("feature `{}` not computed", self.features[k])))
            .collect::<Result<_, _>>()?;
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
            for b in 0..n {
                for c in 0..k {
                    mask[b * k + c] = allowed(start + b, c);
                }
            }
            for &(root, class) in &self.roots {
                for b in 0..n {
                    if !mask[b * k + class as usize] {
                        continue;
                    }
                    let xb = &x[b * nu..(b + 1) * nu];
                    let mut node = self.nodes[root as usize];
                    while node.feat != LEAF {
                        let next = if xb[node.feat as usize] < node.thr { node.left } else { node.right };
                        node = self.nodes[next as usize];
                    }
                    out[(start + b) * k + class as usize] += node.thr;
                }
            }
        }
        Ok(out)
    }

    /// Role per row of `f` (the class with the highest score). Errors if a model feature is missing.
    pub fn predict(&self, f: &Features) -> Result<Vec<Role>, String> {
        let k = self.classes.len();
        Ok(self
            .scores(f)?
            .chunks_exact(k)
            .map(|s| self.classes[(0..k).fold(0, |b, c| if s[c] > s[b] { c } else { b })])
            .collect())
    }
}

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
    let sheet = grid.sheet;
    let nf = f.names().len();
    let cols: Vec<&[f32]> = (0..nf).map(|i| f.column(i)).collect();
    let mut rows: Vec<u32> = sheet.cells.iter().map(|c| c.row).collect();
    rows.sort_unstable();
    rows.dedup();
    // Mean features of every non-empty row, row-major.
    let mut means = vec![0f32; rows.len() * nf];
    for (ri, &r) in rows.iter().enumerate() {
        let idx = grid.row_cells(r);
        let m = &mut means[ri * nf..(ri + 1) * nf];
        for &i in idx {
            for (k, c) in cols.iter().enumerate() {
                m[k] += c[i as usize];
            }
        }
        let n = idx.len().max(1) as f32;
        m.iter_mut().for_each(|x| *x /= n);
    }
    let mean_of = |r: u32| rows.binary_search(&r).ok().map(|ri| &means[ri * nf..(ri + 1) * nf]);
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
    let names = f.names();
    let (m_names, f_names, u_names, d_names) =
        (prefixed("m", names), prefixed("f", names), prefixed("u", names), prefixed("d", names));
    for (k, c) in cols.iter().enumerate() {
        out.add(m_names[k], segs.iter().map(|s| s.members.iter().map(|&i| c[i as usize]).sum::<f32>() / s.members.len() as f32).collect());
    }
    for (k, c) in cols.iter().enumerate() {
        out.add(f_names[k], segs.iter().map(|s| c[s.members[0] as usize]).collect());
    }
    for k in 0..nf {
        out.add(u_names[k], segs.iter().map(|s| s.row.checked_sub(1).and_then(mean_of).map_or(0.0, |m| m[k])).collect());
    }
    for k in 0..nf {
        out.add(d_names[k], segs.iter().map(|s| mean_of(s.row + 1).map_or(0.0, |m| m[k])).collect());
    }
    (segs, out)
}

/// The row model shipped with hedgehog (`core/models/`, trained by `tools/train/train_rows.py` on
/// DECO, StatSheets, Strudel and synthetic sheets), parsed on first use. `None` only if the
/// embedded files are broken, which the tests rule out.
pub fn embedded() -> Option<&'static RowModel> {
    static MODEL: std::sync::OnceLock<Option<RowModel>> = std::sync::OnceLock::new();
    MODEL
        .get_or_init(|| {
            RowModel::from_json(include_str!("../models/row_coarse.json"), include_str!("../models/row_fine.json")).ok()
        })
        .as_ref()
}

/// The two-stage row model: `coarse` (title / header / data / other) chooses the family, `fine`
/// (the seven roles) chooses inside it.
#[derive(Debug, Clone)]
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

    /// Role per row of `f` (one per segment).
    pub fn predict(&self, f: &Features) -> Result<Vec<Role>, String> {
        let coarse_of = |r: Role| crate::eval::benchmark::coarse(r);
        let families = self.coarse.predict(f)?;
        // Only the fine roles inside a row's family can win: skip the other roles' trees, and a
        // family with a single fine role (title) needs no fine trees at all.
        let fine_classes = &self.fine.classes;
        let single: Vec<bool> = families
            .iter()
            .map(|&fam| fine_classes.iter().filter(|&&c| coarse_of(c) == fam).count() <= 1)
            .collect();
        let pf = self.fine.scores_where(f, |i, c| !single[i] && coarse_of(fine_classes[c]) == families[i])?;
        let kf = fine_classes.len();
        Ok((0..f.rows())
            .map(|i| {
                let family = families[i];
                let sf = &pf[i * kf..(i + 1) * kf];
                let mut best: Option<usize> = None;
                for c in 0..kf {
                    if coarse_of(self.fine.classes[c]) == family && best.is_none_or(|b| sf[c] > sf[b]) {
                        best = Some(c);
                    }
                }
                best.map_or(family, |b| self.fine.classes[b])
            })
            .collect())
    }
}

/// Cell roles from the row model; tables (rectangles) from the rule layout. Every cell of a row
/// segment gets the segment's role, except cells the rules set apart from the rest of their
/// segment (a group label in the key column of a data row): those keep the rules' role.
pub fn analyze(grid: &Grid, model: &RowModel) -> SheetLayout {
    let t0 = std::time::Instant::now();
    let mut lay = crate::layout::analyze(grid);
    crate::normalize::trace("M:layout", t0);
    let f = crate::eval::export::sheet_features_with(grid, &lay);
    crate::normalize::trace("M:cell_features", t0);
    let (segs, sf) = segment_features(grid, &f);
    crate::normalize::trace("M:segment_features", t0);
    let Ok(pred) = model.predict(&sf) else { return lay };
    crate::normalize::trace("M:trees", t0);
    let rules = lay.roles.clone();
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
    lay
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let sheet = Sheet { name: "s".into(), cells, merges: vec![] };
        let grid = Grid::new(&sheet);
        let lay = analyze(&grid, m);
        let role_at = |row: u32, col: u32| lay.roles[grid.cell_index(row, col).unwrap()];
        assert_eq!(role_at(0, 0), Role::Title);
        assert_eq!(role_at(2, 1), Role::Header);
        assert_eq!(role_at(4, 1), Role::Data);
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
