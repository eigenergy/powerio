//! A PSS/E load read through the RAW reader closes the bus balance
//! `S = V·conj(Y_bus·V)` at the voltages the case states.
//!
//! PSS/E defines the load's demand at voltage `V` (p.u.) as
//! `P = PL + IP·V + YP·V²` and `Q = QL + IQ·V − YQ·V²`: YQ is the admittance
//! part's reactive power at 1 p.u. and is negative for an inductive load. While
//! DGENF is on, DGENP and DGENQ serve part of the demand. The test states the
//! voltages, computes the injection the network draws from them, writes loads
//! that supply it by that definition, and checks that the parsed loads balance
//! the same `Y_bus·V`.

use powerio_core::Source;
use powerio_matrix::{
    BalancedNetwork, BuildOptions, BusId, IndexedNetwork, calc_admittance_matrix,
};
use powerio_tx::network::LoadVoltageModel;

/// Bus id, VM (p.u.), and VA (degrees).
const VOLTAGES: [(usize, f64, f64); 3] = [(1, 1.0, 0.0), (2, 1.02, -2.0), (3, 0.97, -4.5)];

/// The PSS/E load fields a test load states; PL and QL close the balance.
struct LoadFields {
    bus: usize,
    ip: f64,
    iq: f64,
    yp: f64,
    yq: f64,
    dgenp: f64,
    dgenq: f64,
    dgenf: i32,
}

/// One load per non-reference bus. Bus 2 is inductive with distributed
/// generation on; bus 3 is capacitive with distributed generation stated but
/// off.
const LOADS: [LoadFields; 2] = [
    LoadFields {
        bus: 2,
        ip: 5.0,
        iq: 3.0,
        yp: 4.0,
        yq: -6.0,
        dgenp: 7.0,
        dgenq: 2.0,
        dgenf: 1,
    },
    LoadFields {
        bus: 3,
        ip: 0.0,
        iq: 0.0,
        yp: 2.0,
        yq: 8.0,
        dgenp: 5.0,
        dgenq: 1.0,
        dgenf: 0,
    },
];

fn case(loads: &str) -> String {
    let buses = VOLTAGES
        .iter()
        .map(|(bus, vm, va)| {
            let ide = if *bus == 1 { 3 } else { 1 };
            format!("{bus},'BUS{bus}        ', 230.0,{ide},1,1,1,{vm},{va},1.1,0.9,1.1,0.9")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let zero_ratings = ["0.0"; 12].join(",");
    let branch = |i: usize, j: usize, r: f64, x: f64, b: f64| {
        format!("{i},{j},'1 ',{r},{x},{b},'',{zero_ratings},0.0,0.0,0.0,0.0,1,1,0.0,1,1.0")
    };
    format!(
        "0, 100.00, 35, 0, 1, 60.00 / synthetic
CASE
COMMENT
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
{buses}
0 / END OF BUS DATA, BEGIN LOAD DATA
{loads}
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
{}
{}
{}
0 / END OF BRANCH DATA, BEGIN SYSTEM SWITCHING DEVICE DATA
0 / END OF SYSTEM SWITCHING DEVICE DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
",
        branch(1, 2, 0.01, 0.1, 0.02),
        branch(2, 3, 0.02, 0.15, 0.0),
        branch(1, 3, 0.0, 0.2, 0.01),
    )
}

fn parse(text: &str) -> BalancedNetwork {
    let source = Source::from_memory("case.raw", text.as_bytes().to_vec()).unwrap();
    powerio_tx::format::parse(source).unwrap().value().clone()
}

/// The complex power `V·conj(Y_bus·V)` each bus injects into the network,
/// in MVA, at the stated voltages.
fn injections(net: &BalancedNetwork) -> Vec<(usize, f64, f64)> {
    let view = IndexedNetwork::new(net);
    let y = calc_admittance_matrix(&view, &BuildOptions::default()).unwrap();
    let voltage = |bus: usize| {
        let (_, vm, va) = VOLTAGES.iter().find(|(id, ..)| *id == bus).unwrap();
        let angle = va.to_radians();
        (vm * angle.cos(), vm * angle.sin())
    };
    let n = view.n();
    let mut current = vec![(0.0, 0.0); n];
    let mut id_of = vec![0; n];
    for (bus, ..) in VOLTAGES {
        id_of[view.bus_index(BusId(bus)).unwrap()] = bus;
    }
    for (part, imaginary) in [(&y.g, false), (&y.b, true)] {
        for (&value, (row, col)) in part {
            let (vr, vi) = voltage(id_of[col]);
            let (yr, yi) = if imaginary {
                (0.0, value)
            } else {
                (value, 0.0)
            };
            current[row].0 += yr * vr - yi * vi;
            current[row].1 += yr * vi + yi * vr;
        }
    }
    (0..n)
        .map(|row| {
            let (vr, vi) = voltage(id_of[row]);
            let (ir, ii) = current[row];
            let base = net.base_mva();
            (
                id_of[row],
                (vr * ir + vi * ii) * base,
                (vi * ir - vr * ii) * base,
            )
        })
        .collect()
}

#[test]
fn psse_loads_close_the_bus_balance_at_the_stated_voltages() {
    let network_only = parse(&case(""));
    let drawn = injections(&network_only);

    // Each load supplies what the network draws from its bus, split by the
    // PSS/E definition: PL and QL take what the other parts leave.
    let records = LOADS
        .iter()
        .enumerate()
        .map(|(k, load)| {
            let LoadFields {
                bus,
                ip,
                iq,
                yp,
                yq,
                dgenp,
                dgenq,
                dgenf,
            } = *load;
            let (_, p, q) = drawn.iter().find(|(id, ..)| *id == bus).copied().unwrap();
            let (_, v, _) = VOLTAGES
                .iter()
                .find(|(id, ..)| *id == bus)
                .copied()
                .unwrap();
            let on = f64::from(dgenf);
            let pl = -p - ip * v - yp * v * v + on * dgenp;
            let ql = -q - iq * v + yq * v * v + on * dgenq;
            format!(
                "{bus},'L{k}',1,1,1,{pl},{ql},{ip},{iq},{yp},{yq},1,1,0,{dgenp},{dgenq},{dgenf},''"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let net = parse(&case(&records));
    assert_eq!(net.loads().len(), LOADS.len());

    for (bus, p, q) in injections(&net) {
        let (_, v, _) = VOLTAGES
            .iter()
            .find(|(id, ..)| *id == bus)
            .copied()
            .unwrap();
        let (mut load_p, mut load_q) = (0.0, 0.0);
        for load in net.loads().iter().filter(|load| load.bus.0 == bus) {
            let Some(LoadVoltageModel::Zip {
                p_constant_power,
                q_constant_power,
                p_constant_current,
                q_constant_current,
                p_constant_impedance,
                q_constant_impedance,
                ..
            }) = &load.voltage_model
            else {
                panic!("bus {bus}: the load states ZIP parts");
            };
            load_p += p_constant_power + p_constant_current * v + p_constant_impedance * v * v;
            load_q += q_constant_power + q_constant_current * v + q_constant_impedance * v * v;
        }
        if bus == 1 {
            continue;
        }
        assert!(
            (p + load_p).abs() < 1e-9 && (q + load_q).abs() < 1e-9,
            "bus {bus}: injection {p} + j{q} MVA, load {load_p} + j{load_q} MVA"
        );
    }
}
