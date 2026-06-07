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

use casemat::matrix::{
    build_adjacency, build_bdoubleprime, build_bprime, build_incidence, build_lacpf, build_lodf,
    build_ptdf, build_weighted_laplacian, build_ybus, BuildOptions, DcConvention, Scheme, Units,
};
use casemat::opf_pipeline::{write_dcopf_bundle as write_bundle, DcOpfOptions};
use casemat::{IndexCore, IndexedNetwork, Network};

pyo3::create_exception!(
    _casemat,
    CasematError,
    pyo3::exceptions::PyException,
    "Error raised by the casemat parser or matrix builders."
);

/// I/O failures map to the matching `OSError` subclass (`FileNotFoundError`,
/// `PermissionError`, …) so Python callers can catch them the usual way; an
/// unknown/uninferable format becomes a `ValueError`; other parse and data
/// errors become [`CasematError`].
fn to_pyerr(e: casemat::Error) -> PyErr {
    match e {
        casemat::Error::Io(io) => io.into(),
        casemat::Error::UnknownFormat(msg) => PyValueError::new_err(msg),
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

/// Materialize a sparse matrix as a `(data, row, col, (nrows, ncols))` tuple of
/// NumPy arrays. A CSR input is walked borrowed; any other storage is converted
/// to CSR once so `outer_iterator()` yields rows. Indices narrow to `i32`. The
/// narrowing is guarded: a dimension past `i32::MAX` raises rather than wrapping
/// to negative indices.
fn coo_triplets<'py>(py: Python<'py>, m: &CsMat<f64>) -> PyResult<Bound<'py, PyAny>> {
    if m.rows() > i32::MAX as usize || m.cols() > i32::MAX as usize {
        return Err(PyValueError::new_err(format!(
            "matrix is {}x{}; an index exceeds i32 range — rebuild with i64 indices",
            m.rows(),
            m.cols()
        )));
    }
    // Walk a CSR view borrowed; only deep-copy when the storage isn't already CSR.
    let csr;
    let view = if m.is_csr() {
        m.view()
    } else {
        csr = m.to_csr();
        csr.view()
    };
    let nnz = view.nnz();
    let mut data: Vec<f64> = Vec::with_capacity(nnz);
    let mut rows: Vec<i32> = Vec::with_capacity(nnz);
    let mut cols: Vec<i32> = Vec::with_capacity(nnz);
    for (r, row) in view.outer_iterator().enumerate() {
        for (c, &v) in row.iter() {
            data.push(v);
            rows.push(r as i32);
            cols.push(c as i32);
        }
    }
    let shape = (view.rows(), view.cols());
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

/// Low-level handle around a parsed `Network`. The user-facing `casemat.Case`
/// (pure Python) wraps this and turns the COO tuples into scipy matrices.
///
/// The derived [`IndexCore`] is built once and cached alongside `inner`, so the
/// matrix builders and topology getters reuse it instead of rebuilding the
/// bus-id map per call.
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
        let g = IndexedNetwork::with_core(&self.inner, &self.core);
        let (pd, qd, gs, bs) = (g.pd(), g.qd(), g.gs(), g.bs());
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.buses.len());
        for (i, b) in self.inner.buses.iter().enumerate() {
            let d = PyDict::new(py);
            d.set_item("id", b.id)?;
            d.set_item("type", b.kind.as_str())?;
            d.set_item("pd", pd[i])?;
            d.set_item("qd", qd[i])?;
            d.set_item("gs", gs[i])?;
            d.set_item("bs", bs[i])?;
            d.set_item("area", b.area)?;
            d.set_item("vm", b.vm)?;
            d.set_item("va", b.va)?;
            d.set_item("base_kv", b.base_kv)?;
            d.set_item("zone", b.zone)?;
            d.set_item("vmax", b.vmax)?;
            d.set_item("vmin", b.vmin)?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn branches<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.branches.len());
        for br in &self.inner.branches {
            let d = PyDict::new(py);
            d.set_item("from_id", br.from)?;
            d.set_item("to_id", br.to)?;
            d.set_item("r", br.r)?;
            d.set_item("x", br.x)?;
            d.set_item("b", br.b)?;
            d.set_item("rate_a", br.rate_a)?;
            d.set_item("rate_b", br.rate_b)?;
            d.set_item("rate_c", br.rate_c)?;
            d.set_item("tap", br.tap)?;
            d.set_item("shift", br.shift)?;
            d.set_item("status", f64::from(br.in_service))?;
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
            d.set_item("bus_id", g.bus)?;
            d.set_item("pg", g.pg)?;
            d.set_item("qg", g.qg)?;
            d.set_item("qmax", g.qmax)?;
            d.set_item("qmin", g.qmin)?;
            d.set_item("vg", g.vg)?;
            d.set_item("mbase", g.mbase)?;
            d.set_item("status", f64::from(g.in_service))?;
            d.set_item("pmax", g.pmax)?;
            d.set_item("pmin", g.pmin)?;
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

    // --- matrix builders: each returns a COO tuple ----------------------

    #[pyo3(signature = (scheme=None))]
    fn bprime<'py>(&self, py: Python<'py>, scheme: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
        let opts = BuildOptions {
            scheme: parse_scheme(scheme.unwrap_or("bx"))?,
            ..BuildOptions::default()
        };
        let view = IndexedNetwork::with_core(&self.inner, &self.core);
        let m = build_bprime(&view, &opts).map_err(to_pyerr)?;
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
        let view = IndexedNetwork::with_core(&self.inner, &self.core);
        let m = build_bdoubleprime(&view, &opts).map_err(to_pyerr)?;
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
        let view = IndexedNetwork::with_core(&self.inner, &self.core);
        let m = build_lacpf(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    fn adjacency<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let view = IndexedNetwork::with_core(&self.inner, &self.core);
        let m = build_adjacency(&view).map_err(to_pyerr)?;
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
        let view = IndexedNetwork::with_core(&self.inner, &self.core);
        let yb = build_ybus(&view, &opts).map_err(to_pyerr)?;
        let g = coo_triplets(py, &yb.g)?;
        let b = coo_triplets(py, &yb.b)?;
        Ok((g, b).into_pyobject(py)?.into_any())
    }

    #[pyo3(signature = (convention=None))]
    fn ptdf<'py>(&self, py: Python<'py>, convention: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
        let conv = parse_convention(convention.unwrap_or("paper"))?;
        let view = IndexedNetwork::with_core(&self.inner, &self.core);
        let m = build_ptdf(&view, conv).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    #[pyo3(signature = (convention=None))]
    fn lodf<'py>(&self, py: Python<'py>, convention: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
        let conv = parse_convention(convention.unwrap_or("paper"))?;
        let view = IndexedNetwork::with_core(&self.inner, &self.core);
        let m = build_lodf(&view, conv).map_err(to_pyerr)?;
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
        let view = IndexedNetwork::with_core(&self.inner, &self.core);
        let parts = build_incidence(&view, conv).map_err(to_pyerr)?;
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
        let view = IndexedNetwork::with_core(&self.inner, &self.core);
        let parts = build_incidence(&view, conv).map_err(to_pyerr)?;
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
            self.inner.buses.len(),
            self.inner.branches.len(),
            self.inner.generators.len()
        )
    }
}

/// Parse a MATPOWER `.m` file from a path.
#[pyfunction]
fn parse_matpower(path: &str) -> PyResult<PyCase> {
    let inner = casemat::parse_matpower_file(path).map_err(to_pyerr)?;
    let core = IndexCore::build(&inner);
    Ok(PyCase { inner, core })
}

/// Parse a MATPOWER case from in-memory `.m` text. When `name` is given it
/// overrides the parsed case name.
#[pyfunction]
#[pyo3(signature = (content, name=None))]
fn parse_matpower_string(content: &str, name: Option<&str>) -> PyResult<PyCase> {
    let mut inner = casemat::parse_matpower(content).map_err(to_pyerr)?;
    if let Some(n) = name {
        inner.name = n.to_string();
    }
    let core = IndexCore::build(&inner);
    Ok(PyCase { inner, core })
}

/// Convert a case file to another format through the neutral hub. Returns
/// `(text, warnings)`: the converted file text and the list of fidelity warnings
/// (fields the target couldn't represent). The input format is the file
/// extension unless `from` overrides it.
#[pyfunction]
#[pyo3(signature = (path, to, from=None))]
fn convert(path: &str, to: &str, from: Option<&str>) -> PyResult<(String, Vec<String>)> {
    let target = casemat::target_format_from_name(to)
        .ok_or_else(|| PyValueError::new_err(format!("unknown target format: {to}")))?;
    let net = casemat::read_path(std::path::Path::new(path), from).map_err(to_pyerr)?;
    let conv = casemat::write_as(&net, target);
    Ok((conv.text, conv.warnings))
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
