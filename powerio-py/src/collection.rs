//! Shared collection ownership for Python, without eager electrical parsing.
use super::{
    PyPioModule, SincalBalancedSelection, SincalSelection, core_error_pyerr, json_value_to_py,
    parse_options,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use std::sync::Arc;

#[pyclass(name = "_Collection", module = "powerio._powerio")]
pub(super) struct PyCollection {
    inner: Option<powerio::Collection>,
}
impl PyCollection {
    fn inner(&self) -> PyResult<&powerio::Collection> {
        self.inner
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("collection is closed"))
    }
}
fn limits(max_bytes: u64, max_files: usize) -> PyResult<powerio::CollectionLimits> {
    // The caller sets the totals; depth and ratio keep the facade defaults.
    let mut limits = powerio::CollectionLimits::default();
    limits.tree = powerio::SourceTreeLimits::new(
        max_files,
        max_bytes,
        powerio::SourceTreeLimits::default().max_depth(),
    )
    .map_err(|e| core_error_pyerr(&e))?;
    limits.archive = powerio::collection::ArchiveLimits::new(
        max_bytes,
        max_files,
        max_bytes,
        powerio::collection::ArchiveLimits::default().max_ratio(),
    )
    .map_err(|e| core_error_pyerr(&e))?;
    Ok(limits)
}
#[pymethods]
impl PyCollection {
    #[staticmethod]
    fn _open(path: &str, max_bytes: u64, max_files: usize) -> PyResult<Self> {
        Ok(Self {
            inner: Some(
                powerio::Collection::open(path, limits(max_bytes, max_files)?)
                    .map_err(|e| core_error_pyerr(&e))?,
            ),
        })
    }
    #[staticmethod]
    fn _from_zip(
        name: &str,
        bytes: &Bound<'_, PyBytes>,
        max_bytes: u64,
        max_files: usize,
    ) -> PyResult<Self> {
        let limits = limits(max_bytes, max_files)?;
        if bytes.as_bytes().len() as u64 > max_bytes {
            return Err(PyValueError::new_err(
                "collection compressed byte limit exceeded",
            ));
        }
        Ok(Self {
            inner: Some(
                powerio::Collection::from_zip(name, Arc::from(bytes.as_bytes()), limits)
                    .map_err(|e| core_error_pyerr(&e))?,
            ),
        })
    }
    #[staticmethod]
    fn _from_files(
        name: &str,
        files: &Bound<'_, PyDict>,
        max_bytes: u64,
        max_files: usize,
    ) -> PyResult<Self> {
        let limits = limits(max_bytes, max_files)?;
        if files.len() > max_files {
            return Err(PyValueError::new_err("collection file limit exceeded"));
        }
        let mut total = 0u64;
        let mut buffers = Vec::new();
        for (path, bytes) in files {
            let path = powerio::ArtifactPath::new(path.extract::<String>()?)
                .map_err(|e| core_error_pyerr(&e))?;
            let bytes = bytes.cast::<PyBytes>()?;
            total = total.saturating_add(bytes.as_bytes().len() as u64);
            if total > max_bytes {
                return Err(PyValueError::new_err("collection byte limit exceeded"));
            }
            buffers.push((path, Arc::from(bytes.as_bytes())));
        }
        Ok(Self {
            inner: Some(
                powerio::Collection::from_files(name, buffers, limits)
                    .map_err(|e| core_error_pyerr(&e))?,
            ),
        })
    }
    fn _inspect<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let collection = self.inner()?;
        let candidates = collection.inspect().map_err(|e| core_error_pyerr(&e))?;
        json_value_to_py(
            py,
            &serde_json::json!({"entries":collection.entries(),"candidates":candidates}),
        )
    }
    #[pyo3(signature=(selection,sincal=None,sincal_balanced=None))]
    fn _parse(
        &self,
        selection: &str,
        sincal: Option<SincalSelection>,
        sincal_balanced: Option<SincalBalancedSelection>,
    ) -> PyResult<(PyPioModule, Vec<String>, bool)> {
        if selection.len() > 1 << 20 {
            return Err(PyValueError::new_err("selection metadata exceeds 1 MiB"));
        }
        let selection: powerio::CollectionSelection =
            serde_json::from_str(selection).map_err(|e| PyValueError::new_err(e.to_string()))?;
        let options = parse_options(selection.format.as_deref(), sincal, sincal_balanced)?;
        let result = self
            .inner()?
            .parse(&selection, &options)
            .map_err(|e| core_error_pyerr(&e))?;
        Ok((
            PyPioModule {
                module: Some(result.module),
            },
            result.dependencies,
            result.incomplete,
        ))
    }
    fn close(&mut self) {
        self.inner = None;
    }
}
