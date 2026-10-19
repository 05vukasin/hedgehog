//! Python bindings. Kept thin: all logic lives in `hh_core`.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[pyclass(frozen)]
struct Workbook(hh_core::Workbook);

#[pymethods]
impl Workbook {
    #[getter]
    fn sheet_names(&self) -> Vec<String> {
        self.0.sheets.iter().map(|s| s.name.clone()).collect()
    }

    fn describe(&self) -> String {
        hh_core::describe(&self.0)
    }

    fn __repr__(&self) -> String {
        format!("Workbook(sheets={:?})", self.sheet_names())
    }
}

#[pyfunction]
fn open(path: &str) -> PyResult<Workbook> {
    hh_core::open(path).map(Workbook).map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Workbook>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    Ok(())
}
