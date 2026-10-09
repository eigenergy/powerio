//! Explicit scalar series reactors lower to existing impedance branches.
use super::{BusSpec, DistLine, DistLineCode, Props, RawDss, RawObject, Reader, UntypedObject};

impl Reader<'_> {
    pub(super) fn series_reactor(
        &mut self,
        object: &RawObject,
        props: &Props<'_>,
        from: &BusSpec,
        to: &BusSpec,
        phases: usize,
        raw: &RawDss,
    ) -> Result<(), String> {
        check_profile(object, raw)?;
        if props.get("enabled").is_some_and(|v| !v.to_bool()) {
            self.net
                .untyped_objects_mut()
                .push(UntypedObject::from(object));
            return Ok(());
        }
        for bus in [from, to] {
            if bus.name.is_empty() || bus.nodes.iter().any(|&n| n < 0 || n == i32::MAX) {
                return Err("invalid series reactor terminal".into());
            }
        }
        let number = |key, default| {
            props
                .get(key)
                .map_or(Ok(default), |v| v.to_f64(Some(self.vars)))
                .map_err(|_| format!("invalid series reactor {key}"))
        };
        let r = number("r", 0.0)?;
        let base = number("basefreq", 60.0)?;
        let frequency = self.net.base_frequency();
        if !base.is_finite() || base <= 0.0 || frequency < 0.51 {
            return Err("series reactor frequency is outside the AC profile".into());
        }
        let x = number("x", 0.0)? / base * frequency;
        if !r.is_finite() || !x.is_finite() || (r == 0.0 && x == 0.0) {
            return Err("series reactor impedance must be finite and nonzero".into());
        }
        // Constructor RecalcElementData establishes the default ratings from
        // its 100 kvar, 12.47 kV, three-phase bank before explicit X is applied.
        let default_amps = default_rating(object, self.vars)?;
        let normal = number("normamps", default_amps)?;
        let emergency = number("emergamps", default_amps * 1.35)?;
        if [normal, emergency]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err("series reactor current rating is invalid".into());
        }
        let stem = format!("_pio_reactor_{}", object.name);
        let mut name = stem.clone();
        let mut suffix = 0;
        while self
            .net
            .lines()
            .iter()
            .any(|l| l.name.eq_ignore_ascii_case(&name))
            || self
                .net
                .line_codes()
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(&name))
        {
            suffix += 1;
            name = format!("{stem}_{suffix}");
        }
        let mut resistance = vec![vec![0.0; phases]; phases];
        let mut reactance = resistance.clone();
        for i in 0..phases {
            resistance[i][i] = r;
            reactance[i][i] = x;
        }
        let mut code = DistLineCode::new(&name, resistance, reactance);
        code.source = Some(format!(
            "OpenDSS Reactor.{} scalar AC snapshot",
            object.name
        ));
        let terminal_from = self.terminals(from, phases, phases, phases);
        let terminal_to = self.terminals(to, phases, phases, phases);
        let mut line = DistLine::new(
            &name,
            &from.name,
            &to.name,
            terminal_from,
            terminal_to,
            &name,
            1.0,
        );
        line.i_max = Some(vec![emergency; phases]);
        line.extras.insert("normamps".into(), normal.into());
        line.extras.insert(
            "dss_original_element".into(),
            format!("reactor.{}", object.name).into(),
        );
        line.extras.insert(
            "dss_flattened_properties".into(),
            serde_json::json!(
                object
                    .props
                    .iter()
                    .map(|p| (&p.name, &p.value.text))
                    .collect::<Vec<_>>()
            ),
        );
        self.net.lines_mut().push(line);
        self.net.line_codes_mut().push(code);
        Ok(())
    }
}

fn check_profile(object: &RawObject, raw: &RawDss) -> Result<(), String> {
    // Do not interpret competing native SpecType setters by last-value lookup.
    // This bounded profile deliberately requires explicit X and refuses all
    // alternative specifications, curves, damping and copy histories.
    for property in &object.props {
        let key = property.name.as_deref().unwrap_or("");
        if matches!(
            key,
            "like"
                | "kvar"
                | "kv"
                | "rmatrix"
                | "xmatrix"
                | "z"
                | "z0"
                | "z1"
                | "z2"
                | "lmh"
                | "rcurve"
                | "lcurve"
                | "rp"
        ) {
            return Err(format!("series reactor property {key} is not qualified"));
        }
        if key == "conn"
            && !matches!(
                property.value.text.to_ascii_lowercase().as_str(),
                "wye" | "y" | "ln"
            )
        {
            return Err("only wye-connected series reactor ports are qualified".into());
        }
    }
    if object.get("x").is_none() {
        return Err("series reactor requires explicit scalar X".into());
    }
    if raw.options.iter().any(|(key, value)| {
        key == "defaultbasefrequency" && value.to_f64(Some(&raw.vars)) != Ok(60.0)
    }) {
        return Err("series reactor creation-frequency history is not qualified".into());
    }
    for command in &raw.commands {
        if matches!(
            command.verb.as_str(),
            "enable" | "disable" | "open" | "close"
        ) {
            let mut scanner = crate::dss::lex::Scanner::new(&command.args, Some(&raw.vars));
            if scanner
                .next_param()
                .is_some_and(|p| p.value.text.to_ascii_lowercase().starts_with("reactor."))
            {
                return Err(
                    "series reactor terminal/enable command history is not qualified".into(),
                );
            }
        }
    }
    Ok(())
}

// Before the first explicit X, each native edit recalculates the default
// kvar bank and its current rating using that edit's phase count.
fn default_rating(object: &RawObject, vars: &super::VarMap) -> Result<f64, String> {
    let mut phases = 3.0;
    let mut amps = (100.0 / phases) / (12.47 / 3.0_f64.sqrt());
    let mut start = 0;
    for end in object.edit_bounds() {
        for property in &object.props[start..end] {
            match property.name.as_deref() {
                Some("x") => return Ok(amps),
                Some("phases") => {
                    phases = property
                        .value
                        .to_f64(Some(vars))
                        .map_err(|_| "invalid series reactor phase history")?;
                    if !phases.is_finite() || phases < 1.0 || phases.fract() != 0.0 {
                        return Err("invalid series reactor phase history".into());
                    }
                }
                _ => {}
            }
        }
        let kv = if (2.0..=3.0).contains(&phases) {
            12.47 / 3.0_f64.sqrt()
        } else {
            12.47
        };
        amps = (100.0 / phases) / kv;
        start = end;
    }
    Ok(amps)
}
