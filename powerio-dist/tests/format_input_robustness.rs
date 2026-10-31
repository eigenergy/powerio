//! Malformed dimensions must fail before allocating or indexing matrices.

use powerio_core::{FormatId, Source};
use serde_json::{Value, json};

fn parse(value: &Value, format: &str) -> Result<(), powerio_core::Error> {
    let source = Source::from_memory("case.json", serde_json::to_vec(value).unwrap())
        .unwrap()
        .with_format(FormatId::new(format).unwrap());
    powerio_dist::parse(source).map(|_| ())
}

#[test]
fn ravens_rejects_invalid_declared_and_inferred_matrix_sizes() {
    for count in [json!(1e300), json!(65), json!(-1), json!(1.5), Value::Null] {
        let doc = json!({"lc": {
            "Ravens.cimObjectType": "PerLengthPhaseImpedance",
            "PerLengthPhaseImpedance.conductorCount": count
        }});
        let error = parse(&doc, "ravens-json").unwrap_err();
        assert!(error.to_string().contains("conductorCount"), "{error}");
    }
    for coordinate in [json!(1e300), json!(65), json!(0), json!(-1), json!(1.5)] {
        let doc = json!({"lc": {
            "Ravens.cimObjectType": "PerLengthPhaseImpedance",
            "PerLengthPhaseImpedance.PhaseImpedanceData": [{
                "PhaseImpedanceData.row": coordinate,
                "PhaseImpedanceData.column": 1
            }]
        }});
        assert!(parse(&doc, "ravens-json").is_err());
    }
    let doc = json!({"lc": {
        "Ravens.cimObjectType": "PerLengthPhaseImpedance",
        "PerLengthPhaseImpedance.conductorCount": 1,
        "PerLengthPhaseImpedance.PhaseImpedanceData": [{
            "PhaseImpedanceData.row": 2,
            "PhaseImpedanceData.column": 1
        }]
    }});
    assert!(parse(&doc, "ravens-json").is_err());
    let doc = json!({"lc": {
        "Ravens.cimObjectType": "PerLengthPhaseImpedance",
        "PerLengthPhaseImpedance.conductorCount": 1.0,
        "PerLengthPhaseImpedance.PhaseImpedanceData": [{
            "PhaseImpedanceData.row": 1.0,
            "PhaseImpedanceData.column": 1.0,
            "PhaseImpedanceData.r": 1.0
        }]
    }});
    assert!(parse(&doc, "ravens-json").is_ok());
}

#[test]
fn ravens_bounds_phase_maps_and_transformer_windings() {
    let doc = json!({"terminal": {"Terminal.phases": format!("PhaseCode.{}", "A".repeat(65))}});
    assert!(parse(&doc, "ravens-json").is_err());
    for code in [
        "PhaseCode.AA",
        "PhaseCode.s1A",
        "PhaseCode.Z",
        "PhaseCode.s3",
    ] {
        let doc = json!({"terminal": {"Terminal.phases": code}});
        assert!(parse(&doc, "ravens-json").is_err(), "{code}");
    }
    for code in [
        "PhaseCode.ABCN",
        "PhaseCode.s1s2N",
        "PhaseCode.s12N",
        "PhaseCode.s12",
        "PhaseCode.X",
        "PhaseCode.XY",
        "PhaseCode.XN",
        "PhaseCode.XYN",
        "OrderedPhaseCodeKind.CBA",
        "PhaseCode.none",
    ] {
        let doc = json!({"terminal": {"Terminal.phases": code}});
        assert!(parse(&doc, "ravens-json").is_ok(), "{code}");
    }
    for field in [
        "ACLineSegment.ACLineSegmentPhase",
        "EnergyConsumer.EnergyConsumerPhase",
        "Switch.SwitchPhase",
        "TransformerTank.TransformerTankEnd",
        "TransformerTankInfo.TransformerEndInfos",
    ] {
        let doc = json!({"object": {field: vec![json!({}); 65]}});
        assert!(parse(&doc, "ravens-json").is_err(), "{field}");
    }
}

#[test]
fn pmd_math_rejects_empty_ragged_and_oversized_matrices() {
    for matrix in [
        json!([]),
        json!([[]]),
        json!([[1, 2]]),
        json!([[1], [2, 3]]),
        json!([false]),
        json!([["bad"]]),
        json!(vec![Vec::<f64>::new(); 65]),
    ] {
        let doc = json!({"data_model": "MATHEMATICAL", "shunt": {"1": {
            "shunt_bus": 1, "gs": matrix
        }}});
        let error = parse(&doc, "pmd-math-json").unwrap_err();
        assert!(error.to_string().contains("shunt.1.gs"), "{error}");
    }
    // Null entries retain PMD's restoration convention; the array shape is valid.
    let doc = json!({"data_model": "MATHEMATICAL", "shunt": {"1": {
        "shunt_bus": 1, "gs": [[null]]
    }}});
    assert!(parse(&doc, "pmd-math-json").is_ok());
}

#[test]
fn pmd_math_empty_transformer_loss_matrices_return_errors() {
    let original: Value = serde_json::from_str(include_str!(
        "../../tests/data/dist/pmd/math/ut_trans_2w_yy_math.json"
    ))
    .unwrap();
    for field in ["br_r", "br_x", "g_fr", "g_to", "b_fr", "b_to"] {
        let mut doc = original.clone();
        doc["branch"]["3"][field] = json!([]);
        assert!(parse(&doc, "pmd-math-json").is_err(), "{field}");
    }
}

#[test]
fn pmd_math_matrix_dimensions_match_each_other_and_the_connections() {
    for branch in [
        json!({"br_r": [[1]], "br_x": [[1, 0], [0, 1]]}),
        json!({"br_r": [[1]], "f_connections": [1, 2]}),
        json!({"br_r": [[1]], "t_connections": []}),
    ] {
        let doc = json!({"data_model": "MATHEMATICAL", "branch": {"1": branch}});
        assert!(parse(&doc, "pmd-math-json").is_err());
    }
}

#[test]
fn pmd_math_bounds_winding_loss_model_before_allocating() {
    let doc = json!({"data_model": "MATHEMATICAL", "map": [{
        "unmap_function": "_map_math2eng_transformer!",
        "to": vec!["transformer.1"; 65]
    }]});
    let error = parse(&doc, "pmd-math-json").unwrap_err();
    assert!(error.to_string().contains("winding units"), "{error}");
}

#[test]
fn pmd_math_bounds_phase_vectors_before_load_folding() {
    for key in ["pd", "qd", "connections"] {
        let doc = json!({"data_model": "MATHEMATICAL", "load": {"1": {
            "load_bus": 1, key: vec![1; 65]
        }}});
        let error = parse(&doc, "pmd-math-json").unwrap_err();
        assert!(
            error.to_string().contains("phase vector exceeds 64"),
            "{error}"
        );
    }
}

#[test]
fn pmd_math_writer_rejects_malformed_raw_engineering_matrices() {
    for matrix in [
        json!(vec![Vec::<f64>::new(); 65]),
        json!([]),
        json!([[1, 2]]),
    ] {
        for source in [false, true] {
            let mut doc = json!({"data_model": "ENGINEERING", "bus": {
                "a": {"terminals": [1]}, "b": {"terminals": [1]}
            }});
            if source {
                doc["voltage_source"] = json!({"v": {
                    "bus": "a", "connections": [1], "vm": [1], "va": [0],
                    "rs": matrix, "xs": [[1]]
                }});
            } else {
                doc["switch"] = json!({"s": {
                    "f_bus": "a", "t_bus": "b", "f_connections": [1], "t_connections": [1],
                    "rs": matrix, "xs": [[1]]
                }});
            }
            let source = Source::from_memory("case.json", serde_json::to_vec(&doc).unwrap())
                .unwrap()
                .with_format(FormatId::new("pmd-json").unwrap());
            let module = powerio_dist::parse(source).unwrap();
            let error = powerio_dist::emit(
                &module,
                powerio_dist::DistTargetFormat::PmdMathJson,
                powerio_core::Destination::memory("case").unwrap(),
            )
            .unwrap_err();
            assert_eq!(
                error.info().unwrap().code,
                "BUILD.DIST.ELECTRICAL_INCOMPLETE"
            );
            assert!(
                error.to_string().contains("square numeric matrix"),
                "{error}"
            );
        }
    }
}

#[test]
fn pmd_math_writer_bounds_conductor_maps_before_lowering() {
    let doc = json!({"data_model": "ENGINEERING", "bus": {
        "a": {"terminals": [1]}, "b": {"terminals": [1]}
    }, "switch": {"s": {
        "f_bus": "a", "t_bus": "b",
        "f_connections": vec![1; 65], "t_connections": vec![1; 65],
        "rs": [[1]], "xs": [[1]]
    }}});
    let source = Source::from_memory("case.json", serde_json::to_vec(&doc).unwrap())
        .unwrap()
        .with_format(FormatId::new("pmd-json").unwrap());
    let module = powerio_dist::parse(source).unwrap();
    let error = powerio_dist::emit(
        &module,
        powerio_dist::DistTargetFormat::PmdMathJson,
        powerio_core::Destination::memory("case").unwrap(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("dimension 65 exceeds 64"),
        "{error}"
    );
}
