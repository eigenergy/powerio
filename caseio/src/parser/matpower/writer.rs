//! Write a [`MpcCase`] back out as a MATPOWER `.m` file.
//!
//! When the case was parsed from text it carries its original source, and the
//! writer echoes it verbatim — an exact round-trip that preserves every field,
//! comment, and numeric token. A case built in memory (e.g. by `synth`) has no
//! source, so the writer falls back to canonical serialization from the typed
//! data.

use std::fmt::Write as _;
use std::path::Path;

use crate::case::MpcCase;
use crate::Result;

/// Serialize `case` to MATPOWER `.m` text.
#[must_use]
pub fn write_matpower(case: &MpcCase) -> String {
    match case.source() {
        Some(text) => text.to_owned(),
        None => canonical(case),
    }
}

/// Write `case` to `path` as MATPOWER `.m`.
pub fn write_matpower_file(case: &MpcCase, path: impl AsRef<Path>) -> Result<()> {
    std::fs::write(path, write_matpower(case))?;
    Ok(())
}

/// Canonical MATPOWER from typed data, for cases with no retained source text.
/// Emits valid `.m` (values equal, formatting normalized); not byte-exact.
#[allow(clippy::too_many_lines)] // flat per-section serializer; splitting adds noise
fn canonical(case: &MpcCase) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "function mpc = {}", matlab_ident(&case.name));
    let _ = writeln!(s, "mpc.version = '2';");
    let _ = writeln!(s, "mpc.baseMVA = {};", case.base_mva);

    let _ = writeln!(s, "mpc.bus = [");
    for b in &case.buses {
        let _ = writeln!(
            s,
            "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{};",
            b.id,
            b.kind as u8,
            b.pd,
            b.qd,
            b.gs,
            b.bs,
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
    for br in &case.branches {
        let _ = writeln!(
            s,
            "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{};",
            br.from_id,
            br.to_id,
            br.r,
            br.x,
            br.b,
            br.rate_a,
            br.rate_b,
            br.rate_c,
            br.tap,
            br.shift,
            br.status,
            br.angmin,
            br.angmax
        );
    }
    let _ = writeln!(s, "];");

    if !case.gens.is_empty() {
        let _ = writeln!(s, "mpc.gen = [");
        for g in &case.gens {
            let _ = writeln!(
                s,
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{};",
                g.bus_id, g.pg, g.qg, g.qmax, g.qmin, g.vg, g.mbase, g.status, g.pmax, g.pmin
            );
        }
        let _ = writeln!(s, "];");

        if case.gens.iter().all(|g| g.cost.is_some()) {
            let _ = writeln!(s, "mpc.gencost = [");
            for g in &case.gens {
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

    if !case.storage.is_empty() {
        let _ = writeln!(s, "mpc.storage = [");
        for st in &case.storage {
            let _ = writeln!(
                s,
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{};",
                st.bus_id,
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
                st.status
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
