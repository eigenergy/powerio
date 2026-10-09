//! A source that states no angle difference limit gets none: a phase shifting
//! winding whose DC angle difference sits near its 150 degree shift stays
//! inside the prepared bounds unless the PowerModels pad is asked for.

use powerio_matrix::{DcOperators, DcOpfAssemblyOptions, build_dc_opf_preparation};
use powerio_prob::{DcOpfInstance, DcPfInstance};
use powerio_tx::{
    BalancedNetwork, Branch, Bus, BusId, BusType, GenCost, Generator, Impedance, Load,
    POWER_MODELS_ANGLE_BOUND_PAD, Transformer3W, Winding,
};

/// Dense Gaussian elimination with partial pivoting for the small reduced
/// system below.
fn solve_dense(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for column in 0..n {
        let pivot = (column..n)
            .max_by(|&i, &j| a[i][column].abs().total_cmp(&a[j][column].abs()))
            .unwrap();
        a.swap(column, pivot);
        b.swap(column, pivot);
        let pivot_row = a[column].clone();
        for row in column + 1..n {
            let factor = a[row][column] / pivot_row[column];
            for (entry, pivot) in a[row].iter_mut().zip(&pivot_row).skip(column) {
                *entry -= factor * pivot;
            }
            b[row] -= factor * b[column];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let sum: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - sum) / a[row][row];
    }
    x
}

/// A reference bus 1 with a generator, and a three winding transformer on
/// buses 1, 2, and 3 whose secondary shifts by 150 degrees; loads on 2 and 3.
/// PSS/E states no angle difference limit, which reads as ±360 degrees.
fn shifted_three_winding() -> BalancedNetwork {
    let mut buses: Vec<Bus> = (1..=3)
        .map(|id| Bus::new(BusId(id), BusType::Pq, 230.0))
        .collect();
    buses[0].kind = BusType::Ref;
    let mut net = BalancedNetwork::in_memory("shifted", 100.0, buses, Vec::<Branch>::new());
    let mut windings = [1, 2, 3].map(|bus| Winding::new(BusId(bus)));
    windings[1].shift = 150.0;
    net.transformers_3w_mut().push(Transformer3W::new(
        windings,
        [Impedance::new(0.0, 0.1, 100.0); 3],
    ));
    let mut generator = Generator::new(BusId(1));
    generator.pmax = 200.0;
    generator.pg = 50.0;
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![0.0, 10.0, 0.0]));
    net.generators_mut().push(generator);
    net.loads_mut().extend([
        Load::new(BusId(2), 30.0, 0.0),
        Load::new(BusId(3), 20.0, 0.0),
    ]);
    net
}

#[test]
fn a_150_degree_winding_meets_the_stated_bounds_and_not_the_pad() {
    let net = shifted_three_winding();

    // The DC power flow puts the shifted winding's angle difference near its
    // shift.
    let operators = DcOperators::build(&DcPfInstance::from_network(net.clone()).unwrap()).unwrap();
    let system = operators.calc_reference_constrained_system().unwrap();
    let size = system.retained_rows.len();
    let mut matrix = vec![vec![0.0; size]; size];
    for (row, values) in system.matrix.outer_iterator().enumerate() {
        for (column, &value) in values.iter() {
            matrix[row][column] += value;
        }
    }
    let reduced = solve_dense(matrix, system.rhs.clone());
    let mut angles = vec![0.0; operators.bus_ids().len()];
    for (index, &row) in system.retained_rows.iter().enumerate() {
        angles[row] = reduced[index];
    }
    let incidence = operators.calc_incidence_matrix();
    let mut differences = vec![0.0; incidence.rows()];
    for (row, values) in incidence.outer_iterator().enumerate() {
        for (column, &value) in values.iter() {
            differences[row] += value * angles[column];
        }
    }
    let widest = differences.iter().fold(0.0f64, |m, d| m.max(d.abs()));
    assert!(
        widest > 140.0_f64.to_radians(),
        "the shifted winding holds about 150 degrees: {widest}"
    );

    let instance = DcOpfInstance::from_network(net).unwrap();
    let stated = build_dc_opf_preparation(&instance, &DcOpfAssemblyOptions::default()).unwrap();
    assert!(!stated.correct_angle_difference_bounds);
    for (min, max) in stated
        .branches
        .angle_min
        .iter()
        .zip(&stated.branches.angle_max)
    {
        assert!(
            *min <= -widest && widest <= *max,
            "[{min}, {max}] holds {widest}"
        );
    }

    // The PowerModels pad, asked for explicitly, cannot hold that difference:
    // it bounds theta_from - theta_to, not the difference net of the shift.
    let padded = build_dc_opf_preparation(
        &instance,
        &DcOpfAssemblyOptions::default().with_correct_angle_difference_bounds(true),
    )
    .unwrap();
    assert!(
        padded
            .branches
            .angle_max
            .iter()
            .all(|&max| max <= POWER_MODELS_ANGLE_BOUND_PAD + 1e-12)
    );
    assert!(widest > POWER_MODELS_ANGLE_BOUND_PAD);
}
