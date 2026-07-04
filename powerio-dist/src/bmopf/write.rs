//! [`DistNetwork`] into strict BMOPF JSON.
//!
//! Output is schema valid wherever the schema permits the data.
//!
//! Numbers serialize through serde_json (shortest round trip form).
//! Nonfinite values cannot appear in JSON; they emit as 0 with a warning
//! naming the element and field.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::convert::Conversion;
use crate::diagnostics::{DiagnosticSeverity, DiagnosticStage, StructuredDiagnostic};
use crate::model::{
    ActivePowerReference, ActivePowerUnit, Configuration, ControlVoltageReference,
    DistControlProfile, DistGenerator, DistIbr, DistLoadVoltageModel, DistNetwork, DistTransformer,
    Mat, ReactivePowerReference, ReactivePowerUnit, VoltVarControl, VoltWattControl, Winding,
    WindingConn, n_winding_impedance_base, pair_keys,
};

/// The `$schema` stamped into every document's `meta`: the current BMOPF
/// schema URI used by BMOPFTools.
const BMOPF_SCHEMA_ID: &str =
    "https://raw.githubusercontent.com/frederikgeth/bmopf-report/main/schema/bmopf.json";

const RAW_BMOPF_TOP_LEVEL: &[&str] = &[
    "capacitor",
    "ibr",
    "control_profile",
    "dc_bus",
    "dc_line",
    "dc_load",
    "dc_source",
];

const IBR_EXTRA_FIELDS: &[&str] = &[
    "dc_link_coupled",
    "p_dc_min",
    "p_dc_max",
    "dc_bus",
    "dc_terminal_map",
    "dc_control",
    "dc_v_set",
    "dc_p_ref",
    "dc_droop",
    "dc_deadband",
    "r_filter",
    "x_filter",
    "b_filter_shunt",
    "grid_forming",
    "v_ref_internal",
    "cost",
    "time_series",
];

const BMOPF_DELTA_ROLLS_EXTRA: &str = "bmopf_delta_rolls";

const TRANSFORMER_NO_LOAD_ALLOWED_EXTRAS: [&str; 5] = [
    "g_no_load",
    "b_no_load",
    "%noloadloss",
    "%imag",
    BMOPF_DELTA_ROLLS_EXTRA,
];
const TRANSFORMER_TWO_WINDING_ALLOWED_EXTRAS: [&str; 18] = [
    "tap_min",
    "tap_max",
    "mintap",
    "maxtap",
    "numtaps",
    "pmd_tm_set",
    "pmd_tm_lb",
    "pmd_tm_ub",
    "pmd_tm_fix",
    "pmd_tm_step",
    "g_no_load",
    "b_no_load",
    "r_neutral_from",
    "x_neutral_from",
    "r_neutral_to",
    "x_neutral_to",
    "%noloadloss",
    "%imag",
];

/// Writes the strict BMOPF document. Every field the schema cannot carry
/// is reported in the warnings.
///
/// # Panics
///
/// Never in practice: the document is maps, strings, and finite numbers,
/// which always serialize.
pub fn write_bmopf_json(net: &DistNetwork) -> Conversion {
    let mut w = Writer {
        warnings: Vec::new(),
        diagnostics: Vec::new(),
        grounded: net
            .buses
            .iter()
            .map(|b| (b.id.to_ascii_lowercase(), b.grounded.clone()))
            .collect(),
    };
    let doc = w.document(net);
    Conversion {
        text: serde_json::to_string_pretty(&doc).expect("maps and finite numbers") + "\n",
        warnings: w.warnings,
        diagnostics: w.diagnostics,
    }
}

struct Writer {
    warnings: Vec<String>,
    diagnostics: Vec<StructuredDiagnostic>,
    grounded: BTreeMap<String, Vec<String>>,
}

impl Writer {
    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    fn diagnostic(
        &mut self,
        code: &'static str,
        element_path: impl Into<String>,
        message: impl Into<String>,
        details: Map<String, Value>,
    ) {
        let message = message.into();
        self.warnings.push(format!("{message} [{code}]"));
        self.diagnostics.push(
            StructuredDiagnostic::new(
                code,
                DiagnosticSeverity::Warning,
                DiagnosticStage::Emit,
                message,
            )
            .with_element_path(element_path)
            .with_details(details),
        );
    }

    fn transformer_diagnostic(
        &mut self,
        t: &DistTransformer,
        code: &'static str,
        message: impl Into<String>,
        mut details: Map<String, Value>,
    ) {
        details.insert("transformer".into(), json!(&t.name));
        self.diagnostic(code, format!("transformer {}", t.name), message, details);
    }

    /// Finite number guard (the jnum pattern): JSON has no Inf/NaN.
    fn num(&mut self, v: f64, what: &str) -> Value {
        if v.is_finite() {
            json!(v)
        } else {
            self.warn(format!("{what}: nonfinite value emitted as 0"));
            json!(0.0)
        }
    }

    fn nums(&mut self, vs: &[f64], what: &str) -> Value {
        Value::Array(vs.iter().map(|&v| self.num(v, what)).collect())
    }

    fn extras_dropped(&mut self, extras: &crate::model::Extras, what: &str) {
        for key in extras.keys() {
            // `bmopf_subtype` is reader bookkeeping; `conn` marks a delta shunt
            // whose geometry already lives in the off diagonal B matrix, so it
            // is preserved, not dropped.
            if key == "bmopf_subtype" || key == "conn" {
                continue;
            }
            self.warn(format!(
                "{what}: `{key}` has no place in the BMOPF schema; dropped from the output"
            ));
        }
    }

    /// Provenance + schema-vintage self-identification (the BMOPF `meta` object):
    /// "generated by powerio vX, targeting BMOPF schema vintage Y." Deterministic
    /// and round-trip stable — no timestamp, and nothing that depends on the
    /// immediate source format (which a round trip would change) — so canonical
    /// output is idempotent. The vintage lives in `$schema` (the canonical
    /// bmopf-report `$id`).
    fn meta() -> Value {
        json!({
            "$schema": BMOPF_SCHEMA_ID,
            "generator": {"tool": "powerio", "version": env!("CARGO_PKG_VERSION")},
        })
    }

    fn document(&mut self, net: &DistNetwork) -> Value {
        let mut doc = Map::new();
        if let Some(name) = &net.name {
            doc.insert("name".into(), json!(name));
        }
        doc.insert("meta".into(), Self::meta());

        let mut buses = Map::new();
        for b in &net.buses {
            let mut o = Map::new();
            o.insert("terminal_names".into(), json!(b.terminals));
            if !b.grounded.is_empty() {
                o.insert("perfectly_grounded_terminals".into(), json!(b.grounded));
            }
            if let Some(v) = b.v_min {
                o.insert("v_min".into(), Value::Array(vec![self.num(v, "bus v_min")]));
            }
            if let Some(v) = b.v_max {
                o.insert("v_max".into(), Value::Array(vec![self.num(v, "bus v_max")]));
            }
            for (key, bound) in [
                ("vpn_min", &b.vpn_min),
                ("vpn_max", &b.vpn_max),
                ("vpp_min", &b.vpp_min),
                ("vpp_max", &b.vpp_max),
                ("vsym_min", &b.vsym_min),
                ("vsym_max", &b.vsym_max),
            ] {
                if let Some(v) = bound {
                    o.insert(key.into(), self.nums(v, &format!("bus {key}")));
                }
            }
            // Coordinates and other extras have no bus fields in the schema.
            self.extras_dropped(&b.extras, &format!("bus {}", b.id));
            buses.insert(b.id.clone(), Value::Object(o));
        }
        doc.insert("bus".into(), Value::Object(buses));

        if !net.linecodes.is_empty() {
            let mut codes = Map::new();
            for c in &net.linecodes {
                let mut o = Map::new();
                // The schema requires R_series_1_1 and X_series_1_1; an
                // empty matrix would drop them and invalidate the output.
                let dim = c.r_series.len().max(c.x_series.len()).max(1);
                if c.r_series.is_empty() && c.x_series.is_empty() {
                    self.warn(format!(
                        "linecode {}: no series matrix; emitted as 1 conductor \
                         zero impedance",
                        c.name
                    ));
                } else if c.r_series.is_empty() || c.x_series.is_empty() {
                    self.warn(format!(
                        "linecode {}: R_series and X_series sizes disagree; the \
                         empty one emitted as zeros",
                        c.name
                    ));
                }
                self.required_matrix(&mut o, "R_series", &c.r_series, dim, &c.name);
                self.required_matrix(&mut o, "X_series", &c.x_series, dim, &c.name);
                self.flat_matrix(&mut o, "G_from", &c.g_from, &c.name);
                self.flat_matrix(&mut o, "G_to", &c.g_to, &c.name);
                self.flat_matrix(&mut o, "B_from", &c.b_from, &c.name);
                self.flat_matrix(&mut o, "B_to", &c.b_to, &c.name);
                if let Some(i_max) = &c.i_max {
                    o.insert("i_max".into(), self.nums(i_max, "linecode i_max"));
                }
                if let Some(s_max) = &c.s_max {
                    o.insert("s_max".into(), self.nums(s_max, "linecode s_max"));
                }
                self.extras_dropped(&c.extras, &format!("linecode {}", c.name));
                codes.insert(c.name.clone(), Value::Object(o));
            }
            doc.insert("linecode".into(), Value::Object(codes));
        }

        self.branches(net, &mut doc);
        self.injections(net, &mut doc);
        self.control_profiles(net, &mut doc);
        self.ibrs(net, &mut doc);

        let transformers = self.transformers(net);
        if !transformers.is_empty() {
            doc.insert("transformer".into(), Value::Object(transformers));
        }

        self.untyped_bmopf_tables(net, &mut doc);
        self.warn_unemitted_untyped(net);
        self.prune_unreferenced_buses(&mut doc);
        Value::Object(doc)
    }

    fn warn_unemitted_untyped(&mut self, net: &DistNetwork) {
        for u in &net.untyped {
            if Self::is_emitted_untyped(u) {
                continue;
            }
            let message = format!(
                "{} {}: class is not represented in BMOPF; dropped from the output",
                u.class, u.name
            );
            if u.class == "regcontrol" || u.class == "autotrans" {
                let mut details = Map::new();
                details.insert("class".into(), json!(&u.class));
                details.insert("name".into(), json!(&u.name));
                let code = if u.class == "regcontrol" {
                    "EMIT.BMOPF.REGCONTROL_DROPPED"
                } else {
                    "EMIT.BMOPF.AUTOTRANSFORMER_DROPPED"
                };
                self.diagnostic(code, format!("{} {}", u.class, u.name), message, details);
            } else {
                self.warn(message);
            }
        }
    }

    fn is_emitted_untyped(u: &crate::model::UntypedObject) -> bool {
        RAW_BMOPF_TOP_LEVEL.contains(&u.class.as_str()) || u.class.starts_with("transformer.")
    }

    fn untyped_bmopf_tables(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        for u in &net.untyped {
            let Some(value) = raw_bmopf_value(u) else {
                self.warn(format!(
                    "{} {}: untyped BMOPF object could not be parsed as JSON; dropped from the output",
                    u.class, u.name
                ));
                continue;
            };
            if RAW_BMOPF_TOP_LEVEL.contains(&u.class.as_str()) {
                doc.entry(u.class.clone())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .expect("BMOPF tables are objects")
                    .insert(u.name.clone(), value);
            } else if let Some(subtype) = u.class.strip_prefix("transformer.") {
                doc.entry("transformer")
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .expect("transformer table is an object")
                    .entry(subtype.to_string())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .expect("transformer subtype table is an object")
                    .insert(u.name.clone(), value);
            }
        }
    }

    fn prune_unreferenced_buses(&mut self, doc: &mut Map<String, Value>) {
        let mut refs = BTreeMap::new();
        for (key, value) in doc.iter() {
            if key != "bus" {
                collect_bus_usage(value, &mut refs);
            }
        }
        let Some(buses) = doc.get_mut("bus").and_then(Value::as_object_mut) else {
            return;
        };
        let ids: Vec<String> = buses.keys().cloned().collect();
        for id in ids {
            let Some(used) = refs.get(&id) else {
                buses.remove(&id);
                self.warn(format!(
                    "bus {id}: no emitted BMOPF element references this bus; dropped from the output"
                ));
                continue;
            };
            let Some(bus) = buses.get_mut(&id).and_then(Value::as_object_mut) else {
                continue;
            };
            prune_string_array(
                bus,
                "terminal_names",
                used,
                &mut self.warnings,
                &format!("bus {id}"),
            );
            prune_string_array(
                bus,
                "perfectly_grounded_terminals",
                used,
                &mut self.warnings,
                &format!("bus {id}"),
            );
            if matches!(
                bus.get("perfectly_grounded_terminals"),
                Some(Value::Array(terms)) if terms.is_empty()
            ) {
                bus.remove("perfectly_grounded_terminals");
            }
        }
    }

    /// Lines and switches.
    fn branches(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        if !net.lines.is_empty() {
            let mut lines = Map::new();
            for l in &net.lines {
                let mut o = Map::new();
                o.insert("length".into(), self.num(l.length, "line length"));
                o.insert("linecode".into(), json!(l.linecode));
                o.insert("bus_from".into(), json!(l.bus_from));
                o.insert("bus_to".into(), json!(l.bus_to));
                o.insert("terminal_map_from".into(), json!(l.terminal_map_from));
                o.insert("terminal_map_to".into(), json!(l.terminal_map_to));
                self.extras_dropped(&l.extras, &format!("line {}", l.name));
                lines.insert(l.name.clone(), Value::Object(o));
            }
            doc.insert("line".into(), Value::Object(lines));
        }
        if !net.switches.is_empty() {
            let mut switches = Map::new();
            for s in &net.switches {
                let mut o = Map::new();
                o.insert("bus_from".into(), json!(s.bus_from));
                o.insert("bus_to".into(), json!(s.bus_to));
                o.insert("terminal_map_from".into(), json!(s.terminal_map_from));
                o.insert("terminal_map_to".into(), json!(s.terminal_map_to));
                o.insert("open_switch".into(), json!(s.open));
                if let Some(i_max) = &s.i_max {
                    o.insert("i_max".into(), self.nums(i_max, "switch i_max"));
                }
                self.extras_dropped(&s.extras, &format!("switch {}", s.name));
                switches.insert(s.name.clone(), Value::Object(o));
            }
            doc.insert("switch".into(), Value::Object(switches));
        }
    }

    /// Loads, generators, shunts, and the voltage sources.
    fn injections(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        let mut loads = Map::new();
        for l in &net.loads {
            let mut o = Map::new();
            o.insert("configuration".into(), json!(config_str(l.configuration)));
            o.insert("p_nom".into(), self.nums(&l.p_nom, "load p_nom"));
            o.insert("q_nom".into(), self.nums(&l.q_nom, "load q_nom"));
            o.insert("bus".into(), json!(l.bus));
            o.insert("terminal_map".into(), json!(l.terminal_map));
            self.load_voltage_model(&mut o, &l.voltage_model, &format!("load {}", l.name));
            self.extras_dropped(&l.extras, &format!("load {}", l.name));
            loads.insert(l.name.clone(), Value::Object(o));
        }
        let mut gens = Map::new();
        for g in &net.generators {
            gens.insert(g.name.clone(), self.generator(g));
        }
        if !loads.is_empty() {
            doc.insert("load".into(), Value::Object(loads));
        }
        if !gens.is_empty() {
            doc.insert("generator".into(), Value::Object(gens));
        }
        if !net.shunts.is_empty() {
            let mut shunts = Map::new();
            for s in &net.shunts {
                let mut o = Map::new();
                o.insert("bus".into(), json!(s.bus));
                o.insert("terminal_map".into(), json!(s.terminal_map));
                // The schema requires G_1_1 and B_1_1.
                let dim = s.g.len().max(s.b.len()).max(1);
                if s.g.is_empty() && s.b.is_empty() {
                    self.warn(format!(
                        "shunt {}: no admittance matrix; emitted as 1 conductor \
                         zero admittance",
                        s.name
                    ));
                } else if s.g.is_empty() || s.b.is_empty() {
                    self.warn(format!(
                        "shunt {}: G and B sizes disagree; the empty one emitted \
                         as zeros",
                        s.name
                    ));
                }
                self.required_matrix(&mut o, "G", &s.g, dim, &s.name);
                self.required_matrix(&mut o, "B", &s.b, dim, &s.name);
                self.extras_dropped(&s.extras, &format!("shunt {}", s.name));
                shunts.insert(s.name.clone(), Value::Object(o));
            }
            doc.insert("shunt".into(), Value::Object(shunts));
        }
        let mut sources = Map::new();
        if net.sources.is_empty() {
            self.warn("network has no voltage source; BMOPF requires exactly one");
        }
        for (i, vs) in net.sources.iter().enumerate() {
            if i > 0 {
                self.warn(format!(
                    "voltage source {}: the BMOPF formulation expects exactly one source; \
                     this network has {}",
                    vs.name,
                    net.sources.len()
                ));
            }
            let mut o = Map::new();
            o.insert(
                "v_magnitude".into(),
                self.nums(&vs.v_magnitude, "voltage_source v_magnitude"),
            );
            o.insert(
                "v_angle".into(),
                self.nums(&vs.v_angle, "voltage_source v_angle"),
            );
            o.insert("bus".into(), json!(vs.bus));
            o.insert("terminal_map".into(), json!(vs.terminal_map));
            let mut extras = vs.extras.clone();
            if let Some(cost) = extras.remove("cost") {
                o.insert("cost".into(), cost);
            }
            self.extras_dropped(&extras, &format!("voltage source {}", vs.name));
            sources.insert(vs.name.clone(), Value::Object(o));
        }
        doc.insert("voltage_source".into(), Value::Object(sources));
    }

    fn control_profiles(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        if net.control_profiles.is_empty() {
            return;
        }
        let mut profiles = Map::new();
        for profile in &net.control_profiles {
            profiles.insert(profile.name.clone(), self.control_profile(profile));
        }
        doc.insert("control_profile".into(), Value::Object(profiles));
    }

    fn control_profile(&mut self, profile: &DistControlProfile) -> Value {
        let mut o = Map::new();
        if let Some(pf) = &profile.power_factor {
            o.insert(
                "power_factor".into(),
                json!({ "pf": self.num(pf.pf, "power factor") }),
            );
        }
        if let Some(vv) = &profile.volt_var {
            o.insert("volt_var".into(), self.volt_var(vv));
        }
        if let Some(vw) = &profile.volt_watt {
            o.insert("volt_watt".into(), self.volt_watt(vw));
        }
        for (key, value) in &profile.extras {
            if value.is_object() {
                o.insert(key.clone(), value.clone());
            } else {
                self.warn(format!(
                    "control_profile {}: extra `{key}` is not an object; dropped from the output",
                    profile.name
                ));
            }
        }
        Value::Object(o)
    }

    fn volt_var(&mut self, vv: &VoltVarControl) -> Value {
        let mut o = Map::new();
        if let Some(v) = vv.voltage_reference {
            o.insert("voltage_reference".into(), json_enum(v));
        }
        o.insert(
            "breakpoints".into(),
            self.nums(&vv.breakpoints, "volt_var breakpoints"),
        );
        o.insert(
            "q_limits".into(),
            self.nums(&vv.q_limits, "volt_var q_limits"),
        );
        if let Some(v) = vv.q_unit {
            o.insert("q_unit".into(), json_enum::<ReactivePowerUnit>(v));
        }
        if let Some(v) = vv.q_ref {
            o.insert("q_ref".into(), json_enum::<ReactivePowerReference>(v));
        }
        if let Some(v) = vv.p_min_for_q {
            o.insert("p_min_for_q".into(), self.num(v, "volt_var p_min_for_q"));
        }
        if let Some(v) = vv.p_min_for_q_max {
            o.insert(
                "p_min_for_q_max".into(),
                self.num(v, "volt_var p_min_for_q_max"),
            );
        }
        Value::Object(o)
    }

    fn volt_watt(&mut self, vw: &VoltWattControl) -> Value {
        let mut o = Map::new();
        if let Some(v) = vw.voltage_reference {
            o.insert(
                "voltage_reference".into(),
                json_enum::<ControlVoltageReference>(v),
            );
        }
        o.insert(
            "breakpoints".into(),
            self.nums(&vw.breakpoints, "volt_watt breakpoints"),
        );
        o.insert(
            "p_limits".into(),
            self.nums(&vw.p_limits, "volt_watt p_limits"),
        );
        if let Some(v) = vw.p_unit {
            o.insert("p_unit".into(), json_enum::<ActivePowerUnit>(v));
        }
        if let Some(v) = vw.p_ref {
            o.insert("p_ref".into(), json_enum::<ActivePowerReference>(v));
        }
        Value::Object(o)
    }

    fn ibrs(&mut self, net: &DistNetwork, doc: &mut Map<String, Value>) {
        if net.ibrs.is_empty() {
            return;
        }
        let mut ibrs = Map::new();
        for ibr in &net.ibrs {
            ibrs.insert(ibr.name.clone(), self.ibr(ibr));
        }
        doc.insert("ibr".into(), Value::Object(ibrs));
    }

    fn ibr(&mut self, ibr: &DistIbr) -> Value {
        let mut o = Map::new();
        o.insert("bus".into(), json!(ibr.bus));
        o.insert("terminal_map".into(), json!(ibr.terminal_map));
        o.insert("topology".into(), json_enum(ibr.topology));
        o.insert("prime_mover".into(), json_enum(ibr.prime_mover));
        o.insert("s_max".into(), self.nums(&ibr.s_max, "ibr s_max"));
        if let Some(v) = &ibr.i_max {
            o.insert("i_max".into(), self.nums(v, "ibr i_max"));
        }
        if let Some(v) = ibr.p_avail {
            o.insert("p_avail".into(), self.num(v, "ibr p_avail"));
        }
        if let Some(v) = &ibr.p_min {
            o.insert("p_min".into(), self.nums(v, "ibr p_min"));
        }
        if let Some(v) = &ibr.p_max {
            o.insert("p_max".into(), self.nums(v, "ibr p_max"));
        }
        if let Some(v) = &ibr.q_min {
            o.insert("q_min".into(), self.nums(v, "ibr q_min"));
        }
        if let Some(v) = &ibr.q_max {
            o.insert("q_max".into(), self.nums(v, "ibr q_max"));
        }
        if let Some(v) = &ibr.control_profile {
            o.insert("control_profile".into(), json!(v));
        }
        if let Some(v) = ibr.voltage_aggregation {
            o.insert("voltage_aggregation".into(), json_enum(v));
        }
        for (key, value) in &ibr.extras {
            if IBR_EXTRA_FIELDS.contains(&key.as_str()) {
                o.insert(key.clone(), value.clone());
            } else {
                self.warn(format!(
                    "ibr {}: extra `{key}` has no place in the BMOPF schema; dropped from the output",
                    ibr.name
                ));
            }
        }
        Value::Object(o)
    }

    fn load_voltage_model(
        &mut self,
        o: &mut Map<String, Value>,
        model: &DistLoadVoltageModel,
        what: &str,
    ) {
        match model {
            DistLoadVoltageModel::ConstantPower { v_nom } => {
                o.insert("model".into(), json!("constant_power"));
                if !v_nom.is_empty() {
                    o.insert("v_nom".into(), self.nums(v_nom, &format!("{what} v_nom")));
                }
            }
            DistLoadVoltageModel::ConstantCurrent { v_nom } => {
                o.insert("model".into(), json!("constant_current"));
                o.insert("v_nom".into(), self.nums(v_nom, &format!("{what} v_nom")));
            }
            DistLoadVoltageModel::ConstantImpedance { v_nom } => {
                o.insert("model".into(), json!("constant_impedance"));
                o.insert("v_nom".into(), self.nums(v_nom, &format!("{what} v_nom")));
            }
            DistLoadVoltageModel::Zip {
                v_nom,
                alpha_z,
                alpha_i,
                alpha_p,
                beta_z,
                beta_i,
                beta_p,
            } => {
                o.insert("model".into(), json!("zip"));
                o.insert("v_nom".into(), self.nums(v_nom, &format!("{what} v_nom")));
                o.insert(
                    "alpha_z".into(),
                    self.nums(alpha_z, &format!("{what} alpha_z")),
                );
                o.insert(
                    "alpha_i".into(),
                    self.nums(alpha_i, &format!("{what} alpha_i")),
                );
                o.insert(
                    "alpha_p".into(),
                    self.nums(alpha_p, &format!("{what} alpha_p")),
                );
                o.insert(
                    "beta_z".into(),
                    self.nums(beta_z, &format!("{what} beta_z")),
                );
                o.insert(
                    "beta_i".into(),
                    self.nums(beta_i, &format!("{what} beta_i")),
                );
                o.insert(
                    "beta_p".into(),
                    self.nums(beta_p, &format!("{what} beta_p")),
                );
            }
            DistLoadVoltageModel::Exponential {
                v_nom,
                gamma_p,
                gamma_q,
            } => {
                o.insert("model".into(), json!("exponential"));
                o.insert("v_nom".into(), self.nums(v_nom, &format!("{what} v_nom")));
                o.insert(
                    "gamma_p".into(),
                    self.nums(gamma_p, &format!("{what} gamma_p")),
                );
                o.insert(
                    "gamma_q".into(),
                    self.nums(gamma_q, &format!("{what} gamma_q")),
                );
            }
        }
    }

    fn generator(&mut self, g: &DistGenerator) -> Value {
        let mut o = Map::new();
        // BMOPF generators carry bounds and cost, no dispatch setpoint: a
        // fixed injection becomes pinned bounds. Explicit source bounds win
        // over the setpoint, which then has nowhere to go.
        let what = format!("generator {}", g.name);
        for (key_lo, key_hi, lo, hi, nom) in [
            ("p_min", "p_max", &g.p_min, &g.p_max, &g.p_nom),
            ("q_min", "q_max", &g.q_min, &g.q_max, &g.q_nom),
        ] {
            if lo.is_some() || hi.is_some() {
                // Pinned bounds ARE the setpoint; only a setpoint that
                // differs from the bounds has nowhere to go.
                let pinned = lo.as_deref() == Some(nom) && hi.as_deref() == Some(nom);
                if !nom.is_empty() && !nom.iter().all(|&v| v == 0.0) && !pinned {
                    self.warn(format!(
                        "{what}: explicit {key_lo}/{key_hi} bounds win over the setpoint, \
                         which has no BMOPF field"
                    ));
                }
                if let Some(v) = lo {
                    o.insert(key_lo.into(), self.nums(v, key_lo));
                }
                if let Some(v) = hi {
                    o.insert(key_hi.into(), self.nums(v, key_hi));
                }
            } else if !nom.is_empty() {
                // A fixed injection becomes pinned bounds.
                o.insert(key_lo.into(), self.nums(nom, key_lo));
                o.insert(key_hi.into(), self.nums(nom, key_hi));
            }
        }
        // BMOPF generation cost is per phase conductor; powerio carries a single
        // value, so broadcast the scalar to one entry per phase.
        let n_phase = if g.p_nom.is_empty() {
            g.terminal_map.len().max(1)
        } else {
            g.p_nom.len()
        };
        let cost = g.cost.unwrap_or_else(|| {
            self.warnings.push(format!(
                "{what}: no generation cost in the source; emitted cost 0"
            ));
            0.0
        });
        o.insert(
            "cost".into(),
            self.nums(&vec![cost; n_phase], "generator cost"),
        );
        o.insert("bus".into(), json!(g.bus));
        o.insert("configuration".into(), json!(config_str(g.configuration)));
        o.insert("terminal_map".into(), json!(g.terminal_map));
        if g.configuration == Configuration::Delta {
            self.warn(format!(
                "{what}: the BMOPF formulation covers WYE generators; DELTA emitted as written"
            ));
        }
        self.extras_dropped(&g.extras, &what);
        Value::Object(o)
    }

    /// Transformers keyed by subtype; wye-wye three phase units decompose
    /// into one single_phase entry per phase, the convention the public
    /// example networks use.
    fn transformers(&mut self, net: &DistNetwork) -> Map<String, Value> {
        let mut by_subtype: Map<String, Value> = Map::new();
        let insert = |sub: &str, name: String, v: Value, map: &mut Map<String, Value>| {
            map.entry(sub.to_string())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("subtype maps are objects")
                .insert(name, v);
        };
        for t in &net.transformers {
            self.warn_nonuniform_per_phase_taps(t);
            match classify(t) {
                Kind::SinglePhase => {
                    if t.windings.iter().any(|w| w.conn == WindingConn::Delta) {
                        // An open wye / open delta leg. The single_phase shape
                        // carries the terminals and impedance faithfully, but
                        // has no field for the wye/delta connection, so a
                        // consumer that models the subtype literally reads it
                        // as a wye-wye unit. Flag it; the line to line topology
                        // survives in the terminal map.
                        let connection = match (t.windings[0].conn, t.windings[1].conn) {
                            (WindingConn::Wye, WindingConn::Delta) => "wye/delta",
                            (WindingConn::Delta, WindingConn::Wye) => "delta/wye",
                            _ => "delta",
                        };
                        let mut details = Map::new();
                        details.insert("connection".into(), json!(connection));
                        details.insert("emitted_subtype".into(), json!("single_phase"));
                        self.transformer_diagnostic(
                            t,
                            "EMIT.BMOPF.TRANSFORMER_CONNECTION_LOSSY",
                            format!(
                                "transformer {}: single phase wye/delta emitted as single_phase; \
                                 the wye/delta connection is not encoded in the subtype, only the \
                                 line to line terminal map",
                                t.name
                            ),
                            details,
                        );
                    }
                    let v = self.two_winding(t, &t.windings[0], &t.windings[1], 1.0, true, true);
                    insert("single_phase", t.name.clone(), v, &mut by_subtype);
                }
                Kind::SinglePhaseShape(sub) => {
                    let v = self.two_winding(t, &t.windings[0], &t.windings[1], 1.0, true, true);
                    insert(sub, t.name.clone(), v, &mut by_subtype);
                }
                Kind::CenterTap => {
                    let v = self.center_tap(t);
                    insert("center_tap", t.name.clone(), v, &mut by_subtype);
                }
                Kind::WyeDelta => {
                    let v = self.three_phase(t, 0);
                    insert("wye_delta", t.name.clone(), v, &mut by_subtype);
                }
                Kind::DeltaWye => {
                    let v = self.three_phase(t, 1);
                    insert("delta_wye", t.name.clone(), v, &mut by_subtype);
                }
                Kind::WyeWye3 => {
                    for (k, v) in self.decompose_wye_wye(t) {
                        insert("single_phase", k, v, &mut by_subtype);
                    }
                }
                Kind::NWinding => {
                    let v = self.n_winding(t);
                    insert("n_winding", t.name.clone(), v, &mut by_subtype);
                }
                Kind::Unsupported(why) => {
                    let mut details = Map::new();
                    details.insert("reason".into(), json!(&why));
                    details.insert("phases".into(), json!(t.phases));
                    details.insert("windings".into(), json!(t.windings.len()));
                    self.transformer_diagnostic(
                        t,
                        "EMIT.BMOPF.TRANSFORMER_UNSUPPORTED",
                        format!(
                            "transformer {}: {why}; not representable in the four BMOPF \
                             subtypes, dropped from the output",
                            t.name
                        ),
                        details,
                    );
                }
            }
        }
        by_subtype
    }

    /// Shared single_phase / center_tap shape. `to_scale` rescales the to
    /// side ratings (used by the wye-wye decomposition).
    fn two_winding(
        &mut self,
        t: &DistTransformer,
        from: &Winding,
        to: &Winding,
        s_scale: f64,
        emit_no_load: bool,
        warn_extras: bool,
    ) -> Value {
        let s = from.s_rating * s_scale;
        let zb_from = from.v_ref * from.v_ref / s;
        let zb_to = to.v_ref * to.v_ref / s;
        let mut o = Map::new();
        o.insert("bus_from".into(), json!(from.bus));
        o.insert("bus_to".into(), json!(to.bus));
        o.insert("s_rating".into(), self.num(s, "transformer s_rating"));
        o.insert(
            "v_nom_from".into(),
            self.num(from.v_ref, "transformer v_nom_from"),
        );
        o.insert(
            "v_nom_to".into(),
            self.num(to.v_ref, "transformer v_nom_to"),
        );
        o.insert(
            "r_series_from".into(),
            self.num(from.r_pct / 100.0 * zb_from, "transformer r_series_from"),
        );
        o.insert(
            "r_series_to".into(),
            self.num(to.r_pct / 100.0 * zb_to, "transformer r_series_to"),
        );
        // The whole leakage reactance rides on the from side, the
        // convention the public example uses.
        if t.xsc_pct.is_empty() {
            self.transformer_diagnostic(
                t,
                "EMIT.BMOPF.TRANSFORMER_MISSING_XSC",
                format!(
                    "transformer {}: xsc_pct is empty; emitted x_series_from=0",
                    t.name
                ),
                Map::new(),
            );
        }
        let xhl = t.xsc_pct.first().copied().unwrap_or(0.0);
        o.insert(
            "x_series_from".into(),
            self.num(xhl / 100.0 * zb_from, "transformer x_series_from"),
        );
        o.insert("x_series_to".into(), json!(0.0));
        o.insert("terminal_map_from".into(), json!(from.terminal_map));
        o.insert("terminal_map_to".into(), json!(to.terminal_map));
        self.transformer_neutral_fields(&mut o, t, from, to);
        self.transformer_tap_fields(&mut o, t, from, to);
        if emit_no_load {
            self.transformer_no_load_fields(&mut o, t, from, s);
        }
        if warn_extras {
            self.transformer_extras_dropped(t, &TRANSFORMER_TWO_WINDING_ALLOWED_EXTRAS);
        }
        o.into()
    }

    fn center_tap(&mut self, t: &DistTransformer) -> Value {
        let from = &t.windings[0];
        let (w2, w3) = (&t.windings[1], &t.windings[2]);
        let common = center_tap_common_terminal(w2, w3);
        let r_neutral = self.center_tap_neutral(t, "r_neutral", w2.r_neutral, w3.r_neutral);
        let x_neutral = self.center_tap_neutral(t, "x_neutral", w2.x_neutral, w3.x_neutral);
        if (w2.tap - w3.tap).abs() > 1e-9 {
            let mut details = Map::new();
            details.insert("from_tap".into(), json!(from.tap));
            details.insert("secondary_taps".into(), json!([w2.tap, w3.tap]));
            details.insert("emitted_secondary_tap".into(), json!(w2.tap));
            self.transformer_diagnostic(
                t,
                "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_TAP_COLLAPSED",
                format!(
                    "transformer {}: center tap secondary half winding taps ({}, {}) differ; emitted the first half tap",
                    t.name, w2.tap, w3.tap
                ),
                details,
            );
        }
        let to = center_tap_to_winding(w2, w3, &common, from.s_rating, r_neutral, x_neutral);
        if w2.s_rating.to_bits() != from.s_rating.to_bits()
            || w3.s_rating.to_bits() != from.s_rating.to_bits()
        {
            let mut details = Map::new();
            details.insert("from_s_rating".into(), json!(from.s_rating));
            details.insert("half_s_ratings".into(), json!([w2.s_rating, w3.s_rating]));
            self.transformer_diagnostic(
                t,
                "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_RATING_COLLAPSED",
                format!(
                    "transformer {}: center tap half winding s_ratings ({}, {}) differ \
                     from the primary's {}; BMOPF carries one transformer rating, and \
                     the first secondary half rating is used for the to-side impedance base",
                    t.name, w2.s_rating, w3.s_rating, from.s_rating
                ),
                details,
            );
        }
        let s = from.s_rating;
        let zb_from = winding_base(from);
        let zb_to = winding_base(w2);
        if t.xsc_pct.is_empty() {
            self.transformer_diagnostic(
                t,
                "EMIT.BMOPF.TRANSFORMER_MISSING_XSC",
                format!(
                    "transformer {}: xsc_pct is empty; emitted x_series_from=0",
                    t.name
                ),
                Map::new(),
            );
        }
        let (x_from_pct, x_to_pct) = self.center_tap_leakage_percentages(t);

        let mut o = Map::new();
        o.insert("bus_from".into(), json!(from.bus));
        o.insert("bus_to".into(), json!(to.bus));
        o.insert("s_rating".into(), self.num(s, "transformer s_rating"));
        o.insert(
            "v_nom_from".into(),
            self.num(from.v_ref, "transformer v_nom_from"),
        );
        o.insert(
            "v_nom_to".into(),
            self.num(to.v_ref, "transformer v_nom_to"),
        );
        o.insert(
            "r_series_from".into(),
            self.num(from.r_pct / 100.0 * zb_from, "transformer r_series_from"),
        );
        o.insert(
            "r_series_to".into(),
            self.num(w2.r_pct / 100.0 * zb_to, "transformer r_series_to"),
        );
        o.insert(
            "x_series_from".into(),
            self.num(x_from_pct / 100.0 * zb_from, "transformer x_series_from"),
        );
        o.insert(
            "x_series_to".into(),
            self.num(x_to_pct / 100.0 * zb_to, "transformer x_series_to"),
        );
        o.insert("terminal_map_from".into(), json!(from.terminal_map));
        o.insert("terminal_map_to".into(), json!(to.terminal_map));
        self.transformer_neutral_fields(&mut o, t, from, &to);
        self.transformer_tap_fields(&mut o, t, from, &to);
        self.transformer_no_load_fields(&mut o, t, from, s);
        self.transformer_extras_dropped(t, &TRANSFORMER_TWO_WINDING_ALLOWED_EXTRAS);
        o.into()
    }

    fn center_tap_leakage_percentages(&mut self, t: &DistTransformer) -> (f64, f64) {
        let (x_from_pct, x_to_pct) = center_tap_star_percentages(&t.xsc_pct);
        if x_from_pct.is_finite()
            && x_to_pct.is_finite()
            && x_from_pct >= -1e-12
            && x_to_pct >= -1e-12
        {
            return (x_from_pct.max(0.0), x_to_pct.max(0.0));
        }
        let xhl = t.xsc_pct.first().copied().unwrap_or(0.0);
        let emitted_from = if xhl.is_finite() { xhl.max(0.0) } else { 0.0 };
        let mut details = Map::new();
        details.insert("xsc_pct".into(), json!(&t.xsc_pct));
        details.insert("star_percentages".into(), json!([x_from_pct, x_to_pct]));
        details.insert("emitted_percentages".into(), json!([emitted_from, 0.0]));
        self.transformer_diagnostic(
            t,
            "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_LEAKAGE_UNREPRESENTABLE",
            format!(
                "transformer {}: center tap leakage star arms ({x_from_pct}, {x_to_pct}) \
                 are not representable as nonnegative BMOPF fields; emitted xhl on the \
                 from side and zero on the to side",
                t.name
            ),
            details,
        );
        (emitted_from, 0.0)
    }

    /// `wye_delta` stays in the legacy lumped wye side form. `delta_wye`
    /// uses split fields referred to each winding's own base.
    fn three_phase(&mut self, t: &DistTransformer, wye_idx: usize) -> Value {
        let from = &t.windings[0];
        let to = &t.windings[1];
        let is_delta_wye = wye_idx == 1;
        let s = from.s_rating;
        let mut o = Map::new();
        o.insert("bus_from".into(), json!(from.bus));
        o.insert("bus_to".into(), json!(to.bus));
        o.insert("s_rating".into(), self.num(s, "transformer s_rating"));
        o.insert(
            "v_nom_from".into(),
            self.num(from.v_ref, "transformer v_nom_from"),
        );
        o.insert(
            "v_nom_to".into(),
            self.num(to.v_ref, "transformer v_nom_to"),
        );
        if t.xsc_pct.is_empty() {
            let emitted = if is_delta_wye {
                "x_series_from=0 and x_series_to=0"
            } else {
                "x_series=0"
            };
            self.transformer_diagnostic(
                t,
                "EMIT.BMOPF.TRANSFORMER_MISSING_XSC",
                format!(
                    "transformer {}: xsc_pct is empty; emitted {emitted}",
                    t.name,
                ),
                Map::new(),
            );
        }
        let xhl = t.xsc_pct.first().copied().unwrap_or(0.0);
        if is_delta_wye {
            let zb_from = winding_base(from);
            let zb_to = winding_base(to);
            o.insert(
                "r_series_from".into(),
                self.num(from.r_pct / 100.0 * zb_from, "transformer r_series_from"),
            );
            o.insert(
                "r_series_to".into(),
                self.num(to.r_pct / 100.0 * zb_to, "transformer r_series_to"),
            );
            o.insert(
                "x_series_from".into(),
                self.num(xhl / 2.0 / 100.0 * zb_from, "transformer x_series_from"),
            );
            o.insert(
                "x_series_to".into(),
                self.num(xhl / 2.0 / 100.0 * zb_to, "transformer x_series_to"),
            );
        } else {
            let wye = &t.windings[wye_idx];
            let zb_wye = wye.v_ref * wye.v_ref / s;
            o.insert(
                "r_series".into(),
                self.num(
                    (from.r_pct + to.r_pct) / 100.0 * zb_wye,
                    "transformer r_series",
                ),
            );
            o.insert(
                "x_series".into(),
                self.num(xhl / 100.0 * zb_wye, "transformer x_series"),
            );
        }
        o.insert("terminal_map_from".into(), json!(from.terminal_map));
        o.insert("terminal_map_to".into(), json!(to.terminal_map));
        self.transformer_neutral_fields(&mut o, t, from, to);
        self.transformer_tap_fields(&mut o, t, from, to);
        self.transformer_no_load_fields(&mut o, t, from, s);
        self.transformer_extras_dropped(t, &TRANSFORMER_TWO_WINDING_ALLOWED_EXTRAS);
        o.into()
    }

    fn n_winding(&mut self, t: &DistTransformer) -> Value {
        let s = t.windings.first().map_or(f64::NAN, |w| w.s_rating);
        if t.windings
            .iter()
            .any(|w| w.s_rating.to_bits() != s.to_bits())
        {
            let mut details = Map::new();
            details.insert(
                "s_ratings".into(),
                json!(t.windings.iter().map(|w| w.s_rating).collect::<Vec<_>>()),
            );
            self.transformer_diagnostic(
                t,
                "EMIT.BMOPF.TRANSFORMER_N_WINDING_RATING_COLLAPSED",
                format!(
                    "transformer {}: n_winding BMOPF carries one s_rating; emitted the first winding rating",
                    t.name
                ),
                details,
            );
        }
        let mut o = Map::new();
        o.insert("s_rating".into(), self.num(s, "transformer s_rating"));
        let windings: Vec<Value> = t
            .windings
            .iter()
            .enumerate()
            .map(|(idx, w)| {
                let mut wj = Map::new();
                wj.insert("bus".into(), json!(w.bus));
                wj.insert("terminal_map".into(), json!(w.terminal_map));
                wj.insert(
                    "v_nom".into(),
                    self.num(n_winding_bmopf_v_nom(w), "transformer winding v_nom"),
                );
                wj.insert(
                    "configuration".into(),
                    json!(match w.conn {
                        WindingConn::Wye => "WYE",
                        WindingConn::Delta => "DELTA",
                    }),
                );
                let zbase = n_winding_base(w, s).unwrap_or(f64::NAN);
                wj.insert(
                    "r_winding".into(),
                    self.num(w.r_pct / 100.0 * zbase, "transformer winding r_winding"),
                );
                if let Some(delta_roll) = bmopf_delta_roll(t, idx, w) {
                    wj.insert("delta_roll".into(), json!(delta_roll));
                }
                Value::Object(wj)
            })
            .collect();
        o.insert("windings".into(), Value::Array(windings));
        let base_z = t
            .windings
            .first()
            .and_then(|w| n_winding_base(w, s))
            .unwrap_or(f64::NAN);
        let mut x_sc = Map::new();
        for (idx, (i, j)) in pair_keys(t.windings.len()).into_iter().enumerate() {
            let x_pct = t.xsc_pct.get(idx).copied().unwrap_or_else(|| {
                let mut details = Map::new();
                details.insert("winding_pair".into(), json!(format!("{}_{}", i + 1, j + 1)));
                self.transformer_diagnostic(
                    t,
                    "EMIT.BMOPF.TRANSFORMER_MISSING_XSC",
                    format!(
                        "transformer {}: missing x_sc for winding pair {}_{}; emitted 0",
                        t.name,
                        i + 1,
                        j + 1
                    ),
                    details,
                );
                0.0
            });
            x_sc.insert(
                format!("{}_{}", i + 1, j + 1),
                self.num(x_pct / 100.0 * base_z, "transformer x_sc"),
            );
        }
        o.insert("x_sc".into(), Value::Object(x_sc));
        if let Some(first) = t.windings.first() {
            self.transformer_no_load_fields(&mut o, t, first, s);
        }
        self.warn_unrepresented_neutral_fields(t, "n_winding BMOPF");
        self.taps_dropped(t);
        self.transformer_extras_dropped(t, &TRANSFORMER_NO_LOAD_ALLOWED_EXTRAS);
        o.into()
    }

    /// A three phase wye-wye unit becomes one single_phase entry per phase
    /// (`name_1`..), each at line to neutral voltage and a third of the
    /// rating. That keeps the impedance base v^2/s, so the percent values
    /// carry over unchanged. The public IEEE13 example records the line to
    /// line voltage on its decomposed units instead; both are self
    /// consistent, they differ in the v_ref convention.
    fn decompose_wye_wye(&mut self, t: &DistTransformer) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        let (from, to) = (&t.windings[0], &t.windings[1]);
        let sqrt3 = 3f64.sqrt();
        for k in 0..t.phases {
            let per = |w: &Winding| {
                let neutral = w.terminal_map.last().cloned().unwrap_or_default();
                Winding {
                    bus: w.bus.clone(),
                    terminal_map: vec![w.terminal_map[k].clone(), neutral],
                    conn: WindingConn::Wye,
                    v_ref: w.v_ref / sqrt3,
                    s_rating: w.s_rating / 3.0,
                    r_pct: w.r_pct,
                    tap: w.tap,
                    r_neutral: if k == 0 { w.r_neutral } else { None },
                    x_neutral: if k == 0 { w.x_neutral } else { None },
                }
            };
            let f = per(from);
            let to_1 = per(to);
            let mut t1 = t.clone();
            t1.windings = vec![f.clone(), to_1.clone()];
            split_no_load_extras(&mut t1, t.phases);
            let v = self.two_winding(&t1, &f, &to_1, 1.0, true, false);
            out.push((format!("{}_{}", t.name, k + 1), v));
        }
        let mut details = Map::new();
        details.insert("emitted_subtype".into(), json!("single_phase"));
        details.insert("units".into(), json!(t.phases));
        self.transformer_diagnostic(
            t,
            "EMIT.BMOPF.TRANSFORMER_WYE_WYE_DECOMPOSED",
            format!(
                "transformer {}: three phase wye-wye decomposed into {} single_phase units",
                t.name, t.phases
            ),
            details,
        );
        self.transformer_extras_dropped(t, &TRANSFORMER_TWO_WINDING_ALLOWED_EXTRAS);
        out
    }

    fn taps_dropped(&mut self, t: &DistTransformer) {
        for w in &t.windings {
            if (w.tap - 1.0).abs() > 1e-12 {
                let mut details = Map::new();
                details.insert("tap".into(), json!(w.tap));
                self.transformer_diagnostic(
                    t,
                    "EMIT.BMOPF.TRANSFORMER_TAP_DROPPED",
                    format!(
                        "transformer {}: off nominal tap {} has no BMOPF field; dropped",
                        t.name, w.tap
                    ),
                    details,
                );
            }
        }
    }

    fn transformer_tap_fields(
        &mut self,
        o: &mut Map<String, Value>,
        t: &DistTransformer,
        from: &Winding,
        to: &Winding,
    ) {
        if to.tap.abs() <= 1e-12 {
            if (from.tap - 1.0).abs() > 1e-12 || (to.tap - 1.0).abs() > 1e-12 {
                let mut details = Map::new();
                details.insert("from_tap".into(), json!(from.tap));
                details.insert("to_tap".into(), json!(to.tap));
                self.transformer_diagnostic(
                    t,
                    "EMIT.BMOPF.TRANSFORMER_TAP_DROPPED",
                    format!(
                        "transformer {}: to-side tap {} cannot form a finite BMOPF ratio; dropped",
                        t.name, to.tap
                    ),
                    details,
                );
            }
        } else {
            let tap = from.tap / to.tap;
            if (tap - 1.0).abs() > 1e-12 || t.extras.contains_key("tap") {
                o.insert("tap".into(), self.num(tap, "transformer tap"));
            }
        }
        for key in ["tap_min", "tap_max"] {
            if let Some(v) = extras_number(&t.extras, key) {
                o.insert(key.into(), self.num(v, &format!("transformer {key}")));
            }
        }
    }

    fn warn_nonuniform_per_phase_taps(&mut self, t: &DistTransformer) {
        let Some(tm_set) = t.extras.get("pmd_tm_set").and_then(Value::as_array) else {
            return;
        };
        for (idx, raw) in tm_set.iter().enumerate() {
            let Some(taps) = tap_values(raw) else {
                continue;
            };
            let Some(first) = taps.first().copied() else {
                continue;
            };
            if taps.iter().any(|tap| (tap - first).abs() > 1e-9) {
                let mut details = Map::new();
                details.insert("winding".into(), json!(idx + 1));
                details.insert("source_taps".into(), json!(taps));
                details.insert("emitted_winding_tap".into(), json!(first));
                self.transformer_diagnostic(
                    t,
                    "EMIT.BMOPF.TRANSFORMER_PER_PHASE_TAP_COLLAPSED",
                    format!(
                        "transformer {}: winding {} has non-uniform per phase taps; emitted the first phase tap",
                        t.name,
                        idx + 1
                    ),
                    details,
                );
            }
        }
    }

    fn transformer_neutral_fields(
        &mut self,
        o: &mut Map<String, Value>,
        t: &DistTransformer,
        from: &Winding,
        to: &Winding,
    ) {
        self.transformer_neutral_field(o, t, "r_neutral_from", from.r_neutral);
        self.transformer_neutral_field(o, t, "x_neutral_from", from.x_neutral);
        self.transformer_neutral_field(o, t, "r_neutral_to", to.r_neutral);
        self.transformer_neutral_field(o, t, "x_neutral_to", to.x_neutral);
    }

    fn transformer_neutral_field(
        &mut self,
        o: &mut Map<String, Value>,
        t: &DistTransformer,
        key: &str,
        value: Option<f64>,
    ) {
        let Some(v) = value else {
            return;
        };
        if v.is_finite() && v >= 0.0 {
            o.insert(key.into(), json!(v));
        } else {
            let mut details = Map::new();
            details.insert("field".into(), json!(key));
            details.insert("value".into(), json!(v));
            self.transformer_diagnostic(
                t,
                "EMIT.BMOPF.TRANSFORMER_NEUTRAL_DROPPED",
                format!(
                    "transformer {}: {key}={v} is not a nonnegative finite BMOPF neutral impedance; dropped",
                    t.name
                ),
                details,
            );
        }
    }

    fn center_tap_neutral(
        &mut self,
        t: &DistTransformer,
        field: &str,
        a: Option<f64>,
        b: Option<f64>,
    ) -> Option<f64> {
        if let (Some(a), Some(b)) = (a, b) {
            let mut details = Map::new();
            details.insert("field".into(), json!(field));
            details.insert("values".into(), json!([a, b]));
            self.transformer_diagnostic(
                t,
                "EMIT.BMOPF.TRANSFORMER_CENTER_TAP_NEUTRAL_COLLAPSED",
                format!(
                    "transformer {}: center tap secondary has two {field} values ({a}, {b}); emitted the first",
                    t.name
                ),
                details,
            );
        }
        a.or(b)
    }

    fn warn_unrepresented_neutral_fields(&mut self, t: &DistTransformer, target: &str) {
        for (idx, w) in t.windings.iter().enumerate() {
            if w.r_neutral.is_some() || w.x_neutral.is_some() {
                let mut details = Map::new();
                details.insert("target".into(), json!(target));
                details.insert("winding".into(), json!(idx + 1));
                self.transformer_diagnostic(
                    t,
                    "EMIT.BMOPF.TRANSFORMER_NEUTRAL_DROPPED",
                    format!(
                        "transformer {} winding {}: neutral impedance has no {target} field; dropped",
                        t.name,
                        idx + 1
                    ),
                    details,
                );
            }
        }
    }

    fn transformer_no_load_fields(
        &mut self,
        o: &mut Map<String, Value>,
        t: &DistTransformer,
        from: &Winding,
        s: f64,
    ) {
        if let Some(v) = t.extras.get("g_no_load") {
            o.insert("g_no_load".into(), v.clone());
        } else if let Some(loss_pct) = extras_number(&t.extras, "%noloadloss") {
            if self.is_phase_to_phase_single_phase(from) {
                let mut details = Map::new();
                details.insert("field".into(), json!("%noloadloss"));
                details.insert("reason".into(), json!("phase_to_phase_single_phase"));
                self.transformer_diagnostic(
                    t,
                    "EMIT.BMOPF.TRANSFORMER_NO_LOAD_SHUNT_DROPPED",
                    format!(
                        "transformer {}: phase-to-phase %noloadloss cannot be represented as a BMOPF no-load shunt; dropped",
                        t.name
                    ),
                    details,
                );
            } else {
                let v_stamp = no_load_voltage_base(from);
                if s.is_finite() && s > 0.0 && v_stamp.is_finite() && v_stamp > 0.0 {
                    let y_base = s / (v_stamp * v_stamp);
                    o.insert(
                        "g_no_load".into(),
                        self.num(loss_pct / 100.0 * y_base, "transformer g_no_load"),
                    );
                } else {
                    let mut details = Map::new();
                    details.insert("field".into(), json!("%noloadloss"));
                    details.insert("s_rating".into(), json!(s));
                    details.insert("v_nom_from".into(), json!(v_stamp));
                    self.transformer_diagnostic(
                        t,
                        "EMIT.BMOPF.TRANSFORMER_NO_LOAD_SHUNT_UNCONVERTIBLE",
                        format!(
                            "transformer {}: %noloadloss cannot be converted without a positive s_rating and v_nom_from",
                            t.name
                        ),
                        details,
                    );
                }
            }
        }

        if let Some(v) = t.extras.get("b_no_load") {
            o.insert("b_no_load".into(), v.clone());
        } else if let Some(imag_pct) = extras_number(&t.extras, "%imag") {
            if self.is_phase_to_phase_single_phase(from) {
                let mut details = Map::new();
                details.insert("field".into(), json!("%imag"));
                details.insert("reason".into(), json!("phase_to_phase_single_phase"));
                self.transformer_diagnostic(
                    t,
                    "EMIT.BMOPF.TRANSFORMER_NO_LOAD_SHUNT_DROPPED",
                    format!(
                        "transformer {}: phase-to-phase %imag cannot be represented as a BMOPF no-load shunt; dropped",
                        t.name
                    ),
                    details,
                );
            } else {
                let v_stamp = no_load_voltage_base(from);
                if s.is_finite() && s > 0.0 && v_stamp.is_finite() && v_stamp > 0.0 {
                    let y_base = s / (v_stamp * v_stamp);
                    o.insert(
                        "b_no_load".into(),
                        self.num(imag_pct / 100.0 * y_base, "transformer b_no_load"),
                    );
                } else {
                    let mut details = Map::new();
                    details.insert("field".into(), json!("%imag"));
                    details.insert("s_rating".into(), json!(s));
                    details.insert("v_nom_from".into(), json!(v_stamp));
                    self.transformer_diagnostic(
                        t,
                        "EMIT.BMOPF.TRANSFORMER_NO_LOAD_SHUNT_UNCONVERTIBLE",
                        format!(
                            "transformer {}: %imag cannot be converted without a positive s_rating and v_nom_from",
                            t.name
                        ),
                        details,
                    );
                }
            }
        } else if !self.is_phase_to_phase_single_phase(from)
            && extras_number(&t.extras, "%noloadloss").is_some()
        {
            o.insert("b_no_load".into(), json!(0.0));
        }
    }

    fn is_phase_to_phase_single_phase(&self, winding: &Winding) -> bool {
        n_winding_phase_count(winding) == 1
            && !self
                .grounded
                .get(&winding.bus.to_ascii_lowercase())
                .is_some_and(|g| winding.terminal_map.iter().any(|t| g.contains(t)))
    }

    fn transformer_extras_dropped(&mut self, t: &DistTransformer, allowed: &[&str]) {
        for key in t.extras.keys() {
            if key == "bmopf_subtype" || key == "tap" || allowed.contains(&key.as_str()) {
                continue;
            }
            let mut details = Map::new();
            details.insert("field".into(), json!(key));
            self.transformer_diagnostic(
                t,
                "EMIT.BMOPF.TRANSFORMER_EXTRA_DROPPED",
                format!(
                    "transformer {}: `{key}` has no place in the BMOPF schema; dropped from the output",
                    t.name
                ),
                details,
            );
        }
    }

    /// Emits a matrix whose `_1_1` entry the schema requires; an empty one
    /// becomes `dim` by `dim` zeros so the required key exists.
    fn required_matrix(
        &mut self,
        o: &mut Map<String, Value>,
        prefix: &str,
        m: &Mat,
        dim: usize,
        name: &str,
    ) {
        if m.is_empty() {
            self.flat_matrix(o, prefix, &vec![vec![0.0; dim]; dim], name);
        } else {
            self.flat_matrix(o, prefix, m, name);
        }
    }

    fn flat_matrix(&mut self, o: &mut Map<String, Value>, prefix: &str, m: &Mat, name: &str) {
        for (i, row) in m.iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                o.insert(
                    format!("{prefix}_{}_{}", i + 1, j + 1),
                    self.num(v, &format!("{name} {prefix}")),
                );
            }
        }
    }
}

fn collect_bus_usage(value: &Value, refs: &mut BTreeMap<String, BTreeSet<String>>) {
    match value {
        Value::Object(o) => {
            add_bus_usage(o, refs, "bus", "terminal_map");
            add_bus_usage(o, refs, "bus_from", "terminal_map_from");
            add_bus_usage(o, refs, "bus_to", "terminal_map_to");
            for value in o.values() {
                collect_bus_usage(value, refs);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_bus_usage(value, refs);
            }
        }
        _ => {}
    }
}

fn add_bus_usage(
    o: &Map<String, Value>,
    refs: &mut BTreeMap<String, BTreeSet<String>>,
    bus_key: &str,
    map_key: &str,
) {
    let Some(id) = o.get(bus_key).and_then(Value::as_str) else {
        return;
    };
    let entry = refs.entry(id.to_string()).or_default();
    if let Some(terms) = o.get(map_key).and_then(Value::as_array) {
        entry.extend(terms.iter().filter_map(Value::as_str).map(str::to_string));
    }
}

fn prune_string_array(
    o: &mut Map<String, Value>,
    key: &str,
    used: &BTreeSet<String>,
    warnings: &mut Vec<String>,
    what: &str,
) {
    let Some(Value::Array(values)) = o.get_mut(key) else {
        return;
    };
    let old = std::mem::take(values);
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for value in old {
        if value.as_str().is_some_and(|s| used.contains(s)) {
            kept.push(value);
        } else {
            dropped.push(value);
        }
    }
    if !dropped.is_empty() {
        let names: Vec<String> = dropped
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        warnings.push(format!(
            "{what}: `{key}` entries {names:?} are not referenced by emitted BMOPF elements; dropped from the output"
        ));
    }
    *values = kept;
}

enum Kind {
    SinglePhase,
    /// Two windings already in the shared single_phase/center_tap shape,
    /// emitted under the named subtype.
    SinglePhaseShape(&'static str),
    CenterTap,
    WyeDelta,
    DeltaWye,
    WyeWye3,
    NWinding,
    Unsupported(String),
}

fn classify(t: &DistTransformer) -> Kind {
    // A network read from BMOPF records its subtype; trust it so writing
    // back reproduces the grouping (center tap reads as two windings).
    // An unknown or shape mismatched subtype falls through to the shape
    // based classification below.
    if let Some(sub) = t.extras.get("bmopf_subtype").and_then(|v| v.as_str()) {
        if t.windings.len() == 2 {
            match sub {
                "single_phase" => return Kind::SinglePhase,
                "center_tap" => return Kind::SinglePhaseShape("center_tap"),
                "wye_delta" => return Kind::WyeDelta,
                "delta_wye" => return Kind::DeltaWye,
                _ => {}
            }
        }
        if sub == "n_winding" && t.windings.len() >= 2 {
            return Kind::NWinding;
        }
    }
    let conns: Vec<WindingConn> = t.windings.iter().map(|w| w.conn).collect();
    match (t.phases, conns.as_slice()) {
        // single_phase covers the plain 1-phase wye-wye unit and both open
        // wye / open delta leg orientations (one delta winding wired line to
        // line). The single_phase shape holds the delta side: it carries two
        // phase terminals, no conn discriminator, and its line to line v_ref
        // makes the per winding impedance base v^2/s already right. The
        // pattern reads as the three pairs wye-wye, delta-wye, wye-delta.
        (
            1,
            [WindingConn::Wye | WindingConn::Delta, WindingConn::Wye]
            | [WindingConn::Wye, WindingConn::Delta],
        ) => Kind::SinglePhase,
        (1, [WindingConn::Wye, WindingConn::Wye, WindingConn::Wye]) => Kind::CenterTap,
        (3, [WindingConn::Wye, WindingConn::Delta]) => Kind::WyeDelta,
        (3, [WindingConn::Delta, WindingConn::Wye]) => Kind::DeltaWye,
        // The decomposition indexes terminal_map[phase] and takes the last
        // entry as the neutral; anything else is not safely decomposable.
        (3, [WindingConn::Wye, WindingConn::Wye])
            if t.windings
                .iter()
                .all(|w| w.terminal_map.len() == t.phases + 1) =>
        {
            Kind::WyeWye3
        }
        (3, [WindingConn::Wye, WindingConn::Wye]) => Kind::Unsupported(
            "three phase wye-wye whose terminal maps do not list each phase plus a neutral".into(),
        ),
        (_, _) if t.windings.len() >= 3 => Kind::NWinding,
        _ => Kind::Unsupported(format!(
            "{} phase with {} windings ({:?})",
            t.phases,
            t.windings.len(),
            conns
        )),
    }
}

fn raw_bmopf_value(u: &crate::model::UntypedObject) -> Option<Value> {
    let (_, text) = u.props.first()?;
    serde_json::from_str(text).ok()
}

fn extras_number(extras: &crate::model::Extras, key: &str) -> Option<f64> {
    let v = extras.get(key)?;
    v.as_f64()
        .or_else(|| v.as_i64().map(|v| v as f64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .filter(|v| v.is_finite())
}

fn tap_values(v: &Value) -> Option<Vec<f64>> {
    if let Some(items) = v.as_array() {
        let out: Vec<f64> = items.iter().filter_map(value_number).collect();
        Some(out)
    } else {
        value_number(v).map(|tap| vec![tap])
    }
}

fn value_number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|v| v as f64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .filter(|v| v.is_finite())
}

fn split_no_load_extras(t: &mut DistTransformer, phases: usize) {
    let phases = phases.max(1) as f64;
    for key in ["g_no_load", "b_no_load"] {
        if let Some(v) = extras_number(&t.extras, key) {
            t.extras.insert(key.into(), json!(v / phases));
        }
    }
}

fn center_tap_common_terminal(w2: &Winding, w3: &Winding) -> String {
    w2.terminal_map
        .iter()
        .find(|term| w3.terminal_map.contains(term))
        .cloned()
        .unwrap_or_default()
}

fn center_tap_to_winding(
    w2: &Winding,
    w3: &Winding,
    common: &str,
    s_rating: f64,
    r_neutral: Option<f64>,
    x_neutral: Option<f64>,
) -> Winding {
    let terminal_map = center_tap_terminal_map(w2, w3, common);
    Winding {
        bus: w2.bus.clone(),
        terminal_map,
        conn: WindingConn::Wye,
        v_ref: w2.v_ref,
        s_rating,
        r_pct: w2.r_pct,
        tap: w2.tap,
        r_neutral,
        x_neutral,
    }
}

fn center_tap_terminal_map(w2: &Winding, w3: &Winding, common: &str) -> Vec<String> {
    let mut hots: Vec<String> = Vec::new();
    for term in w2.terminal_map.iter().chain(&w3.terminal_map) {
        if term != common && !hots.contains(term) {
            hots.push(term.clone());
        }
    }
    let first = hots.first().cloned().unwrap_or_default();
    let second = hots.get(1).cloned().unwrap_or_default();
    vec![first, common.to_string(), second]
}

fn center_tap_star_percentages(xsc_pct: &[f64]) -> (f64, f64) {
    let xhl = xsc_pct.first().copied().unwrap_or(0.0);
    let xht = xsc_pct.get(1).copied().unwrap_or(xhl);
    let xlt = xsc_pct.get(2).copied().unwrap_or(0.0);
    ((xhl + xht - xlt) / 2.0, (xhl + xlt - xht) / 2.0)
}

fn winding_base(w: &Winding) -> f64 {
    w.v_ref * w.v_ref / w.s_rating
}

fn n_winding_phase_count(w: &Winding) -> usize {
    crate::model::n_winding_phase_count(w.conn, &w.terminal_map)
}

fn n_winding_bmopf_v_nom(w: &Winding) -> f64 {
    if w.conn == WindingConn::Wye && n_winding_phase_count(w) >= 2 {
        w.v_ref / 3f64.sqrt()
    } else {
        w.v_ref
    }
}

fn n_winding_base(w: &Winding, s: f64) -> Option<f64> {
    n_winding_impedance_base(n_winding_phase_count(w), n_winding_bmopf_v_nom(w), s)
}

fn bmopf_delta_roll(t: &DistTransformer, idx: usize, w: &Winding) -> Option<i64> {
    if w.conn != WindingConn::Delta {
        return None;
    }
    t.extras
        .get(BMOPF_DELTA_ROLLS_EXTRA)
        .and_then(Value::as_object)
        .and_then(|rolls| rolls.get(&(idx + 1).to_string()))
        .and_then(Value::as_i64)
        .filter(|roll| *roll == 1 || *roll == -1)
        .or(Some(-1))
}

fn no_load_voltage_base(from: &Winding) -> f64 {
    let phases = match from.conn {
        WindingConn::Wye => from.terminal_map.len().saturating_sub(1),
        WindingConn::Delta => from.terminal_map.len(),
    };
    if phases >= 3 {
        from.v_ref / 3f64.sqrt()
    } else {
        from.v_ref
    }
}

fn config_str(c: Configuration) -> &'static str {
    match c {
        Configuration::Wye => "WYE",
        Configuration::Delta => "DELTA",
        Configuration::SinglePhase => "SINGLE_PHASE",
    }
}

fn json_enum<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("enum serializes to a string")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bmopf::parse_bmopf_str;
    use crate::model::DistLoadVoltageModel;

    #[test]
    fn load_voltage_models_round_trip_through_bmopf() {
        let text = r#"{
            "bus": {
                "b1": {"terminal_names": ["1", "2", "3", "4"], "perfectly_grounded_terminals": ["4"]}
            },
            "voltage_source": {
                "source": {
                    "bus": "b1", "terminal_map": ["1", "2", "3", "4"],
                    "v_magnitude": [7200.0, 7200.0, 7200.0, 0.0],
                    "v_angle": [0.0, -120.0, 120.0, 0.0]
                }
            },
            "load": {
                "zip": {
                    "bus": "b1", "terminal_map": ["1", "2", "3", "4"],
                    "configuration": "WYE", "p_nom": [1.0, 2.0, 3.0], "q_nom": [0.1, 0.2, 0.3],
                    "model": "zip", "v_nom": [7200.0, 7200.0, 7200.0],
                    "alpha_z": [0.2, 0.2, 0.2], "alpha_i": [0.3, 0.3, 0.3], "alpha_p": [0.5, 0.5, 0.5],
                    "beta_z": [0.1, 0.1, 0.1], "beta_i": [0.4, 0.4, 0.4], "beta_p": [0.5, 0.5, 0.5]
                },
                "exp": {
                    "bus": "b1", "terminal_map": ["1", "2", "3", "4"],
                    "configuration": "WYE", "p_nom": [1.0, 1.0, 1.0], "q_nom": [0.0, 0.0, 0.0],
                    "model": "exponential", "v_nom": [7200.0, 7200.0, 7200.0],
                    "gamma_p": [1.2, 1.2, 1.2], "gamma_q": [2.1, 2.1, 2.1]
                }
            }
        }"#;
        let net = parse_bmopf_str(text).unwrap();
        let zip = net.loads.iter().find(|l| l.name == "zip").unwrap();
        let exp = net.loads.iter().find(|l| l.name == "exp").unwrap();
        assert!(matches!(
            &zip.voltage_model,
            DistLoadVoltageModel::Zip { alpha_z, .. } if alpha_z == &vec![0.2, 0.2, 0.2]
        ));
        assert!(matches!(
            &exp.voltage_model,
            DistLoadVoltageModel::Exponential { gamma_q, .. } if gamma_q == &vec![2.1, 2.1, 2.1]
        ));

        let out = write_bmopf_json(&net);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let v: Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(
            v["load"]["zip"]["alpha_i"],
            serde_json::json!([0.3, 0.3, 0.3])
        );
        assert_eq!(
            v["load"]["exp"]["gamma_p"],
            serde_json::json!([1.2, 1.2, 1.2])
        );
    }
}
