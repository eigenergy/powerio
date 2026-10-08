use num_complex::Complex64;

use super::{schema::NativeDatabase, tests::database};
use crate::{DistBus, DistShunt, MulticonductorNetwork};

pub(super) fn schema_sql() -> &'static str {
    "CREATE TABLE ShuntImpedance (
     Element_ID INTEGER, Variant_ID INTEGER, R REAL DEFAULT 3, X REAL DEFAULT 4,
     Flag_Z0 INTEGER DEFAULT 1, Flag_Z0_Input INTEGER DEFAULT 3,
     Flag_I INTEGER DEFAULT 1, Ireg REAL DEFAULT 0, Flag_Macro INTEGER DEFAULT 0,
     Stp_ID INTEGER DEFAULT 0, Macro_ID INTEGER DEFAULT 0);"
}

fn native(edit: &str) -> NativeDatabase {
    let bytes = database(&format!(
        "UPDATE Element SET Type='ShuntImpedance';
         ALTER TABLE Element ADD COLUMN Flag_Input INTEGER DEFAULT 6;
         ALTER TABLE Element ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Terminal ADD COLUMN Flag_Terminal INTEGER DEFAULT 7;
         ALTER TABLE Terminal ADD COLUMN Flag_State INTEGER DEFAULT 1;
         DELETE FROM Terminal WHERE TerminalNo=2;
         {} INSERT INTO ShuntImpedance (Element_ID,Variant_ID) VALUES (30,1); {edit}",
        schema_sql()
    ));
    NativeDatabase::decode(&bytes, None).unwrap()
}

fn bus() -> DistBus {
    DistBus::new("10", ["3", "n", "1", "2"].map(str::to_owned).to_vec())
}

fn current(shunt: &DistShunt, voltage: &[Complex64]) -> Vec<Complex64> {
    shunt
        .g
        .iter()
        .zip(&shunt.b)
        .map(|(g, b)| {
            g.iter()
                .zip(b)
                .zip(voltage)
                .map(|((&g, &b), &v)| Complex64::new(g, b) * v)
                .sum()
        })
        .collect()
}

#[test]
fn shunt_connections_obey_ohms_law_with_unbalanced_voltages() {
    for code in 1..=7 {
        let pair = (4..=6).contains(&code);
        let edit = format!(
            "UPDATE Terminal SET Flag_Terminal={code}; UPDATE ShuntImpedance SET Flag_Z0={}",
            i32::from(!pair)
        );
        let circuit = native(&edit)
            .shunt_impedance_input(30)
            .unwrap()
            .circuit(&bus())
            .unwrap();
        let mut voltage: Vec<Complex64> = circuit
            .shunt
            .terminal_map
            .iter()
            .map(|p| match p.as_str() {
                "1" => Complex64::new(230.0, 11.0),
                "2" => Complex64::new(-111.0, -201.0),
                "3" => Complex64::new(-98.0, 193.0),
                _ => Complex64::new(0.0, 0.0),
            })
            .collect();
        let actual = current(&circuit.shunt, &voltage);
        let last = voltage.len() - 1;
        let z = Complex64::new(3.0, 4.0);
        for i in 0..last {
            let expected = (voltage[i] - voltage[last]) / z;
            assert!((actual[i] - expected).norm() < 1e-12, "code {code}");
        }
        assert!(actual.iter().sum::<Complex64>().norm() < 1e-12);
        assert_eq!(circuit.bus.grounded.is_empty(), pair);
        assert_eq!(bus().grounded.as_slice(), []);
        assert!(
            !circuit
                .switch
                .terminal_map_from
                .iter()
                .any(|p| p == "n" || p == "star")
        );
        if code == 6 {
            assert_eq!(circuit.shunt.terminal_map, ["3", "1"]);
        }
        // The full primitive has no fictitious current from uniform voltage.
        voltage.fill(Complex64::new(25.0, -13.0));
        assert!(
            current(&circuit.shunt, &voltage)
                .iter()
                .all(|i| i.norm() < 1e-12)
        );
        let mut net = MulticonductorNetwork::new();
        net.buses_mut().extend([bus(), circuit.bus]);
        net.shunts_mut().push(circuit.shunt);
        net.switches_mut().push(circuit.switch);
        crate::require_electrical_readiness(&net).unwrap();
        let out = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
        let parsed = crate::testkit::parse_str(&out.text, "pmd-json").unwrap();
        crate::require_electrical_readiness(&parsed).unwrap();
        assert_eq!(parsed.shunts()[0].g, net.shunts()[0].g);
        assert_eq!(parsed.shunts()[0].b, net.shunts()[0].b);
        // PMD assigns a numeric conductor identifier to the private star.
        // Check its position and grounding instead of requiring its label.
        let projected = &parsed.shunts()[0];
        let original = &net.shunts()[0];
        assert_eq!(projected.terminal_map.len(), original.terminal_map.len());
        for (p, o) in projected.terminal_map.iter().zip(&original.terminal_map) {
            assert_eq!(
                parsed.bus(&projected.bus).unwrap().grounded.contains(p),
                net.bus(&original.bus).unwrap().grounded.contains(o),
            );
        }
    }
}

#[test]
fn ungrounded_star_floats_at_the_phase_average() {
    let circuit = native("UPDATE ShuntImpedance SET Flag_Z0=0, Flag_Z0_Input=999")
        .shunt_impedance_input(30)
        .unwrap()
        .circuit(&bus())
        .unwrap();
    assert_eq!(circuit.bus.grounded.as_slice(), []);
    assert_eq!(circuit.bus.terminals, ["1", "2", "3", "star"]);
    let voltage = [
        Complex64::new(240.0, 0.0),
        Complex64::new(-80.0, -180.0),
        Complex64::new(-100.0, 210.0),
    ];
    let star = voltage.iter().sum::<Complex64>() / 3.0;
    let v = [voltage[0], voltage[1], voltage[2], star];
    let actual = current(&circuit.shunt, &v);
    assert!(actual[3].norm() < 1e-12);
    assert!(actual[..3].iter().sum::<Complex64>().norm() < 1e-12);
    for i in 0..3 {
        assert!((actual[i] - (voltage[i] - star) / Complex64::new(3.0, 4.0)).norm() < 1e-12);
    }
}

#[test]
fn open_shunt_port_retains_its_primitive_and_grounding() {
    let closed = native("")
        .shunt_impedance_input(30)
        .unwrap()
        .circuit(&bus())
        .unwrap();
    let open = native("UPDATE Terminal SET Flag_State=0")
        .shunt_impedance_input(30)
        .unwrap()
        .circuit(&bus())
        .unwrap();
    assert_eq!(closed.shunt, open.shunt);
    assert_eq!(closed.bus, open.bus);
    assert!(!closed.switch.open && open.switch.open);
}

#[test]
fn shunt_numeric_limits_preserve_reactive_sign_and_reject_unrepresentable_values() {
    for x in [-4.0, 4.0, -1e200, 1e-200] {
        let circuit = native(&format!("UPDATE ShuntImpedance SET R=0, X={x}"))
            .shunt_impedance_input(30)
            .unwrap()
            .circuit(&bus())
            .unwrap();
        assert!((circuit.shunt.b[0][0] * x + 1.0).abs() < 1e-14);
        assert_eq!(circuit.shunt.g[0][0].abs().to_bits(), 0);
    }
    for edit in [
        "UPDATE ShuntImpedance SET R=0, X=0",
        "UPDATE ShuntImpedance SET R=-1",
        "UPDATE ShuntImpedance SET R=NULL",
        "UPDATE ShuntImpedance SET X=1e999",
        "UPDATE ShuntImpedance SET R=1e308, X=1e-308",
        "UPDATE ShuntImpedance SET R=0, X=1e-310",
    ] {
        assert!(native(edit).shunt_impedance_input(30).is_err(), "{edit}");
    }
    // Branch admittance fits, but the three-branch star diagonal does not.
    assert!(
        native("UPDATE ShuntImpedance SET R=0, X=1e-308")
            .shunt_impedance_input(30)
            .unwrap()
            .circuit(&bus())
            .is_err()
    );
}

#[test]
fn shunt_rejects_unresolved_modes_and_ambiguous_records() {
    for edit in [
        "UPDATE ShuntImpedance SET Flag_Z0=2",
        "UPDATE ShuntImpedance SET Flag_Z0_Input=2",
        "UPDATE ShuntImpedance SET Stp_ID=4",
        "UPDATE ShuntImpedance SET Macro_ID=4",
        "UPDATE ShuntImpedance SET Flag_Macro=1",
        "UPDATE ShuntImpedance SET Ireg=0.1",
        "UPDATE ShuntImpedance SET Flag_I=2",
        "UPDATE Element SET Flag_Input=2",
        "UPDATE Element SET Flag_State=0",
        "UPDATE Terminal SET Flag_Terminal=8",
        "UPDATE Terminal SET Flag_Terminal=1; UPDATE ShuntImpedance SET Flag_Z0=0",
        "UPDATE Terminal SET Flag_Terminal=4",
        "DELETE FROM ShuntImpedance",
        "UPDATE ShuntImpedance SET Variant_ID=2",
        "INSERT INTO ShuntImpedance SELECT * FROM ShuntImpedance",
        "ALTER TABLE ShuntImpedance ADD COLUMN Flag_Lf INTEGER",
        "ALTER TABLE ShuntImpedance ADD COLUMN Flag_Lf INTEGER DEFAULT 1",
        "ALTER TABLE ShuntImpedance ADD COLUMN Typ_ID INTEGER DEFAULT 5",
        "ALTER TABLE ShuntImpedance ADD COLUMN Flag_Typ_ID INTEGER DEFAULT 1",
    ] {
        assert!(native(edit).shunt_impedance_input(30).is_err(), "{edit}");
    }
    let input = native("").shunt_impedance_input(30).unwrap();
    let mut wrong = bus();
    wrong.id = "999".into();
    assert!(input.circuit(&wrong).is_err());
    wrong = bus();
    wrong.terminals.retain(|p| p != "3");
    assert!(input.circuit(&wrong).is_err());
}
