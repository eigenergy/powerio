//! PyO3 extension behind the `powerio` Python package.
//!
//! The extension exposes parsing, emission, serialization, transformations,
//! matrices, modules, and calculation values. Base operations do not import
//! NumPy or SciPy.
//!
//! The matrix methods hand back COO triplets as plain Python lists
//! (`data`, `row`, `col`, `shape`); there is no NumPy at this layer. The
//! Python `powerio` package assembles those into `scipy.sparse` matrices and
//! NetworkX graphs when the corresponding extra is installed.
//!
//! Indices narrow to `i32` to match SciPy's default index width.
//! `coo_triplets` checks the bound before conversion.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};
use sprs::CsMat;

use powerio::{BalancedNetwork, BranchSusceptanceFormula, PwdDisplay};
use powerio_matrix::DcOperators;
use powerio_matrix::matrix::{
    BuildOptions, Scheme, SensitivityOptions, SensitivitySolver, calc_adjacency_matrix,
    calc_admittance_matrix, calc_bdoubleprime_matrix, calc_bprime_matrix, calc_lacpf_matrix,
    calc_ptdf_lodf_with_options,
};
use powerio_tx::{
    Detection, IndexCore, IndexedNetwork, JsonClass, NormalizeOptions,
    POWER_MODELS_ANGLE_BOUND_PAD, classify_json_text as classify_balanced_json_text,
};

pyo3::create_exception!(
    powerio,
    PowerIOError,
    pyo3::exceptions::PyValueError,
    "Base error raised by the powerio parser, emitter, or matrix calculations.\n\n\
     Subclasses `ValueError`: every failure it covers is a statement about a \
     value the caller supplied. I/O failures do not reach it; they raise the \
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

/// Convert one core emission result to the private dictionary consumed by the
/// Python wrapper. Memory artifacts carry bytes; committed artifacts carry
/// their filesystem path. The public wrapper turns these records into
/// `Artifact` and `EmitResult` values.
fn emit_result_to_py<'py>(
    py: Python<'py>,
    result: powerio_core::EmitResult,
) -> PyResult<Bound<'py, PyDict>> {
    let layout = match result.layout() {
        powerio_core::OutputLayout::File => "file",
        powerio_core::OutputLayout::Directory => "directory",
    };
    let fidelity = match result.fidelity() {
        powerio_core::Fidelity::ExactSameFormat => "exact_same_format",
        powerio_core::Fidelity::Canonical => "canonical",
    };
    let diagnostics: Vec<_> = result
        .diagnostics()
        .iter()
        .map(PyDiagnostic::from)
        .collect();
    let artifacts = PyList::empty(py);
    match result.into_output() {
        powerio_core::EmittedOutput::Memory {
            artifacts: memory_artifacts,
        } => {
            for artifact in memory_artifacts {
                let record = PyDict::new(py);
                record.set_item("name", artifact.name().as_str())?;
                record.set_item("data", PyBytes::new(py, artifact.bytes()))?;
                record.set_item("path", py.None())?;
                artifacts.append(record)?;
            }
        }
        powerio_core::EmittedOutput::Path {
            root,
            artifacts: paths,
        } => {
            for path in paths {
                let name = if layout == "directory" {
                    path.strip_prefix(&root)
                        .ok()
                        .and_then(std::path::Path::to_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| path.to_string_lossy().into_owned())
                } else {
                    path.file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| path.to_string_lossy().into_owned())
                };
                let record = PyDict::new(py);
                record.set_item("name", name)?;
                record.set_item("data", py.None())?;
                record.set_item("path", path_to_str(&path)?)?;
                artifacts.append(record)?;
            }
        }
        _ => {
            return Err(PowerIOError::new_err(
                "this powerio build returned an unsupported output type",
            ));
        }
    }
    let output = PyDict::new(py);
    output.set_item("artifacts", artifacts)?;
    output.set_item("layout", layout)?;
    output.set_item("fidelity", fidelity)?;
    output.set_item("diagnostics", diagnostics)?;
    Ok(output)
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

/// Normalize an API spelling for case insensitive matching.
fn normalize(s: &str) -> String {
    s.to_ascii_lowercase().replace(['-', '_'], "")
}

/// Return a sparse matrix as a `(data, row, col, (nrows, ncols))` tuple of
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
        &self.module.diagnostics
    }

    fn dc_operators(&self, formula: &str) -> PyResult<DcOperators> {
        let formula = parse_formula(formula)?;
        let instance = powerio_prob::DcPfInstance::from_network(self.inner().clone())
            .map_err(|error| core_error_pyerr(&error))?
            .with_branch_susceptance_formula(formula);
        DcOperators::build(&instance).map_err(|error| core_error_pyerr(&error))
    }
}

/// The parse options a binding builds from its optional `format` argument.
fn parse_options(format: Option<&str>) -> PyResult<powerio::ParseOptions> {
    let mut options = powerio::ParseOptions::default();
    if let Some(format) = format {
        options = options
            .format(format)
            .map_err(|error| core_error_pyerr(&error))?;
    }
    Ok(options)
}

fn core_error_pyerr(error: &powerio_core::Error) -> PyErr {
    // Parse and Data map onto their subclasses so callers see one error
    // taxonomy whichever Rust layer raised the failure.
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
/// the precise `OSError` subclass with the path attached.
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

fn active_power_control_to_py<'py>(
    py: Python<'py>,
    control: Option<&powerio_tx::ActivePowerControl>,
) -> PyResult<Bound<'py, PyAny>> {
    let Some(control) = control else {
        return Ok(py.None().into_bound(py));
    };
    let value = PyDict::new(py);
    value.set_item("participate", control.participate)?;
    value.set_item("droop_percent", control.droop_percent)?;
    value.set_item("participation_factor", control.participation_factor)?;
    value.set_item(
        "minimum_target_active_power_mw",
        control.minimum_target_active_power_mw,
    )?;
    value.set_item(
        "maximum_target_active_power_mw",
        control.maximum_target_active_power_mw,
    )?;
    Ok(value.into_any())
}

fn generator_energy_source_name(value: powerio_tx::GeneratorEnergySource) -> &'static str {
    match value {
        powerio_tx::GeneratorEnergySource::Hydro => "hydro",
        powerio_tx::GeneratorEnergySource::Nuclear => "nuclear",
        powerio_tx::GeneratorEnergySource::Wind => "wind",
        powerio_tx::GeneratorEnergySource::Thermal => "thermal",
        powerio_tx::GeneratorEnergySource::Solar => "solar",
        powerio_tx::GeneratorEnergySource::Other => "other",
        _ => "unknown",
    }
}

fn terminal_reference_to_py<'py>(
    py: Python<'py>,
    reference: Option<&powerio_tx::TerminalReference>,
) -> PyResult<Bound<'py, PyAny>> {
    let Some(reference) = reference else {
        return Ok(py.None().into_bound(py));
    };
    let equipment = PyDict::new(py);
    equipment.set_item("component_type", reference.equipment.component_type())?;
    equipment.set_item("local_id", reference.equipment.local_id())?;
    let value = PyDict::new(py);
    value.set_item("equipment", equipment)?;
    value.set_item("terminal", reference.terminal)?;
    Ok(value.into_any())
}

fn transformer_control_to_py<'py>(
    py: Python<'py>,
    control: Option<&powerio_tx::TransformerControl>,
) -> PyResult<Bound<'py, PyAny>> {
    let Some(control) = control else {
        return Ok(py.None().into_bound(py));
    };
    let value = PyDict::new(py);
    let mode = match control.mode {
        powerio_tx::TransformerControlMode::Fixed => "fixed",
        powerio_tx::TransformerControlMode::Voltage => "voltage",
        powerio_tx::TransformerControlMode::ReactiveFlow => "reactive_flow",
        powerio_tx::TransformerControlMode::ActiveFlow => "active_flow",
        powerio_tx::TransformerControlMode::DcLineQuantity => "dc_line_quantity",
        powerio_tx::TransformerControlMode::AsymmetricActiveFlow => "asymmetric_active_flow",
        _ => "unknown",
    };
    value.set_item("mode", mode)?;
    value.set_item("enabled", control.enabled)?;
    value.set_item("controlled_bus", control.controlled_bus.map(|bus| bus.0))?;
    value.set_item(
        "controlled_bus_on_winding_side",
        control.controlled_bus_on_winding_side,
    )?;
    value.set_item(
        "regulating_terminal",
        terminal_reference_to_py(py, control.regulating_terminal.as_ref())?,
    )?;
    value.set_item("tap_min", control.tap_min)?;
    value.set_item("tap_max", control.tap_max)?;
    value.set_item("band_min", control.band_min)?;
    value.set_item("band_max", control.band_max)?;
    value.set_item("tap_position_count", control.ntp)?;
    value.set_item("mva_base", control.mva_base)?;
    value.set_item("winding_connection_angle", control.winding_connection_angle)?;
    Ok(value.into_any())
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
    fn n_static_var_compensators(&self) -> usize {
        self.inner().static_var_compensators().len()
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

    /// The exact source neutral hierarchy and connectivity records, or
    /// `None` when the source supplied only the balanced calculation tables.
    /// Python's network tables are immutable dictionary copies, so this uses
    /// the same representation as `hvdc`, `transformers_3w`, and `areas`.
    #[getter]
    fn detailed_connectivity<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let Some(details) = self.inner().detailed_connectivity().as_deref() else {
            return Ok(py.None().into_bound(py));
        };
        let value = serde_json::to_value(details).map_err(serialize_pyerr)?;
        json_value_to_py(py, &value)
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
            d.set_item("section_count", s.section_count)?;
            d.set_item("uid", s.uid.as_deref())?;
            rows.push(d);
        }
        PyList::new(py, rows)
    }

    #[getter]
    fn static_var_compensators<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        balanced_field_to_py(py, self.inner(), "static_var_compensators")
    }

    #[getter]
    fn branches<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let mut rows: Vec<Bound<'py, PyDict>> = Vec::with_capacity(self.inner().branches().len());
        for br in self.inner().branches() {
            let d = PyDict::new(py);
            d.set_item("name", br.name.as_deref())?;
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
            d.set_item(
                "control",
                transformer_control_to_py(py, br.control.as_ref())?,
            )?;
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
            d.set_item(
                "energy_source",
                generator_energy_source_name(g.energy_source),
            )?;
            d.set_item("pg", g.pg)?;
            d.set_item("qg", g.qg)?;
            d.set_item("pmax", g.pmax)?;
            d.set_item("pmin", g.pmin)?;
            d.set_item("qmax", g.qmax)?;
            d.set_item("qmin", g.qmin)?;
            d.set_item("vg", g.vg)?;
            d.set_item("mbase", g.mbase)?;
            d.set_item("in_service", g.in_service)?;
            d.set_item("voltage_regulation_on", g.voltage_regulation_on)?;
            d.set_item("regulated_bus", g.regulated_bus.map(|bus| bus.0))?;
            d.set_item(
                "regulating_terminal",
                terminal_reference_to_py(py, g.regulating_terminal.as_ref())?,
            )?;
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
            d.set_item(
                "active_power_control",
                active_power_control_to_py(py, g.active_power_control.as_ref())?,
            )?;
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

    /// Calculate the per branch susceptances.
    #[pyo3(signature = (formula="series_susceptance"))]
    fn calc_branch_susceptances(&self, formula: &str) -> PyResult<Vec<f64>> {
        Ok(self
            .dc_operators(formula)?
            .calc_branch_susceptances()
            .to_vec())
    }

    /// Calculate `Bf = diag(b) A`, branches by buses.
    #[pyo3(signature = (formula="series_susceptance"))]
    fn calc_branch_flow_matrix<'py>(
        &self,
        py: Python<'py>,
        formula: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        coo_triplets(py, &self.dc_operators(formula)?.calc_branch_flow_matrix())
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

    /// Calculate `b .* shift` in branch order.
    #[pyo3(signature = (formula="series_susceptance"))]
    fn calc_branch_phase_shift_injection(&self, formula: &str) -> PyResult<Vec<f64>> {
        Ok(self
            .dc_operators(formula)?
            .calc_branch_phase_shift_injection())
    }

    /// Calculate `A' (b .* shift)` in bus order.
    #[pyo3(signature = (formula="series_susceptance"))]
    fn calc_bus_phase_shift_injection(&self, formula: &str) -> PyResult<Vec<f64>> {
        Ok(self.dc_operators(formula)?.calc_bus_phase_shift_injection())
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
        let parsed = powerio::GeoLayer::parse(text, name_hint)
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
fn parse_display<'py>(
    py: Python<'py>,
    path: &str,
    from_: Option<&str>,
) -> PyResult<Bound<'py, PyAny>> {
    if let Some(from_) = from_
        && !matches!(
            from_.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
            "pwd" | "powerworldpwd" | "powerworlddisplay"
        )
    {
        return Err(PowerIOError::new_err(format!(
            "unsupported display format: {from_}"
        )));
    }
    let display = powerio_tx::format::powerworld::__parse_pwd_file(std::path::Path::new(path))
        .map_err(core_pyerr)?;
    let payload = pwd_display_to_dict(py, &display)?;
    Ok(("powerworld", payload).into_pyobject(py)?.into_any())
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
        let parsed = powerio::GeoLayer::parse(text, name_hint)
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
/// carries a list of these; `PioModule.diagnostics` returns them natively
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

/// Release and schema identity for this build.
///
/// The wheel embeds the Rust core directly, so there is no C ABI integer.
#[pyfunction]
fn versions_json() -> PyResult<String> {
    let doc = serde_json::json!({
        powerio::version::VERSION_KEY: powerio::VERSION,
        "bmopf_schema": powerio_dist_bmopf_schema(),
        "module_schema": {
            "name": "powerio.module",
            "version": 1,
        },
    });
    serde_json::to_string(&doc).map_err(serialize_pyerr)
}

fn powerio_dist_bmopf_schema() -> &'static str {
    powerio_dist::BMOPF_SCHEMA_VERSION
}

#[pyclass(
    name = "ComponentId",
    module = "powerio._powerio",
    frozen,
    eq,
    hash,
    skip_from_py_object
)]
#[derive(Clone, PartialEq, Eq, Hash)]
struct PyComponentId {
    inner: powerio_core::ComponentId,
}

#[pymethods]
impl PyComponentId {
    #[new]
    fn new(component_type: &str, local_id: &str) -> PyResult<Self> {
        powerio_core::ComponentId::new(component_type, local_id)
            .map(|inner| Self { inner })
            .map_err(|error| core_error_pyerr(&error))
    }

    #[getter]
    fn component_type(&self) -> &str {
        self.inner.component_type()
    }

    #[getter]
    fn local_id(&self) -> &str {
        self.inner.local_id()
    }

    fn __str__(&self) -> String {
        self.inner.to_string()
    }

    fn __repr__(&self) -> String {
        format!(
            "ComponentId(component_type={:?}, local_id={:?})",
            self.inner.component_type(),
            self.inner.local_id()
        )
    }
}

macro_rules! scuc_scalar_record {
    ($py_type:ident, $python_name:literal, $rust_type:path, { $($field:ident: $field_type:ty),+ $(,)? }) => {
        #[pyclass(
            name = $python_name,
            module = "powerio._powerio",
            frozen,
            skip_from_py_object
        )]
        #[derive(Clone)]
        struct $py_type {
            inner: $rust_type,
        }

        #[pymethods]
        impl $py_type {
            $(
                #[getter]
                fn $field(&self) -> $field_type {
                    self.inner.$field
                }
            )+
        }

        impl From<&$rust_type> for $py_type {
            fn from(value: &$rust_type) -> Self {
                Self { inner: value.clone() }
            }
        }
    };
}

scuc_scalar_record!(
    PyScucStartupCostAdjustment,
    "ScucStartupCostAdjustment",
    powerio_prob::ScucStartupCostAdjustment,
    { cost: f64, maximum_down_time: f64 }
);
scuc_scalar_record!(
    PyScucStartupLimit,
    "ScucStartupLimit",
    powerio_prob::ScucStartupLimit,
    { start_time: f64, end_time: f64, maximum_startups: u64 }
);
scuc_scalar_record!(
    PyScucEnergyRequirement,
    "ScucEnergyRequirement",
    powerio_prob::ScucEnergyRequirement,
    { start_time: f64, end_time: f64, energy: f64 }
);
scuc_scalar_record!(
    PyScucInitialCommitment,
    "ScucInitialCommitment",
    powerio_prob::ScucInitialCommitment,
    { accumulated_up_time: f64, accumulated_down_time: f64 }
);
scuc_scalar_record!(
    PyScucRampLimits,
    "ScucRampLimits",
    powerio_prob::ScucRampLimits,
    { up: f64, down: f64, startup: f64, shutdown: f64 }
);
scuc_scalar_record!(
    PyScucReserveLimits,
    "ScucReserveLimits",
    powerio_prob::ScucReserveLimits,
    {
        regulation_up: f64,
        regulation_down: f64,
        synchronized: f64,
        nonsynchronized: f64,
        ramping_up_online: f64,
        ramping_down_online: f64,
        ramping_up_offline: f64,
        ramping_down_offline: f64
    }
);
scuc_scalar_record!(
    PyScucEnergyCostBlock,
    "ScucEnergyCostBlock",
    powerio_prob::ScucEnergyCostBlock,
    { marginal_cost: f64, block_size: f64 }
);
scuc_scalar_record!(
    PyScucReserveCosts,
    "ScucReserveCosts",
    powerio_prob::ScucReserveCosts,
    {
        regulation_up: f64,
        regulation_down: f64,
        synchronized: f64,
        nonsynchronized: f64,
        ramping_up_online: f64,
        ramping_down_online: f64,
        ramping_up_offline: f64,
        ramping_down_offline: f64,
        reactive_up: f64,
        reactive_down: f64
    }
);
scuc_scalar_record!(
    PyScucViolationCosts,
    "ScucViolationCosts",
    powerio_prob::ScucViolationCosts,
    {
        active_power_balance: f64,
        reactive_power_balance: f64,
        branch_thermal_limit: f64,
        energy_requirement: f64
    }
);

#[pyclass(
    name = "ScucReactiveCapability",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucReactiveCapability {
    kind: &'static str,
    reactive_power_at_zero_active_power: Option<f64>,
    reactive_power_at_zero_active_power_min: Option<f64>,
    reactive_power_at_zero_active_power_max: Option<f64>,
    slope: Option<f64>,
    slope_min: Option<f64>,
    slope_max: Option<f64>,
}

#[pymethods]
impl PyScucReactiveCapability {
    #[getter]
    fn kind(&self) -> &'static str {
        self.kind
    }

    #[getter]
    fn reactive_power_at_zero_active_power(&self) -> Option<f64> {
        self.reactive_power_at_zero_active_power
    }

    #[getter]
    fn reactive_power_at_zero_active_power_min(&self) -> Option<f64> {
        self.reactive_power_at_zero_active_power_min
    }

    #[getter]
    fn reactive_power_at_zero_active_power_max(&self) -> Option<f64> {
        self.reactive_power_at_zero_active_power_max
    }

    #[getter]
    fn slope(&self) -> Option<f64> {
        self.slope
    }

    #[getter]
    fn slope_min(&self) -> Option<f64> {
        self.slope_min
    }

    #[getter]
    fn slope_max(&self) -> Option<f64> {
        self.slope_max
    }
}

impl From<&powerio_prob::ScucReactiveCapability> for PyScucReactiveCapability {
    fn from(value: &powerio_prob::ScucReactiveCapability) -> Self {
        use powerio_prob::ScucReactiveCapability::{Bounded, Linear, None};
        match value {
            None => Self {
                kind: "none",
                reactive_power_at_zero_active_power: std::option::Option::None,
                reactive_power_at_zero_active_power_min: std::option::Option::None,
                reactive_power_at_zero_active_power_max: std::option::Option::None,
                slope: std::option::Option::None,
                slope_min: std::option::Option::None,
                slope_max: std::option::Option::None,
            },
            Linear {
                reactive_power_at_zero_active_power,
                slope,
            } => Self {
                kind: "linear",
                reactive_power_at_zero_active_power: Some(*reactive_power_at_zero_active_power),
                reactive_power_at_zero_active_power_min: std::option::Option::None,
                reactive_power_at_zero_active_power_max: std::option::Option::None,
                slope: Some(*slope),
                slope_min: std::option::Option::None,
                slope_max: std::option::Option::None,
            },
            Bounded {
                reactive_power_at_zero_active_power_min,
                reactive_power_at_zero_active_power_max,
                slope_min,
                slope_max,
            } => Self {
                kind: "bounded",
                reactive_power_at_zero_active_power: std::option::Option::None,
                reactive_power_at_zero_active_power_min: Some(
                    *reactive_power_at_zero_active_power_min,
                ),
                reactive_power_at_zero_active_power_max: Some(
                    *reactive_power_at_zero_active_power_max,
                ),
                slope: std::option::Option::None,
                slope_min: Some(*slope_min),
                slope_max: Some(*slope_max),
            },
            _ => unreachable!("unrecognized ScucReactiveCapability value"),
        }
    }
}

#[pyclass(
    name = "ScucDevicePeriod",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucDevicePeriod {
    inner: powerio_prob::ScucDevicePeriod,
}

#[pymethods]
impl PyScucDevicePeriod {
    #[getter]
    fn on_status_min(&self) -> bool {
        self.inner.on_status_min
    }

    #[getter]
    fn on_status_max(&self) -> bool {
        self.inner.on_status_max
    }

    #[getter]
    fn active_power_min(&self) -> f64 {
        self.inner.active_power_min
    }

    #[getter]
    fn active_power_max(&self) -> f64 {
        self.inner.active_power_max
    }

    #[getter]
    fn reactive_power_min(&self) -> f64 {
        self.inner.reactive_power_min
    }

    #[getter]
    fn reactive_power_max(&self) -> f64 {
        self.inner.reactive_power_max
    }

    #[getter]
    fn energy_cost_blocks<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .energy_cost_blocks
                .iter()
                .map(PyScucEnergyCostBlock::from),
        )
    }

    #[getter]
    fn reserve_costs(&self) -> PyScucReserveCosts {
        PyScucReserveCosts::from(&self.inner.reserve_costs)
    }
}

impl From<&powerio_prob::ScucDevicePeriod> for PyScucDevicePeriod {
    fn from(value: &powerio_prob::ScucDevicePeriod) -> Self {
        Self {
            inner: value.clone(),
        }
    }
}

#[pyclass(
    name = "ScucDevice",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucDevice {
    inner: powerio_prob::ScucDevice,
}

#[pymethods]
impl PyScucDevice {
    #[getter]
    fn id(&self) -> PyComponentId {
        PyComponentId {
            inner: self.inner.id.clone(),
        }
    }

    #[getter]
    fn kind(&self) -> &'static str {
        match self.inner.kind {
            powerio_prob::ScucDeviceKind::Producer => "producer",
            powerio_prob::ScucDeviceKind::Consumer => "consumer",
            _ => unreachable!("unrecognized ScucDeviceKind value"),
        }
    }

    #[getter]
    fn on_cost(&self) -> f64 {
        self.inner.on_cost
    }

    #[getter]
    fn startup_cost(&self) -> f64 {
        self.inner.startup_cost
    }

    #[getter]
    fn startup_cost_adjustments<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .startup_cost_adjustments
                .iter()
                .map(PyScucStartupCostAdjustment::from),
        )
    }

    #[getter]
    fn shutdown_cost(&self) -> f64 {
        self.inner.shutdown_cost
    }

    #[getter]
    fn startup_limits<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .startup_limits
                .iter()
                .map(PyScucStartupLimit::from),
        )
    }

    #[getter]
    fn energy_upper_bounds<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .energy_upper_bounds
                .iter()
                .map(PyScucEnergyRequirement::from),
        )
    }

    #[getter]
    fn energy_lower_bounds<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .energy_lower_bounds
                .iter()
                .map(PyScucEnergyRequirement::from),
        )
    }

    #[getter]
    fn minimum_up_time(&self) -> f64 {
        self.inner.minimum_up_time
    }

    #[getter]
    fn minimum_down_time(&self) -> f64 {
        self.inner.minimum_down_time
    }

    #[getter]
    fn ramp_limits(&self) -> PyScucRampLimits {
        PyScucRampLimits::from(&self.inner.ramp_limits)
    }

    #[getter]
    fn reserve_limits(&self) -> PyScucReserveLimits {
        PyScucReserveLimits::from(&self.inner.reserve_limits)
    }

    #[getter]
    fn initial_on_status(&self) -> bool {
        self.inner.initial_on_status
    }

    #[getter]
    fn initial_commitment(&self) -> PyScucInitialCommitment {
        PyScucInitialCommitment::from(&self.inner.initial_commitment)
    }

    #[getter]
    fn reactive_capability(&self) -> PyScucReactiveCapability {
        PyScucReactiveCapability::from(&self.inner.reactive_capability)
    }

    #[getter]
    fn periods<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.inner.periods.iter().map(PyScucDevicePeriod::from))
    }
}

impl From<&powerio_prob::ScucDevice> for PyScucDevice {
    fn from(value: &powerio_prob::ScucDevice) -> Self {
        Self {
            inner: value.clone(),
        }
    }
}

#[pyclass(
    name = "ScucShunt",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucShunt {
    inner: powerio_prob::ScucShunt,
}

#[pymethods]
impl PyScucShunt {
    #[getter]
    fn id(&self) -> PyComponentId {
        PyComponentId {
            inner: self.inner.id.clone(),
        }
    }

    #[getter]
    fn conductance_per_step(&self) -> f64 {
        self.inner.conductance_per_step
    }

    #[getter]
    fn susceptance_per_step(&self) -> f64 {
        self.inner.susceptance_per_step
    }

    #[getter]
    fn step_min(&self) -> i64 {
        self.inner.step_min
    }

    #[getter]
    fn step_max(&self) -> i64 {
        self.inner.step_max
    }

    #[getter]
    fn initial_step(&self) -> i64 {
        self.inner.initial_step
    }
}

impl From<&powerio_prob::ScucShunt> for PyScucShunt {
    fn from(value: &powerio_prob::ScucShunt) -> Self {
        Self {
            inner: value.clone(),
        }
    }
}

#[pyclass(
    name = "ScucBranchSwitchingCost",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucBranchSwitchingCost {
    inner: powerio_prob::ScucBranchSwitchingCost,
}

#[pymethods]
impl PyScucBranchSwitchingCost {
    #[getter]
    fn id(&self) -> PyComponentId {
        PyComponentId {
            inner: self.inner.id.clone(),
        }
    }

    #[getter]
    fn connection_cost(&self) -> f64 {
        self.inner.connection_cost
    }

    #[getter]
    fn disconnection_cost(&self) -> f64 {
        self.inner.disconnection_cost
    }
}

impl From<&powerio_prob::ScucBranchSwitchingCost> for PyScucBranchSwitchingCost {
    fn from(value: &powerio_prob::ScucBranchSwitchingCost) -> Self {
        Self {
            inner: value.clone(),
        }
    }
}

#[pyclass(
    name = "ScucTransformerControl",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucTransformerControl {
    inner: powerio_prob::ScucTransformerControl,
}

#[pymethods]
impl PyScucTransformerControl {
    #[getter]
    fn id(&self) -> PyComponentId {
        PyComponentId {
            inner: self.inner.id.clone(),
        }
    }

    #[getter]
    fn tap_ratio_min(&self) -> f64 {
        self.inner.tap_ratio_min
    }

    #[getter]
    fn tap_ratio_max(&self) -> f64 {
        self.inner.tap_ratio_max
    }

    #[getter]
    fn phase_shift_min(&self) -> f64 {
        self.inner.phase_shift_min
    }

    #[getter]
    fn phase_shift_max(&self) -> f64 {
        self.inner.phase_shift_max
    }
}

impl From<&powerio_prob::ScucTransformerControl> for PyScucTransformerControl {
    fn from(value: &powerio_prob::ScucTransformerControl) -> Self {
        Self {
            inner: value.clone(),
        }
    }
}

fn component_id_tuple<'py>(
    py: Python<'py>,
    ids: &[powerio_core::ComponentId],
) -> PyResult<Bound<'py, PyTuple>> {
    PyTuple::new(py, ids.iter().map(|id| PyComponentId { inner: id.clone() }))
}

fn f64_tuple<'py>(py: Python<'py>, values: &[f64]) -> PyResult<Bound<'py, PyTuple>> {
    PyTuple::new(py, values.iter().copied())
}

fn nested_f64_tuple<'py>(py: Python<'py>, values: &[Vec<f64>]) -> PyResult<Bound<'py, PyTuple>> {
    let rows = values
        .iter()
        .map(|row| f64_tuple(py, row).map(Bound::unbind))
        .collect::<PyResult<Vec<_>>>()?;
    PyTuple::new(py, rows)
}

fn nested_i64_tuple<'py>(py: Python<'py>, values: &[Vec<i64>]) -> PyResult<Bound<'py, PyTuple>> {
    let rows = values
        .iter()
        .map(|row| PyTuple::new(py, row.iter().copied()).map(Bound::unbind))
        .collect::<PyResult<Vec<_>>>()?;
    PyTuple::new(py, rows)
}

fn nested_bool_tuple<'py>(py: Python<'py>, values: &[Vec<bool>]) -> PyResult<Bound<'py, PyTuple>> {
    let rows = values
        .iter()
        .map(|row| PyTuple::new(py, row.iter().copied()).map(Bound::unbind))
        .collect::<PyResult<Vec<_>>>()?;
    PyTuple::new(py, rows)
}

#[pyclass(
    name = "ScucActiveReserveZone",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucActiveReserveZone {
    inner: powerio_prob::ScucActiveReserveZone,
}

#[pymethods]
impl PyScucActiveReserveZone {
    #[getter]
    fn id(&self) -> PyComponentId {
        PyComponentId {
            inner: self.inner.id.clone(),
        }
    }

    #[getter]
    fn buses<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        component_id_tuple(py, &self.inner.buses)
    }

    #[getter]
    fn regulation_up_requirement_fraction(&self) -> f64 {
        self.inner.regulation_up_requirement_fraction
    }

    #[getter]
    fn regulation_down_requirement_fraction(&self) -> f64 {
        self.inner.regulation_down_requirement_fraction
    }

    #[getter]
    fn synchronized_requirement_fraction(&self) -> f64 {
        self.inner.synchronized_requirement_fraction
    }

    #[getter]
    fn nonsynchronized_requirement_fraction(&self) -> f64 {
        self.inner.nonsynchronized_requirement_fraction
    }

    #[getter]
    fn ramping_up_requirement<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        f64_tuple(py, &self.inner.ramping_up_requirement)
    }

    #[getter]
    fn ramping_down_requirement<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        f64_tuple(py, &self.inner.ramping_down_requirement)
    }

    #[getter]
    fn regulation_up_violation_cost(&self) -> f64 {
        self.inner.regulation_up_violation_cost
    }

    #[getter]
    fn regulation_down_violation_cost(&self) -> f64 {
        self.inner.regulation_down_violation_cost
    }

    #[getter]
    fn synchronized_violation_cost(&self) -> f64 {
        self.inner.synchronized_violation_cost
    }

    #[getter]
    fn nonsynchronized_violation_cost(&self) -> f64 {
        self.inner.nonsynchronized_violation_cost
    }

    #[getter]
    fn ramping_up_violation_cost(&self) -> f64 {
        self.inner.ramping_up_violation_cost
    }

    #[getter]
    fn ramping_down_violation_cost(&self) -> f64 {
        self.inner.ramping_down_violation_cost
    }
}

impl From<&powerio_prob::ScucActiveReserveZone> for PyScucActiveReserveZone {
    fn from(value: &powerio_prob::ScucActiveReserveZone) -> Self {
        Self {
            inner: value.clone(),
        }
    }
}

#[pyclass(
    name = "ScucReactiveReserveZone",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucReactiveReserveZone {
    inner: powerio_prob::ScucReactiveReserveZone,
}

#[pymethods]
impl PyScucReactiveReserveZone {
    #[getter]
    fn id(&self) -> PyComponentId {
        PyComponentId {
            inner: self.inner.id.clone(),
        }
    }

    #[getter]
    fn buses<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        component_id_tuple(py, &self.inner.buses)
    }

    #[getter]
    fn reactive_up_requirement<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        f64_tuple(py, &self.inner.reactive_up_requirement)
    }

    #[getter]
    fn reactive_down_requirement<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        f64_tuple(py, &self.inner.reactive_down_requirement)
    }

    #[getter]
    fn reactive_up_violation_cost(&self) -> f64 {
        self.inner.reactive_up_violation_cost
    }

    #[getter]
    fn reactive_down_violation_cost(&self) -> f64 {
        self.inner.reactive_down_violation_cost
    }
}

impl From<&powerio_prob::ScucReactiveReserveZone> for PyScucReactiveReserveZone {
    fn from(value: &powerio_prob::ScucReactiveReserveZone) -> Self {
        Self {
            inner: value.clone(),
        }
    }
}

#[pyclass(
    name = "ScucContingency",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucContingency {
    inner: powerio_prob::ScucContingency,
}

#[pymethods]
impl PyScucContingency {
    #[getter]
    fn id(&self) -> PyComponentId {
        PyComponentId {
            inner: self.inner.id.clone(),
        }
    }

    #[getter]
    fn components<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        component_id_tuple(py, &self.inner.components)
    }
}

impl From<&powerio_prob::ScucContingency> for PyScucContingency {
    fn from(value: &powerio_prob::ScucContingency) -> Self {
        Self {
            inner: value.clone(),
        }
    }
}

#[pyclass(
    name = "ScucInputs",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucInputs {
    inner: powerio_prob::ScucInputs,
}

#[pymethods]
impl PyScucInputs {
    #[getter]
    fn interval_durations<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        f64_tuple(py, &self.inner.interval_durations)
    }

    #[getter]
    fn devices<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.inner.devices.iter().map(PyScucDevice::from))
    }

    #[getter]
    fn shunts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.inner.shunts.iter().map(PyScucShunt::from))
    }

    #[getter]
    fn branch_switching_costs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .branch_switching_costs
                .iter()
                .map(PyScucBranchSwitchingCost::from),
        )
    }

    #[getter]
    fn transformer_controls<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .transformer_controls
                .iter()
                .map(PyScucTransformerControl::from),
        )
    }

    #[getter]
    fn active_reserve_zones<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .active_reserve_zones
                .iter()
                .map(PyScucActiveReserveZone::from),
        )
    }

    #[getter]
    fn reactive_reserve_zones<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .reactive_reserve_zones
                .iter()
                .map(PyScucReactiveReserveZone::from),
        )
    }

    #[getter]
    fn contingencies<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner.contingencies.iter().map(PyScucContingency::from),
        )
    }

    #[getter]
    fn violation_costs(&self) -> PyScucViolationCosts {
        PyScucViolationCosts::from(&self.inner.violation_costs)
    }
}

#[pyclass(
    name = "Residuals",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone, Copy)]
struct PyResiduals {
    inner: powerio_prob::Residuals,
}

#[pymethods]
impl PyResiduals {
    #[getter]
    fn max_active_power_mismatch(&self) -> Option<f64> {
        self.inner.max_active_power_mismatch
    }

    #[getter]
    fn max_reactive_power_mismatch(&self) -> Option<f64> {
        self.inner.max_reactive_power_mismatch
    }
}

#[pyclass(
    name = "ScucNetworkOutputs",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucNetworkOutputs {
    inner: powerio_prob::ScucNetworkOutputs,
}

macro_rules! nested_output_methods {
    ($py_type:ident, $($field:ident => $convert:ident),+ $(,)?) => {
        #[pymethods]
        impl $py_type {
            $(
                #[getter]
                fn $field<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
                    $convert(py, &self.inner.$field)
                }
            )+
        }
    };
}

nested_output_methods!(
    PyScucNetworkOutputs,
    bus_vm => nested_f64_tuple,
    bus_va => nested_f64_tuple,
    shunt_step => nested_i64_tuple,
    ac_line_on_status => nested_bool_tuple,
    transformer_tm => nested_f64_tuple,
    transformer_ta => nested_f64_tuple,
    transformer_on_status => nested_bool_tuple,
    dc_line_pdc_fr => nested_f64_tuple,
    dc_line_qdc_fr => nested_f64_tuple,
    dc_line_qdc_to => nested_f64_tuple,
);

#[pyclass(
    name = "ScucDeviceOutputs",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyScucDeviceOutputs {
    inner: powerio_prob::ScucDeviceOutputs,
}

nested_output_methods!(
    PyScucDeviceOutputs,
    on_status => nested_bool_tuple,
    startup_status => nested_bool_tuple,
    shutdown_status => nested_bool_tuple,
    p_on => nested_f64_tuple,
    q => nested_f64_tuple,
    p_reg_res_up => nested_f64_tuple,
    p_reg_res_down => nested_f64_tuple,
    p_syn_res => nested_f64_tuple,
    p_nsyn_res => nested_f64_tuple,
    p_ramp_res_up_online => nested_f64_tuple,
    p_ramp_res_up_offline => nested_f64_tuple,
    p_ramp_res_down_online => nested_f64_tuple,
    p_ramp_res_down_offline => nested_f64_tuple,
    q_res_up => nested_f64_tuple,
    q_res_down => nested_f64_tuple,
);

#[pyclass(
    name = "ActivePower",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone, Copy)]
struct PyActivePower {
    inner: powerio_prob::ActivePower,
}

#[pymethods]
impl PyActivePower {
    #[staticmethod]
    fn watts(value: f64) -> Self {
        Self {
            inner: powerio_prob::ActivePower::from_watts(value),
        }
    }

    #[staticmethod]
    fn megawatts(value: f64) -> Self {
        Self {
            inner: powerio_prob::ActivePower::from_megawatts(value),
        }
    }

    #[getter]
    fn value(&self) -> f64 {
        self.inner.value()
    }

    #[getter]
    fn unit(&self) -> &'static str {
        match self.inner.unit() {
            powerio_prob::ActivePowerUnit::Watts => "watts",
            powerio_prob::ActivePowerUnit::Megawatts => "megawatts",
            _ => unreachable!("all active power units have Python spellings"),
        }
    }

    fn __repr__(&self) -> String {
        format!("ActivePower.{}({:?})", self.unit(), self.value())
    }
}

#[pyclass(
    name = "ReactivePower",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone, Copy)]
struct PyReactivePower {
    inner: powerio_prob::ReactivePower,
}

#[pymethods]
impl PyReactivePower {
    #[staticmethod]
    fn vars(value: f64) -> Self {
        Self {
            inner: powerio_prob::ReactivePower::from_vars(value),
        }
    }

    #[staticmethod]
    fn megavars(value: f64) -> Self {
        Self {
            inner: powerio_prob::ReactivePower::from_megavars(value),
        }
    }

    #[getter]
    fn value(&self) -> f64 {
        self.inner.value()
    }

    #[getter]
    fn unit(&self) -> &'static str {
        match self.inner.unit() {
            powerio_prob::ReactivePowerUnit::Vars => "vars",
            powerio_prob::ReactivePowerUnit::Megavars => "megavars",
            _ => unreachable!("all reactive power units have Python spellings"),
        }
    }

    fn __repr__(&self) -> String {
        format!("ReactivePower.{}({:?})", self.unit(), self.value())
    }
}

#[pyclass(
    name = "ApparentPower",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone, Copy)]
struct PyApparentPower {
    inner: powerio_prob::ApparentPower,
}

#[pymethods]
impl PyApparentPower {
    #[staticmethod]
    fn volt_amperes(value: f64) -> Self {
        Self {
            inner: powerio_prob::ApparentPower::from_volt_amperes(value),
        }
    }

    #[staticmethod]
    fn megavolt_amperes(value: f64) -> Self {
        Self {
            inner: powerio_prob::ApparentPower::from_megavolt_amperes(value),
        }
    }

    #[getter]
    fn value(&self) -> f64 {
        self.inner.value()
    }

    #[getter]
    fn unit(&self) -> &'static str {
        match self.inner.unit() {
            powerio_prob::ApparentPowerUnit::VoltAmperes => "volt_amperes",
            powerio_prob::ApparentPowerUnit::MegavoltAmperes => "megavolt_amperes",
            _ => unreachable!("all apparent power units have Python spellings"),
        }
    }

    fn __repr__(&self) -> String {
        format!("ApparentPower.{}({:?})", self.unit(), self.value())
    }
}

fn operating_update_field(update: &powerio_prob::OperatingPointUpdate) -> &'static str {
    use powerio_prob::OperatingPointUpdate as U;
    match update {
        U::LoadActivePower { .. } => "load_active_power",
        U::LoadReactivePower { .. } => "load_reactive_power",
        U::GeneratorActivePower { .. } => "generator_active_power",
        U::GeneratorReactivePower { .. } => "generator_reactive_power",
        U::GeneratorVoltageMagnitude { .. } => "generator_voltage_magnitude",
        U::GeneratorInService { .. } => "generator_in_service",
        U::BranchInService { .. } => "branch_in_service",
        U::TransformerTapRatio { .. } => "transformer_tap_ratio",
        U::TransformerPhaseShift { .. } => "transformer_phase_shift",
        U::SwitchClosed { .. } => "switch_closed",
        _ => unreachable!("all operating point updates have Python spellings"),
    }
}

#[pyclass(
    name = "OperatingPointUpdate",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyOperatingPointUpdate {
    inner: powerio_prob::OperatingPointUpdate,
}

#[pymethods]
impl PyOperatingPointUpdate {
    #[staticmethod]
    #[pyo3(signature = (load, p, *, terminal=None))]
    fn set_load_active_power(
        load: &PyComponentId,
        p: &PyActivePower,
        terminal: Option<String>,
    ) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::LoadActivePower {
                load: load.inner.clone(),
                terminal,
                p: p.inner,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (load, q, *, terminal=None))]
    fn set_load_reactive_power(
        load: &PyComponentId,
        q: &PyReactivePower,
        terminal: Option<String>,
    ) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::LoadReactivePower {
                load: load.inner.clone(),
                terminal,
                q: q.inner,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (generator, p, *, terminal=None))]
    fn set_generator_active_power(
        generator: &PyComponentId,
        p: &PyActivePower,
        terminal: Option<String>,
    ) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::GeneratorActivePower {
                generator: generator.inner.clone(),
                terminal,
                p: p.inner,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (generator, q, *, terminal=None))]
    fn set_generator_reactive_power(
        generator: &PyComponentId,
        q: &PyReactivePower,
        terminal: Option<String>,
    ) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::GeneratorReactivePower {
                generator: generator.inner.clone(),
                terminal,
                q: q.inner,
            },
        }
    }

    #[staticmethod]
    fn set_generator_voltage_magnitude(generator: &PyComponentId, vm_pu: f64) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::GeneratorVoltageMagnitude {
                generator: generator.inner.clone(),
                vm_pu,
            },
        }
    }

    #[staticmethod]
    fn set_generator_in_service(generator: &PyComponentId, in_service: bool) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::GeneratorInService {
                generator: generator.inner.clone(),
                in_service,
            },
        }
    }

    #[staticmethod]
    fn set_branch_in_service(branch: &PyComponentId, in_service: bool) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::BranchInService {
                branch: branch.inner.clone(),
                in_service,
            },
        }
    }

    #[staticmethod]
    fn set_transformer_tap_ratio(transformer: &PyComponentId, tap_ratio: f64) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::TransformerTapRatio {
                transformer: transformer.inner.clone(),
                tap_ratio,
            },
        }
    }

    #[staticmethod]
    fn set_transformer_phase_shift(transformer: &PyComponentId, shift_degrees: f64) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::TransformerPhaseShift {
                transformer: transformer.inner.clone(),
                shift_degrees,
            },
        }
    }

    #[staticmethod]
    fn set_switch_closed(switch: &PyComponentId, closed: bool) -> Self {
        Self {
            inner: powerio_prob::OperatingPointUpdate::SwitchClosed {
                switch: switch.inner.clone(),
                closed,
            },
        }
    }

    #[getter]
    fn field(&self) -> &'static str {
        operating_update_field(&self.inner)
    }

    fn __repr__(&self) -> String {
        format!("OperatingPointUpdate(field={:?})", self.field())
    }
}

#[pyclass(
    name = "NetworkUpdate",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyNetworkUpdate {
    inner: powerio_prob::NetworkUpdate,
}

#[pymethods]
impl PyNetworkUpdate {
    #[staticmethod]
    #[pyo3(signature = (branch, rating, *, terminal=None))]
    fn set_branch_thermal_rating(
        branch: &PyComponentId,
        rating: &PyApparentPower,
        terminal: Option<String>,
    ) -> Self {
        Self {
            inner: powerio_prob::NetworkUpdate::BranchThermalRating {
                branch: branch.inner.clone(),
                terminal,
                rating: rating.inner,
            },
        }
    }

    #[getter]
    fn field(&self) -> &'static str {
        match self.inner {
            powerio_prob::NetworkUpdate::BranchThermalRating { .. } => "branch_thermal_rating",
            _ => unreachable!("all network updates have Python spellings"),
        }
    }

    fn __repr__(&self) -> String {
        format!("NetworkUpdate(field={:?})", self.field())
    }
}

#[pyclass(
    name = "CalculationUpdate",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyCalculationUpdate {
    inner: powerio_prob::CalculationUpdate,
}

#[pymethods]
impl PyCalculationUpdate {
    #[new]
    fn new(update: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(update) = update.extract::<PyRef<'_, PyOperatingPointUpdate>>() {
            Ok(Self {
                inner: powerio_prob::CalculationUpdate::OperatingPoint(update.inner.clone()),
            })
        } else if let Ok(update) = update.extract::<PyRef<'_, PyNetworkUpdate>>() {
            Ok(Self {
                inner: powerio_prob::CalculationUpdate::Network(update.inner.clone()),
            })
        } else {
            Err(PyTypeError::new_err(
                "CalculationUpdate takes an OperatingPointUpdate or NetworkUpdate",
            ))
        }
    }

    #[getter]
    fn data_role(&self) -> &'static str {
        match self.inner {
            powerio_prob::CalculationUpdate::OperatingPoint(_) => "operating_point",
            powerio_prob::CalculationUpdate::Network(_) => "network",
            _ => unreachable!("all calculation updates have Python spellings"),
        }
    }

    fn __repr__(&self) -> String {
        format!("CalculationUpdate(data_role={:?})", self.data_role())
    }
}

fn updated_field_name(field: powerio_prob::UpdatedField) -> &'static str {
    use powerio_prob::UpdatedField as F;
    match field {
        F::LoadActivePower => "load_active_power",
        F::LoadReactivePower => "load_reactive_power",
        F::GeneratorActivePower => "generator_active_power",
        F::GeneratorReactivePower => "generator_reactive_power",
        F::GeneratorVoltageMagnitude => "generator_voltage_magnitude",
        F::GeneratorInService => "generator_in_service",
        F::BranchThermalRating => "branch_thermal_rating",
        F::BranchInService => "branch_in_service",
        F::TransformerTapRatio => "transformer_tap_ratio",
        F::TransformerPhaseShift => "transformer_phase_shift",
        F::SwitchClosed => "switch_closed",
        _ => unreachable!("all updated fields have Python spellings"),
    }
}

#[pyclass(
    name = "UpdateChange",
    module = "powerio._powerio",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyUpdateChange {
    component_id: powerio_core::ComponentId,
    field: powerio_prob::UpdatedField,
    terminal: Option<String>,
}

impl From<&powerio_prob::UpdateChange> for PyUpdateChange {
    fn from(change: &powerio_prob::UpdateChange) -> Self {
        Self {
            component_id: change.component_id().clone(),
            field: change.field(),
            terminal: change.terminal().map(str::to_owned),
        }
    }
}

#[pymethods]
impl PyUpdateChange {
    #[getter]
    fn component_id(&self) -> PyComponentId {
        PyComponentId {
            inner: self.component_id.clone(),
        }
    }

    #[getter]
    fn field(&self) -> &'static str {
        updated_field_name(self.field)
    }

    #[getter]
    fn terminal(&self) -> Option<&str> {
        self.terminal.as_deref()
    }

    fn __repr__(&self) -> String {
        format!(
            "UpdateChange(component_id={:?}, field={:?}, terminal={:?})",
            self.component_id.to_string(),
            self.field(),
            self.terminal
        )
    }
}

#[pyclass(name = "UpdateReport", module = "powerio._powerio", frozen)]
struct PyUpdateReport {
    inner: powerio_prob::UpdateReport,
}

#[pymethods]
impl PyUpdateReport {
    #[getter]
    fn changes(&self) -> Vec<PyUpdateChange> {
        self.inner
            .changes()
            .iter()
            .map(PyUpdateChange::from)
            .collect()
    }

    #[getter]
    fn connectivity_changed(&self) -> bool {
        self.inner.connectivity_changed()
    }

    fn __len__(&self) -> usize {
        self.inner.changes().len()
    }

    fn __bool__(&self) -> bool {
        !self.inner.is_empty()
    }

    fn __repr__(&self) -> String {
        format!(
            "UpdateReport(changes={}, connectivity_changed={})",
            self.inner.changes().len(),
            self.inner.connectivity_changed()
        )
    }
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
    for diagnostic in &module.diagnostics {
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

    fn child(&self, value: powerio::PioValue) -> PyResult<Self> {
        module_with_records(self.module()?, value).map(|module| Self {
            module: Some(module),
        })
    }

    fn ac_scuc_instance(&self) -> PyResult<&powerio::AcScucInstance> {
        match &self.module()?.value() {
            powerio::PioValue::AcScucInstance(instance) => Ok(instance),
            other => Err(PyTypeError::new_err(format!(
                "{} is not an AC security constrained unit commitment instance",
                other.type_name()
            ))),
        }
    }

    fn ac_scuc_solution(&self) -> PyResult<&powerio::AcScucSolution> {
        match &self.module()?.value() {
            powerio::PioValue::AcScucSolution(solution) => Ok(solution),
            other => Err(PyTypeError::new_err(format!(
                "{} is not an AC security constrained unit commitment solution",
                other.type_name()
            ))),
        }
    }

    fn balanced_calculation_network(&self) -> PyResult<&powerio::BalancedNetwork> {
        let value = &self.module()?.value();
        match value {
            powerio::PioValue::DcPfInstance(instance) => Ok(instance.network()),
            powerio::PioValue::AcPfInstance(instance) => Ok(instance.network()),
            powerio::PioValue::DcOpfInstance(instance) => Ok(instance.network()),
            powerio::PioValue::AcOpfInstance(instance) => Ok(instance.network()),
            powerio::PioValue::AcScucInstance(instance) => Ok(instance.network()),
            powerio::PioValue::DcPfSolution(solution) => Ok(solution.network()),
            powerio::PioValue::AcPfSolution(solution) => Ok(solution.network()),
            powerio::PioValue::DcOpfSolution(solution) => Ok(solution.network()),
            powerio::PioValue::AcOpfSolution(solution) => Ok(solution.network()),
            powerio::PioValue::SocwrOpfSolution(solution) => Ok(solution.network()),
            powerio::PioValue::AcScucSolution(solution) => Ok(solution.instance().network()),
            other => Err(PyTypeError::new_err(format!(
                "{} does not contain a balanced calculation",
                other.type_name()
            ))),
        }
    }

    fn multiconductor_calculation_network(&self) -> PyResult<&powerio::MulticonductorNetwork> {
        let value = &self.module()?.value();
        match value {
            powerio::PioValue::McAcPfInstance(instance) => Ok(instance.network()),
            powerio::PioValue::McAcOpfInstance(instance) => Ok(instance.network()),
            powerio::PioValue::McAcPfSolution(solution) => Ok(solution.network()),
            powerio::PioValue::McAcOpfSolution(solution) => Ok(solution.network()),
            other => Err(PyTypeError::new_err(format!(
                "{} does not contain a multiconductor calculation",
                other.type_name()
            ))),
        }
    }
}

fn collection_values(values: &Bound<'_, PyList>) -> PyResult<Vec<powerio::PioValue>> {
    values
        .iter()
        .map(|value| {
            let module = value.extract::<PyRef<'_, PyPioModule>>()?;
            Ok(module.module()?.value().clone())
        })
        .collect()
}

enum PyUpdateBatch {
    Empty,
    OperatingPoint(Vec<powerio_prob::OperatingPointUpdate>),
    Network(Vec<powerio_prob::NetworkUpdate>),
    Calculation(Vec<powerio_prob::CalculationUpdate>),
}

fn extract_update_batch(updates: &Bound<'_, PyAny>) -> PyResult<PyUpdateBatch> {
    let mut operating = Vec::new();
    let mut network = Vec::new();
    let mut calculation = Vec::new();
    let iterator = updates
        .try_iter()
        .map_err(|_| PyTypeError::new_err("updates must be an iterable of typed updates"))?;
    for item in iterator {
        let item = item?;
        if let Ok(update) = item.extract::<PyRef<'_, PyOperatingPointUpdate>>() {
            if !network.is_empty() || !calculation.is_empty() {
                return Err(PyTypeError::new_err(
                    "one update batch cannot mix update classes",
                ));
            }
            operating.push(update.inner.clone());
        } else if let Ok(update) = item.extract::<PyRef<'_, PyNetworkUpdate>>() {
            if !operating.is_empty() || !calculation.is_empty() {
                return Err(PyTypeError::new_err(
                    "one update batch cannot mix update classes",
                ));
            }
            network.push(update.inner.clone());
        } else if let Ok(update) = item.extract::<PyRef<'_, PyCalculationUpdate>>() {
            if !operating.is_empty() || !network.is_empty() {
                return Err(PyTypeError::new_err(
                    "one update batch cannot mix update classes",
                ));
            }
            calculation.push(update.inner.clone());
        } else {
            return Err(PyTypeError::new_err(
                "typed updates must be OperatingPointUpdate, NetworkUpdate, or CalculationUpdate values",
            ));
        }
    }
    if !operating.is_empty() {
        Ok(PyUpdateBatch::OperatingPoint(operating))
    } else if !network.is_empty() {
        Ok(PyUpdateBatch::Network(network))
    } else if !calculation.is_empty() {
        Ok(PyUpdateBatch::Calculation(calculation))
    } else {
        Ok(PyUpdateBatch::Empty)
    }
}

macro_rules! apply_typed_updates {
    ($source:expr, $value:expr, $updates:expr, $wrap:path $(,)?) => {{
        let mut typed = module_with_records($source, $value.clone())?;
        let report = powerio_prob::apply_updates(&mut typed, $updates)
            .map_err(|error| core_error_pyerr(&error))?;
        let updated_value = $wrap(typed.value().clone());
        let updated = module_with_records(&typed, updated_value)?;
        Ok((updated, report))
    }};
}

fn apply_operating_point_updates(
    source: &powerio_core::PioModule<powerio::PioValue>,
    updates: &[powerio_prob::OperatingPointUpdate],
) -> PyResult<(
    powerio_core::PioModule<powerio::PioValue>,
    powerio_prob::UpdateReport,
)> {
    match &source.value() {
        powerio::PioValue::BalancedNetwork(value) => {
            apply_typed_updates!(source, value, updates, powerio::PioValue::BalancedNetwork)
        }
        powerio::PioValue::MulticonductorNetwork(value) => apply_typed_updates!(
            source,
            value,
            updates,
            powerio::PioValue::MulticonductorNetwork,
        ),
        powerio::PioValue::BalancedOperatingPoint(value) => apply_typed_updates!(
            source,
            value,
            updates,
            powerio::PioValue::BalancedOperatingPoint,
        ),
        powerio::PioValue::MulticonductorOperatingPoint(value) => apply_typed_updates!(
            source,
            value,
            updates,
            powerio::PioValue::MulticonductorOperatingPoint,
        ),
        value => Err(PyTypeError::new_err(format!(
            "OperatingPointUpdate cannot be applied to {}",
            value.type_name()
        ))),
    }
}

fn apply_network_updates(
    source: &powerio_core::PioModule<powerio::PioValue>,
    updates: &[powerio_prob::NetworkUpdate],
) -> PyResult<(
    powerio_core::PioModule<powerio::PioValue>,
    powerio_prob::UpdateReport,
)> {
    match &source.value() {
        powerio::PioValue::BalancedNetwork(value) => {
            apply_typed_updates!(source, value, updates, powerio::PioValue::BalancedNetwork)
        }
        powerio::PioValue::MulticonductorNetwork(value) => apply_typed_updates!(
            source,
            value,
            updates,
            powerio::PioValue::MulticonductorNetwork,
        ),
        value => Err(PyTypeError::new_err(format!(
            "NetworkUpdate cannot be applied to {}",
            value.type_name()
        ))),
    }
}

fn apply_calculation_updates(
    source: &powerio_core::PioModule<powerio::PioValue>,
    updates: &[powerio_prob::CalculationUpdate],
) -> PyResult<(
    powerio_core::PioModule<powerio::PioValue>,
    powerio_prob::UpdateReport,
)> {
    match &source.value() {
        powerio::PioValue::DcPfInstance(value) => {
            apply_typed_updates!(source, value, updates, powerio::PioValue::DcPfInstance)
        }
        powerio::PioValue::AcPfInstance(value) => {
            apply_typed_updates!(source, value, updates, powerio::PioValue::AcPfInstance)
        }
        powerio::PioValue::DcOpfInstance(value) => {
            apply_typed_updates!(source, value, updates, powerio::PioValue::DcOpfInstance)
        }
        powerio::PioValue::AcOpfInstance(value) => {
            apply_typed_updates!(source, value, updates, powerio::PioValue::AcOpfInstance)
        }
        powerio::PioValue::McAcPfInstance(value) => {
            apply_typed_updates!(source, value, updates, powerio::PioValue::McAcPfInstance)
        }
        powerio::PioValue::McAcOpfInstance(value) => {
            apply_typed_updates!(source, value, updates, powerio::PioValue::McAcOpfInstance)
        }
        value => Err(PyTypeError::new_err(format!(
            "CalculationUpdate cannot be applied to {}",
            value.type_name()
        ))),
    }
}

fn apply_bus_load_active_power_to_module(
    source: &powerio_core::PioModule<powerio::PioValue>,
    bus_id: usize,
    total: powerio_prob::ActivePower,
    allocation: &str,
) -> PyResult<(
    powerio_core::PioModule<powerio::PioValue>,
    powerio_prob::UpdateReport,
)> {
    let allocation = match allocation {
        "equal" => powerio_prob::LoadAllocation::Equal,
        "proportional_to_current_active_power" => {
            powerio_prob::LoadAllocation::ProportionalToCurrentActivePower
        }
        _ => {
            return Err(PyValueError::new_err(
                "allocation must be 'equal' or 'proportional_to_current_active_power'",
            ));
        }
    };

    macro_rules! apply_to {
        ($value:expr, $wrap:path) => {{
            let mut typed = module_with_records(source, $value.clone())?;
            let report = powerio_prob::apply_bus_load_active_power(
                &mut typed,
                powerio_tx::BusId(bus_id),
                total,
                allocation,
            )
            .map_err(|error| core_error_pyerr(&error))?;
            let updated = module_with_records(&typed, $wrap(typed.value().clone()))?;
            Ok((updated, report))
        }};
    }

    match &source.value() {
        powerio::PioValue::DcPfInstance(value) => {
            apply_to!(value, powerio::PioValue::DcPfInstance)
        }
        powerio::PioValue::AcPfInstance(value) => {
            apply_to!(value, powerio::PioValue::AcPfInstance)
        }
        powerio::PioValue::DcOpfInstance(value) => {
            apply_to!(value, powerio::PioValue::DcOpfInstance)
        }
        powerio::PioValue::AcOpfInstance(value) => {
            apply_to!(value, powerio::PioValue::AcOpfInstance)
        }
        value => Err(PyTypeError::new_err(format!(
            "a bus load allocation requires a balanced power flow or optimal power flow instance; received {}",
            value.type_name()
        ))),
    }
}

fn apply_update_batch(
    source: &powerio_core::PioModule<powerio::PioValue>,
    batch: &PyUpdateBatch,
) -> PyResult<(
    powerio_core::PioModule<powerio::PioValue>,
    powerio_prob::UpdateReport,
)> {
    match batch {
        PyUpdateBatch::Empty => Ok((
            module_with_records(source, source.value().clone())?,
            powerio_prob::UpdateReport::default(),
        )),
        PyUpdateBatch::OperatingPoint(updates) => apply_operating_point_updates(source, updates),
        PyUpdateBatch::Network(updates) => apply_network_updates(source, updates),
        PyUpdateBatch::Calculation(updates) => apply_calculation_updates(source, updates),
    }
}

fn collection_update_target_mut<'a>(
    value: &'a mut powerio::PioValue,
    time_index: Option<usize>,
    scenario_id: Option<&str>,
) -> PyResult<&'a mut powerio::PioValue> {
    match value {
        powerio::PioValue::TimeSeries(series) => {
            let Some(index) = time_index else {
                return Err(PyTypeError::new_err(
                    "a TimeSeries entry update requires a time index",
                ));
            };
            let target = series
                .get_mut(index)
                .ok_or_else(|| PyValueError::new_err("time series index out of range"))?;
            collection_update_target_mut(target, None, scenario_id)
        }
        powerio::PioValue::ScenarioSet(scenarios) => {
            let Some(id) = scenario_id else {
                return Err(PyTypeError::new_err(
                    "a ScenarioSet entry update requires a scenario ID",
                ));
            };
            let target = scenarios
                .get_mut(id)
                .ok_or_else(|| PyValueError::new_err(format!("unknown scenario ID {id:?}")))?;
            collection_update_target_mut(target, time_index, None)
        }
        _ if time_index.is_none() && scenario_id.is_none() => Ok(value),
        _ if time_index.is_some() => Err(PyTypeError::new_err(
            "the selected value is not a TimeSeries",
        )),
        _ => Err(PyTypeError::new_err(
            "the selected value is not a ScenarioSet",
        )),
    }
}

fn apply_collection_entry_updates(
    source: &powerio_core::PioModule<powerio::PioValue>,
    time_index: Option<usize>,
    scenario_id: Option<&str>,
    batch: &PyUpdateBatch,
) -> PyResult<(
    powerio_core::PioModule<powerio::PioValue>,
    powerio_prob::UpdateReport,
)> {
    let mut parent_value = source.value().clone();
    let entry_value =
        collection_update_target_mut(&mut parent_value, time_index, scenario_id)?.clone();
    let entry_module = module_with_records(source, entry_value)?;
    let (updated_entry, report) = apply_update_batch(&entry_module, batch)?;
    *collection_update_target_mut(&mut parent_value, time_index, scenario_id)? =
        updated_entry.value().clone();
    let updated_parent = module_with_records(&updated_entry, parent_value)?;
    Ok((updated_parent, report))
}

#[pymethods]
impl PyPioModule {
    /// Clone this owner rooted module without serializing its value.
    fn _copy(&self) -> PyResult<Self> {
        Ok(Self {
            module: Some(self.module()?.clone()),
        })
    }

    /// Construct one typed time series from owner rooted PowerIO values.
    #[staticmethod]
    fn _from_time_series(
        values: &Bound<'_, PyList>,
        time_points: Vec<(String, Option<f64>)>,
    ) -> PyResult<Self> {
        let values = collection_values(values)?;
        let time_points = time_points
            .into_iter()
            .map(|(label, duration_seconds)| {
                let duration = duration_seconds
                    .map(|seconds| {
                        Duration::try_from_secs_f64(seconds).map_err(|_| {
                            PyValueError::new_err(format!(
                                "time point {label:?} duration must be finite and nonnegative"
                            ))
                        })
                    })
                    .transpose()?;
                powerio_core::TimePoint::new(label, duration)
                    .map_err(|error| core_error_pyerr(&error))
            })
            .collect::<PyResult<Vec<_>>>()?;
        let series = powerio::PioTimeSeries::from_values(time_points, values)
            .map_err(|error| core_error_pyerr(&error))?;
        Ok(Self {
            module: Some(powerio_core::PioModule::new(powerio::PioValue::TimeSeries(
                series,
            ))),
        })
    }

    /// Construct one typed scenario set from owner rooted PowerIO values.
    #[staticmethod]
    #[pyo3(signature = (values, ids, probabilities=None))]
    fn _from_scenario_set(
        values: &Bound<'_, PyList>,
        ids: Vec<String>,
        probabilities: Option<HashMap<String, f64>>,
    ) -> PyResult<Self> {
        let values = collection_values(values)?;
        if values.len() != ids.len() {
            return Err(PyValueError::new_err(format!(
                "scenario set has {} values for {} IDs",
                values.len(),
                ids.len()
            )));
        }
        if let Some(probabilities) = &probabilities
            && (probabilities.len() != ids.len()
                || ids.iter().any(|id| !probabilities.contains_key(id)))
        {
            return Err(PyValueError::new_err(
                "scenario probabilities must name every scenario ID exactly once",
            ));
        }
        let scenarios = ids
            .into_iter()
            .zip(values)
            .map(|(id, value)| {
                let probability = probabilities
                    .as_ref()
                    .and_then(|probabilities| probabilities.get(&id))
                    .copied();
                let id =
                    powerio_core::ScenarioId::new(id).map_err(|error| core_error_pyerr(&error))?;
                Ok(powerio_core::Scenario::new(id, probability, value))
            })
            .collect::<PyResult<Vec<_>>>()?;
        let scenarios = powerio::PioScenarioSet::from_scenarios(scenarios)
            .map_err(|error| core_error_pyerr(&error))?;
        Ok(Self {
            module: Some(powerio_core::PioModule::new(
                powerio::PioValue::ScenarioSet(scenarios),
            )),
        })
    }

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

    /// Private path acquisition used by the public `parse` function.
    #[staticmethod]
    #[pyo3(signature = (path, format=None))]
    fn _parse_path(path: &str, format: Option<&str>) -> PyResult<Self> {
        let source = powerio_core::Source::open(Path::new(path))
            .map_err(|error| core_open_pyerr(Path::new(path), &error))?;
        powerio::parse_with_options(source, &parse_options(format)?)
            .map(|module| Self {
                module: Some(module),
            })
            .map_err(|error| core_error_pyerr(&error))
    }

    /// Private memory acquisition used for bytes-like and file-like sources.
    #[staticmethod]
    #[pyo3(signature = (data, name, format=None))]
    fn _parse_memory(data: &[u8], name: &str, format: Option<&str>) -> PyResult<Self> {
        let source = powerio_core::Source::from_memory(name, data.to_vec())
            .map_err(|error| core_error_pyerr(&error))?;
        powerio::parse_with_options(source, &parse_options(format)?)
            .map(|module| Self {
                module: Some(module),
            })
            .map_err(|error| core_error_pyerr(&error))
    }

    #[staticmethod]
    fn _deserialize_path(path: &str) -> PyResult<Self> {
        let source = powerio_core::Source::open(Path::new(path))
            .map_err(|error| core_open_pyerr(Path::new(path), &error))?;
        powerio::deserialize(source)
            .map(|module| Self {
                module: Some(module),
            })
            .map_err(|error| core_error_pyerr(&error))
    }

    #[staticmethod]
    fn _deserialize_memory(data: &[u8]) -> PyResult<Self> {
        let source = powerio_core::Source::from_memory("<memory>.pio.json", data.to_vec())
            .map_err(|error| core_error_pyerr(&error))?;
        powerio::deserialize(source)
            .map(|module| Self {
                module: Some(module),
            })
            .map_err(|error| core_error_pyerr(&error))
    }

    fn _emit_memory<'py>(&self, py: Python<'py>, format: &str) -> PyResult<Bound<'py, PyDict>> {
        let destination =
            powerio_core::Destination::memory("case").map_err(|error| core_error_pyerr(&error))?;
        let result = powerio::emit(self.module()?, format, destination)
            .map_err(|error| core_error_pyerr(&error))?;
        emit_result_to_py(py, result)
    }

    fn _emit_path<'py>(
        &self,
        py: Python<'py>,
        format: &str,
        path: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let result = powerio::emit(
            self.module()?,
            format,
            powerio_core::Destination::path(path),
        )
        .map_err(|error| core_error_pyerr(&error))?;
        emit_result_to_py(py, result)
    }

    fn _serialize_memory<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let destination = powerio_core::Destination::memory("module.pio.json")
            .map_err(|error| core_error_pyerr(&error))?;
        let result = powerio::serialize(self.module()?, destination)
            .map_err(|error| core_error_pyerr(&error))?;
        emit_result_to_py(py, result)
    }

    fn _serialize_path<'py>(&self, py: Python<'py>, path: &str) -> PyResult<Bound<'py, PyDict>> {
        let result = powerio::serialize(self.module()?, powerio_core::Destination::path(path))
            .map_err(|error| core_error_pyerr(&error))?;
        emit_result_to_py(py, result)
    }

    fn _to_dc_pf_instance(&self) -> PyResult<Self> {
        let module = powerio::transform::to_dc_pf_instance(self.module()?)
            .map_err(|error| core_error_pyerr(&error))?
            .map_value(powerio::PioValue::DcPfInstance);
        Ok(Self {
            module: Some(module),
        })
    }

    fn _to_ac_pf_instance(&self) -> PyResult<Self> {
        let module = powerio::transform::to_ac_pf_instance(self.module()?)
            .map_err(|error| core_error_pyerr(&error))?
            .map_value(powerio::PioValue::AcPfInstance);
        Ok(Self {
            module: Some(module),
        })
    }

    fn _to_dc_opf_instance(&self) -> PyResult<Self> {
        let module = powerio::transform::to_dc_opf_instance(self.module()?)
            .map_err(|error| core_error_pyerr(&error))?
            .map_value(powerio::PioValue::DcOpfInstance);
        Ok(Self {
            module: Some(module),
        })
    }

    fn _to_ac_opf_instance(&self) -> PyResult<Self> {
        let module = powerio::transform::to_ac_opf_instance(self.module()?)
            .map_err(|error| core_error_pyerr(&error))?
            .map_value(powerio::PioValue::AcOpfInstance);
        Ok(Self {
            module: Some(module),
        })
    }

    fn _to_mc_ac_pf_instance(&self) -> PyResult<Self> {
        let module = powerio::transform::to_mc_ac_pf_instance(self.module()?)
            .map_err(|error| core_error_pyerr(&error))?
            .map_value(powerio::PioValue::McAcPfInstance);
        Ok(Self {
            module: Some(module),
        })
    }

    fn _to_mc_ac_opf_instance(&self) -> PyResult<Self> {
        let module = powerio::transform::to_mc_ac_opf_instance(self.module()?)
            .map_err(|error| core_error_pyerr(&error))?
            .map_value(powerio::PioValue::McAcOpfInstance);
        Ok(Self {
            module: Some(module),
        })
    }

    /// The balanced network shared by a balanced calculation instance or
    /// solution. `BalancedNetwork::clone` only retains its copy on write
    /// tables; it does not copy them.
    fn _balanced_calculation_network(&self) -> PyResult<PyBalancedNetwork> {
        let module = self.module()?;
        let network = self.balanced_calculation_network()?.clone();
        Ok(case_from_module(module_with_records(module, network)?))
    }

    /// The multiconductor network shared by a multiconductor calculation
    /// instance or solution.
    fn _multiconductor_calculation_network(&self) -> PyResult<PyMulticonductorNetwork> {
        let module = self.module()?;
        let network = self.multiconductor_calculation_network()?.clone();
        Ok(PyMulticonductorNetwork::from_module(module_with_records(
            module, network,
        )?))
    }

    /// The exact typed instance solved by a calculation solution.
    fn _calculation_solution_instance(&self) -> PyResult<Self> {
        let value = match &self.module()?.value() {
            powerio::PioValue::DcPfSolution(solution) => {
                powerio::PioValue::DcPfInstance(solution.instance().clone())
            }
            powerio::PioValue::AcPfSolution(solution) => {
                powerio::PioValue::AcPfInstance(solution.instance().clone())
            }
            powerio::PioValue::DcOpfSolution(solution) => {
                powerio::PioValue::DcOpfInstance(solution.instance().clone())
            }
            powerio::PioValue::AcOpfSolution(solution) => {
                powerio::PioValue::AcOpfInstance(solution.instance().clone())
            }
            powerio::PioValue::SocwrOpfSolution(solution) => {
                powerio::PioValue::AcOpfInstance(solution.instance().clone())
            }
            powerio::PioValue::McAcPfSolution(solution) => {
                powerio::PioValue::McAcPfInstance(solution.instance().clone())
            }
            powerio::PioValue::McAcOpfSolution(solution) => {
                powerio::PioValue::McAcOpfInstance(solution.instance().clone())
            }
            powerio::PioValue::AcScucSolution(solution) => {
                powerio::PioValue::AcScucInstance(solution.instance().clone())
            }
            other => {
                return Err(PyTypeError::new_err(format!(
                    "{} is not a calculation solution",
                    other.type_name()
                )));
            }
        };
        self.child(value)
    }

    /// The source neutral scheduling, reserve, and contingency inputs of an
    /// AC security constrained unit commitment instance.
    fn _ac_scuc_inputs(&self) -> PyResult<PyScucInputs> {
        Ok(PyScucInputs {
            inner: self.ac_scuc_instance()?.inputs().clone(),
        })
    }

    /// How an AC security constrained unit commitment calculation ended.
    fn _ac_scuc_solution_termination(&self) -> PyResult<&'static str> {
        match self.ac_scuc_solution()?.termination() {
            powerio_prob::Termination::Converged => Ok("converged"),
            powerio_prob::Termination::IterationLimit => Ok("iteration_limit"),
            powerio_prob::Termination::Infeasible => Ok("infeasible"),
            powerio_prob::Termination::Unbounded => Ok("unbounded"),
            powerio_prob::Termination::Failed => Ok("failed"),
            powerio_prob::Termination::NotReported => Ok("not_reported"),
            _ => Err(PyValueError::new_err(
                "this powerio build returned an unsupported termination value",
            )),
        }
    }

    /// Numerical residuals reported with an AC SCUC solution.
    fn _ac_scuc_solution_residuals(&self) -> PyResult<PyResiduals> {
        Ok(PyResiduals {
            inner: *self.ac_scuc_solution()?.residuals(),
        })
    }

    /// Producer or solver identity recorded with an AC SCUC solution.
    fn _ac_scuc_solution_producer(&self) -> PyResult<Option<String>> {
        Ok(self.ac_scuc_solution()?.producer().map(str::to_owned))
    }

    /// Per interval network outputs from an AC SCUC solution.
    fn _ac_scuc_solution_network_outputs(&self) -> PyResult<PyScucNetworkOutputs> {
        Ok(PyScucNetworkOutputs {
            inner: self.ac_scuc_solution()?.network_outputs().clone(),
        })
    }

    /// Per interval dispatchable device outputs from an AC SCUC solution.
    fn _ac_scuc_solution_device_outputs(&self) -> PyResult<PyScucDeviceOutputs> {
        Ok(PyScucDeviceOutputs {
            inner: self.ac_scuc_solution()?.device_outputs().clone(),
        })
    }

    /// Objective value reported with an AC SCUC solution.
    fn _ac_scuc_solution_objective(&self) -> PyResult<Option<f64>> {
        Ok(self.ac_scuc_solution()?.objective())
    }

    /// Canonical structural type name. This stays private to the wrapper;
    /// callers use `isinstance(module.value, ...)`.
    #[getter]
    fn _type_name(&self) -> PyResult<String> {
        Ok(self.module()?.value().type_name().to_owned())
    }

    /// The module's diagnostics, in encounter order.
    #[getter]
    fn diagnostics(&self) -> PyResult<Vec<PyDiagnostic>> {
        Ok(self
            .module()?
            .diagnostics
            .iter()
            .map(PyDiagnostic::from)
            .collect())
    }

    /// Validate and apply one homogeneous batch of typed updates atomically.
    fn _apply_updates(&mut self, updates: &Bound<'_, PyAny>) -> PyResult<PyUpdateReport> {
        let batch = extract_update_batch(updates)?;
        if matches!(batch, PyUpdateBatch::Empty) {
            return Ok(PyUpdateReport {
                inner: powerio_prob::UpdateReport::default(),
            });
        }
        let source = self.module()?;
        let (updated, report) = apply_update_batch(source, &batch)?;
        self.module = Some(updated);
        Ok(PyUpdateReport { inner: report })
    }

    /// Replace one bus's aggregate active demand through a named allocation
    /// rule owned by PowerIO.
    #[pyo3(signature = (bus_id, total, *, allocation="proportional_to_current_active_power"))]
    fn _apply_bus_load_active_power(
        &mut self,
        bus_id: usize,
        total: &PyActivePower,
        allocation: &str,
    ) -> PyResult<PyUpdateReport> {
        let (updated, report) =
            apply_bus_load_active_power_to_module(self.module()?, bus_id, total.inner, allocation)?;
        self.module = Some(updated);
        Ok(PyUpdateReport { inner: report })
    }

    /// Apply updates to one collection entry while retaining the collection.
    #[pyo3(signature = (updates, *, time_index=None, scenario_id=None))]
    fn _apply_collection_updates(
        &mut self,
        updates: &Bound<'_, PyAny>,
        time_index: Option<usize>,
        scenario_id: Option<&str>,
    ) -> PyResult<PyUpdateReport> {
        if time_index.is_none() && scenario_id.is_none() {
            return Err(PyTypeError::new_err(
                "a collection entry update requires a time index or scenario ID",
            ));
        }
        let batch = extract_update_batch(updates)?;
        if matches!(batch, PyUpdateBatch::Empty) {
            return Ok(PyUpdateReport {
                inner: powerio_prob::UpdateReport::default(),
            });
        }
        let (updated, report) =
            apply_collection_entry_updates(self.module()?, time_index, scenario_id, &batch)?;
        self.module = Some(updated);
        Ok(PyUpdateReport { inner: report })
    }

    fn _time_series_len(&self) -> PyResult<usize> {
        let powerio::PioValue::TimeSeries(series) = &self.module()?.value() else {
            return Err(PowerIODataError::new_err(
                "the module value is not a TimeSeries",
            ));
        };
        Ok(series.len())
    }

    fn _time_series_points(&self) -> PyResult<Vec<(String, Option<f64>)>> {
        let powerio::PioValue::TimeSeries(series) = &self.module()?.value() else {
            return Err(PowerIODataError::new_err(
                "the module value is not a TimeSeries",
            ));
        };
        Ok(series
            .time_points()
            .iter()
            .map(|point| {
                (
                    point.label().to_owned(),
                    point.duration().map(|duration| duration.as_secs_f64()),
                )
            })
            .collect())
    }

    fn _time_series_get(&self, index: usize) -> PyResult<Self> {
        let powerio::PioValue::TimeSeries(series) = &self.module()?.value() else {
            return Err(PowerIODataError::new_err(
                "the module value is not a TimeSeries",
            ));
        };
        let value = series
            .get(index)
            .ok_or_else(|| PyValueError::new_err("time series index out of range"))?;
        self.child(value.clone())
    }

    fn _scenario_entries(&self) -> PyResult<Vec<(String, Option<f64>)>> {
        let powerio::PioValue::ScenarioSet(scenarios) = &self.module()?.value() else {
            return Err(PowerIODataError::new_err(
                "the module value is not a ScenarioSet",
            ));
        };
        Ok(scenarios
            .iter()
            .map(|scenario| (scenario.id().as_str().to_owned(), scenario.probability()))
            .collect())
    }

    fn _scenario_get(&self, id: &str) -> PyResult<Self> {
        let powerio::PioValue::ScenarioSet(scenarios) = &self.module()?.value() else {
            return Err(PowerIODataError::new_err(
                "the module value is not a ScenarioSet",
            ));
        };
        let value = scenarios
            .get(id)
            .ok_or_else(|| PyValueError::new_err(format!("unknown scenario ID {id:?}")))?;
        self.child(value.clone())
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
        let powerio::PioValue::MulticonductorNetwork(network) = &source.value() else {
            let Err(error) = powerio::transform::to_balanced_report(
                source,
                powerio::transform::MulticonductorToBalancedOptions {
                    base_mva,
                    ..Default::default()
                },
            ) else {
                unreachable!("a non-multiconductor value cannot pass the type check")
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
        let powerio::PioValue::BalancedNetwork(network) = &module.value() else {
            return Err(PowerIODataError::new_err(format!(
                "the module carries a {} value; as_balanced_network takes a balanced network",
                module.value().type_name()
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
        let powerio::PioValue::MulticonductorNetwork(network) = &module.value() else {
            return Err(PowerIODataError::new_err(format!(
                "the module carries a {} value; as_multiconductor_network takes a \
                 multiconductor network",
                module.value().type_name()
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
                "PioModule(value={}, diagnostics={}, history={})",
                module.value().type_name(),
                module.diagnostics.len(),
                module.history().len()
            ),
            None => "PioModule(<consumed>)".to_owned(),
        }
    }
}

/// Classify top level JSON markers. Returns `(status, domain, format)`.
///
/// `status` is `known` for a case document, or the classification family
/// itself for the outcomes that are not one: `module` (PowerIO IR),
/// `model-json` (bare balanced model JSON, read with
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
    let parsed = powerio::GeoLayer::parse(text, name_hint)
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

#[pymodule]
fn _powerio(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("PowerIOError", m.py().get_type::<PowerIOError>())?;
    m.add("PowerIOParseError", m.py().get_type::<PowerIOParseError>())?;
    m.add("PowerIODataError", m.py().get_type::<PowerIODataError>())?;
    m.add_class::<PyBalancedNetwork>()?;
    m.add_function(wrap_pyfunction!(parse_display, m)?)?;
    m.add_class::<PyMulticonductorNetwork>()?;
    m.add_class::<PyPioModule>()?;
    m.add_class::<PyComponentId>()?;
    m.add_class::<PyScucStartupCostAdjustment>()?;
    m.add_class::<PyScucStartupLimit>()?;
    m.add_class::<PyScucEnergyRequirement>()?;
    m.add_class::<PyScucInitialCommitment>()?;
    m.add_class::<PyScucRampLimits>()?;
    m.add_class::<PyScucReserveLimits>()?;
    m.add_class::<PyScucReactiveCapability>()?;
    m.add_class::<PyScucEnergyCostBlock>()?;
    m.add_class::<PyScucReserveCosts>()?;
    m.add_class::<PyScucDevicePeriod>()?;
    m.add_class::<PyScucDevice>()?;
    m.add_class::<PyScucShunt>()?;
    m.add_class::<PyScucBranchSwitchingCost>()?;
    m.add_class::<PyScucTransformerControl>()?;
    m.add_class::<PyScucActiveReserveZone>()?;
    m.add_class::<PyScucReactiveReserveZone>()?;
    m.add_class::<PyScucContingency>()?;
    m.add_class::<PyScucViolationCosts>()?;
    m.add_class::<PyScucInputs>()?;
    m.add_class::<PyResiduals>()?;
    m.add_class::<PyScucNetworkOutputs>()?;
    m.add_class::<PyScucDeviceOutputs>()?;
    m.add_class::<PyActivePower>()?;
    m.add_class::<PyReactivePower>()?;
    m.add_class::<PyApparentPower>()?;
    m.add_class::<PyOperatingPointUpdate>()?;
    m.add_class::<PyNetworkUpdate>()?;
    m.add_class::<PyCalculationUpdate>()?;
    m.add_class::<PyUpdateChange>()?;
    m.add_class::<PyUpdateReport>()?;
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
        let retained = Source::from_memory("case.m", b"test".to_vec()).unwrap();
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
        assert_eq!(copied.diagnostics.len(), source.diagnostics.len());
        assert_eq!(copied.diagnostics[0].code(), source.diagnostics[0].code());
        assert_eq!(copied.history(), source.history());
        assert_eq!(copied.extensions(), source.extensions());
        assert_eq!(
            copied.source().unwrap().primary_buffer().unwrap().bytes(),
            b"test"
        );
    }
}
