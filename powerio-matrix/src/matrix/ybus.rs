//! Bus admittance matrix `Y_bus = G + jB` per MATPOWER's `makeYbus`.
//!
//! For each in-service branch from bus `i` to bus `j` with series impedance
//! `z = r + jx`, terminal shunts `y_fr`/`y_to`, complex tap `a = tap * exp(j shift)`:
//!
//! ```text
//! Y[i,i] += (1/z + y_fr) / |a|^2
//! Y[j,j] += (1/z + y_to)
//! Y[i,j] += -(1/z) / conj(a)
//! Y[j,i] += -(1/z) / a
//! ```
//!
//! Plus bus shunts `Y[i,i] += (g_s + j b_s) / baseMVA`.

use num_complex::Complex64;
use sprs::CsMat;

use crate::Result;
use crate::indexed::IndexedNetwork;

use super::triplet::CooBuilder;

/// `Re(Y_bus)` and `Im(Y_bus)` as separate CSR matrices.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct YbusParts {
    pub g: CsMat<f64>,
    pub b: CsMat<f64>,
}

/// Internal flags used to derive MATPOWER `Bp` and `Bpp` from `Y_bus`.
// Six independent on/off switches into one Y_bus kernel; an enum per pair
// would just spread the same data across more types.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct YbusFlags {
    pub zero_resistance: bool,
    pub zero_charging: bool,
    pub unity_taps: bool,
    pub zero_shifts: bool,
    pub skip_bus_shunts: bool,
    /// Matches `BuildOptions::default()`: false, so a zero impedance branch
    /// (preserved in networks and instances) errors the physical admittances
    /// unless a caller opts into skipping it.
    pub skip_zero_impedance: bool,
}

/// Calculate the bus admittance matrix, returned as separate real and
/// imaginary sparse matrices.
pub fn calc_admittance_matrix(
    case: &IndexedNetwork,
    opts: &super::BuildOptions,
) -> Result<YbusParts> {
    let flags = YbusFlags {
        zero_resistance: false,
        zero_charging: false,
        unity_taps: !opts.include_taps,
        zero_shifts: !opts.include_shifts,
        skip_bus_shunts: false,
        skip_zero_impedance: opts.skip_zero_impedance,
    };
    build_ybus_with_flags(case, flags)
}

// i/j bus indices, r/x impedance, a complex tap: the single-letter names are
// the standard makeYbus notation and the math reads worse spelled out.
#[allow(clippy::many_single_char_names)]
pub(crate) fn build_ybus_with_flags(case: &IndexedNetwork, flags: YbusFlags) -> Result<YbusParts> {
    // The bus shunt block divides by `per_unit_base()`. A zero or non-finite
    // base made that `0.0/0.0`, storing NaN on every diagonal — including
    // shunt-free buses, since `CooBuilder::add` skips only exact zero — and
    // the matrix shipped with no error.
    case.network().check_base_mva()?;
    let n = case.n();
    let mut stamp = AdmittanceStamp::with_capacity(n, case.branches().len());

    for (row_idx, br) in case.in_service_branches() {
        let i = case
            .bus_index(br.from)
            .ok_or(powerio_tx::Error::UnknownBus {
                bus_id: br.from,
                element_index: row_idx,
            })?;
        let j = case.bus_index(br.to).ok_or(powerio_tx::Error::UnknownBus {
            bus_id: br.to,
            element_index: row_idx,
        })?;

        let shift_rad = if flags.zero_shifts {
            0.0
        } else {
            case.to_radians(br.shift)
        };
        if i == j {
            // A self-loop's whole block lands on its one bus, through the
            // pi model's one fold.
            let Some(model) = branch_pi_model(br, flags, shift_rad, row_idx)? else {
                continue;
            };
            let folded = model.calc_folded();
            if !folded.re.is_finite() || !folded.im.is_finite() {
                return Err(powerio_tx::Error::NonFiniteSusceptance { row: row_idx }.into());
            }
            stamp.add_shunt(i, folded);
            continue;
        }
        let Some(block) = branch_admittance(br, flags, shift_rad, row_idx)? else {
            // Zero-impedance branch (r² + x² = 0): no admittance to scatter.
            continue;
        };
        stamp.add_branch(i, j, block);
    }

    if !flags.skip_bus_shunts {
        // ÷ per-unit base (1.0 if the network is already normalized), so a
        // normalized network's shunts aren't divided by base a second time.
        let base = case.per_unit_base();
        for idx in 0..n {
            stamp.add_shunt(
                idx,
                Complex64::new(case.gs()[idx] / base, case.bs()[idx] / base),
            );
        }
    }

    Ok(stamp.finish(false))
}

/// The one bus admittance accumulator: every `Y_bus` in the crate, from a
/// network view ([`calc_admittance_matrix`], `Bp`, `Bpp`) or from a
/// preparation's branch columns, is stamped through it, so the scatter of a
/// pi model block and the self-loop fold exist once.
pub(crate) struct AdmittanceStamp {
    g: CooBuilder,
    b: CooBuilder,
}

impl AdmittanceStamp {
    /// An `n × n` accumulator sized for `branches` pi model blocks.
    pub(crate) fn with_capacity(n: usize, branches: usize) -> Self {
        Self {
            g: CooBuilder::with_capacity(n, 4 * branches + n),
            b: CooBuilder::with_capacity(n, 4 * branches + n),
        }
    }

    /// Scatter one branch block `[Yff, Yft, Ytf, Ytt]` between distinct
    /// buses `i` and `j`. A self-loop is folded by
    /// [`PiModel::calc_folded`](powerio_tx::PiModel::calc_folded) and stamped
    /// with [`add_shunt`](Self::add_shunt) instead.
    pub(crate) fn add_branch(&mut self, i: usize, j: usize, block: [Complex64; 4]) {
        debug_assert_ne!(i, j, "a self-loop is stamped through its fold");
        let [y_ii, y_ij, y_ji, y_jj] = block;
        self.add(i, i, y_ii);
        self.add(j, j, y_jj);
        self.add(i, j, y_ij);
        self.add(j, i, y_ji);
    }

    /// Add a bus shunt admittance to the diagonal.
    pub(crate) fn add_shunt(&mut self, bus: usize, y: Complex64) {
        self.add(bus, bus, y);
    }

    fn add(&mut self, i: usize, j: usize, y: Complex64) {
        self.g.add(i, j, y.re);
        self.b.add(i, j, y.im);
    }

    /// The CSR parts. With `every_diagonal`, both keep an entry at every
    /// diagonal position, an explicit zero where nothing stamps it.
    pub(crate) fn finish(self, every_diagonal: bool) -> YbusParts {
        if every_diagonal {
            YbusParts {
                g: self.g.finish_csr_with_diagonal(),
                b: self.b.finish_csr_with_diagonal(),
            }
        } else {
            YbusParts {
                g: self.g.finish_csr(),
                b: self.b.finish_csr(),
            }
        }
    }
}

/// The four entries of a branch's 2×2 nodal admittance block, in per-unit:
/// `[Yff, Yft, Ytf, Ytt]` (= `[y_ii, y_ij, y_ji, y_jj]` in `makeYbus` notation).
/// A pure function of the branch — no bus indexing, no shunt fold — so the Y_bus
/// assembly and the gridfm branch table compute the same numbers from one place.
/// `flags` lets the Y_bus builder zero taps/shifts/charging; pass
/// [`YbusFlags::default`] for the physical admittances (taps and shifts on).
///
/// Returns `Ok(None)` for a zero-impedance branch — one whose impedance
/// magnitude is under [`MIN_DIVISIBLE_MAGNITUDE`](powerio_tx::dc::MIN_DIVISIBLE_MAGNITUDE) —
/// which the callers skip (Y_bus) or zero out (gridfm). `row` only labels the
/// error.
///
/// # Errors
/// [`powerio_tx::Error::NonFiniteSusceptance`] when `r`/`x` are NaN/Inf, so a bad value can't
/// slip a NaN into Y_bus or a Parquet column. [`powerio_tx::Error::DegenerateTap`] when the
/// tap ratio is one the four admittances cannot be divided by.
pub(crate) fn branch_admittance(
    br: &crate::network::Branch,
    flags: YbusFlags,
    shift_rad: f64,
    row: usize,
) -> Result<Option<[Complex64; 4]>> {
    let Some(model) = branch_pi_model(br, flags, shift_rad, row)? else {
        return Ok(None);
    };
    let out = model.calc_block();
    // Each input is bounded on its own; the products can still overflow
    // when several sit near their bound at once.
    if out.iter().any(|y| !y.re.is_finite() || !y.im.is_finite()) {
        return Err(powerio_tx::Error::NonFiniteSusceptance { row }.into());
    }
    Ok(Some(out))
}

/// The pi model primitives of a branch under `flags`, bounded: the series
/// admittance (`Ok(None)` for a zero impedance branch the flags skip), the
/// terminal charging, the tap magnitude and `shift_rad`. [`branch_admittance`]
/// forms its block; a self-loop folds it.
///
/// # Errors
/// As [`branch_admittance`].
pub(crate) fn branch_pi_model(
    br: &crate::network::Branch,
    flags: YbusFlags,
    shift_rad: f64,
    row: usize,
) -> Result<Option<powerio_tx::PiModel>> {
    let r = if flags.zero_resistance { 0.0 } else { br.r };
    let x = br.x;
    // Zero impedance in every sense the builder can act on; exact zero used to
    // be the whole test. The guard and the division are shared with the AC
    // path, so the two cannot drift; only the skip-vs-error policy is local.
    let Some((g, b)) = powerio_tx::calc_series_admittance_of(r, x, row)? else {
        if flags.skip_zero_impedance {
            return Ok(None);
        }
        return Err(powerio_tx::Error::ZeroImpedance { row }.into());
    };

    let charging = if flags.zero_charging {
        crate::network::BranchCharging::new(0.0, 0.0, 0.0, 0.0)
    } else {
        br.calc_terminal_charging()
    };

    // A tap of 1e-200 underflows `tap²` to zero and scatters +/-Inf through
    // the four admittances.
    let tap = if flags.unity_taps {
        1.0
    } else {
        br.calc_divisible_tap(row)?
    };
    // `shift_rad` is supplied already in radians and already zeroed when
    // `flags.zero_shifts` is set (the caller has the network, so it knows
    // whether the source angle is degrees or, normalized, radians).
    Ok(Some(powerio_tx::PiModel::new(
        Complex64::new(g, b),
        Complex64::new(charging.g_fr, charging.b_fr),
        Complex64::new(charging.g_to, charging.b_to),
        tap,
        shift_rad,
    )))
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
pub(crate) fn branch_flows(
    y: &[Complex64; 4],
    vi: Complex64,
    vj: Complex64,
) -> (Complex64, Complex64) {
    let (i_from, i_to) = branch_currents(y, vi, vj);
    (vi * i_from.conj(), vj * i_to.conj())
}

/// The terminal currents `I_from = Yff vi + Yft vj` and `I_to = Ytf vi + Ytt vj`
/// of one branch block: the one current [`branch_flows`] and
/// [`branch_flow_partials`] read.
fn branch_currents(y: &[Complex64; 4], vi: Complex64, vj: Complex64) -> (Complex64, Complex64) {
    (y[0] * vi + y[1] * vj, y[2] * vi + y[3] * vj)
}

/// The partials of [`branch_flows`]' `(S_from, S_to)` with respect to the
/// polar terminal voltages, each ordered `[d/dva_from, d/dvm_from, d/dva_to,
/// d/dvm_to]`, at `vi = vm_from e^{j va_from}` and `vj = vm_to e^{j va_to}`.
/// With `dV/dva = j V` and `dV/dvm = V / vm`:
///
/// ```text
/// dS_f/dva_f = j vi conj(Yft vj)         dS_f/dva_t = -j vi conj(Yft vj)
/// dS_f/dvm_f = v̂i conj(I_f) + vi conj(Yff v̂i)   dS_f/dvm_t = vi conj(Yft v̂j)
/// dS_t/dva_f = -j vj conj(Ytf vi)        dS_t/dva_t = j vj conj(Ytf vi)
/// dS_t/dvm_f = vj conj(Ytf v̂i)          dS_t/dvm_t = v̂j conj(I_t) + vj conj(Ytt v̂j)
/// ```
///
/// with `v̂ = e^{j va}`.
pub(crate) fn branch_flow_partials(
    y: &[Complex64; 4],
    (vm_from, va_from): (f64, f64),
    (vm_to, va_to): (f64, f64),
) -> ([Complex64; 4], [Complex64; 4]) {
    let [yff, yft, ytf, ytt] = *y;
    let vhat_i = Complex64::from_polar(1.0, va_from);
    let vhat_j = Complex64::from_polar(1.0, va_to);
    let vi = vhat_i * vm_from;
    let vj = vhat_j * vm_to;
    let (i_from, i_to) = branch_currents(y, vi, vj);
    let j = Complex64::i();
    let from = [
        j * vi * (yft * vj).conj(),
        vhat_i * i_from.conj() + vi * (yff * vhat_i).conj(),
        -j * vi * (yft * vj).conj(),
        vi * (yft * vhat_j).conj(),
    ];
    let to = [
        -j * vj * (ytf * vi).conj(),
        vj * (ytf * vhat_i).conj(),
        j * vj * (ytf * vi).conj(),
        vhat_j * i_to.conj() + vj * (ytt * vhat_j).conj(),
    ];
    (from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The analytic flow partials match central differences of the flows.
    #[test]
    fn flow_partials_match_central_differences() {
        let y = powerio_tx::PiModel::new(
            Complex64::new(2.0, -9.0),
            Complex64::new(0.0, 0.04),
            Complex64::new(0.001, 0.05),
            1.03,
            0.08,
        )
        .calc_block();
        let point = [1.02, 0.11, 0.97, -0.07];
        let flows = |p: [f64; 4]| {
            branch_flows(
                &y,
                Complex64::from_polar(p[0], p[1]),
                Complex64::from_polar(p[2], p[3]),
            )
        };
        let (from, to) = branch_flow_partials(&y, (point[0], point[1]), (point[2], point[3]));
        // Partial order [va_f, vm_f, va_t, vm_t] against point [vm_f, va_f, vm_t, va_t].
        for (k, coordinate) in [1usize, 0, 3, 2].into_iter().enumerate() {
            let h = 1e-6;
            let (mut hi, mut lo) = (point, point);
            hi[coordinate] += h;
            lo[coordinate] -= h;
            let ((fh, th), (fl, tl)) = (flows(hi), flows(lo));
            let (df, dt) = ((fh - fl) / (2.0 * h), (th - tl) / (2.0 * h));
            assert!(
                (df - from[k]).norm() < 1e-6,
                "from {k}: {df} vs {}",
                from[k]
            );
            assert!((dt - to[k]).norm() < 1e-6, "to {k}: {dt} vs {}", to[k]);
        }
    }
}
