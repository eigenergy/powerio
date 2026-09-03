//! #440: fixture-based classification pins for the families the JSON header
//! rewrite (typed header struct in place of a full `serde_json::Value`
//! materialization) touches. Read straight from `tests/data` rather than
//! inline literals, and independent of `classify_corpus.rs`'s exhaustive
//! walk, so this stays a clean before/after comparison regardless of that
//! walk's own state.

use std::path::{Path, PathBuf};

use powerio_tx::TargetFormat;
use powerio_tx::format::routing::{
    Detection, DistributionFormat, JsonClass, SourceFormat, TransmissionFormat, classify_json_text,
};

mod helpers;
use helpers::emit_value;

fn data_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(data_root().join(rel)).unwrap_or_else(|error| panic!("{rel}: {error}"))
}

#[test]
fn pandapower_fixture_classifies_as_pandapower() {
    assert_eq!(
        classify_json_text(&read("pandapower/example.json")),
        JsonClass::Case(Detection::Known(SourceFormat::Transmission(
            TransmissionFormat::PandapowerJson
        )))
    );
}

#[test]
fn egret_fixtures_classify_as_egret() {
    for name in ["case9.json", "case14.json", "case30.json", "dcline3.json"] {
        assert_eq!(
            classify_json_text(&read(&format!("egret/{name}"))),
            JsonClass::Case(Detection::Known(SourceFormat::Transmission(
                TransmissionFormat::EgretJson
            ))),
            "{name}"
        );
    }
}

#[test]
fn bmopf_fixtures_classify_as_bmopf() {
    for name in ["example_ieee13.json", "example_enwl_n1_f2.json"] {
        assert_eq!(
            classify_json_text(&read(&format!("dist/bmopf/{name}"))),
            JsonClass::Case(Detection::Known(SourceFormat::Distribution(
                DistributionFormat::BmopfJson
            ))),
            "{name}"
        );
    }
}

#[test]
fn powerio_ir_classifies_as_module() {
    assert_eq!(
        classify_json_text(r#"{"schema":"powerio.module","version":"0.11.0"}"#),
        JsonClass::Module
    );
}

/// PowerModels JSON has no vendored fixture file (its markers are `baseMVA`,
/// `branch`, `gen`, `gencost` at top level), so this converts a MATPOWER case
/// once instead, matching how the parse benchmark builds its own PowerModels
/// JSON text.
#[test]
fn a_powermodels_json_case_classifies_as_powermodels() {
    let net = powerio_tx::parse(powerio_core::Source::open(data_root().join("case118.m")).unwrap())
        .unwrap()
        .into_value();
    let text = emit_value(&net, TargetFormat::PowerModelsJson)
        .unwrap()
        .text;
    assert_eq!(
        classify_json_text(&text),
        JsonClass::Case(Detection::Known(SourceFormat::Transmission(
            TransmissionFormat::PowerModelsJson
        )))
    );
}
