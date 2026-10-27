//! Same-bus ungrounded wye banks, including the IEEE NEV neutral conductor.
use super::{BusSpec, DistShunt, Props, RawDss, RawObject, Reader, UntypedObject};

impl Reader<'_> {
    /// Grounded and delta banks keep the existing path. A non-ground Bus2
    /// must either resolve here or remain an explicit readiness gap.
    pub(super) fn common_return_capacitor(&mut self, object: &RawObject, raw: &RawDss) -> bool {
        let props = Props::new(object);
        let from = super::bus_spec(props.get("bus1"), "");
        let Some(to) = props.get("bus2").map(super::Value::to_bus_spec) else {
            return false;
        };
        let phases = self.usize_or(&props, "phases", "capacitor", &object.name, 3);
        if super::same_bus_ground_return(&from, &to, phases) {
            return false;
        }
        match self.common_return_bank(object, raw, &props, from, &to, phases) {
            Ok(shunt) => self.net.shunts_mut().push(shunt),
            Err(error) => {
                self.warn(
                    &super::C::READ_DSS_OBJECT_UNTYPED,
                    format!("capacitor {}: {error}; kept untyped", object.name),
                );
                self.net
                    .untyped_objects_mut()
                    .push(UntypedObject::from(object));
            }
        }
        true
    }

    fn common_return_bank(
        &mut self,
        object: &RawObject,
        raw: &RawDss,
        props: &Props<'_>,
        from: BusSpec,
        to: &BusSpec,
        phases: usize,
    ) -> Result<DistShunt, String> {
        check_profile(object, raw)?;
        let nodes = super::filled_phase_nodes(&from, phases);
        let returns = super::filled_phase_nodes(to, phases);
        let Some(&neutral) = returns.first() else {
            return Err("a bank needs at least one phase".into());
        };
        if from.name.is_empty()
            || !from.name.eq_ignore_ascii_case(&to.name)
            || neutral <= 0
            || returns.iter().any(|&n| n != neutral)
            || nodes.iter().any(|&n| n <= 0 || n == neutral)
            || nodes
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != phases
            || from
                .nodes
                .iter()
                .chain(&to.nodes)
                .any(|&n| n < 0 || n == i32::MAX)
        {
            return Err("only distinct phases and one same-bus common return are qualified".into());
        }
        if props.get("conn").is_some_and(|v| {
            !v.text.to_ascii_lowercase().starts_with('w') && !v.text.eq_ignore_ascii_case("ln")
        }) {
            return Err("explicit common-return bank must be wye connected".into());
        }
        let number = |key, default| {
            props
                .get(key)
                .map_or(Ok(default), |v| v.to_f64(Some(self.vars)))
                .map_err(|_| format!("invalid {key}"))
        };
        let kv = number("kv", 12.47)?;
        let base = number("basefreq", 60.0)?;
        let kvar = props
            .get("kvar")
            .map_or(Ok(vec![1200.0]), |v| v.to_vector(Some(self.vars)))
            .map_err(|_| "invalid kvar")?;
        if kvar.len() != 1
            || !kvar[0].is_finite()
            || kvar[0] <= 0.0
            || !kv.is_finite()
            || kv <= 0.0
            || !base.is_finite()
            || base <= 0.0
            || self.net.base_frequency() < 0.51
        {
            return Err("bank needs one positive finite AC kvar/kV rating".into());
        }
        let voltage = if phases == 2 || phases == 3 {
            kv * 1e3 / 3f64.sqrt()
        } else {
            kv * 1e3
        };
        let b =
            kvar[0] * 1e3 / phases as f64 / voltage.powi(2) * (self.net.base_frequency() / base);
        if !b.is_finite() || b <= 0.0 {
            return Err("bank susceptance must be positive and finite".into());
        }
        let mut terminal_map = self.terminals(&from, phases, phases, phases);
        terminal_map.extend(self.terminals(
            &BusSpec {
                name: from.name.clone(),
                nodes: vec![neutral],
            },
            1,
            1,
            1,
        ));
        let n = phases + 1;
        let mut matrix = vec![vec![0.0; n]; n];
        for i in 0..phases {
            matrix[i][i] = b;
            matrix[i][phases] = -b;
            matrix[phases][i] = -b;
            matrix[phases][phases] += b;
        }
        let mut extras = super::extras_from_leftovers(props);
        extras.insert("conn".into(), "wye".into());
        extras.insert("phases".into(), phases.into());
        extras.insert("kv".into(), kv.into());
        Ok(DistShunt {
            name: object.name.clone(),
            bus: from.name,
            terminal_map,
            g: vec![vec![0.0; n]; n],
            b: matrix,
            extras,
        })
    }
}

fn check_profile(object: &RawObject, raw: &RawDss) -> Result<(), String> {
    if raw
        .capacitor_copies
        .contains(&object.name.to_ascii_lowercase())
    {
        return Err("copied capacitor connection/step state is not qualified".into());
    }
    for key in ["bus1", "bus2", "conn", "phases"] {
        if object
            .props
            .iter()
            .filter(|p| p.name.as_deref() == Some(key))
            .count()
            > 1
        {
            return Err(format!(
                "repeated {key} connection history is not qualified"
            ));
        }
    }
    if object.props.iter().any(|p| {
        matches!(
            p.name.as_deref(),
            Some("cmatrix" | "cuf" | "r" | "xl" | "harm" | "numsteps" | "states")
        )
    }) {
        return Err(
            "filtered, stepped or capacitance-specified common-return banks are not qualified"
                .into(),
        );
    }
    if object.get("enabled").is_some_and(|v| !v.to_bool()) {
        return Err("disabled bank retained without an energized equivalent".into());
    }
    if object.get("basefreq").is_none()
        && raw
            .options
            .iter()
            .any(|(k, v)| k == "defaultbasefrequency" && v.to_f64(Some(&raw.vars)) != Ok(60.0))
    {
        return Err("bank creation-frequency history needs an explicit basefreq".into());
    }
    if raw.commands.iter().any(|cmd| {
        matches!(cmd.verb.as_str(), "enable" | "disable" | "open" | "close")
            && super::super::lex::Scanner::new(&cmd.args, Some(&raw.vars))
                .next_param()
                .is_some_and(|p| p.value.text.to_ascii_lowercase().starts_with("capacitor."))
    }) {
        return Err("capacitor terminal/enable commands are not qualified".into());
    }
    Ok(())
}
