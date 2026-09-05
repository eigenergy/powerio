//! Cross-field checks over source data before conversion can normalize its shape.

use crate::{collect::Diagnostics, diagnostics::codes as C, model::DistBus};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub(super) fn report(doc: &Map<String, Value>, diagnostics: &mut Diagnostics) {
    let mut check = Check { doc, diagnostics };
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
            for key in ["v_min", "v_max", "vpn_min", "vpn_max"] {
                check.dimension(bus, key, &[phases], &path);
            }
            for key in ["vpp_min", "vpp_max"] {
                check.dimension(bus, key, &[phases * phases.saturating_sub(1) / 2], &path);
            }
            for (lo, hi) in [
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
}

impl Check<'_> {
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
        for root in ["tap", "tap_ratio"] {
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
        for (bus, map) in [
            ("dc_bus", "terminal_map"),
            ("dc_bus_from", "terminal_map_from"),
            ("dc_bus_to", "terminal_map_to"),
        ] {
            self.map_table(
                value,
                "dc_bus",
                bus,
                if record.contains_key("dc_terminal_map") {
                    "dc_terminal_map"
                } else {
                    map
                },
                path,
            );
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
                    self.dimension(value, "terminal_map_to", &[from.len()], path);
                    self.matrices(record, from.len(), path);
                    self.dimension(value, "i_max", &[from.len()], path);
                    self.dimension(
                        value,
                        "s_max",
                        &[phase_count(&from, self.doc.get("terminal_conventions"))],
                        path,
                    );
                    if let Some(id) = record.get("linecode").and_then(Value::as_str) {
                        if let Some(code) = self
                            .doc
                            .get("linecode")
                            .and_then(|c| c.get(id))
                            .and_then(Value::as_object)
                        {
                            self.matrices(code, from.len(), &format!("{path}.linecode"));
                        } else {
                            self.error(path, "referenced linecode does not exist");
                        }
                    }
                }
                if record.contains_key("terminal_map") {
                    self.matrices(record, map.len(), path);
                    if path.starts_with("voltage_source.") {
                        for key in ["v_magnitude", "v_angle"] {
                            self.dimension(value, key, &[map.len()], path);
                        }
                        for key in ["p_min", "p_max", "cost"] {
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
                        for key in ["p_min", "p_max", "q_min", "q_max", "s_max", "cost"] {
                            self.dimension(value, key, &[np], path);
                        }
                        self.dimension(value, "i_max", &[np, map.len()], path);
                    }
                }
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
