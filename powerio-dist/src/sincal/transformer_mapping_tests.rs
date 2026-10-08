use std::{collections::BTreeMap, f64::consts::PI};

use num_complex::Complex64;

use super::{
    schema::NativeDatabase, transformer::WindingKind, transformer_tests::impedance_database,
};
use crate::{DistBus, DistShunt, MulticonductorNetwork};

const ZERO: Complex64 = Complex64::new(0.0, 0.0);
const RATIO: f64 = 27.5;

pub(super) fn native(edit: &str) -> NativeDatabase {
    let bytes = impedance_database(&format!(
        "CREATE TABLE CalcParameter (Variant_ID INTEGER, Flag_LFZ0 INTEGER);
         INSERT INTO CalcParameter VALUES (1,1);
         ALTER TABLE TwoWindingTransformer ADD COLUMN Flag_Lf INTEGER DEFAULT 1;
         ALTER TABLE TwoWindingTransformer ADD COLUMN Flag_Macro INTEGER DEFAULT 0;
         ALTER TABLE TwoWindingTransformer ADD COLUMN Flag_Boost INTEGER DEFAULT 0;
         ALTER TABLE TwoWindingTransformer ADD COLUMN C01 REAL DEFAULT 0;
         ALTER TABLE TwoWindingTransformer ADD COLUMN C02 REAL DEFAULT 0;
         ALTER TABLE TwoWindingTransformer ADD COLUMN ElemLoading_ID INTEGER DEFAULT 0;
         ALTER TABLE TwoWindingTransformer ADD COLUMN Ctrl_OpSer_ID INTEGER DEFAULT 0;
         ALTER TABLE TwoWindingTransformer ADD COLUMN Ctrl_OpPnt_ID INTEGER DEFAULT 0;
         ALTER TABLE TwoWindingTransformer ADD COLUMN CompImp_ID INTEGER DEFAULT 0;
         ALTER TABLE TwoWindingTransformer ADD COLUMN CtrlRange_ID INTEGER DEFAULT 0;
         UPDATE TwoWindingTransformer SET roh=rohm;
         {edit}"
    ));
    NativeDatabase::decode(&bytes, None).unwrap()
}

fn buses() -> BTreeMap<i64, DistBus> {
    [10, 20]
        .into_iter()
        .map(|id| {
            (
                id,
                DistBus::new(
                    id.to_string(),
                    ["1", "2", "3", "n"].map(str::to_owned).to_vec(),
                ),
            )
        })
        .collect()
}

fn phases(zero: Complex64, positive: Complex64, negative: Complex64) -> [Complex64; 3] {
    std::array::from_fn(|p| {
        let rotation = Complex64::from_polar(1.0, -(p as f64) * 2.0 * PI / 3.0);
        zero + rotation * positive + rotation.conj() * negative
    })
}

fn currents(shunt: &DistShunt, v: [Complex64; 6]) -> [Complex64; 6] {
    std::array::from_fn(|i| {
        (0..6)
            .map(|j| Complex64::new(shunt.g[i][j], shunt.b[i][j]) * v[j])
            .sum()
    })
}

fn ports(a: [Complex64; 3], b: [Complex64; 3]) -> [Complex64; 6] {
    [a[0], a[1], a[2], b[0], b[1], b[2]]
}

fn close(actual: Complex64, expected: Complex64) {
    assert!(
        (actual - expected).norm() <= 1e-9 * expected.norm().max(1.0),
        "{actual} != {expected}"
    );
}

// Original synthetic measurements of a passive zero-sequence T circuit,
// expressed on their documented native measurement sides.
fn measured_t(group: i64) -> String {
    let z1 = Complex64::new(0.1, 0.3);
    let z2 = Complex64::new(0.2, 0.4);
    let z3 = Complex64::new(10.0, 20.0);
    let za = (z1 + z3) * RATIO * RATIO;
    let zb = z2 + z3;
    let zsc = (z1 + z2 * z3 / (z2 + z3)) * RATIO * RATIO;
    format!(
        "UPDATE TwoWindingTransformer SET VecGrp={group}, Flag_Z0_Input=4,
        ZABNL={}, RX_ZABNL={}, ZBANL={}, RX_ZBANL={}, ZABSC={}, RX_ZABSC={};",
        za.norm(),
        za.re / za.im,
        zb.norm(),
        zb.re / zb.im,
        zsc.norm(),
        zsc.re / zsc.im
    )
}

#[test]
fn all_ordinary_groups_match_coil_polarity_for_both_rotating_sequences() {
    for code in [
        1, 4, 5, 6, 7, 10, 13, 14, 23, 24, 25, 26, 35, 38, 39, 40, 41, 44, 45, 48, 49, 58, 59, 60,
        61, 70,
    ] {
        let measurements = if [5, 39].contains(&code) {
            measured_t(code)
        } else {
            String::new()
        };
        let db = native(&format!(
            "UPDATE TwoWindingTransformer SET VecGrp={code}, Vfe=0,i0=0;{measurements}"
        ));
        let input = db.transformer_connection(30).unwrap();
        let circuit = db.transformer_circuit(30, &buses()).unwrap();
        // Superpose unequal positive and negative sequence voltages. Verify
        // their physical coil voltages before checking the assembled port.
        let positive = Complex64::new(210.0, 20.0);
        let negative = Complex64::new(17.0, -12.0);
        let clock = Complex64::from_polar(1.0, -f64::from(input.vector_group.clock) * PI / 6.0);
        let vp = phases(ZERO, positive * RATIO, negative * RATIO);
        let vs = phases(ZERO, positive * clock, negative * clock.conj());
        let coil_voltage = |incidence: [i8; 4], v: [Complex64; 3], kind| {
            let value: Complex64 = incidence[..3]
                .iter()
                .zip(v)
                .map(|(&n, v)| f64::from(n) * v)
                .sum();
            value
                / if kind == WindingKind::Delta {
                    3.0_f64.sqrt()
                } else {
                    1.0
                }
        };
        for coil in &input.coils {
            close(
                coil_voltage(coil.primary, vp, input.vector_group.primary) / RATIO,
                coil_voltage(coil.secondary, vs, input.vector_group.secondary),
            );
        }
        for current in currents(&circuit.shunt, ports(vp, vs)) {
            close(current, ZERO);
        }
        // Reciprocity is a physical invariant of this passive fixed-ratio
        // circuit, even when the two off-diagonal 3x3 blocks differ.
        for i in 0..6 {
            for j in 0..6 {
                close(
                    Complex64::new(circuit.shunt.g[i][j], circuit.shunt.b[i][j]),
                    Complex64::new(circuit.shunt.g[j][i], circuit.shunt.b[j][i]),
                );
            }
        }
    }
}

#[test]
fn unequal_phase_excitation_accounts_for_leakage_core_and_ground_path_power() {
    let circuit = native("").transformer_circuit(30, &buses()).unwrap();
    let z = Complex64::new(0.048, 0.064);
    let ym = Complex64::new(1000.0, -(3e6_f64).sqrt()) / 160_000.0;
    let z0 = Complex64::new(2.0, 3.0);
    let vs1 = Complex64::new(225.0, -10.0);
    let vs2 = Complex64::new(10.0, 4.0);
    let i1 = Complex64::new(30.0, -12.0);
    let i2 = Complex64::new(-4.0, 3.0);
    let vp1 = vs1 + z * i1;
    let vp2 = vs2 + z * i2;
    let v0 = Complex64::new(2.0, -1.0);
    let a = Complex64::from_polar(RATIO, PI / 6.0);
    let v = ports(
        phases(v0, a * vp1, a.conj() * vp2),
        phases(Complex64::new(9.0, 4.0), vs1, vs2),
    );
    let actual = currents(&circuit.shunt, v);
    let expected = ports(
        phases(
            v0 / z0,
            (i1 + ym * vp1 / 2.0) / a.conj(),
            (i2 + ym * vp2 / 2.0) / a,
        ),
        phases(ZERO, -i1 + ym * vs1 / 2.0, -i2 + ym * vs2 / 2.0),
    );
    for (actual, expected) in actual.into_iter().zip(expected) {
        close(actual, expected);
    }
    let power: Complex64 = v.into_iter().zip(actual).map(|(v, i)| v * i.conj()).sum();
    let loss = 3.0
        * (z * (i1.norm_sqr() + i2.norm_sqr())
            + ym.conj() / 2.0
                * (vp1.norm_sqr() + vp2.norm_sqr() + vs1.norm_sqr() + vs2.norm_sqr())
            + v0.norm_sqr() / z0.conj());
    close(power, loss);
    assert!(power.re > 1000.0 && power.im > 0.0);
}

#[test]
fn additional_rotation_preserves_both_rotating_sequences_and_complex_losses() {
    // All ordinary groups without an external zero-sequence ground path.
    for group in [1, 6, 13, 23, 25, 35, 40, 44, 48, 58, 60, 70] {
        for degrees in [-150.0_f64, -17.5, 30.0, 150.0, 360.0] {
            let db = native(&format!(
                "UPDATE TwoWindingTransformer SET VecGrp={group}, AddRotate={degrees}"
            ));
            let input = db.transformer_connection(30).unwrap();
            let circuit = db.transformer_circuit(30, &buses()).unwrap();
            let angle = f64::from(input.vector_group.clock) * PI / 6.0 + degrees.to_radians();
            let a = Complex64::from_polar(RATIO, angle);
            let z = Complex64::new(0.048, 0.064);
            let ym = Complex64::new(1000.0, -(3e6_f64).sqrt()) / 160_000.0;
            let vs1 = Complex64::new(219.0, -13.0);
            let vs2 = Complex64::new(17.0, 9.0);
            let i1 = Complex64::new(31.0, -11.0);
            let i2 = Complex64::new(-7.0, 5.0);
            let internal1 = vs1 + z * i1;
            let internal2 = vs2 + z * i2;
            let v = ports(
                phases(
                    Complex64::new(9.0, 3.0),
                    a * internal1,
                    a.conj() * internal2,
                ),
                phases(Complex64::new(-2.0, 7.0), vs1, vs2),
            );
            let actual = currents(&circuit.shunt, v);
            let expected = ports(
                phases(
                    ZERO,
                    (i1 + ym * internal1 / 2.0) / a.conj(),
                    (i2 + ym * internal2 / 2.0) / a,
                ),
                phases(ZERO, -i1 + ym * vs1 / 2.0, -i2 + ym * vs2 / 2.0),
            );
            for (actual, expected) in actual.into_iter().zip(expected) {
                close(actual, expected);
            }
            // Ideal rotation contributes no power; only the same physical
            // leakage and core circuit consumes complex power at every angle.
            let power: Complex64 = v.into_iter().zip(actual).map(|(v, i)| v * i.conj()).sum();
            let loss = 3.0
                * (z * (i1.norm_sqr() + i2.norm_sqr())
                    + ym.conj() / 2.0
                        * (internal1.norm_sqr()
                            + internal2.norm_sqr()
                            + vs1.norm_sqr()
                            + vs2.norm_sqr()));
            close(power, loss);
            for i in 0..6 {
                for j in 0..6 {
                    close(
                        Complex64::new(circuit.shunt.g[i][j], circuit.shunt.b[i][j]),
                        Complex64::new(circuit.shunt.g[j][i], circuit.shunt.b[j][i]),
                    );
                }
            }
        }
    }
}

#[test]
fn authentic_additional_rotation_matches_paired_csv_transformer_parameters() {
    let source = powerio_core::Source::from_memory(
        "case.sinx",
        &include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx")[..],
    )
    .unwrap();
    let input = super::read(&source, None)
        .unwrap()
        .transformer_nominal(19)
        .unwrap();
    let primitive = super::transformer_mapping::phase_admittance(&input).unwrap();
    let transformer = include_str!("../../../tests/data/sincal/simbench-csv/Transformer.csv")
        .lines()
        .nth(1)
        .unwrap()
        .split(';')
        .collect::<Vec<_>>();
    let row = include_str!("../../../tests/data/sincal/simbench-csv/TransformerType.csv")
        .lines()
        .skip(1)
        .map(|line| line.split(';').collect::<Vec<_>>())
        .find(|fields| fields[0] == transformer[3])
        .unwrap();
    let n = |i: usize| row[i].parse::<f64>().unwrap();
    let va = n(1) * 1e6;
    let lv = n(3) * 1000.0;
    let zbase = lv * lv / va;
    let r = n(6) * 1000.0 / va * zbase;
    let zabs = n(5) / 100.0 * zbase;
    let z = Complex64::new(r, (zabs * zabs - r * r).sqrt());
    let pcore = n(7) * 1000.0;
    let score = n(8) / 100.0 * va;
    let ym = Complex64::new(pcore, -(score * score - pcore * pcore).sqrt()) / (lv * lv);
    let ratio = Complex64::from_polar(n(2) / n(3), n(4).to_radians());
    assert!((n(4) - 150.0).abs() < 1e-12);
    // Independent CSV parameters supply the expected port currents. This
    // validates component projection, not a native unbalanced load-flow run.
    let v1 = Complex64::new(207.0, 16.0);
    let v2 = Complex64::new(23.0, -7.0);
    let vp1 = Complex64::new(11_000.0, 400.0);
    let vp2 = Complex64::new(200.0, -100.0);
    let i1 = (vp1 / ratio - v1) / z;
    let i2 = (vp2 / ratio.conj() - v2) / z;
    let v = ports(phases(ZERO, vp1, vp2), phases(ZERO, v1, v2));
    let expected = ports(
        phases(
            ZERO,
            (i1 + ym * vp1 / ratio / 2.0) / ratio.conj(),
            (i2 + ym * vp2 / ratio.conj() / 2.0) / ratio,
        ),
        phases(ZERO, -i1 + ym * v1 / 2.0, -i2 + ym * v2 / 2.0),
    );
    for (row, expected) in primitive.iter().zip(expected) {
        close(row.iter().zip(v).map(|(y, v)| y * v).sum(), expected);
    }
}

#[test]
fn common_mode_current_uses_only_the_native_grounded_side_without_rescaling() {
    for (group, side) in [(14, Some(0)), (59, Some(1)), (6, None), (1, None)] {
        let circuit = native(&format!("UPDATE TwoWindingTransformer SET VecGrp={group}"))
            .transformer_circuit(30, &buses())
            .unwrap();
        let v = ports(
            [Complex64::new(2.0, 3.0); 3],
            [Complex64::new(4.0, -2.0); 3],
        );
        let actual = currents(&circuit.shunt, v);
        for p in 0..6 {
            close(
                actual[p],
                if side == Some(p / 3) {
                    v[p] / Complex64::new(2.0, 3.0)
                } else {
                    ZERO
                },
            );
        }
    }
}

#[test]
fn both_grounded_circuit_reproduces_independent_open_and_short_measurements() {
    let z1 = Complex64::new(0.1, 0.3);
    let z2 = Complex64::new(0.2, 0.4);
    let z3 = Complex64::new(10.0, 20.0);
    for group in [5, 39] {
        let circuit = native(&measured_t(group))
            .transformer_circuit(30, &buses())
            .unwrap();
        let ratio = if group == 39 { -RATIO } else { RATIO };
        for (ip, is) in [
            (Complex64::new(1.0, 0.0), ZERO),
            (ZERO, Complex64::new(1.0, 0.0)),
            (Complex64::new(1.0, 0.0), -z3 * ratio / (z2 + z3)),
        ] {
            let center = (ip * ratio + is) * z3;
            let vp = (ip * ratio * z1 + center) * ratio;
            let vs = is * z2 + center;
            for (actual, expected) in currents(&circuit.shunt, ports([vp; 3], [vs; 3]))
                .into_iter()
                .zip(ports([ip; 3], [is; 3]))
            {
                close(actual, expected);
            }
        }
    }
}

#[test]
fn transformer_switches_isolate_native_ports_without_mutating_bus_neutrals() {
    let buses = buses();
    let baseline = native("").transformer_circuit(30, &buses).unwrap();
    for states in [[1, 1], [0, 1], [1, 0], [0, 0]] {
        let circuit = native(&format!(
            "UPDATE Terminal SET Flag_State={} WHERE TerminalNo=1;
            UPDATE Terminal SET Flag_State={} WHERE TerminalNo=2",
            states[0], states[1]
        ))
        .transformer_circuit(30, &buses)
        .unwrap();
        assert_eq!(circuit.shunt, baseline.shunt);
        assert_eq!(circuit.auxiliary_bus.id, "sincal:transformer:30");
        assert_eq!(circuit.auxiliary_bus.grounded, Vec::<String>::new());
        assert_eq!(circuit.terminal_switches.len(), 2);
        for (side, switch) in circuit.terminal_switches.iter().enumerate() {
            assert_eq!(switch.name, format!("sincal:terminal:{}", [40, 50][side]));
            assert_eq!(switch.bus_from, ["10", "20"][side]);
            assert_eq!(switch.bus_to, circuit.auxiliary_bus.id);
            assert_eq!(switch.terminal_map_from, ["1", "2", "3"]);
            assert_eq!(
                switch.terminal_map_to,
                circuit.auxiliary_bus.terminals[3 * side..3 * side + 3]
            );
            assert_eq!(switch.open, states[side] == 0);
        }
    }
    assert!(
        buses
            .values()
            .all(|b| b.grounded.is_empty() && b.terminals.contains(&"n".into()))
    );
}

#[test]
fn transformer_circuit_refuses_unresolved_modes_and_degenerate_impedances() {
    for edit in [
        "UPDATE CalcParameter SET Flag_LFZ0=0",
        "INSERT INTO CalcParameter VALUES (1,1)",
        "UPDATE Element SET Flag_State=0",
        "UPDATE TwoWindingTransformer SET roh=2",
        "UPDATE Terminal SET Flag_Terminal=1",
        "UPDATE TwoWindingTransformer SET AddRotate=30",
        "UPDATE TwoWindingTransformer SET Stp_ID1=9",
        "UPDATE TwoWindingTransformer SET Stp_ID2=9",
        "UPDATE TwoWindingTransformer SET Flag_Lf=2",
        "UPDATE TwoWindingTransformer SET Flag_Macro=1",
        "UPDATE TwoWindingTransformer SET Flag_Boost=1",
        "UPDATE TwoWindingTransformer SET C01=0.01",
        "UPDATE TwoWindingTransformer SET C02=NULL",
        "UPDATE TwoWindingTransformer SET ElemLoading_ID=9",
        "UPDATE TwoWindingTransformer SET Ctrl_OpSer_ID=9",
        "UPDATE TwoWindingTransformer SET Ctrl_OpPnt_ID=9",
        "UPDATE TwoWindingTransformer SET CompImp_ID=9",
        "UPDATE TwoWindingTransformer SET CtrlRange_ID=9",
        "UPDATE TwoWindingTransformer SET uk=0,ur=0",
        "UPDATE TwoWindingTransformer SET R0=0,X0=0",
        "UPDATE TwoWindingTransformer SET Un1=1e308",
        "UPDATE TwoWindingTransformer SET uk=1e308, ur=1e308",
    ] {
        assert!(
            native(edit).transformer_circuit(30, &buses()).is_err(),
            "accepted {edit}"
        );
    }
    let mut missing = buses();
    missing.remove(&20);
    assert!(native("").transformer_circuit(30, &missing).is_err());
    let mut missing = buses();
    missing.get_mut(&10).unwrap().terminals.remove(1);
    assert!(native("").transformer_circuit(30, &missing).is_err());
    let mut wrong_id = buses();
    wrong_id.get_mut(&20).unwrap().id = "10".into();
    assert!(native("").transformer_circuit(30, &wrong_id).is_err());
    let nonpassive = format!(
        "{} UPDATE TwoWindingTransformer SET
        ZABNL={}, RX_ZABNL=1, ZBANL={}, RX_ZBANL=1, ZABSC={}, RX_ZABSC=0;",
        measured_t(5),
        2.0_f64.sqrt() * RATIO * RATIO,
        2.0_f64.sqrt(),
        100.0 * RATIO * RATIO
    );
    assert!(
        native(&nonpassive)
            .transformer_circuit(30, &buses())
            .is_err()
    );
    let capacitive_core = format!(
        "{} UPDATE TwoWindingTransformer SET
        ZABNL={}, RX_ZABNL=10, ZBANL={}, RX_ZBANL=10, ZABSC={}, RX_ZABSC=5;",
        measured_t(5),
        101.0_f64.sqrt() * RATIO * RATIO,
        101.0_f64.sqrt(),
        104.0_f64.sqrt() * RATIO * RATIO
    );
    assert!(
        native(&capacitive_core)
            .transformer_circuit(30, &buses())
            .is_err()
    );
    let singular = format!("{} UPDATE TwoWindingTransformer SET ZABSC=0", measured_t(5));
    assert!(native(&singular).transformer_circuit(30, &buses()).is_err());
}

#[test]
fn full_transformer_primitive_survives_fresh_pmd_without_physics_in_extras() {
    for edit in [
        "UPDATE Terminal SET Flag_State=0 WHERE TerminalNo=2",
        "UPDATE Terminal SET Flag_State=0 WHERE TerminalNo=2;
         UPDATE TwoWindingTransformer SET VecGrp=13, AddRotate=-17.5",
    ] {
        check_fresh_pmd(edit);
    }
}

fn check_fresh_pmd(edit: &str) {
    let source_buses = buses();
    let circuit = native(edit).transformer_circuit(30, &source_buses).unwrap();
    let mut net = MulticonductorNetwork::new();
    net.buses_mut().extend(source_buses.into_values());
    net.buses_mut().push(circuit.auxiliary_bus);
    net.shunts_mut().push(circuit.shunt);
    net.switches_mut().extend(circuit.terminal_switches);
    crate::require_electrical_readiness(&net).unwrap();
    let emitted = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
    // PMD assigns numeric conductor labels consistently across buses and
    // their ports. These name substitutions do not change the circuit.
    assert_eq!(emitted.diagnostics.len(), 21, "{:?}", emitted.diagnostics);
    assert_eq!(
        emitted
            .diagnostics
            .iter()
            .filter(|d| d.code() == "EMIT.PMD.VALUE_SUBSTITUTED")
            .count(),
        20
    );
    let metadata_loss = emitted
        .diagnostics
        .iter()
        .find(|d| d.code() == "EMIT.PMD.FIELD_DROPPED")
        .unwrap();
    assert!(metadata_loss.message().contains("`sincal_transformer`"));
    let reparsed = crate::testkit::parse_str(&emitted.text, "pmd-json").unwrap();
    crate::require_electrical_readiness(&reparsed).unwrap();
    assert_eq!(reparsed.buses().len(), 3);
    assert_eq!(reparsed.switches().len(), 2);
    for (side, (actual, expected)) in reparsed.switches().iter().zip(net.switches()).enumerate() {
        assert_eq!(actual.name, expected.name);
        assert_eq!(actual.bus_from, expected.bus_from);
        assert_eq!(actual.bus_to, expected.bus_to);
        assert_eq!(actual.open, expected.open);
        assert_eq!(actual.terminal_map_from, expected.terminal_map_from);
        assert_eq!(
            actual.terminal_map_to,
            reparsed.shunts()[0].terminal_map[3 * side..3 * side + 3]
        );
    }
    assert_eq!(reparsed.shunts().len(), 1);
    let internal = reparsed
        .buses()
        .iter()
        .find(|b| b.id == net.shunts()[0].bus)
        .unwrap();
    assert_eq!(reparsed.shunts()[0].terminal_map, internal.terminals);
    assert!(reparsed.buses().iter().all(|b| b.grounded.is_empty()));
    assert_eq!(reparsed.shunts()[0].bus, net.shunts()[0].bus);
    for (a, b) in reparsed.shunts()[0]
        .g
        .iter()
        .flatten()
        .chain(reparsed.shunts()[0].b.iter().flatten())
        .zip(
            net.shunts()[0]
                .g
                .iter()
                .flatten()
                .chain(net.shunts()[0].b.iter().flatten()),
        )
    {
        assert!((a - b).abs() <= 1e-12 * b.abs().max(1e-20));
    }
}
