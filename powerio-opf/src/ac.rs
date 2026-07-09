//! AC-OPF instance data. Placeholder for the AC analog of [`DcOpfInstance`]:
//! the shape is sketched so downstream naming (Rust and, eventually, the C ABI
//! and PowerIO.jl binding) can settle early, but no builder exists yet. Building
//! one needs the full complex admittance rather than the DC susceptance
//! approximation [`build_dc_opf_instance`] draws from, voltage magnitude bounds,
//! and reactive generation bounds and costs alongside the active power data the
//! DC instance already carries.
//!
//! Input data only, like [`DcOpfInstance`]: no bus voltage variable
//! formulation, no solver or GPU assumptions. A consumer reads the instance and
//! builds its own AC-OPF model on top of it.
//!
//! [`DcOpfInstance`]: crate::DcOpfInstance
//! [`build_dc_opf_instance`]: crate::build_dc_opf_instance

/// Static AC-OPF instance data for a case. The field set is not finalized and
/// no constructor exists yet.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AcOpfInstance {
    /// Number of buses.
    pub n: usize,
    /// Number of in-service branches.
    pub m: usize,
}
