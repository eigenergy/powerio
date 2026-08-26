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
    powerio::parse(source)
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
    tx_error_to_core(powerio::Error::FormatRead {
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

pub(crate) fn tx_error_to_core(error: powerio::Error) -> powerio_core::Error {
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
    powerio::parse(source).map(module_to_parsed)
}

fn module_to_parsed(module: powerio_core::PioModule<BalancedNetwork>) -> ParsedCase {
    ParsedCase {
        diagnostics: module.diagnostics().to_vec(),
        retained_source: module.source().is_some(),
        network: module.into_value(),
        document: None,
    }
}
