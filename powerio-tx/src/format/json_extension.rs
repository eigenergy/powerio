//! PowerIO typed fields that a PowerModels or egret JSON row has no column
//! for.
//!
//! Both tools keep keys they do not know, so the writers state each such
//! field under its PowerIO IR name and in IR units, and the readers restore
//! it. A key whose value does not have the PowerIO shape stays in the row's
//! extras, as any other unknown key does, so a document another tool wrote
//! with a key of the same name still reads.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::network::{
    Area, BalancedNetwork, Branch, BranchRatingSet, Bus, Extras, Generator, GeneratorEnergySource,
    Hvdc, Shunt, Storage,
};

/// Preserve source-assigned identities and untyped fields without replacing
/// fields already written in the target's native units. Callers filter out
/// any absent native fields that must not be filled from foreign extras.
pub(super) fn carry_row<'a>(
    mut row: Value,
    uid: Option<&str>,
    extras: impl IntoIterator<Item = (&'a String, &'a Value)>,
    net: &BalancedNetwork,
) -> Value {
    if let Value::Object(map) = &mut row {
        if let Some(uid) = uid.filter(|uid| !net.uid_is_generated(Some(uid))) {
            map.entry("uid")
                .or_insert_with(|| Value::String(uid.to_owned()));
        }
        for (key, value) in extras {
            map.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    row
}

/// Area attributes shared by both JSON extensions, with powers in MW.
pub(super) fn write_area(area: &Area, row: &mut Map<String, Value>) {
    if area.net_interchange != 0.0 {
        row.insert(
            "net_interchange_mw".into(),
            super::jnum(area.net_interchange),
        );
    }
    if area.tolerance != 0.0 {
        row.insert("tolerance_mw".into(), super::jnum(area.tolerance));
    }
    put(row, "name", area.name.as_ref());
    put(row, "uid", area.uid.as_ref());
    put(row, "area_type", area.area_type.as_ref());
}

/// State `value` under `key`. A `None` inside it is left out rather than
/// written as `null`, which the writers report as a non-finite number; a
/// missing optional field reads back as `None`.
fn put<T: Serialize>(row: &mut Map<String, Value>, key: &str, value: Option<&T>) {
    if let Some(value) = value {
        let mut json = serde_json::to_value(value).expect("PowerIO typed fields are plain data");
        // The shared typed serializer distinguishes an absent Option from a
        // non-finite float. Keep the latter's JSON null so finish() reports it.
        let typed = value
            .serialize(powerio_core::__implementation::nonfinite::NonFiniteSer(
                serde_json::value::Serializer,
            ))
            .expect("PowerIO typed fields are plain data");
        drop_nulls(&mut json, &typed);
        row.insert(key.to_owned(), json);
    }
}

fn drop_nulls(value: &mut Value, typed: &Value) {
    match (value, typed) {
        (Value::Object(map), Value::Object(types)) => {
            map.retain(|key, field| {
                !field.is_null() || types.get(key).is_some_and(|value| !value.is_null())
            });
            for (key, field) in map {
                if let Some(typed) = types.get(key) {
                    drop_nulls(field, typed);
                }
            }
        }
        (Value::Array(items), Value::Array(types)) => {
            for (field, typed) in items.iter_mut().zip(types) {
                drop_nulls(field, typed);
            }
        }
        _ => {}
    }
}

/// The value under `key` when it has the PowerIO shape.
fn get<T: DeserializeOwned>(extras: &Extras, key: &str) -> Option<T> {
    extras.get(key).and_then(|value| T::deserialize(value).ok())
}

/// Remove and return the value under `key` when it has the PowerIO shape;
/// any other value stays an extra.
fn take<T: DeserializeOwned>(extras: &mut Extras, key: &str) -> Option<T> {
    let value = get(extras, key)?;
    extras.remove(key);
    Some(value)
}

/// The source-assigned identity PowerIO writes as a row's `uid`. A `uid` of
/// another type stays an extra.
pub(super) fn read_uid(extras: &mut Extras) -> Option<String> {
    take(extras, "uid")
}

/// A document-level PowerIO key: `None` when absent or null, and the decoding
/// error when the value has another shape, which the reader reports or
/// ignores rather than refusing the document.
pub(super) fn read_document_key<T: DeserializeOwned>(
    value: Option<Value>,
) -> Result<Option<T>, serde_json::Error> {
    value
        .filter(|value| !value.is_null())
        .map(T::deserialize)
        .transpose()
}

pub(super) fn write_bus(bus: &Bus, row: &mut Map<String, Value>) {
    put(row, "location", bus.location.as_ref());
}

pub(super) fn read_bus(bus: &mut Bus) {
    bus.location = take(&mut bus.extras, "location").or(bus.location.take());
}

pub(super) fn write_branch(branch: &Branch, row: &mut Map<String, Value>) {
    if !row.contains_key("name") {
        put(row, "name", branch.name.as_ref());
    }
    put(row, "control", branch.control.as_ref());
    put(row, "route", branch.route.as_ref());
    if !branch.rating_sets.is_empty() {
        put(row, "rating_sets", Some(&branch.rating_sets));
    }
}

pub(super) fn read_branch(branch: &mut Branch) {
    branch.name = take(&mut branch.extras, "name").or(branch.name.take());
    branch.control = take(&mut branch.extras, "control").or(branch.control.take());
    branch.route = take(&mut branch.extras, "route").or(branch.route.take());
    // Rating sets beyond rate_a, rate_b, and rate_c, each `{name, rate_mva}`
    // in MVA; a set without a rate keeps the whole value an extra.
    if let Some(sets) = take::<Vec<BranchRatingSet>>(&mut branch.extras, "rating_sets") {
        branch.rating_sets = sets;
    }
}

pub(super) fn write_generator(generator: &Generator, row: &mut Map<String, Value>) {
    if generator.energy_source != GeneratorEnergySource::default() {
        put(row, "energy_source", Some(&generator.energy_source));
    }
    if !generator.voltage_regulation_on {
        put(row, "voltage_regulation_on", Some(&false));
    }
    put(
        row,
        "regulating_terminal",
        generator.regulating_terminal.as_ref(),
    );
    put(row, "regulated_bus", generator.regulated_bus.as_ref());
    put(
        row,
        "active_power_control",
        generator.active_power_control.as_ref(),
    );
}

/// A generator keeps no extras, so its row's extras are only read.
pub(super) fn read_generator(generator: &mut Generator, extras: &Extras) {
    if let Some(uid) = get(extras, "uid") {
        generator.uid = Some(uid);
    }
    if let Some(source) = get(extras, "energy_source") {
        generator.energy_source = source;
    }
    if let Some(on) = get(extras, "voltage_regulation_on") {
        generator.voltage_regulation_on = on;
    }
    if let Some(terminal) = get(extras, "regulating_terminal") {
        generator.regulating_terminal = Some(terminal);
    }
    if let Some(bus) = get(extras, "regulated_bus") {
        generator.regulated_bus = Some(bus);
    }
    if let Some(control) = get(extras, "active_power_control") {
        generator.active_power_control = Some(control);
    }
}

pub(super) fn write_shunt(shunt: &Shunt, row: &mut Map<String, Value>) {
    put(row, "section_count", shunt.section_count.as_ref());
    put(row, "control", shunt.control.as_ref());
}

pub(super) fn read_shunt(shunt: &mut Shunt) {
    shunt.section_count = take(&mut shunt.extras, "section_count").or(shunt.section_count);
    shunt.control = take(&mut shunt.extras, "control").or(shunt.control.take());
}

pub(super) fn write_hvdc(hvdc: &Hvdc, row: &mut Map<String, Value>) {
    put(row, "resistance_ohm", hvdc.resistance_ohm.as_ref());
    put(row, "nominal_voltage_kv", hvdc.nominal_voltage_kv.as_ref());
    put(row, "converters_mode", hvdc.converters_mode.as_ref());
    put(row, "converter1", hvdc.converter1.as_ref());
    put(row, "converter2", hvdc.converter2.as_ref());
}

pub(super) fn read_hvdc(hvdc: &mut Hvdc) {
    hvdc.resistance_ohm = take(&mut hvdc.extras, "resistance_ohm").or(hvdc.resistance_ohm);
    hvdc.nominal_voltage_kv =
        take(&mut hvdc.extras, "nominal_voltage_kv").or(hvdc.nominal_voltage_kv);
    hvdc.converters_mode = take(&mut hvdc.extras, "converters_mode").or(hvdc.converters_mode);
    hvdc.converter1 = take(&mut hvdc.extras, "converter1").or(hvdc.converter1.take());
    hvdc.converter2 = take(&mut hvdc.extras, "converter2").or(hvdc.converter2.take());
}

pub(super) fn write_storage(storage: &Storage, row: &mut Map<String, Value>) {
    put(
        row,
        "active_power_control",
        storage.active_power_control.as_ref(),
    );
}

pub(super) fn read_storage(storage: &mut Storage) {
    storage.active_power_control =
        take(&mut storage.extras, "active_power_control").or(storage.active_power_control.take());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::Location;
    use crate::network::{
        ActivePowerControl, BalancedNetwork, BusId, HvdcConverter, HvdcConverterKind,
        HvdcConvertersMode, ShuntBlock, SwitchedShuntControl, SwitchedShuntMode, TerminalReference,
        TransformerControl, TransformerControlMode,
    };
    use powerio_core::ComponentId;

    fn network_with_typed_fields() -> BalancedNetwork {
        let doc = r#"{"baseMVA":100.0,"per_unit":true,
            "bus":{"1":{"bus_i":1,"bus_type":3,"vm":1.0,"va":0.0,"base_kv":345.0},
                   "2":{"bus_i":2,"bus_type":1,"vm":1.0,"va":0.0,"base_kv":345.0}},
            "branch":{"1":{"f_bus":1,"t_bus":2,"br_r":0.01,"br_x":0.1,"rate_a":2.5,"tap":1.02}},
            "gen":{"1":{"gen_bus":1,"pg":0.5,"pmax":1.0,"pmin":0.0,"qmax":1.0,"qmin":-1.0}},
            "shunt":{"1":{"shunt_bus":2,"gs":0.0,"bs":0.2}},
            "dcline":{"1":{"f_bus":1,"t_bus":2,"pf":0.1,"pt":-0.1,"pmaxf":1.0,"pminf":-1.0,
                           "qmaxf":1.0,"qminf":-1.0,"qmaxt":1.0,"qmint":-1.0}},
            "storage":{"1":{"storage_bus":2,"ps":0.0,"qs":0.0,"energy":1.0,"energy_rating":2.0,
                            "qmax":1.0,"qmin":-1.0}}}"#;
        let mut net = crate::parse_str(doc, "powermodels-json").unwrap().network;
        let terminal = |kind: &str, id: &str| TerminalReference {
            equipment: ComponentId::new(kind, id).unwrap(),
            terminal: 1,
        };
        net.buses_mut()[0].location = Some(Location {
            x: 10.4,
            y: 63.4,
            kind: None,
        });
        let branch = &mut net.branches_mut()[0];
        branch.name = Some("north tie".into());
        branch.control = Some(TransformerControl::new(TransformerControlMode::Voltage));
        branch.route = Some(vec![Location {
            x: 10.5,
            y: 63.5,
            kind: None,
        }]);
        let generator = &mut net.generators_mut()[0];
        generator.energy_source = GeneratorEnergySource::Hydro;
        generator.voltage_regulation_on = false;
        generator.regulating_terminal = Some(terminal("generator", "g1"));
        generator.regulated_bus = Some(BusId(2));
        generator.active_power_control = Some(ActivePowerControl::new(true));
        let shunt = &mut net.shunts_mut()[0];
        shunt.section_count = Some(3);
        shunt.control = Some(SwitchedShuntControl::new(
            SwitchedShuntMode::Locked,
            1.05,
            0.95,
            vec![ShuntBlock::new(4, 5.0)],
        ));
        let converter = |id: &str| HvdcConverter {
            component: ComponentId::new("hvdc_converter", id).unwrap(),
            kind: HvdcConverterKind::Vsc,
            loss_factor_percent: 1.1,
            voltage_regulator_on: Some(true),
            voltage_setpoint_kv: Some(345.0),
            reactive_power_setpoint_mvar: None,
            power_factor: None,
            regulating_terminal: None,
        };
        let hvdc = &mut net.hvdc_mut()[0];
        hvdc.resistance_ohm = Some(0.5);
        hvdc.nominal_voltage_kv = Some(400.0);
        hvdc.converters_mode = Some(HvdcConvertersMode::Side1RectifierSide2Inverter);
        hvdc.converter1 = Some(converter("c1"));
        hvdc.converter2 = Some(converter("c2"));
        net.storage_mut()[0].active_power_control = Some(ActivePowerControl::new(false));
        net
    }

    #[test]
    fn typed_fields_round_trip_through_powermodels_and_egret_json() {
        let net = network_with_typed_fields();
        for (target, token) in [
            (
                crate::format::TargetFormat::PowerModelsJson,
                "powermodels-json",
            ),
            (crate::format::TargetFormat::EgretJson, "egret-json"),
        ] {
            let written = crate::format::emit_value_text(&net, target).unwrap();
            // The controls' `None` fields are left out, not written as null.
            assert!(
                written
                    .diagnostics
                    .iter()
                    .all(|d| !d.code().ends_with("NOT_A_NUMBER")),
                "{token}: {:?}",
                written.render_diagnostics()
            );
            let back = crate::parse_str(&written.text, token).unwrap().network;
            assert_eq!(back.buses()[0].location, net.buses()[0].location, "{token}");
            let (a, b) = (&net.branches()[0], &back.branches()[0]);
            assert_eq!(
                (&b.name, &b.control, &b.route),
                (&a.name, &a.control, &a.route),
                "{token}"
            );
            assert!(b.extras.is_empty(), "{token}: {:?}", b.extras);
            let (a, b) = (&net.generators()[0], &back.generators()[0]);
            assert_eq!(b.energy_source, a.energy_source, "{token}");
            assert_eq!(b.voltage_regulation_on, a.voltage_regulation_on, "{token}");
            assert_eq!(b.regulating_terminal, a.regulating_terminal, "{token}");
            assert_eq!(b.regulated_bus, a.regulated_bus, "{token}");
            assert_eq!(b.active_power_control, a.active_power_control, "{token}");
            let (a, b) = (&net.shunts()[0], &back.shunts()[0]);
            assert_eq!((b.section_count, &b.control), (a.section_count, &a.control));
            let (a, b) = (&net.hvdc()[0], &back.hvdc()[0]);
            assert_eq!(b.resistance_ohm, a.resistance_ohm, "{token}");
            assert_eq!(b.nominal_voltage_kv, a.nominal_voltage_kv, "{token}");
            assert_eq!(b.converters_mode, a.converters_mode, "{token}");
            assert_eq!(
                (&b.converter1, &b.converter2),
                (&a.converter1, &a.converter2)
            );
        }
        // egret states no storage; PowerModels does.
        let written =
            crate::format::emit_value_text(&net, crate::format::TargetFormat::PowerModelsJson)
                .unwrap();
        let back = crate::parse_str(&written.text, "powermodels-json")
            .unwrap()
            .network;
        assert_eq!(
            back.storage()[0].active_power_control,
            net.storage()[0].active_power_control
        );
    }

    #[test]
    fn a_value_without_the_powerio_shape_stays_an_extra() {
        let mut shunt = Shunt::new(BusId(1), 0.0, 10.0);
        shunt
            .extras
            .insert("control".into(), Value::String("another tool's".into()));
        shunt.extras.insert("section_count".into(), Value::from(2));
        read_shunt(&mut shunt);
        assert_eq!(shunt.section_count, Some(2));
        assert_eq!(shunt.control, None);
        assert_eq!(
            shunt.extras.get("control"),
            Some(&Value::String("another tool's".into()))
        );
        assert!(!shunt.extras.contains_key("section_count"));
    }

    #[test]
    fn nested_nonfinite_extension_numbers_reach_the_writer_diagnostic() {
        for nonfinite in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut net = network_with_typed_fields();
            net.shunts_mut()[0].control.as_mut().unwrap().vhigh = nonfinite;
            for target in [
                crate::format::TargetFormat::PowerModelsJson,
                crate::format::TargetFormat::EgretJson,
            ] {
                let written = crate::format::emit_value_text(&net, target).unwrap();
                assert!(
                    written.diagnostics.iter().any(|diagnostic| {
                        diagnostic.code().ends_with("NOT_A_NUMBER")
                            && diagnostic.message().contains("vhigh")
                    }),
                    "{target:?}: {:?}",
                    written.render_diagnostics()
                );
                assert!(written.text.contains("\"vhigh\": null"));
            }
        }
    }

    #[test]
    fn optional_nulls_and_literal_nonfinite_strings_are_not_numeric_findings() {
        #[derive(Serialize)]
        struct Nested {
            absent: Option<f64>,
            invalid: Option<f64>,
            label: &'static str,
        }
        let mut row = Map::new();
        put(
            &mut row,
            "nested",
            Some(&Nested {
                absent: None,
                invalid: Some(f64::NAN),
                label: "NaN",
            }),
        );
        assert!(row["nested"].get("absent").is_none());
        assert_eq!(row["nested"]["invalid"], Value::Null);
        assert_eq!(row["nested"]["label"], "NaN");
    }
}
