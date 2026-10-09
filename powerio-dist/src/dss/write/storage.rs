//! Native static equivalent for bounded constant-power consumption.
use super::{Configuration, DistLoad, DistLoadVoltageModel, DssWriter, num};

pub(super) fn validate(load: &DistLoad) -> Result<(), String> {
    let DistLoadVoltageModel::BoundedConstantPower {
        v_min_pu,
        current_limit_pu,
        ..
    } = &load.voltage_model
    else {
        return Ok(());
    };
    load.voltage_model.power_scale(
        0,
        load.voltage_model
            .v_nom()
            .first()
            .copied()
            .unwrap_or(f64::NAN),
    )?;
    for phase in 0..load.p_nom.len() {
        load.voltage_model.power_scale(
            phase,
            load.voltage_model
                .v_nom()
                .get(phase)
                .copied()
                .unwrap_or(f64::NAN),
        )?;
    }
    let conductors = if load.configuration == Configuration::Delta && load.p_nom.len() == 3 {
        3
    } else {
        load.p_nom.len() + 1
    };
    if load.terminal_map.len() != conductors {
        return Err(format!(
            "load {}: bounded-power branches require complete conductor maps",
            load.name
        ));
    }
    if load.p_nom.is_empty()
        || load.p_nom.iter().any(|p| !p.is_finite() || *p <= 0.0)
        || load.q_nom.len() != load.p_nom.len()
        || load.q_nom.iter().any(|q| !q.is_finite())
        || load.p_nom.len() > 3
        || current_limit_pu.is_some_and(|limit| (limit * v_min_pu - 1.0).abs() > 1e-12)
    {
        return Err(format!(
            "load {}: native bounded-power equivalent requires one to three consuming branches and a current limit equal to 1/v_min_pu",
            load.name
        ));
    }
    Ok(())
}

impl DssWriter {
    pub(super) fn write_static_storage(&mut self, load: &DistLoad) {
        let DistLoadVoltageModel::BoundedConstantPower {
            v_nom,
            v_min_pu,
            v_max_pu,
            current_limit_pu,
        } = &load.voltage_model
        else {
            unreachable!()
        };
        let phases = load.p_nom.len();
        // Single branch uses wye even across two hot conductors: native
        // one-phase delta has a different out-of-band voltage law.
        let conn = if load.configuration == Configuration::Delta && phases > 1 {
            "delta"
        } else {
            "wye"
        };
        let kv = v_nom[0] / 1000.0
            * if conn == "wye" && phases > 1 {
                3.0_f64.sqrt()
            } else {
                1.0
            };
        let kw = load.p_nom.iter().sum::<f64>() / 1000.0;
        let kvar = load.q_nom.iter().sum::<f64>() / 1000.0;
        let rating = 2.0 * kw.hypot(kvar);
        self.check_name("storage", &load.name);
        for key in load
            .extras
            .keys()
            .filter(|key| !matches!(key.as_str(), "dss_original_element" | "phases"))
        {
            self.warn(&super::C::EMIT_DSS_EXTRAS_DROPPED, format!("load {}: extra {key} is not part of the typed bounded-power equivalent; omitted", load.name));
        }
        let bus = self.bus_ref(&load.bus, &load.terminal_map);
        self.line_out(&format!(
            "New Storage.{} phases={phases} bus1={} conn={conn} kv={} kwrated={} kva={} %idlingkw=100 state=idling kvar={} vminpu={} vmaxpu={} limitcurrent={} model=1",
            load.name, bus, num(kv), num(kw), num(rating), num(-kvar), num(*v_min_pu), num(*v_max_pu), if current_limit_pu.is_some() { "yes" } else { "no" }
        ));
    }
}
