use std::f64::consts::{PI, TAU};

use num_complex::Complex64;
use powerio_core::Source;

use super::{
    read,
    schema::NativeDatabase,
    semantics::State,
    tests::database,
    transformer::{CoilIncidence, VectorGroup, WindingKind},
};

pub(super) fn transformer_database(edit: &str) -> Vec<u8> {
    database(&format!(
        "UPDATE Element SET Type='TwoWindingTransformer';
         ALTER TABLE Element ADD COLUMN Flag_Input INTEGER DEFAULT 7;
         ALTER TABLE Element ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Terminal ADD COLUMN Flag_Terminal INTEGER DEFAULT 7;
         ALTER TABLE Terminal ADD COLUMN Flag_State INTEGER DEFAULT 1;
         CREATE TABLE TwoWindingTransformer (
             Element_ID INTEGER, Variant_ID INTEGER,
             Typ_ID INTEGER DEFAULT 0, Macro_ID INTEGER DEFAULT 0,
             MasterElm_ID INTEGER DEFAULT 0, TransformerTap_ID INTEGER DEFAULT 0,
             TransformerCon_ID INTEGER DEFAULT 0, Flag_Ct INTEGER DEFAULT 0,
             VecGrp INTEGER DEFAULT 14, Un1 REAL DEFAULT 11, Un2 REAL DEFAULT 0.4,
             Sn REAL DEFAULT 0.1, AddRotate REAL DEFAULT 0,
             Stp_ID1 INTEGER DEFAULT 0, Stp_ID2 INTEGER DEFAULT 0,
             Flag_roh INTEGER DEFAULT 1, Flag_ConNode INTEGER DEFAULT 1,
             Flag_Tap INTEGER DEFAULT 0, roh REAL DEFAULT 2, roh1 REAL, roh2 REAL,
             roh3 REAL, rohm REAL DEFAULT 1, ukr REAL DEFAULT 2.5,
             alpha REAL DEFAULT 0, phi REAL DEFAULT 0);
         INSERT INTO TwoWindingTransformer (Element_ID, Variant_ID) VALUES (30, 1);
         {edit}"
    ))
}

fn coil_voltage(incidence: CoilIncidence, voltages: [Complex64; 4]) -> Complex64 {
    incidence
        .into_iter()
        .zip(voltages)
        .map(|(i, v)| f64::from(i) * v)
        .sum()
}

fn assert_close<const N: usize>(actual: [f64; N], expected: [f64; N]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }
}

#[test]
fn vector_groups_give_opposite_positive_and_negative_sequence_rotation() {
    let codes = [
        1, 4, 5, 6, 7, 10, 13, 14, 23, 24, 25, 26, 35, 38, 39, 40, 41, 44, 45, 48, 49, 58, 59, 60,
        61, 70,
    ];
    let scale = |kind| {
        if kind == WindingKind::Delta {
            3.0_f64.sqrt()
        } else {
            1.0
        }
    };
    for code in codes {
        let group = VectorGroup::decode(code).unwrap();
        let coils = group.coils(&[0, 1, 2]).unwrap();
        for sequence in [1.0, -1.0] {
            let mut primary = [Complex64::new(0.0, 0.0); 4];
            let mut secondary = primary;
            for phase in 0..3 {
                primary[phase] = Complex64::from_polar(1.0, -sequence * phase as f64 * TAU / 3.0);
                secondary[phase] = primary[phase]
                    * Complex64::from_polar(1.0, -sequence * f64::from(group.clock) * PI / 6.0);
            }
            for coil in &coils {
                let left = coil_voltage(coil.primary, primary) / scale(group.primary);
                let right = coil_voltage(coil.secondary, secondary) / scale(group.secondary);
                assert!(
                    (left - right).norm() < 2e-15,
                    "code {code}, sequence {sequence}, coil {coil:?}"
                );
            }
        }
    }
}

#[test]
fn winding_selection_is_not_a_bus_phase_set_and_does_not_ground_neutrals() {
    let bytes = transformer_database("UPDATE Terminal SET Flag_Terminal=1;");
    let decoded = NativeDatabase::decode(&bytes, None)
        .unwrap()
        .transformer_connection(30)
        .unwrap();
    assert_eq!(decoded.coils.len(), 1);
    assert_eq!(decoded.coils[0].primary, [1, 0, 0, -1]);
    assert_eq!(decoded.coils[0].secondary, [1, -1, 0, 0]);
    assert_eq!(
        decoded.vector_group.primary,
        WindingKind::Wye {
            grounding_enabled: true
        }
    );
    assert_eq!(decoded.neutral_points, [None, None]);
    let floating = [Complex64::new(2.0, 1.0); 4];
    assert!(coil_voltage(decoded.coils[0].primary, floating).norm() < 1e-15);
    let common_mode = [
        Complex64::new(1.0, 0.0),
        Complex64::new(1.0, 0.0),
        Complex64::new(1.0, 0.0),
        Complex64::new(0.0, 0.0),
    ];
    assert!(coil_voltage(decoded.coils[0].secondary, common_mode).norm() < 1e-15);
    assert!((coil_voltage(decoded.coils[0].primary, common_mode).re - 1.0).abs() < 1e-15);
    let ca = VectorGroup::decode(59).unwrap().coils(&[2, 0]).unwrap();
    assert_eq!(ca[0].winding, 2);
    assert_eq!(ca[0].primary, [-1, 0, 1, 0]);
    assert!(VectorGroup::decode(14).unwrap().coils(&[0, 0]).is_err());
}

#[test]
fn transformer_units_rotation_and_active_taps_remain_distinct() {
    let bytes = transformer_database(
        "UPDATE Element SET Flag_State=0;
         UPDATE Terminal SET Flag_State=0 WHERE TerminalNo=2;
         UPDATE TwoWindingTransformer SET AddRotate=150, alpha=90, phi=-3,
             roh1=NULL, roh2=99, roh3=NULL, Stp_ID1=12;",
    );
    let decoded = NativeDatabase::decode(&bytes, None)
        .unwrap()
        .transformer_connection(30)
        .unwrap();
    assert_eq!(decoded.element, 30);
    assert_eq!(decoded.state, State::Off);
    assert_eq!(decoded.ports[0].state, State::On);
    assert_eq!(decoded.ports[1].state, State::Off);
    assert_eq!(decoded.vector_group.clock, 1);
    assert!((decoded.additional_rotation_rad - 5.0 * PI / 6.0).abs() < 1e-15);
    assert_eq!(decoded.neutral_points, [Some(12), None]);
    assert_close(decoded.rated_ll_volts, [11_000.0, 400.0]);
    assert!((decoded.rated_va - 100_000.0).abs() < 1e-10);
    assert_eq!(decoded.tap.side, 0);
    assert_close(decoded.tap.positions.map(Option::unwrap), [2.0; 3]);
    assert!((decoded.tap.midpoint - 1.0).abs() < 1e-15);
    assert!((decoded.tap.step_fraction - 0.025).abs() < 1e-15);
    assert!((decoded.tap.boost_angle_rad - PI / 2.0).abs() < 1e-15);
    assert!((decoded.tap.rotation_per_step_rad + PI / 60.0).abs() < 1e-15);
    let bytes = transformer_database(
        "UPDATE TwoWindingTransformer SET Flag_Tap=1, Flag_ConNode=2,
             roh=NULL, roh1=1, roh2=2, roh3=3;",
    );
    let decoded = NativeDatabase::decode(&bytes, None)
        .unwrap()
        .transformer_connection(30)
        .unwrap();
    assert_eq!(decoded.tap.side, 1);
    assert_close(decoded.tap.positions.map(Option::unwrap), [1.0, 2.0, 3.0]);
}

#[test]
fn transformer_connection_rejects_unresolved_or_ambiguous_inputs() {
    for edit in [
        "UPDATE TwoWindingTransformer SET VecGrp=74",
        "UPDATE TwoWindingTransformer SET VecGrp=2",
        "UPDATE TwoWindingTransformer SET Flag_Ct=1",
        "UPDATE TwoWindingTransformer SET TransformerTap_ID=4",
        "UPDATE TwoWindingTransformer SET Typ_ID=4",
        "UPDATE TwoWindingTransformer SET Flag_roh=2",
        "UPDATE TwoWindingTransformer SET Flag_Tap=1",
        "UPDATE TwoWindingTransformer SET Un1=0",
        "UPDATE TwoWindingTransformer SET Sn=1e308",
        "UPDATE TwoWindingTransformer SET AddRotate=NULL",
        "UPDATE TwoWindingTransformer SET Flag_ConNode=3",
        "UPDATE Terminal SET Flag_Terminal=8",
        "UPDATE Terminal SET Flag_Terminal=1 WHERE TerminalNo=1",
        "DELETE FROM TwoWindingTransformer",
        "INSERT INTO TwoWindingTransformer (Element_ID, Variant_ID) VALUES (30, 1)",
    ] {
        let decoded = NativeDatabase::decode(&transformer_database(edit), None).unwrap();
        assert!(
            decoded.transformer_connection(30).is_err(),
            "accepted {edit}"
        );
    }
}

#[test]
fn authentic_transformer_preserves_separate_rotation_and_nominal_bases() {
    let data = include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let source = Source::from_memory("case.sinx", &data[..]).unwrap();
    let decoded = read(&source, None)
        .unwrap()
        .transformer_connection(19)
        .unwrap();
    assert_eq!(decoded.vector_group.clock, 0);
    assert_close(decoded.rated_ll_volts, [20_000.0, 400.0]);
    assert!((decoded.rated_va - 160_000.0).abs() < 1e-10);
    assert!((decoded.additional_rotation_rad - 5.0 * PI / 6.0).abs() < 1e-15);
    assert_eq!(decoded.coils.len(), 3);
    assert_close(decoded.tap.positions.map(Option::unwrap), [0.0; 3]);
}

#[test]
fn transformer_taps_ignore_absent_winding_inputs() {
    let bytes = transformer_database(
        "UPDATE Terminal SET Flag_Terminal=1;
         UPDATE TwoWindingTransformer SET Flag_Tap=1, roh=NULL, roh1=3, roh2=NULL, roh3=NULL;",
    );
    let decoded = NativeDatabase::decode(&bytes, None)
        .unwrap()
        .transformer_connection(30)
        .unwrap();
    assert!((decoded.tap.positions[0].unwrap() - 3.0).abs() < 1e-15);
    assert_eq!(decoded.tap.positions[1..], [None, None]);
}

pub(super) fn impedance_database(edit: &str) -> Vec<u8> {
    transformer_database(&format!(
        "ALTER TABLE TwoWindingTransformer ADD COLUMN uk REAL DEFAULT 5;
         ALTER TABLE TwoWindingTransformer ADD COLUMN ur REAL DEFAULT 3;
         ALTER TABLE TwoWindingTransformer ADD COLUMN Vfe REAL DEFAULT 1;
         ALTER TABLE TwoWindingTransformer ADD COLUMN i0 REAL DEFAULT 2;
         ALTER TABLE TwoWindingTransformer ADD COLUMN Flag_Z0_Input INTEGER DEFAULT 2;
         ALTER TABLE TwoWindingTransformer ADD COLUMN R0 REAL DEFAULT 2;
         ALTER TABLE TwoWindingTransformer ADD COLUMN X0 REAL DEFAULT 3;
         ALTER TABLE TwoWindingTransformer ADD COLUMN Z0_Z1 REAL;
         ALTER TABLE TwoWindingTransformer ADD COLUMN R0_X0 REAL;
         ALTER TABLE TwoWindingTransformer ADD COLUMN R0_R1 REAL;
         ALTER TABLE TwoWindingTransformer ADD COLUMN X0_X1 REAL;
         ALTER TABLE TwoWindingTransformer ADD COLUMN ZABNL REAL;
         ALTER TABLE TwoWindingTransformer ADD COLUMN ZBANL REAL;
         ALTER TABLE TwoWindingTransformer ADD COLUMN ZABSC REAL;
         ALTER TABLE TwoWindingTransformer ADD COLUMN RX_ZABNL REAL;
         ALTER TABLE TwoWindingTransformer ADD COLUMN RX_ZBANL REAL;
         ALTER TABLE TwoWindingTransformer ADD COLUMN RX_ZABSC REAL;
         {edit}"
    ))
}

#[test]
fn transformer_nominal_parameters_use_secondary_voltage_and_total_power_bases() {
    let native = NativeDatabase::decode(&impedance_database(""), None).unwrap();
    let input = native.transformer_nominal(30).unwrap();
    assert_close(
        [input.series_secondary_ohm.re, input.series_secondary_ohm.im],
        [0.048, 0.064],
    );
    assert_close(
        [
            input.no_load_secondary_siemens.re,
            input.no_load_secondary_siemens.im,
        ],
        [1000.0 / 160_000.0, -(3e6_f64).sqrt() / 160_000.0],
    );
    assert_eq!(input.connection.vector_group.clock, 1);
    // The nominal parameters are independent of the stored nonzero fixed tap.
    assert!((input.connection.tap.positions[0].unwrap() - 2.0).abs() < 1e-15);
}

#[test]
fn transformer_zero_sequence_respects_active_input_mode_and_measurement_side() {
    use super::transformer_impedance::ZeroSequenceInput;
    for (edit, expected_side, expected) in [
        ("", 0, [2.0, 3.0]),
        ("UPDATE TwoWindingTransformer SET VecGrp=59", 1, [2.0, 3.0]),
        (
            "UPDATE TwoWindingTransformer SET Flag_Z0_Input=3, R0_R1=2, X0_X1=0.5, R0=NULL, X0=NULL",
            0,
            [72.6, 24.2],
        ),
        (
            "UPDATE TwoWindingTransformer SET Flag_Z0_Input=1, Z0_Z1=2, R0_X0=0.75, R0=NULL, X0=NULL",
            0,
            [72.6, 96.8],
        ),
    ] {
        let input = NativeDatabase::decode(&impedance_database(edit), None)
            .unwrap()
            .transformer_nominal(30)
            .unwrap();
        let ZeroSequenceInput::GroundedSide {
            side,
            impedance_ohm,
        } = input.zero_sequence
        else {
            panic!("expected one grounded side");
        };
        assert_eq!(side, expected_side);
        assert_close([impedance_ohm.re, impedance_ohm.im], expected);
    }
    let input = NativeDatabase::decode(
        &impedance_database(
            "UPDATE Element SET Flag_Input=2;
         UPDATE TwoWindingTransformer SET VecGrp=6, Flag_Z0_Input=NULL, R0=NULL, X0=NULL;",
        ),
        None,
    )
    .unwrap()
    .transformer_nominal(30)
    .unwrap();
    assert!(matches!(
        input.zero_sequence,
        ZeroSequenceInput::NoGroundPath
    ));
}

#[test]
fn both_grounded_transformer_keeps_open_short_measurement_bases_separate() {
    use super::transformer_impedance::ZeroSequenceInput;
    let input = NativeDatabase::decode(
        &impedance_database(
            "UPDATE TwoWindingTransformer SET VecGrp=5, Flag_Z0_Input=4,
             ZABNL=5, RX_ZABNL=0.75, ZBANL=10, RX_ZBANL=0, ZABSC=1, RX_ZABSC=0;",
        ),
        None,
    )
    .unwrap()
    .transformer_nominal(30)
    .unwrap();
    let ZeroSequenceInput::BothGrounded {
        open_primary_ohm,
        open_secondary_ohm,
        short_primary_ohm,
    } = input.zero_sequence
    else {
        panic!("expected both-grounded measurements");
    };
    assert_close([open_primary_ohm.re, open_primary_ohm.im], [3.0, 4.0]);
    assert_close([open_secondary_ohm.re, open_secondary_ohm.im], [0.0, 10.0]);
    assert_close([short_primary_ohm.re, short_primary_ohm.im], [0.0, 1.0]);
}

#[test]
fn transformer_nominal_rejects_impossible_or_missing_measurements() {
    for edit in [
        "UPDATE TwoWindingTransformer SET uk=2", // R cannot exceed |Z|.
        "UPDATE TwoWindingTransformer SET i0=0", // P cannot exceed |S|.
        "UPDATE TwoWindingTransformer SET Vfe=-1",
        "UPDATE TwoWindingTransformer SET R0=-1",
        "UPDATE TwoWindingTransformer SET Un2=1e152",
        "UPDATE TwoWindingTransformer SET Vfe=1e308",
        "UPDATE TwoWindingTransformer SET VecGrp=5", // Both-grounded requires mode 4.
        "UPDATE TwoWindingTransformer SET Flag_Z0_Input=1", // Active ratios absent.
        "UPDATE Element SET Flag_Input=2",           // Required zero-sequence category absent.
    ] {
        let native = NativeDatabase::decode(&impedance_database(edit), None).unwrap();
        assert!(native.transformer_nominal(30).is_err(), "accepted {edit}");
    }
}

#[test]
fn authentic_nominal_transformer_matches_independent_rated_data() {
    let data = include_bytes!("../../../tests/data/sincal/1-LV-rural1--0-sw.sinx");
    let source = Source::from_memory("case.sinx", &data[..]).unwrap();
    let input = read(&source, None)
        .unwrap()
        .transformer_nominal(19)
        .unwrap();
    // Paired SimBench CSV: 160 kVA, 400 V, 4% uk, 2.35 kW copper,
    // 0.46 kW core loss and 0.28751% no-load current.
    assert!((input.series_secondary_ohm.re - 2350.0 / 160_000.0).abs() < 1e-15);
    assert!((input.series_secondary_ohm.norm() - 0.04).abs() < 1e-15);
    assert!((input.no_load_secondary_siemens.re - 460.0 / 160_000.0).abs() < 1e-15);
    assert!((input.no_load_secondary_siemens.norm() - 0.002_875_1).abs() < 1e-15);
}

#[test]
fn nominal_core_boundary_allows_only_relative_floating_point_roundoff() {
    // Original synthetic nameplate: 10 kVA at 0.7% no-load current is
    // exactly 70 VA, equal to 0.07 kW core loss. Independent binary SI
    // conversions can put the apparent power one rounding step below P.
    let native = NativeDatabase::decode(
        &impedance_database("UPDATE TwoWindingTransformer SET Sn=0.01, i0=0.7, Vfe=0.07"),
        None,
    )
    .unwrap();
    let input = native.transformer_nominal(30).unwrap();
    assert_eq!(input.no_load_secondary_siemens.im.abs().to_bits(), 0);
    assert!((input.no_load_secondary_siemens.re - 70.0 / 160_000.0).abs() < 1e-15);

    // A small, but materially larger than roundoff, excess remains invalid.
    // Zero apparent power must not acquire an absolute near-zero allowance.
    for edit in [
        "UPDATE TwoWindingTransformer SET Sn=0.01, i0=0.7, Vfe=0.070000001",
        "UPDATE TwoWindingTransformer SET i0=0, Vfe=1e-300",
        "UPDATE TwoWindingTransformer SET Sn=0.001, i0=0.1, Vfe=0.002",
    ] {
        let native = NativeDatabase::decode(&impedance_database(edit), None).unwrap();
        assert!(native.transformer_nominal(30).is_err(), "accepted {edit}");
    }

    // Preserve a genuine small reactive component on the valid side.
    let native = NativeDatabase::decode(
        &impedance_database("UPDATE TwoWindingTransformer SET Sn=0.01, i0=0.700000001, Vfe=0.07"),
        None,
    )
    .unwrap();
    assert!(
        native
            .transformer_nominal(30)
            .unwrap()
            .no_load_secondary_siemens
            .im
            < 0.0
    );
}

#[test]
fn nominal_transformer_conflicts_identify_the_active_nameplate_fields() {
    for (edit, expected) in [
        (
            "UPDATE TwoWindingTransformer SET uk=2,ur=3",
            "short-circuit resistance ur exceeds magnitude uk",
        ),
        (
            "UPDATE TwoWindingTransformer SET Sn=0.01,i0=0.1,Vfe=0.02",
            "no-load core loss Vfe exceeds apparent power from i0 and Sn",
        ),
    ] {
        let native = NativeDatabase::decode(&impedance_database(edit), None).unwrap();
        let error = match native.transformer_nominal(30) {
            Ok(_) => panic!("accepted inconsistent nameplate: {edit}"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains(expected), "{error}");
    }
}
