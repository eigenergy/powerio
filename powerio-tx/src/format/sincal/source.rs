//! Native binary acquisition and module assembly for the explicit profile.
use powerio_core::{Diagnostic, Error, FormatId, PioModule, Source};
use powerio_sincal::{AcquiredProject, DatabaseSnapshot};

use crate::BalancedNetwork;
use crate::diagnostics::codes;
use crate::format::routing::{TransmissionFormat, parse_transmission_format};

pub(in crate::format) fn handles(source: &Source) -> bool {
    match source.format() {
        Some(format) => {
            format.as_str().eq_ignore_ascii_case("sincal")
                || parse_transmission_format(format.as_str())
                    == Some(TransmissionFormat::SincalBalanced)
        }
        None => std::path::Path::new(source.name())
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("sinx")),
    }
}

pub(in crate::format) fn parse(source: Source) -> Result<PioModule<BalancedNetwork>, Error> {
    let selected = source
        .format()
        .and_then(|f| parse_transmission_format(f.as_str()));
    if selected != Some(TransmissionFormat::SincalBalanced) {
        return Err(Error::new(&codes::REQUEST_SINCAL_PROFILE_REQUIRED,
            "SINCAL can contain balanced or multiconductor networks; select format 'sincal-balanced' for an explicitly positive-sequence interpretation. Unbalanced inputs must use their conductor-resolved reader; there is no balancing fallback.")
            .with_source(source));
    }
    let acquired = AcquiredProject::read(&source).map_err(|e| {
        Error::new(&codes::PARSE_SINCAL_MALFORMED, e.to_string()).with_source(source.clone())
    })?;
    let retained = acquired
        .source
        .with_format(FormatId::new("sincal-balanced")?);
    let snapshot = DatabaseSnapshot::decode(acquired.database.bytes(), None).map_err(|e| {
        Error::new(&codes::PARSE_SINCAL_MALFORMED, e.to_string()).with_source(retained.clone())
    })?;
    let network = super::read_balanced_snapshot(&snapshot, source.name()).map_err(|e| {
        Error::new(&codes::PARSE_SINCAL_MALFORMED, e.to_string())
            .with_cause(e)
            .with_source(retained.clone())
    })?;
    let mut diagnostics = vec![
        Diagnostic::of(
            &codes::READ_SINCAL_CONVERSION_BASE,
            "The balanced profile maps native MW/kV/ohm quantities on a 100 MVA conversion base; this is not a native system base.",
        ),
        Diagnostic::of(
            &codes::READ_SINCAL_RETAINED_SOURCE_ONLY,
            "Fault, dynamic, protection, economic and diagram data, stored calculation results, and native settings outside the selected static load-flow profile remain only in retained source. They are not part of the typed balanced network.",
        ),
    ];
    if !network.generators().is_empty() {
        diagnostics.push(Diagnostic::of(&codes::READ_SINCAL_LIMITS_UNSPECIFIED,
            "Native generator capability limits are disabled in this profile; typed P/Q limits are unbounded. This does not establish OPF capability data."));
    }
    PioModule::parsed(network, retained, diagnostics)
}
