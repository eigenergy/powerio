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
use powerio_matrix::{NodeRef, build_multiconductor_admittance};

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

    let system = build_multiconductor_admittance(&net).unwrap();
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
    net.linecodes_mut()
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

    let system = build_multiconductor_admittance(&net).unwrap();
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
    net.linecodes_mut().push(code);
    net.lines_mut().push(DistLine::new(
        "l1",
        "a",
        "b",
        strings(&["1", "2"]),
        strings(&["1", "2"]),
        "lc2",
        1.0,
    ));

    let system = build_multiconductor_admittance(&net).unwrap();
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

    let system = build_multiconductor_admittance(&net).unwrap();
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

    let system = build_multiconductor_admittance(&net).unwrap();
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
    let system = build_multiconductor_admittance(&net).unwrap();
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
        vec![2.0],
        1,
    ));

    let system = build_multiconductor_admittance(&net).unwrap();
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

    // The leakage reactance is a real impedance on the primary base:
    // z_base = 7200^2 / 25000, x = 2% of it, stamped as -1/x on the primary
    // diagonal susceptance.
    let b = dense(system.susceptance());
    let z_base = 7200.0f64 * 7200.0 / 25_000.0;
    let expected = -1.0 / (0.02 * z_base);
    assert!(
        (b[p_node][p_node] - expected).abs() < 1e-12,
        "{} vs {expected}",
        b[p_node][p_node]
    );
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
    let system = build_multiconductor_admittance(&net).unwrap();
    assert!(
        system
            .diagnostics()
            .iter()
            .any(|d| d.code() == "BUILD.MULTI.UNSUPPORTED_STAMP" && d.message().contains("three")),
        "{:?}",
        system.diagnostics()
    );
}

#[test]
fn axis_mappings_survive_source_row_reordering() {
    let build = |reversed: bool| {
        let mut net = MulticonductorNetwork::default();
        net.buses_mut().push(DistBus::new("a", strings(&["1"])));
        net.buses_mut().push(DistBus::new("b", strings(&["1"])));
        net.linecodes_mut()
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
        build_multiconductor_admittance(&net).unwrap()
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
    let source = powerio_core::Source::from_bytes("<memory>", dss.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("dss").unwrap());
    let net = powerio_dist::parse(source).unwrap().into_value();
    let system = build_multiconductor_admittance(&net).unwrap();
    assert!(system.index().len() >= 6, "three phases at two buses");
    assert!(system.susceptance().nnz() > 0);
    // The source anchors the system through the augmented rows.
    assert!(!system.augmented().labels.is_empty());
}
