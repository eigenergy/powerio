mod helpers;
#[allow(unused_imports)]
use helpers::*;
use powerio_prob::{DcOpfOptions, Error, Units, build_dc_opf_instance};
use powerio_tx::{
    BalancedNetwork, Branch, Bus, BusId, BusType, DcConvention, GenCost, Generator, IndexedNetwork,
};

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
    let problem = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("build");

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
    let problem = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("build");
    assert_eq!(problem.n_generators(), 4);
    let shared = problem.generators.bus_of_gen[0];
    assert_eq!(shared, problem.generators.bus_of_gen[3]);
    assert!((problem.generators.q[0] - problem.generators.q[3]).abs() > 1e-12);
    assert!((problem.generators.c[0] - problem.generators.c[3]).abs() > 1e-12);

    let gens = &problem.generators;
    let nodal = problem.nodal_generator_data();
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

    let unlimited = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("default");
    assert_close(unlimited.branches.f_max[0], 0.0);
    assert!(!unlimited.synthesize_unrated_limits);

    // The bus voltage ceilings are 1.1, the reactance is 0.2, and the window
    // is ±30°.
    let window = 30.0_f64.to_radians();
    let synthesized = build_dc_opf_instance(&view, &options).expect("synthesized");
    assert!(synthesized.synthesize_unrated_limits);
    assert_close(
        synthesized.branches.f_max[0],
        1.1 * (2.42 - 2.42 * window.cos()).sqrt() / 0.2,
    );

    let native = build_dc_opf_instance(
        &view,
        &DcOpfOptions {
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
        build_dc_opf_instance(&IndexedNetwork::new(&normalized), &options).expect("normalized");
    assert_close(derived.branches.f_max[0], synthesized.branches.f_max[0]);

    // Bounds that run past the half turn state no window, so the bound falls
    // back to the voltage ceilings alone.
    let mut wide = net.clone();
    wide.branches_mut()[0].angmin = -360.0;
    wide.branches_mut()[0].angmax = 360.0;
    let wide = build_dc_opf_instance(&IndexedNetwork::new(&wide), &options).expect("wide bounds");
    assert_close(wide.branches.f_max[0], 1.1 * 2.2 / 0.2);

    // `0/0` is the MATPOWER spelling of the same unconstrained branch, so it
    // reaches the same bound. Reading it as a zero wide window would give a
    // zero limit, which the instance reads back as unlimited.
    let mut zero = net.clone();
    zero.branches_mut()[0].angmin = 0.0;
    zero.branches_mut()[0].angmax = 0.0;
    let zero = build_dc_opf_instance(&IndexedNetwork::new(&zero), &options).expect("zero bounds");
    assert_close(zero.branches.f_max[0], 1.1 * 2.2 / 0.2);

    let mut rated = net.clone();
    rated.branches_mut()[0].rate_a = 50.0;
    let kept = build_dc_opf_instance(&IndexedNetwork::new(&rated), &options).expect("rated branch");
    assert_close(kept.branches.f_max[0], 0.5);
}

#[test]
fn a_network_of_two_islands_grounds_a_bus_in_each() {
    let net = two_island_network();
    let problem =
        build_dc_opf_instance(&IndexedNetwork::new(&net), &DcOpfOptions::default()).expect("build");

    assert_eq!(problem.reference_buses.len(), 2);
    assert!(!problem.reference_buses.is_empty());
    assert_eq!(
        problem.reference_buses.iter().copied().collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert!(matches!(
        problem.reference_buses.single(),
        Err(powerio_prob::Error::Core(
            powerio_tx::Error::ReferenceBusCount { found: 2, .. }
        ))
    ));

    // The set serializes as a plain array of dense bus indices.
    let json = serde_json::to_value(&problem).expect("serialize");
    assert_eq!(json["reference_buses"], serde_json::json!([0, 2]));

    let one_island = build_dc_opf_instance(
        &IndexedNetwork::new(&small_network()),
        &DcOpfOptions::default(),
    )
    .expect("build");
    assert_eq!(one_island.reference_buses.single().expect("one bus"), 0);
}

#[test]
fn per_unit_and_native_units_scale_all_power_coefficients() {
    let net = small_network();
    let view = IndexedNetwork::new(&net);
    let native = build_dc_opf_instance(
        &view,
        &DcOpfOptions {
            units: Units::Native,
            ..DcOpfOptions::default()
        },
    )
    .expect("native");
    let per_unit = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("per unit");
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
    assert_close(native.branches.b[0], per_unit.branches.b[0] * base);
    assert_close(native.branches.f_max[0], per_unit.branches.f_max[0] * base);
}

#[test]
fn cost_constant_term_is_kept() {
    let net = small_network();
    let problem =
        build_dc_opf_instance(&IndexedNetwork::new(&net), &DcOpfOptions::default()).expect("build");
    assert_close(problem.generators.c0[0], 5.0);
    let nodal = problem.nodal_generator_data();
    assert_close(nodal.c0[problem.generators.bus_of_gen[0]], 5.0);
}

/// A bus shunt draws constant real power under the DC approximation. The
/// instance must carry it: the bus susceptance matrix cannot, because its row
/// sums are zero.
#[test]
fn bus_shunt_conductance_reaches_the_instance() {
    let mut net = small_network();
    net.shunts_mut()
        .push(powerio_tx::network::Shunt::new(BusId(30), 5.0, 0.0));
    let view = IndexedNetwork::new(&net);
    let base = view.base_mva();

    let per_unit = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("per unit");
    assert_eq!(per_unit.g_s.len(), per_unit.n_buses);
    assert_close(per_unit.g_s[1], 5.0 / base);
    assert_close(per_unit.g_s[0], 0.0);

    let native = build_dc_opf_instance(
        &view,
        &DcOpfOptions {
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
        build_dc_opf_instance(&IndexedNetwork::new(&net), &DcOpfOptions::default()).expect("build");
    assert_eq!(problem.g_s.len(), problem.n_buses);
    assert!(problem.g_s.iter().all(|value| value.abs() < 1e-12));
}

#[test]
fn a_non_finite_susceptance_is_refused_under_every_convention() {
    // The DC conventions divide by the reactance, and `1/±inf` is `0.0`: the
    // branch would join the instance as a zero-weight edge with nothing to
    // report. The Matpower rule divides by `x * tap`, so two finite factors
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
        for convention in [
            DcConvention::ReactanceOnly,
            DcConvention::Matpower,
            DcConvention::SeriesImpedance,
        ] {
            // Only Matpower divides by the tap, so a reactance that is finite
            // on its own binds that convention alone; the others read a
            // perfectly good `1/x` and must keep building.
            if x.is_finite() && convention != DcConvention::Matpower {
                continue;
            }
            let got = build_dc_opf_instance(
                &view,
                &DcOpfOptions {
                    convention,
                    ..DcOpfOptions::default()
                },
            );
            assert!(
                matches!(
                    got,
                    Err(Error::Core(
                        powerio_tx::Error::NonFiniteSusceptance { row: 0 }
                            | powerio_tx::Error::DegenerateTap { row: 0, .. }
                    ))
                ),
                "{convention:?} accepted x = {x}, tap = {tap}: {:?}",
                got.map(|p| p.branches.b)
            );
        }
    }
}

#[test]
fn matpower_convention_applies_tap_and_phase_shift() {
    let mut net = small_network();
    net.branches_mut()[0].tap = 1.25;
    net.branches_mut()[0].shift = 10.0;
    let view = IndexedNetwork::new(&net);
    let series = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("series");
    let matpower = build_dc_opf_instance(
        &view,
        &DcOpfOptions {
            convention: DcConvention::Matpower,
            ..DcOpfOptions::default()
        },
    )
    .expect("matpower");

    // Only Matpower scales the susceptance by the tap. This branch has no
    // resistance, so the default reads the same `1/x` there.
    assert_close(series.branches.b[0], 1.0 / 0.2);
    // Both live conventions carry the phase shift.
    assert_close(series.branches.shift[0], 10.0_f64.to_radians());
    let expected_b = 1.0 / (0.2 * 1.25);
    let expected_shift = 10.0_f64.to_radians();
    assert!((matpower.branches.b[0] - expected_b).abs() < 1e-12);
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
    let problem = build_dc_opf_instance(
        &IndexedNetwork::new(&net),
        &DcOpfOptions {
            convention: DcConvention::Matpower,
            ..DcOpfOptions::default()
        },
    )
    .expect("build shifted instance");

    assert_close(problem.p_shift.iter().sum::<f64>(), 0.0);
    let fixed = problem.fixed_nodal_withdrawal();
    let flow_offset = problem.branch_flow_offset();
    let b = problem.branches.b[0];
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
    let problem = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("build");

    assert_eq!(problem.n_generators(), 2);
    assert!(!problem.generators.source_rows.contains(&1));
    assert!(!problem.branches.source_rows.contains(&2));
    assert_eq!(problem.branches.angle_min.len(), problem.n_branches());
    assert_eq!(problem.branches.angle_max.len(), problem.n_branches());
    assert_eq!(problem.bus_ids[0], view.bus_id(0));
}

#[test]
fn missing_and_unsupported_costs_are_distinct() {
    let mut missing = small_network();
    missing.generators_mut()[0].cost = None;
    let error = build_dc_opf_instance(&IndexedNetwork::new(&missing), &DcOpfOptions::default())
        .expect_err("missing cost");
    assert!(matches!(
        error,
        Error::Core(powerio_tx::Error::MissingGenCost { gen_index: 0 })
    ));

    let mut piecewise = small_network();
    piecewise.generators_mut()[0].cost = Some(GenCost::with_ncost(
        1,
        0.0,
        0.0,
        2,
        vec![0.0, 0.0, 1.0, 1.0],
    ));
    let error = build_dc_opf_instance(&IndexedNetwork::new(&piecewise), &DcOpfOptions::default())
        .expect_err("unsupported cost");
    assert!(matches!(
        error,
        Error::UnsupportedCostModel {
            gen_index: 0,
            model: 1,
            ..
        }
    ));
}

#[test]
fn zero_reactance_can_be_skipped_or_rejected() {
    let mut net = small_network();
    net.branches_mut().insert(0, branch(10, 30, 0.0));
    let view = IndexedNetwork::new(&net);
    let skipped = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("skip");
    assert_eq!(skipped.branches.skipped_zero_impedance, vec![0]);
    assert_eq!(skipped.branches.source_rows, vec![1]);

    let error = build_dc_opf_instance(
        &view,
        &DcOpfOptions {
            skip_zero_impedance: false,
            ..DcOpfOptions::default()
        },
    )
    .expect_err("reject");
    assert!(matches!(
        error,
        Error::Core(powerio_tx::Error::ZeroImpedance { row: 0 })
    ));
}

#[test]
fn a_reactance_the_instance_cannot_divide_by_reads_as_zero_impedance() {
    // #292, the rule the matrix builders apply: `x = 1e-300` gives a finite
    // `b = 1e300` that annihilates every real branch sharing a bus with it.
    let mut net = small_network();
    net.branches_mut().insert(0, branch(10, 30, 1e-300));
    let view = IndexedNetwork::new(&net);
    let skipped = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("skip");
    assert_eq!(skipped.branches.skipped_zero_impedance, vec![0]);

    let error = build_dc_opf_instance(
        &view,
        &DcOpfOptions {
            skip_zero_impedance: false,
            ..DcOpfOptions::default()
        },
    )
    .expect_err("reject");
    assert!(matches!(
        error,
        Error::Core(powerio_tx::Error::ZeroImpedance { row: 0 })
    ));
}

#[test]
fn a_tap_the_instance_cannot_divide_by_is_refused() {
    for tap in [1e-200, f64::NAN, f64::INFINITY] {
        let mut net = small_network();
        net.branches_mut()[0].tap = tap;
        let error = build_dc_opf_instance(
            &IndexedNetwork::new(&net),
            &DcOpfOptions {
                convention: DcConvention::Matpower,
                ..DcOpfOptions::default()
            },
        )
        .expect_err("a tap the susceptance divides by must be refused");
        assert!(
            matches!(
                error,
                Error::Core(powerio_tx::Error::DegenerateTap { row: 0, .. })
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
        build_dc_opf_instance(&IndexedNetwork::new(&net), &DcOpfOptions::default()).expect("build");
    assert_eq!(problem.generators.q[0].to_bits(), 0.0_f64.to_bits());
    assert_eq!(
        problem.nodal_generator_data().q[0].to_bits(),
        0.0_f64.to_bits()
    );
}

/// #342: `BusCost` read a negative quadratic coefficient two ways, quadratic
/// for a lone generator and flat inside a shared bus merge. The build now
/// refuses the row before bus count can decide.
#[test]
fn a_concave_cost_row_is_refused_however_many_generators_share_the_bus() {
    let lone = case_from_text(&[(-0.5, 5.0)]);
    let error = build_dc_opf_instance(&IndexedNetwork::new(&lone), &DcOpfOptions::default())
        .expect_err("a lone concave row");
    assert!(
        matches!(error, Error::ConcaveCost { gen_index: 0, c2 } if c2.to_bits() == (-0.5f64).to_bits()),
        "{error}"
    );
    assert_eq!(error.code().code, "BUILD.INSTANCE.CONCAVE_COST");

    let shared = case_from_text(&[(0.04, 20.0), (-0.5, 5.0)]);
    let error = build_dc_opf_instance(&IndexedNetwork::new(&shared), &DcOpfOptions::default())
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
        build_dc_opf_instance(&IndexedNetwork::new(&flat), &DcOpfOptions::default()).expect("flat");
    let nodal = problem.nodal_generator_data();
    let bus = problem.generators.bus_of_gen[0];
    assert_eq!(nodal.q[bus].to_bits(), 0.0_f64.to_bits());
    assert_close(nodal.c[bus], 3.0 * 100.0);

    // A convex row keeps its curve, `q = 2 c2` scaled by base².
    let convex = case_from_text(&[(0.11, 5.0)]);
    let problem = build_dc_opf_instance(&IndexedNetwork::new(&convex), &DcOpfOptions::default())
        .expect("convex");
    assert_close(problem.generators.q[0], 2.0 * 0.11 * 100.0 * 100.0);
}

#[test]
fn zero_base_mva_is_rejected() {
    let mut net = small_network();
    *net.base_mva_mut() = 0.0;
    let error = build_dc_opf_instance(&IndexedNetwork::new(&net), &DcOpfOptions::default())
        .expect_err("zero base");
    assert!(matches!(
        error,
        Error::Core(powerio_tx::Error::InvalidBaseMva { .. })
    ));
}

#[test]
fn serde_round_trip() {
    let net = case9();
    let view = IndexedNetwork::new(&net);
    let problem = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("build");
    let json = serde_json::to_string(&problem).expect("serialize");
    let back: powerio_prob::DcOpfInstance = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.name, problem.name);
    assert_eq!(back.bus_ids, problem.bus_ids);
    assert_eq!(back.generators.source_rows, problem.generators.source_rows);
    assert_eq!(back.branches.source_rows, problem.branches.source_rows);
    assert_eq!(
        back.synthesize_unrated_limits,
        problem.synthesize_unrated_limits
    );
    for (left, right) in back.branches.b.iter().zip(&problem.branches.b) {
        assert!((left - right).abs() < 1e-12);
    }
}

#[test]
fn instance_deserializes_without_synthesize_unrated_limits() {
    let net = case9();
    let problem =
        build_dc_opf_instance(&IndexedNetwork::new(&net), &DcOpfOptions::default()).expect("build");
    let mut value = serde_json::to_value(problem).expect("serialize");
    value
        .as_object_mut()
        .expect("instance object")
        .remove("synthesize_unrated_limits");
    let back: powerio_prob::DcOpfInstance =
        serde_json::from_value(value).expect("read earlier instance");
    assert!(!back.synthesize_unrated_limits);
}

#[test]
fn options_deserialize_without_synthesize_unrated_limits() {
    // A document written before the field existed carries the other three
    // fields only; it must deserialize to the field's default (off).
    let json = r#"{
        "convention": "Matpower",
        "units": "PerUnit",
        "skip_zero_impedance": true
    }"#;
    let options: DcOpfOptions = serde_json::from_str(json).expect("deserialize");
    assert!(!options.synthesize_unrated_limits);
}

#[cfg(feature = "matrix")]
mod matrix_tests {
    use powerio_prob::matrix::{
        DcOpfBundleMetadata, DcOpfBundleOptions, build_dc_opf_matrices, write_dcopf_bundle,
    };
    use powerio_tx::{GenCostPolicyReport, MissingGenCostPolicy};

    use super::*;

    #[test]
    fn optional_matrices_match_generic_matrix_builders() {
        let net = case9();
        let view = IndexedNetwork::new(&net);
        let problem = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("build");
        let matrices = build_dc_opf_matrices(&problem);
        assert_eq!(matrices.incidence.rows(), problem.n_buses);
        assert_eq!(matrices.incidence.cols(), problem.n_branches());
        assert_eq!(matrices.generator_bus.cols(), problem.n_generators());
        assert_eq!(matrices.generator_cost.rows(), problem.n_generators());

        let incidence = powerio_matrix::build_incidence(
            &view,
            problem.convention,
            &powerio_matrix::BuildOptions::default(),
        )
        .expect("matrix incidence");
        assert_eq!(matrices.incidence, incidence.a);
        assert_eq!(problem.branches.b, incidence.b);
        assert_eq!(problem.p_shift, incidence.p_shift);
    }

    #[test]
    fn bundle_directory_name_is_confined_to_the_output_directory() {
        let net = case9();
        let mut problem =
            build_dc_opf_instance(&IndexedNetwork::new(&net), &DcOpfOptions::default())
                .expect("build");
        problem.name = "../escape/../../attempt".to_owned();
        let output = tempfile::tempdir().expect("tempdir");
        let bundle = write_dcopf_bundle(&problem, output.path(), &DcOpfBundleOptions::default())
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
        let problem = build_dc_opf_instance(
            &IndexedNetwork::new(&net),
            &DcOpfOptions {
                synthesize_unrated_limits: true,
                ..DcOpfOptions::default()
            },
        )
        .expect("build");
        let output = tempfile::tempdir().expect("tempdir");
        let options = DcOpfBundleOptions {
            metadata: DcOpfBundleMetadata {
                cost_policy: MissingGenCostPolicy::Require,
                cost_report: GenCostPolicyReport {
                    patched: 1,
                    ..GenCostPolicyReport::default()
                },
            },
        };
        let bundle = write_dcopf_bundle(&problem, output.path(), &options).expect("bundle");

        let incidence = powerio_matrix::io::read_mtx(bundle.dir.join("A.mtx")).expect("A");
        let branch_b = powerio_matrix::io::read_vector_mtx(bundle.dir.join("b.mtx")).expect("b");
        assert_eq!(incidence, build_dc_opf_matrices(&problem).incidence);
        assert_eq!(branch_b, problem.branches.b);
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(bundle.dir.join("dcopf_meta.json")).expect("manifest"),
        )
        .expect("manifest json");
        assert_eq!(manifest["schema"], "powerio.dcopf");
        assert_eq!(manifest["powerio_version"], powerio_tx::VERSION);
        let c0_gen =
            powerio_matrix::io::read_vector_mtx(bundle.dir.join("c0_gen.mtx")).expect("c0_gen");
        assert_eq!(c0_gen, problem.generators.c0);
        let shift =
            powerio_matrix::io::read_vector_mtx(bundle.dir.join("shift.mtx")).expect("shift");
        assert_eq!(shift, problem.branches.shift);
        let flow_offset = powerio_matrix::io::read_vector_mtx(bundle.dir.join("flow_offset.mtx"))
            .expect("flow_offset");
        assert_eq!(flow_offset, problem.branch_flow_offset());
        let fixed_withdrawal =
            powerio_matrix::io::read_vector_mtx(bundle.dir.join("fixed_withdrawal.mtx"))
                .expect("fixed_withdrawal");
        assert_eq!(fixed_withdrawal, problem.fixed_nodal_withdrawal());
        // The shunt conductance a nodal balance subtracts beside `pd`.
        let g_s = powerio_matrix::io::read_vector_mtx(bundle.dir.join("gs.mtx")).expect("gs");
        assert_eq!(g_s, problem.g_s);
        let c0 = powerio_matrix::io::read_vector_mtx(bundle.dir.join("c0.mtx")).expect("c0");
        assert_eq!(c0, problem.nodal_generator_data().c0);
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
}
