use powerio::{
    Branch, Bus, BusId, BusType, DcConvention, Error, GenCost, Generator, IndexedNetwork, Network,
    parse_matpower_file,
};
use powerio_prob::{DcOpfOptions, Units, build_dc_opf_instance};

fn case9() -> Network {
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
    generator.cost = Some(GenCost::new(2, 0.0, 0.0, vec![c2, c1, 0.0]));
    generator
}

fn small_network() -> Network {
    let mut network = Network::in_memory(
        "small",
        100.0,
        vec![bus(10, BusType::Ref), bus(30, BusType::Pq)],
        vec![branch(10, 30, 0.2)],
    );
    network.generators.push(generator(10, 1.0, 2.0));
    network
}

#[test]
fn instance_is_complete_and_indexed() {
    let net = case9();
    let view = IndexedNetwork::new(&net);
    let problem = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("build");

    assert_eq!(problem.name, "case9");
    assert_eq!(problem.n_buses, 9);
    assert_eq!(problem.n_source_generators, net.generators.len());
    assert_eq!(problem.n_source_branches, net.branches.len());
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
fn several_generators_at_one_bus_keep_separate_costs() {
    let mut net = case9();
    let mut extra = net.generators[0].clone();
    extra.uid = Some("extra-generator".to_owned());
    extra.cost = Some(GenCost::new(2, 0.0, 0.0, vec![7.0, 3.0, 1.0]));
    net.generators.push(extra);

    let view = IndexedNetwork::new(&net);
    let problem = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("build");
    assert_eq!(problem.n_generators(), 4);
    assert_eq!(
        problem.generators.bus_of_gen[0],
        problem.generators.bus_of_gen[3]
    );
    assert!((problem.generators.q[0] - problem.generators.q[3]).abs() > 1e-12);
    assert!((problem.generators.c[0] - problem.generators.c[3]).abs() > 1e-12);
    assert!(matches!(
        problem.nodal_generator_data(),
        Err(Error::MultipleGeneratorsAtBus { .. })
    ));
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
    assert_close(native.branches.b[0], per_unit.branches.b[0] * base);
    assert_close(native.branches.f_max[0], per_unit.branches.f_max[0] * base);
}

#[test]
fn matpower_convention_applies_tap_and_phase_shift() {
    let mut net = small_network();
    net.branches[0].tap = 1.25;
    net.branches[0].shift = 10.0;
    let view = IndexedNetwork::new(&net);
    let paper = build_dc_opf_instance(&view, &DcOpfOptions::default()).expect("paper");
    let matpower = build_dc_opf_instance(
        &view,
        &DcOpfOptions {
            convention: DcConvention::Matpower,
            ..DcOpfOptions::default()
        },
    )
    .expect("matpower");

    assert_close(paper.branches.b[0], 1.0 / 0.2);
    assert_close(paper.branches.shift[0], 0.0);
    let expected_b = 1.0 / (0.2 * 1.25);
    let expected_shift = 10.0_f64.to_radians();
    assert!((matpower.branches.b[0] - expected_b).abs() < 1e-12);
    assert!((matpower.branches.shift[0] - expected_shift).abs() < 1e-12);
    assert!((matpower.p_shift[0] + expected_b * expected_shift).abs() < 1e-12);
    assert!((matpower.p_shift[1] - expected_b * expected_shift).abs() < 1e-12);
}

#[test]
fn source_maps_exclude_out_of_service_elements() {
    let mut net = case9();
    net.generators[1].in_service = false;
    net.branches[2].in_service = false;
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
    missing.generators[0].cost = None;
    let error = build_dc_opf_instance(&IndexedNetwork::new(&missing), &DcOpfOptions::default())
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
    net.branches.insert(0, branch(10, 30, 0.0));
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
    assert!(matches!(error, Error::ZeroImpedance { row: 0 }));
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
    for (left, right) in back.branches.b.iter().zip(&problem.branches.b) {
        assert!((left - right).abs() < 1e-12);
    }
}

#[cfg(feature = "matrix")]
mod matrix_tests {
    use powerio::{GenCostPolicyReport, MissingGenCostPolicy};
    use powerio_prob::matrix::{
        DcOpfBundleMetadata, DcOpfBundleOptions, build_dc_opf_matrices, write_dcopf_bundle,
    };

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
    fn bundle_uses_instance_data_and_records_metadata() {
        let net = parse_matpower_file("../tests/data/case14.m").expect("parse case14");
        let problem = build_dc_opf_instance(&IndexedNetwork::new(&net), &DcOpfOptions::default())
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
        assert_eq!(manifest["schema_version"], "0.2.0");
        assert_eq!(manifest["dimensions"]["n_buses"], problem.n_buses);
        assert_eq!(
            manifest["dimensions"]["n_generators"],
            problem.n_generators()
        );
        assert_eq!(manifest["patched_gen_costs"], 1);
        assert_eq!(manifest["cost_policy"]["mode"], "require");
    }
}
