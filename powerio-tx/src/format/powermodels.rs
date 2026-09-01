//! Write a [`BalancedNetwork`] as PowerModels.jl network data JSON.
//!
//! Output is idiomatic PowerModels data with `per_unit = true`, the same form
//! PowerModels itself exports: powers are divided by `baseMVA`, angles are in
//! radians, and gen cost coefficients are rescaled to the per-unit basis (a
//! polynomial term `p^j` by `baseMVA^j`, a piecewise curve's MW breakpoints by
//! `1/baseMVA`). Because the data already declares per unit, PowerModels reads
//! it with its default `validate = true` without rerunning `make_per_unit!`, so
//! it lands on the same network as the original MATPOWER case.
//! Loads and shunts are first-class on the `BalancedNetwork`; branch terminal admittance
//! writes as PowerModels' `g_fr`/`b_fr`/`g_to`/`b_to` fields, with MATPOWER
//! `BR_B` expanded only when no richer terminal model is present. `transformer`
//! follows PowerModels' rule (raw tap `≠ 0`). `hvdc` maps onto `dcline` field
//! for field; `storage` is mapped to the closest PowerModels block and emits a
//! warning when present.
//!
//! The typed reader treats an explicit JSON `null` (or a value the lenient
//! decoders cannot read) exactly like an absent key, uniformly: `bus_i: null`
//! falls back to `index` the way a missing `bus_i` does, a null cost `model`
//! drops the cost block, all-null ratings or solution fields read as unstated,
//! and a null switch `state` falls through to `status`. The 0.9 tree walk was
//! sensitive to key presence at those spots; one rule replaces the four
//! special cases.

use serde_json::{Map, Value};

use super::decode::{
    lenient_bool, lenient_f64, lenient_flag, lenient_i64, lenient_string, lenient_table,
    lenient_u64, sorted_rows,
};
use super::{TextEmission, finish, jnum, warn_extra_branch_rating_sets};
use crate::diagnostics::codes::EMIT_POWERMODELS as F;
use crate::diagnostics::{Diagnostics, codes};
use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, BranchCharging, BranchCurrentRatings,
    BranchSolution, Bus, BusId, BusType, GEN_EXTRA_KEYS, GenCost, Generator, Hvdc, Load,
    LoadVoltageModel, Shunt, SourceFormat, Storage, Switch,
};
use crate::normalize::{self, GEN_PU_KEYS};
use crate::{Error, Result};

#[must_use]
pub fn write_powermodels_json(net: &BalancedNetwork) -> TextEmission {
    let mut warnings = Diagnostics::new();

    // Per-unit write factors, the exact inverse of the reader's pscale/ascale:
    // powers ÷ baseMVA, angles degrees → radians. Cost rescale needs the base.
    let base = net.base_mva();
    let p = 1.0 / base;
    let a = normalize::DEG_TO_RAD;

    let mut bus = Map::new();
    for b in net.buses() {
        bus.insert(b.id.to_string(), bus_obj(b, a));
    }

    let mut branch = Map::new();
    for (i, br) in net.branches().iter().enumerate() {
        let idx = i + 1;
        branch.insert(idx.to_string(), branch_obj(br, idx, p, a));
    }

    let mut gen_map = Map::new();
    for (i, g) in net.generators().iter().enumerate() {
        let idx = i + 1;
        gen_map.insert(idx.to_string(), gen_obj(g, idx, p, base));
    }

    let mut load = Map::new();
    for (i, l) in net.loads().iter().enumerate() {
        let idx = i + 1;
        load.insert(idx.to_string(), load_obj(l, idx, p));
    }
    let mut shunt = Map::new();
    for (i, s) in net.shunts().iter().enumerate() {
        let idx = i + 1;
        shunt.insert(idx.to_string(), shunt_obj(s, idx, p));
    }

    let mut dcline = Map::new();
    for (i, dc) in net.hvdc().iter().enumerate() {
        let idx = i + 1;
        dcline.insert(idx.to_string(), dcline_obj(dc, idx, p));
    }
    let mut storage = Map::new();
    for (i, st) in net.storage().iter().enumerate() {
        let idx = i + 1;
        storage.insert(idx.to_string(), storage_obj(st, idx, p));
    }
    let mut switch = Map::new();
    for (i, sw) in net.switches().iter().enumerate() {
        let idx = i + 1;
        switch.insert(idx.to_string(), switch_obj(sw, idx, p));
    }
    if !storage.is_empty() {
        warnings.push(
            &F.value_collapsed,
            format!(
                "{} storage unit(s) mapped with warnings to the PowerModels storage schema",
                storage.len()
            ),
        );
    }
    if !net.transformers_3w().is_empty() {
        warnings.push(&F.record_dropped, format!(
            "{} 3-winding transformer(s) dropped: the PowerModels JSON writer emits no 3-winding record",
            net.transformers_3w().len()
        ));
    }
    super::warn_dropped_areas(&F, "PowerModels JSON", net, &mut warnings);
    let voltage_loads = net
        .loads()
        .iter()
        .filter(|l| {
            l.voltage_model
                .as_ref()
                .is_some_and(LoadVoltageModel::has_non_matpower_fields)
        })
        .count();
    if voltage_loads > 0 {
        warnings.push(&F.field_dropped, format!(
            "{voltage_loads} voltage dependent load model(s) dropped: PowerModels load records carry static pd/qd only"
        ));
    }
    warn_extra_branch_rating_sets(&F, "PowerModels JSON", net, &mut warnings);
    if net
        .buses()
        .iter()
        .any(|b| b.evhi.is_some() || b.evlo.is_some())
    {
        warnings.push(
            &F.field_dropped,
            "emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band",
        );
    }

    let mut root = Map::new();
    root.insert("name".into(), Value::String(net.name().clone()));
    root.insert("baseMVA".into(), jnum(net.base_mva()));
    root.insert("per_unit".into(), Value::Bool(true));
    root.insert("source_type".into(), Value::String("matpower".into()));
    root.insert("source_version".into(), Value::String("2".into()));
    root.insert("bus".into(), Value::Object(bus));
    root.insert("branch".into(), Value::Object(branch));
    root.insert("gen".into(), Value::Object(gen_map));
    root.insert("load".into(), Value::Object(load));
    root.insert("shunt".into(), Value::Object(shunt));
    root.insert("dcline".into(), Value::Object(dcline));
    root.insert("storage".into(), Value::Object(storage));
    root.insert("switch".into(), Value::Object(switch));

    finish(&F, root, warnings)
}

/// PowerModels back-reference `["bus"|"branch"|…, index]`.
fn source_id(kind: &str, idx: usize) -> Value {
    Value::Array(vec![Value::String(kind.into()), Value::from(idx as u64)])
}

fn status_int(in_service: bool) -> Value {
    Value::from(u64::from(in_service))
}

fn bus_obj(b: &Bus, a: f64) -> Value {
    let mut m = Map::new();
    m.insert("bus_i".into(), Value::from(b.id.0 as u64));
    m.insert("index".into(), Value::from(b.id.0 as u64));
    m.insert("bus_type".into(), Value::from(u64::from(b.kind as u8)));
    m.insert("vm".into(), jnum(b.vm));
    m.insert("va".into(), jnum(b.va * a));
    m.insert("vmax".into(), jnum(b.vmax));
    m.insert("vmin".into(), jnum(b.vmin));
    m.insert("base_kv".into(), jnum(b.base_kv));
    m.insert("area".into(), Value::from(b.area as u64));
    m.insert("zone".into(), Value::from(b.zone as u64));
    if let Some(name) = &b.name {
        m.insert("name".into(), Value::String(name.clone()));
    }
    m.insert("source_id".into(), source_id("bus", b.id.0));
    Value::Object(m)
}

fn branch_obj(br: &Branch, idx: usize, p: f64, a: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("f_bus".into(), Value::from(br.from.0 as u64));
    m.insert("t_bus".into(), Value::from(br.to.0 as u64));
    m.insert("br_r".into(), jnum(br.r));
    m.insert("br_x".into(), jnum(br.x));
    let charging = br.calc_terminal_charging();
    m.insert("b_fr".into(), jnum(charging.b_fr));
    m.insert("b_to".into(), jnum(charging.b_to));
    m.insert("g_fr".into(), jnum(charging.g_fr));
    m.insert("g_to".into(), jnum(charging.g_to));
    m.insert("tap".into(), jnum(br.calc_effective_tap()));
    m.insert("shift".into(), jnum(br.shift * a));
    m.insert("br_status".into(), status_int(br.in_service));
    m.insert("angmin".into(), jnum(br.angmin * a));
    m.insert("angmax".into(), jnum(br.angmax * a));
    // PowerModels' rule: a transformer is a branch with an off-nominal raw tap.
    // A pure phase shifter (tap 0, shift ≠ 0) is not flagged, matching matpower.jl.
    m.insert("transformer".into(), Value::Bool(br.tap != 0.0));
    // PowerModels omits a rate when it is 0 (unlimited).
    if br.rate_a != 0.0 {
        m.insert("rate_a".into(), jnum(br.rate_a * p));
    }
    if br.rate_b != 0.0 {
        m.insert("rate_b".into(), jnum(br.rate_b * p));
    }
    if br.rate_c != 0.0 {
        m.insert("rate_c".into(), jnum(br.rate_c * p));
    }
    if let Some(current) = br.current_ratings {
        if current.c_rating_a != 0.0 {
            m.insert("c_rating_a".into(), jnum(current.c_rating_a));
        }
        if current.c_rating_b != 0.0 {
            m.insert("c_rating_b".into(), jnum(current.c_rating_b));
        }
        if current.c_rating_c != 0.0 {
            m.insert("c_rating_c".into(), jnum(current.c_rating_c));
        }
    }
    if let Some(solution) = br.solution {
        m.insert("pf".into(), jnum(solution.pf * p));
        m.insert("qf".into(), jnum(solution.qf * p));
        m.insert("pt".into(), jnum(solution.pt * p));
        m.insert("qt".into(), jnum(solution.qt * p));
    }
    m.insert("source_id".into(), source_id("branch", idx));
    Value::Object(m)
}

fn gen_obj(g: &Generator, idx: usize, p: f64, base: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("gen_bus".into(), Value::from(g.bus.0 as u64));
    m.insert("pg".into(), jnum(g.pg * p));
    m.insert("qg".into(), jnum(g.qg * p));
    m.insert("qmax".into(), jnum(g.qmax * p));
    m.insert("qmin".into(), jnum(g.qmin * p));
    m.insert("vg".into(), jnum(g.vg));
    m.insert("mbase".into(), jnum(g.mbase));
    m.insert("gen_status".into(), status_int(g.in_service));
    m.insert("pmax".into(), jnum(g.pmax * p));
    m.insert("pmin".into(), jnum(g.pmin * p));
    // Gen capability columns, in PowerModels' field order, for those present. Only
    // the ramp rates are per-unitized; the PQ curve points and apf stay raw.
    for (i, key) in GEN_EXTRA_KEYS.iter().enumerate() {
        if let Some(v) = g.caps[i] {
            let scaled = if GEN_PU_KEYS.contains(key) {
                jnum(v * p)
            } else {
                jnum(v)
            };
            m.insert((*key).into(), scaled);
        }
    }
    if let Some(cost) = &g.cost {
        let coeffs: Vec<Value> = normalize::cost_to_pu(cost, base)
            .into_iter()
            .map(jnum)
            .collect();
        // Emit `ncost` consistent with the coefficients actually written. The reader
        // un-scales by the array length, so a mismatched `ncost` (from a malformed
        // row that claimed more coefficients than it carried) would reconstruct the
        // wrong polynomial degree.
        let ncost = if cost.model == 1 {
            coeffs.len() / 2
        } else {
            coeffs.len()
        };
        m.insert("model".into(), Value::from(u64::from(cost.model)));
        m.insert("ncost".into(), Value::from(ncost as u64));
        m.insert("startup".into(), jnum(cost.startup));
        m.insert("shutdown".into(), jnum(cost.shutdown));
        m.insert("cost".into(), Value::Array(coeffs));
    }
    m.insert("source_id".into(), source_id("gen", idx));
    Value::Object(m)
}

fn load_obj(l: &Load, idx: usize, p: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("load_bus".into(), Value::from(l.bus.0 as u64));
    m.insert("pd".into(), jnum(l.p * p));
    m.insert("qd".into(), jnum(l.q * p));
    m.insert("status".into(), status_int(l.in_service));
    m.insert("source_id".into(), source_id("bus", l.bus.0));
    Value::Object(m)
}

fn shunt_obj(s: &Shunt, idx: usize, p: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("shunt_bus".into(), Value::from(s.bus.0 as u64));
    m.insert("gs".into(), jnum(s.g * p));
    m.insert("bs".into(), jnum(s.b * p));
    m.insert("status".into(), status_int(s.in_service));
    m.insert("source_id".into(), source_id("bus", s.bus.0));
    Value::Object(m)
}

/// One PowerModels `dcline` entry.
///
/// Every field [`Hvdc`] holds has a slot here — the four power/reactive flows,
/// both terminal voltages, the aggregate bounds, the four reactive limits, the
/// loss model, and the cost curve — so this mapping reports no loss. The
/// per-end active bounds PowerModels adds are derived below and do not displace
/// the aggregate pair, which rides `mp_pmin`/`mp_pmax` and reads back exactly.
fn dcline_obj(dc: &Hvdc, idx: usize, p: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("f_bus".into(), Value::from(dc.from.0 as u64));
    m.insert("t_bus".into(), Value::from(dc.to.0 as u64));
    m.insert("br_status".into(), status_int(dc.in_service));
    m.insert("pf".into(), jnum(dc.pf * p));
    // MATPOWER uses the opposite sign for Pt/Qf/Qt; PowerModels flips them.
    m.insert("pt".into(), jnum(-dc.pt * p));
    m.insert("qf".into(), jnum(-dc.qf * p));
    m.insert("qt".into(), jnum(-dc.qt * p));
    m.insert("vf".into(), jnum(dc.vf));
    m.insert("vt".into(), jnum(dc.vt));
    // Per-end active-power bounds, derived from the aggregate Pmin/Pmax and the
    // loss model exactly as PowerModels' matpower loader does (_mp2pm_dcline!), so
    // the line reads back through PowerModels' own correct_dclines! pass. Derived
    // in raw MW, then per-unitized like everything else.
    let (pminf, pmaxf, pmint, pmaxt) = dcline_p_bounds(dc.pmin, dc.pmax, dc.loss0, dc.loss1);
    m.insert("pminf".into(), jnum(pminf * p));
    m.insert("pmaxf".into(), jnum(pmaxf * p));
    m.insert("pmint".into(), jnum(pmint * p));
    m.insert("pmaxt".into(), jnum(pmaxt * p));
    // The original aggregate bounds, kept raw, as PowerModels does.
    m.insert("mp_pmin".into(), jnum(dc.pmin));
    m.insert("mp_pmax".into(), jnum(dc.pmax));
    m.insert("qminf".into(), jnum(dc.qminf * p));
    m.insert("qmaxf".into(), jnum(dc.qmaxf * p));
    m.insert("qmint".into(), jnum(dc.qmint * p));
    m.insert("qmaxt".into(), jnum(dc.qmaxt * p));
    m.insert("loss0".into(), jnum(dc.loss0 * p));
    m.insert("loss1".into(), jnum(dc.loss1));
    if let Some(cost) = &dc.cost {
        let coeffs: Vec<Value> = normalize::cost_to_pu(cost, 1.0 / p)
            .into_iter()
            .map(jnum)
            .collect();
        let ncost = if cost.model == 1 {
            coeffs.len() / 2
        } else {
            coeffs.len()
        };
        m.insert("model".into(), Value::from(u64::from(cost.model)));
        m.insert("ncost".into(), Value::from(ncost as u64));
        m.insert("startup".into(), jnum(cost.startup));
        m.insert("shutdown".into(), jnum(cost.shutdown));
        m.insert("cost".into(), Value::Array(coeffs));
    }
    m.insert("source_id".into(), source_id("dcline", idx));
    Value::Object(m)
}

/// Per-end active-power bounds `(pminf, pmaxf, pmint, pmaxt)` for an HVDC line,
/// from the aggregate Pmin/Pmax and the loss model, branching on the bound signs
/// exactly as PowerModels' `_mp2pm_dcline!` does. Inputs and outputs are raw MW.
fn dcline_p_bounds(pmin: f64, pmax: f64, loss0: f64, loss1: f64) -> (f64, f64, f64, f64) {
    let l = 1.0 - loss1;
    if pmin >= 0.0 && pmax >= 0.0 {
        (pmin, pmax, loss0 - pmax * l, loss0 - pmin * l)
    } else if pmin >= 0.0 {
        (pmin, (-pmax + loss0) / l, pmax, loss0 - pmin * l)
    } else if pmax >= 0.0 {
        ((pmin + loss0) / l, pmax, loss0 - pmax * l, -pmin)
    } else {
        ((pmin + loss0) / l, (-pmax + loss0) / l, pmax, -pmin)
    }
}

fn storage_obj(st: &Storage, idx: usize, p: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("storage_bus".into(), Value::from(st.bus.0 as u64));
    // ps/qs are the dispatch setpoint; PowerModels' make_per_unit! leaves them raw
    // (it rescales the energy/ratings/limits below), so we do too.
    m.insert("ps".into(), jnum(st.ps));
    m.insert("qs".into(), jnum(st.qs));
    m.insert("energy".into(), jnum(st.energy * p));
    m.insert("energy_rating".into(), jnum(st.energy_rating * p));
    m.insert("charge_rating".into(), jnum(st.charge_rating * p));
    m.insert("discharge_rating".into(), jnum(st.discharge_rating * p));
    m.insert("charge_efficiency".into(), jnum(st.charge_efficiency));
    m.insert("discharge_efficiency".into(), jnum(st.discharge_efficiency));
    m.insert("thermal_rating".into(), jnum(st.thermal_rating * p));
    if let Some(current_rating) = st.current_rating {
        m.insert("current_rating".into(), jnum(current_rating));
    }
    m.insert("qmin".into(), jnum(st.qmin * p));
    m.insert("qmax".into(), jnum(st.qmax * p));
    m.insert("r".into(), jnum(st.r));
    m.insert("x".into(), jnum(st.x));
    m.insert("p_loss".into(), jnum(st.p_loss * p));
    m.insert("q_loss".into(), jnum(st.q_loss * p));
    m.insert("status".into(), status_int(st.in_service));
    m.insert("source_id".into(), source_id("storage", idx));
    Value::Object(m)
}

fn switch_obj(sw: &Switch, idx: usize, p: f64) -> Value {
    let mut m = Map::new();
    m.insert("index".into(), Value::from(idx as u64));
    m.insert("f_bus".into(), Value::from(sw.from.0 as u64));
    m.insert("t_bus".into(), Value::from(sw.to.0 as u64));
    m.insert("state".into(), status_int(sw.closed));
    if let Some(rating) = sw.thermal_rating {
        m.insert("thermal_rating".into(), jnum(rating * p));
    }
    if let Some(rating) = sw.current_rating {
        m.insert("current_rating".into(), jnum(rating));
    }
    if let Some(pf) = sw.pf {
        m.insert("pf".into(), jnum(pf * p));
    }
    if let Some(qf) = sw.qf {
        m.insert("qf".into(), jnum(qf * p));
    }
    if let Some(pt) = sw.pt {
        m.insert("pt".into(), jnum(pt * p));
    }
    if let Some(qt) = sw.qt {
        m.insert("qt".into(), jnum(qt * p));
    }
    m.insert("source_id".into(), source_id("switch", idx));
    Value::Object(m)
}

// ---- Reader: PowerModels JSON → BalancedNetwork -------------------------------------

const FMT: &str = "PowerModels JSON";

/// Parse PowerModels.jl network data JSON into a [`BalancedNetwork`]. Loads and shunts
/// are read as separate elements and the raw text is retained, so writing back
/// to PowerModels JSON is a byte-exact echo. `per_unit = true` input (powerio's own
/// output, and PowerModels' own export) is converted to the neutral MW/degree
/// convention (powers ×baseMVA, angles to degrees, cost coefficients un-scaled),
/// following PowerModels' own exceptions (storage `ps`/`qs` stay raw, dcline
/// `pt`/`qf`/`qt` flip sign); `per_unit = false` is read as-is.
/// Owned-source entry used by the format hub: parse by borrowing `source`, then
/// move the buffer into the retained source (no copy). `name_hint` (e.g. a file
/// stem) names the network when the JSON carries no `name`.
pub(crate) fn parse_powermodels_json_source(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    // Typed decoding, no generic JSON tree: every known field lands in its
    // struct slot and only the unknown remainder of each element is retained
    // as extras, so peak memory is the model plus per-element leftovers
    // rather than a full DOM beside the source (#293).
    let document: Document = serde_json::from_str(source).map_err(|e| Error::FormatRead {
        format: FMT,
        message: e.to_string(),
    })?;

    // `baseMVA` is every per-unit divisor here; zero, negative, or non-finite
    // would silently poison the scaled quantities with NaN/Inf or flipped
    // signs, so reject it at the door.
    let base_mva = document
        .base_mva
        .filter(|b| b.is_finite() && *b > 0.0)
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "missing, nonpositive, or non-finite numeric `baseMVA`".into(),
        })?;
    let per_unit = document.per_unit.unwrap_or(false);
    if document.multinetwork.unwrap_or(false) {
        warnings.push(
            &codes::READ_POWERMODELS_RECORD_DROPPED,
            "multinetwork=true: only the top-level single snapshot was read",
        );
    }
    let pscale = if per_unit { base_mva } else { 1.0 };
    let ascale = if per_unit { normalize::RAD_TO_DEG } else { 1.0 };
    let name = document
        .name
        .as_deref()
        .or(name_hint)
        .unwrap_or("case")
        .to_string();

    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: None,
        case_metadata: crate::network::CaseMetadata::default(),
        detailed_connectivity: None,
        buses: sorted_rows(document.bus, |row| row.index)
            .into_iter()
            .map(|(_, row)| read_bus(row, ascale))
            .collect::<Result<Vec<_>>>()?
            .into(),
        loads: sorted_rows(document.load, |row| row.index)
            .into_iter()
            .map(|(_, row)| read_load(row, pscale))
            .collect::<Vec<_>>()
            .into(),
        shunts: sorted_rows(document.shunt, |row| row.index)
            .into_iter()
            .map(|(_, row)| read_shunt(row, pscale))
            .collect::<Vec<_>>()
            .into(),
        static_var_compensators: Vec::new().into(),
        branches: read_branches(document.branch, pscale, ascale, warnings).into(),
        switches: sorted_rows(document.switch, |row| row.index)
            .into_iter()
            .map(|(_, row)| read_switch(row, pscale))
            .collect::<Vec<_>>()
            .into(),
        generators: sorted_rows(document.generators, |row| row.index)
            .into_iter()
            .map(|(_, row)| read_gen(&row, pscale, base_mva, per_unit))
            .collect::<Vec<_>>()
            .into(),
        storage: sorted_rows(document.storage, |row| row.index)
            .into_iter()
            .map(|(_, row)| read_storage(row, pscale))
            .collect::<Vec<_>>()
            .into(),
        hvdc: sorted_rows(document.dcline, |row| row.index)
            .into_iter()
            .map(|(_, row)| read_hvdc(row, pscale, base_mva, per_unit))
            .collect::<Vec<_>>()
            .into(),
        transformers_3w: Vec::new().into(),
        areas: Vec::new().into(),
        solver: None,
        source_format: SourceFormat::PowerModelsJson,
    });
    net.check_references(FMT)?;
    Ok(net)
}

/// The typed document. Unknown top-level sections are ignored, as the tree
/// walk before it ignored them; unknown element fields land in each row's
/// flattened extras.
#[derive(Default, serde::Deserialize)]
struct Document {
    #[serde(rename = "baseMVA", default, deserialize_with = "lenient_f64")]
    base_mva: Option<f64>,
    #[serde(default, deserialize_with = "lenient_bool")]
    per_unit: Option<bool>,
    #[serde(default, deserialize_with = "lenient_bool")]
    multinetwork: Option<bool>,
    #[serde(default, deserialize_with = "lenient_string")]
    name: Option<String>,
    #[serde(default, deserialize_with = "lenient_table")]
    bus: Vec<(String, BusRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    load: Vec<(String, LoadRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    shunt: Vec<(String, ShuntRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    branch: Vec<(String, BranchRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    switch: Vec<(String, SwitchRow)>,
    #[serde(rename = "gen", default, deserialize_with = "lenient_table")]
    generators: Vec<(String, GenRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    storage: Vec<(String, StorageRow)>,
    #[serde(default, deserialize_with = "lenient_table")]
    dcline: Vec<(String, DclineRow)>,
}

fn bustype(code: i64) -> BusType {
    match code {
        2 => BusType::Pv,
        3 => BusType::Ref,
        4 => BusType::Isolated,
        _ => BusType::Pq,
    }
}

#[derive(Default, serde::Deserialize)]
struct BusRow {
    #[serde(default, deserialize_with = "lenient_u64")]
    bus_i: Option<u64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    bus_type: Option<i64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    vm: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    va: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    base_kv: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    vmax: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    vmin: Option<f64>,
    #[serde(default, deserialize_with = "lenient_u64")]
    area: Option<u64>,
    #[serde(default, deserialize_with = "lenient_u64")]
    zone: Option<u64>,
    #[serde(default, deserialize_with = "lenient_string")]
    name: Option<String>,
    #[serde(rename = "source_id", default)]
    _source_id: Option<serde::de::IgnoredAny>,
    #[serde(flatten)]
    extras: crate::network::Extras,
}

fn read_bus(row: BusRow, ascale: f64) -> Result<Bus> {
    let id = row
        .bus_i
        .or_else(|| row.index.and_then(|i| u64::try_from(i).ok()))
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "bus record missing integer `bus_i`".into(),
        })? as usize;
    Ok(Bus {
        id: BusId(id),
        kind: bustype(row.bus_type.unwrap_or(1)),
        vm: row.vm.unwrap_or(1.0),
        va: row.va.unwrap_or(0.0) * ascale,
        base_kv: row.base_kv.unwrap_or(0.0),
        vmax: row.vmax.unwrap_or(0.0),
        vmin: row.vmin.unwrap_or(0.0),
        evhi: None,
        evlo: None,
        area: row.area.unwrap_or(0) as usize,
        zone: row.zone.unwrap_or(0) as usize,
        name: row.name,
        uid: None,
        location: None,
        extras: row.extras,
    })
}

#[derive(Default, serde::Deserialize)]
struct LoadRow {
    #[serde(default, deserialize_with = "lenient_u64")]
    load_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pd: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qd: Option<f64>,
    #[serde(default, deserialize_with = "lenient_flag")]
    status: Option<bool>,
    #[serde(rename = "source_id", default)]
    _source_id: Option<serde::de::IgnoredAny>,
    #[serde(flatten)]
    extras: crate::network::Extras,
}

fn read_load(row: LoadRow, pscale: f64) -> Load {
    Load {
        bus: BusId(row.load_bus.unwrap_or(0) as usize),
        p: row.pd.unwrap_or(0.0) * pscale,
        q: row.qd.unwrap_or(0.0) * pscale,
        voltage_model: None,
        in_service: row.status.unwrap_or(true),
        uid: None,
        extras: row.extras,
    }
}

#[derive(Default, serde::Deserialize)]
struct ShuntRow {
    #[serde(default, deserialize_with = "lenient_u64")]
    shunt_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    gs: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    bs: Option<f64>,
    #[serde(default, deserialize_with = "lenient_flag")]
    status: Option<bool>,
    #[serde(rename = "source_id", default)]
    _source_id: Option<serde::de::IgnoredAny>,
    #[serde(flatten)]
    extras: crate::network::Extras,
}

fn read_shunt(row: ShuntRow, pscale: f64) -> Shunt {
    Shunt {
        bus: BusId(row.shunt_bus.unwrap_or(0) as usize),
        g: row.gs.unwrap_or(0.0) * pscale,
        b: row.bs.unwrap_or(0.0) * pscale,
        in_service: row.status.unwrap_or(true),
        section_count: None,
        control: None,
        uid: None,
        extras: row.extras,
    }
}

#[derive(Default, serde::Deserialize)]
struct BranchRow {
    #[serde(default, deserialize_with = "lenient_u64")]
    f_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_u64")]
    t_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    br_r: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    br_x: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    b_fr: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    b_to: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    g_fr: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    g_to: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    rate_a: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    rate_b: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    rate_c: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    c_rating_a: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    c_rating_b: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    c_rating_c: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    tap: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    shift: Option<f64>,
    #[serde(default, deserialize_with = "lenient_bool")]
    transformer: Option<bool>,
    #[serde(default, deserialize_with = "lenient_flag")]
    br_status: Option<bool>,
    #[serde(default, deserialize_with = "lenient_f64")]
    angmin: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    angmax: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pt: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qt: Option<f64>,
    #[serde(rename = "source_id", default)]
    _source_id: Option<serde::de::IgnoredAny>,
    #[serde(flatten)]
    extras: crate::network::Extras,
}

/// Read the branch table, reporting the taps the `transformer` flag makes
/// this reader discard. One aggregated warning names the first few branches
/// and the total: a producer that never sets the flag would otherwise emit
/// one line per transformer.
fn read_branches(
    rows: Vec<(String, BranchRow)>,
    pscale: f64,
    ascale: f64,
    warnings: &mut Diagnostics,
) -> Vec<Branch> {
    const NAMED: usize = 3;
    let mut discarded: Vec<String> = Vec::new();
    let branches = sorted_rows(rows, |row| row.index)
        .into_iter()
        .map(|(key, row)| read_branch(row, pscale, ascale, &key, &mut discarded))
        .collect();
    if !discarded.is_empty() {
        let head = discarded
            .iter()
            .take(NAMED)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let rest = discarded.len().saturating_sub(NAMED);
        let tail = if rest > 0 {
            format!(" and {rest} more")
        } else {
            String::new()
        };
        warnings.push(
            &codes::READ_POWERMODELS_FIELD_DROPPED,
            format!(
                "{} branch(es) carry an off-nominal `tap` without `transformer: true`, \
             so the tap is discarded and the branch reads as a line: {head}{tail}",
                discarded.len()
            ),
        );
    }
    branches
}

// Exact compare on purpose: only the literal 1.0 carries no information.
// An epsilon compare would silence the warning for real near-unit taps.
#[allow(clippy::float_cmp)]
fn read_branch(
    row: BranchRow,
    pscale: f64,
    ascale: f64,
    key: &str,
    discarded: &mut Vec<String>,
) -> Branch {
    // PowerModels stores the effective tap (1.0 for a line); the `transformer`
    // flag disambiguates an explicit-tap transformer from a line, which is what
    // the neutral raw-tap convention (0 = line) needs.
    let transformer = row.transformer.unwrap_or(false);
    let tap = if transformer {
        row.tap.unwrap_or(1.0)
    } else {
        // The `transformer` flag decides the type, so this rule drops a
        // non-unit tap on an untagged branch. Warn about the drop. Taps of
        // 1 and 0 both mean no off-nominal ratio and stay quiet.
        if let Some(raw) = row.tap {
            if raw != 0.0 && raw != 1.0 {
                discarded.push(format!(
                    "`{key}` ({} -> {}) tap {raw}",
                    row.f_bus.unwrap_or(0),
                    row.t_bus.unwrap_or(0),
                ));
            }
        }
        0.0
    };
    let b_fr = row.b_fr.unwrap_or(0.0);
    let b_to = row.b_to.unwrap_or(0.0);
    Branch {
        from: BusId(row.f_bus.unwrap_or(0) as usize),
        to: BusId(row.t_bus.unwrap_or(0) as usize),
        r: row.br_r.unwrap_or(0.0),
        x: row.br_x.unwrap_or(0.0),
        b: b_fr + b_to,
        charging: Some(BranchCharging {
            g_fr: row.g_fr.unwrap_or(0.0),
            b_fr,
            g_to: row.g_to.unwrap_or(0.0),
            b_to,
        }),
        rate_a: row.rate_a.unwrap_or(0.0) * pscale,
        rate_b: row.rate_b.unwrap_or(0.0) * pscale,
        rate_c: row.rate_c.unwrap_or(0.0) * pscale,
        rating_sets: Vec::new(),
        current_ratings: (row.c_rating_a.is_some()
            || row.c_rating_b.is_some()
            || row.c_rating_c.is_some())
        .then_some(BranchCurrentRatings {
            c_rating_a: row.c_rating_a.unwrap_or(0.0),
            c_rating_b: row.c_rating_b.unwrap_or(0.0),
            c_rating_c: row.c_rating_c.unwrap_or(0.0),
        }),
        tap,
        shift: row.shift.unwrap_or(0.0) * ascale,
        in_service: row.br_status.unwrap_or(true),
        angmin: row.angmin.unwrap_or(0.0) * ascale,
        angmax: row.angmax.unwrap_or(0.0) * ascale,
        control: None,
        solution: (row.pf.is_some() || row.qf.is_some() || row.pt.is_some() || row.qt.is_some())
            .then_some(BranchSolution {
                pf: row.pf.unwrap_or(0.0) * pscale,
                qf: row.qf.unwrap_or(0.0) * pscale,
                pt: row.pt.unwrap_or(0.0) * pscale,
                qt: row.qt.unwrap_or(0.0) * pscale,
            }),
        uid: None,
        route: None,
        extras: row.extras,
    }
}

#[derive(Default, serde::Deserialize)]
struct SwitchRow {
    #[serde(default, deserialize_with = "lenient_u64")]
    f_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_u64")]
    t_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_flag")]
    state: Option<bool>,
    #[serde(default, deserialize_with = "lenient_flag")]
    status: Option<bool>,
    #[serde(default, deserialize_with = "lenient_f64")]
    thermal_rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    current_rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pt: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qt: Option<f64>,
    #[serde(rename = "source_id", default)]
    _source_id: Option<serde::de::IgnoredAny>,
    #[serde(flatten)]
    extras: crate::network::Extras,
}

fn read_switch(row: SwitchRow, pscale: f64) -> Switch {
    let closed = row.state.or(row.status).unwrap_or(true);
    Switch {
        from: BusId(row.f_bus.unwrap_or(0) as usize),
        to: BusId(row.t_bus.unwrap_or(0) as usize),
        closed,
        thermal_rating: row.thermal_rating.map(|x| x * pscale),
        current_rating: row.current_rating,
        pf: row.pf.map(|x| x * pscale),
        qf: row.qf.map(|x| x * pscale),
        pt: row.pt.map(|x| x * pscale),
        qt: row.qt.map(|x| x * pscale),
        uid: None,
        extras: row.extras,
    }
}

#[derive(Default, serde::Deserialize)]
struct GenRow {
    #[serde(default, deserialize_with = "lenient_u64")]
    gen_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pg: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qg: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pmax: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pmin: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qmax: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qmin: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    vg: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    mbase: Option<f64>,
    #[serde(default, deserialize_with = "lenient_flag")]
    gen_status: Option<bool>,
    #[serde(flatten)]
    cost: CostFields,
    #[serde(flatten)]
    extras: crate::network::Extras,
}

fn read_gen(row: &GenRow, pscale: f64, base_mva: f64, per_unit: bool) -> Generator {
    let mut caps: crate::network::GenCaps = [None; GEN_EXTRA_KEYS.len()];
    for (i, key) in GEN_EXTRA_KEYS.iter().enumerate() {
        if let Some(val) = row.extras.get(*key).and_then(Value::as_f64) {
            // Only the ramp rates are per-unit; the PQ curve points and apf are raw.
            caps[i] = Some(if GEN_PU_KEYS.contains(key) {
                val * pscale
            } else {
                val
            });
        }
    }
    let cost = row.cost.read(base_mva, per_unit);
    Generator {
        bus: BusId(row.gen_bus.unwrap_or(0) as usize),
        pg: row.pg.unwrap_or(0.0) * pscale,
        qg: row.qg.unwrap_or(0.0) * pscale,
        // The writer emits an unbounded limit (±Inf) as JSON null; read a missing
        // limit back as unbounded, not as a binding 0.0. (±Inf · pscale stays ±Inf.)
        pmax: row.pmax.unwrap_or(f64::INFINITY) * pscale,
        pmin: row.pmin.unwrap_or(f64::NEG_INFINITY) * pscale,
        qmax: row.qmax.unwrap_or(f64::INFINITY) * pscale,
        qmin: row.qmin.unwrap_or(f64::NEG_INFINITY) * pscale,
        vg: row.vg.unwrap_or(1.0),
        mbase: row.mbase.unwrap_or(base_mva),
        in_service: row.gen_status.unwrap_or(true),
        cost,
        caps,
        regulated_bus: None,
        active_power_control: None,
        uid: None,
    }
}

/// The MATPOWER cost columns generators and dclines share. `model`'s
/// presence decides whether a cost exists at all, so it stays a raw slot.
#[derive(Default, serde::Deserialize)]
struct CostFields {
    #[serde(default)]
    model: Option<Value>,
    #[serde(default, deserialize_with = "lenient_u64")]
    ncost: Option<u64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    startup: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    shutdown: Option<f64>,
    #[serde(default)]
    cost: Option<Value>,
}

impl CostFields {
    fn read(&self, base_mva: f64, per_unit: bool) -> Option<GenCost> {
        let model_slot = self.model.as_ref()?;
        // Keep non-numeric entries as NaN rather than dropping them: silently
        // filtering would shift every later coefficient's polynomial degree.
        let mut coeffs_raw: Vec<f64> = self
            .cost
            .as_ref()
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|c| c.as_f64().unwrap_or(f64::NAN)).collect())
            .unwrap_or_default();
        // An out-of-range model number must not wrap into 1/2 (`as u8` turns
        // 257 into Piecewise and rescales coefficients that were never
        // per-unit); saturate into the unknown-model passthrough instead.
        let model = model_slot
            .as_u64()
            .map_or(2, |m| u8::try_from(m).unwrap_or(u8::MAX));
        // MATPOWER pads gencost rows to the matrix width with trailing zeros,
        // and third-party JSON can retain that padding. Trim to the declared
        // ncost before the per-unit unscale, as `cost_to_pu` does on the way
        // out, so padding can't read as a higher-degree polynomial and
        // mis-scale every coefficient.
        // Fallible conversion: an ncost beyond usize (32-bit targets) reads
        // as undeclared instead of truncating into a small in-range value.
        let declared_ncost = self.ncost.and_then(|n| usize::try_from(n).ok());
        if let Some(n) = declared_ncost {
            let keep = if model == 1 { n.saturating_mul(2) } else { n };
            if keep < coeffs_raw.len() {
                coeffs_raw.truncate(keep);
            }
        }
        let k = coeffs_raw.len();
        // Undo PowerModels' per-unit cost scaling for the neutral MW basis
        // (the inverse of the writer's per-unit rescale); a non-per-unit
        // source is read as-is.
        let coeffs = if per_unit {
            normalize::cost_from_pu(&coeffs_raw, model, base_mva)
        } else {
            coeffs_raw
        };
        // A polynomial's ncost is its coefficient count; a piecewise curve
        // stores 2·ncost values ((mw, cost) pairs).
        let default_ncost = if model == 1 { k / 2 } else { k };
        Some(GenCost {
            model,
            startup: self.startup.unwrap_or(0.0),
            shutdown: self.shutdown.unwrap_or(0.0),
            // Clamp to what the coefficients can back: an ncost declared
            // beyond the vector length would make the GenCost internally
            // inconsistent.
            ncost: declared_ncost.map_or(default_ncost, |n| n.min(default_ncost)),
            coeffs,
        })
    }
}

#[derive(Default, serde::Deserialize)]
struct DclineRow {
    #[serde(default, deserialize_with = "lenient_u64")]
    f_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_u64")]
    t_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_flag")]
    br_status: Option<bool>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pt: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qt: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    vf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    vt: Option<f64>,
    /// Absorbed so it stays out of extras; the read uses the raw originals.
    #[serde(rename = "pmin", default, deserialize_with = "lenient_f64")]
    _pmin: Option<f64>,
    /// Absorbed so it stays out of extras; the read uses the raw originals.
    #[serde(rename = "pmax", default, deserialize_with = "lenient_f64")]
    _pmax: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    mp_pmin: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    mp_pmax: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pminf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pmaxf: Option<f64>,
    /// Absorbed so it stays out of extras; the read uses the raw originals.
    #[serde(rename = "pmint", default, deserialize_with = "lenient_f64")]
    _pmint: Option<f64>,
    /// Absorbed so it stays out of extras; the read uses the raw originals.
    #[serde(rename = "pmaxt", default, deserialize_with = "lenient_f64")]
    _pmaxt: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qminf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qmaxf: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qmint: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qmaxt: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    loss0: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    loss1: Option<f64>,
    #[serde(flatten)]
    cost: CostFields,
    #[serde(rename = "source_id", default)]
    _source_id: Option<serde::de::IgnoredAny>,
    #[serde(flatten)]
    extras: crate::network::Extras,
}

fn read_hvdc(row: DclineRow, pscale: f64, base_mva: f64, per_unit: bool) -> Hvdc {
    // Aggregate bounds come from PowerModels' raw originals (mp_pmin/mp_pmax);
    // fall back to the from-end per-unit bounds for input that lacks them.
    let pmin = row
        .mp_pmin
        .unwrap_or_else(|| row.pminf.unwrap_or(0.0) * pscale);
    let pmax = row
        .mp_pmax
        .unwrap_or_else(|| row.pmaxf.unwrap_or(0.0) * pscale);
    let cost = row.cost.read(base_mva, per_unit);
    Hvdc {
        from: BusId(row.f_bus.unwrap_or(0) as usize),
        to: BusId(row.t_bus.unwrap_or(0) as usize),
        in_service: row.br_status.unwrap_or(true),
        pf: row.pf.unwrap_or(0.0) * pscale,
        // PowerModels flips Pt/Qf/Qt vs MATPOWER; undo it for the neutral model.
        pt: -row.pt.unwrap_or(0.0) * pscale,
        qf: -row.qf.unwrap_or(0.0) * pscale,
        qt: -row.qt.unwrap_or(0.0) * pscale,
        vf: row.vf.unwrap_or(1.0),
        vt: row.vt.unwrap_or(1.0),
        pmin,
        pmax,
        // Unbounded reactive limits (±Inf) write as null; read them back unbounded.
        qminf: row.qminf.unwrap_or(f64::NEG_INFINITY) * pscale,
        qmaxf: row.qmaxf.unwrap_or(f64::INFINITY) * pscale,
        qmint: row.qmint.unwrap_or(f64::NEG_INFINITY) * pscale,
        qmaxt: row.qmaxt.unwrap_or(f64::INFINITY) * pscale,
        loss0: row.loss0.unwrap_or(0.0) * pscale,
        loss1: row.loss1.unwrap_or(0.0),
        resistance_ohm: None,
        nominal_voltage_kv: None,
        converters_mode: None,
        converter1: None,
        converter2: None,
        cost,
        uid: None,
        extras: row.extras,
    }
}

#[derive(Default, serde::Deserialize)]
struct StorageRow {
    #[serde(default, deserialize_with = "lenient_u64")]
    storage_bus: Option<u64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    ps: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qs: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    energy: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    energy_rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    charge_rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    discharge_rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    charge_efficiency: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    discharge_efficiency: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    thermal_rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    current_rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qmin: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    qmax: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    r: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    x: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    p_loss: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    q_loss: Option<f64>,
    #[serde(default, deserialize_with = "lenient_flag")]
    status: Option<bool>,
    #[serde(rename = "source_id", default)]
    _source_id: Option<serde::de::IgnoredAny>,
    #[serde(flatten)]
    extras: crate::network::Extras,
}

fn read_storage(row: StorageRow, pscale: f64) -> Storage {
    Storage {
        bus: BusId(row.storage_bus.unwrap_or(0) as usize),
        ps: row.ps.unwrap_or(0.0),
        qs: row.qs.unwrap_or(0.0),
        energy: row.energy.unwrap_or(0.0) * pscale,
        energy_rating: row.energy_rating.unwrap_or(0.0) * pscale,
        charge_rating: row.charge_rating.unwrap_or(0.0) * pscale,
        discharge_rating: row.discharge_rating.unwrap_or(0.0) * pscale,
        charge_efficiency: row.charge_efficiency.unwrap_or(1.0),
        discharge_efficiency: row.discharge_efficiency.unwrap_or(1.0),
        thermal_rating: row.thermal_rating.unwrap_or(0.0) * pscale,
        current_rating: row.current_rating,
        // Unbounded reactive limits (±Inf) write as null; read them back unbounded.
        qmin: row.qmin.unwrap_or(f64::NEG_INFINITY) * pscale,
        qmax: row.qmax.unwrap_or(f64::INFINITY) * pscale,
        r: row.r.unwrap_or(0.0),
        x: row.x.unwrap_or(0.0),
        p_loss: row.p_loss.unwrap_or(0.0) * pscale,
        q_loss: row.q_loss.unwrap_or(0.0) * pscale,
        in_service: row.status.unwrap_or(true),
        active_power_control: None,
        uid: None,
        extras: row.extras,
    }
}

#[cfg(test)]
mod tests {
    fn parse_powermodels_json(content: &str) -> Result<BalancedNetwork> {
        let mut warnings = Diagnostics::new();
        parse_powermodels_json_source(content, None, &mut warnings)
    }

    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn boolean_status_fields_read_out_of_service() {
        let doc = r#"{"baseMVA":100.0,"per_unit":true,
            "bus":{"1":{"bus_i":1,"bus_type":3,"vm":1.0,"va":0.0,"base_kv":345.0},
                   "2":{"bus_i":2,"bus_type":1,"vm":1.0,"va":0.0,"base_kv":345.0}},
            "branch":{"1":{"f_bus":1,"t_bus":2,"br_r":0.01,"br_x":0.1,"br_status":false}},
            "gen":{"1":{"gen_bus":1,"pg":0.5,"gen_status":false}}}"#;
        let net = parse_powermodels_json(doc).unwrap();
        assert!(
            !net.branches()[0].in_service,
            "br_status: false must read out of service"
        );
        assert!(
            !net.generators()[0].in_service,
            "gen_status: false must read out of service"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact values pass straight through the reader
    fn nonunit_tap_without_transformer_flag_warns_and_stays_a_line() {
        let doc = r#"{"baseMVA":100.0,"per_unit":true,
            "bus":{"1":{"bus_i":1,"bus_type":3,"vm":1.0,"va":0.0,"base_kv":345.0},
                   "2":{"bus_i":2,"bus_type":1,"vm":1.0,"va":0.0,"base_kv":345.0}},
            "branch":{"1":{"index":1,"f_bus":1,"t_bus":2,"br_r":0.01,"br_x":0.1,"tap":1.05},
                      "2":{"index":2,"f_bus":1,"t_bus":2,"br_r":0.01,"br_x":0.1,"tap":1.0},
                      "3":{"index":3,"f_bus":1,"t_bus":2,"br_r":0.01,"br_x":0.1,
                           "tap":1.05,"transformer":true}}}"#;
        let mut warnings = Diagnostics::new();
        let net = parse_powermodels_json_source(doc, None, &mut warnings).unwrap();
        // The inference rule is unchanged: without the flag the tap is dropped
        // (raw 0 = line); only the drop of a non-unit value is reported.
        assert_eq!(net.branches()[0].tap, 0.0);
        assert_eq!(net.branches()[1].tap, 0.0);
        assert_eq!(net.branches()[2].tap, 1.05);
        // One aggregated warning naming the offending branch by its map key,
        // which is the identity a file without an inner `index` still has.
        let lines = warnings.lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("1 branch(es)") && lines[0].contains("`1` (1 -> 2) tap 1.05"),
            "{lines:?}"
        );
    }

    #[test]
    fn nonpositive_base_mva_is_rejected() {
        for base in ["0.0", "-100.0", "1e999"] {
            let doc = format!(
                r#"{{"baseMVA":{base},"bus":{{"1":{{"bus_i":1,"bus_type":3,"vm":1.0,"va":0.0,"base_kv":345.0}}}}}}"#
            );
            assert!(
                parse_powermodels_json(&doc).is_err(),
                "baseMVA {base} must be rejected"
            );
        }
    }

    #[test]
    fn padded_cost_rows_trim_to_ncost_before_unscaling() {
        // MATPOWER pads gencost rows to the matrix width; the trailing zero
        // here is padding, and ncost declares the real quadratic. Untrimmed,
        // the per-unit unscale would treat the row as cubic and divide every
        // coefficient by an extra factor of base.
        let fields: CostFields = serde_json::from_value(serde_json::json!({
            "model": 2, "ncost": 3,
            "cost": [1.0, 1.0, 1.0, 0.0]
        }))
        .unwrap();
        let cost = fields.read(100.0, true).unwrap();
        assert_eq!(cost.ncost, 3);
        assert_eq!(cost.coeffs.len(), 3);
        assert!(approx(cost.coeffs[0], 1e-4));
        assert!(approx(cost.coeffs[1], 1e-2));
        assert!(approx(cost.coeffs[2], 1.0));
    }

    #[test]
    fn out_of_range_cost_model_does_not_wrap_into_rescaling() {
        // 257 as u8 would wrap to 1 (piecewise) and rescale coefficients that
        // were never per-unit; it must saturate into the unknown-model
        // passthrough instead.
        let fields: CostFields = serde_json::from_value(serde_json::json!({
            "model": 257,
            "cost": [10.0, 5.0]
        }))
        .unwrap();
        let cost = fields.read(100.0, true).unwrap();
        assert_eq!(cost.coeffs, vec![10.0, 5.0]);
    }

    #[test]
    fn gen_pu_keys_subset_of_extra_keys() {
        // The per-unitized columns must be a subset of the emitted capability
        // columns; a key not in GEN_EXTRA_KEYS would never be written or scaled,
        // and a typo here silently mis-scales a ramp rate.
        for k in GEN_PU_KEYS {
            assert!(
                GEN_EXTRA_KEYS.contains(&k),
                "{k} is not a GEN_EXTRA_KEYS column"
            );
        }
    }

    #[test]
    fn dcline_p_bounds_four_quadrants() {
        // loss0 = 1, loss1 = 0.1 ⇒ l = 0.9. Each sign quadrant of (pmin, pmax)
        // hand-computed against PowerModels' _mp2pm_dcline!.
        let q1 = dcline_p_bounds(2.0, 10.0, 1.0, 0.1);
        assert!(
            approx(q1.0, 2.0) && approx(q1.1, 10.0) && approx(q1.2, -8.0) && approx(q1.3, -0.8)
        );

        let q2 = dcline_p_bounds(2.0, -5.0, 1.0, 0.1);
        assert!(
            approx(q2.0, 2.0)
                && approx(q2.1, 6.0 / 0.9)
                && approx(q2.2, -5.0)
                && approx(q2.3, -0.8)
        );

        let q3 = dcline_p_bounds(-3.0, 10.0, 1.0, 0.1);
        assert!(
            approx(q3.0, -2.0 / 0.9)
                && approx(q3.1, 10.0)
                && approx(q3.2, -8.0)
                && approx(q3.3, 3.0)
        );

        let q4 = dcline_p_bounds(-3.0, -5.0, 1.0, 0.1);
        assert!(
            approx(q4.0, -2.0 / 0.9)
                && approx(q4.1, 6.0 / 0.9)
                && approx(q4.2, -5.0)
                && approx(q4.3, 3.0)
        );
    }
}
