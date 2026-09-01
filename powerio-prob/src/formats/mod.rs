//! Format to calculation mappings: sources that define a particular
//! calculation parse to the corresponding instance or solution rather than
//! to a bare network.
//!
//! A format does not become a calculation instance merely because it contains
//! enough parameters to construct one. The GO Challenge 3 data format was
//! written for Challenge 3 and later uses. PowerIO maps its Challenge 3
//! input/problem data file to [`AcScucInstance`] because the required data
//! supplies the AC security constrained unit commitment inputs. Optional format fields outside the
//! Challenge 3 formulation do not become SCUC inputs merely because they are
//! present.
//! DeepMind OPFData explicitly represents a solved AC OPF, so it maps to
//! [`AcOpfSolution`]. BMOPF JSON parses in `powerio-dist` to a
//! `MulticonductorNetwork`; callers construct the required calculation
//! instance explicitly.
//!
//! [`AcScucInstance`]: crate::AcScucInstance
//! [`AcOpfSolution`]: crate::AcOpfSolution

mod goc3;
mod opfdata;
mod pypsa;

#[doc(hidden)]
pub use goc3::{__emit_goc3_output, __parse_goc3_output_buffer, __parse_goc3_problem_buffer};
#[doc(hidden)]
pub use opfdata::__decode_opfdata_solution;
#[doc(hidden)]
pub use pypsa::{__decode_pypsa_sequence, PypsaSequence};

/// The text a reader decodes: the buffer's byte order mark free slice,
/// validated as UTF-8.
pub(crate) fn source_text(
    buffer: &powerio_core::SourceBuffer,
) -> Result<&str, powerio_core::Error> {
    std::str::from_utf8(buffer.content_bytes()).map_err(|error| {
        let cause = powerio_tx::Error::FormatRead {
            format: "case text",
            message: format!("not valid UTF-8: {error}"),
        };
        powerio_core::Error::new(cause.code(), cause.to_string())
    })
}
