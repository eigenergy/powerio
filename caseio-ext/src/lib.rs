//! PyO3 extension behind the `caseio` Python package.
//!
//! The dependency-light half of the Python story: parse, lossless write, and
//! cross-format convert, with no numpy/scipy. Everything crosses the boundary
//! as plain dicts and strings, so `import caseio` pulls in nothing but the
//! interpreter. The matrix builders live in the separate `casemat` package.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use caseio::{BusType, IndexedNetwork, Network, TargetFormat};

pyo3::create_exception!(
    _caseio,
    CaseioError,
    pyo3::exceptions::PyException,
    "Error raised by the caseio parser or converter."
);

/// I/O failures map to the matching `OSError` subclass; parse and data errors
/// become [`CaseioError`].
fn to_pyerr(e: caseio::Error) -> PyErr {
    match e {
        caseio::Error::Io(io) => io.into(),
        other => CaseioError::new_err(other.to_string()),
    }
}

fn bus_type_str(kind: BusType) -> &'static str {
    match kind {
        BusType::Pq => "PQ",
        BusType::Pv => "PV",
        BusType::Ref => "REF",
        BusType::Isolated => "ISOLATED",
    }
}

/// A parsed power network: the format-neutral [`Network`] exposed to Python as
/// dict tables plus topology diagnostics. No matrices — those are in `casemat`.
#[pyclass(name = "PyCase")]
pub struct PyCase {
    inner: Network,
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
        IndexedNetwork::new(&self.inner).is_radial()
    }

    #[getter]
    fn n_connected_components(&self) -> usize {
        IndexedNetwork::new(&self.inner).n_connected_components()
    }

    /// Dense `[0, n)` index of the single reference bus. Raises if not exactly
    /// one reference bus is present.
    fn reference_bus_index(&self) -> PyResult<usize> {
        IndexedNetwork::new(&self.inner).reference_bus_index().map_err(to_pyerr)
    }

    #[getter]
    fn buses<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for b in &self.inner.buses {
            let d = PyDict::new(py);
            d.set_item("id", b.id)?;
            d.set_item("type", bus_type_str(b.kind))?;
            d.set_item("vm", b.vm)?;
            d.set_item("va", b.va)?;
            d.set_item("base_kv", b.base_kv)?;
            d.set_item("area", b.area)?;
            d.set_item("zone", b.zone)?;
            d.set_item("vmax", b.vmax)?;
            d.set_item("vmin", b.vmin)?;
            list.append(d)?;
        }
        Ok(list)
    }

    #[getter]
    fn loads<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for l in &self.inner.loads {
            let d = PyDict::new(py);
            d.set_item("bus", l.bus)?;
            d.set_item("p", l.p)?;
            d.set_item("q", l.q)?;
            d.set_item("in_service", l.in_service)?;
            list.append(d)?;
        }
        Ok(list)
    }

    #[getter]
    fn shunts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for s in &self.inner.shunts {
            let d = PyDict::new(py);
            d.set_item("bus", s.bus)?;
            d.set_item("g", s.g)?;
            d.set_item("b", s.b)?;
            d.set_item("in_service", s.in_service)?;
            list.append(d)?;
        }
        Ok(list)
    }

    #[getter]
    fn branches<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
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
            list.append(d)?;
        }
        Ok(list)
    }

    #[getter]
    fn gens<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
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
                    cd.set_item("coeffs", c.coeffs.clone())?;
                    d.set_item("cost", cd)?;
                }
                None => d.set_item("cost", py.None())?,
            }
            list.append(d)?;
        }
        Ok(list)
    }

    fn connectivity_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let r = IndexedNetwork::new(&self.inner).connectivity_report();
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
    Ok(PyCase { inner })
}

/// Parse a MATPOWER case from in-memory `.m` text. `name` overrides the case
/// name (defaults to "case").
#[pyfunction]
#[pyo3(signature = (content, name=None))]
fn parse_string(content: &str, name: Option<&str>) -> PyResult<PyCase> {
    let mut inner = caseio::parse_matpower(content).map_err(to_pyerr)?;
    if let Some(n) = name {
        inner.name = n.to_string();
    }
    Ok(PyCase { inner })
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
    let target = fmt_from_str(to)
        .ok_or_else(|| PyValueError::new_err(format!("unknown target format: {to}")))?;
    let net = read_network(path, from)?;
    let conv = caseio::write_as(&net, target);
    Ok((conv.text, conv.warnings))
}

/// Map a format name (with common aliases) to a [`TargetFormat`].
fn fmt_from_str(s: &str) -> Option<TargetFormat> {
    Some(match s.to_ascii_lowercase().as_str() {
        "matpower" | "m" => TargetFormat::Matpower,
        "powermodels-json" | "powermodels" | "pm" => TargetFormat::PowerModelsJson,
        "egret-json" | "egret" => TargetFormat::EgretJson,
        "psse" | "raw" => TargetFormat::Psse,
        "powerworld" | "aux" => TargetFormat::PowerWorld,
        _ => return None,
    })
}

/// Read `path` into a [`Network`], choosing the reader from `from` or the file
/// extension. EGRET is write-only.
fn read_network(path: &str, from: Option<&str>) -> PyResult<Network> {
    let p = std::path::Path::new(path);
    let fmt = match from {
        Some(f) => fmt_from_str(f)
            .ok_or_else(|| PyValueError::new_err(format!("unknown source format: {f}")))?,
        None => match p.extension().and_then(|e| e.to_str()) {
            Some("m") => TargetFormat::Matpower,
            Some("json") => TargetFormat::PowerModelsJson,
            Some("raw") => TargetFormat::Psse,
            Some("aux") => TargetFormat::PowerWorld,
            other => {
                return Err(PyValueError::new_err(format!(
                    "cannot infer input format from extension {other:?}; pass from="
                )))
            }
        },
    };
    let read_str = || std::fs::read_to_string(p).map_err(|e| PyValueError::new_err(e.to_string()));
    let net = match fmt {
        TargetFormat::Matpower => caseio::parse_matpower_file(path).map_err(to_pyerr)?,
        TargetFormat::PowerModelsJson => {
            caseio::parse_powermodels_json(&read_str()?).map_err(to_pyerr)?
        }
        TargetFormat::Psse => caseio::parse_psse(&read_str()?).map_err(to_pyerr)?,
        TargetFormat::PowerWorld => caseio::parse_powerworld(&read_str()?).map_err(to_pyerr)?,
        TargetFormat::EgretJson => {
            return Err(PyValueError::new_err(
                "reading EGRET JSON is not supported yet (write-only)",
            ))
        }
    };
    Ok(net)
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
