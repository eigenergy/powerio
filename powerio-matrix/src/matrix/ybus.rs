//! Bus admittance matrix `Y_bus = G + jB` per MATPOWER's `makeYbus`.
//!
//! For each in-service branch from bus `i` to bus `j` with series impedance
//! `z = r + jx`, total line charging `b`, complex tap `a = tap * exp(j shift)`:
//!
//! ```text
//! Y[i,i] += (1/z + j b/2) / |a|^2
//! Y[j,j] += (1/z + j b/2)
//! Y[i,j] += -(1/z) / conj(a)
//! Y[j,i] += -(1/z) / a
//! ```
//!
//! Plus bus shunts `Y[i,i] += (g_s + j b_s) / baseMVA`.

use num_complex::Complex64;
use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::{Error, Result};

use super::triplet::CooBuilder;

/// `Re(Y_bus)` and `Im(Y_bus)` as separate CSR matrices.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct YbusParts {
    pub g: CsMat<f64>,
    pub b: CsMat<f64>,
}

/// Internal flags used to derive B', B'' from `Y_bus` per MATPOWER `makeB`.
// Five independent on/off switches into one Y_bus kernel; an enum per pair
// would just spread the same state across more types.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct YbusFlags {
    pub zero_resistance: bool,
    pub zero_charging: bool,
    pub unity_taps: bool,
    pub zero_shifts: bool,
    pub skip_bus_shunts: bool,
}

pub fn build_ybus(case: &IndexedNetwork, opts: &super::BuildOptions) -> Result<YbusParts> {
    let flags = YbusFlags {
        zero_resistance: false,
        zero_charging: false,
        unity_taps: !opts.include_taps,
        zero_shifts: !opts.include_shifts,
        skip_bus_shunts: false,
    };
    build_ybus_with_flags(case, flags)
}

// i/j bus indices, r/x impedance, a complex tap: the single-letter names are
// the standard makeYbus notation and the math reads worse spelled out.
#[allow(clippy::many_single_char_names)]
pub(crate) fn build_ybus_with_flags(case: &IndexedNetwork, flags: YbusFlags) -> Result<YbusParts> {
    let n = case.n();
    let mut g_coo = CooBuilder::with_capacity(n, 4 * case.branches().len() + n);
    let mut b_coo = CooBuilder::with_capacity(n, 4 * case.branches().len() + n);

    for (row_idx, br) in case.in_service_branches() {
        let i = case.bus_index(br.from).ok_or(Error::UnknownBus {
            bus_id: br.from,
            element_index: row_idx,
        })?;
        let j = case.bus_index(br.to).ok_or(Error::UnknownBus {
            bus_id: br.to,
            element_index: row_idx,
        })?;

        let shift_rad = if flags.zero_shifts {
            0.0
        } else {
            case.angle_radians(br.shift)
        };
        let Some([y_ii, y_ij, y_ji, y_jj]) = branch_admittance(br, flags, shift_rad, row_idx)?
        else {
            // Zero-impedance branch (r² + x² = 0): no admittance to scatter.
            continue;
        };

        if i == j {
            // Self-loop branch: combine all four contributions onto bus i.
            let combined = y_ii + y_jj + y_ij + y_ji;
            g_coo.add(i, i, combined.re);
            b_coo.add(i, i, combined.im);
            continue;
        }

        g_coo.add(i, i, y_ii.re);
        b_coo.add(i, i, y_ii.im);
        g_coo.add(j, j, y_jj.re);
        b_coo.add(j, j, y_jj.im);
        g_coo.add(i, j, y_ij.re);
        b_coo.add(i, j, y_ij.im);
        g_coo.add(j, i, y_ji.re);
        b_coo.add(j, i, y_ji.im);
    }

    if !flags.skip_bus_shunts {
        // ÷ per-unit base (1.0 if the network is already normalized), so a
        // normalized network's shunts aren't divided by base a second time.
        let base = case.per_unit_base();
        for idx in 0..n {
            g_coo.add(idx, idx, case.gs()[idx] / base);
            b_coo.add(idx, idx, case.bs()[idx] / base);
        }
    }

    Ok(YbusParts {
        g: g_coo.finish_csr(),
        b: b_coo.finish_csr(),
    })
}

/// The four entries of a branch's 2×2 nodal admittance block, in per-unit:
/// `[Yff, Yft, Ytf, Ytt]` (= `[y_ii, y_ij, y_ji, y_jj]` in `makeYbus` notation).
/// A pure function of the branch — no bus indexing, no shunt fold — so the Y_bus
/// assembly and the gridfm branch table compute the same numbers from one place.
/// `flags` lets the Y_bus builder zero taps/shifts/charging; pass
/// [`YbusFlags::default`] for the physical admittances (taps and shifts on).
///
/// Returns `Ok(None)` for a zero-impedance branch (`r² + x² = 0`), which the
/// callers skip (Y_bus) or zero out (gridfm). `row` only labels the error.
///
/// # Errors
/// [`Error::NonFiniteSusceptance`] when `r`/`x` are NaN/Inf, so a bad value can't
/// slip a NaN into Y_bus or a Parquet column.
#[allow(clippy::many_single_char_names)]
pub(crate) fn branch_admittance(
    br: &crate::network::Branch,
    flags: YbusFlags,
    shift_rad: f64,
    row: usize,
) -> Result<Option<[Complex64; 4]>> {
    let r = if flags.zero_resistance { 0.0 } else { br.r };
    let x = br.x;
    let denom = r * r + x * x;
    if denom == 0.0 {
        return Ok(None);
    }
    // NaN/Inf r or x makes `denom` non-finite (and slips past `== 0.0`), which
    // would write NaN into Y_bus and silently break the downstream M-matrix/SDDM
    // checks. Reject it the same way `incidence` does.
    if !denom.is_finite() {
        return Err(Error::NonFiniteSusceptance { row });
    }
    let y_series = Complex64::new(r / denom, -x / denom);

    let b_charging = if flags.zero_charging { 0.0 } else { br.b };
    let y_shunt_half = Complex64::new(0.0, b_charging / 2.0);

    let tap_mag = if flags.unity_taps {
        1.0
    } else {
        br.effective_tap()
    };
    // `shift_rad` is supplied already in radians and already zeroed when
    // `flags.zero_shifts` is set (the caller has the network, so it knows whether
    // the source angle is degrees or — for a normalized network — radians).
    let a = Complex64::from_polar(tap_mag, shift_rad);
    let a_norm_sqr = tap_mag * tap_mag;

    let y_ff = (y_series + y_shunt_half) / a_norm_sqr;
    let y_tt = y_series + y_shunt_half;
    let y_ft = -y_series / a.conj();
    let y_tf = -y_series / a;
    Ok(Some([y_ff, y_ft, y_tf, y_tt]))
}

/// Complex from/to power injections for one branch at the given bus voltages, in
/// MVA before the per-unit → MW scaling the caller applies. `vi`/`vj` are complex
/// bus voltages `vm·e^{jθ}` (θ in radians) and `y = [Yff, Yft, Ytf, Ytt]`:
///
/// ```text
/// S_from = vi · conj(Yff·vi + Yft·vj)
/// S_to   = vj · conj(Ytf·vi + Ytt·vj)
/// ```
///
/// At a converged operating point these are the line flows; powerio computes them
/// at the case's stored voltages (the parsed snapshot), not from a fresh solve.
#[cfg(feature = "gridfm")]
pub(crate) fn branch_flows(
    y: &[Complex64; 4],
    vi: Complex64,
    vj: Complex64,
) -> (Complex64, Complex64) {
    let i_from = y[0] * vi + y[1] * vj;
    let i_to = y[2] * vi + y[3] * vj;
    (vi * i_from.conj(), vj * i_to.conj())
}
