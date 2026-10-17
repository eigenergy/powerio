use num_complex::Complex64;
use powerio_core::Source;

use super::{
    infeeder::{
        InfeederSetpoint, InternalImpedance, RegulationPoint, SourceGrounding, SourceZeroSequence,
    },
    load::VoltageInput,
    read,
    schema::NativeDatabase,
    semantics::{Connection, State},
    tests::database,
};

pub(super) fn infeeder_database(edit: &str) -> Vec<u8> {
    database(&format!(
        "UPDATE Element SET Type='Infeeder';
         DELETE FROM Terminal WHERE TerminalNo=2;
         ALTER TABLE Element ADD COLUMN Flag_Input INTEGER DEFAULT 7;
         ALTER TABLE Element ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Terminal ADD COLUMN Flag_Terminal INTEGER DEFAULT 7;
         ALTER TABLE Terminal ADD COLUMN Flag_State INTEGER DEFAULT 1;
         CREATE TABLE Infeeder (
             Element_ID INTEGER, Variant_ID INTEGER,
             Typ_ID INTEGER DEFAULT 0, Mpl_ID INTEGER DEFAULT 0,
             IncrSer_ID INTEGER DEFAULT 0, Macro_ID INTEGER DEFAULT 0,
             MasterElm_ID INTEGER DEFAULT 0, Node_ID INTEGER DEFAULT 0,
             PowerLimit_ID INTEGER DEFAULT 0, Flag_LfLimit INTEGER DEFAULT 0,
             Flag_LfCtrl INTEGER DEFAULT 0, Flag_Pctrl INTEGER DEFAULT 0,
             Flag_Qctrl INTEGER DEFAULT 0, Flag_Macro INTEGER DEFAULT 0,
             Kr REAL DEFAULT 0, Rlf REAL DEFAULT 0, Xlf REAL DEFAULT 0,
             Flag_Lf INTEGER DEFAULT 3, u REAL DEFAULT 105, Ug REAL DEFAULT 11,
             delta REAL DEFAULT 30, phi REAL DEFAULT -60,
             P REAL DEFAULT 2, Q REAL DEFAULT -0.5, S REAL DEFAULT 5,
             I REAL DEFAULT 0.2, cosphi REAL DEFAULT 0.8,
             fP REAL DEFAULT 2, fQ REAL DEFAULT 3, fS REAL DEFAULT 4,
             fI REAL DEFAULT 5, xi REAL DEFAULT 0, Flag_Typ INTEGER DEFAULT 2,
             Sk2 REAL DEFAULT 100, R_X REAL DEFAULT 0.1, cact REAL DEFAULT 1,
             R REAL DEFAULT 999, X REAL DEFAULT 999,
             Flag_Z0 INTEGER DEFAULT 0, Flag_Z0_Input INTEGER DEFAULT 2,
             Z0_Z1 REAL DEFAULT 2, R0_X0 REAL DEFAULT 0.2,
             R0 REAL DEFAULT 0.5, X0 REAL DEFAULT 1,
             Stp_ID INTEGER DEFAULT 0, DayOpSer_ID INTEGER DEFAULT 0,
             WeekOpSer_ID INTEGER DEFAULT 0, YearOpSer_ID INTEGER DEFAULT 0);
         INSERT INTO Infeeder (Element_ID, Variant_ID) VALUES (30,1);
         {edit}"
    ))
}

fn input(edit: &str) -> super::infeeder::InfeederInput {
    NativeDatabase::decode(&infeeder_database(edit), None)
        .unwrap()
        .infeeder_input(30)
        .unwrap()
}

fn near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-12 * expected.abs().max(1.0),
        "{actual} != {expected}"
    );
}

#[test]
fn source_and_terminal_voltage_modes_remain_distinct() {
    for (mode, relative, at) in [
        (3, true, RegulationPoint::Internal),
        (6, false, RegulationPoint::Internal),
        (8, true, RegulationPoint::Terminal),
        (9, false, RegulationPoint::Terminal),
    ] {
        let inactive = if relative { "Ug" } else { "u" };
        let decoded = input(&format!(
            "UPDATE Infeeder SET Flag_Lf={mode}, {inactive}=NULL, P=NULL, Q=NULL, fP=NULL, fQ=NULL, I=NULL, S=NULL"
        ));
        let InfeederSetpoint::Voltage {
            voltage,
            angle_rad,
            at: location,
        } = decoded.setpoint
        else {
            panic!("wrong setpoint");
        };
        assert_eq!(location, at);
        assert_eq!(
            voltage,
            if relative {
                VoltageInput::Relative(1.05)
            } else {
                VoltageInput::Absolute(11_000.0)
            }
        );
        near(angle_rad, std::f64::consts::PI / 6.0);
    }
    for (mode, relative, at) in [
        (5, true, RegulationPoint::Terminal),
        (7, false, RegulationPoint::Terminal),
        (11, true, RegulationPoint::Internal),
        (12, false, RegulationPoint::Internal),
    ] {
        let decoded = input(&format!(
            "UPDATE Infeeder SET Flag_Lf={mode}, Q=NULL, delta=NULL"
        ));
        let InfeederSetpoint::ActivePowerVoltage {
            watts,
            voltage,
            at: location,
        } = decoded.setpoint
        else {
            panic!("wrong setpoint");
        };
        near(watts, 4e6);
        assert_eq!(location, at);
        assert_eq!(
            voltage,
            if relative {
                VoltageInput::Relative(1.05)
            } else {
                VoltageInput::Absolute(11_000.0)
            }
        );
    }
}

#[test]
fn infeeder_current_and_power_modes_read_only_active_factors() {
    let current =
        input("UPDATE Infeeder SET Flag_Lf=1, fP=NULL, fQ=NULL, fS=NULL, u=NULL, Ug=NULL");
    let InfeederSetpoint::Current { amperes, angle_rad } = current.setpoint else {
        panic!("wrong setpoint");
    };
    near(amperes, 1000.0);
    near(angle_rad, -std::f64::consts::PI / 3.0);
    for (mode, p, q) in [(2, 4e6, -1.5e6), (4, 16e6, 12e6), (10, 4e6, 3e6)] {
        let decoded = input(&format!(
            "UPDATE Infeeder SET Flag_Lf={mode}, fI=NULL, phi=NULL, u=NULL, Ug=NULL, delta=NULL"
        ));
        let InfeederSetpoint::Power { watts, vars } = decoded.setpoint else {
            panic!("wrong setpoint");
        };
        near(watts, p);
        near(vars, q);
    }
    let reactive = input("UPDATE Infeeder SET Flag_Lf=4, cosphi=0");
    assert_eq!(
        reactive.setpoint,
        InfeederSetpoint::Power {
            watts: 0.0,
            vars: 20e6
        }
    );
}

#[test]
fn load_flow_impedance_is_not_short_circuit_impedance() {
    let ideal =
        input("UPDATE Infeeder SET Flag_Typ=NULL, Sk2=NULL, R_X=NULL, R=99, X=999, cact=NULL");
    assert_eq!(ideal.internal_impedance, InternalImpedance::Ideal);
    assert_eq!(
        ideal
            .internal_impedance
            .positive_sequence_ohms(20_000.0)
            .unwrap(),
        Complex64::new(0.0, 0.0)
    );
    for cact in [1.0, 1.1, 2.0] {
        let decoded = input(&format!(
            "UPDATE Infeeder SET xi=10, cact={cact}, R=NULL, X=NULL"
        ));
        let z = decoded
            .internal_impedance
            .positive_sequence_ohms(20_000.0)
            .unwrap();
        near(z.re, 0.04);
        near(z.im, 0.4);
        let z2 = decoded
            .internal_impedance
            .positive_sequence_ohms(10_000.0)
            .unwrap();
        near(z2.re, 0.01);
        near(z2.im, 0.1);
        // One sequence current exercises both voltage drop and real/reactive loss.
        let current = Complex64::new(100.0, -50.0);
        let drop = z * current;
        near(drop.re, 24.0);
        near(drop.im, 38.0);
        let loss = drop * current.conj();
        near(loss.re, 500.0);
        near(loss.im, 5000.0);
    }
    let z = input("UPDATE Infeeder SET xi=10").internal_impedance;
    for voltage in [0.0, -1.0, f64::INFINITY, 1e308, 1e-308] {
        assert!(z.positive_sequence_ohms(voltage).is_err());
    }
}

#[test]
fn source_grounding_and_profiles_preserve_active_references() {
    let off=input("UPDATE Infeeder SET Flag_Z0_Input=NULL, R0=NULL, X0=NULL, Stp_ID=-5;
                   UPDATE Element SET Flag_State=0; UPDATE Terminal SET Flag_State=0, Flag_Terminal=6");
    assert_eq!(off.grounding, SourceGrounding::Ungrounded);
    assert_eq!(off.state, State::Off);
    assert_eq!(off.terminal.state, State::Off);
    assert_eq!(off.terminal.connection, Connection::L31);
    assert_eq!(off.element, 30);
    let solid = input("UPDATE Infeeder SET Flag_Z0=1, Stp_ID=-5");
    assert_eq!(
        solid.grounding,
        SourceGrounding::Solid(SourceZeroSequence::DirectOhms(Complex64::new(0.5, 1.0)))
    );
    let grounded = input(
        "UPDATE Infeeder SET Flag_Z0=2, Flag_Z0_Input=1, R0=NULL, X0=NULL, Stp_ID=42, DayOpSer_ID=7, WeekOpSer_ID=8, YearOpSer_ID=9",
    );
    assert_eq!(
        grounded.grounding,
        SourceGrounding::Impedance {
            sequence: SourceZeroSequence::MagnitudeRatio {
                z0_over_z1: 2.0,
                r0_over_x0: 0.2
            },
            neutral_point: Some(42)
        }
    );
    assert_eq!(grounded.operating_series, [Some(7), Some(8), Some(9)]);
    let same = input(
        "UPDATE Infeeder SET Flag_Z0=1, Flag_Z0_Input=3, R0=NULL, X0=NULL, Z0_Z1=NULL, R0_X0=NULL",
    );
    assert_eq!(
        same.grounding,
        SourceGrounding::Solid(SourceZeroSequence::SameAsPositive)
    );
}

#[test]
fn infeeder_rejects_unresolved_controls_invalid_modes_and_ambiguous_rows() {
    for edit in [
        "UPDATE Infeeder SET Typ_ID=1",
        "UPDATE Infeeder SET Flag_Qctrl=1",
        "UPDATE Infeeder SET Node_ID=10",
        "UPDATE Infeeder SET Kr=1",
        "UPDATE Infeeder SET Rlf=1",
        "UPDATE Infeeder SET Xlf=1",
        "UPDATE Infeeder SET Flag_Lf=99",
        "UPDATE Infeeder SET u=0",
        "UPDATE Infeeder SET Flag_Lf=6, Ug=1e308",
        "UPDATE Infeeder SET Flag_Lf=10, cosphi=0",
        "UPDATE Infeeder SET Flag_Lf=4, cosphi=-0.8",
        "UPDATE Infeeder SET Flag_Lf=2, fP=1e308",
        "UPDATE Infeeder SET Flag_Lf=1, I=-1",
        "UPDATE Infeeder SET xi=-1",
        "UPDATE Infeeder SET xi=5e-324",
        "UPDATE Infeeder SET xi=10; UPDATE Element SET Flag_Input=2",
        "UPDATE Infeeder SET xi=10, Sk2=0",
        "UPDATE Infeeder SET xi=10, Flag_Typ=1",
        "UPDATE Infeeder SET xi=10, R_X=-1",
        "UPDATE Infeeder SET Flag_Z0=3",
        "UPDATE Infeeder SET Flag_Z0=1, Flag_Z0_Input=4",
        "UPDATE Infeeder SET Flag_Z0=2, Stp_ID=-1",
        "UPDATE Infeeder SET Flag_Z0=1; UPDATE Element SET Flag_Input=2",
        "UPDATE Element SET Flag_Input=0",
        "UPDATE Terminal SET Flag_Terminal=8",
        "UPDATE Terminal SET TerminalNo=2",
        "UPDATE Infeeder SET Variant_ID=2",
        "INSERT INTO Infeeder SELECT * FROM Infeeder",
        "DELETE FROM Infeeder",
    ] {
        let native = NativeDatabase::decode(&infeeder_database(edit), None).unwrap();
        assert!(native.infeeder_input(30).is_err(), "accepted {edit}");
    }
}

#[test]
fn infeeder_joins_are_variant_local() {
    let bytes = infeeder_database(
        "INSERT INTO Variant VALUES (2,NULL,1);
        INSERT INTO Node (Node_ID,Variant_ID) VALUES (10,2);
        INSERT INTO Element (Element_ID,Variant_ID,Type) VALUES (30,2,'Infeeder');
        INSERT INTO Terminal VALUES (40,2,30,10,1,7,1);
        INSERT INTO Infeeder (Element_ID,Variant_ID,Flag_Lf,Ug) VALUES (30,2,6,33)",
    );
    let first = NativeDatabase::decode(&bytes, Some(1))
        .unwrap()
        .infeeder_input(30)
        .unwrap();
    let second = NativeDatabase::decode(&bytes, Some(2))
        .unwrap()
        .infeeder_input(30)
        .unwrap();
    assert!(matches!(
        first.setpoint,
        InfeederSetpoint::Voltage {
            voltage: VoltageInput::Relative(_),
            ..
        }
    ));
    assert!(matches!(
        second.setpoint,
        InfeederSetpoint::Voltage {
            voltage: VoltageInput::Absolute(33_000.0),
            ..
        }
    ));
}

#[test]
fn authentic_infeeder_voltage_matches_paired_bus_rating_and_result() {
    let bytes = include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let source = Source::from_memory("case.sinx", &bytes[..]).unwrap();
    let native = read(&source, None).unwrap();
    let decoded = native.infeeder_input(18).unwrap();
    let node = &native.node_inputs().unwrap()[&decoded.terminal.node];
    let InfeederSetpoint::Voltage {
        voltage: VoltageInput::Relative(value),
        angle_rad,
        at: RegulationPoint::Internal,
    } = decoded.setpoint
    else {
        panic!("wrong setpoint")
    };
    near(node.nominal_ll_volts, 20_000.0);
    near(value, 1.025);
    near(angle_rad, 0.0);
    let csv = include_str!("../../../tests/data/sincal/simbench-csv/NodePFResult.csv");
    let row = csv
        .lines()
        .skip(1)
        .find(|l| l.split(';').next() == node.name.as_deref())
        .unwrap();
    let fields: Vec<_> = row.split(';').collect();
    near(value, fields[1].parse().unwrap());
    assert_eq!(decoded.internal_impedance, InternalImpedance::Ideal);
    assert_eq!(decoded.operating_series, [None; 3]);
}
