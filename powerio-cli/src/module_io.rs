use std::path::Path;

use powerio::{BalancedNetwork, Diagnostic};

#[allow(dead_code)] // The binary consumes these fields; the library target does not.
pub(crate) struct MemorySidecar {
    pub path: String,
    pub bytes: Vec<u8>,
}

pub(crate) struct MemoryEmission {
    pub text: String,
    #[allow(dead_code)] // The binary consumes sidecars; the library target does not.
    pub sidecars: Vec<MemorySidecar>,
    pub diagnostics: Vec<Diagnostic>,
}

impl MemoryEmission {
    pub(crate) fn render_diagnostics(&self) -> Vec<String> {
        powerio_core::render_diagnostics(&self.diagnostics)
    }
}

fn unpack_memory_emission(
    result: powerio_core::EmitResult,
    primary_name: Option<&str>,
) -> anyhow::Result<MemoryEmission> {
    let diagnostics = result.diagnostics().to_vec();
    let powerio_core::EmittedOutput::Memory { artifacts } = result.into_output() else {
        anyhow::bail!("a memory destination returned path output")
    };
    let single_artifact = artifacts.len() == 1;
    let mut text = None;
    let mut sidecars = Vec::new();
    for artifact in artifacts {
        let full_name = artifact.name().as_str().to_owned();
        let name = full_name
            .strip_prefix("output/")
            .unwrap_or(&full_name)
            .to_owned();
        let bytes = artifact.into_bytes();
        let primary = primary_name.map_or(single_artifact, |expected| name == expected);
        if primary {
            text = Some(String::from_utf8(bytes).map_err(|error| {
                anyhow::anyhow!("format serializer returned non-UTF-8 text: {error}")
            })?);
        } else {
            sidecars.push(MemorySidecar { path: name, bytes });
        }
    }
    let text = text.ok_or_else(|| anyhow::anyhow!("emission did not contain its primary text"))?;
    Ok(MemoryEmission {
        text,
        sidecars,
        diagnostics,
    })
}

pub(crate) fn emit_balanced_module(
    module: &powerio_core::PioModule<BalancedNetwork>,
    target: powerio_tx::TargetFormat,
    options: &powerio_tx::EmitOptions,
) -> anyhow::Result<MemoryEmission> {
    let result = powerio_tx::emit_with_options(
        module,
        target,
        options,
        powerio_core::Destination::memory("output")?,
    )?;
    unpack_memory_emission(result, None)
}

pub(crate) fn emit_multiconductor_module(
    module: &powerio_core::PioModule<powerio_dist::MulticonductorNetwork>,
    target: powerio_dist::DistTargetFormat,
) -> anyhow::Result<MemoryEmission> {
    emit_multiconductor_module_with_options(module, target, &powerio_dist::EmitOptions::default())
}

pub(crate) fn emit_multiconductor_module_with_options(
    module: &powerio_core::PioModule<powerio_dist::MulticonductorNetwork>,
    target: powerio_dist::DistTargetFormat,
    options: &powerio_dist::EmitOptions,
) -> anyhow::Result<MemoryEmission> {
    let result = powerio_dist::emit_with_options(
        module,
        target,
        options,
        powerio_core::Destination::memory("output")?,
    )?;
    let primary = matches!(target, powerio_dist::DistTargetFormat::Dss).then_some("case.dss");
    unpack_memory_emission(result, primary)
}

fn declare_format(
    source: powerio_core::Source,
    format: Option<&str>,
) -> Result<powerio_core::Source, powerio_core::Error> {
    match format {
        Some(format) => Ok(source.with_format(powerio_core::FormatId::new(format)?)),
        None => Ok(source),
    }
}

pub(crate) fn load_balanced_module(
    path: impl AsRef<Path>,
    format: Option<&str>,
) -> Result<powerio_core::PioModule<BalancedNetwork>, powerio_core::Error> {
    let source = declare_format(powerio_core::Source::open(path.as_ref())?, format)?;
    powerio_tx::parse(source)
}

#[allow(dead_code)] // The corpus library uses it; the binary does not.
pub(crate) fn load_balanced_memory(
    text: &str,
    format: &str,
) -> Result<powerio_core::PioModule<BalancedNetwork>, powerio_core::Error> {
    load_balanced_memory_named(text, format, None)
}

pub(crate) fn load_balanced_memory_named(
    text: &str,
    format: &str,
    name_hint: Option<&str>,
) -> Result<powerio_core::PioModule<BalancedNetwork>, powerio_core::Error> {
    let name = name_hint.map_or_else(|| "<memory>".to_owned(), str::to_owned);
    let source = powerio_core::Source::from_memory(name, text.as_bytes().to_vec())?;
    powerio_tx::parse(declare_format(source, Some(format))?)
}

pub(crate) fn load_multiconductor_module(
    path: &Path,
    format: Option<&str>,
) -> Result<powerio_core::PioModule<powerio_dist::MulticonductorNetwork>, powerio_core::Error> {
    let source = declare_format(powerio_core::Source::open(path)?, format)?;
    powerio_dist::parse(source)
}

pub(crate) fn load_multiconductor_memory(
    text: &str,
    format: &str,
) -> Result<powerio_core::PioModule<powerio_dist::MulticonductorNetwork>, powerio_core::Error> {
    let source = powerio_core::Source::from_memory("<memory>", text.as_bytes().to_vec())?;
    powerio_dist::parse(declare_format(source, Some(format))?)
}
