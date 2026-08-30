//! The byte exact echo tier extends past the two network kinds: any module
//! that retains a source already in the requested format writes it back
//! unchanged, instead of refusing every kind but `BalancedNetwork` and
//! `MulticonductorNetwork` with `REQUEST.WRITE.UNSUPPORTED_VALUE_KIND`.

use std::collections::BTreeMap;

use powerio::{PioValue, emit, write_module_str};
use powerio_core::{Destination, Source, WrittenOutput};

/// A minimal Egret `ModelData` document with `system.time_keys`, so
/// `powerio::parse` routes it to the time series reader instead of the
/// balanced hub. No declared format: the common case for a bare `.json`
/// source (`Source::open`/`Source::from_bytes` with no `.with_format`),
/// which is also the reclassify-by-content branch of the echo check.
const EGRET_TIME_SERIES: &str = r#"{
    "elements": {
        "bus": {"1": {"matpower_bustype": "ref", "base_kv": 138.0},
                "2": {"matpower_bustype": "PQ", "base_kv": 138.0}},
        "load": {"load_1": {"bus": "2",
            "p_load": {"data_type": "time_series", "values": [10.0, 20.0]},
            "q_load": 3.0}},
        "generator": {"1": {"bus": "1", "pg": 12.0, "qg": 0.0,
            "p_min": 0.0, "p_max": 50.0, "q_min": -10.0, "q_max": 10.0}},
        "branch": {"1": {"from_bus": "1", "to_bus": "2",
            "resistance": 0.01, "reactance": 0.1, "charging_susceptance": 0.0,
            "rating_long_term": 100.0, "rating_short_term": 100.0,
            "rating_emergency": 100.0, "transformer_phase_shift": 0.0}}
    },
    "system": {"baseMVA": 100.0, "time_keys": ["t1", "t2"]}
}"#;

#[test]
fn a_time_series_kind_echoes_its_retained_source_on_a_same_format_write() {
    let source =
        powerio_core::Source::from_bytes("case.json", EGRET_TIME_SERIES.as_bytes().to_vec())
            .unwrap();
    let module = powerio::parse(source).unwrap();
    assert!(
        matches!(module.value(), PioValue::BalancedNetworkTimeSeries(_)),
        "the time_keys document should route to the time series kind"
    );

    let (text, diagnostics) = write_module_str(&module, "egret-json").unwrap();
    assert_eq!(
        text, EGRET_TIME_SERIES,
        "the write must echo the source byte for byte"
    );
    assert!(diagnostics.is_empty());
}

#[test]
fn a_time_series_kind_still_refuses_a_format_its_retained_source_is_not() {
    let source =
        powerio_core::Source::from_bytes("case.json", EGRET_TIME_SERIES.as_bytes().to_vec())
            .unwrap();
    let module = powerio::parse(source).unwrap();

    let error = write_module_str(&module, "matpower").unwrap_err();
    assert!(error.to_string().contains("no matpower writer"), "{error}");
}

fn memory_directory(result: &powerio_core::WriteResult) -> BTreeMap<String, Vec<u8>> {
    let WrittenOutput::Memory { artifacts } = result.output() else {
        panic!("a memory destination must return memory artifacts");
    };
    artifacts
        .iter()
        .map(|artifact| {
            (
                artifact.name().as_str().to_owned(),
                artifact.bytes().to_vec(),
            )
        })
        .collect()
}

#[test]
fn a_pypsa_network_time_series_emits_its_complete_directory_byte_exactly() {
    const FILES: [(&str, &[u8]); 7] = [
        ("network.csv", b"name\nseries\n"),
        ("buses.csv", b"name,v_nom\nB1,138.0\nB2,138.0\n"),
        ("loads.csv", b"name,bus,p_set,q_set\nL1,B2,5.0,1.0\n"),
        (
            "generators.csv",
            b"name,bus,control,p_nom,p_set\nG1,B1,Slack,100.0,12.0\n",
        ),
        ("snapshots.csv", b",snapshot\n0,now\n1,later\n"),
        ("loads-p_set.csv", b"snapshot,L1\nnow,10.0\nlater,20.0\n"),
        // Outside the electrical profile and not decoded by the reader. The
        // same format echo still owns the complete source inventory, including
        // nested binary artifacts.
        ("extras/vendor.bin", b"\0\xffpowerio\r\n"),
    ];

    let root = tempfile::tempdir().unwrap();
    let source_dir = root.path().join("source");
    std::fs::create_dir_all(source_dir.join("extras")).unwrap();
    for (name, bytes) in FILES {
        std::fs::write(source_dir.join(name), bytes).unwrap();
    }

    let module = powerio::parse_file(&source_dir).expect("the PyPSA series parses");
    assert!(matches!(
        module.value(),
        PioValue::BalancedNetworkTimeSeries(_)
    ));
    assert_eq!(
        module
            .source()
            .and_then(Source::format)
            .map(powerio_core::FormatId::as_str),
        Some("pypsa-csv")
    );

    let result = emit(&module, "pypsa-csv", Destination::memory("copy").unwrap())
        .expect("same format directory emission succeeds");
    assert!(result.diagnostics().is_empty());

    let actual = memory_directory(&result);
    let expected: BTreeMap<_, _> = FILES
        .into_iter()
        .map(|(name, bytes)| (format!("copy/{name}"), bytes.to_vec()))
        .collect();
    assert_eq!(actual, expected);
}

#[cfg(feature = "gridfm")]
#[test]
fn a_gridfm_scenario_set_emits_its_complete_directory_byte_exactly() {
    let case = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let base = powerio_tx::parse(Source::open(case).unwrap())
        .expect("case9 parses")
        .into_value();
    let mut varied = base.clone();
    varied.loads_mut()[0].p += 5.0;
    let snapshots = [
        powerio_matrix::GridfmSnapshot::new(&base, 3),
        powerio_matrix::GridfmSnapshot::new(&varied, 4),
    ];
    let source_root = tempfile::tempdir().unwrap();
    powerio_matrix::write_gridfm_batch(
        &snapshots,
        source_root.path(),
        &powerio_matrix::GridfmOptions::default(),
    )
    .expect("the GridFM fixture writes");

    let module = powerio::parse_file(source_root.path()).expect("the GridFM dataset parses");
    assert!(matches!(
        module.value(),
        PioValue::BalancedNetworkScenarioSet(_)
    ));
    let retained = module
        .source()
        .expect("the parsed module retains its source");
    let expected: BTreeMap<_, _> = retained
        .entry_names()
        .unwrap()
        .into_iter()
        .map(|name| {
            let bytes = retained.buffer(&name).unwrap().bytes().to_vec();
            (format!("copy/{}", name.as_str()), bytes)
        })
        .collect();

    let result = emit(&module, "gridfm", Destination::memory("copy").unwrap())
        .expect("same format directory emission succeeds");
    assert!(result.diagnostics().is_empty());
    assert_eq!(memory_directory(&result), expected);
}
