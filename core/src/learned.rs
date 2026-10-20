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

/// A multi-class tree ensemble.
#[derive(Debug, Clone)]
pub struct TreeModel {
    pub features: Vec<String>,
    pub classes: Vec<Role>,
    base: Vec<f32>,
    /// Every tree's nodes, concatenated; child indices are absolute (-1 = leaf).
    feat: Vec<u32>,
    thr: Vec<f32>,
    left: Vec<i32>,
    right: Vec<i32>,
    value: Vec<f32>,
    /// (root node, class) per tree.
    roots: Vec<(u32, u8)>,
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
        let mut m = TreeModel {
            features,
            classes,
            base,
            feat: Vec::new(),
            thr: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
            value: Vec::new(),
            roots: Vec::new(),
        };
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
            let off = m.feat.len() as i32;
            for k in 0..n {
                let (lk, rk) = (l[k] as i64, r[k] as i64);
                let leaf = lk < 0;
                if !leaf && (lk as usize >= n || rk < 0 || rk as usize >= n || f[k] as usize >= m.features.len()) {
                    return Err("tree node out of range".into());
                }
                m.feat.push(if leaf { 0 } else { f[k] as u32 });
                m.thr.push(th[k] as f32);
                m.left.push(if leaf { -1 } else { off + lk as i32 });
                m.right.push(if leaf { -1 } else { off + rk as i32 });
                m.value.push(v[k] as f32);
            }
            m.roots.push((off as u32, class as u8));
        }
        Ok(m)
    }

    pub fn trees(&self) -> usize {
        self.roots.len()
    }

    /// Role per row of `f` (one per `sheet.cells` entry). Errors if a model feature is missing.
    pub fn predict(&self, f: &Features) -> Result<Vec<Role>, String> {
        let cols: Vec<&[f32]> = self
            .features
            .iter()
            .map(|n| f.get(n).ok_or(format!("feature `{n}` not computed")))
            .collect::<Result<_, _>>()?;
        let mut x = vec![0f32; cols.len()];
        let mut scores = vec![0f32; self.classes.len()];
        let mut out = Vec::with_capacity(f.rows());
        for i in 0..f.rows() {
            for (xk, c) in x.iter_mut().zip(&cols) {
                *xk = c[i];
            }
            scores.copy_from_slice(&self.base);
            for &(root, class) in &self.roots {
                let mut k = root as usize;
                while self.left[k] >= 0 {
                    k = if x[self.feat[k] as usize] < self.thr[k] { self.left[k] } else { self.right[k] } as usize;
                }
                scores[class as usize] += self.value[k];
            }
            let best = (0..scores.len()).fold(0, |b, c| if scores[c] > scores[b] { c } else { b });
            out.push(self.classes[best]);
        }
        Ok(out)
    }
}

/// Cell roles from the model; tables (rectangles) from the rule layout for now.
pub fn analyze(grid: &Grid, model: &TreeModel) -> SheetLayout {
    let f = crate::eval::export::sheet_features(grid);
    let mut lay = crate::layout::analyze(grid);
    if let Ok(roles) = model.predict(&f) {
        lay.roles = roles;
    }
    lay
}

#[cfg(test)]
mod tests {
    use super::*;

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
