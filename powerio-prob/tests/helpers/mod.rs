//! Compatibility helpers keeping the integration suites on the old call
//! shapes while they exercise the module parse surface underneath.
#![allow(dead_code)]
#![allow(unused_imports)]

use std::path::Path;

use powerio_core::{FormatId, Source};
use powerio_tx::BalancedNetwork;

use powerio_tx::Diagnostic;
pub use powerio_tx::write_network;

/// The old parse output shape: the typed network plus the reader's findings.
#[derive(Debug)]
pub struct Parsed {
    pub network: BalancedNetwork,
    pub diagnostics: Vec<Diagnostic>,
}

impl Parsed {
    pub fn rendered_diagnostics(&self) -> Vec<String> {
        powerio_tx::diagnostics::render_diagnostics(&self.diagnostics)
    }
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
        network: module.into_value(),
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
        Source::from_bytes(name, text.as_bytes().to_vec())?,
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
    let source = declared(Source::from_bytes(name, bytes.to_vec())?, Some(from))?;
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
