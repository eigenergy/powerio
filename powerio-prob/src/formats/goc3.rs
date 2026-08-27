//! Map DOE GO Challenge 3 JSON to [`AcScucInstance`].

use powerio_core::{Error, PioModule, Source};

use super::source_text;
use crate::diagnostics::codes;
use crate::instance::AcScucInstance;
use crate::scopf::{ScopfError, parse_scopf_str};

/// Parse one GO Challenge 3 input source into the AC security constrained
/// unit commitment instance: the balanced network the document describes plus
/// the complete typed scheduling categories. The module retains the source
/// and the reader's findings.
///
/// The network comes from the hub's GOC3 reader and the categories from the
/// typed GO Challenge 3 projection; both read the same retained document, and
/// the bus identities the two halves resolved are reconciled here before the
/// pair is accepted as one instance.
///
/// # Errors
/// An invalid document from either half, or bus identities that disagree
/// between the two; every failure retains the source.
pub fn parse_goc3_instance(source: Source) -> Result<PioModule<AcScucInstance>, Error> {
    match parse_goc3_text(&source) {
        Ok((instance, diagnostics)) => PioModule::parsed(instance, source, diagnostics),
        Err(error) => Err(error.with_source(source)),
    }
}

fn parse_goc3_text(
    source: &Source,
) -> Result<(AcScucInstance, Vec<powerio_core::Diagnostic>), Error> {
    let name = source.name().to_owned();
    let buffer = source.primary_buffer()?;
    let content = source_text(&buffer)?;
    let (network, diagnostics, _document) = powerio_tx::parse_goc3_json(content)
        .map_err(|error| Error::new(error.code(), format!("{name}: {error}")))?;
    let inputs = parse_scopf_str(content, "goc3-json").map_err(|error| scopf_error(&error))?;

    // The two halves resolved bus identities independently; the instance is
    // only coherent when they resolved the same table the same way.
    let stated = inputs.static_data.bus.len();
    if stated != network.buses().len() {
        return Err(Error::new(
            &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
            format!(
                "{name}: the typed categories state {stated} buses; the network states {}",
                network.buses().len()
            ),
        ));
    }
    for (row, bus) in inputs.static_data.bus.iter().zip(network.buses()) {
        if row.i != bus.id {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                format!(
                    "{name}: bus \"{}\" resolved to id {} in the categories and {} in the network",
                    row.uid, row.i.0, bus.id.0
                ),
            ));
        }
    }

    let instance = AcScucInstance::new(network, inputs)?;
    Ok((instance, diagnostics))
}

fn scopf_error(error: &ScopfError) -> Error {
    Error::new(error.code(), error.to_string())
}
