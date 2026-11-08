use super::{
    BTreeMap, BalancedNetwork, Bus, BusId, Diagnostics, Ids, Result, Value, available, base, codes,
    error, integer, json, limits, minmax, number, power_scale, push, retained, rows, string, uid,
};
use crate::network::Hvdc;

pub(super) fn read_hvdc(
    root: &Value,
    arcs: &BTreeMap<i64, (BusId, BusId)>,
    buses: &BTreeMap<BusId, &Bus>,
    warnings: &mut Diagnostics,
) -> Result<Vec<Hvdc>> {
    let mut links = Vec::new();
    for row in rows(root, "TwoTerminalGenericHVDCLine")? {
        let (from, to) = *arcs
            .get(&integer(row, "arc")?)
            .ok_or_else(|| error("HVDC references missing Arc"))?;
        let scale = power_scale(row)?;
        let mut dc = Hvdc::new(from, to);
        dc.uid = uid("TwoTerminalGenericHVDCLine", row)?;
        dc.in_service = available(row)?;
        dc.pf = number(row, "active_power_flow")? * scale;
        let rating = number(row, "rating")? * scale;
        dc.pmin = -rating;
        dc.pmax = rating;
        if let Some(limits) = row.get("operational_flow_limit") {
            let forward_min = number(limits, "from_to_min")? * scale;
            let reverse_min = number(limits, "to_from_min")? * scale;
            let forward_max = number(limits, "from_to_max")? * scale;
            let reverse_max = number(limits, "to_from_max")? * scale;
            if forward_max > 0.0 && reverse_max > 0.0 && (forward_min != 0.0 || reverse_min != 0.0)
            {
                retained(
                    warnings,
                    "HVDC directional minimum flows are retained outside the continuous power interval",
                );
            }
            dc.pmin = if reverse_max > 0.0 {
                -reverse_max
            } else {
                forward_min
            };
            dc.pmax = if forward_max > 0.0 {
                forward_max
            } else {
                -reverse_min
            };
        }
        let (lo, hi) = limits(row, "reactive_power_limits_from", (0.0, 0.0))?;
        dc.qminf = lo * scale;
        dc.qmaxf = hi * scale;
        let (lo, hi) = limits(row, "reactive_power_limits_to", (0.0, 0.0))?;
        dc.qmint = lo * scale;
        dc.qmaxt = hi * scale;
        dc.vf = buses[&from].vm;
        dc.vt = buses[&to].vm;
        if let Some(loss) = row.get("loss") {
            let curve = &loss["value_curve"];
            let data = &curve["function_data"];
            if curve["curve_type"] != "INPUT_OUTPUT" || data["function_type"] != "LINEAR" {
                return Err(error(
                    "HVDC loss must be a linear input-output curve for this electrical mapping",
                ));
            }
            let basis = match string(loss, "power_units")? {
                "NATURAL_UNITS" => 1.0,
                "COMPONENT_BASE" => base(row)?,
                other => return Err(error(format!("unsupported HVDC loss power_units {other}"))),
            };
            dc.loss0 = number(data, "constant_term")? * basis;
            dc.loss1 = number(data, "proportional_term")?;
        }
        dc.pt = Hvdc::calc_delivered_power(dc.pf, dc.loss0, dc.loss1);
        super::metadata::restore(
            root,
            row,
            &mut dc,
            &["uid", "cost", "vf", "vt", "qf", "qt", "pt"],
        )?;
        links.push(dc);
    }
    Ok(links)
}

pub(super) fn write_hvdc(
    net: &BalancedNetwork,
    doc: &mut Value,
    ids: &mut Ids,
    buses: &BTreeMap<BusId, i64>,
    version: u8,
    warnings: &mut Diagnostics,
) -> Result<()> {
    for dc in net.hvdc() {
        let id = ids.get(dc.uid.as_deref());
        super::metadata::record(doc, id, dc, &["uid", "cost", "vf", "vt", "qf", "qt", "pt"])?;
        let arc = ids.related("arc", id);
        push(
            doc,
            "Arc",
            json!({"id":arc,"from_id":buses[&dc.from],"to_id":buses[&dc.to]}),
        );
        let mut row = json!({"id":id,"name":format!("hvdc_{id}"),"available":dc.in_service,"arc":arc,"active_power_flow":dc.pf,"rating":dc.pmin.abs().max(dc.pmax.abs()),"reactive_power_limits_from":minmax(dc.qminf,dc.qmaxf),"reactive_power_limits_to":minmax(dc.qmint,dc.qmaxt),"base_power":net.base_mva(),"power_units":"NATURAL_UNITS","loss":{"power_units":"NATURAL_UNITS","value_curve":{"curve_type":"INPUT_OUTPUT","function_data":{"function_type":"LINEAR","constant_term":dc.loss0,"proportional_term":dc.loss1}}}});
        if version >= 2 {
            row["operational_flow_limit"] = json!({"from_to_min":dc.pmin.max(0.0),"from_to_max":dc.pmax.max(0.0),"to_from_min":(-dc.pmax).max(0.0),"to_from_max":(-dc.pmin).max(0.0)});
        } else if (dc.pmin + dc.pmax).abs() > 1e-12 {
            warnings.push(
                &codes::EMIT_SIENNA.field_dropped,
                format!("HVDC {id}: 0.1 cannot express directional active-power bounds"),
            );
        }
        if dc.qf != 0.0
            || dc.qt != 0.0
            || !dc.pt_matches_loss_model(1e-9)
            || dc.cost.is_some()
            || dc.converter1.is_some()
            || dc.converter2.is_some()
            || dc.nominal_voltage_kv.is_some()
            || dc.resistance_ohm.is_some()
        {
            warnings.push(&codes::EMIT_SIENNA.field_dropped,format!("HVDC {id}: generic line carries scheduled sending power and losses; terminal reactive dispatch, inconsistent received power, cost and converter details are not mapped"));
        }
        push(doc, "TwoTerminalGenericHVDCLine", row);
    }
    Ok(())
}
