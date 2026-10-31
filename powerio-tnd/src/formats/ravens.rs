//! MG-RAVENS, the LANL CIM-derived microgrid interchange schema
//! (<https://github.com/lanl-ansi/MG-RAVENS>), across both electrical
//! models.
//!
//! One token, `ravens-json`, names the format. The balanced profile, the
//! register the upstream MATPOWER converter writes, is read and written by
//! `powerio-tx` and parses to a [`BalancedNetwork`]. The multiconductor
//! profile, the register the upstream OpenDSS converter writes, is read and
//! written by `powerio-dist` and parses to a [`MulticonductorNetwork`]. The
//! two readers each refuse the other's profile. This adapter is where they
//! meet: [`parse`] reads the profile from the document
//! ([`powerio_core::cim::ravens::detect_profile`]) and hands the source to
//! the crate that owns it, so a caller need not know which register a file
//! uses. The `powerio` facade routes the token here.
//!
//! A document carrying both profiles' classes is refused with
//! `PARSE.RAVENS.MIXED_PROFILE`. Reading such a document into an
//! [`IntegratedNetwork`](crate::IntegratedNetwork), with the balanced
//! classes as the transmission module and the per-phase classes as feeders,
//! is the follow-up this adapter is placed for.

pub use powerio_core::cim::ravens::{
    BALANCED_MARKERS, MULTICONDUCTOR_MARKERS, NAMESPACE, Profile, TOKEN, detect_profile,
};
use powerio_core::{Error, FormatId, PioModule, Source};
use powerio_dist::MulticonductorNetwork;
use powerio_tx::BalancedNetwork;

/// A parsed MG-RAVENS document: the module of whichever network kind its
/// profile selects.
#[derive(Debug)]
pub enum RavensModule {
    /// The balanced profile, read by `powerio-tx`.
    Balanced(PioModule<BalancedNetwork>),
    /// The multiconductor profile, read by `powerio-dist`.
    Multiconductor(PioModule<MulticonductorNetwork>),
}

impl RavensModule {
    /// The profile the document was read as.
    #[must_use]
    pub fn profile(&self) -> Profile {
        match self {
            Self::Balanced(_) => Profile::Balanced,
            Self::Multiconductor(_) => Profile::Multiconductor,
        }
    }
}

/// The profile of a source's primary document.
///
/// # Errors
/// The source cannot be read, or its document is not UTF-8 text.
pub fn profile_of(source: &Source) -> Result<Profile, Error> {
    let buffer = source.primary_buffer()?;
    let text = std::str::from_utf8(buffer.content_bytes()).map_err(|e| {
        Error::new(
            &powerio_tx::diagnostics::codes::PARSE_SOURCE_MALFORMED,
            format!("MG-RAVENS JSON is not UTF-8 text: {e}"),
        )
    })?;
    Ok(detect_profile(text))
}

/// Parse an MG-RAVENS document into the module its profile selects. The
/// source is declared `ravens-json` on the way in, so an extension or an
/// alias the caller passed does not matter; the module retains the source
/// for the byte exact echo on a same format write.
///
/// # Errors
/// The source cannot be read, the document carries both profiles
/// (`PARSE.RAVENS.MIXED_PROFILE`), or the owning reader refuses it, with that
/// reader's findings and the source attached.
pub fn parse(source: Source) -> Result<RavensModule, Error> {
    let source = source.with_format(FormatId::new(TOKEN)?);
    match profile_of(&source) {
        Ok(Profile::Multiconductor) => {
            powerio_dist::parse(source).map(RavensModule::Multiconductor)
        }
        Ok(Profile::Mixed) => Err(Error::new(
            &crate::diagnostics::PARSE_RAVENS_MIXED_PROFILE,
            "the document carries classes of both MG-RAVENS profiles (the balanced \
             AlgorithmSettings, ProducerCostFunction, or PowerTransformerEnd beside per-phase \
             or tank classes); no reader takes both, and reading the two parts into an \
             IntegratedNetwork is not implemented",
        )
        .with_source(source)),
        Ok(_) => powerio_tx::parse(source).map(RavensModule::Balanced),
        Err(error) => Err(error.with_source(source)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(text: &str) -> Source {
        Source::from_memory("case.json", text.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn a_mixed_document_is_refused_with_its_own_code() {
        let text = r#"{
            "ConnectivityNode": {"a": {"Ravens.cimObjectType": "ConnectivityNode"}},
            "PerLengthLineParameter": {},
            "MySettings": {"Ravens.cimObjectType": "AlgorithmSettings"}
        }"#;
        let error = parse(memory(text)).unwrap_err();
        assert_eq!(
            error.info().map(|info| info.code),
            Some("PARSE.RAVENS.MIXED_PROFILE"),
            "{error}"
        );
    }

    #[test]
    fn the_multiconductor_example_reads_as_a_multiconductor_module() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/data/dist/ravens/case3_balanced.json"
        );
        let module = parse(Source::open(path).unwrap()).unwrap();
        assert_eq!(module.profile(), Profile::Multiconductor);
        let RavensModule::Multiconductor(module) = module else {
            unreachable!("profile checked");
        };
        // The finite-impedance source has its own internal EMF node.
        assert_eq!(module.value().buses().len(), 4);
        assert_eq!(
            module
                .source()
                .and_then(|s| s.format())
                .map(FormatId::as_str),
            Some(TOKEN)
        );
    }
}
