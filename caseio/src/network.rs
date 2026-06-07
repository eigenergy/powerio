//! Format-neutral network model — the hub every converter meets at.
//!
//! Readers map their format into a [`Network`]; writers map a `Network` back out.
//! Unlike [`MpcCase`](crate::MpcCase) (the MATPOWER-shaped view the matrix layer
//! uses, with demand and shunts folded into the bus row), `Network` makes loads
//! and shunts first-class, so a format that carries several loads per bus (PSS/E,
//! PowerModels) maps without losing them. Two things make conversion honest:
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

use serde_json::Value;

use crate::case::{BusType, DcLine, GenCost, Generator as MpcGen, MpcCase, Storage as MpcStorage};
use crate::Error;

/// Source-format fields the neutral model doesn't name, kept for round-trip and
/// cross-format passthrough. Keys are the field names; values are JSON scalars.
pub type Extras = BTreeMap<String, Value>;

/// Which format a [`Network`] was read from. Drives the same-format byte-exact
/// echo on write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Matpower,
    PowerModelsJson,
    EgretJson,
    Psse,
    PowerWorld,
    /// Built in memory (e.g. from synth or an edited case); no source text.
    InMemory,
}

/// A format-neutral power network.
#[derive(Debug, Clone)]
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
    /// same-format round-trip.
    pub source: Option<Arc<str>>,
}

#[derive(Debug, Clone)]
pub struct Bus {
    /// Stable bus id (1-based in MATPOWER; preserved verbatim).
    pub id: usize,
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

#[derive(Debug, Clone)]
pub struct Load {
    pub bus: usize,
    /// Active demand (MW).
    pub p: f64,
    /// Reactive demand (MVAr).
    pub q: f64,
    pub in_service: bool,
    pub extras: Extras,
}

#[derive(Debug, Clone)]
pub struct Shunt {
    pub bus: usize,
    /// Shunt conductance (MW at V = 1 p.u.).
    pub g: f64,
    /// Shunt susceptance (MVAr at V = 1 p.u.).
    pub b: f64,
    pub in_service: bool,
    pub extras: Extras,
}

#[derive(Debug, Clone)]
pub struct Branch {
    pub from: usize,
    pub to: usize,
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
}

#[derive(Debug, Clone)]
pub struct Generator {
    pub bus: usize,
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
    pub extras: Extras,
}

#[derive(Debug, Clone)]
pub struct Storage {
    pub bus: usize,
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
#[derive(Debug, Clone)]
pub struct Hvdc {
    pub from: usize,
    pub to: usize,
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

/// The MATPOWER gen capability / ramp columns past `PMIN`, in order. Carried as
/// generator extras so they survive into formats that name them (PowerModels).
const GEN_EXTRA_KEYS: [&str; 11] = [
    "pc1", "pc2", "qc1min", "qc1max", "qc2min", "qc2max", "ramp_agc", "ramp_10",
    "ramp_30", "ramp_q", "apf",
];

fn num(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

impl MpcCase {
    /// Lift this MATPOWER-shaped case into the neutral [`Network`]: split bus
    /// demand into [`Load`]s and bus shunts into [`Shunt`]s, carry the gen
    /// capability columns as extras, and keep the source for a byte-exact
    /// MATPOWER round-trip. MATPOWER is just one reader into the hub.
    #[must_use]
    pub fn to_network(&self) -> Network {
        let buses = self
            .buses
            .iter()
            .map(|b| Bus {
                id: b.id,
                kind: b.kind,
                vm: b.vm,
                va: b.va,
                base_kv: b.base_kv,
                vmax: b.vmax,
                vmin: b.vmin,
                area: b.area,
                zone: b.zone,
                name: b.name.clone(),
                extras: Extras::new(),
            })
            .collect();

        let mut loads = Vec::new();
        let mut shunts = Vec::new();
        for b in &self.buses {
            let in_service = b.kind != BusType::Isolated;
            if b.pd != 0.0 || b.qd != 0.0 {
                loads.push(Load { bus: b.id, p: b.pd, q: b.qd, in_service, extras: Extras::new() });
            }
            if b.gs != 0.0 || b.bs != 0.0 {
                shunts.push(Shunt { bus: b.id, g: b.gs, b: b.bs, in_service, extras: Extras::new() });
            }
        }

        let branches = self
            .branches
            .iter()
            .map(|br| Branch {
                from: br.from_id,
                to: br.to_id,
                r: br.r,
                x: br.x,
                b: br.b,
                rate_a: br.rate_a,
                rate_b: br.rate_b,
                rate_c: br.rate_c,
                tap: br.tap,
                shift: br.shift,
                in_service: br.is_in_service(),
                angmin: br.angmin,
                angmax: br.angmax,
                extras: Extras::new(),
            })
            .collect();

        let generators = self.gens.iter().map(gen_to_network).collect();
        let storage = self.storage.iter().map(storage_to_network).collect();
        let hvdc = self.dclines.iter().map(hvdc_to_network).collect();

        Network {
            name: self.name.clone(),
            base_mva: self.base_mva,
            buses,
            loads,
            shunts,
            branches,
            generators,
            storage,
            hvdc,
            source_format: SourceFormat::Matpower,
            source: self.source().map(Arc::from),
        }
    }
}

impl Network {
    /// Error if two buses share an id, or if any element references a bus that
    /// doesn't exist. Readers call this after parsing so a missing/garbled id
    /// (which would otherwise default to a placeholder and silently re-wire the
    /// network) fails loudly instead.
    pub(crate) fn check_references(&self, format: &'static str) -> crate::Result<()> {
        let mut ids = std::collections::BTreeSet::new();
        for b in &self.buses {
            if !ids.insert(b.id) {
                return Err(Error::FormatRead {
                    format,
                    message: format!("duplicate bus id {}", b.id),
                });
            }
        }
        let check = |bus: usize, what: &str| -> crate::Result<()> {
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

    /// Fold the neutral model back into the MATPOWER-shaped [`MpcCase`] the matrix
    /// layer and the MATPOWER writer use: loads and shunts are summed back onto
    /// their bus. Used to emit canonical MATPOWER from a non-MATPOWER source. The
    /// result carries no source document, so the MATPOWER writer serializes
    /// canonically.
    #[must_use]
    pub fn to_mpc_case(&self) -> MpcCase {
        use crate::case::{Branch as McBranch, Bus as McBus, DcLine as McDcLine};

        // Aggregate demand and shunts onto their bus (MATPOWER allows one of each).
        let mut demand: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
        for l in &self.loads {
            let e = demand.entry(l.bus).or_default();
            e.0 += l.p;
            e.1 += l.q;
        }
        let mut shunt: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
        for s in &self.shunts {
            let e = shunt.entry(s.bus).or_default();
            e.0 += s.g;
            e.1 += s.b;
        }

        let buses = self
            .buses
            .iter()
            .map(|b| {
                let (pd, qd) = demand.get(&b.id).copied().unwrap_or((0.0, 0.0));
                let (gs, bs) = shunt.get(&b.id).copied().unwrap_or((0.0, 0.0));
                McBus {
                    id: b.id,
                    kind: b.kind,
                    pd,
                    qd,
                    gs,
                    bs,
                    area: b.area,
                    vm: b.vm,
                    va: b.va,
                    base_kv: b.base_kv,
                    zone: b.zone,
                    vmax: b.vmax,
                    vmin: b.vmin,
                    name: b.name.clone(),
                }
            })
            .collect();

        let branches = self
            .branches
            .iter()
            .map(|br| McBranch {
                from_id: br.from,
                to_id: br.to,
                r: br.r,
                x: br.x,
                b: br.b,
                rate_a: br.rate_a,
                rate_b: br.rate_b,
                rate_c: br.rate_c,
                tap: br.tap,
                shift: br.shift,
                status: f64::from(br.in_service),
                angmin: br.angmin,
                angmax: br.angmax,
            })
            .collect();

        let gens = self.generators.iter().map(gen_to_mpc).collect();
        let storage = self.storage.iter().map(storage_to_mpc).collect();
        let dclines: Vec<McDcLine> = self.hvdc.iter().map(hvdc_to_mpc).collect();

        MpcCase::new(self.name.clone(), self.base_mva, buses, branches)
            .with_gens(gens)
            .with_storage(storage)
            .with_dclines(dclines)
    }
}

fn gen_to_mpc(g: &Generator) -> MpcGen {
    // Reconstruct the contiguous MATPOWER capability-column prefix from extras.
    let mut extra = Vec::new();
    for k in GEN_EXTRA_KEYS {
        match g.extras.get(k).and_then(Value::as_f64) {
            Some(v) => extra.push(v),
            None => break,
        }
    }
    MpcGen {
        bus_id: g.bus,
        pg: g.pg,
        qg: g.qg,
        qmax: g.qmax,
        qmin: g.qmin,
        vg: g.vg,
        mbase: g.mbase,
        status: f64::from(g.in_service),
        pmax: g.pmax,
        pmin: g.pmin,
        cost: g.cost.clone(),
        extra,
    }
}

fn storage_to_mpc(s: &Storage) -> MpcStorage {
    MpcStorage {
        bus_id: s.bus,
        ps: s.ps,
        qs: s.qs,
        energy: s.energy,
        energy_rating: s.energy_rating,
        charge_rating: s.charge_rating,
        discharge_rating: s.discharge_rating,
        charge_efficiency: s.charge_efficiency,
        discharge_efficiency: s.discharge_efficiency,
        thermal_rating: s.thermal_rating,
        qmin: s.qmin,
        qmax: s.qmax,
        r: s.r,
        x: s.x,
        p_loss: s.p_loss,
        q_loss: s.q_loss,
        status: f64::from(s.in_service),
    }
}

fn hvdc_to_mpc(d: &Hvdc) -> DcLine {
    DcLine {
        from_id: d.from,
        to_id: d.to,
        status: f64::from(d.in_service),
        pf: d.pf,
        pt: d.pt,
        qf: d.qf,
        qt: d.qt,
        vf: d.vf,
        vt: d.vt,
        pmin: d.pmin,
        pmax: d.pmax,
        qminf: d.qminf,
        qmaxf: d.qmaxf,
        qmint: d.qmint,
        qmaxt: d.qmaxt,
        loss0: d.loss0,
        loss1: d.loss1,
        extra: Vec::new(),
    }
}

fn gen_to_network(g: &MpcGen) -> Generator {
    let extras = GEN_EXTRA_KEYS
        .iter()
        .zip(&g.extra)
        .map(|(&k, &v)| (k.to_string(), num(v)))
        .collect();
    Generator {
        bus: g.bus_id,
        pg: g.pg,
        qg: g.qg,
        pmax: g.pmax,
        pmin: g.pmin,
        qmax: g.qmax,
        qmin: g.qmin,
        vg: g.vg,
        mbase: g.mbase,
        in_service: g.is_in_service(),
        cost: g.cost.clone(),
        extras,
    }
}

fn storage_to_network(s: &MpcStorage) -> Storage {
    Storage {
        bus: s.bus_id,
        ps: s.ps,
        qs: s.qs,
        energy: s.energy,
        energy_rating: s.energy_rating,
        charge_rating: s.charge_rating,
        discharge_rating: s.discharge_rating,
        charge_efficiency: s.charge_efficiency,
        discharge_efficiency: s.discharge_efficiency,
        thermal_rating: s.thermal_rating,
        qmin: s.qmin,
        qmax: s.qmax,
        r: s.r,
        x: s.x,
        p_loss: s.p_loss,
        q_loss: s.q_loss,
        in_service: s.is_in_service(),
        extras: Extras::new(),
    }
}

fn hvdc_to_network(d: &DcLine) -> Hvdc {
    Hvdc {
        from: d.from_id,
        to: d.to_id,
        in_service: d.is_in_service(),
        pf: d.pf,
        pt: d.pt,
        qf: d.qf,
        qt: d.qt,
        vf: d.vf,
        vt: d.vt,
        pmin: d.pmin,
        pmax: d.pmax,
        qminf: d.qminf,
        qmaxf: d.qmaxf,
        qmint: d.qmint,
        qmaxt: d.qmaxt,
        loss0: d.loss0,
        loss1: d.loss1,
        extras: Extras::new(),
    }
}
