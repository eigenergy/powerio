//! PyO3 extension behind the `caseio` Python package.
//!
//! The dependency-light half of the Python story: parse, lossless write, and
//! cross-format convert, with no numpy/scipy. Everything crosses the boundary
//! as plain dicts and strings, so `import caseio` pulls in nothing but the
//! interpreter. The matrix builders live in the separate `casemat` package.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use caseio::{IndexCore, IndexedNetwork, Network};

pyo3::create_exception!(
    _caseio,
    CaseioError,
    pyo3::exceptions::PyException,
    "Error raised by the caseio parser or converter."
);

/// I/O failures map to the matching `OSError` subclass; an unknown/uninferable
/// format becomes a `ValueError`; other parse and data errors become
/// [`CaseioError`].
fn to_pyerr(e: caseio::Error) -> PyErr {
    match e {
        caseio::Error::Io(io) => io.into(),
        caseio::Error::UnknownFormat(msg) => PyValueError::new_err(msg),
        other => CaseioError::new_err(other.to_string()),
    }
}

/// A parsed power network: the format-neutral [`Network`] exposed to Python as
/// dict tables plus topology diagnostics. No matrices — those are in `casemat`.
///
/// The derived [`IndexCore`] is built once and cached alongside `inner`, so the
/// topology getters reuse it instead of rebuilding the bus-id map per call.
#[pyclass(name = "PyCase")]
pub struct PyCase {
    inner: Network,
    core: IndexCore,
}

#[pymethods]
impl PyCase {
    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    #[getter]
    fn base_mva(&self) -> f64 {
        self.inner.base_mva
    }

    #[getter]
    fn source_format(&self) -> String {
        format!("{:?}", self.inner.source_format)
    }

    #[getter]
    fn n(&self) -> usize {
        self.inner.buses.len()
    }

    #[getter]
    fn n_branches(&self) -> usize {
        self.inner.branches.len()
    }

    #[getter]
    fn n_gens(&self) -> usize {
        self.inner.generators.len()
    }

    #[getter]
    fn n_loads(&self) -> usize {
        self.inner.loads.len()
    }

    #[getter]
    fn n_shunts(&self) -> usize {
        self.inner.shunts.len()
    }

    #[getter]
    fn is_radial(&self) -> bool {
        IndexedNetwork::with_core(&self.inner, &self.core).is_radial()
    }

    #[getter]
    fn n_connected_components(&self) -> usize {
        IndexedNetwork::with_core(&self.inner, &self.core).n_connected_components()
    }

    /// Dense `[0, n)` index of the single reference bus. Raises if not exactly
    /// one reference bus is present.
    fn reference_bus_index(&self) -> PyResult<usize> {
        IndexedNetwork::with_core(&self.inner, &self.core)
            .reference_bus_index()
            .map_err(to_pyerr)
    }

    #[getter]
    fn buses<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.buses.len());
        for b in &self.inner.buses {
            let d = PyDict::new(py);
            d.set_item("id", b.id)?;
            d.set_item("type", b.kind.as_str())?;
            d.set_item("vm", b.vm)?;
            d.set_item("va", b.va)?;
            d.set_item("base_kv", b.base_kv)?;
            d.set_item("area", b.area)?;
            d.set_item("zone", b.zone)?;
            d.set_item("vmax", b.vmax)?;
            d.set_item("vmin", b.vmin)?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn loads<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.loads.len());
        for l in &self.inner.loads {
            let d = PyDict::new(py);
            d.set_item("bus", l.bus)?;
            d.set_item("p", l.p)?;
            d.set_item("q", l.q)?;
            d.set_item("in_service", l.in_service)?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn shunts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.shunts.len());
        for s in &self.inner.shunts {
            let d = PyDict::new(py);
            d.set_item("bus", s.bus)?;
            d.set_item("g", s.g)?;
            d.set_item("b", s.b)?;
            d.set_item("in_service", s.in_service)?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn branches<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.branches.len());
        for br in &self.inner.branches {
            let d = PyDict::new(py);
            d.set_item("from", br.from)?;
            d.set_item("to", br.to)?;
            d.set_item("r", br.r)?;
            d.set_item("x", br.x)?;
            d.set_item("b", br.b)?;
            d.set_item("rate_a", br.rate_a)?;
            d.set_item("rate_b", br.rate_b)?;
            d.set_item("rate_c", br.rate_c)?;
            d.set_item("tap", br.tap)?;
            d.set_item("shift", br.shift)?;
            d.set_item("in_service", br.in_service)?;
            d.set_item("angmin", br.angmin)?;
            d.set_item("angmax", br.angmax)?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn gens<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.generators.len());
        for g in &self.inner.generators {
            let d = PyDict::new(py);
            d.set_item("bus", g.bus)?;
            d.set_item("pg", g.pg)?;
            d.set_item("qg", g.qg)?;
            d.set_item("pmax", g.pmax)?;
            d.set_item("pmin", g.pmin)?;
            d.set_item("qmax", g.qmax)?;
            d.set_item("qmin", g.qmin)?;
            d.set_item("vg", g.vg)?;
            d.set_item("mbase", g.mbase)?;
            d.set_item("in_service", g.in_service)?;
            match &g.cost {
                Some(c) => {
                    let cd = PyDict::new(py);
                    cd.set_item("model", c.model)?;
                    cd.set_item("startup", c.startup)?;
                    cd.set_item("shutdown", c.shutdown)?;
                    cd.set_item("ncost", c.ncost)?;
                    cd.set_item("coeffs", &c.coeffs)?;
                    d.set_item("cost", cd)?;
                }
                None => d.set_item("cost", py.None())?,
            }
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    fn connectivity_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let r = IndexedNetwork::with_core(&self.inner, &self.core).connectivity_report();
        let d = PyDict::new(py);
        d.set_item("n_buses", r.n_buses)?;
        d.set_item("n_branches_in_service", r.n_branches_in_service)?;
        d.set_item("n_components", r.n_components)?;
        d.set_item("isolated_buses", r.isolated_buses)?;
        Ok(d)
    }

    /// Serialize back to the source format. For a MATPOWER-parsed case this is
    /// the byte-exact source echo.
    fn write(&self) -> String {
        caseio::write_matpower(&self.inner)
    }

    fn __repr__(&self) -> String {
        format!(
            "Case(name={:?}, n_buses={}, n_branches={}, n_gens={})",
            self.inner.name,
            self.inner.buses.len(),
            self.inner.branches.len(),
            self.inner.generators.len()
        )
    }
}

/// Parse a MATPOWER `.m` file from a path.
#[pyfunction]
fn parse(path: &str) -> PyResult<PyCase> {
    let inner = caseio::parse_matpower_file(path).map_err(to_pyerr)?;
    let core = IndexCore::build(&inner);
    Ok(PyCase { inner, core })
}

/// Parse a MATPOWER case from in-memory `.m` text. When `name` is given it
/// overrides the parsed case name.
#[pyfunction]
#[pyo3(signature = (content, name=None))]
fn parse_string(content: &str, name: Option<&str>) -> PyResult<PyCase> {
    let mut inner = caseio::parse_matpower(content).map_err(to_pyerr)?;
    if let Some(n) = name {
        inner.name = n.to_string();
    }
    let core = IndexCore::build(&inner);
    Ok(PyCase { inner, core })
}

/// Serialize `case` back to MATPOWER `.m` text (byte-exact echo for a
/// MATPOWER-parsed case).
#[pyfunction]
fn write(case: &PyCase) -> String {
    case.write()
}

/// Convert a case file to another format through the neutral hub. Returns
/// `(text, warnings)`. The input format is the file extension unless `from`
/// overrides it.
#[pyfunction]
#[pyo3(signature = (path, to, from=None))]
fn convert(path: &str, to: &str, from: Option<&str>) -> PyResult<(String, Vec<String>)> {
    let target = caseio::target_format_from_name(to)
        .ok_or_else(|| PyValueError::new_err(format!("unknown target format: {to}")))?;
    let net = caseio::read_path(std::path::Path::new(path), from).map_err(to_pyerr)?;
    let conv = caseio::write_as(&net, target);
    Ok((conv.text, conv.warnings))
}

#[pymodule]
fn _caseio(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("CaseioError", m.py().get_type::<CaseioError>())?;
    m.add_class::<PyCase>()?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(parse_string, m)?)?;
    m.add_function(wrap_pyfunction!(write, m)?)?;
    m.add_function(wrap_pyfunction!(convert, m)?)?;
    Ok(())
}
