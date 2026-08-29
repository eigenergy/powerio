//! The DC operator surface: PowerModels signs, the branch flow identity
//! `p_branch = -Bf va + b .* shift`, stable axis mappings that survive source
//! row reordering, formula-selective guards (#324), and injection updates
//! that reconstruct no network dependent matrix.

use powerio_core::Source;
use powerio_matrix::DcOperators;
use powerio_prob::{DcPfInstance, merge_zero_impedance_buses};
use powerio_tx::{BalancedNetwork, Branch, Bus, BusId, BusType, DcConvention};

fn case9() -> BalancedNetwork {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    powerio_tx::parse(Source::open(path).unwrap())
        .expect("case9 parses")
        .into_value()
}

fn case118() -> BalancedNetwork {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case118.m");
    powerio_tx::parse(Source::open(path).unwrap())
        .expect("case118 parses")
        .into_value()
}

fn dense(matrix: &powerio_matrix::SparseMatrix) -> Vec<Vec<f64>> {
    let mut out = vec![vec![0.0; matrix.cols()]; matrix.rows()];
    for (row, row_vec) in matrix.outer_iterator().enumerate() {
        for (column, &value) in row_vec.iter() {
            out[row][column] += value;
        }
    }
    out
}

/// Gaussian elimination with partial pivoting, for the small dense reduced
/// systems these tests solve directly instead of through any solver crate.
fn solve_dense(matrix: &[Vec<f64>], rhs: &[f64]) -> Vec<f64> {
    let n = rhs.len();
    let mut a = matrix.to_vec();
    let mut b = rhs.to_vec();
    for column in 0..n {
        let pivot = (column..n)
            .max_by(|&i, &j| a[i][column].abs().total_cmp(&a[j][column].abs()))
            .expect("column has at least one row");
        a.swap(column, pivot);
        b.swap(column, pivot);
        let diag = a[column][column];
        assert!(diag.abs() > 1e-12, "singular at column {column}");
        for row in (column + 1)..n {
            let factor = a[row][column] / diag;
            if factor == 0.0 {
                continue;
            }
            for col in column..n {
                a[row][col] -= factor * a[column][col];
            }
            b[row] -= factor * b[column];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let sum: f64 = ((row + 1)..n).map(|col| a[row][col] * x[col]).sum();
        x[row] = (b[row] - sum) / a[row][row];
    }
    x
}

/// `(row, angle radians)` for every reference bus, in bus table order, read
/// straight from the network rather than any internal operator state.
fn reference_rows_radians(net: &BalancedNetwork) -> Vec<(usize, f64)> {
    net.buses()
        .iter()
        .enumerate()
        .filter(|(_, bus)| bus.kind == BusType::Ref)
        .map(|(row, bus)| (row, bus.va.to_radians()))
        .collect()
}

/// The grounded rhs at a retained bus row, oracle spelling: `p - p_shift -
/// L(row, :) * va` restricted to the reference columns, with `L = -B` taken
/// from the already validated public susceptance matrix rather than any
/// state the reference coupling fix touches. Matches MATPOWER's dcpf.m
/// `Pbus(pvpq) - B(pvpq,ref)*Va0(ref)`, generalized to any number of
/// reference buses in one island.
fn independent_grounded_rhs(
    public_b: &[Vec<f64>],
    p: &[f64],
    shift: &[f64],
    references: &[(usize, f64)],
    row: usize,
) -> f64 {
    let coupling: f64 = references
        .iter()
        .map(|&(reference_row, va_ref)| -public_b[row][reference_row] * va_ref)
        .sum();
    p[row] - shift[row] - coupling
}

#[test]
fn public_susceptances_carry_powermodels_signs() {
    let net = case9();
    let instance = DcPfInstance::from_network(net.clone()).unwrap();
    let operators = DcOperators::build(&instance).unwrap();

    assert_eq!(operators.bus_ids().len(), net.buses().len());
    assert_eq!(operators.branch_identities().len(), net.branches().len());
    for (column, branch) in net.branches().iter().enumerate() {
        let b = operators.branch_susceptances()[column];
        // PowerModels: b = imag(inv(r + jx)) = -x/(r² + x²), negative for an
        // inductive branch.
        let expected = -branch.x / (branch.r * branch.r + branch.x * branch.x);
        assert!(
            (b - expected).abs() < 1e-12,
            "column {column}: {b} vs {expected}"
        );
        assert!(b < 0.0, "inductive branch column {column} is negative");
    }
}

#[test]
fn the_branch_flow_identity_holds() {
    let net = case9();
    let instance = DcPfInstance::from_network(net.clone()).unwrap();
    let operators = DcOperators::build(&instance).unwrap();
    let n = operators.bus_ids().len();
    let m = operators.branch_identities().len();

    // Any angle vector: the identity is linear algebra over the emitted
    // operators, so a synthetic assignment proves the spelling.
    let va: Vec<f64> = (0..n).map(|row| 0.01 * row as f64).collect();
    let bf = dense(&operators.branch_susceptance_matrix());
    let b = operators.branch_susceptances();
    let a = dense(operators.incidence());

    for column in 0..m {
        // p_branch = -Bf va + b .* shift; case9 states no phase shifts, so
        // the shift term is zero and the flow is -(Bf va).
        let bf_va: f64 = (0..n)
            .map(|row| bf[column][row] * va[row])
            .collect::<Vec<_>>()
            .iter()
            .sum();
        let p_branch = -bf_va;
        // Independent spelling: the flow from angle difference over the
        // series reactance, f = (va_from - va_to) * (-b) for the PowerModels
        // sign of b.
        let from = (0..n).find(|&row| a[row][column] > 0.0).unwrap();
        let to = (0..n).find(|&row| a[row][column] < 0.0).unwrap();
        let direct = (va[from] - va[to]) * -b[column];
        assert!(
            (p_branch - direct).abs() < 1e-12,
            "column {column}: {p_branch} vs {direct}"
        );
    }

    // The nodal balance ties the two matrix operators together:
    // -B va + p_shift equals A * (-Bf va + b .* shift).
    let bus = dense(&operators.bus_susceptance_matrix());
    let shift = operators.phase_shift_injection();
    for row in 0..n {
        let b_va: f64 = (0..n).map(|col| bus[row][col] * va[col]).sum();
        let mut through_branches = 0.0;
        for column in 0..m {
            let bf_va: f64 = (0..n).map(|k| bf[column][k] * va[k]).sum();
            through_branches += a[row][column] * -bf_va;
        }
        assert!(
            ((-b_va + shift[row]) - (through_branches + shift[row])).abs() < 1e-9,
            "row {row}"
        );
    }
}

#[test]
fn the_reference_constrained_system_solves_the_stated_problem() {
    let net = case9();
    let reference = reference_rows_radians(&net);
    assert_eq!(reference.len(), 1, "case9 has one reference");
    let (ref_row, va_ref) = reference[0];
    assert_eq!(va_ref, 0.0, "case9 states its reference angle at zero");

    let instance = DcPfInstance::from_network(net).unwrap();
    let operators = DcOperators::build(&instance).unwrap();
    let system = operators.reference_constrained_system().unwrap();

    let n = operators.bus_ids().len();
    assert_eq!(system.retained_rows.len(), n - 1, "case9 has one reference");
    assert_eq!(system.rhs.len(), system.retained_rows.len());
    // The grounded matrix is the positive factor weight spelling: strictly
    // positive diagonal, symmetric, diagonally dominant.
    let grounded = dense(&system.matrix);
    for (row, values) in grounded.iter().enumerate() {
        assert!(values[row] > 0.0, "diagonal row {row}");
        let off: f64 = values
            .iter()
            .enumerate()
            .filter(|(column, _)| *column != row)
            .map(|(_, value)| value.abs())
            .sum();
        assert!(values[row] + 1e-9 >= off, "dominance row {row}");
        for (column, &value) in values.iter().enumerate() {
            assert!((value - grounded[column][row]).abs() < 1e-12, "symmetry");
        }
    }
    // Its entries are the negation of the public bus susceptance matrix at
    // the retained rows: the sign conversion happens at this fill only.
    let public = dense(&operators.bus_susceptance_matrix());
    for (reduced_row, &row) in system.retained_rows.iter().enumerate() {
        for (reduced_col, &col) in system.retained_rows.iter().enumerate() {
            assert!(
                (grounded[reduced_row][reduced_col] + public[row][col]).abs() < 1e-12,
                "grounded is -B at ({row},{col})"
            );
        }
    }

    // Solving the system must reproduce the stated injections: reinstate the
    // fixed reference angle and check the full nodal balance at every
    // retained bus, not just that the matrix and rhs have the right shape.
    let solved = solve_dense(&grounded, &system.rhs);
    let mut va_full = vec![0.0; n];
    for (reduced, &row) in system.retained_rows.iter().enumerate() {
        va_full[row] = solved[reduced];
    }
    va_full[ref_row] = va_ref;
    let p = operators.bus_power_injection();
    let shift = operators.phase_shift_injection();
    for &row in &system.retained_rows {
        let b_va: f64 = (0..n).map(|col| public[row][col] * va_full[col]).sum();
        assert!(
            (-b_va + shift[row] - p[row]).abs() < 1e-9,
            "row {row} reproduces the stated injection"
        );
    }
}

#[test]
fn axis_mappings_survive_source_row_reordering() {
    let net = case9();
    // Give every branch a uid so identity is order free.
    let mut named = net.clone();
    for (row, branch) in named.branches_mut().iter_mut().enumerate() {
        branch.uid = Some(format!("branch-{row}"));
    }
    let mut named_reordered = named.clone();
    named_reordered.branches_mut().reverse();

    let a = DcOperators::build(&DcPfInstance::from_network(named).unwrap()).unwrap();
    let b = DcOperators::build(&DcPfInstance::from_network(named_reordered).unwrap()).unwrap();
    // The same identity resolves to the same susceptance whatever the source
    // row order.
    for (column, identity) in a.branch_identities().iter().enumerate() {
        let other = b
            .branch_identities()
            .iter()
            .position(|candidate| candidate == identity)
            .expect("identity survives reordering");
        assert!(
            (a.branch_susceptances()[column] - b.branch_susceptances()[other]).abs() < 1e-12,
            "{identity}"
        );
    }
}

#[test]
fn guards_read_only_the_selected_formula() {
    // #324: a tap of 1e-200 is unread by SeriesSusceptance and ReactanceOnly,
    // so it cannot reject the branch; TapAdjustedReactance reports it.
    let mut net = case9();
    net.branches_mut()[0].tap = 1e-200;

    for approximation in [DcConvention::SeriesSusceptance, DcConvention::ReactanceOnly] {
        let instance = DcPfInstance::from_network(net.clone())
            .unwrap()
            .with_approximation(approximation);
        DcOperators::build(&instance).unwrap_or_else(|error| {
            panic!("{approximation:?} read an unread tap: {error}");
        });
    }
    let instance = DcPfInstance::from_network(net.clone())
        .unwrap()
        .with_approximation(DcConvention::TapAdjustedReactance);
    let error = DcOperators::build(&instance).unwrap_err();
    assert!(error.to_string().contains("tap"), "{error}");
}

#[test]
fn zero_impedance_refuses_and_the_merge_resolves() {
    let mut net = case9();
    let mut tie = net.branches()[0].clone();
    tie.from = BusId(5);
    tie.to = BusId(6);
    tie.r = 0.0;
    tie.x = 0.0;
    net.branches_mut().push(tie);

    let instance = DcPfInstance::from_network(net.clone()).unwrap();
    let error = DcOperators::build(&instance).unwrap_err();
    assert!(
        error.to_string().contains("merge_zero_impedance_buses"),
        "{error}"
    );

    let (merged, _, _) = merge_zero_impedance_buses(&net).unwrap();
    let merged_instance = DcPfInstance::from_network(merged).unwrap();
    DcOperators::build(&merged_instance).expect("the merged network projects");
}

#[test]
fn injection_updates_reconstruct_no_matrix() {
    let net = case9();
    let instance = DcPfInstance::from_network(net.clone()).unwrap();
    let mut operators = DcOperators::build(&instance).unwrap();
    let incidence_before: *const f64 = operators.incidence().data().as_ptr();
    let before = operators.bus_power_injection().to_vec();

    // A changed operating state: scale every load; the instance rebuilds its
    // specifications, the operators refresh injections only.
    let mut changed = net.clone();
    for load in changed.loads_mut() {
        load.p *= 1.1;
    }
    let updated = DcPfInstance::from_network(changed).unwrap();
    operators.update(&updated).unwrap();
    let after = operators.bus_power_injection().to_vec();
    assert_ne!(before, after, "the injections moved");
    assert_eq!(
        operators.incidence().data().as_ptr(),
        incidence_before,
        "the incidence matrix was not reconstructed"
    );
}

#[test]
fn the_grounded_rhs_subtracts_the_shift_injection() {
    // A shifted branch makes the shift term visible: with p = -B va + p_shift,
    // the grounded solve needs rhs = p - p_shift at the retained buses.
    let mut net = case9();
    net.branches_mut()[0].shift = 10.0;
    let instance = DcPfInstance::from_network(net).unwrap();
    let operators = DcOperators::build(&instance).unwrap();
    let system = operators.reference_constrained_system().unwrap();

    let p = operators.bus_power_injection().to_vec();
    let shift = operators.phase_shift_injection();
    assert!(
        shift.iter().any(|value| value.abs() > 1e-9),
        "the shifted branch must inject"
    );
    for (reduced, &row) in system.retained_rows.iter().enumerate() {
        assert!(
            (system.rhs[reduced] - (p[row] - shift[row])).abs() < 1e-12,
            "row {row}"
        );
    }
}

#[test]
fn reference_angles_couple_into_the_grounded_rhs() {
    // case118 states its reference bus (69) at Va = 30 degrees. MATPOWER's
    // dcpf.m solves Va(pvpq) = B(pvpq,pvpq) \ (Pbus(pvpq) -
    // B(pvpq,ref)*Va0(ref)); the grounded rhs here must carry that same
    // Va0(ref) term rather than solving as though the reference sat at zero.
    let net = case118();
    let reference = reference_rows_radians(&net);
    assert_eq!(reference.len(), 1, "case118 states one reference bus");
    let (ref_row, va_ref) = reference[0];
    assert!(
        (va_ref - 30_f64.to_radians()).abs() < 1e-12,
        "bus 69 states 30 degrees"
    );

    let instance = DcPfInstance::from_network(net).unwrap();
    let operators = DcOperators::build(&instance).unwrap();
    let system = operators.reference_constrained_system().unwrap();

    // Independent rhs, built from the already validated public susceptance
    // matrix (untouched by this fix) rather than any internal state it
    // changes: p - p_shift - L(:, ref) * va_ref, with L = -B.
    let public_b = dense(&operators.bus_susceptance_matrix());
    let p = operators.bus_power_injection().to_vec();
    let shift = operators.phase_shift_injection();
    for (reduced, &row) in system.retained_rows.iter().enumerate() {
        let expected = independent_grounded_rhs(&public_b, &p, &shift, &reference, row);
        assert!(
            (system.rhs[reduced] - expected).abs() < 1e-9,
            "row {row}: {} vs {expected}",
            system.rhs[reduced]
        );
    }

    // Solve the reduced system, reinstate the fixed reference angle, and
    // check the full nodal balance holds at every retained bus. Under the
    // old rhs (no coupling term) this residual is off by the reference
    // angle instead of closing.
    let solved = solve_dense(&dense(&system.matrix), &system.rhs);
    let mut va_full = vec![0.0; operators.bus_ids().len()];
    for (reduced, &row) in system.retained_rows.iter().enumerate() {
        va_full[row] = solved[reduced];
    }
    va_full[ref_row] = va_ref;
    for &row in &system.retained_rows {
        let b_va: f64 = (0..va_full.len())
            .map(|col| public_b[row][col] * va_full[col])
            .sum();
        assert!(
            (-b_va + shift[row] - p[row]).abs() < 1e-7,
            "row {row} balances with the reference angle honored"
        );
    }
}

#[test]
fn two_reference_buses_in_one_island_carry_a_nonzero_flow() {
    // Two reference buses in one island, joined through a passthrough bus
    // whose angle is unknown: both reference angles hold fixed and disagree
    // by 20 degrees, so a nonzero flow must run through the passthrough bus
    // even though it states no injection of its own. The old rhs pinned
    // every reference angle to zero, so the two references agreed and no
    // flow resulted. Nothing about this topology is exotic: MATPOWER's dcpf
    // holds both angles the same way.
    let mut bus1 = Bus::new(BusId(1), BusType::Ref, 230.0);
    bus1.va = 0.0;
    let bus2 = Bus::new(BusId(2), BusType::Pq, 230.0);
    let mut bus3 = Bus::new(BusId(3), BusType::Ref, 230.0);
    bus3.va = 20.0;

    let x = 0.1;
    let branch_12 = Branch::new(BusId(1), BusId(2), 0.0, x);
    let branch_23 = Branch::new(BusId(2), BusId(3), 0.0, x);
    let net = BalancedNetwork::in_memory(
        "two-reference-island",
        100.0,
        vec![bus1, bus2, bus3],
        vec![branch_12, branch_23],
    );

    let instance = DcPfInstance::from_network(net.clone()).unwrap();
    let operators = DcOperators::build(&instance).unwrap();
    let system = operators.reference_constrained_system().unwrap();
    assert_eq!(system.retained_rows.len(), 1, "only bus 2 is retained");

    // Independent rhs, the same oracle spelling as the case118 test: p -
    // p_shift - L(:, ref) * va_ref, with L = -B from the already validated
    // public susceptance matrix, summed over both eliminated reference rows.
    let reference = reference_rows_radians(&net);
    assert_eq!(reference.len(), 2, "both bus 1 and bus 3 are reference");
    let public_b = dense(&operators.bus_susceptance_matrix());
    let p = operators.bus_power_injection().to_vec();
    let shift = operators.phase_shift_injection();
    let retained_row = system.retained_rows[0];
    let expected_rhs = independent_grounded_rhs(&public_b, &p, &shift, &reference, retained_row);
    assert!(
        (system.rhs[0] - expected_rhs).abs() < 1e-9,
        "{} vs {expected_rhs}",
        system.rhs[0]
    );

    let solved = solve_dense(&dense(&system.matrix), &system.rhs);
    let va2 = solved[0];

    // Cross check by hand, independent of the Laplacian oracle above: equal
    // reactances split the 20 degree gap evenly, so bus 2 sits at the
    // midpoint of the two stated reference angles, and each branch carries
    // the same nonzero flow.
    let weight = 1.0 / x;
    let va1 = 0.0_f64;
    let va3 = 20_f64.to_radians();
    let expected_va2 = (weight * va1 + weight * va3) / (weight + weight);
    assert!((va2 - expected_va2).abs() < 1e-9, "{va2} vs {expected_va2}");

    let b = operators.branch_susceptances();
    let flow_12 = -b[0] * (va1 - va2);
    let flow_23 = -b[1] * (va2 - va3);
    let expected_flow = -weight * (va3 - va1) / 2.0;
    assert!(
        (flow_12 - expected_flow).abs() < 1e-9,
        "{flow_12} vs {expected_flow}"
    );
    assert!(
        (flow_23 - expected_flow).abs() < 1e-9,
        "{flow_23} vs {expected_flow}"
    );
    assert!(flow_12.abs() > 1e-6, "the flow must not be zero");
}

#[test]
fn a_subnormal_reactance_is_refused_like_zero() {
    // x = 1e-160 divides to a finite 1e160 weight that would annihilate every
    // real branch at its buses; the divisibility floor refuses it the same
    // way exact zero is refused.
    let mut net = case9();
    net.branches_mut()[0].r = 0.0;
    net.branches_mut()[0].x = 1e-160;
    let instance = DcPfInstance::from_network(net).unwrap();
    let error = DcOperators::build(&instance).unwrap_err();
    assert!(
        error.to_string().contains("merge_zero_impedance_buses"),
        "{error}"
    );
}
