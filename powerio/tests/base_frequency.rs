//! System base frequency carries through the formats that record it and is
//! reported as a fidelity loss by the ones that don't.
//!
//! `Network::base_frequency` (50 or 60 Hz, default 60) threads through PSS/E
//! `BASFRQ` and pandapower `f_hz`. MATPOWER, PowerModels, egret, and PowerWorld
//! have no frequency field, so a non-default value writes with a warning rather
//! than silently reading back as 60 Hz.

// Frequencies are exact decimal values parsed from the fixtures (50.0, 60.0); bit
// equality is the intended assertion.
#![allow(clippy::float_cmp)]

use powerio::{
    DEFAULT_BASE_FREQUENCY, Network, TargetFormat, parse_pandapower_json, parse_psse, parse_str,
    write_as, write_pandapower_json, write_psse,
};

/// A 50 Hz PSS/E header round-trips: the reader takes `BASFRQ`, the writer emits
/// it, and a second read recovers it.
#[test]
fn psse_base_frequency_round_trips() {
    let raw = "0, 100.00, 33, 0, 0, 50.00 / fifty hertz\n\
        CASE\nCOMMENT\n\
        1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        2,'B2          ', 138.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        0 / END OF BUS DATA, BEGIN LOAD DATA\n\
        Q\n";
    let net = parse_psse(raw).unwrap();
    assert_eq!(net.base_frequency, 50.0);

    let text = write_psse(&net).text;
    let reparsed = parse_psse(&text).unwrap();
    assert_eq!(reparsed.base_frequency, 50.0);
}

/// A PSS/E header without `BASFRQ` (the short `SBASE, title` form) defaults to 60.
#[test]
fn psse_missing_base_frequency_defaults_to_sixty() {
    let raw = "0, 100.00\nCASE\nCOMMENT\n\
        1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        0 / END OF BUS DATA, BEGIN LOAD DATA\nQ\n";
    let net = parse_psse(raw).unwrap();
    assert_eq!(net.base_frequency, DEFAULT_BASE_FREQUENCY);
}

/// pandapower labels the file with `f_hz` and computes line charging against it,
/// so a 50 Hz network round-trips both the frequency and the exact susceptance.
#[test]
fn pandapower_f_hz_round_trips_with_line_charging() {
    // Start from a PSS/E case (default 60 Hz) carrying one charging branch, then
    // relabel it 50 Hz and confirm the pandapower hop preserves both.
    let raw = "0, 100.00, 33, 0, 0, 60.00 / x\nCASE\nCOMMENT\n\
        1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        2,'B2          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        0 / END OF BUS DATA, BEGIN LOAD DATA\n\
        0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\n\
        0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA\n\
        0 / END OF GENERATOR DATA, BEGIN BRANCH DATA\n\
        1,2,'1 ',0.01,0.05,0.02,100.0,0.0,0.0,0,0,0,0,1,1,0,1,1\n\
        0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA\n\
        0 / END OF TRANSFORMER DATA, BEGIN AREA DATA\nQ\n";
    let mut net = parse_psse(raw).unwrap();
    net.base_frequency = 50.0;
    let b0 = net.branches[0].b;

    let pp = write_pandapower_json(&net).text;
    let back = parse_pandapower_json(&pp).unwrap().network;

    assert_eq!(back.base_frequency, 50.0);
    assert!(
        (back.branches[0].b - b0).abs() < 1e-9,
        "line charging changed across the f_hz hop: {} != {b0}",
        back.branches[0].b
    );
}

/// Writing a non-default frequency to a format with no frequency field warns,
/// rather than silently dropping 50 Hz to the 60 Hz default.
#[test]
fn dropped_frequency_warns_for_formats_without_a_field() {
    let raw = "0, 100.00, 33, 0, 0, 50.00 / x\nCASE\nCOMMENT\n\
        1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        0 / END OF BUS DATA, BEGIN LOAD DATA\nQ\n";
    let net = parse_psse(raw).unwrap();

    for target in [
        TargetFormat::Matpower,
        TargetFormat::PowerModelsJson,
        TargetFormat::EgretJson,
        TargetFormat::PowerWorld,
    ] {
        let conv = write_as(&net, target).unwrap();
        assert!(
            conv.warnings.iter().any(|w| w.contains("frequency")),
            "{target:?} should warn that it drops the 50 Hz label, got {:?}",
            conv.warnings
        );
    }
    // PSS/E and pandapower carry it, so no frequency warning there.
    for target in [TargetFormat::Psse { rev: 33 }, TargetFormat::PandapowerJson] {
        let conv = write_as(&net, target).unwrap();
        assert!(
            !conv.warnings.iter().any(|w| w.contains("frequency")),
            "{target:?} carries the frequency and should not warn, got {:?}",
            conv.warnings
        );
    }
}

/// The JSON transport carries the field, and JSON written before it existed
/// (without the key) deserializes to the 60 Hz default.
#[test]
fn json_transport_round_trips_and_defaults() {
    let raw = "0, 100.00, 33, 0, 0, 50.00 / x\nCASE\nCOMMENT\n\
        1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        0 / END OF BUS DATA, BEGIN LOAD DATA\nQ\n";
    let net = parse_psse(raw).unwrap();
    let json = net.to_json().unwrap();
    assert_eq!(Network::from_json(&json).unwrap().base_frequency, 50.0);

    // A JSON document with no base_frequency key falls back to the default.
    let without: serde_json::Value = {
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v.as_object_mut().unwrap().remove("base_frequency");
        v
    };
    let restored = Network::from_json(&without.to_string()).unwrap();
    assert_eq!(restored.base_frequency, DEFAULT_BASE_FREQUENCY);
}

/// `parse_str` exposes the same threading for in-memory PSS/E text.
#[test]
fn parse_str_psse_reads_frequency() {
    let raw = "0, 100.00, 33, 0, 0, 50.00 / x\nCASE\nCOMMENT\n\
        1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9\n\
        0 / END OF BUS DATA, BEGIN LOAD DATA\nQ\n";
    let parsed = parse_str(raw, "psse").unwrap();
    assert_eq!(parsed.network.base_frequency, 50.0);
}
