//! The #232 acceptance behaviors: hand checked feeder stamps for series,
//! shunt, capacitor, transformer, switch, source, and grounding; exclusion of
//! grounded terminals from the unknowns; exact unity merges with no
//! fabricated impedance; structured diagnostics for unsupported stamps; and
//! axis mappings that survive source row reordering. Every value is per
//! conductor in actual units — no positive sequence transformation exists
//! anywhere on this surface.
// Hand checks index square arrays by both loop variables on purpose.
#![allow(clippy::needless_range_loop)]

use powerio_dist::{
    Configuration, DistBus, DistCapacitor, DistLine, DistLineCode, DistShunt, DistSwitch,
    DistTransformer, DistWinding, DistWindingConn, MulticonductorNetwork, VoltageSource,
};
use powerio_matrix::{
    NodeRef, PassiveFamily, TransformerLeakageBase, calc_multiconductor_admittance_matrix,
    calc_passive_primitives, calc_transformer_leakage_impedances,
    calc_transformer_primitive_admittance, calc_transformer_winding_admittance,
};

fn dense(matrix: &powerio_matrix::SparseMatrix) -> Vec<Vec<f64>> {
    let mut out = vec![vec![0.0; matrix.cols()]; matrix.rows()];
    for (row, row_vec) in matrix.outer_iterator().enumerate() {
        for (column, &value) in row_vec.iter() {
            out[row][column] += value;
        }
    }
    out
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn grounded_terminals_and_terminal_zero_are_not_unknowns() {
    let mut net = MulticonductorNetwork::default();
    let mut a = DistBus::new("a", strings(&["1", "2", "0"]));
    a.grounded = strings(&["2"]);
    net.buses_mut().push(a);
    net.buses_mut().push(DistBus::new("b", strings(&["1"])));

    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let index = system.index();
    assert_eq!(index.len(), 2, "a.1 and b.1 only");
    assert_eq!(index.resolve("a", "1"), Some(NodeRef::Node(0)));
    assert_eq!(index.resolve("a", "2"), Some(NodeRef::Ground));
    assert_eq!(index.resolve("a", "0"), Some(NodeRef::Ground));
    assert_eq!(index.resolve("b", "1"), Some(NodeRef::Node(1)));
    assert_eq!(index.resolve("b", "9"), None);
}

#[test]
fn a_single_conductor_line_stamps_its_hand_inverted_admittance() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut().push(DistBus::new("a", strings(&["1"])));
    net.buses_mut().push(DistBus::new("b", strings(&["1"])));
    // z = (1 + 2j) ohm/m over 10 m: Y = 1/(10 + 20j) = (10 - 20j)/500
    //   = 0.02 - 0.04j siemens.
    net.line_codes_mut()
        .push(DistLineCode::new("lc", vec![vec![1.0]], vec![vec![2.0]]));
    net.lines_mut().push(DistLine::new(
        "l1",
        "a",
        "b",
        strings(&["1"]),
        strings(&["1"]),
        "lc",
        10.0,
    ));

    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let g = dense(system.conductance());
    let b = dense(system.susceptance());
    for (matrix, diagonal, off) in [(&g, 0.02, -0.02), (&b, -0.04, 0.04)] {
        assert!((matrix[0][0] - diagonal).abs() < 1e-12);
        assert!((matrix[1][1] - diagonal).abs() < 1e-12);
        assert!((matrix[0][1] - off).abs() < 1e-12);
        assert!((matrix[1][0] - off).abs() < 1e-12);
    }
}

#[test]
fn a_two_conductor_line_matches_the_hand_inverse_and_shunt_halves() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut()
        .push(DistBus::new("a", strings(&["1", "2"])));
    net.buses_mut()
        .push(DistBus::new("b", strings(&["1", "2"])));
    // Z per meter: diagonal 2j, mutual 1j; over 1 m, Z = [[2j, 1j], [1j, 2j]].
    // inv(Z) = 1/j * inv([[2,1],[1,2]]) = -j * (1/3)[[2,-1],[-1,2]].
    let mut code = DistLineCode::new(
        "lc2",
        vec![vec![0.0, 0.0], vec![0.0, 0.0]],
        vec![vec![2.0, 1.0], vec![1.0, 2.0]],
    );
    // A from-end shunt half: b_from = diag(0.5) S/m.
    code.b_from = vec![vec![0.5, 0.0], vec![0.0, 0.5]];
    net.line_codes_mut().push(code);
    net.lines_mut().push(DistLine::new(
        "l1",
        "a",
        "b",
        strings(&["1", "2"]),
        strings(&["1", "2"]),
        "lc2",
        1.0,
    ));

    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let b = dense(system.susceptance());
    // Node order: a.1, a.2, b.1, b.2.
    let y11 = -2.0 / 3.0;
    let y12 = 1.0 / 3.0;
    // Series block at (a, a) plus the from-end shunt half on the diagonal.
    assert!((b[0][0] - (y11 + 0.5)).abs() < 1e-12);
    assert!((b[0][1] - y12).abs() < 1e-12);
    // Series block at (b, b): no shunt half was stated at the to end.
    assert!((b[2][2] - y11).abs() < 1e-12);
    assert!((b[2][3] - y12).abs() < 1e-12);
    // Coupling block carries the negated inverse.
    assert!((b[0][2] + y11).abs() < 1e-12);
    assert!((b[0][3] + y12).abs() < 1e-12);
    // The conductance matrix is untouched by a purely reactive line.
    let g = dense(system.conductance());
    assert!(g.iter().flatten().all(|value| value.abs() < 1e-12));
}

#[test]
fn a_closed_switch_merges_nodes_and_an_open_one_does_not() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut().push(DistBus::new("a", strings(&["1"])));
    net.buses_mut().push(DistBus::new("b", strings(&["1"])));
    net.buses_mut().push(DistBus::new("c", strings(&["1"])));
    let closed = DistSwitch::new(
        "s-closed",
        "a",
        "b",
        strings(&["1"]),
        strings(&["1"]),
        false,
    );
    let open = DistSwitch::new("s-open", "b", "c", strings(&["1"]), strings(&["1"]), true);
    net.switches_mut().push(closed);
    net.switches_mut().push(open);

    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let index = system.index();
    // a.1 and b.1 are one exact node; c.1 is its own.
    assert_eq!(index.len(), 2);
    assert_eq!(index.resolve("a", "1"), index.resolve("b", "1"));
    assert_ne!(index.resolve("b", "1"), index.resolve("c", "1"));
    // No fabricated impedance entered the passive matrix.
    assert_eq!(system.conductance().nnz(), 0);
    assert_eq!(system.susceptance().nnz(), 0);
}

#[test]
fn a_wye_capacitor_stamps_its_nameplate_susceptance() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut()
        .push(DistBus::new("a", strings(&["1", "2", "3", "0"])));
    // 300 var at 1000 V line to line, wye over three phases and the grounded
    // neutral: per phase b = 100 / (1000/sqrt(3))^2 = 3e-4 S.
    net.capacitors_mut().push(DistCapacitor::new(
        "cap",
        "a",
        strings(&["1", "2", "3", "0"]),
        Configuration::Wye,
        300.0,
        1000.0,
    ));

    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let b = dense(system.susceptance());
    let per_phase = 100.0 / (1000.0 / 3f64.sqrt()).powi(2);
    for phase in 0..3 {
        assert!(
            (b[phase][phase] - per_phase).abs() < 1e-15,
            "phase {phase}: {}",
            b[phase][phase]
        );
    }
}

#[test]
fn delta_capacitor_rating_is_divided_by_physical_coils_not_terminals() {
    // An open-delta bank with two terminals is one phase-to-phase coil:
    // 100 var / (1000 V)^2 = 1e-4 S, not half that value.
    let mut open_delta = MulticonductorNetwork::default();
    open_delta
        .buses_mut()
        .push(DistBus::new("a", strings(&["1", "2"])));
    open_delta.capacitors_mut().push(DistCapacitor::new(
        "open-delta",
        "a",
        strings(&["1", "2"]),
        Configuration::Delta,
        100.0,
        1000.0,
    ));

    let system = calc_multiconductor_admittance_matrix(&open_delta).unwrap();
    let b = dense(system.susceptance());
    let coil_b = 100.0 / 1000.0_f64.powi(2);
    assert!((b[0][0] - coil_b).abs() < 1e-15);
    assert!((b[1][1] - coil_b).abs() < 1e-15);
    assert!((b[0][1] + coil_b).abs() < 1e-15);
    assert!((b[1][0] + coil_b).abs() < 1e-15);

    // A three-terminal delta still shares the total bank rating across
    // three physical coils, preserving the balanced-bank convention.
    let mut full_delta = MulticonductorNetwork::default();
    full_delta
        .buses_mut()
        .push(DistBus::new("b", strings(&["1", "2", "3"])));
    full_delta.capacitors_mut().push(DistCapacitor::new(
        "full-delta",
        "b",
        strings(&["1", "2", "3"]),
        Configuration::Delta,
        300.0,
        1000.0,
    ));
    let system = calc_multiconductor_admittance_matrix(&full_delta).unwrap();
    let b = dense(system.susceptance());
    let coil_b = 100.0 / 1000.0_f64.powi(2);
    for phase in 0..3 {
        assert!((b[phase][phase] - 2.0 * coil_b).abs() < 1e-15);
        for other in 0..3 {
            if phase != other {
                assert!((b[phase][other] + coil_b).abs() < 1e-15);
            }
        }
    }
}

#[test]
fn shunt_matrices_stamp_verbatim() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut()
        .push(DistBus::new("a", strings(&["1", "2"])));
    net.shunts_mut().push(DistShunt::new(
        "sh",
        "a",
        strings(&["1", "2"]),
        vec![vec![1.0, -0.25], vec![-0.25, 2.0]],
        vec![vec![0.5, 0.0], vec![0.0, 0.75]],
    ));
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let g = dense(system.conductance());
    let b = dense(system.susceptance());
    assert!((g[0][0] - 1.0).abs() < 1e-15);
    assert!((g[0][1] + 0.25).abs() < 1e-15);
    assert!((g[1][1] - 2.0).abs() < 1e-15);
    assert!((b[0][0] - 0.5).abs() < 1e-15);
    assert!((b[1][1] - 0.75).abs() < 1e-15);
}

#[test]
fn sources_and_ideal_transformers_enter_the_augmented_system() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut().push(DistBus::new("src", strings(&["1"])));
    net.buses_mut().push(DistBus::new("p", strings(&["1"])));
    net.buses_mut().push(DistBus::new("s", strings(&["1"])));
    let source = VoltageSource::new("vs", "src", strings(&["1"]), vec![7200.0], vec![0.0]);
    net.sources_mut().push(source);
    let primary = DistWinding::new("p", strings(&["1"]), DistWindingConn::Wye, 7200.0, 25_000.0);
    let secondary = DistWinding::new("s", strings(&["1"]), DistWindingConn::Wye, 240.0, 25_000.0);
    net.transformers_mut().push(DistTransformer::new(
        "t1",
        vec![primary, secondary],
        vec![0.0],
        1,
    ));

    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let index = system.index();
    let augmented = system.augmented();
    assert_eq!(augmented.labels.len(), 2, "one source row, one ratio row");

    // The source row pins v(src.1) to the stated complex voltage.
    let source_row = augmented
        .labels
        .iter()
        .position(|label| label.starts_with("source:vs"))
        .unwrap();
    let NodeRef::Node(src_node) = index.resolve("src", "1").unwrap() else {
        panic!("src.1 is an unknown")
    };
    let constraints = dense(&augmented.constraint_re);
    assert!((constraints[source_row][src_node] - 1.0).abs() < 1e-15);
    assert!((augmented.rhs_re[source_row] - 7200.0).abs() < 1e-9);
    assert!(augmented.rhs_im[source_row].abs() < 1e-9);

    // The transformer row states v_p - a v_s = 0 with a = 7200/240 = 30.
    let ratio_row = augmented
        .labels
        .iter()
        .position(|label| label.starts_with("transformer:t1"))
        .unwrap();
    let NodeRef::Node(p_node) = index.resolve("p", "1").unwrap() else {
        panic!("p.1 is an unknown")
    };
    let NodeRef::Node(s_node) = index.resolve("s", "1").unwrap() else {
        panic!("s.1 is an unknown")
    };
    assert!((constraints[ratio_row][p_node] - 1.0).abs() < 1e-15);
    assert!((constraints[ratio_row][s_node] + 30.0).abs() < 1e-12);
    assert!(augmented.rhs_re[ratio_row].abs() < 1e-15);

    assert_eq!(system.susceptance().nnz(), 0);
}

#[test]
fn unsupported_stamps_are_structured_diagnostics() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut().push(DistBus::new("a", strings(&["1"])));
    net.buses_mut().push(DistBus::new("b", strings(&["1"])));
    net.buses_mut().push(DistBus::new("c", strings(&["1"])));
    let w = |bus: &str| DistWinding::new(bus, strings(&["1"]), DistWindingConn::Wye, 100.0, 1000.0);
    net.transformers_mut().push(DistTransformer::new(
        "three",
        vec![w("a"), w("b"), w("c")],
        vec![1.0, 1.0, 1.0],
        1,
    ));
    // Leakage stated, but a three winding unit that is no BMOPF centre tap
    // has no primitive.
    let error = calc_multiconductor_admittance_matrix(&net).unwrap_err();
    assert_eq!(error.code().code, "BUILD.DIST.TRANSFORMER_UNSUPPORTED");
    assert!(error.to_string().contains("3 windings"), "{error}");
}

#[test]
fn axis_mappings_survive_source_row_reordering() {
    let build = |reversed: bool| {
        let mut net = MulticonductorNetwork::default();
        net.buses_mut().push(DistBus::new("a", strings(&["1"])));
        net.buses_mut().push(DistBus::new("b", strings(&["1"])));
        net.line_codes_mut()
            .push(DistLineCode::new("lc", vec![vec![1.0]], vec![vec![1.0]]));
        net.lines_mut().push(DistLine::new(
            "l1",
            "a",
            "b",
            strings(&["1"]),
            strings(&["1"]),
            "lc",
            1.0,
        ));
        if reversed {
            net.buses_mut().reverse();
        }
        calc_multiconductor_admittance_matrix(&net).unwrap()
    };
    let forward = build(false);
    let reversed = build(true);
    // The same (bus, terminal) resolves in both, and the stamped values read
    // identically through the mapping whatever the table order.
    for (bus, terminal) in [("a", "1"), ("b", "1")] {
        let NodeRef::Node(i) = forward.index().resolve(bus, terminal).unwrap() else {
            panic!()
        };
        let NodeRef::Node(j) = reversed.index().resolve(bus, terminal).unwrap() else {
            panic!()
        };
        let gf = dense(forward.conductance());
        let gr = dense(reversed.conductance());
        assert!((gf[i][i] - gr[j][j]).abs() < 1e-15);
    }
}

#[test]
fn a_parsed_micro_feeder_assembles_end_to_end() {
    let dss = "New Circuit.c basekv=12.47 pu=1 phases=3 bus1=a\n\
               New Line.l1 bus1=a.1.2.3 bus2=b.1.2.3 phases=3 r1=0.1 x1=0.2 length=1 units=km\n\
               New Load.ld bus1=b.1.2.3 phases=3 conn=wye kv=7.2 kw=30 kvar=9\n";
    let source = powerio_core::Source::from_memory("<memory>", dss.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("dss").unwrap());
    let net = powerio_dist::parse(source).unwrap().into_value();
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert!(system.index().len() >= 6, "three phases at two buses");
    assert!(system.susceptance().nnz() > 0);
    // The source anchors the system through the augmented rows.
    assert_ne!(system.augmented().labels, [] as [std::string::String; 0]);
}

#[test]
fn a_leakage_transformer_stamps_its_primitive() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut().push(DistBus::new("p", strings(&["1"])));
    net.buses_mut().push(DistBus::new("s", strings(&["1"])));
    net.transformers_mut().push(DistTransformer::new(
        "t",
        vec![
            DistWinding::new("p", strings(&["1"]), DistWindingConn::Wye, 7200.0, 25000.0),
            DistWinding::new("s", strings(&["1"]), DistWindingConn::Wye, 240.0, 25000.0),
        ],
        vec![2.0],
        1,
    ));
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    // A one terminal winding grounds its coil: y = S / (j xsc V²) at the
    // primary, scaled by the turns ratio across.
    let y = num_complex::Complex64::new(0.0, 0.02 * 7200.0 * 7200.0 / 25000.0).finv();
    let ratio = 7200.0 / 240.0;
    let g = dense(system.conductance());
    let b = dense(system.susceptance());
    let expected = [[y, -y * ratio], [-y * ratio, y * ratio * ratio]];
    for i in 0..2 {
        for j in 0..2 {
            let scale = expected[1][1].norm();
            assert!((g[i][j] - expected[i][j].re).abs() < 1e-12 * scale);
            assert!((b[i][j] - expected[i][j].im).abs() < 1e-12 * scale);
        }
    }
    assert_eq!(system.augmented().labels, [] as [String; 0]);
}

/// A source that already fixes a solidly grounded neutral at zero settles
/// it: a second `v = 0` row would leave the constraint rows rank deficient.
/// A nonzero source voltage on that node is a contradiction, refused.
#[test]
fn a_source_on_a_solid_neutral_is_not_constrained_twice() {
    let build = |neutral_volts: f64| {
        let mut net = MulticonductorNetwork::default();
        net.buses_mut()
            .push(DistBus::new("p", strings(&["1", "n"])));
        net.buses_mut()
            .push(DistBus::new("s", strings(&["1", "n"])));
        let mut primary = DistWinding::new(
            "p",
            strings(&["1", "n"]),
            DistWindingConn::Wye,
            7200.0,
            25000.0,
        );
        primary.r_pct = 0.5;
        primary.r_neutral = Some(0.0);
        primary.x_neutral = Some(0.0);
        let mut secondary = DistWinding::new(
            "s",
            strings(&["1", "n"]),
            DistWindingConn::Wye,
            240.0,
            25000.0,
        );
        secondary.r_pct = 0.5;
        net.transformers_mut().push(DistTransformer::new(
            "t",
            vec![primary, secondary],
            vec![2.0],
            1,
        ));
        net.sources_mut().push(VoltageSource::new(
            "vs",
            "p",
            strings(&["1", "n"]),
            vec![7200.0, neutral_volts],
            vec![0.0, 0.0],
        ));
        calc_multiconductor_admittance_matrix(&net)
    };
    let system = build(0.0).unwrap();
    assert_eq!(system.augmented().labels, ["source:vs:1", "source:vs:n"]);
    let error = build(5.0).unwrap_err();
    assert!(error.to_string().contains("grounds neutral"), "{error}");
}

#[test]
fn a_solidly_grounded_leakage_neutral_is_an_exact_row() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut()
        .push(DistBus::new("p", strings(&["1", "n"])));
    net.buses_mut()
        .push(DistBus::new("s", strings(&["1", "n"])));
    let mut primary = DistWinding::new(
        "p",
        strings(&["1", "n"]),
        DistWindingConn::Wye,
        7200.0,
        25000.0,
    );
    primary.r_pct = 0.5;
    primary.r_neutral = Some(0.0);
    primary.x_neutral = Some(0.0);
    let mut secondary = DistWinding::new(
        "s",
        strings(&["1", "n"]),
        DistWindingConn::Wye,
        240.0,
        25000.0,
    );
    secondary.r_pct = 0.5;
    net.transformers_mut().push(DistTransformer::new(
        "t",
        vec![primary, secondary],
        vec![2.0],
        1,
    ));
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert_eq!(system.augmented().labels, ["transformer:t:neutral:p.n"]);
    let NodeRef::Node(neutral) = system.index().resolve("p", "n").unwrap() else {
        panic!("the neutral is an unknown")
    };
    let row = dense(&system.augmented().constraint_re);
    assert_eq!(row[0][neutral], 1.0);
    assert_eq!(row[0].iter().filter(|value| **value != 0.0).count(), 1);
    // The passive system is exactly the primitive's scatter.
    let primitive = calc_transformer_primitive_admittance(&net, &net.transformers()[0]).unwrap();
    let g = dense(system.conductance());
    for (r, &(bus, terminal)) in primitive.terminals.iter().enumerate() {
        for (c, &(bus_c, terminal_c)) in primitive.terminals.iter().enumerate() {
            let (NodeRef::Node(i), NodeRef::Node(j)) = (
                system.index().resolve(bus, terminal).unwrap(),
                system.index().resolve(bus_c, terminal_c).unwrap(),
            ) else {
                panic!()
            };
            assert_eq!(g[i][j], primitive.primitive[r][c].re);
        }
    }
}

#[test]
fn every_leakage_base_states_one_physical_impedance() {
    use num_complex::Complex64;

    let mut first = DistWinding::new(
        "p",
        strings(&["1", "2", "3", "0"]),
        DistWindingConn::Wye,
        12_470.0,
        300_000.0,
    );
    first.r_pct = 0.5;
    let mut second = DistWinding::new(
        "s",
        strings(&["1", "2", "3"]),
        DistWindingConn::Delta,
        480.0,
        150_000.0,
    );
    second.r_pct = 0.7;
    let transformer = DistTransformer::new("t", vec![first, second], vec![6.0], 3);
    let nameplate =
        calc_transformer_leakage_impedances(&transformer, &TransformerLeakageBase::Nameplate)
            .unwrap();
    // The same unit on BMOPF phase bases (three coils each) with a power
    // base equal to the first rating: identical ohms at the first winding.
    let phase = calc_transformer_leakage_impedances(
        &transformer,
        &TransformerLeakageBase::WindingPhase {
            power_base: 300_000.0,
            phases: vec![3.0, 3.0],
        },
    )
    .unwrap();
    let v2 = 12_470.0f64.powi(2);
    let ohms_nameplate = nameplate[0][1] * v2 / 300_000.0;
    let ohms_phase = phase[0][1] * v2 / (300_000.0 / 3.0);
    // Per phase ohms are three times the three phase base's: the n-winding
    // resistances and the reactance both carry the phase count.
    assert!((ohms_phase - ohms_nameplate * 3.0).norm() < 1e-9 * ohms_phase.norm());

    let mut regulator = DistTransformer::new(
        "r",
        vec![
            DistWinding::new("p", strings(&["1", "n"]), DistWindingConn::Wye, 2400.0, 5e5),
            DistWinding::new("s", strings(&["1", "n"]), DistWindingConn::Wye, 2400.0, 5e5),
        ],
        vec![0.0],
        1,
    );
    regulator.windings[1].tap = 1.05;
    for (key, value) in [
        ("r_series_from", 0.5),
        ("x_series_from", 2.0),
        ("r_series_to", 0.25),
        ("x_series_to", 0.5),
    ] {
        regulator
            .extras
            .insert(key.into(), serde_json::json!(value));
    }
    let stated =
        calc_transformer_leakage_impedances(&regulator, &TransformerLeakageBase::StatedOhms)
            .unwrap();
    let n = 1.0 / 1.05;
    assert_eq!(
        stated[0][1],
        Complex64::new(0.5 + n * n * 0.25, 2.0 + n * n * 0.5)
    );
}

#[test]
fn one_open_line_terminal_preserves_remote_charging_without_bus_injection() {
    use num_complex::Complex64;

    let mut net = MulticonductorNetwork::new();
    for id in ["source", "external", "line-end"] {
        net.buses_mut().push(DistBus::new(id, strings(&["1"])));
    }
    let mut code = DistLineCode::new("pi", vec![vec![1.0]], vec![vec![2.0]]);
    code.b_from = vec![vec![0.01]];
    code.b_to = vec![vec![0.01]];
    net.line_codes_mut().push(code);
    net.lines_mut().push(DistLine::new(
        "line",
        "source",
        "line-end",
        strings(&["1"]),
        strings(&["1"]),
        "pi",
        1.0,
    ));
    net.switches_mut().push(DistSwitch::new(
        "terminal",
        "external",
        "line-end",
        strings(&["1"]),
        strings(&["1"]),
        true,
    ));

    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let node = |bus| match system.index().resolve(bus, "1").unwrap() {
        NodeRef::Node(index) => index,
        NodeRef::Ground => panic!("unexpected grounding"),
    };
    let source = node("source");
    let end = node("line-end");
    let external = node("external");
    assert_ne!(end, external);
    let g = dense(system.conductance());
    let b = dense(system.susceptance());
    let y = |i: usize, j: usize| Complex64::new(g[i][j], b[i][j]);
    for i in 0..system.index().len() {
        assert!(y(external, i).norm() < 1e-14);
        assert!(y(i, external).norm() < 1e-14);
    }
    // Zero current at the open line end determines its floating voltage.
    // Both pi charging halves contribute to the source current through Z.
    let voltage = Complex64::new(230.0, 0.0);
    let open_voltage = -y(end, source) * voltage / y(end, end);
    let open_current = y(end, source) * voltage + y(end, end) * open_voltage;
    assert!(open_current.norm() < 1e-12);
    let source_current = y(source, source) * voltage + y(source, end) * open_voltage;
    let z = Complex64::new(1.0, 2.0);
    let half = Complex64::new(0.0, 0.01);
    let expected = (half + half / (Complex64::new(1.0, 0.0) + z * half)) * voltage;
    assert!((source_current - expected).norm() < 1e-12);
    assert!(
        (source_current - half * voltage).norm() > 2.0,
        "remote charging was lost"
    );

    // Closing the terminal merges the external and internal line-end nodes
    // exactly, with no artificial series impedance introduced by the switch.
    net.switches_mut()[0].open = false;
    let closed = calc_multiconductor_admittance_matrix(&net).unwrap();
    assert_eq!(
        closed.index().resolve("external", "1"),
        closed.index().resolve("line-end", "1")
    );
}

#[test]
fn coupled_dielectric_shunts_dissipate_unbalanced_power_but_not_common_mode() {
    let mut net = MulticonductorNetwork::new();
    for id in ["a", "b"] {
        // Deliberately differ from the line's phase-coordinate ordering.
        net.buses_mut()
            .push(DistBus::new(id, strings(&["3", "1", "2"])));
    }
    let mut code = DistLineCode::new(
        "pi",
        vec![
            vec![0.5, 0.2, 0.2],
            vec![0.2, 0.5, 0.2],
            vec![0.2, 0.2, 0.5],
        ],
        vec![vec![0.0; 3]; 3],
    );
    // G1=G2=1e-5 S/m, G0=0, divided into two pi halves. This is
    // a positive-semidefinite phase matrix despite negative mutual terms.
    code.g_from = vec![vec![-1e-5 / 6.0; 3]; 3];
    for i in 0..3 {
        code.g_from[i][i] = 1e-5 / 3.0;
    }
    code.g_to.clone_from(&code.g_from);
    net.line_codes_mut().push(code);
    net.lines_mut().push(DistLine::new(
        "l",
        "a",
        "b",
        strings(&["1", "2", "3"]),
        strings(&["1", "2", "3"]),
        "pi",
        250.0,
    ));
    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let g = dense(system.conductance());
    for (phase_voltages, expected_watts) in [
        ([230.0, 100.0, -20.0], 78.166_666_666_666_67),
        ([230.0; 3], 0.0),
    ] {
        let mut voltage = vec![0.0; system.index().len()];
        for bus in ["a", "b"] {
            for (phase, value) in ["1", "2", "3"].into_iter().zip(phase_voltages) {
                let NodeRef::Node(index) = system.index().resolve(bus, phase).unwrap() else {
                    panic!("unexpected grounding");
                };
                voltage[index] = value;
            }
        }
        // Equal voltages at both ends remove series losses. V' G V must
        // equal the documented shunt power, including mutual conductance.
        let mut power = 0.0;
        for i in 0..voltage.len() {
            for j in 0..voltage.len() {
                power += voltage[i] * g[i][j] * voltage[j];
            }
        }
        assert!((power - expected_watts).abs() < 1e-9);
    }
}

#[test]
fn bmopf_unequal_capacitors_stamp_the_actual_coil_ratings() {
    let text = r#"{"bus":{"b":{"terminal_names":["a","b","c","n"],"perfectly_grounded_terminals":["n"]}},"capacitor":{"c":{"bus":"b","terminal_map":["a","b","c","n"],"configuration":"WYE","q_rated":[13,25,7],"v_nom":230}}}"#;
    let source = powerio_core::Source::from_memory("caps.json", text.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("bmopf").unwrap());
    let module = powerio_dist::parse(source).unwrap();
    let y = calc_multiconductor_admittance_matrix(module.value()).unwrap();
    let b = dense(y.susceptance());
    assert_eq!(b.len(), 3);
    for (i, q) in [13.0, 25.0, 7.0].into_iter().enumerate() {
        for j in 0..3 {
            let expected = if i == j { q / 52900.0 } else { 0.0 };
            assert!((b[i][j] - expected).abs() < 1e-12);
        }
    }
}

/// The line, shunt and capacitor primitives are what the nodal builder
/// scatters: a two bus feeder with one of each assembles to exactly their
/// sum over the node indexing.
#[test]
fn the_nodal_system_is_the_scatter_of_the_element_primitives() {
    use num_complex::Complex64;
    use powerio_matrix::{
        calc_capacitor_primitive_admittance, calc_line_primitive_admittance,
        calc_shunt_primitive_admittance,
    };
    // `n` is each bus's neutral by the terminal conventions, so the WYE
    // bank's coils return through it.
    let mut net = MulticonductorNetwork::default();
    net.buses_mut()
        .push(DistBus::new("a", strings(&["1", "2", "n"])));
    net.buses_mut()
        .push(DistBus::new("b", strings(&["1", "2", "n"])));
    let mut code = DistLineCode::new(
        "lc",
        vec![vec![0.3, 0.1], vec![0.1, 0.3]],
        vec![vec![0.8, 0.4], vec![0.4, 0.8]],
    );
    code.b_from = vec![vec![1e-4, -2e-5], vec![-2e-5, 1e-4]];
    code.b_to = code.b_from.clone();
    net.line_codes_mut().push(code);
    let line = DistLine::new(
        "l1",
        "a",
        "b",
        strings(&["1", "2"]),
        strings(&["1", "2"]),
        "lc",
        2.5,
    );
    net.lines_mut().push(line.clone());
    let shunt = DistShunt::new(
        "sh",
        "b",
        strings(&["1", "n"]),
        vec![vec![0.01, -0.01], vec![-0.01, 0.01]],
        vec![vec![0.02, -0.02], vec![-0.02, 0.02]],
    );
    net.shunts_mut().push(shunt.clone());
    let capacitor = DistCapacitor::new(
        "c",
        "a",
        strings(&["1", "2", "n"]),
        Configuration::Wye,
        3.0e4,
        400.0,
    );
    net.capacitors_mut().push(capacitor.clone());

    let system = calc_multiconductor_admittance_matrix(&net).unwrap();
    let index = system.index();
    let node = |bus: &str, terminal: &str| match index.resolve(bus, terminal) {
        Some(NodeRef::Node(dense)) => dense,
        other => panic!("{bus}.{terminal}: {other:?}"),
    };
    let n = index.len();
    let mut expected = vec![vec![Complex64::new(0.0, 0.0); n]; n];
    let mut scatter = |terminals: &[usize], primitive: &[Vec<Complex64>]| {
        for (row, &i) in terminals.iter().enumerate() {
            for (column, &j) in terminals.iter().enumerate() {
                expected[i][j] += primitive[row][column];
            }
        }
    };
    let code = &net.line_codes()[0];
    let line_primitive = calc_line_primitive_admittance(&line, code).unwrap();
    assert_eq!(line_primitive.len(), 4);
    scatter(
        &[
            node("a", "1"),
            node("a", "2"),
            node("b", "1"),
            node("b", "2"),
        ],
        &line_primitive,
    );
    scatter(
        &[node("b", "1"), node("b", "n")],
        &calc_shunt_primitive_admittance(&shunt),
    );
    let capacitor_primitive = calc_capacitor_primitive_admittance(&net, &capacitor)
        .unwrap()
        .expect("wye bank");
    // Two phase coils to the neutral, each q / 2 / (v / sqrt 3)^2.
    let coil = 3.0e4 / 2.0 / (400.0 / 3f64.sqrt()).powi(2);
    assert!((capacitor_primitive[2][2].im - 2.0 * coil).abs() < 1e-12);
    scatter(
        &[node("a", "1"), node("a", "2"), node("a", "n")],
        &capacitor_primitive,
    );
    let g = dense(system.conductance());
    let b = dense(system.susceptance());
    for i in 0..n {
        for j in 0..n {
            let scale = expected[i][j].norm().max(1.0);
            assert!(
                (g[i][j] - expected[i][j].re).abs() <= 1e-12 * scale,
                "G[{i}][{j}]"
            );
            assert!(
                (b[i][j] - expected[i][j].im).abs() <= 1e-12 * scale,
                "B[{i}][{j}]"
            );
        }
    }
}

/// A near zero jumper's series impedance is singular, not an admittance of
/// order 1e15: a pivot of 1e-14 ohm or less is refused.
#[test]
fn a_near_zero_series_impedance_is_singular() {
    let code = DistLineCode::new(
        "lc",
        vec![vec![0.3, 0.1], vec![0.1, 0.3]],
        vec![vec![0.8, 0.4], vec![0.4, 0.8]],
    );
    let mut line = DistLine::new(
        "l1",
        "a",
        "b",
        strings(&["1", "2"]),
        strings(&["1", "2"]),
        "lc",
        2.5,
    );
    assert!(powerio_matrix::calc_line_primitive_admittance(&line, &code).is_ok());
    line.length = 1e-16;
    assert!(powerio_matrix::calc_line_primitive_admittance(&line, &code).is_err());
}

#[test]
fn a_two_winding_leakage_inverts_to_one_series_admittance() {
    use num_complex::Complex64;

    let mut first = DistWinding::new("p", strings(&["1"]), DistWindingConn::Wye, 7200.0, 25000.0);
    first.r_pct = 1.0;
    let mut second = DistWinding::new("s", strings(&["1"]), DistWindingConn::Wye, 240.0, 50000.0);
    second.r_pct = 2.0;
    let transformer = DistTransformer::new("t", vec![first, second], vec![8.0], 1);
    let leakage =
        calc_transformer_leakage_impedances(&transformer, &TransformerLeakageBase::Nameplate)
            .unwrap();
    // The second winding's resistance is referred to the first's rating.
    let expected = Complex64::new(0.01 + 0.02 * 25000.0 / 50000.0, 0.08);
    assert!((leakage[0][1] - expected).norm() < 1e-15);
    assert_eq!(leakage[0][1], leakage[1][0]);
    let admittance = calc_transformer_winding_admittance(&transformer)
        .unwrap()
        .unwrap();
    let y = (expected / 25000.0).finv();
    for (row, signs) in admittance.iter().zip([[1.0, -1.0], [-1.0, 1.0]]) {
        for (&value, sign) in row.iter().zip(signs) {
            assert!((value - y * sign).norm() < 1e-9 * y.norm());
        }
    }
    let short = DistTransformer::new(
        "short",
        transformer
            .windings
            .iter()
            .cloned()
            .map(|mut w| {
                w.r_pct = 0.0;
                w
            })
            .collect(),
        vec![0.0],
        1,
    );
    assert!(
        calc_transformer_winding_admittance(&short)
            .unwrap()
            .is_none()
    );
}

#[test]
fn the_passive_element_list_states_each_family_in_table_order() {
    let mut net = MulticonductorNetwork::default();
    net.buses_mut()
        .push(DistBus::new("a", strings(&["1", "2"])));
    net.buses_mut()
        .push(DistBus::new("b", strings(&["1", "2"])));
    net.line_codes_mut()
        .push(DistLineCode::new("lc", vec![vec![1.0]], vec![vec![2.0]]));
    net.lines_mut().push(DistLine::new(
        "l",
        "a",
        "b",
        strings(&["1"]),
        strings(&["1"]),
        "lc",
        1.0,
    ));
    net.capacitors_mut().push(DistCapacitor::new(
        "c",
        "b",
        strings(&["1", "2"]),
        Configuration::SinglePhase,
        1000.0,
        240.0,
    ));
    let listed = calc_passive_primitives(&net).unwrap();
    let (primitives, findings) = (&listed.elements, &listed.diagnostics);
    assert_eq!(findings.len(), 0, "{findings:?}");
    assert!(listed.grounded.is_empty() && listed.ideal_transformers.is_empty());
    let families: Vec<_> = primitives.iter().map(|p| p.family).collect();
    assert_eq!(families, [PassiveFamily::Line, PassiveFamily::Capacitor]);
    assert_eq!(primitives[0].terminals, [("a", "1"), ("b", "1")]);
    assert_eq!(primitives[1].terminals, [("b", "1"), ("b", "2")]);
}
