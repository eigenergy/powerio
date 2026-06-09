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

/// Which format a [`Network`] was read from. Drives the same-format byte-exact
/// echo on write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SourceFormat {
    Matpower,
    PowerModelsJson,
    EgretJson,
    Psse,
    PowerWorld,
    /// Built in memory (e.g. from synth or an edited case); no source text.
    InMemory,
    /// A normalized derived view ([`Network::to_normalized`]): per unit, radians,
    /// filtered, densely reindexed. Distinct from [`InMemory`](SourceFormat::InMemory)
    /// so consumers can tell a per-unit product from a raw in-memory network; it
    /// has no source text and a different unit basis than a parsed network.
    Normalized,
}

/// A format-neutral power network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Network {
    pub name: String,
    pub base_mva: f64,
    pub buses: Vec<Bus>,
    pub loads: Vec<Load>,
    pub shunts: Vec<Shunt>,
    pub branches: Vec<Branch>,
    pub generators: Vec<Generator>,
    pub storage: Vec<Storage>,
    pub hvdc: Vec<Hvdc>,
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
    /// Shunt susceptance (MVAr at V = 1 p.u.).
    pub b: f64,
    pub in_service: bool,
    pub extras: Extras,
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

/// The MATPOWER gen capability / ramp columns past `PMIN`, in order. The index
/// into this array is the slot index into a [`GenCaps`].
pub(crate) const GEN_EXTRA_KEYS: [&str; 11] = [
    "pc1", "pc2", "qc1min", "qc1max", "qc2min", "qc2max", "ramp_agc", "ramp_10", "ramp_30",
    "ramp_q", "apf",
];

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
            buses,
            loads: Vec::new(),
            shunts: Vec::new(),
            branches,
            generators: Vec::new(),
            storage: Vec::new(),
            hvdc: Vec::new(),
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

    /// Rebuild a `Network` from JSON produced by [`to_json`](Network::to_json).
    ///
    /// Validates the result (unique bus ids, no dangling references) before
    /// returning, so the JSON transport — the C ABI and Julia bridge ride on it —
    /// can't hand back a network the file readers would have rejected.
    pub fn from_json(text: &str) -> crate::Result<Network> {
        let net: Network = serde_json::from_str(text).map_err(|e| Error::FormatRead {
            format: "JSON",
            message: e.to_string(),
        })?;
        net.check_references("JSON")?;
        Ok(net)
    }

    /// Whether this is a normalized (per-unit, radian, filtered, reindexed)
    /// derived product from [`to_normalized`](Network::to_normalized), rather
    /// than a raw network at the file's unit basis. Unit-sensitive code that
    /// takes a `&Network` can check this instead of silently assuming MW.
    #[must_use]
    pub fn is_normalized(&self) -> bool {
        self.source_format == SourceFormat::Normalized
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
        }
        for l in &self.loads {
            check(l.bus, "load")?;
        }
        for s in &self.shunts {
            check(s.bus, "shunt")?;
        }
        for g in &self.generators {
            check(g.bus, "generator")?;
        }
        for d in &self.hvdc {
            check(d.from, "dcline")?;
            check(d.to, "dcline")?;
        }
        for s in &self.storage {
            check(s.bus, "storage")?;
        }
        Ok(())
    }
}
