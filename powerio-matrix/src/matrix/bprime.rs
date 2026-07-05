//! Shuntless susceptance Laplacian used as PowerIO's B' matrix.
//!
//! Per Stott & Alsac (1974), B' is the susceptance Laplacian with all
//! shunts removed and tap ratios / phase shifts ignored:
//!
//! - Off-diagonal `B'_ij = -x / (r² + x²)`  (BX scheme; default)
//!   or `B'_ij = -1 / x`              (XB scheme)
//! - Diagonal     `B'_ii = sum_j |B'_ij|`
//!
//! Result: positive diag, negative off-diag, diag = sum of |off-diag| — the
//! positive (M-matrix) Laplacian convention SDDM solvers expect.
//!
//! This is intentionally not an exact reproduction of MATPOWER `makeB` for
//! phase shifter cases. MATPOWER cancels tap magnitudes while leaving `SHIFT`
//! in the temporary branch table used for `Bp`; PowerIO treats B' as an
//! undirected edge Laplacian and leaves phase shifter injections to the DC
//! builders.

use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::{Error, Result};

use super::{BuildOptions, Scheme, triplet::CooBuilder};

pub fn build_bprime(case: &IndexedNetwork, opts: &BuildOptions) -> Result<CsMat<f64>> {
    let n = case.n();
    let mut coo = CooBuilder::with_capacity(n, 4 * case.branches().len() + n);

    for (row_idx, br) in case.in_service_branches() {
        let i = case.bus_index(br.from).ok_or(Error::UnknownBus {
            bus_id: br.from,
            element_index: row_idx,
        })?;
        let j = case.bus_index(br.to).ok_or(Error::UnknownBus {
            bus_id: br.to,
            element_index: row_idx,
        })?;

        let b_off = match opts.scheme {
            Scheme::Bx => {
                let denom = br.r * br.r + br.x * br.x;
                if denom == 0.0 {
                    if opts.skip_zero_impedance {
                        continue;
                    }
                    return Err(Error::ZeroImpedance { row: row_idx });
                }
                -br.x / denom
            }
            Scheme::Xb => {
                if br.x == 0.0 {
                    if opts.skip_zero_impedance {
                        continue;
                    }
                    return Err(Error::ZeroImpedance { row: row_idx });
                }
                -1.0 / br.x
            }
        };

        // A NaN/Inf reactance (the MATPOWER tokenizer accepts `NaN`/`Inf`) slips
        // past the `== 0.0` checks above and would write a non-finite entry that
        // silently poisons MatrixStats / sddm_check. Reject it loudly instead.
        if !b_off.is_finite() {
            return Err(Error::NonFiniteSusceptance { row: row_idx });
        }

        if i == j {
            // self-loop: contributes only as a shunt, has no place in B'
            continue;
        }

        coo.add_sym(i, j, b_off);
        coo.add(i, i, -b_off);
        coo.add(j, j, -b_off);
    }

    Ok(coo.finish_csr())
}
