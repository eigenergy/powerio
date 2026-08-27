//! PyO3 extension behind the `powerio` Python package.
//!
//! The extension exposes parsing, writing, conversion, matrices, packages, and
//! problem instances. Parse and conversion values cross as Python dictionaries
//! and strings, so the base package does not import NumPy or SciPy.
//!
//! The matrix methods hand back COO triplets as plain Python lists
//! (`data`, `row`, `col`, `shape`); there is no NumPy at this layer. The
//! Python `powerio` package assembles those into `scipy.sparse` matrices and
//! NetworkX graphs when the corresponding extra is installed.
//!
//! Indices narrow to `i32` to match SciPy's default index width.
//! `coo_triplets` checks the bound before conversion.

use std::collections::BTreeMap;
use std::path::Path;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use sprs::CsMat;

use powerio::package::{
    DiagnosticSeverity, NetworkPackage, OperatingPointSeries, Origin, SourceDescriptor,
};
use powerio_matrix::matrix::{
    BuildOptions, DcConvention, Scheme, SensitivityOptions, SensitivitySolver, build_adjacency,
    build_bdoubleprime, build_bprime, build_incidence, build_lacpf, build_ptdf_lodf_with_options,
    build_weighted_laplacian, build_ybus,
};
use powerio_matrix::{
    BalancedNetwork, DisplayData, IndexCore, IndexedNetwork, MissingGenCostPolicy,
    NormalizeOptions, POWER_MODELS_ANGLE_BOUND_PAD, PwdDisplay, WriteOptions,
};
use powerio_prob::Units;
use powerio_prob::matrix::{
    DcOpfAssemblyOptions, DcOpfBundleMetadata, DcOpfBundleOptions,
    write_dcopf_bundle as write_bundle,
};

#[cfg(feature = "gridfm")]
use powerio_matrix::io::gridfm::{
    GridfmOptions, GridfmOutputs, GridfmRead, numbered_snapshots,
    read_gridfm_dataset as gridfm_read_dataset, read_gridfm_scenarios as gridfm_read_scenarios,
    write_gridfm_batch as gridfm_write_batch, write_gridfm_dataset as gridfm_write_dataset,
};

pyo3::create_exception!(
    powerio,
    PowerIOError,
    pyo3::exceptions::PyValueError,
    "Base error raised by the powerio parser, converter, or matrix builders.\n\n\
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

/// Map a [`powerio_matrix::Error`] onto the right Python exception, driven by
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
    category: powerio_matrix::ErrorCategory,
    code: &'static str,
    msg: String,
) -> PyErr {
    use powerio_matrix::ErrorCategory as C;
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

fn core_pyerr(e: powerio_matrix::CoreError) -> PyErr {
    // Hand I/O to PyO3 by value so it picks the precise `OSError` subclass.
    if let powerio_matrix::CoreError::Io(io) = e {
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
        E::Core(inner) => core_pyerr(inner),
        other => {
            let category = other.category();
            let code = other.code().code;
            categorized_pyerr(category, code, other.to_string())
        }
    }
}

fn prob_pyerr(e: powerio_prob::Error) -> PyErr {
    use powerio_prob::Error as E;
    match e {
        E::Io(io) => io.into(),
        E::Core(inner) => core_pyerr(inner),
        E::Matrix(inner) => to_pyerr(inner),
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

/// Accepts `series`/`series-impedance`, `matpower`/`mp`, and
/// `reactance-only` (case- and separator-insensitive).
fn parse_convention(s: &str) -> PyResult<DcConvention> {
    match normalize(s).as_str() {
        "series" | "seriesimpedance" => Ok(DcConvention::SeriesImpedance),
        "matpower" | "mp" => Ok(DcConvention::Matpower),
        "reactanceonly" => Ok(DcConvention::ReactanceOnly),
        // 0.8 spelled b = 1/x "paper"/"paper-pure" and made it the default.
        // Name its successor: the nearest-looking option, "series", is a
        // different formula, so a caller who guesses gets numbers instead of
        // an error.
        "paper" | "paperpure" | "pure" => Err(PyValueError::new_err(
            "convention 'paper-pure' is now 'reactance-only'; it is no longer \
             the default, and 'series' is a different formula (b = x/(r²+x²))",
        )),
        other => Err(PyValueError::new_err(format!(
            "unknown convention {other:?}; expected 'series', 'matpower', or 'reactance-only'"
        ))),
    }
}

/// PTDF/LODF options from the Python keywords. The solver defaults to `auto`,
/// which is dense below the reduced-dimension threshold and the iterative
/// conjugate gradient path above it — the same policy the CLI `sensitivities`
/// command applies, so a very large case cannot force the dense n×n
/// factorization from Python.
fn sensitivity_options(
    convention: Option<&str>,
    solver: Option<&str>,
) -> PyResult<SensitivityOptions> {
    let convention = parse_convention(convention.unwrap_or("series"))?;
    let solver = match normalize(solver.unwrap_or("auto")).as_str() {
        "auto" => SensitivitySolver::Auto,
        "dense" => SensitivitySolver::Dense,
        "iterative" | "cg" => SensitivitySolver::Iterative,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown solver {other:?}; expected 'auto', 'dense', or 'iterative'"
            )));
        }
    };
    Ok(SensitivityOptions {
        convention,
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
            powerio_matrix::parse_gen_cost_csv(&text).map_err(core_pyerr)?
        }
        None => Vec::new(),
    };
    Ok(WriteOptions {
        missing_gen_cost,
        gen_cost_patches,
    })
}

fn package_pyerr(e: powerio::package::Error) -> PyErr {
    // A package failure is a PowerIOError like every other failure. It used to
    // raise a bare `ValueError` for everything, so `except powerio.PowerIOError`
    // did not catch it, and every distinct cause read as one opaque string.
    // A wrapped failure raises what it would have raised on its own: same
    // class, and `e.filename` still set on a missing file.
    match e {
        powerio::package::Error::Core(inner) => core_pyerr(inner),
        powerio::package::Error::Multiconductor(inner) => dist_to_pyerr(inner),
        other => categorized_pyerr(other.category(), other.code(), other.to_string()),
    }
}

/// A JSON serialization failure, raised the way every other package failure is.
///
/// Only for JSON this crate wrote. The blanket `From<serde_json::Error>` names
/// every failure `Error::Serialize`, whose category is `Output`, so a document
/// the caller handed us must go through [`payload_pyerr`] instead of blaming
/// our own writer for it.
fn serialize_pyerr(e: serde_json::Error) -> PyErr {
    package_pyerr(e.into())
}

/// A failure reading JSON the caller supplied, raised as a data failure so
/// `except powerio.PowerIOParseError` catches it and a consumer branching on
/// the category fixes its input rather than retrying a write.
fn payload_pyerr(e: serde_json::Error) -> PyErr {
    package_pyerr(powerio::package::Error::Payload(e.to_string()))
}

fn package_to_json(pkg: &NetworkPackage) -> PyResult<String> {
    let text = pkg.to_json_pretty().map_err(package_pyerr)?;
    NetworkPackage::from_json(&text).map_err(package_pyerr)?;
    Ok(text)
}

fn package_warning_messages(pkg: &NetworkPackage) -> Vec<String> {
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

fn package_to_balanced_py(pkg: &NetworkPackage) -> PyResult<PyBalancedNetwork> {
    let warnings = package_warning_messages(pkg);
    let inner = pkg
        .as_balanced()
        .ok_or_else(|| PyValueError::new_err("package model_kind is not balanced"))?
        .clone();
    let _ = warnings;
    Ok(case_from_parts(
        inner,
        pkg.diagnostics.iter().cloned().map(Into::into).collect(),
    ))
}

fn package_to_dist_py(pkg: &NetworkPackage) -> PyResult<PyMulticonductorNetwork> {
    let net = pkg
        .as_multiconductor()
        .ok_or_else(|| PyValueError::new_err("package model_kind is not multiconductor"))?
        .clone();
    Ok(PyMulticonductorNetwork::from_network(net, Vec::new()))
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
    pkg: &mut NetworkPackage,
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
    use powerio_matrix::format::routing::{Detection, Domain, JsonClass};
    match powerio_matrix::format::routing::classify_json_text(&text) {
        JsonClass::Package => Err(PyValueError::new_err(format!(
            "{} is a .pio.json package; read it with powerio.Package.from_json",
            input.display()
        ))),
        JsonClass::ModelJson => Err(PyValueError::new_err(format!(
            "{} is bare powerio model JSON, which is not a case format; read it with \
             powerio.from_json",
            input.display()
        ))),
        JsonClass::Case(Detection::Known(format)) => Ok(format.domain() == Domain::Distribution),
        JsonClass::Case(Detection::Unknown) => Ok(false),
        JsonClass::Case(Detection::Ambiguous) => Err(PyValueError::new_err(format!(
            "ambiguous JSON markers in {}; pass from_",
            input.display()
        ))),
    }
}

/// Whether a format token names the DOE GO Challenge 3 document, whose time
/// series lift into the package's operating points. TEMPORARY alongside
/// `parse_goc3_json`: the calculation instance types replace this extraction.
fn format_is_goc3(token: &str) -> bool {
    normalize(token) == "goc3" || normalize(token) == "goc3json"
}

/// Package a GO Challenge 3 document: the balanced snapshot plus the operating
/// point series the document carries.
fn goc3_package(text: &str) -> PyResult<NetworkPackage> {
    let (network, diagnostics, document) =
        powerio_matrix::format::parse_goc3_json(text).map_err(core_pyerr)?;
    let mut pkg = NetworkPackage::from_balanced_with_read_diagnostics(
        network,
        diagnostics.into_iter().map(Into::into),
    );
    pkg.attach_goc3_operating_points(&document);
    Ok(pkg)
}

fn build_package_from_path(
    input: &Path,
    from_: Option<&str>,
    scenario: i64,
) -> PyResult<NetworkPackage> {
    let reads_gridfm = from_.is_some_and(format_is_gridfm)
        || (from_.is_none() && input.is_dir() && looks_like_gridfm_dir(input));
    if reads_gridfm {
        #[cfg(feature = "gridfm")]
        {
            let read = gridfm_read_dataset(input.to_string_lossy().as_ref(), scenario)
                .map_err(to_pyerr)?;
            let mut pkg = NetworkPackage::from_balanced_with_read_diagnostics(
                read.network,
                read.diagnostics.into_iter().map(Into::into),
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
        let source = dist_source_from_path(input, from_, None)?;
        let module = powerio_dist::parse(source).map_err(|error| core_error_pyerr(&error))?;
        let format = module
            .value()
            .source_format()
            .map(powerio_dist::DistSourceFormat::name)
            .or(from_)
            .unwrap_or("unknown");
        let retained_source = module.source().is_some();
        let mut pkg = NetworkPackage::from_multiconductor_module(module);
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

    let declared_goc3 = from_.is_some_and(format_is_goc3);
    let sniffed_goc3 = from_.is_none()
        && input.extension().and_then(|e| e.to_str()) == Some("json")
        && matches!(
            std::fs::read_to_string(input)
                .ok()
                .map(|text| powerio_matrix::format::routing::classify_json_text(&text)),
            Some(powerio_matrix::format::routing::JsonClass::Case(
                powerio_matrix::format::routing::Detection::Known(format)
            )) if format_is_goc3(format.name())
        );
    if declared_goc3 || sniffed_goc3 {
        let text = std::fs::read_to_string(input).map_err(PyErr::from)?;
        let mut pkg = goc3_package(&text)?;
        set_package_source(
            &mut pkg,
            input,
            package_source_kind(input, "goc3-json"),
            "goc3-json",
            false,
        );
        pkg.run_sane_validation();
        return Ok(pkg);
    }

    let module = py_parse_module_path(input, from_)?;
    let format = module.value().source_format().name();
    let retained_source = module.source().is_some();
    let mut pkg = NetworkPackage::from_balanced_module(module);
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

fn build_package_from_str(text: &str, from_: Option<&str>) -> PyResult<NetworkPackage> {
    if from_.is_some_and(format_is_gridfm) {
        return Err(PyValueError::new_err(
            "gridfm input is a dataset directory; provide a path",
        ));
    }

    let source_format = if let Some(format) = from_ {
        Some(format.to_owned())
    } else {
        use powerio_matrix::format::routing::{Detection, JsonClass};
        match powerio_matrix::format::routing::classify_json_text(text) {
            JsonClass::Package => {
                return Err(PyValueError::new_err(
                    "text is a .pio.json package; read it with \
                     powerio.Package.from_json",
                ));
            }
            JsonClass::ModelJson => {
                return Err(PyValueError::new_err(
                    "text is bare powerio model JSON, which is not a case format; \
                     read it with powerio.from_json",
                ));
            }
            JsonClass::Case(Detection::Known(format)) => Some(format.name().to_owned()),
            JsonClass::Case(Detection::Ambiguous) => {
                return Err(PyValueError::new_err("ambiguous JSON markers; pass from_"));
            }
            JsonClass::Case(Detection::Unknown) => None,
        }
    };

    if let Some(format) = source_format.as_deref() {
        if format_is_distribution(format) {
            let source = dist_source_from_bytes(text.as_bytes(), format)?;
            let module = powerio_dist::parse(source).map_err(|error| core_error_pyerr(&error))?;
            let mut pkg = NetworkPackage::from_multiconductor_module(module);
            pkg.run_sane_validation();
            return Ok(pkg);
        }
    }

    if source_format.as_deref().is_some_and(format_is_goc3) {
        let mut pkg = goc3_package(text)?;
        pkg.run_sane_validation();
        return Ok(pkg);
    }

    let module = py_parse_module_bytes(
        text.as_bytes(),
        source_format.as_deref().unwrap_or("matpower"),
    )?;
    let mut pkg = NetworkPackage::from_balanced_module(module);
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

/// Low level handle around a parsed [`BalancedNetwork`]. The public `powerio.BalancedNetwork`
/// (pure Python) wraps this: the IO getters and topology methods delegate
/// straight to it, and the matrix methods turn its COO tuples into scipy.
///
/// The derived [`IndexCore`] is built once and cached alongside `inner`, so the
/// matrix builders and topology getters reuse it instead of rebuilding the
/// bus-id map per call.
#[pyclass(name = "_BalancedNetwork", module = "powerio._powerio")]
pub struct PyBalancedNetwork {
    /// The parsed module: the typed network plus retained source and the
    /// reader's findings. Same format writes echo the retained bytes exactly;
    /// a handle built from a bare network writes canonically.
    module: powerio_core::PioModule<BalancedNetwork>,
    core: IndexCore,
    /// The findings as `CODE: message` lines, rendered once at construction.
    warnings: Vec<String>,
}

impl PyBalancedNetwork {
    fn inner(&self) -> &BalancedNetwork {
        self.module.value()
    }

    fn diagnostics(&self) -> &[powerio_matrix::diagnostics::Diagnostic] {
        self.module.diagnostics()
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

fn py_parse_module_path(
    path: &std::path::Path,
    from: Option<&str>,
) -> PyResult<powerio_core::PioModule<powerio_matrix::BalancedNetwork>> {
    let mut source = powerio_core::Source::open(path).map_err(|e| core_open_pyerr(path, &e))?;
    if let Some(token) = from {
        source = source.with_format(
            powerio_core::FormatId::new(token.to_ascii_lowercase().replace('_', "-"))
                .map_err(|e| core_error_pyerr(&e))?,
        );
    }
    // The parse can hit the filesystem lazily (a directory source acquires
    // member files on demand), so its failures get the same OSError
    // unwrapping as the open.
    powerio_matrix::parse(source).map_err(|e| core_open_pyerr(path, &e))
}

fn py_parse_module_bytes(
    bytes: &[u8],
    from: &str,
) -> PyResult<powerio_core::PioModule<powerio_matrix::BalancedNetwork>> {
    let source = powerio_core::Source::from_bytes("<memory>", bytes.to_vec())
        .map_err(|e| core_error_pyerr(&e))?
        .with_format(
            powerio_core::FormatId::new(from.to_ascii_lowercase().replace('_', "-"))
                .map_err(|e| core_error_pyerr(&e))?,
        );
    powerio_matrix::parse(source).map_err(|e| core_error_pyerr(&e))
}

/// Wrap a parsed module as a `PyBalancedNetwork`, building the index core once
/// and keeping the reader's findings on the handle.
fn case_from_module(module: powerio_core::PioModule<BalancedNetwork>) -> PyBalancedNetwork {
    let core = IndexCore::build(module.value());
    PyBalancedNetwork {
        core,
        warnings: powerio_matrix::diagnostics::render_diagnostics(module.diagnostics()),
        module,
    }
}

/// Wrap a bare network with findings: derived handles carry no retained
/// source and write canonically.
fn case_from_parts(
    network: BalancedNetwork,
    diagnostics: Vec<powerio_matrix::diagnostics::Diagnostic>,
) -> PyBalancedNetwork {
    let mut module = powerio_core::PioModule::new(network);
    for diagnostic in diagnostics {
        module
            .add_diagnostic(diagnostic)
            .expect("derived findings carry no identities or spans to collide");
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
    fn source_format(&self) -> String {
        self.inner().source_format().name().to_owned()
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

    #[getter]
    fn n_loads(&self) -> usize {
        self.inner().loads().len()
    }

    #[getter]
    fn n_shunts(&self) -> usize {
        self.inner().shunts().len()
    }

    #[getter]
    fn is_radial(&self) -> bool {
        IndexedNetwork::with_core(self.inner(), &self.core).is_radial()
    }

    #[getter]
    fn n_connected_components(&self) -> usize {
        IndexedNetwork::with_core(self.inner(), &self.core).n_connected_components()
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
            let charging = br.terminal_charging();
            d.set_item("b", br.total_charging_b())?;
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

    fn connectivity_report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let r = IndexedNetwork::with_core(self.inner(), &self.core).connectivity_report();
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
        powerio_matrix::write_as(&self.module, powerio_matrix::TargetFormat::Matpower)
            .map(|conv| conv.text)
            .map_err(|error| core_error_pyerr(&error))
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
    ) -> PyResult<(String, Vec<String>)> {
        let target = to
            .parse::<powerio_matrix::TargetFormat>()
            .map_err(core_pyerr)?;
        let opts = write_options(
            missing_gen_cost,
            default_gen_cost,
            gen_cost_csv,
            MissingGenCostPolicy::Preserve,
        )?;
        let conv = powerio_matrix::write_as_with_options(&self.module, target, &opts)
            .map_err(|e| core_error_pyerr(&e))?;
        let rendered = conv.rendered_diagnostics();
        Ok((conv.text, rendered))
    }

    /// Serialize this case to `to`, bypassing source echo for the same
    /// format. Returns `(text, warnings)`.
    fn to_canonical_format(&self, to: &str) -> PyResult<(String, Vec<String>)> {
        let target = to
            .parse::<powerio_matrix::TargetFormat>()
            .map_err(core_pyerr)?;
        let conv = self
            .inner()
            .to_canonical_format(target)
            .map_err(core_pyerr)?;
        let rendered = conv.rendered_diagnostics();
        Ok((conv.text, rendered))
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
    ) -> PyResult<Vec<String>> {
        let (text, warnings) =
            self.to_format(to, missing_gen_cost, default_gen_cost, gen_cost_csv)?;
        commit_text_file(std::path::Path::new(path), text.into_bytes())?;
        Ok(warnings)
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

    // --- matrix builders: each returns a COO tuple ----------------------

    /// MATPOWER FDPF Bp matrix.
    #[pyo3(signature = (scheme=None))]
    fn bprime<'py>(&self, py: Python<'py>, scheme: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
        let opts = BuildOptions {
            scheme: parse_scheme(scheme.unwrap_or("bx"))?,
            ..BuildOptions::default()
        };
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = build_bprime(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    /// MATPOWER FDPF Bpp matrix.
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
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = build_bdoubleprime(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    #[pyo3(signature = (*, include_taps=true, include_shifts=true))]
    fn lacpf<'py>(
        &self,
        py: Python<'py>,
        include_taps: bool,
        include_shifts: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_options(Scheme::Bx, include_taps, include_shifts);
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = build_lacpf(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    fn adjacency<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = build_adjacency(&view).map_err(to_pyerr)?;
        coo_triplets(py, &m)
    }

    /// `(Re(Y_bus), Im(Y_bus))` as two COO tuples.
    #[pyo3(signature = (*, include_taps=true, include_shifts=true))]
    fn ybus_parts<'py>(
        &self,
        py: Python<'py>,
        include_taps: bool,
        include_shifts: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = build_options(Scheme::Bx, include_taps, include_shifts);
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let yb = build_ybus(&view, &opts).map_err(to_pyerr)?;
        let g = coo_triplets(py, &yb.g)?;
        let b = coo_triplets(py, &yb.b)?;
        Ok((g, b).into_pyobject(py)?.into_any())
    }

    #[pyo3(signature = (convention=None, solver=None))]
    fn ptdf<'py>(
        &self,
        py: Python<'py>,
        convention: Option<&str>,
        solver: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = sensitivity_options(convention, solver)?;
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = build_ptdf_lodf_with_options(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m.ptdf)
    }

    #[pyo3(signature = (convention=None, solver=None))]
    fn lodf<'py>(
        &self,
        py: Python<'py>,
        convention: Option<&str>,
        solver: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let opts = sensitivity_options(convention, solver)?;
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let m = build_ptdf_lodf_with_options(&view, &opts).map_err(to_pyerr)?;
        coo_triplets(py, &m.lodf)
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
        let conv = parse_convention(convention.unwrap_or("series"))?;
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
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
        let conv = parse_convention(convention.unwrap_or("series"))?;
        let view = IndexedNetwork::with_core(self.inner(), &self.core);
        let parts = build_incidence(&view, conv, &BuildOptions::default()).map_err(to_pyerr)?;
        let l = build_weighted_laplacian(&parts.a, &parts.b);
        coo_triplets(py, &l)
    }

    /// This network's coordinates as the canonical GeoJSON layer. Raises when
    /// the network carries none.
    fn geo_layer_json(&self) -> PyResult<String> {
        self.inner()
            .geo_layer()
            .extracted_geojson()
            .map_err(|error| PyValueError::new_err(error.to_string()))
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
        let parsed = powerio_matrix::geo::GeoLayer::parse_bytes(text.as_bytes(), name_hint)
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
        let mut policy_network = self.inner().clone();
        let cost_report = policy_network
            .apply_gen_cost_policy(&cost_opts.gen_cost_patches, cost_opts.missing_gen_cost)
            .map_err(core_pyerr)?;
        let instance = powerio_prob::DcOpfInstance::from_network(policy_network)
            .map_err(|error| core_error_pyerr(&error))?
            .with_approximation(parse_convention(convention.unwrap_or("series"))?);
        let mut assembly = DcOpfAssemblyOptions::default();
        assembly.units = parse_units(units.unwrap_or("perunit"))?;
        let options = DcOpfBundleOptions {
            assembly,
            metadata: DcOpfBundleMetadata {
                cost_policy: cost_opts.missing_gen_cost,
                cost_report,
            },
        };
        let outputs = write_bundle(&instance, out_dir, &options).map_err(prob_pyerr)?;
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
            gridfm_write_dataset(self.inner(), scenario, out_dir, &opts).map_err(to_pyerr)?;
        gridfm_outputs_to_dict(py, &outputs)
    }

    /// Write this case as a PyPSA CSV folder. Returns
    /// `{"dir", "files", "warnings"}`.
    fn write_pypsa_csv_folder<'py>(
        &self,
        py: Python<'py>,
        out_dir: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let outputs = powerio_matrix::write_pypsa_csv_folder(self.inner(), out_dir)
            .map_err(|error| core_error_pyerr(&error))?;
        pypsa_outputs_to_dict(py, &outputs)
    }

    fn __repr__(&self) -> String {
        format!(
            "BalancedNetwork(name={:?}, n_buses={}, n_branches={}, n_gens={})",
            self.inner().name(),
            self.inner().buses().len(),
            self.inner().branches().len(),
            self.inner().generators().len()
        )
    }
}

/// Parse a case file from a path, inferring the format from the extension unless
/// `from_` is given.
#[pyfunction]
#[pyo3(signature = (path, from_=None))]
fn parse_file(path: &str, from_: Option<&str>) -> PyResult<PyBalancedNetwork> {
    py_parse_module_path(std::path::Path::new(path), from_).map(case_from_module)
}

/// Parse a case from in-memory text in the named source format `from_`
/// (`matpower`, `powermodels-json`, `egret-json`, `pandapower-json`, `psse`,
/// `powerworld`, `pslf`, `goc3-json`, `surge-json`; aliases
/// `m`/`pm`/`egret`/`pp`/`raw`/`aux`/`epc`/`goc3`/`surge`).
#[pyfunction]
#[pyo3(signature = (text, from_))]
fn parse_str(text: &str, from_: &str) -> PyResult<PyBalancedNetwork> {
    py_parse_module_bytes(text.as_bytes(), from_).map(case_from_module)
}

/// Parse a case from in-memory bytes in the named source format `from_`.
/// Accepts every `parse_str` name plus `pwb`: PowerWorld binary has no text
/// form, so this is the only in-memory way to read one. Text formats must be
/// UTF-8.
#[pyfunction]
#[pyo3(signature = (data, from_))]
fn parse_bytes(data: &[u8], from_: &str) -> PyResult<PyBalancedNetwork> {
    py_parse_module_bytes(data, from_).map(case_from_module)
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
    let display = powerio_matrix::parse_display_file(std::path::Path::new(path), from_)
        .map_err(core_pyerr)?;
    display_data_to_py(py, display)
}

/// Parse display bytes in the named display format `from_`. Returns
/// `(kind, payload)`.
#[pyfunction]
#[pyo3(signature = (data, from_))]
fn parse_display_bytes<'py>(
    py: Python<'py>,
    data: &[u8],
    from_: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let display = powerio_matrix::parse_display_bytes(data, from_).map_err(core_pyerr)?;
    display_data_to_py(py, display)
}

/// Rebuild a case from JSON produced by `BalancedNetwork.to_json()`.
#[pyfunction]
fn from_json(text: &str) -> PyResult<PyBalancedNetwork> {
    let inner = powerio_matrix::BalancedNetwork::from_json(text).map_err(core_pyerr)?;
    Ok(case_from_parts(inner, Vec::new()))
}

/// Read a PyPSA CSV folder into a case.
#[pyfunction]
fn read_pypsa_csv_folder(path: &str) -> PyResult<PyBalancedNetwork> {
    py_parse_module_path(std::path::Path::new(path), Some("pypsa-csv")).map(case_from_module)
}

/// Convert a case file to another format through the network model. Returns
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
) -> PyResult<(String, Vec<String>)> {
    let target = to
        .parse::<powerio_matrix::TargetFormat>()
        .map_err(core_pyerr)?;
    let opts = write_options(
        missing_gen_cost,
        default_gen_cost,
        gen_cost_csv,
        MissingGenCostPolicy::Preserve,
    )?;
    let conv =
        powerio_matrix::convert_file_with_options(std::path::Path::new(path), target, from_, &opts)
            .map_err(|e| core_open_pyerr(std::path::Path::new(path), &e))?;
    if let Some(out) = out {
        commit_text_file(std::path::Path::new(out), conv.text.clone().into_bytes())?;
    }
    let rendered = conv.rendered_diagnostics();
    Ok((conv.text, rendered))
}

/// Convert in-memory case `text` to another format through the network model,
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
) -> PyResult<(String, Vec<String>)> {
    let target = to
        .parse::<powerio_matrix::TargetFormat>()
        .map_err(core_pyerr)?;
    let opts = write_options(
        missing_gen_cost,
        default_gen_cost,
        gen_cost_csv,
        MissingGenCostPolicy::Preserve,
    )?;
    let conv =
        powerio_matrix::convert_str_with_options(text, target, from_.unwrap_or("matpower"), &opts)
            .map_err(|e| core_error_pyerr(&e))?;
    let rendered = conv.rendered_diagnostics();
    Ok((conv.text, rendered))
}

/// Writes `text` to `path` and every sidecar beside it.
///
/// A dss write of a network with bus coordinates emits a `Buscoords <name>`
/// directive and returns the CSV as a sidecar. Writing the text alone leaves
/// a case that names a file which does not exist, and OpenDSS then refuses
/// to compile it. A sidecar path is relative by construction, but
/// `ConversionSidecar::path` is a public field a caller can set, so the path
/// is checked the way the CLI checks it: every component must be a plain
/// name, and a path that reaches out of the output directory is refused.
/// Whether a sidecar path names a file inside the output directory, which is
/// the CLI's `is_relative_component_path` rule: every component must be a
/// plain name.
///
/// Rejecting only an absolute path and `..` leaves three ways out. An empty
/// path makes `join` resolve back to the output directory itself, so the
/// write targets the directory. A Windows drive-relative path such as
/// `C:x.csv` is not absolute, and its prefix component makes `join` discard
/// the directory entirely. A rooted path with no drive letter is not absolute
/// on Windows either. None of the three holds a `..` component.
fn sidecar_stays_in_output_dir(path: &str) -> bool {
    !path.is_empty()
        && std::path::Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// The case file and its sidecars land beside each other in the caller's
/// directory, which may hold unrelated files, so each file commits
/// individually through the no-replace destination and a refusal removes the
/// files this call created, leaving the directory as it was.
fn write_with_sidecars(
    path: &str,
    text: &str,
    sidecars: &[powerio_dist::ConversionSidecar],
) -> PyResult<()> {
    let path = std::path::Path::new(path);
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut committed: Vec<std::path::PathBuf> = Vec::new();
    let mut commit = |target: &std::path::Path, bytes: Vec<u8>| -> PyResult<()> {
        commit_text_file(target, bytes)?;
        committed.push(target.to_path_buf());
        Ok(())
    };
    let result = (|| {
        commit(path, text.as_bytes().to_vec())?;
        for sidecar in sidecars {
            if !sidecar_stays_in_output_dir(&sidecar.path) {
                return Err(PowerIOError::new_err(format!(
                    "refusing to write the sidecar `{}`: the path must stay in the output directory",
                    sidecar.path
                )));
            }
            commit(
                dir.join(&sidecar.path).as_path(),
                sidecar.text.clone().into_bytes(),
            )?;
        }
        Ok(())
    })();
    if result.is_err() {
        for created in &committed {
            let _ = std::fs::remove_file(created);
        }
    }
    result
}

/// Commit one complete text file through the no-replace destination: staged
/// beside the target and moved into place only when no entry exists there.
fn commit_text_file(path: &std::path::Path, bytes: Vec<u8>) -> PyResult<()> {
    let artifact = powerio_core::MemoryArtifact::new(
        powerio_core::ArtifactPath::new("case").expect("static placeholder name"),
        bytes,
    );
    powerio_core::Destination::path(path)
        .__commit_artifacts(false, vec![artifact], Vec::new())
        .map(|_| ())
        .map_err(|error| core_error_pyerr(&error))
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
    /// The findings as `CODE: message` lines, rendered once at construction.
    rendered_warnings: Vec<String>,
}

impl PyMulticonductorNetwork {
    fn inner(&self) -> &powerio_dist::MulticonductorNetwork {
        self.module.value()
    }

    /// A parsed handle: warnings rendered from the module's findings.
    fn from_module(module: powerio_core::PioModule<powerio_dist::MulticonductorNetwork>) -> Self {
        Self {
            rendered_warnings: powerio_dist::diagnostics::render_diagnostics(module.diagnostics()),
            module,
        }
    }

    /// A derived handle: no retained source, so writes are canonical.
    fn from_network(
        net: powerio_dist::MulticonductorNetwork,
        rendered_warnings: Vec<String>,
    ) -> Self {
        Self {
            module: powerio_core::PioModule::new(net),
            rendered_warnings,
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

    /// Parse warnings: everything the reader could not represent or had to
    /// assume.
    fn warnings(&self) -> Vec<String> {
        self.rendered_warnings.clone()
    }

    /// This network's coordinates as the canonical GeoJSON layer. Raises when
    /// the network carries none.
    fn geo_layer_json(&self) -> PyResult<String> {
        powerio::package::dist_geo_layer(self.inner())
            .extracted_geojson()
            .map_err(|error| PyValueError::new_err(error.to_string()))
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
        let parsed = powerio_matrix::geo::GeoLayer::parse_bytes(text.as_bytes(), name_hint)
            .map_err(|error| PowerIOParseError::new_err(error.to_string()))?;
        let mut net = self.inner().clone();
        let report = powerio::package::apply_dist_geo_layer(&mut net, &parsed.layer);
        *net.source_format_mut() = None;
        let mut rendered = self.rendered_warnings.clone();
        rendered.extend(
            parsed
                .diagnostics
                .iter()
                .map(powerio_matrix::diagnostics::render_diagnostic),
        );
        Ok((
            PyMulticonductorNetwork::from_network(net, rendered),
            geo_report_dict(py, &report)?,
        ))
    }

    fn n_buses(&self) -> usize {
        self.inner().buses().len()
    }

    fn n_lines(&self) -> usize {
        self.inner().lines().len()
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

    fn n_sources(&self) -> usize {
        self.inner().sources().len()
    }

    /// Serialize to `to` (`dss`, `pmd-json`, `bmopf-json`). Returns
    /// `(text, warnings)`. Writing back to the source format echoes the
    /// retained source byte for byte.
    fn to_format(&self, to: &str) -> PyResult<(String, Vec<String>)> {
        let target = to
            .parse::<powerio_dist::DistTargetFormat>()
            .map_err(dist_to_pyerr)?;
        let conv = powerio_dist::write_as(&self.module, target);
        {
            let rendered = conv.rendered_diagnostics();
            Ok((conv.text, rendered))
        }
    }

    /// Serialize to `to`, bypassing source echo for the same format.
    fn to_canonical_format(&self, to: &str) -> PyResult<(String, Vec<String>)> {
        let target = to
            .parse::<powerio_dist::DistTargetFormat>()
            .map_err(dist_to_pyerr)?;
        let conv = powerio_dist::write_network(self.inner(), target);
        {
            let rendered = conv.rendered_diagnostics();
            Ok((conv.text, rendered))
        }
    }

    /// Serialize to `to` and write it to `path` exactly as produced (no
    /// newline translation; see `BalancedNetwork.write_file`). Returns the fidelity
    /// warnings.
    fn write_file(&self, path: &str, to: &str) -> PyResult<Vec<String>> {
        let target = to
            .parse::<powerio_dist::DistTargetFormat>()
            .map_err(dist_to_pyerr)?;
        let conv = powerio_dist::write_as(&self.module, target);
        write_with_sidecars(path, &conv.text, &conv.sidecars)?;
        Ok(conv.rendered_diagnostics())
    }

    /// The collapsed bus and terminal graph projection as JSON.
    fn graph_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner().graph())
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

/// Convert a distribution case file to `to`. Returns `(text, warnings)`; the
/// warnings carry both the parse warnings and the writer's fidelity losses.
#[pyfunction]
#[pyo3(signature = (path, to, from_=None))]
fn dist_convert_file(path: &str, to: &str, from_: Option<&str>) -> PyResult<(String, Vec<String>)> {
    let to = to
        .parse::<powerio_dist::DistTargetFormat>()
        .map_err(dist_to_pyerr)?;
    let source = dist_source_from_path(std::path::Path::new(path), from_, None)?;
    let conv =
        powerio_dist::convert_source(source, to).map_err(|error| core_error_pyerr(&error))?;
    {
        let rendered = conv.rendered_diagnostics();
        Ok((conv.text, rendered))
    }
}

/// Convert an in-memory distribution case of the named source format `from_`
/// to `to`. Returns `(text, warnings)`; the warnings carry both the parse
/// warnings and the writer's fidelity losses.
#[pyfunction]
#[pyo3(signature = (text, to, from_))]
fn dist_convert_str(text: &str, to: &str, from_: &str) -> PyResult<(String, Vec<String>)> {
    let to = to
        .parse::<powerio_dist::DistTargetFormat>()
        .map_err(dist_to_pyerr)?;
    let source = dist_source_from_bytes(text.as_bytes(), from_)?;
    let conv =
        powerio_dist::convert_source(source, to).map_err(|error| core_error_pyerr(&error))?;
    {
        let rendered = conv.rendered_diagnostics();
        Ok((conv.text, rendered))
    }
}

/// Low level handle around a parsed `.pio.json` package. Parses the document
/// once; the user facing `powerio.Package` wraps it. Not frozen: `validate`
/// rewrites the handle's diagnostics in place, matching the Rust and C APIs.
#[pyclass(name = "_Package", module = "powerio._powerio")]
struct PyPackage {
    pkg: NetworkPackage,
}

#[pymethods]
impl PyPackage {
    /// Parse `.pio.json` document text into a package handle.
    #[staticmethod]
    fn from_json(text: &str) -> PyResult<Self> {
        NetworkPackage::from_json(text)
            .map_err(package_pyerr)
            .map(|pkg| Self { pkg })
    }

    /// Build a package from a case file or folder.
    #[staticmethod]
    #[pyo3(signature = (path, from_=None, scenario=0))]
    fn from_file(path: &str, from_: Option<&str>, scenario: i64) -> PyResult<Self> {
        build_package_from_path(Path::new(path), from_, scenario).map(|pkg| Self { pkg })
    }

    /// Build a package from in-memory case text.
    #[staticmethod]
    #[pyo3(signature = (text, from_=None))]
    fn from_str(text: &str, from_: Option<&str>) -> PyResult<Self> {
        build_package_from_str(text, from_).map(|pkg| Self { pkg })
    }

    /// Wrap a balanced network handle in a package.
    #[staticmethod]
    #[pyo3(signature = (network, include_solver_metadata=false))]
    fn from_balanced(
        network: PyRef<'_, PyBalancedNetwork>,
        include_solver_metadata: bool,
    ) -> PyResult<Self> {
        let mut pkg = NetworkPackage::from_balanced_with_read_diagnostics(
            network.inner().clone(),
            network.diagnostics().iter().cloned().map(Into::into),
        );
        if include_solver_metadata {
            pkg.attach_normalized_solver_table_metadata()
                .map_err(core_pyerr)?;
        }
        Ok(Self { pkg })
    }

    /// Wrap a multiconductor network handle in a package.
    #[staticmethod]
    fn from_multiconductor(network: PyRef<'_, PyMulticonductorNetwork>) -> Self {
        Self {
            pkg: NetworkPackage::from_multiconductor(network.inner().clone()),
        }
    }

    /// The explicit top level model kind.
    fn model_kind(&self) -> &'static str {
        match self.pkg.model_kind() {
            powerio::package::ModelKind::Balanced => "balanced",
            powerio::package::ModelKind::Multiconductor => "multiconductor",
            _ => "unknown",
        }
    }

    /// Serialize to pretty `.pio.json`.
    fn to_json(&self) -> PyResult<String> {
        package_to_json(&self.pkg)
    }

    /// Rebuild a balanced network handle from the payload.
    fn as_balanced(&self) -> PyResult<PyBalancedNetwork> {
        package_to_balanced_py(&self.pkg)
    }

    /// Rebuild a multiconductor network handle from the payload.
    fn as_multiconductor(&self) -> PyResult<PyMulticonductorNetwork> {
        package_to_dist_py(&self.pkg)
    }

    /// The operating point series as JSON, or `null` when absent.
    fn operating_points_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.pkg.operating_points).map_err(serialize_pyerr)
    }

    /// Replace the operating point series from JSON and rerun validation.
    /// `null` or an empty series clears it.
    fn set_operating_points_json(&mut self, json: &str) -> PyResult<()> {
        let series: Option<OperatingPointSeries> =
            serde_json::from_str(json).map_err(payload_pyerr)?;
        match series {
            Some(series) => self.pkg.set_operating_points(series),
            None => self.pkg.clear_operating_points(),
        }
        self.pkg.run_sane_validation();
        Ok(())
    }

    /// The study block as JSON, or `null` when absent.
    fn study_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.pkg.study).map_err(serialize_pyerr)
    }

    /// Materialize one operating point into a new static package handle.
    fn materialize_operating_point(&self, index: usize) -> PyResult<Self> {
        self.pkg
            .materialize_operating_point(index)
            .map_err(package_pyerr)
            .map(|pkg| Self { pkg })
    }

    /// Materialize one study commit into a new static package handle.
    fn materialize_study_commit(&self, index: usize) -> PyResult<Self> {
        self.pkg
            .materialize_study_commit(index)
            .map_err(package_pyerr)
            .map(|pkg| Self { pkg })
    }

    /// Run the package semantic validation profile in place.
    fn validate(&mut self) {
        self.pkg.run_sane_validation();
    }

    /// The validation summary as JSON.
    fn validation_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.pkg.validation).map_err(serialize_pyerr)
    }

    /// The structured diagnostics array as JSON.
    fn diagnostics_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.pkg.diagnostics).map_err(serialize_pyerr)
    }

    /// Readiness report for multiconductor to balanced lowering, as JSON.
    #[pyo3(signature = (base_mva=100.0))]
    fn multiconductor_to_balanced_preflight_json(&self, base_mva: f64) -> PyResult<String> {
        let net = self.pkg.as_multiconductor().ok_or_else(|| {
            PyValueError::new_err(format!(
                "multiconductor preflight requires a multiconductor package, got {}",
                self.model_kind()
            ))
        })?;
        let report = powerio::package::check_multiconductor_to_balanced_lowering(
            net,
            powerio::package::MulticonductorToBalancedOptions {
                base_mva,
                ..Default::default()
            },
        );
        serde_json::to_string(&report).map_err(serialize_pyerr)
    }

    /// Lower a multiconductor package to a new balanced package handle.
    #[pyo3(signature = (base_mva=100.0))]
    fn lower_multiconductor_to_balanced(&self, base_mva: f64) -> PyResult<Self> {
        self.pkg
            .lower_multiconductor_to_balanced(powerio::package::MulticonductorToBalancedOptions {
                base_mva,
                ..Default::default()
            })
            .map_err(|e| PowerIODataError::new_err(e.to_string()))
            .map(|pkg| Self { pkg })
    }

    fn __repr__(&self) -> String {
        format!(
            "Package(model_kind={}, diagnostics={}, operating_points={})",
            self.model_kind(),
            self.pkg.diagnostics.len(),
            self.pkg.operating_points().map_or(0, |s| s.points.len())
        )
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
    use powerio_matrix::format::routing::{Detection, JsonClass};
    let class = powerio_matrix::format::routing::classify_json_text(text);
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
    powerio_matrix::format::routing::JSON_CLASSES
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// Tolerant read of a geographic sidecar (headerless buscoords CSV, aliased
/// CSV/JSON records, GeoJSON): returns `{"geojson": <canonical form>,
/// "warnings": [...]}`. `name_hint` (a file name) picks CSV against JSON;
/// otherwise the content is sniffed.
#[pyfunction(signature = (text, name_hint = None))]
fn parse_geo<'py>(
    py: Python<'py>,
    text: &str,
    name_hint: Option<&str>,
) -> PyResult<Bound<'py, PyDict>> {
    let parsed = powerio_matrix::geo::GeoLayer::parse_bytes(text.as_bytes(), name_hint)
        .map_err(|error| PowerIOParseError::new_err(error.to_string()))?;
    let out = PyDict::new(py);
    out.set_item("geojson", parsed.layer.to_geojson())?;
    out.set_item("warnings", parsed.warnings)?;
    Ok(out)
}

/// A `{matched_buses, matched_branches, unmatched_features, unlocated_buses,
/// unlocated_branches, notes}` dict from one geo apply pass.
fn geo_report_dict<'py>(
    py: Python<'py>,
    report: &powerio_matrix::geo::GeoApplyReport,
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

/// Parse SCOPF source text and return its language neutral document as JSON.
#[pyfunction(signature = (text, from_ = "goc3-json", *, index_base = 0))]
fn parse_scopf(text: &str, from_: &str, index_base: i32) -> PyResult<String> {
    let index_base = match index_base {
        0 => powerio_prob::IndexBase::Zero,
        1 => powerio_prob::IndexBase::One,
        other => {
            return Err(PyValueError::new_err(format!(
                "index_base must be 0 or 1, got {other}"
            )));
        }
    };
    let instance = powerio_prob::parse_scopf_str(text, from_).map_err(|error| match error {
        error @ powerio_prob::ScopfError::UnsupportedFormat(_) => {
            PyValueError::new_err(error.to_string())
        }
        error => PowerIOParseError::new_err(error.to_string()),
    })?;
    powerio_prob::scopf::json::to_json_with_index_base(&instance, index_base)
        .map_err(|error| PowerIOParseError::new_err(error.to_string()))
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
    d.set_item("warnings", outputs.rendered_diagnostics())?;
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
    let net_refs: Vec<_> = cases.iter().map(|c| c.inner()).collect();
    let snapshots = numbered_snapshots(&net_refs, base_scenario).map_err(to_pyerr)?;
    let outputs = gridfm_write_batch(&snapshots, out_dir, &opts).map_err(to_pyerr)?;
    gridfm_outputs_to_dict(py, &outputs)
}

/// Turn a [`GridfmRead`] into the `(case, scenario, warnings)` triple the Python
/// `read_gridfm*` functions return: the reconstructed network wrapped as a
/// `PyBalancedNetwork` (with its index core, exactly as `parse_file` does), the scenario id,
/// and the fidelity warnings the lossy read surfaced.
#[cfg(feature = "gridfm")]
fn gridfm_read_to_py(read: GridfmRead) -> (PyBalancedNetwork, i64, Vec<String>) {
    (
        case_from_parts(read.network, read.diagnostics.clone()),
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
fn read_gridfm(dir: &str, scenario: i64) -> PyResult<(PyBalancedNetwork, i64, Vec<String>)> {
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
fn read_gridfm_scenarios(dir: &str) -> PyResult<Vec<(PyBalancedNetwork, i64, Vec<String>)>> {
    let reads = gridfm_read_scenarios(dir).map_err(to_pyerr)?;
    Ok(reads.into_iter().map(gridfm_read_to_py).collect())
}

#[pymodule]
fn _powerio(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("PowerIOError", m.py().get_type::<PowerIOError>())?;
    m.add("PowerIOParseError", m.py().get_type::<PowerIOParseError>())?;
    m.add("PowerIODataError", m.py().get_type::<PowerIODataError>())?;
    m.add_class::<PyBalancedNetwork>()?;
    m.add_function(wrap_pyfunction!(parse_file, m)?)?;
    m.add_function(wrap_pyfunction!(parse_str, m)?)?;
    m.add_function(wrap_pyfunction!(parse_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(parse_display_file, m)?)?;
    m.add_function(wrap_pyfunction!(parse_display_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(from_json, m)?)?;
    m.add_function(wrap_pyfunction!(read_pypsa_csv_folder, m)?)?;
    m.add_function(wrap_pyfunction!(convert_file, m)?)?;
    m.add_function(wrap_pyfunction!(convert_str, m)?)?;
    m.add_class::<PyMulticonductorNetwork>()?;
    m.add_function(wrap_pyfunction!(dist_parse_file, m)?)?;
    m.add_function(wrap_pyfunction!(dist_parse_str, m)?)?;
    m.add_function(wrap_pyfunction!(dist_convert_file, m)?)?;
    m.add_function(wrap_pyfunction!(dist_convert_str, m)?)?;
    m.add_class::<PyPackage>()?;
    m.add_function(wrap_pyfunction!(classify_json_text, m)?)?;
    m.add_function(wrap_pyfunction!(json_classes, m)?)?;
    m.add_function(wrap_pyfunction!(parse_scopf, m)?)?;
    m.add_function(wrap_pyfunction!(parse_geo, m)?)?;
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

#[cfg(test)]
mod tests {
    use super::sidecar_stays_in_output_dir;

    /// `ConversionSidecar::path` is a public field, so a caller can set it to
    /// anything. Every path here reaches out of the output directory without
    /// holding a `..` component, which is why the check is a whitelist of
    /// plain-name components rather than a blacklist.
    #[test]
    fn a_sidecar_path_that_leaves_the_output_directory_is_refused() {
        for path in [
            "",            // `join` resolves back to the directory itself
            "/etc/passwd", // rooted, and not absolute on Windows
            "../up.csv",
            "sub/../../up.csv",
        ] {
            assert!(
                !sidecar_stays_in_output_dir(path),
                "{path:?} must be refused"
            );
        }
        // Only Windows reads a drive prefix. On every other platform these
        // are ordinary file names, and `join` keeps them in the directory.
        #[cfg(windows)]
        for path in ["C:evil.csv", "C:\\evil.csv", "\\\\server\\share\\x.csv"] {
            assert!(
                !sidecar_stays_in_output_dir(path),
                "{path:?} must be refused"
            );
        }
        for path in ["coords.csv", "sub/coords.csv", "sub/deeper/coords.csv"] {
            assert!(
                sidecar_stays_in_output_dir(path),
                "{path:?} must be allowed"
            );
        }
    }
}
