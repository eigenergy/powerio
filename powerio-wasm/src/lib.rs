//! In-memory browser conversion using the native PowerIO parser and writers.

use std::collections::BTreeMap;
use std::sync::Arc;

use powerio::{ArtifactPath, Destination, Diagnostic, EmittedOutput, PioModule, PioValue, Source};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn capabilities() -> String {
    let formats: Vec<_> = powerio::grid_formats()
        .map(|entry| {
            json!({
                "token": entry.format.token,
                "label": entry.label,
                "family": entry.family,
                "canRead": entry.can_read,
                // The converter offers no GridLAB-D target: an ordinary OpenDSS
                // feeder states no explicit conductor and ground interpretation
                // for it, so the bulk targets stay as they were. The catalog
                // still lists both profiles for reading and family detection.
                "canEmit": entry.format.can_emit
                    && !matches!(
                        entry.format.token,
                        "bmopf-json" | "gridlabd" | "gridlabd-arras"
                    ),
                "isDirectory": entry.format.is_directory,
                "extension": entry.format.extension,
                "requiresValueType": (entry.format.token == "goc3-json")
                    .then_some("powerio.AcScucSolution"),
            })
        })
        .collect();
    json!({ "version": powerio::VERSION, "formats": formats }).to_string()
}

fn diagnostics(entries: &[Diagnostic]) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| {
            json!({
                "code": entry.code(),
                "severity": entry.severity().as_str(),
                "message": entry.message(),
                "target": entry.target(),
                "suggestedAction": entry.suggested_action(),
                "spans": entry.spans().iter().map(|span| json!({
                    "source": span.source().as_str(),
                    "start": span.byte_start(),
                    "end": span.byte_end(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn failure(error: &powerio::Error) -> String {
    json!({"ok": false, "diagnostics": diagnostics(error.diagnostics())}).to_string()
}

/// One case and its parsed module, retained until the worker releases it.
#[wasm_bindgen]
pub struct Conversion {
    name: String,
    files: BTreeMap<String, Arc<[u8]>>,
    module: Option<PioModule<PioValue>>,
    artifacts: Vec<powerio::MemoryArtifact>,
}

#[wasm_bindgen]
impl Conversion {
    #[wasm_bindgen(constructor)]
    pub fn new(name: String) -> Self {
        Self {
            name,
            files: BTreeMap::new(),
            module: None,
            artifacts: Vec::new(),
        }
    }

    /// Return a structured error for invalid or duplicate project paths.
    pub fn add_file(&mut self, name: String, bytes: &[u8]) -> String {
        if let Err(error) = ArtifactPath::new(&name) {
            return failure(&error);
        }
        if self.files.contains_key(&name) {
            return json!({"ok": false, "diagnostics": [{
                "code": "WEB.INPUT.DUPLICATE_PATH", "severity": "error",
                "message": format!("More than one file has the project path `{name}`. Choose their containing folder to preserve relative paths.")
            }]}).to_string();
        }
        if self.files.len() >= 4096
            || self.files.values().map(|file| file.len()).sum::<usize>() + bytes.len()
                > 64 * 1024 * 1024
        {
            return json!({"ok": false, "diagnostics": [{
                "code": "WEB.INPUT.LIMIT", "severity": "error",
                "message": "This project exceeds 4096 files or 64 MiB. Choose a smaller project or use PowerIO in the terminal."
            }]}).to_string();
        }
        self.files.insert(name, Arc::from(bytes));
        json!({"ok": true}).to_string()
    }

    #[allow(clippy::needless_pass_by_value)] // wasm-bindgen receives optional JavaScript strings as owned strings.
    pub fn inspect(&mut self, primary: Option<String>, format: Option<String>) -> String {
        self.module = None;
        self.artifacts.clear();
        let source = self.source(primary.as_deref(), format.as_deref());
        let module = match source.and_then(powerio::parse) {
            Ok(module) => module,
            Err(error) => return failure(&error),
        };
        let detected =
            module
                .source()
                .and_then(Source::format)
                .map(|format| match format.as_str() {
                    "powerworld-pwb" => "pwb".to_owned(),
                    token => powerio::resolve_format(token)
                        .map_or(token, |resolved| resolved.token)
                        .to_owned(),
                });
        let family = detected.as_ref().and_then(|token| {
            powerio::grid_formats()
                .find(|entry| entry.format.token == token)
                .map(|entry| entry.family)
        });
        let response = json!({
            "ok": true, "format": detected, "family": family,
            "valueType": module.value().type_name(), "diagnostics": diagnostics(module.diagnostics()),
        }).to_string();
        self.module = Some(module);
        response
    }

    /// Emit one target and retain its artifacts for transfer to JavaScript.
    pub fn emit(&mut self, format: &str, name: String) -> String {
        self.artifacts.clear();
        let Some(module) = self.module.as_ref() else {
            return json!({"ok": false, "diagnostics": [{
                "code": "WEB.CONVERT.NOT_PARSED", "severity": "error", "message": "Parse the case before converting it."
            }]}).to_string();
        };
        let result = Destination::memory(name).and_then(|out| powerio::emit(module, format, out));
        match result {
            Ok(result) => {
                let response = json!({
                    "ok": true,
                    "layout": match result.layout() {
                        powerio::OutputLayout::File => "file",
                        powerio::OutputLayout::Directory => "directory",
                    },
                    "fidelity": match result.fidelity() {
                        powerio::Fidelity::ExactSameFormat => "exact",
                        powerio::Fidelity::Canonical => "canonical",
                    },
                    "diagnostics": diagnostics(result.diagnostics()),
                })
                .to_string();
                if let EmittedOutput::Memory { artifacts } = result.into_output() {
                    self.artifacts = artifacts;
                }
                response
            }
            Err(error) => failure(&error),
        }
    }

    pub fn artifact_count(&self) -> usize {
        self.artifacts.len()
    }

    pub fn artifact_name(&self, index: usize) -> Option<String> {
        self.artifacts
            .get(index)
            .map(|artifact| artifact.name().as_str().to_owned())
    }

    pub fn take_artifact(&mut self, index: usize) -> Vec<u8> {
        self.artifacts
            .get_mut(index)
            .map_or_else(Vec::new, |artifact| {
                let empty = powerio::MemoryArtifact::new(artifact.name().clone(), Vec::new());
                std::mem::replace(artifact, empty).into_bytes()
            })
    }
}

impl Conversion {
    fn source(
        &self,
        primary: Option<&str>,
        format: Option<&str>,
    ) -> Result<Source, powerio::Error> {
        let source = if self.files.len() == 1
            && self
                .files
                .first_key_value()
                .is_some_and(|(name, _)| primary == Some(name.as_str()))
        {
            let (name, bytes) = self.files.first_key_value().expect("one file");
            Source::from_memory(name, Arc::clone(bytes))?
        } else {
            let files = self
                .files
                .iter()
                .map(|(name, bytes)| ArtifactPath::new(name).map(|path| (path, Arc::clone(bytes))))
                .collect::<Result<Vec<_>, _>>()?;
            Source::from_memory_tree(
                &self.name,
                files,
                primary.map(ArtifactPath::new).transpose()?,
            )?
        };
        if let Some(format) = format.filter(|value| !value.is_empty()) {
            Ok(source.with_format(powerio::FormatId::new(format)?))
        } else {
            Ok(source)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_reader_profiles_report_their_family_without_enabling_sincal_writing() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!(
                "../../tests/data/sincal/synthetic-multiconductor.sql"
            ))
            .unwrap();
        let multiconductor = connection.serialize("main").unwrap().to_vec();
        for (filename, bytes, token, family) in [
            (
                "feeder.xml",
                include_bytes!("../../tests/data/dist/cim/micro_feeder.xml").as_slice(),
                "cim-dist",
                "distribution",
            ),
            (
                "case.sinx",
                include_bytes!("../../tests/data/sincal/1-LV-rural1--0-sw.sinx").as_slice(),
                "sincal-balanced",
                "transmission",
            ),
            (
                "case.db",
                multiconductor.as_slice(),
                "sincal-multiconductor",
                "distribution",
            ),
        ] {
            let mut converter = Conversion::new(filename.into());
            converter.add_file(filename.into(), bytes);
            let result: Value =
                serde_json::from_str(&converter.inspect(Some(filename.into()), Some(token.into())))
                    .unwrap();
            assert_eq!(result["ok"], true, "{result}");
            assert_eq!(result["family"], family);
            assert_eq!(result["format"], token);
        }
        let catalog: Value = serde_json::from_str(&capabilities()).unwrap();
        for token in ["sincal-balanced", "sincal-multiconductor"] {
            let format = catalog["formats"]
                .as_array()
                .unwrap()
                .iter()
                .find(|format| format["token"] == token)
                .unwrap();
            assert_eq!(format["canRead"], true);
            assert_eq!(format["canEmit"], false);
        }
    }

    #[test]
    fn gridlabd_profiles_are_listed_for_reading_but_not_offered_as_targets() {
        let catalog: Value = serde_json::from_str(&capabilities()).unwrap();
        for token in ["gridlabd", "gridlabd-arras"] {
            let format = catalog["formats"]
                .as_array()
                .unwrap()
                .iter()
                .find(|format| format["token"] == token)
                .unwrap();
            assert_eq!(format["family"], "distribution");
            assert_eq!(format["canRead"], true);
            assert_eq!(format["canEmit"], false);
        }
    }

    #[test]
    fn dss_companion_suffixes_do_not_change_native_parse_or_source_echo() {
        let mut converter = Conversion::new("feeder".into());
        let files: [(&str, &[u8]); 3] = [
            (
                "Master.dss",
                b"New Circuit.example basekv=12.47 bus1=source\nRedirect loads.inc\n",
            ),
            (
                "loads.inc",
                b"New Load.ld bus1=source phases=3 conn=wye kv=12.47 kw=10 kvar=2\n",
            ),
            ("notes.asset", b"opaque companion bytes\x00\xff"),
        ];
        for (name, bytes) in files {
            converter.add_file(name.into(), bytes);
        }
        let result: Value =
            serde_json::from_str(&converter.inspect(Some("Master.dss".into()), None)).unwrap();
        assert_eq!(result["ok"], true, "{result}");
        let result: Value = serde_json::from_str(&converter.emit("dss", "feeder".into())).unwrap();
        assert_eq!(result["ok"], true, "{result}");
        for (name, bytes) in files {
            let index = converter
                .artifacts
                .iter()
                .position(|artifact| artifact.name().as_str().ends_with(name))
                .unwrap();
            assert_eq!(converter.take_artifact(index), bytes);
        }
    }

    #[test]
    fn missing_primary_is_reported_instead_of_reading_another_file() {
        let mut converter = Conversion::new("case".into());
        converter.add_file("case9.m".into(), include_bytes!("../../tests/data/case9.m"));
        let result: Value =
            serde_json::from_str(&converter.inspect(Some("missing.m".into()), None)).unwrap();
        assert_eq!(result["ok"], false);
        assert_ne!(
            result["diagnostics"].as_array().unwrap().as_slice(),
            &[] as &[Value]
        );
    }

    #[test]
    fn binary_powerworld_source_matches_its_catalog_entry() {
        let mut converter = Conversion::new("case.pwb".into());
        converter.add_file(
            "case.pwb".into(),
            include_bytes!("../../tests/data/powerworld/ACTIVSg200.pwb"),
        );
        let result: Value =
            serde_json::from_str(&converter.inspect(Some("case.pwb".into()), None)).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["format"], "pwb");
        assert_eq!(result["family"], "transmission");
    }

    #[test]
    fn binding_preserves_native_artifacts_and_diagnostic_codes() {
        let bytes = include_bytes!("../../tests/data/case9.m");
        let mut converter = Conversion::new("case9.m".into());
        converter.add_file("case9.m".into(), bytes);
        let inspection: Value =
            serde_json::from_str(&converter.inspect(Some("case9.m".into()), None)).unwrap();
        assert_eq!(inspection["format"], "matpower");
        assert_eq!(inspection["family"], "transmission");
        let native =
            powerio::parse(Source::from_memory("case9.m", bytes.to_vec()).unwrap()).unwrap();
        for format in [
            "matpower",
            "powermodels-json",
            "psse",
            "pandapower-json",
            "cgmes",
        ] {
            let emitted =
                powerio::emit(&native, format, Destination::memory("case9").unwrap()).unwrap();
            let output: Value =
                serde_json::from_str(&converter.emit(format, "case9".into())).unwrap();
            assert_eq!(
                output["diagnostics"],
                json!(diagnostics(emitted.diagnostics()))
            );
            let EmittedOutput::Memory { artifacts } = emitted.output() else {
                panic!("memory expected")
            };
            assert_eq!(
                output["layout"],
                if emitted.layout() == powerio::OutputLayout::File {
                    "file"
                } else {
                    "directory"
                }
            );
            assert_eq!(converter.artifact_count(), artifacts.len());
            for (index, artifact) in artifacts.iter().enumerate() {
                assert_eq!(converter.take_artifact(index), artifact.bytes());
            }
        }
    }

    #[test]
    fn indexed_mutation_warning_reaches_inspection_without_changing_values_or_echo() {
        let original = include_str!("../../tests/data/case9.m");
        let mutation = "mpc.bus(:, [3 4]) ... ignored ) ' ; ,\n = mpc.bus(:, [3 4]) / 1000;";
        let source = format!("{original}\nx = [1 2].';\n{mutation}\n");
        let mut converter = Conversion::new("mutated.m".into());
        converter.add_file("mutated.m".into(), source.as_bytes());
        let result: Value =
            serde_json::from_str(&converter.inspect(Some("mutated.m".into()), None)).unwrap();
        assert_eq!(result["ok"], true, "{result}");
        let warnings: Vec<_> = result["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["code"] == "READ.MATPOWER.INDEXED_ASSIGNMENT")
            .collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["severity"], "warning");
        let start = source.find(mutation).unwrap();
        assert_eq!(warnings[0]["spans"][0]["start"], start);
        assert_eq!(warnings[0]["spans"][0]["end"], start + mutation.len());
        let native =
            powerio::parse(Source::from_memory("case9.m", original.as_bytes().to_vec()).unwrap())
                .unwrap();
        let (
            powerio::PioValue::BalancedNetwork(expected),
            powerio::PioValue::BalancedNetwork(actual),
        ) = (native.value(), converter.module.as_ref().unwrap().value())
        else {
            panic!("balanced values expected");
        };
        let expected_load: f64 = expected.loads().iter().map(|load| load.p).sum();
        let actual_load: f64 = actual.loads().iter().map(|load| load.p).sum();
        assert!((actual_load - expected_load).abs() < 1e-12);
        let emitted: Value =
            serde_json::from_str(&converter.emit("matpower", "mutated.m".into())).unwrap();
        assert_eq!(emitted["ok"], true, "{emitted}");
        assert_eq!(converter.artifact_count(), 1);
        assert_eq!(converter.take_artifact(0), source.as_bytes());
    }
}
