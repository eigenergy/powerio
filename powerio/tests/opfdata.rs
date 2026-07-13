//! DeepMind OPFData JSON reader and conversion tests.
//!
//! The official case-14 document is the smallest fixture that contains every
//! published node and edge table. Count-changing tests below exercise the same
//! schema rules used by every FullTop and N-1 grid size; the reader contains no
//! case-name or fixed-element-count dispatch.

use std::path::{Path, PathBuf};

use powerio::{
    BranchCharging, BusId, BusType, Error, Network, SourceFormat, TargetFormat, convert_file,
    parse_file, parse_str, write_as,
};
use serde_json::Value;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/opfdataset/example_0.json")
}

fn fixture_text() -> String {
    std::fs::read_to_string(fixture()).unwrap()
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1.0e-10 * actual.abs().max(expected.abs()).max(1.0),
        "{actual} != {expected}"
    );
}

#[test]
fn parses_official_schema_complete_solved_snapshot() {
    let parsed = parse_file(fixture(), None).unwrap();
    let net = &parsed.network;

    assert_eq!(net.source_format, SourceFormat::OpfDataJson);
    assert_eq!(net.name, "example_0");
    assert_close(net.base_mva, 100.0);
    assert_eq!(net.buses.len(), 14);
    assert_eq!(net.generators.len(), 5);
    assert_eq!(net.loads.len(), 11);
    assert_eq!(net.shunts.len(), 1);
    assert_eq!(net.branches.len(), 20);

    assert_eq!(net.buses[0].id, BusId(1));
    assert_eq!(net.buses[0].kind, BusType::Ref);
    assert_eq!(net.buses[1].kind, BusType::Pv);
    assert_close(net.buses[0].vm, 1.060_000_010_369_160_5);
    assert_close(net.buses[0].va, 0.0);
    assert_close(net.buses[0].vmin, 0.94);
    assert_close(net.buses[0].vmax, 1.06);

    let generator = &net.generators[0];
    assert_eq!(generator.bus, BusId(1));
    assert_close(generator.pg, 286.070_948_069_333_44);
    assert_close(generator.qg, 3.883_803_174_297_159);
    assert_close(generator.pmin, 0.0);
    assert_close(generator.pmax, 340.0);
    assert_close(generator.vg, net.buses[0].vm);
    assert_close(generator.mbase, 100.0);
    let cost = generator.cost.as_ref().unwrap();
    assert_eq!(cost.model, 2);
    assert_eq!(cost.ncost, 3);
    assert_close(cost.coeffs[0], 0.0);
    assert_close(cost.coeffs[1], 7.920_951);
    assert_close(cost.coeffs[2], 0.0);

    assert_eq!(net.loads[0].bus, BusId(2));
    assert_close(net.loads[0].p, 20.649_686_030_854_52);
    assert_close(net.loads[0].q, 14.970_012_102_581_254);
    assert_eq!(net.shunts[0].bus, BusId(9));
    assert_close(net.shunts[0].g, 0.0);
    assert_close(net.shunts[0].b, 19.0);

    let line = &net.branches[0];
    assert_eq!((line.from, line.to), (BusId(1), BusId(2)));
    assert_close(line.r, 0.01938);
    assert_close(line.x, 0.05917);
    assert_close(line.b, 0.0528);
    assert_eq!(
        line.charging,
        Some(BranchCharging::new(0.0, 0.0264, 0.0, 0.0264))
    );
    assert_close(line.rate_a, 472.0);
    assert_close(line.angmin, -30.0);
    assert_close(line.angmax, 30.0);
    let flow = line.solution.unwrap();
    assert_close(flow.pf, 200.987_451_502_624_43);
    assert_close(flow.qf, -4.314_632_222_308_059);
    assert_close(flow.pt, -194.019_590_717_250_68);
    assert_close(flow.qt, 19.820_585_478_075_1);

    let transformer = &net.branches[17];
    assert_eq!((transformer.from, transformer.to), (BusId(4), BusId(7)));
    assert_close(transformer.x, 0.20912);
    assert_close(transformer.tap, 0.978);
    assert_close(transformer.rate_a, 141.0);

    assert_eq!(parsed.warnings.len(), 2, "{:?}", parsed.warnings);
    assert!(parsed.warnings[0].contains("solver initial values"));
    assert!(parsed.warnings[1].contains("synthesized IDs"));
}

#[test]
fn detects_aliases_and_echoes_the_official_source_exactly() {
    let source = fixture_text();
    for alias in [
        "opfdata-json",
        "opfdata",
        "OPFData",
        "gridopt-json",
        "gridopt",
    ] {
        let parsed = parse_str(&source, alias).unwrap();
        assert_eq!(parsed.network.source_format, SourceFormat::OpfDataJson);
    }

    let parsed = parse_file(fixture(), None).unwrap();
    let echo = write_as(&parsed.network, TargetFormat::OpfDataJson).unwrap();
    assert_eq!(echo.text, source);
    assert!(echo.warnings.is_empty());
}

#[test]
fn converts_to_classical_json_and_matpower_with_fidelity_warnings() {
    let power_models = convert_file(fixture(), TargetFormat::PowerModelsJson, None).unwrap();
    let back = parse_str(&power_models.text, "powermodels-json")
        .unwrap()
        .network;
    assert_eq!(back.buses.len(), 14);
    assert_eq!(back.generators.len(), 5);
    assert_eq!(back.branches.len(), 20);
    assert_close(back.generators[0].pg, 286.070_948_069_333_44);
    assert!(
        power_models
            .warnings
            .iter()
            .any(|warning| warning.contains("solver initial values"))
    );

    let matpower = convert_file(fixture(), TargetFormat::Matpower, None).unwrap();
    assert!(matpower.text.contains("mpc.bus"));
    assert!(matpower.text.contains("mpc.gencost"));
    assert!(
        matpower
            .warnings
            .iter()
            .any(|warning| warning.contains("branch solution value"))
    );
}

#[test]
fn opfdata_target_without_retained_source_is_unsupported() {
    let err = write_as(&Network::new("memory", 100.0), TargetFormat::OpfDataJson).unwrap_err();
    assert!(matches!(
        err,
        Error::WriteUnsupported {
            format: "opfdata-json"
        }
    ));
}

fn modified(mut edit: impl FnMut(&mut Value)) -> String {
    let mut value: Value = serde_json::from_str(&fixture_text()).unwrap();
    edit(&mut value);
    serde_json::to_string(&value).unwrap()
}

fn recompute_objective(value: &mut Value) {
    let generators = value["grid"]["nodes"]["generator"].as_array().unwrap();
    let solution = value["solution"]["nodes"]["generator"].as_array().unwrap();
    let objective = generators
        .iter()
        .zip(solution)
        .map(|(generator, solved)| {
            let row = generator.as_array().unwrap();
            let pg = solved[0].as_f64().unwrap();
            row[8].as_f64().unwrap() * pg * pg
                + row[9].as_f64().unwrap() * pg
                + row[10].as_f64().unwrap()
        })
        .sum::<f64>();
    value["metadata"]["objective"] = Value::from(objective);
}

#[test]
fn accepts_variable_fulltop_and_n_minus_one_element_counts() {
    let line_outage = modified(|value| {
        for path in [
            "/grid/edges/ac_line/senders",
            "/grid/edges/ac_line/receivers",
            "/grid/edges/ac_line/features",
            "/solution/edges/ac_line/senders",
            "/solution/edges/ac_line/receivers",
            "/solution/edges/ac_line/features",
        ] {
            value
                .pointer_mut(path)
                .unwrap()
                .as_array_mut()
                .unwrap()
                .pop();
        }
    });
    let parsed = parse_str(&line_outage, "opfdata").unwrap();
    assert_eq!(parsed.network.generators.len(), 5);
    assert_eq!(parsed.network.branches.len(), 19);
    assert!(!parsed.warnings.iter().any(|w| w.contains("objective")));

    let generator_outage = modified(|value| {
        let removed = value["grid"]["nodes"]["generator"]
            .as_array_mut()
            .unwrap()
            .len()
            - 1;
        value["grid"]["nodes"]["generator"]
            .as_array_mut()
            .unwrap()
            .pop();
        value["solution"]["nodes"]["generator"]
            .as_array_mut()
            .unwrap()
            .pop();

        let senders = value["grid"]["edges"]["generator_link"]["senders"]
            .as_array_mut()
            .unwrap();
        let link = senders
            .iter()
            .position(|sender| sender.as_u64() == Some(removed as u64))
            .unwrap();
        senders.remove(link);
        value["grid"]["edges"]["generator_link"]["receivers"]
            .as_array_mut()
            .unwrap()
            .remove(link);
        recompute_objective(value);
    });
    let parsed = parse_str(&generator_outage, "opfdata").unwrap();
    assert_eq!(parsed.network.generators.len(), 4);
    assert_eq!(parsed.network.branches.len(), 20);
    assert!(!parsed.warnings.iter().any(|w| w.contains("objective")));
}

#[test]
fn maps_general_quadratic_costs_from_per_unit_to_mw() {
    let source = modified(|value| {
        value["grid"]["nodes"]["generator"][0][8] = Value::from(100.0);
        value["grid"]["nodes"]["generator"][0][9] = Value::from(200.0);
        value["grid"]["nodes"]["generator"][0][10] = Value::from(3.0);
        recompute_objective(value);
    });
    let parsed = parse_str(&source, "opfdata").unwrap();
    let cost = parsed.network.generators[0].cost.as_ref().unwrap();
    assert_close(cost.coeffs[0], 0.01);
    assert_close(cost.coeffs[1], 2.0);
    assert_close(cost.coeffs[2], 3.0);
    assert!(!parsed.warnings.iter().any(|w| w.contains("objective")));
}

#[test]
fn retains_published_schema_extensions_and_warns_on_projection() {
    let source = modified(|value| {
        value["dataset_revision"] = Value::from(2);
        value["metadata"]["solver"] = Value::from("example");
    });
    let parsed = parse_str(&source, "opfdata").unwrap();
    assert!(
        parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("`dataset_revision`"))
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("`metadata.solver`"))
    );
    let echo = write_as(&parsed.network, TargetFormat::OpfDataJson).unwrap();
    assert_eq!(echo.text, source);
}

#[test]
fn rejects_bad_base_bus_type_and_feature_width() {
    let bad_base = modified(|value| value["grid"]["context"][0][0][0] = Value::from(0.0));
    let err = parse_str(&bad_base, "opfdata").unwrap_err();
    assert!(
        err.to_string().contains("baseMVA must be positive"),
        "{err}"
    );

    let bad_type = modified(|value| value["grid"]["nodes"]["bus"][0][1] = Value::from(9.0));
    let err = parse_str(&bad_type, "opfdata").unwrap_err();
    assert!(err.to_string().contains("invalid bus type 9"), "{err}");

    let bad_width = modified(|value| {
        value["grid"]["edges"]["ac_line"]["features"][0]
            .as_array_mut()
            .unwrap()
            .pop();
    });
    let err = parse_str(&bad_width, "opfdata").unwrap_err();
    assert!(err.to_string().contains("invalid OPFData schema"), "{err}");
}

#[test]
fn rejects_bad_links_and_solution_topology() {
    let duplicate_link = modified(|value| {
        value["grid"]["edges"]["generator_link"]["senders"][1] = Value::from(0);
    });
    let err = parse_str(&duplicate_link, "opfdata").unwrap_err();
    assert!(err.to_string().contains("more than one link"), "{err}");

    let bad_endpoint = modified(|value| {
        value["grid"]["edges"]["load_link"]["receivers"][0] = Value::from(99);
    });
    let err = parse_str(&bad_endpoint, "opfdata").unwrap_err();
    assert!(err.to_string().contains("bus index 99"), "{err}");

    let topology_mismatch = modified(|value| {
        value["solution"]["edges"]["transformer"]["receivers"][0] = Value::from(8);
    });
    let err = parse_str(&topology_mismatch, "opfdata").unwrap_err();
    assert!(err.to_string().contains("topology differs"), "{err}");
}

#[test]
fn warns_when_objective_does_not_match_and_guides_unsupported_inputs() {
    let wrong_objective = modified(|value| value["metadata"]["objective"] = Value::from(-1.0));
    let parsed = parse_str(&wrong_objective, "opfdata").unwrap();
    assert!(
        parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("metadata.objective"))
    );

    for path in ["cache.pt", "group.tar.gz"] {
        let err = parse_file(path, Some("opfdata")).unwrap_err();
        assert!(err.to_string().contains("extract"), "{path}: {err}");
    }
}
