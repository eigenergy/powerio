//! A phase shifting winding whose DC angle difference sits near its 150
//! degree shift. PowerModels' pad centered on zero, the default, cuts that
//! operating point off, because it bounds `theta_from - theta_to` rather than
//! the difference net of the shift. The stated bounds, and the pad centered
//! on the shift, both selected explicitly, hold it.

use std::collections::HashMap;

use powerio_matrix::{
    DcOperators, DcOpfAssemblyOptions, DcOpfPreparation, build_dc_opf_preparation,
};
use powerio_prob::{DcOpfInstance, DcPfInstance};
use powerio_tx::{
    AngleDifferenceBounds, BalancedNetwork, Branch, Bus, BusId, BusType, GenCost, Generator,
    Impedance, Load, POWER_MODELS_ANGLE_BOUND_PAD, Transformer3W, Winding,
    correct_angle_difference_bounds,
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

/// The DC power flow angle of every bus, star buses included.
fn dc_angles(net: &BalancedNetwork) -> HashMap<BusId, f64> {
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
    operators.bus_ids().iter().copied().zip(angles).collect()
}

/// `theta_from - theta_to` of every prepared branch at the given angles.
fn branch_differences(prep: &DcOpfPreparation, angles: &HashMap<BusId, f64>) -> Vec<f64> {
    prep.branches
        .from_bus
        .iter()
        .zip(&prep.branches.to_bus)
        .map(|(&from, &to)| angles[&prep.bus_ids[from]] - angles[&prep.bus_ids[to]])
        .collect()
}

fn assert_holds(prep: &DcOpfPreparation, differences: &[f64]) {
    for ((min, max), difference) in prep
        .branches
        .angle_min
        .iter()
        .zip(&prep.branches.angle_max)
        .zip(differences)
    {
        assert!(
            min <= difference && difference <= max,
            "[{min}, {max}] holds {difference}"
        );
    }
}

#[test]
fn a_150_degree_winding_meets_the_stated_bounds_and_the_centered_pad() {
    let net = shifted_three_winding();
    let angles = dc_angles(&net);
    let instance = DcOpfInstance::from_network(net).unwrap();

    // The default is PowerModels' pad centered on zero, which the DC power
    // flow's angle difference near the 150 degree shift does not meet.
    let default = build_dc_opf_preparation(&instance, &DcOpfAssemblyOptions::default()).unwrap();
    assert!(default.correct_angle_difference_bounds);
    assert_eq!(
        default.angle_difference_bounds,
        Some(AngleDifferenceBounds::PowerModelsPad)
    );
    let differences = branch_differences(&default, &angles);
    let widest = differences.iter().fold(0.0f64, |m, d| m.max(d.abs()));
    assert!(
        widest > 140.0_f64.to_radians(),
        "the shifted winding holds about 150 degrees: {widest}"
    );
    let (_, zero_centered_max) = correct_angle_difference_bounds(0.0, 0.0);
    assert!(widest > zero_centered_max);
    assert!(
        default
            .branches
            .angle_max
            .iter()
            .all(|&max| (max - zero_centered_max).abs() < 1e-12)
    );

    // The stated bounds are absent here and hold it.
    let stated = build_dc_opf_preparation(
        &instance,
        &DcOpfAssemblyOptions::default()
            .with_angle_difference_bounds(AngleDifferenceBounds::Stated),
    )
    .unwrap();
    assert!(!stated.correct_angle_difference_bounds);
    assert_holds(&stated, &branch_differences(&stated, &angles));

    // So does the pad centered on each branch's shift.
    let padded = build_dc_opf_preparation(
        &instance,
        &DcOpfAssemblyOptions::default()
            .with_angle_difference_bounds(AngleDifferenceBounds::ShiftCenteredPad),
    )
    .unwrap();
    assert!(padded.correct_angle_difference_bounds);
    assert_eq!(padded.branches.shift, stated.branches.shift);
    for ((min, max), shift) in padded
        .branches
        .angle_min
        .iter()
        .zip(&padded.branches.angle_max)
        .zip(&padded.branches.shift)
    {
        assert!((min - (shift - POWER_MODELS_ANGLE_BOUND_PAD)).abs() < 1e-12);
        assert!((max - (shift + POWER_MODELS_ANGLE_BOUND_PAD)).abs() < 1e-12);
    }
    assert_holds(&padded, &branch_differences(&padded, &angles));

    // The legacy flag turned off selects the stated bounds.
    let unpadded = build_dc_opf_preparation(
        &instance,
        &DcOpfAssemblyOptions::default().with_correct_angle_difference_bounds(false),
    )
    .unwrap();
    assert_eq!(
        unpadded.angle_difference_bounds,
        Some(AngleDifferenceBounds::Stated)
    );
    assert_eq!(unpadded.branches.angle_min, stated.branches.angle_min);

    let unbounded = build_dc_opf_preparation(
        &instance,
        &DcOpfAssemblyOptions::default().with_angle_difference_bounds(AngleDifferenceBounds::None),
    )
    .unwrap();
    assert!(
        unbounded
            .branches
            .angle_min
            .iter()
            .chain(&unbounded.branches.angle_max)
            .all(|bound| bound.is_infinite())
    );
}

fn shifted_ring() -> BalancedNetwork {
    let buses = [10, 20, 30, 40]
        .map(|id| {
            Bus::new(
                BusId(id),
                if id == 10 { BusType::Ref } else { BusType::Pq },
                230.0,
            )
        })
        .to_vec();
    let mut branches = [(10, 20), (20, 30), (30, 40)]
        .map(|(from, to)| {
            let mut branch = Branch::new(BusId(from), BusId(to), 0.0, 1.0);
            branch.shift = 150.0;
            branch
        })
        .to_vec();
    branches.push(Branch::new(BusId(10), BusId(40), 0.0, 1000.0));
    let mut inactive = Branch::new(BusId(10), BusId(40), 0.0, 1.0);
    inactive.in_service = false;
    branches.insert(0, inactive);
    let mut net = BalancedNetwork::in_memory("unwrapped-ring", 100.0, buses, branches);
    let mut generator = Generator::new(BusId(10));
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![1.0, 0.0]));
    net.generators_mut().push(generator);
    net
}

#[test]
fn unbounded_policies_keep_a_dc_solution_beyond_one_turn_in_raw_and_normalized_units() {
    let raw = shifted_ring();
    for net in [raw.clone(), raw.to_normalized().unwrap()] {
        let angles = dc_angles(&net);
        let instance = DcOpfInstance::from_network(net).unwrap();
        let padded = build_dc_opf_preparation(&instance, &DcOpfAssemblyOptions::default()).unwrap();
        let differences = branch_differences(&padded, &angles);
        let widest = differences.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        assert!((widest.to_degrees() - 450.0 * 1000.0 / 1003.0).abs() < 1e-8);
        assert!(widest > std::f64::consts::TAU);
        assert!(
            differences
                .iter()
                .zip(&padded.branches.angle_max)
                .any(|(d, max)| d > max)
        );
        for policy in [AngleDifferenceBounds::None, AngleDifferenceBounds::Stated] {
            let options = DcOpfAssemblyOptions::default().with_angle_difference_bounds(policy);
            let prep = build_dc_opf_preparation(&instance, &options).unwrap();
            assert_holds(&prep, &branch_differences(&prep, &angles));
            assert_eq!(prep.branches.angle_min, vec![f64::NEG_INFINITY; 4]);
            assert_eq!(prep.branches.angle_max, vec![f64::INFINITY; 4]);
            assert_eq!(prep.branches.analysis_rows, padded.branches.analysis_rows);
            assert_eq!(
                prep.branches.analysis_sources,
                padded.branches.analysis_sources
            );
            let json = serde_json::to_value(&prep.branches).unwrap();
            assert_eq!(json["angle_min"][0], "-Infinity");
            assert_eq!(json["angle_max"][0], "Infinity");
            let restored: powerio_matrix::DcBranchParameters =
                serde_json::from_value(json).unwrap();
            assert_eq!(restored, prep.branches);
        }
    }
}

#[test]
fn stated_limits_preserve_one_sided_and_finite_intervals() {
    use powerio_tx::stated_angle_difference_bounds as bounds;
    let full = std::f64::consts::TAU;
    let finite = 30.0_f64.to_radians();
    assert_eq!(bounds(-full, finite), (f64::NEG_INFINITY, finite));
    assert_eq!(bounds(-finite, full), (-finite, f64::INFINITY));
    assert_eq!(
        bounds(-full - 1.0, full + 1.0),
        (f64::NEG_INFINITY, f64::INFINITY)
    );
    assert_eq!(bounds(0.0, 0.0), (f64::NEG_INFINITY, f64::INFINITY));
    assert_eq!(bounds(-finite, finite), (-finite, finite));
    assert_eq!(bounds(-finite, 0.0), (-finite, 0.0));
    assert_eq!(bounds(0.0, finite), (0.0, finite));
    assert_eq!(
        AngleDifferenceBounds::None.calc_bounds(-finite, finite, 0.0),
        (f64::NEG_INFINITY, f64::INFINITY)
    );
}

#[test]
fn bundle_roundtrip_keeps_absent_and_one_sided_angle_limits() {
    let mut net = shifted_ring();
    net.branches_mut()[1].angmax = 30.0;
    let instance = DcOpfInstance::from_network(net).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let options = powerio_matrix::DcOpfBundleOptions {
        assembly: DcOpfAssemblyOptions::default()
            .with_angle_difference_bounds(AngleDifferenceBounds::Stated),
        ..Default::default()
    };
    let output = powerio_matrix::emit_dcopf_bundle(&instance, temp.path(), &options).unwrap();
    let minimum =
        powerio_matrix::io::mtx::read_vector_mtx(output.dir.join("angle_min.mtx")).unwrap();
    let maximum =
        powerio_matrix::io::mtx::read_vector_mtx(output.dir.join("angle_max.mtx")).unwrap();
    assert_eq!(minimum, vec![f64::NEG_INFINITY; 4]);
    assert!((maximum[0] - 30.0_f64.to_radians()).abs() < 1e-12);
    assert_eq!(&maximum[1..], &[f64::INFINITY; 3]);
}

#[test]
fn ac_preparations_preserve_unbounded_angles_and_json_in_both_unit_systems() {
    use powerio_matrix::{
        AcOpfAssemblyOptions, AcPfAssemblyOptions, build_ac_opf_preparation,
        build_ac_pf_preparation,
    };
    use powerio_prob::{AcOpfInstance, AcPfInstance};
    let mut raw = shifted_ring();
    raw.branches_mut()[1].angmax = 30.0;
    for net in [raw.clone(), raw.to_normalized().unwrap()] {
        for policy in [AngleDifferenceBounds::Stated, AngleDifferenceBounds::None] {
            let opf = build_ac_opf_preparation(
                &AcOpfInstance::from_network(net.clone()).unwrap(),
                &AcOpfAssemblyOptions::default().with_angle_difference_bounds(policy),
            )
            .unwrap();
            let pf = build_ac_pf_preparation(
                &AcPfInstance::from_network(net.clone()).unwrap(),
                &AcPfAssemblyOptions::default().with_angle_difference_bounds(policy),
            )
            .unwrap();
            for branches in [&opf.branches, &pf.branches] {
                assert_eq!(branches.angle_min, vec![f64::NEG_INFINITY; 4]);
                if policy == AngleDifferenceBounds::Stated {
                    assert!((branches.angle_max[0] - 30.0_f64.to_radians()).abs() < 1e-12);
                } else {
                    assert!(
                        branches.angle_max[0].is_infinite()
                            && branches.angle_max[0].is_sign_positive()
                    );
                }
                assert_eq!(&branches.angle_max[1..], &[f64::INFINITY; 3]);
                let json = serde_json::to_value(branches).unwrap();
                assert_eq!(json["angle_min"][0], "-Infinity");
                let restored: powerio_matrix::AcBranchData = serde_json::from_value(json).unwrap();
                assert_eq!(&restored, branches);
            }
        }
    }
}
