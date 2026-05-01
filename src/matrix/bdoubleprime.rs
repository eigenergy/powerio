//! FDPF `B''` matrix — reactive-power Jacobian.
//!
//! Per MATPOWER `makeB.m`:
//! - **XB scheme**: `B'' = -Im(Y_bus)` with phase shifts zeroed.
//! - **BX scheme**: `B'' = -Im(Y_bus)` with line resistance and phase shifts
//!   zeroed.
//!
//! Tap ratios, line charging, and bus shunts are kept in both schemes —
//! they are what give B″ its strict diagonal dominance (full rank).

use sprs::CsMat;

use crate::case::MpcCase;
use crate::Result;

use super::ybus::{YbusFlags, build_ybus_with_flags};
use super::{BuildOptions, Scheme};

pub fn build_bdoubleprime(case: &MpcCase, opts: &BuildOptions) -> Result<CsMat<f64>> {
    let flags = YbusFlags {
        zero_resistance: matches!(opts.scheme, Scheme::Bx),
        zero_charging: false,
        unity_taps: false,
        zero_shifts: true,
        skip_bus_shunts: false,
    };
    let parts = build_ybus_with_flags(case, flags)?;
    Ok(negate_matrix(&parts.b))
}

fn negate_matrix(a: &CsMat<f64>) -> CsMat<f64> {
    let mut out = a.clone();
    for v in out.data_mut() {
        *v = -*v;
    }
    out
}
