//! PyO3 extension behind the `powerio` Python package.
//!
//! One Rust↔Python boundary for both halves of PowerIO: the dependency-light IO
//! surface (parse, lossless write, cross-format convert) and the matrix surface
//! (B'/B''/Y_bus, PTDF/LODF, incidence, weighted Laplacian, adjacency, DC OPF).
//! Parse and convert cross the boundary as plain dicts and strings, so
//! `import powerio` pulls in nothing but the interpreter.
//!
//! The matrix methods hand back COO triplets as plain Python lists
//! (`data`, `row`, `col`, `shape`); there is no numpy at this layer. The
//! pure-Python `powerio` package (python/powerio/) assembles those into
//! `scipy.sparse` matrices and networkx graphs lazily, so scipy/numpy/networkx
//! stay out of the Rust build and a missing extra surfaces as an
//! `ImportError` in Python rather than a link error.
//!
//! Indices narrow to `i32` to
//! match scipy's default index width; the largest index is bounded by
//! `max(n_buses, n_branches)` (`2n` for the LACPF block), far under 2³¹, and
//! `coo_triplets` guards the bound anyway.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use sprs::CsMat;

use powerio_matrix::matrix::{
    BuildOptions, DcConvention, Scheme, Units, build_adjacency, build_bdoubleprime, build_bprime,
    build_incidence, build_lacpf, build_lodf, build_ptdf, build_weighted_laplacian, build_ybus,
};
use powerio_matrix::opf_pipeline::{DcOpfOptions, write_dcopf_bundle as write_bundle};
use powerio_matrix::{IndexCore, IndexedNetwork, Network};

#[cfg(feature = "gridfm")]
use powerio_matrix::io::gridfm::{
    GridfmOptions, GridfmOutputs, GridfmRead, numbered_snapshots,
    read_gridfm_dataset as gridfm_read_dataset, read_gridfm_scenarios as gridfm_read_scenarios,
    write_gridfm_batch as gridfm_write_batch, write_gridfm_dataset as gridfm_write_dataset,
};

pyo3::create_exception!(
    _powerio,
    PowerIOError,
    pyo3::exceptions::PyException,
    "Base error raised by the powerio parser, converter, or matrix builders."
);

pyo3::create_exception!(
    _powerio,
    PowerIOParseError,
    PowerIOError,
    "A case file is malformed or unparseable (missing/short rows, bad numbers, \
     unbalanced brackets, format read failures)."
);

pyo3::create_exception!(
    _powerio,
    PowerIODataError,
    PowerIOError,
    "A well-formed case cannot satisfy a requested operation (no generators, \
     wrong reference bus count, an unknown bus reference, zero/non-finite \
     branch impedance, a disconnected or singular network, a scenario batch \
     shape mismatch, or a dimension/cost mismatch)."
);

/// Map a [`powerio_matrix::Error`] onto the right Python exception, driven by
/// [`Error::category`]. I/O failures become the matching `OSError` subclass
/// (`FileNotFoundError`, `PermissionError`, …); an unknown/uninferable format
/// becomes a `ValueError`; malformed input becomes [`PowerIOParseError`] and an
/// unmet operation precondition becomes [`PowerIODataError`]. Both subclass
/// [`PowerIOError`], so existing `except PowerIOError` handlers keep working;
/// output-side write failures fall back to the [`PowerIOError`] base.
fn to_pyerr(e: powerio_matrix::Error) -> PyErr {
    use powerio_matrix::{Error as E, ErrorCategory as C};
    // `Io` carries the underlying `std::io::Error`; hand it to PyO3 by value so
    // it picks the precise `OSError` subclass. (Returning here also keeps the
    // `to_string()` below off the I/O path.)
    if let E::Io(io) = e {
        return io.into();
    }
    let msg = e.to_string();
    match e.category() {
        C::UnknownFormat => PyValueError::new_err(msg),
        C::Parse => PowerIOParseError::new_err(msg),
        C::Data => PowerIODataError::new_err(msg),
        // `Io` is handled above; `Output` (mtx/parquet) maps to the base.
        C::Io | C::Output => PowerIOError::new_err(msg),
    }
}

/// Convert an output path to a `String`, raising rather than returning a lossily
/// mangled path that no longer opens the file that was written.
fn path_to_str(p: &std::path::Path) -> PyResult<String> {
    p.to_str().map(str::to_owned).ok_or_else(|| {
        PowerIOError::new_err(format!(
            "output path is not valid UTF-8 and cannot be returned as a string: {}",
            p.display()
        ))
    })
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

/// Accepts `tree`, `lattice`/`lattice2d`, and `pegase`/`pegase-like` (case- and
/// separator-insensitive).
fn parse_topology(s: &str) -> PyResult<powerio_matrix::synth::Topology> {
    use powerio_matrix::synth::Topology;
    match normalize(s).as_str() {
        "tree" => Ok(Topology::Tree),
        "lattice" | "lattice2d" => Ok(Topology::Lattice2D),
        "pegase" | "pegaselike" => Ok(Topology::PegaseLike),
        other => Err(PyValueError::new_err(format!(
            "unknown topology {other:?}; expected 'tree', 'lattice', or 'pegase-like'"
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
/// plain Python lists. A CSR input is walked borrowed; any other storage is
/// converted to CSR once so `outer_iterator()` yields rows. Indices narrow to
/// `i32`. The narrowing is guarded: a dimension past `i32::MAX` raises rather
/// than wrapping to negative indices.
fn coo_triplets<'py>(py: Python<'py>, m: &CsMat<f64>) -> PyResult<Bound<'py, PyAny>> {
    if m.rows() > i32::MAX as usize || m.cols() > i32::MAX as usize {
        return Err(PyValueError::new_err(format!(
            "matrix is {}x{}; an index exceeds i32 range; rebuild with i64 indices",
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
    Ok((data, rows, cols, shape).into_pyobject(py)?.into_any())
}

fn build_options(scheme: Scheme, include_taps: bool, include_shifts: bool) -> BuildOptions {
    BuildOptions {
        scheme,
        include_taps,
        include_shifts,
        ..BuildOptions::default()
    }
}

/// Low-level handle around a parsed [`Network`]. The user-facing `powerio.Network`
/// (pure Python) wraps this: the IO getters and topology methods delegate
/// straight to it, and the matrix methods turn its COO tuples into scipy.
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
    // --- metadata -------------------------------------------------------

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
    fn n_buses(&self) -> usize {
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
    /// one reference bus is present; for the multi-reference case use
    /// :meth:`reference_bus_indices`.
    fn reference_bus_index(&self) -> PyResult<usize> {
        IndexedNetwork::with_core(&self.inner, &self.core)
            .reference_bus_index()
            .map_err(to_pyerr)
    }

    /// Dense `[0, n)` indices of every reference (slack) bus, ascending. May be
    /// empty (no reference) or hold several (a slack per island, or a normalized
    /// case that kept the file's multiple references).
    fn reference_bus_indices(&self) -> Vec<usize> {
        IndexedNetwork::with_core(&self.inner, &self.core).reference_bus_indices()
    }

    // --- tables (the format-neutral Network, as dict rows) --------------

    #[getter]
    fn buses<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.buses.len());
        for b in &self.inner.buses {
            let d = PyDict::new(py);
            d.set_item("id", b.id.0)?;
            d.set_item("kind", b.kind.as_str())?;
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
            d.set_item("bus", l.bus.0)?;
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
            d.set_item("bus", s.bus.0)?;
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
            d.set_item("from_id", br.from.0)?;
            d.set_item("to_id", br.to.0)?;
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
    fn generators<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.generators.len());
        for g in &self.inner.generators {
            let d = PyDict::new(py);
            d.set_item("bus", g.bus.0)?;
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

    /// Serialize this case to MATPOWER `.m` text. For a MATPOWER-parsed case this
    /// is the byte-exact source echo.
    fn to_matpower(&self) -> String {
        self.inner.to_matpower()
    }

    /// Serialize this case to the JSON transport.
    fn to_json(&self) -> PyResult<String> {
        self.inner.to_json().map_err(to_pyerr)
    }

    /// Serialize this case to another format. Returns `(text, warnings)`.
    fn to_format(&self, to: &str) -> PyResult<(String, Vec<String>)> {
        let target = to
            .parse::<powerio_matrix::TargetFormat>()
            .map_err(to_pyerr)?;
        let conv = self.inner.to_format(target);
        Ok((conv.text, conv.warnings))
    }

    /// A normalized, computation-ready copy of this case: per unit, radians,
    /// out-of-service filtered, densely reindexed (1-based), bus types
    /// canonicalized. The raw case is unchanged; the result carries no retained
    /// source, so writing it serializes the per-unit model rather than echoing.
    fn to_normalized(&self) -> PyResult<PyCase> {
        let inner = self.inner.to_normalized().map_err(to_pyerr)?;
        let core = IndexCore::build(&inner);
        Ok(PyCase { inner, core })
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
    /// map as plain lists (the wrapper turns them into 1-D numpy arrays).
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
        let b = parts.b;
        let p_shift = parts.p_shift;
        let branch_of_col: Vec<i64> = parts.branch_of_col.iter().map(|&x| x as i64).collect();
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

    /// Write the DC OPF bundle into `out_dir/<case>_dcopf/`. Returns
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
        dir_files_dict(py, &outputs.dir, &outputs.files)
    }

    /// Write the gridfm-datakit Parquet dataset for this case under
    /// `out_dir/<case>/raw/`. Returns
    /// `{"dir", "files", "dropped_zero_impedance", "degenerate_cost_gens"}`.
    /// Available when the extension is built with the Rust `gridfm` feature.
    #[cfg(feature = "gridfm")]
    #[pyo3(signature = (out_dir, scenario=0, include_y_bus=true, include_taps=true, include_shifts=true))]
    fn write_gridfm<'py>(
        &self,
        py: Python<'py>,
        out_dir: &str,
        scenario: i64,
        include_y_bus: bool,
        include_taps: bool,
        include_shifts: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let opts = GridfmOptions {
            include_y_bus,
            include_taps,
            include_shifts,
        };
        let outputs =
            gridfm_write_dataset(&self.inner, scenario, out_dir, &opts).map_err(to_pyerr)?;
        gridfm_outputs_to_dict(py, &outputs)
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

/// Parse a case file from a path, inferring the format from the extension unless
/// `from_` is given.
#[pyfunction]
#[pyo3(signature = (path, from_=None))]
fn parse_file(path: &str, from_: Option<&str>) -> PyResult<PyCase> {
    let inner = powerio_matrix::parse_file(std::path::Path::new(path), from_).map_err(to_pyerr)?;
    let core = IndexCore::build(&inner);
    Ok(PyCase { inner, core })
}

/// Parse a case from in-memory text in the named `format` (`matpower`,
/// `powermodels-json`, `egret-json`, `psse`, `powerworld`; aliases
/// `m`/`pm`/`egret`/`raw`/`aux`).
#[pyfunction]
#[pyo3(signature = (text, format=None))]
fn parse_str(text: &str, format: Option<&str>) -> PyResult<PyCase> {
    let inner = powerio_matrix::parse_str(text, format.unwrap_or("matpower")).map_err(to_pyerr)?;
    let core = IndexCore::build(&inner);
    Ok(PyCase { inner, core })
}

/// Rebuild a case from JSON produced by `Network.to_json()`.
#[pyfunction]
fn from_json(text: &str) -> PyResult<PyCase> {
    let inner = powerio_matrix::Network::from_json(text).map_err(to_pyerr)?;
    let core = IndexCore::build(&inner);
    Ok(PyCase { inner, core })
}

/// Convert a case file to another format through the neutral hub. Returns
/// `(text, warnings)`: the converted file text and the list of fidelity warnings
/// (fields the target couldn't represent). The input format is the file
/// extension unless `from` overrides it.
#[pyfunction]
#[pyo3(signature = (path, to, from_=None))]
fn convert_file(path: &str, to: &str, from_: Option<&str>) -> PyResult<(String, Vec<String>)> {
    let target = to
        .parse::<powerio_matrix::TargetFormat>()
        .map_err(to_pyerr)?;
    let conv = powerio_matrix::convert_file(std::path::Path::new(path), target, from_)
        .map_err(to_pyerr)?;
    Ok((conv.text, conv.warnings))
}

/// Convert in-memory case `text` to another format through the neutral hub,
/// with no file staging. Returns `(text, warnings)` like `convert_file`.
/// `format` names the input format (default `matpower`).
#[pyfunction]
#[pyo3(signature = (text, to, format=None))]
fn convert_str(text: &str, to: &str, format: Option<&str>) -> PyResult<(String, Vec<String>)> {
    let target = to
        .parse::<powerio_matrix::TargetFormat>()
        .map_err(to_pyerr)?;
    let conv = powerio_matrix::convert_str(text, target, format.unwrap_or("matpower"))
        .map_err(to_pyerr)?;
    Ok((conv.text, conv.warnings))
}

/// Generate a synthetic case: a spanning `tree`, a 2-D `lattice` (`n` rounds up
/// to a perfect square), or a `pegase-like` mesh (tree plus ~30% extra edges).
/// `n` below 2 is raised to 2 (lattice: at least a 2×2 grid). Identical
/// arguments (including `seed`) generate the identical case; bus 1 is the
/// reference. Buses and branches only — no loads, shunts, or generators.
#[pyfunction]
#[pyo3(signature = (topology=None, n=64, r_over_x=0.1, mean_x=0.05, seed=0x00C0_FFEE))]
fn generate_case(
    topology: Option<&str>,
    n: usize,
    r_over_x: f64,
    mean_x: f64,
    seed: u64,
) -> PyResult<PyCase> {
    let spec = powerio_matrix::synth::SynthSpec {
        topology: parse_topology(topology.unwrap_or("tree"))?,
        n,
        r_over_x,
        mean_x,
        seed,
    };
    let inner = powerio_matrix::synth::generate(&spec);
    let core = IndexCore::build(&inner);
    Ok(PyCase { inner, core })
}

/// Build a `{dir, files}` dict from an outputs directory and its written files.
/// Shared by the DC OPF and gridfm write paths. Paths go through [`path_to_str`]
/// (so a non-UTF8 path raises instead of being mangled).
fn dir_files_dict<'py>(
    py: Python<'py>,
    dir: &std::path::Path,
    files: &[std::path::PathBuf],
) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("dir", path_to_str(dir)?)?;
    let files: Vec<String> = files
        .iter()
        .map(|p| path_to_str(p))
        .collect::<PyResult<_>>()?;
    d.set_item("files", files)?;
    Ok(d)
}

/// Build the `{dir, files, dropped_zero_impedance, degenerate_cost_gens}` dict a
/// gridfm write returns.
#[cfg(feature = "gridfm")]
fn gridfm_outputs_to_dict<'py>(
    py: Python<'py>,
    outputs: &GridfmOutputs,
) -> PyResult<Bound<'py, PyDict>> {
    let d = dir_files_dict(py, &outputs.dir, &outputs.files)?;
    d.set_item("dropped_zero_impedance", outputs.dropped_zero_impedance)?;
    d.set_item("degenerate_cost_gens", outputs.degenerate_cost_gens)?;
    Ok(d)
}

/// Write a batch of cases as one gridfm-datakit dataset, row-stacked and keyed by
/// the `scenario` column. The k-th case is stamped `base_scenario + k`; all cases
/// must share one base element set (same bus/branch/gen counts and bus-id order).
/// Available when the extension is built with the Rust `gridfm` feature.
#[cfg(feature = "gridfm")]
#[pyfunction]
#[pyo3(signature = (cases, out_dir, base_scenario=0, include_y_bus=true, include_taps=true, include_shifts=true))]
fn write_gridfm_batch<'py>(
    py: Python<'py>,
    cases: Vec<PyRef<'py, PyCase>>,
    out_dir: &str,
    base_scenario: i64,
    include_y_bus: bool,
    include_taps: bool,
    include_shifts: bool,
) -> PyResult<Bound<'py, PyDict>> {
    let opts = GridfmOptions {
        include_y_bus,
        include_taps,
        include_shifts,
    };
    // The shared numbering builder stamps the k-th case `base_scenario + k`, the
    // same rule (and checked arithmetic) the CLI uses.
    let net_refs: Vec<_> = cases.iter().map(|c| &c.inner).collect();
    let snapshots = numbered_snapshots(&net_refs, base_scenario).map_err(to_pyerr)?;
    let outputs = gridfm_write_batch(&snapshots, out_dir, &opts).map_err(to_pyerr)?;
    gridfm_outputs_to_dict(py, &outputs)
}

/// Turn a [`GridfmRead`] into the `(case, scenario, warnings)` triple the Python
/// `read_gridfm*` functions return: the reconstructed network wrapped as a
/// `PyCase` (with its index core, exactly as `parse_file` does), the scenario id,
/// and the fidelity warnings the lossy read surfaced.
#[cfg(feature = "gridfm")]
fn gridfm_read_to_py(read: GridfmRead) -> (PyCase, i64, Vec<String>) {
    let core = IndexCore::build(&read.network);
    (
        PyCase {
            inner: read.network,
            core,
        },
        read.scenario,
        read.warnings,
    )
}

/// Read one scenario of a gridfm-datakit Parquet dataset back into a case,
/// returning `(case, scenario, warnings)` (the pure-Python layer wraps it as a
/// `GridfmRead` namedtuple). `dir` resolves leniently: the `raw/` leaf, a
/// `<case>/` directory, or a parent with one `*/raw/` child. The read is lossy but
/// power-flow-complete; `warnings` lists what the gridfm schema couldn't
/// round-trip. Available when the extension is built with the Rust `gridfm` feature.
#[cfg(feature = "gridfm")]
#[pyfunction]
#[pyo3(signature = (dir, scenario=0))]
fn read_gridfm(dir: &str, scenario: i64) -> PyResult<(PyCase, i64, Vec<String>)> {
    gridfm_read_dataset(dir, scenario)
        .map(gridfm_read_to_py)
        .map_err(to_pyerr)
}

/// Read every scenario of a gridfm dataset, one `(case, scenario, warnings)`
/// triple per scenario id (ascending) over the shared topology — the read side of
/// the scenario batch. Available when the extension is built with the Rust
/// `gridfm` feature.
#[cfg(feature = "gridfm")]
#[pyfunction]
fn read_gridfm_scenarios(dir: &str) -> PyResult<Vec<(PyCase, i64, Vec<String>)>> {
    let reads = gridfm_read_scenarios(dir).map_err(to_pyerr)?;
    Ok(reads.into_iter().map(gridfm_read_to_py).collect())
}

#[pymodule]
fn _powerio(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("PowerIOError", m.py().get_type::<PowerIOError>())?;
    m.add("PowerIOParseError", m.py().get_type::<PowerIOParseError>())?;
    m.add("PowerIODataError", m.py().get_type::<PowerIODataError>())?;
    m.add_class::<PyCase>()?;
    m.add_function(wrap_pyfunction!(parse_file, m)?)?;
    m.add_function(wrap_pyfunction!(parse_str, m)?)?;
    m.add_function(wrap_pyfunction!(from_json, m)?)?;
    m.add_function(wrap_pyfunction!(convert_file, m)?)?;
    m.add_function(wrap_pyfunction!(convert_str, m)?)?;
    m.add_function(wrap_pyfunction!(generate_case, m)?)?;
    // Whether the gridfm Parquet surface (arrow/parquet) was compiled in, so the
    // pure-Python layer can raise an ImportError instead of an AttributeError.
    m.add("_has_gridfm", cfg!(feature = "gridfm"))?;
    #[cfg(feature = "gridfm")]
    m.add_function(wrap_pyfunction!(write_gridfm_batch, m)?)?;
    #[cfg(feature = "gridfm")]
    m.add_function(wrap_pyfunction!(read_gridfm, m)?)?;
    #[cfg(feature = "gridfm")]
    m.add_function(wrap_pyfunction!(read_gridfm_scenarios, m)?)?;
    Ok(())
}
