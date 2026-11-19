use super::{
    BTreeMap, BTreeSet, BalancedNetwork, Diagnostics, Ids, Result, Value, error, integer, json,
    metadata, optional, power_scale, push, rows, string, uid,
};
use crate::network::Area;

type AreaTables = (Vec<Area>, BTreeMap<i64, usize>, BTreeMap<i64, usize>);

pub(super) fn read(root: &Value) -> Result<AreaTables> {
    let mut areas = Vec::new();
    let mut mappings = [BTreeMap::new(), BTreeMap::new()];
    for (index, kind) in ["Area", "LoadZone"].into_iter().enumerate() {
        for row in rows(root, kind)? {
            let id = integer(row, "id")?;
            let mut area =
                Area::new(usize::try_from(id).map_err(|_| error("negative area or zone ID"))?);
            area.name = Some(string(row, "name")?.into());
            area.uid = uid(kind, row)?;
            area.net_interchange =
                optional(row, "scheduled_net_interchange", 0.0)? * power_scale(row)?;
            metadata::restore(
                root,
                row,
                &mut area,
                &[
                    "number",
                    "uid",
                    "name",
                    "slack_bus",
                    "tolerance",
                    "area_type",
                ],
            )?;
            mappings[index].insert(id, area.number);
            if index == 0 && super::metadata::ext(root, id)["implicit"].as_bool() != Some(true) {
                areas.push(area);
            }
        }
    }
    let [areas_by_id, zones_by_id] = mappings;
    Ok((areas, areas_by_id, zones_by_id))
}

pub(super) fn write(
    net: &BalancedNetwork,
    doc: &mut Value,
    ids: &mut Ids,
    warnings: &mut Diagnostics,
    version: u8,
) -> Result<[BTreeMap<usize, i64>; 2]> {
    let mut mappings = [BTreeMap::new(), BTreeMap::new()];
    for (index, kind) in ["Area", "LoadZone"].into_iter().enumerate() {
        let mut numbers: BTreeSet<_> = net
            .buses()
            .iter()
            .map(|b| if index == 0 { b.area } else { b.zone })
            .collect();
        if index == 0 {
            numbers.extend(net.areas().iter().map(|a| a.number));
        }
        for number in numbers {
            let source = if index == 0 {
                net.areas().iter().find(|a| a.number == number)
            } else {
                None
            };
            let implicit = Area::new(number);
            let area = source.unwrap_or(&implicit);
            let original = ids
                .area_numbers
                .get(&(kind.into(), number))
                .map(|id| format!("sienna:{kind}:{id}"));
            let id = ids.get(area.uid.as_deref().or(original.as_deref()));
            mappings[index].insert(number, id);
            metadata::record(
                doc,
                id,
                area,
                &[
                    "number",
                    "uid",
                    "name",
                    "slack_bus",
                    "tolerance",
                    "area_type",
                ],
            )?;
            super::metadata::ext_mut(doc, id)["implicit"] = json!(source.is_none());
            let mut row = json!({"id":id,"name":area.name.clone().unwrap_or_else(||format!("{kind}_{number}")),"base_power":net.base_mva(),"power_units":"NATURAL_UNITS","peak_active_power":0.0,"peak_reactive_power":0.0});
            if index == 0 && version >= 3 {
                row["scheduled_net_interchange"] = json!(area.net_interchange);
            }
            if area.slack_bus.is_some()
                || area.tolerance != 0.0
                || (version < 3 && area.net_interchange != 0.0)
            {
                warnings.push(&super::codes::EMIT_SIENNA.field_dropped, format!("area {number}: slack bus and interchange tolerance require ext; 0.1/0.2 have no scheduled interchange field"));
            }
            push(doc, kind, row);
        }
    }
    Ok(mappings)
}
