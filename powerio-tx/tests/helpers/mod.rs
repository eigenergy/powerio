//! Shared parse and emission helpers for integration tests.
#![allow(dead_code)]
#![allow(unused_imports)]

use std::path::{Path, PathBuf};

use powerio_core::{FormatId, Source};
use powerio_tx::network::BalancedNetwork;

use powerio_tx::diagnostics::Diagnostic;

#[derive(Debug, Clone)]
pub struct TextEmission {
    pub text: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl TextEmission {
    pub fn render_diagnostics(&self) -> Vec<String> {
        powerio_tx::diagnostics::render_diagnostics(&self.diagnostics)
    }
}

#[derive(Debug)]
pub struct DirectoryEmission {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

impl DirectoryEmission {
    pub fn render_diagnostics(&self) -> Vec<String> {
        powerio_tx::diagnostics::render_diagnostics(&self.diagnostics)
    }
}

fn text_from_result(result: powerio_core::EmitResult) -> TextEmission {
    let diagnostics = result.diagnostics().to_vec();
    let powerio_core::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory destination returns memory output");
    };
    assert_eq!(artifacts.len(), 1, "text emission has one artifact");
    let bytes = artifacts.pop().unwrap().into_bytes();
    TextEmission {
        text: String::from_utf8(bytes).expect("case text is UTF-8"),
        diagnostics,
    }
}

/// A network as it comes back from its own serde representation, the form
/// PowerIO IR nests as `value.data`.
pub fn serde_round_trip(network: &BalancedNetwork) -> BalancedNetwork {
    serde_json::from_str(&serde_json::to_string(network).unwrap()).unwrap()
}

pub fn emit_module(
    module: &powerio_core::PioModule<BalancedNetwork>,
    target: powerio_tx::TargetFormat,
) -> Result<TextEmission, powerio_core::Error> {
    powerio_tx::emit(module, target, powerio_core::Destination::memory("case")?)
        .map(text_from_result)
}

pub fn emit_module_with_options(
    module: &powerio_core::PioModule<BalancedNetwork>,
    target: powerio_tx::TargetFormat,
    options: &powerio_tx::EmitOptions,
) -> Result<TextEmission, powerio_core::Error> {
    powerio_tx::emit_with_options(
        module,
        target,
        options,
        powerio_core::Destination::memory("case")?,
    )
    .map(text_from_result)
}

pub fn emit_value(
    network: &BalancedNetwork,
    target: powerio_tx::TargetFormat,
) -> Result<TextEmission, powerio_core::Error> {
    emit_module(&powerio_core::PioModule::new(network.clone()), target)
}

pub fn emit_powermodels_json(network: &BalancedNetwork) -> TextEmission {
    emit_value(network, powerio_tx::TargetFormat::PowerModelsJson).unwrap()
}

pub fn emit_egret_json(network: &BalancedNetwork) -> TextEmission {
    emit_value(network, powerio_tx::TargetFormat::EgretJson).unwrap()
}

pub fn emit_pandapower_json(network: &BalancedNetwork) -> TextEmission {
    emit_value(network, powerio_tx::TargetFormat::PandapowerJson).unwrap()
}

pub fn emit_powerworld(network: &BalancedNetwork) -> TextEmission {
    emit_value(network, powerio_tx::TargetFormat::PowerWorld).unwrap()
}

pub fn emit_pslf(network: &BalancedNetwork) -> TextEmission {
    emit_value(network, powerio_tx::TargetFormat::Pslf).unwrap()
}

pub fn emit_psse(network: &BalancedNetwork) -> TextEmission {
    emit_psse_rev(network, 33)
}

pub fn emit_psse_rev(network: &BalancedNetwork, rev: u32) -> TextEmission {
    emit_value(network, powerio_tx::TargetFormat::Psse { rev }).unwrap()
}

pub fn emit_matpower(network: &BalancedNetwork) -> String {
    emit_value(network, powerio_tx::TargetFormat::Matpower)
        .unwrap()
        .text
}

pub fn emit_pypsa_csv_folder(
    network: &BalancedNetwork,
    output: impl AsRef<Path>,
) -> Result<DirectoryEmission, powerio_core::Error> {
    emit_pypsa_csv_folder_with_options(network, output, &powerio_tx::EmitOptions::default())
}

pub fn emit_pypsa_csv_folder_with_options(
    network: &BalancedNetwork,
    output: impl AsRef<Path>,
    options: &powerio_tx::EmitOptions,
) -> Result<DirectoryEmission, powerio_core::Error> {
    let result = powerio_tx::__emit_pypsa_csv_with_options(
        &powerio_core::PioModule::new(network.clone()),
        options,
        powerio_core::Destination::path(output.as_ref()),
    )?;
    let diagnostics = result.diagnostics().to_vec();
    let powerio_core::EmittedOutput::Path { root, artifacts } = result.into_output() else {
        unreachable!("path destination returns path output");
    };
    Ok(DirectoryEmission {
        dir: root,
        files: artifacts,
        diagnostics,
    })
}

/// The old parse output shape: the typed network plus the reader's findings.
/// The parsed module rides along so echo assertions can write through it.
#[derive(Debug)]
pub struct Parsed {
    pub network: BalancedNetwork,
    pub diagnostics: Vec<Diagnostic>,
    pub module: powerio_core::PioModule<BalancedNetwork>,
}

impl Parsed {
    pub fn render_diagnostics(&self) -> Vec<String> {
        powerio_tx::diagnostics::render_diagnostics(&self.diagnostics)
    }

    /// Emit through the module: a same format target echoes the retained
    /// source bytes exactly.
    pub fn emit(
        &self,
        target: powerio_tx::TargetFormat,
    ) -> Result<TextEmission, powerio_core::Error> {
        emit_module(&self.module, target)
    }
}

pub fn parse_file_and_emit(
    path: impl AsRef<Path>,
    target: powerio_tx::TargetFormat,
    from: Option<&str>,
) -> Result<TextEmission, powerio_core::Error> {
    let module = parse_module(path, from)?;
    emit_module(&module, target)
}

fn declared(source: Source, from: Option<&str>) -> Result<Source, powerio_core::Error> {
    match from {
        None => Ok(source),
        Some(token) => {
            Ok(source.with_format(FormatId::new(token.to_ascii_lowercase().replace('_', "-"))?))
        }
    }
}

fn module_to_parsed(module: powerio_core::PioModule<BalancedNetwork>) -> Parsed {
    Parsed {
        diagnostics: module.diagnostics().to_vec(),
        network: module.value().clone(),
        module,
    }
}

pub fn parse_file(
    path: impl AsRef<Path>,
    from: Option<&str>,
) -> Result<Parsed, powerio_core::Error> {
    let source = declared(Source::open(path.as_ref())?, from)?;
    powerio_tx::parse(source).map(module_to_parsed)
}

pub fn parse_module(
    path: impl AsRef<Path>,
    from: Option<&str>,
) -> Result<powerio_core::PioModule<BalancedNetwork>, powerio_core::Error> {
    let source = declared(Source::open(path.as_ref())?, from)?;
    powerio_tx::parse(source)
}

pub fn parse_str(text: &str, from: &str) -> Result<Parsed, powerio_core::Error> {
    parse_str_with_name(text, from, None)
}

pub fn parse_str_with_name(
    text: &str,
    from: &str,
    name_hint: Option<&str>,
) -> Result<Parsed, powerio_core::Error> {
    let name = name_hint.map_or_else(|| "<memory>".to_owned(), std::string::ToString::to_string);
    let source = declared(
        Source::from_memory(name, text.as_bytes().to_vec())?,
        Some(from),
    )?;
    powerio_tx::parse(source).map(module_to_parsed)
}

pub fn parse_bytes(bytes: &[u8], from: &str) -> Result<Parsed, powerio_core::Error> {
    parse_bytes_with_name(bytes, from, None)
}

pub fn parse_bytes_with_name(
    bytes: &[u8],
    from: &str,
    name_hint: Option<&str>,
) -> Result<Parsed, powerio_core::Error> {
    let name = name_hint.map_or_else(|| "<memory>".to_owned(), std::string::ToString::to_string);
    let source = declared(Source::from_memory(name, bytes.to_vec())?, Some(from))?;
    powerio_tx::parse(source).map(module_to_parsed)
}

pub fn parse_matpower(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    parse_str(text, "matpower").map(|parsed| parsed.network)
}

pub fn parse_matpower_file(path: impl AsRef<Path>) -> Result<BalancedNetwork, powerio_core::Error> {
    parse_file(path, Some("matpower")).map(|parsed| parsed.network)
}

pub fn parse_psse(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    parse_str(text, "psse").map(|parsed| parsed.network)
}

pub fn parse_powerworld(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    parse_str(text, "powerworld").map(|parsed| parsed.network)
}

pub fn parse_pslf(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    parse_str(text, "pslf").map(|parsed| parsed.network)
}

pub fn parse_egret_json(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    parse_str(text, "egret-json").map(|parsed| parsed.network)
}

pub fn parse_powermodels_json(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    parse_str(text, "powermodels-json").map(|parsed| parsed.network)
}

pub fn parse_pandapower_json(text: &str) -> Result<Parsed, powerio_core::Error> {
    parse_str(text, "pandapower-json")
}

pub fn parse_surge_json(text: &str) -> Result<Parsed, powerio_core::Error> {
    parse_str(text, "surge-json")
}

pub fn parse_deepmind_opfdata_json(text: &str) -> Result<Parsed, powerio_core::Error> {
    parse_str(text, "opfdata-json")
}

pub fn read_pypsa_csv_folder(path: impl AsRef<Path>) -> Result<Parsed, powerio_core::Error> {
    parse_file(path, Some("pypsa-csv"))
}
