//! Parse PSS/E `.raw` revisions 32 through 35 and emit revisions 33 through 35
//! (see [`write_psse_rev`]).
//!
//! Covers the core sections — bus, load, fixed shunt, generator, branch, and the
//! 2- and 3-winding transformer records — which together carry a transmission
//! power flow case. Revision 32 records end before the bus voltage limits
//! (`NVHI`, `NVLO`, `EVHI`, `EVLO`), the load `INTRPT` field, the transformer
//! `VECGRP` field, and the winding `CNXA` field that revision 33 added; the
//! reader keys each layout off the header revision and a revision 32 source
//! is always written fresh at revision 33 or later.
//! A switched shunt keeps its steady-state susceptance `BINIT`
//! as the shunt `b` and carries its mode, voltage band, regulated bus, RMPCT, and
//! step blocks on [`SwitchedShuntControl`]. Transformer impedance and winding
//! bases (`CZ`/`CW`) are normalized to the system base and per unit tap ratios;
//! the serializer emits the canonical `CZ = 1`, `CW = 1` form.
//! Two-terminal DC lines parse and emit as the neutral
//! [`Hvdc`] (power-setpoint model; converter firing-angle/transformer detail
//! rides through in extras). The other advanced sections (VSC and multi-terminal
//! DC, FACTS, GNE) are not modeled: during emission they become empty sections,
//! on read they're skipped, and storage carried on the `BalancedNetwork` is reported as
//! dropped. Same format emission is byte exact through the retained source (see
//! [`crate::emit`]); this serializer is the cross format path.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde_json::Value;

use super::{
    TextEmission, branch_rating_set_drop_warning, jnum, sanitize_quoted,
    warn_extra_branch_rating_sets,
};
use std::borrow::Cow;

use crate::diagnostics::codes::EMIT_PSSE as F;
use crate::diagnostics::{Diagnostics, codes};
use crate::network::{
    Area, BalancedNetwork, BalancedNetworkTables, Branch, BranchCharging, BranchRatingSet, Bus,
    BusId, BusType, ComponentMetadata, DetailedConnectivity, Extras, Generator,
    GeneratorEnergySource, Hvdc, Impedance, Load, LoadVoltageModel, Shunt, ShuntBlock,
    SolverParams, SourceFormat, Switch, SwitchedShuntControl, SwitchedShuntMode, TerminalReference,
    Transformer3W, TransformerControl, TransformerControlMode, Winding,
};
use crate::{Error, Result};

const FMT: &str = "PSS/E .raw";
#[cfg(test)]
const REV: u32 = 33;
const PSSE_EXTRA_BRANCH_RATINGS: usize = 9;

fn psse_extra_rating_name(slot: usize) -> String {
    format!("RATE{}", slot + 4)
}

fn psse_extra_rating_slot(name: &str) -> Option<usize> {
    let upper = name.trim().to_ascii_uppercase();
    let suffix = upper
        .strip_prefix("RATE")
        .or_else(|| upper.strip_prefix("RATING"))?
        .trim_start_matches([' ', '_']);
    let n = suffix.parse::<usize>().ok()?;
    (4..=12).contains(&n).then_some(n - 4)
}

fn read_extra_branch_ratings(
    fields: &[Cow<'_, str>],
    rating_start: usize,
    named_record: bool,
) -> Result<Vec<BranchRatingSet>> {
    if !named_record {
        return Ok(Vec::new());
    }
    let mut ratings = Vec::new();
    for slot in 0..PSSE_EXTRA_BRANCH_RATINGS {
        let rate_mva = num_at(fields, rating_start + 3 + slot, 0.0)?;
        if rate_mva.abs() > f64::EPSILON {
            ratings.push(BranchRatingSet::new(psse_extra_rating_name(slot), rate_mva));
        }
    }
    Ok(ratings)
}

fn psse_extra_rating_values(
    branch: &Branch,
    branch_index: usize,
    warnings: &mut Diagnostics,
) -> [f64; PSSE_EXTRA_BRANCH_RATINGS] {
    let mut values = [0.0; PSSE_EXTRA_BRANCH_RATINGS];
    let mut used = [false; PSSE_EXTRA_BRANCH_RATINGS];
    let mut deferred = Vec::new();

    for rating in &branch.rating_sets {
        if let Some(slot) = psse_extra_rating_slot(&rating.name) {
            if !used[slot] {
                values[slot] = rating.rate_mva;
                used[slot] = true;
                continue;
            }
        }
        deferred.push(rating);
    }

    for rating in deferred {
        if let Some(slot) = used.iter().position(|is_used| !*is_used) {
            values[slot] = rating.rate_mva;
            used[slot] = true;
            warnings.push(
                &codes::EMIT_PSSE_RATING_SET_REMAPPED,
                branch_rating_set_rename_warning(
                    branch_index,
                    branch,
                    rating,
                    &psse_extra_rating_name(slot),
                ),
            );
        } else {
            warnings.push(
                &F.rating_set_dropped,
                branch_rating_set_drop_warning("PSS/E v34/v35", branch_index, branch, rating),
            );
        }
    }

    values
}

fn branch_rating_set_rename_warning(
    branch_index: usize,
    branch: &Branch,
    rating: &BranchRatingSet,
    emitted_name: &str,
) -> String {
    format!(
        "branch {} ({} to {}) rating set {}={} MVA emitted as {} in PSS/E v34/v35; rating set names outside RATE4-RATE12 are not preserved",
        branch_index + 1,
        branch.from,
        branch.to,
        rating.name,
        rating.rate_mva,
        emitted_name
    )
}

fn warn_psse_extra_branch_ratings_dropped(net: &BalancedNetwork, warnings: &mut Diagnostics) {
    warn_extra_branch_rating_sets(&F, "PSS/E v33", net, warnings);
}

fn warn_generator_energy_sources_dropped(net: &BalancedNetwork, warnings: &mut Diagnostics) {
    let mut counts = [0_usize; 5];
    for generator in net.generators() {
        let index = match generator.energy_source {
            GeneratorEnergySource::Hydro => Some(0),
            GeneratorEnergySource::Nuclear => Some(1),
            GeneratorEnergySource::Wind => Some(2),
            GeneratorEnergySource::Thermal => Some(3),
            GeneratorEnergySource::Solar => Some(4),
            GeneratorEnergySource::Other => None,
        };
        if let Some(index) = index {
            counts[index] += 1;
        }
    }
    let total = counts.iter().sum::<usize>();
    if total == 0 {
        return;
    }
    let summary = ["hydro", "nuclear", "wind", "thermal", "solar"]
        .into_iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .map(|(source, count)| format!("{source}={count}"))
        .collect::<Vec<_>>()
        .join(", ");
    warnings.push(
        &F.field_dropped,
        format!(
            "{total} generator energy source value(s) dropped ({summary}): PSS/E RAW and RAWX generator records have no energy source field"
        ),
    );
}

/// Characters that would corrupt a single-quoted PSS/E name field. The quote
/// toggles the reader's quoted state early, and `/` truncates the record at the
/// inline-comment delimiter (a PSS/E record splits on `/` before tokenizing).
const NAME_FORBIDDEN: &[char] = &['\'', '/'];

fn write_system_switch(
    out: &mut String,
    switch: &Switch,
    ckt: &str,
    num: &mut impl FnMut(f64) -> String,
    sanitized_quoted: &mut usize,
) {
    let raw_name = switch
        .extras
        .get("psse_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
    *sanitized_quoted += usize::from(matches!(name, std::borrow::Cow::Owned(_)));

    let mut record = vec![
        switch.from.to_string(),
        switch.to.to_string(),
        format!("'{ckt}'"),
        num(extra_f64(&switch.extras, "psse_xpu").unwrap_or(0.0)),
        num(switch.thermal_rating.unwrap_or(0.0)),
    ];
    record.extend((2..=12).map(|rating| {
        num(extra_f64(&switch.extras, &format!("psse_rate{rating}")).unwrap_or(0.0))
    }));
    record.extend([
        i32::from(switch.closed).to_string(),
        extra_i64(&switch.extras, "psse_nstat")
            .unwrap_or(1)
            .to_string(),
        extra_i64(&switch.extras, "psse_met")
            .unwrap_or(1)
            .to_string(),
        extra_i64(&switch.extras, "psse_stype")
            .unwrap_or(1)
            .to_string(),
        format!("'{name}'"),
    ]);
    let _ = writeln!(out, "{}", record.join(", "));
}

// ---- Writer -----------------------------------------------------------------

/// Serialize `net` to PSS/E `.raw` at the default revision (33).
#[must_use]
#[cfg(test)]
fn write_psse(net: &BalancedNetwork) -> TextEmission {
    write_psse_rev(net, REV)
}

/// Serialize `net` to PSS/E `.raw` at `rev` (33, 34, or 35).
///
/// Revisions 34 and 35 add the expanded system-wide header with its
/// end-of-system-wide-data marker, the named 12-rating branch record, the
/// 12-rating transformer winding line (COD at 15, NODE after CONT), and the
/// load distributed-generation / load-type trailing columns; 35 also inserts
/// the generator NREG/BASLOD columns and the switched shunt ID/NREG columns
/// with (S, N, B) step triples. The reader keys each layout off the header
/// revision. Any other `rev` falls back to the 33 layout. Same-format
/// byte exact echo still rides the retained source (see [`crate::emit`]);
/// this serializer is the cross format path.
#[must_use]
pub fn write_psse_rev(net: &BalancedNetwork, rev: u32) -> TextEmission {
    write_psse_rev_inner(net, rev, true)
}

/// The RAWX writer reuses the electrical records but writes detailed
/// connectivity directly into JSON tables. Omitting the nested RAW substation
/// records here prevents duplicate conversion work and duplicate diagnostics.
pub(super) fn write_psse_rev_without_detailed_connectivity(
    net: &BalancedNetwork,
    rev: u32,
) -> TextEmission {
    write_psse_rev_inner(net, rev, false)
}

#[expect(clippy::too_many_lines)]
fn write_psse_rev_inner(
    net: &BalancedNetwork,
    rev: u32,
    include_detailed_connectivity: bool,
) -> TextEmission {
    // v34+ wraps the global parameters in a system-wide data section, names
    // branches and carries 12 ratings, and adds load DG / load-type columns.
    let modern = rev >= 34;
    let mut warnings = Diagnostics::new();
    let mut nonfinite = false;
    let mut sanitized_quoted = 0usize;
    let mut s = String::new();
    // A formatter that records when a value can't be represented (PSS/E is fixed
    // numeric — no Inf/NaN).
    let mut num = |x: f64| -> String {
        if x.is_finite() {
            let s = format!("{x}");
            // PSS/E v33 readers treat a record whose first field is exactly "0" as
            // a section terminator (PowerModels' pti.jl). A transformer impedance
            // line can start with R = 0, so never emit a bare integer "0": give it
            // a decimal, matching PSS/E's own numeric convention.
            if s.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                format!("{s}.0")
            } else {
                s
            }
        } else {
            nonfinite = true;
            let sentinel = if x > 0.0 {
                1.0e10
            } else if x < 0.0 {
                -1.0e10
            } else {
                0.0
            };
            format!("{sentinel}.0")
        }
    };

    // The case name reaches the header line and the title line. Both are
    // single records, so an embedded terminator would make the rest of the
    // name parse as bus data.
    let case_name = sanitize_quoted(net.name(), NAME_FORBIDDEN, ' ');
    let _ = writeln!(
        s,
        "0, {}, {rev}, 0, {}, {}   / powerio export: {}",
        net.base_mva(),
        i32::from(modern),
        num(net.base_frequency()),
        case_name
    );
    let _ = writeln!(s, "{case_name}");
    let _ = writeln!(s);
    if modern {
        // v34+ system-wide block: emit the solver keyword lines (the fields that
        // are set), then close the block.
        if let Some(sp) = &net.solver() {
            if let Some(t) = sp.zero_impedance_threshold {
                let _ = writeln!(s, "GENERAL, THRSHZ={}", num(t));
            }
            let mut newton = Vec::new();
            if let Some(t) = sp.newton_tolerance {
                newton.push(format!("TOLN={}", num(t)));
            }
            if let Some(n) = sp.max_iterations {
                newton.push(format!("ITMXN={n}"));
            }
            if !newton.is_empty() {
                let _ = writeln!(s, "NEWTON, {}", newton.join(", "));
            }
            let flags: Vec<String> = [
                ("ACTAPS", sp.adjust_taps),
                ("AREAIN", sp.adjust_area_interchange),
                ("PHSHFT", sp.adjust_phase_shift),
                ("DCTAPS", sp.adjust_dc_taps),
                ("SWSHNT", sp.adjust_switched_shunt),
            ]
            .into_iter()
            .filter_map(|(name, v)| v.map(|b| format!("{name}={}", i32::from(b))))
            .collect();
            if !flags.is_empty() {
                let _ = writeln!(s, "SOLVER, {}", flags.join(", "));
            }
        }
        let _ = writeln!(s, "0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA");
    }

    // Bus, with area/zone kept for the load records that reference them.
    let mut bus_area: BTreeMap<BusId, (usize, usize)> = BTreeMap::new();
    for b in net.buses() {
        bus_area.insert(b.id, (b.area, b.zone));
        let owner = extra_i64(&b.extras, "psse_owner").unwrap_or(1);
        let raw_name = b.name.as_deref().unwrap_or("");
        let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
        if matches!(name, std::borrow::Cow::Owned(_)) {
            sanitized_quoted += 1;
        }
        // The last two columns are EVHI/EVLO; emit the emergency band when set,
        // else echo the normal band.
        let _ = writeln!(
            s,
            "{}, '{:<12}', {}, {}, {}, {}, {owner}, {}, {}, {}, {}, {}, {}",
            b.id,
            name,
            num(b.base_kv),
            ide(b.kind),
            b.area,
            b.zone,
            num(b.vm),
            num(b.va),
            num(b.vmax),
            num(b.vmin),
            num(b.evhi.unwrap_or(b.vmax)),
            num(b.evlo.unwrap_or(b.vmin))
        );
    }
    let _ = writeln!(s, "0 / END OF BUS DATA, BEGIN LOAD DATA");

    // v33 ends the load record at INTRPT; v34 adds PDGEN/QDGEN/STDG and v35 a
    // LOADTYPE string. PSS/E-sourced rows replay these from extras; other
    // sources get the documented defaults.
    // Per-bus circuit-id counters so parallel devices on a bus get distinct ids
    // (PSS/E requires (bus, id) to be unique); a captured `extras["id"]` wins.
    let mut load_ids: BTreeMap<BusId, BTreeSet<String>> = BTreeMap::new();
    for l in net.loads() {
        let (bus_area, bus_zone) = bus_area.get(&l.bus).copied().unwrap_or((1, 1));
        let area = extra_i64(&l.extras, "psse_area")
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(bus_area);
        let zone = extra_i64(&l.extras, "psse_zone")
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(bus_zone);
        let id = quoted_device_id(&l.extras, l.bus, &mut load_ids, &mut sanitized_quoted);
        let (pl, ql, ip, iq, yp, yq) = load_components_for_write(l, &id, &mut warnings);
        let owner = extra_i64(&l.extras, "psse_owner").unwrap_or(1);
        let scal = typed_psse_scal(l, &id, &mut warnings)
            .or_else(|| extra_i64(&l.extras, "psse_scal"))
            .unwrap_or(1);
        let intrpt = extra_i64(&l.extras, "psse_intrpt").unwrap_or(0);
        let typed_load_type = l.voltage_model.as_ref().and_then(typed_psse_load_type);
        if rev < 35 && typed_load_type.is_some() {
            warnings.push(
                &F.record_dropped,
                format!(
                    "PSS/E load at bus {} id {id:?}: load type requires revision 35; dropped",
                    l.bus
                ),
            );
        }
        let modern_tail = if rev >= 35 {
            let pdgen = extra_f64(&l.extras, "psse_pdgen").unwrap_or(0.0);
            let qdgen = extra_f64(&l.extras, "psse_qdgen").unwrap_or(0.0);
            let flagstatus = extra_i64(&l.extras, "psse_flagstatus").unwrap_or(0);
            let raw_loadtype = typed_load_type.or_else(|| {
                l.extras
                    .get("psse_loadtype")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            let loadtype =
                sanitize_quoted(raw_loadtype.as_deref().unwrap_or(""), NAME_FORBIDDEN, ' ');
            if matches!(loadtype, std::borrow::Cow::Owned(_)) {
                sanitized_quoted += 1;
            }
            format!(
                ", {}, {}, {flagstatus}, '{loadtype}'",
                num(pdgen),
                num(qdgen)
            )
        } else if modern {
            let pdgen = extra_f64(&l.extras, "psse_pdgen").unwrap_or(0.0);
            let qdgen = extra_f64(&l.extras, "psse_qdgen").unwrap_or(0.0);
            let flagstatus = extra_i64(&l.extras, "psse_flagstatus").unwrap_or(0);
            format!(", {}, {}, {flagstatus}", num(pdgen), num(qdgen))
        } else {
            String::new()
        };
        let _ = writeln!(
            s,
            "{}, '{id}', {}, {}, {}, {}, {}, {}, {}, {}, {}, {owner}, {scal}, {intrpt}{modern_tail}",
            l.bus,
            i32::from(l.in_service),
            area,
            zone,
            num(pl),
            num(ql),
            num(ip),
            num(iq),
            num(yp),
            num(yq)
        );
    }
    let _ = writeln!(s, "0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA");

    // Fixed shunts here; switched shunts (control = Some) go in their own section.
    let mut shunt_ids: BTreeMap<BusId, BTreeSet<String>> = BTreeMap::new();
    for sh in net.shunts().iter().filter(|s| s.control.is_none()) {
        let id = quoted_device_id(&sh.extras, sh.bus, &mut shunt_ids, &mut sanitized_quoted);
        let _ = writeln!(
            s,
            "{}, '{id}', {}, {}, {}",
            sh.bus,
            i32::from(sh.in_service),
            num(sh.g),
            num(sh.b)
        );
    }
    let _ = writeln!(s, "0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA");

    let mut gen_ids: BTreeMap<BusId, BTreeSet<String>> = BTreeMap::new();
    for g in net.generators() {
        let preferred_id =
            detailed_source_property(net, "generator", g.uid.as_deref(), "psse_eqid")
                .filter(|id| !id.is_empty());
        let id = quoted_circuit_id(preferred_id, g.bus, &mut gen_ids, &mut sanitized_quoted);
        // IREG/NREG identify the exact regulated bus and node when detailed
        // connectivity is available.
        let (ireg, regulated_node) = regulating_target(
            net,
            g.regulating_terminal.as_ref(),
            g.regulated_bus,
            format_args!("generator at bus {}", g.bus),
            &mut warnings,
        );
        if rev < 35 && regulated_node != 0 {
            warnings.push(
                &F.field_dropped,
                format!(
                    "PSS/E generator at bus {} id {id:?}: regulating node {regulated_node} has no NREG field before revision 35; emitted only IREG",
                    g.bus
                ),
            );
        }

        let source_float = |property, default, warnings: &mut Diagnostics| {
            generator_source_float(net, g, property, default, warnings)
        };
        let source_integer = |property, default, warnings: &mut Diagnostics| {
            generator_source_integer(net, g, property, default, warnings)
        };
        let zr = source_float("psse_zr", 0.0, &mut warnings);
        let zx = source_float("psse_zx", 1.0, &mut warnings);
        let rt = source_float("psse_rt", 0.0, &mut warnings);
        let xt = source_float("psse_xt", 0.0, &mut warnings);
        let gtap = source_float("psse_gtap", 1.0, &mut warnings);
        let rmpct = source_float("psse_rmpct", 100.0, &mut warnings);
        let baslod = source_integer("psse_baslod", 0, &mut warnings);
        let owners = [
            (
                source_integer("psse_o1", 1, &mut warnings),
                source_float("psse_f1", 1.0, &mut warnings),
            ),
            (
                source_integer("psse_o2", 0, &mut warnings),
                source_float("psse_f2", 1.0, &mut warnings),
            ),
            (
                source_integer("psse_o3", 0, &mut warnings),
                source_float("psse_f3", 1.0, &mut warnings),
            ),
            (
                source_integer("psse_o4", 0, &mut warnings),
                source_float("psse_f4", 1.0, &mut warnings),
            ),
        ];
        let wmod = source_integer("psse_wmod", 0, &mut warnings);
        let wpf = source_float("psse_wpf", 1.0, &mut warnings);
        if rev < 35 && baslod != 0 {
            warnings.push(
                &F.field_dropped,
                format!(
                    "PSS/E generator at bus {} id {id:?}: BASLOD {baslod} has no field before revision 35; dropped",
                    g.bus
                ),
            );
        }

        let mut record = vec![
            g.bus.to_string(),
            format!("'{id}'"),
            num(g.pg),
            num(g.qg),
            num(g.qmax),
            num(g.qmin),
            num(g.vg),
            ireg.to_string(),
        ];
        if rev >= 35 {
            record.push(regulated_node.to_string());
        }
        record.extend([
            num(g.mbase),
            num(zr),
            num(zx),
            num(rt),
            num(xt),
            num(gtap),
            i32::from(g.in_service).to_string(),
            num(rmpct),
            num(g.pmax),
            num(g.pmin),
        ]);
        if rev >= 35 {
            record.push(baslod.to_string());
        }
        for (owner, fraction) in owners {
            record.push(owner.to_string());
            record.push(num(fraction));
        }
        record.push(wmod.to_string());
        record.push(num(wpf));
        let _ = writeln!(s, "{}", record.join(", "));
    }
    let _ = writeln!(s, "0 / END OF GENERATOR DATA, BEGIN BRANCH DATA");

    // Non-transformer branches here; transformers go in their own section.
    // Parallel branches between the same bus pair get distinct circuit ids (PSS/E
    // keys a branch on (I, J, CKT)); a captured source CKT wins.
    let mut branch_ids: BTreeMap<(BusId, BusId), BTreeSet<String>> = BTreeMap::new();
    let mut transformer_ids: BTreeMap<(BusId, BusId), BTreeSet<String>> = BTreeMap::new();
    for (branch_index, br) in net
        .branches()
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.is_transformer())
    {
        let ckt = quoted_circuit_id(
            br.extras.get("id").and_then(Value::as_str),
            (br.from, br.to),
            &mut branch_ids,
            &mut sanitized_quoted,
        );
        let charging = br.calc_terminal_charging();
        let b_total = charging.calc_total_b();
        let b_mid = b_total / 2.0;
        let bi = charging.b_fr - b_mid;
        let bj = charging.b_to - b_mid;
        let met = extra_i64(&br.extras, "psse_met").unwrap_or(1);
        let len = extra_f64(&br.extras, "psse_len").unwrap_or(0.0);
        let owners = psse_ownership(&br.extras);
        if modern {
            let raw_name = br.name.as_deref().unwrap_or("");
            let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
            if matches!(name, std::borrow::Cow::Owned(_)) {
                sanitized_quoted += 1;
            }
            // v34+: a quoted line NAME at field 6, then twelve rating columns,
            // pushing STAT to field 23 (the layout the reader expects at rev>=34).
            // RATE4-RATE12 come from extra branch rating sets when present.
            let extra_ratings = psse_extra_rating_values(br, branch_index, &mut warnings);
            let _ = writeln!(
                s,
                "{}, {}, '{ckt}', {}, {}, {}, '{:<12}', {}, {}, {}, \
                 {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {met}, {}, \
                 {}, {}, {}, {}, {}, {}, {}, {}",
                br.from,
                br.to,
                num(br.r),
                num(br.x),
                num(b_total),
                name,
                num(br.rate_a),
                num(br.rate_b),
                num(br.rate_c),
                num(extra_ratings[0]),
                num(extra_ratings[1]),
                num(extra_ratings[2]),
                num(extra_ratings[3]),
                num(extra_ratings[4]),
                num(extra_ratings[5]),
                num(extra_ratings[6]),
                num(extra_ratings[7]),
                num(extra_ratings[8]),
                num(charging.g_fr),
                num(bi),
                num(charging.g_to),
                num(bj),
                i32::from(br.in_service),
                num(len),
                owners[0].0,
                num(owners[0].1),
                owners[1].0,
                num(owners[1].1),
                owners[2].0,
                num(owners[2].1),
                owners[3].0,
                num(owners[3].1)
            );
        } else {
            let _ = writeln!(
                s,
                "{}, {}, '{ckt}', {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {met}, {}, \
                 {}, {}, {}, {}, {}, {}, {}, {}",
                br.from,
                br.to,
                num(br.r),
                num(br.x),
                num(b_total),
                num(br.rate_a),
                num(br.rate_b),
                num(br.rate_c),
                num(charging.g_fr),
                num(bi),
                num(charging.g_to),
                num(bj),
                i32::from(br.in_service),
                num(len),
                owners[0].0,
                num(owners[0].1),
                owners[1].0,
                num(owners[1].1),
                owners[2].0,
                num(owners[2].1),
                owners[3].0,
                num(owners[3].1)
            );
        }
    }
    if rev >= 35 {
        // Revision 35 inserts system switching device data between branch and
        // transformer data.
        let _ = writeln!(
            s,
            "0 / END OF BRANCH DATA, BEGIN SYSTEM SWITCHING DEVICE DATA"
        );
        if include_detailed_connectivity {
            let mut switch_ids = BTreeMap::new();
            for switch in net.switches() {
                let ckt = quoted_circuit_id(
                    switch.extras.get("psse_ckt").and_then(Value::as_str),
                    (switch.from, switch.to),
                    &mut switch_ids,
                    &mut sanitized_quoted,
                );
                write_system_switch(&mut s, switch, &ckt, &mut num, &mut sanitized_quoted);
            }
        }
        let _ = writeln!(
            s,
            "0 / END OF SYSTEM SWITCHING DEVICE DATA, BEGIN TRANSFORMER DATA"
        );
    } else {
        let _ = writeln!(s, "0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA");
        if include_detailed_connectivity && !net.switches().is_empty() {
            warnings.push(
                &F.record_dropped,
                format!(
                    "{} system switching device record(s) dropped: PSS/E revision {rev} has no system switching device section",
                    net.switches().len()
                ),
            );
        }
    }

    for (branch_index, br) in net
        .branches()
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_transformer())
    {
        let source_id = br
            .extras
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| detailed_source_id(net, "transformer", br.uid.as_deref()));
        let transformer_id = quoted_circuit_id(
            source_id,
            (br.from, br.to),
            &mut transformer_ids,
            &mut sanitized_quoted,
        );
        // 2-winding, 4-line record. CW=1 (turns ratio p.u.), CZ=1 (Z on system
        // base). Record 1 carries the full owner block (O1..O4,F1..F4) and the
        // VECGRP string: PSS/E v33 readers count a 2-winding transformer as a
        // fixed 43-field record (21 + 3 + 17 + 2), so the owner padding matters.
        // MAG1/MAG2 = the branch charging projected to one magnetizing
        // admittance (CM = 1, so p.u. on the system base); a 2-winding
        // transformer that carries line charging keeps the total.
        let charging = br.calc_terminal_charging();
        let raw_name = br.name.as_deref().unwrap_or("");
        let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
        if matches!(name, std::borrow::Cow::Owned(_)) {
            sanitized_quoted += 1;
        }
        let raw_vecgrp = br
            .extras
            .get("psse_vecgrp")
            .and_then(Value::as_str)
            .unwrap_or("");
        let vecgrp = sanitize_quoted(raw_vecgrp, NAME_FORBIDDEN, ' ');
        if matches!(vecgrp, std::borrow::Cow::Owned(_)) {
            sanitized_quoted += 1;
        }
        let nmetr = extra_i64(&br.extras, "psse_nmetr").unwrap_or(2);
        let owners = psse_ownership(&br.extras);
        let zcod = extra_i64(&br.extras, "psse_zcod").unwrap_or(0);
        if rev < 35 && zcod != 0 {
            warnings.push(
                &F.field_dropped,
                format!(
                    "PSS/E transformer {}-{} ZCOD {zcod} dropped: the field requires revision 35",
                    br.from, br.to
                ),
            );
        }
        let mut main = vec![
            br.from.to_string(),
            br.to.to_string(),
            "0".to_owned(),
            format!("'{transformer_id}'"),
            "1".to_owned(),
            "1".to_owned(),
            "1".to_owned(),
            num(charging.calc_total_g()),
            num(charging.calc_total_b()),
            nmetr.to_string(),
            format!("'{name:<12}'"),
            i32::from(br.in_service).to_string(),
        ];
        for (owner, fraction) in owners {
            main.push(owner.to_string());
            main.push(num(fraction));
        }
        main.push(format!("'{vecgrp:<12}'"));
        if rev >= 35 {
            main.push(zcod.to_string());
        }
        let _ = writeln!(s, "{}", main.join(", "));
        // Winding-1 control columns (COD, CONT, RMA/RMI, VMA/VMI, NTP) come from
        // the regulating-control data when present, else the fixed defaults.
        let ctl = br.control.as_ref();
        let sbase = ctl
            .filter(|c| c.mva_base > 0.0)
            .map_or(net.base_mva(), |c| c.mva_base);
        let cod = ctl.map_or(0, |c| {
            let code = mode_to_cod(c.mode);
            if c.enabled { code } else { -code }
        });
        let (cont, node) = ctl.map_or((0, 0), |control| {
            regulating_target(
                net,
                control.regulating_terminal.as_ref(),
                control.controlled_bus,
                format_args!("transformer {}-{}", br.from, br.to),
                &mut warnings,
            )
        });
        let cont = ctl.map_or(i64::try_from(cont).unwrap_or(i64::MAX), |control| {
            emit_controlled_bus(
                control,
                cont,
                format_args!("transformer {}-{}", br.from, br.to),
                &mut warnings,
            )
        });
        let (rma, rmi, vma, vmi, ntp) = ctl.map_or((1.1, 0.9, 1.1, 0.9, 33), |c| {
            (c.tap_max, c.tap_min, c.band_max, c.band_min, c.ntp)
        });
        let cnxa = ctl
            .and_then(|control| control.winding_connection_angle)
            .unwrap_or(0.0);
        let tab = extra_i64(&br.extras, "psse_tab").unwrap_or(0);
        let cr = extra_f64(&br.extras, "psse_cr").unwrap_or(0.0);
        let cx = extra_f64(&br.extras, "psse_cx").unwrap_or(0.0);
        let _ = writeln!(s, "{}, {}, {}", num(br.r), num(br.x), num(sbase));
        if modern {
            // v34+ winding line: twelve ratings (RATE4-RATE12 from extra rating
            // sets), then COD, CONT, NODE, RMA, RMI, VMA, VMI, NTP, TAB, CR,
            // CX, CNXA — COD at 15, matching the reader.
            let extra_ratings = psse_extra_rating_values(br, branch_index, &mut warnings);
            let _ = writeln!(
                s,
                "{}, 0, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, \
                 {cod}, {cont}, {node}, {}, {}, {}, {}, {ntp}, {tab}, {}, {}, {}",
                num(br.calc_effective_tap()),
                num(br.shift),
                num(br.rate_a),
                num(br.rate_b),
                num(br.rate_c),
                num(extra_ratings[0]),
                num(extra_ratings[1]),
                num(extra_ratings[2]),
                num(extra_ratings[3]),
                num(extra_ratings[4]),
                num(extra_ratings[5]),
                num(extra_ratings[6]),
                num(extra_ratings[7]),
                num(extra_ratings[8]),
                num(rma),
                num(rmi),
                num(vma),
                num(vmi),
                num(cr),
                num(cx),
                num(cnxa)
            );
        } else {
            let _ = writeln!(
                s,
                "{}, 0, {}, {}, {}, {}, {cod}, {cont}, {}, {}, {}, {}, {ntp}, {tab}, {}, {}, {}",
                num(br.calc_effective_tap()),
                num(br.shift),
                num(br.rate_a),
                num(br.rate_b),
                num(br.rate_c),
                num(rma),
                num(rmi),
                num(vma),
                num(vmi),
                num(cr),
                num(cx),
                num(cnxa)
            );
        }
        let _ = writeln!(s, "1.0, 0");
    }

    // 3-winding transformers: a 5-line record. CW=1, CZ=1, CM=1 (same conventions
    // as the 2-winding record); line 2 carries the three pairwise impedances and
    // the star-point voltage, lines 3-5 the per-winding tap/angle/ratings.
    let mut transformer_3w_ids: BTreeMap<(BusId, BusId, BusId), u32> = BTreeMap::new();
    for t in net.transformers_3w() {
        let buses = (t.windings[0].bus, t.windings[1].bus, t.windings[2].bus);
        let next_id = transformer_3w_ids.entry(buses).or_default();
        *next_id += 1;
        let positional = next_id.to_string();
        let raw_id = t
            .extras
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| detailed_source_id(net, "transformer", t.uid.as_deref()))
            .unwrap_or(positional.as_str());
        let transformer_id = sanitize_quoted(raw_id, NAME_FORBIDDEN, ' ');
        if matches!(transformer_id, std::borrow::Cow::Owned(_)) {
            sanitized_quoted += 1;
        }
        let raw_name = t.name.as_deref().unwrap_or("");
        let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
        if matches!(name, std::borrow::Cow::Owned(_)) {
            sanitized_quoted += 1;
        }
        let raw_vecgrp = t
            .extras
            .get("psse_vecgrp")
            .and_then(Value::as_str)
            .unwrap_or("");
        let vecgrp = sanitize_quoted(raw_vecgrp, NAME_FORBIDDEN, ' ');
        if matches!(vecgrp, std::borrow::Cow::Owned(_)) {
            sanitized_quoted += 1;
        }
        let nmetr = extra_i64(&t.extras, "psse_nmetr").unwrap_or(2);
        let owners = psse_ownership(&t.extras);
        let zcod = extra_i64(&t.extras, "psse_zcod").unwrap_or(0);
        if rev < 35 && zcod != 0 {
            warnings.push(
                &F.field_dropped,
                format!(
                    "PSS/E three winding transformer {}-{}-{} ZCOD {zcod} dropped: the field requires revision 35",
                    t.windings[0].bus, t.windings[1].bus, t.windings[2].bus
                ),
            );
        }
        let mut main = vec![
            t.windings[0].bus.to_string(),
            t.windings[1].bus.to_string(),
            t.windings[2].bus.to_string(),
            format!("'{transformer_id}'"),
            "1".to_owned(),
            "1".to_owned(),
            "1".to_owned(),
            num(t.mag_g),
            num(t.mag_b),
            nmetr.to_string(),
            format!("'{name:<12}'"),
            i32::from(t.in_service).to_string(),
        ];
        for (owner, fraction) in owners {
            main.push(owner.to_string());
            main.push(num(fraction));
        }
        main.push(format!("'{vecgrp:<12}'"));
        if rev >= 35 {
            main.push(zcod.to_string());
        }
        let _ = writeln!(s, "{}", main.join(", "));
        // Line 2: the three pairwise (R, X) on the system base (CZ=1), each with
        // its declared SBASE column, then the star voltage.
        let [z12, z23, z31] = t.z;
        let _ = writeln!(
            s,
            "{}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}",
            num(z12.r),
            num(z12.x),
            num(z12.base_mva),
            num(z23.r),
            num(z23.x),
            num(z23.base_mva),
            num(z31.r),
            num(z31.x),
            num(z31.base_mva),
            num(t.star_vm),
            num(t.star_va)
        );
        for (winding_index, w) in t.windings.iter().enumerate() {
            let control = w.control.as_ref();
            if control.is_some_and(|control| control.mode == TransformerControlMode::DcLineQuantity)
            {
                warnings.push(
                    &F.field_dropped,
                    format!(
                        "PSS/E three winding transformer winding at bus {}: COD 4 DC line quantity control is valid only for two winding transformers; emitted fixed control",
                        w.bus
                    ),
                );
            }
            let control =
                control.filter(|control| control.mode != TransformerControlMode::DcLineQuantity);
            let cod = control.map_or(0, |control| {
                let code = mode_to_cod(control.mode);
                if control.enabled { code } else { -code }
            });
            let (cont, node) = control.map_or((0, 0), |control| {
                regulating_target(
                    net,
                    control.regulating_terminal.as_ref(),
                    control.controlled_bus,
                    format_args!("three winding transformer winding at bus {}", w.bus),
                    &mut warnings,
                )
            });
            let cont = control.map_or(i64::try_from(cont).unwrap_or(i64::MAX), |control| {
                emit_controlled_bus(
                    control,
                    cont,
                    format_args!("three winding transformer winding at bus {}", w.bus),
                    &mut warnings,
                )
            });
            let (rma, rmi, vma, vmi, ntp) = control.map_or((1.1, 0.9, 1.1, 0.9, 33), |control| {
                (
                    control.tap_max,
                    control.tap_min,
                    control.band_max,
                    control.band_min,
                    control.ntp,
                )
            });
            let cnxa = control
                .and_then(|control| control.winding_connection_angle)
                .unwrap_or(0.0);
            let suffix = winding_index + 1;
            let tab = extra_i64(&t.extras, &format!("psse_tab{suffix}")).unwrap_or(0);
            let cr = extra_f64(&t.extras, &format!("psse_cr{suffix}")).unwrap_or(0.0);
            let cx = extra_f64(&t.extras, &format!("psse_cx{suffix}")).unwrap_or(0.0);
            if modern {
                // v34+ winding layout (twelve ratings, NODE after CONT); the
                // Winding model carries three ratings, so RATE4-RATE12 are 0.
                let _ = writeln!(
                    s,
                    "{}, {}, {}, {}, {}, {}, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, \
                     {cod}, {cont}, {node}, {}, {}, {}, {}, {ntp}, {tab}, {}, {}, {}",
                    num(w.tap),
                    num(w.nominal_kv),
                    num(w.shift),
                    num(w.rate_a),
                    num(w.rate_b),
                    num(w.rate_c),
                    num(rma),
                    num(rmi),
                    num(vma),
                    num(vmi),
                    num(cr),
                    num(cx),
                    num(cnxa)
                );
            } else {
                let _ = writeln!(
                    s,
                    "{}, {}, {}, {}, {}, {}, {cod}, {cont}, {}, {}, {}, {}, {ntp}, {tab}, {}, {}, {}",
                    num(w.tap),
                    num(w.nominal_kv),
                    num(w.shift),
                    num(w.rate_a),
                    num(w.rate_b),
                    num(w.rate_c),
                    num(rma),
                    num(rmi),
                    num(vma),
                    num(vmi),
                    num(cr),
                    num(cx),
                    num(cnxa)
                );
            }
        }
    }
    let _ = writeln!(s, "0 / END OF TRANSFORMER DATA, BEGIN AREA DATA");
    for a in net.areas() {
        let raw_name = a.name.as_deref().unwrap_or("");
        let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
        if matches!(name, std::borrow::Cow::Owned(_)) {
            sanitized_quoted += 1;
        }
        let _ = writeln!(
            s,
            "{}, {}, {}, {}, '{:<12}'",
            a.number,
            a.slack_bus.map_or(0, |b| b.0),
            num(a.net_interchange),
            num(a.tolerance),
            name
        );
    }

    // Two-terminal DC lines occupy the first of the otherwise-empty sections:
    // emit their 3-line records (if any) between the begin/end markers, then the
    // remaining sections as bare terminators so the file parses as a complete case.
    let _ = writeln!(s, "{}", EMPTY_SECTIONS[0]);
    for (i, dc) in net.hvdc().iter().enumerate() {
        let raw_name = dc_str(&dc.extras, "psse_dc_name").unwrap_or_else(|| format!("DC{}", i + 1));
        let name = sanitize_quoted(&raw_name, NAME_FORBIDDEN, ' ');
        if matches!(name, std::borrow::Cow::Owned(_)) {
            sanitized_quoted += 1;
        }
        let name = format!("'{name}'");
        let mdc = if dc.in_service {
            dc_int(&dc.extras, "psse_dc_mdc").unwrap_or(1)
        } else {
            0
        };
        let rdc = dc_f64(&dc.extras, "psse_dc_rdc").unwrap_or(0.0);
        let vschd = dc_f64(&dc.extras, "psse_dc_vschd").unwrap_or(0.0);
        let l1_tail = dc_tail(&dc.extras, "psse_dc_control_tail", DEFAULT_CONTROL_TAIL);
        let (rect_tail, dropped_rectifier_bridges) =
            dc_converter_tail(&dc.extras, "psse_dc_rectifier_tail", rev);
        let (inv_tail, dropped_inverter_bridges) =
            dc_converter_tail(&dc.extras, "psse_dc_inverter_tail", rev);
        for (end, bridges) in [
            ("rectifier", dropped_rectifier_bridges),
            ("inverter", dropped_inverter_bridges),
        ] {
            if let Some(bridges) = bridges {
                warnings.push(
                    &codes::EMIT_PSSE.field_dropped,
                    format!(
                        "DC line `{}` {end} states {bridges} bridge(s) in series; a PSS/E revision 33 two-terminal DC record has no NDR/NDI column",
                        dc.uid.as_deref().unwrap_or("<unnamed>")
                    ),
                );
            }
        }
        // SETVL in the stored mode's own unit, the exact inverse of the
        // reader's derivation: MDC 2 schedules a current, so the rectifier
        // power converts back to amps through the scheduled voltage. Every
        // other mode states the rectifier power in MW; a demand the source
        // measured at the inverter (negative SETVL) comes back measured at
        // the rectifier, which names the same operating point.
        let setvl = if let Some(stated) = dc_f64(&dc.extras, "psse_dc_setvl") {
            // A current schedule the reader could not price. It reads as zero
            // power, so only the retained record can state it back.
            stated
        } else if mdc == 2 && vschd > 0.0 {
            1000.0 * dc.pf / vschd
        } else if mdc == 1 && dc.extras.contains_key(SETVL_AT_INVERTER) {
            // The source measured its demand at the inverter, which SETVL
            // states as a negative number. Writing the rectifier power instead
            // names a different operating point: the re-read would price the
            // drop off the larger end. Only MDC 1 reads the sign that way, so a
            // blocked line states the one number both its ends read back as.
            -dc.pt
        } else {
            dc.pf
        };
        let _ = writeln!(
            s,
            "{name}, {mdc}, {}, {}, {}, {l1_tail}",
            num(rdc),
            num(setvl),
            num(vschd)
        );
        let _ = writeln!(s, "{}, {rect_tail}", dc.from);
        let _ = writeln!(s, "{}, {inv_tail}", dc.to);
    }
    // Sections up to and including the SWITCHED SHUNT begin marker.
    for line in &EMPTY_SECTIONS[1..=9] {
        let _ = writeln!(s, "{line}");
    }
    // Switched shunts: BINIT becomes the susceptance, the control record the rest.
    // v35 inserts a quoted shunt ID at field 1 and NREG after SWREG, and its step
    // blocks are (S, N, B) triples with a leading per-block status; v33/34 have
    // neither and use (N, B) pairs. The writer must match the reader's layout at
    // each revision or every later field is read columns off.
    let mut sw_ids: BTreeMap<BusId, BTreeSet<String>> = BTreeMap::new();
    for sh in net.shunts().iter().filter(|s| s.control.is_some()) {
        let Some(c) = sh.control.as_ref() else {
            continue;
        };
        let modsw = extra_i64(&sh.extras, "psse_modsw")
            .filter(|raw| modsw_to_mode(*raw) == c.mode)
            .unwrap_or_else(|| mode_to_modsw(c.mode));
        let adjm = extra_i64(&sh.extras, "psse_adjm").unwrap_or(0);
        let raw_rmidnt = sh
            .extras
            .get("psse_rmidnt")
            .and_then(Value::as_str)
            .unwrap_or("");
        let rmidnt = sanitize_quoted(raw_rmidnt, NAME_FORBIDDEN, ' ');
        if matches!(rmidnt, std::borrow::Cow::Owned(_)) {
            sanitized_quoted += 1;
        }
        let (swrem, nreg) = regulating_target(
            net,
            c.regulating_terminal.as_ref(),
            c.control_bus,
            format_args!("switched shunt at bus {}", sh.bus),
            &mut warnings,
        );
        let mut blocks = String::new();
        for blk in &c.blocks {
            if rev >= 35 {
                // The neutral model has no per-block status: every block is in
                // service (S = 1).
                let _ = write!(blocks, ", 1, {}, {}", blk.steps, num(blk.b));
            } else {
                let _ = write!(blocks, ", {}, {}", blk.steps, num(blk.b));
            }
        }
        if rev >= 35 {
            let id = quoted_device_id(&sh.extras, sh.bus, &mut sw_ids, &mut sanitized_quoted);
            let _ = writeln!(
                s,
                "{}, '{id}', {}, {adjm}, {}, {}, {}, {swrem}, {nreg}, {}, '{rmidnt}', {}{blocks}",
                sh.bus,
                modsw,
                i32::from(sh.in_service),
                num(c.vhigh),
                num(c.vlow),
                num(c.rmpct),
                num(sh.b)
            );
        } else {
            let _ = writeln!(
                s,
                "{}, {}, {adjm}, {}, {}, {}, {swrem}, {}, '{rmidnt}', {}{blocks}",
                sh.bus,
                modsw,
                i32::from(sh.in_service),
                num(c.vhigh),
                num(c.vlow),
                num(c.rmpct),
                num(sh.b)
            );
        }
    }
    for line in &EMPTY_SECTIONS[10..12] {
        let _ = writeln!(s, "{line}");
    }
    let detailed_records = if include_detailed_connectivity && net.detailed_connectivity().is_some()
    {
        match super::rawx::write_raw_substation_data(net) {
            Ok((records, detailed_warnings)) => {
                warnings.absorb(detailed_warnings);
                records
            }
            Err(error) => {
                warnings.push(
                    &F.field_dropped,
                    format!(
                        "detailed connectivity dropped from PSS/E revision {rev} output: {error}"
                    ),
                );
                None
            }
        }
    } else {
        None
    };
    if rev >= 34 {
        let _ = writeln!(
            s,
            "0 / END OF INDUCTION MACHINE DATA, BEGIN SUBSTATION DATA"
        );
        if let Some(records) = detailed_records {
            s.push_str(&records);
        }
        let _ = writeln!(s, "0 / END OF SUBSTATION DATA");
    } else {
        let _ = writeln!(s, "{}", EMPTY_SECTIONS[12]);
        if detailed_records.is_some() {
            warnings.push(
                &F.record_dropped,
                "detailed substation connectivity dropped: PSS/E revision 33 has no substation data block",
            );
        }
    }
    let _ = writeln!(s, "Q");

    if net.hvdc().iter().any(dc_states_beyond_record) {
        warnings.push(
            &F.value_defaulted,
            "DC line converter detail (firing angles, converter transformer taps, reactive \
             output) defaulted: PSS/E two-terminal DC is written from the power setpoint and \
             line resistance only",
        );
    }
    if !net.storage().is_empty() {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} storage unit(s) dropped: PSS/E has no storage record",
                net.storage().len()
            ),
        );
    }
    if net.generators().iter().any(|g| g.cost.is_some()) {
        warnings.push(
            &F.field_dropped,
            "generator cost curves dropped: PSS/E .raw has no cost data",
        );
    }
    warn_generator_energy_sources_dropped(net, &mut warnings);
    if net.hvdc().iter().any(|d| d.cost.is_some()) {
        warnings.push(
            &F.field_dropped,
            "DC line cost curves dropped: PSS/E .raw has no cost data",
        );
    }
    if net.branches().iter().any(Branch::has_angle_limits) {
        warnings.push(
            &F.field_dropped,
            "branch angle limits (angmin/angmax) dropped: PSS/E branch records carry none",
        );
    }
    if rev == 33 {
        let named_branches = net
            .branches()
            .iter()
            .filter(|branch| {
                !branch.is_transformer()
                    && branch
                        .name
                        .as_deref()
                        .is_some_and(|name| !name.trim().is_empty())
            })
            .count();
        if named_branches > 0 {
            warnings.push(
                &F.field_dropped,
                format!(
                    "{named_branches} non-transformer branch name(s) dropped: PSS/E revision 33 branch records have no name field"
                ),
            );
        }
    }
    let current_ratings = net
        .branches()
        .iter()
        .filter(|b| b.current_ratings.is_some())
        .count();
    if current_ratings > 0 {
        warnings.push(&F.field_dropped, format!(
            "{current_ratings} branch current rating record(s) dropped: PSS/E branch ratings are MVA ratings"
        ));
    }
    if rev >= 35 {
        let switch_current_ratings = net
            .switches()
            .iter()
            .filter(|switch| switch.current_rating.is_some())
            .count();
        if switch_current_ratings > 0 {
            warnings.push(
                &F.field_dropped,
                format!(
                    "{switch_current_ratings} switch current rating(s) dropped: PSS/E system switching device records carry MVA ratings, not current ratings"
                ),
            );
        }
        let switch_power_flows = net
            .switches()
            .iter()
            .filter(|switch| {
                switch.pf.is_some()
                    || switch.qf.is_some()
                    || switch.pt.is_some()
                    || switch.qt.is_some()
            })
            .count();
        if switch_power_flows > 0 {
            warnings.push(
                &F.field_dropped,
                format!(
                    "{switch_power_flows} switch power flow result set(s) dropped: PSS/E system switching device records carry no power flow result fields"
                ),
            );
        }
        if include_detailed_connectivity {
            let rating_set_names = net
                .switches()
                .iter()
                .filter(|switch| switch.extras.contains_key("psse_rsetnam"))
                .count();
            if rating_set_names > 0 {
                warnings.push(
                    &F.field_dropped,
                    format!(
                        "{rating_set_names} system switching device rating set name(s) dropped: PSS/E RAW revision 35 writes explicit RATE1-RATE12 fields"
                    ),
                );
            }
        }
    }
    if !modern {
        warn_psse_extra_branch_ratings_dropped(net, &mut warnings);
    }
    // This writer replays device ids and every `psse_*` key its reader
    // retained; a foreign format's keys (LoadID, pslf_circuit, ...) have no
    // record column and drop, declared once (#330).
    super::warn_dropped_extras(
        &F,
        "PSS/E .raw",
        net,
        |key| key == "id" || key.starts_with("psse_"),
        &mut warnings,
    );
    let branch_solutions = net
        .branches()
        .iter()
        .filter(|b| b.solution.is_some())
        .count();
    if branch_solutions > 0 {
        warnings.push(&F.field_dropped, format!(
            "{branch_solutions} branch solution value set(s) dropped: PSS/E RAW power flow result fields are not written"
        ));
    }
    let transformer_terminal_shunts = net
        .branches()
        .iter()
        .filter(|b| {
            b.is_transformer()
                && b.charging
                    .is_some_and(|c| c.g_to.abs() > f64::EPSILON || c.b_to.abs() > f64::EPSILON)
        })
        .count();
    if transformer_terminal_shunts > 0 {
        warnings.push(&F.value_collapsed, format!(
            "{transformer_terminal_shunts} transformer terminal admittance record(s) collapsed to magnetizing admittance: PSS/E transformer records cannot preserve terminal side assignment"
        ));
    }
    if net.generators().iter().any(Generator::has_caps) {
        warnings.push(
            &F.field_dropped,
            "generator ramp/capability columns dropped: PSS/E .raw has no equivalent fields",
        );
    }
    if nonfinite {
        warnings.push(
            &F.not_a_number,
            "non-finite values written as ±1e10 sentinels (PSS/E has no Inf/NaN)",
        );
    }
    if sanitized_quoted > 0 {
        warnings.push(
            &F.value_substituted,
            format!(
                "{sanitized_quoted} quoted PSS/E field(s) contained a quote or '/' that would \
             corrupt a record; replaced with spaces"
            ),
        );
    }

    TextEmission::new(s, warnings)
}

/// MATPOWER/neutral bus kind → PSS/E bus type code (IDE).
fn ide(kind: BusType) -> u8 {
    kind as u8 // 1=PQ, 2=PV, 3=ref/swing, 4=isolated — same codes
}

/// The circuit id for an element: its sanitized `extras["id"]` when present and
/// still free on this bus, else the lowest positional id still free, so parallel
/// devices stay distinct and the PSS/E `(bus, id)` uniqueness rule holds even
/// when source ids collide before or after sanitation. `used` tracks the ids
/// already emitted per bus.
fn quoted_device_id(
    extras: &Extras,
    bus: BusId,
    used: &mut BTreeMap<BusId, BTreeSet<String>>,
    sanitized_quoted: &mut usize,
) -> String {
    quoted_circuit_id(
        extras.get("id").and_then(Value::as_str),
        bus,
        used,
        sanitized_quoted,
    )
}

fn quoted_circuit_id<K: Ord + Clone>(
    preferred: Option<&str>,
    key: K,
    used: &mut BTreeMap<K, BTreeSet<String>>,
    sanitized_quoted: &mut usize,
) -> String {
    let sanitized = preferred.map(|id| {
        let cleaned = sanitize_quoted(id, NAME_FORBIDDEN, ' ');
        if matches!(cleaned, std::borrow::Cow::Owned(_)) {
            *sanitized_quoted += 1;
        }
        cleaned.into_owned()
    });
    super::allocate_circuit_id(sanitized.as_deref(), key, used)
}

/// Whether an HVDC line states DC-side data the two-terminal record cannot
/// carry. The record states MDC/RDC/SETVL/VSCHD plus converter tails, and
/// [`read_dc_line`] reads that shape back with the received power priced by
/// the line's own drop, `pmax` at the larger end, and neutral terminals — so
/// a line matching the record's own shape writes with no loss, and one that
/// states more (a received power off the record's drop model, terminal
/// voltage setpoints, reactive limits, its own power band, or a loss model)
/// earns the dropped-detail warning regardless of which format it came from.
#[allow(clippy::float_cmp)] // the neutral shape is produced bit-exactly by the reader
fn dc_states_beyond_record(d: &Hvdc) -> bool {
    let rdc = dc_f64(&d.extras, "psse_dc_rdc").unwrap_or(0.0);
    let vschd = dc_f64(&d.extras, "psse_dc_vschd").unwrap_or(0.0);
    // A blocked line writes MDC 0, under which the reader prices no drop and
    // both ends read as the one stated number. Only a line that is both in
    // service and scheduling a voltage has a current to price the loss with.
    let priced = d.in_service && vschd > 0.0;
    let at_inverter = d.in_service && d.extras.contains_key(SETVL_AT_INVERTER);
    // The drop the rewrite reproduces from the replayed RDC/VSCHD. A whisker
    // of tolerance covers the units round trip of a current-mode record
    // (SETVL -> kA -> SETVL). An inverter-measured record writes back as
    // `-pt`, so the rewrite states the received power exactly and derives the
    // sent power from it. That makes `pf` the field the record cannot carry on
    // its own, and a source stating one that disagrees with the drop model
    // would lose it silently.
    if at_inverter {
        let expected_pf = if priced {
            let i = d.pt / vschd;
            d.pt + i * i * rdc
        } else {
            d.pt
        };
        if (d.pf - expected_pf).abs() > 1e-9 * d.pt.abs().max(1.0) {
            return true;
        }
    }
    let expected_pt = if at_inverter {
        d.pt
    } else if priced {
        let i = d.pf / vschd;
        d.pf - i * i * rdc
    } else {
        d.pf
    };
    (d.pt - expected_pt).abs() > 1e-9 * d.pf.abs().max(1.0)
        || d.vf != 1.0
        || d.vt != 1.0
        || d.pmin != 0.0
        || d.pmax != d.pf.abs().max(d.pt.abs())
        || [d.qf, d.qt, d.qminf, d.qmaxf, d.qmint, d.qmaxt]
            .iter()
            .any(|q| *q != 0.0)
        || d.loss0 != 0.0
        || d.loss1 != 0.0
}

fn detailed_source_id<'a>(
    net: &'a BalancedNetwork,
    component_type: &str,
    uid: Option<&str>,
) -> Option<&'a str> {
    detailed_source_property(net, component_type, uid, "psse_eqid")
}

fn detailed_source_property<'a>(
    net: &'a BalancedNetwork,
    component_type: &str,
    uid: Option<&str>,
    property: &str,
) -> Option<&'a str> {
    let uid = uid?;
    net.detailed_connectivity()
        .as_deref()?
        .component_metadata
        .iter()
        .find(|metadata| {
            metadata.component.component_type() == component_type
                && metadata.component.local_id() == uid
        })?
        .properties
        .get(property)
        .map(String::as_str)
}

fn generator_source_float(
    net: &BalancedNetwork,
    generator: &Generator,
    property: &str,
    default: f64,
    warnings: &mut Diagnostics,
) -> f64 {
    let Some(raw) = detailed_source_property(net, "generator", generator.uid.as_deref(), property)
    else {
        return default;
    };
    if let Some(value) = raw.parse::<f64>().ok().filter(|value| value.is_finite()) {
        return value;
    }
    warnings.push(
        &F.value_substituted,
        format!(
            "PSS/E generator at bus {}: retained {} value {raw:?} is not a finite number; emitted default {default}",
            generator.bus,
            property.trim_start_matches("psse_").to_ascii_uppercase()
        ),
    );
    default
}

fn generator_source_integer(
    net: &BalancedNetwork,
    generator: &Generator,
    property: &str,
    default: i64,
    warnings: &mut Diagnostics,
) -> i64 {
    let Some(raw) = detailed_source_property(net, "generator", generator.uid.as_deref(), property)
    else {
        return default;
    };
    let value = raw.parse::<f64>().ok().filter(|value| {
        value.is_finite() && *value >= i64::MIN as f64 && *value <= i64::MAX as f64
    });
    if let Some(value) = value {
        #[allow(clippy::cast_possible_truncation)]
        return value as i64;
    }
    warnings.push(
        &F.value_substituted,
        format!(
            "PSS/E generator at bus {}: retained {} value {raw:?} is not a finite integer code; emitted default {default}",
            generator.bus,
            property.trim_start_matches("psse_").to_ascii_uppercase()
        ),
    );
    default
}

fn detailed_regulating_target(
    net: &BalancedNetwork,
    reference: &TerminalReference,
) -> Option<(BusId, i32)> {
    let detailed = net.detailed_connectivity().as_deref()?;
    let terminal = detailed.terminals.iter().find(|terminal| {
        terminal.equipment == reference.equipment && terminal.terminal == reference.terminal
    })?;
    let node_id = terminal.node.as_ref()?;
    let node = detailed
        .connectivity_nodes
        .iter()
        .find(|node| &node.component == node_id)?;
    Some((node.calculated_bus?, node.node_number?))
}

fn regulating_target(
    net: &BalancedNetwork,
    reference: Option<&TerminalReference>,
    bus: Option<BusId>,
    description: impl std::fmt::Display,
    warnings: &mut Diagnostics,
) -> (usize, i32) {
    if let Some(reference) = reference {
        if let Some((bus, node)) = detailed_regulating_target(net, reference) {
            return (bus.0, node);
        }
        warnings.push(
            &F.field_dropped,
            format!(
                "PSS/E {description}: the regulating terminal has no PSS/E bus and node mapping; emitted the regulated bus with node 0"
            ),
        );
    }
    (bus.map_or(0, |bus| bus.0), 0)
}

fn signed_controlled_bus(bus: usize, on_winding_side: bool) -> i64 {
    let bus = i64::try_from(bus).unwrap_or(i64::MAX);
    if on_winding_side { -bus } else { bus }
}

fn emit_controlled_bus(
    control: &TransformerControl,
    bus: usize,
    description: impl std::fmt::Display,
    warnings: &mut Diagnostics,
) -> i64 {
    if control.controlled_bus_on_winding_side
        && control
            .controlled_bus
            .is_none_or(|controlled| controlled.0 == 0)
    {
        warnings.push(
            &F.field_dropped,
            format!(
                "PSS/E {description}: a negative CONT requires a nonzero controlled bus; emitted CONT=0"
            ),
        );
        return 0;
    }
    signed_controlled_bus(bus, control.controlled_bus_on_winding_side)
}

/// Converter-line tail (everything after the AC terminal bus) for a synthesized
/// two-terminal DC record: NBR/NBI bridges, firing-angle limits, converter
/// transformer R/X and tap data, and the metered-end id. PSS/E-sourced lines
/// replay their own tail; these defaults serve a cross-format source.
const DEFAULT_CONVERTER_TAIL: &str =
    "1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0";

/// Fields a two-terminal DC converter line states after the bus number in
/// revision 33: NBR through XCAPR.
const CONVERTER_TAIL_FIELDS_33: usize = 16;

/// Where `NDR`/`NDI`, the number of bridges in series, sits in that tail from
/// revision 34 on: after ICR and before IFR.
const CONVERTER_TAIL_BRIDGE_INDEX: usize = 13;

/// The converter tail of a two-terminal DC record at `rev`.
///
/// The rectifier and inverter lines state `NDR`/`NDI` from revision 34 on and
/// have no such column in revision 33, so a tail retained from one revision
/// carries one field more or fewer than the record being written and the
/// column belongs at its own position, not at the end. Returns the tail and
/// the bridge count a revision 33 record has no column for.
fn dc_converter_tail(extras: &Extras, key: &str, rev: u32) -> (String, Option<String>) {
    let states_bridges = rev >= 34;
    let stated = extras
        .get(key)
        .and_then(Value::as_array)
        .filter(|fields| !fields.is_empty());
    let Some(stated) = stated else {
        let mut fields = DEFAULT_CONVERTER_TAIL
            .split(", ")
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if states_bridges {
            fields.insert(CONVERTER_TAIL_BRIDGE_INDEX, "0".to_owned());
        }
        return (fields.join(", "), None);
    };
    let mut fields = stated
        .iter()
        .filter_map(Value::as_str)
        // These come from a source file's `extras` and are replayed into a
        // record, so they go through the quoting seam like every other
        // interpolated string: a terminator here would forge a whole DC
        // record or a section end.
        .map(|field| sanitize_quoted(field, NAME_FORBIDDEN, ' ').into_owned())
        .collect::<Vec<_>>();
    let mut dropped_bridges = None;
    match (fields.len() > CONVERTER_TAIL_FIELDS_33, states_bridges) {
        (true, false) => {
            let bridges = fields.remove(CONVERTER_TAIL_BRIDGE_INDEX);
            if bridges.trim() != "0" {
                dropped_bridges = Some(bridges);
            }
        }
        (false, true) if fields.len() == CONVERTER_TAIL_FIELDS_33 => {
            fields.insert(CONVERTER_TAIL_BRIDGE_INDEX, "0".to_owned());
        }
        _ => {}
    }
    (fields.join(", "), dropped_bridges)
}

/// Control-line tail (everything after VSCHD) for a synthesized two-terminal DC
/// record: compounding voltage, margin, metering code, and minimum firing data.
const DEFAULT_CONTROL_TAIL: &str = "0.0, 0.0, 0.0, 'I', 0.0, 20, 1.0";

/// Extras key marking a demand the record measured at the inverter (SETVL
/// negative under MDC 1). Read, written, and audited in three places, so it is
/// spelled once.
const SETVL_AT_INVERTER: &str = "psse_dc_setvl_at_inverter";

const EMPTY_SECTIONS: [&str; 13] = [
    "0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA",
    "0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA",
    "0 / END OF VSC DC LINE DATA, BEGIN IMPEDANCE CORRECTION DATA",
    "0 / END OF IMPEDANCE CORRECTION DATA, BEGIN MULTI-TERMINAL DC DATA",
    "0 / END OF MULTI-TERMINAL DC DATA, BEGIN MULTI-SECTION LINE DATA",
    "0 / END OF MULTI-SECTION LINE DATA, BEGIN ZONE DATA",
    "0 / END OF ZONE DATA, BEGIN INTER-AREA TRANSFER DATA",
    "0 / END OF INTER-AREA TRANSFER DATA, BEGIN OWNER DATA",
    "0 / END OF OWNER DATA, BEGIN FACTS DEVICE DATA",
    "0 / END OF FACTS DEVICE DATA, BEGIN SWITCHED SHUNT DATA",
    "0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA",
    "0 / END OF GNE DEVICE DATA, BEGIN INDUCTION MACHINE DATA",
    "0 / END OF INDUCTION MACHINE DATA",
];

// ---- Reader -----------------------------------------------------------------

/// The PSS/E revision declared in a retained `.raw` header (field 3, `REV`), or
/// 33 when that field is absent. The format hub uses it to decide whether same
/// format emission can echo the source bytes or must serialize at a different
/// revision.
pub(crate) fn header_rev(source: &str) -> Result<u32> {
    let header = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !is_comment(line))
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "empty file".into(),
        })?;
    let header_fields = fields(header);
    parse_revision(header_fields.get(2).map(AsRef::as_ref))
}

/// The header `REV` field as one of the revisions the reader lays records out
/// for: 32 through 35. An absent field means 33, the historical default.
fn parse_revision(field: Option<&str>) -> Result<u32> {
    let Some(field) = field.filter(|field| !field.is_empty()) else {
        return Ok(33);
    };
    let unsupported = || Error::FormatRead {
        format: FMT,
        message: format!(
            "header REV {field:?} is not a supported revision; expected integral 32, 33, 34, or 35"
        ),
    };
    let revision = field.parse::<f64>().map_err(|_| unsupported())?;
    match revision.to_bits() {
        value if value == 32.0_f64.to_bits() => Ok(32),
        value if value == 33.0_f64.to_bits() => Ok(33),
        value if value == 34.0_f64.to_bits() => Ok(34),
        value if value == 35.0_f64.to_bits() => Ok(35),
        _ => Err(unsupported()),
    }
}

/// The fields a revision 32 record must state for the typed model: the count
/// through the last field the reader maps into the network, and that field's
/// name. The fields after it (ownership, metering, the retained extras) are
/// optional in every revision, so a record that ends before `width` is
/// missing electrical data rather than trailing options.
#[derive(Clone, Copy)]
struct Revision32Shape {
    record: &'static str,
    width: usize,
    last: &'static str,
}

impl Revision32Shape {
    const fn new(record: &'static str, width: usize, last: &'static str) -> Self {
        Self {
            record,
            width,
            last,
        }
    }
}

const REVISION32_TRANSFORMER_IMPEDANCE_2W: Revision32Shape =
    Revision32Shape::new("TRANSFORMER DATA impedance line", 3, "SBASE1-2");
const REVISION32_TRANSFORMER_IMPEDANCE_3W: Revision32Shape =
    Revision32Shape::new("TRANSFORMER DATA impedance line", 11, "ANSTAR");
const REVISION32_TRANSFORMER_WINDING: Revision32Shape =
    Revision32Shape::new("TRANSFORMER DATA winding line", 13, "NTP");
const REVISION32_TRANSFORMER_WINDING_2: Revision32Shape =
    Revision32Shape::new("TRANSFORMER DATA winding 2 line", 2, "NOMV2");

/// The typed width of the first line of a revision 32 record in `section`.
/// Sections the reader skips, and the sections revision 32 does not have,
/// carry no shape.
fn revision32_shape(section: Section) -> Option<Revision32Shape> {
    Some(match section {
        Section::Bus => Revision32Shape::new("BUS DATA", 9, "VA"),
        Section::Load => Revision32Shape::new("LOAD DATA", 13, "SCALE"),
        Section::FixedShunt => Revision32Shape::new("FIXED SHUNT DATA", 5, "BL"),
        Section::SwitchedShunt => Revision32Shape::new("SWITCHED SHUNT DATA", 10, "BINIT"),
        Section::Generator => Revision32Shape::new("GENERATOR DATA", 18, "PB"),
        Section::Branch => Revision32Shape::new("BRANCH DATA", 14, "ST"),
        Section::Transformer => Revision32Shape::new("TRANSFORMER DATA", 12, "STAT"),
        Section::TwoTerminalDc => Revision32Shape::new("TWO-TERMINAL DC DATA", 5, "VSCHD"),
        Section::Area => Revision32Shape::new("AREA DATA", 4, "PTOL"),
        Section::SystemSwitch | Section::SystemWide | Section::Skip => return None,
    })
}

/// Report a revision 32 record that ends before the last field its typed
/// layout reads. The finding carries the collector's current record span,
/// the byte range of the record being decoded, when the collector is located
/// in the source buffer. The missing fields have already taken their
/// defaults, so the finding is a warning rather than a failure.
fn check_revision32_width(f: &[Cow<'_, str>], shape: Revision32Shape, warnings: &mut Diagnostics) {
    if f.len() >= shape.width {
        return;
    }
    let first = f.first().map_or("", Cow::as_ref);
    warnings.push(
        &codes::READ_PSSE_VALUE_DEFAULTED,
        format!(
            "PSS/E revision 32 {} record beginning {first:?} has {} field(s); the revision 32 layout reads {} fields through {}, so the missing fields took their defaults",
            shape.record,
            f.len(),
            shape.width,
            shape.last
        ),
    );
}

#[derive(Debug, Clone, Copy)]
enum RegulatingNodeTarget {
    Generator(usize),
    SwitchedShunt(usize),
    Transformer(usize),
    ThreeWindingTransformer { transformer: usize, winding: usize },
}

#[derive(Debug, Clone, Copy)]
struct PendingRegulatingNode {
    target: RegulatingNodeTarget,
    node: i32,
}

fn apply_pending_regulating_nodes(
    net: &mut BalancedNetwork,
    pending: &[PendingRegulatingNode],
    warnings: &mut Diagnostics,
) {
    let detailed = net.detailed_connectivity().clone();
    for pending in pending {
        let (target, bus, description): (&mut Option<TerminalReference>, BusId, String) =
            match pending.target {
                RegulatingNodeTarget::Generator(index) => {
                    let generator = &mut net.generators_mut()[index];
                    (
                        &mut generator.regulating_terminal,
                        generator.regulated_bus.unwrap_or(generator.bus),
                        format!("PSS/E generator at bus {}", generator.bus),
                    )
                }
                RegulatingNodeTarget::SwitchedShunt(index) => {
                    let shunt = &mut net.shunts_mut()[index];
                    let control = shunt
                        .control
                        .as_mut()
                        .expect("a pending switched shunt node has control data");
                    (
                        &mut control.regulating_terminal,
                        control.control_bus.unwrap_or(shunt.bus),
                        format!("PSS/E switched shunt at bus {}", shunt.bus),
                    )
                }
                RegulatingNodeTarget::Transformer(index) => {
                    let transformer = &mut net.branches_mut()[index];
                    let control = transformer
                        .control
                        .as_mut()
                        .expect("a pending transformer node has control data");
                    (
                        &mut control.regulating_terminal,
                        control.controlled_bus.unwrap_or(transformer.from),
                        format!(
                            "PSS/E two winding transformer {}-{}",
                            transformer.from, transformer.to
                        ),
                    )
                }
                RegulatingNodeTarget::ThreeWindingTransformer {
                    transformer,
                    winding,
                } => {
                    let winding_data =
                        &mut net.transformers_3w_mut()[transformer].windings[winding];
                    let control = winding_data
                        .control
                        .as_mut()
                        .expect("a pending transformer winding node has control data");
                    (
                        &mut control.regulating_terminal,
                        control.controlled_bus.unwrap_or(winding_data.bus),
                        format!(
                            "PSS/E three winding transformer winding {} at bus {}",
                            winding + 1,
                            winding_data.bus
                        ),
                    )
                }
            };
        if let Some(reference) = detailed
            .as_deref()
            .and_then(|detailed| super::rawx::regulating_terminal(detailed, bus, pending.node))
        {
            *target = Some(reference);
        } else {
            warnings.push(
                &codes::READ_PSSE_REFERENCE_DROPPED,
                format!(
                    "{description}: node {} at regulated bus {bus} has no detailed connectivity terminal",
                    pending.node
                ),
            );
        }
    }
}

/// Parse `source` by borrowing it; the caller retains the buffer. `name_hint`
/// (e.g. a file stem) names the network when the title line is blank.
///
/// Every finding raised at a record, and a failure that ends the read at
/// one, carries that record's byte range when `warnings` is located in the
/// source buffer (the format hub locates it); an unlocated collector attaches
/// no span.
// A flat reader: header parse plus one match arm per section. Splitting it would
// add indirection without clarity.
pub(crate) fn parse_psse_source(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    parse_psse_source_inner(source, name_hint, warnings, false)
}

pub(super) fn parse_psse_source_deferred_regulating_nodes(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    parse_psse_source_inner(source, name_hint, warnings, true)
}

#[expect(clippy::too_many_lines)]
fn parse_psse_source_inner(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
    defer_regulating_nodes: bool,
) -> Result<BalancedNetwork> {
    let content: &str = source;
    let mut lines = RawLines::new(content);

    // Header line 1: IC, SBASE, REV, ...
    let header = loop {
        let Some((start, raw)) = lines.next_line() else {
            return Err(Error::FormatRead {
                format: FMT,
                message: "empty file".into(),
            });
        };
        let line = raw.trim();
        if !line.is_empty() && !is_comment(line) {
            mark_record(warnings, start, raw);
            break raw;
        }
    };
    let header_fields = fields(header);
    let base_mva = header_fields
        .get(1)
        .filter(|field| !field.is_empty())
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "missing SBASE in header".into(),
        })?
        .parse::<f64>()
        .map_err(|_| Error::FormatRead {
            format: FMT,
            message: "header SBASE is not a number".into(),
        })?;
    if !base_mva.is_finite() || base_mva <= 0.0 {
        return Err(Error::FormatRead {
            format: FMT,
            message: format!("header SBASE must be positive and finite, got {base_mva}"),
        });
    }
    let raw_rev = parse_revision(header_fields.get(2).map(AsRef::as_ref))?;
    // BASFRQ is the sixth header field (IC, SBASE, REV, XFRRAT, NXFRAT, BASFRQ);
    // older revisions that carry only `SBASE, title` lack it, so default it.
    let base_frequency = match header_fields.get(5).map(AsRef::as_ref) {
        None | Some("") => crate::network::DEFAULT_BASE_FREQUENCY,
        Some(field) => {
            let value = field.parse::<f64>().map_err(|_| Error::FormatRead {
                format: FMT,
                message: format!("header BASFRQ {field:?} is not a number"),
            })?;
            if !value.is_finite() || value <= 0.0 {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "header BASFRQ must be positive and finite when present, got {value}"
                    ),
                });
            }
            value
        }
    };
    warnings.leave_record();
    // Line 2 is the case title; the emitter writes the network name there, so
    // read it back.
    let title = lines.next_line().map_or("", |(_, line)| line).trim();
    let name = if title.is_empty() {
        name_hint.unwrap_or("case").to_string()
    } else {
        title.to_string()
    };
    lines.next_line(); // line 3: second comment

    let mut buses = Vec::new();
    let mut loads = Vec::new();
    let mut shunts = Vec::new();
    let mut generators = Vec::new();
    let mut generator_source_metadata = Vec::new();
    let mut generator_ids: BTreeMap<BusId, BTreeSet<String>> = BTreeMap::new();
    let mut branches = Vec::new();
    let mut switches = Vec::new();
    let mut transformers_3w = Vec::new();
    let mut hvdc = Vec::new();
    let mut areas = Vec::new();
    let mut solver = SolverParams::default();
    let mut bus_base_kv: BTreeMap<BusId, f64> = BTreeMap::new();
    let mut bus_area_zone: BTreeMap<BusId, (usize, usize)> = BTreeMap::new();
    let mut unmodeled_sections: BTreeMap<String, usize> = BTreeMap::new();
    let mut regulating_nodes = Vec::new();

    // Sections appear in fixed order, each ended by a record whose first field is
    // `0`. We read the ones we model and treat the rest as skipped.
    let mut section = Section::Bus;
    let mut saw_bus_marker = false;
    let mut skipped_section_name: Option<String> = None;
    while let Some((start, raw)) = lines.next_line() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if is_comment(line) {
            continue;
        }
        if line == "Q" {
            break;
        }
        if is_terminator(line) {
            warnings.leave_record();
            // The terminator names the section that begins next ("…, BEGIN
            // SWITCHED SHUNT DATA"); read that rather than counting, so the many
            // unmodeled sections between transformers and switched shunts don't
            // throw off the position.
            let next_section = section_after_marker(line, raw_rev);
            skipped_section_name =
                introduced_section_name(line).filter(|_| matches!(next_section, Section::Skip));
            section = next_section;
            saw_bus_marker |= matches!(section, Section::Bus);
            continue;
        }
        // Every finding raised while this record is decoded, and a failure
        // that ends the read here, carries the record's byte range; a
        // continuation line extends it.
        mark_record(warnings, start, raw);
        let f = fields(line);
        // Revision 32 has the shortest layouts, so a record that ends before
        // its last typed field is missing electrical data; the reader reports
        // the record, with its byte range when the collector is located.
        if raw_rev == 32
            && let Some(shape) = revision32_shape(section)
        {
            check_revision32_width(&f, shape, warnings);
        }
        match section {
            Section::Bus if !saw_bus_marker && buses.is_empty() && is_system_wide_record(&f) => {
                // The v34+ system-wide block precedes the bus data; capture its
                // solver keyword lines (this is the first one that triggered).
                section = Section::SystemWide;
                parse_solver_line(&f, &mut solver, warnings);
            }
            Section::Bus => {
                let bus = read_bus(&f, raw_rev)?;
                bus_base_kv.insert(bus.id, bus.base_kv);
                bus_area_zone.insert(bus.id, (bus.area, bus.zone));
                buses.push(bus);
            }
            Section::Load => {
                let mut load = read_load(&f, raw_rev, warnings)?;
                drop_bus_default_area_zone(&mut load, &bus_area_zone);
                loads.push(load);
            }
            Section::FixedShunt => shunts.push(read_shunt(&f)?),
            Section::SwitchedShunt => {
                let index = shunts.len();
                let (shunt, node) = read_switched_shunt(&f, raw_rev, warnings)?;
                shunts.push(shunt);
                if node != 0 {
                    regulating_nodes.push(PendingRegulatingNode {
                        target: RegulatingNodeTarget::SwitchedShunt(index),
                        node,
                    });
                }
            }
            Section::Generator => {
                let index = generators.len();
                let (generator, node, source_metadata) = read_gen(&f, raw_rev, &mut generator_ids)?;
                generators.push(generator);
                generator_source_metadata.push(source_metadata);
                if node != 0 {
                    regulating_nodes.push(PendingRegulatingNode {
                        target: RegulatingNodeTarget::Generator(index),
                        node,
                    });
                }
            }
            Section::Branch => branches.push(read_branch(&f, raw_rev)?),
            Section::SystemSwitch => switches.push(read_system_switch(&f)?),
            Section::Transformer => {
                // 2-winding = 4 lines (K field == 0); 3-winding = 5 lines.
                // int_at parses through f64: v34/35 exporters write K in float
                // form ("0.00"), and an i64 parse would misclassify the record
                // as 3-winding and desynchronize the section.
                let two_winding = int_at(&f, 2, 0)? == 0;
                let l2 = next_continuation_line(
                    &mut lines,
                    warnings,
                    "transformer",
                    "transformer impedance line",
                )?;
                let l3 = next_continuation_line(
                    &mut lines,
                    warnings,
                    "transformer",
                    "winding data line 1",
                )?;
                let l4 = next_continuation_line(
                    &mut lines,
                    warnings,
                    "transformer",
                    "winding data line 2",
                )?;
                let (f2, f3, f4) = (fields(l2), fields(l3), fields(l4));
                if two_winding {
                    if raw_rev == 32 {
                        for (f, shape) in [
                            (&f2, REVISION32_TRANSFORMER_IMPEDANCE_2W),
                            (&f3, REVISION32_TRANSFORMER_WINDING),
                            (&f4, REVISION32_TRANSFORMER_WINDING_2),
                        ] {
                            check_revision32_width(f, shape, warnings);
                        }
                    }
                    let index = branches.len();
                    let (transformer, node) = read_transformer(
                        &f,
                        &f2,
                        &f3,
                        &f4,
                        raw_rev,
                        base_mva,
                        &bus_base_kv,
                        warnings,
                    )?;
                    branches.push(transformer);
                    if node != 0 {
                        regulating_nodes.push(PendingRegulatingNode {
                            target: RegulatingNodeTarget::Transformer(index),
                            node,
                        });
                    }
                } else {
                    let l5 = next_continuation_line(
                        &mut lines,
                        warnings,
                        "transformer",
                        "winding data line 3",
                    )?;
                    let f5 = fields(l5);
                    if raw_rev == 32 {
                        for (f, shape) in [
                            (&f2, REVISION32_TRANSFORMER_IMPEDANCE_3W),
                            (&f3, REVISION32_TRANSFORMER_WINDING),
                            (&f4, REVISION32_TRANSFORMER_WINDING),
                            (&f5, REVISION32_TRANSFORMER_WINDING),
                        ] {
                            check_revision32_width(f, shape, warnings);
                        }
                    }
                    let index = transformers_3w.len();
                    let (transformer, nodes) = read_transformer_3w(
                        &f,
                        &f2,
                        &f3,
                        &f4,
                        &f5,
                        raw_rev,
                        base_mva,
                        &bus_base_kv,
                        warnings,
                    )?;
                    transformers_3w.push(transformer);
                    for (winding, node) in nodes.into_iter().enumerate() {
                        if node != 0 {
                            regulating_nodes.push(PendingRegulatingNode {
                                target: RegulatingNodeTarget::ThreeWindingTransformer {
                                    transformer: index,
                                    winding,
                                },
                                node,
                            });
                        }
                    }
                }
            }
            Section::TwoTerminalDc => {
                // 3-line record: control line, then the rectifier and inverter
                // converter lines whose first field is the AC terminal bus.
                let rectifier = next_continuation_line(
                    &mut lines,
                    warnings,
                    "two-terminal DC",
                    "rectifier line",
                )?;
                let inverter = next_continuation_line(
                    &mut lines,
                    warnings,
                    "two-terminal DC",
                    "inverter line",
                )?;
                hvdc.push(read_dc_line(
                    &f,
                    &fields(rectifier),
                    &fields(inverter),
                    hvdc.len(),
                    warnings,
                )?);
            }
            Section::Area => areas.push(read_area(&f)?),
            Section::SystemWide => parse_solver_line(&f, &mut solver, warnings),
            Section::Skip => {
                if let Some(name) = skipped_section_name.as_ref() {
                    *unmodeled_sections.entry(name.clone()).or_default() += 1;
                }
            }
        }
    }

    warnings.leave_record();

    if raw_rev >= 34 {
        unmodeled_sections.retain(|name, _| !name.starts_with("SUBSTATION"));
    }
    warn_unmodeled_sections(unmodeled_sections, raw_rev, warnings);

    let mut net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva,
        base_frequency,
        geo: None,
        case_metadata: crate::network::CaseMetadata::default(),
        detailed_connectivity: None,
        buses: buses.into(),
        loads: loads.into(),
        shunts: shunts.into(),
        static_var_compensators: Vec::new().into(),
        branches: branches.into(),
        switches: switches.into(),
        generators: generators.into(),
        storage: Vec::new().into(),
        hvdc: hvdc.into(),
        transformers_3w: transformers_3w.into(),
        areas: areas.into(),
        solver: (!solver.is_empty()).then_some(solver),
        source_format: SourceFormat::Psse,
    });
    attach_generator_source_metadata(&mut net, generator_source_metadata)?;
    if raw_rev >= 34 {
        super::rawx::read_raw_detailed_connectivity(source, &mut net, warnings)?;
    }
    if !defer_regulating_nodes {
        apply_pending_regulating_nodes(&mut net, &regulating_nodes, warnings);
    }
    drop_stale_control_pointers(&mut net, warnings);
    net.check_references(FMT)?;
    Ok(net)
}

#[derive(Clone, Copy)]
enum Section {
    Bus,
    Load,
    FixedShunt,
    SwitchedShunt,
    Generator,
    Branch,
    SystemSwitch,
    Transformer,
    TwoTerminalDc,
    Area,
    SystemWide,
    Skip,
}

/// The section a terminator introduces. Sections we don't model map to
/// [`Section::Skip`]. Case-insensitive on the marker text, so the number of
/// skipped sections between the modeled ones doesn't matter.
fn section_after_marker(line: &str, rev: u32) -> Section {
    if let Some(name) = introduced_section_name(line) {
        return section_from_name(&name);
    }

    // Older RAW writers commonly label only the section that ended. Follow
    // the published record order instead of discarding every later section.
    match ended_section_name(line).as_deref() {
        Some("SYSTEM-WIDE") => Section::Bus,
        Some("BUS") => Section::Load,
        Some("LOAD") => Section::FixedShunt,
        Some("FIXED SHUNT" | "FIXED BUS SHUNT") => Section::Generator,
        Some("GENERATOR" | "GEN") => Section::Branch,
        Some("BRANCH" | "NON TRANSFORMER BRANCH" | "NON-TRANSFORMER BRANCH") => {
            if rev >= 35 {
                Section::SystemSwitch
            } else {
                Section::Transformer
            }
        }
        Some("SYSTEM SWITCHING DEVICE") => Section::Transformer,
        Some("TRANSFORMER" | "TRANSFORMER BRANCH") => Section::Area,
        Some("AREA" | "AREA INTERCHANGE") => Section::TwoTerminalDc,
        Some("FACTS DEVICE" | "FACTS CONTROL DEVICE") => Section::SwitchedShunt,
        _ => Section::Skip,
    }
}

fn section_from_name(name: &str) -> Section {
    match name {
        "BUS" => Section::Bus,
        "LOAD" => Section::Load,
        "FIXED SHUNT" => Section::FixedShunt,
        "SWITCHED SHUNT" => Section::SwitchedShunt,
        "GENERATOR" | "GEN" => Section::Generator,
        "BRANCH" => Section::Branch,
        "SYSTEM SWITCHING DEVICE" => Section::SystemSwitch,
        "TRANSFORMER" => Section::Transformer,
        "TWO-TERMINAL DC" | "TWO TERMINAL DC" | "2-TERMINAL DC" | "2 TERMINAL DC" => {
            Section::TwoTerminalDc
        }
        "AREA" | "AREA INTERCHANGE" => Section::Area,
        _ => Section::Skip,
    }
}

/// A record line's first field is `0` (the section terminator).
fn is_terminator(line: &str) -> bool {
    fields(line).first().map(Cow::as_ref) == Some("0")
}

/// The lines of the RAW text, each with the byte offset of its first
/// character, so a record's byte range can be attached to the findings it
/// produces. A line terminator (`\n` or `\r\n`) is excluded from the yielded
/// text, as by `str::lines`.
struct RawLines<'a> {
    text: &'a str,
    offset: usize,
}

impl<'a> RawLines<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, offset: 0 }
    }

    fn next_line(&mut self) -> Option<(usize, &'a str)> {
        if self.offset >= self.text.len() {
            return None;
        }
        let start = self.offset;
        let rest = &self.text[start..];
        let (line, consumed) = match rest.find('\n') {
            Some(end) => (&rest[..end], end + 1),
            None => (rest, rest.len()),
        };
        self.offset += consumed;
        Some((start, line.strip_suffix('\r').unwrap_or(line)))
    }
}

/// Mark the record on `raw`, a line starting at byte `start`, as the
/// collector's current record: its text without surrounding whitespace.
fn mark_record(warnings: &mut Diagnostics, start: usize, raw: &str) {
    let record_start = start + (raw.len() - raw.trim_start().len());
    warnings.enter_record(record_start, record_start + raw.trim().len());
}

/// The next data line of a multi-line record, extending the collector's
/// current record over it.
fn next_continuation_line<'a>(
    lines: &mut RawLines<'a>,
    warnings: &mut Diagnostics,
    record: &str,
    expected: &str,
) -> Result<&'a str> {
    while let Some((start, raw)) = lines.next_line() {
        let line = raw.trim();
        if line.is_empty() || is_comment(line) {
            continue;
        }
        if line.eq_ignore_ascii_case("q") || is_section_marker(line) || is_bare_terminator(line) {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!(
                    "PSS/E {record} record ended before {expected}: found section terminator `{line}`"
                ),
            });
        }
        let line_start = start + (raw.len() - raw.trim_start().len());
        warnings.extend_record(line_start + line.len());
        return Ok(line);
    }
    Err(Error::FormatRead {
        format: FMT,
        message: format!("PSS/E {record} record ended before {expected}"),
    })
}

fn is_bare_terminator(line: &str) -> bool {
    let f = fields(line);
    f.len() == 1 && f.first().map(Cow::as_ref) == Some("0")
}

fn transformer_basis_codes(f: &[Cow<'_, str>]) -> Result<(i64, i64)> {
    let cw = num_at(f, 4, 1.0)?;
    if cw.fract() != 0.0 {
        return Err(bad_field(4, f.get(4).map_or("", Cow::as_ref)));
    }
    let cz = num_at(f, 5, 1.0)?;
    if cz.fract() != 0.0 {
        return Err(bad_field(5, f.get(5).map_or("", Cow::as_ref)));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok((cw as i64, cz as i64))
}

fn transformer_label(f: &[Cow<'_, str>]) -> String {
    let i = f.first().map_or("?", Cow::as_ref);
    let j = f.get(1).map_or("?", Cow::as_ref);
    let k = f.get(2).map_or("?", Cow::as_ref);
    let id = f.get(3).map_or("", Cow::as_ref);
    format!("{i}-{j}-{k} id {id:?}")
}

#[expect(clippy::too_many_arguments)]
fn convert_transformer_impedance(
    r: f64,
    x: f64,
    sbase: f64,
    system_base: f64,
    cz: i64,
    label: &str,
    pair: &str,
    warnings: &mut Diagnostics,
) -> (f64, f64) {
    let base_ok = sbase.is_finite() && sbase > 0.0;
    match cz {
        1 => (r, x),
        2 => {
            if base_ok {
                let scale = system_base / sbase;
                (r * scale, x * scale)
            } else {
                warnings.push(&codes::READ_PSSE_VALUE_SUBSTITUTED, format!(
                    "PSS/E transformer {label} pair {pair}: CZ=2 impedance has invalid SBASE {sbase}; read as system-base p.u."
                ));
                (r, x)
            }
        }
        3 => {
            if !base_ok {
                warnings.push(&codes::READ_PSSE_VALUE_SUBSTITUTED, format!(
                    "PSS/E transformer {label} pair {pair}: CZ=3 impedance has invalid SBASE {sbase}; read as system-base p.u."
                ));
                return (r, x);
            }
            let r_pair = (r / 1_000_000.0) / sbase;
            let z_pair = x.abs();
            let x_pair = (z_pair.mul_add(z_pair, -(r_pair * r_pair)))
                .max(0.0)
                .sqrt()
                .copysign(x);
            let scale = system_base / sbase;
            (r_pair * scale, x_pair * scale)
        }
        other => {
            warnings.push(&codes::READ_PSSE_VALUE_UNSUPPORTED, format!(
                "PSS/E transformer {label} pair {pair}: unsupported CZ={other}; read impedance as system-base p.u."
            ));
            (r, x)
        }
    }
}

#[expect(clippy::too_many_arguments)]
fn convert_transformer_magnetizing_admittance(
    mag_g: f64,
    mag_b: f64,
    system_base: f64,
    winding_base: f64,
    bus_base_kv: f64,
    nominal_kv: f64,
    cm: i64,
    label: &str,
    warnings: &mut Diagnostics,
) -> (f64, f64) {
    match cm {
        // MAG1 and MAG2 are conductance and susceptance in p.u. on the system
        // MVA base and winding-1 bus voltage base.
        1 => (mag_g, mag_b),
        // MAG1 is no-load loss in watts and MAG2 is exciting current in p.u.
        // on the winding MVA base. Convert both to the system base, referred to
        // the winding-1 bus voltage base. This matches PSS/E and PowSybl's
        // TransformerConverter.
        2 => {
            let system_base_ok = system_base.is_finite() && system_base > 0.0;
            let winding_base_ok = winding_base.is_finite() && winding_base > 0.0;
            let bus_base_ok = bus_base_kv.is_finite() && bus_base_kv > 0.0;
            let nominal = if nominal_kv.is_finite() && nominal_kv > 0.0 {
                nominal_kv
            } else {
                bus_base_kv
            };
            if !(system_base_ok && winding_base_ok && bus_base_ok && nominal > 0.0) {
                warnings.push(&codes::READ_PSSE_VALUE_SUBSTITUTED, format!(
                    "PSS/E transformer {label}: CM=2 magnetizing data needs positive system, winding, bus-voltage, and nominal-voltage bases; read MAG1/MAG2 as p.u. admittance"
                ));
                return (mag_g, mag_b);
            }

            let voltage_scale = (bus_base_kv / nominal).powi(2);
            let g = mag_g / (1_000_000.0 * system_base) * voltage_scale;
            let y = mag_b * (winding_base / system_base) * voltage_scale;
            let b_squared = y.mul_add(y, -(g * g));
            if b_squared >= 0.0 {
                (g, -b_squared.sqrt())
            } else {
                warnings.push(&codes::READ_PSSE_VALUE_SUBSTITUTED, format!(
                    "PSS/E transformer {label}: CM=2 exciting current magnitude {y} is below conductance {g}; set magnetizing susceptance to 0"
                ));
                (g, 0.0)
            }
        }
        other => {
            warnings.push(&codes::READ_PSSE_VALUE_UNSUPPORTED, format!(
                "PSS/E transformer {label}: unsupported CM={other}; read MAG1/MAG2 as p.u. admittance"
            ));
            (mag_g, mag_b)
        }
    }
}

fn default_windv(cw: i64, bus: BusId, bus_base_kv: &BTreeMap<BusId, f64>) -> f64 {
    if cw == 2 {
        bus_base_kv
            .get(&bus)
            .copied()
            .filter(|v| *v > 0.0)
            .unwrap_or(1.0)
    } else {
        1.0
    }
}

fn winding_ratio(
    w: &[Cow<'_, str>],
    bus: BusId,
    cw: i64,
    bus_base_kv: &BTreeMap<BusId, f64>,
    label: &str,
    winding: &str,
    warnings: &mut Diagnostics,
) -> Result<f64> {
    let windv = num_at(w, 0, default_windv(cw, bus, bus_base_kv))?;
    let nomv = num_at(w, 1, 0.0)?;
    Ok(winding_ratio_value(
        windv,
        nomv,
        bus,
        cw,
        bus_base_kv,
        label,
        winding,
        "WINDV",
        warnings,
    ))
}

#[expect(clippy::too_many_arguments)]
fn winding_ratio_value(
    value: f64,
    nominal_kv: f64,
    bus: BusId,
    cw: i64,
    bus_base_kv: &BTreeMap<BusId, f64>,
    label: &str,
    winding: &str,
    field: &str,
    warnings: &mut Diagnostics,
) -> f64 {
    let base_kv = bus_base_kv.get(&bus).copied().unwrap_or(0.0);
    let needs_base = matches!(cw, 2 | 3);
    if needs_base && !(base_kv.is_finite() && base_kv > 0.0) {
        warnings.push(&codes::READ_PSSE_VALUE_SUBSTITUTED, format!(
            "PSS/E transformer {label} {winding}: CW={cw} needs a positive bus base kV for bus {bus}; read {field} as a p.u. tap ratio"
        ));
        return value;
    }
    match cw {
        1 => value,
        2 => value / base_kv,
        3 => {
            let nominal = if nominal_kv.is_finite() && nominal_kv > 0.0 {
                nominal_kv
            } else {
                base_kv
            };
            value * nominal / base_kv
        }
        other => {
            warnings.push(&codes::READ_PSSE_VALUE_UNSUPPORTED, format!(
                "PSS/E transformer {label} {winding}: unsupported CW={other}; read {field} as a p.u. tap ratio"
            ));
            value
        }
    }
}

#[expect(clippy::too_many_arguments)]
fn two_winding_tap(
    l1: &[Cow<'_, str>],
    l3: &[Cow<'_, str>],
    l4: &[Cow<'_, str>],
    from: BusId,
    to: BusId,
    cw: i64,
    bus_base_kv: &BTreeMap<BusId, f64>,
    warnings: &mut Diagnostics,
) -> Result<f64> {
    let label = transformer_label(l1);
    let ratio1 = winding_ratio(l3, from, cw, bus_base_kv, &label, "winding 1", warnings)?;
    let ratio2 = winding_ratio(l4, to, cw, bus_base_kv, &label, "winding 2", warnings)?;
    if ratio2.abs() <= f64::EPSILON {
        warnings.push(&codes::READ_PSSE_VALUE_SUBSTITUTED, format!(
            "PSS/E transformer {label}: winding 2 ratio is zero; used winding 1 ratio as the branch tap"
        ));
        Ok(ratio1)
    } else {
        Ok(ratio1 / ratio2)
    }
}

/// A terminator that also delimits a named section (`... END OF X DATA, BEGIN Y
/// DATA`), as opposed to the case header (whose first field is also `0`).
fn is_section_marker(line: &str) -> bool {
    if !is_terminator(line) {
        return false;
    }
    let u = line.to_ascii_uppercase();
    u.contains("END OF") || u.contains("BEGIN ") || u.contains("START OF ")
}

/// The upper-cased section name a `BEGIN <name> DATA` or `START OF <name> DATA`
/// marker introduces.
fn introduced_section_name(line: &str) -> Option<String> {
    let u = line.to_ascii_uppercase();
    let (start, prefix_len) = u
        .find("BEGIN ")
        .map(|idx| (idx, "BEGIN ".len()))
        .or_else(|| u.find("START OF ").map(|idx| (idx, "START OF ".len())))?;
    let start = start + prefix_len;
    let rest = &u[start..];
    let end = rest.find(" DATA")?;
    Some(rest[..end].trim().to_string())
}

/// The upper-cased section name in an `END OF <name> DATA` marker.
fn ended_section_name(line: &str) -> Option<String> {
    let u = line.to_ascii_uppercase();
    let start = u.find("END OF ")? + "END OF ".len();
    let rest = &u[start..];
    let end = rest.find(" DATA")?;
    Some(rest[..end].trim().to_string())
}

/// Warn about non-empty PSS/E sections the reader does not model (VSC and
/// multi-terminal DC, impedance correction, substation/node, multi-section line,
/// induction machine, FACTS, GNE, owner/zone, ...). Counts come from the parser
/// pass itself, so bare `0` terminators and malformed continuation boundaries are
/// classified the same way as the records that get skipped. No emission target
/// names revision 32, so a revision 32 source is never written back as its own
/// text and its skipped sections survive only in the retained module source.
fn warn_unmodeled_sections(
    totals: BTreeMap<String, usize>,
    raw_rev: u32,
    warnings: &mut Diagnostics,
) {
    let retention = if raw_rev == 32 {
        "retained only in the module source; fresh output uses revision 33 or later and drops it"
    } else {
        "preserved only in a same-format .raw echo, dropped on any other write"
    };
    for (name, rows) in totals {
        warnings.push(
            &codes::READ_PSSE_SECTION_UNSUPPORTED,
            format!("PSS/E {name} section ({rows} record line(s)) is not modeled: {retention}"),
        );
    }
}

fn drop_stale_control_pointers(net: &mut BalancedNetwork, warnings: &mut Diagnostics) {
    let bus_ids: BTreeSet<BusId> = net.buses().iter().map(|b| b.id).collect();
    let missing = |bus: BusId| !bus_ids.contains(&bus);

    for (idx, g) in net.generators_mut().iter_mut().enumerate() {
        let Some(bus) = g.regulated_bus.filter(|b| missing(*b)) else {
            continue;
        };
        warnings.push(&codes::READ_PSSE_REFERENCE_DROPPED, format!(
            "PSS/E GENERATOR DATA record {} at bus {}: IREG references missing bus id {}; dropped remote voltage control",
            idx + 1,
            g.bus,
            bus
        ));
        g.regulated_bus = None;
    }

    for (idx, br) in net.branches_mut().iter_mut().enumerate() {
        let Some(control) = br.control.as_mut() else {
            continue;
        };
        let Some(bus) = control.controlled_bus.filter(|b| missing(*b)) else {
            continue;
        };
        warnings.push(&codes::READ_PSSE_REFERENCE_DROPPED, format!(
            "PSS/E TRANSFORMER DATA record {} ({}-{}): CONT references missing bus id {}; dropped transformer control pointer",
            idx + 1,
            br.from,
            br.to,
            bus
        ));
        control.controlled_bus = None;
        control.controlled_bus_on_winding_side = false;
    }

    for (transformer_index, transformer) in net.transformers_3w_mut().iter_mut().enumerate() {
        for (winding_index, winding) in transformer.windings.iter_mut().enumerate() {
            let Some(control) = winding.control.as_mut() else {
                continue;
            };
            let Some(bus) = control.controlled_bus.filter(|bus| missing(*bus)) else {
                continue;
            };
            warnings.push(
                &codes::READ_PSSE_REFERENCE_DROPPED,
                format!(
                    "PSS/E TRANSFORMER DATA record {} winding {} at bus {}: CONT references missing bus id {}; dropped transformer control pointer",
                    transformer_index + 1,
                    winding_index + 1,
                    winding.bus,
                    bus
                ),
            );
            control.controlled_bus = None;
            control.controlled_bus_on_winding_side = false;
        }
    }

    for (idx, shunt) in net.shunts_mut().iter_mut().enumerate() {
        let Some(control) = shunt.control.as_mut() else {
            continue;
        };
        let Some(bus) = control.control_bus.filter(|b| missing(*b)) else {
            continue;
        };
        warnings.push(&codes::READ_PSSE_REFERENCE_DROPPED, format!(
            "PSS/E SWITCHED SHUNT DATA record {} at bus {}: SWREM references missing bus id {}; dropped switched shunt control pointer",
            idx + 1,
            shunt.bus,
            bus
        ));
        control.control_bus = None;
    }

    for (idx, area) in net.areas_mut().iter_mut().enumerate() {
        let Some(bus) = area.slack_bus.filter(|b| missing(*b)) else {
            continue;
        };
        warnings.push(&codes::READ_PSSE_REFERENCE_DROPPED, format!(
            "PSS/E AREA DATA record {} area {}: ISW references missing bus id {}; dropped area swing pointer",
            idx + 1,
            area.number,
            bus
        ));
        area.slack_bus = None;
    }
}

fn is_comment(line: &str) -> bool {
    line.starts_with("@!") || line.starts_with('@')
}

fn is_system_wide_record(f: &[Cow<'_, str>]) -> bool {
    matches!(
        f.first().map(|s| s.to_ascii_uppercase()),
        Some(first) if matches!(
            first.as_str(),
            "GENERAL" | "GAUSS" | "NEWTON" | "SOLVER" | "ADJUST" | "TYSL" | "RATING"
        )
    )
}

/// Parse a v34+ system-wide keyword line (`GENERAL`/`NEWTON`/`SOLVER`, each a
/// keyword then `KEY=VALUE` tokens) into the solver record. Every field that
/// has no typed home is diagnosed because only retained same format source can
/// carry it back out.
fn parse_solver_line(f: &[Cow<'_, str>], solver: &mut SolverParams, warnings: &mut Diagnostics) {
    let Some(keyword) = f.first().map(|s| s.to_ascii_uppercase()) else {
        return;
    };
    if !matches!(keyword.as_str(), "GENERAL" | "NEWTON" | "SOLVER") {
        warnings.push(
            &codes::READ_PSSE_FIELD_DROPPED,
            format!(
                "PSS/E system wide {keyword} record has no typed representation; retained only in the same format source"
            ),
        );
        return;
    }
    for tok in &f[1..] {
        if tok.is_empty() {
            continue;
        }
        let Some((key, val)) = tok.split_once('=') else {
            warnings.push(
                &codes::READ_PSSE_FIELD_DROPPED,
                format!(
                    "PSS/E system wide {keyword} token {tok:?} is not a KEY=VALUE field; retained only in the same format source"
                ),
            );
            continue;
        };
        let (key, val) = (key.trim().to_ascii_uppercase(), val.trim());
        match (keyword.as_str(), key.as_str()) {
            ("GENERAL", "THRSHZ") => {
                solver.zero_impedance_threshold = parse_system_wide_float(
                    &keyword,
                    &key,
                    val,
                    warnings,
                );
            }
            ("NEWTON", "TOLN") => {
                solver.newton_tolerance =
                    parse_system_wide_float(&keyword, &key, val, warnings);
            }
            ("NEWTON", "ITMXN") => {
                solver.max_iterations = parse_system_wide_u32(&keyword, &key, val, warnings);
            }
            ("SOLVER", "ACTAPS") => {
                solver.adjust_taps = parse_system_wide_enable(&keyword, &key, val, warnings);
            }
            ("SOLVER", "AREAIN") => {
                solver.adjust_area_interchange =
                    parse_system_wide_enable(&keyword, &key, val, warnings);
            }
            ("SOLVER", "PHSHFT") => {
                solver.adjust_phase_shift =
                    parse_system_wide_enable(&keyword, &key, val, warnings);
            }
            ("SOLVER", "DCTAPS") => {
                solver.adjust_dc_taps =
                    parse_system_wide_enable(&keyword, &key, val, warnings);
            }
            ("SOLVER", "SWSHNT") => {
                solver.adjust_switched_shunt =
                    parse_system_wide_enable(&keyword, &key, val, warnings);
            }
            _ => warnings.push(
                &codes::READ_PSSE_FIELD_DROPPED,
                format!(
                    "PSS/E system wide {keyword}.{key} has no typed representation; retained only in the same format source"
                ),
            ),
        }
    }
}

fn invalid_system_wide_value(
    keyword: &str,
    key: &str,
    value: &str,
    expected: &str,
    warnings: &mut Diagnostics,
) {
    warnings.push(
        &codes::READ_PSSE_VALUE_SUBSTITUTED,
        format!(
            "PSS/E system wide {keyword}.{key} value {value:?} is not {expected}; left the typed solver field unset"
        ),
    );
}

fn parse_system_wide_float(
    keyword: &str,
    key: &str,
    value: &str,
    warnings: &mut Diagnostics,
) -> Option<f64> {
    let parsed = value.parse::<f64>().ok().filter(|value| value.is_finite());
    if parsed.is_none() {
        invalid_system_wide_value(keyword, key, value, "a finite number", warnings);
    }
    parsed
}

fn parse_system_wide_u32(
    keyword: &str,
    key: &str,
    value: &str,
    warnings: &mut Diagnostics,
) -> Option<u32> {
    let parsed = value.parse::<f64>().ok().filter(|value| {
        value.is_finite() && value.fract() == 0.0 && *value >= 0.0 && *value <= f64::from(u32::MAX)
    });
    if let Some(parsed) = parsed {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        return Some(parsed as u32);
    }
    invalid_system_wide_value(
        keyword,
        key,
        value,
        "a nonnegative integer in the u32 range",
        warnings,
    );
    None
}

/// A `SOLVER` adjustment flag: numeric zero is disabled, any other finite
/// number is enabled, and the documented text spellings are accepted.
fn parse_system_wide_enable(
    keyword: &str,
    key: &str,
    value: &str,
    warnings: &mut Diagnostics,
) -> Option<bool> {
    if let Some(number) = value.parse::<f64>().ok().filter(|value| value.is_finite()) {
        return Some(number != 0.0);
    }
    match value.to_ascii_uppercase().as_str() {
        "ENABLED" | "ON" | "YES" => Some(true),
        "DISABLED" | "OFF" | "NO" => Some(false),
        _ => {
            invalid_system_wide_value(
                keyword,
                key,
                value,
                "a finite number or ENABLED/DISABLED, ON/OFF, or YES/NO",
                warnings,
            );
            None
        }
    }
}

/// Return the record body before an inline `/` comment, but only when the slash
/// is outside a single-quoted PSS/E field.
fn strip_inline_comment(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '/' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Split a PSS/E record into trimmed, unquoted fields, dropping a trailing
/// `/comment`. Commas keep empty fields (column position is significant — a
/// blank quoted name must not shift later columns), while whitespace also
/// separates fields outside quotes. PSS/E readers accept both delimiters in one
/// record, so `1 2, 3` is the same three fields as `1, 2, 3`.
///
/// Both paths trim after unquoting: PSS/E string columns are fixed width and
/// blank padded, so `' 1'` and `'1 '` name one device and `'BUS A       '` is
/// the name `BUS A`. The two delimiter styles used to disagree here, and the
/// same record body tokenized differently depending on which a producer chose.
pub(super) fn fields(line: &str) -> Vec<Cow<'_, str>> {
    let code = strip_inline_comment(line);
    let comma_delimited = code.contains(',');
    let mut out = Vec::with_capacity(if comma_delimited {
        code.bytes().filter(|b| *b == b',').count() + 1
    } else {
        8
    });
    // A bare field borrows `code`; only a field that contained quote
    // characters splices into an owned buffer, so the common numeric column
    // costs no allocation at all (#293).
    let mut owned: Option<String> = None;
    let mut field_start = 0usize;
    let mut segment_start = 0usize;
    let mut quoted = false;
    // A quoted span opened this field, so `''` holds its column instead of
    // vanishing and shifting every later one, as it does under commas.
    let mut was_quoted = false;
    let mut after_comma = false;
    for (i, c) in code.char_indices() {
        match c {
            '\'' => {
                owned
                    .get_or_insert_with(String::new)
                    .push_str(&code[segment_start..i]);
                segment_start = i + 1;
                quoted = !quoted;
                was_quoted = true;
            }
            ',' if !quoted => {
                let content = owned.is_some() || !code[field_start..i].trim().is_empty();
                if content || was_quoted {
                    push_field(&mut out, owned.take(), &code[segment_start..i]);
                } else if after_comma {
                    out.push(Cow::Borrowed(""));
                }
                field_start = i + 1;
                segment_start = field_start;
                was_quoted = false;
                after_comma = true;
            }
            c if c.is_whitespace() && !quoted => {
                let content = owned.is_some() || !code[field_start..i].trim().is_empty();
                if content || was_quoted {
                    push_field(&mut out, owned.take(), &code[segment_start..i]);
                    after_comma = false;
                }
                field_start = i + c.len_utf8();
                segment_start = field_start;
                was_quoted = false;
            }
            _ => {}
        }
    }
    let content = owned.is_some() || !code[field_start..].trim().is_empty();
    if was_quoted || content {
        push_field(&mut out, owned.take(), &code[segment_start..]);
    } else if after_comma {
        out.push(Cow::Borrowed(""));
    }
    out
}

/// Finish one field: splice the trailing raw segment onto any owned prefix,
/// trim, and push — borrowed when no quote character forced ownership.
fn push_field<'a>(out: &mut Vec<Cow<'a, str>>, owned: Option<String>, raw: &'a str) {
    match owned {
        Some(mut joined) => {
            joined.push_str(raw);
            let trimmed = joined.trim();
            if trimmed.len() == joined.len() {
                out.push(Cow::Owned(joined));
            } else {
                out.push(Cow::Owned(trimmed.to_string()));
            }
        }
        None => out.push(Cow::Borrowed(raw.trim())),
    }
}

fn bad_field(i: usize, tok: &str) -> Error {
    Error::FormatRead {
        format: FMT,
        message: format!("field {i} {tok:?} is not a finite number"),
    }
}

fn finite_field(i: usize, token: &str) -> Result<f64> {
    token
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| bad_field(i, token))
}

/// Field `i` as f64. Absent or empty → `default` (a genuinely optional column).
/// Present but unparseable → a hard error: a malformed number must not silently
/// become a plausible default (e.g. a garbled reactance collapsing to 0.0, which
/// would drop the branch from every matrix) and corrupt the result.
fn num_at(f: &[Cow<'_, str>], i: usize, default: f64) -> Result<f64> {
    match f.get(i).map(Cow::as_ref) {
        None | Some("") => Ok(default),
        Some(s) => finite_field(i, s),
    }
}
/// Field `i` as a bus id (parsed as f64 then truncated, the PSS/E convention);
/// the range policy is [`crate::format::id_from_f64`].
fn id_at(f: &[Cow<'_, str>], i: usize, default: usize) -> Result<usize> {
    match f.get(i).map(Cow::as_ref) {
        None | Some("") => Ok(default),
        Some(s) => {
            let v = finite_field(i, s)?;
            crate::format::id_from_f64(v, format_args!("field {i}")).map_err(|message| {
                Error::FormatRead {
                    format: FMT,
                    message,
                }
            })
        }
    }
}

/// PSS/E uses the sign of `CONT` to identify which side of a regulating
/// winding the controlled bus lies on. Return the absolute bus id and whether
/// the source value was negative.
fn signed_id_at(f: &[Cow<'_, str>], i: usize, default: usize) -> Result<(usize, bool)> {
    match f.get(i).map(Cow::as_ref) {
        None | Some("") => Ok((default, false)),
        Some(s) => {
            let value = finite_field(i, s)?;
            let negative = value.is_sign_negative() && value != 0.0;
            crate::format::id_from_f64(value.abs(), format_args!("field {i}"))
                .map(|id| (id, negative))
                .map_err(|message| Error::FormatRead {
                    format: FMT,
                    message,
                })
        }
    }
}
/// Field `i` as a status flag (nonzero = in service).
fn on_at(f: &[Cow<'_, str>], i: usize, default: bool) -> Result<bool> {
    match f.get(i).map(Cow::as_ref) {
        None | Some("") => Ok(default),
        Some(s) => finite_field(i, s).map(|value| value != 0.0),
    }
}
/// Field `i` as an integer code (bus type, etc.).
fn int_at(f: &[Cow<'_, str>], i: usize, default: i64) -> Result<i64> {
    match f.get(i).map(Cow::as_ref) {
        None | Some("") => Ok(default),
        // v34/35 exporters write integer fields in float form (`0.00` for `0`), so
        // parse through f64 and truncate, the way `id_at` already does.
        #[allow(clippy::cast_possible_truncation)]
        Some(s) => finite_field(i, s).map(|value| value as i64),
    }
}

fn bustype(code: i64) -> BusType {
    match code {
        2 => BusType::Pv,
        3 => BusType::Ref,
        4 => BusType::Isolated,
        _ => BusType::Pq,
    }
}

// The EVHI/EVLO equality below is an exact compare on purpose: the emergency
// band is typed only when its token differs from the normal-band token.
#[allow(clippy::float_cmp)]
fn read_bus(f: &[Cow<'_, str>], raw_rev: u32) -> Result<Bus> {
    // I, NAME, BASKV, IDE, AREA, ZONE, OWNER, VM, VA, then from revision 33
    // NVHI, NVLO, EVHI, EVLO. A revision 32 record ends at VA and its voltage
    // limits are the PSS/E defaults.
    let id = f
        .first()
        .and_then(|x| x.parse::<f64>().ok())
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "bus record missing numeric id (field I)".into(),
        })?;
    let id =
        crate::format::id_from_f64(id, "bus field I").map_err(|message| Error::FormatRead {
            format: FMT,
            message,
        })?;
    let name = f
        .get(1)
        .filter(|n| !n.is_empty())
        .map(|n| n.trim().to_string());
    let (vmax, vmin) = if raw_rev >= 33 {
        (num_at(f, 9, 1.1)?, num_at(f, 10, 0.9)?)
    } else {
        (1.1, 0.9)
    };
    // EVHI/EVLO default to the normal band when absent. Keep them typed only
    // when they actually differ, so the common equal-band case stays `None`.
    let (evhi, evlo) = if raw_rev >= 33 {
        (num_at(f, 11, vmax)?, num_at(f, 12, vmin)?)
    } else {
        (vmax, vmin)
    };
    let owner = int_at(f, 6, 1)?;
    let mut extras = Extras::new();
    if owner != 1 {
        extras.insert("psse_owner".into(), Value::from(owner));
    }
    Ok(Bus {
        id: BusId(id),
        kind: bustype(int_at(f, 3, 1)?),
        vm: num_at(f, 7, 1.0)?,
        va: num_at(f, 8, 0.0)?,
        base_kv: num_at(f, 2, 0.0)?,
        vmax,
        vmin,
        evhi: (evhi != vmax).then_some(evhi),
        evlo: (evlo != vmin).then_some(evlo),
        area: id_at(f, 4, 0)?,
        zone: id_at(f, 5, 0)?,
        name,
        uid: None,
        location: None,
        extras,
    })
}

/// Capture an element's circuit id (field `i`, a quoted 1-2 char string) into its
/// extras under `"id"`, so a round trip keeps the id and parallel devices on a bus
/// stay distinguishable. An id of `1` is the positional default the writer
/// allocates when no id is retained, so it restates nothing and is not kept —
/// parallel devices still round-trip, because the allocator hands `1` to the
/// device with no retained id and every explicit non-`1` id is replayed.
fn device_extras(f: &[Cow<'_, str>], i: usize) -> Extras {
    let mut extras = Extras::new();
    if let Some(id) = f
        .get(i)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "1")
    {
        extras.insert("id".into(), Value::String(id.to_string()));
    }
    extras
}

#[allow(clippy::float_cmp)]
fn retain_float_extra(
    extras: &mut Extras,
    fields: &[Cow<'_, str>],
    index: usize,
    key: &str,
    default: f64,
) -> Result<()> {
    let value = num_at(fields, index, default)?;
    if value != default {
        extras.insert(key.into(), jnum(value));
    }
    Ok(())
}

fn retain_integer_extra(
    extras: &mut Extras,
    fields: &[Cow<'_, str>],
    index: usize,
    key: &str,
    default: i64,
) -> Result<()> {
    let value = int_at(fields, index, default)?;
    if value != default {
        extras.insert(key.into(), Value::from(value));
    }
    Ok(())
}

fn retain_string_extra(extras: &mut Extras, fields: &[Cow<'_, str>], index: usize, key: &str) {
    if let Some(value) = fields
        .get(index)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        extras.insert(key.into(), Value::String(value.to_owned()));
    }
}

/// A load record's AREA and ZONE default to the values of its bus, and the
/// writer re-derives those defaults from the bus table. Only an assignment that
/// differs from the bus is information the record carries, so only that one
/// is retained as an extra; a matching value would otherwise appear as a
/// model change after a round trip through PSS/E and as a dropped field on
/// every cross-format emission.
fn drop_bus_default_area_zone(load: &mut Load, bus_area_zone: &BTreeMap<BusId, (usize, usize)>) {
    let Some(&(bus_area, bus_zone)) = bus_area_zone.get(&load.bus) else {
        return;
    };
    for (key, bus_value) in [("psse_area", bus_area), ("psse_zone", bus_zone)] {
        let matches_bus = extra_i64(&load.extras, key)
            .and_then(|value| usize::try_from(value).ok())
            .is_some_and(|value| value == bus_value);
        if matches_bus {
            load.extras.remove(key);
        }
    }
}

fn read_load(f: &[Cow<'_, str>], raw_rev: u32, warnings: &mut Diagnostics) -> Result<Load> {
    // I, ID, STATUS, AREA, ZONE, PL, QL, ...
    let bus = id_at(f, 0, 0)?;
    let id = f.get(1).map_or("", |s| s.trim());
    let pl = num_at(f, 5, 0.0)?;
    let ql = num_at(f, 6, 0.0)?;
    let ip = num_at(f, 7, 0.0)?;
    let iq = num_at(f, 8, 0.0)?;
    let yp = num_at(f, 9, 0.0)?;
    let yq = num_at(f, 10, 0.0)?;
    let mut extras = device_extras(f, 1);
    for (field, key) in [(3, "psse_area"), (4, "psse_zone")] {
        let value = id_at(f, field, 0)?;
        if value != 0 {
            extras.insert(key.into(), Value::from(value as u64));
        }
    }
    // A record with zero I/Y components states the constant-power pair alone,
    // and that pair is exactly the typed p/q the writer falls back to — so the
    // six components are retained only when one of the distributing terms is
    // nonzero and the split genuinely says more than the total.
    if [ip, iq, yp, yq].iter().any(|v| *v != 0.0) {
        for (key, value) in [
            ("psse_pl", pl),
            ("psse_ql", ql),
            ("psse_ip", ip),
            ("psse_iq", iq),
            ("psse_yp", yp),
            ("psse_yq", yq),
        ] {
            extras.insert(key.into(), jnum(value));
        }
    }
    // INTRPT joins the record at revision 33; a revision 32 record ends at
    // SCALE.
    let mut retained_integers = vec![(11, "psse_owner", 1_i64), (12, "psse_scal", 1_i64)];
    if raw_rev >= 33 {
        retained_integers.push((13, "psse_intrpt", 0_i64));
    }
    for (field, key, default) in retained_integers {
        let value = int_at(f, field, default)?;
        if value != default {
            extras.insert(key.into(), Value::from(value));
        }
    }
    if raw_rev >= 34 {
        for (field, key) in [(14, "psse_pdgen"), (15, "psse_qdgen")] {
            let value = num_at(f, field, 0.0)?;
            if value != 0.0 {
                extras.insert(key.into(), jnum(value));
            }
        }
        let flag = int_at(f, 16, 0)?;
        if flag != 0 {
            extras.insert("psse_flagstatus".into(), Value::from(flag));
        }
    }
    if raw_rev >= 35 {
        if let Some(loadtype) = f.get(17).map(|s| s.trim()).filter(|s| !s.is_empty()) {
            extras.insert("psse_loadtype".into(), Value::String(loadtype.to_string()));
        }
    }
    let scal = int_at(f, 12, 1)?;
    // LOADTYPE is the revision 35 trailing field; earlier layouts end before it.
    let load_type = if raw_rev >= 35 {
        f.get(17).and_then(|s| s.trim().parse::<i32>().ok())
    } else {
        None
    };
    let has_zip_components = [ip, iq, yp, yq].iter().any(|v| *v != 0.0);
    let voltage_model =
        (has_zip_components || scal != 1 || load_type.is_some()).then_some(LoadVoltageModel::Zip {
            p_constant_power: pl,
            q_constant_power: ql,
            p_constant_current: ip,
            q_constant_current: iq,
            p_constant_impedance: yp,
            q_constant_impedance: yq,
            v_nom: None,
            load_type,
            scaling: (scal != 1).then_some(scal as f64),
        });
    let has_load_options = extras.contains_key("psse_intrpt")
        || extras.contains_key("psse_pdgen")
        || extras.contains_key("psse_qdgen")
        || extras.contains_key("psse_flagstatus");
    if has_load_options {
        warnings.push(&codes::READ_PSSE_RETAINED_SOURCE_ONLY, format!(
            "PSS/E load at bus {bus} id {id:?}: interruptible/DG/flag fields are retained in extras"
        ));
    }
    Ok(Load {
        bus: BusId(bus),
        p: pl + ip + yp,
        q: ql + iq + yq,
        voltage_model,
        in_service: on_at(f, 2, true)?,
        uid: None,
        extras,
    })
}

fn read_shunt(f: &[Cow<'_, str>]) -> Result<Shunt> {
    // I, ID, STATUS, GL, BL
    Ok(Shunt {
        bus: BusId(id_at(f, 0, 0)?),
        g: num_at(f, 3, 0.0)?,
        b: num_at(f, 4, 0.0)?,
        in_service: on_at(f, 2, true)?,
        section_count: None,
        control: None,
        uid: None,
        extras: device_extras(f, 1),
    })
}

fn read_switched_shunt(
    f: &[Cow<'_, str>],
    rev: u32,
    warnings: &mut Diagnostics,
) -> Result<(Shunt, i32)> {
    // v33/34: I, MODSW, ADJM, STAT, VSWHI, VSWLO, SWREM, RMPCT, RMIDNT, BINIT(9),
    // then (Ni, Bi) step pairs. v35: I, ID, MODSW, ADJM, ST, VSWHI, VSWLO,
    // SWREG, NREG, RMPCT, RMIDNT, BINIT(11), then (Si, Ni, Bi) triples — the ID
    // shifts everything by one and NREG shifts the fields after SWREG by another.
    // BINIT becomes the shunt `b` (gs = 0); the mode, voltage band, regulated
    // bus, RMPCT, and step blocks ride on the switching-control record.
    let o = usize::from(rev >= 35);
    let o2 = 2 * o;
    let bus = id_at(f, 0, 0)?;
    let modsw = int_at(f, 1 + o, 1)?;
    let adjm = int_at(f, 2 + o, 0)?;
    let swrem = id_at(f, 6 + o, 0)?;
    // Step blocks follow BINIT; stop at the first empty (padding) block or the
    // end of the record. The v35 per-block status Si leads each triple; the
    // neutral ShuntBlock carries no block status, so keep the block either way.
    let mut blocks = Vec::new();
    let mut i = 10 + o2;
    let stride = 2 + o;
    let mut block_number = 1usize;
    while i + stride <= f.len() {
        if rev >= 35 {
            let status = int_at(f, i, 1)?;
            if status != 1 {
                warnings.push(
                    &codes::READ_PSSE_FIELD_DROPPED,
                    format!(
                        "PSS/E switched shunt at bus {bus} block {block_number} has S={status}; block status is not represented and the block was retained as enabled"
                    ),
                );
            }
        }
        let steps = int_at(f, i + o, 0)?;
        let b = num_at(f, i + o + 1, 0.0)?;
        if steps == 0 && b == 0.0 {
            break;
        }
        blocks.push(ShuntBlock {
            steps: steps.clamp(0, i64::from(u32::MAX)) as u32,
            g: 0.0,
            b,
        });
        i += stride;
        block_number += 1;
    }
    let mode = modsw_to_mode(modsw);
    let control = SwitchedShuntControl {
        mode,
        vhigh: num_at(f, 4 + o, 0.0)?,
        vlow: num_at(f, 5 + o, 0.0)?,
        control_bus: (swrem != 0).then_some(BusId(swrem)),
        regulating_terminal: None,
        rmpct: num_at(f, 7 + o2, 100.0)?,
        blocks,
    };
    let mut extras = if rev >= 35 {
        device_extras(f, 1)
    } else {
        Extras::new()
    };
    // PSS/E defines additional discrete control codes beyond the neutral
    // Discrete mode. Keep the source code when the typed mode is unchanged so
    // its validation rules survive fresh PSS/E emission.
    if modsw != mode_to_modsw(mode) {
        extras.insert("psse_modsw".into(), Value::from(modsw));
    }
    // ADJM changes how mixed reactor and capacitor blocks are combined. The
    // neutral shunt model has no equivalent field, so retain the PSS/E value
    // as source metadata for fresh RAW and RAWX emission.
    if adjm != 0 {
        extras.insert("psse_adjm".into(), Value::from(adjm));
    }
    retain_string_extra(&mut extras, f, 8 + o2, "psse_rmidnt");
    let regulating_node = if rev >= 35 {
        i32::try_from(int_at(f, 8, 0)?).map_err(|_| Error::FormatRead {
            format: FMT,
            message: "switched shunt NREG is outside the i32 range".into(),
        })?
    } else {
        0
    };
    Ok((
        Shunt {
            bus: BusId(bus),
            g: 0.0,
            b: num_at(f, 9 + o2, 0.0)?,
            in_service: on_at(f, 3 + o, true)?,
            section_count: None,
            control: Some(control),
            uid: None,
            // Keep the v35 shunt ID so it survives a round trip.
            extras,
        },
        regulating_node,
    ))
}

/// PSS/E `MODSW` switched-shunt mode code → neutral mode.
fn modsw_to_mode(modsw: i64) -> SwitchedShuntMode {
    match modsw {
        0 => SwitchedShuntMode::Locked,
        1 => SwitchedShuntMode::Continuous,
        _ => SwitchedShuntMode::Discrete,
    }
}

/// Neutral switched-shunt mode → PSS/E `MODSW` (the 0/1/2 codes; modes beyond
/// discrete collapse to 2).
fn mode_to_modsw(mode: SwitchedShuntMode) -> i64 {
    match mode {
        SwitchedShuntMode::Locked => 0,
        SwitchedShuntMode::Continuous => 1,
        SwitchedShuntMode::Discrete => 2,
    }
}

fn read_area(f: &[Cow<'_, str>]) -> Result<Area> {
    // I, ISW, PDES, PTOL, 'ARNAME'
    let isw = id_at(f, 1, 0)?;
    Ok(Area {
        number: id_at(f, 0, 0)?,
        slack_bus: (isw != 0).then_some(BusId(isw)),
        net_interchange: num_at(f, 2, 0.0)?,
        tolerance: num_at(f, 3, 0.0)?,
        name: f
            .get(4)
            .filter(|n| !n.trim().is_empty())
            .map(|n| n.trim().to_string()),
        uid: None,
        area_type: Some("ControlArea".to_owned()),
    })
}

fn retain_generator_float(
    properties: &mut BTreeMap<String, String>,
    f: &[Cow<'_, str>],
    index: usize,
    property: &str,
    default: f64,
) -> Result<()> {
    let Some(token) = f
        .get(index)
        .map(Cow::as_ref)
        .filter(|token| !token.is_empty())
    else {
        return Ok(());
    };
    let value = finite_field(index, token)?;
    // These fields are retained only to reproduce PSS/E data that the neutral
    // model does not name. A writer default is a restatement, not extra data.
    #[allow(clippy::float_cmp)]
    if value != default {
        properties.insert(property.to_owned(), value.to_string());
    }
    Ok(())
}

fn retain_generator_integer(
    properties: &mut BTreeMap<String, String>,
    f: &[Cow<'_, str>],
    index: usize,
    property: &str,
    default: i64,
) -> Result<()> {
    let Some(token) = f
        .get(index)
        .map(Cow::as_ref)
        .filter(|token| !token.is_empty())
    else {
        return Ok(());
    };
    #[allow(clippy::cast_possible_truncation)]
    let value = finite_field(index, token)? as i64;
    if value != default {
        properties.insert(property.to_owned(), value.to_string());
    }
    Ok(())
}

fn retain_generator_id(
    properties: &mut BTreeMap<String, String>,
    f: &[Cow<'_, str>],
    bus: BusId,
    used: &mut BTreeMap<BusId, BTreeSet<String>>,
) {
    let Some(id) = f.get(1).map(Cow::as_ref).filter(|id| !id.is_empty()) else {
        return;
    };

    let taken = used.get(&bus);
    let mut n = 1u32;
    let generated = loop {
        let candidate = n.to_string();
        if taken.is_none_or(|ids| !ids.contains(&candidate)) {
            break candidate;
        }
        n += 1;
    };

    // Track the id the writer will allocate. PSS/E requires ids to be unique
    // on a bus; a valid source-supplied id therefore wins when it is retained.
    super::allocate_circuit_id(Some(id), bus, used);
    if id != generated {
        properties.insert("psse_eqid".to_owned(), id.to_owned());
    }
}

fn read_gen(
    f: &[Cow<'_, str>],
    raw_rev: u32,
    generator_ids: &mut BTreeMap<BusId, BTreeSet<String>>,
) -> Result<(Generator, i32, BTreeMap<String, String>)> {
    // v33/34: I, ID, PG, QG, QT, QB, VS, IREG, MBASE(8), ..., STAT(14), ...,
    // PT(16), PB(17). v35 inserts NREG after IREG (and BASLOD after PB),
    // shifting MBASE through PB by one; v34 keeps the v33 layout.
    let o = usize::from(raw_rev >= 35);
    let bus = id_at(f, 0, 0)?;
    // IREG names the regulated bus. Zero means implicit own-terminal control;
    // an explicit same-bus IREG remains distinct so fresh output can retain it.
    let ireg = id_at(f, 7, 0)?;
    let regulating_node = if raw_rev >= 35 {
        i32::try_from(int_at(f, 8, 0)?).map_err(|_| Error::FormatRead {
            format: FMT,
            message: "generator NREG is outside the i32 range".into(),
        })?
    } else {
        0
    };
    let mut source_metadata = BTreeMap::new();
    retain_generator_id(&mut source_metadata, f, BusId(bus), generator_ids);
    for (index, property, default) in [
        (9 + o, "psse_zr", 0.0),
        (10 + o, "psse_zx", 1.0),
        (11 + o, "psse_rt", 0.0),
        (12 + o, "psse_xt", 0.0),
        (13 + o, "psse_gtap", 1.0),
        (15 + o, "psse_rmpct", 100.0),
    ] {
        retain_generator_float(&mut source_metadata, f, index, property, default)?;
    }
    let owner_start = if raw_rev >= 35 { 20 } else { 18 };
    if raw_rev >= 35 {
        retain_generator_integer(&mut source_metadata, f, 19, "psse_baslod", 0)?;
    }
    for owner in 0..4 {
        retain_generator_integer(
            &mut source_metadata,
            f,
            owner_start + owner * 2,
            &format!("psse_o{}", owner + 1),
            i64::from(owner == 0),
        )?;
        retain_generator_float(
            &mut source_metadata,
            f,
            owner_start + owner * 2 + 1,
            &format!("psse_f{}", owner + 1),
            1.0,
        )?;
    }
    retain_generator_integer(&mut source_metadata, f, owner_start + 8, "psse_wmod", 0)?;
    retain_generator_float(&mut source_metadata, f, owner_start + 9, "psse_wpf", 1.0)?;
    Ok((
        Generator {
            bus: BusId(bus),
            energy_source: GeneratorEnergySource::default(),
            pg: num_at(f, 2, 0.0)?,
            qg: num_at(f, 3, 0.0)?,
            qmax: num_at(f, 4, 0.0)?,
            qmin: num_at(f, 5, 0.0)?,
            vg: num_at(f, 6, 1.0)?,
            mbase: num_at(f, 8 + o, 100.0)?,
            in_service: on_at(f, 14 + o, true)?,
            pmax: num_at(f, 16 + o, 0.0)?,
            pmin: num_at(f, 17 + o, 0.0)?,
            cost: None,
            caps: Default::default(),
            voltage_regulation_on: true,
            regulating_terminal: None,
            regulated_bus: (ireg != 0).then_some(BusId(ireg)),
            active_power_control: None,
            uid: None,
        },
        regulating_node,
        source_metadata,
    ))
}

fn attach_generator_source_metadata(
    net: &mut BalancedNetwork,
    generator_metadata: Vec<BTreeMap<String, String>>,
) -> Result<()> {
    if generator_metadata.iter().all(BTreeMap::is_empty) {
        return Ok(());
    }
    net.assign_missing_component_ids();
    let component_ids = net
        .generators()
        .iter()
        .map(|generator| {
            let uid = generator.uid.as_deref().ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: "generator identity assignment failed".into(),
            })?;
            powerio_core::ComponentId::new("generator", uid).map_err(|error| Error::FormatRead {
                format: FMT,
                message: error.to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if net.detailed_connectivity().is_none() {
        *net.detailed_connectivity_mut() =
            Some(std::sync::Arc::new(DetailedConnectivity::default()));
    }
    let detailed = std::sync::Arc::make_mut(
        net.detailed_connectivity_mut()
            .as_mut()
            .expect("detailed connectivity was initialized"),
    );
    for (component, properties) in component_ids.into_iter().zip(generator_metadata) {
        if properties.is_empty() {
            continue;
        }
        if let Some(existing) = detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| metadata.component == component)
        {
            existing.properties.extend(properties);
        } else {
            detailed.component_metadata.push(ComponentMetadata {
                component,
                name: None,
                equipment_container: None,
                aliases: Vec::new(),
                external_identifiers: Vec::new(),
                properties,
                fictitious: false,
            });
        }
    }
    Ok(())
}

fn read_branch(f: &[Cow<'_, str>], raw_rev: u32) -> Result<Branch> {
    // v33: I, J, CKT, R, X, B, RATEA, RATEB, RATEC, GI,BI,GJ,BJ, ST(13)
    // v34 exports insert NAME before twelve rating columns, putting STAT after
    // GI/BI/GJ/BJ. v33 can still have a long owner/fraction tail, so the RAW
    // revision, not RATEA parseability, decides the long named layout.
    let named_record = raw_rev >= 34 && f.len() >= 24;
    let rating = if named_record { 7 } else { 6 };
    let status = if named_record { 23 } else { 13 };
    let shunt = if named_record { 19 } else { 9 };
    let br_b = num_at(f, 5, 0.0)?;
    let g_fr = num_at(f, shunt, 0.0)?;
    let b_fr_extra = num_at(f, shunt + 1, 0.0)?;
    let g_to = num_at(f, shunt + 2, 0.0)?;
    let b_to_extra = num_at(f, shunt + 3, 0.0)?;
    let b_fr = br_b / 2.0 + b_fr_extra;
    let b_to = br_b / 2.0 + b_to_extra;
    let from = BusId(id_at(f, 0, 0)?);
    let to = BusId(id_at(f, 1, 0)?);
    let name = if named_record {
        f.get(6)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    } else {
        // Revision 33 has no NAME field. Endpoints and CKT retain identity;
        // inventing a name here would add data on a fresh readback.
        None
    };
    let mut extras = device_extras(f, 2);
    retain_integer_extra(&mut extras, f, status + 1, "psse_met", 1)?;
    retain_float_extra(&mut extras, f, status + 2, "psse_len", 0.0)?;
    retain_psse_ownership(&mut extras, f, status + 3)?;
    Ok(Branch {
        name,
        from,
        to,
        r: num_at(f, 3, 0.0)?,
        x: num_at(f, 4, 0.0)?,
        b: b_fr + b_to,
        charging: Some(BranchCharging {
            g_fr,
            b_fr,
            g_to,
            b_to,
        }),
        rate_a: num_at(f, rating, 0.0)?,
        rate_b: num_at(f, rating + 1, 0.0)?,
        rate_c: num_at(f, rating + 2, 0.0)?,
        rating_sets: read_extra_branch_ratings(f, rating, named_record)?,
        current_ratings: None,
        tap: 0.0,
        shift: 0.0,
        in_service: on_at(f, status, true)?,
        angmin: -360.0,
        angmax: 360.0,
        control: None,
        solution: None,
        uid: None,
        route: None,
        // Capture CKT (field 2) so parallel circuits stay distinct on write-back.
        extras,
    })
}

fn read_system_switch(f: &[Cow<'_, str>]) -> Result<Switch> {
    // I, J, CKT, X, RATE1..RATE12, STAT, NSTAT, MET, TYPE, NAME
    let from = BusId(id_at(f, 0, 0)?);
    let to = BusId(id_at(f, 1, 0)?);
    let ckt = f
        .get(2)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("1");
    let mut switch = Switch::new(from, to, on_at(f, 16, true)?);
    let rate1 = num_at(f, 4, 0.0)?;
    switch.thermal_rating = (rate1 > 0.0).then_some(rate1);
    switch.uid = Some(format!("{from}-{to}-{ckt}"));
    if ckt != "1" {
        switch
            .extras
            .insert("psse_ckt".into(), Value::String(ckt.to_owned()));
    }
    retain_float_extra(&mut switch.extras, f, 3, "psse_xpu", 0.0)?;
    for rating in 2..=12 {
        retain_float_extra(
            &mut switch.extras,
            f,
            3 + rating,
            &format!("psse_rate{rating}"),
            0.0,
        )?;
    }
    retain_integer_extra(&mut switch.extras, f, 17, "psse_nstat", 1)?;
    retain_integer_extra(&mut switch.extras, f, 18, "psse_met", 1)?;
    retain_integer_extra(&mut switch.extras, f, 19, "psse_stype", 1)?;
    retain_string_extra(&mut switch.extras, f, 20, "psse_name");
    Ok(switch)
}

fn retain_transformer_main_extras(
    extras: &mut Extras,
    fields: &[Cow<'_, str>],
    raw_rev: u32,
) -> Result<()> {
    retain_integer_extra(extras, fields, 9, "psse_nmetr", 2)?;
    retain_psse_ownership(extras, fields, 12)?;
    // VECGRP joins the record at revision 33 and ZCOD at revision 35.
    if raw_rev >= 33 {
        retain_string_extra(extras, fields, 20, "psse_vecgrp");
    }
    if raw_rev >= 35 {
        retain_integer_extra(extras, fields, 21, "psse_zcod", 0)?;
    }
    Ok(())
}

fn retain_transformer_winding_extras(
    extras: &mut Extras,
    fields: &[Cow<'_, str>],
    raw_rev: u32,
    suffix: &str,
) -> Result<()> {
    let start = if raw_rev >= 34 { 23 } else { 13 };
    retain_integer_extra(extras, fields, start, &format!("psse_tab{suffix}"), 0)?;
    retain_float_extra(extras, fields, start + 1, &format!("psse_cr{suffix}"), 0.0)?;
    retain_float_extra(extras, fields, start + 2, &format!("psse_cx{suffix}"), 0.0)?;
    Ok(())
}

#[expect(clippy::too_many_arguments)]
fn read_transformer(
    l1: &[Cow<'_, str>],
    l2: &[Cow<'_, str>],
    l3: &[Cow<'_, str>],
    l4: &[Cow<'_, str>],
    raw_rev: u32,
    system_base: f64,
    bus_base_kv: &BTreeMap<BusId, f64>,
    warnings: &mut Diagnostics,
) -> Result<(Branch, i32)> {
    // l1: I, J, K, CKT, CW, CZ, CM, MAG1, MAG2, NMETR, NAME, STAT(11)
    // l2: R1-2, X1-2, SBASE1-2
    // l3 at v33: WINDV1, NOMV1, ANG1, RATA1, RATB1, RATC1, COD1(6), CONT1,
    //     RMA1, RMI1, VMA1, VMI1, NTP1(12), ...
    // v34/35 widen the winding line to twelve ratings (RATE1..3 succeed
    // RATA/B/C in place) and insert NODE after CONT: COD1 lands at 15, CONT1
    // at 16, and RMA1..NTP1 at 18..22.
    // A nonzero control code COD1 marks a regulating winding; capture its limits
    // and regulated bus, else leave the branch's control unset.
    let (cw, cz) = transformer_basis_codes(l1)?;
    let from = BusId(id_at(l1, 0, 0)?);
    let to = BusId(id_at(l1, 1, 0)?);
    let sbase = num_at(l2, 2, system_base)?;
    let label = transformer_label(l1);
    let (r, x) = convert_transformer_impedance(
        num_at(l2, 0, 0.0)?,
        num_at(l2, 1, 0.0)?,
        sbase,
        system_base,
        cz,
        &label,
        "1-2",
        warnings,
    );
    let tap = two_winding_tap(l1, l3, l4, from, to, cw, bus_base_kv, warnings)?;
    let modern = raw_rev >= 34;
    let (control, control_node) = read_transformer_control(
        l3,
        raw_rev,
        sbase,
        from,
        cw,
        bus_base_kv,
        &label,
        "winding 1",
        warnings,
    )?;
    let bus_kv = bus_base_kv.get(&from).copied().unwrap_or(0.0);
    let (mag_g, mag_b) = convert_transformer_magnetizing_admittance(
        num_at(l1, 7, 0.0)?,
        num_at(l1, 8, 0.0)?,
        system_base,
        sbase,
        bus_kv,
        num_at(l3, 1, bus_kv)?,
        int_at(l1, 6, 1)?,
        &label,
        warnings,
    );
    let mut extras = device_extras(l1, 3);
    retain_transformer_main_extras(&mut extras, l1, raw_rev)?;
    retain_transformer_winding_extras(&mut extras, l3, raw_rev, "")?;
    Ok((
        Branch {
            name: l1
                .get(10)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            from,
            to,
            r,
            x,
            b: mag_b,
            charging: Some(BranchCharging {
                g_fr: mag_g,
                b_fr: mag_b,
                g_to: 0.0,
                b_to: 0.0,
            }),
            rate_a: num_at(l3, 3, 0.0)?,
            rate_b: num_at(l3, 4, 0.0)?,
            rate_c: num_at(l3, 5, 0.0)?,
            rating_sets: read_extra_branch_ratings(l3, 3, modern)?,
            current_ratings: None,
            tap,
            shift: num_at(l3, 2, 0.0)?,
            in_service: on_at(l1, 11, true)?,
            angmin: -360.0,
            angmax: 360.0,
            control,
            solution: None,
            uid: None,
            route: None,
            extras,
        },
        control_node,
    ))
}

/// PSS/E transformer control code `COD` → neutral control mode. The sign encodes
/// an enable/disable flag PSS/E carries separately; only the magnitude selects
/// the regulation kind.
fn cod_to_mode(cod: i64) -> TransformerControlMode {
    // `int_at` parses through f64 and saturates, so an extreme COD field can
    // reach i64::MIN, whose magnitude exceeds i64::MAX; `cod.abs()` would
    // overflow (panic under overflow checks). unsigned_abs never overflows,
    // and only the documented magnitudes 1..=5 select a nonfixed mode.
    match cod.unsigned_abs() {
        1 => TransformerControlMode::Voltage,
        2 => TransformerControlMode::ReactiveFlow,
        3 => TransformerControlMode::ActiveFlow,
        4 => TransformerControlMode::DcLineQuantity,
        5 => TransformerControlMode::AsymmetricActiveFlow,
        _ => TransformerControlMode::Fixed,
    }
}

/// Neutral control mode → the magnitude of PSS/E `COD`.
fn mode_to_cod(mode: TransformerControlMode) -> i64 {
    match mode {
        TransformerControlMode::Fixed => 0,
        TransformerControlMode::Voltage => 1,
        TransformerControlMode::ReactiveFlow => 2,
        TransformerControlMode::ActiveFlow => 3,
        TransformerControlMode::DcLineQuantity => 4,
        TransformerControlMode::AsymmetricActiveFlow => 5,
    }
}

#[allow(clippy::float_cmp)]
#[expect(clippy::too_many_arguments)]
fn read_transformer_control(
    winding: &[Cow<'_, str>],
    raw_rev: u32,
    mva_base: f64,
    bus: BusId,
    cw: i64,
    bus_base_kv: &BTreeMap<BusId, f64>,
    label: &str,
    winding_name: &str,
    warnings: &mut Diagnostics,
) -> Result<(Option<TransformerControl>, i32)> {
    let (cod_i, cont_i, node_i, rma_i) = if raw_rev >= 34 {
        (15, 16, Some(17), 18)
    } else {
        (6, 7, None, 8)
    };
    let cod = int_at(winding, cod_i, 0)?;
    let (cont, controlled_bus_on_winding_side) = signed_id_at(winding, cont_i, 0)?;
    let node = node_i.map_or(Ok(0), |index| int_at(winding, index, 0))?;
    let mode = cod_to_mode(cod);
    if cod != 0 && mode == TransformerControlMode::Fixed {
        warnings.push(
            &codes::READ_PSSE_VALUE_UNSUPPORTED,
            format!(
                "PSS/E transformer {label} {winding_name}: unsupported COD={cod}; read its remaining fields with fixed control"
            ),
        );
    }
    let raw_tap_max = num_at(winding, rma_i, 1.1)?;
    let raw_tap_min = num_at(winding, rma_i + 1, 0.9)?;
    let mut tap_max = raw_tap_max;
    let mut tap_min = raw_tap_min;
    if matches!(
        mode,
        TransformerControlMode::Voltage | TransformerControlMode::ReactiveFlow
    ) {
        let nominal_kv = num_at(winding, 1, 0.0)?;
        tap_max = winding_ratio_value(
            tap_max,
            nominal_kv,
            bus,
            cw,
            bus_base_kv,
            label,
            winding_name,
            "RMA",
            warnings,
        );
        tap_min = winding_ratio_value(
            tap_min,
            nominal_kv,
            bus,
            cw,
            bus_base_kv,
            label,
            winding_name,
            "RMI",
            warnings,
        );
    }
    let band_max = num_at(winding, rma_i + 2, 1.1)?;
    let band_min = num_at(winding, rma_i + 3, 0.9)?;
    let ntp = int_at(winding, rma_i + 4, 33)?.clamp(0, i64::from(u32::MAX)) as u32;
    // CNXA joins the winding line at revision 33; a revision 32 line ends at CX.
    let winding_connection_angle = if raw_rev >= 33 {
        num_at(winding, rma_i + 8, 0.0)?
    } else {
        0.0
    };
    let present = cod != 0
        || cont != 0
        || node != 0
        || raw_tap_max != 1.1
        || raw_tap_min != 0.9
        || band_max != 1.1
        || band_min != 0.9
        || ntp != 33
        || winding_connection_angle != 0.0;
    Ok((
        present.then_some(TransformerControl {
            mode,
            enabled: cod > 0,
            controlled_bus: (cont != 0).then_some(BusId(cont)),
            controlled_bus_on_winding_side,
            regulating_terminal: None,
            tap_min,
            tap_max,
            band_min,
            band_max,
            ntp,
            mva_base,
            winding_connection_angle: (mode == TransformerControlMode::AsymmetricActiveFlow
                || winding_connection_angle != 0.0)
                .then_some(winding_connection_angle),
        }),
        i32::try_from(node).map_err(|_| Error::FormatRead {
            format: FMT,
            message: format!("transformer {winding_name} NODE is outside the i32 range"),
        })?,
    ))
}

/// Read a 5-line 3-winding transformer record into a [`Transformer3W`].
// One five-line PSS/E record is decoded as a unit so its basis codes and
// diagnostics remain shared across all three windings.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn read_transformer_3w(
    l1: &[Cow<'_, str>],
    l2: &[Cow<'_, str>],
    l3: &[Cow<'_, str>],
    l4: &[Cow<'_, str>],
    l5: &[Cow<'_, str>],
    raw_rev: u32,
    system_base: f64,
    bus_base_kv: &BTreeMap<BusId, f64>,
    warnings: &mut Diagnostics,
) -> Result<(Transformer3W, [i32; 3])> {
    // l1: I, J, K, CKT, CW, CZ, CM, MAG1, MAG2, NMETR, NAME, STAT(11)
    // l2: R1-2,X1-2,SBASE1-2, R2-3,X2-3,SBASE2-3, R3-1,X3-1,SBASE3-1, VMSTAR, ANSTAR
    // l3/l4/l5: WINDVk, NOMVk, ANGk, RATAk, RATBk, RATCk, ...
    let (cw, cz) = transformer_basis_codes(l1)?;
    let label = transformer_label(l1);
    let buses = [
        BusId(id_at(l1, 0, 0)?),
        BusId(id_at(l1, 1, 0)?),
        BusId(id_at(l1, 2, 0)?),
    ];
    let z = {
        let mut imp = |off: usize, pair: &str| -> Result<Impedance> {
            let sbase = num_at(l2, off + 2, system_base)?;
            let (r, x) = convert_transformer_impedance(
                num_at(l2, off, 0.0)?,
                num_at(l2, off + 1, 0.0)?,
                sbase,
                system_base,
                cz,
                &label,
                pair,
                warnings,
            );
            Ok(Impedance {
                r,
                x,
                base_mva: sbase,
            })
        };
        [imp(0, "1-2")?, imp(3, "2-3")?, imp(6, "3-1")?]
    };
    let windings = {
        let mut winding = |idx: usize, w: &[Cow<'_, str>]| -> Result<(Winding, i32)> {
            let bus = buses[idx];
            let winding_name = match idx {
                0 => "winding 1",
                1 => "winding 2",
                _ => "winding 3",
            };
            let tap = winding_ratio(w, bus, cw, bus_base_kv, &label, winding_name, warnings)?;
            let (control, node) = read_transformer_control(
                w,
                raw_rev,
                system_base,
                bus,
                cw,
                bus_base_kv,
                &label,
                winding_name,
                warnings,
            )?;
            if control
                .as_ref()
                .is_some_and(|control| control.mode == TransformerControlMode::DcLineQuantity)
            {
                return Err(Error::FormatRead {
                    format: FMT,
                    message: format!(
                        "transformer {label} {winding_name} uses COD 4 DC line quantity control, which is valid only for two winding transformers"
                    ),
                });
            }
            Ok((
                Winding {
                    bus,
                    tap,
                    shift: num_at(w, 2, 0.0)?,
                    nominal_kv: num_at(w, 1, 0.0)?,
                    rate_a: num_at(w, 3, 0.0)?,
                    rate_b: num_at(w, 4, 0.0)?,
                    rate_c: num_at(w, 5, 0.0)?,
                    control,
                },
                node,
            ))
        };
        let (winding1, node1) = winding(0, l3)?;
        let (winding2, node2) = winding(1, l4)?;
        let (winding3, node3) = winding(2, l5)?;
        ([winding1, winding2, winding3], [node1, node2, node3])
    };
    let (windings, control_nodes) = windings;
    let winding_base = num_at(l2, 2, system_base)?;
    let bus_kv = bus_base_kv.get(&buses[0]).copied().unwrap_or(0.0);
    let (mag_g, mag_b) = convert_transformer_magnetizing_admittance(
        num_at(l1, 7, 0.0)?,
        num_at(l1, 8, 0.0)?,
        system_base,
        winding_base,
        bus_kv,
        num_at(l3, 1, bus_kv)?,
        int_at(l1, 6, 1)?,
        &label,
        warnings,
    );
    let mut extras = device_extras(l1, 3);
    retain_transformer_main_extras(&mut extras, l1, raw_rev)?;
    for (winding, suffix) in [(l3, "1"), (l4, "2"), (l5, "3")] {
        retain_transformer_winding_extras(&mut extras, winding, raw_rev, suffix)?;
    }
    Ok((
        Transformer3W {
            windings,
            z,
            star_vm: num_at(l2, 9, 1.0)?,
            star_va: num_at(l2, 10, 0.0)?,
            mag_g,
            mag_b,
            // STAT 0 = out of service; 1-4 mark which windings are in service. Treat
            // any nonzero status as the transformer being in service.
            in_service: int_at(l1, 11, 1)? != 0,
            name: l1
                .get(10)
                .filter(|n| !n.is_empty())
                .map(|n| n.trim().to_string()),
            uid: None,
            extras,
        },
        control_nodes,
    ))
}

/// Read a 3-line two-terminal DC line record into an [`Hvdc`].
///
/// The control line `l1` gives the operating mode (`MDC`), the DC line resistance
/// (`RDC`), the power/current demand (`SETVL`), and the scheduled DC voltage
/// (`VSCHD`). The rectifier and inverter lines' first field is the AC terminal
/// bus, which becomes the HVDC from/to. The converter detail beyond the buses
/// (firing angles, converter transformer taps) is retained in extras for a
/// faithful write-back, not modeled electrically; no reactive output is read.
///
/// The two ends differ by the line's own drop. `SETVL` under `MDC = 1` is a
/// power demand in MW — measured at the rectifier, or at the inverter when
/// negative — and under `MDC = 2` a current demand in amps, which the
/// scheduled voltage prices into power. The DC current `SETVL/VSCHD` (kA at
/// kV) makes the line loss `I²·RDC` MW exactly, so the received power is the
/// demand minus that drop. A record scheduling no voltage gives no current to
/// price, and both ends read as the demand.
///
/// Detail matching what the writer would synthesize anyway — the positional
/// `DC{n}` name, an MDC that only restates the service status, zero
/// RDC/VSCHD, and the default converter tails — restates nothing and is not
/// kept, so a record powerio itself wrote reads back with empty extras and
/// rewrites identically.
// One block per two-terminal DC field; splitting it would scatter a record
// that reads end to end.
#[expect(clippy::too_many_lines)]
fn read_dc_line(
    l1: &[Cow<'_, str>],
    rect: &[Cow<'_, str>],
    inv: &[Cow<'_, str>],
    index: usize,
    warnings: &mut Diagnostics,
) -> Result<Hvdc> {
    let mdc = int_at(l1, 1, 1)?;
    let rdc = num_at(l1, 2, 0.0)?;
    let setvl = num_at(l1, 3, 0.0)?;
    let vschd = num_at(l1, 4, 0.0)?;
    // Which end the record measured its demand at, so the writer can state it
    // the same way. Without this a negative SETVL comes back positive and the
    // re-read prices the drop off the rectifier power instead of the inverter
    // power, moving the received power by the difference between the two.
    let mut measured_at_inverter = false;
    let mut unpriceable_current = false;
    let (pf, pt) = match (mdc, vschd > 0.0) {
        // Current demand: SETVL amps -> kA; power at the rectifier is V·I.
        (2, true) => {
            let i = setvl / 1000.0;
            let pf = vschd * i;
            (pf, pf - i * i * rdc)
        }
        // Current demand with no scheduled voltage: there is nothing to price
        // the amps with. Reading them as MW is how a 2000 A schedule becomes a
        // 2000 MW line, so the operating point reads as zero and the record is
        // retained verbatim for the write back.
        (2, false) => {
            unpriceable_current = true;
            (0.0, 0.0)
        }
        // Power demand measured at the inverter: the rectifier supplies it
        // plus the drop.
        (1, true) if setvl < 0.0 => {
            measured_at_inverter = true;
            let pt = -setvl;
            let i = pt / vschd;
            (pt + i * i * rdc, pt)
        }
        // Power demand at the rectifier: the inverter receives it minus the
        // drop.
        (1, true) => {
            let i = setvl / vschd;
            (setvl, setvl - i * i * rdc)
        }
        // No scheduled voltage under a power mode: no current to price the
        // drop with, and SETVL is already MW.
        _ => (setvl, setvl),
    };
    // RDC, SETVL and VSCHD are record fields, so the arithmetic above is
    // arithmetic on untrusted numbers: a huge resistance or a subnormal
    // voltage schedule overflows the squared current, and either end can come
    // back non-finite. A non-finite setpoint is not a setpoint, and it would
    // travel into every matrix and every writer downstream.
    let (pf, pt) =
        if pf.is_finite() && pt.is_finite() {
            (pf, pt)
        } else {
            warnings.push(&codes::READ_PSSE_VALUE_SUBSTITUTED, format!(
            "two-terminal DC record {} states a drop model that does not evaluate to a finite \
             power; both ends read as zero",
            index + 1
        ));
            // The end the demand was measured at is a claim about a drop model
            // that was refused, and the writer would restate it as `-0.0`.
            measured_at_inverter = false;
            (0.0, 0.0)
        };
    if unpriceable_current {
        warnings.push(
            &codes::READ_PSSE_VALUE_SUBSTITUTED,
            format!(
                "two-terminal DC record {} schedules a current with no scheduled voltage; the \
             demand cannot be priced into power and both ends read as zero",
                index + 1
            ),
        );
    }
    let mut extras = Extras::new();
    if let Some(name) = l1
        .first()
        .filter(|n| !n.is_empty() && **n != format!("DC{}", index + 1))
    {
        extras.insert("psse_dc_name".into(), Value::String(name.to_string()));
    }
    // MDC 0 and 1 restate the service status the writer derives on its own;
    // any other mode (2 = current demand in amps) is real control data.
    if !(0..=1).contains(&mdc) {
        extras.insert("psse_dc_mdc".into(), Value::from(mdc));
    }
    if rdc != 0.0 {
        extras.insert("psse_dc_rdc".into(), jnum(rdc));
    }
    if vschd != 0.0 {
        extras.insert("psse_dc_vschd".into(), jnum(vschd));
    }
    if measured_at_inverter {
        extras.insert(SETVL_AT_INVERTER.into(), Value::Bool(true));
    }
    if unpriceable_current {
        extras.insert("psse_dc_setvl".into(), jnum(setvl));
    }
    for (key, fields, start, default) in [
        ("psse_dc_control_tail", l1, 5, DEFAULT_CONTROL_TAIL),
        ("psse_dc_rectifier_tail", rect, 1, DEFAULT_CONVERTER_TAIL),
        ("psse_dc_inverter_tail", inv, 1, DEFAULT_CONVERTER_TAIL),
    ] {
        if !tail_is_default(fields, start, default) {
            extras.insert(key.into(), tail_array(fields, start));
        }
    }
    Ok(Hvdc {
        from: BusId(id_at(rect, 0, 0)?),
        to: BusId(id_at(inv, 0, 0)?),
        in_service: mdc != 0,
        pf,
        pt,
        qf: 0.0,
        qt: 0.0,
        vf: 1.0,
        vt: 1.0,
        pmin: 0.0,
        pmax: pf.abs().max(pt.abs()),
        qminf: 0.0,
        qmaxf: 0.0,
        qmint: 0.0,
        qmaxt: 0.0,
        loss0: 0.0,
        loss1: 0.0,
        resistance_ohm: None,
        nominal_voltage_kv: None,
        converters_mode: None,
        converter1: None,
        converter2: None,
        cost: None,
        uid: None,
        extras,
    })
}

/// Whether the tail of `f` from `start` states exactly the writer's `default`
/// tail, token for token (quotes stripped, as the field splitter leaves them).
/// A matching tail restates the synthesized neutral shape and needs no
/// retention; any textual difference — a real value, extra columns, even a
/// different numeric spelling — keeps the tail, conservatively.
fn tail_is_default(f: &[Cow<'_, str>], start: usize, default: &str) -> bool {
    let defaults = default
        .split(", ")
        .map(|t| t.trim_matches('\''))
        .collect::<Vec<_>>();
    let mut tail = f.iter().skip(start).map(Cow::as_ref).collect::<Vec<_>>();
    // Revision 34 and later state one column more in a two-terminal DC
    // converter tail, the bridge count between ICR and IFR. A record whose
    // bridge count is zero and whose other fields are the default states the
    // same converter as the shorter revision 33 default, and retaining it as
    // an extra would make every later hop report the loss of nothing.
    if tail.len() == defaults.len() + 1
        && default == DEFAULT_CONVERTER_TAIL
        && tail[CONVERTER_TAIL_BRIDGE_INDEX] == "0"
    {
        tail.remove(CONVERTER_TAIL_BRIDGE_INDEX);
    }
    tail == defaults
}

/// The fields of `f` from index `start` as a JSON string array (for extras).
fn tail_array(f: &[Cow<'_, str>], start: usize) -> Value {
    Value::Array(
        f.iter()
            .skip(start)
            .map(|s| Value::String(s.to_string()))
            .collect(),
    )
}

/// A string-valued DC extra.
fn dc_str(extras: &Extras, key: &str) -> Option<String> {
    extras.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// An integer-valued DC extra.
fn dc_int(extras: &Extras, key: &str) -> Option<i64> {
    extras.get(key).and_then(Value::as_i64)
}

/// A float-valued DC extra.
fn dc_f64(extras: &Extras, key: &str) -> Option<f64> {
    extras.get(key).and_then(Value::as_f64)
}

/// A finite float extra carried by a read side passthrough field.
fn extra_f64(extras: &Extras, key: &str) -> Option<f64> {
    extras
        .get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
}

/// An integer extra carried by a read side passthrough field.
fn extra_i64(extras: &Extras, key: &str) -> Option<i64> {
    extras.get(key).and_then(Value::as_i64)
}

fn psse_ownership(extras: &Extras) -> [(i64, f64); 4] {
    [
        (
            extra_i64(extras, "psse_o1").unwrap_or(1),
            extra_f64(extras, "psse_f1").unwrap_or(1.0),
        ),
        (
            extra_i64(extras, "psse_o2").unwrap_or(0),
            extra_f64(extras, "psse_f2").unwrap_or(1.0),
        ),
        (
            extra_i64(extras, "psse_o3").unwrap_or(0),
            extra_f64(extras, "psse_f3").unwrap_or(1.0),
        ),
        (
            extra_i64(extras, "psse_o4").unwrap_or(0),
            extra_f64(extras, "psse_f4").unwrap_or(1.0),
        ),
    ]
}

fn retain_psse_ownership(extras: &mut Extras, fields: &[Cow<'_, str>], start: usize) -> Result<()> {
    for (offset, key, default) in [
        (0, "psse_o1", 1),
        (2, "psse_o2", 0),
        (4, "psse_o3", 0),
        (6, "psse_o4", 0),
    ] {
        retain_integer_extra(extras, fields, start + offset, key, default)?;
    }
    for (offset, key) in [
        (1, "psse_f1"),
        (3, "psse_f2"),
        (5, "psse_f3"),
        (7, "psse_f4"),
    ] {
        retain_float_extra(extras, fields, start + offset, key, 1.0)?;
    }
    Ok(())
}

fn same_load_total(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn typed_psse_scal(l: &Load, id: &str, warnings: &mut Diagnostics) -> Option<i64> {
    let Some(LoadVoltageModel::Zip {
        scaling: Some(scaling),
        ..
    }) = &l.voltage_model
    else {
        return None;
    };
    let scaling = *scaling;
    if !scaling.is_finite() {
        warnings.push(&codes::READ_PSSE_VALUE_SUBSTITUTED, format!(
            "PSS/E load at bus {} id {id:?}: non-finite typed scaling has no SCAL value; used source/default SCAL",
            l.bus
        ));
        return None;
    }
    let rounded = scaling.round();
    if (scaling - rounded).abs() > 1e-9 || rounded < i64::MIN as f64 || rounded > i64::MAX as f64 {
        warnings.push(&codes::READ_PSSE_VALUE_SUBSTITUTED, format!(
            "PSS/E load at bus {} id {id:?}: non-integer typed scaling {scaling} has no SCAL value; used source/default SCAL",
            l.bus
        ));
        return None;
    }
    Some(rounded as i64)
}

fn typed_psse_load_type(model: &LoadVoltageModel) -> Option<String> {
    match model {
        LoadVoltageModel::Zip {
            load_type: Some(load_type),
            ..
        } => Some(load_type.to_string()),
        _ => None,
    }
}

fn load_components_for_write(
    l: &Load,
    id: &str,
    warnings: &mut Diagnostics,
) -> (f64, f64, f64, f64, f64, f64) {
    if let Some(LoadVoltageModel::Zip {
        p_constant_power,
        q_constant_power,
        p_constant_current,
        q_constant_current,
        p_constant_impedance,
        q_constant_impedance,
        v_nom,
        ..
    }) = &l.voltage_model
    {
        if same_load_total(
            p_constant_power + p_constant_current + p_constant_impedance,
            l.p,
        ) && same_load_total(
            q_constant_power + q_constant_current + q_constant_impedance,
            l.q,
        ) {
            if v_nom.is_some() {
                warnings.push(&F.field_dropped, format!(
                    "PSS/E load at bus {} id {id:?}: nominal voltage has no load record field; dropped",
                    l.bus
                ));
            }
            return (
                *p_constant_power,
                *q_constant_power,
                *p_constant_current,
                *q_constant_current,
                *p_constant_impedance,
                *q_constant_impedance,
            );
        }
        warnings.push(
            &F.value_substituted,
            format!(
                "PSS/E load at bus {} id {id:?}: stale voltage model components did not match \
             typed p/q; wrote typed p/q as constant power",
                l.bus
            ),
        );
        return (l.p, l.q, 0.0, 0.0, 0.0, 0.0);
    }
    if matches!(l.voltage_model, Some(LoadVoltageModel::Exponential { .. })) {
        warnings.push(&F.field_dropped, format!(
            "PSS/E load at bus {} id {id:?}: exponential voltage model has no load record fields; wrote typed p/q as constant power",
            l.bus
        ));
        return (l.p, l.q, 0.0, 0.0, 0.0, 0.0);
    }

    let pl = extra_f64(&l.extras, "psse_pl").unwrap_or(l.p);
    let ql = extra_f64(&l.extras, "psse_ql").unwrap_or(l.q);
    let ip = extra_f64(&l.extras, "psse_ip").unwrap_or(0.0);
    let iq = extra_f64(&l.extras, "psse_iq").unwrap_or(0.0);
    let yp = extra_f64(&l.extras, "psse_yp").unwrap_or(0.0);
    let yq = extra_f64(&l.extras, "psse_yq").unwrap_or(0.0);
    let has_components = [
        "psse_pl", "psse_ql", "psse_ip", "psse_iq", "psse_yp", "psse_yq",
    ]
    .iter()
    .any(|key| l.extras.contains_key(*key));
    if has_components
        && (!same_load_total(pl + ip + yp, l.p) || !same_load_total(ql + iq + yq, l.q))
    {
        warnings.push(
            &F.value_substituted,
            format!(
                "PSS/E load at bus {} id {id:?}: stale PL/QL/IP/IQ/YP/YQ extras did not match \
             typed p/q; wrote typed p/q as constant power",
                l.bus
            ),
        );
        (l.p, l.q, 0.0, 0.0, 0.0, 0.0)
    } else {
        (pl, ql, ip, iq, yp, yq)
    }
}

/// A retained converter-line tail joined back into a record fragment, or
/// `default` when the element carries none (a cross-format source).
fn dc_tail(extras: &Extras, key: &str, default: &str) -> String {
    match extras.get(key).and_then(Value::as_array) {
        Some(arr) if !arr.is_empty() => arr
            .iter()
            .filter_map(Value::as_str)
            // These come from a source file's `extras` and are replayed into
            // a record, so they go through the quoting seam like every other
            // interpolated string: a terminator here would forge a whole DC
            // record or a section end.
            .map(|f| sanitize_quoted(f, NAME_FORBIDDEN, ' ').into_owned())
            .collect::<Vec<_>>()
            .join(", "),
        _ => default.to_string(),
    }
}

#[cfg(test)]
mod tests {

    fn parse_psse(content: &str) -> Result<BalancedNetwork> {
        let mut warnings = Diagnostics::new();
        parse_psse_source(content, None, &mut warnings)
    }
    use super::*;
    use crate::diagnostics::Diagnostic;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn rejects_malformed_fractional_and_unsupported_header_revisions() {
        assert_eq!(header_rev("0, 100.00\nCASE\nCOMMENT\nQ\n").unwrap(), 33);
        assert_eq!(
            header_rev("0, 100.00, , 0, 0, 60.00\nCASE\nCOMMENT\nQ\n").unwrap(),
            33
        );

        for revision in ["not-a-revision", "NaN", "34.5", "31", "36"] {
            let raw = format!(
                "0, 100.00, {revision}, 0, 0, 60.00 / revision check\n\
                 CASE\nCOMMENT\n\
                 1,'B1          ',230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
                 0 / END OF BUS DATA, BEGIN LOAD DATA\nQ\n"
            );
            let error = parse_psse(&raw).unwrap_err().to_string();
            assert!(
                error.contains("expected integral 32, 33, 34, or 35"),
                "REV {revision:?} returned the wrong error: {error}"
            );
        }
        assert_eq!(
            header_rev("0, 100.00, 32, 0, 0, 60.00\nCASE\nCOMMENT\nQ\n").unwrap(),
            32
        );
    }

    /// Revision 32 records end before the fields revision 33 added: the bus
    /// voltage limits, the load INTRPT field, the transformer VECGRP field, and
    /// the winding CNXA field. The reader lays the records out by the header
    /// revision and reports a record that ends before its last typed field.
    #[test]
    fn revision32_layouts_read_and_short_records_are_reported() {
        let raw = "0, 100.00, 32, 0, 0, 60.00 / revision 32 layouts\n\
                   CASE\nCOMMENT\n\
                   1,'B1          ',230.0,3,1,1,1,1.02,0.0\n\
                   2,'B2          ',115.0,1,1,1,1,1.0,-1.5\n\
                   3,'B3          ',115.0,1,1,1,1\n\
                   0 / END OF BUS DATA, BEGIN LOAD DATA\n\
                   2,'1 ',1,1,1,15.0,5.0,0.0,0.0,0.0,0.0,1,1\n\
                   3,'1 ',1,1,1,10.0\n\
                   0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\n\
                   0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA\n\
                   1,'1 ',30.0,0.0,20.0,-20.0,1.02,0,100.0,0.0,1.0,0.0,0.0,1.0,1,100.0,80.0,0.0\n\
                   0 / END OF GENERATOR DATA, BEGIN BRANCH DATA\n\
                   2,3,'1 ',0.01,0.05,0.02,100.0,0.0,0.0,0.0,0.0,0.0,0.0,1,1,0.0\n\
                   0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA\n\
                   1,2,0,'1 ',1,1,1,0.0,0.0,2,'T1          ',1,1,1.0\n\
                   0.0,0.1,100.0\n\
                   0.98,0.0,0.0,50.0,0.0,0.0,0,0,1.5,0.51,1.5,0.51,10,0,0.0,0.0\n\
                   1.0,0.0\n\
                   0 / END OF TRANSFORMER DATA, BEGIN AREA DATA\n\
                   0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA\n\
                   0 / END OF TWO-TERMINAL DC DATA, BEGIN VOLTAGE SOURCE CONVERTER DATA\n\
                   0 / END OF VOLTAGE SOURCE CONVERTER DATA, BEGIN IMPEDANCE CORRECTION DATA\n\
                   0 / END OF IMPEDANCE CORRECTION DATA, BEGIN MULTI-TERMINAL DC DATA\n\
                   0 / END OF MULTI-TERMINAL DC DATA, BEGIN MULTI-SECTION LINE DATA\n\
                   0 / END OF MULTI-SECTION LINE DATA, BEGIN ZONE DATA\n\
                   1,'ZONE 1      '\n\
                   0 / END OF ZONE DATA, BEGIN INTER-AREA TRANSFER DATA\n\
                   0 / END OF INTER-AREA TRANSFER DATA, BEGIN OWNER DATA\n\
                   0 / END OF OWNER DATA, BEGIN FACTS CONTROL DEVICE DATA\n\
                   0 / END OF FACTS CONTROL DEVICE DATA, BEGIN SWITCHED SHUNT DATA\n\
                   0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA\n\
                   0 / END OF GNE DEVICE DATA\nQ\n";
        let mut warnings = Diagnostics::new();
        let net = parse_psse_source(raw, None, &mut warnings).unwrap();

        assert_eq!(net.buses().len(), 3);
        let bus1 = &net.buses()[0];
        close(bus1.vm, 1.02);
        close(bus1.vmax, 1.1);
        close(bus1.vmin, 0.9);
        assert_eq!(bus1.evhi, None);
        let bus3 = &net.buses()[2];
        close(bus3.vm, 1.0);
        close(bus3.va, 0.0);
        assert_eq!(net.loads().len(), 2);
        close(net.loads()[0].p, 15.0);
        assert!(!net.loads()[0].extras.contains_key("psse_intrpt"));
        close(net.loads()[1].p, 10.0);
        close(net.loads()[1].q, 0.0);
        let transformer = net
            .branches()
            .iter()
            .find(|branch| branch.is_transformer())
            .unwrap();
        close(transformer.calc_effective_tap(), 0.98);
        assert!(!transformer.extras.contains_key("psse_vecgrp"));
        let control = transformer.control.as_ref().unwrap();
        close(control.tap_max, 1.5);
        assert_eq!(control.ntp, 10);
        assert_eq!(control.winding_connection_angle, None);

        let short: Vec<&Diagnostic> = warnings
            .records()
            .iter()
            .filter(|diagnostic| diagnostic.code() == "READ.PSSE.VALUE_DEFAULTED")
            .collect();
        assert_eq!(short.len(), 2, "{:?}", warnings.lines());
        assert!(
            short[0]
                .message()
                .contains("BUS DATA record beginning \"3\" has 7 field(s)")
        );
        assert!(short[0].message().contains("reads 9 fields through VA"));
        assert!(
            short[1]
                .message()
                .contains("LOAD DATA record beginning \"3\" has 6 field(s)")
        );
        assert!(short[1].message().contains("reads 13 fields through SCALE"));
        assert!(
            short.iter().all(|diagnostic| diagnostic.spans().is_empty()),
            "an unlocated collector attaches no span"
        );
        let unmodeled = warnings
            .lines()
            .into_iter()
            .find(|line| line.contains("ZONE section"))
            .expect("the zone record is reported");
        assert!(
            unmodeled.contains("fresh output uses revision 33 or later"),
            "{unmodeled}"
        );
    }

    #[test]
    fn rejects_invalid_system_bases_and_nonfinite_record_fields() {
        let raw = |sbase: &str, frequency: &str, ide: &str, vm: &str| {
            format!(
                "0, {sbase}, 35, 0, 0, {frequency} / validation\n\
                 CASE\nCOMMENT\n\
                 1,'B1          ',230.0,{ide},1,1,1,{vm},0.0,1.1,0.9,1.1,0.9\n\
                 0 / END OF BUS DATA, BEGIN LOAD DATA\nQ\n"
            )
        };

        for sbase in ["NaN", "inf", "0", "-100"] {
            let error = parse_psse(&raw(sbase, "60", "3", "1.0"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("SBASE"), "bad SBASE {sbase}: {error}");
        }
        for frequency in ["NaN", "inf", "0", "-60", "not-a-frequency"] {
            let error = parse_psse(&raw("100", frequency, "3", "1.0"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("BASFRQ"), "bad BASFRQ {frequency}: {error}");
        }
        for (ide, vm) in [("NaN", "1.0"), ("3", "NaN"), ("3", "inf")] {
            let error = parse_psse(&raw("100", "60", ide, vm))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("not a finite number"),
                "IDE={ide}, VM={vm}: {error}"
            );
        }

        for token in ["NaN", "inf", "-inf"] {
            let fields = [Cow::Borrowed(token)];
            assert!(on_at(&fields, 0, true).is_err());
            assert!(int_at(&fields, 0, 0).is_err());
        }
    }

    #[test]
    fn refuses_unsupported_output_revisions_and_invalid_output_base() {
        let mut net = BalancedNetwork::in_memory(
            "revision",
            100.0,
            vec![test_bus(1, BusType::Ref)],
            Vec::new(),
        );
        for rev in [0, 32, 36, u32::MAX] {
            let error =
                crate::format::emit_value_text(&net, crate::format::TargetFormat::Psse { rev })
                    .unwrap_err()
                    .to_string();
            assert!(
                error.contains("only revisions 33, 34, and 35"),
                "revision {rev}: {error}"
            );
        }

        *net.base_mva_mut() = f64::NAN;
        assert!(
            crate::format::emit_value_text(&net, crate::format::TargetFormat::Psse { rev: 35 },)
                .is_err()
        );
        assert!(
            crate::format::emit_value_text(&net, crate::format::TargetFormat::PsseRawx).is_err()
        );
    }

    /// The tokenizer's rules, pinned: both delimiter styles produce the same
    /// fields for one record body, quoted interiors trim (PSS/E strings are
    /// fixed width and blank padded), a `/` inside quotes is text while one
    /// outside ends the record, and a blank quoted field holds its column.
    #[test]
    fn fields_tokenize_the_same_record_the_same_way_in_both_delimiter_styles() {
        assert_eq!(
            fields("1, ' 1', 'BUS A       ', 2.5 / trailing comment"),
            vec!["1", "1", "BUS A", "2.5"]
        );
        assert_eq!(
            fields("1 ' 1' 'BUS A       ' 2.5 / trailing comment"),
            vec!["1", "1", "BUS A", "2.5"]
        );
        assert_eq!(fields("1, 'O/H LINE', 2"), vec!["1", "O/H LINE", "2"]);
        assert_eq!(fields("1 2, 3"), vec!["1", "2", "3"]);
        let generator = fields(
            "30,'1 ',250,83.211,800,-500,1.0475,0,100,0,1.00000    0.00000,0,1,1,100,9999.9,0,1,1",
        );
        assert_eq!(generator[16], "9999.9");
        assert_eq!(generator[17], "0");
        assert_eq!(
            fields("1, '', 3"),
            vec!["1", "", "3"],
            "column position holds"
        );
        assert_eq!(
            fields("1 '' 3"),
            vec!["1", "", "3"],
            "and holds under whitespace too"
        );
    }

    #[test]
    fn extreme_transformer_cod_does_not_overflow() {
        // `int_at` parses COD through f64 and saturates, so `-1e300` becomes
        // i64::MIN; the old `cod.abs()` would overflow (panic under overflow
        // checks). The field decodes to Fixed instead.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 115.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1,2,0,'1 ',2,2,1,0,0,1,'xf',1
0.01,0.10,50.0
241.5,230.0,0.0,100.0,90.0,80.0,-1e300,0,1.1,0.9,1.1,0.9,33
115.0,115.0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let parsed = crate::parse_str(raw, "psse").unwrap();
        let control = parsed.network.branches()[0].control.as_ref().unwrap();
        assert_eq!(control.mode, TransformerControlMode::Fixed);
        assert!(
            parsed
                .render_diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.contains("unsupported COD=")),
            "unknown COD must be diagnosed: {:?}",
            parsed.render_diagnostics()
        );
    }

    fn test_bus(id: usize, kind: BusType) -> Bus {
        Bus {
            id: BusId(id),
            kind,
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
            extras: Extras::default(),
        }
    }

    fn branch_with_terminal_charging() -> Branch {
        Branch {
            name: None,
            from: BusId(1),
            to: BusId(2),
            r: 0.01,
            x: 0.1,
            b: 0.0,
            charging: Some(BranchCharging {
                g_fr: 0.01,
                b_fr: 0.02,
                g_to: 0.03,
                b_to: 0.05,
            }),
            rate_a: 100.0,
            rate_b: 110.0,
            rate_c: 120.0,
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
            extras: Extras::default(),
        }
    }

    fn transformer_with_terminal_charging(charging: BranchCharging) -> Branch {
        Branch {
            name: None,
            from: BusId(1),
            to: BusId(2),
            r: 0.01,
            x: 0.1,
            b: 0.0,
            charging: Some(charging),
            rate_a: 100.0,
            rate_b: 110.0,
            rate_c: 120.0,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: 1.05,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            solution: None,
            uid: None,
            route: None,
            extras: Extras::default(),
        }
    }

    fn assert_terminal_charging_round_trip(text: &str) {
        let back = parse_psse(text).unwrap();
        let charging = back.branches()[0].calc_terminal_charging();
        close(charging.g_fr, 0.01);
        close(charging.b_fr, 0.02);
        close(charging.g_to, 0.03);
        close(charging.b_to, 0.05);
        close(back.branches()[0].b, 0.07);
    }

    #[test]
    fn branch_terminal_charging_writes_gi_bi_gj_bj() {
        let mut net = BalancedNetwork::in_memory(
            "terminal-shunts",
            100.0,
            vec![test_bus(1, BusType::Ref), test_bus(2, BusType::Pq)],
            Vec::new(),
        );
        net.branches_mut().push(branch_with_terminal_charging());

        let rev33 = write_psse(&net);
        assert!(
            rev33.render_diagnostics().is_empty(),
            "{:?}",
            rev33.render_diagnostics()
        );
        assert_terminal_charging_round_trip(&rev33.text);

        let rev35 = write_psse_rev(&net, 35);
        assert!(
            rev35.render_diagnostics().is_empty(),
            "{:?}",
            rev35.render_diagnostics()
        );
        assert_terminal_charging_round_trip(&rev35.text);
    }

    #[test]
    fn transformer_magnetizing_admittance_writes_mag1_mag2() {
        let mut net = BalancedNetwork::in_memory(
            "xfmr-mag",
            100.0,
            vec![test_bus(1, BusType::Ref), test_bus(2, BusType::Pq)],
            Vec::new(),
        );
        net.branches_mut()
            .push(transformer_with_terminal_charging(BranchCharging {
                g_fr: 0.01,
                b_fr: 0.02,
                g_to: 0.0,
                b_to: 0.0,
            }));

        let conv = write_psse(&net);
        assert!(
            !conv
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("magnetizing admittance")),
            "{:?}",
            conv.render_diagnostics()
        );
        let back = parse_psse(&conv.text).unwrap();
        let charging = back.branches()[0].calc_terminal_charging();
        close(charging.g_fr, 0.01);
        close(charging.b_fr, 0.02);
        close(charging.g_to, 0.0);
        close(charging.b_to, 0.0);
        close(back.branches()[0].b, 0.02);

        let rev35 = write_psse_rev(&net, 35).text;
        assert!(rev35.contains(
            "0 / END OF BRANCH DATA, BEGIN SYSTEM SWITCHING DEVICE DATA\n\
             0 / END OF SYSTEM SWITCHING DEVICE DATA, BEGIN TRANSFORMER DATA"
        ));
        let main = rev35
            .lines()
            .find(|line| line.starts_with("1, 2, 0, '1'"))
            .unwrap();
        let main_fields = fields(main);
        assert_eq!(main_fields.len(), 22, "rev35 transformer row: {main:?}");
        assert_eq!(main_fields[21], "0", "ZCOD must be an integer");
    }

    #[test]
    fn transformer_to_side_terminal_admittance_warns_and_collapses_to_mag() {
        let mut net = BalancedNetwork::in_memory(
            "xfmr-mag-collapse",
            100.0,
            vec![test_bus(1, BusType::Ref), test_bus(2, BusType::Pq)],
            Vec::new(),
        );
        net.branches_mut()
            .push(transformer_with_terminal_charging(BranchCharging {
                g_fr: 0.01,
                b_fr: 0.02,
                g_to: 0.03,
                b_to: 0.05,
            }));

        let conv = write_psse(&net);
        assert!(
            conv.render_diagnostics()
                .iter()
                .any(|w| w.contains("magnetizing admittance")),
            "{:?}",
            conv.render_diagnostics()
        );
        let back = parse_psse(&conv.text).unwrap();
        let charging = back.branches()[0].calc_terminal_charging();
        close(charging.g_fr, 0.04);
        close(charging.b_fr, 0.07);
        close(charging.g_to, 0.0);
        close(charging.b_to, 0.0);
        close(back.branches()[0].b, 0.07);
    }

    #[test]
    fn slash_inside_a_quoted_field_is_not_a_comment() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
1,'A/B         ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
Q
";

        let net = parse_psse(raw).unwrap();

        assert_eq!(net.buses().len(), 1);
        assert_eq!(net.buses()[0].name.as_deref(), Some("A/B"));
    }

    #[test]
    fn load_zip_components_are_typed_and_round_trip() {
        let raw = r"0, 100.00, 35, 0, 1, 60.00 / synthetic
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
2,'L1',1,1,1,10.0,3.0,1.0,0.5,2.0,1.5,1,0,1,4.0,2.0,1,'industrial'
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
Q
";
        let mut warnings = Diagnostics::new();
        let net = parse_psse_source(raw, None, &mut warnings).unwrap();

        assert_eq!(net.loads().len(), 1);
        close(net.loads()[0].p, 13.0);
        close(net.loads()[0].q, 5.0);
        let Some(LoadVoltageModel::Zip {
            p_constant_power,
            q_constant_current,
            p_constant_impedance,
            ..
        }) = &net.loads()[0].voltage_model
        else {
            panic!("missing typed ZIP load model");
        };
        close(*p_constant_power, 10.0);
        close(*q_constant_current, 0.5);
        close(*p_constant_impedance, 2.0);
        assert!(
            warnings
                .lines()
                .iter()
                .any(|w| w.contains("interruptible/DG/flag")),
            "missing load option warning: {warnings:?}"
        );

        let text = write_psse_rev(&net, 35).text;
        assert!(
            text.contains("10.0, 3.0, 1.0, 0.5, 2.0, 1.5"),
            "ZIP components were not replayed: {text}"
        );
        assert!(
            text.contains("4.0, 2.0, 1, 'industrial'"),
            "modern load tail was not replayed: {text}"
        );
        let net2 = parse_psse(&text).unwrap();
        close(net2.loads()[0].p, 13.0);
        close(net2.loads()[0].q, 5.0);
    }

    #[test]
    fn tiny_nonzero_zip_components_are_preserved_as_typed_fields() {
        let raw = r"0, 100.00, 35, 0, 1, 60.00 / synthetic
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
2,'L1',1,1,1,10.0,3.0,1e-20,0.0,0.0,0.0,1,1,0,0.0,0.0,0,''
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
Q
";
        let net = parse_psse(raw).unwrap();
        let Some(LoadVoltageModel::Zip {
            p_constant_current, ..
        }) = &net.loads()[0].voltage_model
        else {
            panic!("tiny nonzero ZIP component was not typed");
        };
        assert_eq!(p_constant_current.to_bits(), 1.0e-20_f64.to_bits());

        let matpower = crate::format::matpower::write_matpower_conversion(&net);
        assert!(
            matpower
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("voltage dependent load model")),
            "missing MATPOWER voltage model warning: {:?}",
            matpower.render_diagnostics()
        );
    }

    #[test]
    fn typed_psse_load_scaling_and_type_write_without_extras() {
        let raw = r"0, 100.00, 35, 0, 1, 60.00 / synthetic
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
2,'L1',1,1,1,10.0,3.0,1.0,0.5,2.0,1.5,1,1,0,0.0,0.0,0,''
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
Q
";
        let mut net = parse_psse(raw).unwrap();
        let Some(LoadVoltageModel::Zip {
            scaling,
            load_type,
            v_nom,
            ..
        }) = &mut net.loads_mut()[0].voltage_model
        else {
            panic!("missing typed ZIP load model");
        };
        *scaling = Some(0.0);
        *load_type = Some(7);
        *v_nom = Some(230_000.0);
        net.loads_mut()[0].extras.remove("psse_scal");
        net.loads_mut()[0].extras.remove("psse_loadtype");

        let conv = write_psse_rev(&net, 35);

        assert!(
            conv.text.contains(", 1, 0, 0, 0.0, 0.0, 0, '7'"),
            "typed SCAL/LOADTYPE were not written: {}",
            conv.text
        );
        assert!(
            conv.render_diagnostics()
                .iter()
                .any(|w| w.contains("nominal voltage")),
            "missing nominal voltage warning: {:?}",
            conv.render_diagnostics()
        );
        let rev33 = write_psse(&net);
        assert!(
            rev33
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("load type requires revision 35")),
            "missing rev33 load type warning: {:?}",
            rev33.render_diagnostics()
        );
        let reparsed = parse_psse(&conv.text).unwrap();
        let Some(LoadVoltageModel::Zip {
            scaling, load_type, ..
        }) = &reparsed.loads()[0].voltage_model
        else {
            panic!("missing reparsed ZIP load model");
        };
        assert_eq!(*scaling, Some(0.0));
        assert_eq!(*load_type, Some(7));
    }

    #[test]
    fn mutated_load_does_not_replay_stale_psse_zip_extras() {
        let raw = r"0, 100.00, 35, 0, 1, 60.00 / synthetic
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
2,'L1',1,1,1,10.0,3.0,1.0,0.5,2.0,1.5,1,0,1,4.0,2.0,1,'industrial'
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
Q
";
        let mut net = parse_psse(raw).unwrap();
        net.loads_mut()[0].p = 20.0;
        net.loads_mut()[0].q = 7.0;

        let conv = write_psse_rev(&net, 35);

        assert!(
            conv.text.contains("20.0, 7.0, 0.0, 0.0, 0.0, 0.0"),
            "typed p/q were not written as constant power: {}",
            conv.text
        );
        assert!(
            conv.render_diagnostics()
                .iter()
                .any(|w| w.contains("stale voltage model components")),
            "missing stale voltage model warning: {:?}",
            conv.render_diagnostics()
        );
        let reparsed = parse_psse(&conv.text).unwrap();
        close(reparsed.loads()[0].p, 20.0);
        close(reparsed.loads()[0].q, 7.0);
    }

    #[test]
    fn transformer_continuation_rejects_section_terminator() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1,2,0,'1 ',1,1,1,0,0,1,'xf'
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let err = parse_psse(raw).unwrap_err().to_string();
        assert!(
            err.contains("transformer record ended before transformer impedance line"),
            "{err}"
        );
    }

    #[test]
    fn transformer_impedance_line_can_start_with_zero_resistance() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1,2,0,'1 ',1,1,1,0,0,1,'xf',1
0,0.10,100.0
1.0,230.0,0.0,100.0,90.0,80.0,0,0,1.1,0.9,1.1,0.9,33
1.0,230.0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();

        assert_eq!(net.branches().len(), 1);
        close(net.branches()[0].r, 0.0);
        close(net.branches()[0].x, 0.10);
    }

    #[test]
    fn transformer_non_integral_cz_is_a_hard_error() {
        // A malformed CZ like `2.9` must not silently truncate to a valid
        // looking code `2`; that would apply the wrong impedance base
        // conversion without ever surfacing an "unsupported CZ" warning.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 115.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1,2,0,'1 ',1,2.9,1,0,0,1,'xf',1
0,0.10,100.0
1.0,230.0,0.0,100.0,90.0,80.0,0,0,1.1,0.9,1.1,0.9,33
1.0,230.0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let err = parse_psse(raw).unwrap_err().to_string();
        assert!(err.contains("field 5") && err.contains("2.9"), "{err}");
    }

    #[test]
    fn non_unit_two_winding_transformer_bases_are_converted() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 115.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1,2,0,'1 ',2,2,1,0,0,1,'xf',1
0.01,0.10,50.0
241.5,230.0,0.0,100.0,90.0,80.0,0,0,1.1,0.9,1.1,0.9,33
115.0,115.0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let parsed = crate::parse_str(raw, "psse").unwrap();
        assert!(
            !parsed
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("unsupported CZ") || w.contains("unsupported CW")),
            "unexpected transformer base warning: {:?}",
            parsed.render_diagnostics()
        );
        let br = &parsed.network.branches()[0];
        close(br.r, 0.02);
        close(br.x, 0.20);
        close(br.tap, 1.05);
    }

    #[test]
    fn cz3_load_loss_and_cw3_nominal_voltage_are_converted() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 115.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1,2,0,'1 ',3,3,1,0,0,1,'xf',1
250000.0,0.10,50.0
1.05,230.0,0.0,100.0,90.0,80.0,0,0,1.1,0.9,1.1,0.9,33
1.0,115.0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let parsed = crate::parse_str(raw, "psse").unwrap();
        assert!(
            !parsed
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("unsupported CZ") || w.contains("unsupported CW")),
            "unexpected transformer base warning: {:?}",
            parsed.render_diagnostics()
        );
        let br = &parsed.network.branches()[0];
        close(br.r, 0.01);
        close(br.x, (0.10_f64 * 0.10 - 0.005_f64 * 0.005).sqrt() * 2.0);
        close(br.tap, 1.05);
    }

    #[test]
    fn non_unit_three_winding_transformer_bases_are_converted() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
1,'BUS1        ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'BUS2        ', 115.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
3,'BUS3        ', 13.8,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1,2,3,'1 ',2,2,1,0,0,1,'xf3',1
0.01,0.10,50.0,0.02,0.20,100.0,0.03,0.30,200.0,1.0,0.0
241.5,230.0,0.0,100.0,90.0,80.0,0,0,1.1,0.9,1.1,0.9,33
115.0,115.0,0.0,100.0,90.0,80.0,0,0,1.1,0.9,1.1,0.9,33
13.8,13.8,0.0,100.0,90.0,80.0,0,0,1.1,0.9,1.1,0.9,33
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let parsed = crate::parse_str(raw, "psse").unwrap();
        assert!(
            !parsed
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("unsupported CZ") || w.contains("unsupported CW")),
            "unexpected transformer base warning: {:?}",
            parsed.render_diagnostics()
        );
        let t = &parsed.network.transformers_3w()[0];
        close(t.z[0].r, 0.02);
        close(t.z[0].x, 0.20);
        close(t.z[1].r, 0.02);
        close(t.z[1].x, 0.20);
        close(t.z[2].r, 0.015);
        close(t.z[2].x, 0.15);
        close(t.windings[0].tap, 1.05);
        close(t.windings[1].tap, 1.0);
        close(t.windings[2].tap, 1.0);
    }

    #[test]
    fn dc_continuation_rejects_section_terminator() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN TWO-TERMINAL DC DATA
'DC1',1
0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
Q
";
        let err = parse_psse(raw).unwrap_err().to_string();
        assert!(
            err.contains("two-terminal DC record ended before rectifier line"),
            "{err}"
        );
    }

    #[test]
    fn reads_comment_headers_system_wide_block_and_named_branch_records() {
        let raw = r#"@!IC, SBASE,REV,XFRRAT,NXFRAT,BASFRQ
0, 100.00, 34, 0, 0, 60.00 / synthetic v34 export


GENERAL, THRSHZ=0.0002
RATING, 1, "      ", "                                "
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
@!   I,'NAME        ', BASKV, IDE,AREA,ZONE,OWNER, VM,        VA,    NVHI,   NVLO,   EVHI,   EVLO
1,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
2,'BUS2        ', 230.0000,1,1,1,9,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / END OF BUS DATA, BEGIN LOAD DATA
@!   I,'ID',STAT,AREA,ZONE,      PL,        QL
2,'1 ',1,2,3,10.0,5.0,0,0,0,0,4,1,0,0,0,0
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
@!   I,'ID',      PG,        QG,        QT,        QB,     VS,    IREG,     MBASE,     ZR,         ZX,         RT,         XT,     GTAP,STAT, RMPCT,      PT,        PB
1,'1 ',50.0,5.0,20.0,-10.0,1.0,0,100.0,0.0,1.0,0.0,0.0,1.0,1,100.0,80.0,10.0
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
@!   I,     J,'CKT',     R,          X,         B,                    'N A M E'                 ,   RATE1,   RATE2,   RATE3,   RATE4,   RATE5,   RATE6,   RATE7,   RATE8,   RATE9,  RATE10,  RATE11,  RATE12,    GI,       BI,       GJ,       BJ,STAT,MET,  LEN
1,2,'1 ',0.01,0.05,0.001,'named branch',100.0,90.0,80.0,70.0,0.0,60.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,1,2,12.5,7,0.6,8,0.4,0,1,0,1
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
"#;

        let net = parse_psse(raw).unwrap();

        close(net.base_mva(), 100.0);
        assert_eq!(net.buses().len(), 2);
        assert_eq!(net.loads().len(), 1);
        assert_eq!(net.generators().len(), 1);
        assert_eq!(net.branches().len(), 1);
        close(net.branches()[0].rate_a, 100.0);
        assert_eq!(net.branches()[0].name.as_deref(), Some("named branch"));
        assert_eq!(net.branches()[0].rating_sets.len(), 2);
        assert_eq!(net.branches()[0].rating_sets[0].name, "RATE4");
        close(net.branches()[0].rating_sets[0].rate_mva, 70.0);
        assert_eq!(net.branches()[0].rating_sets[1].name, "RATE6");
        close(net.branches()[0].rating_sets[1].rate_mva, 60.0);
        assert!(net.branches()[0].in_service);
        assert_eq!(net.buses()[1].extras["psse_owner"], Value::from(9));
        assert_eq!(net.loads()[0].extras["psse_area"], Value::from(2));
        assert_eq!(net.loads()[0].extras["psse_zone"], Value::from(3));
        assert_eq!(net.loads()[0].extras["psse_owner"], Value::from(4));
        assert_eq!(net.branches()[0].extras["psse_met"], Value::from(2));
        close(net.branches()[0].extras["psse_len"].as_f64().unwrap(), 12.5);
        assert_eq!(net.branches()[0].extras["psse_o1"], Value::from(7));
        close(net.branches()[0].extras["psse_f1"].as_f64().unwrap(), 0.6);
        assert_eq!(net.branches()[0].extras["psse_o2"], Value::from(8));
        close(net.branches()[0].extras["psse_f2"].as_f64().unwrap(), 0.4);

        let rev33 = write_psse_rev(&net, 33);
        assert!(rev33.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic
                    .message()
                    .contains("1 non-transformer branch name")
                && diagnostic.message().contains("revision 33")
        }));
        assert_eq!(parse_psse(&rev33.text).unwrap().branches()[0].name, None);

        let mut energy_source = net.clone();
        energy_source.generators_mut()[0].energy_source = GeneratorEnergySource::Wind;
        let energy_source = write_psse_rev(&energy_source, 35);
        assert!(energy_source.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic.message().contains("generator energy source")
                && diagnostic.message().contains("wind=1")
        }));

        let written = write_psse_rev(&net, 34);
        assert!(
            !written
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("rating set")),
            "v34 should carry RATE4-RATE12, got {:?}",
            written.render_diagnostics()
        );
        let back = parse_psse(&written.text).unwrap();
        assert_eq!(back.branches()[0].name.as_deref(), Some("named branch"));
        assert_eq!(back.branches()[0].rating_sets.len(), 2);
        assert_eq!(back.branches()[0].rating_sets[0].name, "RATE4");
        close(back.branches()[0].rating_sets[0].rate_mva, 70.0);
        assert_eq!(back.branches()[0].rating_sets[1].name, "RATE6");
        close(back.branches()[0].rating_sets[1].rate_mva, 60.0);
        assert_eq!(back.buses()[1].extras["psse_owner"], Value::from(9));
        assert_eq!(back.loads()[0].extras["psse_area"], Value::from(2));
        assert_eq!(back.loads()[0].extras["psse_zone"], Value::from(3));
        assert_eq!(back.loads()[0].extras["psse_owner"], Value::from(4));
        assert_eq!(back.branches()[0].extras["psse_met"], Value::from(2));
        close(
            back.branches()[0].extras["psse_len"].as_f64().unwrap(),
            12.5,
        );
        assert_eq!(back.branches()[0].extras["psse_o1"], Value::from(7));
        close(back.branches()[0].extras["psse_f1"].as_f64().unwrap(), 0.6);
    }

    #[test]
    fn revision_35_system_switches_round_trip() {
        let raw = r"0, 100.00, 35, 0, 0, 60.00 / synthetic v35 switch
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'B1          ',230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ',230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
3,'B3          ',230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN SYSTEM SWITCHING DEVICE DATA
1,2,'S1',0.0001,55,54,53,52,51,50,49,48,47,46,45,44,0,2,2,3,'tie switch'
2,3,'1',0,0,0,0,0,0,0,0,0,0,0,0,0,1,1,1,1,'pair one'
1,3,'1',0,0,0,0,0,0,0,0,0,0,0,0,0,1,1,1,1,'other pair'
2,3,'2',0,0,0,0,0,0,0,0,0,0,0,0,0,1,1,1,1,'parallel'
0 / END OF SYSTEM SWITCHING DEVICE DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let mut parsed = parse_psse(raw).unwrap();
        assert_eq!(parsed.switches().len(), 4);
        let source_ids = parsed
            .switches()
            .iter()
            .map(|switch| switch.uid.as_deref().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(source_ids, ["1-2-S1", "2-3-1", "1-3-1", "2-3-2"]);
        let switch = &parsed.switches()[0];
        assert_eq!((switch.from, switch.to), (BusId(1), BusId(2)));
        assert!(!switch.closed);
        assert_eq!(switch.thermal_rating, Some(55.0));
        assert_eq!(switch.extras["psse_ckt"], Value::from("S1"));
        assert_eq!(switch.extras["psse_xpu"], jnum(0.0001));
        assert_eq!(switch.extras["psse_rate2"], Value::from(54.0));
        assert_eq!(switch.extras["psse_nstat"], Value::from(2));
        assert_eq!(switch.extras["psse_met"], Value::from(2));
        assert_eq!(switch.extras["psse_stype"], Value::from(3));
        assert_eq!(switch.extras["psse_name"], Value::from("tie switch"));

        parsed.switches_mut()[0].current_rating = Some(2_000.0);
        parsed.switches_mut()[0].pf = Some(10.0);
        parsed.switches_mut()[0].qf = Some(2.0);
        parsed.switches_mut()[0].pt = Some(-9.8);
        parsed.switches_mut()[0].qt = Some(-1.9);
        parsed.switches_mut()[0]
            .extras
            .insert("psse_rsetnam".into(), Value::from("RATESET"));

        let emitted = write_psse_rev(&parsed, 35);
        assert!(emitted.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic.message().contains("switch current rating")
        }));
        assert!(emitted.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic.message().contains("switch power flow result")
        }));
        assert!(emitted.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic.message().contains("rating set name")
                && diagnostic.message().contains("RATE1-RATE12")
        }));
        let reparsed = parse_psse(&emitted.text).unwrap();
        assert_eq!(
            reparsed
                .switches()
                .iter()
                .map(|switch| switch.uid.as_deref().unwrap())
                .collect::<Vec<_>>(),
            source_ids.iter().map(String::as_str).collect::<Vec<_>>()
        );
        let switch = &reparsed.switches()[0];
        assert_eq!((switch.from, switch.to), (BusId(1), BusId(2)));
        assert!(!switch.closed);
        assert_eq!(switch.thermal_rating, Some(55.0));
        assert_eq!(switch.extras["psse_rate12"], Value::from(44.0));
        assert_eq!(switch.extras["psse_name"], Value::from("tie switch"));

        let rev34 = write_psse_rev(&parsed, 34);
        assert!(rev34.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.RECORD_DROPPED"
                && diagnostic.message().contains("system switching device")
                && diagnostic.message().contains("revision 34")
        }));
    }

    #[test]
    fn v34_transformer_reads_float_k_and_modern_winding_columns() {
        // v34/35 exporters write K in float form: "0.00" must classify the
        // record as 2-winding (4 lines), or the reader consumes a fifth line and
        // desynchronizes every later section. The winding line uses the v34
        // layout (twelve ratings, NODE after CONT), putting COD at 15 and
        // RMA..NTP at 18..22.
        let raw = r"0, 100.00, 34, 0, 0, 60.00 / synthetic v34 export
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 18.0,2,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 0.00, '1', 1, 1, 1, 0.0, 0.0, 1, 'T1          ', 1, 7, 0.6, 8, 0.4, 0, 1, 0, 1, 'YNd1'
0.01, 0.10, 100.0
1.05, 0.0, 0.0, 100.0, 90.0, 80.0, 70.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1, 2, 0, 1.08, 0.92, 1.05, 0.98, 17, 9, 0.01, 0.02, 0
1.0, 0.0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
1, 1, 0.0, 0.0, 'AREA        '
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.branches().len(), 1, "K = 0.00 is a 2-winding record");
        assert!(net.transformers_3w().is_empty());
        assert_eq!(
            net.areas().len(),
            1,
            "the section after the transformer parsed"
        );
        let br = &net.branches()[0];
        assert_eq!(br.name.as_deref(), Some("T1"));
        close(br.tap, 1.05);
        close(br.rate_a, 100.0);
        assert_eq!(br.rating_sets.len(), 1);
        assert_eq!(br.rating_sets[0].name, "RATE4");
        close(br.rating_sets[0].rate_mva, 70.0);
        let c = br.control.as_ref().expect("COD at 15 marks the control");
        assert_eq!(c.mode, TransformerControlMode::Voltage);
        assert_eq!(c.controlled_bus, Some(BusId(2)));
        close(c.tap_max, 1.08);
        close(c.tap_min, 0.92);
        close(c.band_max, 1.05);
        close(c.band_min, 0.98);
        assert_eq!(c.ntp, 17);
        assert_eq!(br.extras["psse_nmetr"], Value::from(1));
        assert_eq!(br.extras["psse_o1"], Value::from(7));
        close(br.extras["psse_f1"].as_f64().unwrap(), 0.6);
        assert_eq!(br.extras["psse_o2"], Value::from(8));
        close(br.extras["psse_f2"].as_f64().unwrap(), 0.4);
        assert_eq!(br.extras["psse_vecgrp"], Value::from("YNd1"));
        assert_eq!(br.extras["psse_tab"], Value::from(9));
        close(br.extras["psse_cr"].as_f64().unwrap(), 0.01);
        close(br.extras["psse_cx"].as_f64().unwrap(), 0.02);

        let back = parse_psse(&write_psse_rev(&net, 34).text).unwrap();
        assert_eq!(back.branches()[0].name.as_deref(), Some("T1"));
        let br = &back.branches()[0];
        assert_eq!(br.extras["psse_nmetr"], Value::from(1));
        assert_eq!(br.extras["psse_o1"], Value::from(7));
        close(br.extras["psse_f1"].as_f64().unwrap(), 0.6);
        assert_eq!(br.extras["psse_vecgrp"], Value::from("YNd1"));
        assert_eq!(br.extras["psse_tab"], Value::from(9));
        close(br.extras["psse_cr"].as_f64().unwrap(), 0.01);
        close(br.extras["psse_cx"].as_f64().unwrap(), 0.02);
    }

    #[test]
    fn v34_warns_when_custom_rating_name_is_emitted_as_rate_slot() {
        let mut net = BalancedNetwork::in_memory(
            "ratings",
            100.0,
            vec![
                Bus::new(BusId(1), BusType::Ref, 230.0),
                Bus::new(BusId(2), BusType::Pq, 230.0),
            ],
            Vec::new(),
        );
        let mut branch = Branch::new(BusId(1), BusId(2), 0.01, 0.05);
        branch.rate_a = 100.0;
        branch
            .rating_sets
            .push(BranchRatingSet::new("emergency", 125.0));
        net.branches_mut().push(branch);

        let written = write_psse_rev(&net, 34);

        assert!(
            written.render_diagnostics().iter().any(|w| {
                w.contains("rating set emergency=125")
                    && w.contains("emitted as RATE4")
                    && w.contains("names outside RATE4-RATE12 are not preserved")
            }),
            "missing rating rename warning: {:?}",
            written.render_diagnostics()
        );
        let back = parse_psse(&written.text).unwrap();
        assert_eq!(back.branches()[0].rating_sets.len(), 1);
        assert_eq!(back.branches()[0].rating_sets[0].name, "RATE4");
        close(back.branches()[0].rating_sets[0].rate_mva, 125.0);
    }

    #[test]
    fn reads_start_of_section_markers_and_gen_alias() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic v33 export
CASE
COMMENT
1,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
2,'BUS2        ', 230.0000,1,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / End of Bus Data, Start of Load Data
2,'1 ',1,1,1,10.0,5.0
0 / End of Load Data, Start of Fixed Shunt Data
0 / End of Fixed Shunt Data, Start of Gen Data
1,'1 ',50.0,5.0,20.0,-10.0,1.0,0,100.0,0.0,1.0,0.0,0.0,1.0,1,100.0,80.0,10.0
0 / End of Gen Data, Start of Branch Data
1,2,'1 ',0.01,0.05,0.001,100.0,90.0,80.0,0.0,0.0,0.0,0.0,1,1,0.0,1,1
0 / End of Branch Data, Start of Transformer Data
0 / End of Transformer Data, Start of Area Interchange Data
Q
";

        let net = parse_psse(raw).unwrap();

        assert_eq!(net.buses().len(), 2);
        assert_eq!(net.loads().len(), 1);
        assert_eq!(net.generators().len(), 1);
        assert_eq!(net.branches().len(), 1);
        assert_eq!(net.branches()[0].name, None);

        let back = parse_psse(&write_psse_rev(&net, 35).text).unwrap();
        assert_eq!(back.branches()[0].name, None);
    }

    #[test]
    fn reads_whitespace_records_with_end_only_section_markers() {
        let raw = r"0 100.00 33 0 0 60.00 / whitespace-delimited export
CASE
COMMENT
1 'BUS 1' 230.0 3 1 1 1 1.0 0.0 1.1 0.9 1.1 0.9
2 'BUS 2' 230.0 1 1 1 1 1.0 0.0 1.1 0.9 1.1 0.9
0 / END OF BUS DATA
2 'L1' 1 1 1 10.0 5.0 0 0 0 0 1 1 0
0 / END OF LOAD DATA
0 / END OF FIXED BUS SHUNT DATA
1 'G1' 50.0 5.0 20.0 -10.0 1.0 0 100.0 0.0 1.0 0.0 0.0 1.0 1 100.0 80.0 10.0
0 / END OF GENERATOR DATA
1 2 '1' 0.01 0.05 0.001 100.0 90.0 80.0 0.0 0.0 0.0 0.0 1 1 0.0 1 1
0 / END OF NON TRANSFORMER BRANCH DATA
0 / END OF TRANSFORMER BRANCH DATA
Q
";

        let net = parse_psse(raw).unwrap();

        assert_eq!(net.buses().len(), 2);
        assert_eq!(net.loads().len(), 1);
        assert_eq!(net.generators().len(), 1);
        assert_eq!(net.branches().len(), 1);
    }

    #[test]
    fn v33_long_branch_with_blank_ratea_keeps_v33_columns() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic v33 export
CASE
COMMENT
1,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
2,'BUS2        ', 230.0000,1,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
1,2,'1 ',0.01,0.05,0.001,,90.0,80.0,0.0,0.0,0.0,0.0,1,1,0.0,1,1.0,2,0.0,3,0.0,4,0.0
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";

        let net = parse_psse(raw).unwrap();

        assert_eq!(net.branches().len(), 1);
        close(net.branches()[0].rate_a, 0.0);
        close(net.branches()[0].rate_b, 90.0);
        close(net.branches()[0].rate_c, 80.0);
        assert!(net.branches()[0].in_service);
    }

    #[test]
    fn captured_load_ids_round_trip_and_parallel_loads_stay_distinct() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
2,'A',1,1,1,10.0,5.0,0,0,0,0,1,1,0
2,'B',1,1,1,20.0,8.0,0,0,0,0,1,1,0
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let id = |l: &Load| {
            l.extras
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.loads().len(), 2);
        assert_eq!(id(&net.loads()[0]).as_deref(), Some("A"));
        assert_eq!(id(&net.loads()[1]).as_deref(), Some("B"));

        // A round trip keeps the captured ids.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        assert_eq!(id(&net2.loads()[0]).as_deref(), Some("A"));
        assert_eq!(id(&net2.loads()[1]).as_deref(), Some("B"));

        // With the ids stripped (a synthesized network, e.g. from MATPOWER), the
        // two loads on bus 2 still write with distinct positional ids, so the
        // output is valid PSS/E rather than two colliding (bus, '1') records.
        // The reader keeps only the second: id `1` is the positional default
        // the writer re-allocates on its own, so retaining it restates nothing.
        let mut synth = net.clone();
        for l in synth.loads_mut() {
            l.extras.remove("id");
        }
        let out = write_psse(&synth).text;
        assert!(out.contains("2, '1',") && out.contains("2, '2',"), "{out}");
        let net3 = parse_psse(&out).unwrap();
        let ids: Vec<_> = net3.loads().iter().filter_map(&id).collect();
        assert_eq!(ids, vec!["2".to_string()]);
    }

    #[test]
    fn sanitized_load_ids_are_allocated_after_cleaning() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
2,'A',1,1,1,10.0,5.0,0,0,0,0,1,1,0
2,'B',1,1,1,20.0,8.0,0,0,0,0,1,1,0
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let mut net = parse_psse(raw).unwrap();
        net.loads_mut()[0]
            .extras
            .insert("id".into(), Value::String("A/B".into()));
        net.loads_mut()[1]
            .extras
            .insert("id".into(), Value::String("A'B".into()));

        let conv = write_psse(&net);
        let reparsed = parse_psse(&conv.text).unwrap();
        let ids: Vec<_> = reparsed
            .loads()
            .iter()
            .filter_map(|l| l.extras.get("id").and_then(Value::as_str))
            .collect();

        // The collision fallback allocated `1`, the positional default, which
        // the reader has no reason to keep.
        assert_eq!(ids, vec!["A B"]);
        assert!(
            conv.render_diagnostics()
                .iter()
                .any(|w| w.contains("2 quoted PSS/E field")),
            "missing sanitation warning: {:?}",
            conv.render_diagnostics()
        );
    }

    #[test]
    fn two_winding_transformer_charging_round_trips_via_mag2() {
        // MAG2 (line-1 field 8) carries the transformer's magnetizing susceptance;
        // at CM = 1 it maps to the branch charging b and must survive a round trip.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 0, '1', 1, 1, 1, 0, 0.04, 2, 'XF          ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0
1.025, 0, 0.0, 100.0, 90.0, 80.0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
1.0, 0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.branches().len(), 1);
        assert!(net.branches()[0].is_transformer());
        close(net.branches()[0].b, 0.04);

        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        close(net2.branches()[0].b, 0.04);
    }

    #[test]
    fn cm2_transformer_magnetizing_data_is_converted_and_round_trips() {
        let raw = r"0, 100.00, 35, 0, 1, 60.00 / synthetic
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 0, '1', 1, 1, 2, 100000, 0.2, 2, 'XF          ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 50.0
1.025, 230.0, 0.0, 100.0, 90.0, 80.0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
1.0, 138.0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        let branch = &net.branches()[0];
        let expected_g: f64 = 0.001;
        let expected_b = -f64::sqrt(0.1_f64.powi(2) - expected_g.powi(2));
        let charging = branch.calc_terminal_charging();
        close(charging.g_fr, expected_g);
        close(charging.b_fr, expected_b);

        // The writer emits neutral p.u. admittance as CM=1, so conversion is
        // not applied a second time on the next read.
        let back = parse_psse(&write_psse_rev(&net, 35).text).unwrap();
        let back_charging = back.branches()[0].calc_terminal_charging();
        close(back_charging.g_fr, expected_g);
        close(back_charging.b_fr, expected_b);
    }

    #[test]
    fn parallel_branches_round_trip_and_stay_distinct() {
        // Two circuits between buses 1 and 2: each keeps a distinct CKT so the
        // output is valid PSS/E rather than two colliding (I, J, '1') records.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
1,2,'1 ',0.01,0.05,0.001,0,0,0,0,0,0,0,1,1,0.0
1,2,'2 ',0.02,0.06,0.002,0,0,0,0,0,0,0,1,1,0.0
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let ckt = |b: &Branch| {
            b.extras
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.branches().len(), 2);
        // CKT `1` is the positional default the writer re-allocates on its own,
        // so only the second circuit's id carries information worth keeping.
        assert_eq!(ckt(&net.branches()[0]).as_deref(), None);
        assert_eq!(ckt(&net.branches()[1]).as_deref(), Some("2"));

        // Round trip keeps both circuits distinct: the id-less branch takes '1'
        // positionally and the explicit '2' is replayed.
        let out = write_psse(&net).text;
        assert!(
            out.contains("1, 2, '1',") && out.contains("1, 2, '2',"),
            "{out}"
        );
        let net2 = parse_psse(&out).unwrap();
        assert_eq!(net2.branches().len(), 2);
        assert_eq!(ckt(&net2.branches()[0]).as_deref(), None);
        assert_eq!(ckt(&net2.branches()[1]).as_deref(), Some("2"));
    }

    #[test]
    fn parallel_transformers_preserve_distinct_circuit_ids() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1,2,0,'1 ',1,1,1,0,0,2,'XF1         ',1,1,1,0,1,0,1,0,1,'            '
0.01,0.10,100.0
1.0,0,0,100,90,80,0,0,1.1,0.9,1.1,0.9,33,0,0,0,0
1.0,0
1,2,0,'2 ',1,1,1,0,0,2,'XF2         ',1,1,1,0,1,0,1,0,1,'            '
0.02,0.20,100.0
1.0,0,0,100,90,80,0,0,1.1,0.9,1.1,0.9,33,0,0,0,0
1.0,0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let circuit_id = |branch: &Branch| {
            branch
                .extras
                .get("id")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        };
        let network = parse_psse(raw).unwrap();
        assert_eq!(network.branches().len(), 2);
        assert_eq!(circuit_id(&network.branches()[0]).as_deref(), None);
        assert_eq!(circuit_id(&network.branches()[1]).as_deref(), Some("2"));

        let output = write_psse(&network).text;
        assert!(output.lines().any(|line| line.starts_with("1, 2, 0, '1',")));
        assert!(output.lines().any(|line| line.starts_with("1, 2, 0, '2',")));
        let reparsed = parse_psse(&output).unwrap();
        assert_eq!(reparsed.branches().len(), 2);
        assert_eq!(circuit_id(&reparsed.branches()[0]).as_deref(), None);
        assert_eq!(circuit_id(&reparsed.branches()[1]).as_deref(), Some("2"));
    }

    #[test]
    fn reads_and_writes_solver_params() {
        let raw = r"0, 100.00, 34, 0, 1, 60.00 / x
CASE
COMMENT
GENERAL, THRSHZ=0.0001
NEWTON, TOLN=0.1, ITMXN=25
SOLVER, ACTAPS=1, AREAIN=0, PHSHFT=1, DCTAPS=1, SWSHNT=0
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
Q
";
        let net = parse_psse(raw).unwrap();
        let sp = net.solver().as_ref().expect("solver params parsed");
        close(sp.zero_impedance_threshold.unwrap(), 0.0001);
        close(sp.newton_tolerance.unwrap(), 0.1);
        assert_eq!(sp.max_iterations, Some(25));
        assert_eq!(sp.adjust_taps, Some(true));
        assert_eq!(sp.adjust_area_interchange, Some(false));
        assert_eq!(sp.adjust_phase_shift, Some(true));
        assert_eq!(sp.adjust_switched_shunt, Some(false));

        // Round trip at rev 34 keeps the tolerances and the adjustment flags.
        let net2 = parse_psse(&write_psse_rev(&net, 34).text).unwrap();
        let sp2 = net2
            .solver()
            .as_ref()
            .expect("solver params survive the write");
        close(sp2.newton_tolerance.unwrap(), 0.1);
        assert_eq!(sp2.max_iterations, Some(25));
        assert_eq!(sp2.adjust_taps, Some(true));
        assert_eq!(sp2.adjust_area_interchange, Some(false));
    }

    #[test]
    fn system_wide_losses_and_invalid_values_are_diagnosed() {
        let raw = r"0, 100.00, 35, 0, 1, 60.00 / x
CASE
COMMENT
GENERAL, THRSHZ=bad, EXTRA=1
GAUSS, ITMXG=20
NEWTON, TOLN=NaN, ITMXN=-1, EXTRA=2
SOLVER, ACTAPS=MAYBE, AREAIN=OFF, UNKNOWN=3
ADJUST, TOL=0.1
TYSL, ITMX=5
RATING, 1, 'RATE SET'
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
Q
";
        let mut warnings = Diagnostics::new();
        let network = parse_psse_source(raw, None, &mut warnings).unwrap();
        let solver = network.solver().as_ref().unwrap();
        assert_eq!(solver.adjust_area_interchange, Some(false));
        assert_eq!(solver.zero_impedance_threshold, None);
        assert_eq!(solver.newton_tolerance, None);
        assert_eq!(solver.max_iterations, None);
        assert_eq!(solver.adjust_taps, None);

        let diagnostics = warnings.lines();
        for record in ["GAUSS", "ADJUST", "TYSL", "RATING"] {
            assert!(
                diagnostics.iter().any(|line| {
                    line.contains("READ.PSSE.FIELD_DROPPED")
                        && line.contains(&format!("system wide {record} record"))
                }),
                "missing diagnostic for {record}: {diagnostics:?}"
            );
        }
        for field in ["GENERAL.EXTRA", "NEWTON.EXTRA", "SOLVER.UNKNOWN"] {
            assert!(
                diagnostics.iter().any(|line| {
                    line.contains("READ.PSSE.FIELD_DROPPED") && line.contains(field)
                }),
                "missing diagnostic for {field}: {diagnostics:?}"
            );
        }
        for field in [
            "GENERAL.THRSHZ",
            "NEWTON.TOLN",
            "NEWTON.ITMXN",
            "SOLVER.ACTAPS",
        ] {
            assert!(
                diagnostics.iter().any(|line| {
                    line.contains("READ.PSSE.VALUE_SUBSTITUTED") && line.contains(field)
                }),
                "missing invalid value diagnostic for {field}: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn reads_and_writes_area_records() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
5,'B5          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
1, 5, 100.0, 10.0, 'AREA-ONE    '
0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.areas().len(), 1, "the area record was read");
        let a = &net.areas()[0];
        assert_eq!(a.number, 1);
        assert_eq!(a.slack_bus, Some(BusId(5)));
        close(a.net_interchange, 100.0);
        close(a.tolerance, 10.0);
        assert_eq!(a.name.as_deref(), Some("AREA-ONE"));
        assert_eq!(a.area_type.as_deref(), Some("ControlArea"));

        // Round trip: write and re-read keeps the interchange and swing bus.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        assert_eq!(net2.areas().len(), 1);
        let a2 = &net2.areas()[0];
        assert_eq!(a2.number, 1);
        assert_eq!(a2.slack_bus, Some(BusId(5)));
        close(a2.net_interchange, 100.0);
        assert_eq!(a2.name.as_deref(), Some("AREA-ONE"));
        assert_eq!(a2.area_type.as_deref(), Some("ControlArea"));
    }

    #[test]
    fn reads_and_writes_a_switched_shunt() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
7,'B7          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN SWITCHED SHUNT DATA
3, 2, 0, 1, 1.05, 0.95, 7, 100.0, '', 19.0, 2, 25.0, 1, 50.0
0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.shunts().len(), 1);
        let sh = &net.shunts()[0];
        assert_eq!(sh.bus, BusId(3));
        close(sh.b, 19.0);
        let c = sh.control.as_ref().expect("switched-shunt control parsed");
        assert_eq!(c.mode, SwitchedShuntMode::Discrete);
        close(c.vhigh, 1.05);
        close(c.vlow, 0.95);
        assert_eq!(c.control_bus, Some(BusId(7)));
        close(c.rmpct, 100.0);
        assert_eq!(c.blocks.len(), 2);
        assert_eq!(c.blocks[0].steps, 2);
        close(c.blocks[0].b, 25.0);
        assert_eq!(c.blocks[1].steps, 1);
        close(c.blocks[1].b, 50.0);

        // Round trip: written to the SWITCHED SHUNT section and re-read intact.
        let text = write_psse(&net).text;
        assert!(text.contains("BEGIN SWITCHED SHUNT DATA"));
        let net2 = parse_psse(&text).unwrap();
        assert_eq!(net2.shunts().len(), 1);
        let c2 = net2.shunts()[0]
            .control
            .as_ref()
            .expect("control survives the write");
        assert_eq!(c2.mode, SwitchedShuntMode::Discrete);
        assert_eq!(c2.control_bus, Some(BusId(7)));
        assert_eq!(c2.blocks.len(), 2);
        close(c2.blocks[0].b, 25.0);
        close(net2.shunts()[0].b, 19.0);
    }

    #[test]
    fn fresh_emission_preserves_the_psse_switched_shunt_control_code() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
3,'B3          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN SWITCHED SHUNT DATA
3, 3, 0, 1, 1.0, 0.0, 0, 100.0, '', 0.0, 1, 10.0
0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA
Q
";
        let net = parse_psse(raw).unwrap();
        let shunt = &net.shunts()[0];
        assert_eq!(
            shunt.control.as_ref().unwrap().mode,
            SwitchedShuntMode::Discrete
        );
        assert_eq!(extra_i64(&shunt.extras, "psse_modsw"), Some(3));

        let text = write_psse(&net).text;
        assert!(text.lines().any(|line| line.starts_with("3, 3, 0, 1,")));
        let reparsed = parse_psse(&text).unwrap();
        assert_eq!(
            extra_i64(&reparsed.shunts()[0].extras, "psse_modsw"),
            Some(3)
        );

        let mut changed = net;
        changed.shunts_mut()[0].control.as_mut().unwrap().mode = SwitchedShuntMode::Continuous;
        let changed_text = write_psse(&changed).text;
        assert!(
            changed_text
                .lines()
                .any(|line| line.starts_with("3, 1, 0, 1,"))
        );
    }

    #[test]
    fn fresh_emission_preserves_the_switched_shunt_adjustment_method() {
        for (rev, record) in [
            (
                33,
                "3, 2, 1, 1, 1.05, 0.95, 0, 100.0, '', 0.0, 1, -10.0, 1, 15.0",
            ),
            (
                35,
                "3, 'S1', 2, 1, 1, 1.05, 0.95, 0, 0, 100.0, 'remote-id', 0.0, 0, 1, -10.0, 1, 1, 15.0",
            ),
        ] {
            let raw = format!(
                "0, 100.00, {rev}, 0, 0, 60.00 / adjustment method\n\
                 CASE\n\
                 COMMENT\n\
                 3,'B3',230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
                 0 / END OF BUS DATA, BEGIN LOAD DATA\n\
                 0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\n\
                 0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA\n\
                 0 / END OF GENERATOR DATA, BEGIN BRANCH DATA\n\
                 0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA\n\
                 0 / END OF TRANSFORMER DATA, BEGIN AREA DATA\n\
                 0 / END OF AREA DATA, BEGIN SWITCHED SHUNT DATA\n\
                 {record}\n\
                 0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA\n\
                 Q\n"
            );
            let mut diagnostics = Diagnostics::new();
            let net = parse_psse_source(&raw, None, &mut diagnostics).unwrap();
            assert_eq!(extra_i64(&net.shunts()[0].extras, "psse_adjm"), Some(1));
            if rev >= 35 {
                assert_eq!(
                    net.shunts()[0].extras["psse_rmidnt"],
                    Value::from("remote-id")
                );
                assert!(diagnostics.records().iter().any(|diagnostic| {
                    diagnostic.code() == "READ.PSSE.FIELD_DROPPED"
                        && diagnostic.message().contains("block 1")
                        && diagnostic.message().contains("S=0")
                }));
            }

            let emitted = write_psse_rev(&net, rev).text;
            let record = emitted
                .lines()
                .skip_while(|line| !line.contains("BEGIN SWITCHED SHUNT DATA"))
                .nth(1)
                .expect("fresh output has a switched-shunt record");
            let columns = fields(record);
            let adjm_column = if rev >= 35 { 3 } else { 2 };
            assert_eq!(columns[adjm_column], "1", "revision {rev} ADJM");
            if rev >= 35 {
                assert_eq!(columns[10], "remote-id");
                assert_eq!(columns[12], "1", "fresh output enables retained blocks");
            }

            let reparsed = parse_psse(&emitted).unwrap();
            assert_eq!(
                extra_i64(&reparsed.shunts()[0].extras, "psse_adjm"),
                Some(1),
                "revision {rev} reparsed ADJM"
            );
            if rev >= 35 {
                assert_eq!(
                    reparsed.shunts()[0].extras["psse_rmidnt"],
                    Value::from("remote-id")
                );
            }
        }
    }

    #[test]
    fn v35_switched_shunt_write_round_trips_through_the_id_column() {
        // v35 inserts a quoted shunt ID at field 1 and NREG after SWREG, and its
        // step blocks are (S, N, B) triples; the writer must emit that layout or
        // the reader misplaces every later field. Build a switched shunt, write
        // the v35 layout, and confirm it reads back intact.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
3,'B3          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
7,'B7          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN SWITCHED SHUNT DATA
3, 2, 0, 1, 1.05, 0.95, 7, 100.0, '', 19.0, 2, 25.0, 1, 50.0
0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA
Q
";
        let net = parse_psse(raw).unwrap();
        let text = write_psse_rev(&net, 35).text;
        let net2 = parse_psse(&text).unwrap();
        assert_eq!(net2.shunts().len(), 1);
        let sh = &net2.shunts()[0];
        assert_eq!(sh.bus, BusId(3));
        close(sh.b, 19.0);
        let c = sh
            .control
            .as_ref()
            .expect("v35 switched-shunt control survives the write");
        assert_eq!(c.mode, SwitchedShuntMode::Discrete);
        close(c.vhigh, 1.05);
        close(c.vlow, 0.95);
        assert_eq!(c.control_bus, Some(BusId(7)));
        close(c.rmpct, 100.0);
        assert_eq!(c.blocks.len(), 2);
        close(c.blocks[0].b, 25.0);
        close(c.blocks[1].b, 50.0);
    }

    #[test]
    fn reads_and_writes_a_generator_remote_regulated_bus() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 18.0,2,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
7,'B7          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
3,'1', 50.0, 5.0, 30.0, -20.0, 1.02, 7, 100.0, 0, 1, 0, 0, 1, 1, 100.0, 80.0, 0.0, 1, 1
1,'1', 10.0, 0.0, 10.0, -10.0, 1.0, 0, 100.0, 0, 1, 0, 0, 1, 1, 100.0, 50.0, 0.0, 1, 1
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.generators().len(), 2);
        let g3 = net.generators().iter().find(|g| g.bus == BusId(3)).unwrap();
        assert_eq!(
            g3.regulated_bus,
            Some(BusId(7)),
            "IREG names the remote regulated bus"
        );
        // IREG 0 means own-terminal control: no remote bus.
        let g1 = net.generators().iter().find(|g| g.bus == BusId(1)).unwrap();
        assert_eq!(g1.regulated_bus, None);

        // Round trip: IREG is written at field 7 and re-read intact.
        let text = write_psse(&net).text;
        let net2 = parse_psse(&text).unwrap();
        let g3b = net2
            .generators()
            .iter()
            .find(|g| g.bus == BusId(3))
            .unwrap();
        assert_eq!(g3b.regulated_bus, Some(BusId(7)));
        let g1b = net2
            .generators()
            .iter()
            .find(|g| g.bus == BusId(1))
            .unwrap();
        assert_eq!(g1b.regulated_bus, None);
    }

    #[test]
    fn reads_a_v35_generator_record_with_nreg() {
        // v35 inserts NREG after IREG (and BASLOD after PB), shifting MBASE,
        // STAT, PT, and PB by one. Reading at the v33 offsets takes NREG as
        // MBASE and GTAP (1.0) as STAT, silently returning this out of service
        // unit to service.
        let raw = "0, 100.00, 35, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
1,'1 ',50.0,5.0,20.0,-10.0,1.0,0,2,900.0,0.0,1.0,0.0,0.0,1.0,0,100.0,80.0,10.0,0.0,1,1.0
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.generators().len(), 1);
        let g = &net.generators()[0];
        close(g.mbase, 900.0);
        assert!(!g.in_service, "STAT = 0 at the shifted index");
        close(g.pmax, 80.0);
        close(g.pmin, 10.0);
        assert_eq!(g.regulated_bus, None, "IREG stays at field 7");

        // The v35 writer emits NREG and BASLOD, so the record reads back intact.
        let net2 = parse_psse(&write_psse_rev(&net, 35).text).unwrap();
        let g2 = &net2.generators()[0];
        close(g2.mbase, 900.0);
        assert!(!g2.in_service);
        close(g2.pmax, 80.0);
        close(g2.pmin, 10.0);
    }

    #[test]
    fn stale_control_pointers_warn_and_drop() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 18.0,2,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 13.8,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
1,'1', 50.0, 5.0, 30.0, -20.0, 1.02, 99, 100.0, 0, 1, 0, 0, 1, 1, 100.0, 80.0, 0.0, 1, 1
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 0, '1', 1, 1, 1, 0, 0, 2, 'REG         ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0
1.025, 0, 2.5, 100.0, 90.0, 80.0, 1, 98, 1.08, 0.92, 1.05, 0.98, 17, 0, 0, 0, 0
1.0, 0
1, 2, 3, '3W', 1, 1, 1, 0, 0, 2, 'REG3W       ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0, 0.02, 0.20, 100.0, 0.03, 0.30, 100.0, 1.0, 0.0
1.0, 230.0, 0.0, 100.0, 90.0, 80.0, 1, -95, 1.08, 0.92, 1.05, 0.98, 17, 0, 0, 0, 0
1.0, 18.0, 0.0, 100.0, 90.0, 80.0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
1.0, 13.8, 0.0, 100.0, 90.0, 80.0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
1, 97, 0.0, 0.0, 'AREA        '
0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
0 / END OF VSC DC LINE DATA, BEGIN IMPEDANCE CORRECTION DATA
0 / END OF IMPEDANCE CORRECTION DATA, BEGIN MULTI-TERMINAL DC DATA
0 / END OF MULTI-TERMINAL DC DATA, BEGIN MULTI-SECTION LINE DATA
0 / END OF MULTI-SECTION LINE DATA, BEGIN ZONE DATA
0 / END OF ZONE DATA, BEGIN INTER-AREA TRANSFER DATA
0 / END OF INTER-AREA TRANSFER DATA, BEGIN OWNER DATA
0 / END OF OWNER DATA, BEGIN FACTS DEVICE DATA
0 / END OF FACTS DEVICE DATA, BEGIN SWITCHED SHUNT DATA
2, 2, 0, 1, 1.05, 0.95, 96, 100.0, '', 19.0, 2, 25.0
0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA
Q
";
        let mut warnings = Diagnostics::new();
        let net = parse_psse_source(raw, None, &mut warnings).unwrap();

        assert_eq!(net.generators()[0].regulated_bus, None);
        assert_eq!(
            net.branches()[0]
                .control
                .as_ref()
                .and_then(|c| c.controlled_bus),
            None
        );
        let three_winding_control = net.transformers_3w()[0].windings[0]
            .control
            .as_ref()
            .expect("the winding control remains after its stale pointer is dropped");
        assert_eq!(three_winding_control.controlled_bus, None);
        assert!(!three_winding_control.controlled_bus_on_winding_side);
        assert_eq!(
            net.shunts()[0].control.as_ref().and_then(|c| c.control_bus),
            None
        );
        assert_eq!(net.areas()[0].slack_bus, None);
        assert!(
            warnings.lines().iter().any(|w| w.contains("GENERATOR DATA")
                && w.contains("IREG")
                && w.contains("missing bus id 99")),
            "missing IREG warning: {warnings:?}"
        );
        assert!(
            warnings
                .lines()
                .iter()
                .any(|w| w.contains("TRANSFORMER DATA")
                    && w.contains("CONT")
                    && w.contains("missing bus id 98")),
            "missing CONT warning: {warnings:?}"
        );
        assert!(
            warnings.lines().iter().any(|w| {
                w.contains("winding 1") && w.contains("CONT") && w.contains("missing bus id 95")
            }),
            "missing three winding CONT warning: {warnings:?}"
        );
        assert!(
            warnings
                .lines()
                .iter()
                .any(|w| w.contains("SWITCHED SHUNT DATA")
                    && w.contains("SWREM")
                    && w.contains("missing bus id 96")),
            "missing SWREM warning: {warnings:?}"
        );
        assert!(
            warnings.lines().iter().any(|w| w.contains("AREA DATA")
                && w.contains("ISW")
                && w.contains("missing bus id 97")),
            "missing ISW warning: {warnings:?}"
        );
    }

    #[test]
    fn truncated_transformer_continuation_names_expected_line() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 18.0,2,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 0, '1', 1, 1, 1, 0, 0, 2, 'REG         ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let err = parse_psse(raw).unwrap_err().to_string();
        assert!(
            err.contains("transformer record ended before transformer impedance line"),
            "got {err}"
        );
    }

    #[test]
    fn unmodeled_section_counts_skip_bare_terminators() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
'VSC1', 1
2, 3
0
0 / END OF VSC DC LINE DATA, BEGIN IMPEDANCE CORRECTION DATA
Q
";
        let mut warnings = Diagnostics::new();
        parse_psse_source(raw, None, &mut warnings).unwrap();
        assert!(
            warnings
                .lines()
                .iter()
                .any(|w| w.contains("VSC DC LINE section (2 record line(s))")),
            "bare terminator should not be counted as skipped data: {warnings:?}"
        );
    }

    #[test]
    fn reads_a_v35_switched_shunt_with_an_id_column() {
        // v35: I, ID, MODSW, ADJM, ST, VSWHI, VSWLO, SWREG, NREG, RMPCT, RMIDNT,
        // BINIT, then (S, N, B) triples. Reading it at the v33 offsets misparses
        // VSWLO as SWREM (regression: a real v35 case pointed switched-shunt
        // control at a nonexistent bus 1) and NREG as RMPCT.
        let raw = "0, 100.00, 35, 0, 0, 60.00 / x
CASE
COMMENT
5,'B5          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
7,'B7          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN SWITCHED SHUNT DATA
5,'1 ',2,0,1,1.05,0.95,7,3,80.0,'',19.0,1,2,25.0,0,1,50.0
0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.shunts().len(), 1);
        let sh = &net.shunts()[0];
        assert_eq!(sh.bus, BusId(5));
        close(sh.b, 19.0);
        assert!(sh.in_service);
        let c = sh.control.as_ref().expect("switched-shunt control parsed");
        assert_eq!(c.mode, SwitchedShuntMode::Discrete);
        close(c.vhigh, 1.05);
        close(c.vlow, 0.95);
        assert_eq!(
            c.control_bus,
            Some(BusId(7)),
            "SWREG at field 7, not NREG at 8"
        );
        close(c.rmpct, 80.0);
        // Both (S, N, B) blocks are kept; the leading status column is skipped.
        assert_eq!(c.blocks.len(), 2);
        assert_eq!(c.blocks[0].steps, 2);
        close(c.blocks[0].b, 25.0);
        assert_eq!(c.blocks[1].steps, 1);
        close(c.blocks[1].b, 50.0);
    }

    #[test]
    fn reads_and_writes_a_two_terminal_dc_line() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
4,'B4          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
5,'B5          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
'DCLINE1', 1, 2.5, 350.0, 500.0, 0.0, 0.0, 0.0, 'I', 0.0, 20, 1.0
4, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
5, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.hvdc().len(), 1, "the two-terminal DC line was read");
        let dc = &net.hvdc()[0];
        assert_eq!(dc.from, BusId(4), "rectifier bus is the from end");
        assert_eq!(dc.to, BusId(5), "inverter bus is the to end");
        assert!(dc.in_service);
        close(dc.pf, 350.0);
        // The inverter receives the demand minus the line's own drop:
        // I = 350 MW / 500 kV = 0.7 kA, so I²·RDC = 0.49 · 2.5 = 1.225 MW.
        close(dc.pt, 348.775);
        close(dc.pmax, 350.0);

        // Round trip: write and re-read keeps the buses and both ends' power.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        assert_eq!(net2.hvdc().len(), 1, "the DC line survives the write");
        let dc2 = &net2.hvdc()[0];
        assert_eq!(dc2.from, BusId(4));
        assert_eq!(dc2.to, BusId(5));
        assert!(dc2.in_service);
        close(dc2.pf, 350.0);
        close(dc2.pt, 348.775);
    }

    /// The other two SETVL spellings, and the guard. A negative SETVL under
    /// MDC 1 is the same demand measured at the inverter, so the ends swap
    /// which one carries the drop; under MDC 2 SETVL is amps, which today read
    /// as 2000 MW instead of the 100 MW the schedule prices them at; and with
    /// no scheduled voltage there is no current to price the drop with, so
    /// both ends read as the demand.
    #[test]
    fn dc_line_setvl_modes_price_the_line_drop() {
        let record = |mdc: &str, setvl: &str, vschd: &str| {
            format!(
                "0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
                 1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 4,'B4          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 5,'B5          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 0 / END OF BUS DATA, BEGIN LOAD DATA
                 0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
                 0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
                 0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
                 0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
                 0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
                 0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
                 'DC1', {mdc}, 2.5, {setvl}, {vschd}, 0.0, 0.0, 0.0, 'I', 0.0, 20, 1.0
                 4, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
                 5, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
                 0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
Q
"
            )
        };

        // Inverter-measured demand: the rectifier supplies 350 + 1.225.
        let dc = parse_psse(&record("1", "-350.0", "500.0")).unwrap().hvdc()[0].clone();
        close(dc.pt, 350.0);
        close(dc.pf, 351.225);

        // Current demand: 200 A at 500 kV is 100 MW at the rectifier, and
        // I²·RDC = 0.04 · 2.5 = 0.1 MW of drop. The write leg converts the
        // rectifier power back to amps, so a re-read agrees.
        let net = parse_psse(&record("2", "200.0", "500.0")).unwrap();
        let dc = &net.hvdc()[0];
        close(dc.pf, 100.0);
        close(dc.pt, 99.9);
        let back = parse_psse(&write_psse(&net).text).unwrap();
        close(back.hvdc()[0].pf, 100.0);
        close(back.hvdc()[0].pt, 99.9);

        // No scheduled voltage: no current to price the drop with.
        let dc = parse_psse(&record("1", "350.0", "0.0")).unwrap().hvdc()[0].clone();
        close(dc.pf, 350.0);
        close(dc.pt, 350.0);
    }

    /// Every SETVL spelling must survive a write and a re-read. The negative
    /// one is the trap: writing the rectifier power back instead of the
    /// inverter demand names a different operating point, because the re-read
    /// then prices the line drop off the larger end.
    #[test]
    fn every_setvl_spelling_round_trips() {
        let record = |mdc: &str, setvl: &str, vschd: &str| {
            format!(
                "0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
                 1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 4,'B4          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 5,'B5          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 0 / END OF BUS DATA, BEGIN LOAD DATA
                 0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
                 0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
                 0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
                 0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
                 0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
                 0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
                 'DC1', {mdc}, 2.5, {setvl}, {vschd}, 0.0, 0.0, 0.0, 'I', 0.0, 20, 1.0
                 4, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
                 5, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
                 0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
Q
"
            )
        };

        for (mdc, setvl, vschd) in [
            ("1", "350.0", "500.0"),
            ("1", "-350.0", "500.0"),
            ("2", "200.0", "500.0"),
            ("1", "350.0", "0.0"),
        ] {
            let net = parse_psse(&record(mdc, setvl, vschd)).unwrap();
            let dc = net.hvdc()[0].clone();
            let back = parse_psse(&write_psse(&net).text).unwrap();
            let dc2 = &back.hvdc()[0];
            close(dc2.pf, dc.pf);
            close(dc2.pt, dc.pt);
            assert!(
                (dc2.pf - dc.pf).abs() < 1e-12 && (dc2.pt - dc.pt).abs() < 1e-12,
                "MDC {mdc} SETVL {setvl} VSCHD {vschd} moved: \
                 {} -> {} and {} -> {}",
                dc.pf,
                dc2.pf,
                dc.pt,
                dc2.pt
            );
        }
    }

    /// A current schedule with no scheduled voltage cannot be priced into
    /// power at all. Reading the amps as MW is how a 2000 A schedule becomes a
    /// 2000 MW line, so both ends read as zero, the record is retained, and
    /// the reader says why.
    #[test]
    fn a_current_schedule_with_no_voltage_is_not_read_as_power() {
        let text = "0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
                 1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 4,'B4          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 5,'B5          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 0 / END OF BUS DATA, BEGIN LOAD DATA
                 0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
                 0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
                 0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
                 0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
                 0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
                 0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
                 'DC1', 2, 2.5, 2000.0, 0.0, 0.0, 0.0, 0.0, 'I', 0.0, 20, 1.0
                 4, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
                 5, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
                 0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
Q
";
        let parsed = crate::parse_str(text, "psse").unwrap();
        let dc = &parsed.network.hvdc()[0];
        close(dc.pf, 0.0);
        close(dc.pt, 0.0);
        assert!(
            parsed
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("cannot be priced into power")),
            "{:?}",
            parsed.render_diagnostics()
        );
        // The record still round trips: the amps are retained verbatim.
        let out = write_psse(&parsed.network).text;
        assert!(
            out.lines().any(|l| l.contains("2000")),
            "the schedule must survive the write: {out}"
        );
    }

    /// A blocked line (`MDC = 0`) states one number both its ends read back as.
    /// Pricing the drop model against it anyway made every out of service DC
    /// record earn the dropped-detail warning for a record the rewrite
    /// reproduces exactly.
    #[test]
    fn a_blocked_dc_record_prices_no_drop_and_earns_no_warning() {
        let text = "0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
                 1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 4,'B4          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 5,'B5          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
                 0 / END OF BUS DATA, BEGIN LOAD DATA
                 0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
                 0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
                 0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
                 0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
                 0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
                 0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
                 'DC1', 0, 2.5, 350.0, 500.0, 0.0, 0.0, 0.0, 'I', 0.0, 20, 1.0
                 4, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
                 5, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
                 0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
Q
";
        let net = parse_psse(text).unwrap();
        let dc = net.hvdc()[0].clone();
        assert!(!dc.in_service);
        close(dc.pf, 350.0);
        close(dc.pt, 350.0);

        // Clear the retained source so the write synthesizes rather than echoes.
        let conv = write_psse(&net);
        assert!(
            !conv
                .render_diagnostics()
                .iter()
                .any(|w| w.contains("converter detail")),
            "a record the rewrite reproduces exactly earns no warning: {:?}",
            conv.render_diagnostics()
        );
        let reparsed = parse_psse(&conv.text).unwrap();
        let back = &reparsed.hvdc()[0];
        assert!(!back.in_service);
        close(back.pf, 350.0);
        close(back.pt, 350.0);
    }

    #[test]
    fn reads_and_writes_a_regulating_transformer_control() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 13.8,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 0, '1', 1, 1, 1, 0, 0, 2, 'REG         ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0
1.025, 0, 2.5, 100.0, 90.0, 80.0, 1, 3, 1.08, 0.92, 1.05, 0.98, 17, 0, 0, 0, 0
1.0, 0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.branches().len(), 1);
        let c = net.branches()[0].control.as_ref().expect("control parsed");
        assert_eq!(c.mode, TransformerControlMode::Voltage);
        assert_eq!(c.controlled_bus, Some(BusId(3)));
        close(c.tap_max, 1.08);
        close(c.tap_min, 0.92);
        close(c.band_min, 0.98);
        assert_eq!(c.ntp, 17);
        close(c.mva_base, 100.0);

        // Round trip: write and re-read keeps the control block and the tap/shift.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        let c2 = net2.branches()[0]
            .control
            .as_ref()
            .expect("control survives");
        assert_eq!(c2.mode, TransformerControlMode::Voltage);
        assert_eq!(c2.controlled_bus, Some(BusId(3)));
        close(c2.tap_max, 1.08);
        assert_eq!(c2.ntp, 17);
        close(net2.branches()[0].tap, 1.025);
        close(net2.branches()[0].shift, 2.5);
    }

    #[test]
    fn reads_and_writes_a_three_winding_transformer() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 13.8,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 3, '1', 1, 1, 1, 0.0, 0.0, 2, 'T3W         ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0, 0.02, 0.20, 100.0, 0.03, 0.30, 100.0, 0.98, -1.5
1.0, 230.0, 0.0, 100.0, 90.0, 80.0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
1.025, 138.0, 0.0, 110.0, 0, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
0.95, 13.8, 30.0, 50.0, 0, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(
            net.transformers_3w().len(),
            1,
            "the 3-winding record was read"
        );
        assert!(
            net.branches().is_empty(),
            "a 3W is not folded into branches"
        );
        let t = &net.transformers_3w()[0];
        assert_eq!(
            [t.windings[0].bus, t.windings[1].bus, t.windings[2].bus],
            [BusId(1), BusId(2), BusId(3)]
        );
        close(t.z[0].r, 0.01);
        close(t.z[2].x, 0.30);
        close(t.windings[0].rate_a, 100.0);
        close(t.windings[1].tap, 1.025);
        close(t.windings[2].shift, 30.0);
        close(t.star_vm, 0.98);
        close(t.star_va, -1.5);

        // Round trip: write and re-read keeps the windings and the star voltage.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        assert_eq!(net2.transformers_3w().len(), 1);
        assert!(net2.branches().is_empty());
        let t2 = &net2.transformers_3w()[0];
        close(t2.z[1].x, 0.20);
        close(t2.windings[2].tap, 0.95);
        close(t2.star_va, -1.5);
        assert_eq!(t2.name.as_deref(), Some("T3W"));

        let rev35 = write_psse_rev(&net, 35).text;
        let main = rev35
            .lines()
            .find(|line| line.starts_with("1, 2, 3, '1'"))
            .unwrap();
        let main_fields = fields(main);
        assert_eq!(main_fields.len(), 22, "rev35 transformer row: {main:?}");
        assert_eq!(main_fields[21], "0", "ZCOD must be an integer");

        let dc_control_raw = raw.replacen(
            "1.0, 230.0, 0.0, 100.0, 90.0, 80.0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0",
            "1.0, 230.0, 0.0, 100.0, 90.0, 80.0, 4, 1, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0",
            1,
        );
        let error = parse_psse(&dc_control_raw).unwrap_err().to_string();
        assert!(
            error.contains("COD 4 DC line quantity control")
                && error.contains("valid only for two winding transformers"),
            "{error}"
        );

        let mut invalid_for_output = net;
        let mut control = TransformerControl::new(TransformerControlMode::DcLineQuantity);
        control.controlled_bus = Some(BusId(1));
        invalid_for_output.transformers_3w_mut()[0].windings[0].control = Some(control);
        let emitted = write_psse_rev(&invalid_for_output, 35);
        assert!(
            emitted.render_diagnostics().iter().any(|diagnostic| {
                diagnostic.contains("COD 4 DC line quantity control")
                    && diagnostic.contains("emitted fixed control")
            }),
            "invalid three winding control must be diagnosed: {:?}",
            emitted.render_diagnostics()
        );
        let back = parse_psse(&emitted.text).unwrap();
        assert!(back.transformers_3w()[0].windings[0].control.is_none());
    }

    #[test]
    fn disabled_three_winding_controls_keep_negative_cod_in_v33_and_v35() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / disabled winding controls
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 13.8,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 3, '1', 1, 1, 1, 0.0, 0.0, 2, 'T3-CONTROL  ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0, 0.02, 0.20, 100.0, 0.03, 0.30, 100.0, 1.0, 0.0
1.0, 230.0, 0.0, 100.0, 90.0, 80.0, -1, -1, 1.08, 0.92, 1.05, 0.98, 17, 0, 0, 0, 0
1.0, 138.0, 0.0, 70.0, 60.0, 50.0, -2, 2, 1.07, 0.93, 50.0, -50.0, 19, 0, 0, 0, 0
1.0, 13.8, 0.0, 40.0, 30.0, 20.0, -3, 3, 1.06, 0.94, 60.0, -60.0, 21, 0, 0, 0, 0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        let modes = [
            TransformerControlMode::Voltage,
            TransformerControlMode::ReactiveFlow,
            TransformerControlMode::ActiveFlow,
        ];
        for (winding, mode) in net.transformers_3w()[0].windings.iter().zip(modes) {
            let control = winding.control.as_ref().expect("control parsed");
            assert_eq!(control.mode, mode);
            assert!(!control.enabled);
        }
        let first = net.transformers_3w()[0].windings[0]
            .control
            .as_ref()
            .unwrap();
        assert_eq!(first.controlled_bus, Some(BusId(1)));
        assert!(first.controlled_bus_on_winding_side);

        for revision in [33, 35] {
            let emitted = write_psse_rev(&net, revision).text;
            let back = parse_psse(&emitted).unwrap();
            for (winding, mode) in back.transformers_3w()[0].windings.iter().zip(modes) {
                let control = winding.control.as_ref().expect("control survives");
                assert_eq!(control.mode, mode);
                assert!(!control.enabled, "revision {revision} lost negative COD");
            }
            assert!(
                back.transformers_3w()[0].windings[0]
                    .control
                    .as_ref()
                    .unwrap()
                    .controlled_bus_on_winding_side,
                "revision {revision} lost the negative CONT meaning"
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn dc_line_and_asymmetric_active_power_controls_keep_cod_in_v33_and_v35() {
        // PSS/E 33.0 Program Application Guide table 6-3 defines |COD|=4 as
        // control of a DC line quantity on a two-winding transformer. The
        // PSS/E 35.4.1 Data Formats transformer section also defines |COD|=5
        // as asymmetric active power flow control. The sign enables or
        // suppresses automatic adjustment.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / COD 4 and 5
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 0, '4P', 1, 1, 1, 0, 0, 2, 'DC ENABLED  ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0
1.0, 230.0, 0.0, 100.0, 90.0, 80.0, 4, 0, 1.08, 0.92, 0.0, 0.0, 17, 0, 0, 0, 0
1.0, 138.0
1, 2, 0, '4N', 1, 1, 1, 0, 0, 2, 'DC DISABLED ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0
1.0, 230.0, 0.0, 100.0, 90.0, 80.0, -4, 0, 1.08, 0.92, 0.0, 0.0, 17, 0, 0, 0, 0
1.0, 138.0
1, 2, 0, '5P', 1, 1, 1, 0, 0, 2, 'ASYM ENABLED', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0
1.0, 230.0, 2.0, 100.0, 90.0, 80.0, 5, 0, 15.0, -15.0, 100.0, -100.0, 21, 0, 0, 0, 12.5
1.0, 138.0
1, 2, 0, '5N', 1, 1, 1, 0, 0, 2, 'ASYM DISABL ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0
1.0, 230.0, -2.0, 100.0, 90.0, 80.0, -5, 0, 15.0, -15.0, 100.0, -100.0, 21, 0, 0, 0, 0
1.0, 138.0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        let expected = [
            (TransformerControlMode::DcLineQuantity, true),
            (TransformerControlMode::DcLineQuantity, false),
            (TransformerControlMode::AsymmetricActiveFlow, true),
            (TransformerControlMode::AsymmetricActiveFlow, false),
        ];
        assert_eq!(net.branches().len(), expected.len());
        for (branch, (mode, enabled)) in net.branches().iter().zip(expected) {
            let control = branch.control.as_ref().expect("control parsed");
            assert_eq!(control.mode, mode);
            assert_eq!(control.enabled, enabled);
        }
        assert_eq!(
            net.branches()[2]
                .control
                .as_ref()
                .unwrap()
                .winding_connection_angle,
            Some(12.5)
        );

        for revision in [33, 35] {
            let emitted = write_psse_rev(&net, revision).text;
            let back = parse_psse(&emitted).unwrap();
            for (branch, (mode, enabled)) in back.branches().iter().zip(expected) {
                let control = branch.control.as_ref().expect("control survives");
                assert_eq!(control.mode, mode, "revision {revision} changed COD");
                assert_eq!(
                    control.enabled, enabled,
                    "revision {revision} changed the COD sign"
                );
            }
            assert_eq!(
                back.branches()[2]
                    .control
                    .as_ref()
                    .unwrap()
                    .winding_connection_angle,
                Some(12.5),
                "revision {revision} lost CNXA"
            );
        }

        let rawx = crate::format::write_rawx(&net).unwrap();
        let rawx_back =
            crate::format::rawx::parse_rawx_source(&rawx.text, None, &mut Diagnostics::new())
                .unwrap();
        for (branch, (mode, enabled)) in rawx_back.branches().iter().zip(expected) {
            let control = branch.control.as_ref().expect("RAWX control survives");
            assert_eq!(control.mode, mode);
            assert_eq!(control.enabled, enabled);
        }

        let xiidm = crate::format::write_xiidm(&net).unwrap();
        let xiidm_diagnostics = xiidm.render_diagnostics();
        for mode in [
            "DC line quantity control",
            "asymmetric active power flow control",
        ] {
            assert!(
                xiidm_diagnostics.iter().any(|line| line.contains(mode)),
                "XIIDM must diagnose unsupported {mode}: {xiidm_diagnostics:?}"
            );
        }

        let (_, cgmes_diagnostics) = crate::format::cgmes::artifacts(&net).unwrap();
        let dropped_controls = cgmes_diagnostics
            .lines()
            .into_iter()
            .filter(|line| line.contains("automatic tap/phase control"))
            .count();
        assert_eq!(
            dropped_controls,
            expected.len(),
            "CGMES must diagnose each unsupported control"
        );
    }

    #[test]
    fn output_diagnoses_unrepresentable_regulating_terminal_and_invalid_cont_sign() {
        let mut net = BalancedNetwork::in_memory(
            "controls",
            100.0,
            vec![test_bus(1, BusType::Ref), test_bus(2, BusType::Pq)],
            Vec::new(),
        );

        let mut missing_terminal =
            transformer_with_terminal_charging(BranchCharging::new(0.0, 0.0, 0.0, 0.0));
        let mut control = TransformerControl::new(TransformerControlMode::Voltage);
        control.controlled_bus = Some(BusId(2));
        control.regulating_terminal = Some(
            serde_json::from_value(serde_json::json!({
                "equipment": {
                    "component_type": "transformer",
                    "local_id": "not-in-detailed-connectivity"
                },
                "terminal": 2
            }))
            .unwrap(),
        );
        missing_terminal.control = Some(control);
        net.branches_mut().push(missing_terminal);

        let mut missing_bus =
            transformer_with_terminal_charging(BranchCharging::new(0.0, 0.0, 0.0, 0.0));
        missing_bus.from = BusId(2);
        missing_bus.to = BusId(1);
        let mut control = TransformerControl::new(TransformerControlMode::Voltage);
        control.controlled_bus_on_winding_side = true;
        missing_bus.control = Some(control);
        net.branches_mut().push(missing_bus);

        let emitted = write_psse_rev(&net, 35);
        let diagnostics = emitted.render_diagnostics();
        assert!(diagnostics.iter().any(|line| {
            line.contains("regulating terminal has no PSS/E bus and node mapping")
                && line.contains("node 0")
        }));
        assert!(diagnostics.iter().any(|line| {
            line.contains("negative CONT requires a nonzero controlled bus")
                && line.contains("CONT=0")
        }));
        let invalid_control = net.branches()[1].control.as_ref().unwrap();
        let mut direct_diagnostics = Diagnostics::new();
        assert_eq!(
            emit_controlled_bus(
                invalid_control,
                2,
                "test transformer",
                &mut direct_diagnostics
            ),
            0,
            "an unrelated derived bus must not give an absent CONT a meaning"
        );

        let back = parse_psse(&emitted.text).unwrap();
        assert_eq!(
            back.branches()[0].control.as_ref().unwrap().controlled_bus,
            Some(BusId(2))
        );
        assert!(
            !back.branches()[1]
                .control
                .as_ref()
                .unwrap()
                .controlled_bus_on_winding_side
        );
    }

    #[test]
    fn three_winding_cross_format_warns_and_survives_normalization() {
        // Same 3-winding record plus a slack generator so to_normalized has a
        // reference to anchor.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 13.8,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
1,'1 ',50.0,5.0,20.0,-10.0,1.0,0,100.0,0.0,1.0,0.0,0.0,1.0,1,100.0,80.0,10.0
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 3, '1', 1, 1, 1, 0.0, 0.0, 2, 'T3W         ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0, 0.02, 0.20, 100.0, 0.03, 0.30, 100.0, 0.98, -1.5
1.0, 230.0, 0.0, 100.0, 90.0, 80.0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
1.025, 138.0, 0.0, 110.0, 0, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
0.95, 13.8, 30.0, 50.0, 0, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.transformers_3w().len(), 1);

        // Cross-format write to MATPOWER drops the 3W but must report it, not drop
        // it silently.
        let mpc = crate::format::emit_value_text(&net, crate::TargetFormat::Matpower).unwrap();
        assert!(
            mpc.render_diagnostics()
                .iter()
                .any(|w| w.contains("3-winding")),
            "MATPOWER write must warn on the dropped 3-winding transformer, got {:?}",
            mpc.render_diagnostics()
        );

        // The normalized form keeps the 3-winding transformer.
        let norm = net.to_normalized().unwrap();
        assert_eq!(
            norm.transformers_3w().len(),
            1,
            "to_normalized keeps the 3W"
        );
        norm.validate().unwrap();
    }

    #[test]
    fn writing_a_different_revision_re_emits_instead_of_echoing() {
        // A PSS/E v33 source echoes byte-for-byte when written back as v33, but a
        // request for v34 must re-emit the v34 layout, not return the v33 bytes.
        let raw = "0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let source =
            powerio_core::Source::from_memory("case.raw", raw.as_bytes().to_vec()).unwrap();
        let module =
            crate::format::parse(source.with_format(powerio_core::FormatId::new("psse").unwrap()))
                .unwrap();
        let same =
            crate::format::emit_text(&module, crate::TargetFormat::Psse { rev: 33 }).unwrap();
        assert_eq!(same.text, raw, "same revision echoes the retained source");
        let v34 = crate::format::emit_text(&module, crate::TargetFormat::Psse { rev: 34 }).unwrap();
        assert_ne!(v34.text, raw, "a different revision must re-emit, not echo");
        assert!(
            v34.text.contains("END OF SYSTEM-WIDE DATA"),
            "v34 output carries the system-wide marker, got:\n{}",
            v34.text
        );
    }

    #[test]
    fn revision_34_substation_section_is_modeled() {
        // PSS/E revision 34 added the node breaker extension. Even a substation
        // without node rows remains a typed substation rather than being
        // reported as an unsupported section.
        let raw = "0, 100.00, 34, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN SUBSTATION DATA
1, 'SUB1', 21.3, -157.8, 0.001
1, 'N1', 1, 1, 1.0, 0.0
2, 'N2', 1, 1, 1.0, 0.0
0 / END OF SUBSTATION NODE DATA, BEGIN SUBSTATION SWITCHING DEVICE DATA
1, 2, 'S1', 'BREAKER', 2, 1, 1, 0.0001, 100, 90, 80
0 / END OF SUBSTATION SWITCHING DEVICE DATA, BEGIN SUBSTATION EQUIPMENT TERMINAL DATA
0 / END OF SUBSTATION EQUIPMENT TERMINAL DATA
0 / END OF SUBSTATION DATA, BEGIN GNE DEVICE DATA
Q
";
        let parsed = crate::parse_str(raw, "psse").unwrap();
        let detailed = parsed
            .network
            .detailed_connectivity()
            .as_deref()
            .expect("revision 34 substation detail parsed");
        assert_eq!(detailed.substations.len(), 1);
        assert_eq!(detailed.connectivity_nodes.len(), 2);
        assert_eq!(detailed.switches.len(), 1);
        assert!(
            parsed
                .render_diagnostics()
                .iter()
                .all(|line| { !(line.contains("SUBSTATION") && line.contains("not modeled")) })
        );

        let emitted = write_psse_rev(&parsed.network, 34);
        assert!(emitted.diagnostics.iter().all(|diagnostic| {
            !diagnostic
                .message()
                .contains("detailed connectivity dropped")
        }));
        let reparsed = parse_psse(&emitted.text).unwrap();
        let detailed = reparsed.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.substations.len(), 1);
        assert_eq!(detailed.connectivity_nodes.len(), 2);
        assert_eq!(detailed.switches.len(), 1);
    }

    #[test]
    fn revision_35_substation_connectivity_survives_fresh_emission() {
        let raw = "0, 100.00, 35, 0, 1, 60.00 / x
CASE
COMMENT
1,'B1',230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2',230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
1,'G1',10,0,10,-10,1,0,0,100,0,1,0,0,1,1,100,20,0,0,1,1
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
1,2,'L1',0.01,0.1,0.0,100,100,100,0,0,0,0,1
0 / END OF BRANCH DATA, BEGIN SYSTEM SWITCHING DEVICE DATA
0 / END OF SYSTEM SWITCHING DEVICE DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN SWITCHED SHUNT DATA
0 / END OF SWITCHED SHUNT DATA, BEGIN INDUCTION MACHINE DATA
0 / END OF INDUCTION MACHINE DATA, BEGIN SUBSTATION DATA
1,'SUB',0,0,0.1
1,'GEN',1,1,1,0
2,'LINE',1,1,1,0
3,'BUSBAR',1,1,1,0
0 / END OF SUBSTATION NODE DATA, BEGIN SUBSTATION SWITCHING DEVICE DATA
1,3,'S1','BREAKER',2,1,1,0,0,0,0
0 / END OF SUBSTATION SWITCHING DEVICE DATA, BEGIN SUBSTATION EQUIPMENT TERMINAL DATA
1,1,'M','G1'
1,2,'B',2,'L1'
0 / END OF SUBSTATION EQUIPMENT TERMINAL DATA
0 / END OF SUBSTATION DATA
Q
";
        let parsed = parse_psse(raw).unwrap();
        let detailed = parsed.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.substations.len(), 1);
        assert_eq!(detailed.connectivity_nodes.len(), 3);
        assert_eq!(detailed.switches.len(), 1);
        assert_eq!(detailed.busbar_sections.len(), 1);

        let emitted = write_psse_rev(&parsed, 35).text;
        assert!(emitted.contains("1, 'G1',"));
        assert!(emitted.contains("1, 2, 'L1',"));
        let reparsed = parse_psse(&emitted).unwrap();
        let detailed = reparsed.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.substations.len(), 1);
        assert_eq!(detailed.connectivity_nodes.len(), 3);
        assert_eq!(detailed.switches.len(), 1);
        assert_eq!(detailed.busbar_sections.len(), 1);
    }

    #[test]
    fn reads_writes_and_drops_an_emergency_voltage_band() {
        // Bus 1 has a distinct EVHI/EVLO (1.2/0.8) vs the normal band (1.1/0.9);
        // bus 2's emergency band equals its normal band.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.2,0.8
2,'B2          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
1,'1 ',50.0,5.0,20.0,-10.0,1.0,0,100.0,0.0,1.0,0.0,0.0,1.0,1,100.0,80.0,10.0
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        let b1 = net.buses().iter().find(|b| b.id == BusId(1)).unwrap();
        assert!(
            b1.evhi.is_some() && b1.evlo.is_some(),
            "distinct band typed"
        );
        close(b1.evhi.unwrap(), 1.2);
        close(b1.evlo.unwrap(), 0.8);
        let b2 = net.buses().iter().find(|b| b.id == BusId(2)).unwrap();
        assert!(
            b2.evhi.is_none() && b2.evlo.is_none(),
            "an emergency band equal to the normal band stays None"
        );

        // Round trip through the PSS/E writer keeps the distinct band.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        let r1 = net2.buses().iter().find(|b| b.id == BusId(1)).unwrap();
        close(r1.evhi.unwrap(), 1.2);
        close(r1.evlo.unwrap(), 0.8);

        // A cross-format write to MATPOWER (single voltage band) reports the drop.
        let mpc = crate::format::emit_value_text(&net, crate::TargetFormat::Matpower).unwrap();
        assert!(
            mpc.render_diagnostics()
                .iter()
                .any(|w| w.contains("emergency voltage band")),
            "MATPOWER write must warn on the dropped emergency band, got {:?}",
            mpc.render_diagnostics()
        );
    }

    #[test]
    fn a_case_name_with_a_terminator_cannot_forge_a_bus_record() {
        // The case name reaches the header and title lines verbatim. A
        // terminator inside it used to end the record, so the rest parsed as
        // bus data and the written file described a network the source never
        // had.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let mut net = parse_psse(raw).unwrap();
        *net.name_mut() =
            "A\n42, 'INJECTED   ', 500.0, 2, 9, 9, 1, 1.0, 0.0, 1.1, 0.9, 1.1, 0.9".to_owned();
        let text = write_psse(&net).text;
        let back = parse_psse(&text).unwrap();
        assert_eq!(back.buses().len(), 1, "forged bus record in:\n{text}");
    }

    #[test]
    fn writes_v34_v35_layouts_that_round_trip() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
2,'1',1,1,1,10.0,5.0,0,0,0,0,1,1,0
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
1,2,'1 ',0.01,0.05,0.001,111.0,90.0,80.0,0,0,0,0,1,1,0,1,1
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();

        for rev in [34u32, 35] {
            let text = write_psse_rev(&net, rev).text;
            // v34+ wraps the globals in a system-wide section with its end marker.
            assert!(
                text.contains("END OF SYSTEM-WIDE DATA, BEGIN BUS DATA"),
                "rev {rev} missing the system-wide marker"
            );
            let header = text.lines().next().unwrap();
            assert!(header.contains(&format!(", {rev}, ")), "header {header:?}");
            // The branch uses the named 12-rating layout (>= 24 comma fields).
            let branch = text.lines().find(|l| l.starts_with("1, 2, '1'")).unwrap();
            assert!(
                branch.split(',').count() >= 24,
                "rev {rev} branch is not the named layout: {branch:?}"
            );

            let back = parse_psse(&text).unwrap();
            assert_eq!(back.buses().len(), 2);
            assert_eq!(back.loads().len(), 1);
            assert_eq!(back.branches().len(), 1);
            close(back.branches()[0].rate_a, 111.0);
            close(back.loads()[0].p, 10.0);
            assert!(back.branches()[0].in_service);
        }

        // The v35 load record carries the trailing LOADTYPE field.
        assert!(
            write_psse_rev(&net, 35).text.contains(", ''"),
            "v35 load should carry a LOADTYPE field"
        );
    }

    #[test]
    fn writer_generator_defaults_do_not_become_source_metadata() {
        let mut net = BalancedNetwork::new("generator defaults", 100.0);
        net.buses_mut().push(test_bus(1, BusType::Ref));
        net.generators_mut().push(Generator::new(BusId(1)));

        for revision in [33, 34, 35] {
            let back = parse_psse(&write_psse_rev(&net, revision).text).unwrap();
            assert!(
                back.detailed_connectivity().is_none(),
                "revision {revision} retained generated generator fields"
            );
        }
    }

    #[test]
    fn writer_sanitizes_bus_names_that_would_corrupt_a_record() {
        // A name with an apostrophe closes the single-quoted field early; a name
        // with '/' truncates the record at the inline-comment delimiter. Either
        // shifts every later column. The writer replaces both and warns, so the
        // second bus's base kV survives the round trip.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
2,'BUS2        ', 138.0000,1,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let mut net = parse_psse(raw).unwrap();
        net.buses_mut()[0].name = Some("O'Brien/X".to_string());

        let conv = write_psse(&net);
        let reparsed = parse_psse(&conv.text).unwrap();

        assert_eq!(reparsed.buses().len(), 2);
        close(reparsed.buses()[0].base_kv, 230.0);
        close(reparsed.buses()[1].base_kv, 138.0);
        let name = reparsed.buses()[0].name.as_deref().unwrap();
        assert!(!name.contains('\'') && !name.contains('/'), "got {name:?}");
        assert!(
            conv.render_diagnostics()
                .iter()
                .any(|w| w.contains("quoted PSS/E field")),
            "expected a sanitization warning, got {:?}",
            conv.render_diagnostics()
        );
    }

    #[test]
    fn malformed_first_bus_id_is_not_treated_as_system_wide_data() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic malformed export
CASE
COMMENT
BAD,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / END OF BUS DATA, BEGIN LOAD DATA
Q
";

        let err = parse_psse(raw).unwrap_err();

        assert!(
            err.to_string().contains("bus record missing numeric id"),
            "malformed bus id should be reported directly: {err}"
        );
    }

    #[test]
    fn a_bus_id_past_the_int64_ceiling_is_refused() {
        // The id column is read as f64, and an unchecked cast would saturate:
        // two distinct ids above the ceiling would collapse onto one reported
        // int64 id at the C ABI. The shared range policy refuses them at the
        // reader boundary.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic out-of-range export
CASE
COMMENT
1e300,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
2,'BUS2        ', 138.0000,1,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / END OF BUS DATA, BEGIN LOAD DATA
Q
";

        let err = parse_psse(raw).unwrap_err();

        let text = err.to_string();
        assert!(
            text.contains("bus field I") && text.contains("outside the id range"),
            "an out-of-range bus id should be refused: {err}"
        );
    }
}
