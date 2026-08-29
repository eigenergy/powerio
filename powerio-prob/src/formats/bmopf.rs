//! Map BMOPF JSON to [`McAcOpfInstance`].

use powerio_core::{Error, PioModule, Source};

use crate::instance::McAcOpfInstance;

/// Parse one BMOPF source into the multiconductor AC OPF instance it
/// defines: the multiconductor network with its exact per phase data, the
/// per phase cost objective the schema states, and every stated terminal
/// voltage, conductor, and per phase generator limit active. The module
/// retains the source and the reader's findings.
///
/// # Errors
/// An invalid document, or a network with no voltage source to anchor the
/// calculation; either failure retains the source and the findings.
pub fn parse_bmopf_instance(source: Source) -> Result<PioModule<McAcOpfInstance>, Error> {
    let source = match source.format() {
        Some(_) => source,
        None => source.with_format(powerio_core::FormatId::new("bmopf-json")?),
    };
    let mut module = powerio_dist::parse(source)?;
    let retained = module.take_source();
    let diagnostics = module.diagnostics().to_vec();
    match module.try_map_value(McAcOpfInstance::from_network) {
        Ok(mapped) => Ok(match retained {
            Some(source) => mapped.with_source(source),
            None => mapped,
        }),
        Err(error) => {
            let error = error.with_diagnostics(diagnostics);
            Err(match retained {
                Some(source) => error.with_source(source),
                None => error,
            })
        }
    }
}
