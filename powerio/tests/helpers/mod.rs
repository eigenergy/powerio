//! Integration helpers over the public PowerIO module surface.
#![allow(dead_code)]
#![allow(unused_imports)]

use std::path::Path;

use powerio::BalancedNetwork;
use powerio::Diagnostic;

/// A balanced network plus the diagnostics produced while parsing it.
#[derive(Debug)]
pub struct Parsed {
    pub network: BalancedNetwork,
    pub diagnostics: Vec<Diagnostic>,
}

impl Parsed {
    pub fn render_diagnostics(&self) -> Vec<String> {
        powerio_core::render_diagnostics(&self.diagnostics)
    }
}

pub fn serialize_module_text(
    module: &powerio::PioModule<powerio::PioValue>,
) -> Result<String, powerio_core::Error> {
    let destination = powerio::Destination::memory("module.pio.json")?;
    let result = powerio::serialize(module, destination)?;
    let powerio::EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        unreachable!("a memory destination returns memory artifacts")
    };
    let artifact = artifacts
        .pop()
        .filter(|_| artifacts.is_empty())
        .expect("PowerIO IR serialization returns one artifact");
    String::from_utf8(artifact.into_bytes()).map_err(|cause| {
        powerio_core::Error::new(
            &powerio::codes::READ_MODULE_INVALID,
            "PowerIO IR serialization returned non-UTF-8 bytes",
        )
        .with_cause(cause)
    })
}

pub fn deserialize_module_text(
    text: &str,
) -> Result<powerio::PioModule<powerio::PioValue>, powerio_core::Error> {
    let source = powerio::Source::from_memory("module.pio.json", text.as_bytes().to_vec())?;
    powerio::deserialize(source)
}

fn module_to_parsed(module: powerio_core::PioModule<BalancedNetwork>) -> Parsed {
    Parsed {
        diagnostics: module.diagnostics().to_vec(),
        network: module.into_value(),
    }
}

pub fn load_balanced_case(
    path: impl AsRef<Path>,
    from: Option<&str>,
) -> Result<Parsed, powerio_core::Error> {
    load_balanced_module(path, from).map(module_to_parsed)
}

/// The parse options for an optional format token.
pub fn options_for(format: Option<&str>) -> Result<powerio::ParseOptions, powerio_core::Error> {
    let mut options = powerio::ParseOptions::default();
    if let Some(format) = format {
        options = options.format(format)?;
    }
    Ok(options)
}

pub fn load_balanced_module(
    path: impl AsRef<Path>,
    from: Option<&str>,
) -> Result<powerio_core::PioModule<BalancedNetwork>, powerio_core::Error> {
    let source = powerio::Source::open(path.as_ref())?;
    into_balanced_module(powerio::parse_with_options(source, &options_for(from)?)?)
}

pub fn load_balanced_memory(text: &str, from: &str) -> Result<Parsed, powerio_core::Error> {
    load_balanced_memory_named(text, from, None)
}

pub fn load_balanced_memory_named(
    text: &str,
    from: &str,
    name_hint: Option<&str>,
) -> Result<Parsed, powerio_core::Error> {
    let name = name_hint.map_or_else(|| "<memory>".to_owned(), std::string::ToString::to_string);
    let source = powerio::Source::from_memory(name, text.as_bytes().to_vec())?;
    into_balanced_module(powerio::parse_with_options(
        source,
        &powerio::ParseOptions::default().format(from).unwrap(),
    )?)
    .map(module_to_parsed)
}

pub fn load_balanced_bytes(bytes: &[u8], from: &str) -> Result<Parsed, powerio_core::Error> {
    load_balanced_bytes_named(bytes, from, None)
}

pub fn load_balanced_bytes_named(
    bytes: &[u8],
    from: &str,
    name_hint: Option<&str>,
) -> Result<Parsed, powerio_core::Error> {
    let name = name_hint.map_or_else(|| "<memory>".to_owned(), std::string::ToString::to_string);
    let source = powerio::Source::from_memory(name, bytes.to_vec())?;
    into_balanced_module(powerio::parse_with_options(
        source,
        &powerio::ParseOptions::default().format(from).unwrap(),
    )?)
    .map(module_to_parsed)
}

pub fn parse_matpower(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    load_balanced_memory(text, "matpower").map(|parsed| parsed.network)
}

pub fn parse_matpower_file(path: impl AsRef<Path>) -> Result<BalancedNetwork, powerio_core::Error> {
    load_balanced_case(path, Some("matpower")).map(|parsed| parsed.network)
}

pub fn parse_psse(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    load_balanced_memory(text, "psse").map(|parsed| parsed.network)
}

pub fn parse_powerworld(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    load_balanced_memory(text, "powerworld").map(|parsed| parsed.network)
}

pub fn parse_pslf(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    load_balanced_memory(text, "pslf").map(|parsed| parsed.network)
}

pub fn parse_egret_json(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    load_balanced_memory(text, "egret-json").map(|parsed| parsed.network)
}

pub fn parse_powermodels_json(text: &str) -> Result<BalancedNetwork, powerio_core::Error> {
    load_balanced_memory(text, "powermodels-json").map(|parsed| parsed.network)
}

pub fn parse_pandapower_json(text: &str) -> Result<Parsed, powerio_core::Error> {
    load_balanced_memory(text, "pandapower-json")
}

pub fn parse_surge_json(text: &str) -> Result<Parsed, powerio_core::Error> {
    load_balanced_memory(text, "surge-json")
}

pub fn parse_deepmind_opfdata_json(text: &str) -> Result<Parsed, powerio_core::Error> {
    load_balanced_memory(text, "opfdata-json")
}

pub fn read_pypsa_csv_folder(path: impl AsRef<Path>) -> Result<Parsed, powerio_core::Error> {
    load_balanced_case(path, Some("pypsa-csv"))
}

/// Parse `.dss`/distribution text through the module surface and hand back
/// the bare network, for suites that build packages from typed values.
pub fn load_multiconductor_memory(text: &str, from: &str) -> powerio_dist::MulticonductorNetwork {
    load_multiconductor_module(text, from).into_value()
}

/// Parse distribution text into the compiled module (network plus findings).
pub fn load_multiconductor_module(
    text: &str,
    from: &str,
) -> powerio_core::PioModule<powerio_dist::MulticonductorNetwork> {
    let source =
        powerio::Source::from_memory("<memory>", text.as_bytes().to_vec()).expect("memory source");
    into_multiconductor_module(
        powerio::parse_with_options(
            source,
            &powerio::ParseOptions::default().format(from).unwrap(),
        )
        .expect("source parses"),
    )
    .expect("source contains a multiconductor network")
}

fn wrong_value_type(actual: &str, expected: &str) -> powerio_core::Error {
    powerio_core::Error::new(
        &powerio::codes::REQUEST_MODULE_WRONG_MODEL_KIND,
        format!("{actual} cannot be used as {expected}"),
    )
}

#[allow(clippy::result_large_err)]
fn into_balanced_module(
    module: powerio::PioModule<powerio::PioValue>,
) -> Result<powerio::PioModule<BalancedNetwork>, powerio_core::Error> {
    let actual = module.value().type_name().to_owned();
    module
        .try_map_value(|value| match value {
            powerio::PioValue::BalancedNetwork(network) => Ok(network),
            powerio::PioValue::DcPfInstance(instance) => Ok(instance.network().clone()),
            powerio::PioValue::AcPfInstance(instance) => Ok(instance.network().clone()),
            powerio::PioValue::DcOpfInstance(instance) => Ok(instance.network().clone()),
            powerio::PioValue::AcOpfInstance(instance) => Ok(instance.network().clone()),
            powerio::PioValue::AcScucInstance(instance) => Ok(instance.network().clone()),
            powerio::PioValue::DcPfSolution(solution) => Ok(solution.network().clone()),
            powerio::PioValue::AcPfSolution(solution) => Ok(solution.network().clone()),
            powerio::PioValue::DcOpfSolution(solution) => Ok(solution.network().clone()),
            powerio::PioValue::AcOpfSolution(solution) => Ok(solution.network().clone()),
            powerio::PioValue::AcScucSolution(solution) => {
                Ok(solution.instance().network().clone())
            }
            other => Err(other),
        })
        .map_err(|_| wrong_value_type(&actual, "a balanced network"))
}

#[allow(clippy::result_large_err)]
fn into_multiconductor_module(
    module: powerio::PioModule<powerio::PioValue>,
) -> Result<powerio::PioModule<powerio_dist::MulticonductorNetwork>, powerio_core::Error> {
    let actual = module.value().type_name().to_owned();
    module
        .try_map_value(|value| match value {
            powerio::PioValue::MulticonductorNetwork(network) => Ok(network),
            powerio::PioValue::McAcPfInstance(instance) => Ok(instance.network().clone()),
            powerio::PioValue::McAcOpfInstance(instance) => Ok(instance.network().clone()),
            powerio::PioValue::McAcPfSolution(solution) => Ok(solution.network().clone()),
            powerio::PioValue::McAcOpfSolution(solution) => Ok(solution.network().clone()),
            other => Err(other),
        })
        .map_err(|_| wrong_value_type(&actual, "a multiconductor network"))
}
