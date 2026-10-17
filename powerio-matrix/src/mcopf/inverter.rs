//! Inverter capability and normalized smooth-control data. No solver expressions.
use super::{
    Configuration, Context, McOpfCoil, McOpfDevice, McOpfDeviceKind, McOpfDroop, Result, bounds,
    caps, costs, extras, finite, invalid, unsupported,
};
use powerio_dist::{
    ActivePowerReference, ActivePowerUnit, ControlVoltageReference as Ref, DistIbr, IbrTopology,
    IbrVoltageAggregation, ReactivePowerReference, ReactivePowerUnit, VoltVarControl,
    VoltWattControl,
};
pub(super) fn prepare(
    ctx: &Context<'_>,
    instance: &powerio_prob::McAcOpfInstance,
    ib: f64,
    sb: f64,
    vb: f64,
    weight: f64,
) -> Result<Vec<McOpfDevice>> {
    let mut out = Vec::new();
    for profile in ctx.net.control_profiles() {
        extras(&profile.name, &profile.extras, &[])?;
    }
    for (row, inv) in ctx.net.ibrs().iter().enumerate() {
        extras(
            &inv.name,
            &inv.extras,
            &["cost", "dc_link_coupled", "p_dc_min", "p_dc_max"],
        )?;
        let config = match inv.topology {
            IbrTopology::SinglePhase => Configuration::SinglePhase,
            IbrTopology::ThreeLeg => Configuration::Delta,
            IbrTopology::FourLeg => Configuration::Wye,
            _ => return Err(unsupported(&inv.name, "unknown inverter topology")),
        };
        let (ids, pairs) = ctx.coils(&inv.name, &inv.bus, &inv.terminal_map, config)?;
        let n = pairs.len();
        if inv.topology == IbrTopology::ThreeLeg && n != 3 {
            return Err(invalid(&inv.name, "THREE_LEG requires three phase coils"));
        }
        if inv.topology == IbrTopology::FourLeg && ids.len() != n + 1 {
            return Err(invalid(&inv.name, "FOUR_LEG requires explicit neutral"));
        }
        let smax = caps(&inv.name, Some(&inv.s_max), n, sb)?;
        let star = config != Configuration::Delta;
        let mut imax = caps(&inv.name, inv.i_max.as_ref(), n + usize::from(star), ib)?;
        if n == 1 && star {
            imax[0] = match (imax[0], imax[1]) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            imax[1] = None;
        }
        let pmin = bounds(&inv.name, inv.p_min.as_ref(), n, sb)?;
        let pmax = bounds(&inv.name, inv.p_max.as_ref(), n, sb)?;
        let qmin = bounds(&inv.name, inv.q_min.as_ref(), n, sb)?;
        let qmax = bounds(&inv.name, inv.q_max.as_ref(), n, sb)?;
        let cost = inverter_cost(inv, n, sb)?;
        let (profile, slope) = control_profile(ctx, inv, config)?;
        let vv = profile.and_then(|p| p.volt_var.as_ref());
        let vw = profile.and_then(|p| p.volt_watt.as_ref());
        let context = CurveContext {
            inv,
            pairs: &pairs,
            vb,
            sb,
        };
        let mut coils = Vec::new();
        for (k, &(positive, negative)) in pairs.iter().enumerate() {
            if pmin[k].zip(pmax[k]).is_some_and(|(l, h)| l > h)
                || qmin[k].zip(qmax[k]).is_some_and(|(l, h)| l > h)
            {
                return Err(invalid(&inv.name, "reversed inverter capability bounds"));
            }
            let (volt_var, volt_watt) = control_curves(&context, vv, vw, smax[k], pmax[k], k)?;
            let qbox = slope.is_none() && volt_var.is_none();
            coils.push(McOpfCoil {
                positive,
                negative,
                prescribed: None,
                load_law: None,
                reactive_slope: slope,
                volt_var,
                volt_watt,
                p_min: pmin[k],
                p_max: pmax[k],
                q_min: if qbox { qmin[k] } else { None },
                q_max: if qbox { qmax[k] } else { None },
                current_max: imax[k],
                apparent_max: smax[k],
                cost: cost[k].unwrap_or(0.0) * weight,
            });
        }
        let net_active_bounds = dc_link(inv, sb)?;
        let active = instance
            .constraints()
            .generator_capability
            .selects(&format!("ibr:{}", inv.name));
        if !active {
            for coil in &mut coils {
                coil.p_min = None;
                coil.p_max = None;
                coil.q_min = None;
                coil.q_max = None;
                coil.current_max = None;
                coil.apparent_max = None;
            }
        }
        out.push(McOpfDevice {
            identity: inv.name.clone(),
            kind: McOpfDeviceKind::Ibr,
            source_row: row,
            terminals: ids,
            coils,
            neutral_current_max: if star && active { imax[n] } else { None },
            net_active_bounds,
        });
    }
    Ok(out)
}

struct CurveContext<'a> {
    inv: &'a DistIbr,
    pairs: &'a super::CoilPairs,
    vb: f64,
    sb: f64,
}

fn curve(
    context: &CurveContext<'_>,
    reference: Option<Ref>,
    knots: &[f64],
    values: Vec<f64>,
    k: usize,
) -> Result<McOpfDroop> {
    let CurveContext { inv, pairs, vb, .. } = *context;
    let n = pairs.len();
    finite(&inv.name, knots)?;
    finite(&inv.name, &values)?;
    if knots.len() != values.len()
        || knots.len() < 2
        || knots.windows(2).any(|w| w[0] >= w[1])
        || knots.iter().any(|v| *v <= 0.0)
    {
        return Err(invalid(&inv.name, "invalid droop knots"));
    }
    let reference = reference.unwrap_or(Ref::PnPerPhase);
    let avg = match inv.voltage_aggregation {
        Some(IbrVoltageAggregation::Average) => true,
        Some(IbrVoltageAggregation::PerPhase) => false,
        None => matches!(
            reference,
            Ref::PnAveraged | Ref::PgAveraged | Ref::PpAveraged
        ),
        _ => return Err(unsupported(&inv.name, "unknown voltage aggregation")),
    };
    let monitors: Vec<_> = pairs
        .iter()
        .enumerate()
        .map(|(j, &(p, q))| match reference {
            Ref::PnPerPhase | Ref::PnAveraged => Ok((p, q)),
            Ref::PgPerPhase | Ref::PgAveraged => Ok((p, None)),
            Ref::PpPerPhase | Ref::PpAveraged if n > 1 => Ok((p, Some(pairs[(j + 1) % n].0))),
            _ => Err(invalid(&inv.name, "invalid controller voltage reference")),
        })
        .collect::<Result<_>>()?;
    let epsilon = 2e-3 * knots.iter().sum::<f64>() / (knots.len() as f64) / vb;
    Ok(McOpfDroop {
        monitors: if avg { monitors } else { vec![monitors[k]] },
        knots: knots.iter().map(|v| v / vb).collect(),
        values,
        epsilon,
    })
}

fn control_curves(
    context: &CurveContext<'_>,
    vv: Option<&VoltVarControl>,
    vw: Option<&VoltWattControl>,
    smax: Option<f64>,
    pmax: Option<f64>,
    k: usize,
) -> Result<(Option<McOpfDroop>, Option<McOpfDroop>)> {
    let CurveContext { inv, pairs, sb, .. } = *context;
    let n = pairs.len();
    let volt_var = vv
        .map(|c| {
            if c.breakpoints.len() != 4
                || c.q_limits.len() != 2
                || c.q_unit.is_some_and(|u| u != ReactivePowerUnit::VaFraction)
                || c.q_ref.is_some_and(|u| u != ReactivePowerReference::VarMax)
                || c.p_min_for_q.is_some()
                || c.p_min_for_q_max.is_some()
            {
                return Err(unsupported(&inv.name, "unsupported volt-var policy"));
            }
            let base = smax.ok_or_else(|| invalid(&inv.name, "volt-var requires finite rating"))?;
            curve(
                context,
                c.voltage_reference,
                &c.breakpoints,
                vec![base * c.q_limits[1], 0.0, 0.0, base * c.q_limits[0]],
                k,
            )
        })
        .transpose()?;
    let volt_watt = vw
        .map(|c| {
            if c.breakpoints.len() != 2
                || c.p_limits.len() != 2
                || c.p_unit.is_some_and(|u| u != ActivePowerUnit::VaFraction)
            {
                return Err(unsupported(&inv.name, "unsupported volt-watt policy"));
            }
            let base = match c.p_ref.unwrap_or(ActivePowerReference::SMax) {
                ActivePowerReference::SMax => smax,
                ActivePowerReference::PMax => pmax,
                ActivePowerReference::PAvailable => inv.p_avail.map(|v| v / (n as f64) / sb),
                _ => None,
            }
            .ok_or_else(|| invalid(&inv.name, "missing volt-watt reference power"))?;
            curve(
                context,
                c.voltage_reference,
                &c.breakpoints,
                vec![base * c.p_limits[1], base * c.p_limits[0]],
                k,
            )
        })
        .transpose()?;
    Ok((volt_var, volt_watt))
}

fn dc_link(inv: &DistIbr, sb: f64) -> Result<Option<[f64; 2]>> {
    let coupled = inv
        .extras
        .get("dc_link_coupled")
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| invalid(&inv.name, "dc_link_coupled must be boolean"))
        })
        .transpose()?
        .unwrap_or(false);
    let net_active_bounds = if coupled {
        let read = |key| -> Result<f64> {
            let value = inv
                .extras
                .get(key)
                .map_or(Some(0.0), serde_json::Value::as_f64)
                .ok_or_else(|| invalid(&inv.name, "invalid DC-link bound"))?
                / sb;
            finite(&inv.name, &[value])?;
            Ok(value)
        };
        let limits = [read("p_dc_min")?, read("p_dc_max")?];
        if limits[0] > limits[1] {
            return Err(invalid(&inv.name, "reversed DC-link bounds"));
        }
        Some(limits)
    } else {
        if inv.extras.contains_key("p_dc_min") || inv.extras.contains_key("p_dc_max") {
            return Err(invalid(&inv.name, "DC-link bounds require coupling"));
        }
        None
    };
    Ok(net_active_bounds)
}

fn control_profile<'a>(
    ctx: &'a Context<'_>,
    inv: &DistIbr,
    config: Configuration,
) -> Result<(Option<&'a powerio_dist::DistControlProfile>, Option<f64>)> {
    let profile = inv
        .control_profile
        .as_ref()
        .map(|name| {
            ctx.net
                .control_profiles()
                .iter()
                .find(|p| p.name == *name)
                .ok_or_else(|| invalid(&inv.name, "missing control profile"))
        })
        .transpose()?;
    let pf = profile.and_then(|p| p.power_factor.as_ref()).map(|p| p.pf);
    if pf.is_some_and(|v| !v.is_finite() || v.abs() > 1.0 || v == 0.0) {
        return Err(invalid(
            &inv.name,
            "power factor must satisfy 0 < |pf| <= 1",
        ));
    }
    let slope = pf.map(|v| -v.signum() * v.abs().acos().tan());
    let vv = profile.and_then(|p| p.volt_var.as_ref());
    let vw = profile.and_then(|p| p.volt_watt.as_ref());
    if (vv.is_some() || vw.is_some()) && (pf.is_some() || config == Configuration::Delta) {
        return Err(unsupported(
            &inv.name,
            "conflicting PF/droop or THREE_LEG droop controls",
        ));
    }
    Ok((profile, slope))
}

fn inverter_cost(inv: &DistIbr, n: usize, sb: f64) -> Result<Vec<Option<f64>>> {
    let cost: Option<Vec<f64>> = inv
        .extras
        .get("cost")
        .map(|v| {
            if let Some(x) = v.as_f64() {
                Ok(vec![x])
            } else {
                serde_json::from_value(v.clone())
            }
        })
        .transpose()
        .map_err(|_| invalid(&inv.name, "invalid inverter cost"))?;
    let cost = costs(&inv.name, cost.as_ref(), n, sb)?;
    Ok(cost)
}
