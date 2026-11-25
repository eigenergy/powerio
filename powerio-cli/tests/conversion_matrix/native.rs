//! Opt-in native seed evidence. The regular matrix still uses its own corpus.

use powerio_core::{Destination, EmittedOutput, PioModule, Source};
use serde_json::{Value, json};

use super::*;

const CASE14: &str = "case14.m";
const IEEE13: &str = "dist/opendss/ieee13/IEEE13Nodeckt.dss";

struct Measurement {
    checks: BTreeMap<&'static str, Value>,
    cell: Cell,
}

impl Measurement {
    fn run<T>(&mut self, check: &'static str, result: Result<T, String>) -> Option<T> {
        match result {
            Ok(value) => {
                self.checks.insert(check, json!({"status": "pass"}));
                Some(value)
            }
            Err(error) => {
                self.checks.insert(
                    check,
                    json!({"status": "fail", "detail": sanitize_report_text(&error)}),
                );
                None
            }
        }
    }

    fn record_readback(&mut self) {
        let failures = self.cell.failures.join("\n");
        let readback = if self.cell.readback_ok {
            Ok(())
        } else {
            Err(failures.clone())
        };
        if self.run("powerio_readback", readback).is_some() {
            self.run(
                "matrix_invariants",
                if failures.is_empty() {
                    Ok(())
                } else {
                    Err(failures)
                },
            );
        }
    }
}

fn measure(run: impl FnOnce(&mut Measurement) -> Option<()>) -> Value {
    let mut measurement = Measurement {
        checks: BTreeMap::new(),
        cell: Cell::default(),
    };
    run(&mut measurement);
    let mut report = evidence::drill_down(&measurement.cell);
    report.insert("checks".into(), json!(measurement.checks));
    report.insert(
        "limitations".into(),
        json!(evidence::limitations(&measurement.cell)),
    );
    Value::Object(report)
}

fn retained_files<T: Clone + Into<powerio::PioValue>>(
    module: &PioModule<T>,
    format: &str,
    expected: &[(&Path, &str)],
) -> Result<(), String> {
    let destination = Destination::memory("case").map_err(|err| err.to_string())?;
    let result = powerio::emit(module, format, destination).map_err(|err| err.to_string())?;
    let EmittedOutput::Memory { artifacts } = result.output() else {
        return Err("unexpected path output".into());
    };
    let mut files: BTreeMap<_, _> = artifacts
        .iter()
        .map(|artifact| (artifact.name().as_str(), artifact.bytes()))
        .collect();
    // A DSS project wraps the retained files with a generated entrypoint.
    if format == "dss" {
        let entrypoint = b"Redirect \"source/IEEE13Nodeckt.dss\"\n";
        if files.remove("case/case.dss") != Some(entrypoint.as_slice()) {
            return Err("the replay entrypoint does not redirect to the retained deck".into());
        }
    }
    for (fixture, name) in expected {
        let bytes = std::fs::read(fixture).map_err(|err| err.to_string())?;
        match files.remove(name) {
            None => return Err(format!("{name}: emitted artifact is missing")),
            Some(actual) if actual != bytes => return Err(format!("{name}: bytes changed")),
            Some(_) => {}
        }
    }
    if !files.is_empty() {
        return Err(format!("unexpected retained artifacts: {:?}", files.keys()));
    }
    Ok(())
}

fn case14(out: &Path, measurement: &mut Measurement) -> Option<()> {
    let target = TRANSMISSION_FORMATS[0];
    assert_eq!(target.token, "matpower");
    let path = data(CASE14);
    let parsed = measurement.run(
        "source_parse",
        Source::open(&path)
            .map_err(|err| err.to_string())
            .and_then(|source| parse_transmission_source(source, target.token)),
    )?;
    let payload = TransmissionPayload {
        label: "case14",
        network: parsed.network,
        module: parsed.module,
        emitted: None,
        parse_findings: parsed.diagnostics,
    };
    measurement
        .cell
        .record(payload.label, Leg::Source, &payload.parse_findings);
    measurement.run(
        "retained_files",
        retained_files(&payload.module, target.token, &[(&path, "case")]),
    );
    let conversion = measurement.run(
        "fresh_write",
        emit_transmission(&PioModule::new(payload.network.clone()), target),
    )?;
    let Emitted::Document(text) = &conversion.emitted else {
        unreachable!("MATPOWER writes one document");
    };
    std::fs::write(out.join(CASE14), text).unwrap();
    validate_transmission_emission(&payload, target, &mut measurement.cell, &conversion);
    measurement.record_readback();
    Some(())
}

fn ieee13(out: &Path, measurement: &mut Measurement) -> Option<()> {
    let target = DISTRIBUTION_FORMATS[0];
    let path = data(IEEE13);
    let parsed = measurement.run(
        "source_parse",
        load_distribution_case(&path, target.token).map_err(|err| err.to_string()),
    )?;
    let payload = DistributionPayload {
        label: "IEEE 13",
        network: parsed.network,
        module: parsed.module,
        text: std::fs::read_to_string(&path).unwrap(),
        parse_findings: parsed.diagnostics,
    };
    measurement
        .cell
        .record(payload.label, Leg::Source, &payload.parse_findings);
    let lines = path.with_file_name("IEEELineCodes.DSS");
    let coords = path.with_file_name("IEEE13Node_BusXY.csv");
    measurement.run(
        "retained_files",
        retained_files(
            &payload.module,
            target.token,
            &[
                (&path, "case/source/IEEE13Nodeckt.dss"),
                (&lines, "case/source/IEEELineCodes.DSS"),
                (&coords, "case/source/IEEE13Node_BusXY.csv"),
            ],
        ),
    );
    let conversion = measurement.run(
        "fresh_write",
        emit_distribution_value(&payload.network, target.target),
    )?;
    let directory = out.join("ieee13");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("input.dss"), &conversion.text).unwrap();
    for (file, text) in &conversion.sidecars {
        std::fs::write(directory.join(file), text).unwrap();
    }
    validate_distribution_emission(
        &payload,
        target.name,
        target,
        false,
        &mut measurement.cell,
        &conversion,
    );
    measurement.record_readback();
    Some(())
}

#[test]
#[ignore = "run by evals/validation/native_fidelity.py"]
fn write_native_seed_evidence() {
    let out = PathBuf::from(std::env::var_os("POWERIO_NATIVE_FIDELITY_DIR").unwrap());
    assert!(out.is_absolute());
    let report = json!({
        "provenance": evidence::provenance(),
        "powerio_version": env!("CARGO_PKG_VERSION"),
        "measurements": {
            "case14": measure(|measurement| case14(&out, measurement)),
            "ieee13": measure(|measurement| ieee13(&out, measurement)),
        }
    });
    std::fs::write(
        out.join("matrix.json"),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
}
