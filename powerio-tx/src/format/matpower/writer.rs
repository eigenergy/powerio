//! Write a [`BalancedNetwork`] back out as a MATPOWER `.m` file.
//!
//! When the network was read from MATPOWER text it carries its original source,
//! and the writer echoes it verbatim — an exact round-trip that preserves every
//! field, comment, and numeric token. A network built in memory (e.g. by
//! `synth`) or read from another format has no MATPOWER source, so the writer
//! falls back to canonical serialization, folding loads and shunts back onto the
//! bus row.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::diagnostics::Diagnostics;
use crate::format::{Conversion, warn_extra_branch_rating_sets};

/// The MATPOWER write side family, named once for the block below.
use crate::diagnostics::codes::EMIT_MATPOWER as F;
use crate::network::{BalancedNetwork, BusId, GenCost, Generator, SourceFormat};

/// Serialize `net` to MATPOWER `.m` text. Echoes the retained source verbatim
/// when `net` came from MATPOWER; otherwise emits canonical `.m`.
#[must_use]
pub fn write_matpower(net: &BalancedNetwork) -> String {
    match &net.source {
        Some(text) if net.source_format == SourceFormat::Matpower => text.to_string(),
        _ => canonical(net),
    }
}

/// MATPOWER conversion with fidelity warnings. The byte-exact echo path (a
/// network that kept its MATPOWER source) drops nothing; the canonical path
/// can't carry everything the neutral model holds, so it itemizes what it leaves
/// out — the cross-format leg of the fidelity behavior (see [`Conversion`]).
pub(crate) fn write_matpower_conversion(net: &BalancedNetwork) -> Conversion {
    let text = write_matpower(net);
    // Echoed retained MATPOWER source: byte-exact, nothing dropped.
    if net.source.is_some() && net.source_format == SourceFormat::Matpower {
        return Conversion::faithful(text);
    }

    Conversion::new(text, canonical_warnings(net))
}

/// One bus name as a MATLAB single-quoted string body.
///
/// A single quote doubles, as MATLAB requires. A control character becomes a
/// space: a single-quoted literal cannot span a line, so a name holding a
/// newline writes a file MATPOWER's own loader will not parse, and powerio
/// reading its own output back is no evidence otherwise.
fn matlab_string(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .replace('\'', "''")
}

// One block per field the canonical writer cannot carry; splitting it would
// scatter a list that is only useful read end to end.
#[allow(clippy::too_many_lines)]
fn canonical_warnings(net: &BalancedNetwork) -> Diagnostics {
    // The canonical writer (see `canonical`) emits the standard bus/branch/gen/
    // gencost/dcline/dclinecost/storage blocks only. Report every neutral-model
    // field it can't.
    let mut warnings = Diagnostics::new();
    if !net.switches.is_empty() {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} switch(es) dropped: MATPOWER has no switch table",
                net.switches.len()
            ),
        );
    }
    if !net.transformers_3w.is_empty() {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} 3-winding transformer(s) dropped: the canonical MATPOWER writer emits no \
             3-winding record (star-expand them into branches before writing to keep them)",
                net.transformers_3w.len()
            ),
        );
    }
    if net
        .buses
        .iter()
        .any(|b| b.evhi.is_some() || b.evlo.is_some())
    {
        warnings.push(
            &F.field_dropped,
            "emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band",
        );
    }
    let non_matpower_charging = net
        .branches
        .iter()
        .filter(|b| b.has_non_matpower_charging())
        .count();
    if non_matpower_charging > 0 {
        warnings.push(&F.value_collapsed, format!(
            "{non_matpower_charging} branch terminal admittance record(s) collapsed to total susceptance: MATPOWER cannot carry conductance or asymmetric terminal charging"
        ));
    }
    let current_ratings = net
        .branches
        .iter()
        .filter(|b| b.current_ratings.is_some())
        .count();
    if current_ratings > 0 {
        warnings.push(&F.field_dropped, format!(
            "{current_ratings} branch current rating record(s) dropped: MATPOWER branch rows carry MVA ratings only"
        ));
    }
    // The 21-column gen row is all-or-nothing: MATPOWER's matrix is
    // rectangular, so once any generator states capability or ramp data every
    // row grows the columns and the ones with nothing to say pad with zeros.
    // Zero is not "unspecified" in those columns — `RAMP_10 = 0` states a unit
    // that cannot ramp — so the padding is a disclosure, and the readback
    // cannot tell it from stated data.
    let with_caps = net.generators.iter().any(Generator::has_caps);
    if with_caps {
        let padded = net.generators.iter().filter(|g| !g.has_caps()).count();
        if padded > 0 {
            warnings.push(
                &F.value_defaulted,
                format!(
                    "{padded} generator(s) with no capability or ramp data written with zeros in \
                 columns 11-21: MATPOWER's gen matrix is rectangular, and a zero there reads \
                 back as a stated limit rather than as absent"
                ),
            );
        }
    }
    // An out of service load or shunt has no spelling in a MATPOWER bus row:
    // the row states one demand and one shunt with no status. Writing the
    // value anyway states an idle element as live load, so it is dropped and
    // named here.
    let idle_load: (f64, f64) = net
        .loads
        .iter()
        .filter(|l| !l.in_service)
        .fold((0.0, 0.0), |acc, l| (acc.0 + l.p, acc.1 + l.q));
    let idle_loads = net.loads.iter().filter(|l| !l.in_service).count();
    if idle_loads > 0 {
        warnings.push(
            &F.record_dropped,
            format!(
                "{idle_loads} out of service load(s) dropped, {:.4} MW and {:.4} MVAr: a MATPOWER \
             bus row states one demand with no status, so an idle load would read back as live",
                idle_load.0, idle_load.1
            ),
        );
    }
    let idle_shunts = net.shunts.iter().filter(|s| !s.in_service).count();
    if idle_shunts > 0 {
        warnings.push(&F.record_dropped, format!(
            "{idle_shunts} out of service shunt(s) dropped: a MATPOWER bus row states one shunt \
             with no status"
        ));
    }
    warn_extra_branch_rating_sets(&F, "MATPOWER .m", net, &mut warnings);
    let branch_solutions = net.branches.iter().filter(|b| b.solution.is_some()).count();
    if branch_solutions > 0 {
        warnings.push(&F.field_dropped, format!(
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
        warnings.push(&F.field_dropped, format!(
            "{voltage_loads} voltage dependent load model(s) dropped: MATPOWER carries only static Pd/Qd"
        ));
    }
    // A network with no costs at all writes no `mpc.gencost` and loses nothing
    // — absence is the source's own shape, not a drop, so it earns no warning
    // (`MissingGenCostPolicy::zero` synthesizes costs for callers who need
    // them). Only a partial set is a real loss: the block is all-or-nothing.
    let with_cost = net.generators.iter().filter(|g| g.cost.is_some()).count();
    if with_cost > 0 && with_cost < net.generators.len() {
        warnings.push(&F.field_dropped, format!(
            "gen cost dropped: {with_cost} of {} generators carry cost data, but MATPOWER's `mpc.gencost` block is all-or-nothing",
            net.generators.len()
        ));
    }
    // The canonical writer emits named columns only and replays no extras key,
    // so the predicate is always false and the count is every element carrying
    // one. This is the same helper the PSS/E, PSLF and PowerWorld writers use;
    // the hand-rolled version here reported a bare yes/no.
    crate::format::warn_dropped_extras(&F, "canonical MATPOWER .m", net, |_| false, &mut warnings);
    // `mpc.areas` carries the area number and reference bus and nothing else.
    let lossy_areas = net
        .areas
        .iter()
        .filter(|a| a.name.is_some() || a.net_interchange != 0.0 || a.tolerance != 0.0)
        .count();
    if lossy_areas > 0 {
        warnings.push(&F.field_dropped, format!(
            "{lossy_areas} of {} area record(s) carry a name or interchange data: `mpc.areas` holds only the area number and reference bus",
            net.areas.len()
        ));
    }
    warnings
}

/// Canonical MATPOWER from the neutral model, for networks with no MATPOWER
/// source. Loads and shunts are summed back onto their bus (MATPOWER carries one
/// of each per bus). Emits valid `.m` (values equal, formatting normalized); not
/// byte-exact. HVDC lines ride `mpc.dcline`/`mpc.dclinecost`, the same blocks
/// the reader reads.
#[allow(clippy::too_many_lines)] // flat per-section serializer; splitting adds noise
fn canonical(net: &BalancedNetwork) -> String {
    // Aggregate demand and shunts onto their bus. MATPOWER's bus row states
    // one demand and one shunt per bus with no status of their own, so an out
    // of service element cannot be written as anything but absent — folding
    // its value in would state it as live load. `canonical_warnings` reports
    // what that leaves out.
    let mut demand: BTreeMap<BusId, (f64, f64)> = BTreeMap::new();
    for l in net.loads.iter().filter(|l| l.in_service) {
        let e = demand.entry(l.bus).or_default();
        e.0 += l.p;
        e.1 += l.q;
    }
    let mut shunt: BTreeMap<BusId, (f64, f64)> = BTreeMap::new();
    for s in net.shunts.iter().filter(|s| s.in_service) {
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

    // Bus names ride the same `mpc.bus_name` cell array the reader reads,
    // one quoted entry per bus in bus order (the reader attaches by position
    // and requires a full set). An unnamed bus writes the empty string, which
    // reads back as unnamed.
    if net.buses.iter().any(|b| b.name.is_some()) {
        let _ = writeln!(s, "mpc.bus_name = {{");
        for b in &net.buses {
            let name = matlab_string(b.name.as_deref().unwrap_or(""));
            let _ = writeln!(s, "\t'{name}';");
        }
        let _ = writeln!(s, "}};");
    }

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

    if !net.areas.is_empty() {
        // `mpc.areas` is `[area, refbus]` and the reader reads it back. An
        // area with no reference bus writes 0, which reads back as none.
        let _ = writeln!(s, "mpc.areas = [");
        for a in &net.areas {
            let _ = writeln!(s, "\t{}\t{};", a.number, a.slack_bus.map_or(0, |b| b.0));
        }
        let _ = writeln!(s, "];");
    }

    if !net.generators.is_empty() {
        // The 21-column layout (Pc1..APF) is standard MATPOWER and the reader
        // reads it; emit it whenever any generator carries capability/ramp
        // columns, padding absent slots with 0 (MATPOWER's own "unspecified")
        // to keep the matrix rectangular.
        let with_caps = net.generators.iter().any(Generator::has_caps);
        let _ = writeln!(s, "mpc.gen = [");
        for g in &net.generators {
            let _ = write!(
                s,
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
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
            if with_caps {
                for slot in &g.caps {
                    let _ = write!(s, "\t{}", slot.unwrap_or(0.0));
                }
            }
            let _ = writeln!(s, ";");
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

    if !net.hvdc.is_empty() {
        let _ = writeln!(s, "mpc.dcline = [");
        for d in &net.hvdc {
            let _ = writeln!(
                s,
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{};",
                d.from,
                d.to,
                f64::from(d.in_service),
                d.pf,
                d.pt,
                d.qf,
                d.qt,
                d.vf,
                d.vt,
                d.pmin,
                d.pmax,
                d.qminf,
                d.qmaxf,
                d.qmint,
                d.qmaxt,
                d.loss0,
                d.loss1
            );
        }
        let _ = writeln!(s, "];");

        // `mpc.dclinecost` must cover every dcline when present, and a line
        // with no usage cost takes the all-zero polynomial row `toggle_dcline`
        // itself pads with — zero cost and no cost term price a line the same
        // — so unlike `mpc.gencost` this block is never all-or-nothing.
        if net.hvdc.iter().any(|d| d.cost.is_some()) {
            let _ = writeln!(s, "mpc.dclinecost = [");
            let width = net
                .hvdc
                .iter()
                .filter_map(|d| d.cost.as_ref())
                .map(|c| c.coeffs.len())
                .max()
                .unwrap_or(0)
                .max(2);
            let zero = GenCost {
                model: 2,
                startup: 0.0,
                shutdown: 0.0,
                ncost: 2,
                coeffs: Vec::new(),
            };
            for d in &net.hvdc {
                let c = d.cost.as_ref().unwrap_or(&zero);
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
