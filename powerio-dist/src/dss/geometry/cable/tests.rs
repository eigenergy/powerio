use super::*;

fn geometry() -> Geometry {
    Geometry {
        wires: (0..4)
            .map(|i| {
                let j = f64::from(i);
                super::super::Wire {
                    x: j * 0.1,
                    height: -1.0 - 0.05 * f64::from(i % 2),
                    radius: 0.008 + j * 0.0005,
                    capacitance_radius: 0.01 + j * 0.0005,
                    gmr: 0.006 + j * 0.0003,
                    rdc: 0.0002 * (1.0 + j / 5.0),
                    rac: 0.00021 * (1.0 + j / 5.0),
                    cable: (i < 3).then_some(Data {
                        screen: Screen::ConcentricNeutral {
                            k: (13 + i) as usize,
                            diastrand: 0.0015,
                            gmrstrand: 0.0006,
                            rstrand: 0.0008,
                        },
                        epsr: 2.3 + j * 0.1,
                        inslayer: 0.004,
                        diains: 0.026,
                        diacable: Some(0.031),
                    }),
                }
            })
            .collect(),
        phases: 3,
        reduce: true,
        reduction_order: None,
        earth_model: EarthModel::Carson,
        rho: 37.0,
    }
}

#[test]
fn cable_and_bare_neutral_reduction_matches_pinned_native_values() {
    // DSS C-API 87d85c2, dss_cn_geometry_oracle.py carson-60.0-4-3-True.
    let matrices = calculate(&geometry(), 60.0).unwrap();
    let r = [
        0.000_265_503_608_882_421_4,
        3.519_501_778_096_733e-6,
        6.498_786_398_968_169e-7,
        3.519_501_778_096_733e-6,
        0.000_302_151_139_117_561_83,
        2.374_709_548_094_71e-6,
        6.498_786_398_968_169e-7,
        2.374_709_548_094_71e-6,
        0.000_341_845_252_252_206_93,
    ];
    let x = [
        8.428_299_081_358_696e-5,
        -5.959_678_010_096_394e-6,
        -3.215_202_146_906_68e-6,
        -5.959_678_010_096_394e-6,
        8.041_560_706_820_995e-5,
        -5.162_644_883_651_848e-6,
        -3.215_202_146_906_68e-6,
        -5.162_644_883_651_848e-6,
        7.349_568_921_535_918e-5,
    ];
    for (actual, expected) in matrices
        .r
        .iter()
        .flatten()
        .zip(r)
        .chain(matrices.x.iter().flatten().zip(x))
    {
        assert!((actual - expected).abs() < 1e-15);
    }
    // To a few ulps: an engine constant (OpenDSS's 2π is 6.283185307)
    // shows at 3e-11.
    assert!((matrices.b[0][0] / 1.311_763_202_020_904_5e-7 - 1.0).abs() < 1e-14);
    assert_eq!(matrices.b[0][1].to_bits(), 0.0_f64.to_bits());
    let mut unreduced = geometry();
    unreduced.reduce = false;
    let matrices = calculate(&unreduced, 60.0).unwrap();
    assert_eq!(matrices.r.len(), 4);
    assert_eq!(matrices.b[3], vec![0.0; 4]);
}

#[test]
fn incomplete_and_mixed_phase_cables_are_explicitly_refused() {
    let mut input = geometry();
    input.wires[0].cable.as_mut().unwrap().diains = 0.001;
    assert!(calculate(&input, 60.0).unwrap_err().contains("incomplete"));
    input = geometry();
    input.wires[0].cable = None;
    assert!(
        calculate(&input, 60.0)
            .unwrap_err()
            .contains("all phase positions")
    );
    input = geometry();
    input.wires[0].height = f64::NAN;
    assert!(calculate(&input, 60.0).is_err());
}

/// IEEE 13 node assets geometry 607: a 1/0 tape-shielded cable with a bare
/// 1/0 copper neutral 3 in away, both 4 ft underground, in SI units.
fn tape_shield_geometry() -> Geometry {
    const IN: f64 = 0.0254;
    const MI: f64 = 1609.344;
    let ts_rac = 0.97 / MI;
    let cu_rac = 0.607 / MI;
    Geometry {
        wires: vec![
            super::super::Wire {
                cable: Some(Data {
                    screen: Screen::TapeShield {
                        diashield: 0.88 * IN,
                        tapelayer: 0.005 * IN,
                        tapelap: 20.0,
                    },
                    epsr: 2.3,
                    inslayer: 0.220 * IN,
                    diains: 0.82 * IN,
                    diacable: Some(1.06 * IN),
                }),
                x: 0.0,
                height: -4.0 * 0.3048,
                radius: 0.368 / 2.0 * IN,
                capacitance_radius: 0.368 / 2.0 * IN,
                gmr: 0.13320 * IN,
                rdc: ts_rac / 1.02,
                rac: ts_rac,
            },
            super::super::Wire {
                cable: None,
                x: 0.25 * 0.3048,
                height: -4.0 * 0.3048,
                radius: 0.368 / 2.0 * IN,
                capacitance_radius: 0.368 / 2.0 * IN,
                gmr: 0.13356 * IN,
                rdc: cu_rac / 1.02,
                rac: cu_rac,
            },
        ],
        phases: 1,
        reduce: true,
        reduction_order: None,
        earth_model: EarthModel::Deri,
        rho: 100.0,
    }
}

#[test]
fn tape_shield_cable_matches_opendss() {
    // OpenDSS (DSS C-API 0.14.5) Line.684652 of MG-RAVENS IEEE13_Assets.dss,
    // per foot over 0.3048: the shield and the bare neutral are reduced.
    let matrices = calculate(&tape_shield_geometry(), 60.0).unwrap();
    let close = |a: f64, b: f64| (a - b).abs() <= 1e-12 * b.abs();
    assert!(close(matrices.r[0][0], 0.000_802_906_788_157_298_7));
    assert!(close(matrices.x[0][0], 0.000_429_278_946_174_020_5));
    let susceptance = 2.0 * std::f64::consts::PI * 60.0 * 1.663_587_801_621_559e-10;
    assert!((matrices.b[0][0] - susceptance).abs() <= 1e-14 * susceptance);
}

#[test]
fn tape_shield_data_is_validated_and_never_mixed_with_neutral_rings() {
    let mut input = tape_shield_geometry();
    if let Some(Data {
        screen: Screen::TapeShield { tapelayer, .. },
        ..
    }) = &mut input.wires[0].cable
    {
        *tapelayer = 0.0;
    }
    assert!(calculate(&input, 60.0).unwrap_err().contains("tape-shield"));
    let mut mixed = geometry();
    mixed.wires[1].cable = tape_shield_geometry().wires[0].cable;
    assert!(
        calculate(&mixed, 60.0)
            .unwrap_err()
            .contains("mixing concentric-neutral and tape-shield")
    );
    let mut data = Data::tape_shield();
    assert!(data.apply("tapelap", 100.0).is_err());
    assert!(data.apply("tapelap", 0.0).is_ok());
    assert!(data.apply("k", 13.0).unwrap_err().contains("TSData"));
}
