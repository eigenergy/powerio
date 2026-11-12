//! Cross-field checks over source data before conversion can normalize its shape.

use crate::{collect::Diagnostics, diagnostics::codes as C, model::DistBus};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// Checks the semantic rules of BMOPF conformance (dsopt-schema
/// `docs/conformance.md`) and the structural constraints the reader relies
/// on. `current` says the document states the current schema; a document written
/// against an earlier version keeps that version's line `s_max` reading.
pub(super) fn report(doc: &Map<String, Value>, diagnostics: &mut Diagnostics, current: bool) {
    let mut check = Check {
        doc,
        diagnostics,
        current,
    };
    if let Some(roles) = doc.get("terminal_conventions") {
        let mut seen = BTreeSet::new();
        for role in ["phase", "neutral", "earth"] {
            for label in strings(roles.get(role)) {
                if !seen.insert(label) {
                    check.error(
                        &format!("terminal_conventions.{role}"),
                        "role lists must be unique and disjoint",
                    );
                }
            }
        }
    }
    if let Some(buses) = doc.get("bus").and_then(Value::as_object) {
        for (id, bus) in buses {
            let path = format!("bus.{id}");
            let names = strings(bus.get("terminal_names"));
            if names.is_empty()
                || names.contains(&"g")
                || names.iter().collect::<BTreeSet<_>>().len() != names.len()
            {
                check.error(
                    &path,
                    "terminal_names must be nonempty, unique and exclude implicit ground g",
                );
            }
            for name in strings(bus.get("perfectly_grounded_terminals")) {
                if !names.contains(&name) {
                    check.error(
                        &path,
                        "a perfectly grounded terminal is absent from terminal_names",
                    );
                }
            }
            let phases = phase_count(&names, doc.get("terminal_conventions"));
            check.bus_terminal_rules(bus, &names, phases, &path);
            // `vpg_*` is the phase to ground bound since the revised draft and the
            // only spelling the schema accepts; `v_*` is the first draft's spelling.
            for key in ["vpg_min", "vpg_max", "v_min", "v_max", "vpn_min", "vpn_max"] {
                check.dimension(bus, key, &[phases], &path);
            }
            for key in ["vpp_min", "vpp_max"] {
                check.dimension(bus, key, &[phases * phases.saturating_sub(1) / 2], &path);
            }
            // Renamed bounds: `vpg_*` since the revised draft, `vng_max` in the current schema.
            for (current, legacy) in [
                ("vpg_min", "v_min"),
                ("vpg_max", "v_max"),
                ("vng_max", "vn_max"),
            ] {
                if let (Some(a), Some(b)) = (bus.get(current), bus.get(legacy))
                    && a != b
                {
                    check.error(
                        &path,
                        &format!("{current} and {legacy} both state the bound and disagree"),
                    );
                }
            }
            for (lo, hi) in [
                ("vpg_min", "vpg_max"),
                ("v_min", "v_max"),
                ("vpn_min", "vpn_max"),
                ("vpp_min", "vpp_max"),
            ] {
                if let (Some(low), Some(high)) = (
                    bus.get(lo).and_then(Value::as_array),
                    bus.get(hi).and_then(Value::as_array),
                ) && low
                    .iter()
                    .zip(high)
                    .any(|(a, b)| a.as_f64().zip(b.as_f64()).is_some_and(|(a, b)| a > b))
                {
                    check.error(&path, "a minimum voltage exceeds its maximum");
                }
            }
        }
    }
    check.frequency();
    check.controls();
    for (kind, table) in doc {
        if matches!(
            kind.as_str(),
            "meta" | "extras" | "terminal_conventions" | "bus"
        ) {
            continue;
        }
        check.walk(table, kind);
    }
}

/// Whether a terminal list has a neutral: one the conventions name as
/// neutral, or, without conventions, `n`/`N`, or `"4"` when the terminals
/// are exactly `1`-`4`.
fn has_neutral(names: &[&str], roles: Option<&Value>) -> bool {
    if let Some(roles) = roles {
        return strings(roles.get("neutral"))
            .iter()
            .any(|n| names.contains(n));
    }
    let numeric_four = names.len() == 4 && ["1", "2", "3", "4"].iter().all(|t| names.contains(t));
    names
        .iter()
        .any(|n| n.eq_ignore_ascii_case("n") || (numeric_four && *n == "4"))
}

/// A number, or every entry of a non-empty array, finite and strictly positive.
pub(super) fn strictly_positive(value: &Value) -> bool {
    let one = |v: &Value| v.as_f64().is_some_and(|x| x.is_finite() && x > 0.0);
    one(value)
        || value
            .as_array()
            .is_some_and(|a| !a.is_empty() && a.iter().all(one))
}

fn floats(value: Option<&Value>) -> Option<Vec<f64>> {
    value?.as_array()?.iter().map(Value::as_f64).collect()
}

fn strings(value: Option<&Value>) -> Vec<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn phase_count(names: &[&str], roles: Option<&Value>) -> usize {
    DistBus::new("", names.iter().map(|s| (*s).to_owned()).collect())
        .phase_indices(roles)
        .len()
}

struct Check<'a> {
    doc: &'a Map<String, Value>,
    diagnostics: &'a mut Diagnostics,
    /// The document states the current schema.
    current: bool,
}

impl Check<'_> {
    /// BMOPF.NEUTRAL and BMOPF.DIMENSION on a bus: neutral bounds need a
    /// neutral terminal, pair bounds two phases, and sequence bounds three.
    fn bus_terminal_rules(&mut self, bus: &Value, names: &[&str], phases: usize, path: &str) {
        if !has_neutral(names, self.doc.get("terminal_conventions")) {
            for key in ["vng_max", "vn_max", "vpn_min", "vpn_max"] {
                if bus.get(key).is_some() {
                    self.error(path, &format!("{key} needs a neutral terminal on the bus"));
                }
            }
        }
        if phases < 2 && (bus.get("vpp_min").is_some() || bus.get("vpp_max").is_some()) {
            self.error(path, "vpp_* needs at least two phase terminals");
        }
        if phases != 3
            && ["vpos_min", "vpos_max", "vneg_max", "vzero_max"]
                .iter()
                .any(|key| bus.get(*key).is_some())
        {
            self.error(path, "sequence bounds need exactly three phase terminals");
        }
    }

    /// BMOPF.FREQUENCY: a line geometry's or a linecode derivation's
    /// frequency equals `meta.frequency` when both are stated.
    fn frequency(&mut self) {
        let Some(system) = self
            .doc
            .get("meta")
            .and_then(|m| m.get("frequency"))
            .and_then(Value::as_f64)
        else {
            return;
        };
        let mut stated = Vec::new();
        for (table, pointer) in [
            ("line_geometry", "/frequency"),
            ("linecode", "/derivation/frequency"),
        ] {
            for (name, record) in self
                .doc
                .get(table)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if let Some(f) = record.pointer(pointer).and_then(Value::as_f64) {
                    stated.push((format!("{table}.{name}"), f));
                }
            }
        }
        for (path, f) in stated {
            if (f - system).abs() > 1e-9 * system.abs().max(1.0) {
                self.error(
                    &path,
                    &format!("frequency {f} differs from meta.frequency {system}"),
                );
            }
        }
    }

    /// BMOPF.BOUNDS for control laws: breakpoints do not decrease, Volt-Watt
    /// `p_limits` is `[p_low, p_high]`, and Volt-VAr `q_limits` is
    /// `[q_absorb <= 0, q_inject >= 0]`.
    fn controls(&mut self) {
        let mut found = Vec::new();
        for (name, profile) in self
            .doc
            .get("control_profile")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            for law in ["volt_var", "volt_watt"] {
                let Some(curve) = profile.get(law) else {
                    continue;
                };
                let path = format!("control_profile.{name}.{law}");
                if floats(curve.get("breakpoints"))
                    .is_some_and(|b| b.windows(2).any(|w| w[0] > w[1]))
                {
                    found.push((path.clone(), "breakpoints must not decrease"));
                }
                if law == "volt_watt"
                    && floats(curve.get("p_limits")).is_some_and(|p| p.len() == 2 && p[0] > p[1])
                {
                    found.push((path.clone(), "p_limits must be [p_low, p_high]"));
                }
                if law == "volt_var"
                    && floats(curve.get("q_limits"))
                        .is_some_and(|q| q.len() == 2 && !(q[0] <= 0.0 && q[1] >= 0.0))
                {
                    found.push((path, "q_limits must be [q_absorb <= 0, q_inject >= 0]"));
                }
            }
        }
        for (path, message) in found {
            self.error(&path, message);
        }
    }

    /// A line or switch: its end maps, matrices and limits have one entry per
    /// conductor, and a referenced linecode exists and matches.
    fn line_conductors(
        &mut self,
        value: &Value,
        record: &Map<String, Value>,
        from: &[&str],
        path: &str,
    ) {
        self.dimension(value, "terminal_map_to", &[from.len()], path);
        self.matrices(record, from.len(), path);
        self.dimension(value, "i_max", &[from.len()], path);
        // One entry per conductor in the schema; the revised draft's line text, and so an
        // earlier document, said one per phase.
        let phases = phase_count(from, self.doc.get("terminal_conventions"));
        let s_max: &[usize] = if self.current {
            &[from.len()]
        } else {
            &[from.len(), phases]
        };
        self.dimension(value, "s_max", s_max, path);
        if let Some(id) = record.get("linecode").and_then(Value::as_str) {
            if let Some(code) = self
                .doc
                .get("linecode")
                .and_then(|c| c.get(id))
                .and_then(Value::as_object)
            {
                self.matrices(code, from.len(), &format!("{path}.linecode"));
                if let Some(limits) = code.get("s_max").and_then(Value::as_array)
                    && limits.len() != from.len()
                {
                    self.error(
                        &format!("{path}.linecode"),
                        "linecode s_max needs one entry per conductor of the line",
                    );
                }
            } else {
                self.error(path, "referenced linecode does not exist");
            }
        }
    }

    /// Strictly positive values and the field combinations the schema
    /// requires: ratings, nominal voltages, taps and ratios, wire radius and
    /// GMR, and frequencies are positive; a wire states a resistance and a
    /// radius or GMR; a voltage-dependent load states `v_nom`, and a ZIP or
    /// exponential load its coefficients.
    fn constraints(&mut self, record: &Map<String, Value>, path: &str) {
        let positive: &[&str] =
            if path.starts_with("transformer.") && path.matches('.').count() == 2 {
                &[
                    "s_rating",
                    "v_nom_from",
                    "v_nom_to",
                    "tap_ratio",
                    "tap_ratio_min",
                    "tap_ratio_max",
                    "voltage_ratio",
                    "voltage_ratio_min",
                    "voltage_ratio_max",
                ]
            } else if path.starts_with("transformer.n_winding.") && path.contains(".windings[") {
                &[
                    "v_nom",
                    "s_rating",
                    "tap_ratio",
                    "tap_ratio_min",
                    "tap_ratio_max",
                ]
            } else if path.starts_with("capacitor.") {
                &["v_nom"]
            } else if path.starts_with("line_geometry.")
                || (path.starts_with("linecode.") && path.ends_with(".derivation"))
            {
                &["frequency"]
            } else if path.starts_with("wire_data.") {
                &["gmr", "radius"]
            } else {
                &[]
            };
        for key in positive {
            if record.get(*key).is_some_and(|v| !strictly_positive(v)) {
                self.error(&format!("{path}.{key}"), "must be strictly positive");
            }
        }
        if path.starts_with("wire_data.") && path.matches('.').count() == 1 {
            if !record.contains_key("r_dc") && !record.contains_key("r_ac") {
                self.error(path, "a wire states r_dc or r_ac");
            }
            if !record.contains_key("gmr") && !record.contains_key("radius") {
                self.error(path, "a wire states gmr or radius");
            }
        }
        if path.starts_with("load.") && path.matches('.').count() == 1 {
            let needed: &[&str] = match record.get("model").and_then(Value::as_str) {
                None | Some("CONSTANT_POWER") => &[],
                Some("ZIP") => &[
                    "v_nom", "alpha_p", "alpha_i", "alpha_z", "beta_p", "beta_i", "beta_z",
                ],
                Some("EXPONENTIAL") => &["v_nom", "gamma_p", "gamma_q"],
                Some(_) => &["v_nom"],
            };
            for key in needed {
                if !record.contains_key(*key) {
                    self.error(path, &format!("the load model needs `{key}`"));
                }
            }
        }
    }

    fn error(&mut self, path: &str, message: &str) {
        self.diagnostics.push(
            &C::READ_BMOPF_SEMANTIC_INVALID,
            format!("{path}: {message}"),
        );
    }

    fn dimension(&mut self, record: &Value, key: &str, expected: &[usize], path: &str) {
        if let Some(values) = record.get(key).and_then(Value::as_array)
            && !expected.contains(&values.len())
        {
            self.error(
                &format!("{path}.{key}"),
                &format!("expected array length {expected:?}, found {}", values.len()),
            );
        }
    }

    fn map(&mut self, record: &Value, bus_key: &str, map_key: &str, path: &str) {
        self.map_table(record, "bus", bus_key, map_key, path);
    }

    fn map_table(&mut self, record: &Value, table: &str, bus_key: &str, map_key: &str, path: &str) {
        let Some(bus_id) = record.get(bus_key).and_then(Value::as_str) else {
            return;
        };
        let Some(bus) = self.doc.get(table).and_then(|b| b.get(bus_id)) else {
            self.error(
                &format!("{path}.{bus_key}"),
                "referenced bus does not exist",
            );
            return;
        };
        let terminals = strings(bus.get("terminal_names"));
        let map = record
            .get(map_key)
            .and_then(Value::as_str)
            .map_or_else(|| strings(record.get(map_key)), |terminal| vec![terminal]);
        for terminal in map {
            if terminal != "g" && !terminals.contains(&terminal) {
                self.error(
                    &format!("{path}.{map_key}"),
                    &format!("terminal {terminal} does not exist on bus {bus_id}"),
                );
            }
        }
    }

    fn matrices(&mut self, record: &Map<String, Value>, n: usize, path: &str) {
        for key in record.keys() {
            let parts: Vec<_> = key.rsplitn(3, '_').collect();
            if parts.len() != 3
                || !matches!(
                    parts[2],
                    "R_series"
                        | "X_series"
                        | "G_shunt"
                        | "B_shunt"
                        | "G_from"
                        | "B_from"
                        | "G_to"
                        | "B_to"
                        | "G"
                        | "B"
                )
            {
                continue;
            }
            if let (Ok(i), Ok(j)) = (parts[1].parse::<usize>(), parts[0].parse::<usize>())
                && (i == 0 || j == 0 || i > n || j > n)
            {
                self.error(
                    &format!("{path}.{key}"),
                    &format!("matrix index exceeds conductor dimension {n}"),
                );
            }
        }
    }

    fn tap_bounds(&mut self, value: &Value, record: &Map<String, Value>, path: &str) {
        for root in ["tap", "tap_ratio", "voltage_ratio"] {
            let count = if path.starts_with("transformer.open_delta_regulator.") {
                2
            } else {
                1
            };
            let at = |key: &str, index: usize| {
                record.get(key).and_then(|v| {
                    v.as_f64().or_else(|| {
                        v.as_array()
                            .and_then(|a| a.get(index))
                            .and_then(Value::as_f64)
                    })
                })
            };
            for key in [
                root.to_owned(),
                format!("{root}_min"),
                format!("{root}_max"),
            ] {
                self.dimension(value, &key, &[count], path);
            }
            for i in 0..count {
                let lo = at(&format!("{root}_min"), i);
                let hi = at(&format!("{root}_max"), i);
                let tap = at(root, i);
                if lo.zip(hi).is_some_and(|(lo, hi)| lo > hi)
                    || tap.is_some_and(|t| {
                        t <= 0.0 || lo.is_some_and(|v| t < v) || hi.is_some_and(|v| t > v)
                    })
                {
                    self.error(path, "tap must be positive and consistent with its bounds");
                }
            }
        }
    }

    fn references_and_profiles(&mut self, value: &Value, record: &Map<String, Value>, path: &str) {
        for field in ["control_profile", "line_geometry", "wire_data"] {
            if let Some(id) = record.get(field).and_then(Value::as_str)
                && self
                    .doc
                    .get(field)
                    .and_then(|table| table.get(id))
                    .is_none()
            {
                self.error(
                    &format!("{path}.{field}"),
                    "referenced record does not exist",
                );
            }
        }
        // An inverter's DC coupling names its DC terminals in
        // `dc_terminal_map`, which the schema requires beside `dc_bus`; its
        // `terminal_map` names AC terminals, so it cannot stand in.
        let inverter = path.starts_with("ibr.");
        if inverter && record.contains_key("dc_bus") && !record.contains_key("dc_terminal_map") {
            self.error(
                &format!("{path}.dc_terminal_map"),
                "an inverter with `dc_bus` requires `dc_terminal_map`",
            );
        }
        for (bus, map) in [
            ("dc_bus", "terminal_map"),
            ("dc_bus_from", "terminal_map_from"),
            ("dc_bus_to", "terminal_map_to"),
        ] {
            let map = if record.contains_key("dc_terminal_map") {
                "dc_terminal_map"
            } else if inverter && bus == "dc_bus" {
                continue;
            } else {
                map
            };
            self.map_table(value, "dc_bus", bus, map, path);
        }
        if path.starts_with("dc_grounding.") {
            self.map_table(value, "dc_bus", "dc_bus", "terminal", path);
        }
        if path.starts_with("dc_branch.") && record.contains_key("dc_bus_from") {
            let count = strings(record.get("terminal_map_from")).len();
            for key in ["terminal_map_to", "r", "i_max"] {
                self.dimension(value, key, &[count], path);
            }
        }
        if path.starts_with("dc_bus.") && record.contains_key("terminal_names") {
            let names = strings(record.get("terminal_names"));
            if names.iter().collect::<BTreeSet<_>>().len() != names.len()
                || strings(record.get("perfectly_grounded_terminals"))
                    .iter()
                    .any(|n| !names.contains(n))
                || record
                    .get("pole")
                    .and_then(Value::as_object)
                    .is_some_and(|p| p.keys().any(|n| !names.contains(&n.as_str())))
            {
                self.error(path, "DC terminals must be unique and include every grounded terminal and pole label");
            }
            for key in ["v_dc_nom", "v_dc_min", "v_dc_max"] {
                self.dimension(value, key, &[names.len()], path);
            }
            if let (Some(lo), Some(hi)) = (
                record.get("v_dc_min").and_then(Value::as_array),
                record.get("v_dc_max").and_then(Value::as_array),
            ) && lo
                .iter()
                .zip(hi)
                .any(|(a, b)| a.as_f64().zip(b.as_f64()).is_some_and(|(a, b)| a > b))
            {
                self.error(path, "minimum DC voltage exceeds maximum");
            }
        }
        self.profiles_and_windings(value, record, path);
    }

    fn profiles_and_windings(&mut self, value: &Value, record: &Map<String, Value>, path: &str) {
        if let Some(profiles) = record.get("time_series").and_then(Value::as_object) {
            for (field, id) in profiles {
                if !record.get(field).is_some_and(numeric_value) {
                    self.error(
                        &format!("{path}.time_series.{field}"),
                        "profile requires a stated numeric field on the element",
                    );
                }
                if id
                    .as_str()
                    .and_then(|id| self.doc.get("time_series").and_then(|table| table.get(id)))
                    .is_none()
                {
                    self.error(
                        &format!("{path}.time_series.{field}"),
                        "referenced time profile does not exist",
                    );
                }
            }
        }
        if path.starts_with("time_series.") && record.contains_key("values") {
            let count = record
                .get("values")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            self.dimension(value, "time", &[count], path);
            if let Some(time) = record.get("time").and_then(Value::as_array)
                && time.windows(2).any(|pair| {
                    pair[0]
                        .as_f64()
                        .zip(pair[1].as_f64())
                        .is_some_and(|(a, b)| a >= b)
                })
            {
                self.error(path, "profile time must be strictly increasing");
            }
        }
        if let Some(windings) = record.get("windings").and_then(Value::as_array)
            && let Some(pairs) = record.get("x_sc").and_then(Value::as_object)
        {
            let n = windings.len();
            for key in pairs.keys() {
                let valid = key
                    .split_once('_')
                    .and_then(|(a, b)| a.parse::<usize>().ok().zip(b.parse::<usize>().ok()))
                    .is_some_and(|(a, b)| a > 0 && a < b && b <= n);
                if !valid {
                    self.error(
                        &format!("{path}.x_sc.{key}"),
                        "expected winding pair i_j with 1 <= i < j <= winding count",
                    );
                }
            }
            if pairs.len() != n * n.saturating_sub(1) / 2 {
                self.error(
                    &format!("{path}.x_sc"),
                    "short-circuit table must state every winding pair",
                );
            }
        }
    }

    fn no_load_shunt(&mut self, record: &Map<String, Value>, path: &str) {
        if let Some(shunt) = record.get("no_load_shunt") {
            let count = record
                .get("windings")
                .and_then(Value::as_array)
                .map_or_else(
                    || {
                        if path.starts_with("transformer.center_tap.") {
                            3
                        } else {
                            2
                        }
                    },
                    Vec::len,
                );
            let winding = shunt.get("winding").and_then(Value::as_u64);
            if winding.is_none_or(|index| index == 0 || index > count as u64) {
                self.error(
                    &format!("{path}.no_load_shunt.winding"),
                    "no-load shunt winding does not exist",
                );
            }
            if shunt
                .get("g")
                .and_then(Value::as_f64)
                .is_none_or(|v| v < 0.0)
                || shunt.get("b").and_then(Value::as_f64).is_none()
                || record.contains_key("g_no_load")
                || record.contains_key("b_no_load")
            {
                self.error(&format!("{path}.no_load_shunt"), "requires nonnegative g, finite b and no competing g_no_load/b_no_load representation");
            }
        }
    }

    fn cost_aliases(&mut self, record: &Map<String, Value>, path: &str) {
        if let (Some(old), Some(new)) = (
            record.get("cost").and_then(Value::as_array),
            record.get("energy_cost_rate").and_then(Value::as_array),
        ) {
            let old: Vec<_> = old.iter().map(Value::as_f64).collect();
            let new: Vec<_> = new.iter().map(Value::as_f64).collect();
            if old != new {
                self.error(path, "cost and energy_cost_rate must agree in phase order");
            }
        }
    }

    fn walk(&mut self, value: &Value, path: &str) {
        match value {
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    self.walk(item, &format!("{path}[{i}]"));
                }
            }
            Value::Object(record) => {
                for (bus, map) in [
                    ("bus", "terminal_map"),
                    ("bus_from", "terminal_map_from"),
                    ("bus_to", "terminal_map_to"),
                ] {
                    self.map(value, bus, map, path);
                }
                let map = strings(record.get("terminal_map"));
                let from = strings(record.get("terminal_map_from"));
                let np = phase_count(&map, self.doc.get("terminal_conventions"));
                if record.contains_key("terminal_map_from")
                    && (path.starts_with("line.") || path.starts_with("switch."))
                {
                    self.line_conductors(value, record, &from, path);
                }
                if record.contains_key("terminal_map") {
                    self.matrices(record, map.len(), path);
                    if path.starts_with("voltage_source.") {
                        for key in ["v_magnitude", "v_angle"] {
                            self.dimension(value, key, &[map.len()], path);
                        }
                        for key in ["p_min", "p_max", "cost", "energy_cost_rate"] {
                            self.dimension(value, key, &[np], path);
                        }
                    }
                    if path.starts_with("load.") {
                        let n = match record.get("configuration").and_then(Value::as_str) {
                            Some("SINGLE_PHASE") => 1,
                            Some("WYE") => map.len().saturating_sub(1),
                            Some("DELTA") => map.len() * map.len().saturating_sub(1) / 2,
                            _ => np,
                        };
                        for key in [
                            "p_nom", "q_nom", "v_nom", "alpha_p", "alpha_i", "alpha_z", "beta_p",
                            "beta_i", "beta_z", "gamma_p", "gamma_q",
                        ] {
                            self.dimension(value, key, &[n], path);
                        }
                    }
                    if path.starts_with("generator.") || path.starts_with("ibr.") {
                        for key in [
                            "p_min",
                            "p_max",
                            "q_min",
                            "q_max",
                            "s_max",
                            "cost",
                            "energy_cost_rate",
                        ] {
                            self.dimension(value, key, &[np], path);
                        }
                        self.dimension(value, "i_max", &[np, map.len()], path);
                    }
                }
                self.cost_aliases(record, path);
                self.constraints(record, path);
                self.no_load_shunt(record, path);
                self.tap_bounds(value, record, path);
                self.references_and_profiles(value, record, path);
                for (key, item) in record {
                    if !matches!(key.as_str(), "meta" | "extras" | "provenance") {
                        self.walk(item, &format!("{path}.{key}"));
                    }
                }
            }
            _ => {}
        }
    }
}

fn numeric_value(value: &Value) -> bool {
    value.is_number()
        || value
            .as_array()
            .is_some_and(|values| !values.is_empty() && values.iter().all(numeric_value))
}
