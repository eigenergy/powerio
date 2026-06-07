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

use crate::network::{Network, SourceFormat};

/// Serialize `net` to MATPOWER `.m` text. Echoes the retained source verbatim
/// when `net` came from MATPOWER; otherwise emits canonical `.m`.
#[must_use]
pub fn write_matpower(net: &Network) -> String {
    match &net.source {
        Some(text) if net.source_format == SourceFormat::Matpower => text.to_string(),
        _ => canonical(net),
    }
}

/// Canonical MATPOWER from the neutral model, for networks with no MATPOWER
/// source. Loads and shunts are summed back onto their bus (MATPOWER carries one
/// of each per bus). Emits valid `.m` (values equal, formatting normalized); not
/// byte-exact. HVDC lines are not emitted.
#[allow(clippy::too_many_lines)] // flat per-section serializer; splitting adds noise
fn canonical(net: &Network) -> String {
    // Aggregate demand and shunts onto their bus.
    let mut demand: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
    for l in &net.loads {
        let e = demand.entry(l.bus).or_default();
        e.0 += l.p;
        e.1 += l.q;
    }
    let mut shunt: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
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
            br.b,
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
            for g in &net.generators {
                let c = g.cost.as_ref().expect("checked all gens have cost");
                let _ = write!(
                    s,
                    "\t{}\t{}\t{}\t{}",
                    c.model, c.startup, c.shutdown, c.ncost
                );
                for coeff in &c.coeffs {
                    let _ = write!(s, "\t{coeff}");
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
