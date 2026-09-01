//! Typed balanced network models and format implementations.
//!
//! Readers and writers cover MATPOWER `.m`, PowerModels JSON, PSS/E `.raw`,
//! PowerWorld `.aux`, pandapower JSON, PyPSA CSV, egret JSON, PSLF `.epc`,
//! PSS/E RAWX 35, PowSybl XIIDM 1.17, CIM CGMES 2.4.15 and 3.0, GO Challenge 3
//! JSON, Surge JSON, and DeepMind OPFData JSON. PowerWorld `.pwb` and OPFData
//! files are read only. Direct GOC3 parsing in this component crate returns
//! the balanced network projection for Rust consumers that omit
//! `powerio-prob`; its diagnostics name the omitted calculation data. The top
//! level `powerio::parse` returns the declared `AcScucInstance` or
//! `AcScucSolution`. `.pwd` display files parse through [`parse_display`].
//! Each reader produces a [`BalancedNetwork`]. The top level `powerio` facade
//! owns universal parsing and `emit`; this component crate supplies the typed
//! transmission implementation.
//!
//! A reader that retains source text can return those bytes when writing the
//! same format. Matrix and problem instance builders live in separate crates.
//!
//! ```
//! use powerio_core::Source;
//! use powerio_tx::{parse, parse_format_id};
//!
//! let src = "\
//! function mpc = example
//! mpc.version = '2';
//! mpc.baseMVA = 100;
//! mpc.bus = [
//! \t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
//! \t2\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
//! ];
//! mpc.branch = [
//! \t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
//! ];
//! ";
//! let source = Source::from_memory("example.m", src.as_bytes().to_vec())?
//!     .with_format(parse_format_id("matpower")?);
//! let module = parse(source)?;
//! assert_eq!(module.value.buses().len(), 2);
//! # Ok::<(), powerio_core::Error>(())
//! ```

/// The powerio crate version, for provenance fields written by downstream
/// crates whose own version can drift from the core's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

mod collect;
pub mod dc;
pub mod diagnostics;
pub mod error;
pub mod format;
pub mod gen_cost;
pub mod geo;
pub mod indexed;
pub mod network;
mod normalize;
mod operations;
#[doc(hidden)]
pub mod version;

pub use dc::BranchSusceptanceFormula;
pub use diagnostics::{Diagnostic, DiagnosticCode, DiagnosticSeverity, EmitFamily};
pub use error::{Error, ErrorCategory, Result};
pub use format::routing::{
    Detection, JSON_CLASSES, JsonClass, classify_json_bytes, classify_json_text,
    parse_distribution_format, parse_transmission_format,
};
#[cfg(test)]
pub(crate) use format::test_parse::{parse_file, parse_str};
pub use format::{
    DisplayData, DisplayFormat, EmitOptions, PwdDisplay, PwdSubstation, SOURCE_FORMAT_NAMES,
    TargetFormat, emit, emit_with_options, parse, parse_display, parse_display_format,
    parse_format_id, parse_target_format,
};

#[doc(hidden)]
pub use format::{
    __emit_pypsa_csv, __emit_pypsa_csv_with_options, __parse_opfdata_json,
    __parse_pypsa_csv_time_series,
};
pub use gen_cost::{GenCostPatch, GenCostPolicyReport, MissingGenCostPolicy, parse_gen_cost_csv};
pub use geo::{
    Canvas, CoordinateSpace, CoordsKind, ElementKey, GeoApplyReport, GeoFeature, GeoGeometry,
    GeoLayer, GeoMeta, GeoParsed, GeoTarget, Location, apply_substation_points,
    to_geo_layer_from_aux_substations, to_geo_layer_from_pwd, to_lonlat_from_pwd_mercator,
};
pub use indexed::{ConnectivityReport, IndexCore, IndexedNetwork};
pub use network::{
    AcDcConverterControlMode, ActivePowerControl, Area, BalancedNetwork, BoundaryLine,
    BoundaryLineGeneration, Branch, BranchCharging, BranchCurrentRatings, BranchRatingSet,
    BranchSolution, Bus, BusBreakerBus, BusId, BusType, BusbarSection, CalculatedBus, CaseMetadata,
    ComponentAlias, ComponentMetadata, ConnectivityNode, CurveStyle, DEFAULT_BASE_FREQUENCY,
    DcBusbar, DcConverterOperatingMode, DcConverterUnit, DcGround, DcLine, DcNode, DcPolarity,
    DcSeriesDevice, DcSwitch, DcSwitchKind, DcTerminal, DcTopologicalNode, DetailedConnectivity,
    DroopCurve, DroopCurveSegment, ExternalIdentifier, Extras, GenCaps, GenCost, Generator, Hvdc,
    HvdcConverter, HvdcConverterKind, HvdcConvertersMode, Impedance, InternalConnection,
    LineCommutatedConverter, LineCommutatedConverterOperatingMode,
    LineCommutatedConverterReactiveModel, Load, LoadVoltageModel, LoadingLimits,
    MinMaxReactiveLimits, OperationalLimitGroup, ReactiveCapabilityCurve,
    ReactiveCapabilityCurvePoint, ReactiveLimits, Shunt, ShuntBlock, SolverParams, SourceFormat,
    StaticVarCompensator, StaticVarCompensatorRegulationMode, Storage, Subnetwork, Substation,
    Switch, SwitchKind, SwitchedShuntControl, SwitchedShuntMode, TapChanger, TapChangerKind,
    TapChangerRegulationMode, TapChangerStep, TemporaryLimit, Terminal, TerminalReference, TieLine,
    TopologyEndpoint, TopologyKind, TopologySwitch, Transformer3W, TransformerControl,
    TransformerControlMode, VoltageLevel, VoltageSourceConverter, Winding,
    calc_series_admittance_of, repair_values,
};
pub use normalize::{
    NormalizeOptions, NormalizeSourceRows, NormalizedNetwork, POWER_MODELS_ANGLE_BOUND_PAD,
    correct_angle_difference_bounds,
};
pub use operations::Selector;
