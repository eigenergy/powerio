//! MATPOWER-compatible FDPF `Bpp` matrix.
//!
//! In fast decoupled power flow, `Bpp` is the fixed approximation to the
//! reactive power versus voltage magnitude Jacobian block used for the Q step.
//!
//! Per MATPOWER `makeB.m`:
//! - **XB scheme**: `Bpp = -Im(Y_bus)` with phase shifts zeroed.
//! - **BX scheme**: `Bpp = -Im(Y_bus)` with line resistance and phase shifts
//!   zeroed.
//!
//! Tap ratios, line charging, and bus shunts are kept in both schemes.

use sprs::CsMat;

use crate::Result;
use crate::indexed::IndexedNetwork;

use super::ybus::{YbusFlags, build_ybus_with_flags};
use super::{BuildOptions, Scheme, negate_into};

pub fn build_bdoubleprime(case: &IndexedNetwork, opts: &BuildOptions) -> Result<CsMat<f64>> {
    let flags = YbusFlags {
        zero_resistance: matches!(opts.scheme, Scheme::Bx),
        zero_charging: false,
        unity_taps: false,
        zero_shifts: true,
        skip_bus_shunts: false,
        skip_zero_impedance: opts.skip_zero_impedance,
        skip_self_loops: false,
    };
    // `parts.b` is owned and discarded here, so negate it in place rather than
    // cloning the structure.
    let parts = build_ybus_with_flags(case, flags)?;
    Ok(negate_into(parts.b))
}
