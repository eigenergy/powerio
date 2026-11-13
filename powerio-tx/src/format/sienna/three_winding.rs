use super::{
    BASE, BTreeMap, BTreeSet, BalancedNetwork, Branch, Bus, BusId, Ids, Result, Value, base, error,
    integer, json, metadata, number, optional, positive, push, rows, string, uid,
};
use crate::network::{Impedance, MagnetizingConnection, Transformer3W, Winding};

pub(super) fn read(
    root: &Value,
    buses: &mut Vec<Bus>,
    branches: &mut Vec<Branch>,
    bus_ids: &BTreeMap<i64, BusId>,
) -> Result<Vec<Transformer3W>> {
    let circuits = rows(root, "TransformerCircuit")?;
    let offset = rows(root, "Line")?.len();
    let circuit_index: BTreeMap<i64, usize> = circuits
        .iter()
        .enumerate()
        .filter_map(|(index, r)| r["id"].as_i64().map(|id| (id, index)))
        .collect();
    let bus_index: BTreeMap<BusId, usize> =
        buses.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let mut removed = BTreeSet::new();
    let mut stars = BTreeSet::new();
    let mut transformers = Vec::new();
    for row in rows(root, "ThreeWindingTransformer")? {
        let star = *bus_ids
            .get(&integer(row, "star_bus")?)
            .ok_or_else(|| error("three-winding transformer references missing star bus"))?;
        if !stars.insert(star) {
            return Err(error("three-winding transformers share a star bus"));
        }
        let mut legs = Vec::new();
        let mut source = Vec::new();
        for key in ["primary_circuit", "secondary_circuit", "tertiary_circuit"] {
            let id = integer(row, key)?;
            let index = *circuit_index
                .get(&id)
                .ok_or_else(|| error("missing three-winding circuit"))?;
            if !removed.insert(index + offset) {
                return Err(error("transformers share a circuit"));
            }
            let leg = &branches[index + offset];
            if leg.to != star {
                return Err(error("three-winding circuits must point to their star bus"));
            }
            legs.push(leg);
            source.push(&circuits[index]);
        }
        let windings = std::array::from_fn(|i| {
            let mut w = Winding::new(legs[i].from);
            w.tap = legs[i].tap;
            w.shift = legs[i].shift;
            w.rate_a = legs[i].rate_a;
            w.rate_b = legs[i].rate_b;
            w.rate_c = legs[i].rate_c;
            w.control.clone_from(&legs[i].control);
            w
        });
        let mut z = [Impedance::new(0.0, 0.0, BASE); 3];
        for (i, suffix) in ["12", "23", "31"].into_iter().enumerate() {
            let other = (i + 1) % 3;
            z[i] = Impedance::new(
                legs[i].r + legs[other].r,
                legs[i].x + legs[other].x,
                optional(row, &format!("base_power_{suffix}"), BASE)?,
            );
            positive(z[i].base_mva, "pair impedance base")?;
        }
        let mut t = Transformer3W::new(windings, z);
        t.uid = uid("ThreeWindingTransformer", row)?;
        t.name = Some(string(row, "name")?.into());
        let star_bus = bus_index
            .get(&star)
            .map(|&i| &buses[i])
            .ok_or_else(|| error("missing star bus"))?;
        t.star_vm = star_bus.vm;
        t.star_va = star_bus.va;
        t.set_winding_in_service(std::array::from_fn(|i| legs[i].in_service));
        t.in_service = legs.iter().any(|b| b.in_service);
        if let Some(y) = row.get("magnetizing_shunt") {
            let kv = buses[bus_index[&legs[0].from]].base_kv;
            let scale = magnetizing_scale(row, source[0], kv)?;
            t.mag_g = number(y, "real")? * scale;
            t.mag_b = number(y, "imag")? * scale;
        }
        t.magnetizing_connection = match row
            .get("shunt_location")
            .and_then(Value::as_str)
            .unwrap_or("PRIMARY")
        {
            "PRIMARY" => MagnetizingConnection::Winding1Terminal,
            "STAR" => MagnetizingConnection::StarPoint,
            _ => return Err(error("unsupported three-winding shunt placement")),
        };
        metadata::restore(root, row, &mut t, &["uid", "name", "extras"])?;
        transformers.push(t);
    }
    let mut index = 0;
    branches.retain(|_| {
        let keep = !removed.contains(&index);
        index += 1;
        keep
    });
    buses.retain(|b| !stars.contains(&b.id));
    Ok(transformers)
}

/// The factor from the stated magnetizing admittance units to per unit on the
/// network base at the primary bus, whose base voltage is `kv`.
fn magnetizing_scale(row: &Value, primary: &Value, kv: f64) -> Result<f64> {
    Ok(
        match row
            .get("admittance_units")
            .and_then(Value::as_str)
            .unwrap_or("COMPONENT_BASE")
        {
            "COMPONENT_BASE" => {
                // The primary circuit's stated voltage base rebases the admittance
                // onto the bus base, as the two-winding path does.
                let ratio = if primary.get("base_voltage_primary").is_some() {
                    positive(kv, "bus base_voltage")?
                        / positive(
                            number(primary, "base_voltage_primary")?,
                            "base_voltage_primary",
                        )?
                } else {
                    1.0
                };
                base(primary)? / BASE * ratio.powi(2)
            }
            "COMPONENT_MVAR" => 1.0 / BASE,
            "NATURAL_UNITS" => positive(kv, "base_voltage")?.powi(2) / BASE,
            _ => return Err(error("unsupported three-winding admittance units")),
        },
    )
}

pub(super) fn write(
    net: &BalancedNetwork,
    doc: &mut Value,
    ids: &mut Ids,
    buses: &BTreeMap<BusId, i64>,
    version: u8,
) -> Result<()> {
    let mut number = net.buses().iter().map(|b| b.id.0).max().unwrap_or(0);
    let bus_by_id: BTreeMap<BusId, &Bus> = net.buses().iter().map(|b| (b.id, b)).collect();
    for t in net.transformers_3w() {
        let id = ids.get(t.uid.as_deref());
        metadata::record(doc, id, t, &["uid", "name", "extras"])?;
        number = number
            .checked_add(1)
            .ok_or_else(|| error("no unused star bus number"))?;
        let star = ids.related("star_bus", id);
        push(
            doc,
            "ACBus",
            json!({"id":star,"number":number,"name":format!("star_{id}"),"available":true,"bustype":"PQ","base_voltage":1.0,"magnitude":t.star_vm,"angle":t.star_va.to_radians(),"voltage_limits":{"min":0.0,"max":2.0}}),
        );
        let mut circuits = [0; 3];
        for (i, (w, (r, x))) in t.windings.iter().zip(t.calc_star_impedances()).enumerate() {
            circuits[i] = ids.related(
                ["primary_circuit", "secondary_circuit", "tertiary_circuit"][i],
                id,
            );
            let arc = ids.related("arc", circuits[i]);
            push(
                doc,
                "Arc",
                json!({"id":arc,"from_id":buses[&w.bus],"to_id":star}),
            );
            let terminal_kv = bus_by_id[&w.bus].base_kv;
            let mut circuit = json!({"id":circuits[i],"available":t.in_service && t.winding_in_service()[i],"arc":arc,"r":r,"x":x,"tap":w.tap,"alpha":w.shift.to_radians(),"base_power":net.base_mva(),"power_units":"NATURAL_UNITS","parameter_units":"COMPONENT_BASE",});
            super::put_ratings(&mut circuit, [w.rate_a, w.rate_b, w.rate_c], net.base_mva());
            if terminal_kv > 0.0 {
                circuit["base_voltage_primary"] = json!(terminal_kv);
                circuit["base_voltage_secondary"] = json!(1.0);
            }
            if let Some(control) = &w.control {
                super::control::write(&mut circuit, control, version)?;
            }
            push(doc, "TransformerCircuit", circuit);
        }
        let mut row = json!({"id":id,"name":t.name.clone().unwrap_or_else(||format!("transformer_{id}")),"primary_circuit":circuits[0],"secondary_circuit":circuits[1],"tertiary_circuit":circuits[2],"star_bus":star,"admittance_units":"COMPONENT_BASE","magnetizing_shunt":{"real":t.mag_g,"imag":t.mag_b},"shunt_location":match t.magnetizing_connection {MagnetizingConnection::StarPoint=>"STAR",MagnetizingConnection::Winding1Terminal=>"PRIMARY"},"parameter_units":"COMPONENT_BASE"});
        for (i, suffix) in ["12", "23", "31"].into_iter().enumerate() {
            let basis = if t.z[i].base_mva > 0.0 {
                t.z[i].base_mva
            } else {
                net.base_mva()
            };
            row[format!("base_power_{suffix}")] = json!(basis);
            row[format!("r_{suffix}")] = json!(t.z[i].r * basis / net.base_mva());
            row[format!("x_{suffix}")] = json!(t.z[i].x * basis / net.base_mva());
        }
        push(doc, "ThreeWindingTransformer", row);
    }
    Ok(())
}
