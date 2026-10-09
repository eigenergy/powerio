//! Fixed inverter power is a signed voltage-dependent load, without dispatch execution.
use super::{
    Configuration, DistLoad, DistLoadVoltageModel, Props, RawDss, RawObject, Reader, UntypedObject,
    bus_spec,
};

impl Reader<'_> {
    pub(super) fn static_storage_objects(&mut self, raw: &RawDss) {
        for object in raw.of_class("storage") {
            if object.get("enabled").is_some_and(|v| !v.to_bool()) {
                self.net
                    .untyped_objects_mut()
                    .push(UntypedObject::from(object));
                continue;
            }
            match self.static_storage(object, raw) {
                Ok(load) => {
                    self.warn(&super::C::READ_DSS_RETAINED_SOURCE_ONLY,
                        format!("storage {}: typed fixed AC power and voltage law; dispatch, energy capacity and efficiency settings remain source-only", object.name));
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

    fn static_storage(&mut self, object: &RawObject, raw: &RawDss) -> Result<DistLoad, String> {
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
        let (kw, kvar) = static_power(object, self.vars)?;
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
                | "kw"
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
                "property {key} is outside the qualified static profile"
            ));
        }
    }
    if object
        .get("model")
        .is_some_and(|v| v.to_f64(Some(&raw.vars)) != Ok(1.0))
    {
        return Err("only voltage model 1 is qualified".into());
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

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idling,
    Charging,
    Discharging,
}

// Replay reactive-mode setters independently of the active-power state.
// Clipping/priority remains an explicit blocker.
fn static_power(object: &RawObject, vars: &super::VarMap) -> Result<(f64, f64), String> {
    let (state, kw) = active_power(object, vars)?;
    let (mut kva, mut pf, mut kvar) = (25.0, 1.0, 0.0);
    let (mut kva_set, mut q_set, mut negative_set, mut kvar_mode) = (false, false, false, false);
    let (mut q_max, mut q_abs) = (25.0, 25.0);
    for prop in &object.props {
        let key = prop.name.as_deref().unwrap_or("");
        if !matches!(
            key,
            "kwrated" | "kva" | "pf" | "kvar" | "kvarmax" | "kvarmaxabs"
        ) {
            continue;
        }
        let value = number(&prop.value, vars, key)?;
        match key {
            "kwrated" => {
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
    if kva <= 0.0 || q_max < 0.0 || q_abs < 0.0 || pf == 0.0 || pf.abs() > 1.0 {
        return Err("invalid inverter rating or power factor".into());
    }
    let demand_q = if kvar_mode {
        -kvar
    } else {
        kw * (1.0 / (pf * pf) - 1.0).sqrt() * pf.signum()
    };
    // Native non-idle PF limiting tests both Q limits before choosing a sign.
    let pf_limit =
        state != State::Idling && !kvar_mode && (demand_q.abs() > q_max || demand_q.abs() > q_abs);
    if !kw.is_finite()
        || !demand_q.is_finite()
        || kw.hypot(demand_q) > kva
        || demand_q > q_abs
        || -demand_q > q_max
        || pf_limit
    {
        return Err(
            "static setpoint exceeds an inverter rating; clipping/priority is not qualified".into(),
        );
    }
    Ok((kw, demand_q))
}

fn number(value: &super::Value, vars: &super::VarMap, key: &str) -> Result<f64, String> {
    let value = value
        .to_f64(Some(vars))
        .map_err(|_| format!("invalid {key}"))?;
    if !value.is_finite() {
        return Err(format!("nonfinite {key}"));
    }
    Ok(value)
}

fn state(value: &str) -> Result<State, String> {
    let text = value.to_ascii_lowercase();
    [
        ("idling", State::Idling),
        ("charging", State::Charging),
        ("discharging", State::Discharging),
    ]
    .into_iter()
    .find_map(|(name, state)| (!text.is_empty() && name.starts_with(&text)).then_some(state))
    .ok_or_else(|| "unrecognized storage state".into())
}

// Replay setters and native EndEdit SOC guards. This is a fixed snapshot, not
// time integration: efficiencies without an inverter curve affect DC energy,
// not the AC setpoint.
fn active_power(object: &RawObject, vars: &super::VarMap) -> Result<(State, f64), String> {
    let (mut rated, mut idle) = (25.0, 1.0);
    let (mut capacity, mut stored, mut reserve_percent) = (50.0, 50.0, 20.0);
    // The pinned native constructor initializes kWhReserve before pctReserve;
    // only an explicit kWhRated or %Reserve setter makes it 20% of capacity.
    let mut reserve = 0.0;
    let (mut charge, mut discharge) = (100.0, 100.0);
    let mut mode = State::Idling;
    let mut start = 0;
    for end in object.edit_bounds() {
        for prop in &object.props[start..end] {
            let key = prop.name.as_deref().unwrap_or("");
            if key == "state" {
                mode = state(&prop.value.text)?;
                continue;
            }
            if !matches!(
                key,
                "kwrated"
                    | "%idlingkw"
                    | "kwhrated"
                    | "kwhstored"
                    | "%stored"
                    | "%reserve"
                    | "%charge"
                    | "%discharge"
                    | "kw"
            ) {
                continue;
            }
            let value = number(&prop.value, vars, key)?;
            match key {
                "kwrated" => {
                    if value <= 0.0 {
                        return Err("invalid kW rating".into());
                    }
                    rated = value;
                }
                "%idlingkw" => idle = value,
                "kwhrated" => {
                    capacity = value;
                    stored = value;
                    reserve = capacity * reserve_percent / 100.0;
                }
                "kwhstored" => stored = value,
                "%stored" => stored = value * 0.01 * capacity,
                "%reserve" => {
                    reserve_percent = value;
                    reserve = capacity * value / 100.0;
                }
                "%charge" => charge = value,
                "%discharge" => discharge = value,
                "kw" => {
                    if value > 0.0 {
                        mode = State::Discharging;
                        discharge = value / rated * 100.0;
                    } else if value < 0.0 {
                        mode = State::Charging;
                        charge = value.abs() / rated * 100.0;
                    } else {
                        mode = State::Idling;
                    }
                }
                _ => unreachable!(),
            }
        }
        if (mode == State::Charging && stored >= capacity)
            || (mode == State::Discharging && stored <= reserve)
        {
            mode = State::Idling;
        }
        start = end;
    }
    if idle < 0.0
        || capacity <= 0.0
        || stored < 0.0
        || reserve_percent < 0.0
        || charge < 0.0
        || discharge < 0.0
    {
        return Err("invalid static setpoint or state of charge".into());
    }
    let kw = match mode {
        State::Idling => (rated * idle / 100.0).min(rated),
        State::Charging => (rated * charge / 100.0).min(rated),
        State::Discharging => -(rated * discharge / 100.0).min(rated),
    };
    Ok((mode, kw))
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
