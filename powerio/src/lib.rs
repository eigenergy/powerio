//! PowerIO: compiler infrastructure for power system data.
//!
//! The short `powerio` name is the entry facade over the component crates:
//! `powerio-core` (sources, diagnostics, errors, modules), `powerio-tx`
//! (the balanced transmission model and its format parsing and emission),
//! `powerio-dist` (the multiconductor distribution model), and `powerio-prob`
//! (operating points, problem instances, and solutions). The facade owns the
//! dynamic value boundary: [`PioValue`], [`parse`], [`emit`], [`serialize`],
//! and [`deserialize`].
//!
//! [`parse`] compiles one [`Source`] into `PioModule<PioValue>`, routing to
//! whichever built in family claims it.
//! Inspect the value with ordinary enum matching and emit the module without
//! discarding its source or diagnostics:
//!
//! ```no_run
//! let source = powerio::Source::open("case9.m")?;
//! let module = powerio::parse(source, None)?;
//! match module.value() {
//!     powerio::PioValue::BalancedNetwork(network) => {
//!         println!("{} buses", network.buses().len());
//!     }
//!     other => println!("parsed {}", other.type_name()),
//! }
//! powerio::emit(
//!     &module,
//!     "matpower",
//!     powerio::Destination::path("copy.m"),
//! )?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Files, directories, and memory buffers all enter through [`Source`], so
//! the operation does not multiply into path-, text-, and byte-specific
//! parse verbs.

/// The facade version recorded on producers and stored modules.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

use powerio_tx::format;
pub use powerio_tx::{
    Area, BalancedNetwork, Branch, BranchCharging, BranchCurrentRatings, BranchRatingSet,
    BranchSolution, BranchSusceptanceFormula, Bus, BusId, BusType, Canvas, CoordinateSpace,
    CoordsKind, DEFAULT_BASE_FREQUENCY, Detection, DisplayData, DisplayFormat, ElementKey, Extras,
    GenCaps, GenCost, Generator, GeoApplyReport, GeoFeature, GeoGeometry, GeoLayer, GeoMeta,
    GeoParsed, GeoTarget, Hvdc, Impedance, IndexCore, IndexedNetwork, JSON_CLASSES, JsonClass,
    Load, LoadVoltageModel, Location, PwdDisplay, PwdSubstation, Selector, Shunt, ShuntBlock,
    SolverParams, SourceFormat, Storage, Switch, SwitchedShuntControl, SwitchedShuntMode,
    Transformer3W, TransformerControl, TransformerControlMode, Winding, apply_substation_points,
    calc_series_admittance_of, classify_json_bytes, classify_json_text, repair_values,
    to_geo_layer_from_pwd, to_lonlat_from_pwd_mercator,
};
/// Balanced network records and the public network and geographic submodules.
/// Derived indexes, normalization data, solver tables, and component error
/// types remain available from `powerio-tx` rather than being duplicated at
/// the facade root.
pub use powerio_tx::{geo, network, version};

pub use powerio_core::diagnostic_codes;
/// The common module records and containers. These explicit facade exports
/// keep ordinary callers out of the component crate paths.
pub use powerio_core::{
    ArtifactPath, ComponentId, Destination, Diagnostic, DiagnosticCode, DiagnosticId,
    DiagnosticInfo, DiagnosticSeverity, DiagnosticStage, Digest, DigestAlgorithm, EmitResult,
    EmittedOutput, Fidelity, FormatId, HistoryEntry, HistoryId, HistoryKind, MemoryArtifact,
    OutputLayout, PioModule, Producer, Scenario, ScenarioId, ScenarioSet, Source, SourceBuffer,
    SourceDescriptor, SourceId, SourceMapEntry, SourceRelation, SourceSpan, StagedEdit, TimePoint,
    TimeSeries,
};

/// The facade error covers source acquisition, routing, stored modules, and
/// component failures converted at their boundary.
pub use powerio_core::Error;
pub type Result<T> = std::result::Result<T, powerio_core::Error>;

/// Distribution types remain grouped under `powerio::dist` where their names
/// overlap with balanced network types. Common unambiguous records are also
/// available at the facade root.
pub use powerio_dist as dist;
pub use powerio_dist::{ConductorMatrix, DistGeoMeta, DistGraphEdgeKind, MulticonductorNetwork};

pub use powerio_prob::solution::{SocwrOpfDuals, SocwrOpfSolution, SocwrOpfValues};
/// The balanced calculation types used by solver consumers. The full problem
/// vocabulary lives in [`powerio_prob`]; these types sit at the facade root so
/// a consumer does not need a second PowerIO dependency to name its boundary.
pub use powerio_prob::{
    AcBusSpecification, AcOpfInstance, AcOpfSolution, AcPfInstance, AcPfSolution, AcScucInstance,
    AcScucSolution, ActivePower, ActivePowerUnit, ApparentPower, ApparentPowerUnit,
    BalancedCalculationInstance, CalculationUpdate, DcBusSpecification, DcOpfInstance,
    DcOpfSolution, DcPfInstance, DcPfSolution, LoadAllocation, McAcOpfInstance, McAcOpfSolution,
    McAcPfInstance, McAcPfSolution, NetworkUpdate, OperatingPointUpdate, ReactivePower,
    ReactivePowerUnit, Termination, ThreeWindingTransformerTerminalActivePower,
    ThreeWindingTransformerTerminalPower, UpdateChange, UpdateReport, UpdatedField,
    apply_bus_load_active_power, apply_updates,
};

/// Matrix and graph data, re-exported from `powerio-matrix` under the
/// `matrix` feature. Matrix construction is never a parse result, so the
/// facade's automatic parsing and [`PioValue`] do not change with this
/// feature.
#[cfg(feature = "matrix")]
pub use powerio_matrix as matrix;

#[cfg(feature = "gridfm")]
#[doc(hidden)]
#[path = "gridfm.rs"]
pub mod __gridfm;
pub mod codes;
mod formats;
pub use formats::{FormatInfo, resolve_format};
#[cfg(feature = "gridfm")]
mod collect;
pub mod dist_geo;
#[cfg(feature = "gridfm")]
pub use __gridfm::codes as gridfm_codes;
mod stored;
mod write;
pub use write::emit;
mod ir;
#[cfg(feature = "schema")]
pub use ir::generate_ir_schema;
pub use ir::{deserialize, serialize};
pub mod transform;
pub use transform::{
    apply_geo_layer, to_ac_opf_instance, to_ac_pf_instance, to_dc_opf_instance, to_dc_pf_instance,
    to_mc_ac_opf_instance, to_mc_ac_pf_instance,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Goc3DataFileKind {
    Problem,
    Solution,
}

#[derive(Default)]
struct Goc3DataFiles {
    problem: Option<SourceBuffer>,
    solution: Option<SourceBuffer>,
}

impl Goc3DataFiles {
    fn insert(&mut self, kind: Goc3DataFileKind, buffer: SourceBuffer) -> Result<()> {
        let slot = match kind {
            Goc3DataFileKind::Problem => &mut self.problem,
            Goc3DataFileKind::Solution => &mut self.solution,
        };
        if let Some(existing) = slot {
            return Err(Error::new(
                &powerio_tx::diagnostics::codes::READ_GOC3_AMBIGUOUS_DOCUMENTS,
                format!(
                    "GO Challenge 3 source contains both `{}` and `{}` as {} data files",
                    existing.name(),
                    buffer.name(),
                    match kind {
                        Goc3DataFileKind::Problem => "problem",
                        Goc3DataFileKind::Solution => "solution",
                    }
                ),
            ));
        }
        *slot = Some(buffer);
        Ok(())
    }
}

fn goc3_file_kind(buffer: &SourceBuffer) -> Result<Option<Goc3DataFileKind>> {
    let value: serde_json::Value =
        serde_json::from_slice(buffer.content_bytes()).map_err(|error| {
            Error::new(
                &powerio_tx::diagnostics::codes::PARSE_GOC3_MALFORMED,
                format!("{}: {error}", buffer.name()),
            )
        })?;
    let Some(root) = value.as_object() else {
        return Ok(None);
    };
    let problem = root.contains_key("network")
        && root.contains_key("time_series_input")
        && root.contains_key("reliability");
    let solution = root.contains_key("time_series_output");
    match (problem, solution) {
        (true, false) => Ok(Some(Goc3DataFileKind::Problem)),
        (false, true) => Ok(Some(Goc3DataFileKind::Solution)),
        (false, false) => Ok(None),
        (true, true) => Err(Error::new(
            &powerio_tx::diagnostics::codes::READ_GOC3_AMBIGUOUS_DOCUMENTS,
            format!(
                "{} contains both the GO Challenge 3 problem and solution roots",
                buffer.name()
            ),
        )),
    }
}

fn goc3_data_files(source: &Source) -> Result<Goc3DataFiles> {
    let mut buffers = if source.is_directory() {
        let mut buffers = Vec::new();
        for name in source.entry_names()? {
            if std::path::Path::new(name.as_str())
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                buffers.push(source.buffer(&name)?);
            }
        }
        buffers
    } else {
        let mut buffers = vec![source.primary_buffer()?];
        // `entry_names` succeeds here only for an in-memory source with named
        // buffers. A file source never searches sibling files.
        if let Ok(names) = source.entry_names() {
            for name in names {
                buffers.push(source.root_buffer(name.as_str())?);
            }
        }
        buffers
    };
    buffers.sort_by(|left, right| left.name().cmp(right.name()));

    let mut files = Goc3DataFiles::default();
    for buffer in buffers {
        if let Some(kind) = goc3_file_kind(&buffer)? {
            files.insert(kind, buffer)?;
        }
    }
    Ok(files)
}

fn directory_has_goc3_data(source: &Source) -> bool {
    source.entry_names().is_ok_and(|names| {
        names.into_iter().any(|name| {
            std::path::Path::new(name.as_str())
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
                && source.buffer(&name).is_ok_and(|buffer| {
                    serde_json::from_slice::<serde_json::Value>(buffer.content_bytes()).is_ok_and(
                        |value| {
                            value.as_object().is_some_and(|root| {
                                root.contains_key("time_series_output")
                                    || (root.contains_key("network")
                                        && root.contains_key("time_series_input")
                                        && root.contains_key("reliability"))
                            })
                        },
                    )
                })
        })
    })
}

/// Parse a display artifact from a source, inferring its format when `format`
/// is `None`. Paths and memory use [`Source::open`] and
/// [`Source::from_memory`] like case parsing.
///
/// # Errors
/// The format cannot be inferred, the source cannot be acquired, or the
/// display data is malformed.
pub fn parse_display(source: Source, format: Option<&str>) -> Result<DisplayData> {
    powerio_tx::parse_display(source, format)
        .map_err(|error| Error::new(error.code(), error.to_string()).with_cause(error))
}

/// Transform the `Substation` table in PowerWorld AUX text into a geographic
/// layer without exposing the component parser's borrowed `AuxFile` type.
///
/// Rows without a finite number, latitude, and longitude are skipped. A valid
/// AUX document with no usable substation coordinates returns an empty layer.
///
/// # Errors
/// The AUX section syntax is malformed.
pub fn to_geo_layer_from_aux_text(text: &str) -> Result<GeoLayer> {
    let aux = powerio_tx::format::powerworld::aux_sections(text)
        .map_err(|error| Error::new(error.code(), error.to_string()).with_cause(error))?;
    Ok(powerio_tx::to_geo_layer_from_aux_substations(&aux))
}

/// A possibly partial assignment of instantaneous operating quantities over
/// one network's fixed equipment identities.
pub use powerio_prob::OperatingPoint;
mod value;
pub use value::{PioScenarioSet, PioTimeSeries, PioValue};

/// Parse one source into a compiled module of whichever built in family
/// claims it. Balanced network formats produce
/// [`PioValue::BalancedNetwork`]; network only distribution formats (OpenDSS
/// `.dss`, PMD ENGINEERING JSON, and BMOPF JSON) produce
/// [`PioValue::MulticonductorNetwork`]. A source that defines a particular
/// calculation produces that calculation's value. One DOE GO Challenge 3
/// problem data file produces [`PioValue::AcScucInstance`]. One source that
/// contains a problem data file and its matching solution data file produces
/// [`PioValue::AcScucSolution`]; a solution data file alone is rejected because
/// its row identities and time axis come from the problem. DeepMind OPFData
/// JSON, which explicitly represents a solved AC OPF, produces
/// [`PioValue::AcOpfSolution`]. The parser's findings are the module's
/// diagnostics and the source is retained for the byte exact same format tier
/// of [`emit`].
///
/// The family comes from the source's declared format when one was selected,
/// and otherwise from the name and content: a `.dss` extension routes to the
/// distribution parser, a `.json` document routes by its top level markers
/// ([`format::routing::classify_json_text`]), a name with no recognized
/// extension whose content opens a JSON document (an in-memory source has no
/// extension) routes the same way, and every other name routes to
/// the balanced network hub, whose own detection and refusals apply.
///
/// Bare model JSON, the network serialization, decodes to
/// [`PioValue::BalancedNetwork`] like any other balanced source. PowerIO IR is
/// not a grid exchange format: [`parse`] refuses it and [`deserialize`] reads
/// the PowerIO 1.0 IR.
///
/// # Errors
/// The routed family's failure, carrying its findings and the retained
/// source.
pub fn parse(
    mut source: powerio_core::Source,
    format: Option<&str>,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    if let Some(format) = format {
        source = source.with_format(powerio_core::FormatId::new(format)?);
    }
    match routed_family(&source)? {
        RoutedFamily::Goc3 => parse_goc3(source),
        RoutedFamily::OpfData => powerio_prob::__internal::__decode_opfdata_solution(source)
            .map(|module| module.map_value(PioValue::from)),
        RoutedFamily::Distribution(detected) => {
            let source = match (source.format(), detected) {
                (None, Some(format)) => {
                    source.with_format(powerio_core::FormatId::new(format.name())?)
                }
                _ => source,
            };
            powerio_dist::parse(source).map(|module| module.map_value(PioValue::from))
        }
        RoutedFamily::PypsaDirectory => parse_pypsa(source),
        #[cfg(feature = "gridfm")]
        RoutedFamily::Gridfm => parse_gridfm(source),
        RoutedFamily::Egret => parse_egret(source),
        RoutedFamily::Balanced(json_class) => format::parse_with_json_class(source, json_class)
            .map(|module| module.map_value(PioValue::from)),
    }
}

/// Parse the official GO Challenge 3 problem file, or a problem and its
/// matching solution file supplied by one directory or one memory source.
/// File roles come from the required top level JSON fields, not filenames.
/// A solution file alone is incomplete because it contains neither the
/// component definitions nor the time axis.
fn parse_goc3(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    let source = source.with_format(powerio_core::FormatId::new("goc3-json")?);
    let files = match goc3_data_files(&source) {
        Ok(files) => files,
        Err(error) => return Err(error.with_source(source)),
    };
    let Some(problem) = files.problem else {
        let message = if files.solution.is_some() {
            "a GO Challenge 3 solution file requires the matching problem file in the same source"
        } else {
            "the source contains neither a GO Challenge 3 problem file nor a solution file"
        };
        return Err(Error::new(
            &powerio_tx::diagnostics::codes::READ_GOC3_PROBLEM_REQUIRED,
            message,
        )
        .with_source(source));
    };

    let (instance, diagnostics) =
        match powerio_prob::__internal::__parse_goc3_problem_buffer(&problem) {
            Ok(parsed) => parsed,
            Err(error) => return Err(error.with_source(source)),
        };
    let value = match files.solution {
        Some(solution) => {
            let solution = match powerio_prob::__internal::__parse_goc3_output_buffer(
                std::sync::Arc::new(instance),
                &solution,
            ) {
                Ok(solution) => solution,
                Err(error) => return Err(error.with_source(source)),
            };
            PioValue::from(solution)
        }
        None => PioValue::from(instance),
    };
    powerio_core::PioModule::parsed(value, source, diagnostics)
}

/// PyPSA CSV dispatch: one snapshot with no series siblings is the scalar
/// profile through the balanced hub; a declared axis routes to the sequence
/// parser, producing a network series or, when only operating quantities
/// vary, an operating point series over one shared network.
fn parse_pypsa(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    if !source.is_directory() {
        // A file claiming the PyPSA token gets the hub's own refusal wording.
        return format::parse(source).map(|module| module.map_value(PioValue::from));
    }
    // Directory routing identified the source before the typed reader runs.
    // Record that decision on an undeclared source so same format emission
    // can distinguish a PyPSA directory from the GridFM directory family.
    let source = match source.format() {
        Some(_) => source,
        None => source.with_format(powerio_core::FormatId::new("pypsa-csv")?),
    };
    let axis = match format::__pypsa_axis(&source) {
        Ok(axis) => axis,
        Err(error) => {
            let core = powerio_core::Error::new(error.code(), error.to_string());
            return Err(core.with_source(source));
        }
    };
    match axis {
        format::PypsaAxis::SingleSnapshot => {
            format::parse(source).map(|module| module.map_value(PioValue::from))
        }
        format::PypsaAxis::Series => {
            match powerio_prob::__internal::__decode_pypsa_sequence(&source) {
                Ok((sequence, diagnostics)) => {
                    let value = match sequence {
                        powerio_prob::__internal::PypsaSequence::Networks(series) => {
                            PioValue::from(series)
                        }
                        powerio_prob::__internal::PypsaSequence::OperatingPoints(points) => {
                            PioValue::from(points)
                        }
                    };
                    powerio_core::PioModule::parsed(value, source, diagnostics)
                }
                Err(error) => Err(error.with_source(source)),
            }
        }
    }
}

/// gridfm dispatch: every scenario of the Parquet dataset as one scenario
/// set over shared element identities.
#[cfg(feature = "gridfm")]
fn parse_gridfm(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    if !source.is_directory() {
        // A file claiming the gridfm token gets the hub's own refusal wording.
        return format::parse(source).map(|module| module.map_value(PioValue::from));
    }
    let source = match source.format() {
        Some(_) => source,
        None => source.with_format(powerio_core::FormatId::new("gridfm")?),
    };
    match __gridfm::parse_gridfm_source(&source) {
        Ok((set, diagnostics)) => {
            powerio_core::PioModule::parsed(PioValue::from(set), source, diagnostics)
        }
        Err(error) => Err(error.with_source(source)),
    }
}

/// Egret dispatch: a document declaring `system.time_keys` routes to the
/// sequence parser and produces a balanced network time series; a scalar
/// document routes through the balanced hub.
fn parse_egret(
    source: powerio_core::Source,
) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
    let declares_series = {
        let buffer = source.primary_buffer()?;
        std::str::from_utf8(buffer.content_bytes()).is_ok_and(format::__egret_declares_time_series)
    };
    if !declares_series {
        return format::parse(source).map(|module| module.map_value(PioValue::from));
    }
    let parsed = {
        let buffer = source.primary_buffer()?;
        let stem = std::path::Path::new(source.name())
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned);
        match std::str::from_utf8(buffer.content_bytes()) {
            Ok(text) => format::__parse_egret_time_series(text, stem.as_deref())
                .map_err(|error| powerio_core::Error::new(error.code(), error.to_string())),
            Err(error) => {
                let cause = powerio_tx::Error::FormatRead {
                    format: "case text",
                    message: format!("not valid UTF-8: {error}"),
                };
                Err(powerio_core::Error::new(cause.code(), cause.to_string()))
            }
        }
    };
    match parsed {
        Ok(series) => powerio_core::PioModule::parsed(PioValue::from(series), source, Vec::new()),
        Err(error) => Err(error.with_source(source)),
    }
}

/// The family a source routes to. The balanced hub is the default: it owns
/// the guidance for unknown names and refused shapes. `Balanced` carries the
/// JSON classification when routing here was itself the result of one (so
/// the balanced hub does not classify the same text a second time); `None`
/// when the source routed here by extension, by a declared token, or by a
/// directory shape, none of which run a JSON classification at all.
enum RoutedFamily {
    Balanced(Option<format::routing::JsonClass>),
    Distribution(Option<format::routing::DistributionFormat>),
    Goc3,
    OpfData,
    PypsaDirectory,
    Egret,
    #[cfg(feature = "gridfm")]
    Gridfm,
}

fn routed_family(
    source: &powerio_core::Source,
) -> std::result::Result<RoutedFamily, powerio_core::Error> {
    if let Some(declared) = source.format() {
        return Ok(family_of_token(declared.as_str()));
    }
    if source.is_directory() {
        // GOC3 pairs are identified by their official JSON roots rather than
        // filenames. The format parser performs the exact cardinality and
        // schema checks after routing.
        if directory_has_goc3_data(source) {
            return Ok(RoutedFamily::Goc3);
        }
        // PyPSA is a CSV folder containing network.csv. GridFM is a Parquet
        // dataset with bus_data.parquet at one of its documented locations.
        // Anything else falls to the balanced hub's refusal.
        let marker = powerio_core::ArtifactPath::new("network.csv")
            .expect("static name is a valid artifact path");
        if source.buffer(&marker).is_ok() {
            return Ok(RoutedFamily::PypsaDirectory);
        }
        #[cfg(feature = "gridfm")]
        if let Ok(entries) = source.entry_names()
            && entries.iter().any(|entry| {
                entry.as_str().ends_with("bus_data.parquet")
                    && matches!(entry.as_str().matches('/').count(), 0..=2)
            })
        {
            return Ok(RoutedFamily::Gridfm);
        }
        return Ok(RoutedFamily::Balanced(None));
    }
    let extension = std::path::Path::new(source.name())
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "dss" => Ok(RoutedFamily::Distribution(Some(
            format::routing::DistributionFormat::Dss,
        ))),
        "json" => json_family(source),
        // Extensions with dedicated non-JSON readers keep them; anything
        // else (a nameless in-memory source above all) can still carry a
        // JSON document, so content that opens one routes by classification,
        // mirroring the balanced hub's own sniff.
        "m" | "raw" | "aux" | "epc" | "pwb" | "pwd" => Ok(RoutedFamily::Balanced(None)),
        _ => {
            let jsonish = source.primary_buffer().is_ok_and(|buffer| {
                std::str::from_utf8(buffer.content_bytes()).is_ok_and(|text| {
                    // Strip a UTF-8 BOM the way the JSON classifier does, so
                    // a BOM-prefixed nameless document routes the same as
                    // the identical content saved with a .json name.
                    text.trim_start_matches('\u{feff}')
                        .trim_start()
                        .starts_with(['{', '['])
                })
            });
            if jsonish {
                json_family(source)
            } else {
                Ok(RoutedFamily::Balanced(None))
            }
        }
    }
}

/// The family a JSON document's content markers select.
fn json_family(
    source: &powerio_core::Source,
) -> std::result::Result<RoutedFamily, powerio_core::Error> {
    use format::routing::{Detection, JsonClass, SourceFormat, TransmissionFormat};

    let buffer = source.primary_buffer()?;
    // Family routing needs decoded text; a non-UTF-8 `.json` fails in
    // the balanced hub with its own wording. Classification never ran, so
    // the balanced hub gets no hint and classifies it itself.
    let Ok(text) = std::str::from_utf8(buffer.content_bytes()) else {
        return Ok(RoutedFamily::Balanced(None));
    };
    if serde_json::from_str::<serde_json::Value>(text).is_ok_and(|value| {
        value
            .as_object()
            .is_some_and(|root| root.contains_key("time_series_output"))
    }) {
        return Ok(RoutedFamily::Goc3);
    }
    let class = format::routing::classify_json_text(text);
    match class {
        JsonClass::Case(Detection::Known(SourceFormat::Transmission(
            TransmissionFormat::Goc3Json,
        ))) => Ok(RoutedFamily::Goc3),
        JsonClass::Case(Detection::Known(SourceFormat::Transmission(
            TransmissionFormat::DeepMindOpfDataJson,
        ))) => Ok(RoutedFamily::OpfData),
        JsonClass::Case(Detection::Known(SourceFormat::Transmission(
            TransmissionFormat::EgretJson,
        ))) => Ok(RoutedFamily::Egret),
        JsonClass::Case(Detection::Known(SourceFormat::Distribution(format))) => {
            Ok(RoutedFamily::Distribution(Some(format)))
        }
        JsonClass::Module => Err(powerio_core::Error::new(
            &codes::REQUEST_PARSE_POWERIO_IR,
            "PowerIO IR is not a grid exchange format; call deserialize(source)",
        )),
        // The balanced hub's own JSON detection carries the refusal
        // wording for unrecognized or ambiguous documents, and decodes bare
        // model JSON itself; it gets this classification so it never
        // re-derives it from the same bytes.
        JsonClass::Case(Detection::Known(_) | Detection::Ambiguous | Detection::Unknown)
        | JsonClass::ModelJson => Ok(RoutedFamily::Balanced(Some(class))),
    }
}

/// The family a declared format token selects. Unknown tokens fall to the
/// balanced hub, which owns the refusal wording and the accepted name list.
fn family_of_token(token: &str) -> RoutedFamily {
    use format::TargetFormat;

    if token == "model-json" {
        return RoutedFamily::Balanced(None);
    }

    if powerio_dist::parse_dist_target_format(token).is_some() {
        return RoutedFamily::Distribution(None);
    }
    if format::is_pypsa_csv_name(token) {
        return RoutedFamily::PypsaDirectory;
    }
    #[cfg(feature = "gridfm")]
    if token.eq_ignore_ascii_case("gridfm") {
        return RoutedFamily::Gridfm;
    }
    match format::parse_target_format(token) {
        Some(TargetFormat::Goc3Json) => RoutedFamily::Goc3,
        Some(TargetFormat::DeepMindOpfDataJson) => RoutedFamily::OpfData,
        Some(TargetFormat::EgretJson) => RoutedFamily::Egret,
        _ => RoutedFamily::Balanced(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(name: &str, text: &str) -> powerio_core::Source {
        powerio_core::Source::from_memory(name, text.as_bytes().to_vec()).expect("memory source")
    }

    fn parse(
        source: powerio_core::Source,
    ) -> std::result::Result<powerio_core::PioModule<PioValue>, powerio_core::Error> {
        super::parse(source, None)
    }

    fn assert_value_type(module: &powerio_core::PioModule<PioValue>, expected: &str) {
        assert_eq!(module.value().type_name(), expected);
    }

    #[test]
    fn a_matpower_source_parses_to_a_balanced_network() {
        let case = "function mpc = case\n\
                    mpc.version = '2';\n\
                    mpc.baseMVA = 100;\n\
                    mpc.bus = [1 3 0 0 0 0 1 1 0 230 1 1.1 0.9;];\n\
                    mpc.gen = [1 0 0 10 -10 1 100 1 10 0;];\n\
                    mpc.branch = [];\n";
        let module = parse(
            memory("case.m", case).with_format(powerio_core::FormatId::new("matpower").unwrap()),
        )
        .expect("matpower parses");
        assert_value_type(&module, "powerio.BalancedNetwork");
    }

    #[test]
    fn memory_parse_retains_its_name_and_optional_format() {
        let case = "function mpc = inline\n\
                    mpc.version = '2';\n\
                    mpc.baseMVA = 100;\n\
                    mpc.bus = [1 3 0 0 0 0 1 1 0 230 1 1.1 0.9;];\n\
                    mpc.gen = [1 0 0 10 -10 1 100 1 10 0;];\n\
                    mpc.branch = [];\n";

        let detected =
            super::parse(memory("inline-case.m", case), None).expect("name detects MATPOWER");
        assert_eq!(detected.source().unwrap().name(), "inline-case.m");
        assert_eq!(
            detected.source().unwrap().format().map(FormatId::as_str),
            Some("matpower")
        );

        let declared = super::parse(memory("consumer-input", case), Some("matpower"))
            .expect("declared MATPOWER");
        let source = declared.source().expect("source retained");
        assert_eq!(source.name(), "consumer-input");
        assert_eq!(source.format().map(FormatId::as_str), Some("matpower"));
    }

    #[test]
    fn universal_parse_reads_declared_iso_8859_1_xiidm_and_retains_exact_bytes() {
        let text = r#"<?xml version="1.0" encoding="ISO-8859-1"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="case" caseDate="2026-01-01T00:00:00Z" forecastDistance="0" sourceFormat="Réseau PowSybl" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:voltageLevel id="VL" nominalV="225" topologyKind="BUS_BREAKER">
    <iidm:busBreakerTopology><iidm:bus id="B" v="225" angle="0"/></iidm:busBreakerTopology>
    <iidm:generator id="G" energySource="OTHER" minP="0" maxP="100" voltageRegulatorOn="true" targetP="50" targetV="225" bus="B" connectableBus="B"><iidm:minMaxReactiveLimits minQ="-20" maxQ="20"/></iidm:generator>
  </iidm:voltageLevel>
</iidm:network>"#;
        let bytes: Vec<u8> = text
            .chars()
            .map(|value| u8::try_from(u32::from(value)).expect("fixture is ISO-8859-1"))
            .collect();
        assert!(std::str::from_utf8(&bytes).is_err());

        for (name, format) in [
            ("case.xiidm", None),
            ("case.xml", None),
            ("memory", Some("xiidm")),
        ] {
            let source = Source::from_memory(name, bytes.clone()).unwrap();
            let module = super::parse(source, format).unwrap();
            let PioValue::BalancedNetwork(network) = &module.value() else {
                panic!(
                    "expected BalancedNetwork, got {}",
                    module.value().type_name()
                );
            };
            assert_eq!(
                network.case_metadata().source_model_format.as_deref(),
                Some("Réseau PowSybl")
            );
            let retained = module.source().unwrap();
            assert_eq!(retained.format().map(FormatId::as_str), Some("xiidm"));
            assert_eq!(retained.primary_buffer().unwrap().bytes(), bytes);

            let emitted =
                emit(&module, "xiidm", Destination::memory("copy.xiidm").unwrap()).unwrap();
            assert_eq!(emitted.fidelity(), Fidelity::ExactSameFormat);
            let EmittedOutput::Memory { artifacts } = emitted.into_output() else {
                panic!("memory destination returned a path output");
            };
            assert_eq!(artifacts.len(), 1);
            assert_eq!(artifacts[0].bytes(), bytes);
        }
    }

    #[test]
    fn a_dss_source_parses_to_a_multiconductor_network() {
        let module = parse(memory(
            "feeder.dss",
            "New Circuit.c basekv=12.47 bus1=src\n",
        ))
        .expect("dss parses");
        let PioValue::MulticonductorNetwork(network) = &module.value() else {
            panic!(
                "expected multiconductor network, got {}",
                module.value().type_name()
            );
        };
        assert_eq!(network.name().as_deref(), Some("c"));
    }

    #[test]
    fn a_declared_distribution_format_routes_without_an_extension() {
        let module = parse(
            memory("<memory>", "New Circuit.c basekv=12.47 bus1=src\n")
                .with_format(powerio_core::FormatId::new("dss").unwrap()),
        )
        .expect("declared dss parses");
        assert_value_type(&module, "powerio.MulticonductorNetwork");
    }

    #[test]
    fn json_routes_by_top_level_markers() {
        // A PMD document carries `data_model`, which no balanced format does.
        let module = parse(memory(
            "feeder.json",
            r#"{"data_model": "ENGINEERING", "bus": {}}"#,
        ))
        .expect("pmd parses");
        assert_value_type(&module, "powerio.MulticonductorNetwork");
    }

    #[test]
    fn bare_model_json_parses_to_a_balanced_network() {
        use powerio_tx::{Bus, BusId, BusType};
        let network = powerio_tx::BalancedNetwork::in_memory(
            "transport",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            vec![],
        );
        let json = network.to_json().expect("network serializes");
        let module = parse(memory("net.json", &json)).expect("model json parses");
        assert_value_type(&module, "powerio.BalancedNetwork");
    }

    #[test]
    fn the_error_path_retains_the_source() {
        let error = parse(memory("case.m", "not matpower at all")).expect_err("malformed");
        assert!(error.retained_source().is_some());
    }

    fn fixture(path: &str) -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(root.join(path)).unwrap()
    }

    #[test]
    fn goc3_parses_to_an_scuc_instance() {
        // A calculation defining source produces its calculation's value,
        // never the bare network its data could also build.
        let text = fixture("../powerio-prob/tests/data/goc3_small.json");
        let module = parse(memory("goc3_small.json", &text)).expect("goc3 parses");
        assert_value_type(&module, "powerio.AcScucInstance");
        assert!(module.source().is_some());
        let PioValue::AcScucInstance(instance) = &module.value() else {
            unreachable!();
        };
        assert_eq!(instance.network().buses().len(), 2);
    }

    #[test]
    fn goc3_problem_and_solution_parse_with_the_one_public_operation() {
        let problem = fixture("../tests/data/goc3/goc3_small.json");
        let solution = fixture("../tests/data/goc3/goc3_small_solution.json");
        let source = powerio_core::Source::from_memory("problem.json", problem.into_bytes())
            .unwrap()
            .with_named_buffer("solution.json", solution.into_bytes())
            .unwrap();
        let module = parse(source).expect("problem and solution parse together");
        let PioValue::AcScucSolution(solution) = &module.value() else {
            panic!(
                "expected AC SCUC solution, got {}",
                module.value().type_name()
            );
        };
        assert_eq!(solution.instance().network().buses().len(), 2);
        assert_eq!(
            solution.network_outputs().shunt_step,
            vec![vec![1], vec![2]]
        );
        assert_eq!(module.sources().len(), 2);

        let emitted = emit(
            &module,
            "goc3-json",
            Destination::memory("solution.json").unwrap(),
        )
        .expect("solution emits as official GOC3 output");
        let EmittedOutput::Memory { artifacts } = emitted.output() else {
            unreachable!();
        };
        assert_eq!(artifacts.len(), 1);
        let document: serde_json::Value = serde_json::from_slice(artifacts[0].bytes()).unwrap();
        assert!(document.get("time_series_output").is_some());
        assert!(document.get("network").is_none());
    }

    #[test]
    fn goc3_solution_alone_names_the_missing_problem() {
        let solution = fixture("../tests/data/goc3/goc3_small_solution.json");
        let error = parse(memory("solution.json", &solution))
            .expect_err("a solution without its problem is incomplete");
        assert!(error.to_string().contains("matching problem file"));
        assert!(error.retained_source().is_some());
    }

    #[test]
    fn opfdata_parses_to_an_ac_opf_solution() {
        let text = fixture("../tests/data/opfdataset/example_0.json");
        let module = parse(memory("example_0.json", &text)).expect("opfdata parses");
        let PioValue::AcOpfSolution(solution) = &module.value() else {
            panic!(
                "expected AC OPF solution, got {}",
                module.value().type_name()
            );
        };
        assert_eq!(
            module
                .sources()
                .first()
                .and_then(|source| source.format())
                .map(powerio_core::FormatId::as_str),
            Some("opfdata-json")
        );
        assert_eq!(
            *solution.termination(),
            powerio_prob::Termination::NotReported
        );
        assert!((solution.objective() - 2_265.953_939_003_096).abs() < 1e-9);

        let instance = solution.instance();
        assert_eq!(instance.network().buses().len(), 14);
        assert_eq!(instance.network().generators().len(), 5);
        let initial = instance.initial_point().expect("OPFData includes initials");
        let generator_id = instance.network().generators()[0]
            .uid
            .as_deref()
            .expect("parsed generators have stable identities");
        assert!((initial.generator_active_power(generator_id).unwrap() - 170.0).abs() < 1e-9);
        assert!((initial.generator_voltage_setpoint(generator_id).unwrap() - 1.0).abs() < 1e-12);
        assert!(solution.residuals().max_active_power_mismatch.unwrap() < 1.0);
        assert!(solution.residuals().max_reactive_power_mismatch.unwrap() < 1.0);
    }

    #[test]
    fn malformed_opfdata_uses_the_universal_parse_error_path() {
        let error = parse(
            memory("broken.json", "{\"grid\": {}}")
                .with_format(powerio_core::FormatId::new("opfdata-json").unwrap()),
        )
        .expect_err("malformed OPFData");
        assert!(error.retained_source().is_some());
    }

    const BMOPF_TINY: &str = r#"{
      "bus": {"a": {"terminal_names": ["1", "2", "3", "n"],
        "perfectly_grounded_terminals": ["n"]}},
      "voltage_source": {"s": {"bus": "a", "terminal_map": ["1", "2", "3"],
        "v_magnitude": [240.0, 240.0, 240.0], "v_angle": [0.0, -2.0944, 2.0944]}}
    }"#;

    #[test]
    fn bmopf_parses_to_a_multiconductor_network() {
        // BMOPF shares the multiconductor network model. Callers construct a
        // power flow or optimal power flow instance explicitly afterward.
        let module = parse(memory("feeder.json", BMOPF_TINY)).expect("sniffed bmopf parses");
        assert_value_type(&module, "powerio.MulticonductorNetwork");

        let module = parse(
            memory("<memory>", BMOPF_TINY)
                .with_format(powerio_core::FormatId::new("bmopf-json").unwrap()),
        )
        .expect("declared bmopf parses");
        assert_value_type(&module, "powerio.MulticonductorNetwork");
    }

    #[test]
    fn nameless_json_text_routes_by_content() {
        use powerio_tx::{Bus, BusId, BusType};
        // An in-memory source has no extension, so the family comes
        // from the document's own markers: a calculation defining format, a
        // distribution format, and bare model JSON all dispatch the same way
        // they would from a `.json` file.
        let goc3 = fixture("../powerio-prob/tests/data/goc3_small.json");
        let module = parse(memory("<memory>", &goc3)).expect("nameless goc3 parses");
        assert_value_type(&module, "powerio.AcScucInstance");

        let module = parse(memory("<memory>", BMOPF_TINY)).expect("nameless bmopf parses");
        assert_value_type(&module, "powerio.MulticonductorNetwork");

        let network = powerio_tx::BalancedNetwork::in_memory(
            "transport",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            vec![],
        );
        let json = network.to_json().expect("network serializes");
        let module = parse(memory("<memory>", &json)).expect("nameless model json parses");
        assert_value_type(&module, "powerio.BalancedNetwork");
    }

    #[test]
    fn a_declared_problem_format_that_fails_retains_the_source() {
        let error = parse(
            memory("broken.json", "{\"network\": {}}")
                .with_format(powerio_core::FormatId::new("goc3-json").unwrap()),
        )
        .expect_err("malformed goc3");
        assert!(error.retained_source().is_some());
    }

    const PYPSA_STATIC: [(&str, &str); 4] = [
        ("network.csv", "name\nseq\n"),
        ("buses.csv", "name,v_nom\nB1,138.0\nB2,138.0\n"),
        ("loads.csv", "name,bus,p_set,q_set\nL1,B2,5.0,1.0\n"),
        (
            "generators.csv",
            "name,bus,control,p_nom,p_set\nG1,B1,Slack,100.0,12.0\n",
        ),
    ];

    fn pypsa_folder(extra: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        for (name, content) in PYPSA_STATIC.iter().chain(extra) {
            std::fs::write(temp.path().join(name), content).unwrap();
        }
        temp
    }

    #[test]
    fn a_pypsa_snapshot_parses_to_a_balanced_network() {
        let dir = pypsa_folder(&[("snapshots.csv", ",snapshot\n0,now\n")]);
        let module =
            parse(powerio_core::Source::open(dir.path()).unwrap()).expect("snapshot parses");
        assert_value_type(&module, "powerio.BalancedNetwork");
    }

    #[test]
    fn a_pypsa_input_series_parses_to_a_network_time_series() {
        let dir = pypsa_folder(&[
            ("snapshots.csv", ",snapshot\n0,now\n1,later\n"),
            ("loads-p_set.csv", "snapshot,L1\nnow,10.0\nlater,20.0\n"),
        ]);
        let module = parse(powerio_core::Source::open(dir.path()).unwrap()).expect("series parses");
        assert_value_type(&module, "powerio.TimeSeries<powerio.BalancedNetwork>");
        assert!(module.source().is_some());
        let PioValue::TimeSeries(series) = &module.value() else {
            unreachable!();
        };
        assert_eq!(series.len(), 2);
        let PioValue::BalancedNetwork(later) = series.get(1).unwrap() else {
            unreachable!();
        };
        assert!((later.loads()[0].p - 20.0).abs() < 1e-12);
    }

    #[test]
    fn a_pypsa_voltage_series_parses_to_operating_points() {
        let dir = pypsa_folder(&[
            ("snapshots.csv", ",snapshot\n0,now\n1,later\n"),
            (
                "buses-v_mag_pu.csv",
                "snapshot,B1,B2\nnow,1.0,0.99\nlater,1.0,0.97\n",
            ),
            (
                "buses-v_ang.csv",
                "snapshot,B1,B2\nnow,0.0,-0.017453292519943295\nlater,0.0,-0.03490658503988659\n",
            ),
        ]);
        let module = parse(powerio_core::Source::open(dir.path()).unwrap()).expect("series parses");
        assert_value_type(
            &module,
            "powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>",
        );
        let PioValue::TimeSeries(series) = &module.value() else {
            unreachable!();
        };
        let PioValue::BalancedOperatingPoint(later) = series.get(1).unwrap() else {
            unreachable!();
        };
        assert!((later.bus_voltage_magnitude(powerio_tx::BusId(2)).unwrap() - 0.97).abs() < 1e-12);
    }

    #[test]
    fn a_pypsa_axis_with_no_series_stays_a_network_time_series() {
        // Several declared snapshots and nothing varying preserve the axis
        // as networks sharing every table; nothing here is an operating
        // point series.
        let dir = pypsa_folder(&[("snapshots.csv", ",snapshot\n0,now\n1,later\n")]);
        let module = parse(powerio_core::Source::open(dir.path()).unwrap()).expect("axis parses");
        assert_value_type(&module, "powerio.TimeSeries<powerio.BalancedNetwork>");
    }

    const EGRET_SERIES: &str = r#"{
        "model_name": "uc2",
        "elements": {
            "bus": {"1": {"matpower_bustype": "ref", "base_kv": 138.0},
                    "2": {"matpower_bustype": "PQ", "base_kv": 138.0}},
            "load": {"load_1": {"bus": "2",
                "p_load": {"data_type": "time_series", "values": [10.0, 20.0]},
                "q_load": 3.0}},
            "generator": {"1": {"bus": "1", "pg": 12.0, "qg": 0.0,
                "p_min": 0.0, "p_max": 50.0, "q_min": -10.0, "q_max": 10.0}},
            "branch": {"1": {"from_bus": "1", "to_bus": "2",
                "resistance": 0.01, "reactance": 0.1, "charging_susceptance": 0.0,
                "rating_long_term": 100.0, "rating_short_term": 100.0,
                "rating_emergency": 100.0, "transformer_phase_shift": 0.0}}
        },
        "system": {"baseMVA": 100.0, "time_keys": ["t1", "t2"]}
    }"#;

    #[test]
    fn egret_time_keys_parse_to_a_network_time_series() {
        let module = parse(
            memory("uc2.json", EGRET_SERIES)
                .with_format(powerio_core::FormatId::new("egret-json").unwrap()),
        )
        .expect("egret series parses");
        assert_value_type(&module, "powerio.TimeSeries<powerio.BalancedNetwork>");
        assert!(module.source().is_some());

        // The sniffed route agrees with the declared one.
        let module = parse(memory("uc2.json", EGRET_SERIES)).expect("sniffed egret parses");
        assert_value_type(&module, "powerio.TimeSeries<powerio.BalancedNetwork>");
    }

    #[cfg(feature = "gridfm")]
    #[test]
    fn powerio_ir_uses_deserialize_not_parse() {
        use powerio_tx::{Bus, BusId, BusType};
        let network = powerio_tx::BalancedNetwork::in_memory(
            "stored",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            vec![],
        );
        let original = powerio_core::PioModule::new(PioValue::BalancedNetwork(network));
        let emitted = serialize(&original, Destination::memory("case.pio.json").unwrap())
            .expect("module serializes");
        let EmittedOutput::Memory { artifacts } = emitted.into_output() else {
            unreachable!();
        };
        let module = deserialize(
            Source::from_memory("case.pio.json", artifacts[0].bytes().to_vec()).unwrap(),
        )
        .expect("module deserializes");
        assert_value_type(&module, "powerio.BalancedNetwork");
        assert!(module.source().is_some());
    }

    #[cfg(feature = "gridfm")]
    #[test]
    fn a_gridfm_dataset_parses_to_a_scenario_set() {
        // Write a two scenario dataset with the matrix writer, then parse the
        // directory through the universal parse.
        let case = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
        let base = powerio_tx::parse(powerio_core::Source::open(case).unwrap())
            .expect("case9 parses")
            .into_value();
        let mut varied = base.clone();
        varied.loads_mut()[0].p += 5.0;
        let out = tempfile::tempdir().unwrap();
        let snapshots = [
            powerio_matrix::GridfmSnapshot::new(&base, 0),
            powerio_matrix::GridfmSnapshot::new(&varied, 1),
        ];
        powerio_matrix::emit_gridfm_batch(
            &snapshots,
            out.path(),
            &powerio_matrix::GridfmOptions::default(),
        )
        .expect("dataset writes");

        let module =
            parse(powerio_core::Source::open(out.path()).unwrap()).expect("dataset parses");
        assert_value_type(&module, "powerio.ScenarioSet<powerio.BalancedNetwork>");
        assert!(module.source().is_some());
        let PioValue::ScenarioSet(set) = &module.value() else {
            unreachable!();
        };
        assert_eq!(set.len(), 2);
        assert!(set.get("0").is_some());
        assert!(set.get("1").is_some());
    }

    #[test]
    fn an_unrecognized_directory_is_refused_with_the_hub_wording() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a case").unwrap();
        let error =
            parse(powerio_core::Source::open(dir.path()).unwrap()).expect_err("refused directory");
        assert!(error.to_string().contains("directory"), "{error}");
    }

    #[test]
    fn a_scalar_egret_document_stays_a_balanced_network() {
        let scalar = EGRET_SERIES
            .replace(r#", "time_keys": ["t1", "t2"]"#, "")
            .replace(
                r#"{"data_type": "time_series", "values": [10.0, 20.0]}"#,
                "10.0",
            );
        let module = parse(
            memory("uc2.json", &scalar)
                .with_format(powerio_core::FormatId::new("egret-json").unwrap()),
        )
        .expect("scalar egret parses");
        assert_value_type(&module, "powerio.BalancedNetwork");
    }
}
