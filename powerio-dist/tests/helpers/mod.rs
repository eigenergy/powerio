//! Compatibility helpers keeping the integration suites on the old call
//! shapes while they exercise the module parse surface underneath.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use powerio_dist::{DistTargetFormat, Error, MulticonductorNetwork};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sidecar {
    pub path: String,
    pub text: String,
}

/// The old conversion output shape, with rendered warnings materialized.
#[derive(Debug, Clone)]
pub struct Conv {
    pub text: String,
    pub sidecars: Vec<Sidecar>,
    pub warnings: Vec<String>,
    pub diagnostics: Vec<powerio_dist::Diagnostic>,
}

fn emit_module_with_options(
    module: &powerio_core::PioModule<MulticonductorNetwork>,
    target: DistTargetFormat,
    options: &powerio_dist::EmitOptions,
) -> Result<Conv, powerio_core::Error> {
    let result = powerio_dist::emit_with_options(
        module,
        target,
        options,
        powerio_core::Destination::memory("out")?,
    )?;
    let diagnostics = result.diagnostics().to_vec();
    let warnings = powerio_dist::diagnostics::render_diagnostics(&diagnostics);
    let powerio_core::EmittedOutput::Memory { artifacts } = result.into_output() else {
        unreachable!("memory destination returns memory output");
    };
    let mut text = None;
    let mut sidecars = Vec::new();
    for artifact in artifacts {
        let path = artifact.name().as_str().to_owned();
        let content = String::from_utf8(artifact.into_bytes()).expect("format text is UTF-8");
        if (target == DistTargetFormat::Dss && path.ends_with("case.dss"))
            || target != DistTargetFormat::Dss
        {
            text = Some(content);
        } else {
            sidecars.push(Sidecar {
                path: path.trim_start_matches("out/").to_owned(),
                text: content,
            });
        }
    }
    Ok(Conv {
        text: text.expect("emission has a primary artifact"),
        sidecars,
        warnings,
        diagnostics,
    })
}

pub fn emit_value(net: &MulticonductorNetwork, target: DistTargetFormat) -> Conv {
    emit_module_with_options(
        &powerio_core::PioModule::new(net.clone()),
        target,
        &powerio_dist::EmitOptions::default(),
    )
    .unwrap()
}

pub fn emit_dss(net: &MulticonductorNetwork) -> Conv {
    emit_value(net, DistTargetFormat::Dss)
}

pub fn emit_dss_with_options(
    net: &MulticonductorNetwork,
    options: &powerio_dist::DssEmitOptions,
) -> Conv {
    let mut emit_options = powerio_dist::EmitOptions::default();
    emit_options.dss = options.clone();
    emit_module_with_options(
        &powerio_core::PioModule::new(net.clone()),
        DistTargetFormat::Dss,
        &emit_options,
    )
    .unwrap()
}

pub fn emit_bmopf_json(net: &MulticonductorNetwork) -> Conv {
    emit_value(net, DistTargetFormat::BmopfJson)
}

pub fn emit_bmopf_json_with_options(
    net: &MulticonductorNetwork,
    options: powerio_dist::BmopfEmitOptions,
) -> Conv {
    let mut emit_options = powerio_dist::EmitOptions::default();
    emit_options.bmopf = options;
    emit_module_with_options(
        &powerio_core::PioModule::new(net.clone()),
        DistTargetFormat::BmopfJson,
        &emit_options,
    )
    .unwrap()
}

pub fn emit_pmd_json(net: &MulticonductorNetwork) -> Conv {
    emit_value(net, DistTargetFormat::PmdJson)
}

/// The old parse output shape: the typed network with the reader's findings
/// riding along. Dereferences to the network so field access keeps its old
/// spelling; `warnings` and `source` shadow the fields the network lost.
/// Mutation through `DerefMut` edits a copy of the module's value, so the
/// module's echo tier stays byte exact.
#[derive(Debug)]
pub struct Parsed {
    pub warnings: Vec<String>,
    pub diagnostics: Vec<powerio_dist::Diagnostic>,
    pub source: Option<Arc<String>>,
    pub module: powerio_core::PioModule<MulticonductorNetwork>,
    network: MulticonductorNetwork,
}

impl std::ops::Deref for Parsed {
    type Target = MulticonductorNetwork;
    fn deref(&self) -> &MulticonductorNetwork {
        &self.network
    }
}

impl std::ops::DerefMut for Parsed {
    fn deref_mut(&mut self) -> &mut MulticonductorNetwork {
        &mut self.network
    }
}

impl Parsed {
    /// Write through the module: a same format target echoes the retained
    /// source bytes exactly.
    pub fn emit(&self, target: DistTargetFormat) -> Conv {
        emit_module_with_options(&self.module, target, &powerio_dist::EmitOptions::default())
            .unwrap()
    }

    /// Write from the typed value, bypassing the echo tier.
    pub fn emit_value(&self, target: DistTargetFormat) -> Conv {
        emit_value(&self.network, target)
    }
}

fn from_module(module: powerio_core::PioModule<MulticonductorNetwork>) -> Parsed {
    let source = module.source().and_then(|source| {
        let buffer = source.primary_buffer().ok()?;
        let text = std::str::from_utf8(buffer.content_bytes()).ok()?;
        Some(Arc::new(text.to_owned()))
    });
    Parsed {
        warnings: powerio_dist::diagnostics::render_diagnostics(&module.diagnostics),
        diagnostics: module.diagnostics.clone(),
        source,
        network: module.value().clone(),
        module,
    }
}

fn declared(
    source: powerio_core::Source,
    from: Option<&str>,
) -> Result<powerio_core::Source, Error> {
    match from {
        None => Ok(source),
        Some(token) => {
            // The old entries settled the format before any work; keep the
            // error shape.
            if powerio_dist::parse_dist_target_format(token).is_none() {
                return Err(Error::UnknownFormat(token.to_string()));
            }
            let id = powerio_core::FormatId::new(token.to_ascii_lowercase().replace('_', "-"))
                .map_err(|_| Error::UnknownFormat(token.to_string()))?;
            Ok(source.with_format(id))
        }
    }
}

fn core_to_dist(error: &powerio_core::Error) -> Error {
    Error::FormatRead {
        format: "case text",
        message: error.to_string(),
    }
}

pub fn parse_str(text: &str, from: &str) -> Result<Parsed, Error> {
    let source = declared(
        powerio_core::Source::from_memory("<memory>", text.as_bytes().to_vec())
            .map_err(|error| core_to_dist(&error))?,
        Some(from),
    )?;
    powerio_dist::parse(source)
        .map(from_module)
        .map_err(|error| core_to_dist(&error))
}

pub fn parse_bytes(bytes: &[u8], from: &str) -> Result<Parsed, Error> {
    let source = declared(
        powerio_core::Source::from_memory("<memory>", bytes.to_vec())
            .map_err(|error| core_to_dist(&error))?,
        Some(from),
    )?;
    powerio_dist::parse(source)
        .map(from_module)
        .map_err(|error| core_to_dist(&error))
}

pub fn parse_file(path: impl AsRef<Path>, from: Option<&str>) -> Result<Parsed, Error> {
    if let Some(token) = from
        && powerio_dist::parse_dist_target_format(token).is_none()
    {
        return Err(Error::UnknownFormat(token.to_string()));
    }
    let source = declared(
        powerio_core::Source::open(path.as_ref()).map_err(|error| core_to_dist(&error))?,
        from,
    )?;
    powerio_dist::parse(source)
        .map(from_module)
        .map_err(|error| core_to_dist(&error))
}

/// The old infallible string entry: `.dss` text parses into the model with
/// filesystem includes refused (an in-memory source has no named buffers
/// here).
pub fn parse_dss_str(text: &str) -> Parsed {
    parse_str(text, "dss").expect("dss text parses without a filesystem")
}

pub fn parse_dss_file(path: impl AsRef<Path>) -> Result<Parsed, Error> {
    parse_file(path, Some("dss"))
}

/// The old include-root entry: includes are confined to `root`, selected on
/// the source at construction. The case file must sit under `root`.
pub fn parse_dss_file_with_root(
    path: impl AsRef<Path>,
    root: impl AsRef<Path>,
) -> Result<Parsed, Error> {
    let source = powerio_core::Source::open(path.as_ref())
        .and_then(|source| source.with_acquisition_root(root.as_ref()))
        .map_err(|error| core_to_dist(&error))?;
    let source = declared(source, Some("dss"))?;
    powerio_dist::parse(source)
        .map(from_module)
        .map_err(|error| core_to_dist(&error))
}

pub fn parse_bmopf_str(text: &str) -> Result<Parsed, Error> {
    parse_str(text, "bmopf-json")
}

pub fn parse_bmopf_file(path: impl AsRef<Path>) -> Result<Parsed, Error> {
    parse_file(path, Some("bmopf-json"))
}

pub fn parse_pmd_str(text: &str) -> Result<Parsed, Error> {
    parse_str(text, "pmd-json")
}

pub fn parse_pmd_file(path: impl AsRef<Path>) -> Result<Parsed, Error> {
    parse_file(path, Some("pmd-json"))
}

/// The old one-shot converter: the findings carry the reader's, then the
/// writer's.
pub fn convert_str(text: &str, to: DistTargetFormat, from: &str) -> Result<Conv, Error> {
    let source = declared(
        powerio_core::Source::from_memory("<memory>", text.as_bytes().to_vec())
            .map_err(|error| core_to_dist(&error))?,
        Some(from),
    )?;
    let module = powerio_dist::parse(source).map_err(|error| core_to_dist(&error))?;
    let mut conv = emit_module_with_options(&module, to, &powerio_dist::EmitOptions::default())
        .map_err(|error| core_to_dist(&error))?;
    let mut diagnostics = module.diagnostics.clone();
    diagnostics.append(&mut conv.diagnostics);
    conv.warnings = powerio_dist::diagnostics::render_diagnostics(&diagnostics);
    conv.diagnostics = diagnostics;
    Ok(conv)
}
