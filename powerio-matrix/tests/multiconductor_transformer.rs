//! The finite leakage transformer primitive against frozen direct OpenDSS
//! references (OpenDSSDirect.py 0.9.4, DSS-Python 0.15.7, C-API backend
//! 0.14.5), BMOPFTools' winding primitives, and its structural invariants.
// Hand checks index square arrays by both loop variables on purpose.
#![allow(clippy::needless_range_loop)]
// Frozen references are copied verbatim from the OpenDSS output.
#![allow(clippy::unreadable_literal)]

use num_complex::Complex64;
use powerio_dist::{DistBus, DistTransformer, DistWinding, DistWindingConn, MulticonductorNetwork};
use powerio_matrix::TransformerPrimitive;
use serde_json::Value;

/// The primitive over a network that declares each winding bus with the
/// terminals the windings name, in first named order.
fn calc_transformer_primitive_admittance(
    transformer: &DistTransformer,
) -> powerio_matrix::Result<TransformerPrimitive<'_>> {
    let mut network = MulticonductorNetwork::default();
    for winding in &transformer.windings {
        let buses = network.buses_mut();
        let index = buses
            .iter()
            .position(|bus| bus.id == winding.bus)
            .unwrap_or_else(|| {
                buses.push(DistBus::new(winding.bus.clone(), Vec::new()));
                buses.len() - 1
            });
        for terminal in &winding.terminal_map {
            if !buses[index].terminals.contains(terminal) {
                buses[index].terminals.push(terminal.clone());
            }
        }
    }
    powerio_matrix::calc_transformer_primitive_admittance(&network, transformer)
        .map_err(powerio_matrix::Error::from)
}

fn assert_unsupported<T: std::fmt::Debug>(result: powerio_matrix::Result<T>) {
    let error = result.unwrap_err();
    assert_eq!(
        error.code().code,
        "BUILD.DIST.TRANSFORMER_UNSUPPORTED",
        "{error}"
    );
}

fn transformer(conn1: DistWindingConn, conn2: DistWindingConn) -> DistTransformer {
    let mut w1 = DistWinding::new("hv", vec!["1".into()], conn1, 12_470.0, 300_000.0);
    w1.terminal_map = if conn1 == DistWindingConn::Wye {
        vec!["1".into(), "2".into(), "3".into(), "0".into()]
    } else {
        vec!["1".into(), "2".into(), "3".into()]
    };
    w1.r_pct = 0.5;
    let mut w2 = DistWinding::new("lv", vec!["1".into()], conn2, 480.0, 300_000.0);
    w2.terminal_map = if conn2 == DistWindingConn::Wye {
        vec!["1".into(), "2".into(), "3".into(), "0".into()]
    } else {
        vec!["1".into(), "2".into(), "3".into()]
    };
    w2.r_pct = 0.5;
    DistTransformer::new("t", vec![w1, w2], vec![6.0], 3)
}

fn dss_golden_transformer(conn1: DistWindingConn, conn2: DistWindingConn) -> DistTransformer {
    let mut t = transformer(conn1, conn2);
    for winding in &mut t.windings {
        winding.tap = if winding.bus == "hv" { 1.03 } else { 0.97 };
    }
    t
}

fn dss_golden_n_winding(roll1: i8, roll2: i8) -> DistTransformer {
    let mut t = dss_golden_transformer(DistWindingConn::Delta, DistWindingConn::Delta);
    t.extras
        .insert("bmopf_subtype".into(), Value::String("n_winding".into()));
    t.extras.insert(
        "bmopf_delta_rolls".into(),
        serde_json::json!({"1": roll1, "2": roll2}),
    );
    t
}

fn autotransformer(regulator_type: &str, tap_ratio: f64) -> DistTransformer {
    let from = DistWinding::new(
        "src",
        vec!["p".into(), "n".into()],
        DistWindingConn::Wye,
        2_400.0,
        500_000.0,
    );
    let mut to = DistWinding::new(
        "reg",
        vec!["p".into(), "n".into()],
        DistWindingConn::Wye,
        2_400.0,
        500_000.0,
    );
    to.tap = tap_ratio;
    let mut transformer = DistTransformer::new("reg", vec![from, to], vec![0.0], 1);
    transformer.extras.insert(
        "bmopf_subtype".into(),
        Value::String("single_phase_autotransformer".into()),
    );
    transformer.extras.insert(
        "regulator_type".into(),
        Value::String(regulator_type.into()),
    );
    transformer
        .extras
        .insert("r_series_from".into(), serde_json::json!(0.5));
    transformer
        .extras
        .insert("x_series_from".into(), serde_json::json!(2.0));
    transformer
        .extras
        .insert("r_series_to".into(), serde_json::json!(0.25));
    transformer
        .extras
        .insert("x_series_to".into(), serde_json::json!(0.5));
    transformer
}

fn assert_entry(p: &TransformerPrimitive<'_>, row: usize, col: usize, re: f64, im: f64) {
    let got = p.primitive[row][col];
    let expected = Complex64::new(re, im);
    assert!(
        (got - expected).norm() < 1.0e-11,
        "Y[{row},{col}] = {got:?}, expected {expected:?}"
    );
}

// Frozen direct OpenDSS references. Generated with OpenDSSDirect.py 0.9.4,
// DSS-Python 0.15.7, C-API backend 0.14.5 (SVN 3723), using:
// phases=3, buses=[hv lv], kvs=[12.47 .48], kvas=[300 300],
// %Rs=[.5 .5], XHL=6, taps=[1.03 .97], %noloadloss=0, %imag=0,
// ppm_antifloat=0 and RNeut=-1 on both windings.  The reference keys
// are the terminal order used by these tests. Full matrices and reports
// are recorded under docs/evidence/mc-pf-yprim; these entries pin each
// connection's actual admittance
// values, including the delta phase incidence and tap scaling.
#[test]
fn direct_dss_golden_yy_with_fixed_taps() {
    let p_source = dss_golden_transformer(DistWindingConn::Wye, DistWindingConn::Wye);
    let p = calc_transformer_primitive_admittance(&p_source).unwrap();
    assert_entry(&p, 0, 0, 0.004914871575352249, -0.029489229452113495);
    assert_entry(&p, 0, 4, -0.13558226374781318, 0.8134935824868792);
    assert_entry(&p, 3, 3, 0.014744614726056746, -0.08846768835634049);
    assert_entry(&p, 4, 4, 3.7401893337699468, -22.441136002619682);
    assert_entry(&p, 4, 7, -3.7401893337699468, 22.441136002619682);
    assert_entry(&p, 7, 7, 11.22056800130984, -67.32340800785904);
}

#[test]
fn direct_dss_golden_dd_with_fixed_taps() {
    let p_source = dss_golden_transformer(DistWindingConn::Delta, DistWindingConn::Delta);
    let p = calc_transformer_primitive_admittance(&p_source).unwrap();
    assert_entry(&p, 0, 0, 0.0032765810502348334, -0.019659486301409002);
    assert_entry(&p, 0, 1, -0.0016382905251174167, 0.009829743150704501);
    assert_entry(&p, 0, 3, -0.09038817583187549, 0.542329054991253);
    assert_entry(&p, 0, 4, 0.04519408791593774, -0.2711645274956265);
    assert_entry(&p, 3, 3, 2.493459555846632, -14.960757335079794);
    assert_entry(&p, 3, 4, -1.246729777923316, 7.480378667539897);
}

#[test]
fn direct_dss_golden_yd_with_fixed_taps() {
    let p_source = dss_golden_transformer(DistWindingConn::Wye, DistWindingConn::Delta);
    let p = calc_transformer_primitive_admittance(&p_source).unwrap();
    assert_entry(&p, 0, 0, 0.004914871575352249, -0.029489229452113495);
    assert_entry(&p, 0, 3, -0.004914871575352249, 0.029489229452113495);
    assert_entry(&p, 0, 4, -0.0782784564721388, 0.4696707388328328);
    assert_entry(&p, 3, 3, 0.014744614726056746, -0.08846768835634049);
    assert_entry(&p, 4, 4, 2.493459555846632, -14.960757335079794);
    assert_entry(&p, 4, 5, -1.246729777923316, 7.480378667539897);
}

#[test]
fn direct_dss_golden_dy_with_fixed_taps() {
    let p_source = dss_golden_transformer(DistWindingConn::Delta, DistWindingConn::Wye);
    let p = calc_transformer_primitive_admittance(&p_source).unwrap();
    assert_entry(&p, 0, 0, 0.0032765810502348334, -0.019659486301409002);
    assert_entry(&p, 0, 1, -0.0016382905251174167, 0.009829743150704501);
    assert_entry(&p, 0, 3, -0.0782784564721388, 0.4696707388328328);
    assert_entry(&p, 0, 4, 0.0782784564721388, -0.4696707388328328);
    assert_entry(&p, 3, 3, 3.7401893337699468, -22.441136002619682);
    assert_entry(&p, 3, 6, -3.7401893337699468, 22.441136002619682);
}

#[test]
fn direct_dss_golden_n_winding_rolls_pin_each_delta_incidence() {
    let plus_minus_source = dss_golden_n_winding(1, -1);
    let plus_minus = calc_transformer_primitive_admittance(&plus_minus_source).unwrap();
    assert_entry(
        &plus_minus,
        0,
        0,
        0.0032765810502348334,
        -0.019659486301409002,
    );
    assert_entry(&plus_minus, 0, 3, -0.04519408791593774, 0.2711645274956265);
    assert_entry(&plus_minus, 0, 4, -0.04519408791593774, 0.2711645274956265);
    assert_entry(&plus_minus, 1, 3, 0.09038817583187549, -0.542329054991253);
    let minus_plus_source = dss_golden_n_winding(-1, 1);
    let minus_plus = calc_transformer_primitive_admittance(&minus_plus_source).unwrap();
    assert_entry(&minus_plus, 0, 3, -0.04519408791593774, 0.2711645274956265);
    assert_entry(&minus_plus, 0, 4, 0.09038817583187549, -0.542329054991253);
    assert_entry(&minus_plus, 1, 3, -0.04519408791593774, 0.2711645274956265);
}

#[test]
fn type_b_autotransformer_matches_bmopf_winding_primitive() {
    let tap = 1.05;
    let n_eff = 1.0 / tap;
    let p_source = autotransformer("B", tap);
    let p = calc_transformer_primitive_admittance(&p_source).unwrap();
    let z = Complex64::new(0.5 + n_eff * n_eff * 0.25, 2.0 + n_eff * n_eff * 0.5);
    let y = z.finv();
    assert_eq!(p.terminals.len(), 4);
    assert!((p.primitive[0][0] - y).norm() < 1e-12);
    assert!((p.primitive[0][1] + n_eff * y).norm() < 1e-12);
    assert!((p.primitive[1][1] - n_eff * n_eff * y).norm() < 1e-12);
    for row in &p.primitive {
        assert!(row.iter().copied().sum::<Complex64>().norm() < 1e-12);
    }
}

#[test]
fn type_a_autotransformer_reads_its_ratio_as_type_b_does() {
    let tap = 1.05;
    let a_source = autotransformer("A", tap);
    let a = calc_transformer_primitive_admittance(&a_source).unwrap();
    let b_source = autotransformer("B", tap);
    let b = calc_transformer_primitive_admittance(&b_source).unwrap();
    assert_eq!(a.primitive, b.primitive);
    assert_eq!(a.shunt, b.shunt);
}

#[test]
fn autotransformer_no_load_shunt_spans_the_from_winding() {
    let mut transformer = autotransformer("B", 1.05);
    transformer
        .extras
        .insert("g_no_load".into(), serde_json::json!(0.001));
    transformer
        .extras
        .insert("b_no_load".into(), serde_json::json!(-0.002));
    let p = calc_transformer_primitive_admittance(&transformer).unwrap();
    let shunt = Complex64::new(0.001, -0.002);
    assert!((p.shunt[0][0] - shunt).norm() < 1e-14);
    assert!((p.shunt[0][2] + shunt).norm() < 1e-14);
    assert!((p.shunt[2][0] + shunt).norm() < 1e-14);
    assert!((p.shunt[2][2] - shunt).norm() < 1e-14);
}

#[test]
fn ideal_autotransformer_is_rejected_as_a_constraint() {
    let mut transformer = autotransformer("B", 1.05);
    for key in [
        "r_series_from",
        "x_series_from",
        "r_series_to",
        "x_series_to",
    ] {
        transformer
            .extras
            .insert(key.into(), serde_json::json!(0.0));
    }
    assert_unsupported(calc_transformer_primitive_admittance(&transformer));
}

#[test]
fn yy_has_zero_sum_rows_and_finite_leakage() {
    let p_source = transformer(DistWindingConn::Wye, DistWindingConn::Wye);
    let p = calc_transformer_primitive_admittance(&p_source).unwrap();
    assert_eq!(p.terminals.len(), 8);
    for row in &p.primitive {
        let sum: Complex64 = row.iter().copied().sum();
        assert!(sum.norm() < 1e-12, "row sum {sum:?}");
    }
    assert!(
        p.primitive
            .iter()
            .flatten()
            .all(|z| z.re.is_finite() && z.im.is_finite())
    );
}

#[test]
fn mixed_connection_couples_delta_phases() {
    let p_source = transformer(DistWindingConn::Wye, DistWindingConn::Delta);
    let p = calc_transformer_primitive_admittance(&p_source).unwrap();
    assert_eq!(p.delta_direction, 1);
    // Delta terminal 1 is coupled to terminal 2 through its coil.
    let i = p.terminals.iter().position(|t| *t == ("lv", "1")).unwrap();
    let j = p.terminals.iter().position(|t| *t == ("lv", "2")).unwrap();
    assert!(p.primitive[i][j].norm() > 0.0);
}

#[test]
fn all_two_winding_connection_pairs_have_physical_finite_yprim() {
    for (from, to) in [
        (DistWindingConn::Wye, DistWindingConn::Wye),
        (DistWindingConn::Wye, DistWindingConn::Delta),
        (DistWindingConn::Delta, DistWindingConn::Wye),
        (DistWindingConn::Delta, DistWindingConn::Delta),
    ] {
        let primitive_source = transformer(from, to);
        let primitive = calc_transformer_primitive_admittance(&primitive_source).unwrap();
        assert!(
            primitive
                .primitive
                .iter()
                .flatten()
                .all(|z| z.re.is_finite() && z.im.is_finite())
        );
        for row in &primitive.primitive {
            let sum: Complex64 = row.iter().copied().sum();
            assert!(sum.norm() < 1e-10, "{from:?}/{to:?} row sum {sum:?}");
        }
    }
}

#[test]
fn two_winding_n_winding_uses_each_delta_roll_in_the_coil_map() {
    let mut opposite = transformer(DistWindingConn::Delta, DistWindingConn::Delta);
    opposite
        .extras
        .insert("bmopf_subtype".into(), Value::String("n_winding".into()));
    opposite.extras.insert(
        "bmopf_delta_rolls".into(),
        serde_json::json!({"1": 1, "2": -1}),
    );
    let mut aligned = opposite.clone();
    aligned.extras.insert(
        "bmopf_delta_rolls".into(),
        serde_json::json!({"1": 1, "2": 1}),
    );
    let p_opposite = calc_transformer_primitive_admittance(&opposite).unwrap();
    let p_aligned = calc_transformer_primitive_admittance(&aligned).unwrap();
    assert!(
        p_opposite
            .primitive
            .iter()
            .zip(&p_aligned.primitive)
            .flat_map(|(a, b)| a.iter().zip(b))
            .any(|(a, b)| (*a - *b).norm() > 1e-12)
    );
}

#[test]
fn ideal_transformer_is_rejected() {
    let mut t = transformer(DistWindingConn::Wye, DistWindingConn::Wye);
    t.xsc_pct[0] = 0.0;
    t.windings.iter_mut().for_each(|w| w.r_pct = 0.0);
    assert_unsupported(calc_transformer_primitive_admittance(&t));
}

#[test]
fn no_load_shunt_is_stamped_per_delta_coil() {
    let mut t = transformer(DistWindingConn::Wye, DistWindingConn::Delta);
    t.extras.insert(
        "no_load_shunt".into(),
        serde_json::json!({"winding": 2, "g": 0.001, "b": -0.002}),
    );
    let p = calc_transformer_primitive_admittance(&t).unwrap();
    let lv: Vec<_> = p
        .terminals
        .iter()
        .enumerate()
        .filter(|(_, t)| t.0 == "lv")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(lv.len(), 3);
    for &i in &lv {
        assert!((p.shunt[i][i] - Complex64::new(0.002, -0.004)).norm() < 1e-14);
    }
    for a in 0..3 {
        let i = lv[a];
        let j = lv[(a + 1) % 3];
        assert!((p.shunt[i][j] - Complex64::new(-0.001, 0.002)).norm() < 1e-14);
    }
}

#[test]
fn unequal_winding_ratings_are_referred_to_winding_one_base() {
    let mut t = transformer(DistWindingConn::Wye, DistWindingConn::Wye);
    t.windings[1].s_rating = 600_000.0;
    let p = calc_transformer_primitive_admittance(&t).unwrap();
    let z = Complex64::new(1.0, 0.0) / p.winding_admittance[0][0];
    let expected = Complex64::new(
        3.0 / 300_000.0 * (0.005 + 0.005 * 300_000.0 / 600_000.0),
        3.0 / 300_000.0 * 0.06,
    );
    assert!((z - expected).norm() < 1e-14, "{z:?} != {expected:?}");
}

fn one_phase_transformer() -> DistTransformer {
    let mut w1 = DistWinding::new(
        "hv",
        vec!["p".into(), "n".into()],
        DistWindingConn::Wye,
        100.0,
        1_000.0,
    );
    w1.r_pct = 1.0;
    let mut w2 = DistWinding::new(
        "lv",
        vec!["p".into(), "n".into()],
        DistWindingConn::Wye,
        10.0,
        1_000.0,
    );
    w2.r_pct = 1.0;
    DistTransformer::new("one", vec![w1, w2], vec![0.0], 1)
}

fn center_tap_transformer() -> DistTransformer {
    let mut primary = DistWinding::new(
        "mv",
        vec!["1".into(), "2".into()],
        DistWindingConn::Wye,
        11_000.0,
        25_000.0,
    );
    primary.r_pct = 0.5;
    let mut leg_1 = DistWinding::new(
        "lv",
        vec!["1".into(), "4".into()],
        DistWindingConn::Wye,
        240.0,
        25_000.0,
    );
    leg_1.r_pct = 0.5;
    let mut leg_2 = DistWinding::new(
        "lv",
        vec!["4".into(), "2".into()],
        DistWindingConn::Wye,
        240.0,
        25_000.0,
    );
    leg_2.r_pct = 0.5;
    let mut transformer =
        DistTransformer::new("split", vec![primary, leg_1, leg_2], vec![4.0, 4.0, 4.0], 1);
    transformer
        .extras
        .insert("bmopf_subtype".into(), Value::String("center_tap".into()));
    transformer
}

#[test]
fn center_tap_has_five_terminal_coupled_primitive_and_series_aiding_legs() {
    let p_source = center_tap_transformer();
    let p = calc_transformer_primitive_admittance(&p_source).unwrap();
    let terminal_keys = p.terminals.clone();
    assert_eq!(
        terminal_keys,
        vec![
            ("mv", "1"),
            ("mv", "2"),
            ("lv", "1"),
            ("lv", "4"),
            ("lv", "2")
        ]
    );
    assert_eq!(p.winding_admittance.len(), 3);
    for row in &p.primitive {
        assert!(row.iter().copied().sum::<Complex64>().norm() < 1e-10);
    }

    // The no-current voltage relationship is +11 kV across the primary,
    // +240 V from leg 1 to centre, and +240 V from centre to leg 2. Thus
    // the two outer secondary terminals are 180 degrees apart.
    let voltage = [
        Complex64::new(11_000.0, 0.0),
        Complex64::default(),
        Complex64::new(240.0, 0.0),
        Complex64::default(),
        Complex64::new(-240.0, 0.0),
    ];
    for row in &p.series {
        let current: Complex64 = row.iter().zip(voltage).map(|(y, v)| *y * v).sum();
        assert!(current.norm() < 1e-9, "no-load current {current:?}");
    }
}

#[test]
fn center_tap_supports_fixed_tap_excitation_and_neutral_impedance() {
    let mut transformer = center_tap_transformer();
    transformer.windings[0].tap = 1.05;
    transformer.windings[1].r_neutral = Some(5.0);
    transformer.windings[1].x_neutral = Some(1.0);
    transformer.extras.insert(
        "no_load_shunt".into(),
        serde_json::json!({"winding": 2, "g": 0.001, "b": -0.002}),
    );
    let p = calc_transformer_primitive_admittance(&transformer).unwrap();
    let primary = p
        .terminals
        .iter()
        .position(|terminal| *terminal == ("mv", "1"))
        .unwrap();
    let center = p
        .terminals
        .iter()
        .position(|terminal| *terminal == ("lv", "4"))
        .unwrap();
    let untapped_source = center_tap_transformer();
    let untapped = calc_transformer_primitive_admittance(&untapped_source).unwrap();
    assert!((p.series[primary][primary] - untapped.series[primary][primary]).norm() > 1e-8);
    let mut ungrounded = transformer.clone();
    ungrounded.windings[1].r_neutral = None;
    ungrounded.windings[1].x_neutral = None;
    let ungrounded = calc_transformer_primitive_admittance(&ungrounded).unwrap();
    assert!(
        (p.series[center][center]
            - ungrounded.series[center][center]
            - Complex64::new(5.0, 1.0).finv())
        .norm()
            < 1e-14
    );
    let leg_1 = p
        .terminals
        .iter()
        .position(|terminal| *terminal == ("lv", "1"))
        .unwrap();
    assert!((p.shunt[leg_1][center] - Complex64::new(-0.001, 0.002)).norm() < 1e-14);

    let floating_source = center_tap_transformer();
    let floating = calc_transformer_primitive_admittance(&floating_source).unwrap();
    assert_eq!(floating.grounded, []);
    let mut solid = center_tap_transformer();
    solid.windings[1].r_neutral = Some(0.0);
    solid.windings[1].x_neutral = Some(0.0);
    let solid = calc_transformer_primitive_admittance(&solid).unwrap();
    assert!(solid.grounded.contains(&("lv", "4")));
}

#[test]
fn general_three_winding_transformer_does_not_require_center_tap_marker() {
    let mut transformer = center_tap_transformer();
    transformer.extras.clear();
    let primitive = calc_transformer_primitive_admittance(&transformer).unwrap();
    assert_eq!(primitive.terminals.len(), 5);
    assert_eq!(primitive.winding_admittance.len(), 3);
}

#[test]
fn center_tap_fixed_tap_and_excitation_match_direct_opendss_yprim() {
    // Frozen from BMOPFTools' transformer_interoperability/center_tap.dss
    // using OpenDSSDirect.py 0.9.4, with winding-1 tap=1.06,
    // %noloadloss=.2, %imag=.4, and ppm_antifloat=0. Repeated centre-tap
    // rows are aggregated into the five physical PowerIO terminals.
    let mut transformer = center_tap_transformer();
    transformer.name = "tx".into();
    transformer.windings[0].bus = "f".into();
    transformer.windings[0].terminal_map = vec!["1".into(), "4".into()];
    transformer.windings[0].v_ref = 240.0;
    transformer.windings[0].s_rating = 10_000.0;
    transformer.windings[0].r_pct = 1.0;
    transformer.windings[0].tap = 1.06;
    for winding in &mut transformer.windings[1..] {
        winding.bus = "t".into();
        winding.v_ref = 120.0;
        winding.s_rating = 10_000.0;
        winding.r_pct = 1.0;
    }
    transformer.xsc_pct = vec![4.0, 4.0, 4.0];
    transformer.extras.insert(
        "no_load_shunt".into(),
        serde_json::json!({
            "winding": 2,
            "g": 0.001388888888888889,
            "b": -0.002777777777777778
        }),
    );
    let p = calc_transformer_primitive_admittance(&transformer).unwrap();
    assert_entry(&p, 0, 0, 2.0601769444774067, -4.1203538889548135);
    assert_entry(&p, 0, 2, -2.1837875611460507, 4.367575122292101);
    assert_entry(&p, 0, 4, 2.183787561146052, -4.367575122292104);
    assert_entry(&p, 2, 2, 9.260648148148144, -18.52129629629629);
    assert_entry(&p, 2, 3, -13.890277777777772, 27.780555555555544);
    assert_entry(&p, 3, 3, 27.779166666666658, -55.558333333333316);
    assert_entry(&p, 4, 4, 9.259259259259258, -18.518518518518515);
}

#[test]
fn one_phase_common_base_matches_direct_oracle() {
    let p_source = one_phase_transformer();
    let p = calc_transformer_primitive_admittance(&p_source).unwrap();
    let hv_p = p.terminals.iter().position(|x| *x == ("hv", "p")).unwrap();
    let hv_n = p.terminals.iter().position(|x| *x == ("hv", "n")).unwrap();
    let lv_p = p.terminals.iter().position(|x| *x == ("lv", "p")).unwrap();
    let lv_n = p.terminals.iter().position(|x| *x == ("lv", "n")).unwrap();
    assert!((p.primitive[hv_p][hv_p] - Complex64::new(5.0, 0.0)).norm() < 1e-12);
    assert!((p.primitive[hv_p][lv_p] + Complex64::new(5.0e1, 0.0)).norm() < 1e-12);
    assert!((p.primitive[lv_p][lv_p] - Complex64::new(5.0e2, 0.0)).norm() < 1e-10);
    assert!((p.primitive[hv_p][hv_n] + Complex64::new(5.0, 0.0)).norm() < 1e-12);
    assert!((p.primitive[lv_p][lv_n] + Complex64::new(5.0e2, 0.0)).norm() < 1e-10);
}

#[test]
fn fixed_tap_scales_the_referred_terminal_matrix() {
    let mut t = one_phase_transformer();
    t.windings[1].tap = 2.0;
    let p = calc_transformer_primitive_admittance(&t).unwrap();
    let hv_p = p.terminals.iter().position(|x| *x == ("hv", "p")).unwrap();
    let lv_p = p.terminals.iter().position(|x| *x == ("lv", "p")).unwrap();
    assert!((p.primitive[hv_p][hv_p] - Complex64::new(5.0, 0.0)).norm() < 1e-12);
    assert!((p.primitive[hv_p][lv_p] + Complex64::new(2.5e1, 0.0)).norm() < 1e-12);
    assert!((p.primitive[lv_p][lv_p] - Complex64::new(1.25e2, 0.0)).norm() < 1e-10);
}

#[test]
fn solid_neutral_is_returned_as_an_exact_ground_constraint() {
    let mut t = one_phase_transformer();
    t.windings[0].r_neutral = Some(0.0);
    t.windings[0].x_neutral = Some(0.0);
    let p = calc_transformer_primitive_admittance(&t).unwrap();
    assert!(p.grounded.contains(&("hv", "n")));
}

#[test]
fn wye_phase_permutation_keeps_the_declared_last_neutral_physical() {
    let mut t = transformer(DistWindingConn::Wye, DistWindingConn::Wye);
    t.windings[0].terminal_map = vec!["b".into(), "c".into(), "a".into(), "n".into()];
    t.windings[1].terminal_map = vec!["c".into(), "a".into(), "b".into(), "m".into()];
    t.windings[0].r_neutral = Some(0.0);
    t.windings[0].x_neutral = Some(0.0);
    let p = calc_transformer_primitive_admittance(&t).unwrap();
    let hv_b = p.terminals.iter().position(|x| *x == ("hv", "b")).unwrap();
    let lv_c = p.terminals.iter().position(|x| *x == ("lv", "c")).unwrap();
    assert!(
        (p.primitive[hv_b][lv_c] - Complex64::new(-0.13546023971044013, 0.8127614382626409)).norm()
            < 1e-12,
        "got {:?}",
        p.primitive[hv_b][lv_c]
    );
    assert!(p.grounded.contains(&("hv", "n")));
}

#[test]
fn negative_neutral_resistance_is_the_floating_sentinel() {
    let mut t = one_phase_transformer();
    t.windings[0].r_neutral = Some(-1.0);
    t.windings[0].x_neutral = Some(0.0);
    let p = calc_transformer_primitive_admittance(&t).unwrap();
    let neutral = p.terminals.iter().position(|x| *x == ("hv", "n")).unwrap();
    assert!(p.series[neutral][neutral].norm() < 1.0e5);
}
