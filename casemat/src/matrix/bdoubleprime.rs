//! FDPF `B''` matrix. Reactive power Jacobian.
//!
//! Per MATPOWER `makeB.m`:
//! - **XB scheme**: `B'' = -Im(Y_bus)` with phase shifts zeroed.
//! - **BX scheme**: `B'' = -Im(Y_bus)` with line resistance and phase shifts
//!   zeroed.
//!
//! Tap ratios, line charging, and bus shunts are kept in both schemes —
//! they are what give B″ its strict diagonal dominance (full rank).

use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::Result;

use super::ybus::{YbusFlags, build_ybus_with_flags};
use super::{negate_into, BuildOptions, Scheme};

pub fn build_bdoubleprime(case: &IndexedNetwork, opts: &BuildOptions) -> Result<CsMat<f64>> {
    let flags = YbusFlags {
        zero_resistance: matches!(opts.scheme, Scheme::Bx),
        zero_charging: false,
        unity_taps: false,
        zero_shifts: true,
        skip_bus_shunts: false,
    };
    // `parts.b` is owned and discarded here, so negate it in place rather than
    // cloning the structure.
    let parts = build_ybus_with_flags(case, flags)?;
    Ok(negate_into(parts.b))
}
