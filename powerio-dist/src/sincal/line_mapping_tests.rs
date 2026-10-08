use std::collections::BTreeMap;

use num_complex::Complex64;

use super::{schema::NativeDatabase, tests::sequence_database};
use crate::{DistBus, MulticonductorNetwork};

fn native(edit: &str) -> NativeDatabase {
    NativeDatabase::decode(&line_database(edit), None).unwrap()
}

pub(super) fn line_database(edit: &str) -> Vec<u8> {
    sequence_database(&format!(
        "ALTER TABLE Element ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Element ADD COLUMN VoltLevel_ID INTEGER DEFAULT 1;
         ALTER TABLE Terminal ADD COLUMN Flag_State INTEGER DEFAULT 1;
         ALTER TABLE Terminal ADD COLUMN Flag_Terminal INTEGER DEFAULT 7;
         ALTER TABLE CalcParameter ADD COLUMN f REAL DEFAULT 50;
         ALTER TABLE CalcParameter ADD COLUMN Temp_Cond REAL DEFAULT 20;
         CREATE TABLE VoltageLevel (VoltLevel_ID INTEGER,Variant_ID INTEGER,Temp_Line REAL,Temp_Cable REAL,Un REAL,f REAL,Flag_Volt INTEGER);
         INSERT INTO VoltageLevel VALUES (1,1,20,20,0.4,50,1);
         CREATE TABLE LineSeg (Line_ID INTEGER,Variant_ID INTEGER);
         ALTER TABLE Line ADD COLUMN Flag_LineTyp INTEGER DEFAULT 1;
         ALTER TABLE Line ADD COLUMN Flag_Ll INTEGER DEFAULT 0;
         ALTER TABLE Line ADD COLUMN Flag_Ground INTEGER DEFAULT 0;
         ALTER TABLE Line ADD COLUMN Flag_Macro INTEGER DEFAULT 0;
         ALTER TABLE Line ADD COLUMN Macro_ID INTEGER DEFAULT 0;
         ALTER TABLE Line ADD COLUMN LineTemp_ID INTEGER DEFAULT 0;
         ALTER TABLE Line ADD COLUMN ElemLoading_ID INTEGER DEFAULT 0;
         ALTER TABLE Line ADD COLUMN Flag_Lf INTEGER DEFAULT 1;
         ALTER TABLE Line ADD COLUMN va REAL DEFAULT 0;
         ALTER TABLE Line ADD COLUMN Un REAL DEFAULT 0.4;
         ALTER TABLE Line ADD COLUMN alpha REAL DEFAULT 0.004;
         {edit}"
    ))
}

fn buses() -> BTreeMap<i64, DistBus> {
    [10, 20]
        .into_iter()
        .map(|id| {
            (
                id,
                DistBus::new(id.to_string(), ["1", "2", "3"].map(str::to_owned).to_vec()),
            )
        })
        .collect()
}

#[test]
fn independent_reduced_phase_lines_keep_order_units_ratings_and_open_ports() {
    for (code, phases) in [
        (1, vec!["1"]),
        (2, vec!["2"]),
        (3, vec!["3"]),
        (4, vec!["1", "2"]),
        (5, vec!["2", "3"]),
        (6, vec!["3", "1"]),
    ] {
        for open in [false, true] {
            let mut source_buses = buses();
            for bus in source_buses.values_mut() {
                bus.terminals = phases.iter().map(|p| (*p).to_owned()).collect();
                bus.terminals.push("n".into());
            }
            let circuit = native(&format!(
                "UPDATE Terminal SET Flag_Terminal={code};
                 UPDATE Terminal SET Flag_State={} WHERE TerminalNo=2;
                 UPDATE Line SET r=0.4, r0=0.4, x=0.2, x0=0.2, c=10, c0=10, ParSys=2, fr=0.8;
                 UPDATE VoltageLevel SET Temp_Cable=70, f=60; UPDATE CalcParameter SET f=60;",
                i32::from(!open),
            ))
            .line_circuit(30, &source_buses)
            .unwrap();
            let count = phases.len();
            assert_eq!(circuit.line.terminal_map_from, phases);
            assert_eq!(circuit.line.terminal_map_to, phases);
            assert_eq!(circuit.code.n_conductors, count);
            assert_eq!(circuit.code.i_max, Some(vec![320.0; count]));
            for (matrix, diagonal) in [
                (&circuit.code.r_series, 0.00024),
                (&circuit.code.x_series, 0.00012),
                (&circuit.code.b_from, std::f64::consts::TAU * 60.0 * 1e-11),
            ] {
                assert_eq!(matrix.len(), count);
                for (i, row) in matrix.iter().enumerate() {
                    assert_eq!(row.len(), count);
                    for (j, &value) in row.iter().enumerate() {
                        if i == j {
                            assert!((value - diagonal).abs() < diagonal * 1e-12);
                        } else {
                            assert_eq!(value.abs().to_bits(), 0.0_f64.to_bits());
                        }
                    }
                }
            }
            assert_eq!(circuit.code.b_from, circuit.code.b_to);
            assert_eq!(circuit.auxiliary_buses.len(), usize::from(open));
            if open {
                assert_eq!(circuit.auxiliary_buses[0].terminals, phases);
                assert_eq!(circuit.auxiliary_buses[0].grounded.as_slice(), []);
                assert!(circuit.terminal_switches[0].open);
                assert_eq!(circuit.terminal_switches[0].terminal_map_from, phases);
            }
            let mut net = MulticonductorNetwork::new();
            *net.base_frequency_mut() = circuit.frequency_hz;
            net.buses_mut().extend(source_buses.into_values());
            net.buses_mut().extend(circuit.auxiliary_buses);
            net.lines_mut().push(circuit.line);
            net.line_codes_mut().push(circuit.code);
            net.switches_mut().extend(circuit.terminal_switches);
            crate::require_electrical_readiness(&net).unwrap();
            let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
            let parsed = crate::testkit::parse_str(&output.text, "pmd-json").unwrap();
            assert_eq!(parsed.lines()[0].terminal_map_from, phases);
            assert_eq!(parsed.line_codes()[0].n_conductors, count);
            assert_eq!(parsed.line_codes()[0].i_max, net.line_codes()[0].i_max);
            assert_eq!(parsed.switches().len(), usize::from(open));
            assert!(parsed.buses().iter().all(|bus| bus.grounded.is_empty()));
            for (a, b) in parsed.line_codes()[0]
                .r_series
                .iter()
                .flatten()
                .zip(net.line_codes()[0].r_series.iter().flatten())
            {
                assert!((a - b).abs() < 1e-15);
            }
        }
    }
}

#[test]
fn reduced_phase_lines_do_not_infer_coupling_or_cross_phase_connections() {
    let base = "UPDATE Line SET r0=r, x0=x, c0=c; UPDATE Terminal SET Flag_Terminal=6;";
    for edit in [
        "UPDATE Line SET r0=0.9",
        "UPDATE Line SET x0=0.9",
        "UPDATE Line SET c0=0",
        "UPDATE Line SET va=0.1", // G0=0, so dielectric conductance couples phases
        "UPDATE Terminal SET Flag_Terminal=4 WHERE TerminalNo=2",
        "UPDATE Terminal SET Flag_Terminal=8",
    ] {
        assert!(
            native(&format!("{base}{edit}"))
                .line_circuit(30, &buses())
                .is_err(),
            "{edit}"
        );
    }
    let mut missing = buses();
    missing.get_mut(&20).unwrap().terminals.retain(|p| p != "3");
    assert!(native(base).line_circuit(30, &missing).is_err());
}

#[test]
fn coupled_series_only_lines_match_documented_missing_phase_admittances() {
    // Corrected sequence impedances for the entire 250 m, two-parallel line:
    // 70 C at alpha=.004 and 60 Hz instead of the rated 50 Hz.
    let z1 = Complex64::new(0.045, 0.015);
    let z0 = Complex64::new(0.135, 0.06);
    let ys = (z0.inv() + 2.0 * z1.inv()) / 3.0;
    let ym = (z0.inv() - z1.inv()) / 3.0;
    for code in 1..=6 {
        let circuit = native(&format!(
            "UPDATE Terminal SET Flag_Terminal={code};
             UPDATE Terminal SET Flag_State=0 WHERE TerminalNo=2;
             UPDATE Line SET c=0, c0=0, ParSys=2;
             UPDATE VoltageLevel SET Temp_Cable=70, f=60;
             UPDATE CalcParameter SET f=60;"
        ))
        .line_circuit(30, &buses())
        .unwrap();
        let n = circuit.code.n_conductors;
        // Independently use the printed admittance-elimination equations,
        // not the mapper's impedance restriction, to calculate port currents.
        let (diagonal, mutual) = if n == 1 {
            (ys - 2.0 * ym * ym / (ys + ym), Complex64::new(0.0, 0.0))
        } else {
            (ys - ym * ym / ys, ym - ym * ym / ys)
        };
        let drop = [Complex64::new(4.0, -1.0), Complex64::new(-2.0, 3.0)];
        let current: Vec<_> = (0..n)
            .map(|i| {
                (0..n)
                    .map(|j| {
                        if i == j {
                            diagonal * drop[j]
                        } else {
                            mutual * drop[j]
                        }
                    })
                    .sum::<Complex64>()
            })
            .collect();
        for (i, expected) in drop[..n].iter().enumerate() {
            let actual: Complex64 = (0..n)
                .map(|j| {
                    Complex64::new(circuit.code.r_series[i][j], circuit.code.x_series[i][j])
                        * circuit.line.length
                        * current[j]
                })
                .sum();
            assert!((actual - expected).norm() < 1e-12);
        }
        let loss: Complex64 = drop[..n]
            .iter()
            .zip(&current)
            .map(|(v, i)| v * i.conj())
            .sum();
        assert!(loss.re > 0.0 && loss.im > 0.0);
        assert!(circuit.terminal_switches[0].open);
        assert_eq!(
            circuit.auxiliary_buses[0].terminals,
            circuit.line.terminal_map_to
        );
        for matrix in [
            &circuit.code.g_from,
            &circuit.code.g_to,
            &circuit.code.b_from,
            &circuit.code.b_to,
        ] {
            assert!(matrix.iter().flatten().all(|value| *value == 0.0));
        }
        let mut net = MulticonductorNetwork::new();
        *net.base_frequency_mut() = circuit.frequency_hz;
        net.buses_mut().extend(buses().into_values());
        net.buses_mut().extend(circuit.auxiliary_buses);
        net.line_codes_mut().push(circuit.code);
        net.lines_mut().push(circuit.line);
        net.switches_mut().extend(circuit.terminal_switches);
        crate::require_electrical_readiness(&net).unwrap();
        let output = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
        let parsed = crate::testkit::parse_str(&output.text, "pmd-json").unwrap();
        assert_eq!(
            parsed.lines()[0].terminal_map_to,
            net.lines()[0].terminal_map_to
        );
        assert!(parsed.switches()[0].open);
        for (a, b) in [
            &parsed.line_codes()[0].r_series,
            &parsed.line_codes()[0].x_series,
        ]
        .into_iter()
        .zip([&net.line_codes()[0].r_series, &net.line_codes()[0].x_series])
        {
            for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
                assert!((a - b).abs() < 1e-15);
            }
        }
    }
}

#[test]
fn series_coupling_does_not_admit_unverified_reduced_phase_charging() {
    for edit in [
        "UPDATE Line SET c=1",
        "UPDATE Line SET c0=1",
        "UPDATE Line SET c=1, c0=1",
        "UPDATE Line SET va=0.1",
    ] {
        let db = native(&format!(
            "UPDATE Terminal SET Flag_Terminal=6; UPDATE Line SET c=0, c0=0; {edit};"
        ));
        let error = db.line_circuit(30, &buses()).err().unwrap().to_string();
        assert!(error.contains("with shunts"), "{error}");
    }
}

#[test]
fn closed_line_maps_native_port_order_and_retains_coupled_pi_data() {
    let db = native("UPDATE Terminal SET Terminal_ID=1000 WHERE TerminalNo=1");
    let circuit = db.line_circuit(30, &buses()).unwrap();
    let raw = db.sequence_line(30).unwrap();
    assert_eq!(circuit.line.bus_from, "10");
    assert_eq!(circuit.line.bus_to, "20");
    assert_eq!(circuit.line.terminal_map_from, ["1", "2", "3"]);
    assert_eq!(circuit.line.terminal_map_to, ["1", "2", "3"]);
    assert_eq!(circuit.code, raw.code);
    assert!((circuit.line.length - 250.0).abs() < 1e-12);
    assert!((circuit.frequency_hz - 50.0).abs() < 1e-12);
    assert!(circuit.code.r_series[0][1] > 0.0);
    assert_eq!(circuit.auxiliary_buses.as_slice(), []);
    assert_eq!(circuit.terminal_switches.as_slice(), []);
    assert_eq!(circuit.code.i_max, Some(vec![200.0; 3]));
}

#[test]
fn line_context_applies_network_frequency_to_parallel_circuits() {
    let circuit = native(
        "UPDATE CalcParameter SET f=60;
        UPDATE VoltageLevel SET f=60;
        UPDATE Line SET ParSys=2, fr=0.8",
    )
    .line_circuit(30, &buses())
    .unwrap();
    assert!((circuit.frequency_hz - 60.0).abs() < 1e-12);
    assert!((circuit.code.r_series[0][0] - 0.00025).abs() < 1e-15);
    assert!((circuit.code.x_series[0][0] - 0.00012).abs() < 1e-15);
    assert!((circuit.code.x_series[0][1] - 0.00006).abs() < 1e-15);
    // Sum both ends and all columns to recover the two parallel zero-
    // sequence capacitances at the network frequency, not the rated 50 Hz.
    let b0 = circuit.code.b_from[0].iter().sum::<f64>() * 2.0;
    let expected = std::f64::consts::TAU * 60.0 * 8e-12;
    assert!((b0 - expected).abs() < expected * 1e-12);
    assert_eq!(circuit.code.i_max, Some(vec![320.0; 3]));
}

#[test]
fn temperature_correction_uses_the_active_voltage_level_temperature() {
    let baseline = native("").line_circuit(30, &buses()).unwrap();
    for (kind, temperature, factor) in [(1, 70, 1.2), (2, -5, 0.9)] {
        let active = if kind == 1 { "Temp_Cable" } else { "Temp_Line" };
        let inactive = if kind == 1 { "Temp_Line" } else { "Temp_Cable" };
        let circuit = native(&format!(
            "UPDATE Line SET Flag_LineTyp={kind};
            UPDATE VoltageLevel SET {active}={temperature}, {inactive}=NULL;
            INSERT INTO VoltageLevel VALUES (1,2,900,900,11,60,1)"
        ))
        .line_circuit(30, &buses())
        .unwrap();
        for (r, base) in circuit
            .code
            .r_series
            .iter()
            .flatten()
            .zip(baseline.code.r_series.iter().flatten())
        {
            assert!((r - base * factor).abs() < 1e-15);
        }
        assert_eq!(circuit.code.x_series, baseline.code.x_series);
        assert_eq!(circuit.code.b_from, baseline.code.b_from);
        assert_eq!(circuit.code.b_to, baseline.code.b_to);
        assert_eq!(circuit.code.i_max, baseline.code.i_max);
    }
    // At the 20 °C reference, no temperature coefficient is consumed.
    assert!(
        native("UPDATE Line SET alpha=NULL")
            .line_circuit(30, &buses())
            .is_ok()
    );
}

#[test]
fn dielectric_shunt_dissipates_documented_power_without_zero_sequence_loss() {
    let circuit = native(
        "UPDATE Line SET Un=0.6, va=1.2, ParSys=3, fr=0.5;
        UPDATE CalcParameter SET f=60; UPDATE VoltageLevel SET f=60",
    )
    .line_circuit(30, &buses())
    .unwrap();
    let power = |real: [f64; 3], imag: [f64; 3]| {
        let mut loss = 0.0;
        for g in [&circuit.code.g_from, &circuit.code.g_to] {
            for i in 0..3 {
                for j in 0..3 {
                    loss += circuit.line.length * g[i][j] * (real[i] * real[j] + imag[i] * imag[j]);
                }
            }
        }
        loss
    };
    // At 400 V line-line, 1.2 kW/km over 0.25 km and three systems,
    // rated 600 V, dissipates 900*(400/600)^2 = 400 W at equal ends.
    let root3 = 3.0_f64.sqrt();
    assert!(
        (power(
            [400.0 / root3, -200.0 / root3, -200.0 / root3],
            [0.0, -200.0, 200.0]
        ) - 400.0)
            .abs()
            < 1e-10
    );
    assert!((power([230.0, 100.0, -20.0], [0.0; 3]) - 78.166_666_666_666_67).abs() < 1e-10);
    // Common-mode voltage has zero loss: the manual gives G0=0. A
    // diagonal phase shunt would incorrectly dissipate common-mode power.
    assert!(power([230.0; 3], [0.0; 3]).abs() < 1e-10);
    assert_eq!(circuit.code.g_from, circuit.code.g_to);
    assert!(circuit.code.g_from[0][1] < 0.0);
    assert_eq!(circuit.code.i_max, Some(vec![300.0; 3]));
}

#[test]
fn temperature_and_loss_corrections_reject_invalid_active_values() {
    for edit in [
        "UPDATE VoltageLevel SET Temp_Cable=1e999",
        "UPDATE VoltageLevel SET Temp_Cable=70; UPDATE Line SET alpha=NULL",
        "UPDATE VoltageLevel SET Temp_Cable=-230", // zero resistance factor
        "UPDATE VoltageLevel SET Temp_Cable=-300", // negative resistance factor
        "UPDATE VoltageLevel SET Temp_Cable=70; UPDATE Line SET alpha=1e308",
        "UPDATE VoltageLevel SET Temp_Cable=70; UPDATE Line SET r=1e308, alpha=1e305",
        "UPDATE Line SET va=NULL",
        "UPDATE Line SET va=1e999",
        "UPDATE Line SET va=1e308, ParSys=1e20",
        "UPDATE Line SET va=1e-320",
        "UPDATE Line SET va=0.1, Un=0",
    ] {
        assert!(
            native(edit).line_circuit(30, &buses()).is_err(),
            "accepted {edit}"
        );
    }
}

#[test]
fn open_terminals_keep_full_line_and_isolate_only_their_native_ports() {
    for states in [[0, 1], [1, 0], [0, 0]] {
        let db = native(&format!(
            "UPDATE Terminal SET Flag_State={} WHERE TerminalNo=1;
            UPDATE Terminal SET Flag_State={} WHERE TerminalNo=2",
            states[0], states[1]
        ));
        let circuit = db.line_circuit(30, &buses()).unwrap();
        assert_eq!(circuit.code, db.sequence_line(30).unwrap().code);
        assert_eq!(circuit.code.b_from, circuit.code.b_to);
        assert!(circuit.code.b_from[0][0] > 0.0);
        let open_count = states.iter().filter(|&&s| s == 0).count();
        assert_eq!(circuit.auxiliary_buses.len(), open_count);
        assert_eq!(circuit.terminal_switches.len(), open_count);
        for (index, end) in [&circuit.line.bus_from, &circuit.line.bus_to]
            .into_iter()
            .enumerate()
        {
            let native_bus = if index == 0 { "10" } else { "20" };
            if states[index] == 1 {
                assert_eq!(end, native_bus);
            } else {
                assert_ne!(end, native_bus);
                let internal = circuit
                    .auxiliary_buses
                    .iter()
                    .find(|b| &b.id == end)
                    .unwrap();
                assert_eq!(internal.terminals, ["1", "2", "3"]);
                assert_eq!(internal.grounded.as_slice(), []);
                let switch = circuit
                    .terminal_switches
                    .iter()
                    .find(|s| &s.bus_to == end)
                    .unwrap();
                assert_eq!(switch.bus_from, native_bus);
                assert!(switch.open);
                assert_eq!(switch.terminal_map_from, switch.terminal_map_to);
            }
        }
    }
}

#[test]
fn line_projection_refuses_unresolved_modes_and_unapplied_corrections() {
    for edit in [
        "UPDATE Element SET Flag_State=0",
        "UPDATE Terminal SET Flag_Terminal=4",
        "UPDATE Line SET Flag_LineTyp=3",
        "UPDATE Line SET Flag_LineTyp=4",
        "UPDATE Line SET Flag_Ll=1",
        "UPDATE Line SET Flag_Ground=1",
        "UPDATE Line SET Flag_Macro=1",
        "UPDATE Line SET Macro_ID=8",
        "UPDATE Line SET LineTemp_ID=8",
        "UPDATE Line SET ElemLoading_ID=8",
        "UPDATE Line SET Flag_Lf=2",
        "UPDATE Line SET va=-0.1",
        "UPDATE Line SET fn=0",
        "UPDATE CalcParameter SET f=60",
        "UPDATE CalcParameter SET Temp_Cond=30",
        "UPDATE VoltageLevel SET Temp_Cable=NULL",
        "UPDATE VoltageLevel SET f=60",
        "UPDATE VoltageLevel SET Flag_Volt=2",
        "UPDATE VoltageLevel SET Flag_Volt=NULL",
        "UPDATE VoltageLevel SET Flag_Volt=0",
        "ALTER TABLE VoltageLevel DROP COLUMN Flag_Volt",
        "UPDATE Line SET Un=0.2",
        "UPDATE VoltageLevel SET Un=0",
        "UPDATE Element SET VoltLevel_ID=2",
        "INSERT INTO VoltageLevel VALUES (1,1,20,20,0.4,50,1)",
        "INSERT INTO CalcParameter VALUES (1,1,50,20)",
        "INSERT INTO LineSeg VALUES (30,1)",
        "DELETE FROM Terminal WHERE TerminalNo=2",
    ] {
        assert!(
            native(edit).line_circuit(30, &buses()).is_err(),
            "accepted {edit}"
        );
    }
    let mut missing = buses();
    missing.remove(&20);
    assert!(native("").line_circuit(30, &missing).is_err());
    let mut missing = buses();
    missing.get_mut(&20).unwrap().terminals.pop();
    assert!(native("").line_circuit(30, &missing).is_err());
}

#[test]
fn line_context_uses_variant_local_references_and_active_temperature() {
    let db = native(
        "INSERT INTO VoltageLevel VALUES (1,2,90,90,11,60,1);
        INSERT INTO LineSeg VALUES (30,2);
        UPDATE VoltageLevel SET Temp_Line=NULL WHERE Variant_ID=1",
    );
    assert!(db.line_circuit(30, &buses()).is_ok());
    let db = native("UPDATE Line SET Flag_LineTyp=2; UPDATE VoltageLevel SET Temp_Cable=NULL");
    assert!(db.line_circuit(30, &buses()).is_ok());
    // A source column cannot shadow a joined calculation attribute.
    let db = native(
        "ALTER TABLE Line ADD COLUMN CalcFrequency REAL DEFAULT 50;
        UPDATE CalcParameter SET f=60",
    );
    assert!(db.line_circuit(30, &buses()).is_err());
}

#[test]
fn open_line_circuit_survives_fresh_pmd_emission() {
    let mut net = MulticonductorNetwork::new();
    let source_buses = buses();
    let circuit = native(
        "UPDATE Terminal SET Flag_State=0 WHERE TerminalNo=2;
        UPDATE VoltageLevel SET Temp_Cable=70, f=60;
        UPDATE CalcParameter SET f=60;
        UPDATE Line SET va=0.1, ParSys=2, fr=0.8",
    )
    .line_circuit(30, &source_buses)
    .unwrap();
    *net.base_frequency_mut() = circuit.frequency_hz;
    net.buses_mut().extend(source_buses.into_values());
    net.buses_mut().extend(circuit.auxiliary_buses);
    net.line_codes_mut().push(circuit.code);
    net.lines_mut().push(circuit.line);
    net.switches_mut().extend(circuit.terminal_switches);
    crate::require_electrical_readiness(&net).unwrap();
    let emitted = crate::convert::emit_value_text(&net, crate::DistTargetFormat::PmdJson);
    // PMD keeps the electrical circuit but cannot retain native line/switch
    // provenance. Its existing writer must report those two metadata losses.
    assert_eq!(emitted.diagnostics.len(), 2, "{:?}", emitted.diagnostics);
    assert!(
        emitted
            .diagnostics
            .iter()
            .all(|d| d.code() == "EMIT.PMD.FIELD_DROPPED")
    );
    assert!(
        emitted
            .diagnostics
            .iter()
            .any(|d| d.message().contains("`sincal`"))
    );
    assert!(
        emitted
            .diagnostics
            .iter()
            .any(|d| d.message().contains("`sincal_line_end`"))
    );
    let reparsed = crate::testkit::parse_str(&emitted.text, "pmd-json").unwrap();
    crate::require_electrical_readiness(&reparsed).unwrap();
    assert_eq!(reparsed.buses().len(), 3);
    assert_eq!(reparsed.lines().len(), 1);
    assert_eq!(reparsed.switches().len(), 1);
    assert!(reparsed.switches()[0].open);
    assert_eq!(reparsed.lines()[0].bus_to, net.lines()[0].bus_to);
    assert_eq!(reparsed.switches()[0].bus_from, "20");
    assert_eq!(reparsed.switches()[0].bus_to, net.lines()[0].bus_to);
    let actual = &reparsed.line_codes()[0];
    let expected = &net.line_codes()[0];
    for (a, b) in [
        &actual.r_series,
        &actual.x_series,
        &actual.g_from,
        &actual.g_to,
        &actual.b_from,
        &actual.b_to,
    ]
    .into_iter()
    .zip([
        &expected.r_series,
        &expected.x_series,
        &expected.g_from,
        &expected.g_to,
        &expected.b_from,
        &expected.b_to,
    ]) {
        for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
            assert!((a - b).abs() <= 1e-12 * b.abs().max(1e-20));
        }
    }
}

#[test]
fn legacy_line_temperatures_use_only_documented_selected_defaults() {
    for (kind, selected, inactive) in [
        (1, "Temp_Cable", "Temp_Line"),
        (2, "Temp_Line", "Temp_Cable"),
    ] {
        let explicit = super::legacy_tests::legacy(&format!("UPDATE Line SET Flag_LineTyp={kind}"))
            .line_circuit(30, &buses())
            .unwrap();
        let legacy = format!("UPDATE Line SET Flag_LineTyp={kind};");
        let null = super::legacy_tests::legacy(&format!(
            "{legacy} UPDATE VoltageLevel SET {selected}=NULL,{inactive}=91"
        ))
        .line_circuit(30, &buses())
        .unwrap();
        assert_eq!(null.code, explicit.code);
        assert_eq!(null.defaulted, [format!("VoltageLevel.{selected}")]);
        let warmer = super::legacy_tests::legacy(&format!(
            "{legacy} UPDATE VoltageLevel SET {selected}=70,{inactive}=NULL"
        ))
        .line_circuit(30, &buses())
        .unwrap();
        assert_eq!(warmer.defaulted.as_slice(), []);
        assert!((warmer.code.r_series[0][0] / explicit.code.r_series[0][0] - 1.2).abs() < 1e-12);
        for bad in ["NULL", "'unknown'", "1e999"] {
            let db = native(&format!(
                "UPDATE Line SET Flag_LineTyp={kind}; UPDATE VoltageLevel SET {selected}={bad}"
            ));
            assert!(db.line_circuit(30, &buses()).is_err());
        }
        let absent = super::legacy_tests::legacy(&format!(
            "{legacy} ALTER TABLE VoltageLevel DROP COLUMN {selected}"
        ));
        assert!(absent.line_circuit(30, &buses()).is_err());
    }
}

#[test]
fn network_records_applied_line_temperature_defaults() {
    let net =
        super::legacy_tests::legacy("UPDATE VoltageLevel SET Temp_Cable=NULL WHERE VoltLevel_ID=1")
            .network()
            .unwrap();
    assert_eq!(net.defaulted()["Line.30"], ["VoltageLevel.Temp_Cable"]);
    let explicit = super::legacy_tests::legacy("").network().unwrap();
    assert_eq!(net.line_codes(), explicit.line_codes());
    assert_eq!(net.sources(), explicit.sources());
    assert_eq!(net.loads(), explicit.loads());
}

#[test]
#[ignore = "exports native lines using documented legacy temperature defaults"]
fn export_legacy_temperature_lines() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("POWERIO_SINCAL_TEMPERATURE_RECORDS").expect("records directory"),
    );
    let output = std::path::PathBuf::from(
        std::env::var_os("POWERIO_SINCAL_TEMPERATURE_EXPORT").expect("output path"),
    );
    export_defaulted_lines(&[3, 5, 12], &directory, &output, false);
}

#[test]
#[ignore = "exports native sparse line circuits and ideal connections"]
fn export_sparse_legacy_lines() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("POWERIO_SINCAL_SPARSE_LINE_RECORDS").expect("records directory"),
    );
    let output = std::path::PathBuf::from(
        std::env::var_os("POWERIO_SINCAL_SPARSE_LINE_EXPORT").expect("output path"),
    );
    export_defaulted_lines(&[3, 5, 14, 15], &directory, &output, true);
}

fn export_defaulted_lines(
    case_numbers: &[i32],
    directory: &std::path::Path,
    output: &std::path::Path,
    sparse: bool,
) {
    let mut cases = Vec::new();
    for &case in case_numbers {
        let path = directory.join(format!("representative{case:02}.json"));
        let db = NativeDatabase::from_snapshot(
            powerio_sincal::DatabaseSnapshot::decode_records(
                &std::fs::read(path).unwrap(),
                Some(1),
            )
            .unwrap(),
        )
        .unwrap();
        let buses: BTreeMap<i64, DistBus> = db
            .node_inputs()
            .unwrap()
            .keys()
            .map(|id| {
                (
                    *id,
                    DistBus::new(id.to_string(), ["1", "2", "3"].map(str::to_owned).to_vec()),
                )
            })
            .collect();
        let mut lines = Vec::new();
        let mut connections = Vec::new();
        let mut graph_network = crate::MulticonductorNetwork::new();
        graph_network.buses_mut().extend(buses.values().cloned());
        for (&id, kind) in &db.elements {
            if kind != "Line" {
                continue;
            }
            if sparse && let Some((switch, defaulted)) = db.connection_switch(id, &buses).unwrap() {
                if !defaulted.is_empty() {
                    graph_network.switches_mut().push(switch.clone());
                    connections.push(
                        serde_json::json!({"element":id,"switch":switch,"defaulted":defaulted}),
                    );
                }
                continue;
            }
            if let Ok(c) = db.line_circuit(id, &buses)
                && !c.defaulted.is_empty()
                && c.defaulted.iter().any(|f| !f.starts_with("VoltageLevel.")) == sparse
            {
                let mut value = serde_json::json!({"element":id,"line":c.line,"code":c.code,
                    "auxiliary_buses":c.auxiliary_buses,"switches":c.terminal_switches,"defaulted":c.defaulted});
                if sparse {
                    value["ideal_switch"] = serde_json::to_value(c.ideal_connection()).unwrap();
                }
                lines.push(value);
            }
        }
        cases.push(if sparse {
            serde_json::json!({"case":case,"lines":lines,"connections":connections,"graph":graph_network.to_graph()})
        } else {
            serde_json::json!({"case":case,"lines":lines})
        });
    }
    std::fs::write(output, serde_json::to_vec_pretty(&cases).unwrap()).unwrap();
}

#[test]
fn sparse_legacy_line_fields_preserve_explicit_electrical_values_and_provenance() {
    let sparse = "UPDATE Line SET Flag_Ll=NULL,Flag_Ground=NULL,Flag_Macro=NULL,ParSys=NULL,va=NULL,alpha=NULL;";
    for temperature in [20, 70] {
        let context = format!("UPDATE VoltageLevel SET Temp_Cable={temperature};");
        let explicit = super::legacy_tests::legacy(&context).network().unwrap();
        let actual = super::legacy_tests::legacy(&format!("{context}{sparse}"))
            .network()
            .unwrap();
        assert_eq!(actual.line_codes(), explicit.line_codes());
        assert_eq!(actual.lines(), explicit.lines());
        assert_eq!(actual.loads(), explicit.loads());
        let mut fields = vec!["Flag_Ground", "Flag_Ll", "Flag_Macro", "ParSys", "va"];
        if temperature != 20 {
            fields.insert(4, "alpha");
        }
        assert_eq!(actual.defaulted()["Line.30"], fields);
    }
    let parallel = super::legacy_tests::legacy("UPDATE Line SET ParSys=2")
        .network()
        .unwrap();
    let single = super::legacy_tests::legacy("UPDATE Line SET ParSys=NULL")
        .network()
        .unwrap();
    assert!(
        (single.line_codes()[0].r_series[0][0] / parallel.line_codes()[0].r_series[0][0] - 2.0)
            .abs()
            < 1e-12
    );
    assert!(!parallel.defaulted().contains_key("Line.30"));
    let lossy = super::legacy_tests::legacy("UPDATE Line SET va=0.5")
        .network()
        .unwrap();
    assert!(lossy.line_codes()[0].g_from[0][0] > 0.0);
    assert!(
        single.line_codes()[0]
            .g_from
            .iter()
            .flatten()
            .all(|v| *v == 0.0)
    );
}

#[test]
fn sparse_line_defaults_never_replace_required_inputs_or_modern_nulls() {
    // Nonfinite numbers cannot be expressed in acquisition JSON. Exercise
    // their direct SQLite rejection without serializing them into JSON NULL.
    for field in ["va", "alpha", "ParSys", "fr", "fn"] {
        assert!(
            super::mapping_tests::native(&format!(
                "UPDATE VoltageLevel SET Temp_Cable=70; UPDATE Line SET {field}=1e999"
            ))
            .network()
            .is_err()
        );
    }

    for field in [
        "Flag_Ll",
        "Flag_Ground",
        "Flag_Macro",
        "ParSys",
        "fr",
        "fn",
        "va",
        "alpha",
    ] {
        let edit = format!("UPDATE VoltageLevel SET Temp_Cable=70; UPDATE Line SET {field}=NULL;");
        assert!(
            super::mapping_tests::native(&edit).network().is_err(),
            "modern NULL {field}"
        );
        assert!(
            super::legacy_tests::legacy(&edit).network().is_ok(),
            "legacy NULL {field}"
        );
        let missing =
            format!("UPDATE VoltageLevel SET Temp_Cable=70; ALTER TABLE Line DROP COLUMN {field}");
        assert!(
            super::legacy_tests::legacy(&missing).network().is_err(),
            "missing {field}"
        );
        let bad = format!("UPDATE VoltageLevel SET Temp_Cable=70; UPDATE Line SET {field}='bad'");
        assert!(
            super::legacy_tests::legacy(&bad).network().is_err(),
            "bad {field}"
        );
    }
    for field in [
        "r",
        "x",
        "r0",
        "x0",
        "l",
        "Un",
        "Flag_Z0_Input",
        "Flag_LineTyp",
    ] {
        assert!(
            super::legacy_tests::legacy(&format!("UPDATE Line SET {field}=NULL"))
                .network()
                .is_err(),
            "required {field}"
        );
    }
    for edit in [
        "Flag_Ll=1",
        "Flag_Ground=1",
        "Flag_Macro=1",
        "ParSys=0",
        "ParSys=-1",
        "va=-1",
        "fr=0",
        "fn=0",
    ] {
        assert!(
            super::legacy_tests::legacy(&format!(
                "UPDATE VoltageLevel SET Temp_Cable=70; UPDATE Line SET {edit}"
            ))
            .network()
            .is_err(),
            "invalid/unsupported {edit}"
        );
    }
}
