//! The PowerModelsDistribution ENGINEERING model as JSON ("PMD JSON").
//!
//! The byte conventions follow PMD's own `print_file`/`parse_file` pair:
//! matrices as arrays of arrays read back via `hcat` (inner arrays are
//! columns), `Inf`/`NaN` as `null` restored by field suffix (`_ub`/`max`
//! to +Inf, `_lb`/`min` to -Inf, anything else NaN), enums as uppercase
//! strings, kV and kW scales with angles in degrees, meters for lengths,
//! per unit transformer impedances, and integer terminals with grounding
//! as `grounded` plus `rg`/`xg` on the bus.
//!
//! A multinetwork document (PMD's `make_multinetwork` output, with
//! `"multinetwork": true` and one network per time step under `nw`) is a
//! time series rather than one network: [`parse_multinetwork`] reads it as a
//! [`TimeSeries`] of networks, and [`crate::parse`] refuses it with a message
//! that says so.

pub mod math;
pub(crate) mod read;
mod write;

pub use math::PmdMathEmitOptions;
pub(crate) use write::emit_pmd_json_text;

use powerio_core::{PioModule, Source, TimeSeries};

use crate::MulticonductorNetwork;

/// Whether the source's primary document is a PMD multinetwork ENGINEERING
/// model (`"multinetwork": true`). The check reads the one flag without
/// building the document tree. A source that cannot be read, or is not a
/// JSON object, is not one.
#[must_use]
pub fn is_multinetwork(source: &Source) -> bool {
    crate::convert::primary(source).is_ok_and(|buffer| {
        crate::convert::source_text(&buffer).is_ok_and(read::text_is_multinetwork)
    })
}

/// Parse a PMD multinetwork ENGINEERING document into one
/// [`MulticonductorNetwork`] per time step. Time points follow the document's
/// `mn_lookup` times, are labeled by their `nw` keys, and carry each step's
/// `time_elapsed` as their duration. The source is retained on the module,
/// so a same format emission echoes the input bytes.
///
/// # Errors
/// The document is not JSON, not the ENGINEERING model, not a multinetwork
/// document, or holds no time step. The failure carries the reader's
/// findings and retains the source.
pub fn parse_multinetwork(
    source: Source,
) -> Result<PioModule<TimeSeries<MulticonductorNetwork>>, powerio_core::Error> {
    let mut warnings = crate::collect::Diagnostics::new();
    let parsed = crate::convert::primary(&source).and_then(|buffer| {
        read::parse_pmd_multinetwork_collecting(
            crate::convert::source_text(&buffer)?,
            &mut warnings,
        )
    });
    match parsed {
        Ok(series) => {
            let source = match powerio_core::FormatId::new("pmd-json") {
                Ok(format) => source.with_format(format),
                Err(_) => source,
            };
            PioModule::parsed(series, source, warnings.into_records())
        }
        Err(error) => {
            let core = powerio_core::Error::new(error.code(), error.to_string());
            Err(core
                .with_diagnostics(warnings.into_records())
                .with_cause(error)
                .with_source(source))
        }
    }
}
