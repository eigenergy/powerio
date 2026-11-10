use powerio::{Destination, EmittedOutput, PioModule, PioValue, Source};
use powerio_tx::acdc::CAPABILITY;
const CASE: &str = include_str!("../../tests/data/pglib-hvdc/case5_3_he.m");
fn source(name: &str, text: &str) -> Source {
    Source::from_memory(name, text.as_bytes().to_vec()).unwrap()
}
fn parse(text: &str) -> PioModule<PioValue> {
    powerio::parse(source("case5.m", text)).unwrap()
}
fn network(m: &PioModule<PioValue>) -> &powerio::BalancedNetwork {
    let PioValue::BalancedNetwork(n) = m.value() else {
        panic!()
    };
    n
}
fn bytes(r: powerio_core::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { mut artifacts } = r.into_output() else {
        panic!()
    };
    artifacts.pop().unwrap().into_bytes()
}
fn stored(m: &PioModule<PioValue>) -> serde_json::Value {
    serde_json::from_slice(&bytes(
        powerio::serialize(m, Destination::memory("x.json").unwrap()).unwrap(),
    ))
    .unwrap()
}
fn read_json(v: &serde_json::Value) -> Result<PioModule<PioValue>, powerio_core::Error> {
    powerio::deserialize(source("x.json", &v.to_string()))
}

#[test]
fn case5_typed_import_echo_ir_and_fresh_export() {
    let m = parse(CASE);
    let e = network(&m).ac_dc_equivalent().unwrap();
    assert_eq!(
        (e.buses.len(), e.branches.len(), e.stations.len()),
        (3, 3, 3)
    );
    assert_eq!(e.pole_count, 2);
    assert_eq!(
        bytes(powerio::emit(&m, "matpower-acdc", Destination::memory("x.m").unwrap()).unwrap()),
        CASE.as_bytes()
    );
    let document = stored(&m);
    assert_eq!(
        document["required_capabilities"],
        serde_json::json!([CAPABILITY])
    );
    let restored = read_json(&document).unwrap();
    let fresh = bytes(
        powerio::emit(
            &restored,
            "matpower-acdc",
            Destination::memory("fresh.m").unwrap(),
        )
        .unwrap(),
    );
    let fresh = String::from_utf8(fresh).unwrap();
    assert!(fresh.contains("mpc.convdc = ["));
    assert_eq!(network(&parse(&fresh)).ac_dc_equivalent(), Some(e));
    assert!(powerio::emit(&m, "matpower", Destination::memory("x.m").unwrap()).is_err());
    assert!(powerio::emit(&restored, "psse", Destination::memory("x.raw").unwrap()).is_err());
    assert!(
        powerio::parse(
            source("x.m", CASE).with_format(powerio_tx::parse_format_id("matpower").unwrap())
        )
        .is_err()
    );
}

#[test]
fn required_capability_is_checked_in_every_nested_container() {
    let document = stored(&parse(CASE));
    let net = document["value"]["data"].clone();
    let times = serde_json::json!([{"label":"t0"}]);
    let ts = serde_json::json!({"time_points":times,"values":[net]});
    let sc = |candidate: serde_json::Value| serde_json::json!({"scenarios":[{"id":"s0","value":candidate}]});
    for (kind, data) in [
        ("powerio.BalancedNetwork", net.clone()),
        (
            "powerio.OperatingPoint<powerio.BalancedNetwork>",
            serde_json::json!({"network":net,"quantities":{}}),
        ),
        ("powerio.TimeSeries<powerio.BalancedNetwork>", ts.clone()),
        (
            "powerio.ScenarioSet<powerio.BalancedNetwork>",
            sc(net.clone()),
        ),
        (
            "powerio.ScenarioSet<powerio.TimeSeries<powerio.BalancedNetwork>>",
            sc(ts),
        ),
        (
            "powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>>",
            sc(serde_json::json!({"network":net,"time_points":times,"values":[{"quantities":{}}]})),
        ),
        (
            "powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>",
            serde_json::json!({"network":net,"time_points":times,"values":[{"quantities":{}}]}),
        ),
        (
            "powerio.ScenarioSet<powerio.OperatingPoint<powerio.BalancedNetwork>>",
            serde_json::json!({"network":net,"scenarios":[{"id":"s0","quantities":{}}]}),
        ),
    ] {
        let mut candidate = document.clone();
        candidate["value"] = serde_json::json!({"type":kind,"data":data});
        candidate.as_object_mut().unwrap().remove("source_map");
        let module = read_json(&candidate).unwrap_or_else(|e| panic!("{kind}: {e}"));
        assert_eq!(
            stored(&module)["required_capabilities"],
            serde_json::json!([CAPABILITY])
        );
        candidate
            .as_object_mut()
            .unwrap()
            .remove("required_capabilities");
        assert!(read_json(&candidate).is_err(), "{kind}");
        candidate["required_capabilities"] = serde_json::json!(["unknown-model"]);
        assert!(read_json(&candidate).is_err());
    }
    let ordinary = powerio::parse(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/case9.m"
    ))
    .unwrap();
    assert!(stored(&ordinary).get("required_capabilities").is_none());
}

#[test]
fn legacy_calculations_refuse_before_normalization_or_supply_filtering() {
    let m = parse(CASE);
    let n = network(&m);
    assert!(n.to_normalized().is_err());
    assert!(powerio::AcPfInstance::from_network(n.clone()).is_err());
    assert!(powerio::DcPfInstance::from_network(n.clone()).is_err());
    assert!(powerio::AcOpfInstance::from_network(n.clone()).is_err());
    assert!(powerio::DcOpfInstance::from_network(n.clone()).is_err());
    let mut copy = n.clone();
    let before = serde_json::to_value(&copy).unwrap();
    let r = copy.assign_island_references(powerio_tx::IslandReferencePolicy::Stated);
    assert!(
        r.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "REQUEST.ACDC.UNSUPPORTED")
    );
    assert_eq!(serde_json::to_value(&copy).unwrap(), before);
    let mut e = n.ac_dc_equivalent().unwrap().clone();
    e.stations[0].settings.pcc_withdrawal_mvar = 13.0;
    copy.set_ac_dc_equivalent(Some(e)).unwrap();
    assert_ne!(copy.ac_dc_equivalent(), n.ac_dc_equivalent());
}

#[test]
fn malformed_extensions_never_fall_back_to_ac_only() {
    let ordinary = include_str!("../../tests/data/case9.m");
    assert!(
        powerio::parse(source(
            "indexed.m",
            &format!("{ordinary}\nmpc.convdc(1,:) = [1 2 3];\n")
        ))
        .is_err()
    );
    for bad in [
        format!("{CASE}\nmpc.busdc = [];\n"),
        CASE.replace("mpc.dcpol=2;", "mpc.dcpol=3;"),
        CASE.replace("mpc.dcpol=2;", ""),
        CASE.replace("mpc.dcconv = [", "mpc.dcconv = [\n1 2 3;"),
        format!("{CASE}\nmpc.gendc = [1 2 3];\n"),
        CASE.replace(
            "%column_names%   busdc_i grid",
            "%column_names%   wrong grid",
        ),
    ] {
        assert!(powerio::parse(source("bad.m", &bad)).is_err());
    }
}

fn change_first_row(text: &str, table: &str, changes: &[(usize, &str)]) -> String {
    let start = text.find(&format!("mpc.{table} = [")).unwrap();
    let row_start = text[start..].find('\n').unwrap() + start + 1;
    let row_end = text[row_start..].find(';').unwrap() + row_start;
    let mut row: Vec<_> = text[row_start..row_end]
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    for &(column, value) in changes {
        value.clone_into(&mut row[column]);
    }
    format!(
        "{}{}{}",
        &text[..row_start],
        row.join(" "),
        &text[row_end..]
    )
}

#[test]
fn asymmetric_conventions_unused_settings_and_disabled_parameters_survive() {
    let input = change_first_row(
        CASE,
        "dcconv",
        &[
            (2, "2"),
            (4, "17"),
            (5, "-13"),
            (10, "0"),
            (13, "0"),
            (16, "0"),
            (17, "220"),
            (21, "0"),
            (24, "2.1"),
            (25, "3.4"),
            (26, "0"),
            (27, "-8"),
            (28, "1.03"),
            (29, "0.02"),
            (30, "91"),
            (31, "-57"),
            (32, "43"),
            (33, "-19"),
        ],
    );
    let m = parse(&input);
    let n = network(&m);
    let e = n.ac_dc_equivalent().unwrap();
    let c = &e.stations[0];
    assert!((c.settings.pcc_withdrawal_mw + 17.0).abs() < 1e-12);
    assert!((c.settings.pcc_withdrawal_mvar - 13.0).abs() < 1e-12);
    assert!((c.valve_reactive_mvar.min + 19.0).abs() < 1e-12);
    assert!((c.valve_reactive_mvar.max - 43.0).abs() < 1e-12);
    assert!(!c.in_service && !c.transformer.enabled && !c.filter_enabled && !c.reactor.enabled);
    assert!((c.ac_base_kv - 220.0).abs() < 1e-12);
    let restored = read_json(&stored(&m)).unwrap();
    let output = bytes(
        powerio::emit(
            &restored,
            "matpower-acdc",
            Destination::memory("x.m").unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        network(&parse(&String::from_utf8(output).unwrap())).ac_dc_equivalent(),
        Some(e)
    );
    // Disabled records still require valid references and integral flags.
    for (col, value) in [
        (0, "999"),
        (1, "999"),
        (6, "0.5"),
        (10, "2"),
        (21, "-1"),
        (2, "4"),
    ] {
        let bad = change_first_row(&input, "dcconv", &[(col, value)]);
        assert!(
            powerio::parse(source("bad.m", &bad)).is_err(),
            "column {col}"
        );
    }
}

#[test]
fn dc_grid_labels_zero_ratings_parallel_records_and_bus_namespaces_are_preserved() {
    let input = change_first_row(CASE, "dcbus", &[(1, "99"), (2, "7")]);
    let input = change_first_row(&input, "dcbranch", &[(5, "0")]);
    let m = parse(&input);
    let e = network(&m).ac_dc_equivalent().unwrap();
    assert_eq!(e.buses[0].source_grid, 99);
    assert!(e.buses[0].withdrawal_mw > 0.0);
    assert!(e.branches[0].ratings_mw[0].abs() < 1e-12);
    // Reusing the first station's endpoint does not merge station identities.
    let input = change_first_row(&input, "dcconv", &[(0, "2"), (1, "3")]);
    let m = parse(&input);
    let e = network(&m).ac_dc_equivalent().unwrap();
    assert_eq!(e.stations[0].dc_bus, e.stations[1].dc_bus);
    assert_ne!(e.stations[0].component, e.stations[1].component);
    for bad in [
        change_first_row(CASE, "dcbus", &[(0, "2")]),
        change_first_row(CASE, "dcbus", &[(0, "1.5")]),
        change_first_row(CASE, "dcbranch", &[(8, "0"), (0, "999")]),
    ] {
        assert!(powerio::parse(source("bad.m", &bad)).is_err());
    }
}

#[test]
fn explicit_subsets_and_bus_remaps_preserve_dc_records_and_references() {
    let m = parse(CASE);
    let n = network(&m);
    let e = n.ac_dc_equivalent().unwrap();
    let all: std::collections::BTreeSet<_> = n.buses().iter().map(|b| b.id).collect();
    assert_eq!(n.subset_buses(&all).network.ac_dc_equivalent(), Some(e));
    let kept = std::collections::BTreeSet::from([powerio::BusId(2)]);
    let subset = n.subset_buses(&kept).network;
    let sub = subset.ac_dc_equivalent().unwrap();
    assert_eq!(sub.buses, e.buses);
    assert_eq!(sub.branches, e.branches);
    assert_eq!(sub.stations.len(), 1);
    subset.validate().unwrap();
    let mut merged = n.clone();
    merged.merge_bus(powerio::BusId(3), powerio::BusId(2));
    merged.validate().unwrap();
    assert_eq!(
        merged.ac_dc_equivalent().unwrap().stations[0].ac_bus,
        powerio::BusId(3)
    );
    assert_eq!(n.ac_dc_equivalent(), Some(e));
    let mut deenergized = n.clone();
    deenergized.de_energize_buses(&kept);
    assert!(!deenergized.ac_dc_equivalent().unwrap().stations[0].in_service);
}

#[test]
fn ir_rejects_invalid_electrical_data_even_with_the_capability() {
    let d = stored(&parse(CASE));
    for (pointer, value) in [
        (
            "/value/data/ac_dc_equivalent/stations/0/ac_bus",
            serde_json::json!(999),
        ),
        (
            "/value/data/ac_dc_equivalent/pole_count",
            serde_json::json!(3),
        ),
        (
            "/value/data/ac_dc_equivalent/branches/0/resistance_pu",
            serde_json::json!("NaN"),
        ),
        (
            "/value/data/ac_dc_equivalent/stations/0/losses/constant_mw",
            serde_json::json!(-1),
        ),
    ] {
        let mut bad = d.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        assert!(read_json(&bad).is_err(), "{pointer}");
    }
}

#[test]
fn empty_dc_branches_allow_back_to_back_and_empty_metadata_does_not_block_calculations() {
    let m = parse(CASE);
    let mut n = network(&m).clone();
    let mut e = n.ac_dc_equivalent().unwrap().clone();
    e.buses.truncate(1);
    e.branches.clear();
    for c in &mut e.stations {
        c.dc_bus = e.buses[0].component.clone();
    }
    n.set_ac_dc_equivalent(Some(e.clone())).unwrap();
    let module = PioModule::new(n);
    let fresh = bytes(
        powerio::emit(
            &module,
            "matpower-acdc",
            Destination::memory("x.m").unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        network(&parse(&String::from_utf8(fresh).unwrap())).ac_dc_equivalent(),
        Some(&e)
    );
    let ordinary = include_str!("../../tests/data/case9.m");
    let m = parse(&format!("{ordinary}\nmpc.dcpol = 2;\n"));
    assert!(powerio::AcPfInstance::from_network(network(&m).clone()).is_ok());
    assert!(
        network(&m)
            .to_normalized()
            .unwrap()
            .ac_dc_equivalent()
            .is_some()
    );
}

#[test]
fn declared_area_layout_survives_and_ragged_or_surplus_columns_fail() {
    let mut input = CASE.replace("Vdcmin  Cdc", "Vdcmin  Cdc area");
    let start = input.find("mpc.dcbus = [").unwrap();
    let end = start + input[start..].find("];").unwrap();
    let block = input[start..end].replace("0;", "0 7;");
    input.replace_range(start..end, &block);
    let m = parse(&input);
    let e = network(&m).ac_dc_equivalent().unwrap();
    assert!(e.buses.iter().all(|b| b.area == Some(7)));
    let restored = read_json(&stored(&m)).unwrap();
    let fresh = bytes(
        powerio::emit(
            &restored,
            "matpower-acdc",
            Destination::memory("x.m").unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        network(&parse(&String::from_utf8(fresh).unwrap())).ac_dc_equivalent(),
        Some(e)
    );
    let bad = input.replace("Cdc area", "Cdc");
    assert!(powerio::parse(source("bad.m", &bad)).is_err());
    // Appending data to only one station is a ragged electrical table.
    let start = input.find("mpc.dcconv = [").unwrap();
    let end = start + input[start..].find(';').unwrap();
    input.insert_str(end, " 999");
    assert!(powerio::parse(source("bad.m", &input)).is_err());
}

#[test]
fn unsupported_exports_refuse_before_directory_or_source_shortcuts() {
    let m = parse(CASE);
    for target in [
        "pypsa-csv",
        "gridfm",
        "cgmes",
        "sincal-balanced",
        "powermodels-json",
    ] {
        assert!(
            powerio::emit(&m, target, Destination::memory("out").unwrap()).is_err(),
            "{target}"
        );
    }
}

#[cfg(feature = "gridfm")]
#[test]
fn direct_gridfm_builder_refuses_dc_equipment() {
    let m = parse(CASE);
    assert!(
        powerio_matrix::io::gridfm::build_gridfm_dataset(
            network(&m),
            0,
            &powerio_matrix::io::gridfm::GridfmOptions::default()
        )
        .is_err()
    );
}
