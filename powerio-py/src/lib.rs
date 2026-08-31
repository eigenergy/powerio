//! PyO3 extension behind the `powerio` Python package.
//!
//! The extension exposes parsing, writing, conversion, matrices, packages, and
//! problem instances. Parse and emission values cross as Python dictionaries
//! and strings, so the base package does not import NumPy or SciPy.
//!
//! The matrix methods hand back COO triplets as plain Python lists
//! (`data`, `row`, `col`, `shape`); there is no NumPy at this layer. The
//! Python `powerio` package assembles those into `scipy.sparse` matrices and
//! NetworkX graphs when the corresponding extra is installed.
//!
//! Indices narrow to `i32` to match SciPy's default index width.
//! `coo_triplets` checks the bound before conversion.

use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use sprs::CsMat;

use powerio::{BalancedNetwork, BranchSusceptanceFormula, DisplayData, PwdDisplay};
use powerio_matrix::DcOperators;
use powerio_matrix::matrix::{
    BuildOptions, Scheme, SensitivityOptions, SensitivitySolver, calc_adjacency_matrix,
    calc_admittance_matrix, calc_bdoubleprime_matrix, calc_bprime_matrix, calc_lacpf_matrix,
    calc_ptdf_lodf_with_options,
};
use powerio_matrix::{
    DcOpfAssemblyOptions, DcOpfBundleMetadata, DcOpfBundleOptions, Units,
    emit_dcopf_bundle as write_bundle,
};
use powerio_tx::{
    Detection, EmitOptions, IndexCore, IndexedNetwork, JsonClass, MissingGenCostPolicy,
    NormalizeOptions, POWER_MODELS_ANGLE_BOUND_PAD, TargetFormat,
    classify_json_text as classify_balanced_json_text, parse_gen_cost_csv, parse_target_format,
};

#[cfg(feature = "gridfm")]
use powerio_matrix::io::gridfm::{
    GridfmOptions, GridfmOutputs, emit_gridfm_batch as gridfm_write_batch,
    emit_gridfm_dataset as gridfm_write_dataset, number_snapshots as numbered_snapshots,
};

pyo3::create_exception!(
    powerio,
    PowerIOError,
    pyo3::exceptions::PyValueError,
    "Base error raised by the powerio parser, emitter, or matrix calculations.\n\n\
     Subclasses `ValueError`: every failure it covers is a statement about a \
     value the caller supplied, and `except ValueError` was what callers wrote \
     before the hierarchy existed. I/O failures do not reach it; they raise the \
     matching `OSError` subclass by value. Failures mapped from the Rust core \
     carry the diagnostic code string as a `.code` attribute."
);

pyo3::create_exception!(
    powerio,
    PowerIOParseError,
    PowerIOError,
    "A case file is malformed or unparseable (missing/short rows, bad numbers, \
     unbalanced brackets, format read failures)."
);

pyo3::create_exception!(
    powerio,
    PowerIODataError,
    PowerIOError,
    "A well-formed case cannot satisfy a requested operation (no generators, \
     wrong reference bus count, an unknown bus reference, zero/non-finite \
     branch impedance, a disconnected or singular network, a scenario batch \
     shape mismatch, or a dimension/cost mismatch)."
);

/// Map a PowerIO error onto the right Python exception, driven by
/// [`Error::category`]. I/O failures become the matching `OSError` subclass
/// (`FileNotFoundError`, `PermissionError`, …); an unknown/uninferable format
/// becomes a `ValueError`; malformed input becomes [`PowerIOParseError`] and an
/// unmet operation precondition becomes [`PowerIODataError`]. Both subclass
/// [`PowerIOError`], so existing `except PowerIOError` handlers keep working;
/// output-side write failures fall back to the [`PowerIOError`] base.
/// Map a classified powerio failure onto the Python exception hierarchy.
///
/// Every crate's error carries the same `category()`, so one mapping serves
/// all of them and the hierarchy cannot drift per surface.
fn categorized_pyerr(
    category: powerio_tx::ErrorCategory,
    code: &'static str,
    msg: String,
) -> PyErr {
    use powerio_tx::ErrorCategory as C;
    let err = match category {
        // A request refusal is a PowerIOError, never a bare ValueError, the
        // same rule core_error_pyerr states; PowerIOError subclasses
        // ValueError so existing except clauses keep matching.
        C::Request => PowerIOError::new_err(msg),
        C::Parse => PowerIOParseError::new_err(msg),
        C::Data => PowerIODataError::new_err(msg),
        // `Io` is unwrapped by the callers below when it still carries the
        // original `std::io::Error`; `Output` (mtx/parquet) maps to the base.
        C::Io | C::Output => PowerIOError::new_err(msg),
    };
    with_code(err, code)
}

/// Attach the diagnostic code as a `.code` attribute on the exception value,
/// the Python counterpart of `Error::code()`.
fn with_code(err: PyErr, code: &'static str) -> PyErr {
    Python::attach(|py| {
        let _ = err.value(py).setattr("code", code);
    });
    err
}

fn core_pyerr(e: powerio_tx::Error) -> PyErr {
    // Hand I/O to PyO3 by value so it picks the precise `OSError` subclass.
    if let powerio_tx::Error::Io(io) = e {
        return io.into();
    }
    let category = e.category();
    let code = e.code().code;
    categorized_pyerr(category, code, e.to_string())
}

fn to_pyerr(e: powerio_matrix::Error) -> PyErr {
    use powerio_matrix::Error as E;
    match e {
        E::Io(io) => io.into(),
        E::Transmission(inner) => core_pyerr(inner),
        other => {
            let category = other.category();
            let code = other.code().code;
            categorized_pyerr(category, code, other.to_string())
        }
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

/// Parse one stable 1.0 branch susceptance formula name.
fn parse_formula(s: &str) -> PyResult<BranchSusceptanceFormula> {
    BranchSusceptanceFormula::from_formula_name(s).ok_or_else(|| {
        PyValueError::new_err(format!(
            "unknown branch susceptance formula {s:?}; expected \
             'series_susceptance', 'tap_adjusted_reactance', or 'reactance_only'"
        ))
    })
}

/// PTDF/LODF options from the Python keywords. The solver defaults to `auto`,
/// which is dense below the reduced-dimension threshold and the sparse
/// Cholesky path above it — the same policy the CLI `sensitivities` command
/// applies, so a very large case cannot force the dense n×n factorization
/// from Python.
fn sensitivity_options(
    formula: Option<&str>,
    solver: Option<&str>,
) -> PyResult<SensitivityOptions> {
    let formula = parse_formula(formula.unwrap_or("series_susceptance"))?;
    let solver = match normalize(solver.unwrap_or("auto")).as_str() {
        "auto" => SensitivitySolver::Auto,
        "dense" => SensitivitySolver::Dense,
        "sparse" => SensitivitySolver::Sparse,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown solver {other:?}; expected 'auto', 'dense', or 'sparse'"
            )));
        }
    };
    Ok(SensitivityOptions {
        formula,
        solver,
        ..SensitivityOptions::default()
    })
}

/// Accepts `perunit`/`pu`/`per-unit` and `native`.
fn parse_units(s: &str) -> PyResult<Units> {
    // The alias table is `Units: FromStr` in powerio-prob, shared with the
    // C ABI binding.
    s.parse::<Units>().map_err(PyValueError::new_err)
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
            Ok(MissingGenCostPolicy::calc_quadratic(c2, c1, c0))
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

fn emit_options(
    missing_gen_cost: Option<&str>,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&str>,
    default_policy: MissingGenCostPolicy,
) -> PyResult<EmitOptions> {
    let missing_gen_cost =
        parse_missing_gen_cost(missing_gen_cost, default_gen_cost, default_policy)?;
    let gen_cost_patches = match gen_cost_csv {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                PyValueError::new_err(format!("reading gen_cost_csv {path:?}: {e}"))
            })?;
            parse_gen_cost_csv(&text).map_err(core_pyerr)?
        }
        None => Vec::new(),
    };
    Ok(EmitOptions {
        missing_gen_cost,
        gen_cost_patches,
    })
}

/// Extract the single UTF-8 artifact produced for an in-memory file target.
/// Directory formats must name a destination when they produce companions;
/// returning only their primary text would silently discard the inventory.
fn emitted_text(
    result: powerio_core::EmitResult,
    format: &str,
) -> PyResult<(String, Vec<powerio_core::Diagnostic>)> {
    let diagnostics = result.diagnostics().to_vec();
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        unreachable!("a memory destination always returns memory artifacts")
    };
    if artifacts.len() != 1 {
        return Err(PowerIOError::new_err(format!(
            "{format} emits {} artifacts; provide a destination path",
            artifacts.len()
        )));
    }
    let text = String::from_utf8(artifacts.remove(0).into_bytes()).map_err(|_| {
        PowerIOError::new_err(format!("{format} emitted bytes that are not UTF-8 text"))
    })?;
    Ok((text, diagnostics))
}

fn py_diagnostics(result: &powerio_core::EmitResult) -> Vec<PyDiagnostic> {
    result
        .diagnostics()
        .iter()
        .map(PyDiagnostic::from)
        .collect()
}

/// A JSON serialization failure in this binding's own writer, raised on the
/// base class with the emit code, the same classification `Error::code()`
/// gave it when the stored document owned the writer.
fn serialize_pyerr(e: serde_json::Error) -> PyErr {
    categorized_pyerr(
        powerio_tx::ErrorCategory::Output,
        powerio::codes::EMIT_MODULE_SERIALIZE_FAILED.code,
        e.to_string(),
    )
}

/// One list of 1.0 diagnostic records as the JSON array every diagnostics
/// surface (a stored module, a parsed network, the balanced lowering
/// readiness report) publishes: code, severity, message, and target, the
/// last `null` when the finding carries none.
fn diagnostics_json_array(diagnostics: &[powerio_core::Diagnostic]) -> Vec<serde_json::Value> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            serde_json::json!({
                "code": diagnostic.code(),
                "severity": format!("{:?}", diagnostic.severity()).to_lowercase(),
                "message": diagnostic.message(),
                "target": diagnostic.target(),
            })
        })
        .collect()
}

/// Package a GO Challenge 3 document: the balanced snapshot plus the operating
/// point series the document carries.
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

fn build_options(
    scheme: Scheme,
    include_taps: bool,
    include_shifts: bool,
    skip_zero_impedance: bool,
) -> BuildOptions {
    BuildOptions {
        scheme,
        include_taps,
        include_shifts,
        skip_zero_impedance,
    }
}

/// Low level handle around a parsed [`BalancedNetwork`]. The public `powerio.BalancedNetwork`
/// (pure Python) wraps this: the IO getters and topology methods delegate
/// straight to it, and the matrix methods turn its COO tuples into scipy.
///
/// The derived [`IndexCore`] is built once and cached alongside `inner`, so the
/// matrix calculations and topology getters reuse it instead of rebuilding the
/// bus-id map per call.
#[pyclass(name = "_BalancedNetwork", module = "powerio._powerio")]
pub struct PyBalancedNetwork {
    /// The parsed module: the typed network plus retained source and the
    /// reader's findings. Same format writes echo the retained bytes exactly;
    /// a handle built from a bare network writes canonically.
    module: powerio_core::PioModule<BalancedNetwork>,
    core: IndexCore,
}

impl PyBalancedNetwork {
    fn inner(&self) -> &BalancedNetwork {
        self.module.value()
    }

    fn diagnostics(&self) -> &[powerio_core::Diagnostic] {
        self.module.diagnostics()
    }

    fn dc_operators(&self, formula: &str) -> PyResult<DcOperators> {
        let formula = parse_formula(formula)?;
        let instance = powerio_prob::DcPfInstance::from_network(self.inner().clone())
            .map_err(|error| core_error_pyerr(&error))?
            .with_branch_susceptance_formula(formula);
        DcOperators::build(&instance).map_err(|error| core_error_pyerr(&error))
    }
}

fn core_error_pyerr(error: &powerio_core::Error) -> PyErr {
    // Parse and Data map onto their subclasses so a caller branching on
    // `except powerio.PowerIODataError` sees one taxonomy whichever layer
    // raised the failure; the other categories keep the base class the 0.9
    // entries raised (a request refusal is a PowerIOError, never ValueError).
    let err = match error.category() {
        powerio_core::ErrorCategory::Parse => PowerIOParseError::new_err(error.to_string()),
        powerio_core::ErrorCategory::Data => PowerIODataError::new_err(error.to_string()),
        _ => PowerIOError::new_err(error.to_string()),
    };
    if let Some(code) = error.diagnostics().first().map(|d| d.code().to_owned()) {
        Python::attach(|py| {
            let _ = err.value(py).setattr("code", code);
        });
    }
    err
}

/// Projects a source acquisition failure with an operating system cause onto
/// the precise `OSError` subclass with the path attached, matching what the
/// old path entries raised; anything else falls back to the coded error.
fn core_open_pyerr(path: &std::path::Path, error: &powerio_core::Error) -> PyErr {
    let mut cause = std::error::Error::source(error);
    while let Some(inner) = cause {
        if let Some(io) = inner.downcast_ref::<std::io::Error>()
            && let Some(errno) = io.raw_os_error()
        {
            return pyo3::exceptions::PyOSError::new_err((
                errno,
                io.to_string(),
                path.display().to_string(),
            ));
        }
        cause = inner.source();
    }
    core_error_pyerr(error)
}

/// Wrap a parsed module as a `PyBalancedNetwork`, building the index core once
/// and keeping the reader's findings on the handle.
fn case_from_module(module: powerio_core::PioModule<BalancedNetwork>) -> PyBalancedNetwork {
    let core = IndexCore::build(module.value());
    PyBalancedNetwork { core, module }
}

/// Wrap a bare network with findings: derived handles carry no retained
/// source and write canonically.
fn case_from_parts(
    network: BalancedNetwork,
    diagnostics: Vec<powerio_core::Diagnostic>,
) -> PyBalancedNetwork {
    let mut module = powerio_core::PioModule::new(network);
    for diagnostic in diagnostics {
        // A refusal here can only be the record cap; the finding that does
        // not fit is dropped rather than failing the handle it annotates.
        let _ = module.add_diagnostic(diagnostic);
    }
    case_from_module(module)
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
impl PyBalancedNetwork {
    // --- metadata -------------------------------------------------------

    #[getter]
    fn name(&self) -> String {
        self.inner().name().clone()
    }

    #[getter]
    fn base_mva(&self) -> f64 {
        self.inner().base_mva()
    }

    #[getter]
    fn base_frequency(&self) -> f64 {
        self.inner().base_frequency()
    }

    #[getter]
    fn source_format(&self) -> String {
        self.inner().source_format().name().to_owned()
    }

    #[getter]
    fn n_buses(&self) -> usize {
        self.inner().buses().len()
    }

    #[getter]
    fn n_branches(&self) -> usize {
        self.inner().branches().len()
    }

    #[getter]
    fn n_gens(&self) -> usize {
        self.inner().generators().len()
    }

    /// Preferred spelling; `n_gens` remains for 0.10 compatibility.
    #[getter]
    fn n_generators(&self) -> usize {
        self.inner().generators().len()
    }

    #[getter]
    fn n_loads(&self) -> usize {
        self.inner().loads().len()
    }

    #[getter]
    fn n_shunts(&self) -> usize {
        self.inner().shunts().len()
    }

    #[getter]
    fn n_switches(&self) -> usize {
        self.inner().switches().len()
    }

    #[getter]
    fn n_storage(&self) -> usize {
        self.inner().storage().len()
    }

    #[getter]
    fn n_hvdc(&self) -> usize {
        self.inner().hvdc().len()
    }

    #[getter]
    fn n_transformers_3w(&self) -> usize {
        self.inner().transformers_3w().len()
    }

    #[getter]
    fn n_areas(&self) -> usize {
        self.inner().areas().len()
    }

    #[getter]
    fn is_radial(&self) -> bool {
        IndexedNetwork::with_core(self.inner(), &self.core).is_radial()
    }

    #[getter]
    fn n_connected_components(&self) -> usize {
        IndexedNetwork::with_core(self.inner(), &self.core).calc_island_count()
    }

    /// Power system terminology for `n_connected_components`.
    #[getter]
    fn n_islands(&self) -> usize {
        IndexedNetwork::with_core(self.inner(), &self.core).calc_island_count()
    }

    /// Dense `[0, n)` index of the single reference bus. Raises if not exactly
    /// one reference bus is present; for the multi-reference case use
    /// :meth:`reference_bus_indices`.
    fn reference_bus_index(&self) -> PyResult<usize> {
        IndexedNetwork::with_core(self.inner(), &self.core)
            .reference_bus_index()
            .map_err(core_pyerr)
    }

    /// Dense `[0, n)` indices of every reference (slack) bus, ascending. May be
    /// empty (no reference) or hold several (a slack per island, or a normalized
    /// case that kept the file's multiple references).
    fn reference_bus_indices(&self) -> Vec<usize> {
        IndexedNetwork::with_core(self.inner(), &self.core).reference_bus_indices()
    }

    /// The star-lowered network: each in-service 3-winding transformer replaced
    /// by its star bus and three branches. This is the space `bprime`, `ybus`,
    /// `is_radial`, `n_connected_components` and `reference_bus_indices` are
    /// computed over, while `buses` and `branches` mirror the case file. The
    /// two differ only for a case that carries such a transformer; otherwise
    /// this returns the same tables.
    fn lowered(&self) -> PyBalancedNetwork {
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let inner = view.network().clone();
        let core = IndexCore::build(&inner);
        let _ = core;
        case_from_parts(inner, self.diagnostics().to_vec())
    }

    // --- tables (the format-neutral BalancedNetwork, as dict rows) --------------

    #[getter]
    fn buses<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner().buses().len());
        for b in self.inner().buses() {
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
            d.set_item("uid", b.uid.as_deref())?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn loads<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner().loads().len());
        for l in self.inner().loads() {
            let d = PyDict::new(py);
            d.set_item("bus", l.bus.0)?;
            d.set_item("p", l.p)?;
            d.set_item("q", l.q)?;
            d.set_item("in_service", l.in_service)?;
            d.set_item("uid", l.uid.as_deref())?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn shunts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner().shunts().len());
        for s in self.inner().shunts() {
            let d = PyDict::new(py);
            d.set_item("bus", s.bus.0)?;
            d.set_item("g", s.g)?;
            d.set_item("b", s.b)?;
            d.set_item("in_service", s.in_service)?;
            d.set_item("uid", s.uid.as_deref())?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn branches<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner().branches().len());
        for br in self.inner().branches() {
            let d = PyDict::new(py);
            d.set_item("from_id", br.from.0)?;
            d.set_item("to_id", br.to.0)?;
            d.set_item("r", br.r)?;
            d.set_item("x", br.x)?;
            let charging = br.calc_terminal_charging();
            d.set_item("b", br.calc_total_charging_b())?;
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
            d.set_item("uid", br.uid.as_deref())?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn switches<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner().switches().len());
        for sw in self.inner().switches() {
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
            d.set_item("uid", sw.uid.as_deref())?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn generators<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner().generators().len());
        for g in self.inner().generators() {
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
            // The MATPOWER gen columns past PMIN, in column order, `None` where
            // the source carried no value. A list, not a name-keyed dict: the
            // consumer that wants them (the ppc bridge) wants the column order,
            // and an absent value is what separates "no ramp limit" from "a
            // ramp limit of zero".
            d.set_item("caps", g.caps.to_vec())?;
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
            d.set_item("uid", g.uid.as_deref())?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    /// Complete storage rows using the balanced model's own serialized field
    /// names. Returning copies keeps the native network immutable.
    #[getter]
    fn storage<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        balanced_field_to_py(py, self.inner(), "storage")
    }

    /// Complete HVDC rows using the balanced model's own serialized field
    /// names. Returning copies keeps the native network immutable.
    #[getter]
    fn hvdc<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        balanced_field_to_py(py, self.inner(), "hvdc")
    }

    /// Complete three winding transformer rows using the balanced model's
    /// own serialized field names.
    #[getter]
    fn transformers_3w<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        balanced_field_to_py(py, self.inner(), "transformers_3w")
    }

    /// Complete area rows using the balanced model's own serialized field
    /// names.
    #[getter]
    fn areas<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        balanced_field_to_py(py, self.inner(), "areas")
    }

    fn connectivity_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.calc_connectivity_report(py)
    }

    fn calc_connectivity_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let r = IndexedNetwork::with_core(self.inner(), &self.core).calc_connectivity_report();
        let d = PyDict::new(py);
        d.set_item("n_buses", r.n_buses)?;
        d.set_item("n_branches_in_service", r.n_branches_in_service)?;
        d.set_item("n_components", r.n_components)?;
        d.set_item("isolated_buses", r.isolated_buses)?;
        Ok(d)
    }

    /// Serialize this case to MATPOWER `.m` text. For a MATPOWER-parsed case this
    /// is the byte-exact source echo, written through the module.
    fn to_matpower(&self) -> PyResult<String> {
        let destination =
            powerio_core::Destination::memory("case").map_err(|error| core_error_pyerr(&error))?;
        let result = powerio_tx::emit(&self.module, TargetFormat::Matpower, destination)
            .map_err(|error| core_error_pyerr(&error))?;
        emitted_text(result, "matpower").map(|(text, _)| text)
    }

    /// Serialize this case to the JSON transport.
    fn to_json(&self) -> PyResult<String> {
        self.inner().to_json().map_err(core_pyerr)
    }

    /// Serialize this case to another format. Returns `(text, warnings)`.
    #[pyo3(signature = (to, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
    fn to_format(
        &self,
        to: &str,
        missing_gen_cost: Option<&str>,
        default_gen_cost: Option<&str>,
        gen_cost_csv: Option<&str>,
    ) -> PyResult<(String, Vec<PyDiagnostic>)> {
        let target = to.parse::<TargetFormat>().map_err(core_pyerr)?;
        let opts = emit_options(
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
            MissingGenCostPolicy::Preserve,
        )?;
        let destination =
            powerio_core::Destination::memory("case").map_err(|error| core_error_pyerr(&error))?;
        let result = powerio_tx::emit_with_options(&self.module, target, &opts, destination)
            .map_err(|error| core_error_pyerr(&error))?;
        let (text, diagnostics) = emitted_text(result, to)?;
        Ok((text, diagnostics.iter().map(PyDiagnostic::from).collect()))
    }

    /// Serialize this case to `to`, bypassing source echo for the same
    /// format. Returns `(text, warnings)`.
    fn to_canonical_format(&self, to: &str) -> PyResult<(String, Vec<PyDiagnostic>)> {
        let target = to.parse::<TargetFormat>().map_err(core_pyerr)?;
        let module = powerio_core::PioModule::new(self.inner().clone());
        let destination =
            powerio_core::Destination::memory("case").map_err(|error| core_error_pyerr(&error))?;
        let result = powerio_tx::emit(&module, target, destination)
            .map_err(|error| core_error_pyerr(&error))?;
        let (text, diagnostics) = emitted_text(result, to)?;
        Ok((text, diagnostics.iter().map(PyDiagnostic::from).collect()))
    }

    /// Serialize this case to `to` and write it to `path` exactly as
    /// produced. Returns the fidelity warnings. Prefer this over writing
    /// `to_format` text through `open(path, "w")`: Python's text mode
    /// translates newlines on Windows, and a case whose retained source has
    /// CRLF endings comes out with doubled carriage returns, which PSS/E
    /// family tools reject.
    #[pyo3(signature = (path, to, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
    fn write_file(
        &self,
        path: &str,
        to: &str,
        missing_gen_cost: Option<&str>,
        default_gen_cost: Option<&str>,
        gen_cost_csv: Option<&str>,
    ) -> PyResult<Vec<PyDiagnostic>> {
        let target = to.parse::<TargetFormat>().map_err(core_pyerr)?;
        let options = emit_options(
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
            MissingGenCostPolicy::Preserve,
        )?;
        let result = powerio_tx::emit_with_options(
            &self.module,
            target,
            &options,
            powerio_core::Destination::path(path),
        )
        .map_err(|error| core_error_pyerr(&error))?;
        Ok(py_diagnostics(&result))
    }

    /// A normalized, computation-ready copy of this case: per unit, radians,
    /// out-of-service filtered, densely reindexed (1-based), bus types
    /// canonicalized. The raw case is unchanged; the result carries no retained
    /// source, so writing it serializes the per-unit model rather than echoing.
    fn to_normalized(&self) -> PyResult<PyBalancedNetwork> {
        let normalized = self.inner().to_normalized().map_err(core_pyerr)?;
        Ok(case_from_parts(normalized, self.diagnostics().to_vec()))
    }

    #[pyo3(signature = (*, clamp_angle_bounds=false, angle_bound_pad=None))]
    fn to_normalized_with_options(
        &self,
        clamp_angle_bounds: bool,
        angle_bound_pad: Option<f64>,
    ) -> PyResult<PyBalancedNetwork> {
        let options = NormalizeOptions {
            clamp_angle_bounds,
            angle_bound_pad: angle_bound_pad.unwrap_or(POWER_MODELS_ANGLE_BOUND_PAD),
        };
        let normalized = self
            .inner()
            .to_normalized_with_options(&options)
            .map_err(core_pyerr)?;
        let mut diagnostics = self.diagnostics().to_vec();
        diagnostics.extend(normalized.diagnostics);
        Ok(case_from_parts(normalized.network, diagnostics))
    }

    // --- matrix calculations: each returns a COO tuple -----------------

    /// MATPOWER FDPF Bp matrix. `skip_zero_impedance=False` refuses a zero
    /// impedance branch (`r` and `x` both zero); pass `True` to drop it
    /// instead.
    #[pyo3(signature = (scheme=None, *, skip_zero_impedance=false))]
    fn bprime<'py>(
        &self,
        py: Python<'py>,
        scheme: Option<&str>,
        skip_zero_impedance: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = BuildOptions {
            scheme: parse_scheme(scheme.unwrap_or("bx"))?,
            skip_zero_impedance,
            ..BuildOptions::default()
        };
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = calc_bprime_matrix(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    /// Calculate the PowerModels branch by bus incidence matrix.
    #[pyo3(signature = (formula="series_susceptance"))]
    fn calc_incidence_matrix<'py>(
        &self,
        py: Python<'py>,
        formula: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        coo_triplets(py, &self.dc_operators(formula)?.calc_incidence_matrix())
    }

    /// Calculate `Bf = diag(b) A`, branches by buses.
    #[pyo3(signature = (formula="series_susceptance"))]
    fn calc_branch_susceptance_matrix<'py>(
        &self,
        py: Python<'py>,
        formula: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        coo_triplets(
            py,
            &self.dc_operators(formula)?.calc_branch_susceptance_matrix(),
        )
    }

    /// Calculate `B = A' diag(b) A`, buses by buses.
    #[pyo3(signature = (formula="series_susceptance"))]
    fn calc_bus_susceptance_matrix<'py>(
        &self,
        py: Python<'py>,
        formula: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        coo_triplets(
            py,
            &self.dc_operators(formula)?.calc_bus_susceptance_matrix(),
        )
    }

    /// Calculate `A' (b .* shift)` in bus order.
    #[pyo3(signature = (formula="series_susceptance"))]
    fn calc_phase_shift_injection(&self, formula: &str) -> PyResult<Vec<f64>> {
        Ok(self.dc_operators(formula)?.calc_phase_shift_injection())
    }

    /// Calculate `-Bf * va + b .* shift` in active branch order.
    #[pyo3(signature = (voltage_angles, formula="series_susceptance"))]
    fn calc_branch_flow_dc(&self, voltage_angles: Vec<f64>, formula: &str) -> PyResult<Vec<f64>> {
        self.dc_operators(formula)?
            .calc_branch_flow_dc(&voltage_angles)
            .map_err(|error| core_error_pyerr(&error))
    }

    /// Calculate `-B * va + p_shift` in bus order.
    #[pyo3(signature = (voltage_angles, formula="series_susceptance"))]
    fn calc_bus_injection_dc(&self, voltage_angles: Vec<f64>, formula: &str) -> PyResult<Vec<f64>> {
        self.dc_operators(formula)?
            .calc_bus_injection_dc(&voltage_angles)
            .map_err(|error| core_error_pyerr(&error))
    }

    /// MATPOWER FDPF Bpp matrix. `skip_zero_impedance` as in `bprime`.
    #[pyo3(signature = (scheme=None, *, skip_zero_impedance=false))]

    fn bdoubleprime<'py>(
        &self,
        py: Python<'py>,
        scheme: Option<&str>,
        skip_zero_impedance: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = BuildOptions {
            scheme: parse_scheme(scheme.unwrap_or("bx"))?,
            skip_zero_impedance,
            ..BuildOptions::default()
        };
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = calc_bdoubleprime_matrix(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    /// `skip_zero_impedance` as in `bprime`.
    #[pyo3(signature = (*, include_taps=true, include_shifts=true, skip_zero_impedance=false))]
    fn lacpf<'py>(
        &self,
        py: Python<'py>,
        include_taps: bool,
        include_shifts: bool,
        skip_zero_impedance: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_options(
            Scheme::Bx,
            include_taps,
            include_shifts,
            skip_zero_impedance,
        );
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = calc_lacpf_matrix(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    fn adjacency<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = calc_adjacency_matrix(&view).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    /// `(Re(Y_bus), Im(Y_bus))` as two COO tuples. `skip_zero_impedance` as
    /// in `bprime`.
    #[pyo3(signature = (*, include_taps=true, include_shifts=true, skip_zero_impedance=false))]
    fn ybus_parts<'py>(
        &self,
        py: Python<'py>,
        include_taps: bool,
        include_shifts: bool,
        skip_zero_impedance: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_options(
            Scheme::Bx,
            include_taps,
            include_shifts,
            skip_zero_impedance,
        );
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let yb = calc_admittance_matrix(&view, &opts).map_err(to_pyerr)?;
        let g = coo_triplets(py, &yb.g)?;
        let b = coo_triplets(py, &yb.b)?;
        Ok((g, b).into_pyobject(py)?.into_any())
    }

    #[pyo3(signature = (formula=None, solver=None))]
    fn ptdf<'py>(
        &self,
        py: Python<'py>,
        formula: Option<&str>,
        solver: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = sensitivity_options(formula, solver)?;
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = calc_ptdf_lodf_with_options(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m.ptdf)
    }

    #[pyo3(signature = (formula=None, solver=None))]
    fn lodf<'py>(
        &self,
        py: Python<'py>,
        formula: Option<&str>,
        solver: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = sensitivity_options(formula, solver)?;
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = calc_ptdf_lodf_with_options(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m.lodf)
    }

    /// Weighted Laplacian `L = -B` for the chosen formula.
    #[pyo3(signature = (formula=None))]
    fn weighted_laplacian<'py>(
        &self,
        py: Python<'py>,
        formula: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut l = self
            .dc_operators(formula.unwrap_or("series_susceptance"))?
            .calc_bus_susceptance_matrix();
        l.data_mut().iter_mut().for_each(|value| *value = -*value);
        coo_triplets(py, &l)
    }

    /// This network's coordinates as the canonical GeoJSON layer. Raises when
    /// the network carries none.
    fn geo_layer_json(&self) -> PyResult<String> {
        self.to_geo_layer_json()
    }

    /// Transform this network's coordinates to the canonical GeoJSON layer.
    fn to_geo_layer_json(&self) -> PyResult<String> {
        Ok(self.inner().to_geo_layer().to_geojson())
    }

    /// Apply a geographic sidecar (any form `parse_geo` accepts) and return
    /// `(placed_network, report)`; this network is unchanged, matching
    /// `to_normalized` and the distribution equivalent. The placed copy drops
    /// the retained source text, so a same-format write re-serializes.
    #[pyo3(signature = (text, name_hint=None))]
    fn apply_geo_layer<'py>(
        &self,
        py: Python<'py>,
        text: &str,
        name_hint: Option<&str>,
    ) -> PyResult<(PyBalancedNetwork, Bound<'py, PyDict>)> {
        let parsed = powerio::GeoLayer::parse_text(text, name_hint)
            .map_err(|error| PowerIOParseError::new_err(error.to_string()))?;
        let mut inner = self.inner().clone();
        let report = inner.apply_geo_layer(&parsed.layer);
        let mut diagnostics = self.diagnostics().to_vec();
        diagnostics.extend(parsed.diagnostics);
        // Locations never move buses, so the cached index stays valid.
        let core = self.core.clone();
        let _ = core;
        Ok((
            case_from_parts(inner, diagnostics),
            geo_report_dict(py, &report)?,
        ))
    }

    /// Write the DC OPF bundle into `out_dir/<case>_dcopf/`. Returns
    /// `{"dir": str, "files": [str, ...]}`.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (out_dir, formula=None, units=None, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
    fn write_dcopf_bundle<'py>(
        &self,
        py: Python<'py>,
        out_dir: &str,
        formula: Option<&str>,
        units: Option<&str>,
        missing_gen_cost: Option<&str>,
        default_gen_cost: Option<&str>,
        gen_cost_csv: Option<&str>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let cost_opts = emit_options(
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
            MissingGenCostPolicy::Require,
        )?;
        let mut policy_network = self.inner().clone();
        let cost_report = policy_network
            .apply_gen_cost_policy(&cost_opts.gen_cost_patches, cost_opts.missing_gen_cost)
            .map_err(core_pyerr)?;
        let instance = powerio_prob::DcOpfInstance::from_network(policy_network)
            .map_err(|error| core_error_pyerr(&error))?
            .with_branch_susceptance_formula(parse_formula(
                formula.unwrap_or("series_susceptance"),
            )?);
        let mut assembly = DcOpfAssemblyOptions::default();
        assembly.units = parse_units(units.unwrap_or("perunit"))?;
        let options = DcOpfBundleOptions {
            assembly,
            metadata: DcOpfBundleMetadata {
                cost_policy: cost_opts.missing_gen_cost,
                cost_report,
            },
        };
        let outputs = write_bundle(&instance, out_dir, &options).map_err(to_pyerr)?;
        dir_files_dict(py, &outputs.dir, &outputs.files)
    }

    /// Write the gridfm-datakit Parquet dataset for this case under
    /// `out_dir/<case>/raw/`. Returns
    /// `{"dir", "files", "dropped_zero_impedance", "degenerate_cost_gens"}`.
    /// Available when the extension is built with the Rust `gridfm` feature.
    #[cfg(feature = "gridfm")]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (out_dir, *, scenario=0, include_y_bus=true, include_taps=true, include_shifts=true, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
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
        let cost_opts = emit_options(
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
            gridfm_write_dataset(self.inner(), scenario, out_dir, &opts).map_err(to_pyerr)?;
        gridfm_outputs_to_dict(py, &outputs)
    }

    fn __repr__(&self) -> String {
        format!(
            "BalancedNetwork(name={:?}, n_buses={}, n_branches={}, n_generators={})",
            self.inner().name(),
            self.inner().buses().len(),
            self.inner().branches().len(),
            self.inner().generators().len()
        )
    }
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
        powerio_tx::parse_display_file(std::path::Path::new(path), from_).map_err(core_pyerr)?;
    display_data_to_py(py, display)
}

/// Rebuild a case from JSON produced by `BalancedNetwork.to_json()`.
#[pyfunction]
fn from_json(text: &str) -> PyResult<PyBalancedNetwork> {
    let inner = powerio::BalancedNetwork::from_json(text).map_err(core_pyerr)?;
    Ok(case_from_parts(inner, Vec::new()))
}

/// Universal one-file conversion over the dynamic module dispatcher. Parse
/// findings precede writer findings unless the writer returned the exact
/// retained source bytes, which is the faithful echo tier.
fn emit_dynamic(
    module: &powerio_core::PioModule<powerio::PioValue>,
    to: &str,
    options: &EmitOptions,
    destination: powerio_core::Destination,
) -> PyResult<powerio_core::EmitResult> {
    if let powerio::PioValue::BalancedNetwork(network) = module.value()
        && let Ok(target) = to.parse::<TargetFormat>()
    {
        let typed = module_with_records(module, network.clone())?;
        return powerio_tx::emit_with_options(&typed, target, options, destination)
            .map_err(|error| core_error_pyerr(&error));
    }
    powerio::emit(module, to, destination).map_err(|error| core_error_pyerr(&error))
}

fn convert_module_text(
    module: &powerio_core::PioModule<powerio::PioValue>,
    to: &str,
    options: &EmitOptions,
) -> PyResult<(String, Vec<powerio_core::Diagnostic>)> {
    let destination =
        powerio_core::Destination::memory("case").map_err(|error| core_error_pyerr(&error))?;
    let result = emit_dynamic(module, to, options, destination)?;
    let (text, writer_diagnostics) = emitted_text(result, to)?;
    let echoed = module.source().is_some_and(|source| {
        source
            .primary_buffer()
            .is_ok_and(|buffer| buffer.bytes() == text.as_bytes())
    });
    if echoed {
        return Ok((text, writer_diagnostics));
    }
    let mut diagnostics = module.diagnostics().to_vec();
    diagnostics.extend(writer_diagnostics);
    Ok((text, diagnostics))
}

/// Convert a case file through the universal module model. Returns
/// `(text, warnings)`: the converted file text and the list of fidelity warnings
/// (fields the target couldn't represent). The input format is the file
/// extension unless `from` overrides it. `out` writes the text to a file
/// exactly as produced — prefer it over `open(out, "w").write(text)`, whose
/// text mode newline translation on Windows doubles the carriage returns of
/// a CRLF source echo into `\r\r\n`, which PSS/E family tools reject.
#[pyfunction]
#[pyo3(signature = (path, to, from_=None, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None, out=None))]
fn convert_file(
    path: &str,
    to: &str,
    from_: Option<&str>,
    missing_gen_cost: Option<&str>,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&str>,
    out: Option<&str>,
) -> PyResult<(String, Vec<PyDiagnostic>)> {
    let opts = emit_options(
        missing_gen_cost,
        default_gen_cost,
        gen_cost_csv,
        MissingGenCostPolicy::Preserve,
    )?;
    let path = std::path::Path::new(path);
    let mut source =
        powerio_core::Source::open(path).map_err(|error| core_open_pyerr(path, &error))?;
    if let Some(from) = from_ {
        let format = powerio_core::FormatId::new(from.to_ascii_lowercase().replace('_', "-"))
            .map_err(|error| core_error_pyerr(&error))?;
        source = source.with_format(format);
    }
    let module = powerio::parse(source).map_err(|error| core_open_pyerr(path, &error))?;
    let (text, diagnostics) = convert_module_text(&module, to, &opts)?;
    if let Some(out) = out {
        emit_dynamic(&module, to, &opts, powerio_core::Destination::path(out))?;
    }
    let rendered = diagnostics.iter().map(PyDiagnostic::from).collect();
    Ok((text, rendered))
}

/// Convert in-memory case `text` through the universal module model,
/// with no file staging. Returns `(text, warnings)` like `convert_file`.
/// `from_` names the input format (default `matpower`).
#[pyfunction]
#[pyo3(signature = (text, to, from_=None, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
fn convert_str(
    text: &str,
    to: &str,
    from_: Option<&str>,
    missing_gen_cost: Option<&str>,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&str>,
) -> PyResult<(String, Vec<PyDiagnostic>)> {
    let opts = emit_options(
        missing_gen_cost,
        default_gen_cost,
        gen_cost_csv,
        MissingGenCostPolicy::Preserve,
    )?;
    let format = powerio_core::FormatId::new(
        from_
            .unwrap_or("matpower")
            .to_ascii_lowercase()
            .replace('_', "-"),
    )
    .map_err(|error| core_error_pyerr(&error))?;
    let source = powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())
        .map_err(|error| core_error_pyerr(&error))?
        .with_format(format);
    let module = powerio::parse(source).map_err(|error| core_error_pyerr(&error))?;
    let (text, diagnostics) = convert_module_text(&module, to, &opts)?;
    let rendered = diagnostics.iter().map(PyDiagnostic::from).collect();
    Ok((text, rendered))
}

fn dist_to_pyerr(e: powerio_dist::Error) -> PyErr {
    use powerio_dist::Error as E;
    let msg = e.to_string();
    let code = e.code().code;
    match e {
        // OSError(errno, strerror, filename) lets CPython pick the precise
        // subclass (FileNotFoundError etc.) while keeping the path on
        // e.filename, which a bare io::Error conversion would drop.
        E::Io { path, source } => match source.raw_os_error() {
            Some(errno) => pyo3::exceptions::PyOSError::new_err((errno, source.to_string(), path)),
            None => with_code(PowerIOError::new_err(msg), code),
        },
        E::UnknownFormat(_) => PyValueError::new_err(msg),
        E::Json { .. } => with_code(PowerIOParseError::new_err(msg), code),
        _ => with_code(PowerIOError::new_err(msg), code),
    }
}

/// Low-level handle around a parsed multiconductor distribution network in
/// wire coordinates (OpenDSS, PMD ENGINEERING JSON, BMOPF JSON). The
/// user-facing `powerio.dist.MulticonductorNetwork` wraps it.
#[pyclass(name = "_MulticonductorNetwork", module = "powerio._powerio", frozen)]
struct PyMulticonductorNetwork {
    /// The parsed module: the typed network plus retained source and the
    /// reader's findings. Same format writes echo the retained bytes; a
    /// handle built from a bare network writes canonically.
    module: powerio_core::PioModule<powerio_dist::MulticonductorNetwork>,
}

impl PyMulticonductorNetwork {
    fn inner(&self) -> &powerio_dist::MulticonductorNetwork {
        self.module.value()
    }

    fn from_module(module: powerio_core::PioModule<powerio_dist::MulticonductorNetwork>) -> Self {
        Self { module }
    }

    /// A derived handle: no retained source, so writes are canonical.
    fn from_network(net: powerio_dist::MulticonductorNetwork) -> Self {
        Self {
            module: powerio_core::PioModule::new(net),
        }
    }
}

/// A distribution source of the declared `from_` format (when given).
fn dist_source_from_path(
    path: &std::path::Path,
    from: Option<&str>,
    include_root: Option<&str>,
) -> PyResult<powerio_core::Source> {
    let mut source =
        powerio_core::Source::open(path).map_err(|error| core_open_pyerr(path, &error))?;
    if let Some(root) = include_root {
        source = source
            .with_acquisition_root(root)
            .map_err(|error| core_error_pyerr(&error))?;
    }
    if let Some(token) = from {
        source = source.with_format(
            powerio_core::FormatId::new(token.to_ascii_lowercase().replace('_', "-"))
                .map_err(|error| core_error_pyerr(&error))?,
        );
    }
    Ok(source)
}

fn dist_source_from_bytes(bytes: &[u8], from: &str) -> PyResult<powerio_core::Source> {
    Ok(powerio_core::Source::from_bytes("<memory>", bytes.to_vec())
        .map_err(|error| core_error_pyerr(&error))?
        .with_format(
            powerio_core::FormatId::new(from.to_ascii_lowercase().replace('_', "-"))
                .map_err(|error| core_error_pyerr(&error))?,
        ))
}

#[pymethods]
impl PyMulticonductorNetwork {
    fn name(&self) -> Option<&str> {
        self.inner().name().as_deref()
    }

    /// Format the case was parsed from (`dss`, `pmd-json`, `bmopf-json`).
    fn source_format(&self) -> Option<&'static str> {
        self.inner().source_format().map(|f| f.name())
    }

    /// System base frequency in hertz.
    fn base_frequency(&self) -> f64 {
        self.inner().base_frequency()
    }

    /// This network's coordinates as the canonical GeoJSON layer. Raises when
    /// the network carries none.
    fn geo_layer_json(&self) -> PyResult<String> {
        self.to_geo_layer_json()
    }

    /// Transform this network's coordinates to the canonical GeoJSON layer.
    fn to_geo_layer_json(&self) -> PyResult<String> {
        Ok(powerio::dist_geo::to_dist_geo_layer(self.inner()).to_geojson())
    }

    /// Apply a geographic sidecar (any form `parse_geo` accepts) and return
    /// `(placed_network, report)`; this network is unchanged. The placed copy
    /// drops the retained source text, so a same-format write re-serializes.
    #[pyo3(signature = (text, name_hint=None))]
    fn apply_geo_layer<'py>(
        &self,
        py: Python<'py>,
        text: &str,
        name_hint: Option<&str>,
    ) -> PyResult<(PyMulticonductorNetwork, Bound<'py, PyDict>)> {
        let parsed = powerio::GeoLayer::parse_text(text, name_hint)
            .map_err(|error| PowerIOParseError::new_err(error.to_string()))?;
        let mut net = self.inner().clone();
        let report = powerio::dist_geo::apply_dist_geo_layer(&mut net, &parsed.layer);
        *net.source_format_mut() = None;
        Ok((
            PyMulticonductorNetwork::from_network(net),
            geo_report_dict(py, &report)?,
        ))
    }

    fn n_buses(&self) -> usize {
        self.inner().buses().len()
    }

    fn n_lines(&self) -> usize {
        self.inner().lines().len()
    }

    fn n_line_codes(&self) -> usize {
        self.inner().linecodes().len()
    }

    fn n_switches(&self) -> usize {
        self.inner().switches().len()
    }

    fn n_transformers(&self) -> usize {
        self.inner().transformers().len()
    }

    fn n_loads(&self) -> usize {
        self.inner().loads().len()
    }

    fn n_generators(&self) -> usize {
        self.inner().generators().len()
    }

    fn n_ibrs(&self) -> usize {
        self.inner().ibrs().len()
    }

    fn n_control_profiles(&self) -> usize {
        self.inner().control_profiles().len()
    }

    fn n_shunts(&self) -> usize {
        self.inner().shunts().len()
    }

    fn n_capacitors(&self) -> usize {
        self.inner().capacitors().len()
    }

    fn n_sources(&self) -> usize {
        self.inner().sources().len()
    }

    /// Preferred power system spelling; `n_sources` remains compatible.
    fn n_voltage_sources(&self) -> usize {
        self.inner().sources().len()
    }

    fn n_untyped_objects(&self) -> usize {
        self.inner().untyped().len()
    }

    // --- complete read-only tables ------------------------------------

    fn buses<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "buses")
    }

    fn line_codes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "linecodes")
    }

    fn lines<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "lines")
    }

    fn switches<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "switches")
    }

    fn transformers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "transformers")
    }

    fn loads<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "loads")
    }

    fn generators<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "generators")
    }

    fn ibrs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "ibrs")
    }

    fn control_profiles<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "control_profiles")
    }

    fn shunts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "shunts")
    }

    fn capacitors<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "capacitors")
    }

    fn voltage_sources<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "sources")
    }

    fn untyped_objects<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        dist_field_to_py(py, self.inner(), "untyped")
    }

    /// Serialize to `to` (`dss`, `pmd-json`, `bmopf-json`). Returns
    /// `(text, warnings)`. Writing back to the source format echoes the
    /// retained source byte for byte.
    fn to_format(&self, to: &str) -> PyResult<(String, Vec<PyDiagnostic>)> {
        let target = to
            .parse::<powerio_dist::DistTargetFormat>()
            .map_err(dist_to_pyerr)?;
        let destination =
            powerio_core::Destination::memory("case").map_err(|error| core_error_pyerr(&error))?;
        let result = powerio_dist::emit(&self.module, target, destination)
            .map_err(|error| core_error_pyerr(&error))?;
        let (text, diagnostics) = emitted_text(result, to)?;
        Ok((text, diagnostics.iter().map(PyDiagnostic::from).collect()))
    }

    /// Serialize to `to`, bypassing source echo for the same format.
    fn to_canonical_format(&self, to: &str) -> PyResult<(String, Vec<PyDiagnostic>)> {
        let target = to
            .parse::<powerio_dist::DistTargetFormat>()
            .map_err(dist_to_pyerr)?;
        let module = powerio_core::PioModule::new(self.inner().clone());
        let destination =
            powerio_core::Destination::memory("case").map_err(|error| core_error_pyerr(&error))?;
        let result = powerio_dist::emit(&module, target, destination)
            .map_err(|error| core_error_pyerr(&error))?;
        let (text, diagnostics) = emitted_text(result, to)?;
        Ok((text, diagnostics.iter().map(PyDiagnostic::from).collect()))
    }

    /// Serialize to `to` and write it to `path` exactly as produced (no
    /// newline translation; see `BalancedNetwork.write_file`). Returns the fidelity
    /// warnings.
    fn write_file(&self, path: &str, to: &str) -> PyResult<Vec<PyDiagnostic>> {
        let target = to
            .parse::<powerio_dist::DistTargetFormat>()
            .map_err(dist_to_pyerr)?;
        let result =
            powerio_dist::emit(&self.module, target, powerio_core::Destination::path(path))
                .map_err(|error| core_error_pyerr(&error))?;
        Ok(py_diagnostics(&result))
    }

    /// The collapsed bus and terminal graph projection as JSON.
    fn graph_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner().to_graph())
            .map_err(|e| PowerIOError::new_err(format!("serializing graph JSON: {e}")))
    }

    fn __repr__(&self) -> String {
        format!(
            "MulticonductorNetwork(n_buses={}, n_lines={}, n_transformers={}, n_loads={})",
            self.inner().buses().len(),
            self.inner().lines().len(),
            self.inner().transformers().len(),
            self.inner().loads().len()
        )
    }
}

/// Parse a distribution case file. The format comes from `from_` when given,
/// else from the file itself (`.dss`, or `.json` sniffed for the PMD
/// ENGINEERING `data_model` key against the BMOPF layout). `include_root`
/// widens dss include confinement from the case directory to the given
/// directory; the case file must sit under it.
#[pyfunction]
#[pyo3(signature = (path, from_=None, include_root=None))]
fn dist_parse_file(
    path: &str,
    from_: Option<&str>,
    include_root: Option<&str>,
) -> PyResult<PyMulticonductorNetwork> {
    let source = dist_source_from_path(std::path::Path::new(path), from_, include_root)?;
    powerio_dist::parse(source)
        .map(PyMulticonductorNetwork::from_module)
        .map_err(|error| core_error_pyerr(&error))
}

/// Parse an in-memory distribution case of the named source format `from_`
/// (`dss`, `pmd-json`, `bmopf-json`).
#[pyfunction]
#[pyo3(signature = (text, from_))]
fn dist_parse_str(text: &str, from_: &str) -> PyResult<PyMulticonductorNetwork> {
    let source = dist_source_from_bytes(text.as_bytes(), from_)?;
    powerio_dist::parse(source)
        .map(PyMulticonductorNetwork::from_module)
        .map_err(|error| core_error_pyerr(&error))
}

fn convert_dist_module_text(
    module: &powerio_core::PioModule<powerio_dist::MulticonductorNetwork>,
    target: powerio_dist::DistTargetFormat,
) -> PyResult<(String, Vec<powerio_core::Diagnostic>)> {
    let destination =
        powerio_core::Destination::memory("case").map_err(|error| core_error_pyerr(&error))?;
    let result = powerio_dist::emit(module, target, destination)
        .map_err(|error| core_error_pyerr(&error))?;
    let (text, writer_diagnostics) = emitted_text(result, target.name())?;
    let echoed = module.source().is_some_and(|source| {
        source
            .primary_buffer()
            .is_ok_and(|buffer| buffer.bytes() == text.as_bytes())
    });
    if echoed {
        return Ok((text, writer_diagnostics));
    }
    let mut diagnostics = module.diagnostics().to_vec();
    diagnostics.extend(writer_diagnostics);
    Ok((text, diagnostics))
}

/// Convert a distribution case file to `to`. Returns `(text, warnings)`; the
/// warnings carry both the parse warnings and the writer's fidelity losses.
#[pyfunction]
#[pyo3(signature = (path, to, from_=None))]
fn dist_convert_file(
    path: &str,
    to: &str,
    from_: Option<&str>,
) -> PyResult<(String, Vec<PyDiagnostic>)> {
    let to = to
        .parse::<powerio_dist::DistTargetFormat>()
        .map_err(dist_to_pyerr)?;
    let source = dist_source_from_path(std::path::Path::new(path), from_, None)?;
    let module = powerio_dist::parse(source).map_err(|error| core_error_pyerr(&error))?;
    let (text, diagnostics) = convert_dist_module_text(&module, to)?;
    Ok((text, diagnostics.iter().map(PyDiagnostic::from).collect()))
}

/// Convert an in-memory distribution case of the named source format `from_`
/// to `to`. Returns `(text, warnings)`; the warnings carry both the parse
/// warnings and the writer's fidelity losses.
#[pyfunction]
#[pyo3(signature = (text, to, from_))]
fn dist_convert_str(text: &str, to: &str, from_: &str) -> PyResult<(String, Vec<PyDiagnostic>)> {
    let to = to
        .parse::<powerio_dist::DistTargetFormat>()
        .map_err(dist_to_pyerr)?;
    let source = dist_source_from_bytes(text.as_bytes(), from_)?;
    let module = powerio_dist::parse(source).map_err(|error| core_error_pyerr(&error))?;
    let (text, diagnostics) = convert_dist_module_text(&module, to)?;
    Ok((text, diagnostics.iter().map(PyDiagnostic::from).collect()))
}

/// Convert a `serde_json::Value` to the equivalent Python object: an object
/// becomes `dict`, an array becomes `list`, a number becomes `int` when it
/// carries no fractional part and fits `i64`/`u64` and `float` otherwise, and
/// the rest map onto `bool`/`str`/`None` directly. Used for a diagnostic's
/// free form `details` map, the one place this binding hands back arbitrary
/// JSON rather than a fixed shape.
fn json_value_to_py<'py>(
    py: Python<'py>,
    value: &serde_json::Value,
) -> PyResult<Bound<'py, PyAny>> {
    use serde_json::Value as J;
    Ok(match value {
        J::Null => py.None().into_bound(py),
        J::Bool(b) => b.into_pyobject(py)?.to_owned().into_any(),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_pyobject(py)?.into_any()
            } else if let Some(u) = n.as_u64() {
                u.into_pyobject(py)?.into_any()
            } else {
                n.as_f64().unwrap_or(f64::NAN).into_pyobject(py)?.into_any()
            }
        }
        J::String(s) => s.into_pyobject(py)?.into_any(),
        J::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_value_to_py(py, item)?);
            }
            PyList::new(py, out)?.into_any()
        }
        J::Object(map) => json_map_to_py(py, map)?.into_any(),
    })
}

/// Return one serialized model field. The concrete helpers below keep the
/// serde trait out of this crate's public dependency list.
fn serialized_value_field_to_py<'py>(
    py: Python<'py>,
    value: serde_json::Value,
    field: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let field_value = value.get(field).ok_or_else(|| {
        PowerIOError::new_err(format!(
            "the native model serialization has no {field:?} field"
        ))
    })?;
    json_value_to_py(py, field_value)
}

/// Return one balanced model field through that model's own serde surface.
fn balanced_field_to_py<'py>(
    py: Python<'py>,
    model: &BalancedNetwork,
    field: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let value = serde_json::to_value(model).map_err(serialize_pyerr)?;
    serialized_value_field_to_py(py, value, field)
}

/// Return one multiconductor model field through that model's own serde
/// surface.
fn dist_field_to_py<'py>(
    py: Python<'py>,
    model: &powerio_dist::MulticonductorNetwork,
    field: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let value = serde_json::to_value(model).map_err(serialize_pyerr)?;
    // These tables omit their JSON key when empty. The Python table surface
    // still returns an empty list so every declared table is always readable.
    if value.get(field).is_none() && matches!(field, "ibrs" | "control_profiles" | "capacitors") {
        return Ok(PyList::empty(py).into_any());
    }
    serialized_value_field_to_py(py, value, field)
}

/// [`json_value_to_py`] for a JSON object, kept separate so a diagnostic's
/// `details` getter can return `dict` directly instead of the wider `Any`.
fn json_map_to_py<'py>(
    py: Python<'py>,
    map: &serde_json::Map<String, serde_json::Value>,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (key, value) in map {
        dict.set_item(key, json_value_to_py(py, value)?)?;
    }
    Ok(dict)
}

/// One source byte range a diagnostic points at: the Python mirror of
/// `powerio_core::SourceSpan`. `source` is the source ID string, not the
/// bytes themselves; a caller resolves it against the owning module's
/// sources.
#[pyclass(
    name = "SourceSpan",
    module = "powerio._powerio",
    frozen,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
struct PySourceSpan {
    source: String,
    byte_start: u64,
    byte_end: u64,
}

#[pymethods]
impl PySourceSpan {
    #[getter]
    fn source(&self) -> &str {
        &self.source
    }

    #[getter]
    fn byte_start(&self) -> u64 {
        self.byte_start
    }

    #[getter]
    fn byte_end(&self) -> u64 {
        self.byte_end
    }

    fn __repr__(&self) -> String {
        format!(
            "SourceSpan(source={:?}, byte_start={}, byte_end={})",
            self.source, self.byte_start, self.byte_end
        )
    }
}

impl From<&powerio_core::SourceSpan> for PySourceSpan {
    fn from(span: &powerio_core::SourceSpan) -> Self {
        Self {
            source: span.source().as_str().to_owned(),
            byte_start: span.byte_start(),
            byte_end: span.byte_end(),
        }
    }
}

/// One coded, user facing finding from a parse, read, transform, or write
/// pass: the Python mirror of `powerio_core::Diagnostic`. Every module
/// carries a list of these; `PioModule.diagnostics()` returns them natively
/// instead of the `diagnostics_json` string form.
#[pyclass(name = "Diagnostic", module = "powerio._powerio", frozen, eq)]
#[derive(PartialEq)]
struct PyDiagnostic {
    code: String,
    severity: String,
    message: String,
    id: Option<String>,
    target: Option<String>,
    suggested_action: Option<String>,
    related: Vec<String>,
    spans: Vec<PySourceSpan>,
    details: serde_json::Map<String, serde_json::Value>,
}

#[pymethods]
impl PyDiagnostic {
    #[getter]
    fn code(&self) -> &str {
        &self.code
    }

    /// `"error"`, `"warning"`, `"remark"`, or `"note"`.
    #[getter]
    fn severity(&self) -> &str {
        &self.severity
    }

    #[getter]
    fn message(&self) -> &str {
        &self.message
    }

    #[getter]
    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    #[getter]
    fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    #[getter]
    fn suggested_action(&self) -> Option<&str> {
        self.suggested_action.as_deref()
    }

    #[getter]
    fn related(&self) -> Vec<String> {
        self.related.clone()
    }

    #[getter]
    fn spans(&self) -> Vec<PySourceSpan> {
        self.spans.clone()
    }

    /// Free form structured detail, or `None` when the finding carries none.
    #[getter]
    fn details<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        if self.details.is_empty() {
            return Ok(None);
        }
        Ok(Some(json_map_to_py(py, &self.details)?))
    }

    fn __repr__(&self) -> String {
        format!(
            "Diagnostic(code={:?}, severity={:?}, message={:?})",
            self.code, self.severity, self.message
        )
    }

    fn __str__(&self) -> String {
        format!(
            "{} {}: {}",
            self.severity.to_ascii_uppercase(),
            self.code,
            self.message
        )
    }
}

impl From<&powerio_core::Diagnostic> for PyDiagnostic {
    fn from(diagnostic: &powerio_core::Diagnostic) -> Self {
        Self {
            code: diagnostic.code().to_owned(),
            severity: diagnostic.severity().as_str().to_owned(),
            message: diagnostic.message().to_owned(),
            id: diagnostic.id().map(|id| id.as_str().to_owned()),
            target: diagnostic.target().map(str::to_owned),
            suggested_action: diagnostic.suggested_action().map(str::to_owned),
            related: diagnostic
                .related()
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            spans: diagnostic.spans().iter().map(PySourceSpan::from).collect(),
            details: diagnostic.details().clone(),
        }
    }
}

/// Version and schema identity of this build: the release API discovery
/// document. Keys agree with the C `pio_schema_versions_json` report where
/// both apply; the wheel embeds the Rust core directly, so there is no C ABI
/// integer here.
#[pyfunction]
fn versions_json() -> PyResult<String> {
    let doc = serde_json::json!({
        powerio::version::VERSION_KEY: powerio::VERSION,
        "bmopf_schema": powerio_dist_bmopf_schema(),
        "module_schema": {
            "name": powerio::stored::SCHEMA_NAME,
            "version": powerio::stored::SCHEMA_VERSION,
        },
    });
    serde_json::to_string(&doc).map_err(serialize_pyerr)
}

fn powerio_dist_bmopf_schema() -> &'static str {
    powerio_dist::BMOPF_SCHEMA_VERSION
}

#[pyclass(name = "_PioModule", module = "powerio._powerio")]
struct PyPioModule {
    module: Option<powerio_core::PioModule<powerio::PioValue>>,
}

/// Rebuild a typed module around one value with every common record and the
/// retained source from another module. Sources are added first because
/// source map and diagnostic spans validate against them.
fn module_with_records<S, T>(
    module: &powerio_core::PioModule<S>,
    value: T,
) -> PyResult<powerio_core::PioModule<T>> {
    let mut out = powerio_core::PioModule::new(value).with_producer(module.producer().clone());
    for descriptor in module.sources() {
        out.add_source_descriptor(descriptor.clone())
            .map_err(|error| core_error_pyerr(&error))?;
    }
    for entry in module.source_map() {
        out.add_source_map_entry(entry.clone())
            .map_err(|error| core_error_pyerr(&error))?;
    }
    for diagnostic in module.diagnostics() {
        out.add_diagnostic(diagnostic.clone())
            .map_err(|error| core_error_pyerr(&error))?;
    }
    for entry in module.history() {
        out.add_history_entry(entry.clone())
            .map_err(|error| core_error_pyerr(&error))?;
    }
    for (namespace, value) in module.extensions() {
        out.insert_extension(namespace.clone(), value.clone())
            .map_err(|error| core_error_pyerr(&error))?;
    }
    Ok(match module.source() {
        Some(source) => out.with_source(source.clone()),
        None => out,
    })
}

impl PyPioModule {
    fn module(&self) -> PyResult<&powerio_core::PioModule<powerio::PioValue>> {
        self.module
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("the module handle was consumed"))
    }

    fn selector<'a>(
        time_position: Option<usize>,
        scenario: Option<&'a str>,
    ) -> PyResult<powerio::select::StateSelector<'a>> {
        match (time_position, scenario) {
            (Some(position), None) => Ok(powerio::select::StateSelector::TimePosition(position)),
            (None, Some(id)) => Ok(powerio::select::StateSelector::Scenario(id)),
            _ => Err(PyValueError::new_err(
                "pass exactly one of time_position and scenario",
            )),
        }
    }

    fn value_summary(value: &powerio::PioValue) -> serde_json::Value {
        use powerio::PioValue as V;
        match value {
            V::BalancedNetwork(network) => serde_json::json!({
                "buses": network.buses().len(),
                "branches": network.branches().len(),
                "generators": network.generators().len(),
                "loads": network.loads().len(),
            }),
            V::MulticonductorNetwork(network) => serde_json::json!({
                "buses": network.buses().len(),
                "lines": network.lines().len(),
                "transformers": network.transformers().len(),
                "switches": network.switches().len(),
                "loads": network.loads().len(),
            }),
            V::BalancedNetworkTimeSeries(series) => serde_json::json!({
                "points": series.len(),
            }),
            V::BalancedOperatingPointTimeSeries(series) => serde_json::json!({
                "points": series.len(),
            }),
            V::BalancedNetworkScenarioSet(set) => serde_json::json!({
                "scenarios": set.len(),
            }),
            _ => serde_json::json!({}),
        }
    }

    fn operations(value: &powerio::PioValue) -> Vec<&'static str> {
        use powerio::PioValue as V;
        match value {
            V::BalancedNetwork(_) => vec!["inspect", "diagnostics", "emit"],
            V::MulticonductorNetwork(_) => vec![
                "inspect",
                "diagnostics",
                "emit",
                "to_balanced_report",
                "to_balanced",
            ],
            V::BalancedNetworkTimeSeries(_)
            | V::BalancedOperatingPointTimeSeries(_)
            | V::BalancedNetworkScenarioSet(_) => vec![
                "inspect",
                "diagnostics",
                "emit",
                "list_states",
                "inspect_state",
                "export_state",
            ],
            _ => vec!["inspect", "diagnostics", "emit"],
        }
    }

    /// Infer the no-argument write target without guessing between JSON
    /// families. Prefer an explicit `.pio.json` destination, then the durable
    /// source descriptor recorded by the parser, then a network's source
    /// format, and finally an unambiguous destination extension.
    fn inferred_write_format(&self, path: &Path) -> PyResult<String> {
        let path_text = path.to_string_lossy().to_ascii_lowercase();
        if path_text.ends_with(".pio.json") {
            return Ok("pio-json".to_owned());
        }

        let module = self.module()?;
        if let Some(format) = module.sources().iter().find_map(|source| source.format()) {
            return Ok(format.as_str().to_owned());
        }

        match module.value() {
            powerio::PioValue::BalancedNetwork(network) => {
                let format = network.source_format().name();
                if parse_target_format(format).is_some() || format == "pypsa-csv" {
                    return Ok(format.to_owned());
                }
            }
            powerio::PioValue::MulticonductorNetwork(network) => {
                if let Some(format) = network.source_format().map(|format| format.name())
                    && powerio_dist::parse_dist_target_format(format).is_some()
                {
                    return Ok(format.to_owned());
                }
            }
            _ => {}
        }

        let inferred = match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("m") => Some("matpower"),
            Some("raw") => Some("psse"),
            Some("aux") => Some("powerworld"),
            Some("epc") => Some("pslf"),
            Some("dss") => Some("dss"),
            _ => None,
        };
        inferred.map(str::to_owned).ok_or_else(|| {
            PyValueError::new_err(
                "could not infer a write format; pass format= explicitly (JSON case formats are ambiguous)",
            )
        })
    }
}

#[pymethods]
impl PyPioModule {
    /// Wrap an existing balanced network value in a module without serializing
    /// or reparsing it. Common records and retained source remain attached.
    #[staticmethod]
    fn from_balanced_network(network: &PyBalancedNetwork) -> PyResult<Self> {
        module_with_records(
            &network.module,
            powerio::PioValue::BalancedNetwork(network.inner().clone()),
        )
        .map(|module| Self {
            module: Some(module),
        })
    }

    /// Wrap an existing multiconductor network value in a module without
    /// serializing or reparsing it. Common records and retained source remain
    /// attached.
    #[staticmethod]
    fn from_multiconductor_network(network: &PyMulticonductorNetwork) -> PyResult<Self> {
        module_with_records(
            &network.module,
            powerio::PioValue::MulticonductorNetwork(network.inner().clone()),
        )
        .map(|module| Self {
            module: Some(module),
        })
    }

    /// Parse a case file into a module of whichever family claims it.
    /// `include_root` widens the acquisition root for formats whose includes
    /// reference sibling files.
    #[staticmethod]
    #[pyo3(signature = (path, from_=None, include_root=None))]
    fn from_file(path: &str, from_: Option<&str>, include_root: Option<&str>) -> PyResult<Self> {
        let mut source = powerio_core::Source::open(Path::new(path))
            .map_err(|error| core_open_pyerr(Path::new(path), &error))?;
        if let Some(root) = include_root {
            source = source
                .with_acquisition_root(root)
                .map_err(|error| core_error_pyerr(&error))?;
        }
        if let Some(name) = from_ {
            let format =
                powerio_core::FormatId::new(name).map_err(|error| core_error_pyerr(&error))?;
            source = source.with_format(format);
        }
        powerio::parse(source)
            .map(|module| Self {
                module: Some(module),
            })
            .map_err(|error| core_error_pyerr(&error))
    }

    /// Parse in-memory case text into a module.
    #[staticmethod]
    #[pyo3(signature = (text, from_=None))]
    fn from_str(text: &str, from_: Option<&str>) -> PyResult<Self> {
        let mut source = powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())
            .map_err(|error| core_error_pyerr(&error))?;
        if let Some(name) = from_ {
            let format =
                powerio_core::FormatId::new(name).map_err(|error| core_error_pyerr(&error))?;
            source = source.with_format(format);
        }
        powerio::parse(source)
            .map(|module| Self {
                module: Some(module),
            })
            .map_err(|error| core_error_pyerr(&error))
    }

    /// Parse in-memory case bytes into a module. The only in-memory way to
    /// read a binary format; text formats must be UTF-8. `name` identifies
    /// the buffer for diagnostics and extension-based format detection;
    /// defaults to `<memory>` when not given.
    #[staticmethod]
    #[pyo3(signature = (data, from_=None, name=None))]
    fn from_bytes(data: &[u8], from_: Option<&str>, name: Option<&str>) -> PyResult<Self> {
        let mut source =
            powerio_core::Source::from_bytes(name.unwrap_or("<memory>"), data.to_vec())
                .map_err(|error| core_error_pyerr(&error))?;
        if let Some(format_name) = from_ {
            let format = powerio_core::FormatId::new(format_name)
                .map_err(|error| core_error_pyerr(&error))?;
            source = source.with_format(format);
        }
        powerio::parse(source)
            .map(|module| Self {
                module: Some(module),
            })
            .map_err(|error| core_error_pyerr(&error))
    }

    /// Serialize this module's typed value to `to`. The dynamic writer routes
    /// to the balanced, multiconductor, or stored module family. Returns
    /// `(text, diagnostics)`.
    #[pyo3(signature = (to, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
    fn to_format(
        &self,
        to: &str,
        missing_gen_cost: Option<&str>,
        default_gen_cost: Option<&str>,
        gen_cost_csv: Option<&str>,
    ) -> PyResult<(String, Vec<PyDiagnostic>)> {
        let options = emit_options(
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
            MissingGenCostPolicy::Preserve,
        )?;
        let destination =
            powerio_core::Destination::memory("case").map_err(|error| core_error_pyerr(&error))?;
        let result = emit_dynamic(self.module()?, to, &options, destination)?;
        let (text, diagnostics) = emitted_text(result, to)?;
        Ok((text, diagnostics.iter().map(PyDiagnostic::from).collect()))
    }

    /// Write the complete artifact inventory to `path`. With no `format`,
    /// retain the module's recorded source format when possible, or infer an
    /// unambiguous format from `path`.
    #[pyo3(signature = (path, format=None))]
    fn write_file(&self, path: &str, format: Option<&str>) -> PyResult<Vec<PyDiagnostic>> {
        let path = Path::new(path);
        let inferred;
        let format = match format {
            Some(format) => format,
            None => {
                inferred = self.inferred_write_format(path)?;
                &inferred
            }
        };
        let result = powerio::emit(
            self.module()?,
            format,
            powerio_core::Destination::path(path),
        )
        .map_err(|error| core_error_pyerr(&error))?;
        Ok(result
            .diagnostics()
            .iter()
            .map(PyDiagnostic::from)
            .collect())
    }

    /// The value's permanent kind identifier.
    fn kind(&self) -> PyResult<String> {
        Ok(self.module()?.value().kind().as_str().to_owned())
    }

    /// Value inspection and supported operation discovery, as JSON.
    fn inspect_json(&self) -> PyResult<String> {
        let module = self.module()?;
        let value = module.value();
        let payload = serde_json::json!({
            "kind": value.kind().as_str(),
            "value": Self::value_summary(value),
            "records": {
                "sources": module.sources().len(),
                "source_map": module.source_map().len(),
                "diagnostics": module.diagnostics().len(),
                "history": module.history().len(),
                "extensions": module.extensions().len(),
            },
            "operations": Self::operations(value),
        });
        serde_json::to_string(&payload).map_err(serialize_pyerr)
    }

    /// The module's diagnostics as a JSON array.
    fn diagnostics_json(&self) -> PyResult<String> {
        let diagnostics = diagnostics_json_array(self.module()?.diagnostics());
        serde_json::to_string(&diagnostics).map_err(serialize_pyerr)
    }

    /// The module's diagnostics as native `Diagnostic` objects, in encounter
    /// order. `diagnostics_json` above stays as the explicit serialization
    /// helper for a caller that wants the wire form directly.
    fn diagnostics(&self) -> PyResult<Vec<PyDiagnostic>> {
        Ok(self
            .module()?
            .diagnostics()
            .iter()
            .map(PyDiagnostic::from)
            .collect())
    }

    /// The typed time or scenario inventory, as JSON.
    fn list_states_json(&self) -> PyResult<String> {
        let inventory = powerio::select::list_states(self.module()?.value())
            .map_err(|error| core_error_pyerr(&error))?;
        let payload = match inventory {
            powerio::select::StateInventory::TimePoints(points) => serde_json::json!({
                "keyed_by": "time_position",
                "time_points": points
                    .iter()
                    .map(|point| {
                        serde_json::json!({
                            "position": point.position,
                            "label": point.label,
                            "duration_seconds": point.duration.map(|d| d.as_secs_f64()),
                        })
                    })
                    .collect::<Vec<_>>(),
            }),
            powerio::select::StateInventory::Scenarios(scenarios) => serde_json::json!({
                "keyed_by": "scenario",
                "scenarios": scenarios
                    .iter()
                    .map(|scenario| {
                        serde_json::json!({
                            "id": scenario.id,
                            "probability": scenario.probability,
                        })
                    })
                    .collect::<Vec<_>>(),
            }),
            _ => serde_json::json!({}),
        };
        serde_json::to_string(&payload).map_err(serialize_pyerr)
    }

    /// Select the existing typed item and describe it, as JSON. No clone, no
    /// materialization: the export operation is separate.
    #[pyo3(signature = (time_position=None, scenario=None))]
    fn inspect_state_json(
        &self,
        time_position: Option<usize>,
        scenario: Option<&str>,
    ) -> PyResult<String> {
        let selector = Self::selector(time_position, scenario)?;
        let selected = powerio::select::select_state(self.module()?.value(), selector)
            .map_err(|error| core_error_pyerr(&error))?;
        let payload = match selected {
            powerio::select::SelectedState::BalancedNetwork(network) => serde_json::json!({
                "item": "balanced_network",
                "buses": network.buses().len(),
                "branches": network.branches().len(),
                "generators": network.generators().len(),
                "loads": network.loads().len(),
            }),
            powerio::select::SelectedState::BalancedOperatingPoint(point) => {
                let stated: Vec<&str> = powerio_prob::BALANCED_STATE_QUANTITIES
                    .iter()
                    .copied()
                    .filter(|quantity| point.states(quantity))
                    .collect();
                serde_json::json!({
                    "item": "balanced_operating_point",
                    "stated_quantities": stated,
                    "network_buses": point.network().buses().len(),
                })
            }
            _ => serde_json::json!({}),
        };
        serde_json::to_string(&payload).map_err(serialize_pyerr)
    }

    /// Export the selected item as an independent static module handle.
    #[pyo3(signature = (time_position=None, scenario=None))]
    fn export_state(&self, time_position: Option<usize>, scenario: Option<&str>) -> PyResult<Self> {
        let selector = Self::selector(time_position, scenario)?;
        powerio::select::export_module_state(self.module()?, selector)
            .map(|module| Self {
                module: Some(module),
            })
            .map_err(|error| core_error_pyerr(&error))
    }

    /// Readiness of the multiconductor value for the balanced lowering, as
    /// JSON: the inspect half of the transformation.
    #[pyo3(signature = (base_mva=100.0))]
    fn lowering_readiness_json(&self, base_mva: f64) -> PyResult<String> {
        let readiness = powerio::transform::to_balanced_report(
            self.module()?,
            powerio::transform::MulticonductorToBalancedOptions {
                base_mva,
                ..Default::default()
            },
        )
        .map_err(|error| core_error_pyerr(&error))?;
        let diagnostics = diagnostics_json_array(&readiness.diagnostics);
        let payload = serde_json::json!({
            "convention": readiness.convention,
            "base_mva": readiness.base_mva,
            "ready": readiness.is_ready(),
            "assumptions": readiness.assumptions,
            "approximations": readiness.approximations,
            "diagnostics": diagnostics,
        });
        serde_json::to_string(&payload).map_err(serialize_pyerr)
    }

    /// Lower the multiconductor value to a balanced module. Common records
    /// carry over, retained source is severed after the kind-changing pass,
    /// and the pass appends its findings and one Transform history entry. On
    /// refusal the handle keeps its module and
    /// the raised `PowerIODataError` carries the refusal's diagnostic code as
    /// `.code` and its structured findings as `.diagnostics` (a list of
    /// dicts with `code`/`severity`/`message`/`target`).
    #[pyo3(signature = (base_mva=100.0))]
    fn lower_to_balanced(&self, base_mva: f64) -> PyResult<Self> {
        let source = self.module()?;
        let powerio::PioValue::MulticonductorNetwork(network) = source.value() else {
            let Err(error) = powerio::transform::to_balanced_report(
                source,
                powerio::transform::MulticonductorToBalancedOptions {
                    base_mva,
                    ..Default::default()
                },
            ) else {
                unreachable!("a non-multiconductor value cannot pass the kind check")
            };
            return Err(core_error_pyerr(&error));
        };
        // The transform owns its input. Give it a record-complete sibling so
        // both success and refusal leave the caller's module usable.
        let module = module_with_records(
            source,
            powerio::PioValue::MulticonductorNetwork(network.clone()),
        )?;
        match powerio::transform::to_balanced(
            module,
            powerio::transform::MulticonductorToBalancedOptions {
                base_mva,
                ..Default::default()
            },
        ) {
            Ok(lowered) => Ok(Self {
                module: Some(lowered),
            }),
            Err((_module, error)) => {
                let err = PowerIODataError::new_err(error.to_string());
                let code = error.diagnostics.first().map(|d| d.code().to_owned());
                let diagnostics: Vec<serde_json::Value> = error
                    .diagnostics
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "code": d.code(),
                            "severity": d.severity().as_str(),
                            "message": d.message(),
                            "target": d.target(),
                        })
                    })
                    .collect();
                Python::attach(|py| -> PyResult<()> {
                    if let Some(code) = &code {
                        err.value(py).setattr("code", code)?;
                    }
                    let list = json_value_to_py(py, &serde_json::Value::Array(diagnostics))?;
                    err.value(py).setattr("diagnostics", list)?;
                    Ok(())
                })?;
                Err(err)
            }
        }
    }

    /// The balanced network value as a network handle (cheap table share).
    fn as_balanced_network(&self) -> PyResult<PyBalancedNetwork> {
        let module = self.module()?;
        let powerio::PioValue::BalancedNetwork(network) = module.value() else {
            return Err(PowerIODataError::new_err(format!(
                "the module carries a {} value; as_balanced_network takes a balanced network",
                module.value().kind().as_str()
            )));
        };
        Ok(case_from_module(module_with_records(
            module,
            network.clone(),
        )?))
    }

    /// The multiconductor network value as a network handle.
    fn as_multiconductor_network(&self) -> PyResult<PyMulticonductorNetwork> {
        let module = self.module()?;
        let powerio::PioValue::MulticonductorNetwork(network) = module.value() else {
            return Err(PowerIODataError::new_err(format!(
                "the module carries a {} value; as_multiconductor_network takes a \
                 multiconductor network",
                module.value().kind().as_str()
            )));
        };
        Ok(PyMulticonductorNetwork::from_module(module_with_records(
            module,
            network.clone(),
        )?))
    }

    fn __repr__(&self) -> String {
        match &self.module {
            Some(module) => format!(
                "PioModule(kind={}, diagnostics={}, history={})",
                module.value().kind().as_str(),
                module.diagnostics().len(),
                module.history().len()
            ),
            None => "PioModule(<consumed>)".to_owned(),
        }
    }
}

/// Classify top level JSON markers. Returns `(status, domain, format)`.
///
/// `status` is `known` for a case document, or the classification family
/// itself for the outcomes that are not one: `package` (a `.pio.json`
/// package), `model-json` (bare balanced model JSON, read with
/// `powerio.from_json`), `ambiguous`, or `unknown`. `domain` is
/// `transmission` or `distribution`, and both it and `format` are set only
/// when `status` is `known`. `json_classes()` returns the closed set of
/// families.
#[pyfunction]
fn classify_json_text(text: &str) -> (String, Option<String>, Option<String>) {
    let class = classify_balanced_json_text(text);
    match class {
        JsonClass::Case(Detection::Known(format)) => (
            "known".into(),
            Some(class.family().into()),
            Some(format.name().into()),
        ),
        _ => (class.family().into(), None, None),
    }
}

/// The closed set of JSON classification families, in the spelling every
/// powerio surface uses. A new family appends to it; a spelling never changes.
#[pyfunction]
fn json_classes() -> Vec<String> {
    powerio_tx::JSON_CLASSES
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// Resolve a format alias to `(token, extension, is_directory, can_emit)`.
#[pyfunction]
fn resolve_format(name: &str) -> Option<(String, Option<String>, bool, bool)> {
    powerio::resolve_format(name).map(|info| {
        (
            info.token.to_owned(),
            info.extension.map(str::to_owned),
            info.is_directory,
            info.can_emit,
        )
    })
}

/// Tolerant read of a geographic sidecar (headerless buscoords CSV, aliased
/// CSV/JSON records, GeoJSON): returns `{"geojson": <canonical form>,
/// "diagnostics": [...]}`. `name_hint` (a file name) picks CSV against JSON;
/// otherwise the content is sniffed.
#[pyfunction(signature = (text, name_hint = None))]
fn parse_geo<'py>(
    py: Python<'py>,
    text: &str,
    name_hint: Option<&str>,
) -> PyResult<Bound<'py, PyDict>> {
    let parsed = powerio::GeoLayer::parse_text(text, name_hint)
        .map_err(|error| PowerIOParseError::new_err(error.to_string()))?;
    let out = PyDict::new(py);
    out.set_item("geojson", parsed.layer.to_geojson())?;
    let diagnostics: Vec<PyDiagnostic> =
        parsed.diagnostics.iter().map(PyDiagnostic::from).collect();
    out.set_item("diagnostics", diagnostics)?;
    Ok(out)
}

/// A `{matched_buses, matched_branches, unmatched_features, unlocated_buses,
/// unlocated_branches, notes}` dict from one geo apply pass.
fn geo_report_dict<'py>(
    py: Python<'py>,
    report: &powerio::GeoApplyReport,
) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item("matched_buses", report.matched_buses)?;
    out.set_item("matched_branches", report.matched_branches)?;
    out.set_item("unmatched_features", report.unmatched_features)?;
    out.set_item("unlocated_buses", report.unlocated_buses)?;
    out.set_item("unlocated_branches", report.unlocated_branches)?;
    out.set_item("notes", report.notes.clone())?;
    Ok(out)
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

/// Write a batch of cases as one gridfm-datakit dataset, row stacked and keyed by
/// the `scenario` column. The k-th case is stamped `base_scenario + k`; all cases
/// must share one base element set (same bus/branch/gen counts and bus-id order).
/// Available when the extension is built with the Rust `gridfm` feature.
#[cfg(feature = "gridfm")]
#[allow(clippy::too_many_arguments)]
#[pyfunction]
#[pyo3(signature = (cases, out_dir, *, base_scenario=0, include_y_bus=true, include_taps=true, include_shifts=true, missing_gen_cost=None, default_gen_cost=None, gen_cost_csv=None))]
fn write_gridfm_batch<'py>(
    py: Python<'py>,
    cases: Vec<PyRef<'py, PyBalancedNetwork>>,
    out_dir: &str,
    base_scenario: i64,
    include_y_bus: bool,
    include_taps: bool,
    include_shifts: bool,
    missing_gen_cost: Option<&str>,
    default_gen_cost: Option<&str>,
    gen_cost_csv: Option<&str>,
) -> PyResult<Bound<'py, PyDict>> {
    let cost_opts = emit_options(
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
    let net_refs: Vec<_> = cases.iter().map(|c| c.inner()).collect();
    let snapshots = numbered_snapshots(&net_refs, base_scenario).map_err(to_pyerr)?;
    let outputs = gridfm_write_batch(&snapshots, out_dir, &opts).map_err(to_pyerr)?;
    gridfm_outputs_to_dict(py, &outputs)
}

#[pymodule]
fn _powerio(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("PowerIOError", m.py().get_type::<PowerIOError>())?;
    m.add("PowerIOParseError", m.py().get_type::<PowerIOParseError>())?;
    m.add("PowerIODataError", m.py().get_type::<PowerIODataError>())?;
    m.add_class::<PyBalancedNetwork>()?;
    m.add_function(wrap_pyfunction!(parse_display_file, m)?)?;
    m.add_function(wrap_pyfunction!(from_json, m)?)?;
    m.add_function(wrap_pyfunction!(convert_file, m)?)?;
    m.add_function(wrap_pyfunction!(convert_str, m)?)?;
    m.add_class::<PyMulticonductorNetwork>()?;
    m.add_function(wrap_pyfunction!(dist_parse_file, m)?)?;
    m.add_function(wrap_pyfunction!(dist_parse_str, m)?)?;
    m.add_function(wrap_pyfunction!(dist_convert_file, m)?)?;
    m.add_function(wrap_pyfunction!(dist_convert_str, m)?)?;
    m.add_class::<PyPioModule>()?;
    m.add_class::<PyDiagnostic>()?;
    m.add_class::<PySourceSpan>()?;
    m.add_function(wrap_pyfunction!(versions_json, m)?)?;
    m.add_function(wrap_pyfunction!(classify_json_text, m)?)?;
    m.add_function(wrap_pyfunction!(json_classes, m)?)?;
    m.add_function(wrap_pyfunction!(resolve_format, m)?)?;
    m.add_function(wrap_pyfunction!(parse_geo, m)?)?;
    // Whether the gridfm Parquet surface (arrow/parquet) was compiled in, so the
    // pure-Python layer can raise an ImportError instead of an AttributeError.
    m.add("_has_gridfm", cfg!(feature = "gridfm"))?;
    #[cfg(feature = "gridfm")]
    m.add_function(wrap_pyfunction!(write_gridfm_batch, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::module_with_records;

    #[test]
    fn typed_module_copy_keeps_every_common_record_and_retained_source() {
        use powerio_core::{
            Diagnostic, DiagnosticCode, DiagnosticSeverity, HistoryEntry, HistoryId, HistoryKind,
            PioModule, Producer, Source, SourceDescriptor, SourceId, SourceMapEntry,
            SourceRelation, SourceSpan,
        };

        let network = powerio::BalancedNetwork::new("copy-test", 100.0);
        let retained = Source::from_bytes("case.m", b"test".to_vec()).unwrap();
        let source_id = SourceId::new("input").unwrap();
        let span = SourceSpan::new(source_id.clone(), 0, 4).unwrap();
        let mut source = PioModule::new(powerio::PioValue::BalancedNetwork(network.clone()))
            .with_producer(Producer::new("binding-test", "1").unwrap())
            .with_source(retained);
        source
            .add_source_descriptor(SourceDescriptor::new(source_id, "case.m", 4).unwrap())
            .unwrap();
        source
            .add_source_map_entry(
                SourceMapEntry::new("/buses", SourceRelation::Exact, vec![span.clone()]).unwrap(),
            )
            .unwrap();
        source
            .add_diagnostic(
                Diagnostic::new(
                    DiagnosticCode::new("PARTNER.TEST.COPY").unwrap(),
                    DiagnosticSeverity::Note,
                    "keep me",
                )
                .with_span(span)
                .unwrap(),
            )
            .unwrap();
        source
            .add_history_entry(
                HistoryEntry::new(HistoryId::new("h1").unwrap(), HistoryKind::Parse, "parse")
                    .unwrap(),
            )
            .unwrap();
        source
            .insert_extension("org.example.test", serde_json::json!({"kept": true}))
            .unwrap();

        let copied = module_with_records(&source, network).unwrap();
        assert_eq!(copied.producer(), source.producer());
        assert_eq!(copied.sources(), source.sources());
        assert_eq!(copied.source_map(), source.source_map());
        assert_eq!(copied.diagnostics().len(), source.diagnostics().len());
        assert_eq!(
            copied.diagnostics()[0].code(),
            source.diagnostics()[0].code()
        );
        assert_eq!(copied.history(), source.history());
        assert_eq!(copied.extensions(), source.extensions());
        assert_eq!(
            copied.source().unwrap().primary_buffer().unwrap().bytes(),
            b"test"
        );
    }
}
