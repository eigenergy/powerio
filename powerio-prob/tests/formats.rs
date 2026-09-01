//! GO Challenge 3 decoder and emitter details below the universal parser.

use powerio_core::{Error, FormatId, PioModule, Source};
use powerio_prob::__internal::{
    __emit_goc3_output, __parse_goc3_output_buffer, __parse_goc3_problem_buffer,
};
use powerio_prob::solution::Termination;

type JsonMutation = fn(&mut serde_json::Value);
type ScucInputMutation = fn(&mut powerio_prob::ScucInputs);

fn fixture(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap()
}

fn memory(name: &str, text: &str) -> Source {
    Source::from_memory(name, text.as_bytes().to_vec()).unwrap()
}

fn decode_goc3_problem(source: Source) -> Result<PioModule<powerio_prob::AcScucInstance>, Error> {
    let source = match source.format() {
        Some(_) => source,
        None => source.with_format(FormatId::new("goc3-json").unwrap()),
    };
    let buffer = match source.primary_buffer() {
        Ok(buffer) => buffer,
        Err(error) => return Err(error.with_source(source)),
    };
    match __parse_goc3_problem_buffer(&buffer) {
        Ok((instance, diagnostics)) => PioModule::parsed(instance, source, diagnostics),
        Err(error) => Err(error.with_source(source)),
    }
}

fn goc3_output() -> serde_json::Value {
    serde_json::json!({
        "time_series_output": {
            "bus": [
                {"uid": "bus_00", "vm": [1.0, 1.01], "va": [0.0, 0.0]},
                {"uid": "bus_01", "vm": [0.99, 0.98], "va": [-0.1, -0.2]}
            ],
            "shunt": [
                {"uid": "sh_00", "step": [1, 2]}
            ],
            "simple_dispatchable_device": [
                {
                    "uid": "sd_00", "on_status": [1, 1], "p_on": [0.1, 0.2],
                    "q": [0.0, 0.0], "p_reg_res_up": [0.0, 0.0],
                    "p_reg_res_down": [0.0, 0.0], "p_syn_res": [0.0, 0.0],
                    "p_nsyn_res": [0.0, 0.0], "p_ramp_res_up_online": [0.0, 0.0],
                    "p_ramp_res_down_online": [0.0, 0.0],
                    "p_ramp_res_up_offline": [0.0, 0.0],
                    "p_ramp_res_down_offline": [0.0, 0.0],
                    "q_res_up": [0.0, 0.0], "q_res_down": [0.0, 0.0]
                },
                {
                    "uid": "sd_01", "on_status": [1, 0], "p_on": [0.04, 0.0],
                    "q": [0.0, 0.0], "p_reg_res_up": [0.0, 0.0],
                    "p_reg_res_down": [0.0, 0.0], "p_syn_res": [0.0, 0.0],
                    "p_nsyn_res": [0.0, 0.0], "p_ramp_res_up_online": [0.0, 0.0],
                    "p_ramp_res_down_online": [0.0, 0.0],
                    "p_ramp_res_up_offline": [0.0, 0.0],
                    "p_ramp_res_down_offline": [0.0, 0.0],
                    "q_res_up": [0.0, 0.0], "q_res_down": [0.0, 0.0]
                }
            ],
            "ac_line": [
                {"uid": "acl_00", "on_status": [1, 1]},
                {"uid": "acl_01", "on_status": [1, 0]}
            ],
            "two_winding_transformer": [
                {"uid": "xf_00", "tm": [1.0, 1.01], "ta": [0.0, 0.01], "on_status": [1, 1]}
            ],
            "dc_line": [
                {"uid": "dc_00", "pdc_fr": [0.0, 0.1], "qdc_fr": [0.0, 0.0], "qdc_to": [0.0, 0.0]}
            ]
        }
    })
}

#[test]
fn goc3_parses_to_the_scuc_instance() {
    let text = fixture("tests/data/goc3_small.json");
    let module = decode_goc3_problem(memory("goc3_small.json", &text)).unwrap();
    // The module retains the source it parsed.
    assert!(module.source().is_some());
    let instance = &module.value;

    // The reusable electrical model is stored once.
    assert_eq!(instance.network().buses().len(), 2);
    assert_eq!(instance.inputs().devices.len(), 2);
    assert_eq!(instance.inputs().branch_switching_costs.len(), 3);

    // The scheduling categories and nested time data arrived typed.
    assert!(!instance.inputs().interval_durations.is_empty());
    assert_eq!(instance.inputs().devices[0].periods.len(), 2);
    assert!(module.diagnostics.iter().all(|diagnostic| {
        diagnostic.code() != powerio_tx::diagnostics::codes::READ_GOC3_RETAINED_SOURCE_ONLY.code
    }));
}

#[test]
fn goc3_reports_each_known_optional_field_that_only_retained_source_preserves() {
    let mut document: serde_json::Value =
        serde_json::from_str(&fixture("tests/data/goc3_small.json")).unwrap();
    document["network"]["general"]["season"] = serde_json::json!("summer");
    document["network"]["bus"][0]["con_loss_factor"] = serde_json::json!(0.0);
    document["network"]["development"] = serde_json::json!({"study": "candidate"});
    document["time_series_input"]["development"] = serde_json::json!({"forecast": "candidate"});
    let module = decode_goc3_problem(memory(
        "goc3-optional.json",
        &serde_json::to_string(&document).unwrap(),
    ))
    .unwrap();
    assert!(
        !module.value.network().buses()[0]
            .extras
            .contains_key("con_loss_factor")
    );
    let diagnostics: Vec<_> = module
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code() == powerio_tx::diagnostics::codes::READ_GOC3_RETAINED_SOURCE_ONLY.code
        })
        .collect();
    assert_eq!(diagnostics.len(), 4, "{diagnostics:?}");
    assert_eq!(
        diagnostics[0].message(),
        "optional network.general fields retained in source only: season"
    );
    assert_eq!(
        diagnostics[1].message(),
        "legacy network.bus.con_loss_factor retained in source only for 1 buses (sample uids: bus_00); GO Challenge 3 data format 1.1.1 removed this field"
    );
    assert_eq!(
        diagnostics[2].message(),
        "optional network.development retained in source only"
    );
    assert_eq!(
        diagnostics[3].message(),
        "optional time_series_input.development retained in source only"
    );
}

#[test]
fn goc3_reports_official_optional_fields_without_typed_slots() {
    let mut document: serde_json::Value =
        serde_json::from_str(&fixture("tests/data/goc3_small.json")).unwrap();
    document["network"]["bus"][0]["area"] = serde_json::json!("north");
    document["network"]["bus"][0]["city"] = serde_json::json!("Ann Arbor");
    document["network"]["bus"][0]["longitude"] = serde_json::json!(-83.743);
    document["network"]["simple_dispatchable_device"][0]["description"] =
        serde_json::json!("producer description");
    document["network"]["simple_dispatchable_device"][1]["description"] =
        serde_json::json!("consumer description");
    document["network"]["simple_dispatchable_device"][1]["vm_setpoint"] = serde_json::json!(1.0);
    document["network"]["simple_dispatchable_device"][1]["nameplate_capacity"] =
        serde_json::json!(2.0);

    let module = decode_goc3_problem(memory(
        "goc3-unmapped-optionals.json",
        &serde_json::to_string(&document).unwrap(),
    ))
    .unwrap();
    let untyped: Vec<_> = module
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code()
                == powerio_tx::diagnostics::codes::READ_GOC3_OPTIONAL_FIELD_UNTYPED.code
        })
        .map(powerio_core::Diagnostic::message)
        .collect();
    assert_eq!(untyped.len(), 6, "{untyped:?}");
    assert!(untyped.iter().any(|message| {
        message.contains("`network.bus.area` has no typed PowerIO field")
            && message.contains("retained in `Bus.extras`")
    }));
    assert!(untyped.iter().any(|message| {
        message.contains("`network.bus.city` has no typed PowerIO field")
            && message.contains("retained in `Bus.extras`")
    }));
    assert!(untyped.iter().any(|message| {
        message.contains("`network.bus.longitude` without `latitude`")
            && message.contains("retained in `Bus.extras`")
    }));
    assert!(untyped.iter().any(|message| {
        message.contains("simple_dispatchable_device.description` on a consumer")
            && message.contains("retained in `Load.extras`")
    }));
    assert!(untyped.iter().any(|message| {
        message.contains("simple_dispatchable_device.vm_setpoint` on a consumer")
            && message.contains("retained in `Load.extras`")
    }));
    assert!(untyped.iter().any(|message| {
        message.contains("simple_dispatchable_device.nameplate_capacity` on a consumer")
            && message.contains("retained in `Load.extras`")
    }));
    let source_only: Vec<_> = module
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code() == powerio_tx::diagnostics::codes::READ_GOC3_RETAINED_SOURCE_ONLY.code
        })
        .map(powerio_core::Diagnostic::message)
        .collect();
    assert_eq!(source_only.len(), 1, "{source_only:?}");
    assert!(source_only[0].contains("description` on a producer"));
    assert!(source_only[0].contains("retained in the original source only"));
}

#[test]
fn goc3_development_fields_must_be_objects() {
    for parent in ["network", "time_series_input"] {
        let mut document: serde_json::Value =
            serde_json::from_str(&fixture("tests/data/goc3_small.json")).unwrap();
        document[parent]["development"] = serde_json::json!([]);
        let error = decode_goc3_problem(memory(
            "goc3-invalid-development.json",
            &serde_json::to_string(&document).unwrap(),
        ))
        .unwrap_err();
        assert!(
            error.to_string().contains(&format!("{parent}.development")),
            "{error}"
        );
    }

    let mut document: serde_json::Value =
        serde_json::from_str(&fixture("tests/data/goc3_small.json")).unwrap();
    document["reliability"]["development"] = serde_json::json!({});
    let error = decode_goc3_problem(memory(
        "goc3-undocumented-development.json",
        &serde_json::to_string(&document).unwrap(),
    ))
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("`reliability` contains unknown field `development`"),
        "{error}"
    );
}

#[test]
fn goc3_network_extraction_reports_the_discard() {
    let text = fixture("tests/data/goc3_small.json");
    let module = decode_goc3_problem(memory("goc3_small.json", &text)).unwrap();
    let (_opf, diagnostics) = module.value.to_dc_opf().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message().contains("scheduling")),
        "{diagnostics:?}"
    );
}

#[test]
fn goc3_rejects_a_malformed_document_and_retains_the_source() {
    let error = decode_goc3_problem(memory("broken.json", "{\"network\": {}}")).unwrap_err();
    assert!(error.retained_source().is_some());
}

#[test]
fn goc3_input_follows_the_official_object_and_array_shapes() {
    let fixture: serde_json::Value =
        serde_json::from_str(&fixture("tests/data/goc3_small.json")).unwrap();
    let cases: [(&str, JsonMutation); 3] = [
        ("unknown-root-field", |document| {
            document["extra"] = serde_json::json!(true);
        }),
        ("wrong-optional-type", |document| {
            document["network"]["general"]["season"] = serde_json::json!(3);
        }),
        ("uid-keyed-section", |document| {
            let buses = document["network"]["bus"].take();
            document["network"]["bus"] = serde_json::json!({"bus_00": buses[0].clone()});
        }),
    ];
    for (name, mutate) in cases {
        let mut document = fixture.clone();
        mutate(&mut document);
        let source = memory(name, &serde_json::to_string(&document).unwrap());
        assert!(decode_goc3_problem(source).is_err(), "{name}");
    }
}

#[test]
fn goc3_solution_uses_the_instance_identity_and_time_axis() {
    let text = fixture("tests/data/goc3_small.json");
    let instance = decode_goc3_problem(memory("goc3_small.json", &text)).unwrap();
    let source = memory(
        "solution.json",
        &serde_json::to_string(&goc3_output()).unwrap(),
    );
    let solution = __parse_goc3_output_buffer(
        std::sync::Arc::new(instance.value),
        &source.primary_buffer().unwrap(),
    )
    .unwrap();

    let network = solution.network_outputs();
    assert_eq!(network.bus_vm, vec![vec![1.0, 0.99], vec![1.01, 0.98]]);
    assert_eq!(network.bus_va, vec![vec![0.0, -0.1], vec![0.0, -0.2]]);
    assert_eq!(network.shunt_step, vec![vec![1], vec![2]]);
    assert_eq!(
        network.ac_line_on_status,
        vec![vec![true, true], vec![true, false]]
    );
    let devices = solution.device_outputs();
    assert_eq!(devices.on_status, vec![vec![true, true], vec![true, false]]);
    assert_eq!(
        devices.shutdown_status,
        vec![vec![false, false], vec![false, true]]
    );
    assert_eq!(devices.startup_status, vec![vec![false; 2]; 2]);
}

#[test]
fn goc3_solution_rejects_unknown_devices_and_wrong_time_lengths() {
    let text = fixture("tests/data/goc3_small.json");
    let instance = decode_goc3_problem(memory("goc3_small.json", &text)).unwrap();
    let mut unknown = goc3_output();
    unknown["time_series_output"]["simple_dispatchable_device"][0]["uid"] =
        serde_json::json!("missing");
    let source = memory("unknown.json", &serde_json::to_string(&unknown).unwrap());
    let error = __parse_goc3_output_buffer(
        std::sync::Arc::new(instance.value.clone()),
        &source.primary_buffer().unwrap(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown uid `missing`"));

    let mut wrong_length = goc3_output();
    wrong_length["time_series_output"]["bus"][0]["vm"] = serde_json::json!([1.0]);
    let source = memory(
        "wrong-length.json",
        &serde_json::to_string(&wrong_length).unwrap(),
    );
    let error = __parse_goc3_output_buffer(
        std::sync::Arc::new(instance.value),
        &source.primary_buffer().unwrap(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the instance states 2 intervals")
    );
}

#[test]
fn goc3_output_rejects_missing_unknown_and_nonbinary_fields() {
    let instance = decode_goc3_problem(memory(
        "goc3_small.json",
        &fixture("tests/data/goc3_small.json"),
    ))
    .unwrap();
    let cases: [(&str, JsonMutation); 3] = [
        ("missing-section", |value: &mut serde_json::Value| {
            value["time_series_output"]
                .as_object_mut()
                .unwrap()
                .remove("shunt");
        }),
        ("nonbinary", |value: &mut serde_json::Value| {
            value["time_series_output"]["ac_line"][0]["on_status"][0] = serde_json::json!(2);
        }),
        ("nonofficial", |value: &mut serde_json::Value| {
            value["time_series_output"]["simple_dispatchable_device"][0]["startup_status"] =
                serde_json::json!([0, 0]);
        }),
    ];
    for (name, mutate) in cases {
        let mut document = goc3_output();
        mutate(&mut document);
        let source = memory(name, &serde_json::to_string(&document).unwrap());
        assert!(
            __parse_goc3_output_buffer(
                std::sync::Arc::new(instance.value.clone()),
                &source.primary_buffer().unwrap(),
            )
            .is_err(),
            "{name}"
        );
    }
}

#[test]
fn goc3_output_emission_uses_the_official_fields_and_round_trips() {
    let instance = decode_goc3_problem(memory(
        "goc3_small.json",
        &fixture("tests/data/goc3_small.json"),
    ))
    .unwrap();
    let source = memory(
        "solution.json",
        &serde_json::to_string(&goc3_output()).unwrap(),
    );
    let solution = __parse_goc3_output_buffer(
        std::sync::Arc::new(instance.value),
        &source.primary_buffer().unwrap(),
    )
    .unwrap();
    let text = __emit_goc3_output(&solution).unwrap();
    let emitted: serde_json::Value = serde_json::from_str(&text).unwrap();
    let checked_fixture: serde_json::Value =
        serde_json::from_str(&fixture("../tests/data/goc3/goc3_small_solution.json")).unwrap();
    assert_eq!(emitted, checked_fixture);
    let output = emitted["time_series_output"].as_object().unwrap();
    assert_eq!(output.len(), 6);
    assert!(output.contains_key("bus"));
    assert!(output.contains_key("shunt"));
    assert!(output.contains_key("simple_dispatchable_device"));
    assert!(output.contains_key("ac_line"));
    assert!(output.contains_key("two_winding_transformer"));
    assert!(output.contains_key("dc_line"));
    let device = &output["simple_dispatchable_device"][0];
    assert!(device.get("startup_status").is_none());
    assert!(device.get("shutdown_status").is_none());
    assert_eq!(device["on_status"], serde_json::json!([1, 1]));
    assert_eq!(output["shunt"][0]["step"], serde_json::json!([1, 2]));

    let emitted_source = memory("emitted.json", &text);
    let reparsed = __parse_goc3_output_buffer(
        solution.shared_instance(),
        &emitted_source.primary_buffer().unwrap(),
    )
    .unwrap();
    assert_eq!(reparsed.network_outputs(), solution.network_outputs());
    assert_eq!(reparsed.device_outputs(), solution.device_outputs());
}

#[test]
fn goc3_output_emission_requires_complete_matching_instance_tables() {
    let base = decode_goc3_problem(memory(
        "goc3_small.json",
        &fixture("tests/data/goc3_small.json"),
    ))
    .unwrap()
    .value;
    let cases: [(&str, ScucInputMutation, &str); 5] = [
        (
            "missing-device",
            |inputs| {
                inputs.devices.pop();
            },
            "simple_dispatchable_device table is missing",
        ),
        (
            "missing-shunt",
            |inputs| {
                inputs.shunts.pop();
            },
            "shunt table is missing",
        ),
        (
            "missing-branch",
            |inputs| {
                inputs.branch_switching_costs.pop();
            },
            "switching cost table is missing",
        ),
        (
            "missing-transformer-control",
            |inputs| {
                inputs.transformer_controls.pop();
            },
            "two_winding_transformer control table is missing",
        ),
        (
            "wrong-branch-kind",
            |inputs| {
                let uid = inputs.branch_switching_costs[0].id.local_id().to_owned();
                inputs.branch_switching_costs[0].id =
                    powerio_core::ComponentId::new("transformer", uid).unwrap();
            },
            "switching cost table is missing branch/",
        ),
    ];

    for (name, mutate, expected) in cases {
        let mut inputs = base.inputs().clone();
        mutate(&mut inputs);
        let instance = powerio_prob::AcScucInstance::new(base.network().clone(), inputs).unwrap();
        let solution = powerio_prob::AcScucSolution::new(
            std::sync::Arc::new(instance),
            Termination::NotReported,
            powerio_prob::ScucNetworkOutputs::default(),
            powerio_prob::ScucDeviceOutputs::default(),
            None,
        )
        .unwrap();
        let error = __emit_goc3_output(&solution).unwrap_err();
        assert!(error.to_string().contains(expected), "{name}: {error}");
    }
}

#[test]
fn scuc_solution_rejects_wrong_row_widths_and_nonfinite_values() {
    let instance = decode_goc3_problem(memory(
        "goc3_small.json",
        &fixture("tests/data/goc3_small.json"),
    ))
    .unwrap()
    .value;
    let periods = instance.inputs().interval_durations.len();

    let mut wrong_width = powerio_prob::ScucNetworkOutputs::default();
    wrong_width.bus_vm = vec![vec![1.0]; periods];
    let error = powerio_prob::AcScucSolution::new(
        std::sync::Arc::new(instance.clone()),
        Termination::NotReported,
        wrong_width,
        powerio_prob::ScucDeviceOutputs::default(),
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("instance states 2 components"));

    let mut nonfinite = powerio_prob::ScucNetworkOutputs::default();
    nonfinite.bus_vm = vec![vec![1.0, f64::NAN]; periods];
    let error = powerio_prob::AcScucSolution::new(
        std::sync::Arc::new(instance),
        Termination::NotReported,
        nonfinite,
        powerio_prob::ScucDeviceOutputs::default(),
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("is not finite"));
}

/// The problem format readers declare their format on the source record, so
/// a stored module's descriptor carries the token and same format emission can
/// default to it.
#[test]
fn goc3_problem_sources_declare_their_format() {
    let goc3 = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/goc3/goc3_small.json"
    ))
    .unwrap();
    let module = decode_goc3_problem(
        powerio_core::Source::from_memory("goc3_small.json", goc3.into_bytes()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        module
            .sources()
            .first()
            .and_then(|s| s.format())
            .map(powerio_core::FormatId::as_str),
        Some("goc3-json")
    );
}
