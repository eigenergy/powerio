use powerio::{
    BranchSusceptanceFormula, ConductorMatrix, Destination, EmittedOutput, PioModule, PioValue,
    Source, deserialize, emit, parse, serialize,
};
use powerio_matrix::DcOperators;

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "expected {expected:.16e}, got {actual:.16e}"
    );
}

fn assert_matrix_close(actual: &[Vec<f64>], expected: &[Vec<f64>]) {
    assert_eq!(actual.len(), expected.len());
    for (actual_row, expected_row) in actual.iter().zip(expected) {
        assert_eq!(actual_row.len(), expected_row.len());
        for (&actual, &expected) in actual_row.iter().zip(expected_row) {
            assert_close(actual, expected);
        }
    }
}

fn dense(matrix: &powerio_matrix::SparseMatrix) -> Vec<Vec<f64>> {
    let mut out = vec![vec![0.0; matrix.cols()]; matrix.rows()];
    for (row, values) in matrix.outer_iterator().enumerate() {
        for (column, &value) in values.iter() {
            out[row][column] += value;
        }
    }
    out
}

fn conformance_path() -> &'static str {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/api_conformance.m"
    )
}

fn bmopf_path() -> &'static str {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/dist/bmopf/example_ieee13.json"
    )
}

fn assert_settled_calculation_names_are_exported() {
    fn names<T>() {}
    names::<powerio::DcPfInstance>();
    names::<powerio::AcPfInstance>();
    names::<powerio::DcOpfInstance>();
    names::<powerio::AcOpfInstance>();
    names::<powerio::McAcPfInstance>();
    names::<powerio::McAcOpfInstance>();
    names::<powerio::AcScucInstance>();
    names::<powerio::DcPfSolution>();
    names::<powerio::AcPfSolution>();
    names::<powerio::DcOpfSolution>();
    names::<powerio::AcOpfSolution>();
    names::<powerio::McAcPfSolution>();
    names::<powerio::McAcOpfSolution>();
    names::<powerio::AcScucSolution>();
}

#[test]
fn facade_uses_power_system_names_and_universal_emission() {
    assert_settled_calculation_names_are_exported();
    let formula: BranchSusceptanceFormula = BranchSusceptanceFormula::SeriesSusceptance;
    assert_eq!(formula, BranchSusceptanceFormula::SeriesSusceptance);
    let matrix: ConductorMatrix = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    assert_eq!(matrix.len(), 2);

    let module = parse(Source::open(conformance_path()).unwrap()).unwrap();
    assert!(matches!(&module.value(), PioValue::BalancedNetwork(_)));

    let written = emit(
        &module,
        "matpower",
        Destination::memory("api_conformance.m").unwrap(),
    )
    .unwrap();
    let EmittedOutput::Memory { artifacts } = written.output() else {
        panic!("memory destination returned a path output");
    };
    assert_eq!(artifacts.len(), 1);
    assert!(
        artifacts[0]
            .bytes()
            .starts_with(b"function mpc = api_conformance")
    );
    assert!(written.diagnostics().is_empty());
}

#[test]
fn typed_modules_emit_and_serialize_directly() {
    let parsed = parse(Source::open(conformance_path()).unwrap()).unwrap();
    let PioValue::BalancedNetwork(network) = parsed.value() else {
        panic!("MATPOWER input did not produce a balanced network");
    };

    let network_module = PioModule::new(network.clone());
    let emitted = emit(
        &network_module,
        "matpower",
        Destination::memory("typed.m").unwrap(),
    )
    .unwrap();
    let EmittedOutput::Memory { artifacts } = emitted.output() else {
        panic!("memory destination returned a path output");
    };
    assert!(artifacts[0].bytes().starts_with(b"function mpc"));

    let instance = powerio::DcPfInstance::from_network(network.clone()).unwrap();
    let instance_module = PioModule::new(instance);
    let stored = serialize(
        &instance_module,
        Destination::memory("typed.pio.json").unwrap(),
    )
    .unwrap();
    let EmittedOutput::Memory { artifacts } = stored.output() else {
        panic!("memory destination returned a path output");
    };
    let source = Source::from_memory("typed.pio.json", artifacts[0].bytes().to_vec()).unwrap();
    let decoded = deserialize(source).unwrap();
    assert!(matches!(decoded.value(), PioValue::DcPfInstance(_)));
}

#[test]
fn bmopf_parses_to_a_network_and_calculation_construction_is_explicit() {
    let module = parse(Source::open(bmopf_path()).unwrap()).unwrap();
    let PioValue::MulticonductorNetwork(network) = &module.value() else {
        panic!("BMOPF input did not produce a multiconductor network");
    };
    assert!(!network.buses().is_empty());
    assert!(!network.sources().is_empty());

    let instance = powerio::to_mc_ac_opf_instance(&module).unwrap();
    assert_eq!(
        instance.value().network().buses().len(),
        network.buses().len()
    );
    assert!(instance.source().is_none());
    assert_eq!(
        instance.history().last().map(powerio::HistoryEntry::name),
        Some("to_mc_ac_opf_instance")
    );
}

#[test]
fn shared_case_conforms_to_the_named_dc_operations() {
    let module = parse(Source::open(conformance_path()).unwrap()).unwrap();
    let PioValue::BalancedNetwork(network) = &module.value() else {
        panic!("MATPOWER input did not produce a balanced network");
    };
    assert_eq!(network.generators().len(), 2);

    let instance = powerio::DcPfInstance::from_network(network.clone())
        .unwrap()
        .with_branch_susceptance_formula(BranchSusceptanceFormula::SeriesSusceptance);
    let operators = DcOperators::build(&instance).unwrap();
    assert_eq!(
        operators.branch_susceptance_formula(),
        BranchSusceptanceFormula::SeriesSusceptance
    );
    assert_eq!(
        operators
            .bus_ids()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["1", "2", "3"]
    );
    assert_eq!(operators.branch_identities(), ["1-2", "1-3"]);

    let n_branches = operators.branch_identities().len();
    let n_buses = operators.bus_ids().len();
    let a = dense(&operators.calc_incidence_matrix());
    assert_eq!(a, [vec![1.0, -1.0, 0.0], vec![1.0, 0.0, -1.0]]);

    let expected_b = [
        -0.1 / (0.01 * 0.01 + 0.1 * 0.1),
        -0.2 / (0.02 * 0.02 + 0.2 * 0.2),
    ];
    for (&actual, &expected) in operators.calc_branch_susceptances().iter().zip(&expected_b) {
        assert_close(actual, expected);
    }

    // B = A' * Diagonal(b) * A and Bf = Diagonal(b) * A.
    let b_matrix = dense(&operators.calc_bus_susceptance_matrix());
    let bf = dense(&operators.calc_branch_flow_matrix());
    let expected_b_matrix = [
        vec![
            expected_b[0] + expected_b[1],
            -expected_b[0],
            -expected_b[1],
        ],
        vec![-expected_b[0], expected_b[0], 0.0],
        vec![-expected_b[1], 0.0, expected_b[1]],
    ];
    let expected_bf = [
        vec![expected_b[0], -expected_b[0], 0.0],
        vec![expected_b[1], 0.0, -expected_b[1]],
    ];
    assert_matrix_close(&b_matrix, &expected_b_matrix);
    assert_matrix_close(&bf, &expected_bf);

    // p_shift = A' * (b .* shift).
    let shift_injection = operators.calc_bus_phase_shift_injection();
    let shift = 10.0_f64.to_radians();
    assert_close(shift_injection[0], expected_b[1] * shift);
    assert_close(shift_injection[1], 0.0);
    assert_close(shift_injection[2], -expected_b[1] * shift);

    // p_branch = -Bf * va + b .* shift, and A' * p_branch = -B * va + p_shift.
    let va = [0.03, 0.01, -0.02];
    let branch_flow = operators.calc_branch_flow_dc(&va).unwrap();
    assert_close(branch_flow[0], -expected_b[0] * (va[0] - va[1]));
    assert_close(
        branch_flow[1],
        -expected_b[1] * (va[0] - va[2]) + expected_b[1] * shift,
    );
    for bus in 0..n_buses {
        let at_flow = (0..n_branches)
            .map(|row| a[row][bus] * branch_flow[row])
            .sum::<f64>();
        let rhs = -b_matrix[bus]
            .iter()
            .zip(va)
            .map(|(&coefficient, angle)| coefficient * angle)
            .sum::<f64>()
            + shift_injection[bus];
        assert_close(at_flow, rhs);
    }
}
