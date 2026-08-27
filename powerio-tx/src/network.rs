//! Format neutral balanced network model.
//!
//! Readers map source formats into a [`BalancedNetwork`], and writers map a network to
//! target formats. Loads and shunts have separate tables, so formats can retain
//! several elements at one bus. MATPOWER demand and shunt fields become those
//! records during parsing. [`IndexedNetwork`](crate::IndexedNetwork) provides
//! the dense analysis view used by matrix builders.
//!
//! A network can retain its source bytes and [`SourceFormat`] for same format
//! writing. Each element also has an [`Extras`] map for source fields not named
//! by the typed model.
//!
//! Formats represent different data. Cross format writers report unsupported
//! fields rather than claiming an exact conversion.

// `crate::nonfinite` is text, not a link: the adapters moved into
// powerio-core's hidden implementation module, and schemars copies these doc
// comments verbatim into the frozen 0.9 document schema, so the wording cannot
// change to match.
#![expect(
    rustdoc::broken_intra_doc_links,
    reason = "the frozen 0.9 schema records these doc strings byte for byte"
)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::geo::{GeoMeta, Location};
use crate::{Error, Result};

/// Source format fields the neutral model does not name, kept for round trips
/// and cross format conversion. Keys are field names; values are JSON scalars.
pub type Extras = BTreeMap<String, Value>;

/// System base frequency in hertz when a format records none. Power networks run
/// at 50 or 60 Hz; 60 is the default for the formats (MATPOWER, PowerModels,
/// egret) that carry no frequency field.
pub const DEFAULT_BASE_FREQUENCY: f64 = 60.0;

/// serde default for [`BalancedNetwork::base_frequency`], so JSON written before the
/// field existed still deserializes (the C ABI and Julia bridge ride on the JSON
/// transport).
fn default_base_frequency() -> f64 {
    DEFAULT_BASE_FREQUENCY
}

/// A source bus ID, preserved from the input format.
///
/// MATPOWER IDs are 1-based and can contain gaps. They are distinct from the
/// zero based dense indices produced by
/// [`IndexedNetwork::bus_index`](crate::IndexedNetwork::bus_index). JSON stores
/// this type as an integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct BusId(pub usize);

impl BusId {
    /// The largest id a network may carry. The C ABI reports bus ids as int64,
    /// so an id past this ceiling has no distinct value there;
    /// [`BalancedNetwork::validate`] refuses one.
    pub const MAX: Self = Self(i64::MAX as usize);

    #[must_use]
    pub const fn new(id: usize) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for BusId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Bus type per MATPOWER convention: 1=PQ, 2=PV, 3=ref/slack, 4=isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "UPPERCASE")]
#[repr(u8)]
#[non_exhaustive]
pub enum BusType {
    Pq = 1,
    Pv = 2,
    Ref = 3,
    Isolated = 4,
}

impl BusType {
    /// Map a MATPOWER bus-type code to the enum; unknown codes fall back to PQ.
    pub(crate) fn from_f64(v: f64) -> Self {
        match v as i32 {
            2 => Self::Pv,
            3 => Self::Ref,
            4 => Self::Isolated,
            _ => Self::Pq,
        }
    }

    /// The canonical short name (`"PQ"`, `"PV"`, `"REF"`, `"ISOLATED"`), shared
    /// by the bindings so their bus-type strings can't drift.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pq => "PQ",
            Self::Pv => "PV",
            Self::Ref => "REF",
            Self::Isolated => "ISOLATED",
        }
    }
}

/// A generator cost curve (`mpc.gencost` row).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct GenCost {
    /// 1 = piecewise linear, 2 = polynomial.
    pub model: u8,
    pub startup: f64,
    pub shutdown: f64,
    /// Number of cost coefficients (polynomial) or breakpoints (piecewise).
    pub ncost: usize,
    /// Raw coefficients, highest order first for the polynomial model:
    /// `[c_{k-1}, …, c1, c0]`.
    pub coeffs: Vec<f64>,
}

impl GenCost {
    /// Build a cost row from the values carried after `ncost`.
    ///
    /// Polynomial rows (`model == 2`) store `ncost` coefficients. Piecewise
    /// linear rows (`model == 1`) store flattened `(x, y)` breakpoint pairs, so
    /// `ncost` is half the coefficient count. Use [`GenCost::with_ncost`] for
    /// malformed source rows or callers that need to preserve an explicit
    /// `ncost`.
    #[must_use]
    pub fn new(model: u8, startup: f64, shutdown: f64, coeffs: Vec<f64>) -> Self {
        let ncost = if model == 1 {
            coeffs.len() / 2
        } else {
            coeffs.len()
        };
        Self {
            model,
            startup,
            shutdown,
            ncost,
            coeffs,
        }
    }

    #[must_use]
    pub fn with_ncost(
        model: u8,
        startup: f64,
        shutdown: f64,
        ncost: usize,
        coeffs: Vec<f64>,
    ) -> Self {
        Self {
            model,
            startup,
            shutdown,
            ncost,
            coeffs,
        }
    }

    /// `(q, c)` for the quadratic cost `½ q p² + c p` from a polynomial
    /// (model 2) row. MATPOWER stores `c2 p² + c1 p + c0`, so `q = 2·c2` and
    /// `c = c1`. Linear rows (`ncost == 2`) give `q = 0`. Piecewise (model 1)
    /// or cubic and higher return `None`.
    pub fn quadratic(&self) -> Option<(f64, f64)> {
        self.quadratic_with_constant().map(|(q, c, _)| (q, c))
    }

    /// `(q, c, c0)` for the quadratic cost `½ q p² + c p + c0` from a
    /// polynomial (model 2) row, keeping the constant term that
    /// [`quadratic`](Self::quadratic) drops. Linear rows (`ncost == 2`) give
    /// `q = 0`; constant rows (`ncost == 1`) give `q = c = 0`. Piecewise
    /// (model 1) or cubic and higher return `None`.
    pub fn quadratic_with_constant(&self) -> Option<(f64, f64, f64)> {
        if self.model != 2 {
            return None;
        }
        // Reject a row whose coefficient slice is shorter than `ncost` claims,
        // rather than reading the wrong powers by position.
        if self.coeffs.len() < self.ncost {
            return None;
        }
        // Matches on the stated arity, so a cubic row is refused even when its
        // leading coefficient is zero. `quadratic_with_constant_tol` is the
        // reader that lowers the order first.
        match self.ncost {
            3 => Some((2.0 * self.coeffs[0], self.coeffs[1], self.coeffs[2])),
            2 => Some((0.0, self.coeffs[0], self.coeffs[1])),
            1 => Some((0.0, 0.0, self.coeffs[0])),
            _ => None,
        }
    }

    /// Largest leading polynomial coefficient that
    /// [`quadratic_with_constant_tol`](Self::quadratic_with_constant_tol)
    /// reads as a rounding artifact of the source, not as a term of the curve.
    pub const LEADING_COEFF_TOL: f64 = 1e-12;

    /// `(q, c, c0)` as [`quadratic_with_constant`](Self::quadratic_with_constant)
    /// gives it, after the leading coefficients at or below `tol` come off the
    /// row.
    ///
    /// A model 2 row often carries a leading coefficient near `1e-17`, which
    /// the source produced by rounding. Such a row states a linear curve and
    /// reads as a quadratic one. Pass
    /// [`LEADING_COEFF_TOL`](Self::LEADING_COEFF_TOL) to strip the artifact,
    /// or `0.0` to strip an exact zero alone.
    pub fn quadratic_with_constant_tol(&self, tol: f64) -> Option<(f64, f64, f64)> {
        if self.model != 2 {
            return None;
        }
        if self.coeffs.len() < self.ncost {
            return None;
        }
        let row = &self.coeffs[..self.ncost];
        let mut first = 0;
        while first + 1 < row.len() && row[first].abs() <= tol {
            first += 1;
        }
        match row.len() - first {
            3 => Some((2.0 * row[first], row[first + 1], row[first + 2])),
            2 => Some((0.0, row[first], row[first + 1])),
            1 => Some((0.0, 0.0, row[first])),
            _ => None,
        }
    }
}

/// Which format a [`BalancedNetwork`] was read from. Drives the same format byte exact
/// echo on write.
///
/// Serializes as the same lowercase token [`name`](SourceFormat::name) reports
/// and every string entry point accepts, so the value a document carries is
/// valid input to `from`. Documents written before 0.9.0 spelled the bare
/// variant name ("Matpower"); the read side still accepts those spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum SourceFormat {
    #[serde(rename = "matpower", alias = "Matpower")]
    Matpower,
    #[serde(rename = "powermodels-json", alias = "PowerModelsJson")]
    PowerModelsJson,
    #[serde(rename = "egret-json", alias = "EgretJson")]
    EgretJson,
    #[serde(rename = "psse", alias = "Psse")]
    Psse,
    #[serde(rename = "powerworld", alias = "PowerWorld")]
    PowerWorld,
    #[serde(rename = "pandapower-json", alias = "PandapowerJson")]
    PandapowerJson,
    /// Read from a GE PSLF `.epc` case. Same source text is retained, so a
    /// same-format write echoes it byte-for-byte; a cross-format or
    /// source-dropped write goes through the `.epc` serializer
    /// ([`write_pslf`](crate::write_pslf)).
    #[serde(rename = "pslf", alias = "Pslf")]
    Pslf,
    /// Read from a PowerWorld `.pwb` binary case. Read only: there is no
    /// `.pwb` writer and no retained source text, so writing goes through
    /// another format's writer.
    #[serde(rename = "powerworld-pwb", alias = "PowerWorldBinary")]
    PowerWorldBinary,
    /// Built in memory, for example from synth or an edited case; no source text.
    #[serde(rename = "in-memory", alias = "InMemory")]
    InMemory,
    /// A normalized derived form ([`BalancedNetwork::to_normalized`]): per unit, radians,
    /// filtered, source bus ids preserved. Distinct from
    /// [`InMemory`](SourceFormat::InMemory) so consumers can tell a per unit
    /// product from a raw in memory network; it has no source text and a different
    /// unit basis than a parsed network.
    #[serde(rename = "normalized", alias = "Normalized")]
    Normalized,
    /// Read back from a gridfm-datakit Parquet dataset (the ML→classical bridge,
    /// `powerio-matrix`'s `read_gridfm_dataset`). A lossy, power flow complete
    /// reconstruction with no retained source text: original bus ids are
    /// synthesized `1..n`, per element load/shunt granularity is folded to one
    /// synthetic element per bus, and HVDC/storage/piecewise costs are absent.
    #[serde(rename = "gridfm", alias = "Gridfm")]
    Gridfm,
    /// Read from a PyPSA CSV folder. This is a folder format rather than a
    /// single retained text document, so same-format writes are canonicalized.
    #[serde(rename = "pypsa-csv", alias = "PypsaCsv")]
    PypsaCsv,
    /// Read from an ARPA-E GO Challenge 3 JSON input document. The source is a
    /// unit commitment data set; the neutral transmission model keeps a static
    /// first interval network and retains the source text for the full data.
    #[serde(rename = "goc3-json", alias = "Goc3Json")]
    Goc3Json,
    /// Read from a Surge native JSON document.
    #[serde(rename = "surge-json", alias = "SurgeJson")]
    SurgeJson,
    /// Read from one raw JSON document in a DeepMind OPFData release. The
    /// source carries both solver initial values and a solution. The balanced
    /// model represents the solved snapshot and retains the source for an
    /// exact write back to the same format.
    #[serde(rename = "opfdata-json", alias = "DeepMindOpfDataJson")]
    DeepMindOpfDataJson,
}

impl SourceFormat {
    /// Stable lowercase token for provenance and reporting (package origin,
    /// CLI summaries, Python bindings). The match is exhaustive here so a new
    /// variant fails compilation at the one mapping instead of silently
    /// reporting "unknown" from a downstream wildcard copy.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            SourceFormat::Matpower => "matpower",
            SourceFormat::PowerModelsJson => "powermodels-json",
            SourceFormat::EgretJson => "egret-json",
            SourceFormat::Psse => "psse",
            SourceFormat::PowerWorld => "powerworld",
            SourceFormat::PandapowerJson => "pandapower-json",
            SourceFormat::Pslf => "pslf",
            SourceFormat::PowerWorldBinary => "powerworld-pwb",
            SourceFormat::InMemory => "in-memory",
            SourceFormat::Normalized => "normalized",
            SourceFormat::Gridfm => "gridfm",
            SourceFormat::PypsaCsv => "pypsa-csv",
            SourceFormat::Goc3Json => "goc3-json",
            SourceFormat::SurgeJson => "surge-json",
            SourceFormat::DeepMindOpfDataJson => "opfdata-json",
        }
    }
}

/// A balanced network with stable source bus IDs and separate element tables:
/// an immutable cheap to clone owning handle over private shared tables.
///
/// Cloning the handle bumps one reference count and clones no table
/// allocation. Reads go through the per field accessors; the `*_mut`
/// accessors copy the shared tables once on first write to a shared handle
/// (copy on write), so no other handle ever observes a mutation. The choice
/// of whole value sharing is private and settled by the allocation
/// benchmarks in `evals/allocation`.
#[derive(Debug, Clone)]
pub struct BalancedNetwork {
    tables: std::sync::Arc<BalancedNetworkTables>,
}

impl BalancedNetwork {
    pub(crate) fn from_tables(tables: BalancedNetworkTables) -> Self {
        Self {
            tables: std::sync::Arc::new(tables),
        }
    }

    /// The one mutation door: copies the shared tables on first write to a
    /// shared handle, so no other handle observes the change.
    pub(crate) fn tables_mut(&mut self) -> &mut BalancedNetworkTables {
        std::sync::Arc::make_mut(&mut self.tables)
    }
}

/// A balanced network with stable source bus IDs and separate element tables.
///
/// `remote = "Self"` turns the derived serde impls into inherent functions;
/// the trait impls beneath the struct route them through [`crate::nonfinite`],
/// so a nonfinite float spells as a string on every JSON route.
// The one owned table store behind the `BalancedNetwork` handle. The doc
// string above is frozen into the generated 0.9 schema description, so the
// handle split leaves it as it was; the schema also keeps the handle's name
// through the schemars rename below.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(rename = "BalancedNetwork"))]
#[serde(remote = "Self")]
pub(crate) struct BalancedNetworkTables {
    pub name: String,
    pub base_mva: f64,
    /// System base frequency in hertz (50 or 60). Threaded through the formats
    /// that record it (PSS/E `BASFRQ`, pandapower `f_hz`) and defaulted to
    /// [`DEFAULT_BASE_FREQUENCY`] for the rest. Load-bearing for any
    /// reactance↔henry conversion (pandapower line charging) and reported as a
    /// fidelity loss when a non-default value writes to a format with no
    /// frequency field.
    #[serde(default = "default_base_frequency")]
    pub base_frequency: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoMeta>,
    pub buses: std::sync::Arc<Vec<Bus>>,
    pub loads: std::sync::Arc<Vec<Load>>,
    pub shunts: std::sync::Arc<Vec<Shunt>>,
    pub branches: std::sync::Arc<Vec<Branch>>,
    #[serde(default)]
    pub switches: std::sync::Arc<Vec<Switch>>,
    pub generators: std::sync::Arc<Vec<Generator>>,
    pub storage: std::sync::Arc<Vec<Storage>>,
    pub hvdc: std::sync::Arc<Vec<Hvdc>>,
    /// Three-winding transformers, kept as typed records rather than folded into
    /// `branches`, so a star point and the per-winding data survive a round trip.
    /// `#[serde(default)]` so JSON written before the field existed still
    /// deserializes. [`IndexedNetwork`](crate::IndexedNetwork) lowers each
    /// in-service record into a star bus plus three branches (via
    /// [`Transformer3W::star_expansion`]) before building any matrix, so a
    /// 3-winding transformer does appear in `Y_bus`/connectivity; the canonical
    /// model keeps the typed record for round-trip fidelity.
    #[serde(default)]
    pub transformers_3w: std::sync::Arc<Vec<Transformer3W>>,
    /// Area records: scheduled interchange and per-area swing bus. Distinct from
    /// the bare `area` number on each [`Bus`]; this is the area's metadata, which
    /// every conversion dropped before. `#[serde(default)]` so older JSON still
    /// deserializes.
    #[serde(default)]
    pub areas: std::sync::Arc<Vec<Area>>,
    /// Solver / solution-control metadata when the source carries it, else `None`.
    /// `#[serde(default)]` so older JSON still deserializes.
    #[serde(default)]
    pub solver: Option<SolverParams>,
    pub source_format: SourceFormat,
}

impl Serialize for BalancedNetwork {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        BalancedNetworkTables::serialize(
            &self.tables,
            powerio_core::__implementation::nonfinite::NonFiniteSer(serializer),
        )
    }
}

impl<'de> Deserialize<'de> for BalancedNetwork {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        BalancedNetworkTables::deserialize(powerio_core::__implementation::nonfinite::NonFiniteDe(
            deserializer,
        ))
        .map(BalancedNetwork::from_tables)
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for BalancedNetwork {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        <BalancedNetworkTables as schemars::JsonSchema>::schema_name()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        <BalancedNetworkTables as schemars::JsonSchema>::schema_id()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <BalancedNetworkTables as schemars::JsonSchema>::json_schema(generator)
    }
}

macro_rules! table_accessors {
    ($($(#[$doc:meta])* $field:ident, $field_mut:ident: $ty:ty;)+) => {
        impl BalancedNetwork {
            $(
                $(#[$doc])*
                #[must_use]
                pub fn $field(&self) -> &$ty {
                    &self.tables.$field
                }

                /// Mutable access to the same table; a shared handle copies
                /// its tables once here, so no other handle observes the
                /// change.
                #[must_use]
                pub fn $field_mut(&mut self) -> &mut $ty {
                    &mut self.tables_mut().$field
                }
            )+
        }
    };
}

table_accessors! {
    /// The case name.
    name, name_mut: String;
    /// The geographic metadata when the source carries any.
    geo, geo_mut: Option<GeoMeta>;
    /// Solver / solution-control metadata when the source carries it.
    solver, solver_mut: Option<SolverParams>;
}

/// The element tables sit behind their own shared allocation inside the
/// shared table set, so a time series of networks that varies one table
/// clones only that table per point while every untouched table stays one
/// allocation across the whole series.
macro_rules! shared_table_accessors {
    ($($(#[$doc:meta])* $field:ident, $field_mut:ident: $ty:ty;)+) => {
        impl BalancedNetwork {
            $(
                $(#[$doc])*
                #[must_use]
                pub fn $field(&self) -> &$ty {
                    &self.tables.$field
                }

                /// Mutable access to the same table. A shared handle copies
                /// the table set spine and this one table here, so no other
                /// handle observes the change and untouched tables stay
                /// shared.
                #[must_use]
                pub fn $field_mut(&mut self) -> &mut $ty {
                    std::sync::Arc::make_mut(&mut self.tables_mut().$field)
                }
            )+
        }
    };
}

impl BalancedNetwork {
    /// Replace each element table's allocation with `donor`'s wherever the
    /// contents are equal, so equal tables across derived networks (the
    /// scenarios of one dataset, the points of one series) are stored once.
    /// No value changes; a table that differs anywhere keeps its own
    /// allocation.
    pub fn share_equal_tables(&mut self, donor: &Self) {
        macro_rules! share {
            ($($field:ident),+) => {
                $(
                    if !std::sync::Arc::ptr_eq(&self.tables.$field, &donor.tables.$field)
                        && self.tables.$field == donor.tables.$field
                    {
                        self.tables_mut().$field = donor.tables.$field.clone();
                    }
                )+
            };
        }
        share!(
            buses,
            loads,
            shunts,
            branches,
            switches,
            generators,
            storage,
            hvdc,
            transformers_3w,
            areas
        );
    }
}

shared_table_accessors! {
    buses, buses_mut: Vec<Bus>;
    loads, loads_mut: Vec<Load>;
    shunts, shunts_mut: Vec<Shunt>;
    branches, branches_mut: Vec<Branch>;
    switches, switches_mut: Vec<Switch>;
    generators, generators_mut: Vec<Generator>;
    storage, storage_mut: Vec<Storage>;
    hvdc, hvdc_mut: Vec<Hvdc>;
    /// Three-winding transformers, kept as typed records.
    transformers_3w, transformers_3w_mut: Vec<Transformer3W>;
    /// Area records: scheduled interchange and per-area swing bus.
    areas, areas_mut: Vec<Area>;
}

impl BalancedNetwork {
    /// System MVA base.
    #[must_use]
    pub fn base_mva(&self) -> f64 {
        self.tables.base_mva
    }

    #[must_use]
    pub fn base_mva_mut(&mut self) -> &mut f64 {
        &mut self.tables_mut().base_mva
    }

    /// System base frequency in hertz (50 or 60).
    #[must_use]
    pub fn base_frequency(&self) -> f64 {
        self.tables.base_frequency
    }

    #[must_use]
    pub fn base_frequency_mut(&mut self) -> &mut f64 {
        &mut self.tables_mut().base_frequency
    }

    /// The format the case was parsed from.
    #[must_use]
    pub fn source_format(&self) -> SourceFormat {
        self.tables.source_format
    }

    #[must_use]
    pub fn source_format_mut(&mut self) -> &mut SourceFormat {
        &mut self.tables_mut().source_format
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Bus {
    /// Stable bus id (1-based in MATPOWER; preserved verbatim).
    pub id: BusId,
    pub kind: BusType,
    /// Voltage magnitude (p.u.).
    pub vm: f64,
    /// Voltage angle (degrees).
    pub va: f64,
    pub base_kv: f64,
    pub vmax: f64,
    pub vmin: f64,
    /// Emergency (short-term) voltage band, set only when the source states one
    /// distinct from the normal [`vmax`](Bus::vmax)/[`vmin`](Bus::vmin) band (PSS/E
    /// `EVHI`/`EVLO`). `None` means the emergency band equals the normal band, so
    /// read `evhi.unwrap_or(vmax)` / `evlo.unwrap_or(vmin)`. `#[serde(default)]` so
    /// JSON written before the fields existed still deserializes.
    #[serde(default)]
    pub evhi: Option<f64>,
    #[serde(default)]
    pub evlo: Option<f64>,
    pub area: usize,
    pub zone: usize,
    pub name: Option<String>,
    /// Stable row identity for `.pio.json` payloads and operating point updates:
    /// the source record uid where the format defines one (GOC3), synthesized at
    /// package build otherwise. `#[serde(default)]` so JSON written before the
    /// field existed still deserializes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// Optional bus coordinates in the network coordinate space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    pub extras: Extras,
}

impl Bus {
    #[must_use]
    pub fn new(id: BusId, kind: BusType, base_kv: f64) -> Self {
        Self {
            id,
            kind,
            vm: 1.0,
            va: 0.0,
            base_kv,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: None,
            uid: None,
            location: None,
            extras: Extras::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Load {
    pub bus: BusId,
    /// Active demand (MW).
    pub p: f64,
    /// Reactive demand (MVAr).
    pub q: f64,
    /// Voltage dependence, when the source states one. `None` is constant power.
    #[serde(default)]
    pub voltage_model: Option<LoadVoltageModel>,
    pub in_service: bool,
    /// Stable row identity; see [`Bus::uid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    pub extras: Extras,
}

impl Load {
    #[must_use]
    pub fn new(bus: BusId, p: f64, q: f64) -> Self {
        Self {
            bus,
            p,
            q,
            voltage_model: None,
            in_service: true,
            uid: None,
            extras: Extras::new(),
        }
    }
}

/// Voltage dependence for a transmission load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoadVoltageModel {
    /// Explicit constant power marker.
    ConstantPower,
    /// ZIP load split in source units. The three active parts sum to
    /// [`Load::p`], and the three reactive parts sum to [`Load::q`].
    Zip {
        p_constant_power: f64,
        q_constant_power: f64,
        p_constant_current: f64,
        q_constant_current: f64,
        p_constant_impedance: f64,
        q_constant_impedance: f64,
        #[serde(default)]
        v_nom: Option<f64>,
        /// Source load type code, when a format has one (PSS/E `ID`/`LOADTYPE`
        /// style metadata).
        #[serde(default)]
        load_type: Option<i32>,
        /// Source scaling factor, when a format has one.
        #[serde(default)]
        scaling: Option<f64>,
    },
    /// Exponential voltage model: `P = p * (V / v_nom)^gamma_p`,
    /// `Q = q * (V / v_nom)^gamma_q`.
    Exponential {
        p: f64,
        q: f64,
        #[serde(default)]
        v_nom: Option<f64>,
        gamma_p: f64,
        gamma_q: f64,
    },
}

impl LoadVoltageModel {
    #[must_use]
    pub fn has_non_matpower_fields(&self) -> bool {
        match self {
            Self::ConstantPower => false,
            Self::Zip {
                p_constant_current,
                q_constant_current,
                p_constant_impedance,
                q_constant_impedance,
                v_nom,
                load_type,
                scaling,
                ..
            } => {
                *p_constant_current != 0.0
                    || *q_constant_current != 0.0
                    || *p_constant_impedance != 0.0
                    || *q_constant_impedance != 0.0
                    || v_nom.is_some()
                    || load_type.is_some()
                    || scaling.is_some()
            }
            Self::Exponential { .. } => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Shunt {
    pub bus: BusId,
    /// Shunt conductance (MW at V = 1 p.u.).
    pub g: f64,
    /// Shunt susceptance (MVAr at V = 1 p.u.). For a switched shunt this is the
    /// initial (steady-state) value within the [`control`](Shunt::control) blocks.
    pub b: f64,
    pub in_service: bool,
    /// Switching-control data when this is a switched (adjustable) shunt; `None`
    /// for a fixed shunt. `#[serde(default)]` so JSON written before the field
    /// existed still deserializes.
    #[serde(default)]
    pub control: Option<SwitchedShuntControl>,
    /// Stable row identity; see [`Bus::uid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    pub extras: Extras,
}

impl Shunt {
    #[must_use]
    pub fn new(bus: BusId, g: f64, b: f64) -> Self {
        Self {
            bus,
            g,
            b,
            in_service: true,
            control: None,
            uid: None,
            extras: Extras::new(),
        }
    }
}

/// How a switched shunt adjusts its susceptance. Maps to the PSS/E `MODSW` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SwitchedShuntMode {
    /// Fixed at its initial susceptance, no automatic switching (`MODSW` 0).
    Locked,
    /// Continuous adjustment within the block range (`MODSW` 1).
    Continuous,
    /// Discrete adjustment in fixed steps (`MODSW` 2 and up).
    Discrete,
}

/// One block of a switched shunt: `steps` equal increments of susceptance `b`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ShuntBlock {
    pub steps: u32,
    /// Susceptance increment per step (MVAr at V = 1 p.u.).
    pub b: f64,
}

impl ShuntBlock {
    #[must_use]
    pub const fn new(steps: u32, b: f64) -> Self {
        Self { steps, b }
    }
}

/// Switching-control data for a switched shunt ([`Shunt::control`]): the mode,
/// the regulated voltage band and bus, the reactive-range percentage, and the
/// adjustable susceptance blocks. The shunt's [`b`](Shunt::b) is the initial
/// value within the blocks' total range.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SwitchedShuntControl {
    pub mode: SwitchedShuntMode,
    /// Regulated voltage band (per unit).
    pub vhigh: f64,
    pub vlow: f64,
    /// The regulated bus; `None` means the shunt regulates its own bus.
    pub control_bus: Option<BusId>,
    /// Percent of the controlled device's reactive range to apply (PSS/E `RMPCT`).
    pub rmpct: f64,
    pub blocks: Vec<ShuntBlock>,
}

impl SwitchedShuntControl {
    #[must_use]
    pub fn new(mode: SwitchedShuntMode, vhigh: f64, vlow: f64, blocks: Vec<ShuntBlock>) -> Self {
        Self {
            mode,
            vhigh,
            vlow,
            control_bus: None,
            rmpct: 100.0,
            blocks,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Branch {
    pub from: BusId,
    pub to: BusId,
    /// Series resistance (p.u.).
    pub r: f64,
    /// Series reactance (p.u.).
    pub x: f64,
    /// MATPOWER compatible total line charging susceptance (p.u.). This is the
    /// legacy total projection; when [`charging`](Branch::charging) is present,
    /// per terminal admittance is canonical and this field is compatibility data.
    pub b: f64,
    /// Per terminal shunt admittance (p.u.). If absent, derive symmetric
    /// susceptance from [`b`](Branch::b).
    #[serde(default)]
    pub charging: Option<BranchCharging>,
    pub rate_a: f64,
    pub rate_b: f64,
    pub rate_c: f64,
    /// Additional MVA rating sets beyond A/B/C. Matrix builders continue to use
    /// `rate_a` unless they opt into one of these named sets.
    #[serde(default)]
    pub rating_sets: Vec<BranchRatingSet>,
    /// Current ratings, when the source distinguishes them from MVA ratings.
    #[serde(default)]
    pub current_ratings: Option<BranchCurrentRatings>,
    /// Tap ratio, MATPOWER convention: 0 means "no tap" (a line), treated as 1.
    pub tap: f64,
    /// Phase shift (degrees).
    pub shift: f64,
    pub in_service: bool,
    pub angmin: f64,
    pub angmax: f64,
    /// Regulating-transformer control data, when this branch is a transformer
    /// under automatic tap or phase control. `None` for lines and for fixed-ratio
    /// transformers. `#[serde(default)]` so JSON written before the field existed
    /// still deserializes.
    #[serde(default)]
    pub control: Option<TransformerControl>,
    /// Solved branch flow values, when present in a case snapshot.
    #[serde(default)]
    pub solution: Option<BranchSolution>,
    /// Stable row identity; see [`Bus::uid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// Polyline route in the network's coordinate space (`BalancedNetwork.geo`),
    /// present only when a source provides intermediate geometry; endpoint
    /// only rendering derives from the bus locations. `#[serde(default)]` so
    /// JSON written before the field existed still deserializes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Vec<Location>>,
    pub extras: Extras,
}

/// Extra branch MVA rating set beyond the canonical A/B/C columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct BranchRatingSet {
    pub name: String,
    pub rate_mva: f64,
}

impl BranchRatingSet {
    #[must_use]
    pub fn new(name: impl Into<String>, rate_mva: f64) -> Self {
        Self {
            name: name.into(),
            rate_mva,
        }
    }
}

/// Per terminal branch shunt admittance in p.u. This is the canonical
/// physical branch shunt model when present.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct BranchCharging {
    pub g_fr: f64,
    pub b_fr: f64,
    pub g_to: f64,
    pub b_to: f64,
}

impl BranchCharging {
    #[must_use]
    pub const fn new(g_fr: f64, b_fr: f64, g_to: f64, b_to: f64) -> Self {
        Self {
            g_fr,
            b_fr,
            g_to,
            b_to,
        }
    }

    #[must_use]
    pub fn from_total_b(b: f64) -> Self {
        Self {
            g_fr: 0.0,
            b_fr: b / 2.0,
            g_to: 0.0,
            b_to: b / 2.0,
        }
    }

    #[must_use]
    pub fn total_b(self) -> f64 {
        self.b_fr + self.b_to
    }

    #[must_use]
    pub fn total_g(self) -> f64 {
        self.g_fr + self.g_to
    }

    #[must_use]
    pub fn is_matpower_symmetric(self) -> bool {
        self.g_fr.abs() <= f64::EPSILON
            && self.g_to.abs() <= f64::EPSILON
            && (self.b_fr - self.b_to).abs() <= f64::EPSILON
    }
}

/// Current limits for a branch, in source units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct BranchCurrentRatings {
    pub c_rating_a: f64,
    pub c_rating_b: f64,
    pub c_rating_c: f64,
}

impl BranchCurrentRatings {
    #[must_use]
    pub const fn new(c_rating_a: f64, c_rating_b: f64, c_rating_c: f64) -> Self {
        Self {
            c_rating_a,
            c_rating_b,
            c_rating_c,
        }
    }
}

/// Solved branch terminal flows in MW/MVAr.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct BranchSolution {
    pub pf: f64,
    pub qf: f64,
    pub pt: f64,
    pub qt: f64,
}

impl BranchSolution {
    #[must_use]
    pub const fn new(pf: f64, qf: f64, pt: f64, qt: f64) -> Self {
        Self { pf, qf, pt, qt }
    }
}

impl Branch {
    #[must_use]
    pub fn new(from: BusId, to: BusId, r: f64, x: f64) -> Self {
        Self {
            from,
            to,
            r,
            x,
            b: 0.0,
            charging: None,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: 0.0,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            solution: None,
            uid: None,
            route: None,
            extras: Extras::new(),
        }
    }

    /// Effective tap ratio (0 ⇒ 1).
    #[must_use]
    pub fn effective_tap(&self) -> f64 {
        if self.tap == 0.0 { 1.0 } else { self.tap }
    }

    /// [`effective_tap`](Self::effective_tap) for a builder that divides by it,
    /// which the remap of an exact 0.0 does not make safe on its own.
    ///
    /// # Errors
    /// [`Error::DegenerateTap`] under
    /// [`MIN_DIVISIBLE_MAGNITUDE`](crate::dc::MIN_DIVISIBLE_MAGNITUDE), where a
    /// tap scales an admittance past anything a matrix can carry. `row` only
    /// labels the error.
    pub fn divisible_tap(&self, row: usize) -> Result<f64> {
        let tap = self.effective_tap();
        if !tap.is_finite() || tap.abs() < crate::dc::MIN_DIVISIBLE_MAGNITUDE {
            return Err(Error::DegenerateTap { row, tap });
        }
        Ok(tap)
    }

    /// Per terminal shunt admittance, deriving the legacy symmetric MATPOWER
    /// charging model when the richer field is absent.
    #[must_use]
    pub fn terminal_charging(&self) -> BranchCharging {
        self.charging
            .unwrap_or_else(|| BranchCharging::from_total_b(self.b))
    }

    /// Series admittance `(g, b) = (r, −x) / (r² + x²)` of the branch pi
    /// model, the primitive beside [`effective_tap`](Self::effective_tap) and
    /// [`terminal_charging`](Self::terminal_charging). `Ok(None)` for a zero
    /// impedance branch — one whose impedance magnitude is under
    /// [`MIN_DIVISIBLE_MAGNITUDE`](crate::dc::MIN_DIVISIBLE_MAGNITUDE); the
    /// caller decides whether that is a skip or an error.
    ///
    /// # Errors
    /// [`Error::NonFiniteSusceptance`] when `r`/`x` are NaN/Inf, so a bad
    /// value cannot write NaN or a silent zero downstream. `row` only labels
    /// the error.
    pub fn series_admittance(&self, row: usize) -> Result<Option<(f64, f64)>> {
        series_admittance_of(self.r, self.x, row)
    }

    /// Apparent power bound, per unit, for a branch the source left unrated
    /// (`rate_a == 0`, which reads as unlimited). `angle_window_rad` is the
    /// widest angle difference the branch may hold, in radians. That window and
    /// the two terminal voltage bands give the widest voltage phasor difference
    /// the branch can hold. The difference over `|Z|` bounds the current, and
    /// the larger ceiling turns the current into power. Returns `0.0` for a zero
    /// impedance branch — one under
    /// [`MIN_DIVISIBLE_MAGNITUDE`](crate::dc::MIN_DIVISIBLE_MAGNITUDE), the
    /// bound the rest of the builders divide by — which stays unlimited.
    ///
    /// Both ends of each band are needed, not just the ceilings. `|V_f e^{jδ} −
    /// V_t|²` is convex in `(V_f, V_t)`, so its largest value over the voltage
    /// box sits at a corner — and below a window of roughly 10° that corner is
    /// the mixed one, one terminal high and the other low, not both high.
    /// Reading only the ceilings there understates the bound several fold and
    /// hands an OPF a limit tighter than the branch physically has.
    ///
    /// The caller supplies the window in radians, because
    /// [`angmin`](Self::angmin) and [`angmax`](Self::angmax) are degrees in
    /// the neutral model and radians in a normalized network, and a branch
    /// cannot tell which it holds. Convert them with
    /// [`IndexedNetwork::angle_radians`](crate::IndexedNetwork::angle_radians),
    /// which reads the convention of the network. The method takes the
    /// magnitude of the window and holds it at `π`, the widest phasor
    /// separation two terminals can have.
    #[must_use]
    pub fn synthesize_rate_a(
        &self,
        angle_window_rad: f64,
        (fr_vmin, fr_vmax): (f64, f64),
        (to_vmin, to_vmax): (f64, f64),
    ) -> f64 {
        // The same bound `series_admittance_of` divides by, so the two agree on
        // which branch has no impedance to bound a current with.
        let zmag = self.r.hypot(self.x);
        if zmag < crate::dc::MIN_DIVISIBLE_MAGNITUDE {
            return 0.0;
        }
        let window = angle_window_rad.abs().min(std::f64::consts::PI);
        let cos_window = window.cos();
        // Clamped at zero before the root: the law of cosines is nonnegative in
        // exact arithmetic, and rounding on two nearly equal voltages can carry
        // it a few ulp under.
        let separation = |vf: f64, vt: f64| {
            (vf * vf + vt * vt - 2.0 * vf * vt * cos_window)
                .max(0.0)
                .sqrt()
        };
        let widest = separation(fr_vmax, to_vmax)
            .max(separation(fr_vmax, to_vmin))
            .max(separation(fr_vmin, to_vmax))
            .max(separation(fr_vmin, to_vmin));
        fr_vmax.max(to_vmax) * widest / zmag
    }

    /// Total susceptance projection for MATPOWER shaped formats that only carry
    /// one line charging value.
    #[must_use]
    pub fn total_charging_b(&self) -> f64 {
        self.terminal_charging().total_b()
    }

    /// Whether this branch has charging that a MATPOWER branch row cannot carry.
    #[must_use]
    pub fn has_non_matpower_charging(&self) -> bool {
        self.charging
            .is_some_and(|charging| !charging.is_matpower_symmetric())
    }

    /// A transformer iff the raw tap field is nonzero (an explicit `1` counts) or
    /// there is a phase shift.
    #[must_use]
    pub fn is_transformer(&self) -> bool {
        self.tap != 0.0 || self.shift != 0.0
    }

    /// True when the branch constrains its angle difference, i.e. the limits
    /// deviate from the ±360° "unconstrained" default. Formats without angle
    /// limit fields (PSS/E, PowerWorld) use this to warn on what they drop.
    #[must_use]
    pub fn has_angle_limits(&self) -> bool {
        self.angmin > -360.0 || self.angmax < 360.0
    }
}

/// The series admittance `(g, b)` of an impedance, guarded.
///
/// `None` is an impedance too small to divide by, under
/// [`MIN_DIVISIBLE_MAGNITUDE`](crate::dc::MIN_DIVISIBLE_MAGNITUDE); the caller
/// decides whether that is a skip or an error. The bound is on the impedance
/// magnitude, not on `r² + x²`, which is its square: bounding the square would
/// refuse impedances the DC builders divide by.
///
/// Y_bus takes `r` already zeroed under the XB scheme, so it passes its own
/// pair rather than a branch's.
///
/// # Errors
/// [`Error::NonFiniteSusceptance`] when `r`/`x` are NaN/Inf, so a bad value
/// cannot write NaN or a silent zero downstream. NaN leaves `hypot` NaN, which
/// is not below the bound, so it arrives at that check rather than reading as
/// zero impedance. `row` only labels the error.
pub fn series_admittance_of(r: f64, x: f64, row: usize) -> Result<Option<(f64, f64)>> {
    let magnitude = r.hypot(x);
    if magnitude < crate::dc::MIN_DIVISIBLE_MAGNITUDE {
        return Ok(None);
    }
    if !magnitude.is_finite() {
        return Err(Error::NonFiniteSusceptance { row });
    }
    Ok(Some(crate::dc::series_admittance_parts(r, x)))
}

/// A transmission switch. Closed switches are preserved as data; matrix builders
/// do not lower them into zero impedance branches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Switch {
    pub from: BusId,
    pub to: BusId,
    pub closed: bool,
    #[serde(default)]
    pub thermal_rating: Option<f64>,
    #[serde(default)]
    pub current_rating: Option<f64>,
    #[serde(default)]
    pub pf: Option<f64>,
    #[serde(default)]
    pub qf: Option<f64>,
    #[serde(default)]
    pub pt: Option<f64>,
    #[serde(default)]
    pub qt: Option<f64>,
    /// Stable row identity; see [`Bus::uid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    pub extras: Extras,
}

impl Switch {
    #[must_use]
    pub fn new(from: BusId, to: BusId, closed: bool) -> Self {
        Self {
            from,
            to,
            closed,
            thermal_rating: None,
            current_rating: None,
            pf: None,
            qf: None,
            pt: None,
            qt: None,
            uid: None,
            extras: Extras::new(),
        }
    }
}

/// What a regulating transformer's tap (or phase shift) automatically controls.
/// Maps to the PSS/E control code `COD` and the PSLF transformer `type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TransformerControlMode {
    /// Fixed ratio, no automatic adjustment (PSS/E `COD` 0/±4, PSLF type 1).
    Fixed,
    /// Bus voltage control via tap (LTC; PSS/E `COD` ±1, PSLF type 2).
    Voltage,
    /// Reactive power flow control via tap (PSS/E `COD` ±2).
    ReactiveFlow,
    /// Active power flow control via phase shift (PSS/E `COD` ±3, PSLF type 4).
    ActiveFlow,
}

/// Automatic-control data for a regulating transformer ([`Branch::control`]).
///
/// The limits carry whatever the [`mode`](TransformerControl::mode) regulates:
/// `tap_min`/`tap_max` bound the tap ratio (or the phase angle, for
/// [`ActiveFlow`](TransformerControlMode::ActiveFlow)), and `band_min`/`band_max`
/// bound the controlled quantity (the regulated voltage band, or the
/// scheduled MW/MVAr). `ntp` is the number of discrete tap positions and
/// `controlled_bus` is the regulated bus (`None` = the transformer's own
/// terminal). `mva_base` is the winding MVA base the impedance is referred to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct TransformerControl {
    pub mode: TransformerControlMode,
    pub controlled_bus: Option<BusId>,
    pub tap_min: f64,
    pub tap_max: f64,
    pub band_min: f64,
    pub band_max: f64,
    pub ntp: u32,
    pub mva_base: f64,
}

impl Default for TransformerControl {
    fn default() -> Self {
        // PSS/E's documented defaults for an unset winding-control block.
        TransformerControl {
            mode: TransformerControlMode::Fixed,
            controlled_bus: None,
            tap_min: 0.9,
            tap_max: 1.1,
            band_min: 0.9,
            band_max: 1.1,
            ntp: 33,
            mva_base: 0.0,
        }
    }
}

impl TransformerControl {
    #[must_use]
    pub fn new(mode: TransformerControlMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Generator {
    pub bus: BusId,
    /// Real power set point (MW).
    pub pg: f64,
    /// Reactive power set point (MVAr).
    pub qg: f64,
    pub pmax: f64,
    pub pmin: f64,
    pub qmax: f64,
    pub qmin: f64,
    /// Voltage set point (p.u.).
    pub vg: f64,
    pub mbase: f64,
    pub in_service: bool,
    pub cost: Option<GenCost>,
    /// The MATPOWER gen capability / ramp columns past `PMIN`, aligned to
    /// `GEN_EXTRA_KEYS` by index (`None` for a column the source omitted).
    /// A fixed array, not an [`Extras`] map: a string-keyed map per generator
    /// costs 11 heap allocations each, which dominates the parse of a large
    /// generator-heavy case. Surfaced into formats that name them (PowerModels).
    /// On the JSON snapshot it is a name-keyed object (see `caps_serde`) so the
    /// schema stays additive when `GEN_EXTRA_KEYS` grows; `#[serde(default)]` so a
    /// snapshot that omits it deserializes to the empty set.
    #[serde(default = "default_caps", with = "caps_serde")]
    #[cfg_attr(feature = "schema", schemars(with = "BTreeMap<String, f64>"))]
    pub caps: GenCaps,
    /// The remote bus whose voltage this generator regulates, when that is not its
    /// own terminal bus (PSS/E `IREG`). `None` means it regulates its own bus.
    /// Part of the cross-element voltage-control graph: a format that names a
    /// remote regulated bus (PSS/E) keeps it across a round trip instead of
    /// collapsing every generator onto its own terminal. `#[serde(default)]` so
    /// JSON written before the field existed still deserializes.
    #[serde(default)]
    pub regulated_bus: Option<BusId>,
    /// Stable row identity; see [`Bus::uid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

impl Generator {
    #[must_use]
    pub fn new(bus: BusId) -> Self {
        Self {
            bus,
            pg: 0.0,
            qg: 0.0,
            pmax: 0.0,
            pmin: 0.0,
            qmax: 0.0,
            qmin: 0.0,
            vg: 1.0,
            mbase: 0.0,
            in_service: true,
            cost: None,
            caps: default_caps(),
            regulated_bus: None,
            uid: None,
        }
    }

    /// True when any capability / ramp column is present. Formats without those
    /// fields (PSS/E, PowerWorld) use this to warn on what they drop.
    #[must_use]
    pub fn has_caps(&self) -> bool {
        self.caps.iter().any(Option::is_some)
    }
}

/// A generator's capability / ramp columns, one slot per `GEN_EXTRA_KEYS` name.
pub type GenCaps = [Option<f64>; GEN_EXTRA_KEYS.len()];

/// The empty capability set, for a JSON snapshot that omits the field entirely.
fn default_caps() -> GenCaps {
    [None; GEN_EXTRA_KEYS.len()]
}

/// Serialize [`GenCaps`] as a name-keyed object (`{"ramp_30": 1.2, ...}`) keyed by
/// [`GEN_EXTRA_KEYS`], emitting only the present slots, instead of a length-exact
/// array. A fixed-length array round-trips through serde only at exactly its
/// current length: the day `GEN_EXTRA_KEYS` grows a column, every old snapshot
/// fails to deserialize and every new one fails on an old build, and the C ABI
/// ties the JSON snapshot schema to its version, so that is a forced ABI break.
/// The named map makes a new key purely additive: an old document simply lacks it
/// (deserializes to `None`), and an unknown key from a newer document is ignored.
/// In memory `caps` stays a fixed array, so the per-generator allocation cost the
/// array avoids is unchanged; only the serialized form is named.
mod caps_serde {
    use super::{GEN_EXTRA_KEYS, GenCaps};
    use serde::de::{Deserialize, Deserializer};
    use serde::ser::{SerializeMap, Serializer};
    use std::collections::BTreeMap;

    pub(super) fn serialize<S: Serializer>(caps: &GenCaps, s: S) -> Result<S::Ok, S::Error> {
        let present = caps.iter().filter(|v| v.is_some()).count();
        let mut map = s.serialize_map(Some(present))?;
        for (key, slot) in GEN_EXTRA_KEYS.iter().zip(caps.iter()) {
            if let Some(value) = slot {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<GenCaps, D::Error> {
        // Accept an explicit `null` as the empty set (treated like an omitted
        // field), so a producer that encodes "no caps" as `null` round-trips the
        // same way `cost: Option<_>` does. `#[serde(default)]` only covers an
        // absent key, not a present `null`.
        let named = Option::<BTreeMap<String, f64>>::deserialize(d)?.unwrap_or_default();
        let mut caps: GenCaps = [None; GEN_EXTRA_KEYS.len()];
        for (slot, key) in caps.iter_mut().zip(GEN_EXTRA_KEYS.iter()) {
            *slot = named.get(*key).copied();
        }
        Ok(caps)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Storage {
    pub bus: BusId,
    pub ps: f64,
    pub qs: f64,
    pub energy: f64,
    pub energy_rating: f64,
    pub charge_rating: f64,
    pub discharge_rating: f64,
    pub charge_efficiency: f64,
    pub discharge_efficiency: f64,
    pub thermal_rating: f64,
    #[serde(default)]
    pub current_rating: Option<f64>,
    pub qmin: f64,
    pub qmax: f64,
    pub r: f64,
    pub x: f64,
    pub p_loss: f64,
    pub q_loss: f64,
    pub in_service: bool,
    /// Stable row identity; see [`Bus::uid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    pub extras: Extras,
}

impl Storage {
    #[must_use]
    pub fn new(bus: BusId) -> Self {
        Self {
            bus,
            ps: 0.0,
            qs: 0.0,
            energy: 0.0,
            energy_rating: 0.0,
            charge_rating: 0.0,
            discharge_rating: 0.0,
            charge_efficiency: 1.0,
            discharge_efficiency: 1.0,
            thermal_rating: 0.0,
            current_rating: None,
            qmin: 0.0,
            qmax: 0.0,
            r: 0.0,
            x: 0.0,
            p_loss: 0.0,
            q_loss: 0.0,
            in_service: true,
            uid: None,
            extras: Extras::new(),
        }
    }
}

/// A two-terminal HVDC line (MATPOWER `dcline`).
///
/// `pf`/`pt`/`qf`/`qt` are stored in MATPOWER's sign convention regardless of
/// source: the PowerModels reader un-flips `pt`/`qf`/`qt` on the way in, and the
/// PowerModels writer re-flips them on the way out (PowerModels.jl uses the
/// opposite sign). The flip is a format-boundary translation, so a derived view
/// like `to_normalized` keeps the MATPOWER convention and only scales to per unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Hvdc {
    pub from: BusId,
    pub to: BusId,
    pub in_service: bool,
    pub pf: f64,
    pub pt: f64,
    pub qf: f64,
    pub qt: f64,
    pub vf: f64,
    pub vt: f64,
    pub pmin: f64,
    pub pmax: f64,
    pub qminf: f64,
    pub qmaxf: f64,
    pub qmint: f64,
    pub qmaxt: f64,
    pub loss0: f64,
    pub loss1: f64,
    #[serde(default)]
    pub cost: Option<GenCost>,
    /// Stable row identity; see [`Bus::uid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    pub extras: Extras,
}

impl Hvdc {
    /// The power arriving at the `to` end for a sending end setpoint, under the
    /// MATPOWER dcline loss model `Pt = Pf - loss0 - loss1·Pf`.
    ///
    /// [`pf`](Self::pf), [`pt`](Self::pt), and [`loss0`](Self::loss0) are one
    /// relation, not three independent fields, and a format that states only
    /// the sending end reconstructs the far end from it. Stated here so every
    /// reader spells the same rule: `loss0` and `pf` scale together, so this
    /// holds in per unit as in MW.
    #[must_use]
    pub fn delivered_power(pf: f64, loss0: f64, loss1: f64) -> f64 {
        pf - loss0 - loss1 * pf
    }

    /// Whether [`pt`](Self::pt) agrees with this line's own loss model to
    /// `tol`. A writer whose format states no received power reports the lines
    /// that fail this, because those are the ones it cannot reproduce.
    #[must_use]
    pub fn pt_matches_loss_model(&self, tol: f64) -> bool {
        (self.pt - Self::delivered_power(self.pf, self.loss0, self.loss1)).abs() <= tol
    }

    #[must_use]
    pub fn new(from: BusId, to: BusId) -> Self {
        Self {
            from,
            to,
            in_service: true,
            pf: 0.0,
            pt: 0.0,
            qf: 0.0,
            qt: 0.0,
            vf: 1.0,
            vt: 1.0,
            pmin: 0.0,
            pmax: 0.0,
            qminf: 0.0,
            qmaxf: 0.0,
            qmint: 0.0,
            qmaxt: 0.0,
            loss0: 0.0,
            loss1: 0.0,
            cost: None,
            uid: None,
            extras: Extras::new(),
        }
    }
}

/// An area record: the area's scheduled net interchange and its swing bus.
///
/// The [`number`](Area::number) matches the `area` field carried on each
/// [`Bus`]; this table holds the per-area metadata (the interchange target and
/// the area slack) that the bus number alone can't. Maps to the PSS/E area record
/// (`I, ISW, PDES, PTOL, ARNAME`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Area {
    pub number: usize,
    /// The area swing (slack) bus, or `None` when unset.
    pub slack_bus: Option<BusId>,
    /// Scheduled net interchange (MW); positive is export out of the area.
    pub net_interchange: f64,
    /// Interchange tolerance bandwidth (MW).
    pub tolerance: f64,
    pub name: Option<String>,
}

impl Area {
    #[must_use]
    pub fn new(number: usize) -> Self {
        Self {
            number,
            slack_bus: None,
            net_interchange: 0.0,
            tolerance: 0.0,
            name: None,
        }
    }
}

/// Solver / solution-control metadata: the Newton tolerance and iteration cap,
/// the zero-impedance threshold, and the per-quantity adjustment-enable flags.
///
/// Each field is optional because a source states only the ones it carries. No
/// power flow physics, but it determines whether a downstream solver reproduces
/// the source tool's converged answer. Maps to the PSS/E v34+ system-wide block
/// (`GENERAL THRSHZ`, `NEWTON TOLN`/`ITMXN`, `SOLVER ACTAPS`/`AREAIN`/`PHSHFT`/
/// `DCTAPS`/`SWSHNT`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct SolverParams {
    /// Newton power flow mismatch tolerance (`NEWTON TOLN`).
    pub newton_tolerance: Option<f64>,
    /// Newton iteration cap (`NEWTON ITMXN`).
    pub max_iterations: Option<u32>,
    /// Branches with `|x|` below this are treated as zero impedance (`GENERAL THRSHZ`).
    pub zero_impedance_threshold: Option<f64>,
    /// Whether the solver adjusts transformer taps (`SOLVER ACTAPS`).
    pub adjust_taps: Option<bool>,
    /// Whether the solver adjusts area interchange (`SOLVER AREAIN`).
    pub adjust_area_interchange: Option<bool>,
    /// Whether the solver adjusts phase-shift angles (`SOLVER PHSHFT`).
    pub adjust_phase_shift: Option<bool>,
    /// Whether the solver adjusts DC line taps (`SOLVER DCTAPS`).
    pub adjust_dc_taps: Option<bool>,
    /// Whether the solver adjusts switched shunts (`SOLVER SWSHNT`).
    pub adjust_switched_shunt: Option<bool>,
}

impl SolverParams {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no field is set (so readers can avoid attaching an empty record).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == SolverParams::default()
    }
}

/// A series impedance with the MVA base it is expressed on. Used pairwise by
/// [`Transformer3W`]; a self-contained unit so the base travels with the value
/// instead of being implied by position.
///
/// `r`/`x` are per unit on the *system* base (the same `CZ = 1` convention as
/// [`Branch::r`]/[`Branch::x`], so the matrix math needs no rebasing); `base_mva`
/// records the winding-pair MVA base the source file declared (PSS/E `SBASE1-2`
/// and friends), kept so a write-back reproduces it and so a future `CZ = 2`
/// reader has somewhere to put the winding base it must rebase from. Room to grow
/// (winding voltage base, turns-ratio units) as the transformer control work
/// lands without reshaping the [`Transformer3W::z`] array.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Impedance {
    pub r: f64,
    pub x: f64,
    pub base_mva: f64,
}

impl Impedance {
    #[must_use]
    pub const fn new(r: f64, x: f64, base_mva: f64) -> Self {
        Self { r, x, base_mva }
    }
}

/// One winding of a [`Transformer3W`]: its terminal bus, off-nominal ratio, phase
/// shift, nominal voltage, and thermal ratings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Winding {
    pub bus: BusId,
    /// Off-nominal turns ratio (1.0 = nominal); the PSS/E `WINDV`, `CW = 1`.
    pub tap: f64,
    /// Phase shift (degrees).
    pub shift: f64,
    /// Winding nominal voltage (kV); 0 defers to the terminal bus base kV.
    pub nominal_kv: f64,
    pub rate_a: f64,
    pub rate_b: f64,
    pub rate_c: f64,
}

impl Winding {
    #[must_use]
    pub fn new(bus: BusId) -> Self {
        Self {
            bus,
            tap: 1.0,
            shift: 0.0,
            nominal_kv: 0.0,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
        }
    }
}

/// A three winding transformer with three terminal buses joined at a star point.
///
/// Series impedance is stored for winding pairs 1-2, 2-3, and 3-1. The record
/// also retains star point voltage and per winding control data. PSS/E three
/// winding records and PSLF tertiary winding records map to this type.
/// [`star_expansion`](Transformer3W::star_expansion) turns it into the synthetic
/// star bus plus three branches for a consumer that works in the bus-branch model;
/// [`IndexedNetwork`](crate::IndexedNetwork) applies it before building any matrix,
/// so a 3-winding transformer contributes to `Y_bus` and connectivity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Transformer3W {
    /// The three windings, in order (primary, secondary, tertiary).
    pub windings: [Winding; 3],
    /// Pairwise series impedance `[z12, z23, z31]` (primary-secondary,
    /// secondary-tertiary, tertiary-primary), each per unit on the system base
    /// with its declared MVA base.
    pub z: [Impedance; 3],
    /// Star-point voltage magnitude (p.u.) and angle (degrees), as solved.
    pub star_vm: f64,
    pub star_va: f64,
    /// Magnetizing shunt referred to the star point (p.u. on the system base).
    pub mag_g: f64,
    pub mag_b: f64,
    pub in_service: bool,
    pub name: Option<String>,
    /// Stable row identity; see [`Bus::uid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    pub extras: Extras,
}

impl Transformer3W {
    #[must_use]
    pub fn new(windings: [Winding; 3], z: [Impedance; 3]) -> Self {
        Self {
            windings,
            z,
            star_vm: 1.0,
            star_va: 0.0,
            mag_g: 0.0,
            mag_b: 0.0,
            in_service: true,
            name: None,
            uid: None,
            extras: Extras::new(),
        }
    }

    /// The per-winding star impedances `(r, x)` — winding *k* to the star point —
    /// from the pairwise values, per unit on the system base.
    ///
    /// Standard pairwise→star conversion: `z1 = (z12 + z31 - z23) / 2`, and so on.
    /// Because the impedances are already on a common base, the split is linear in
    /// `r` and `x` separately.
    #[must_use]
    pub fn star_impedances(&self) -> [(f64, f64); 3] {
        let [z12, z23, z31] = self.z;
        let half = |a: f64, b: f64, c: f64| (a + b - c) / 2.0;
        [
            (half(z12.r, z31.r, z23.r), half(z12.x, z31.x, z23.x)),
            (half(z12.r, z23.r, z31.r), half(z12.x, z23.x, z31.x)),
            (half(z23.r, z31.r, z12.r), half(z23.x, z31.x, z12.x)),
        ]
    }

    /// Expand into a synthetic star [`Bus`] (id `star_id`) plus three [`Branch`]es,
    /// one per winding, for a consumer that works in the bus-branch model.
    /// [`IndexedNetwork`](crate::IndexedNetwork) calls this via
    /// `BalancedNetwork::expand_transformers_3w` when assembling matrix inputs. The star
    /// bus carries the stored star voltage and the magnetizing shunt is left to the
    /// caller; each branch takes its winding's tap, phase shift, and ratings.
    #[must_use]
    pub fn star_expansion(&self, star_id: BusId) -> (Bus, [Branch; 3]) {
        let star = Bus {
            id: star_id,
            kind: BusType::Pq,
            vm: self.star_vm,
            va: self.star_va,
            base_kv: self.windings[0].nominal_kv,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 0,
            zone: 0,
            name: self.name.clone(),
            uid: self.uid.clone(),
            location: None,
            extras: Extras::new(),
        };
        let zs = self.star_impedances();
        let branch = |w: &Winding, (r, x): (f64, f64)| Branch {
            from: w.bus,
            to: star_id,
            r,
            x,
            b: 0.0,
            charging: None,
            rate_a: w.rate_a,
            rate_b: w.rate_b,
            rate_c: w.rate_c,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: w.tap,
            shift: w.shift,
            in_service: self.in_service,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            solution: None,
            uid: None,
            route: None,
            extras: Extras::new(),
        };
        let branches = [
            branch(&self.windings[0], zs[0]),
            branch(&self.windings[1], zs[1]),
            branch(&self.windings[2], zs[2]),
        ];
        (star, branches)
    }
}

/// The MATPOWER gen capability / ramp columns past `PMIN`, in order. The index
/// into this array is the slot index into a [`GenCaps`].
pub(crate) const GEN_EXTRA_KEYS: [&str; 11] = [
    "pc1", "pc2", "qc1min", "qc1max", "qc2min", "qc2max", "ramp_agc", "ramp_10", "ramp_30",
    "ramp_q", "apf",
];

/// One value-domain scan finding, internal to the diagnostic and repair
/// passes: an element field whose value falls outside its physical range,
/// paired with the value the repair sets in its place. The public shapes are
/// the coded [`Diagnostic`](crate::Diagnostic) records
/// [`BalancedNetwork::validate_values`] returns and the history entry
/// [`repair_values`] appends.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValueFinding {
    /// Human-readable element locator, e.g. `"bus 3"` or `"generator at bus 5"`.
    pub element: String,
    pub field: &'static str,
    pub old: f64,
    pub new: f64,
    pub reason: &'static str,
}

impl ValueFinding {
    /// The finding as the coded record: target names the element, details
    /// carry the machine readable pieces, the message stays prose.
    pub(crate) fn into_diagnostic(self) -> crate::Diagnostic {
        let mut details = serde_json::Map::new();
        details.insert("element".to_owned(), serde_json::json!(self.element));
        details.insert("field".to_owned(), serde_json::json!(self.field));
        details.insert("value".to_owned(), serde_json::json!(self.old));
        details.insert("repaired_value".to_owned(), serde_json::json!(self.new));
        details.insert("reason".to_owned(), serde_json::json!(self.reason));
        crate::Diagnostic::of(
            &crate::diagnostics::codes::VALIDATE_BALANCED_VALUE_DOMAIN,
            format!(
                "{}: `{}` is {} ({}); the repair sets {}",
                self.element, self.field, self.old, self.reason, self.new
            ),
        )
        .with_target(format!("{}#{}", self.element.replace(' ', "_"), self.field))
        .expect("scan-built targets are nonempty and bounded")
        .with_details(details)
        .expect("scan-built details stay within the record bounds")
    }
}

/// Clamp every out-of-domain value of a parsed module to its repaired value
/// and record the pass: one `Repair` history entry naming each change in its
/// parameters, and one `VALIDATE.BALANCED.VALUE_DOMAIN` finding per repaired
/// field. The retained source is severed — the value no longer matches the
/// bytes, so a same format write serializes the repaired network rather than
/// echoing the input. A module already in domain comes back unchanged.
///
/// # Errors
/// Never on scan output: the record constructors refuse only unbounded
/// caller data, and the scan is bounded by the model.
pub fn repair_values(
    module: powerio_core::PioModule<BalancedNetwork>,
) -> std::result::Result<powerio_core::PioModule<BalancedNetwork>, powerio_core::Error> {
    let repair_ordinal = module
        .history()
        .iter()
        .filter(|entry| entry.kind() == powerio_core::HistoryKind::Repair)
        .count();
    let mut network_findings = Vec::new();
    let mut module = module.map_value(|mut network| {
        network_findings = network.repair_in_place();
        network
    });
    if network_findings.is_empty() {
        return Ok(module);
    }
    let mut parameters = std::collections::BTreeMap::new();
    parameters.insert(
        "repairs".to_owned(),
        serde_json::json!(
            network_findings
                .iter()
                .map(|finding| {
                    serde_json::json!({
                        "element": finding.element,
                        "field": finding.field,
                        "value": finding.old,
                        "repaired_value": finding.new,
                    })
                })
                .collect::<Vec<_>>()
        ),
    );
    let entry = powerio_core::HistoryEntry::new(
        powerio_core::HistoryId::new(format!("repair{repair_ordinal}"))?,
        powerio_core::HistoryKind::Repair,
        "value_domain_repair",
    )?
    .with_parameters(parameters)?;
    module.add_history_entry(entry)?;
    for finding in network_findings {
        module.add_diagnostic(finding.into_diagnostic())?;
    }
    module = module.sever_source();
    Ok(module)
}

/// Voltage magnitude (p.u.) repair: non-positive or above 2 (or non-finite) → 1.0.
/// A zero magnitude is treated as out of domain (a de-energized placeholder), not
/// a valid 0 p.u.
fn repair_vm(vm: f64) -> Option<f64> {
    (!vm.is_finite() || vm <= 0.0 || vm > 2.0).then_some(1.0)
}

/// Voltage angle (degrees) repair: `|va| > 2000` (or non-finite) → 0.0.
fn repair_va(va: f64) -> Option<f64> {
    (!va.is_finite() || va.abs() > 2000.0).then_some(0.0)
}

/// Generator MVA base repair: non-positive (or non-finite) → the system base.
fn repair_mbase(mbase: f64, sbase: f64) -> Option<f64> {
    (!mbase.is_finite() || mbase <= 0.0).then_some(sbase)
}

/// Generator voltage setpoint (p.u.) repair: non-positive (or non-finite) → 1.0.
fn repair_vg(vg: f64) -> Option<f64> {
    (!vg.is_finite() || vg <= 0.0).then_some(1.0)
}

/// The three element counts the star lowering changes, from
/// [`BalancedNetwork::lowered_lengths`]. Every other family keeps its length, since the
/// lowering only appends a star bus, its winding branches, and a magnetizing
/// shunt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoweredLengths {
    pub(crate) buses: usize,
    pub(crate) branches: usize,
    pub(crate) shunts: usize,
}

impl BalancedNetwork {
    #[must_use]
    pub fn new(name: impl Into<String>, base_mva: f64) -> BalancedNetwork {
        BalancedNetwork::from_tables(BalancedNetworkTables {
            name: name.into(),
            base_mva,
            base_frequency: DEFAULT_BASE_FREQUENCY,
            geo: None,
            buses: Vec::new().into(),
            loads: Vec::new().into(),
            shunts: Vec::new().into(),
            branches: Vec::new().into(),
            switches: Vec::new().into(),
            generators: Vec::new().into(),
            storage: Vec::new().into(),
            hvdc: Vec::new().into(),
            transformers_3w: Vec::new().into(),
            areas: Vec::new().into(),
            solver: None,
            source_format: SourceFormat::InMemory,
        })
    }

    /// A network assembled in memory from buses and branches, with no loads,
    /// shunts, generators, storage, HVDC, or retained source document. Synthetic
    /// topology generators and tests use it instead of repeating the struct
    /// literal. The caller owns reference integrity (run `check_references` if
    /// the ids might be inconsistent).
    #[must_use]
    pub fn in_memory(
        name: impl Into<String>,
        base_mva: f64,
        buses: Vec<Bus>,
        branches: Vec<Branch>,
    ) -> BalancedNetwork {
        let mut net = Self::new(name, base_mva);
        *net.buses_mut() = buses;
        *net.branches_mut() = branches;
        net
    }

    /// Serialize the structured tables to model JSON. The C ABI and language
    /// bindings use this representation. The retained `source` text is
    /// excluded (see the field's `#[serde(skip)]`), so the byte-exact echo
    /// stays on the same-format write path; a [`from_json`](BalancedNetwork::from_json)
    /// round-trip reproduces every field except `source`, which returns `None`.
    ///
    /// JSON has no `Inf`/`NaN` literal: a nonfinite field is written as
    /// `"Infinity"`, `"-Infinity"`, or `"NaN"` (see [`crate::nonfinite`]) and
    /// [`from_json`](BalancedNetwork::from_json) reads either a number or one
    /// of those spellings back, so every network round trips — readers
    /// legitimately produce `Inf` limits, and a document written before
    /// 0.9.0 spelled them `null`, which still refuses with a message naming
    /// the change.
    ///
    /// # Errors
    /// A `serde_json` serialization failure (none arise from this model today).
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string(self).map_err(|e| Error::FormatRead {
            format: "JSON",
            message: e.to_string(),
        })
    }

    /// [`to_json`](BalancedNetwork::to_json) plus the fidelity records the
    /// write produced. The write is faithful today — a nonfinite value spells
    /// itself as a string and reads back — so the record list is empty; the
    /// channel stays because it is the shape a write-side finding arrives
    /// through, and a caller wired to it needs no change when one appears.
    ///
    /// # Errors
    /// A `serde_json` serialization failure (none arise from this model today).
    pub fn to_json_with_diagnostics(
        &self,
    ) -> crate::Result<(String, Vec<crate::diagnostics::Diagnostic>)> {
        let text = self.to_json()?;
        Ok((text, Vec::new()))
    }

    /// Serialize this typed network to `format`, the semantic write. The byte
    /// exact echo of an unchanged parsed module lives on the module write
    /// path, [`write_as`](crate::write_as).
    ///
    /// # Errors
    /// [`Error::WriteUnsupported`](crate::Error) for a read
    /// only target, and the writer's own error on a case it cannot state.
    pub fn to_format(&self, format: crate::TargetFormat) -> crate::Result<crate::Conversion> {
        crate::format::write_conversion(self, format)
    }

    /// Serialize this network to `format` from the typed model, the balanced
    /// twin of `MulticonductorNetwork::to_canonical_format` on the
    /// distribution side. Identical to [`to_format`](Self::to_format) now that
    /// source echo belongs to the module write path.
    ///
    /// # Errors
    /// As [`to_format`](Self::to_format).
    pub fn to_canonical_format(
        &self,
        format: crate::TargetFormat,
    ) -> crate::Result<crate::Conversion> {
        crate::format::write_conversion(self, format)
    }

    /// Serialize this network with write-time cost policies.
    ///
    /// The network itself is not mutated.
    pub fn to_format_with_options(
        &self,
        format: crate::TargetFormat,
        options: &crate::WriteOptions,
    ) -> crate::Result<crate::Conversion> {
        if options.is_default() {
            return self.to_format(format);
        }
        let (working, policy_warnings) = crate::format::apply_write_cost_policy(self, options)?;
        let mut conv = crate::format::write_conversion(&working, format)?;
        conv.prepend(policy_warnings);
        Ok(conv)
    }

    /// Serialize this network to MATPOWER `.m` text.
    ///
    /// This is byte-exact when the network was parsed from MATPOWER and still
    /// carries its retained source text.
    #[must_use]
    pub fn to_matpower(&self) -> String {
        crate::write_matpower(self)
    }

    /// Rebuild a `BalancedNetwork` from JSON produced by [`to_json`](BalancedNetwork::to_json).
    ///
    /// A float position accepts a number or the nonfinite spellings
    /// `"Infinity"`, `"-Infinity"`, `"NaN"` (see [`crate::nonfinite`]).
    ///
    /// Validates the result (no buses, unique bus ids, no dangling references)
    /// before returning, so the JSON transport (the C ABI and Julia bridge ride
    /// on it) can't hand back a network the file readers would have rejected
    /// (the same no-buses guard `read_source` applies to every parse path).
    pub fn from_json(text: &str) -> crate::Result<BalancedNetwork> {
        // Tolerate a leading UTF-8 byte order mark, as the format readers do.
        let text = text.trim_start_matches('\u{feff}');
        let net: BalancedNetwork = serde_json::from_str(text).map_err(|e| Error::FormatRead {
            format: "JSON",
            message: e.to_string(),
        })?;
        net.check_references("JSON")?;
        if net.buses().is_empty() {
            return Err(Error::FormatRead {
                format: "JSON",
                message: "case has no buses".into(),
            });
        }
        Ok(net)
    }

    /// Rebuild a `BalancedNetwork` from UTF-8 JSON bytes produced by
    /// [`to_json`](BalancedNetwork::to_json).
    ///
    /// Decoding is strict: invalid UTF-8 returns a coded JSON read error and
    /// is never replaced with Unicode replacement characters. A leading UTF-8
    /// byte order mark is accepted. The decoded document goes through
    /// [`from_json`](BalancedNetwork::from_json), including its reference and
    /// nonempty-network validation.
    pub fn from_json_bytes(bytes: &[u8]) -> crate::Result<BalancedNetwork> {
        let text = std::str::from_utf8(bytes).map_err(|error| Error::FormatRead {
            format: "JSON",
            message: format!("input is not valid UTF-8: {error}"),
        })?;
        Self::from_json(text)
    }

    /// Whether this is a normalized (per-unit, radian, filtered)
    /// derived product from [`to_normalized`](BalancedNetwork::to_normalized), rather
    /// than a raw network at the file's unit basis. Unit-sensitive code that
    /// takes a `&BalancedNetwork` can check this instead of silently assuming MW.
    #[must_use]
    pub fn is_normalized(&self) -> bool {
        self.source_format() == SourceFormat::Normalized
    }

    /// Error unless `base_mva` is a positive, finite number. It is every
    /// per-unit divisor, so a malformed base would otherwise silently poison
    /// downstream values with `NaN`/`Inf` or flipped signs. The per-unit
    /// consumers ([`to_normalized`](BalancedNetwork::to_normalized), the gridfm
    /// export) call this; any other unit-sensitive consumer should too.
    pub fn check_base_mva(&self) -> crate::Result<()> {
        if self.base_mva().is_finite() && self.base_mva() > 0.0 {
            Ok(())
        } else {
            Err(crate::Error::InvalidBaseMva {
                base: self.base_mva(),
            })
        }
    }

    /// Report element fields whose values fall outside their physical domain,
    /// without changing anything, as coded `VALIDATE.BALANCED.VALUE_DOMAIN`
    /// findings. Each record targets the element and carries the field, the
    /// current value, the value a repair would set, and why in details.
    ///
    /// This generalizes the per-reader value clamps (a bus voltage magnitude
    /// outside `[0, 2]`, an angle past `±2000°`, a zero generator MVA base or
    /// voltage setpoint) into one pass any consumer can run, separate from the
    /// structural [`validate`](BalancedNetwork::validate) (which only checks
    /// ids and references). It is non-mutating; [`repair_values`] applies the
    /// fixes to a parsed module and records them.
    #[must_use]
    pub fn validate_values(&self) -> Vec<crate::Diagnostic> {
        self.value_findings()
            .into_iter()
            .map(ValueFinding::into_diagnostic)
            .collect()
    }

    pub(crate) fn value_findings(&self) -> Vec<ValueFinding> {
        let mut out = Vec::new();
        for b in self.buses() {
            if let Some(new) = repair_vm(b.vm) {
                out.push(ValueFinding {
                    element: format!("bus {}", b.id),
                    field: "vm",
                    old: b.vm,
                    new,
                    reason: "voltage magnitude outside [0, 2] p.u.",
                });
            }
            if let Some(new) = repair_va(b.va) {
                out.push(ValueFinding {
                    element: format!("bus {}", b.id),
                    field: "va",
                    old: b.va,
                    new,
                    reason: "voltage angle outside ±2000°",
                });
            }
        }
        for g in self.generators() {
            if let Some(new) = repair_mbase(g.mbase, self.base_mva()) {
                out.push(ValueFinding {
                    element: format!("generator at bus {}", g.bus),
                    field: "mbase",
                    old: g.mbase,
                    new,
                    reason: "non-positive generator MVA base",
                });
            }
            if let Some(new) = repair_vg(g.vg) {
                out.push(ValueFinding {
                    element: format!("generator at bus {}", g.bus),
                    field: "vg",
                    old: g.vg,
                    new,
                    reason: "non-positive voltage setpoint",
                });
            }
        }
        out
    }

    /// Clamp every out-of-domain value to its repaired value (the same rules
    /// [`validate_values`](BalancedNetwork::validate_values) reports), returning the list
    /// of changes made. A second call returns an empty list (the values are now
    /// in domain). Crate private: the recorded public path is
    /// [`repair_values`], which appends the history entry and severs the
    /// retained source echo the mutation invalidates.
    pub(crate) fn repair_in_place(&mut self) -> Vec<ValueFinding> {
        let findings = self.value_findings();
        let sbase = self.base_mva();
        for b in self.buses_mut() {
            if let Some(new) = repair_vm(b.vm) {
                b.vm = new;
            }
            if let Some(new) = repair_va(b.va) {
                b.va = new;
            }
        }
        for g in self.generators_mut() {
            if let Some(new) = repair_mbase(g.mbase, sbase) {
                g.mbase = new;
            }
            if let Some(new) = repair_vg(g.vg) {
                g.vg = new;
            }
        }
        findings
    }

    /// The element counts [`Self::expand_transformers_3w`] would produce, read off
    /// the transformer records instead of building the lowering. A caller that
    /// only needs the lowered lengths — sizing a per-row map, say — would
    /// otherwise pay a whole `BalancedNetwork` clone for three `len()` calls.
    /// `lowered_lengths_match_the_expansion` pins the two against each other.
    pub(crate) fn lowered_lengths(&self) -> LoweredLengths {
        let mut lengths = LoweredLengths {
            buses: self.buses().len(),
            branches: self.branches().len(),
            shunts: self.shunts().len(),
        };
        for t in self.transformers_3w().iter().filter(|t| t.in_service) {
            lengths.buses += 1;
            lengths.branches += 3;
            if t.mag_g != 0.0 || t.mag_b != 0.0 {
                lengths.shunts += 1;
            }
        }
        lengths
    }

    /// A bus-branch lowering of the network for analysis: each in-service
    /// 3-winding transformer becomes a synthetic star bus, its three winding
    /// branches, and (when present) its magnetizing shunt, so the matrix builders
    /// and connectivity see it. Returns the network unchanged (borrowed) when
    /// there are no 3-winding transformers, so the common case allocates nothing.
    ///
    /// The canonical `BalancedNetwork` keeps the typed [`Transformer3W`] records; this is
    /// the derived analysis form that [`IndexedNetwork`](crate::IndexedNetwork)
    /// builds behind the scenes, so callers never see the synthetic buses in the
    /// model they read or write.
    pub(crate) fn expand_transformers_3w(&self) -> std::borrow::Cow<'_, BalancedNetwork> {
        if self.transformers_3w().is_empty() {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut net = self.clone();
        // The star branches carry per-unit impedance (CZ = 1), the same convention
        // the matrix builders read straight off a branch, so no rebasing. The
        // magnetizing shunt is an admittance, so it scales like every other shunt:
        // by the per-unit base for a raw network, by 1 for a normalized one.
        let scale = if net.is_normalized() {
            1.0
        } else {
            net.base_mva()
        };
        // check_references refuses bus ids without headroom for these
        // synthetic ids on every parse path; the checked arithmetic turns a
        // programmatic caller's overflow into a loud panic instead of a
        // wrapped id aliasing an existing bus.
        let base_id = net
            .buses()
            .iter()
            .map(|b| b.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .expect("bus id space exhausted for star expansion");
        for (k, t) in self
            .transformers_3w()
            .iter()
            .filter(|t| t.in_service)
            .enumerate()
        {
            let star_id = BusId(
                base_id
                    .checked_add(k)
                    .expect("bus id space exhausted for star expansion"),
            );
            let (star, branches) = t.star_expansion(star_id);
            net.buses_mut().push(star);
            net.branches_mut().extend(branches);
            if t.mag_g != 0.0 || t.mag_b != 0.0 {
                net.shunts_mut().push(Shunt {
                    bus: star_id,
                    g: t.mag_g * scale,
                    b: t.mag_b * scale,
                    in_service: true,
                    control: None,
                    uid: None,
                    extras: Extras::new(),
                });
            }
        }
        net.transformers_3w_mut().clear();
        std::borrow::Cow::Owned(net)
    }

    /// Check structural integrity: bus ids are unique and every element
    /// references an existing bus. The file readers and [`from_json`](BalancedNetwork::from_json)
    /// run this; a `BalancedNetwork` built by hand (or mutated, e.g. by a scenario
    /// generator) should call it before handing the network to
    /// [`IndexedNetwork`](crate::IndexedNetwork), whose dense indexing assumes it.
    pub fn validate(&self) -> crate::Result<()> {
        self.check_references("network")
    }

    /// Error if two buses share an id, or if any element references a bus that
    /// doesn't exist. Readers call this after parsing so a missing/garbled id
    /// (which would otherwise default to a placeholder and silently re-wire the
    /// network) fails loudly instead.
    pub(crate) fn check_references(&self, format: &'static str) -> crate::Result<()> {
        // HashSet, not BTreeSet: building the id set and probing it once per branch
        // endpoint / load / shunt / gen is the dominant cost of a large parse, and
        // a BTreeSet pays a log-n pointer-chasing probe each time. Pre-size to skip
        // rehashing.
        let mut ids = std::collections::HashSet::with_capacity(self.buses().len());
        for b in self.buses() {
            // The readers parse ids through `as usize`, which saturates rather
            // than failing, and the C ABI reports them as int64. Two distinct
            // ids above the ceiling would surface as one value there, so a
            // branch endpoint would match two bus rows.
            if b.id > BusId::MAX {
                return Err(Error::FormatRead {
                    format,
                    message: format!("bus id {} is outside the int64 id space", b.id),
                });
            }
            if !ids.insert(b.id) {
                return Err(Error::FormatRead {
                    format,
                    message: format!("duplicate bus id {}", b.id),
                });
            }
        }
        let check = |bus: BusId, what: &str| -> crate::Result<()> {
            if ids.contains(&bus) {
                Ok(())
            } else {
                Err(Error::FormatRead {
                    format,
                    message: format!("{what} references unknown bus {bus}"),
                })
            }
        };
        // Format the context only on the error path, not once per branch.
        for (i, br) in self.branches().iter().enumerate() {
            for bus in [br.from, br.to] {
                if !ids.contains(&bus) {
                    return Err(Error::FormatRead {
                        format,
                        message: format!("branch {i} references unknown bus {bus}"),
                    });
                }
            }
            if let Some(bus) = br.control.as_ref().and_then(|c| c.controlled_bus) {
                check(bus, "transformer control")?;
            }
        }
        for (i, sw) in self.switches().iter().enumerate() {
            for bus in [sw.from, sw.to] {
                if !ids.contains(&bus) {
                    return Err(Error::FormatRead {
                        format,
                        message: format!("switch {i} references unknown bus {bus}"),
                    });
                }
            }
        }
        for l in self.loads() {
            check(l.bus, "load")?;
        }
        for s in self.shunts() {
            check(s.bus, "shunt")?;
            if let Some(bus) = s.control.as_ref().and_then(|c| c.control_bus) {
                check(bus, "switched-shunt control")?;
            }
        }
        for g in self.generators() {
            check(g.bus, "generator")?;
            if let Some(bus) = g.regulated_bus {
                check(bus, "generator voltage control")?;
            }
        }
        for d in self.hvdc() {
            check(d.from, "dcline")?;
            check(d.to, "dcline")?;
        }
        for s in self.storage() {
            check(s.bus, "storage")?;
        }
        for a in self.areas() {
            if let Some(slack) = a.slack_bus {
                check(slack, "area swing")?;
            }
        }
        for t in self.transformers_3w() {
            for w in &t.windings {
                check(w.bus, "3-winding transformer")?;
            }
        }
        self.check_star_expansion_headroom(format)
    }

    /// Star expansion allocates synthetic bus ids `max_bus_id + 1 + k`, one per
    /// in-service 3-winding transformer; a bus id near [`BusId::MAX`] would
    /// push those past the ceiling the C ABI reports ids in. The base id
    /// `max + 1` is computed whenever any 3-winding transformer is present,
    /// even if none is in service, so the headroom is
    /// `max(1, in-service count)`. No real case sits there, so refuse it at the
    /// boundary like any other malformed reference.
    fn check_star_expansion_headroom(&self, format: &'static str) -> crate::Result<()> {
        if self.transformers_3w().is_empty() {
            return Ok(());
        }
        let Some(max_id) = self.buses().iter().map(|b| b.id.0).max() else {
            return Ok(());
        };
        let needed = self
            .transformers_3w()
            .iter()
            .filter(|t| t.in_service)
            .count()
            .max(1);
        if max_id
            .checked_add(needed)
            .is_none_or(|top| top > BusId::MAX.0)
        {
            return Err(Error::FormatRead {
                format,
                message: format!(
                    "bus id {max_id} leaves no room to allocate synthetic star bus ids \
                     for 3-winding transformers"
                ),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn source_format_serializes_as_its_name_token_and_reads_the_legacy_spelling() {
        // The exhaustive match keeps a new variant from shipping with a serde
        // spelling that differs from name().
        let all = [
            SourceFormat::Matpower,
            SourceFormat::PowerModelsJson,
            SourceFormat::EgretJson,
            SourceFormat::Psse,
            SourceFormat::PowerWorld,
            SourceFormat::PandapowerJson,
            SourceFormat::Pslf,
            SourceFormat::PowerWorldBinary,
            SourceFormat::InMemory,
            SourceFormat::Normalized,
            SourceFormat::Gridfm,
            SourceFormat::PypsaCsv,
            SourceFormat::Goc3Json,
            SourceFormat::SurgeJson,
            SourceFormat::DeepMindOpfDataJson,
        ];
        for f in all {
            match f {
                SourceFormat::Matpower
                | SourceFormat::PowerModelsJson
                | SourceFormat::EgretJson
                | SourceFormat::Psse
                | SourceFormat::PowerWorld
                | SourceFormat::PandapowerJson
                | SourceFormat::Pslf
                | SourceFormat::PowerWorldBinary
                | SourceFormat::InMemory
                | SourceFormat::Normalized
                | SourceFormat::Gridfm
                | SourceFormat::PypsaCsv
                | SourceFormat::Goc3Json
                | SourceFormat::SurgeJson
                | SourceFormat::DeepMindOpfDataJson => {}
            }
            let token = serde_json::to_value(f).unwrap();
            assert_eq!(token, serde_json::Value::String(f.name().to_owned()));
            let back: SourceFormat = serde_json::from_value(token).unwrap();
            assert_eq!(back, f);
            let legacy = serde_json::Value::String(format!("{f:?}"));
            let from_legacy: SourceFormat = serde_json::from_value(legacy).unwrap();
            assert_eq!(from_legacy, f);
        }
    }

    #[test]
    fn quadratic_with_constant_keeps_c0_across_ncost() {
        let full = GenCost::new(2, 0.0, 0.0, vec![1.5, 2.0, 5.0]);
        assert_eq!(full.quadratic_with_constant(), Some((3.0, 2.0, 5.0)));
        assert_eq!(full.quadratic(), Some((3.0, 2.0)));

        let linear = GenCost::new(2, 0.0, 0.0, vec![2.0, 5.0]);
        assert_eq!(linear.quadratic_with_constant(), Some((0.0, 2.0, 5.0)));

        let constant = GenCost::new(2, 0.0, 0.0, vec![5.0]);
        assert_eq!(constant.quadratic_with_constant(), Some((0.0, 0.0, 5.0)));

        let piecewise = GenCost::new(1, 0.0, 0.0, vec![0.0, 0.0, 1.0, 1.0]);
        assert_eq!(piecewise.quadratic_with_constant(), None);

        let cubic = GenCost::new(2, 0.0, 0.0, vec![1.0, 1.0, 1.0, 1.0]);
        assert_eq!(cubic.quadratic_with_constant(), None);

        let truncated = GenCost::with_ncost(2, 0.0, 0.0, 3, vec![1.0]);
        assert_eq!(truncated.quadratic_with_constant(), None);
    }

    #[test]
    fn a_leading_coefficient_below_the_tolerance_comes_off_the_row() {
        let artifact = GenCost::new(2, 0.0, 0.0, vec![1e-17, 2.0, 5.0]);
        assert_eq!(
            artifact.quadratic_with_constant(),
            Some((2e-17, 2.0, 5.0)),
            "the untouched reader keeps the artifact"
        );
        assert_eq!(
            artifact.quadratic_with_constant_tol(GenCost::LEADING_COEFF_TOL),
            Some((0.0, 2.0, 5.0))
        );
        assert_eq!(
            artifact.quadratic_with_constant_tol(0.0),
            Some((2e-17, 2.0, 5.0)),
            "a zero tolerance strips an exact zero alone"
        );

        // A row states a curve of a lower order once the leading zeros are off,
        // so a cubic row the untouched reader refuses reads as a quadratic one.
        let padded = GenCost::new(2, 0.0, 0.0, vec![0.0, 1.5, 2.0, 5.0]);
        assert_eq!(padded.quadratic_with_constant(), None);
        assert_eq!(
            padded.quadratic_with_constant_tol(0.0),
            Some((3.0, 2.0, 5.0))
        );

        let flat = GenCost::new(2, 0.0, 0.0, vec![1e-17, 1e-17, 1e-17]);
        assert_eq!(
            flat.quadratic_with_constant_tol(GenCost::LEADING_COEFF_TOL),
            Some((0.0, 0.0, 1e-17)),
            "the last coefficient stays, whatever its magnitude"
        );

        let piecewise = GenCost::new(1, 0.0, 0.0, vec![0.0, 0.0, 1.0, 1.0]);
        assert_eq!(
            piecewise.quadratic_with_constant_tol(GenCost::LEADING_COEFF_TOL),
            None
        );

        let truncated = GenCost::with_ncost(2, 0.0, 0.0, 3, vec![1.0]);
        assert_eq!(
            truncated.quadratic_with_constant_tol(GenCost::LEADING_COEFF_TOL),
            None
        );

        let quartic = GenCost::new(2, 0.0, 0.0, vec![1.0, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(
            quartic.quadratic_with_constant_tol(GenCost::LEADING_COEFF_TOL),
            None
        );
    }

    /// The bound at one corner of the voltage box: the law of cosines over the
    /// angle window, scaled by the larger terminal ceiling and the impedance.
    fn expected_rate(window: f64, fr: f64, to: f64, zmag: f64) -> f64 {
        let separation = (fr * fr + to * to - 2.0 * fr * to * window.cos()).sqrt();
        fr.max(to) * separation / zmag
    }

    #[test]
    fn synthesized_rate_follows_the_angle_window_and_the_voltage_bands() {
        let br = Branch::new(BusId(1), BusId(2), 0.03, 0.04);
        let expected = |window: f64, fr: f64, to: f64| expected_rate(window, fr, to, 0.05);
        // A band pinned to one value, so the four corners collapse to one and
        // the bound is the plain law of cosines.
        let at = |v: f64| (v, v);
        close(
            br.synthesize_rate_a(0.5, at(1.1), at(1.06)),
            expected(0.5, 1.1, 1.06),
        );

        // A wider window gives a looser bound.
        assert!(
            br.synthesize_rate_a(0.8, at(1.1), at(1.06))
                > br.synthesize_rate_a(0.5, at(1.1), at(1.06))
        );

        // The magnitude of the window is what counts, and it holds at π.
        close(
            br.synthesize_rate_a(-0.5, at(1.1), at(1.06)),
            expected(0.5, 1.1, 1.06),
        );
        for window in [6.0, 2.0 * std::f64::consts::PI, -360.0] {
            close(
                br.synthesize_rate_a(window, at(1.1), at(1.06)),
                expected(std::f64::consts::PI, 1.1, 1.06),
            );
        }

        let ideal = Branch::new(BusId(1), BusId(2), 0.0, 0.0);
        close(ideal.synthesize_rate_a(0.5, at(1.1), at(1.1)), 0.0);
    }

    #[test]
    fn a_narrow_window_bounds_at_the_mixed_voltage_corner() {
        // The phasor difference is convex in the two voltages, so its largest
        // value over the band box is at a corner. Below roughly 10° that corner
        // is one terminal at its ceiling and the other at its floor, not both at
        // their ceilings. Reading only the ceilings there returns a bound
        // several times tighter than the branch physically has, and an OPF
        // enforces it.
        let br = Branch::new(BusId(1), BusId(2), 0.0, 0.01);
        let (vmin, vmax) = (0.9, 1.1);
        let corner = |window: f64, fr: f64, to: f64| expected_rate(window, fr, to, 0.01);

        let narrow = 2.0_f64.to_radians();
        let bound = br.synthesize_rate_a(narrow, (vmin, vmax), (vmin, vmax));
        close(bound, corner(narrow, vmax, vmin));
        assert!(
            bound > 5.0 * corner(narrow, vmax, vmax),
            "the mixed corner dominates here: {bound} vs {}",
            corner(narrow, vmax, vmax)
        );

        // Past the crossover both ceilings win again, and the bound follows.
        let wide = 30.0_f64.to_radians();
        close(
            br.synthesize_rate_a(wide, (vmin, vmax), (vmin, vmax)),
            corner(wide, vmax, vmax),
        );
    }

    fn bus(id: usize) -> Bus {
        Bus {
            id: BusId(id),
            kind: BusType::Pq,
            vm: 1.0,
            va: 0.0,
            base_kv: 230.0,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: None,
            uid: None,
            location: None,
            extras: Extras::new(),
        }
    }

    #[test]
    fn model_json_bytes_are_strict_utf8_and_keep_model_validation() {
        let net = BalancedNetwork::in_memory("bytes", 100.0, vec![bus(1)], Vec::new());
        let json = net.to_json().expect("serialize model JSON");
        let mut with_bom = b"\xef\xbb\xbf".to_vec();
        with_bom.extend_from_slice(json.as_bytes());
        let back = BalancedNetwork::from_json_bytes(&with_bom).expect("read BOM prefixed JSON");
        assert_eq!(back.name(), "bytes");
        assert_eq!(back.buses().len(), 1);

        let error = BalancedNetwork::from_json_bytes(b"{\"buses\":[]\xff}")
            .expect_err("invalid UTF-8 must not be replaced");
        assert!(
            matches!(&error, crate::Error::FormatRead { format: "JSON", message } if message.starts_with("input is not valid UTF-8:")),
            "{error}"
        );
        assert_eq!(error.code().code, "PARSE.SOURCE.MALFORMED");

        let empty = net
            .to_json()
            .expect("serialize model JSON")
            .replace(&serde_json::to_string(&net.buses()).unwrap(), "[]");
        let error = BalancedNetwork::from_json_bytes(empty.as_bytes())
            .expect_err("the byte API must keep no-bus validation");
        assert!(error.to_string().contains("case has no buses"), "{error}");
    }

    fn winding(b: usize) -> Winding {
        Winding {
            bus: BusId(b),
            tap: 1.0,
            shift: 0.0,
            nominal_kv: 230.0,
            rate_a: 100.0,
            rate_b: 0.0,
            rate_c: 0.0,
        }
    }

    fn transformer_3w() -> Transformer3W {
        let z = |r, x| Impedance {
            r,
            x,
            base_mva: 100.0,
        };
        Transformer3W {
            windings: [winding(1), winding(2), winding(3)],
            z: [z(0.01, 0.10), z(0.02, 0.20), z(0.03, 0.30)],
            star_vm: 0.98,
            star_va: -1.5,
            mag_g: 0.0,
            mag_b: 0.0,
            in_service: true,
            name: Some("T1".into()),
            uid: None,
            extras: Extras::new(),
        }
    }

    #[test]
    fn star_impedances_split_the_pairwise_values() {
        // z1 = (z12 + z31 - z23)/2, z2 = (z12 + z23 - z31)/2, z3 = (z23 + z31 - z12)/2.
        let [(r1, x1), (r2, x2), (r3, x3)] = transformer_3w().star_impedances();
        close(r1, 0.01);
        close(x1, 0.10);
        close(r2, 0.0);
        close(x2, 0.0);
        close(r3, 0.02);
        close(x3, 0.20);
    }

    #[test]
    fn star_expansion_builds_a_star_bus_and_three_branches() {
        let t = transformer_3w();
        let (star, branches) = t.star_expansion(BusId(99));

        assert_eq!(star.id, BusId(99));
        close(star.vm, 0.98);
        close(star.va, -1.5);
        // Each branch runs from its winding bus to the star, carrying the
        // winding tap and ratings and the split impedance.
        for (i, br) in branches.iter().enumerate() {
            assert_eq!(br.from, t.windings[i].bus);
            assert_eq!(br.to, BusId(99));
            close(br.tap, 1.0);
            close(br.rate_a, 100.0);
        }
        close(branches[2].r, 0.02);
        close(branches[2].x, 0.20);
    }

    #[test]
    fn three_winding_transformer_survives_json_transport() {
        let mut net =
            BalancedNetwork::in_memory("t", 100.0, vec![bus(1), bus(2), bus(3)], Vec::new());
        net.transformers_3w_mut().push(transformer_3w());
        net.validate().unwrap();

        let back = BalancedNetwork::from_json(&net.to_json().unwrap()).unwrap();
        assert_eq!(back.transformers_3w().len(), 1);
        close(back.transformers_3w()[0].z[1].x, 0.20);
        assert_eq!(back.transformers_3w()[0].windings[2].bus, BusId(3));
    }

    #[test]
    fn lowered_lengths_match_the_expansion() {
        // `lowered_lengths` counts what `expand_transformers_3w` would append
        // instead of building it. The two must agree on every mix: an
        // out-of-service unit appends nothing, and only a unit with magnetizing
        // admittance appends a shunt.
        let mut magnetizing = transformer_3w();
        magnetizing.mag_b = 0.02;
        let mut out_of_service = transformer_3w();
        out_of_service.in_service = false;
        out_of_service.mag_g = 0.01;

        for units in [
            vec![],
            vec![transformer_3w()],
            vec![magnetizing.clone()],
            vec![out_of_service.clone()],
            vec![transformer_3w(), magnetizing, out_of_service],
        ] {
            let mut net =
                BalancedNetwork::in_memory("t", 100.0, vec![bus(1), bus(2), bus(3)], Vec::new());
            net.shunts_mut().push(Shunt::new(BusId(1), 0.0, 0.5));
            *net.transformers_3w_mut() = units;
            let counted = net.lowered_lengths();
            let built = net.expand_transformers_3w();
            assert_eq!(counted.buses, built.buses().len());
            assert_eq!(counted.branches, built.branches().len());
            assert_eq!(counted.shunts, built.shunts().len());
        }
    }

    #[test]
    fn check_references_rejects_bus_ids_without_star_expansion_headroom() {
        // A bus id at the top of the id space would make the synthetic star id
        // `max_bus_id + 1 + k` run past it during indexed analysis; the parse
        // boundary refuses it like any other malformed reference.
        let mut net = BalancedNetwork::in_memory(
            "t",
            100.0,
            vec![bus(1), bus(2), bus(3), bus(i64::MAX as usize)],
            Vec::new(),
        );
        net.transformers_3w_mut().push(transformer_3w());
        let err = net.validate().unwrap_err().to_string();
        assert!(
            err.contains("no room to allocate synthetic star bus ids"),
            "got {err}"
        );
    }

    #[test]
    fn star_expansion_headroom_counts_only_in_service_transformers() {
        // The headroom needed is the in-service transformer count (plus the
        // base id), not the total: an out-of-service unit allocates no star
        // bus, so a network that only fits the in-service count must not be
        // rejected. A max bus id one under the ceiling fits one in-service
        // star id (max + 1) but not two.
        let mut net = BalancedNetwork::in_memory(
            "t",
            100.0,
            vec![bus(1), bus(2), bus(3), bus(i64::MAX as usize - 1)],
            Vec::new(),
        );
        net.transformers_3w_mut().push(transformer_3w());
        let mut out_of_service = transformer_3w();
        out_of_service.in_service = false;
        net.transformers_3w_mut().push(out_of_service);
        net.validate()
            .expect("in-service count fits; must not be rejected");
    }

    #[test]
    fn check_references_rejects_a_bus_id_past_the_int64_ceiling() {
        // The C ABI reports bus ids as int64, so two distinct usize ids above
        // the ceiling both surface as the same value and a branch endpoint
        // matches two bus rows. Refuse them where every other malformed
        // reference is refused.
        let mut net = BalancedNetwork::in_memory(
            "t",
            100.0,
            vec![bus(1), bus(i64::MAX as usize + 1)],
            Vec::new(),
        );
        let err = net.validate().unwrap_err().to_string();
        assert!(err.contains("outside the int64 id space"), "got {err}");

        // The ceiling itself is a valid id.
        net.buses_mut()[1].id = BusId(i64::MAX as usize);
        net.validate().expect("the ceiling itself is representable");
    }

    #[test]
    fn check_references_rejects_a_dangling_winding_bus() {
        let mut net = BalancedNetwork::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        net.transformers_3w_mut().push(transformer_3w()); // winding 3 references bus 3
        let err = net.validate().unwrap_err().to_string();
        assert!(
            err.contains("3-winding transformer references unknown bus 3"),
            "got {err}"
        );
    }

    /// A regulating transformer (bus 1→2) controlling the voltage at bus `reg`.
    fn regulating_branch(reg: usize) -> Branch {
        Branch {
            from: BusId(1),
            to: BusId(2),
            r: 0.0,
            x: 0.1,
            b: 0.0,
            charging: None,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: 1.0,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: Some(TransformerControl {
                mode: TransformerControlMode::Voltage,
                controlled_bus: Some(BusId(reg)),
                tap_min: 0.95,
                tap_max: 1.05,
                band_min: 1.0,
                band_max: 1.02,
                ntp: 17,
                mva_base: 100.0,
            }),
            solution: None,
            uid: None,
            route: None,
            extras: Extras::new(),
        }
    }

    #[test]
    fn transformer_control_survives_json_transport() {
        let mut net =
            BalancedNetwork::in_memory("t", 100.0, vec![bus(1), bus(2), bus(3)], Vec::new());
        net.branches_mut().push(regulating_branch(3));
        net.validate().unwrap();

        let back = BalancedNetwork::from_json(&net.to_json().unwrap()).unwrap();
        let c = back.branches()[0].control.as_ref().unwrap();
        assert_eq!(c.mode, TransformerControlMode::Voltage);
        assert_eq!(c.controlled_bus, Some(BusId(3)));
        close(c.tap_max, 1.05);
        assert_eq!(c.ntp, 17);
    }

    #[test]
    fn gen_caps_serialize_as_a_named_map_that_grows_additively() {
        let mut caps: GenCaps = [None; GEN_EXTRA_KEYS.len()];
        caps[8] = Some(1.5); // ramp_30
        caps[10] = Some(0.5); // apf
        let g = Generator {
            bus: BusId(1),
            pg: 10.0,
            qg: 0.0,
            pmax: 100.0,
            pmin: 0.0,
            qmax: 50.0,
            qmin: -50.0,
            vg: 1.0,
            mbase: 100.0,
            in_service: true,
            cost: None,
            caps,
            regulated_bus: None,
            uid: None,
        };

        // caps is a name-keyed object emitting only the present slots, not a
        // length-exact array.
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains(r#""caps":{"#), "caps is an object: {json}");
        assert!(json.contains(r#""ramp_30":1.5"#) && json.contains(r#""apf":0.5"#));
        let back: Generator = serde_json::from_str(&json).unwrap();
        assert_eq!(back.caps, g.caps);

        // Growing GEN_EXTRA_KEYS stays additive: an unknown future key is ignored,
        // a missing key reads as None, and an omitted field is the empty set.
        let with_future = r#"{"bus":1,"pg":10,"qg":0,"pmax":100,"pmin":0,"qmax":50,"qmin":-50,
            "vg":1,"mbase":100,"in_service":true,"cost":null,
            "caps":{"ramp_30":1.5,"future_ramp":9.9}}"#;
        let g2: Generator = serde_json::from_str(with_future).unwrap();
        assert_eq!(g2.caps[8], Some(1.5));
        assert_eq!(g2.caps.iter().filter(|v| v.is_some()).count(), 1);
        let no_caps = r#"{"bus":1,"pg":10,"qg":0,"pmax":100,"pmin":0,"qmax":50,"qmin":-50,
            "vg":1,"mbase":100,"in_service":true,"cost":null}"#;
        let g3: Generator = serde_json::from_str(no_caps).unwrap();
        assert!(!g3.has_caps());

        // An explicit `"caps":null` is the empty set too, the same as omitting it.
        let null_caps = r#"{"bus":1,"pg":10,"qg":0,"pmax":100,"pmin":0,"qmax":50,"qmin":-50,
            "vg":1,"mbase":100,"in_service":true,"cost":null,"caps":null}"#;
        let g4: Generator = serde_json::from_str(null_caps).unwrap();
        assert!(!g4.has_caps());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn nonfinite_values_round_trip_through_model_json() {
        let bus = |id, vm| Bus {
            id: BusId(id),
            kind: BusType::Pq,
            vm,
            va: 0.0,
            base_kv: 230.0,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: None,
            uid: None,
            location: None,
            extras: Extras::new(),
        };
        let branch = Branch {
            from: BusId(1),
            to: BusId(2),
            r: 0.0,
            x: f64::INFINITY,
            b: 0.0,
            charging: None,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: 0.0,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            solution: None,
            uid: None,
            route: None,
            extras: Extras::new(),
        };
        // A non-finite generator capability reports at its exact key path
        // (caps serializes as a name-keyed object), not the parent `caps`.
        let mut g = Generator {
            bus: BusId(1),
            pg: 0.0,
            qg: 0.0,
            pmax: 0.0,
            pmin: 0.0,
            qmax: 0.0,
            qmin: 0.0,
            vg: 1.0,
            mbase: 100.0,
            in_service: true,
            cost: None,
            caps: GenCaps::default(),
            regulated_bus: None,
            uid: None,
        };
        g.caps[8] = Some(f64::INFINITY); // ramp_30
        // Three nonfinite values at three nesting depths: a bus vm (NaN, a
        // struct field in a table), a branch x (Inf), and a generator ramp_30
        // cap (Inf, inside the name-keyed caps object).
        let mut net = BalancedNetwork::in_memory(
            "nf",
            100.0,
            vec![bus(1, f64::NAN), bus(2, 1.0)],
            vec![branch],
        );
        net.generators_mut().push(g);

        let text = net.to_json().unwrap();
        assert!(text.contains(r#""vm":"NaN""#), "{text}");
        assert!(text.contains(r#""x":"Infinity""#), "{text}");
        assert!(text.contains(r#""ramp_30":"Infinity""#), "{text}");

        let back = BalancedNetwork::from_json(&text).unwrap();
        assert!(back.buses()[0].vm.is_nan());
        assert_eq!(back.branches()[0].x, f64::INFINITY);
        assert_eq!(back.generators()[0].caps[8], Some(f64::INFINITY));

        // Second write is byte stable, and the empty diagnostics channel
        // reflects that nothing was dropped.
        assert_eq!(back.to_json().unwrap(), text);
        let (_, diagnostics) = net.to_json_with_diagnostics().unwrap();
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn a_null_at_a_float_position_names_the_pre_090_spelling() {
        let net = BalancedNetwork::in_memory("nf", 100.0, vec![bus(1), bus(2)], Vec::new());
        let text = net
            .to_json()
            .unwrap()
            .replacen("\"vm\":1.0", "\"vm\":null", 1);
        assert!(text.contains("\"vm\":null"), "fixture edit failed: {text}");
        let err = BalancedNetwork::from_json(&text).unwrap_err().to_string();
        assert!(err.contains("before 0.9.0"), "{err}");
    }

    #[test]
    fn check_references_rejects_a_dangling_controlled_bus() {
        let mut net = BalancedNetwork::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        net.branches_mut().push(regulating_branch(9)); // controls a bus that doesn't exist
        let err = net.validate().unwrap_err().to_string();
        assert!(
            err.contains("transformer control references unknown bus 9"),
            "got {err}"
        );
    }

    /// A discrete switched shunt on bus 1 regulating the voltage at bus `reg`.
    fn switched_shunt(reg: usize) -> Shunt {
        Shunt {
            bus: BusId(1),
            g: 0.0,
            b: 19.0,
            in_service: true,
            control: Some(SwitchedShuntControl {
                mode: SwitchedShuntMode::Discrete,
                vhigh: 1.05,
                vlow: 0.95,
                control_bus: Some(BusId(reg)),
                rmpct: 100.0,
                blocks: vec![
                    ShuntBlock { steps: 2, b: 25.0 },
                    ShuntBlock { steps: 1, b: 50.0 },
                ],
            }),
            uid: None,
            extras: Extras::new(),
        }
    }

    #[test]
    fn switched_shunt_control_survives_json_transport() {
        let mut net =
            BalancedNetwork::in_memory("t", 100.0, vec![bus(1), bus(2), bus(3)], Vec::new());
        net.shunts_mut().push(switched_shunt(3));
        net.validate().unwrap();

        let back = BalancedNetwork::from_json(&net.to_json().unwrap()).unwrap();
        let c = back.shunts()[0].control.as_ref().unwrap();
        assert_eq!(c.mode, SwitchedShuntMode::Discrete);
        assert_eq!(c.control_bus, Some(BusId(3)));
        assert_eq!(c.blocks.len(), 2);
        close(c.blocks[1].b, 50.0);
    }

    #[test]
    fn check_references_rejects_a_dangling_switched_shunt_control_bus() {
        let mut net = BalancedNetwork::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        net.shunts_mut().push(switched_shunt(9)); // controls a bus that doesn't exist
        let err = net.validate().unwrap_err().to_string();
        assert!(
            err.contains("switched-shunt control references unknown bus 9"),
            "got {err}"
        );
    }

    #[test]
    fn validate_values_flags_and_repair_clamps_out_of_domain_values() {
        let mut net = BalancedNetwork::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        net.buses_mut()[0].vm = 0.0; // outside [0, 2]
        net.buses_mut()[1].va = 9000.0; // past ±2000°
        net.generators_mut().push(Generator {
            bus: BusId(1),
            pg: 10.0,
            qg: 0.0,
            pmax: 100.0,
            pmin: 0.0,
            qmax: 50.0,
            qmin: -50.0,
            vg: 0.0,    // non-positive setpoint
            mbase: 0.0, // non-positive base
            in_service: true,
            cost: None,
            caps: Default::default(),
            regulated_bus: None,
            uid: None,
        });

        let diags = net.validate_values();
        let fields: std::collections::BTreeSet<_> = diags
            .iter()
            .map(|d| d.details()["field"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            fields,
            ["mbase", "va", "vg", "vm"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            "all four out-of-domain fields reported"
        );
        assert!(
            diags
                .iter()
                .all(|d| d.code() == "VALIDATE.BALANCED.VALUE_DOMAIN" && d.target().is_some())
        );
        // Non-mutating: the network still holds the bad values.
        close(net.buses()[0].vm, 0.0);

        // The recorded path: repair the module, read the history entry.
        let module = powerio_core::PioModule::new(net);
        let module = repair_values(module).unwrap();
        let net = module.value();
        close(net.buses()[0].vm, 1.0);
        close(net.buses()[1].va, 0.0);
        close(net.generators()[0].mbase, 100.0); // → base_mva
        close(net.generators()[0].vg, 1.0);
        // Idempotent: nothing left to repair, and a second pass appends
        // nothing.
        assert!(net.validate_values().is_empty());
        let entries = module.history();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind(), powerio_core::HistoryKind::Repair);
        assert_eq!(
            entries[0].parameters()["repairs"].as_array().unwrap().len(),
            diags.len()
        );
        assert_eq!(module.diagnostics().len(), diags.len());
        let module = repair_values(module).unwrap();
        assert_eq!(module.history().len(), 1);
    }

    #[test]
    fn validate_values_is_empty_for_a_clean_network() {
        let net = BalancedNetwork::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        assert!(net.validate_values().is_empty());
    }
}
