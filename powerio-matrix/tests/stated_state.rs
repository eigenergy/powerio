//! The stated-state bus balance against a synthetic case whose stored
//! voltages are an exact solution: the residual vanishes, and perturbing one
//! device's stated output shows up at that device's bus, flagged with it.

use num_complex::Complex64;
use powerio_matrix::{
    BuildOptions, Error, IndexedNetwork, StatedBusFlags, StatedStateMismatch, StatedStateOptions,
    calc_admittance_matrix, calc_stated_branch_flows, calc_stated_state_mismatch,
};
use powerio_tx::{
    BalancedNetwork, Branch, Bus, BusId, BusMergeRule, BusType, Generator, Hvdc, HvdcTreatment,
    Impedance, Load, LoadVoltageModel, Shunt, ShuntBlock, Switch, SwitchedShuntControl,
    SwitchedShuntMode, Transformer3W, Winding,
};

const BASE_MVA: f64 = 100.0;

/// An HVDC line from bus 3 to bus 7 stated in the stored MATPOWER `dcline`
/// signs, the only form the network holds it in.
fn hvdc_line(pf: f64, pt: f64, qf: f64, qt: f64) -> Hvdc {
    let mut line = Hvdc::new(BusId(3), BusId(7));
    line.pf = pf;
    line.pt = pt;
    line.qf = qf;
    line.qt = qt;
    line
}

/// A forward transfer: 100 MW sent from bus 3, 97 MW delivered at bus 7, the
/// converters absorbing 30 and 25 MVAr from their buses.
fn forward_line() -> Hvdc {
    hvdc_line(100.0, 97.0, -30.0, -25.0)
}

/// The reverse transfer over the same line: bus 7 sends 100 MW and bus 3
/// receives 97 MW, the receiving converter absorbing 25 MVAr and the sending
/// one 30.
fn reverse_line() -> Hvdc {
    hvdc_line(-97.0, -100.0, -25.0, -30.0)
}

/// The injection `line` makes into each terminal bus, MW and MVAr: the
/// negative of its bus-to-device terminal powers.
fn hvdc_injections(line: &Hvdc) -> (Complex64, Complex64) {
    let terminal = line.calc_bus_to_device_powers();
    (
        -Complex64::new(terminal.p_from, terminal.q_from),
        -Complex64::new(terminal.p_to, terminal.q_to),
    )
}

/// Buses 1 to 6 and 7 to 8 form two islands joined only by an HVDC line from
/// bus 3 to bus 7; a three winding transformer joins buses 4, 5, and 6; bus
/// 9 is isolated. Bus 6 is a PQ bus with a generator. No device closes the
/// balance yet.
fn skeleton() -> BalancedNetwork {
    skeleton_with(forward_line())
}

fn skeleton_with(hvdc: Hvdc) -> BalancedNetwork {
    let kinds = [
        (1, BusType::Ref),
        (2, BusType::Pv),
        (3, BusType::Pq),
        (4, BusType::Pq),
        (5, BusType::Pq),
        (6, BusType::Pq),
        (7, BusType::Pq),
        (8, BusType::Ref),
        (9, BusType::Isolated),
    ];
    let buses = kinds
        .iter()
        .map(|&(id, kind)| {
            let mut bus = Bus::new(BusId(id), kind, 230.0);
            let i = id as f64;
            bus.vm = if kind == BusType::Isolated {
                0.0
            } else {
                0.97 + 0.008 * i
            };
            bus.va = if kind == BusType::Isolated {
                0.0
            } else {
                -1.7 * i + 0.3 * i * i
            };
            bus
        })
        .collect();
    let mut charged = Branch::new(BusId(1), BusId(2), 0.01, 0.08);
    charged.b = 0.12;
    let mut shifter = Branch::new(BusId(2), BusId(3), 0.005, 0.06);
    shifter.tap = 1.04;
    shifter.shift = 4.0;
    let branches = vec![
        charged,
        shifter,
        Branch::new(BusId(3), BusId(4), 0.02, 0.11),
        Branch::new(BusId(1), BusId(4), 0.015, 0.09),
        Branch::new(BusId(7), BusId(8), 0.01, 0.07),
    ];
    let mut net = BalancedNetwork::in_memory("stated", BASE_MVA, buses, branches);

    let winding = |bus| {
        let mut w = Winding::new(BusId(bus));
        w.tap = if bus == 5 { 1.02 } else { 1.0 };
        w
    };
    let mut t3w = Transformer3W::new(
        [winding(4), winding(5), winding(6)],
        [
            Impedance::new(0.002, 0.05, BASE_MVA),
            Impedance::new(0.003, 0.07, BASE_MVA),
            Impedance::new(0.002, 0.06, BASE_MVA),
        ],
    );
    t3w.mag_b = -0.01;
    net.transformers_3w_mut().push(t3w);

    net.shunts_mut().push(Shunt::new(BusId(3), 1.0, 12.0));
    let mut switched = Shunt::new(BusId(5), 0.0, 25.0);
    switched.control = Some(SwitchedShuntControl::new(
        SwitchedShuntMode::Discrete,
        1.05,
        0.95,
        vec![ShuntBlock::new(1, 25.0)],
    ));
    net.shunts_mut().push(switched);

    let mut zip = Load::new(BusId(4), 0.0, 0.0);
    zip.voltage_model = Some(LoadVoltageModel::Zip {
        p_constant_power: 20.0,
        q_constant_power: 6.0,
        p_constant_current: 8.0,
        q_constant_current: 3.0,
        p_constant_impedance: 5.0,
        q_constant_impedance: -4.0,
        v_nom: None,
        load_type: None,
        scaling: None,
    });
    zip.p = 33.0;
    zip.q = 5.0;
    net.loads_mut().push(zip);

    net.hvdc_mut().push(hvdc);

    let mut pq_generator = Generator::new(BusId(6));
    pq_generator.pg = 10.0;
    pq_generator.qg = 5.0;
    net.generators_mut().push(pq_generator);
    net
}

/// Bus voltages a case stores, per unit, analysis bus order.
fn voltages(view: &IndexedNetwork<'_>) -> Vec<Complex64> {
    view.network()
        .buses()
        .iter()
        .map(|b| Complex64::from_polar(b.vm, b.va.to_radians()))
        .collect()
}

/// `V ∘ conj(Y V)` per unit, from the public admittance matrix.
fn network_injection(net: &BalancedNetwork) -> Vec<Complex64> {
    let view = IndexedNetwork::new(net);
    let y = calc_admittance_matrix(&view, &BuildOptions::default()).unwrap();
    let v = voltages(&view);
    let mut current = vec![Complex64::new(0.0, 0.0); v.len()];
    for (row, vec) in y.g.outer_iterator().enumerate() {
        for (col, &g) in vec.iter() {
            current[row] += Complex64::new(g, 0.0) * v[col];
        }
    }
    for (row, vec) in y.b.outer_iterator().enumerate() {
        for (col, &b) in vec.iter() {
            current[row] += Complex64::new(0.0, b) * v[col];
        }
    }
    v.iter().zip(&current).map(|(v, i)| v * i.conj()).collect()
}

/// [`skeleton`] with the star point voltage solved and one closing device
/// per bus, so the stored voltages solve the stated injections exactly.
fn solved() -> BalancedNetwork {
    solved_with(forward_line())
}

fn solved_with(hvdc: Hvdc) -> BalancedNetwork {
    let mut net = skeleton_with(hvdc);
    // The star point carries no device, so its voltage satisfies its own
    // current balance: Y_ss V_s = -Σ Y_sk V_k.
    let view = IndexedNetwork::new(&net);
    let star = view.n() - 1;
    let y = calc_admittance_matrix(&view, &BuildOptions::default()).unwrap();
    let v = voltages(&view);
    let entry = |row: usize, col: usize| {
        Complex64::new(
            y.g.get(row, col).copied().unwrap_or(0.0),
            y.b.get(row, col).copied().unwrap_or(0.0),
        )
    };
    let sum: Complex64 = v[..star]
        .iter()
        .enumerate()
        .map(|(col, voltage)| entry(star, col) * voltage)
        .sum();
    let v_star = -sum / entry(star, star);
    drop(view);
    net.transformers_3w_mut()[0].star_vm = v_star.norm();
    net.transformers_3w_mut()[0].star_va = v_star.arg().to_degrees();

    // What the skeleton's devices inject at the stored voltages, MW, worked
    // out here rather than read back from the calculation under test: the
    // HVDC line at buses 3 and 7 through its terminal powers, the ZIP load at
    // bus 4, and the generator on PQ bus 6.
    let vm4 = net.buses()[3].vm;
    let (hvdc_from, hvdc_to) = hvdc_injections(&net.hvdc()[0]);
    let existing = |bus: usize| match bus {
        3 => hvdc_from,
        4 => -Complex64::new(
            20.0 + 8.0 * vm4 + 5.0 * vm4 * vm4,
            6.0 + 3.0 * vm4 - 4.0 * vm4 * vm4,
        ),
        6 => Complex64::new(10.0, 5.0),
        7 => hvdc_to,
        _ => Complex64::new(0.0, 0.0),
    };
    let drawn = network_injection(&net);
    // Every source bus but the isolated bus 9 gets its closing device.
    let closing: Vec<_> = net.buses()[..8].iter().map(|b| (b.id, b.kind)).collect();
    for ((id, kind), drawn) in closing.into_iter().zip(drawn) {
        let missing = drawn * BASE_MVA - existing(id.0);
        if matches!(kind, BusType::Ref | BusType::Pv) {
            let mut generator = Generator::new(id);
            generator.pg = missing.re;
            generator.qg = missing.im;
            net.generators_mut().push(generator);
        } else {
            net.loads_mut()
                .push(Load::new(id, -missing.re, -missing.im));
        }
    }
    net
}

fn row_of(mismatch: &StatedStateMismatch, bus: usize) -> usize {
    mismatch
        .buses
        .iter()
        .position(|b| b.bus == BusId(bus))
        .unwrap()
}

fn assert_closed_except(mismatch: &StatedStateMismatch, except: &[usize]) {
    for bus in &mismatch.buses {
        if except.contains(&bus.bus.0) || bus.island.is_none() {
            continue;
        }
        assert!(
            bus.calc_magnitude_mva() < 1e-9,
            "bus {} has mismatch {} + j{}",
            bus.bus,
            bus.p_mw,
            bus.q_mvar
        );
    }
}

#[test]
fn a_self_consistent_case_closes_to_rounding() {
    let net = solved();
    let mismatch = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap();
    // Below 1e-12 per unit on the 100 MVA base.
    assert!(
        mismatch.calc_max_magnitude_mva() / BASE_MVA < 1e-12,
        "largest residual {} MVA",
        mismatch.calc_max_magnitude_mva()
    );
    assert_closed_except(&mismatch, &[]);

    // The two AC islands, largest first; the isolated bus is in neither.
    assert_eq!(mismatch.islands.len(), 2);
    assert_eq!(mismatch.islands[0].n_buses, 7, "buses 1 to 6 and the star");
    assert_eq!(mismatch.islands[1].n_buses, 2);
    let isolated = &mismatch.buses[row_of(&mismatch, 9)];
    assert_eq!(isolated.island, None);
    assert!(!mismatch.top.contains(&row_of(&mismatch, 9)));
    // Buses 1 to 8 and the star point.
    assert_eq!(mismatch.top.len(), 9);
}

#[test]
fn the_network_side_matches_the_admittance_matrix() {
    // The branch flow accumulation and the public Y_bus product agree on
    // every bus of the unclosed skeleton.
    let net = skeleton();
    let drawn = network_injection(&net);
    let flows = calc_stated_branch_flows(&net).unwrap();
    let view = IndexedNetwork::new(&net);
    let mut summed = vec![Complex64::new(0.0, 0.0); view.n()];
    for (row, branch) in view.branches().iter().enumerate() {
        let from = view.bus_index(branch.from).unwrap();
        let to = view.bus_index(branch.to).unwrap();
        summed[from] += Complex64::new(flows.p_from_mw[row], flows.q_from_mvar[row]);
        summed[to] += Complex64::new(flows.p_to_mw[row], flows.q_to_mvar[row]);
    }
    let v = voltages(&view);
    for idx in 0..view.n() {
        let shunt = Complex64::new(view.gs()[idx], -view.bs()[idx]) * v[idx].norm_sqr();
        let total = summed[idx] + shunt;
        assert!(
            (total - drawn[idx] * BASE_MVA).norm() < 1e-9,
            "bus row {idx}: flows {total} vs Y_bus {}",
            drawn[idx] * BASE_MVA
        );
    }
    // Five branches, then the three windings of the transformer.
    assert_eq!(flows.sources.len(), 8);
    assert!(flows.in_service.iter().all(|&on| on));
}

#[test]
fn a_constant_impedance_load_perturbation_lands_on_its_bus() {
    let mut net = solved();
    let delta = 7.0;
    let load = net
        .loads_mut()
        .iter_mut()
        .find(|l| l.voltage_model.is_some())
        .unwrap();
    if let Some(LoadVoltageModel::Zip {
        q_constant_impedance,
        ..
    }) = &mut load.voltage_model
    {
        *q_constant_impedance += delta;
    }
    load.q += delta;
    let mismatch = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap();
    let row = row_of(&mismatch, 4);
    let vm = net.buses()[3].vm;
    // The load draws delta·V² more, so the network is short of exactly that.
    assert!((mismatch.buses[row].q_mvar - delta * vm * vm).abs() < 1e-9);
    assert!(mismatch.buses[row].p_mw.abs() < 1e-9);
    assert_eq!(mismatch.top[0], row);
    assert!(
        mismatch.buses[row]
            .flags
            .contains(StatedBusFlags::VOLTAGE_DEPENDENT_LOAD)
    );
    assert_closed_except(&mismatch, &[4]);
}

#[test]
fn an_hvdc_reactive_perturbation_lands_on_its_terminal_and_closes() {
    let mut net = solved();
    net.hvdc_mut()[0].qt -= 9.0;
    let mismatch = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap();
    let row = row_of(&mismatch, 7);
    // The converter now states 9 MVAr less injection than the voltages need.
    assert!((mismatch.buses[row].q_mvar - 9.0).abs() < 1e-9);
    assert_eq!(mismatch.top[0], row);
    let flags = mismatch.buses[row].flags;
    assert!(flags.contains(StatedBusFlags::HVDC));
    assert!(!flags.contains(StatedBusFlags::VSC));
    assert_eq!(flags.names(), ["hvdc"]);
    assert_closed_except(&mismatch, &[7]);

    // The closure injections at HVDC terminals restore the balance.
    let closure = mismatch.closure_injections(StatedBusFlags::HVDC);
    assert_eq!(
        closure.iter().map(|c| c.bus).collect::<Vec<_>>(),
        [BusId(3), BusId(7)]
    );
    for injection in closure {
        let mut generator = Generator::new(injection.bus);
        generator.pg = injection.p_mw;
        generator.qg = injection.q_mvar;
        net.generators_mut().push(generator);
    }
    let closed = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap();
    assert_closed_except(&closed, &[]);
}

#[test]
fn a_generator_on_a_pq_bus_is_flagged_where_its_output_differs() {
    let mut net = solved();
    let generator = net
        .generators_mut()
        .iter_mut()
        .find(|g| g.bus == BusId(6))
        .unwrap();
    generator.qg += 4.0;
    let mismatch = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap();
    let row = row_of(&mismatch, 6);
    assert!((mismatch.buses[row].q_mvar + 4.0).abs() < 1e-9);
    assert_eq!(mismatch.top[0], row);
    assert!(
        mismatch.buses[row]
            .flags
            .contains(StatedBusFlags::GENERATOR_ON_PQ_BUS)
    );
    assert_closed_except(&mismatch, &[6]);
}

#[test]
fn flags_name_the_attached_equipment() {
    let mismatch = calc_stated_state_mismatch(&solved(), &StatedStateOptions::default()).unwrap();
    let flags = |bus| mismatch.buses[row_of(&mismatch, bus)].flags;
    assert!(flags(1).contains(StatedBusFlags::REFERENCE));
    assert!(flags(3).contains(StatedBusFlags::HVDC));
    assert!(flags(5).contains(StatedBusFlags::SWITCHED_SHUNT));
    // The star point is the analysis network's last bus.
    let star = mismatch.buses.last().unwrap();
    assert_eq!(star.flags, StatedBusFlags::STAR_BUS);
    assert_eq!(
        StatedBusFlags::from_name("generator_on_pq_bus"),
        Some(StatedBusFlags::GENERATOR_ON_PQ_BUS)
    );
}

#[test]
fn a_low_impedance_branch_flags_both_terminals() {
    let mut net = solved();
    let mut tail = Bus::new(BusId(11), BusType::Pq, 230.0);
    tail.vm = net.buses()[0].vm;
    tail.va = net.buses()[0].va;
    net.buses_mut().push(tail);
    net.branches_mut()
        .push(Branch::new(BusId(1), BusId(11), 0.0, 5e-4));
    let mismatch = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap();
    for bus in [1, 11] {
        assert!(
            mismatch.buses[row_of(&mismatch, bus)]
                .flags
                .contains(StatedBusFlags::LOW_IMPEDANCE_BRANCH)
        );
    }
    // Equal stored voltages at both ends: the branch carries nothing.
    assert_closed_except(&mismatch, &[]);
    let looser = calc_stated_state_mismatch(
        &net,
        &StatedStateOptions::default().with_low_impedance_threshold(1e-4),
    )
    .unwrap();
    assert_eq!(
        looser.buses[row_of(&looser, 11)].flags,
        StatedBusFlags::NONE
    );
}

/// Without the line the stored voltages still need its injection, so under
/// `HvdcTreatment::Ignore` each terminal's residual is the injection the line
/// makes there: the negative of its bus-to-device terminal power.
fn ignored(net: &BalancedNetwork) -> StatedStateMismatch {
    calc_stated_state_mismatch(
        net,
        &StatedStateOptions::default().with_hvdc_treatment(HvdcTreatment::Ignore),
    )
    .unwrap()
}

fn residual(mismatch: &StatedStateMismatch, bus: usize) -> Complex64 {
    let row = &mismatch.buses[row_of(mismatch, bus)];
    Complex64::new(row.p_mw, row.q_mvar)
}

#[test]
fn the_forward_terminal_powers_are_the_documented_example() {
    let terminal = forward_line().calc_bus_to_device_powers();
    assert_eq!((terminal.p_from, terminal.q_from), (100.0, 30.0));
    assert_eq!((terminal.p_to, terminal.q_to), (-97.0, 25.0));
    assert!((terminal.calc_active_loss() - 3.0).abs() < 1e-12);
    // The fixed injection closes the case that states it.
    assert_closed_except(
        &calc_stated_state_mismatch(&solved(), &StatedStateOptions::default()).unwrap(),
        &[],
    );
}

#[test]
fn the_sending_end_withdraws_its_terminal_power() {
    let net = solved();
    let line = &net.hvdc()[0];
    let terminal = line.calc_bus_to_device_powers();
    let mismatch = ignored(&net);
    let from = residual(&mismatch, 3);
    // The residual is the injection, the negated terminal power: bus 3 gives
    // up 100 MW and 30 MVAr to the line.
    assert!((from + Complex64::new(terminal.p_from, terminal.q_from)).norm() < 1e-9);
    assert!((from - Complex64::new(-100.0, -30.0)).norm() < 1e-9);
    assert!((from - hvdc_injections(line).0).norm() < 1e-9);
}

#[test]
fn the_receiving_end_injects_the_delivered_power() {
    let net = solved();
    let line = &net.hvdc()[0];
    let terminal = line.calc_bus_to_device_powers();
    let mismatch = ignored(&net);
    let to = residual(&mismatch, 7);
    // Bus 7 receives 97 MW and its converter absorbs 25 MVAr.
    assert!((to + Complex64::new(terminal.p_to, terminal.q_to)).norm() < 1e-9);
    assert!((to - Complex64::new(97.0, -25.0)).norm() < 1e-9);
    // Island 7-8 has nothing else wrong; its total is the delivered power.
    assert!((mismatch.islands[1].p_mw - 97.0).abs() < 1e-9);
    assert_closed_except(&mismatch, &[3, 7]);
}

#[test]
fn a_reverse_transfer_keeps_the_ends_and_a_positive_loss() {
    let net = solved_with(reverse_line());
    let line = &net.hvdc()[0];
    let terminal = line.calc_bus_to_device_powers();
    // Bus 7 now sends: its terminal power is positive into the line and the
    // from end's is negative, and the two still sum to a 3 MW loss.
    assert_eq!((terminal.p_from, terminal.q_from), (-97.0, 25.0));
    assert_eq!((terminal.p_to, terminal.q_to), (100.0, 30.0));
    assert!((terminal.calc_active_loss() - 3.0).abs() < 1e-12);

    // Fixed injection closes the reversed case.
    let fixed = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap();
    assert_closed_except(&fixed, &[]);

    // Ignored, the residuals are the negated terminal powers.
    let mismatch = ignored(&net);
    let (from, to) = (residual(&mismatch, 3), residual(&mismatch, 7));
    assert!((from + Complex64::new(terminal.p_from, terminal.q_from)).norm() < 1e-9);
    assert!((to + Complex64::new(terminal.p_to, terminal.q_to)).norm() < 1e-9);
    assert!((from - Complex64::new(97.0, -25.0)).norm() < 1e-9);
    assert!((to - Complex64::new(-100.0, -30.0)).norm() < 1e-9);
    // The two ends together inject the negative of the loss.
    assert!((from.re + to.re + terminal.calc_active_loss()).abs() < 1e-9);
}

#[test]
fn an_unmerged_zero_impedance_branch_is_refused_and_the_merge_is_flagged() {
    let mut net = solved();
    let mut jumper = Branch::new(BusId(7), BusId(10), 0.0, 0.0);
    jumper.uid = Some("jumper".into());
    let mut tail = Bus::new(BusId(10), BusType::Pq, 230.0);
    tail.vm = net.buses()[6].vm;
    tail.va = net.buses()[6].va;
    net.buses_mut().push(tail);
    net.branches_mut().push(jumper);
    let refused = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap_err();
    assert!(matches!(refused, Error::UnmergedZeroImpedance { .. }));
    let message = refused.to_string();
    assert!(message.contains("branch row 5"), "{message}");
    assert!(message.contains("merge"), "{message}");
    assert!(calc_stated_branch_flows(&net).is_err());

    let (merged, merge, _) = powerio_prob::merge_zero_impedance_buses(&net).unwrap();
    let mismatch = calc_stated_state_mismatch(
        &merged,
        &StatedStateOptions::default().with_merged_buses(merge.merged_buses),
    )
    .unwrap();
    assert_closed_except(&mismatch, &[]);
    let survivor = &mismatch.buses[row_of(&mismatch, 7)];
    assert!(survivor.flags.contains(StatedBusFlags::MERGED_GROUP));
}

#[test]
fn top_k_bounds_the_ranked_list() {
    let mut net = solved();
    for line in net.hvdc_mut() {
        line.qf += 3.0;
        line.qt += 2.0;
    }
    let mismatch =
        calc_stated_state_mismatch(&net, &StatedStateOptions::default().with_top_k(1)).unwrap();
    assert_eq!(mismatch.top, [row_of(&mismatch, 3)]);
}

#[test]
fn a_closed_switch_is_refused_until_merged() {
    // Bus 12 hangs off bus 8 through a closed switch and holds a 5 + j2 MVA
    // load that a generator at bus 8 serves. Stamping branches only would
    // show +5 + j2 at bus 8 and -5 - j2 at bus 12 instead of the switch's
    // flow.
    let mut net = solved();
    let mut tail = Bus::new(BusId(12), BusType::Pq, 230.0);
    tail.vm = net.buses()[7].vm;
    tail.va = net.buses()[7].va;
    net.buses_mut().push(tail);
    net.loads_mut().push(Load::new(BusId(12), 5.0, 2.0));
    let mut serving = Generator::new(BusId(8));
    serving.pg = 5.0;
    serving.qg = 2.0;
    net.generators_mut().push(serving);
    net.switches_mut()
        .push(Switch::new(BusId(8), BusId(12), true));

    let refused = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap_err();
    assert_eq!(refused.code().code, "BUILD.SWITCH.CLOSED", "{refused}");
    assert!(
        refused.to_string().contains("--merge-buses switches"),
        "{refused}"
    );
    // Each branch flow depends on its own terminals only.
    assert!(calc_stated_branch_flows(&net).is_ok());

    let merge = net.merge_buses(&BusMergeRule::closed_switches()).unwrap();
    let mismatch = calc_stated_state_mismatch(
        &merge.network,
        &StatedStateOptions::default().with_merged_buses(merge.merged_buses),
    )
    .unwrap();
    assert_closed_except(&mismatch, &[]);
    let survivor = mismatch
        .buses
        .iter()
        .find(|bus| bus.flags.contains(StatedBusFlags::MERGED_GROUP))
        .unwrap();
    assert!([BusId(8), BusId(12)].contains(&survivor.bus));

    // An open switch joins nothing and is not refused.
    net.switches_mut()[0].closed = false;
    net.loads_mut().pop();
    net.generators_mut().pop();
    let open = calc_stated_state_mismatch(&net, &StatedStateOptions::default()).unwrap();
    assert_closed_except(&open, &[]);
}
