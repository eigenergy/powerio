//! AutoTrans series/common coils represented by ordinary single-phase banks.
//!
//! Equations follow DSS C-API 87d85c2 AutoTrans.CalcY_Terminal. The native
//! leakage base is the high-side voltage, while the series coil sees HV-LV.
//! This first qualified profile is three-phase, fixed unity taps, two windings
//! or a delta tertiary. Other specifications remain explicit readiness gaps.
use super::{
    BusSpec, C, DistTransformer, DistWinding, DistWindingConn, Extras, RawDss, RawObject, Reader,
    UntypedObject, VarMap,
};
use crate::DistSwitch;

#[derive(Clone)]
struct Coil {
    bus: Option<BusSpec>,
    conn: char,
    kv: f64,
    kva: f64,
    r: f64,
}

fn coils(count: usize) -> Vec<Coil> {
    (0..count)
        .map(|i| Coil {
            bus: None,
            conn: if i == 0 { 's' } else { 'w' },
            kv: if i == 0 { 115.0 } else { 12.47 },
            kva: 1000.0,
            r: 0.2,
        })
        .collect()
}

#[allow(
    clippy::too_many_lines,
    reason = "keep ordered winding setters and their qualification checks together"
)]
#[allow(
    clippy::float_cmp,
    reason = "profile counts, unity taps and creation frequency require exact values"
)]
fn parameters(
    object: &RawObject,
    raw: &RawDss,
    vars: &VarMap,
) -> Result<(Vec<Coil>, Vec<f64>, Extras), String> {
    if raw
        .autotrans_copies
        .contains(&object.name.to_ascii_lowercase())
    {
        return Err("MakeLike state is not qualified".into());
    }
    if raw.commands.iter().any(|cmd| {
        matches!(cmd.verb.as_str(), "enable" | "disable" | "open" | "close")
            && crate::dss::lex::Scanner::new(&cmd.args, Some(vars))
                .next_param()
                .is_some_and(|p| p.value.text.to_ascii_lowercase().starts_with("autotrans."))
    }) {
        return Err("terminal/enable command history is not qualified".into());
    }
    if raw
        .options
        .iter()
        .any(|(k, v)| k == "defaultbasefrequency" && v.to_f64(Some(vars)) != Ok(60.0))
    {
        return Err("creation-frequency history is not qualified".into());
    }
    let mut winding = coils(2);
    let mut active = 0;
    let mut x = vec![10.0, 35.0, 30.0];
    let mut extras = Extras::new();
    extras.insert("basefreq".into(), 60.0.into());
    for p in &object.props {
        let key = p
            .name
            .as_deref()
            .ok_or("positional AutoTrans properties are not qualified")?;
        let number = || {
            p.value
                .to_f64(Some(vars))
                .map_err(|_| format!("invalid {key}"))
        };
        match key {
            "phases" if number()? == 3.0 => {}
            "windings" => {
                let n = number()?;
                if n != 2.0 && n != 3.0 {
                    return Err("only two/three winding AutoTrans is qualified".into());
                }
                winding = coils(if n == 2.0 { 2 } else { 3 });
                // Native ActiveWinding is not reset when the array changes.
                if active >= winding.len() {
                    return Err("invalid winding selector after resize".into());
                }
            }
            "wdg" => {
                let n = number()?;
                if n.fract() != 0.0 || n < 1.0 || n > winding.len() as f64 {
                    return Err("invalid winding selector".into());
                }
                active = n as usize - 1;
            }
            "bus" => winding[active].bus = Some(p.value.to_bus_spec()),
            "conn" => {
                winding[active].conn = match p.value.text.to_ascii_lowercase().as_str() {
                    "s" | "series" => 's',
                    "w" | "wye" | "y" | "ln" => 'w',
                    "d" | "delta" | "ll" => 'd',
                    _ => return Err("unknown winding connection".into()),
                };
            }
            "kv" => winding[active].kv = number()?,
            "kva" => {
                winding[active].kva = number()?;
                if active == 0 {
                    let rating = winding[0].kva;
                    for w in &mut winding {
                        w.kva = rating;
                    }
                } else if winding.len() == 2 {
                    winding[0].kva = winding[1].kva;
                }
            }
            "%r" => winding[active].r = number()?,
            "%loadloss" => {
                let r = number()? / 2.0;
                winding[0].r = r;
                winding[1].r = r;
            }
            "xhx" | "xht" | "xxt" => {
                let i = match key {
                    "xhx" => 0,
                    "xht" => 1,
                    _ => 2,
                };
                let value = number()?;
                if value == 0.0 {
                    return Err("zero scalar leakage specification is not qualified".into());
                }
                x[i] = value;
            }
            "%imag" | "%noloadloss" | "ppm_antifloat" => {
                let value = number()?;
                if !value.is_finite() || value < 0.0 {
                    return Err(format!("invalid {key}"));
                }
                extras.insert(key.into(), value.into());
            }
            "basefreq" if number()? == 60.0 => {}
            "tap" if number()? == 1.0 => {}
            "enabled" if p.value.to_bool() => {}
            _ => return Err(format!("AutoTrans property {key} is not qualified")),
        }
    }
    // Explicit scalar measurements after the final resize avoid interpreting
    // unqualified Xscarray/default-array edit-boundary interactions.
    let last_resize = object
        .props
        .iter()
        .rposition(|p| p.name.as_deref() == Some("windings"))
        .unwrap_or(0);
    let keys = if winding.len() == 3 {
        &["xhx", "xht", "xxt"][..]
    } else {
        &["xhx"][..]
    };
    if keys.iter().any(|key| {
        !object.props[last_resize..]
            .iter()
            .any(|p| p.name.as_deref() == Some(*key))
    }) {
        return Err(
            "requires explicit pairwise scalar leakage after the last winding resize".into(),
        );
    }
    if winding[0].conn != 's'
        || winding[1].conn != 'w'
        || winding.get(2).is_some_and(|w| w.conn != 'd')
    {
        return Err("requires series/common-wye coils and an optional delta tertiary".into());
    }
    for w in &winding {
        if !w.kv.is_finite()
            || w.kv <= 0.0
            || !w.kva.is_finite()
            || w.kva <= 0.0
            || !w.r.is_finite()
            || w.r < 0.0
        {
            return Err("invalid winding ratings/resistance".into());
        }
        let bus = w.bus.as_ref().ok_or("requires explicit winding buses")?;
        if bus.name.is_empty()
            || bus.nodes.len() > 3
            || bus.nodes.iter().any(|&n| n <= 0 || n == i32::MAX)
        {
            return Err(
                "only phase-node bus specifications with grounded common return are qualified"
                    .into(),
            );
        }
    }
    if winding[0].kv <= winding[1].kv || x.iter().any(|x| !x.is_finite()) {
        return Err("requires finite leakage and HV greater than LV".into());
    }
    let vs = (winding[0].kv - winding[1].kv) / 3.0_f64.sqrt();
    let vc = winding[1].kv / 3.0_f64.sqrt();
    let f = (1.0 + vc / vs).powi(2);
    if winding.len() == 3 {
        let extra = (f - 1.0) * (winding[0].r + winding[1].r) / 2.0;
        winding[0].r += extra;
        winding[1].r += extra;
        winding[2].r -= extra;
        x[1] = x[0] * (vs + vc) * vc / vs.powi(2) + x[1] * (vs + vc) / vs - x[2] * vc / vs;
    } else {
        winding[0].r *= f;
        winding[1].r *= f;
        x.truncate(1);
    }
    x[0] *= f;
    winding[0].kv = vs;
    winding[1].kv = vc;
    if winding.iter().any(|w| !w.r.is_finite() || w.r < 0.0) || x.iter().any(|x| !x.is_finite()) {
        return Err(
            "equivalent leakage parameters are outside the qualified passive profile".into(),
        );
    }
    Ok((winding, x, extras))
}

impl Reader<'_> {
    #[allow(
        clippy::too_many_lines,
        reason = "allocate each complete coil/contact family in one pass after validation"
    )]
    pub(super) fn autotrans_objects(&mut self, raw: &RawDss) {
        for object in raw.of_class("autotrans") {
            let result = if self.net.base_frequency() < 0.51 {
                Err("DC/GIC AutoTrans is not qualified".into())
            } else {
                parameters(object, raw, self.vars)
            };
            let (coils, x, extras) = match result {
                Ok(value) => value,
                Err(reason) => {
                    self.warn(
                        &C::READ_DSS_OBJECT_UNTYPED,
                        format!("AutoTrans.{}: {reason}; retained untyped", object.name),
                    );
                    self.net
                        .untyped_objects_mut()
                        .push(UntypedObject::from(object));
                    continue;
                }
            };
            let base = format!("_pio_autotrans_{}", object.name);
            let mut stem = base.clone();
            let mut suffix = 0;
            while self
                .buses
                .keys()
                .any(|name| name.contains(&stem.to_ascii_lowercase()))
                || raw.objects.iter().any(|o| {
                    o.name
                        .to_ascii_lowercase()
                        .contains(&stem.to_ascii_lowercase())
                        || o.props.iter().any(|p| {
                            p.value
                                .text
                                .to_ascii_lowercase()
                                .contains(&stem.to_ascii_lowercase())
                        })
                })
                || raw.buscoords.iter().any(|b| {
                    b.bus
                        .to_ascii_lowercase()
                        .contains(&stem.to_ascii_lowercase())
                })
            {
                suffix += 1;
                stem = format!("{base}_{suffix}");
            }
            let bus = format!("{stem}_series");
            let terms = self.terminals(
                &BusSpec {
                    name: bus.clone(),
                    nodes: (1..=6).collect(),
                },
                6,
                6,
                6,
            );
            let original: Vec<_> = coils
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let spec = w.bus.as_ref().expect("profile checks buses");
                    self.terminals(spec, 3, 6, if i == 1 { 6 } else { 3 })
                })
                .collect();
            for side in 0..2 {
                self.net.switches_mut().push(DistSwitch::new(
                    format!("{stem}_contact{side}"),
                    &coils[side].bus.as_ref().unwrap().name,
                    &bus,
                    original[side][..3].to_vec(),
                    terms[side * 3..side * 3 + 3].to_vec(),
                    false,
                ));
            }
            for phase in 0..3 {
                let mut windings = Vec::new();
                for (i, coil) in coils.iter().enumerate() {
                    let (coil_bus, nodes) = match i {
                        0 => (
                            bus.clone(),
                            vec![terms[phase].clone(), terms[phase + 3].clone()],
                        ),
                        1 => (
                            coil.bus.as_ref().unwrap().name.clone(),
                            vec![original[1][phase].clone(), original[1][phase + 3].clone()],
                        ),
                        _ => (
                            coil.bus.as_ref().unwrap().name.clone(),
                            vec![
                                original[2][phase].clone(),
                                original[2][(phase + 1) % 3].clone(),
                            ],
                        ),
                    };
                    // A one-phase delta is a two-ended coil. Wye would add the
                    // ordinary Transformer's extra floating-neutral admittance,
                    // which AutoTrans does not have.
                    let mut w = DistWinding::new(
                        coil_bus,
                        nodes,
                        DistWindingConn::Delta,
                        coil.kv * 1000.0,
                        coil.kva * 1000.0 / 3.0,
                    );
                    w.r_pct = coil.r;
                    windings.push(w);
                }
                let mut bank =
                    DistTransformer::new(format!("{stem}_phase{phase}"), windings, x.clone(), 1);
                bank.extras = extras.clone();
                if phase == 0 {
                    bank.extras.insert(
                        "dss_autotrans_contacts".into(),
                        serde_json::json!([format!("{stem}_contact0"), format!("{stem}_contact1")]),
                    );
                    bank.extras.insert(
                        "dss_autotrans_original".into(),
                        format!("autotrans.{}", object.name).into(),
                    );
                    bank.extras.insert(
                        "dss_autotrans_banks".into(),
                        serde_json::json!(
                            (0..3)
                                .map(|p| format!("{stem}_phase{p}"))
                                .collect::<Vec<_>>()
                        ),
                    );
                }
                self.net.transformers_mut().push(bank);
            }
            self.warn(&C::READ_DSS_RETAINED_SOURCE_ONLY, format!("AutoTrans.{}: static series/common windings use three single-phase transformer equivalents and ideal contacts; source property syntax remains in retained bytes", object.name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readiness::audit_electrical_readiness;

    const CASE: &str = "new circuit.c basekv=345
        new autotrans.t phases=3 windings=3 xhx=7.23 xht=24.45 xxt=28.45 %imag=.033 %noloadloss=.024
        ~ wdg=1 bus=high conn=s kv=345 kva=330000 %r=.0493
        ~ wdg=2 bus=low conn=w kv=161 kva=330000 %r=.0556
        ~ wdg=3 bus=tert conn=d kv=13.8 kva=72000 %r=1.4503
";

    fn read(text: &str) -> crate::MulticonductorNetwork {
        let raw = crate::dss::__parse_raw_with(text, "test.dss", &mut |_: &std::path::Path| {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "test"))
        });
        super::super::to_network_from_raw(&raw).0
    }

    #[test]
    fn ieee_voltage_bases_and_shared_winding_are_typed() {
        let net = read(CASE);
        assert!(audit_electrical_readiness(&net).is_ready());
        assert_eq!(net.transformers().len(), 3);
        assert_eq!(net.switches().len(), 2);
        let bank = &net.transformers()[0];
        assert!((bank.xsc_pct[0] - 25.417_968_75).abs() < 1e-12);
        assert!((bank.xsc_pct[1] - 32.811_718_75).abs() < 1e-12);
        assert!((bank.windings[2].r_pct - 1.318_355_468_75).abs() < 1e-12);
        assert_eq!(bank.windings[0].terminal_map, ["1", "4"]);
        assert_eq!(bank.windings[2].terminal_map, ["1", "2"]);
        assert!(
            bank.windings
                .iter()
                .all(|w| w.conn == DistWindingConn::Delta)
        );
        assert!((bank.windings[0].s_rating - 110_000_000.0).abs() < 1e-6);
        assert_eq!(net.switches()[1].bus_from, bank.windings[1].bus);
        assert_eq!(
            net.switches()[1].terminal_map_to[0],
            bank.windings[0].terminal_map[1]
        );
    }

    #[test]
    fn unqualified_histories_block_without_partial_equipment() {
        for tail in [
            "edit autotrans.t tap=1.01",
            "edit autotrans.t wdg=2 bus=low.1.2.3.4",
            "edit autotrans.t phases=1",
            "edit autotrans.t xscarray=(1 2 3)",
            "edit autotrans.t like=t",
            "open autotrans.t 1 1",
            "edit autotrans.t windings=3",
        ] {
            let net = read(&format!("{CASE}{tail}\n"));
            assert!(!audit_electrical_readiness(&net).is_ready(), "{tail}");
            assert!(net.transformers().is_empty(), "{tail}");
            assert!(net.switches().is_empty(), "{tail}");
        }
        let disabled = read(&format!("{CASE}edit autotrans.t enabled=no\n"));
        assert!(audit_electrical_readiness(&disabled).is_ready());
        let enabled = read(&format!(
            "{CASE}edit autotrans.t enabled=no\nenable autotrans.t\n"
        ));
        assert!(!audit_electrical_readiness(&enabled).is_ready());
    }

    #[test]
    fn generated_family_does_not_capture_declared_buses() {
        let net = read(&format!(
            "{CASE}new load.other bus1=_pio_autotrans_t_series kw=1\n"
        ));
        assert_eq!(
            net.transformers()[0].windings[0].bus,
            "_pio_autotrans_t_1_series"
        );
        assert_eq!(net.loads()[0].bus, "_pio_autotrans_t_series");
    }
}
