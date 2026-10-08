//! A bus merge under the PSS/E zero impedance rule is the small reactance
//! limit of the unmerged DC power flow: the merged angles match the unmerged
//! angles of the kept buses, and the flows recovered on the removed elements
//! match the unmerged flows through them.

use powerio_matrix::DcOperators;
use powerio_prob::DcPfInstance;
use powerio_tx::{
    BalancedNetwork, Branch, BranchSusceptanceFormula, Bus, BusId, BusMergeRule, BusType,
    Generator, Load, MergedFlows, RemovedFlowMethod, Switch, ZeroImpedanceRule,
};

/// The jumper reactance of the unmerged network, per unit: far below the
/// threshold, so the merged network is its limit.
const JUMPER_X: f64 = 1e-7;

/// Dense Gaussian elimination with partial pivoting for the small reduced
/// systems below.
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

/// Solve the DC power flow: bus angles (radians) by bus id, and branch flows
/// in MW by network branch row.
fn solve_dc(network: &BalancedNetwork) -> (Vec<(BusId, f64)>, Vec<f64>) {
    let instance = DcPfInstance::from_network(network.clone()).unwrap();
    let operators = DcOperators::build(&instance).unwrap();
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
    let per_unit = operators.calc_branch_flow_dc(&angles).unwrap();
    let mut flows = vec![0.0; network.branches().len()];
    for (column, &row) in operators.branch_rows().iter().enumerate() {
        flows[row] = per_unit[column] * network.base_mva();
    }
    (
        operators.bus_ids().iter().copied().zip(angles).collect(),
        flows,
    )
}

/// Bus 1 is the reference. Buses 2, 3, and 4 form a triangle of jumpers,
/// one of them in parallel with a second jumper; bus 6 hangs off bus 5
/// through a jumper, which the merged network states as a closed switch.
fn network(closed_switch: bool) -> BalancedNetwork {
    let mut buses: Vec<Bus> = (1..=6)
        .map(|id| Bus::new(BusId(id), BusType::Pq, 230.0))
        .collect();
    buses[0].kind = BusType::Ref;
    let jumper = |from, to, x| Branch::new(BusId(from), BusId(to), 0.0, x);
    let mut branches = vec![
        Branch::new(BusId(1), BusId(2), 0.0, 0.1),
        jumper(2, 3, JUMPER_X),
        jumper(3, 4, JUMPER_X),
        jumper(2, 4, 2.0 * JUMPER_X),
        jumper(4, 2, 3.0 * JUMPER_X),
        Branch::new(BusId(4), BusId(5), 0.0, 0.2),
        Branch::new(BusId(1), BusId(5), 0.0, 0.3),
    ];
    if !closed_switch {
        branches.push(jumper(5, 6, JUMPER_X));
    }
    let mut net = BalancedNetwork::in_memory("merge-dc", 100.0, buses, branches);
    if closed_switch {
        net.switches_mut()
            .push(Switch::new(BusId(5), BusId(6), true));
    }
    let mut slack = Generator::new(BusId(1));
    slack.pg = 140.0;
    let mut inside = Generator::new(BusId(3));
    inside.pg = 30.0;
    net.generators_mut().extend([slack, inside]);
    net.loads_mut().extend([
        Load::new(BusId(3), 40.0, 0.0),
        Load::new(BusId(4), 60.0, 0.0),
        Load::new(BusId(5), 50.0, 0.0),
        Load::new(BusId(6), 20.0, 0.0),
    ]);
    net
}

#[test]
fn merged_dc_angles_and_recovered_flows_match_the_unmerged_case() {
    let unmerged = network(false);
    let (unmerged_angles, unmerged_flows) = solve_dc(&unmerged);
    let angle_of = |bus: BusId| {
        unmerged_angles
            .iter()
            .find(|(id, _)| *id == bus)
            .map(|(_, angle)| *angle)
            .unwrap()
    };

    let with_switch = network(true);
    let rule = BusMergeRule::new(true, Some(ZeroImpedanceRule::PsseThreshold(1e-4)));
    let merge = with_switch.merge_buses(&rule).unwrap();
    // The generator bus 3 carries the jumper group; bus 5 carries bus 6.
    assert_eq!(merge.survivor(BusId(2)), BusId(3));
    assert_eq!(merge.survivor(BusId(4)), BusId(3));
    assert_eq!(merge.survivor(BusId(6)), BusId(5));
    assert_eq!(merge.network.buses().len(), 3);

    let (merged_angles, merged_flows) = solve_dc(&merge.network);
    for (bus, angle) in &merged_angles {
        assert!(
            (angle - angle_of(*bus)).abs() < 1e-6,
            "bus {bus}: merged {angle}, unmerged {}",
            angle_of(*bus)
        );
    }
    for (member, survivor) in &merge.merged_buses {
        let (_, angle) = merged_angles.iter().find(|(id, _)| id == survivor).unwrap();
        assert!((angle - angle_of(*member)).abs() < 1e-6, "member {member}");
    }

    let p_to: Vec<f64> = merged_flows.iter().map(|p| -p).collect();
    let recovered = merge
        .calc_removed_flows(&MergedFlows::new(&merged_flows, &p_to))
        .unwrap();
    assert!(
        recovered.diagnostics.is_empty(),
        "{:#?}",
        recovered.diagnostics
    );
    for (removed, flow) in merge.removed_branches.iter().zip(&recovered.branches) {
        let expected = unmerged_flows[removed.row];
        assert!(
            (flow.p_from - expected).abs() < 1e-3,
            "branch {} ({} to {}): recovered {}, unmerged {expected}",
            removed.row,
            removed.from,
            removed.to,
            flow.p_from
        );
        assert_eq!(flow.method, RemovedFlowMethod::Reactance);
    }
    // The closed switch is the unmerged network's last branch.
    let switch = recovered.switches[0];
    assert!((switch.p_from - unmerged_flows[7]).abs() < 1e-3);
    assert!((switch.p_from - 20.0).abs() < 1e-3);
    assert_eq!(switch.method, RemovedFlowMethod::Tree);
}

/// A reference bus 1 feeding bus 2 by a line; bus 3, with a load, hangs off
/// bus 2 through a closed switch.
fn switched_network() -> BalancedNetwork {
    let mut buses: Vec<Bus> = (1..=3)
        .map(|id| Bus::new(BusId(id), BusType::Pq, 230.0))
        .collect();
    buses[0].kind = BusType::Ref;
    let mut net = BalancedNetwork::in_memory(
        "switched",
        100.0,
        buses,
        vec![Branch::new(BusId(1), BusId(2), 0.0, 0.1)],
    );
    net.switches_mut()
        .push(Switch::new(BusId(2), BusId(3), true));
    let mut slack = Generator::new(BusId(1));
    slack.pg = 30.0;
    slack.pmax = 100.0;
    net.generators_mut().push(slack);
    net.loads_mut().push(Load::new(BusId(3), 30.0, 0.0));
    net
}

#[test]
fn analysis_on_unmerged_closed_switches_is_refused_with_a_code() {
    use powerio_matrix::IndexedNetwork;
    use powerio_matrix::matrix::{BuildOptions, calc_admittance_matrix, calc_ptdf};

    let net = switched_network();
    let view = IndexedNetwork::new(&net);
    let code = |error: powerio_matrix::Error| error.code().code;
    assert_eq!(
        code(calc_admittance_matrix(&view, &BuildOptions::default()).unwrap_err()),
        "BUILD.SWITCH.CLOSED"
    );
    assert_eq!(
        code(calc_ptdf(&view, BranchSusceptanceFormula::default()).unwrap_err()),
        "BUILD.SWITCH.CLOSED"
    );
    let instance = DcPfInstance::from_network(net.clone()).unwrap();
    let error = DcOperators::build(&instance).unwrap_err();
    assert_eq!(error.info().unwrap().code, "BUILD.SWITCH.CLOSED");
    assert!(error.to_string().contains("merge_buses"));
    let opf = powerio_prob::DcOpfInstance::from_network(net.clone()).unwrap();
    let options = powerio_matrix::DcOpfAssemblyOptions::default();
    let error = powerio_matrix::build_dc_opf_preparation(&opf, &options).unwrap_err();
    assert_eq!(code(error), "BUILD.SWITCH.CLOSED");

    // Merging the switch resolves it: the merged network builds and the
    // switch carries the load.
    let merge = net.merge_buses(&BusMergeRule::closed_switches()).unwrap();
    let (_, flows) = solve_dc(&merge.network);
    assert!((flows[0] - 30.0).abs() < 1e-9);
    let p_to: Vec<f64> = flows.iter().map(|p| -p).collect();
    let recovered = merge
        .calc_removed_flows(&MergedFlows::new(&flows, &p_to))
        .unwrap();
    assert!((recovered.switches[0].p_from - 30.0).abs() < 1e-9);
}
