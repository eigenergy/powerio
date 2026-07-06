//! MATPOWER-compatible FDPF `Bp` matrix.
//!
//! In fast decoupled power flow, `Bp` is the fixed approximation to the active
//! power versus voltage angle Jacobian block used for the P step.
//!
//! Per MATPOWER `makeB.m`, `Bp` is built as `-Im(Y_bus)` after modifying the
//! network data used for that one matrix:
//!
//! - bus shunts are cleared
//! - line charging is cleared
//! - tap magnitudes are set to one
//! - line resistance is cleared in the XB scheme
//! - phase shifts remain
//!
//! With zero phase shifts this has the usual Laplacian sign pattern: positive
//! diagonal entries and negative off diagonal entries. Phase shifters change
//! the off diagonal terms, matching MATPOWER.

use sprs::CsMat;

use crate::Result;
use crate::indexed::IndexedNetwork;

use super::ybus::{YbusFlags, build_ybus_with_flags};
use super::{BuildOptions, Scheme, negate_into};

pub fn build_bprime(case: &IndexedNetwork, opts: &BuildOptions) -> Result<CsMat<f64>> {
    let flags = YbusFlags {
        zero_resistance: matches!(opts.scheme, Scheme::Xb),
        zero_charging: true,
        unity_taps: true,
        zero_shifts: false,
        skip_bus_shunts: true,
        skip_zero_impedance: opts.skip_zero_impedance,
        skip_self_loops: true,
    };
    let parts = build_ybus_with_flags(case, flags)?;
    Ok(negate_into(parts.b))
}
