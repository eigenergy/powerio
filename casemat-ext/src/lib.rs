//! PyO3 extension module behind the `casemat` Python package.
//!
//! This crate is the thin Rust↔Python boundary. It does no numerics of its
//! own: every method delegates to the `casemat` library and hands the result
//! back as COO triplets (`data`, `row`, `col`, `shape`) of NumPy arrays. The
//! pure-Python `casemat` package (python/casemat/) assembles those into
//! `scipy.sparse` matrices and networkx graphs, so scipy/networkx stay out of
//! the Rust build and missing-dependency errors surface cleanly in Python.
//!
//! COO (not CSR/CSC triplets) is deliberate: explicit per-entry `(row, col)`
//! can't be misread as the transpose the way a raw `indptr`/`indices` pair can
//! if scipy and sprs disagree on row- vs column-major, and it sidesteps the
//! sprs `IndPtr` slice API. Indices are emitted as `i32` to match scipy's
//! default index width; the largest index value is bounded by
//! `max(n_buses, n_branches)` (`2n` for the LACPF block), far under 2³¹, and
//! `coo_triplets` guards the bound anyway.

use numpy::IntoPyArray;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use sprs::CsMat;

use casemat::case::BusType;
use casemat::matrix::{
    build_adjacency, build_bdoubleprime, build_bprime, build_incidence, build_lacpf, build_lodf,
    build_ptdf, build_weighted_laplacian, build_ybus, BuildOptions, DcConvention, Scheme, Units,
};
use casemat::opf_pipeline::{write_dcopf_bundle as write_bundle, DcOpfOptions};
use casemat::MpcCase;

pyo3::create_exception!(
    _casemat,
    CasematError,
    pyo3::exceptions::PyException,
    "Error raised by the casemat parser or matrix builders."
);

/// I/O failures map to the matching `OSError` subclass (`FileNotFoundError`,
/// `PermissionError`, …) so Python callers can catch them the usual way; parse
/// and data errors become [`CasematError`].
fn to_pyerr(e: casemat::Error) -> PyErr {
    match e {
        casemat::Error::Io(io) => io.into(),
        other => CasematError::new_err(other.to_string()),
    }
}

/// `bx` → `Bx`, `xb` → `Xb` (case- and separator-insensitive).
fn parse_scheme(s: &str) -> PyResult<Scheme> {
    match normalize(s).as_str() {
        "bx" => Ok(Scheme::Bx),
        "xb" => Ok(Scheme::Xb),
        other => Err(PyValueError::new_err(format!(
            "unknown scheme {other:?}; expected 'bx' or 'xb'"
        ))),
    }
}

/// Accepts `paper`/`paper-pure`/`pure` and `matpower`/`mp` (case- and
/// separator-insensitive).
fn parse_convention(s: &str) -> PyResult<DcConvention> {
    match normalize(s).as_str() {
        "paper" | "paperpure" | "pure" => Ok(DcConvention::PaperPure),
        "matpower" | "mp" => Ok(DcConvention::Matpower),
        other => Err(PyValueError::new_err(format!(
            "unknown convention {other:?}; expected 'paper' or 'matpower'"
        ))),
    }
}

/// Accepts `perunit`/`pu`/`per-unit` and `native`.
fn parse_units(s: &str) -> PyResult<Units> {
    match normalize(s).as_str() {
        "perunit" | "pu" => Ok(Units::PerUnit),
        "native" => Ok(Units::Native),
        other => Err(PyValueError::new_err(format!(
            "unknown units {other:?}; expected 'perunit' or 'native'"
        ))),
    }
}

fn normalize(s: &str) -> String {
    s.to_ascii_lowercase().replace(['-', '_'], "")
}

fn bus_type_str(kind: BusType) -> &'static str {
    match kind {
        BusType::Pq => "PQ",
        BusType::Pv => "PV",
        BusType::Ref => "REF",
        BusType::Isolated => "ISOLATED",
    }
}

/// Materialize a sparse matrix as a `(data, row, col, (nrows, ncols))` tuple of
/// NumPy arrays. `to_csr()` first so `outer_iterator()` yields rows regardless
/// of the source storage; indices narrow to `i32`. The narrowing is guarded:
/// a dimension past `i32::MAX` raises rather than wrapping to negative indices.
fn coo_triplets<'py>(py: Python<'py>, m: &CsMat<f64>) -> PyResult<Bound<'py, PyAny>> {
    let m = m.to_csr();
    if m.rows() > i32::MAX as usize || m.cols() > i32::MAX as usize {
        return Err(PyValueError::new_err(format!(
            "matrix is {}x{}; an index exceeds i32 range — rebuild with i64 indices",
            m.rows(),
            m.cols()
        )));
    }
    let nnz = m.nnz();
    let mut data: Vec<f64> = Vec::with_capacity(nnz);
    let mut rows: Vec<i32> = Vec::with_capacity(nnz);
    let mut cols: Vec<i32> = Vec::with_capacity(nnz);
    for (r, row) in m.outer_iterator().enumerate() {
        for (c, &v) in row.iter() {
            data.push(v);
            rows.push(r as i32);
            cols.push(c as i32);
        }
    }
    let shape = (m.rows(), m.cols());
    Ok((
        data.into_pyarray(py),
        rows.into_pyarray(py),
        cols.into_pyarray(py),
        shape,
    )
        .into_pyobject(py)?
        .into_any())
}

fn build_options(scheme: Scheme, include_taps: bool, include_shifts: bool) -> BuildOptions {
    BuildOptions {
        scheme,
        include_taps,
        include_shifts,
        ..BuildOptions::default()
    }
}

/// Low-level handle around a parsed `MpcCase`. The user-facing `casemat.Case`
/// (pure Python) wraps this and turns the COO tuples into scipy matrices.
#[pyclass(name = "PyCase")]
pub struct PyCase {
    inner: MpcCase,
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
    fn n(&self) -> usize {
        self.inner.n()
    }

    #[getter]
    fn n_branches(&self) -> usize {
        self.inner.branches.len()
    }

    #[getter]
    fn n_gens(&self) -> usize {
        self.inner.gens.len()
    }

    #[getter]
    fn is_radial(&self) -> bool {
        self.inner.is_radial()
    }

    #[getter]
    fn n_connected_components(&self) -> usize {
        self.inner.n_connected_components()
    }

    /// Dense `[0, n)` index of the single reference bus. Raises if not exactly
    /// one reference bus is present.
    fn reference_bus_index(&self) -> PyResult<usize> {
        self.inner.reference_bus_index().map_err(to_pyerr)
    }

    #[getter]
    fn buses<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for b in &self.inner.buses {
            let d = PyDict::new(py);
            d.set_item("id", b.id)?;
            d.set_item("type", bus_type_str(b.kind))?;
            d.set_item("pd", b.pd)?;
            d.set_item("qd", b.qd)?;
            d.set_item("gs", b.gs)?;
            d.set_item("bs", b.bs)?;
            d.set_item("area", b.area)?;
            d.set_item("vm", b.vm)?;
            d.set_item("va", b.va)?;
            d.set_item("base_kv", b.base_kv)?;
            d.set_item("zone", b.zone)?;
            d.set_item("vmax", b.vmax)?;
            d.set_item("vmin", b.vmin)?;
            list.append(d)?;
        }
        Ok(list)
    }

    #[getter]
    fn branches<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for br in &self.inner.branches {
            let d = PyDict::new(py);
            d.set_item("from_id", br.from_id)?;
            d.set_item("to_id", br.to_id)?;
            d.set_item("r", br.r)?;
            d.set_item("x", br.x)?;
            d.set_item("b", br.b)?;
            d.set_item("rate_a", br.rate_a)?;
            d.set_item("rate_b", br.rate_b)?;
            d.set_item("rate_c", br.rate_c)?;
            d.set_item("tap", br.tap)?;
            d.set_item("shift", br.shift)?;
            d.set_item("status", br.status)?;
            d.set_item("angmin", br.angmin)?;
            d.set_item("angmax", br.angmax)?;
            list.append(d)?;
        }
        Ok(list)
    }

    #[getter]
    fn gens<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for g in &self.inner.gens {
            let d = PyDict::new(py);
            d.set_item("bus_id", g.bus_id)?;
            d.set_item("pg", g.pg)?;
            d.set_item("qg", g.qg)?;
            d.set_item("qmax", g.qmax)?;
            d.set_item("qmin", g.qmin)?;
            d.set_item("vg", g.vg)?;
            d.set_item("mbase", g.mbase)?;
            d.set_item("status", g.status)?;
            d.set_item("pmax", g.pmax)?;
            d.set_item("pmin", g.pmin)?;
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
        let r = self.inner.connectivity_report();
        let d = PyDict::new(py);
        d.set_item("n_buses", r.n_buses)?;
        d.set_item("n_branches_in_service", r.n_branches_in_service)?;
        d.set_item("n_components", r.n_components)?;
        d.set_item("isolated_buses", r.isolated_buses)?;
        Ok(d)
    }

    // --- matrix builders: each returns a COO tuple ----------------------

    #[pyo3(signature = (scheme=None))]
    fn bprime<'py>(&self, py: Python<'py>, scheme: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
        let opts = BuildOptions {
            scheme: parse_scheme(scheme.unwrap_or("bx"))?,
            ..BuildOptions::default()
        };
        let m = build_bprime(&self.inner, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    /// B'' always keeps tap ratios and zeroes phase shifts (MATPOWER `makeB`);
    /// only the FDPF `scheme` (`"bx"`/`"xb"`) changes its result.
    #[pyo3(signature = (scheme=None))]
    fn bdoubleprime<'py>(
        &self,
        py: Python<'py>,
        scheme: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = BuildOptions {
            scheme: parse_scheme(scheme.unwrap_or("bx"))?,
            ..BuildOptions::default()
        };
        let m = build_bdoubleprime(&self.inner, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    #[pyo3(signature = (include_taps=true, include_shifts=true))]
    fn lacpf<'py>(
        &self,
        py: Python<'py>,
        include_taps: bool,
        include_shifts: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_options(Scheme::Bx, include_taps, include_shifts);
        let m = build_lacpf(&self.inner, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    fn adjacency<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let m = build_adjacency(&self.inner).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    /// `(Re(Y_bus), Im(Y_bus))` as two COO tuples.
    #[pyo3(signature = (include_taps=true, include_shifts=true))]
    fn ybus_parts<'py>(
        &self,
        py: Python<'py>,
        include_taps: bool,
        include_shifts: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_options(Scheme::Bx, include_taps, include_shifts);
        let yb = build_ybus(&self.inner, &opts).map_err(to_pyerr)?;
        let g = coo_triplets(py, &yb.g)?;
        let b = coo_triplets(py, &yb.b)?;
        Ok((g, b).into_pyobject(py)?.into_any())
    }

    #[pyo3(signature = (convention=None))]
    fn ptdf<'py>(&self, py: Python<'py>, convention: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
        let conv = parse_convention(convention.unwrap_or("paper"))?;
        let m = build_ptdf(&self.inner, conv).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    #[pyo3(signature = (convention=None))]
    fn lodf<'py>(&self, py: Python<'py>, convention: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
        let conv = parse_convention(convention.unwrap_or("paper"))?;
        let m = build_lodf(&self.inner, conv).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    /// `(A_coo, b, p_shift, branch_of_col)`: signed incidence as a COO tuple,
    /// then the branch susceptances, phase-shift injection, and column→branch
    /// map as 1-D arrays.
    #[pyo3(signature = (convention=None))]
    fn incidence<'py>(
        &self,
        py: Python<'py>,
        convention: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let conv = parse_convention(convention.unwrap_or("paper"))?;
        let parts = build_incidence(&self.inner, conv).map_err(to_pyerr)?;
        let a = coo_triplets(py, &parts.a)?;
        let b = parts.b.into_pyarray(py);
        let p_shift = parts.p_shift.into_pyarray(py);
        let branch_of_col: Vec<i64> = parts.branch_of_col.iter().map(|&x| x as i64).collect();
        let branch_of_col = branch_of_col.into_pyarray(py);
        Ok((a, b, p_shift, branch_of_col).into_pyobject(py)?.into_any())
    }

    /// Weighted Laplacian `L = A diag(b) Aᵀ` for the chosen DC convention.
    #[pyo3(signature = (convention=None))]
    fn weighted_laplacian<'py>(
        &self,
        py: Python<'py>,
        convention: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let conv = parse_convention(convention.unwrap_or("paper"))?;
        let parts = build_incidence(&self.inner, conv).map_err(to_pyerr)?;
        let l = build_weighted_laplacian(&parts.a, &parts.b);
        coo_triplets(py, &l)
    }

    /// Write the DC-OPF bundle into `out_dir/<case>_dcopf/`. Returns
    /// `{"dir": str, "files": [str, ...]}`.
    #[pyo3(signature = (out_dir, convention=None, units=None))]
    fn write_dcopf_bundle<'py>(
        &self,
        py: Python<'py>,
        out_dir: &str,
        convention: Option<&str>,
        units: Option<&str>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let opts = DcOpfOptions {
            convention: parse_convention(convention.unwrap_or("paper"))?,
            units: parse_units(units.unwrap_or("perunit"))?,
        };
        let outputs = write_bundle(&self.inner, out_dir, &opts).map_err(to_pyerr)?;
        let d = PyDict::new(py);
        d.set_item("dir", outputs.dir.to_string_lossy().into_owned())?;
        let files: Vec<String> = outputs
            .files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        d.set_item("files", files)?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!(
            "PyCase(name={:?}, n_buses={}, n_branches={}, n_gens={})",
            self.inner.name,
            self.inner.n(),
            self.inner.branches.len(),
            self.inner.gens.len()
        )
    }
}

/// Parse a MATPOWER `.m` file from a path.
#[pyfunction]
fn parse_matpower(path: &str) -> PyResult<PyCase> {
    let inner = casemat::parse_matpower_file(path).map_err(to_pyerr)?;
    Ok(PyCase { inner })
}

/// Parse a MATPOWER case from in-memory `.m` text. `name` overrides the case
/// name (defaults to "case").
#[pyfunction]
#[pyo3(signature = (content, name=None))]
fn parse_matpower_string(content: &str, name: Option<&str>) -> PyResult<PyCase> {
    let mut inner = casemat::parse_matpower(content).map_err(to_pyerr)?;
    if let Some(n) = name {
        inner.name = n.to_string();
    }
    Ok(PyCase { inner })
}

/// Convert a case file to another format through the neutral hub. Returns
/// `(text, warnings)`: the converted file text and the list of fidelity warnings
/// (fields the target couldn't represent). The input format is the file
/// extension unless `from` overrides it.
#[pyfunction]
#[pyo3(signature = (path, to, from=None))]
fn convert(path: &str, to: &str, from: Option<&str>) -> PyResult<(String, Vec<String>)> {
    let target = fmt_from_str(to)
        .ok_or_else(|| PyValueError::new_err(format!("unknown target format: {to}")))?;
    let net = read_network_py(path, from)?;
    let conv = casemat::write_as(&net, target);
    Ok((conv.text, conv.warnings))
}

/// Map a format name (with common aliases) to a [`casemat::TargetFormat`].
fn fmt_from_str(s: &str) -> Option<casemat::TargetFormat> {
    use casemat::TargetFormat as T;
    Some(match s.to_ascii_lowercase().as_str() {
        "matpower" | "m" => T::Matpower,
        "powermodels-json" | "powermodels" | "pm" => T::PowerModelsJson,
        "egret-json" | "egret" => T::EgretJson,
        "psse" | "raw" => T::Psse,
        "powerworld" | "aux" => T::PowerWorld,
        _ => return None,
    })
}

/// Read `path` into the neutral network, choosing the reader from `from` or the
/// file extension.
fn read_network_py(path: &str, from: Option<&str>) -> PyResult<casemat::Network> {
    let p = std::path::Path::new(path);
    let fmt = match from {
        Some(f) => fmt_from_str(f)
            .ok_or_else(|| PyValueError::new_err(format!("unknown source format: {f}")))?,
        None => match p.extension().and_then(|e| e.to_str()) {
            Some("m") => casemat::TargetFormat::Matpower,
            Some("json") => casemat::TargetFormat::PowerModelsJson,
            Some("raw") => casemat::TargetFormat::Psse,
            Some("aux") => casemat::TargetFormat::PowerWorld,
            other => {
                return Err(PyValueError::new_err(format!(
                    "cannot infer input format from extension {other:?}; pass from="
                )))
            }
        },
    };
    let read_str = || std::fs::read_to_string(p).map_err(|e| PyValueError::new_err(e.to_string()));
    let net = match fmt {
        casemat::TargetFormat::Matpower => {
            casemat::parse_matpower_file(path).map_err(to_pyerr)?.to_network()
        }
        casemat::TargetFormat::PowerModelsJson => {
            casemat::parse_powermodels_json(&read_str()?).map_err(to_pyerr)?
        }
        casemat::TargetFormat::Psse => casemat::parse_psse(&read_str()?).map_err(to_pyerr)?,
        casemat::TargetFormat::PowerWorld => {
            casemat::parse_powerworld(&read_str()?).map_err(to_pyerr)?
        }
        casemat::TargetFormat::EgretJson => {
            return Err(PyValueError::new_err(
                "reading EGRET JSON is not supported yet (write-only)",
            ))
        }
    };
    Ok(net)
}

#[pymodule]
fn _casemat(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("CasematError", m.py().get_type::<CasematError>())?;
    m.add_class::<PyCase>()?;
    m.add_function(wrap_pyfunction!(parse_matpower, m)?)?;
    m.add_function(wrap_pyfunction!(parse_matpower_string, m)?)?;
    m.add_function(wrap_pyfunction!(convert, m)?)?;
    Ok(())
}
