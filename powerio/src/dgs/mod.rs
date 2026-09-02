//! PowerFactory DGS routing between the balanced and multiconductor
//! networks.
//!
//! The transmission crate decodes a DGS export and decides which family its
//! objects justify ([`powerio_tx::format::dgs::route`]). A sequence data
//! export parses there into a [`BalancedNetwork`](crate::BalancedNetwork).
//! An export with conductor level data parses here, in the one crate that
//! owns both families, into a
//! [`MulticonductorNetwork`](powerio_dist::MulticonductorNetwork).

mod multiconductor;

use powerio_core::{Diagnostic, Error, FormatId, PioModule, Source};
use powerio_tx::diagnostics::codes;
use powerio_tx::format::dgs::{DgsDocument, DgsRoute, SpanContext, route, summarize_markers};

use crate::value::PioValue;

/// Parse a DGS export into whichever network family it justifies.
///
/// # Errors
/// The decoded document's failure with the retained source, a
/// `READ.DGS.ROUTE_UNDECIDED` failure for an export with no topology, and
/// the balanced parser's own failures.
pub(crate) fn parse_dgs(source: Source) -> Result<PioModule<PioValue>, Error> {
    let source = match source.format() {
        Some(_) => source,
        None => source.with_format(FormatId::new("dgs")?),
    };
    let buffer = match source.primary_buffer() {
        Ok(buffer) => buffer,
        Err(error) => return Err(error.with_source(source)),
    };
    let Ok(text) = std::str::from_utf8(buffer.content_bytes()) else {
        return Err(Error::new(
            &codes::PARSE_SOURCE_MALFORMED,
            "the DGS export is not valid UTF-8; DGS V5 ASCII exports are read as UTF-8",
        )
        .with_source(source));
    };
    let document = match DgsDocument::parse(text) {
        Ok(document) => document,
        Err(error) => {
            return Err(Error::new(error.code(), error.to_string())
                .with_cause(error)
                .with_source(source));
        }
    };
    match route(&document) {
        DgsRoute::Balanced => {
            drop(buffer);
            powerio_tx::format::parse_with_json_class(source, None)
                .map(|module| module.map_value(PioValue::from))
        }
        DgsRoute::Undecided(reason) => {
            Err(Error::new(&codes::READ_DGS_ROUTE_UNDECIDED, reason).with_source(source))
        }
        DgsRoute::Multiconductor(markers) => {
            let spans = SpanContext {
                source: buffer.id().clone(),
                offset: (buffer.bytes().len() - buffer.content_bytes().len()) as u64,
            };
            let name = std::path::Path::new(source.name());
            let stem = if source.name().starts_with('<') {
                None
            } else {
                name.file_stem().and_then(|stem| stem.to_str())
            };
            let (network, mut diagnostics) = multiconductor::build(&document, stem, &spans);
            diagnostics.insert(
                0,
                Diagnostic::of(
                    &codes::READ_DGS_ROUTED_MULTICONDUCTOR,
                    format!(
                        "the export carries conductor level data ({}) and was read as a \
                         multiconductor network",
                        summarize_markers(&markers)
                    ),
                ),
            );
            PioModule::parsed(PioValue::from(network), source, diagnostics)
        }
    }
}

/// Whether a declared format token names DGS or a `.pfd` project.
pub(crate) fn is_dgs_token(token: &str) -> bool {
    powerio_tx::format::routing::parse_transmission_format(token)
        == Some(powerio_tx::format::routing::TransmissionFormat::Dgs)
}

/// The `.pfd` refusal as a coded failure carrying the source.
pub(crate) fn encrypted_project(source: Source) -> Error {
    Error::new(
        &codes::READ_DGS_ENCRYPTED_PROJECT,
        powerio_tx::format::dgs::encrypted_project_message(source.name()),
    )
    .with_source(source)
}
