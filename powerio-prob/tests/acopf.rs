use powerio::{
    Branch, BranchCharging, Bus, BusId, BusType, Error, GenCost, Generator, IndexedNetwork,
    Network, parse_matpower_file,
};
use powerio_prob::{AcOpfOptions, Units, build_ac_opf_instance};

fn case9() -> Network {
    parse_matpower_file("../tests/data/case9.m").expect("parse case9")
}

fn case14() -> Network {
    parse_matpower_file("../tests/data/case14.m").expect("parse case14")
}

fn assert_close(left: f64, right: f64) {
    assert!((left - right).abs() < 1e-12, "{left} != {right}");
}

fn bus(id: usize, kind: BusType) -> Bus {
    Bus::new(BusId(id), kind, 230.0)
}

fn branch(from: usize, to: usize, r: f64, x: f64) -> Branch {
    Branch::new(BusId(from), BusId(to), r, x)
}

fn generator(bus: usize, c2: f64, c1: f64, c0: f64) -> Generator {
    let mut generator = Generator::new(BusId(bus));
    generator.pmax = 100.0;
    generator.pmin = 10.0;
    generator.qmax = 40.0;
    generator.qmin = -40.0;
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![c2, c1, c0]));
    generator
}

fn small_network() -> Network {
    let mut network = Network::in_memory(
        "small",
        100.0,
        vec![bus(10, BusType::Ref), bus(30, BusType::Pq)],
        vec![branch(10, 30, 0.05, 0.2)],
    );
    network.generators.push(generator(10, 1.0, 2.0, 5.0));
    network
}

#[test]
fn instance_is_complete_and_indexed() {
    let net = case9();
    let view = IndexedNetwork::new(&net);
    let problem = build_ac_opf_instance(&view, &AcOpfOptions::default()).expect("build");

    assert_eq!(problem.name, "case9");
    assert_eq!(problem.n_buses, 9);
    assert_eq!(problem.n_source_generators, net.generators.len());
    assert_eq!(problem.n_source_branches, net.branches.len());
    assert_eq!(problem.n_generators(), 3);
    assert_eq!(problem.bus_ids.len(), problem.n_buses);
    for vector in [
        &problem.buses.p_d,
        &problem.buses.q_d,
        &problem.buses.g_s,
        &problem.buses.b_s,
        &problem.buses.vm_min,
        &problem.buses.vm_max,
        &problem.buses.vm,
    ] {
        assert_eq!(vector.len(), problem.n_buses);
    }
    for vector in [
        &problem.generators.q,
        &problem.generators.c,
        &problem.generators.c0,
        &problem.generators.pmax,
        &problem.generators.pmin,
        &problem.generators.qmax,
        &problem.generators.qmin,
        &problem.generators.pg,
        &problem.generators.qg,
        &problem.generators.vg,
    ] {
        assert_eq!(vector.len(), problem.n_generators());
    }
    for vector in [
        &problem.branches.g,
        &problem.branches.b,
        &problem.branches.g_fr,
        &problem.branches.b_fr,
        &problem.branches.g_to,
        &problem.branches.b_to,
        &problem.branches.tap,
        &problem.branches.shift,
        &problem.branches.s_max,
        &problem.branches.angle_min,
        &problem.branches.angle_max,
    ] {
        assert_eq!(vector.len(), problem.n_branches());
    }
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
fn case14_taps_shunt_and_series_admittance() {
    let net = case14();
    let view = IndexedNetwork::new(&net);
    let problem = build_ac_opf_instance(&view, &AcOpfOptions::default()).expect("build");

    let mut taps: Vec<f64> = problem
        .branches
        .tap
        .iter()
        .copied()
        .filter(|&tap| (tap - 1.0).abs() > 1e-12)
        .collect();
    taps.sort_by(f64::total_cmp);
    for (tap, expected) in taps.iter().zip([0.932, 0.969, 0.978]) {
        assert_close(*tap, expected);
    }
    assert_eq!(taps.len(), 3);
    assert!(
        problem
            .branches
            .shift
            .iter()
            .all(|&shift| shift.abs() < 1e-12)
    );

    let bus9 = problem
        .bus_ids
        .iter()
        .position(|&id| id == BusId(9))
        .expect("bus 9");
    assert_close(problem.buses.b_s[bus9], 0.19);
    assert_close(problem.buses.g_s[bus9], 0.0);

    let first = &net.branches[0];
    let z_squared = first.r * first.r + first.x * first.x;
    assert_eq!(problem.branches.source_rows[0], 0);
    assert_close(problem.branches.g[0], first.r / z_squared);
    assert_close(problem.branches.b[0], -first.x / z_squared);
    assert_close(problem.buses.vm[0], 1.06);
}

#[test]
fn tap_and_shift_are_carried_separately() {
    let mut net = small_network();
    net.branches[0].tap = 1.25;
    net.branches[0].shift = 10.0;
    let view = IndexedNetwork::new(&net);
    let problem = build_ac_opf_instance(&view, &AcOpfOptions::default()).expect("build");

    assert_close(problem.branches.tap[0], 1.25);
    assert_close(problem.branches.shift[0], 10.0_f64.to_radians());
    let z_squared = 0.05 * 0.05 + 0.2 * 0.2;
    assert_close(problem.branches.g[0], 0.05 / z_squared);
    assert_close(problem.branches.b[0], -0.2 / z_squared);
}

#[test]
fn terminal_charging_split_and_asymmetric() {
    let mut symmetric = small_network();
    symmetric.branches[0].b = 0.10;
    let problem = build_ac_opf_instance(&IndexedNetwork::new(&symmetric), &AcOpfOptions::default())
        .expect("symmetric");
    assert_close(problem.branches.b_fr[0], 0.05);
    assert_close(problem.branches.b_to[0], 0.05);
    assert_close(problem.branches.g_fr[0], 0.0);
    assert_close(problem.branches.g_to[0], 0.0);

    let mut asymmetric = small_network();
    asymmetric.branches[0].charging = Some(BranchCharging::new(0.001, 0.03, 0.002, 0.07));
    let problem =
        build_ac_opf_instance(&IndexedNetwork::new(&asymmetric), &AcOpfOptions::default())
            .expect("asymmetric");
    assert_close(problem.branches.g_fr[0], 0.001);
    assert_close(problem.branches.b_fr[0], 0.03);
    assert_close(problem.branches.g_to[0], 0.002);
    assert_close(problem.branches.b_to[0], 0.07);
}

#[test]
fn per_unit_and_native_units_scale_consistently() {
    let mut net = small_network();
    net.branches[0].rate_a = 250.0;
    net.loads.push(powerio::Load::new(BusId(30), 90.0, 30.0));
    net.shunts.push(powerio::Shunt::new(BusId(30), 2.0, 19.0));
    let view = IndexedNetwork::new(&net);
    let native = build_ac_opf_instance(
        &view,
        &AcOpfOptions {
            units: Units::Native,
            ..AcOpfOptions::default()
        },
    )
    .expect("native");
    let per_unit = build_ac_opf_instance(&view, &AcOpfOptions::default()).expect("per unit");
    let base = view.base_mva();

    assert_eq!(native.units, Units::Native);
    assert_eq!(per_unit.units, Units::PerUnit);
    for (native_vector, per_unit_vector) in [
        (&native.buses.p_d, &per_unit.buses.p_d),
        (&native.buses.q_d, &per_unit.buses.q_d),
        (&native.buses.g_s, &per_unit.buses.g_s),
        (&native.buses.b_s, &per_unit.buses.b_s),
        (&native.generators.pmax, &per_unit.generators.pmax),
        (&native.generators.qmin, &per_unit.generators.qmin),
        (&native.branches.g, &per_unit.branches.g),
        (&native.branches.b, &per_unit.branches.b),
        (&native.branches.b_fr, &per_unit.branches.b_fr),
        (&native.branches.s_max, &per_unit.branches.s_max),
    ] {
        for (native_value, per_unit_value) in native_vector.iter().zip(per_unit_vector) {
            assert_close(*native_value, per_unit_value * base);
        }
    }
    assert_close(
        per_unit.generators.q[0],
        native.generators.q[0] * base * base,
    );
    assert_close(per_unit.generators.c[0], native.generators.c[0] * base);
    assert_close(per_unit.generators.c0[0], native.generators.c0[0]);
    assert_close(per_unit.buses.vm_min[0], native.buses.vm_min[0]);
    assert_close(per_unit.buses.vm_max[0], native.buses.vm_max[0]);
}

#[test]
fn cost_constant_term_is_kept() {
    let net = small_network();
    let problem =
        build_ac_opf_instance(&IndexedNetwork::new(&net), &AcOpfOptions::default()).expect("build");
    assert_close(problem.generators.c0[0], 5.0);
}

#[test]
fn missing_and_unsupported_costs_are_distinct() {
    let mut missing = small_network();
    missing.generators[0].cost = None;
    let error = build_ac_opf_instance(&IndexedNetwork::new(&missing), &AcOpfOptions::default())
        .expect_err("missing cost");
    assert!(matches!(error, Error::MissingGenCost { gen_index: 0 }));

    let mut piecewise = small_network();
    piecewise.generators[0].cost = Some(GenCost::with_ncost(
        1,
        0.0,
        0.0,
        2,
        vec![0.0, 0.0, 1.0, 1.0],
    ));
    let error = build_ac_opf_instance(&IndexedNetwork::new(&piecewise), &AcOpfOptions::default())
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
fn zero_impedance_skip_or_reject() {
    let mut net = small_network();
    net.branches.insert(0, branch(10, 30, 0.0, 0.0));
    let view = IndexedNetwork::new(&net);
    let skipped = build_ac_opf_instance(&view, &AcOpfOptions::default()).expect("skip");
    assert_eq!(skipped.branches.skipped_zero_impedance, vec![0]);
    assert_eq!(skipped.branches.source_rows, vec![1]);

    let error = build_ac_opf_instance(
        &view,
        &AcOpfOptions {
            skip_zero_impedance: false,
            ..AcOpfOptions::default()
        },
    )
    .expect_err("reject");
    assert!(matches!(error, Error::ZeroImpedance { row: 0 }));

    // Zero resistance with nonzero reactance is a valid series element.
    let mut inductive = small_network();
    inductive.branches[0].r = 0.0;
    let problem = build_ac_opf_instance(&IndexedNetwork::new(&inductive), &AcOpfOptions::default())
        .expect("inductive");
    assert_eq!(problem.branches.skipped_zero_impedance, Vec::<usize>::new());
    assert_close(problem.branches.g[0], 0.0);
    assert_close(problem.branches.b[0], -1.0 / 0.2);
}

#[test]
fn out_of_service_exclusion() {
    let mut net = case9();
    net.generators[1].in_service = false;
    net.branches[2].in_service = false;
    let view = IndexedNetwork::new(&net);
    let problem = build_ac_opf_instance(&view, &AcOpfOptions::default()).expect("build");

    assert_eq!(problem.n_generators(), 2);
    assert!(!problem.generators.source_rows.contains(&1));
    assert!(!problem.branches.source_rows.contains(&2));
    assert_eq!(problem.bus_ids[0], view.bus_id(0));
}

#[test]
fn vm_setpoints_follow_generator_voltage() {
    let mut net = small_network();
    net.buses[0].vm = 0.0;
    net.buses[1].vm = 1.02;
    net.generators[0].vg = 1.05;
    let problem =
        build_ac_opf_instance(&IndexedNetwork::new(&net), &AcOpfOptions::default()).expect("build");
    let setpoints = problem.vm_setpoints();
    assert_close(setpoints[0], 1.05);
    assert_close(setpoints[1], 1.02);

    // An out of band setpoint is carried unclamped; feasibility repair is
    // solver preparation.
    let mut wide = small_network();
    wide.generators[0].vg = 1.5;
    let problem = build_ac_opf_instance(&IndexedNetwork::new(&wide), &AcOpfOptions::default())
        .expect("build");
    assert_close(problem.vm_setpoints()[0], 1.5);

    // A generator without a setpoint leaves the case voltage in place.
    let mut unset = small_network();
    unset.buses[0].vm = 1.01;
    unset.generators[0].vg = 0.0;
    let problem = build_ac_opf_instance(&IndexedNetwork::new(&unset), &AcOpfOptions::default())
        .expect("build");
    assert_close(problem.vm_setpoints()[0], 1.01);
}

#[test]
fn normalized_network_builds_identical_instance() {
    let net = case14();
    let normalized = net.to_normalized().expect("normalize");
    let raw =
        build_ac_opf_instance(&IndexedNetwork::new(&net), &AcOpfOptions::default()).expect("raw");
    let derived =
        build_ac_opf_instance(&IndexedNetwork::new(&normalized), &AcOpfOptions::default())
            .expect("normalized");

    assert_eq!(raw.n_buses, derived.n_buses);
    assert_eq!(raw.branches.source_rows, derived.branches.source_rows);
    for (left, right) in [
        (&raw.buses.p_d, &derived.buses.p_d),
        (&raw.buses.b_s, &derived.buses.b_s),
        (&raw.branches.g, &derived.branches.g),
        (&raw.branches.b, &derived.branches.b),
        (&raw.branches.b_fr, &derived.branches.b_fr),
        (&raw.branches.shift, &derived.branches.shift),
        (&raw.branches.s_max, &derived.branches.s_max),
        (&raw.generators.q, &derived.generators.q),
        (&raw.generators.c, &derived.generators.c),
        (&raw.generators.pmax, &derived.generators.pmax),
    ] {
        for (raw_value, derived_value) in left.iter().zip(right) {
            assert!(
                (raw_value - derived_value).abs() < 1e-9,
                "{raw_value} != {derived_value}"
            );
        }
    }
}

#[test]
fn serde_round_trip() {
    let net = case9();
    let view = IndexedNetwork::new(&net);
    let problem = build_ac_opf_instance(&view, &AcOpfOptions::default()).expect("build");
    let json = serde_json::to_string(&problem).expect("serialize");
    let back: powerio_prob::AcOpfInstance = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.name, problem.name);
    assert_eq!(back.bus_ids, problem.bus_ids);
    assert_eq!(back.generators.source_rows, problem.generators.source_rows);
    assert_eq!(back.branches.source_rows, problem.branches.source_rows);
    for (left, right) in back.branches.b.iter().zip(&problem.branches.b) {
        assert!((left - right).abs() < 1e-12);
    }
}

#[cfg(feature = "matrix")]
mod matrix_tests {
    use num_complex::Complex64;

    use super::*;

    /// Assemble a dense Y_bus from the instance with the standard pi model
    /// stamp and compare it against the generic `build_ybus` on the same
    /// network. This is the same algebra an AC consumer implements from the
    /// carried fields.
    #[test]
    fn ybus_entrywise_cross_check() {
        let mut net = case14();
        // A self-loop with tap, shift, and charging: the instance folds it
        // into the bus shunt vectors, `build_ybus` stamps it directly; the
        // two must agree entrywise.
        let mut self_loop = branch(5, 5, 0.02, 0.08);
        self_loop.tap = 1.1;
        self_loop.shift = 15.0;
        self_loop.b = 0.04;
        net.branches.push(self_loop);
        let view = IndexedNetwork::new(&net);
        let problem = build_ac_opf_instance(&view, &AcOpfOptions::default()).expect("build");
        let n = problem.n_buses;

        let mut dense = vec![vec![Complex64::new(0.0, 0.0); n]; n];
        for e in 0..problem.n_branches() {
            let from = problem.branches.from_bus[e];
            let to = problem.branches.to_bus[e];
            let series = Complex64::new(problem.branches.g[e], problem.branches.b[e]);
            let charging_from = Complex64::new(problem.branches.g_fr[e], problem.branches.b_fr[e]);
            let charging_to = Complex64::new(problem.branches.g_to[e], problem.branches.b_to[e]);
            let tap = Complex64::from_polar(problem.branches.tap[e], problem.branches.shift[e]);
            let tap_squared = problem.branches.tap[e] * problem.branches.tap[e];

            dense[from][from] += (series + charging_from) / tap_squared;
            dense[from][to] += -series / tap.conj();
            dense[to][from] += -series / tap;
            dense[to][to] += series + charging_to;
        }
        for (index, dense_row) in dense.iter_mut().enumerate() {
            dense_row[index] += Complex64::new(problem.buses.g_s[index], problem.buses.b_s[index]);
        }

        let ybus = powerio_matrix::build_ybus(&view, &powerio_matrix::BuildOptions::default())
            .expect("ybus");
        for (row, dense_row) in dense.iter().enumerate() {
            for (column, expected) in dense_row.iter().enumerate() {
                let g_entry = ybus.g.get(row, column).copied().unwrap_or(0.0);
                let b_entry = ybus.b.get(row, column).copied().unwrap_or(0.0);
                assert!(
                    (g_entry - expected.re).abs() < 1e-9,
                    "G[{row}][{column}]: {g_entry} != {}",
                    expected.re
                );
                assert!(
                    (b_entry - expected.im).abs() < 1e-9,
                    "B[{row}][{column}]: {b_entry} != {}",
                    expected.im
                );
            }
        }
    }
}

/// A self-loop's pi model stamp folds onto the bus diagonal, matching the
/// Y_bus builder, and a zero base MVA is rejected before scaling.
#[test]
fn self_loop_folds_into_bus_shunt() {
    let mut net = small_network();
    let mut loop_branch = branch(30, 30, 0.05, 0.2);
    loop_branch.tap = 1.25;
    loop_branch.shift = 10.0;
    loop_branch.b = 0.10;
    net.branches.push(loop_branch);
    let plain = build_ac_opf_instance(
        &IndexedNetwork::new(&small_network()),
        &AcOpfOptions::default(),
    )
    .expect("plain");
    let folded = build_ac_opf_instance(&IndexedNetwork::new(&net), &AcOpfOptions::default())
        .expect("folded");

    // The self-loop is not a flow element and is not a skip.
    assert_eq!(folded.n_branches(), plain.n_branches());
    assert_eq!(folded.branches.skipped_zero_impedance, Vec::<usize>::new());

    let z_squared = 0.05 * 0.05 + 0.2 * 0.2;
    let (series_g, series_b) = (0.05 / z_squared, -0.2 / z_squared);
    let (tap, shift) = (1.25_f64, 10.0_f64.to_radians());
    let cross = 2.0 * shift.cos() / tap;
    let g_add = (series_g / (tap * tap)) + series_g - series_g * cross;
    let b_add = (0.05 + series_b) / (tap * tap) + (series_b + 0.05) - series_b * cross;
    assert_close(folded.buses.g_s[1], plain.buses.g_s[1] + g_add);
    assert_close(folded.buses.b_s[1], plain.buses.b_s[1] + b_add);
}

#[test]
fn zero_base_mva_is_rejected() {
    let mut net = small_network();
    net.base_mva = 0.0;
    let error = build_ac_opf_instance(&IndexedNetwork::new(&net), &AcOpfOptions::default())
        .expect_err("zero base");
    assert!(matches!(error, Error::InvalidBaseMva { .. }));
}
