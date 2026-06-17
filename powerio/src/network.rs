//! Format-neutral network model — the hub every converter meets at.
//!
//! Readers map their format into a [`Network`]; writers map a `Network` back out.
//! It is the one canonical data model: format-neutral tables with loads and
//! shunts first-class, so a format that carries several loads per bus (PSS/E,
//! PowerModels) maps without losing them, while MATPOWER (which folds demand and
//! shunts onto the bus row) splits them out on read. The dense-indexed analysis
//! view the matrix builders consume is [`IndexedNetwork`](crate::IndexedNetwork),
//! derived from a `Network`. Two things make conversion honest:
//!
//! - **Retained source.** A `Network` keeps the raw text it was read from plus
//!   its [`SourceFormat`], so writing back to the *same* format echoes it
//!   byte-for-byte (no round-trip drift).
//! - **Extras passthrough.** Every element carries an [`Extras`] map of
//!   source-format fields the neutral model doesn't name, so X→`Network`→X keeps
//!   them and a cross-format writer can pass through what its target understands.
//!
//! Fully lossless any-to-any isn't possible (formats model different things);
//! the contract is byte-exact same-format and maximal-fidelity cross-format with
//! the writer reporting whatever it can't represent.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

/// Source-format fields the neutral model doesn't name, kept for round-trip and
/// cross-format passthrough. Keys are the field names; values are JSON scalars.
pub type Extras = BTreeMap<String, Value>;

/// System base frequency in hertz when a format records none. Power networks run
/// at 50 or 60 Hz; 60 is the default for the formats (MATPOWER, PowerModels,
/// egret) that carry no frequency field.
pub const DEFAULT_BASE_FREQUENCY: f64 = 60.0;

/// serde default for [`Network::base_frequency`], so JSON written before the
/// field existed still deserializes (the C ABI and Julia bridge ride on the JSON
/// transport).
fn default_base_frequency() -> f64 {
    DEFAULT_BASE_FREQUENCY
}

/// A bus identifier as it appears in the source file: the external, stable id
/// (1-based in MATPOWER, and possibly sparse — pegase has gaps in its ids).
/// Distinct from the dense `[0, n)` analysis index, which only
/// [`IndexedNetwork`](crate::IndexedNetwork) produces, via
/// [`bus_index`](crate::IndexedNetwork::bus_index). The two are both integers
/// and trivially confused; making the id its own type stops one being used where
/// the other is meant (using a 1-based id to index a matrix is off-by-one on a
/// contiguous case and pure garbage on a sparse one).
///
/// `#[serde(transparent)]` so the JSON transport carries a bare integer, not a
/// wrapper object — the wire format is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BusId(pub usize);

impl std::fmt::Display for BusId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Bus type per MATPOWER convention: 1=PQ, 2=PV, 3=ref/slack, 4=isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// `(q, c)` for the quadratic cost `½ q p² + c p` from a polynomial
    /// (model 2) row. MATPOWER stores `c2 p² + c1 p + c0`, so `q = 2·c2` and
    /// `c = c1`. Linear rows (`ncost == 2`) give `q = 0`. Piecewise (model 1)
    /// or cubic and higher return `None`.
    pub fn quadratic(&self) -> Option<(f64, f64)> {
        if self.model != 2 {
            return None;
        }
        // Reject a row whose coefficient slice is shorter than `ncost` claims,
        // rather than reading the wrong powers by position.
        if self.coeffs.len() < self.ncost {
            return None;
        }
        match self.ncost {
            3 => Some((2.0 * self.coeffs[0], self.coeffs[1])),
            2 => Some((0.0, self.coeffs[0])),
            1 => Some((0.0, 0.0)),
            _ => None,
        }
    }
}

/// Which format a [`Network`] was read from. Drives the same format byte exact
/// echo on write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SourceFormat {
    Matpower,
    PowerModelsJson,
    EgretJson,
    Psse,
    PowerWorld,
    PandapowerJson,
    /// Read from a GE PSLF `.epc` case. Same source text is retained, so a
    /// same-format write echoes it byte-for-byte; a cross-format or
    /// source-dropped write goes through the `.epc` serializer
    /// ([`write_pslf`](crate::write_pslf)).
    Pslf,
    /// Read from a PowerWorld `.pwb` binary case. Read only: there is no
    /// `.pwb` writer and no retained source text, so writing goes through
    /// another format's writer.
    PowerWorldBinary,
    /// Built in memory, for example from synth or an edited case; no source text.
    InMemory,
    /// A normalized derived view ([`Network::to_normalized`]): per unit, radians,
    /// filtered, source bus ids preserved. Distinct from
    /// [`InMemory`](SourceFormat::InMemory) so consumers can tell a per unit
    /// product from a raw in memory network; it has no source text and a different
    /// unit basis than a parsed network.
    Normalized,
    /// Read back from a gridfm-datakit Parquet dataset (the ML→classical bridge,
    /// `powerio-matrix`'s `read_gridfm_dataset`). A lossy, power flow complete
    /// reconstruction with no retained source text: original bus ids are
    /// synthesized `1..n`, per element load/shunt granularity is folded to one
    /// synthetic element per bus, and HVDC/storage/piecewise costs are absent.
    Gridfm,
    /// Read from a PyPSA CSV folder. This is a folder format rather than a
    /// single retained text document, so same-format writes are canonicalized.
    PypsaCsv,
}

/// A format-neutral power network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Network {
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
    pub buses: Vec<Bus>,
    pub loads: Vec<Load>,
    pub shunts: Vec<Shunt>,
    pub branches: Vec<Branch>,
    pub generators: Vec<Generator>,
    pub storage: Vec<Storage>,
    pub hvdc: Vec<Hvdc>,
    /// Three-winding transformers, kept as typed records rather than folded into
    /// `branches`, so a star point and the per-winding data survive a round trip.
    /// `#[serde(default)]` so JSON written before the field existed still
    /// deserializes. The matrix builders and any consumer that wants the expanded
    /// form (the planned distribution crate) call
    /// [`Transformer3W::star_expansion`].
    #[serde(default)]
    pub transformers_3w: Vec<Transformer3W>,
    /// Area records: scheduled interchange and per-area swing bus. Distinct from
    /// the bare `area` number on each [`Bus`]; this is the area's metadata, which
    /// every conversion dropped before. `#[serde(default)]` so older JSON still
    /// deserializes.
    #[serde(default)]
    pub areas: Vec<Area>,
    /// Solver / solution-control metadata when the source carries it, else `None`.
    /// `#[serde(default)]` so older JSON still deserializes.
    #[serde(default)]
    pub solver: Option<SolverParams>,
    pub source_format: SourceFormat,
    /// Raw source text, when read from a textual format; enables a byte-exact
    /// same-format round-trip. `Arc<String>` (not `Arc<str>`) is deliberate: a
    /// reader that already owns the buffer (the MATPOWER file path) moves it in
    /// with no second copy of the whole file. The trade is one extra indirection
    /// per access; don't "simplify" it back to `Arc<str>`, which would reintroduce
    /// the copy this avoids.
    ///
    /// Skipped in JSON: the structured tables are the transport, not the raw
    /// echo, and skipping also keeps serde's `rc` feature out of the build. A
    /// `from_json` round-trip returns this as `None`.
    #[serde(skip)]
    pub source: Option<Arc<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub extras: Extras,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Load {
    pub bus: BusId,
    /// Active demand (MW).
    pub p: f64,
    /// Reactive demand (MVAr).
    pub q: f64,
    pub in_service: bool,
    pub extras: Extras,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub extras: Extras,
}

/// How a switched shunt adjusts its susceptance. Maps to the PSS/E `MODSW` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShuntBlock {
    pub steps: u32,
    /// Susceptance increment per step (MVAr at V = 1 p.u.).
    pub b: f64,
}

/// Switching-control data for a switched shunt ([`Shunt::control`]): the mode,
/// the regulated voltage band and bus, the reactive-range percentage, and the
/// adjustable susceptance blocks. The shunt's [`b`](Shunt::b) is the initial
/// value within the blocks' total range.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub from: BusId,
    pub to: BusId,
    /// Series resistance (p.u.).
    pub r: f64,
    /// Series reactance (p.u.).
    pub x: f64,
    /// Total line-charging susceptance (p.u.); half goes to each end.
    pub b: f64,
    pub rate_a: f64,
    pub rate_b: f64,
    pub rate_c: f64,
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
    pub extras: Extras,
}

impl Branch {
    /// Effective tap ratio (0 ⇒ 1).
    #[must_use]
    pub fn effective_tap(&self) -> f64 {
        if self.tap == 0.0 { 1.0 } else { self.tap }
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

/// What a regulating transformer's tap (or phase shift) automatically controls.
/// Maps to the PSS/E control code `COD` and the PSLF transformer `type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TransformerControlMode {
    /// Fixed ratio, no automatic adjustment (PSS/E `COD` 0/±4, PSLF type 1).
    Fixed,
    /// Bus voltage control via tap (LTC; PSS/E `COD` ±1, PSLF type 2).
    Voltage,
    /// Reactive-power-flow control via tap (PSS/E `COD` ±2).
    ReactiveFlow,
    /// Active-power-flow control via phase shift (PSS/E `COD` ±3, PSLF type 4).
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub caps: GenCaps,
    /// The remote bus whose voltage this generator regulates, when that is not its
    /// own terminal bus (PSS/E `IREG`). `None` means it regulates its own bus.
    /// Part of the cross-element voltage-control graph: a format that names a
    /// remote regulated bus (PSS/E) keeps it across a round trip instead of
    /// collapsing every generator onto its own terminal. `#[serde(default)]` so
    /// JSON written before the field existed still deserializes.
    #[serde(default)]
    pub regulated_bus: Option<BusId>,
}

impl Generator {
    /// True when any capability / ramp column is present. Formats without those
    /// fields (PSS/E, PowerWorld) use this to warn on what they drop.
    #[must_use]
    pub fn has_caps(&self) -> bool {
        self.caps.iter().any(Option::is_some)
    }
}

/// A generator's capability / ramp columns, one slot per `GEN_EXTRA_KEYS` name.
pub type GenCaps = [Option<f64>; GEN_EXTRA_KEYS.len()];

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub qmin: f64,
    pub qmax: f64,
    pub r: f64,
    pub x: f64,
    pub p_loss: f64,
    pub q_loss: f64,
    pub in_service: bool,
    pub extras: Extras,
}

/// A two-terminal HVDC line (MATPOWER `dcline`).
///
/// `pf`/`pt`/`qf`/`qt` are stored in MATPOWER's sign convention regardless of
/// source: the PowerModels reader un-flips `pt`/`qf`/`qt` on the way in, and the
/// PowerModels writer re-flips them on the way out (PowerModels.jl uses the
/// opposite sign). The flip is a format-boundary translation, so a derived view
/// like `to_normalized` keeps the MATPOWER convention and only scales to per unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub extras: Extras,
}

/// An area record: the area's scheduled net interchange and its swing bus.
///
/// The [`number`](Area::number) matches the `area` field carried on each
/// [`Bus`]; this table holds the per-area metadata (the interchange target and
/// the area slack) that the bus number alone can't. Maps to the PSS/E area record
/// (`I, ISW, PDES, PTOL, ARNAME`).
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Solver / solution-control metadata: the Newton tolerance and iteration cap,
/// the zero-impedance threshold, and the per-quantity adjustment-enable flags.
///
/// Each field is optional because a source states only the ones it carries. No
/// power-flow physics, but it determines whether a downstream solver reproduces
/// the source tool's converged answer. Maps to the PSS/E v34+ system-wide block
/// (`GENERAL THRSHZ`, `NEWTON TOLN`/`ITMXN`, `SOLVER ACTAPS`/`AREAIN`/`PHSHFT`/
/// `DCTAPS`/`SWSHNT`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SolverParams {
    /// Newton power-flow mismatch tolerance (`NEWTON TOLN`).
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Impedance {
    pub r: f64,
    pub x: f64,
    pub base_mva: f64,
}

/// One winding of a [`Transformer3W`]: its terminal bus, off-nominal ratio, phase
/// shift, nominal voltage, and thermal ratings.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// A three-winding transformer: three terminal buses joined at a common star
/// point, with the series impedance given pairwise (winding 1-2, 2-3, 3-1).
///
/// Kept as a typed record (not three [`Branch`]es) so the star-point voltage and
/// the per-winding control data survive a same-format round trip. Both the PSS/E
/// 3-winding record and the PSLF tertiary-winding record map onto it.
/// [`star_expansion`](Transformer3W::star_expansion) turns it into the synthetic
/// star bus plus three branches that the matrix builders — and the planned
/// distribution crate — consume.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub extras: Extras,
}

impl Transformer3W {
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
    /// one per winding, for analysis consumers that work in the bus-branch model
    /// (the matrix builders, the distribution crate). The star bus carries the
    /// stored star voltage and the magnetizing shunt is left to the caller; each
    /// branch takes its winding's tap, phase shift, and ratings.
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
            extras: Extras::new(),
        };
        let zs = self.star_impedances();
        let branch = |w: &Winding, (r, x): (f64, f64)| Branch {
            from: w.bus,
            to: star_id,
            r,
            x,
            b: 0.0,
            rate_a: w.rate_a,
            rate_b: w.rate_b,
            rate_c: w.rate_c,
            tap: w.tap,
            shift: w.shift,
            in_service: self.in_service,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
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

/// A value-domain finding from [`Network::validate_values`]: an element field
/// whose value falls outside its physical range, paired with the value
/// [`repair`](Network::repair) would set in its place.
///
/// `#[non_exhaustive]`: a returns-only record, so downstream code reads it but
/// never constructs it, leaving room to add locator fields without a break.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// Human-readable element locator, e.g. `"bus 3"` or `"generator at bus 5"`.
    pub element: String,
    pub field: &'static str,
    pub old: f64,
    pub new: f64,
    pub reason: &'static str,
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

impl Network {
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
    ) -> Network {
        Network {
            name: name.into(),
            base_mva,
            base_frequency: DEFAULT_BASE_FREQUENCY,
            buses,
            loads: Vec::new(),
            shunts: Vec::new(),
            branches,
            generators: Vec::new(),
            storage: Vec::new(),
            hvdc: Vec::new(),
            transformers_3w: Vec::new(),
            areas: Vec::new(),
            solver: None,
            source_format: SourceFormat::InMemory,
            source: None,
        }
    }

    /// Serialize the structured tables to JSON — the transport the C ABI
    /// (`pio_to_json`) and the Julia bridge consume. The retained `source` text
    /// is excluded (see the field's `#[serde(skip)]`), so the byte-exact echo
    /// stays on the same-format write path; a [`from_json`](Network::from_json)
    /// round-trip reproduces every field except `source`, which returns `None`.
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string(self).map_err(|e| Error::FormatRead {
            format: "JSON",
            message: e.to_string(),
        })
    }

    /// Serialize this network to `format`, preserving the retained source text
    /// on same-format writes and reporting any target-format fidelity warnings.
    #[must_use]
    pub fn to_format(&self, format: crate::TargetFormat) -> crate::Conversion {
        crate::write_as(self, format)
    }

    /// Serialize this network to MATPOWER `.m` text.
    ///
    /// This is byte-exact when the network was parsed from MATPOWER and still
    /// carries its retained source text.
    #[must_use]
    pub fn to_matpower(&self) -> String {
        crate::write_matpower(self)
    }

    /// Rebuild a `Network` from JSON produced by [`to_json`](Network::to_json).
    ///
    /// Validates the result (no buses, unique bus ids, no dangling references)
    /// before returning, so the JSON transport — the C ABI and Julia bridge ride
    /// on it — can't hand back a network the file readers would have rejected
    /// (the same no-buses guard `read_source` applies to every parse path).
    pub fn from_json(text: &str) -> crate::Result<Network> {
        let net: Network = serde_json::from_str(text).map_err(|e| Error::FormatRead {
            format: "JSON",
            message: e.to_string(),
        })?;
        net.check_references("JSON")?;
        if net.buses.is_empty() {
            return Err(Error::FormatRead {
                format: "JSON",
                message: "case has no buses".into(),
            });
        }
        Ok(net)
    }

    /// Whether this is a normalized (per-unit, radian, filtered)
    /// derived product from [`to_normalized`](Network::to_normalized), rather
    /// than a raw network at the file's unit basis. Unit-sensitive code that
    /// takes a `&Network` can check this instead of silently assuming MW.
    #[must_use]
    pub fn is_normalized(&self) -> bool {
        self.source_format == SourceFormat::Normalized
    }

    /// Error unless `base_mva` is a positive, finite number. It is every
    /// per-unit divisor, so a malformed base would otherwise silently poison
    /// downstream values with `NaN`/`Inf` or flipped signs. The per-unit
    /// consumers ([`to_normalized`](Network::to_normalized), the gridfm
    /// export) call this; any other unit-sensitive consumer should too.
    pub fn check_base_mva(&self) -> crate::Result<()> {
        if self.base_mva.is_finite() && self.base_mva > 0.0 {
            Ok(())
        } else {
            Err(crate::Error::InvalidBaseMva {
                base: self.base_mva,
            })
        }
    }

    /// Report element fields whose values fall outside their physical domain,
    /// without changing anything. Each [`Diagnostic`] names the element, the
    /// field, the current value, the value [`repair`](Network::repair) would set,
    /// and why.
    ///
    /// This generalizes the per-reader value clamps (a bus voltage magnitude
    /// outside `[0, 2]`, an angle past `±2000°`, a zero generator MVA base or
    /// voltage setpoint) into one pass any consumer can run, separate from the
    /// structural [`validate`](Network::validate) (which only checks ids and
    /// references). It is non-mutating; call [`repair`](Network::repair) to apply
    /// the fixes.
    #[must_use]
    pub fn validate_values(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for b in &self.buses {
            if let Some(new) = repair_vm(b.vm) {
                out.push(Diagnostic {
                    element: format!("bus {}", b.id),
                    field: "vm",
                    old: b.vm,
                    new,
                    reason: "voltage magnitude outside [0, 2] p.u.",
                });
            }
            if let Some(new) = repair_va(b.va) {
                out.push(Diagnostic {
                    element: format!("bus {}", b.id),
                    field: "va",
                    old: b.va,
                    new,
                    reason: "voltage angle outside ±2000°",
                });
            }
        }
        for g in &self.generators {
            if let Some(new) = repair_mbase(g.mbase, self.base_mva) {
                out.push(Diagnostic {
                    element: format!("generator at bus {}", g.bus),
                    field: "mbase",
                    old: g.mbase,
                    new,
                    reason: "non-positive generator MVA base",
                });
            }
            if let Some(new) = repair_vg(g.vg) {
                out.push(Diagnostic {
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
    /// [`validate_values`](Network::validate_values) reports), returning the list
    /// of changes made. A second call returns an empty list (the values are now
    /// in domain).
    pub fn repair(&mut self) -> Vec<Diagnostic> {
        let findings = self.validate_values();
        let sbase = self.base_mva;
        for b in &mut self.buses {
            if let Some(new) = repair_vm(b.vm) {
                b.vm = new;
            }
            if let Some(new) = repair_va(b.va) {
                b.va = new;
            }
        }
        for g in &mut self.generators {
            if let Some(new) = repair_mbase(g.mbase, sbase) {
                g.mbase = new;
            }
            if let Some(new) = repair_vg(g.vg) {
                g.vg = new;
            }
        }
        findings
    }

    /// Check structural integrity: bus ids are unique and every element
    /// references an existing bus. The file readers and [`from_json`](Network::from_json)
    /// run this; a `Network` built by hand (or mutated, e.g. by a scenario
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
        let mut ids = std::collections::HashSet::with_capacity(self.buses.len());
        for b in &self.buses {
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
        for (i, br) in self.branches.iter().enumerate() {
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
        for l in &self.loads {
            check(l.bus, "load")?;
        }
        for s in &self.shunts {
            check(s.bus, "shunt")?;
            if let Some(bus) = s.control.as_ref().and_then(|c| c.control_bus) {
                check(bus, "switched-shunt control")?;
            }
        }
        for g in &self.generators {
            check(g.bus, "generator")?;
            if let Some(bus) = g.regulated_bus {
                check(bus, "generator voltage control")?;
            }
        }
        for d in &self.hvdc {
            check(d.from, "dcline")?;
            check(d.to, "dcline")?;
        }
        for s in &self.storage {
            check(s.bus, "storage")?;
        }
        for a in &self.areas {
            if let Some(slack) = a.slack_bus {
                check(slack, "area swing")?;
            }
        }
        for t in &self.transformers_3w {
            for w in &t.windings {
                check(w.bus, "3-winding transformer")?;
            }
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
            extras: Extras::new(),
        }
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
        let mut net = Network::in_memory("t", 100.0, vec![bus(1), bus(2), bus(3)], Vec::new());
        net.transformers_3w.push(transformer_3w());
        net.validate().unwrap();

        let back = Network::from_json(&net.to_json().unwrap()).unwrap();
        assert_eq!(back.transformers_3w.len(), 1);
        close(back.transformers_3w[0].z[1].x, 0.20);
        assert_eq!(back.transformers_3w[0].windings[2].bus, BusId(3));
    }

    #[test]
    fn check_references_rejects_a_dangling_winding_bus() {
        let mut net = Network::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        net.transformers_3w.push(transformer_3w()); // winding 3 references bus 3
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
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
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
            extras: Extras::new(),
        }
    }

    #[test]
    fn transformer_control_survives_json_transport() {
        let mut net = Network::in_memory("t", 100.0, vec![bus(1), bus(2), bus(3)], Vec::new());
        net.branches.push(regulating_branch(3));
        net.validate().unwrap();

        let back = Network::from_json(&net.to_json().unwrap()).unwrap();
        let c = back.branches[0].control.as_ref().unwrap();
        assert_eq!(c.mode, TransformerControlMode::Voltage);
        assert_eq!(c.controlled_bus, Some(BusId(3)));
        close(c.tap_max, 1.05);
        assert_eq!(c.ntp, 17);
    }

    #[test]
    fn check_references_rejects_a_dangling_controlled_bus() {
        let mut net = Network::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        net.branches.push(regulating_branch(9)); // controls a bus that doesn't exist
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
            extras: Extras::new(),
        }
    }

    #[test]
    fn switched_shunt_control_survives_json_transport() {
        let mut net = Network::in_memory("t", 100.0, vec![bus(1), bus(2), bus(3)], Vec::new());
        net.shunts.push(switched_shunt(3));
        net.validate().unwrap();

        let back = Network::from_json(&net.to_json().unwrap()).unwrap();
        let c = back.shunts[0].control.as_ref().unwrap();
        assert_eq!(c.mode, SwitchedShuntMode::Discrete);
        assert_eq!(c.control_bus, Some(BusId(3)));
        assert_eq!(c.blocks.len(), 2);
        close(c.blocks[1].b, 50.0);
    }

    #[test]
    fn check_references_rejects_a_dangling_switched_shunt_control_bus() {
        let mut net = Network::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        net.shunts.push(switched_shunt(9)); // controls a bus that doesn't exist
        let err = net.validate().unwrap_err().to_string();
        assert!(
            err.contains("switched-shunt control references unknown bus 9"),
            "got {err}"
        );
    }

    #[test]
    fn validate_values_flags_and_repair_clamps_out_of_domain_values() {
        let mut net = Network::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        net.buses[0].vm = 0.0; // outside [0, 2]
        net.buses[1].va = 9000.0; // past ±2000°
        net.generators.push(Generator {
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
        });

        let diags = net.validate_values();
        let fields: std::collections::BTreeSet<_> = diags.iter().map(|d| d.field).collect();
        assert_eq!(
            fields,
            ["mbase", "va", "vg", "vm"].into_iter().collect(),
            "all four out-of-domain fields reported"
        );
        // Non-mutating: the network still holds the bad values.
        close(net.buses[0].vm, 0.0);

        let applied = net.repair();
        assert_eq!(applied.len(), diags.len());
        close(net.buses[0].vm, 1.0);
        close(net.buses[1].va, 0.0);
        close(net.generators[0].mbase, 100.0); // → base_mva
        close(net.generators[0].vg, 1.0);
        // Idempotent: nothing left to repair.
        assert!(net.validate_values().is_empty());
    }

    #[test]
    fn validate_values_is_empty_for_a_clean_network() {
        let net = Network::in_memory("t", 100.0, vec![bus(1), bus(2)], Vec::new());
        assert!(net.validate_values().is_empty());
    }
}
