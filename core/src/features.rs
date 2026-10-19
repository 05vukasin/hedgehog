//! Per-cell feature table: one row per entry of `sheet.cells` (same order), named f32 columns.
//! Signal modules each append their columns; layout rules and the learned model read them by name.
//!
//! Lookups by name go through a hash index. In hot loops, resolve the column once
//! (`let c = f.get("c_kind").unwrap();` or [`Features::index_of`] + [`Features::column`]) and
//! index it per cell, rather than calling [`Features::value`] per cell.

use rustc_hash::FxHashMap;

#[derive(Debug, Clone, Default)]
pub struct Features {
    rows: usize,
    names: Vec<&'static str>,
    columns: Vec<Vec<f32>>,
    index: FxHashMap<&'static str, usize>,
}

impl Features {
    pub fn new(rows: usize) -> Self {
        Features { rows, ..Default::default() }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn names(&self) -> &[&'static str] {
        &self.names
    }

    /// Adds a column. Panics on a duplicate name or wrong length — both are programming errors.
    pub fn add(&mut self, name: &'static str, values: Vec<f32>) {
        assert_eq!(values.len(), self.rows, "feature `{name}` has wrong length");
        assert!(!self.index.contains_key(name), "feature `{name}` added twice");
        self.index.insert(name, self.names.len());
        self.names.push(name);
        self.columns.push(values);
    }

    /// Position of column `name` (its index in [`Features::names`]).
    #[inline]
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }

    /// Column by position (see [`Features::index_of`]).
    #[inline]
    pub fn column(&self, i: usize) -> &[f32] {
        &self.columns[i]
    }

    #[inline]
    pub fn get(&self, name: &str) -> Option<&[f32]> {
        self.index_of(name).map(|i| self.columns[i].as_slice())
    }

    /// Value of feature `name` for cell `row` (index into `sheet.cells`).
    #[inline]
    pub fn value(&self, name: &str, row: usize) -> Option<f32> {
        self.get(name).map(|c| c[row])
    }
}

#[cfg(test)]
mod tests {
    use super::Features;

    #[test]
    fn lookup_by_name_and_index() {
        let mut f = Features::new(2);
        f.add("a", vec![1.0, 2.0]);
        f.add("b", vec![3.0, 4.0]);
        assert_eq!(f.names(), &["a", "b"]);
        assert_eq!(f.value("b", 1), Some(4.0));
        assert_eq!(f.index_of("a"), Some(0));
        assert_eq!(f.column(1), &[3.0, 4.0]);
        assert_eq!(f.get("c"), None);
    }

    #[test]
    #[should_panic(expected = "added twice")]
    fn duplicate_panics() {
        let mut f = Features::new(1);
        f.add("a", vec![1.0]);
        f.add("a", vec![2.0]);
    }
}
