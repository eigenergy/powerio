//! Test-only compatibility wrappers keeping the unit suites on the old call
//! shapes while they exercise the module parse surface underneath.
#![allow(dead_code)]

use std::sync::Arc;

use crate::convert::{Conversion, DistTargetFormat};
use crate::model::MulticonductorNetwork;

/// The old parse output shape: the typed network with the reader's findings
/// riding along. Dereferences to the network so field access keeps its old
/// spelling; `warnings` and `source` shadow the fields the network lost.
/// Mutation through `DerefMut` edits a copy of the module's value, so the
/// module's echo tier stays byte exact.
#[derive(Debug)]
pub(crate) struct Parsed {
    pub warnings: Vec<String>,
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
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
    pub fn to_format(&self, target: DistTargetFormat) -> Conversion {
        crate::convert::write_as(&self.module, target)
    }

    /// Write from the typed value, bypassing the echo tier.
    pub fn to_canonical_format(&self, target: DistTargetFormat) -> Conversion {
        crate::convert::write_network(&self.network, target)
    }
}

fn from_module(module: powerio_core::PioModule<MulticonductorNetwork>) -> Parsed {
    let source = module.source().and_then(|source| {
        let buffer = source.primary_buffer().ok()?;
        let text = std::str::from_utf8(buffer.content_bytes()).ok()?;
        Some(Arc::new(text.to_owned()))
    });
    Parsed {
        warnings: crate::diagnostics::render_diagnostics(module.diagnostics()),
        diagnostics: module.diagnostics().to_vec(),
        source,
        network: module.value().clone(),
        module,
    }
}

fn declared(
    source: powerio_core::Source,
    from: Option<&str>,
) -> Result<powerio_core::Source, crate::Error> {
    match from {
        None => Ok(source),
        Some(token) => {
            // The old entries settled the format before any work; keep the
            // error shape.
            if crate::convert::dist_target_from_name(token).is_none() {
                return Err(crate::Error::UnknownFormat(token.to_string()));
            }
            let id = powerio_core::FormatId::new(token.to_ascii_lowercase().replace('_', "-"))
                .map_err(|_| crate::Error::UnknownFormat(token.to_string()))?;
            Ok(source.with_format(id))
        }
    }
}

fn core_to_dist(error: &powerio_core::Error) -> crate::Error {
    crate::Error::FormatRead {
        format: "case text",
        message: error.to_string(),
    }
}

pub(crate) fn parse_str(text: &str, from: &str) -> crate::Result<Parsed> {
    let source = declared(
        powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())
            .map_err(|error| core_to_dist(&error))?,
        Some(from),
    )?;
    crate::convert::parse(source)
        .map(from_module)
        .map_err(|error| core_to_dist(&error))
}

pub(crate) fn parse_file(
    path: impl AsRef<std::path::Path>,
    from: Option<&str>,
) -> crate::Result<Parsed> {
    if let Some(token) = from
        && crate::convert::dist_target_from_name(token).is_none()
    {
        return Err(crate::Error::UnknownFormat(token.to_string()));
    }
    let source = declared(
        powerio_core::Source::open(path.as_ref()).map_err(|error| core_to_dist(&error))?,
        from,
    )?;
    crate::convert::parse(source)
        .map(from_module)
        .map_err(|error| core_to_dist(&error))
}

/// The old infallible string entry: `.dss` text parses into the model with
/// filesystem includes refused (an in-memory source has no named buffers
/// here).
pub(crate) fn parse_dss_str(text: &str) -> Parsed {
    parse_str(text, "dss").expect("dss text parses without a filesystem")
}

pub(crate) fn parse_dss_file(path: impl AsRef<std::path::Path>) -> crate::Result<Parsed> {
    parse_file(path, Some("dss"))
}

pub(crate) fn parse_bmopf_str(text: &str) -> crate::Result<Parsed> {
    parse_str(text, "bmopf-json")
}

pub(crate) fn parse_pmd_str(text: &str) -> crate::Result<Parsed> {
    parse_str(text, "pmd-json")
}
