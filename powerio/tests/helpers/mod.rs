//! Compatibility helpers keeping the integration suites on the old call
//! shapes while they exercise the module parse surface underneath.
#![allow(dead_code)]
#![allow(unused_imports)]

use std::path::Path;

use powerio::BalancedNetwork;
use powerio_core::{FormatId, Source};

use powerio::Diagnostic;
pub use powerio::write_network;

/// The old parse output shape: the typed network plus the reader's findings,
/// and the typed DOE GO Challenge 3 document when that reader produced one.
#[derive(Debug)]
pub struct Parsed {
    pub network: BalancedNetwork,
    pub diagnostics: Vec<Diagnostic>,
    pub document: Option<std::sync::Arc<powerio::format::goc3::Goc3Document>>,
}

impl Parsed {
    pub fn rendered_diagnostics(&self) -> Vec<String> {
        powerio::diagnostics::render_diagnostics(&self.diagnostics)
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
        document: None,
    }
}

pub fn parse_file(
    path: impl AsRef<Path>,
    from: Option<&str>,
) -> Result<Parsed, powerio_core::Error> {
    let source = declared(Source::open(path.as_ref())?, from)?;
    powerio::format::parse(source).map(module_to_parsed)
}

pub fn parse_module(
    path: impl AsRef<Path>,
    from: Option<&str>,
) -> Result<powerio_core::PioModule<BalancedNetwork>, powerio_core::Error> {
    let source = declared(Source::open(path.as_ref())?, from)?;
    powerio::format::parse(source)
}

pub fn parse_str(text: &str, from: &str) -> Result<Parsed, powerio_core::Error> {
    parse_str_with_name(text, from, None)
}

pub fn parse_str_with_name(
    text: &str,
    from: &str,
    name_hint: Option<&str>,
) -> Result<Parsed, powerio_core::Error> {
    if from.eq_ignore_ascii_case("goc3-json") {
        let (network, diagnostics, document) =
            powerio::parse_goc3_json(text).map_err(tx_error_to_core)?;
        return Ok(Parsed {
            network,
            diagnostics,
            document: Some(document),
        });
    }
    let name = name_hint.map_or_else(|| "<memory>".to_owned(), std::string::ToString::to_string);
    let source = declared(
        Source::from_bytes(name, text.as_bytes().to_vec())?,
        Some(from),
    )?;
    powerio::format::parse(source).map(module_to_parsed)
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
    powerio::format::parse(source).map(module_to_parsed)
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

fn tx_error_to_core(error: powerio::error::Error) -> powerio_core::Error {
    powerio_core::Error::new(error.code(), error.to_string()).with_cause(error)
}

/// Parse `.dss`/distribution text through the module surface and hand back
/// the bare network, for suites that build packages from typed values.
pub fn dist_parse_str(text: &str, from: &str) -> powerio_dist::MulticonductorNetwork {
    dist_parse_module(text, from).into_value()
}

/// Parse distribution text into the compiled module (network plus findings).
pub fn dist_parse_module(
    text: &str,
    from: &str,
) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    let source = powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())
        .expect("memory source")
        .with_format(powerio_core::FormatId::new(from).expect("format id"));
    powerio_dist::parse(source).expect("distribution text parses")
}
