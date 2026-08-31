//! The DC OPF preparation assembly, over the crate private arrays the
//! matrix and bundle builders derive from an instance.

use super::prep::Units;
use super::prep::{DcOpfOptions, DcOpfPreparation, preparation_from_view};
use crate::Error;
use powerio_tx::{
    BalancedNetwork, Branch, BranchSusceptanceFormula, Bus, BusId, BusType, GenCost, Generator,
    IndexedNetwork,
};

fn parse_matpower_file(
    path: impl AsRef<std::path::Path>,
) -> Result<BalancedNetwork, powerio_core::Error> {
    let source = powerio_core::Source::open(path.as_ref())?
        .with_format(powerio_core::FormatId::new("matpower")?);
    powerio_tx::parse(source).map(powerio_core::PioModule::into_value)
}

fn case9() -> BalancedNetwork {
    parse_matpower_file("../tests/data/case9.m").expect("parse case9")
}

fn assert_close(left: f64, right: f64) {
    assert!((left - right).abs() < 1e-12, "{left} != {right}");
}

fn bus(id: usize, kind: BusType) -> Bus {
    Bus::new(BusId(id), kind, 230.0)
}

fn branch(from: usize, to: usize, x: f64) -> Branch {
    Branch::new(BusId(from), BusId(to), 0.0, x)
}

fn generator(bus: usize, c2: f64, c1: f64) -> Generator {
    let mut generator = Generator::new(BusId(bus));
    generator.pmax = 100.0;
    generator.pmin = 10.0;
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![c2, c1, 5.0]));
    generator
}

fn small_network() -> BalancedNetwork {
    let mut network = BalancedNetwork::in_memory(
        "small",
        100.0,
        vec![bus(10, BusType::Ref), bus(30, BusType::Pq)],
        vec![branch(10, 30, 0.2)],
    );
    network.generators_mut().push(generator(10, 1.0, 2.0));
    network
}

/// A two bus MATPOWER case text, one generator at bus 1 per `(c2, c1)` row,
/// written to a tempdir and read back through the MATPOWER reader.
fn case_from_text(cost_rows: &[(f64, f64)]) -> BalancedNetwork {
    use std::fmt::Write as _;
    let mut gens = String::new();
    let mut costs = String::new();
    for &(c2, c1) in cost_rows {
        gens.push_str("1 0 0 30 -30 1 100 1 100 10 0 0 0 0 0 0 0 0 0 0 0;\n");
        writeln!(costs, "2 0 0 3 {c2} {c1} 0;").expect("write cost row");
    }
    let text = format!(
        "function mpc = concave\n\
         mpc.version = '2';\n\
         mpc.baseMVA = 100;\n\
         mpc.bus = [\n\
         1 3 0 0 0 0 1 1 0 230 1 1.1 0.9;\n\
         2 1 10 0 0 0 1 1 0 230 1 1.1 0.9;\n\
         ];\n\
         mpc.gen = [\n{gens}];\n\
         mpc.branch = [\n1 2 0 0.2 0 0 0 0 0 0 1 -360 360;\n];\n\
         mpc.gencost = [\n{costs}];\n"
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("concave.m");
    std::fs::write(&path, text).expect("write case");
    parse_matpower_file(&path).expect("parse case text")
}

fn two_island_network() -> BalancedNetwork {
    let mut network = BalancedNetwork::in_memory(
        "islands",
        100.0,
        vec![
            bus(10, BusType::Ref),
            bus(30, BusType::Pq),
            bus(50, BusType::Ref),
            bus(70, BusType::Pq),
        ],
        vec![branch(10, 30, 0.2), branch(50, 70, 0.3)],
    );
    network.generators_mut().push(generator(10, 1.0, 2.0));
    network.generators_mut().push(generator(50, 4.0, 6.0));
    network
}

#[test]
fn instance_is_complete_and_indexed() {
    let net = case9();
    let view = IndexedNetwork::new(&net);
    let problem = preparation_from_view(&view, DcOpfOptions::default()).expect("build");

    assert_eq!(problem.name, "case9");
    assert_eq!(problem.n_buses, 9);
    assert_eq!(problem.n_source_generators, net.generators().len());
    assert_eq!(problem.n_source_branches, net.branches().len());
    assert_eq!(problem.n_generators(), 3);
    assert_eq!(problem.bus_ids.len(), problem.n_buses);
    assert_eq!(problem.p_d.len(), problem.n_buses);
    assert_eq!(problem.p_shift.len(), problem.n_buses);
    assert_eq!(problem.generators.bus_of_gen.len(), problem.n_generators());
    assert_eq!(problem.branches.from_bus.len(), problem.n_branches());
    assert!(
        problem
            .generators
            .bus_of_gen
            .iter()
            .all(|&bus| bus < problem.n_buses)
    );
    assert!(
        problem
            .branches
            .from_bus
            .iter()
            .chain(&problem.branches.to_bus)
            .all(|&bus| bus < problem.n_buses)
    );
}

#[test]
fn several_generators_at_one_bus_keep_separate_costs_and_aggregate() {
    let mut net = case9();
    let mut extra = net.generators()[0].clone();
    extra.uid = Some("extra-generator".to_owned());
    extra.cost = Some(GenCost::new(2, 0.0, 0.0, vec![7.0, 3.0, 1.0]));
    net.generators_mut().push(extra);

    let view = IndexedNetwork::new(&net);
    let problem = preparation_from_view(&view, DcOpfOptions::default()).expect("build");
    assert_eq!(problem.n_generators(), 4);
    let shared = problem.generators.bus_of_gen[0];
    assert_eq!(shared, problem.generators.bus_of_gen[3]);
    assert!((problem.generators.q[0] - problem.generators.q[3]).abs() > 1e-12);
    assert!((problem.generators.c[0] - problem.generators.c[3]).abs() > 1e-12);

    let gens = &problem.generators;
    let nodal = problem
        .calc_nodal_generator_data()
        .expect("quadratic nodal costs");
    assert!(nodal.has_gen[shared]);
    let parallel_q = 1.0 / (1.0 / gens.q[0] + 1.0 / gens.q[3]);
    assert_close(nodal.q[shared], parallel_q);
    assert_close(
        nodal.c[shared],
        parallel_q * (gens.c[0] / gens.q[0] + gens.c[3] / gens.q[3]),
    );
    assert_close(nodal.pmax[shared], gens.pmax[0] + gens.pmax[3]);
    assert_close(nodal.pmin[shared], gens.pmin[0] + gens.pmin[3]);

    // A bus with one generator keeps that generator's own curve, bit for bit.
    let alone = gens.bus_of_gen[1];
    assert_eq!(nodal.q[alone].to_bits(), gens.q[1].to_bits());
    assert_eq!(nodal.c[alone].to_bits(), gens.c[1].to_bits());
    assert_eq!(nodal.c0[alone].to_bits(), gens.c0[1].to_bits());

    let idle = (0..problem.n_buses)
        .find(|&bus| !nodal.has_gen[bus])
        .expect("a bus without a generator");
    assert_close(nodal.q[idle], 0.0);
    assert_close(nodal.pmax[idle], 0.0);
    assert_close(nodal.pmin[idle], 0.0);
}

#[test]
fn an_unrated_branch_takes_a_synthesized_limit_on_request() {
    let mut net = small_network();
    net.branches_mut()[0].angmin = -30.0;
    net.branches_mut()[0].angmax = 30.0;
    let view = IndexedNetwork::new(&net);
    let options = DcOpfOptions {
        synthesize_unrated_limits: true,
        ..DcOpfOptions::default()
    };

    let unlimited = preparation_from_view(&view, DcOpfOptions::default()).expect("default");
    assert_close(unlimited.branches.f_max[0], 0.0);
    assert!(!unlimited.synthesize_unrated_limits);

    // The bus voltage ceilings are 1.1, the reactance is 0.2, and the window
    // is ±30°.
    let window = 30.0_f64.to_radians();
    let synthesized = preparation_from_view(&view, options).expect("synthesized");
    assert!(synthesized.synthesize_unrated_limits);
    assert_close(
        synthesized.branches.f_max[0],
        1.1 * (2.42 - 2.42 * window.cos()).sqrt() / 0.2,
    );

    let native = preparation_from_view(
        &view,
        DcOpfOptions {
            units: Units::Native,
            ..options
        },
    )
    .expect("native");
    assert_close(
        native.branches.f_max[0],
        synthesized.branches.f_max[0] * view.base_mva(),
    );

    // The normalized network states the same window in radians. Each builder
    // converts by the convention of the network it holds, so the bound is the
    // same one.
    let normalized = net.to_normalized().expect("normalize");
    let derived =
        preparation_from_view(&IndexedNetwork::new(&normalized), options).expect("normalized");
    assert_close(derived.branches.f_max[0], synthesized.branches.f_max[0]);

    // Bounds that run past the half turn state no window, so the bound falls
    // back to the voltage ceilings alone.
    let mut wide = net.clone();
    wide.branches_mut()[0].angmin = -360.0;
    wide.branches_mut()[0].angmax = 360.0;
    let wide = preparation_from_view(&IndexedNetwork::new(&wide), options).expect("wide bounds");
    assert_close(wide.branches.f_max[0], 1.1 * 2.2 / 0.2);

    // `0/0` is the MATPOWER spelling of the same unconstrained branch, so it
    // reaches the same bound. Reading it as a zero wide window would give a
    // zero limit, which the instance reads back as unlimited.
    let mut zero = net.clone();
    zero.branches_mut()[0].angmin = 0.0;
    zero.branches_mut()[0].angmax = 0.0;
    let zero = preparation_from_view(&IndexedNetwork::new(&zero), options).expect("zero bounds");
    assert_close(zero.branches.f_max[0], 1.1 * 2.2 / 0.2);

    let mut rated = net.clone();
    rated.branches_mut()[0].rate_a = 50.0;
    let kept = preparation_from_view(&IndexedNetwork::new(&rated), options).expect("rated branch");
    assert_close(kept.branches.f_max[0], 0.5);
}

#[test]
fn a_network_of_two_islands_grounds_a_bus_in_each() {
    let net = two_island_network();
    let problem =
        preparation_from_view(&IndexedNetwork::new(&net), DcOpfOptions::default()).expect("build");

    assert_eq!(problem.reference_buses.len(), 2);
    assert!(!problem.reference_buses.is_empty());
    assert_eq!(
        problem.reference_buses.iter().copied().collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert!(matches!(
        problem.reference_buses.single(),
        Err(powerio_prob::Error::Transmission(
            powerio_tx::Error::ReferenceBusCount { found: 2, .. }
        ))
    ));

    // The set serializes as a plain array of dense bus indices.
    let mut json = serde_json::to_value(&problem).expect("serialize");
    assert_eq!(json["reference_buses"], serde_json::json!([0, 2]));
    assert!(json["branches"].get("susceptance_magnitude").is_some());
    assert!(json["branches"].get("b").is_none());

    // The 0.10 field remains readable even though 1.0 emits the explicit
    // positive solver quantity name.
    let branches = json["branches"].as_object_mut().expect("branch parameters");
    let old_b = branches
        .remove("susceptance_magnitude")
        .expect("susceptance magnitudes");
    branches.insert("b".to_owned(), old_b);
    let decoded: DcOpfPreparation = serde_json::from_value(json).expect("read 0.10 field");
    assert_eq!(
        decoded.branches.susceptance_magnitude,
        problem.branches.susceptance_magnitude
    );

    let one_island = preparation_from_view(
        &IndexedNetwork::new(&small_network()),
        DcOpfOptions::default(),
    )
    .expect("build");
    assert_eq!(one_island.reference_buses.single().expect("one bus"), 0);
}

#[test]
fn per_unit_and_native_units_scale_all_power_coefficients() {
    let net = small_network();
    let view = IndexedNetwork::new(&net);
    let native = preparation_from_view(
        &view,
        DcOpfOptions {
            units: Units::Native,
            ..DcOpfOptions::default()
        },
    )
    .expect("native");
    let per_unit = preparation_from_view(&view, DcOpfOptions::default()).expect("per unit");
    let base = view.base_mva();

    assert_eq!(native.units, Units::Native);
    assert_eq!(per_unit.units, Units::PerUnit);
    assert_close(
        per_unit.generators.pmax[0],
        native.generators.pmax[0] / base,
    );
    assert_close(
        per_unit.generators.q[0],
        native.generators.q[0] * base * base,
    );
    assert_close(per_unit.generators.c[0], native.generators.c[0] * base);
    // The constant term carries no power dimension, so it never rescales.
    assert_close(per_unit.generators.c0[0], native.generators.c0[0]);
    assert_close(
        native.branches.susceptance_magnitude[0],
        per_unit.branches.susceptance_magnitude[0] * base,
    );
    assert_close(native.branches.f_max[0], per_unit.branches.f_max[0] * base);
}

#[test]
fn cost_constant_term_is_kept() {
    let net = small_network();
    let problem =
        preparation_from_view(&IndexedNetwork::new(&net), DcOpfOptions::default()).expect("build");
    assert_close(problem.generators.c0[0], 5.0);
    let nodal = problem
        .calc_nodal_generator_data()
        .expect("quadratic nodal costs");
    assert_close(nodal.c0[problem.generators.bus_of_gen[0]], 5.0);
}

/// A bus shunt draws constant real power in the DC power flow model. The
/// instance must carry it: the bus susceptance matrix cannot, because its row
/// sums are zero.
#[test]
fn bus_shunt_conductance_reaches_the_instance() {
    let mut net = small_network();
    net.shunts_mut()
        .push(powerio_tx::network::Shunt::new(BusId(30), 5.0, 0.0));
    let view = IndexedNetwork::new(&net);
    let base = view.base_mva();

    let per_unit = preparation_from_view(&view, DcOpfOptions::default()).expect("per unit");
    assert_eq!(per_unit.g_s.len(), per_unit.n_buses);
    assert_close(per_unit.g_s[1], 5.0 / base);
    assert_close(per_unit.g_s[0], 0.0);

    let native = preparation_from_view(
        &view,
        DcOpfOptions {
            units: Units::Native,
            ..DcOpfOptions::default()
        },
    )
    .expect("native");
    assert_close(native.g_s[1], 5.0);
}

/// A case with no shunt reads zero, not a shorter vector.
#[test]
fn a_shuntless_case_carries_zero_conductance() {
    let net = case9();
    let problem =
        preparation_from_view(&IndexedNetwork::new(&net), DcOpfOptions::default()).expect("build");
    assert_eq!(problem.g_s.len(), problem.n_buses);
    assert!(problem.g_s.iter().all(|value| value.abs() < 1e-12));
}

#[test]
fn a_non_finite_susceptance_is_refused_under_every_formula() {
    // The branch susceptance formulas divide by the reactance, and `1/±inf` is `0.0`: the
    // branch would join the instance as a zero-weight edge with nothing to
    // report. `TapAdjustedReactance` divides by `x * tap`, so two finite factors
    // whose product overflows collapse the same way.
    let cases: [(f64, f64); 5] = [
        (f64::NAN, 1.0),
        (f64::INFINITY, 1.0),
        (f64::NEG_INFINITY, 1.0),
        (0.2, f64::INFINITY),
        (1e300, 1e300),
    ];
    for (x, tap) in cases {
        let mut net = small_network();
        net.branches_mut()[0].x = x;
        net.branches_mut()[0].tap = tap;
        let view = IndexedNetwork::new(&net);
        for formula in [
            BranchSusceptanceFormula::ReactanceOnly,
            BranchSusceptanceFormula::TapAdjustedReactance,
            BranchSusceptanceFormula::SeriesSusceptance,
        ] {
            // Only `TapAdjustedReactance` divides by the tap, so a reactance that is finite
            // on its own binds that formula alone; the others read a
            // perfectly good `1/x` and must keep building.
            if x.is_finite() && formula != BranchSusceptanceFormula::TapAdjustedReactance {
                continue;
            }
            let got = preparation_from_view(
                &view,
                DcOpfOptions {
                    formula,
                    ..DcOpfOptions::default()
                },
            );
            assert!(
                matches!(
                    got,
                    Err(Error::Transmission(
                        powerio_tx::Error::NonFiniteSusceptance { row: 0 }
                            | powerio_tx::Error::DegenerateTap { row: 0, .. }
                    ))
                ),
                "{formula:?} accepted x = {x}, tap = {tap}: {:?}",
                got.map(|p| p.branches.susceptance_magnitude)
            );
        }
    }
}

#[test]
fn tap_adjusted_reactance_applies_tap_and_phase_shift() {
    let mut net = small_network();
    net.branches_mut()[0].tap = 1.25;
    net.branches_mut()[0].shift = 10.0;
    let view = IndexedNetwork::new(&net);
    let series = preparation_from_view(&view, DcOpfOptions::default()).expect("series");
    let matpower = preparation_from_view(
        &view,
        DcOpfOptions {
            formula: BranchSusceptanceFormula::TapAdjustedReactance,
            ..DcOpfOptions::default()
        },
    )
    .expect("matpower");

    // Only `TapAdjustedReactance` scales the susceptance by the tap. This branch has no
    // resistance, so the default reads the same `1/x` there.
    assert_close(series.branches.susceptance_magnitude[0], 1.0 / 0.2);
    // Both live conventions carry the phase shift.
    assert_close(series.branches.shift[0], 10.0_f64.to_radians());
    let expected_b = 1.0 / (0.2 * 1.25);
    let expected_shift = 10.0_f64.to_radians();
    assert!((matpower.branches.susceptance_magnitude[0] - expected_b).abs() < 1e-12);
    assert!((matpower.branches.shift[0] - expected_shift).abs() < 1e-12);
    assert!((matpower.p_shift[0] + expected_b * expected_shift).abs() < 1e-12);
    assert!((matpower.p_shift[1] - expected_b * expected_shift).abs() < 1e-12);
}

#[test]
fn phase_shift_and_shunt_complete_the_dc_balance_and_flow_equations() {
    let mut net = small_network();
    net.branches_mut()[0].shift = 10.0;
    net.loads_mut()
        .push(powerio_tx::Load::new(BusId(30), 20.0, 0.0));
    net.shunts_mut()
        .push(powerio_tx::Shunt::new(BusId(30), 5.0, 0.0));
    let problem = preparation_from_view(
        &IndexedNetwork::new(&net),
        DcOpfOptions {
            formula: BranchSusceptanceFormula::TapAdjustedReactance,
            ..DcOpfOptions::default()
        },
    )
    .expect("build shifted instance");

    assert_close(problem.p_shift.iter().sum::<f64>(), 0.0);
    let fixed = problem.calc_fixed_nodal_withdrawal();
    let flow_offset = problem.calc_branch_flow_offset();
    assert_eq!(problem.calc_fixed_nodal_withdrawal(), fixed);
    assert_eq!(problem.calc_branch_flow_offset(), flow_offset);
    let b = problem.branches.susceptance_magnitude[0];
    let shift = 10.0_f64.to_radians();
    assert_close(flow_offset[0], -b * shift);
    assert_close(fixed[0], -b * shift);
    assert_close(fixed[1], 0.25 + b * shift);

    // Ground bus 0. Bus 1 fixes theta_1 through
    // L theta = Cg pg - fixed; total generation is sum(fixed).
    let theta = [0.0, -fixed[1] / b];
    let l_theta = [b * (theta[0] - theta[1]), b * (theta[1] - theta[0])];
    let generation = fixed.iter().sum::<f64>();
    assert_close(l_theta[0], generation - fixed[0]);
    assert_close(l_theta[1], -fixed[1]);

    // The affine branch equation reaches the equivalent physical balance
    // A f = Cg pg - p_d - g_s.
    let flow = b * (theta[0] - theta[1]) + flow_offset[0];
    assert_close(flow, generation - problem.p_d[0] - problem.g_s[0]);
    assert_close(-flow, -problem.p_d[1] - problem.g_s[1]);
}

#[test]
fn source_maps_exclude_out_of_service_elements() {
    let mut net = case9();
    net.generators_mut()[1].in_service = false;
    net.branches_mut()[2].in_service = false;
    let view = IndexedNetwork::new(&net);
    let problem = preparation_from_view(&view, DcOpfOptions::default()).expect("build");

    assert_eq!(problem.n_generators(), 2);
    assert!(!problem.generators.source_rows.contains(&Some(1)));
    assert!(!problem.branches.source_rows.contains(&Some(2)));
    assert_eq!(problem.branches.angle_min.len(), problem.n_branches());
    assert_eq!(problem.branches.angle_max.len(), problem.n_branches());
    assert_eq!(problem.bus_ids[0], view.bus_id(0));
}

#[test]
fn missing_piecewise_and_unsupported_costs_are_distinct() {
    let mut missing = small_network();
    missing.generators_mut()[0].cost = None;
    let error = preparation_from_view(&IndexedNetwork::new(&missing), DcOpfOptions::default())
        .expect_err("missing cost");
    assert!(matches!(
        error,
        Error::Transmission(powerio_tx::Error::MissingGenCost { gen_index: 0 })
    ));

    let mut piecewise = small_network();
    piecewise.generators_mut()[0].cost = Some(GenCost::with_ncost(
        1,
        0.0,
        0.0,
        3,
        vec![0.0, 1.0, 50.0, 101.0, 100.0, 251.0],
    ));
    let prepared = preparation_from_view(&IndexedNetwork::new(&piecewise), DcOpfOptions::default())
        .expect("convex piecewise cost");
    let cost = prepared.generators.piecewise_linear[0]
        .as_ref()
        .expect("piecewise column");
    assert_eq!(cost.power, vec![0.0, 0.5, 1.0]);
    assert_eq!(cost.value, vec![1.0, 101.0, 251.0]);
    assert_eq!(prepared.generators.q, vec![0.0]);
    assert!(matches!(
        prepared.calc_nodal_generator_data(),
        Err(Error::PiecewiseNodalCost { gen_index: 0 })
    ));

    let mut unsupported = small_network();
    unsupported.generators_mut()[0].cost =
        Some(GenCost::with_ncost(3, 0.0, 0.0, 2, vec![0.0, 1.0]));
    let error = preparation_from_view(&IndexedNetwork::new(&unsupported), DcOpfOptions::default())
        .expect_err("unsupported cost model");
    assert!(matches!(
        error,
        Error::UnsupportedCostModel {
            gen_index: 0,
            model: 3,
            ..
        }
    ));
}

#[test]
fn nonconvex_and_malformed_piecewise_costs_are_typed_errors() {
    let mut nonconvex = small_network();
    nonconvex.generators_mut()[0].cost = Some(GenCost::new(
        1,
        0.0,
        0.0,
        vec![0.0, 0.0, 50.0, 100.0, 100.0, 150.0],
    ));
    let error = preparation_from_view(&IndexedNetwork::new(&nonconvex), DcOpfOptions::default())
        .expect_err("decreasing segment slope");
    assert!(matches!(
        error,
        Error::NonconvexPiecewiseCost {
            gen_index: 0,
            segment: 1
        }
    ));

    let mut truncated = small_network();
    truncated.generators_mut()[0].cost = Some(GenCost::with_ncost(
        1,
        0.0,
        0.0,
        3,
        vec![0.0, 0.0, 50.0, 100.0],
    ));
    let error = preparation_from_view(&IndexedNetwork::new(&truncated), DcOpfOptions::default())
        .expect_err("truncated piecewise row");
    assert!(matches!(
        error,
        Error::InvalidPiecewiseCost {
            gen_index: 0,
            reason: crate::PiecewiseCostInvalidity::Truncated {
                expected_values: 6,
                got: 4
            }
        }
    ));
}

#[test]
fn zero_reactance_can_be_skipped_or_rejected() {
    let mut net = small_network();
    net.branches_mut().insert(0, branch(10, 30, 0.0));
    let view = IndexedNetwork::new(&net);
    // Skipping is an explicit opt-in now: the default preserves the branch
    // and refuses assembly.
    let skipped = preparation_from_view(
        &view,
        DcOpfOptions {
            skip_zero_impedance: true,
            ..DcOpfOptions::default()
        },
    )
    .expect("skip");
    assert_eq!(skipped.branches.skipped_zero_impedance, vec![0]);
    assert_eq!(skipped.branches.source_rows, vec![Some(1)]);

    let error = preparation_from_view(&view, DcOpfOptions::default()).expect_err("reject");
    assert!(matches!(
        error,
        Error::Transmission(powerio_tx::Error::ZeroImpedance { row: 0 })
    ));
}

#[test]
fn a_reactance_the_instance_cannot_divide_by_reads_as_zero_impedance() {
    // #292, the rule the matrix builders apply: `x = 1e-300` gives a finite
    // `b = 1e300` that annihilates every real branch sharing a bus with it.
    let mut net = small_network();
    net.branches_mut().insert(0, branch(10, 30, 1e-300));
    let view = IndexedNetwork::new(&net);
    // Skipping is an explicit opt-in now: the default preserves the branch
    // and refuses assembly.
    let skipped = preparation_from_view(
        &view,
        DcOpfOptions {
            skip_zero_impedance: true,
            ..DcOpfOptions::default()
        },
    )
    .expect("skip");
    assert_eq!(skipped.branches.skipped_zero_impedance, vec![0]);

    let error = preparation_from_view(&view, DcOpfOptions::default()).expect_err("reject");
    assert!(matches!(
        error,
        Error::Transmission(powerio_tx::Error::ZeroImpedance { row: 0 })
    ));
}

#[test]
fn a_tap_the_instance_cannot_divide_by_is_refused() {
    for tap in [1e-200, f64::NAN, f64::INFINITY] {
        let mut net = small_network();
        net.branches_mut()[0].tap = tap;
        let error = preparation_from_view(
            &IndexedNetwork::new(&net),
            DcOpfOptions {
                formula: BranchSusceptanceFormula::TapAdjustedReactance,
                ..DcOpfOptions::default()
            },
        )
        .expect_err("a tap the susceptance divides by must be refused");
        assert!(
            matches!(
                error,
                Error::Transmission(powerio_tx::Error::DegenerateTap { row: 0, .. })
            ),
            "tap {tap}: {error}"
        );
    }
}

#[test]
fn a_cost_rounding_artifact_reaches_neither_space() {
    // A model 2 row carrying a leading 1e-17 from the source's rounding states
    // a linear curve. Generator space used to keep it, so the same case read
    // two ways gave two curves.
    let mut net = small_network();
    net.generators_mut()[0].cost = Some(GenCost::new(2, 0.0, 0.0, vec![1e-17, 2.0, 5.0]));
    let problem =
        preparation_from_view(&IndexedNetwork::new(&net), DcOpfOptions::default()).expect("build");
    assert_eq!(problem.generators.q[0].to_bits(), 0.0_f64.to_bits());
    assert_eq!(
        problem.calc_nodal_generator_data().unwrap().q[0].to_bits(),
        0.0_f64.to_bits()
    );
}

/// #342: `BusCost` read a negative quadratic coefficient two ways, quadratic
/// for a lone generator and flat inside a shared bus merge. The build now
/// refuses the row before bus count can decide.
#[test]
fn a_concave_cost_row_is_refused_however_many_generators_share_the_bus() {
    let lone = case_from_text(&[(-0.5, 5.0)]);
    let error = preparation_from_view(&IndexedNetwork::new(&lone), DcOpfOptions::default())
        .expect_err("a lone concave row");
    assert!(
        matches!(error, Error::ConcaveCost { gen_index: 0, c2 } if c2.to_bits() == (-0.5f64).to_bits()),
        "{error}"
    );
    assert_eq!(error.code().code, "BUILD.INSTANCE.CONCAVE_COST");

    let shared = case_from_text(&[(0.04, 20.0), (-0.5, 5.0)]);
    let error = preparation_from_view(&IndexedNetwork::new(&shared), DcOpfOptions::default())
        .expect_err("a concave row in a merge");
    assert!(
        matches!(error, Error::ConcaveCost { gen_index: 1, c2 } if c2.to_bits() == (-0.5f64).to_bits()),
        "{error}"
    );
    assert_eq!(error.code().code, "BUILD.INSTANCE.CONCAVE_COST");
}

#[test]
fn a_flat_row_and_a_convex_row_still_build() {
    // `c2 == 0` keeps the deliberate flat arm: the flat rate is the bus
    // marginal. Coefficients are per unit scaled, `c` by base.
    let flat = case_from_text(&[(2.0, 1.0), (0.0, 3.0)]);
    let problem =
        preparation_from_view(&IndexedNetwork::new(&flat), DcOpfOptions::default()).expect("flat");
    let nodal = problem
        .calc_nodal_generator_data()
        .expect("quadratic nodal costs");
    assert_eq!(problem.calc_nodal_generator_data().unwrap(), nodal);
    let bus = problem.generators.bus_of_gen[0];
    assert_eq!(nodal.q[bus].to_bits(), 0.0_f64.to_bits());
    assert_close(nodal.c[bus], 3.0 * 100.0);

    // A convex row keeps its curve, `q = 2 c2` scaled by base².
    let convex = case_from_text(&[(0.11, 5.0)]);
    let problem = preparation_from_view(&IndexedNetwork::new(&convex), DcOpfOptions::default())
        .expect("convex");
    assert_close(problem.generators.q[0], 2.0 * 0.11 * 100.0 * 100.0);
}

#[test]
fn zero_base_mva_is_rejected() {
    let mut net = small_network();
    *net.base_mva_mut() = 0.0;
    let error = preparation_from_view(&IndexedNetwork::new(&net), DcOpfOptions::default())
        .expect_err("zero base");
    assert!(matches!(
        error,
        Error::Transmission(powerio_tx::Error::InvalidBaseMva { .. })
    ));
}

mod matrix_tests {
    use crate::dcopf::{
        DcOpfAssemblyOptions, DcOpfBundleMetadata, DcOpfBundleOptions, calc_dc_opf_matrices,
        emit_dcopf_bundle,
    };
    use powerio_prob::DcOpfInstance;
    use powerio_tx::{GenCostPolicyReport, MissingGenCostPolicy};

    use super::*;

    fn assert_branch_flow_matrix_manifest_name(manifest: &serde_json::Value) {
        let operators = manifest["operators"].as_array().expect("operator list");
        let has_name = |name| operators.iter().any(|operator| operator["name"] == name);
        assert!(has_name("branch_flow_matrix"));
        assert!(!has_name("flow_map"));
    }

    #[test]
    fn root_emit_dcopf_bundle_reexport_works() {
        let instance = DcOpfInstance::from_network(case9()).expect("instance");
        let output = tempfile::tempdir().expect("tempdir");
        let bundle =
            crate::emit_dcopf_bundle(&instance, output.path(), &DcOpfBundleOptions::default())
                .expect("bundle through root reexport");
        assert!(bundle.dir.join("dcopf_meta.json").is_file());
    }

    #[test]
    fn optional_matrices_match_generic_matrix_calculations() {
        let net = case9();
        let view = IndexedNetwork::new(&net);
        let problem = preparation_from_view(&view, DcOpfOptions::default()).expect("build");
        let instance = DcOpfInstance::from_network(net.clone()).expect("instance");
        let matrices = calc_dc_opf_matrices(&instance, &DcOpfAssemblyOptions::default())
            .expect("matrices from the instance");
        assert_eq!(matrices.bus_branch_incidence.rows(), problem.n_buses);
        assert_eq!(matrices.bus_branch_incidence.cols(), problem.n_branches());
        assert_eq!(matrices.generator_bus.cols(), problem.n_generators());
        assert_eq!(matrices.generator_cost.rows(), problem.n_generators());

        let incidence =
            crate::matrix::build_incidence(&view, problem.formula, &crate::BuildOptions::default())
                .expect("matrix incidence");
        assert_eq!(matrices.bus_branch_incidence, incidence.a);
        assert_eq!(problem.branches.susceptance_magnitude, incidence.b);
    }

    #[test]
    fn a_bundle_write_never_replaces_an_existing_bundle_directory() {
        let instance = DcOpfInstance::from_network(case9()).expect("instance");

        // A regular file at a produced name inside the bundle directory: the
        // write refuses and the file keeps its bytes.
        let output = tempfile::tempdir().expect("tempdir");
        let bundle_dir = output.path().join("case9_dcopf");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        std::fs::write(bundle_dir.join("A.mtx"), b"precious").unwrap();
        let error = emit_dcopf_bundle(&instance, output.path(), &DcOpfBundleOptions::default())
            .unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error}");
        assert_eq!(
            std::fs::read(bundle_dir.join("A.mtx")).unwrap(),
            b"precious"
        );

        // A symbolic link at the bundle directory name: the link survives and
        // the directory it designates keeps its contents.
        #[cfg(unix)]
        {
            let linked = tempfile::tempdir().expect("tempdir");
            let designated = tempfile::tempdir().expect("tempdir");
            std::fs::write(designated.path().join("keep.txt"), b"kept").unwrap();
            std::os::unix::fs::symlink(designated.path(), linked.path().join("case9_dcopf"))
                .unwrap();
            let error = emit_dcopf_bundle(&instance, linked.path(), &DcOpfBundleOptions::default())
                .unwrap_err();
            assert!(error.to_string().contains("already exists"), "{error}");
            assert!(
                std::fs::symlink_metadata(linked.path().join("case9_dcopf"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                std::fs::read(designated.path().join("keep.txt")).unwrap(),
                b"kept"
            );
        }

        // The same write into a fresh output directory still produces the
        // complete inventory the metadata names.
        let fresh = tempfile::tempdir().expect("tempdir");
        let bundle = emit_dcopf_bundle(&instance, fresh.path(), &DcOpfBundleOptions::default())
            .expect("bundle");
        for file in &bundle.files {
            assert!(file.is_file(), "{file:?}");
        }
        assert!(bundle.dir.join("dcopf_meta.json").is_file());
    }

    #[test]
    fn bundle_directory_name_is_confined_to_the_output_directory() {
        // The case name comes from source content, so a hostile spelling must
        // not steer the bundle outside the output directory.
        let mut net = BalancedNetwork::in_memory(
            "../escape/../../attempt",
            100.0,
            vec![bus(1, BusType::Ref), bus(2, BusType::Pq)],
            vec![branch(1, 2, 0.2)],
        );
        net.generators_mut().push(generator(1, 1.0, 2.0));
        let instance = DcOpfInstance::from_network(net).expect("instance");
        let output = tempfile::tempdir().expect("tempdir");
        let bundle = emit_dcopf_bundle(&instance, output.path(), &DcOpfBundleOptions::default())
            .expect("bundle");
        let canonical = bundle.dir.canonicalize().expect("canonical bundle dir");
        let root = output.path().canonicalize().expect("canonical out dir");
        assert!(
            canonical.starts_with(&root),
            "{canonical:?} escaped {root:?}"
        );
    }

    #[test]
    fn bundle_uses_instance_data_and_records_metadata() {
        use std::collections::BTreeSet;

        let mut net = parse_matpower_file("../tests/data/case14.m").expect("parse case14");
        net.branches_mut()[0].shift = 10.0;
        let shunt_bus = net.buses()[1].id;
        net.shunts_mut()
            .push(powerio_tx::Shunt::new(shunt_bus, 5.0, 0.0));
        let assembly = DcOpfAssemblyOptions {
            synthesize_unrated_limits: true,
            skip_zero_impedance: true,
            ..DcOpfAssemblyOptions::default()
        };
        let instance = DcOpfInstance::from_network(net).expect("instance");
        // The preparation the public writer derives, for the row level
        // expectations below.
        let problem =
            crate::dcopf::build_dc_opf_preparation(&instance, &assembly).expect("prepare");
        let output = tempfile::tempdir().expect("tempdir");
        let options = DcOpfBundleOptions {
            assembly,
            metadata: DcOpfBundleMetadata {
                cost_policy: MissingGenCostPolicy::Require,
                cost_report: GenCostPolicyReport {
                    patched: 1,
                    ..GenCostPolicyReport::default()
                },
            },
        };
        let bundle = emit_dcopf_bundle(&instance, output.path(), &options).expect("bundle");

        let incidence = crate::io::read_mtx(bundle.dir.join("A.mtx")).expect("A");
        let branch_b = crate::io::read_vector_mtx(bundle.dir.join("b.mtx")).expect("b");
        assert_eq!(
            incidence,
            crate::dcopf::matrices_from_preparation(&problem).bus_branch_incidence
        );
        assert_eq!(branch_b, problem.branches.susceptance_magnitude);
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(bundle.dir.join("dcopf_meta.json")).expect("manifest"),
        )
        .expect("manifest json");
        assert_eq!(manifest["schema"], "powerio.dcopf");
        assert_eq!(manifest["powerio_version"], powerio_tx::VERSION);
        assert_eq!(manifest["branch_susceptance_formula"], "series_susceptance");
        assert_branch_flow_matrix_manifest_name(&manifest);
        for removed_alias in ["dc_convention", "convention", "n", "m", "n_gen"] {
            assert!(
                manifest.get(removed_alias).is_none(),
                "manifest retained 1.0 alias {removed_alias}"
            );
        }
        assert!(manifest.get("reference_buses").is_none());
        let c0_gen = crate::io::read_vector_mtx(bundle.dir.join("c0_gen.mtx")).expect("c0_gen");
        assert_eq!(c0_gen, problem.generators.c0);
        let shift = crate::io::read_vector_mtx(bundle.dir.join("shift.mtx")).expect("shift");
        assert_eq!(shift, problem.branches.shift);
        let flow_offset =
            crate::io::read_vector_mtx(bundle.dir.join("flow_offset.mtx")).expect("flow_offset");
        assert_eq!(flow_offset, problem.calc_branch_flow_offset());
        let fixed_withdrawal = crate::io::read_vector_mtx(bundle.dir.join("fixed_withdrawal.mtx"))
            .expect("fixed_withdrawal");
        assert_eq!(fixed_withdrawal, problem.calc_fixed_nodal_withdrawal());
        // The shunt conductance a nodal balance subtracts beside `pd`.
        let g_s = crate::io::read_vector_mtx(bundle.dir.join("gs.mtx")).expect("gs");
        assert_eq!(g_s, problem.g_s);
        let c0 = crate::io::read_vector_mtx(bundle.dir.join("c0.mtx")).expect("c0");
        assert_eq!(c0, problem.calc_nodal_generator_data().unwrap().c0);
        assert_eq!(manifest["dimensions"]["n_buses"], problem.n_buses);
        assert_eq!(
            manifest["dimensions"]["n_generators"],
            problem.n_generators()
        );
        assert_eq!(manifest["patched_gen_costs"], 1);
        assert_eq!(manifest["cost_policy"]["mode"], "require");
        assert_eq!(manifest["build_options"]["skip_zero_impedance"], true);
        assert_eq!(manifest["build_options"]["synthesize_unrated_limits"], true);

        let emitted_files: Vec<_> = manifest["files"]
            .as_array()
            .expect("files array")
            .iter()
            .map(|value| value.as_str().expect("file name"))
            .collect();
        let operator_files: Vec<_> = manifest["operators"]
            .as_array()
            .expect("operators array")
            .iter()
            .map(|value| value["file"].as_str().expect("operator file"))
            .collect();
        let emitted_set: BTreeSet<_> = emitted_files.iter().copied().collect();
        let operator_set: BTreeSet<_> = operator_files.iter().copied().collect();
        assert_eq!(
            emitted_files.len(),
            emitted_set.len(),
            "duplicate output file"
        );
        assert_eq!(
            operator_files.len(),
            operator_set.len(),
            "duplicate operator metadata"
        );
        assert_eq!(operator_set, emitted_set);
    }

    #[test]
    fn zero_impedance_is_refused_by_default_and_projects_after_the_merge() {
        let mut net = case9();
        let mut tie = net.branches()[0].clone();
        tie.from = BusId(5);
        tie.to = BusId(6);
        tie.r = 0.0;
        tie.x = 0.0;
        tie.uid = Some("tie-5-6".to_owned());
        net.branches_mut().push(tie);

        // The default preserves the branch and refuses the finite projection
        // rather than skipping.
        let instance = DcOpfInstance::from_network(net.clone()).expect("instance");
        let refused = calc_dc_opf_matrices(&instance, &DcOpfAssemblyOptions::default());
        assert!(refused.is_err(), "the default preserves and refuses");

        // The explicit merge resolves it, and the merged network projects.
        let (merged, _, _) = powerio_prob::merge_zero_impedance_buses(&net).expect("merge");
        let merged_instance = DcOpfInstance::from_network(merged).expect("merged instance");
        calc_dc_opf_matrices(&merged_instance, &DcOpfAssemblyOptions::default())
            .expect("the merged network projects without skipping");
    }
}
