//! Write a [`Network`] back out as a MATPOWER `.m` file.
//!
//! When the network was read from MATPOWER text it carries its original source,
//! and the writer echoes it verbatim — an exact round-trip that preserves every
//! field, comment, and numeric token. A network built in memory (e.g. by
//! `synth`) or read from another format has no MATPOWER source, so the writer
//! falls back to canonical serialization, folding loads and shunts back onto the
//! bus row.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::format::{Conversion, warn_extra_branch_rating_sets};
use crate::network::{BusId, Network, SourceFormat};

/// Serialize `net` to MATPOWER `.m` text. Echoes the retained source verbatim
/// when `net` came from MATPOWER; otherwise emits canonical `.m`.
#[must_use]
pub fn write_matpower(net: &Network) -> String {
    match &net.source {
        Some(text) if net.source_format == SourceFormat::Matpower => text.to_string(),
        _ => canonical(net),
    }
}

/// MATPOWER conversion with fidelity warnings. The byte-exact echo path (a
/// network that kept its MATPOWER source) drops nothing; the canonical path
/// can't carry everything the neutral model holds, so it itemizes what it leaves
/// out — the cross-format leg of the fidelity behavior (see [`Conversion`]).
pub(crate) fn write_matpower_conversion(net: &Network) -> Conversion {
    let text = write_matpower(net);
    // Echoed retained MATPOWER source: byte-exact, nothing dropped.
    if net.source.is_some() && net.source_format == SourceFormat::Matpower {
        return Conversion {
            text,
            warnings: Vec::new(),
        };
    }

    let warnings = canonical_warnings(net);
    Conversion { text, warnings }
}

#[expect(clippy::too_many_lines)]
fn canonical_warnings(net: &Network) -> Vec<String> {
    // The canonical writer (see `canonical`) emits the standard bus/branch/gen/
    // gencost/storage blocks only. Report every neutral-model field it can't.
    let mut warnings = Vec::new();
    if !net.hvdc.is_empty() {
        warnings.push(format!(
            "{} HVDC dcline(s) dropped: the canonical MATPOWER writer emits no `mpc.dcline` block",
            net.hvdc.len()
        ));
    }
    if !net.switches.is_empty() {
        warnings.push(format!(
            "{} switch(es) dropped: MATPOWER has no switch table",
            net.switches.len()
        ));
    }
    if !net.transformers_3w.is_empty() {
        warnings.push(format!(
            "{} 3-winding transformer(s) dropped: the canonical MATPOWER writer emits no \
             3-winding record (star-expand them into branches before writing to keep them)",
            net.transformers_3w.len()
        ));
    }
    if net
        .buses
        .iter()
        .any(|b| b.evhi.is_some() || b.evlo.is_some())
    {
        warnings.push(
            "emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band"
                .into(),
        );
    }
    let with_caps = net.generators.iter().filter(|g| g.has_caps()).count();
    if with_caps > 0 {
        warnings.push(format!(
            "generator capability/ramp columns dropped for {with_caps} generator(s): the canonical MATPOWER writer emits only the standard gen columns"
        ));
    }
    let non_matpower_charging = net
        .branches
        .iter()
        .filter(|b| b.has_non_matpower_charging())
        .count();
    if non_matpower_charging > 0 {
        warnings.push(format!(
            "{non_matpower_charging} branch terminal admittance record(s) collapsed to total susceptance: MATPOWER cannot carry conductance or asymmetric terminal charging"
        ));
    }
    let current_ratings = net
        .branches
        .iter()
        .filter(|b| b.current_ratings.is_some())
        .count();
    if current_ratings > 0 {
        warnings.push(format!(
            "{current_ratings} branch current rating record(s) dropped: MATPOWER branch rows carry MVA ratings only"
        ));
    }
    warn_extra_branch_rating_sets("MATPOWER .m", net, &mut warnings);
    let branch_solutions = net.branches.iter().filter(|b| b.solution.is_some()).count();
    if branch_solutions > 0 {
        warnings.push(format!(
            "{branch_solutions} branch solution value set(s) dropped: MATPOWER branch rows do not carry solved flow columns"
        ));
    }
    let voltage_loads = net
        .loads
        .iter()
        .filter(|l| {
            l.voltage_model
                .as_ref()
                .is_some_and(crate::network::LoadVoltageModel::has_non_matpower_fields)
        })
        .count();
    if voltage_loads > 0 {
        warnings.push(format!(
            "{voltage_loads} voltage dependent load model(s) dropped: MATPOWER carries only static Pd/Qd"
        ));
    }
    let with_cost = net.generators.iter().filter(|g| g.cost.is_some()).count();
    if !net.generators.is_empty() && with_cost == 0 {
        warnings.push(format!(
            "generator costs absent for {} generator(s): omitted `mpc.gencost`; no zero costs synthesized",
            net.generators.len()
        ));
    } else if with_cost > 0 && with_cost < net.generators.len() {
        warnings.push(format!(
            "gen cost dropped: {with_cost} of {} generators carry cost data, but MATPOWER's `mpc.gencost` block is all-or-nothing",
            net.generators.len()
        ));
    }
    let has_extras = net.buses.iter().any(|b| !b.extras.is_empty())
        || net.branches.iter().any(|b| !b.extras.is_empty())
        || net.loads.iter().any(|l| !l.extras.is_empty())
        || net.shunts.iter().any(|s| !s.extras.is_empty())
        || net.storage.iter().any(|s| !s.extras.is_empty())
        || net.hvdc.iter().any(|d| !d.extras.is_empty());
    if has_extras {
        warnings.push(
            "source-format passthrough fields (extras) dropped: the canonical MATPOWER writer emits only named columns".to_string(),
        );
    }
    warnings
}

/// Canonical MATPOWER from the neutral model, for networks with no MATPOWER
/// source. Loads and shunts are summed back onto their bus (MATPOWER carries one
/// of each per bus). Emits valid `.m` (values equal, formatting normalized); not
/// byte-exact. HVDC lines are not emitted.
#[allow(clippy::too_many_lines)] // flat per-section serializer; splitting adds noise
fn canonical(net: &Network) -> String {
    // Aggregate demand and shunts onto their bus.
    let mut demand: BTreeMap<BusId, (f64, f64)> = BTreeMap::new();
    for l in &net.loads {
        let e = demand.entry(l.bus).or_default();
        e.0 += l.p;
        e.1 += l.q;
    }
    let mut shunt: BTreeMap<BusId, (f64, f64)> = BTreeMap::new();
    for s in &net.shunts {
        let e = shunt.entry(s.bus).or_default();
        e.0 += s.g;
        e.1 += s.b;
    }

    let mut s = String::new();
    let _ = writeln!(s, "function mpc = {}", matlab_ident(&net.name));
    let _ = writeln!(s, "mpc.version = '2';");
    let _ = writeln!(s, "mpc.baseMVA = {};", net.base_mva);

    let _ = writeln!(s, "mpc.bus = [");
    for b in &net.buses {
        let (pd, qd) = demand.get(&b.id).copied().unwrap_or((0.0, 0.0));
        let (gs, bs) = shunt.get(&b.id).copied().unwrap_or((0.0, 0.0));
        let _ = writeln!(
            s,
            "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{};",
            b.id,
            b.kind as u8,
            pd,
            qd,
            gs,
            bs,
            b.area,
            b.vm,
            b.va,
            b.base_kv,
            b.zone,
            b.vmax,
            b.vmin
        );
    }
    let _ = writeln!(s, "];");

    let _ = writeln!(s, "mpc.branch = [");
    for br in &net.branches {
        let _ = writeln!(
            s,
            "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{};",
            br.from,
            br.to,
            br.r,
            br.x,
            br.terminal_charging().total_b(),
            br.rate_a,
            br.rate_b,
            br.rate_c,
            br.tap,
            br.shift,
            f64::from(br.in_service),
            br.angmin,
            br.angmax
        );
    }
    let _ = writeln!(s, "];");

    if !net.generators.is_empty() {
        let _ = writeln!(s, "mpc.gen = [");
        for g in &net.generators {
            let _ = writeln!(
                s,
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{};",
                g.bus,
                g.pg,
                g.qg,
                g.qmax,
                g.qmin,
                g.vg,
                g.mbase,
                f64::from(g.in_service),
                g.pmax,
                g.pmin
            );
        }
        let _ = writeln!(s, "];");

        if net.generators.iter().all(|g| g.cost.is_some()) {
            let _ = writeln!(s, "mpc.gencost = [");
            // MATPOWER's gencost is a rectangular matrix: pad every row's cost
            // values to the widest one with trailing zeros (a case that mixes
            // piecewise and polynomial models has rows of different lengths).
            let width = net
                .generators
                .iter()
                .filter_map(|g| g.cost.as_ref())
                .map(|c| c.coeffs.len())
                .max()
                .unwrap_or(0);
            for g in &net.generators {
                let c = g.cost.as_ref().expect("checked all gens have cost");
                let _ = write!(
                    s,
                    "\t{}\t{}\t{}\t{}",
                    c.model, c.startup, c.shutdown, c.ncost
                );
                for j in 0..width {
                    let _ = write!(s, "\t{}", c.coeffs.get(j).copied().unwrap_or(0.0));
                }
                let _ = writeln!(s, ";");
            }
            let _ = writeln!(s, "];");
        }
    }

    if !net.storage.is_empty() {
        let _ = writeln!(s, "mpc.storage = [");
        for st in &net.storage {
            let _ = writeln!(
                s,
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{};",
                st.bus,
                st.ps,
                st.qs,
                st.energy,
                st.energy_rating,
                st.charge_rating,
                st.discharge_rating,
                st.charge_efficiency,
                st.discharge_efficiency,
                st.thermal_rating,
                st.qmin,
                st.qmax,
                st.r,
                st.x,
                st.p_loss,
                st.q_loss,
                f64::from(st.in_service)
            );
        }
        let _ = writeln!(s, "];");
    }

    s
}

/// Coerce a case name into a legal MATLAB identifier for the `function` header:
/// non-alphanumeric chars become `_`, and a leading non-letter is prefixed so
/// a synth case named e.g. `"grid-1"` still writes a parseable `.m`.
fn matlab_ident(name: &str) -> String {
    let mut ident: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !ident.starts_with(|c: char| c.is_ascii_alphabetic()) {
        ident.insert(0, 'c');
    }
    ident
}
