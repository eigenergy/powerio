//! Idle inverter losses are static voltage-dependent loads, not dispatch models.
use super::{
    Configuration, DistLoad, DistLoadVoltageModel, Props, RawDss, RawObject, Reader, UntypedObject,
    bus_spec,
};

impl Reader<'_> {
    pub(super) fn idle_storage_objects(&mut self, raw: &RawDss) {
        for object in raw.of_class("storage") {
            if object.get("enabled").is_some_and(|v| !v.to_bool()) {
                self.net
                    .untyped_objects_mut()
                    .push(UntypedObject::from(object));
                continue;
            }
            match self.idle_storage(object, raw) {
                Ok(load) => {
                    self.warn(&super::C::READ_DSS_RETAINED_SOURCE_ONLY,
                        format!("storage {}: typed idle AC demand and voltage law; dispatch, energy capacity and efficiency settings remain source-only", object.name));
                    self.net.loads_mut().push(load);
                }
                Err(error) => {
                    self.warn(
                        &super::C::READ_DSS_RETAINED_SOURCE_ONLY,
                        format!(
                            "storage {}: {error}; retained without a typed electrical equivalent",
                            object.name
                        ),
                    );
                    self.net
                        .untyped_objects_mut()
                        .push(UntypedObject::from(object));
                }
            }
        }
    }

    fn idle_storage(&mut self, object: &RawObject, raw: &RawDss) -> Result<DistLoad, String> {
        check_profile(object, raw)?;
        let props = Props::new(object);
        let number = |key, default| {
            props
                .get(key)
                .map_or(Ok(default), |v| v.to_f64(Some(self.vars)))
                .map_err(|_| format!("invalid {key}"))
        };
        let count = number("phases", 3.0)?;
        if !matches!(count, 1.0 | 2.0 | 3.0) {
            return Err("only one to three phases are qualified".into());
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let phases = count as usize;
        let conn = props.get("conn").map_or("wye", |v| v.text.as_str());
        let delta = match conn.to_ascii_lowercase().as_str() {
            "wye" | "y" | "ln" | "w" => false,
            "delta" | "d" | "ll" => true,
            _ => return Err("unrecognized connection".into()),
        };
        if phases == 1 && delta {
            return Err("single-phase delta voltage transitions require a distinct law".into());
        }
        let min = number("vminpu", 0.9)?;
        let max = number("vmaxpu", 1.1)?;
        let (kw, kvar) = idle_power(object, self.vars)?;
        let nominal = voltage_base(object, self.vars)? * if delta { 3.0_f64.sqrt() } else { 1.0 };
        let voltage_model = DistLoadVoltageModel::BoundedConstantPower {
            v_nom: vec![nominal; phases],
            v_min_pu: min,
            v_max_pu: max,
            current_limit_pu: props
                .get("limitcurrent")
                .is_some_and(super::Value::to_bool)
                .then_some(1.0 / min),
        };
        voltage_model.power_scale(0, nominal)?;
        let spec = bus_spec(props.get("bus1"), "");
        if spec.name.is_empty() || spec.nodes.iter().any(|&n| n < 0 || n == i32::MAX) {
            return Err("invalid terminal specification".into());
        }
        let nconds = if delta && phases == 3 {
            phases
        } else {
            phases + 1
        };
        let map = self.terminals(&spec, phases, nconds, nconds);
        let mut name = object.name.clone();
        let mut suffix = 0;
        while self
            .net
            .loads()
            .iter()
            .any(|load| load.name.eq_ignore_ascii_case(&name))
        {
            suffix += 1;
            name = format!("_pio_storage_{}_{suffix}", object.name);
        }
        Ok(DistLoad {
            name,
            bus: spec.name,
            terminal_map: map,
            configuration: if delta {
                Configuration::Delta
            } else if phases == 1 {
                Configuration::SinglePhase
            } else {
                Configuration::Wye
            },
            p_nom: vec![kw * 1000.0 / count; phases],
            q_nom: vec![kvar * 1000.0 / count; phases],
            voltage_model,
            extras: [(
                "dss_original_element".into(),
                format!("storage.{}", object.name).into(),
            )]
            .into(),
        })
    }
}

fn check_profile(object: &RawObject, raw: &RawDss) -> Result<(), String> {
    if raw
        .storage_copies
        .contains(&object.name.to_ascii_lowercase())
    {
        return Err("MakeLike inverter-state history is unresolved".into());
    }
    for property in &object.props {
        let key = property.name.as_deref().unwrap_or("");
        if !matches!(
            key,
            "phases"
                | "bus1"
                | "conn"
                | "kv"
                | "kva"
                | "kwrated"
                | "%idlingkw"
                | "state"
                | "pf"
                | "kvar"
                | "kvarmax"
                | "kvarmaxabs"
                | "vminpu"
                | "vmaxpu"
                | "limitcurrent"
                | "model"
                | "enabled"
                | "kwhrated"
                | "kwhstored"
                | "%stored"
                | "%charge"
                | "%discharge"
                | "%reserve"
                | "%effcharge"
                | "%effdischarge"
        ) {
            return Err(format!(
                "property {key} is outside the qualified idle profile"
            ));
        }
    }
    if object
        .get("state")
        .is_some_and(|v| !v.text.eq_ignore_ascii_case("idling"))
        || object
            .get("model")
            .is_some_and(|v| v.to_f64(Some(&raw.vars)) != Ok(1.0))
    {
        return Err("only explicit/default idling with voltage model 1 is qualified".into());
    }
    if raw.commands.iter().any(|command| {
        matches!(
            command.verb.as_str(),
            "enable" | "disable" | "open" | "close"
        ) && crate::dss::lex::Scanner::new(&command.args, Some(&raw.vars))
            .next_param()
            .is_some_and(|p| p.value.text.to_ascii_lowercase().starts_with("storage."))
    }) {
        return Err("terminal/enable command history is unresolved".into());
    }
    Ok(())
}

// Replay rating and reactive-mode setters. Clip/priority cases remain explicit
// blockers until independently qualified; the feeder's idle setpoint is inside
// every nameplate bound. State-of-charge and efficiencies do not change this
// snapshot's AC idling demand without an inverter efficiency curve.
fn idle_power(object: &RawObject, vars: &super::VarMap) -> Result<(f64, f64), String> {
    let (mut rated, mut kva, mut idle, mut pf, mut kvar) = (25.0, 25.0, 1.0, 1.0, 0.0);
    let (mut kva_set, mut q_set, mut negative_set, mut kvar_mode) = (false, false, false, false);
    let (mut q_max, mut q_abs) = (25.0, 25.0);
    for prop in &object.props {
        let key = prop.name.as_deref().unwrap_or("");
        if !matches!(
            key,
            "kwrated" | "kva" | "%idlingkw" | "pf" | "kvar" | "kvarmax" | "kvarmaxabs"
        ) {
            continue;
        }
        let value = prop
            .value
            .to_f64(Some(vars))
            .map_err(|_| format!("invalid {key}"))?;
        if !value.is_finite() {
            return Err(format!("nonfinite {key}"));
        }
        match key {
            "kwrated" => {
                rated = value;
                if !kva_set {
                    kva = value;
                }
            }
            "kva" => {
                kva = value;
                kva_set = true;
                if !q_set {
                    q_max = kva;
                    if !negative_set {
                        q_abs = kva;
                    }
                }
            }
            "%idlingkw" => idle = value,
            "pf" => {
                pf = value;
                kvar_mode = false;
            }
            "kvar" => {
                kvar = value;
                kvar_mode = true;
            }
            "kvarmax" => {
                q_max = value;
                q_set = true;
                if !negative_set {
                    q_abs = value.abs();
                }
            }
            _ => {
                q_abs = value;
                negative_set = true;
            }
        }
    }
    if rated <= 0.0
        || kva <= 0.0
        || idle <= 0.0
        || q_max < 0.0
        || q_abs < 0.0
        || pf == 0.0
        || pf.abs() > 1.0
    {
        return Err("invalid idle demand, rating or power factor".into());
    }
    let kw = rated * idle / 100.0;
    let demand_q = if kvar_mode {
        -kvar
    } else {
        kw * (1.0 / (pf * pf) - 1.0).sqrt() * pf.signum()
    };
    if kw.hypot(demand_q) >= kva || demand_q > q_abs || -demand_q > q_max {
        return Err(
            "idle setpoint reaches an inverter rating; clipping/priority is not qualified".into(),
        );
    }
    Ok((kw, demand_q))
}

// Changing phases alone does not reset native VBase. KV and Conn assignments
// do, using the phase count at that point. The constructor is exactly 7200 V.
fn voltage_base(object: &RawObject, vars: &super::VarMap) -> Result<f64, String> {
    let (mut phases, mut kv, mut base) = (3.0, 12.47, 7200.0);
    for prop in &object.props {
        match prop.name.as_deref() {
            Some("phases") => {
                phases = prop
                    .value
                    .to_f64(Some(vars))
                    .map_err(|_| "invalid phase history")?;
            }
            Some("kv") => {
                kv = prop
                    .value
                    .to_f64(Some(vars))
                    .map_err(|_| "invalid voltage history")?;
                base = kv * 1000.0
                    / if matches!(phases, 2.0 | 3.0) {
                        3.0_f64.sqrt()
                    } else {
                        1.0
                    };
            }
            Some("conn") => {
                base = kv * 1000.0
                    / if matches!(phases, 2.0 | 3.0) {
                        3.0_f64.sqrt()
                    } else {
                        1.0
                    }
            }
            _ => {}
        }
    }
    Ok(base)
}
