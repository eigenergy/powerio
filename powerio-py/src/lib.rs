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

use std::collections::BTreeMap;
use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use sprs::CsMat;

use powerio_matrix::matrix::{
    BuildOptions, DcConvention, Scheme, Units, build_adjacency, build_bdoubleprime, build_bprime,
    build_incidence, build_lacpf, build_lodf, build_ptdf, build_weighted_laplacian, build_ybus,
};
use powerio_matrix::opf_pipeline::{DcOpfOptions, write_dcopf_bundle as write_bundle};
use powerio_matrix::{
    DisplayData, IndexCore, IndexedNetwork, MissingGenCostPolicy, Network, PwdDisplay, WriteOptions,
};
use powerio_pkg::{
    CompilerPackage, DiagnosticSeverity, DiagnosticStage, Origin, SourceDescriptor,
    StructuredDiagnostic, ValidationSummary,
};

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

fn parse_missing_gen_cost(
    s: Option<&str>,
    default_gen_cost: Option<&str>,
    default_policy: MissingGenCostPolicy,
) -> PyResult<MissingGenCostPolicy> {
    let Some(s) = s else {
        if default_gen_cost.is_some() {
            return Err(PyValueError::new_err(
                "default_gen_cost is only valid with missing_gen_cost='quadratic'",
            ));
        }
        return Ok(default_policy);
    };
    match normalize(s).as_str() {
        "preserve" => {
            if default_gen_cost.is_some() {
                return Err(PyValueError::new_err(
                    "default_gen_cost is only valid with missing_gen_cost='quadratic'",
                ));
            }
            Ok(MissingGenCostPolicy::Preserve)
        }
        "require" => {
            if default_gen_cost.is_some() {
                return Err(PyValueError::new_err(
                    "default_gen_cost is only valid with missing_gen_cost='quadratic'",
                ));
            }
            Ok(MissingGenCostPolicy::Require)
        }
        "zero" => {
            if default_gen_cost.is_some() {
                return Err(PyValueError::new_err(
                    "default_gen_cost is only valid with missing_gen_cost='quadratic'",
                ));
            }
            Ok(MissingGenCostPolicy::zero())
        }
        "quadratic" => {
            let value = default_gen_cost.ok_or_else(|| {
                PyValueError::new_err(
                    "missing_gen_cost='quadratic' requires default_gen_cost='c2,c1,c0'",
                )
            })?;
            let [c2, c1, c0] = parse_cost_triple(value)?;
            Ok(MissingGenCostPolicy::quadratic(c2, c1, c0))
        }
        other => Err(PyValueError::new_err(format!(
            "unknown missing_gen_cost {other:?}; expected 'preserve', 'require', 'zero', or 'quadratic'"
        ))),
    }
}

fn parse_cost_triple(value: &str) -> PyResult<[f64; 3]> {
    let parts: Vec<_> = value.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return Err(PyValueError::new_err(
            "default_gen_cost expects exactly three comma-separated values: c2,c1,c0",
        ));
    }
    let mut out = [0.0; 3];
    for (slot, part) in out.iter_mut().zip(parts) {
        *slot = part.parse::<f64>().map_err(|_| {
            PyValueError::new_err(format!("could not parse default_gen_cost value {part:?}"))
        })?;
        if !slot.is_finite() {
            return Err(PyValueError::new_err(
                "default_gen_cost values must be finite",
            ));
        }
    }
    Ok(out)
}

fn write_options(
    missing_gen_cost: Option<&str>,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&str>,
    default_policy: MissingGenCostPolicy,
) -> PyResult<WriteOptions> {
    let missing_gen_cost =
        parse_missing_gen_cost(missing_gen_cost, default_gen_cost, default_policy)?;
    let gen_cost_patches = match gen_cost_csv {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                PyValueError::new_err(format!("reading gen_cost_csv {path:?}: {e}"))
            })?;
            powerio_matrix::parse_gen_cost_csv(&text).map_err(to_pyerr)?
        }
        None => Vec::new(),
    };
    Ok(WriteOptions {
        missing_gen_cost,
        gen_cost_patches,
    })
}

fn package_pyerr(e: serde_json::Error) -> PyErr {
    PyValueError::new_err(format!("invalid .pio.json package: {e}"))
}

fn package_to_json(pkg: &CompilerPackage) -> PyResult<String> {
    let text = pkg.to_json_pretty().map_err(package_pyerr)?;
    CompilerPackage::from_json(&text).map_err(package_pyerr)?;
    Ok(text)
}

fn package_warning_messages(pkg: &CompilerPackage) -> Vec<String> {
    pkg.diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.severity,
                DiagnosticSeverity::Warning | DiagnosticSeverity::Error | DiagnosticSeverity::Fatal
            )
        })
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

fn package_to_balanced_py(pkg: CompilerPackage) -> PyResult<PyNetwork> {
    let warnings = package_warning_messages(&pkg);
    let inner = pkg
        .as_balanced()
        .ok_or_else(|| PyValueError::new_err("package model_kind is not balanced"))?
        .clone();
    let core = IndexCore::build(&inner);
    Ok(PyNetwork {
        inner,
        core,
        warnings,
    })
}

fn package_to_dist_py(pkg: CompilerPackage) -> PyResult<PyDistNetwork> {
    let net = pkg
        .as_multiconductor()
        .ok_or_else(|| PyValueError::new_err("package model_kind is not multiconductor"))?
        .clone();
    Ok(PyDistNetwork { net })
}

fn add_package_read_warning_diagnostics(
    pkg: &mut CompilerPackage,
    code: &str,
    warnings: &[String],
) {
    pkg.diagnostics.extend(warnings.iter().map(|w| {
        StructuredDiagnostic::new(
            code,
            DiagnosticSeverity::Warning,
            DiagnosticStage::Read,
            w.clone(),
        )
    }));
    pkg.validation = ValidationSummary::from_diagnostics(&pkg.diagnostics);
}

#[derive(Clone, Copy)]
enum PackageSourceKind {
    File,
    BinaryFile,
    Folder,
}

impl PackageSourceKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::BinaryFile => "binary_file",
            Self::Folder => "folder",
        }
    }
}

fn package_source_kind(input: &Path, format: &str) -> PackageSourceKind {
    if input.is_dir() {
        PackageSourceKind::Folder
    } else if format == "powerworld-pwb" {
        PackageSourceKind::BinaryFile
    } else {
        PackageSourceKind::File
    }
}

fn set_package_source(
    pkg: &mut CompilerPackage,
    input: &Path,
    kind: PackageSourceKind,
    format: &str,
    retained_source: bool,
) {
    let path = input.display().to_string();
    pkg.origin = match kind {
        PackageSourceKind::File => Origin::File {
            path: path.clone(),
            format: format.to_owned(),
            hash: None,
            retained_source,
        },
        PackageSourceKind::BinaryFile => Origin::BinaryFile {
            path: path.clone(),
            format: format.to_owned(),
            hash: None,
            decoded_sections: Vec::new(),
        },
        PackageSourceKind::Folder => Origin::Folder {
            path: path.clone(),
            format: format.to_owned(),
            file_hashes: BTreeMap::new(),
        },
    };
    pkg.sources = vec![SourceDescriptor {
        id: "src0".to_owned(),
        kind: kind.as_str().to_owned(),
        path: Some(path),
        format: Some(format.to_owned()),
        hash: None,
    }];
}

fn format_is_gridfm(format: &str) -> bool {
    normalize(format) == "gridfm"
}

fn format_is_distribution(format: &str) -> bool {
    use powerio_matrix::format::routing::{Detection, SourceFormat};
    matches!(
        powerio_matrix::format::routing::classify_format_name(format),
        Detection::Known(SourceFormat::Distribution(_))
    )
}

fn looks_like_gridfm_dir(input: &Path) -> bool {
    input.join("bus_data.parquet").is_file()
        || input.join("raw").join("bus_data.parquet").is_file()
        || std::fs::read_dir(input).is_ok_and(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .filter(|e| e.path().join("raw").join("bus_data.parquet").is_file())
                .take(2)
                .count()
                == 1
        })
}

fn looks_like_distribution_input(input: &Path) -> PyResult<bool> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("m" | "raw" | "aux" | "epc" | "pwb") => return Ok(false),
        Some("dss") => return Ok(true),
        Some("json") => {}
        _ => return Ok(false),
    }
    let text = std::fs::read_to_string(input).map_err(|e| {
        PyValueError::new_err(format!(
            "reading JSON format markers from {}: {e}",
            input.display()
        ))
    })?;
    use powerio_matrix::format::routing::{Detection, Domain};
    match powerio_matrix::format::routing::classify_json_text(&text) {
        Detection::Known(format) => Ok(format.domain() == Domain::Distribution),
        Detection::Unknown => Ok(false),
        Detection::Ambiguous => Err(PyValueError::new_err(format!(
            "ambiguous JSON markers in {}; pass from_",
            input.display()
        ))),
    }
}

fn build_package_from_path(
    input: &Path,
    from_: Option<&str>,
    scenario: i64,
) -> PyResult<CompilerPackage> {
    let reads_gridfm = from_.is_some_and(format_is_gridfm)
        || (from_.is_none() && input.is_dir() && looks_like_gridfm_dir(input));
    if reads_gridfm {
        #[cfg(feature = "gridfm")]
        {
            let read = gridfm_read_dataset(input.to_string_lossy().as_ref(), scenario)
                .map_err(to_pyerr)?;
            let mut pkg = CompilerPackage::from_balanced(read.network);
            add_package_read_warning_diagnostics(
                &mut pkg,
                "READ.GRIDFM.FIDELITY_WARNING",
                &read.warnings,
            );
            set_package_source(&mut pkg, input, PackageSourceKind::Folder, "gridfm", false);
            pkg.run_sane_validation();
            return Ok(pkg);
        }
        #[cfg(not(feature = "gridfm"))]
        {
            let _ = scenario;
            return Err(PyValueError::new_err(
                "powerio was built without the gridfm Parquet surface",
            ));
        }
    }

    if from_.is_some_and(format_is_distribution)
        || (from_.is_none() && looks_like_distribution_input(input)?)
    {
        let net = powerio_dist::parse_file(input, from_).map_err(dist_to_pyerr)?;
        let format = net
            .source_format
            .map(powerio_dist::DistSourceFormat::name)
            .or(from_)
            .unwrap_or("unknown");
        let retained_source = net.source.is_some();
        let mut pkg = CompilerPackage::from_multiconductor(net);
        set_package_source(
            &mut pkg,
            input,
            package_source_kind(input, format),
            format,
            retained_source,
        );
        pkg.run_sane_validation();
        return Ok(pkg);
    }

    let parsed = powerio_matrix::parse_file(input, from_).map_err(to_pyerr)?;
    let format = parsed.network.source_format.name();
    let retained_source = parsed.network.source.is_some();
    let mut pkg = CompilerPackage::from_balanced(parsed.network);
    add_package_read_warning_diagnostics(
        &mut pkg,
        "READ.TRANSMISSION.PARSE_WARNING",
        &parsed.warnings,
    );
    set_package_source(
        &mut pkg,
        input,
        package_source_kind(input, format),
        format,
        retained_source,
    );
    pkg.run_sane_validation();
    Ok(pkg)
}

fn build_package_from_str(text: &str, from_: Option<&str>) -> PyResult<CompilerPackage> {
    if from_.is_some_and(format_is_gridfm) {
        return Err(PyValueError::new_err(
            "gridfm input is a dataset directory; provide a path",
        ));
    }

    let source_format = if let Some(format) = from_ {
        Some(format.to_owned())
    } else {
        use powerio_matrix::format::routing::Detection;
        match powerio_matrix::format::routing::classify_json_text(text) {
            Detection::Known(format) => Some(format.name().to_owned()),
            Detection::Ambiguous => {
                return Err(PyValueError::new_err("ambiguous JSON markers; pass from_"));
            }
            Detection::Unknown => None,
        }
    };

    if let Some(format) = source_format.as_deref() {
        if format_is_distribution(format) {
            let net = powerio_dist::parse_str(text, format).map_err(dist_to_pyerr)?;
            let mut pkg = CompilerPackage::from_multiconductor(net);
            pkg.run_sane_validation();
            return Ok(pkg);
        }
    }

    let parsed = powerio_matrix::parse_str(text, source_format.as_deref().unwrap_or("matpower"))
        .map_err(to_pyerr)?;
    let mut pkg = CompilerPackage::from_balanced(parsed.network);
    add_package_read_warning_diagnostics(
        &mut pkg,
        "READ.TRANSMISSION.PARSE_WARNING",
        &parsed.warnings,
    );
    pkg.run_sane_validation();
    Ok(pkg)
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
#[pyclass(name = "PyNetwork")]
pub struct PyNetwork {
    inner: Network,
    core: IndexCore,
    warnings: Vec<String>,
}

/// Wrap a parse result as a `PyNetwork`, building the index core once and keeping
/// the reader's fidelity warnings on the handle.
fn case_from_parsed(parsed: powerio_matrix::Parsed) -> PyNetwork {
    let core = IndexCore::build(&parsed.network);
    PyNetwork {
        inner: parsed.network,
        core,
        warnings: parsed.warnings,
    }
}

fn pwd_display_to_dict<'py>(py: Python<'py>, display: &PwdDisplay) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("canvas_width", display.canvas_width)?;
    d.set_item("canvas_height", display.canvas_height)?;
    d.set_item("stamp", display.stamp)?;
    let mut rows = Vec::with_capacity(display.substations.len());
    for substation in &display.substations {
        let row = PyDict::new(py);
        row.set_item("number", substation.number)?;
        row.set_item("name", &substation.name)?;
        row.set_item("x", substation.x)?;
        row.set_item("y", substation.y)?;
        rows.push(row);
    }
    d.set_item("substations", PyList::new(py, rows)?)?;
    Ok(d)
}

fn display_data_to_py<'py>(py: Python<'py>, display: DisplayData) -> PyResult<Bound<'py, PyAny>> {
    match display {
        DisplayData::PowerWorld(display) => {
            let payload = pwd_display_to_dict(py, &display)?;
            Ok(("powerworld", payload).into_pyobject(py)?.into_any())
        }
        _ => Err(PowerIOError::new_err("unsupported display data kind")),
    }
}

#[pymethods]
impl PyNetwork {
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

    /// Read fidelity warnings attached at parse time: tables and columns the
    /// model cannot carry, reported instead of dropped silently. Empty for
    /// readers that don't report read warnings (currently every format except
    /// pandapower JSON and PyPSA CSV).
    #[getter]
    fn read_warnings(&self) -> Vec<String> {
        self.warnings.clone()
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
            let charging = br.terminal_charging();
            d.set_item("b", br.legacy_total_charging_b())?;
            d.set_item("g_fr", charging.g_fr)?;
            d.set_item("b_fr", charging.b_fr)?;
            d.set_item("g_to", charging.g_to)?;
            d.set_item("b_to", charging.b_to)?;
            d.set_item("rate_a", br.rate_a)?;
            d.set_item("rate_b", br.rate_b)?;
            d.set_item("rate_c", br.rate_c)?;
            let mut rating_sets = Vec::with_capacity(br.rating_sets.len());
            for rating in &br.rating_sets {
                let item = PyDict::new(py);
                item.set_item("name", &rating.name)?;
                item.set_item("rate_mva", rating.rate_mva)?;
                rating_sets.push(item);
            }
            d.set_item("rating_sets", PyList::new(py, rating_sets)?)?;
            d.set_item("c_rating_a", br.current_ratings.map(|r| r.c_rating_a))?;
            d.set_item("c_rating_b", br.current_ratings.map(|r| r.c_rating_b))?;
            d.set_item("c_rating_c", br.current_ratings.map(|r| r.c_rating_c))?;
            d.set_item("tap", br.tap)?;
            d.set_item("shift", br.shift)?;
            d.set_item("in_service", br.in_service)?;
            d.set_item("angmin", br.angmin)?;
            d.set_item("angmax", br.angmax)?;
            d.set_item("pf", br.solution.map(|s| s.pf))?;
            d.set_item("qf", br.solution.map(|s| s.qf))?;
            d.set_item("pt", br.solution.map(|s| s.pt))?;
            d.set_item("qt", br.solution.map(|s| s.qt))?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn switches<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner.switches.len());
        for sw in &self.inner.switches {
            let d = PyDict::new(py);
            d.set_item("from_id", sw.from.0)?;
            d.set_item("to_id", sw.to.0)?;
            d.set_item("closed", sw.closed)?;
            d.set_item("thermal_rating", sw.thermal_rating)?;
            d.set_item("current_rating", sw.current_rating)?;
            d.set_item("pf", sw.pf)?;
            d.set_item("qf", sw.qf)?;
            d.set_item("pt", sw.pt)?;
            d.set_item("qt", sw.qt)?;
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
    #[pyo3(signature = (to, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
    fn to_format(
        &self,
        to: &str,
        missing_gen_cost: Option<&str>,
        default_gen_cost: Option<&str>,
        gen_cost_csv: Option<&str>,
    ) -> PyResult<(String, Vec<String>)> {
        let target = to
            .parse::<powerio_matrix::TargetFormat>()
            .map_err(to_pyerr)?;
        let opts = write_options(
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
            MissingGenCostPolicy::Preserve,
        )?;
        let conv = self
            .inner
            .to_format_with_options(target, &opts)
            .map_err(to_pyerr)?;
        Ok((conv.text, conv.warnings))
    }

    /// A normalized, computation-ready copy of this case: per unit, radians,
    /// out-of-service filtered, densely reindexed (1-based), bus types
    /// canonicalized. The raw case is unchanged; the result carries no retained
    /// source, so writing it serializes the per-unit model rather than echoing.
    fn to_normalized(&self) -> PyResult<PyNetwork> {
        let inner = self.inner.to_normalized().map_err(to_pyerr)?;
        let core = IndexCore::build(&inner);
        Ok(PyNetwork {
            inner,
            core,
            warnings: self.warnings.clone(),
        })
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
        let parts = build_incidence(&view, conv, &BuildOptions::default()).map_err(to_pyerr)?;
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
        let parts = build_incidence(&view, conv, &BuildOptions::default()).map_err(to_pyerr)?;
        let l = build_weighted_laplacian(&parts.a, &parts.b);
        coo_triplets(py, &l)
    }

    /// Write the DC OPF bundle into `out_dir/<case>_dcopf/`. Returns
    /// `{"dir": str, "files": [str, ...]}`.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (out_dir, convention=None, units=None, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
    fn write_dcopf_bundle<'py>(
        &self,
        py: Python<'py>,
        out_dir: &str,
        convention: Option<&str>,
        units: Option<&str>,
        missing_gen_cost: Option<&str>,
        default_gen_cost: Option<&str>,
        gen_cost_csv: Option<&str>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let cost_opts = write_options(
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
            MissingGenCostPolicy::Require,
        )?;
        let opts = DcOpfOptions {
            convention: parse_convention(convention.unwrap_or("paper"))?,
            units: parse_units(units.unwrap_or("perunit"))?,
            missing_gen_cost: cost_opts.missing_gen_cost,
            gen_cost_patches: cost_opts.gen_cost_patches,
        };
        let outputs = write_bundle(&self.inner, out_dir, &opts).map_err(to_pyerr)?;
        dir_files_dict(py, &outputs.dir, &outputs.files)
    }

    /// Write the gridfm-datakit Parquet dataset for this case under
    /// `out_dir/<case>/raw/`. Returns
    /// `{"dir", "files", "dropped_zero_impedance", "degenerate_cost_gens"}`.
    /// Available when the extension is built with the Rust `gridfm` feature.
    #[cfg(feature = "gridfm")]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (out_dir, scenario=0, include_y_bus=true, include_taps=true, include_shifts=true, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
    fn write_gridfm<'py>(
        &self,
        py: Python<'py>,
        out_dir: &str,
        scenario: i64,
        include_y_bus: bool,
        include_taps: bool,
        include_shifts: bool,
        missing_gen_cost: Option<&str>,
        default_gen_cost: Option<&str>,
        gen_cost_csv: Option<&str>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let cost_opts = write_options(
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
            MissingGenCostPolicy::Preserve,
        )?;
        let opts = GridfmOptions {
            include_y_bus,
            include_taps,
            include_shifts,
            missing_gen_cost: cost_opts.missing_gen_cost,
            gen_cost_patches: cost_opts.gen_cost_patches,
        };
        let outputs =
            gridfm_write_dataset(&self.inner, scenario, out_dir, &opts).map_err(to_pyerr)?;
        gridfm_outputs_to_dict(py, &outputs)
    }

    /// Write this case as a PyPSA CSV folder. Returns
    /// `{"dir", "files", "warnings"}`.
    fn write_pypsa_csv_folder<'py>(
        &self,
        py: Python<'py>,
        out_dir: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let outputs =
            powerio_matrix::write_pypsa_csv_folder(&self.inner, out_dir).map_err(to_pyerr)?;
        pypsa_outputs_to_dict(py, &outputs)
    }

    fn __repr__(&self) -> String {
        format!(
            "Network(name={:?}, n_buses={}, n_branches={}, n_gens={})",
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
fn parse_file(path: &str, from_: Option<&str>) -> PyResult<PyNetwork> {
    powerio_matrix::parse_file(std::path::Path::new(path), from_)
        .map(case_from_parsed)
        .map_err(to_pyerr)
}

/// Parse a case from in-memory text in the named `format` (`matpower`,
/// `powermodels-json`, `egret-json`, `pandapower-json`, `psse`, `powerworld`,
/// `pslf`, `goc3-json`, `surge-json`; aliases `m`/`pm`/`egret`/`pp`/`raw`/`aux`/`epc`/`goc3`/`surge`).
#[pyfunction]
#[pyo3(signature = (text, format=None))]
fn parse_str(text: &str, format: Option<&str>) -> PyResult<PyNetwork> {
    powerio_matrix::parse_str(text, format.unwrap_or("matpower"))
        .map(case_from_parsed)
        .map_err(to_pyerr)
}

/// Parse a display file from a path, inferring the format from the extension
/// unless `from_` is given. Returns `(kind, payload)`.
#[pyfunction]
#[pyo3(signature = (path, from_=None))]
fn parse_display_file<'py>(
    py: Python<'py>,
    path: &str,
    from_: Option<&str>,
) -> PyResult<Bound<'py, PyAny>> {
    let display =
        powerio_matrix::parse_display_file(std::path::Path::new(path), from_).map_err(to_pyerr)?;
    display_data_to_py(py, display)
}

/// Parse display bytes in the named display format. Returns `(kind, payload)`.
#[pyfunction]
#[pyo3(signature = (data, format))]
fn parse_display_bytes<'py>(
    py: Python<'py>,
    data: &[u8],
    format: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let display = powerio_matrix::parse_display_bytes(data, format).map_err(to_pyerr)?;
    display_data_to_py(py, display)
}

/// Rebuild a case from JSON produced by `Network.to_json()`.
#[pyfunction]
fn from_json(text: &str) -> PyResult<PyNetwork> {
    let inner = powerio_matrix::Network::from_json(text).map_err(to_pyerr)?;
    let core = IndexCore::build(&inner);
    Ok(PyNetwork {
        inner,
        core,
        warnings: Vec::new(),
    })
}

/// Read a PyPSA CSV folder into a case.
#[pyfunction]
fn read_pypsa_csv_folder(path: &str) -> PyResult<PyNetwork> {
    powerio_matrix::read_pypsa_csv_folder(std::path::Path::new(path))
        .map(case_from_parsed)
        .map_err(to_pyerr)
}

/// Convert a case file to another format through the neutral hub. Returns
/// `(text, warnings)`: the converted file text and the list of fidelity warnings
/// (fields the target couldn't represent). The input format is the file
/// extension unless `from` overrides it.
#[pyfunction]
#[pyo3(signature = (path, to, from_=None, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
fn convert_file(
    path: &str,
    to: &str,
    from_: Option<&str>,
    missing_gen_cost: Option<&str>,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&str>,
) -> PyResult<(String, Vec<String>)> {
    let target = to
        .parse::<powerio_matrix::TargetFormat>()
        .map_err(to_pyerr)?;
    let opts = write_options(
        missing_gen_cost,
        default_gen_cost,
        gen_cost_csv,
        MissingGenCostPolicy::Preserve,
    )?;
    let conv =
        powerio_matrix::convert_file_with_options(std::path::Path::new(path), target, from_, &opts)
            .map_err(to_pyerr)?;
    Ok((conv.text, conv.warnings))
}

/// Convert in-memory case `text` to another format through the neutral hub,
/// with no file staging. Returns `(text, warnings)` like `convert_file`.
/// `format` names the input format (default `matpower`).
#[pyfunction]
#[pyo3(signature = (text, to, format=None, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
fn convert_str(
    text: &str,
    to: &str,
    format: Option<&str>,
    missing_gen_cost: Option<&str>,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&str>,
) -> PyResult<(String, Vec<String>)> {
    let target = to
        .parse::<powerio_matrix::TargetFormat>()
        .map_err(to_pyerr)?;
    let opts = write_options(
        missing_gen_cost,
        default_gen_cost,
        gen_cost_csv,
        MissingGenCostPolicy::Preserve,
    )?;
    let conv =
        powerio_matrix::convert_str_with_options(text, target, format.unwrap_or("matpower"), &opts)
            .map_err(to_pyerr)?;
    Ok((conv.text, conv.warnings))
}

fn dist_to_pyerr(e: powerio_dist::Error) -> PyErr {
    use powerio_dist::Error as E;
    let msg = e.to_string();
    match e {
        // OSError(errno, strerror, filename) lets CPython pick the precise
        // subclass (FileNotFoundError etc.) while keeping the path on
        // e.filename, which a bare io::Error conversion would drop.
        E::Io { path, source } => match source.raw_os_error() {
            Some(errno) => pyo3::exceptions::PyOSError::new_err((errno, source.to_string(), path)),
            None => PowerIOError::new_err(msg),
        },
        E::UnknownFormat(_) => PyValueError::new_err(msg),
        E::Json { .. } => PowerIOParseError::new_err(msg),
        _ => PowerIOError::new_err(msg),
    }
}

/// Low-level handle around a parsed multiconductor distribution network in
/// wire coordinates (OpenDSS, PMD ENGINEERING JSON, BMOPF JSON). The
/// user-facing `powerio.dist.DistNetwork` wraps it.
#[pyclass(name = "_DistNetwork", frozen)]
struct PyDistNetwork {
    net: powerio_dist::DistNetwork,
}

#[pymethods]
impl PyDistNetwork {
    fn name(&self) -> Option<&str> {
        self.net.name.as_deref()
    }

    /// Format the case was parsed from (`dss`, `pmd-json`, `bmopf-json`).
    fn source_format(&self) -> Option<&'static str> {
        self.net.source_format.map(|f| f.name())
    }

    /// Parse warnings: everything the reader could not represent or had to
    /// assume.
    fn warnings(&self) -> Vec<String> {
        self.net.warnings.clone()
    }

    fn n_buses(&self) -> usize {
        self.net.buses.len()
    }

    fn n_lines(&self) -> usize {
        self.net.lines.len()
    }

    fn n_transformers(&self) -> usize {
        self.net.transformers.len()
    }

    fn n_loads(&self) -> usize {
        self.net.loads.len()
    }

    fn n_generators(&self) -> usize {
        self.net.generators.len()
    }

    fn n_sources(&self) -> usize {
        self.net.sources.len()
    }

    /// Serialize to `to` (`dss`, `pmd-json`, `bmopf-json`). Returns
    /// `(text, warnings)`. Writing back to the source format echoes the
    /// retained source byte for byte.
    fn to_format(&self, to: &str) -> PyResult<(String, Vec<String>)> {
        let target = to
            .parse::<powerio_dist::DistTargetFormat>()
            .map_err(dist_to_pyerr)?;
        let conv = self.net.to_format(target);
        Ok((conv.text, conv.warnings))
    }

    fn __repr__(&self) -> String {
        format!(
            "DistNetwork(n_buses={}, n_lines={}, n_transformers={}, n_loads={})",
            self.net.buses.len(),
            self.net.lines.len(),
            self.net.transformers.len(),
            self.net.loads.len()
        )
    }
}

/// Parse a distribution case file. The format comes from `from_` when given,
/// else from the file itself (`.dss`, or `.json` sniffed for the PMD
/// ENGINEERING `data_model` key against the BMOPF layout).
#[pyfunction]
#[pyo3(signature = (path, from_=None))]
fn dist_parse_file(path: &str, from_: Option<&str>) -> PyResult<PyDistNetwork> {
    powerio_dist::parse_file(std::path::Path::new(path), from_)
        .map(|net| PyDistNetwork { net })
        .map_err(dist_to_pyerr)
}

/// Parse an in-memory distribution case of the named `format` (`dss`,
/// `pmd-json`, `bmopf-json`).
#[pyfunction]
fn dist_parse_str(text: &str, format: &str) -> PyResult<PyDistNetwork> {
    powerio_dist::parse_str(text, format)
        .map(|net| PyDistNetwork { net })
        .map_err(dist_to_pyerr)
}

/// Convert a distribution case file to `to`. Returns `(text, warnings)`; the
/// warnings carry both the parse warnings and the writer's fidelity losses.
#[pyfunction]
#[pyo3(signature = (path, to, from_=None))]
fn dist_convert_file(path: &str, to: &str, from_: Option<&str>) -> PyResult<(String, Vec<String>)> {
    let to = to
        .parse::<powerio_dist::DistTargetFormat>()
        .map_err(dist_to_pyerr)?;
    let conv =
        powerio_dist::convert_file(std::path::Path::new(path), to, from_).map_err(dist_to_pyerr)?;
    Ok((conv.text, conv.warnings))
}

/// Convert an in-memory distribution case of the named `format` to `to`.
/// Returns `(text, warnings)`; the warnings carry both the parse warnings and
/// the writer's fidelity losses.
#[pyfunction]
fn dist_convert_str(text: &str, to: &str, format: &str) -> PyResult<(String, Vec<String>)> {
    let to = to
        .parse::<powerio_dist::DistTargetFormat>()
        .map_err(dist_to_pyerr)?;
    let conv = powerio_dist::convert_str(text, to, format).map_err(dist_to_pyerr)?;
    Ok((conv.text, conv.warnings))
}

/// Return the explicit top-level model kind from a validated `.pio.json`
/// package.
#[pyfunction]
fn package_model_kind(text: &str) -> PyResult<String> {
    let pkg = CompilerPackage::from_json(text).map_err(package_pyerr)?;
    Ok(match pkg.model_kind() {
        powerio_pkg::ModelKind::Balanced => "balanced",
        powerio_pkg::ModelKind::Multiconductor => "multiconductor",
        _ => "unknown",
    }
    .to_owned())
}

/// Parse a case file and return a `.pio.json` compiler package.
#[pyfunction]
#[pyo3(signature = (path, from_=None, scenario=0))]
fn package_parse_file(path: &str, from_: Option<&str>, scenario: i64) -> PyResult<String> {
    let pkg = build_package_from_path(Path::new(path), from_, scenario)?;
    package_to_json(&pkg)
}

/// Parse in-memory case text and return a `.pio.json` compiler package.
#[pyfunction]
#[pyo3(signature = (text, from_=None))]
fn package_parse_str(text: &str, from_: Option<&str>) -> PyResult<String> {
    let pkg = build_package_from_str(text, from_)?;
    package_to_json(&pkg)
}

/// Rebuild a balanced network handle from a validated `.pio.json` package.
#[pyfunction]
fn package_as_balanced(text: &str) -> PyResult<PyNetwork> {
    CompilerPackage::from_json(text)
        .map_err(package_pyerr)
        .and_then(package_to_balanced_py)
}

/// Rebuild a multiconductor network handle from a validated `.pio.json`
/// package.
#[pyfunction]
fn package_as_multiconductor(text: &str) -> PyResult<PyDistNetwork> {
    CompilerPackage::from_json(text)
        .map_err(package_pyerr)
        .and_then(package_to_dist_py)
}

/// Return the package operating point series as JSON, or `null` when absent.
#[pyfunction]
fn package_operating_points(text: &str) -> PyResult<String> {
    let pkg = CompilerPackage::from_json(text).map_err(package_pyerr)?;
    serde_json::to_string(&pkg.operating_points).map_err(package_pyerr)
}

/// Materialize one operating point and return the resulting static package JSON.
#[pyfunction]
fn package_materialize_operating_point(text: &str, index: usize) -> PyResult<String> {
    let pkg = CompilerPackage::from_json(text).map_err(package_pyerr)?;
    let materialized = pkg
        .materialize_operating_point(index)
        .map_err(package_pyerr)?;
    package_to_json(&materialized)
}

/// Classify top level JSON markers. Returns `(status, domain, format)` where
/// `status` is `known`, `unknown`, or `ambiguous`.
#[pyfunction]
fn classify_json_text(text: &str) -> (String, Option<String>, Option<String>) {
    match powerio_matrix::format::routing::classify_json_text(text) {
        powerio_matrix::format::routing::Detection::Known(format) => (
            "known".into(),
            Some(match format.domain() {
                powerio_matrix::format::routing::Domain::Transmission => "transmission".into(),
                powerio_matrix::format::routing::Domain::Distribution => "distribution".into(),
                _ => "unknown".into(),
            }),
            Some(format.name().into()),
        ),
        powerio_matrix::format::routing::Detection::Unknown => ("unknown".into(), None, None),
        powerio_matrix::format::routing::Detection::Ambiguous => ("ambiguous".into(), None, None),
    }
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
    d.set_item("missing_cost_gens", outputs.missing_cost_gens)?;
    d.set_item("unsupported_cost_gens", outputs.unsupported_cost_gens)?;
    d.set_item("synthesized_gen_costs", outputs.synthesized_gen_costs)?;
    d.set_item("patched_gen_costs", outputs.patched_gen_costs)?;
    Ok(d)
}

fn pypsa_outputs_to_dict<'py>(
    py: Python<'py>,
    outputs: &powerio_matrix::PypsaCsvOutputs,
) -> PyResult<Bound<'py, PyDict>> {
    let d = dir_files_dict(py, &outputs.dir, &outputs.files)?;
    d.set_item("warnings", &outputs.warnings)?;
    Ok(d)
}

/// Write a batch of cases as one gridfm-datakit dataset, row-stacked and keyed by
/// the `scenario` column. The k-th case is stamped `base_scenario + k`; all cases
/// must share one base element set (same bus/branch/gen counts and bus-id order).
/// Available when the extension is built with the Rust `gridfm` feature.
#[cfg(feature = "gridfm")]
#[allow(clippy::too_many_arguments)]
#[pyfunction]
#[pyo3(signature = (cases, out_dir, base_scenario=0, include_y_bus=true, include_taps=true, include_shifts=true, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
fn write_gridfm_batch<'py>(
    py: Python<'py>,
    cases: Vec<PyRef<'py, PyNetwork>>,
    out_dir: &str,
    base_scenario: i64,
    include_y_bus: bool,
    include_taps: bool,
    include_shifts: bool,
    missing_gen_cost: Option<&str>,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&str>,
) -> PyResult<Bound<'py, PyDict>> {
    let cost_opts = write_options(
        missing_gen_cost,
        default_gen_cost,
        gen_cost_csv,
        MissingGenCostPolicy::Preserve,
    )?;
    let opts = GridfmOptions {
        include_y_bus,
        include_taps,
        include_shifts,
        missing_gen_cost: cost_opts.missing_gen_cost,
        gen_cost_patches: cost_opts.gen_cost_patches,
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
/// `PyNetwork` (with its index core, exactly as `parse_file` does), the scenario id,
/// and the fidelity warnings the lossy read surfaced.
#[cfg(feature = "gridfm")]
fn gridfm_read_to_py(read: GridfmRead) -> (PyNetwork, i64, Vec<String>) {
    let core = IndexCore::build(&read.network);
    (
        PyNetwork {
            inner: read.network,
            core,
            warnings: read.warnings.clone(),
        },
        read.scenario,
        read.warnings,
    )
}

/// Read one scenario of a gridfm-datakit Parquet dataset back into a case,
/// returning `(case, scenario, warnings)` (the pure-Python layer wraps it as a
/// `GridfmRead` namedtuple). `dir` resolves leniently: the `raw/` leaf, a
/// `<case>/` directory, or a parent with one `*/raw/` child. The read is lossy but
/// power flow complete; `warnings` lists what the gridfm schema couldn't
/// round-trip. Available when the extension is built with the Rust `gridfm` feature.
#[cfg(feature = "gridfm")]
#[pyfunction]
#[pyo3(signature = (dir, scenario=0))]
fn read_gridfm(dir: &str, scenario: i64) -> PyResult<(PyNetwork, i64, Vec<String>)> {
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
fn read_gridfm_scenarios(dir: &str) -> PyResult<Vec<(PyNetwork, i64, Vec<String>)>> {
    let reads = gridfm_read_scenarios(dir).map_err(to_pyerr)?;
    Ok(reads.into_iter().map(gridfm_read_to_py).collect())
}

#[pymodule]
fn _powerio(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("PowerIOError", m.py().get_type::<PowerIOError>())?;
    m.add("PowerIOParseError", m.py().get_type::<PowerIOParseError>())?;
    m.add("PowerIODataError", m.py().get_type::<PowerIODataError>())?;
    m.add_class::<PyNetwork>()?;
    m.add_function(wrap_pyfunction!(parse_file, m)?)?;
    m.add_function(wrap_pyfunction!(parse_str, m)?)?;
    m.add_function(wrap_pyfunction!(parse_display_file, m)?)?;
    m.add_function(wrap_pyfunction!(parse_display_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(from_json, m)?)?;
    m.add_function(wrap_pyfunction!(read_pypsa_csv_folder, m)?)?;
    m.add_function(wrap_pyfunction!(convert_file, m)?)?;
    m.add_function(wrap_pyfunction!(convert_str, m)?)?;
    m.add_class::<PyDistNetwork>()?;
    m.add_function(wrap_pyfunction!(dist_parse_file, m)?)?;
    m.add_function(wrap_pyfunction!(dist_parse_str, m)?)?;
    m.add_function(wrap_pyfunction!(dist_convert_file, m)?)?;
    m.add_function(wrap_pyfunction!(dist_convert_str, m)?)?;
    m.add_function(wrap_pyfunction!(package_model_kind, m)?)?;
    m.add_function(wrap_pyfunction!(package_parse_file, m)?)?;
    m.add_function(wrap_pyfunction!(package_parse_str, m)?)?;
    m.add_function(wrap_pyfunction!(package_as_balanced, m)?)?;
    m.add_function(wrap_pyfunction!(package_as_multiconductor, m)?)?;
    m.add_function(wrap_pyfunction!(package_operating_points, m)?)?;
    m.add_function(wrap_pyfunction!(package_materialize_operating_point, m)?)?;
    m.add_function(wrap_pyfunction!(classify_json_text, m)?)?;
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
