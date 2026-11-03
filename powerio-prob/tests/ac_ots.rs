//! The typed AC optimal transmission switching instance and solution: option
//! resolution and refusal, topology validation and switching cost, the
//! derived current limits against PowerModels, the fixed-topology AC OPF, and
//! the solution's correspondence checks.

// The switching costs and objectives compared here are exact sums.
#![allow(clippy::float_cmp)]

use std::sync::Arc;

use powerio_core::{Error, Source};
use powerio_prob::{
    AcOpfInstance, AcOpfSolution, AcOtsInstance, AcOtsOptions, AcOtsSolution, BranchCurrentLimit,
    BranchSelection, BranchSwitchingCost, CurrentLimitPolicy, IslandingPolicy, ObjectiveTerm,
    Termination,
};
use powerio_tx::{BalancedNetwork, BusId, BusType};

fn parse_case(name: &str) -> BalancedNetwork {
    let path = format!("{}/../tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    powerio_tx::parse(Source::open(&path).unwrap())
        .unwrap_or_else(|error| panic!("{name} parses: {error}"))
        .into_value()
}

/// case9 with its second branch (4-5) initially open, as the Julia test
/// prepares it.
fn case9_with_open_branch() -> BalancedNetwork {
    let mut network = parse_case("case9.m");
    network.branches_mut()[1].in_service = false;
    network
}

fn code(error: &Error) -> Option<&'static str> {
    error.info().map(|info| info.code)
}

fn ids(instance: &AcOtsInstance) -> Vec<String> {
    instance.branch_order().map(str::to_owned).collect()
}

/// The Julia test's explicit options: every branch switchable, at most one
/// open and two changes, costs on rows 1 and 2, and a stated from-terminal
/// current limit on row 1.
fn explicit_options(ids: &[String]) -> AcOtsOptions {
    let mut options = AcOtsOptions::default();
    options.switchable = BranchSelection::All;
    options.max_open = Some(1);
    options.max_switched = Some(2);
    options.switching_costs = vec![
        BranchSwitchingCost::new(ids[1].clone(), 4.0, 7.0),
        BranchSwitchingCost::new(ids[2].clone(), 2.0, 5.0),
    ];
    options.current_limits = CurrentLimitPolicy::Stated;
    options.stated_current_limits = vec![BranchCurrentLimit::new(ids[1].clone(), Some(1.25), None)];
    options
}

/// The defaults over [`case9_with_open_branch`], reconfigured with
/// [`explicit_options`].
fn explicit_instance() -> AcOtsInstance {
    let defaults = AcOtsInstance::from_network(case9_with_open_branch()).unwrap();
    let ids = ids(&defaults);
    defaults.with_options(explicit_options(&ids)).unwrap()
}

/// Close row 1 and open row 2: two changes, one open branch, cost 7 + 2.
fn closed_example() -> Vec<bool> {
    let mut closed = vec![true; 9];
    closed[2] = false;
    closed
}

#[test]
fn defaults_select_the_initially_closed_branches() {
    let instance = AcOtsInstance::from_network(case9_with_open_branch()).unwrap();
    assert_eq!(instance.branches().len(), 9);
    let initial: Vec<bool> = (0..9).map(|row| row != 1).collect();
    assert_eq!(instance.initial_closed(), initial);
    let switchable: Vec<bool> = instance
        .branches()
        .iter()
        .map(|branch| branch.switchable)
        .collect();
    assert_eq!(switchable, initial);
    assert_eq!(instance.options().switchable.kind_name(), "only");
    assert_eq!(instance.options().islanding, IslandingPolicy::Forbidden);
    assert_eq!(instance.options().max_open, None);
    assert_eq!(instance.options().max_switched, None);
    assert!(
        instance
            .objective()
            .contains(&ObjectiveTerm::BranchSwitchingCost)
    );
    // Row 1 is open and fixed: closing it is not a choice.
    let error = instance.validate_topology(&[true; 9]).unwrap_err();
    assert_eq!(code(&error), Some("BUILD.INSTANCE.TOPOLOGY_INVALID"));
    assert_eq!(instance.validate_topology(&initial).unwrap(), 0.0);
}

#[test]
fn explicit_options_resolve_costs_budgets_and_current_limits() {
    let instance = explicit_instance();
    assert!(instance.branches().iter().all(|branch| branch.switchable));
    let row = &instance.branches()[1];
    assert_eq!(row.current.from_pu, Some(1.25));
    assert_eq!(row.current.to_pu, None);
    assert_eq!(row.switching_cost.unwrap().close, 7.0);
    assert!(instance.branches()[0].switching_cost.is_none());

    assert_eq!(instance.validate_topology(&closed_example()).unwrap(), 9.0);
    let error = instance.validate_topology(&[true]).unwrap_err();
    assert_eq!(code(&error), Some("BUILD.INSTANCE.TOPOLOGY_INVALID"));
    let error = instance.validate_topology(&[false; 9]).unwrap_err();
    assert_eq!(code(&error), Some("BUILD.INSTANCE.TOPOLOGY_INVALID"));
    // Two open branches exceed the open budget of one.
    let mut two_open = closed_example();
    two_open[3] = false;
    let error = instance.validate_topology(&two_open).unwrap_err();
    assert!(
        error.to_string().contains("at most 1 may be open"),
        "{error}"
    );
}

#[test]
fn invalid_options_are_refused() {
    let base = AcOpfInstance::from_network(case9_with_open_branch()).unwrap();
    let defaults = AcOtsInstance::from_ac_opf(base.clone()).unwrap();
    let ids = ids(&defaults);
    let refused = |options: AcOtsOptions| {
        let error = AcOtsInstance::new(base.clone(), options).unwrap_err();
        assert_eq!(
            code(&error),
            Some("BUILD.INSTANCE.SWITCHING_OPTIONS_INVALID"),
            "{error}"
        );
        error.to_string()
    };

    let mut options = AcOtsOptions::default();
    options.switchable = BranchSelection::Only(vec![ids[0].clone(), ids[0].clone()]);
    assert!(refused(options).contains("more than once"));
    let mut options = AcOtsOptions::default();
    options.switchable = BranchSelection::Only(vec!["missing".to_owned()]);
    assert!(refused(options).contains("unknown branch `missing`"));
    for cost in [-1.0, f64::NAN, f64::INFINITY] {
        let mut options = AcOtsOptions::default();
        options.switching_costs = vec![BranchSwitchingCost::new(ids[0].clone(), cost, 0.0)];
        refused(options);
    }
    for limit in [0.0, -1.0, f64::NAN] {
        let mut options = AcOtsOptions::default();
        options.stated_current_limits =
            vec![BranchCurrentLimit::new(ids[0].clone(), None, Some(limit))];
        refused(options);
    }
    let mut options = AcOtsOptions::default();
    options.switchable = BranchSelection::Only(vec![ids[0].clone()]);
    options.max_switched = Some(2);
    assert!(refused(options).contains("exceeds the 1 switchable"));
    let mut options = AcOtsOptions::default();
    options.max_open = Some(10);
    refused(options);
    // Row 1 is open and fixed under `None`, so no topology opens zero.
    let mut options = AcOtsOptions::default();
    options.switchable = BranchSelection::None;
    options.max_open = Some(0);
    assert!(refused(options).contains("cannot switch"));
}

#[test]
fn new_keeps_the_base_objective_and_the_term_is_added_once() {
    let base = AcOpfInstance::from_network(parse_case("case9.m")).unwrap();
    let explicit = AcOtsInstance::new(base.clone(), AcOtsOptions::default()).unwrap();
    assert_eq!(explicit.objective(), base.objective());
    assert_eq!(explicit.options().switchable, BranchSelection::All);
    let twice = explicit
        .with_switching_objective()
        .with_switching_objective();
    let terms = twice.objective().terms();
    assert_eq!(
        terms
            .iter()
            .filter(|term| **term == ObjectiveTerm::BranchSwitchingCost)
            .count(),
        1
    );
    assert_eq!(terms.len(), base.objective().terms().len() + 1);
}

/// `c_rating_a` from PowerModels 0.21.7 `calc_current_limits!`, printed by
/// `evals/validation/pm_current_limits.jl`, digit for digit.
#[allow(clippy::unreadable_literal)]
const CASE14_PM_C_RATING_A: [f64; 20] = [
    17.024610503541854,
    4.618928917032787,
    5.209615444146626,
    5.709714635394577,
    5.793351881596143,
    5.770639491493927,
    23.9952530781684,
    5.068870734356915,
    1.9058618576157325,
    4.206024315406389,
    4.809138194889384,
    3.7349587572024867,
    7.255169501635153,
    6.017611399197945,
    9.635508117159512,
    11.740087718547395,
    3.5479113378322005,
    5.07514654043811,
    3.557982947765853,
    2.7338630581987977,
];

#[allow(clippy::unreadable_literal)]
const CASE9_PM_C_RATING_A: [f64; 9] = [
    2.7777777777777777,
    2.7777777777777777,
    1.6666666666666665,
    3.333333333333333,
    1.6666666666666665,
    2.7777777777777777,
    2.7777777777777777,
    2.7777777777777777,
    2.7777777777777777,
];

#[test]
fn derived_current_limits_match_powermodels() {
    for (case, expected) in [
        ("case14.m", &CASE14_PM_C_RATING_A[..]),
        ("case9.m", &CASE9_PM_C_RATING_A[..]),
    ] {
        let base = AcOpfInstance::from_network(parse_case(case)).unwrap();
        let mut options = AcOtsOptions::default();
        options.current_limits = CurrentLimitPolicy::DeriveFromRateA;
        let instance = AcOtsInstance::new(base, options).unwrap();
        assert_eq!(instance.branches().len(), expected.len(), "{case}");
        for (row, (branch, &reference)) in instance.branches().iter().zip(expected).enumerate() {
            for value in [branch.current.from_pu, branch.current.to_pu] {
                let value = value.unwrap_or_else(|| panic!("{case} row {row} has a limit"));
                assert!(
                    (value - reference).abs() <= 1e-12 * reference.max(1.0),
                    "{case} row {row}: {value} vs PowerModels {reference}"
                );
            }
            assert_eq!(branch.stated_current.from_pu, None);
        }
    }
}

#[test]
fn fix_topology_keeps_every_branch_and_reports_the_discards() {
    let instance = explicit_instance();
    let closed = closed_example();
    let (fixed, diagnostics) = instance.fix_topology(&closed).unwrap();
    let statuses: Vec<bool> = fixed
        .network()
        .branches()
        .iter()
        .map(|branch| branch.in_service)
        .collect();
    assert_eq!(statuses, closed);
    assert!(
        !fixed
            .objective()
            .contains(&ObjectiveTerm::BranchSwitchingCost)
    );
    assert_eq!(fixed.constraints(), instance.constraints());
    let messages: Vec<&str> = diagnostics
        .iter()
        .map(powerio_core::Diagnostic::message)
        .collect();
    assert!(diagnostics.len() >= 3, "{messages:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code() == "TRANSFORM.INSTANCE.DATA_DISCARDED"),
        "{messages:#?}"
    );
    // The instance keeps its initial topology.
    assert!(!instance.network().branches()[1].in_service);
    assert!(instance.network().branches()[2].in_service);
}

fn bus_kind(network: &BalancedNetwork, id: usize) -> BusType {
    network
        .buses()
        .iter()
        .find(|bus| bus.id == BusId::new(id))
        .unwrap()
        .kind
}

#[test]
fn allowed_islands_take_one_reference_each() {
    let network = parse_case("case9.m");
    let all_closed = AcOtsInstance::new(
        AcOpfInstance::from_network(network).unwrap(),
        AcOtsOptions::default(),
    )
    .unwrap();
    // Opening 4-5 (row 1) and 9-4 (row 8) cuts buses 1 and 4 off the ring.
    let mut split = vec![true; 9];
    split[1] = false;
    split[8] = false;
    let error = all_closed.validate_topology(&split).unwrap_err();
    assert!(error.to_string().contains("2 islands"), "{error}");

    let mut options = AcOtsOptions::default();
    options.islanding = IslandingPolicy::Allowed;
    let allowed = all_closed.with_options(options).unwrap();
    let (fixed, diagnostics) = allowed.fix_topology(&split).unwrap();
    // Bus 1 keeps its reference; the rest of the ring takes bus 2, host of
    // the largest pmax generator (300 MW).
    assert_eq!(bus_kind(fixed.network(), 1), BusType::Ref);
    assert_eq!(bus_kind(fixed.network(), 2), BusType::Ref);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "TRANSFORM.INSTANCE.ASSUMPTION")
    );
    for island in fixed.network().calc_islands().islands {
        assert_eq!(island.references.len(), 1, "{island:?}");
    }

    // Opening 4-5 (row 1) and 5-6 (row 2) strands load bus 5, which no
    // generator supplies. OTS does not shed load, so the topology is refused
    // rather than handed on as an AC OPF that is infeasible by construction.
    let mut stranded = vec![true; 9];
    stranded[1] = false;
    stranded[2] = false;
    let error = allowed.validate_topology(&stranded).unwrap_err();
    assert_eq!(code(&error), Some("BUILD.INSTANCE.TOPOLOGY_INVALID"));
    assert!(
        error.to_string().contains("no in-service generator"),
        "{error}"
    );
    assert!(allowed.fix_topology(&stranded).is_err());
}

/// Two triangles of three buses each, with generators on buses 1, 2, and 4,
/// plus bus 7, which has no branch. Branch rows 0 to 2 close the first
/// triangle (1-2, 2-3, 3-1) and rows 3 to 5 the second (4-5, 5-6, 6-4), so
/// the initial topology has three AC islands.
fn three_islands() -> BalancedNetwork {
    let case = "function mpc = islands\n\
        mpc.version = '2';\n\
        mpc.baseMVA = 100;\n\
        mpc.bus = [\n\
        \t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\
        \t2\t2\t10\t2\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\
        \t3\t1\t10\t2\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\
        \t4\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\
        \t5\t1\t10\t2\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\
        \t6\t1\t10\t2\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\
        \t7\t1\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\
        ];\n\
        mpc.gen = [\n\
        \t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0;\n\
        \t2\t0\t0\t100\t-100\t1\t100\t1\t200\t0;\n\
        \t4\t0\t0\t100\t-100\t1\t100\t1\t200\t0;\n\
        ];\n\
        mpc.branch = [\n\
        \t1\t2\t0.01\t0.1\t0\t100\t100\t100\t0\t0\t1\t-360\t360;\n\
        \t2\t3\t0.01\t0.1\t0\t100\t100\t100\t0\t0\t1\t-360\t360;\n\
        \t3\t1\t0.01\t0.1\t0\t100\t100\t100\t0\t0\t1\t-360\t360;\n\
        \t4\t5\t0.01\t0.1\t0\t100\t100\t100\t0\t0\t1\t-360\t360;\n\
        \t5\t6\t0.01\t0.1\t0\t100\t100\t100\t0\t0\t1\t-360\t360;\n\
        \t6\t4\t0.01\t0.1\t0\t100\t100\t100\t0\t0\t1\t-360\t360;\n\
        ];\n\
        mpc.gencost = [\n\
        \t2\t0\t0\t3\t0\t1\t0;\n\
        \t2\t0\t0\t3\t0\t1\t0;\n\
        \t2\t0\t0\t3\t0\t1\t0;\n\
        ];\n";
    let source = Source::from_memory("islands.m", case.as_bytes().to_vec()).unwrap();
    powerio_tx::parse(source).unwrap().into_value()
}

#[test]
fn forbidden_means_no_initial_island_splits() {
    let network = three_islands();
    assert_eq!(network.calc_islands().islands.len(), 3);
    let instance = AcOtsInstance::from_network(network).unwrap();
    assert_eq!(instance.options().islanding, IslandingPolicy::Forbidden);

    // A base with several islands accepts its own initial statuses.
    assert_eq!(instance.validate_topology(&[true; 6]).unwrap(), 0.0);
    let (fixed, _) = instance.fix_topology(&[true; 6]).unwrap();
    // An opening that keeps every initial island connected is accepted, in
    // either triangle.
    for open in [0, 4] {
        let mut closed = [true; 6];
        closed[open] = false;
        assert_eq!(instance.validate_topology(&closed).unwrap(), 0.0, "{open}");
    }
    let mut both = [true; 6];
    both[0] = false;
    both[3] = false;
    instance.validate_topology(&both).unwrap();
    // The solution can be built over the initial topology.
    let instance = Arc::new(instance);
    AcOtsSolution::new(
        Arc::clone(&instance),
        vec![true; 6],
        synthetic_ac_solution(Arc::new(fixed), 0.0),
    )
    .unwrap();

    // Opening two branches of one triangle cuts bus 2 or bus 3 off.
    for open in [[0, 1], [1, 2], [0, 2]] {
        let mut closed = [true; 6];
        closed[open[0]] = false;
        closed[open[1]] = false;
        let error = instance.validate_topology(&closed).unwrap_err();
        assert_eq!(code(&error), Some("BUILD.INSTANCE.TOPOLOGY_INVALID"));
        assert!(
            error.to_string().contains("islanding is forbidden"),
            "{error}"
        );
    }
}

#[test]
fn allowed_splits_supplied_islands_but_never_strands_buses() {
    let mut options = AcOtsOptions::default();
    options.switchable = BranchSelection::All;
    options.islanding = IslandingPolicy::Allowed;
    let instance = AcOtsInstance::new(
        AcOpfInstance::from_network(three_islands()).unwrap(),
        options,
    )
    .unwrap();
    assert_eq!(instance.validate_topology(&[true; 6]).unwrap(), 0.0);
    // Opening 1-2 and 2-3 isolates bus 2, which hosts a generator.
    let mut supplied = [true; 6];
    supplied[0] = false;
    supplied[1] = false;
    instance.fix_topology(&supplied).unwrap();
    // Opening 2-3 and 3-1 isolates load bus 3, which has no generator.
    let mut stranded = [true; 6];
    stranded[1] = false;
    stranded[2] = false;
    let error = instance.validate_topology(&stranded).unwrap_err();
    assert_eq!(code(&error), Some("BUILD.INSTANCE.TOPOLOGY_INVALID"));
    assert!(
        error.to_string().contains("no in-service generator"),
        "{error}"
    );
}

fn synthetic_ac_solution(instance: Arc<AcOpfInstance>, objective: f64) -> AcOpfSolution {
    let network = instance.network();
    let buses = network.buses().len();
    let branches = network.branches().len();
    let generators = network.generators().len();
    AcOpfSolution::new(
        instance,
        Termination::NotReported,
        vec![1.01; buses],
        vec![0.0; buses],
        vec![0.0; buses],
        vec![0.0; buses],
        vec![0.0; branches],
        vec![0.0; branches],
        vec![0.0; branches],
        vec![0.0; branches],
        vec![12.0; generators],
        vec![3.0; generators],
        objective,
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn a_solution_answers_its_fixed_topology_problem() {
    let instance = Arc::new(explicit_instance());
    let closed = closed_example();
    let (fixed, _) = instance.fix_topology(&closed).unwrap();
    let ac = synthetic_ac_solution(Arc::new(fixed), 40.0);
    let solution = AcOtsSolution::new(Arc::clone(&instance), closed.clone(), ac.clone()).unwrap();
    assert_eq!(solution.branch_closed(), closed.as_slice());
    assert_eq!(solution.branches_open(), 1);
    assert_eq!(solution.branches_switched(), 2);
    assert_eq!(solution.switching_cost(), 9.0);
    assert_eq!(solution.objective(), 49.0);
    assert_eq!(solution.termination(), &Termination::NotReported);
    assert!(!solution.network().branches()[1].in_service);
    assert!(solution.network().branches()[2].in_service);

    // Without the switching term the objective is the AC objective alone.
    let base_only = Arc::new(
        AcOtsInstance::new(
            AcOpfInstance::from_network(case9_with_open_branch()).unwrap(),
            instance.options().clone(),
        )
        .unwrap(),
    );
    let (fixed, _) = base_only.fix_topology(&closed).unwrap();
    let plain = AcOtsSolution::new(
        base_only,
        closed.clone(),
        synthetic_ac_solution(Arc::new(fixed), 40.0),
    )
    .unwrap();
    assert_eq!(plain.objective(), 40.0);

    // A different topology is a different problem.
    let error = AcOtsSolution::new(Arc::clone(&instance), vec![true; 9], ac).unwrap_err();
    assert_eq!(code(&error), Some("BUILD.SOLUTION.TOPOLOGY_MISMATCH"));
}

#[test]
fn a_solution_refuses_flow_on_open_branches() {
    let instance = Arc::new(explicit_instance());
    let closed = closed_example();
    let fixed = Arc::new(instance.fix_topology(&closed).unwrap().0);
    let network = fixed.network().clone();
    let (buses, branches, generators) = (
        network.buses().len(),
        network.branches().len(),
        network.generators().len(),
    );
    // Row 2 is the only open branch in the final topology.
    let flows_with = |value: f64| {
        let mut flows = vec![0.0; branches];
        flows[2] = value;
        flows
    };
    let solution_with = |flows: Vec<f64>| {
        AcOpfSolution::new(
            Arc::clone(&fixed),
            Termination::Converged,
            vec![1.0; buses],
            vec![0.0; buses],
            vec![0.0; buses],
            vec![0.0; buses],
            flows,
            vec![0.0; branches],
            vec![0.0; branches],
            vec![0.0; branches],
            vec![0.0; generators],
            vec![0.0; generators],
            1.0,
            Vec::new(),
        )
        .unwrap()
    };
    let error = AcOtsSolution::new(
        Arc::clone(&instance),
        closed.clone(),
        solution_with(flows_with(1.0)),
    )
    .unwrap_err();
    assert_eq!(code(&error), Some("BUILD.SOLUTION.VALUE_INVALID"));
    assert!(error.to_string().contains("carries flow"), "{error}");
    // A NaN flow on an open branch records no flow.
    AcOtsSolution::new(instance, closed, solution_with(flows_with(f64::NAN))).unwrap();
}

#[test]
fn nan_rows_the_ac_solution_accepts_construct() {
    let instance = Arc::new(explicit_instance());
    let closed = closed_example();
    let fixed = Arc::new(instance.fix_topology(&closed).unwrap().0);
    let network = fixed.network();
    let (buses, branches, generators) = (
        network.buses().len(),
        network.branches().len(),
        network.generators().len(),
    );
    // A non-converged result: a NaN objective, a NaN dispatch row for an
    // out-of-service generator, and NaN flows on branches with no solution.
    let mut dispatch = vec![12.0; generators];
    dispatch[0] = f64::NAN;
    let mut flows = vec![5.0; branches];
    flows[2] = 0.0;
    flows[3] = f64::NAN;
    let ac = AcOpfSolution::new(
        Arc::clone(&fixed),
        Termination::NotReported,
        vec![1.0; buses],
        vec![0.0; buses],
        vec![0.0; buses],
        vec![0.0; buses],
        flows.clone(),
        flows.clone(),
        flows.clone(),
        flows,
        dispatch.clone(),
        dispatch,
        f64::NAN,
        Vec::new(),
    )
    .unwrap();
    let solution = AcOtsSolution::new(instance, closed, ac).unwrap();
    assert!(solution.objective().is_nan());
    assert_eq!(solution.switching_cost(), 9.0);
}

#[test]
fn two_parallel_branches_count_open_and_switched_rows() {
    let case = "function mpc = two\n\
        mpc.version = '2';\n\
        mpc.baseMVA = 100;\n\
        mpc.bus = [\n\
        \t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\
        \t2\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;\n\
        ];\n\
        mpc.gen = [\n\
        \t1\t0\t0\t100\t-100\t1\t100\t1\t200\t0;\n\
        ];\n\
        mpc.branch = [\n\
        \t1\t2\t0.01\t0.1\t0\t100\t100\t100\t0\t0\t1\t-360\t360;\n\
        \t1\t2\t0.01\t0.1\t0\t100\t100\t100\t0\t0\t1\t-360\t360;\n\
        ];\n\
        mpc.gencost = [\n\
        \t2\t0\t0\t3\t0\t1\t0;\n\
        ];\n";
    let source = Source::from_memory("two.m", case.as_bytes().to_vec()).unwrap();
    let network = powerio_tx::parse(source).unwrap().into_value();
    let instance = Arc::new(AcOtsInstance::from_network(network).unwrap());
    for (closed, open, switched) in [
        (vec![true, true], 0, 0),
        (vec![true, false], 1, 1),
        (vec![false, true], 1, 1),
    ] {
        let (fixed, _) = instance.fix_topology(&closed).unwrap();
        let solution = AcOtsSolution::new(
            Arc::clone(&instance),
            closed.clone(),
            synthetic_ac_solution(Arc::new(fixed), 0.0),
        )
        .unwrap();
        assert_eq!(solution.branches_open(), open, "{closed:?}");
        assert_eq!(solution.branches_switched(), switched, "{closed:?}");
    }
    // Opening both strands bus 2, and islanding is forbidden.
    let error = instance.validate_topology(&[false, false]).unwrap_err();
    assert_eq!(code(&error), Some("BUILD.INSTANCE.TOPOLOGY_INVALID"));
}
