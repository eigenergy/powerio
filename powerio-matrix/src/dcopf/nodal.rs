//! Aggregation of generator column data into bus space.
//!
//! Several generators at one bus are usual in a case file. A bus space
//! formulation carries one variable for the bus total, so the generator data
//! must become one entry for each bus.
//!
//! Bound aggregation is exact: the sum of the generator ranges is the range
//! the bus total can reach. Cost aggregation is an approximation, stated in
//! [`combine_costs`].

use powerio_tx::network::GenCost;

use crate::{Error, Result};

/// `(q, c, c0)` of one generator's cost row, as both instance builders read it.
///
/// A MATPOWER model 2 row often carries a leading coefficient near 1e-17 that
/// the source produced by rounding. It states a linear curve, and reading it as
/// quadratic gives `1/q` near 1e17 wherever the curvature is inverted.
///
/// # Errors
/// [`Error::UnsupportedCostModel`] for a row that states no quadratic curve;
/// [`Error::ConcaveCost`] for a negative quadratic coefficient, which the
/// least cost split in [`combine_costs`] cannot price.
pub(crate) fn quadratic_terms(cost: &GenCost, gen_index: usize) -> Result<(f64, f64, f64)> {
    // One rule, stated once on the hub type. Rolling it again here diverged
    // twice: the threshold applied to `2*c2` rather than to the source
    // coefficient the artifact lives in, and a longer row whose leading
    // coefficients are artifacts errored here while the hub read it.
    let (q, c, c0) = cost
        .quadratic_with_constant_tol(GenCost::LEADING_COEFF_TOL)
        .ok_or(Error::UnsupportedCostModel {
            gen_index,
            model: cost.model,
            ncost: cost.ncost,
        })?;
    // The readers keep a concave row; the convexity assumption is this
    // crate's, so the instance builders refuse it here, before bus count
    // decides whether the row passes through or joins a merge.
    if q < 0.0 {
        return Err(Error::ConcaveCost {
            gen_index,
            c2: q / 2.0,
        });
    }
    Ok((q, c, c0))
}

/// Sum of a generator column vector over the bus of each generator.
pub(crate) fn sum_by_bus(n_buses: usize, bus_of_gen: &[usize], values: &[f64]) -> Vec<f64> {
    let mut totals = vec![0.0; n_buses];
    for (generator, &bus) in bus_of_gen.iter().enumerate() {
        totals[bus] += values[generator];
    }
    totals
}

/// Which buses host a generator.
pub(crate) fn buses_with_generators(n_buses: usize, bus_of_gen: &[usize]) -> Vec<bool> {
    let mut flags = vec![false; n_buses];
    for &bus in bus_of_gen {
        flags[bus] = true;
    }
    flags
}

/// One cost curve `0.5 q p² + c p + c0` for each bus, in dense bus order.
pub(crate) struct NodalCosts {
    pub q: Vec<f64>,
    pub c: Vec<f64>,
    pub c0: Vec<f64>,
}

/// Combine the generator cost curves at each bus into one curve of the bus
/// total.
///
/// The split of a total over quadratic curves that costs least equalizes the
/// marginal cost of the generators. That split gives the parallel rule
/// `q = 1 / Σ(1/qᵢ)`, the marginal weighted term `c = q Σ(cᵢ/qᵢ)`, and a
/// constant that holds the cost of the split at zero total. A generator with
/// no quadratic term keeps one marginal cost at every output, so it makes the
/// bus curve linear at the lowest such cost. It does not make the quadratic
/// generators beside it free: each still runs where its marginal meets that
/// flat rate, and the saving it books there is the constant
/// `−½ Σ (c_flat − cᵢ)² / qᵢ`, independent of the bus total.
///
/// The result is an approximation. It agrees with the generator space cost
/// only while the least cost split stays inside the bound of each generator at
/// the bus. A bus with one generator keeps that generator's coefficients as
/// they are.
pub(crate) fn combine_costs(
    n_buses: usize,
    bus_of_gen: &[usize],
    q: &[f64],
    c: &[f64],
    c0: &[f64],
) -> NodalCosts {
    let mut accumulators = vec![BusCost::default(); n_buses];
    for (generator, &bus) in bus_of_gen.iter().enumerate() {
        accumulators[bus].add(q[generator], c[generator], c0[generator]);
    }

    let mut costs = NodalCosts {
        q: Vec::with_capacity(n_buses),
        c: Vec::with_capacity(n_buses),
        c0: Vec::with_capacity(n_buses),
    };
    for accumulator in accumulators {
        let (q, c, c0) = accumulator.finish();
        costs.q.push(q);
        costs.c.push(c);
        costs.c0.push(c0);
    }
    costs
}

#[derive(Clone, Copy)]
struct BusCost {
    count: usize,
    only: (f64, f64, f64),
    /// `Σ(1/qᵢ)` over the generators with a positive quadratic term.
    reciprocal_q: f64,
    /// `Σ(cᵢ/qᵢ)` over the same generators.
    weighted_c: f64,
    /// `Σ(cᵢ²/qᵢ)` over the same generators.
    weighted_c_squared: f64,
    /// Lowest linear term over the generators with no quadratic term.
    flat_c: f64,
    c0: f64,
}

impl Default for BusCost {
    fn default() -> Self {
        Self {
            count: 0,
            only: (0.0, 0.0, 0.0),
            reciprocal_q: 0.0,
            weighted_c: 0.0,
            weighted_c_squared: 0.0,
            flat_c: f64::INFINITY,
            c0: 0.0,
        }
    }
}

impl BusCost {
    fn add(&mut self, q: f64, c: f64, c0: f64) {
        // `quadratic_terms` refuses a negative `q`, so the flat arm below only
        // ever takes `q == 0` rows.
        debug_assert!(q >= 0.0, "concave cost row reached combine_costs: q = {q}");
        self.count += 1;
        if self.count == 1 {
            self.only = (q, c, c0);
        }
        self.c0 += c0;
        // `quadratic_terms` has already zeroed a rounding artifact, which would
        // otherwise give `1/q` near 1e17 and price the whole bus as free.
        if q > 0.0 {
            self.reciprocal_q += 1.0 / q;
            self.weighted_c += c / q;
            self.weighted_c_squared += c * c / q;
        } else {
            self.flat_c = self.flat_c.min(c);
        }
    }

    fn finish(self) -> (f64, f64, f64) {
        match self.count {
            0 => (0.0, 0.0, 0.0),
            1 => self.only,
            // A flat generator holds the bus marginal at its own rate, so each
            // quadratic generator sits at the output where its marginal meets
            // that rate. `Σ (c_flat - cᵢ)² / qᵢ` is what that split saves
            // against paying the flat rate for the whole total.
            _ if self.flat_c.is_finite() => {
                let flat = self.flat_c;
                let saving = flat * flat * self.reciprocal_q - 2.0 * flat * self.weighted_c
                    + self.weighted_c_squared;
                (0.0, flat, self.c0 - 0.5 * saving)
            }
            _ => {
                let q = 1.0 / self.reciprocal_q;
                let c = q * self.weighted_c;
                (
                    q,
                    c,
                    self.c0 + 0.5 * (c * self.weighted_c - self.weighted_c_squared),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_generator_at_a_bus_passes_through() {
        let costs = combine_costs(2, &[1], &[0.3], &[7.0], &[11.0]);
        assert_eq!(costs.q, vec![0.0, 0.3]);
        assert_eq!(costs.c, vec![0.0, 7.0]);
        assert_eq!(costs.c0, vec![0.0, 11.0]);
    }

    /// A rounding artifact states a linear curve. Read as quadratic it gives
    /// `1/q` near 1e17, which swamps the parallel sum and prices the bus as
    /// nearly free.
    #[test]
    fn a_rounding_artifact_reads_as_a_linear_curve() {
        let artifact = GenCost::new(2, 0.0, 0.0, vec![1e-17, 3.0, 0.0]);
        let (q, c, c0) = quadratic_terms(&artifact, 0).expect("a model 2 row");
        assert_eq!((q, c, c0), (0.0, 3.0, 0.0));

        let costs = combine_costs(1, &[0, 0], &[q, 0.2], &[c, 5.0], &[c0, 0.0]);
        assert_eq!(costs.q, vec![0.0]);
        assert_eq!(costs.c, vec![3.0], "the cheaper linear term sets the bus");
    }

    #[test]
    fn a_concave_row_is_refused_with_its_source_coefficient() {
        let concave = GenCost::new(2, 0.0, 0.0, vec![-0.5, 5.0, 0.0]);
        let error = quadratic_terms(&concave, 3).expect_err("a concave row");
        match error {
            Error::ConcaveCost { gen_index, c2 } => {
                assert_eq!(gen_index, 3);
                assert_eq!(c2.to_bits(), (-0.5_f64).to_bits());
            }
            other => panic!("wrong error: {other}"),
        }
    }

    /// A negative coefficient within the artifact tolerance is the same
    /// rounding artifact as a positive one: the row reads flat, no refusal.
    #[test]
    fn a_tiny_negative_artifact_still_reads_as_a_linear_curve() {
        let artifact = GenCost::new(2, 0.0, 0.0, vec![-1e-17, 3.0, 0.0]);
        let (q, c, c0) = quadratic_terms(&artifact, 0).expect("a model 2 row");
        assert_eq!((q, c, c0), (0.0, 3.0, 0.0));
    }

    #[test]
    fn quadratic_curves_combine_by_the_parallel_rule() {
        let costs = combine_costs(1, &[0, 0], &[2.0, 2.0], &[1.0, 3.0], &[0.0, 0.0]);
        assert_eq!(costs.q, vec![1.0]);
        assert_eq!(costs.c, vec![2.0]);
        // The two generators split every total, so the bus curve holds the cost
        // of the split at zero total.
        assert_eq!(costs.c0, vec![-0.5]);
    }

    #[test]
    fn combined_curve_equals_the_least_cost_split() {
        let (q, c, c0) = ([2.0, 5.0], [1.0, 3.0], [4.0, 6.0]);
        let costs = combine_costs(1, &[0, 0], &q, &c, &c0);
        for total in [-2.0_f64, 0.0, 1.5, 40.0] {
            // The split that costs least equalizes q p + c over the pair.
            let lambda = (total + c[0] / q[0] + c[1] / q[1]) / (1.0 / q[0] + 1.0 / q[1]);
            let split = [(lambda - c[0]) / q[0], (lambda - c[1]) / q[1]];
            let generator_cost: f64 = (0..2)
                .map(|i| 0.5 * q[i] * split[i] * split[i] + c[i] * split[i] + c0[i])
                .sum();
            let bus_cost = 0.5 * costs.q[0] * total * total + costs.c[0] * total + costs.c0[0];
            assert!((bus_cost - generator_cost).abs() < 1e-9);
        }
    }

    #[test]
    fn a_generator_without_a_quadratic_term_makes_the_bus_curve_linear() {
        let costs = combine_costs(1, &[0, 0], &[2.0, 0.0], &[1.0, 3.0], &[1.0, 2.0]);
        assert_eq!(costs.q, vec![0.0]);
        assert_eq!(costs.c, vec![3.0]);
        // The quadratic generator runs at (3 - 1) / 2 = 1 whatever the total,
        // and the 1 the bus saves there comes off the constant.
        assert_eq!(costs.c0, vec![2.0]);
    }

    /// The flat branch, held to the same standard as the quadratic one: the
    /// combined curve must equal the least cost split at every total, not just
    /// carry the right slope. Reading `c0` as the plain sum of the generators'
    /// constants overstated the bus by a fixed amount at every dispatch.
    #[test]
    fn a_flat_generator_still_leaves_its_neighbours_running() {
        let (q, c, c0) = ([2.0, 5.0, 0.0], [1.0, 2.0, 3.0], [4.0, 6.0, 7.0]);
        let costs = combine_costs(1, &[0, 0, 0], &q, &c, &c0);
        assert_eq!(costs.q, vec![0.0]);
        assert_eq!(costs.c, vec![3.0], "the flat rate is the bus marginal");

        for total in [0.0_f64, 1.0, 7.5, 100.0] {
            // Every quadratic generator meets the flat marginal; the flat one
            // carries the remainder.
            let split: Vec<f64> = (0..2).map(|i| (c[2] - c[i]) / q[i]).collect();
            let flat = total - split.iter().sum::<f64>();
            let generator_cost: f64 = (0..2)
                .map(|i| 0.5 * q[i] * split[i] * split[i] + c[i] * split[i] + c0[i])
                .sum::<f64>()
                + c[2] * flat
                + c0[2];
            let bus_cost = costs.c[0] * total + costs.c0[0];
            assert!(
                (bus_cost - generator_cost).abs() < 1e-9,
                "total {total}: bus {bus_cost} vs split {generator_cost}"
            );
        }
    }

    #[test]
    fn bounds_sum_and_the_flags_mark_the_hosting_buses() {
        assert_eq!(
            sum_by_bus(3, &[0, 2, 2], &[1.0, 2.0, 4.0]),
            vec![1.0, 0.0, 6.0]
        );
        assert_eq!(
            buses_with_generators(3, &[0, 2, 2]),
            vec![true, false, true]
        );
    }
}
