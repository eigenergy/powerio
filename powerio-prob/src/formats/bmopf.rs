//! Map BMOPF JSON to [`McAcOpfInstance`].

use powerio_core::{Diagnostic, Error};

use crate::instance::McAcOpfInstance;

/// Parse one BMOPF document into the multiconductor AC OPF instance it
/// defines: the multiconductor network with its exact per phase data, the
/// per phase cost objective the schema states, and every stated terminal
/// voltage, conductor, and per phase generator limit active. `name` labels
/// the source in errors.
///
/// # Errors
/// An invalid document, or a network with no voltage source to anchor the
/// calculation.
pub fn parse_bmopf_instance(
    content: &str,
    name: &str,
) -> Result<(McAcOpfInstance, Vec<Diagnostic>), Error> {
    let source = powerio_core::Source::from_bytes(name, content.as_bytes().to_vec())?
        .with_format(powerio_core::FormatId::new("bmopf-json")?);
    let module = powerio_dist::parse(source)?;
    let diagnostics = module.diagnostics().to_vec();
    let instance = McAcOpfInstance::from_network(module.into_value())?;
    Ok((instance, diagnostics))
}
