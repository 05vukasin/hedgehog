//! Per-cell feature table: one row per entry of `sheet.cells` (same order), named f32 columns.
//! Signal modules each append their columns; layout rules and the learned model read them by name.

#[derive(Debug, Clone, Default)]
pub struct Features {
    rows: usize,
    names: Vec<&'static str>,
    columns: Vec<Vec<f32>>,
}

impl Features {
    pub fn new(rows: usize) -> Self {
        Features { rows, names: Vec::new(), columns: Vec::new() }
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
        assert!(!self.names.contains(&name), "feature `{name}` added twice");
        self.names.push(name);
        self.columns.push(values);
    }

    pub fn get(&self, name: &str) -> Option<&[f32]> {
        self.names.iter().position(|n| *n == name).map(|i| self.columns[i].as_slice())
    }

    /// Value of feature `name` for cell `row` (index into `sheet.cells`).
    pub fn value(&self, name: &str, row: usize) -> Option<f32> {
        self.get(name).map(|c| c[row])
    }
}
