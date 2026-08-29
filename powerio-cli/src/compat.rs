//! Transitional parse shapes for the CLI while its commands move onto the
//! module surface. TEMPORARY: the module carries retained source and
//! findings; these helpers project the old network-plus-findings shape.

use std::path::Path;

use powerio::BalancedNetwork;
use powerio::diagnostics::Diagnostic;

pub(crate) struct ParsedCase {
    pub network: BalancedNetwork,
    pub diagnostics: Vec<Diagnostic>,
    /// Whether the parse retained its source bytes on the module, the fact
    /// the 0.9 package provenance records. The binary reads it; the corpus
    /// library does not.
    #[allow(dead_code)]
    pub retained_source: bool,
    /// The typed DOE GO Challenge 3 document, when that reader produced one.
    /// TEMPORARY: the calculation instance types replace this hand-off. The
    /// binary reads it; the corpus library does not.
    #[allow(dead_code)]
    pub document: Option<std::sync::Arc<powerio::format::goc3::Goc3Document>>,
}

impl ParsedCase {
    pub(crate) fn rendered_diagnostics(&self) -> Vec<String> {
        powerio::diagnostics::render_diagnostics(&self.diagnostics)
    }
}

pub(crate) fn declared(
    source: powerio_core::Source,
    from: Option<&str>,
) -> Result<powerio_core::Source, powerio_core::Error> {
    match from {
        None => Ok(source),
        Some(token) => Ok(source.with_format(powerio_core::FormatId::new(
            token.to_ascii_lowercase().replace('_', "-"),
        )?)),
    }
}

pub(crate) fn parse_module(
    path: impl AsRef<Path>,
    from: Option<&str>,
) -> Result<powerio_core::PioModule<BalancedNetwork>, powerio_core::Error> {
    let source = declared(powerio_core::Source::open(path.as_ref())?, from)?;
    powerio::format::parse(source)
}

pub(crate) fn parse_file(
    path: impl AsRef<Path>,
    from: Option<&str>,
) -> Result<ParsedCase, powerio_core::Error> {
    let path = path.as_ref();
    let is_goc3 = from.is_some_and(|token| token.eq_ignore_ascii_case("goc3-json"))
        || (from.is_none()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("goc3")));
    if is_goc3 {
        let source = powerio_core::Source::open(path)?;
        let buffer = source.primary_buffer()?;
        let text = std::str::from_utf8(buffer.content_bytes())
            .map_err(|_| invalid_text(path))?
            .to_owned();
        return goc3_parsed(&text);
    }
    parse_module(path, from).map(module_to_parsed)
}

fn invalid_text(path: &Path) -> powerio_core::Error {
    tx_error_to_core(powerio::error::Error::FormatRead {
        format: "case text",
        message: format!("{} is not valid UTF-8", path.display()),
    })
}

fn goc3_parsed(text: &str) -> Result<ParsedCase, powerio_core::Error> {
    let (network, diagnostics, document) =
        powerio::parse_goc3_json(text).map_err(tx_error_to_core)?;
    Ok(ParsedCase {
        network,
        diagnostics,
        retained_source: false,
        document: Some(document),
    })
}

pub(crate) fn tx_error_to_core(error: powerio::error::Error) -> powerio_core::Error {
    powerio_core::Error::new(error.code(), error.to_string()).with_cause(error)
}

#[allow(dead_code)] // the corpus library uses it; the binary does not
pub(crate) fn parse_str(text: &str, from: &str) -> Result<ParsedCase, powerio_core::Error> {
    parse_str_with_name(text, from, None)
}

pub(crate) fn parse_str_with_name(
    text: &str,
    from: &str,
    name_hint: Option<&str>,
) -> Result<ParsedCase, powerio_core::Error> {
    if from.eq_ignore_ascii_case("goc3-json") {
        return goc3_parsed(text);
    }
    let name = name_hint.map_or_else(|| "<memory>".to_owned(), str::to_owned);
    let source = declared(
        powerio_core::Source::from_bytes(name, text.as_bytes().to_vec())?,
        Some(from),
    )?;
    powerio::format::parse(source).map(module_to_parsed)
}

fn module_to_parsed(module: powerio_core::PioModule<BalancedNetwork>) -> ParsedCase {
    ParsedCase {
        diagnostics: module.diagnostics().to_vec(),
        retained_source: module.source().is_some(),
        network: module.into_value(),
        document: None,
    }
}

/// The old distribution parse output shape: the typed network beside the
/// reader's findings, with the parsed module riding along so a same format
/// write echoes the retained source.
#[allow(dead_code)] // the binary uses every field; the corpus library reads two
pub(crate) struct ParsedDist {
    pub warnings: Vec<String>,
    pub diagnostics: Vec<powerio_dist::Diagnostic>,
    pub retained_source: bool,
    pub module: powerio_core::PioModule<powerio_dist::MulticonductorNetwork>,
    pub network: powerio_dist::MulticonductorNetwork,
}

impl ParsedDist {
    /// Write through the module: a same format target echoes the retained
    /// source bytes exactly.
    #[allow(dead_code)] // the binary's convert path; the corpus library writes canonically
    pub fn to_format(&self, target: powerio_dist::DistTargetFormat) -> powerio_dist::Conversion {
        powerio_dist::write_as(&self.module, target)
    }
}

pub(crate) fn dist_parse_file(
    path: &std::path::Path,
    from: Option<&str>,
) -> Result<ParsedDist, powerio_core::Error> {
    let mut source = powerio_core::Source::open(path)?;
    if let Some(token) = from {
        source = source.with_format(powerio_core::FormatId::new(
            token.to_ascii_lowercase().replace('_', "-"),
        )?);
    }
    powerio_dist::parse(source).map(dist_module_to_parsed)
}

pub(crate) fn dist_parse_str(text: &str, from: &str) -> Result<ParsedDist, powerio_core::Error> {
    let source = powerio_core::Source::from_bytes("<memory>", text.as_bytes().to_vec())?
        .with_format(powerio_core::FormatId::new(
            from.to_ascii_lowercase().replace('_', "-"),
        )?);
    powerio_dist::parse(source).map(dist_module_to_parsed)
}

fn dist_module_to_parsed(
    module: powerio_core::PioModule<powerio_dist::MulticonductorNetwork>,
) -> ParsedDist {
    ParsedDist {
        warnings: powerio_dist::diagnostics::render_diagnostics(module.diagnostics()),
        diagnostics: module.diagnostics().to_vec(),
        retained_source: module.source().is_some(),
        network: module.value().clone(),
        module,
    }
}
