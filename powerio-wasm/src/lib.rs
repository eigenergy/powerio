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
                "canEmit": entry.format.can_emit && entry.format.token != "bmopf-json",
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
    fn missing_primary_is_reported_instead_of_reading_another_file() {
        let mut converter = Conversion::new("case".into());
        converter.add_file("case9.m".into(), include_bytes!("../../tests/data/case9.m"));
        let result: Value =
            serde_json::from_str(&converter.inspect(Some("missing.m".into()), None)).unwrap();
        assert_eq!(result["ok"], false);
        assert!(!result["diagnostics"].as_array().unwrap().is_empty());
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
}
