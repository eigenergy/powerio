//! HVDC lines enter the balanced power flow instances as fixed injections at
//! their stated terminal powers, unless the caller opts out.

use powerio_core::Source;
use powerio_prob::{AcBusSpecification, AcPfInstance, DcBusSpecification, DcPfInstance};
use powerio_tx::{BalancedNetwork, BusId, HvdcTreatment};

/// Two AC islands, buses 1-2 and 3-4, joined only by one in service HVDC
/// line from bus 2 to bus 4 (100 MW sent, 97 MW delivered, both converters
/// absorbing reactive power). A second, out of service line repeats the path.
const TWO_ISLANDS: &str = "function mpc = two_islands
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
\t1\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t2\t1\t50\t10\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t3\t3\t0\t0\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
\t4\t1\t97\t20\t0\t0\t1\t1\t0\t230\t1\t1.1\t0.9;
];
mpc.gen = [
\t1\t150\t0\t300\t-300\t1\t100\t1\t300\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
\t3\t0\t0\t300\t-300\t1\t100\t1\t300\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0;
];
mpc.branch = [
\t1\t2\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
\t3\t4\t0.01\t0.1\t0\t0\t0\t0\t0\t0\t1\t-360\t360;
];
mpc.gencost = [
\t2\t0\t0\t3\t0.01\t10\t0;
\t2\t0\t0\t3\t0.01\t20\t0;
];
mpc.dcline = [
\t2\t4\t1\t100\t97\t-30\t-25\t1\t1\t0\t200\t-100\t100\t-100\t100\t3\t0;
\t2\t4\t0\t40\t40\t0\t0\t1\t1\t0\t200\t-100\t100\t-100\t100\t0\t0;
];
";

fn two_islands() -> BalancedNetwork {
    let source = Source::from_memory("two_islands.m", TWO_ISLANDS.as_bytes().to_vec())
        .unwrap()
        .with_format(powerio_core::FormatId::new("matpower").unwrap());
    powerio_tx::parse(source)
        .expect("synthetic case parses")
        .into_value()
}

fn dc_injection(instance: &DcPfInstance, bus: usize) -> f64 {
    let row = instance
        .network()
        .buses()
        .iter()
        .position(|b| b.id == BusId(bus))
        .unwrap();
    match instance.specifications()[row] {
        DcBusSpecification::NetActivePower { p_mw } => p_mw,
        other => panic!("bus {bus} states {other:?}"),
    }
}

fn ac_injection(instance: &AcPfInstance, bus: usize) -> (f64, f64) {
    let row = instance
        .network()
        .buses()
        .iter()
        .position(|b| b.id == BusId(bus))
        .unwrap();
    match instance.specifications()[row] {
        AcBusSpecification::Pq { p, q } => (p, q),
        other => panic!("bus {bus} states {other:?}"),
    }
}

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{actual} differs from {expected}"
    );
}

#[test]
fn the_transfer_between_two_islands_enters_both_balances() {
    let instance = DcPfInstance::from_network(two_islands()).unwrap();
    assert_eq!(instance.hvdc_treatment(), HvdcTreatment::FixedInjection);
    // Bus 2 serves 50 MW of load and sends 100 MW; bus 4's 97 MW load is
    // met by the 97 MW the line delivers, so island 3-4 needs nothing from
    // its own generator.
    close(dc_injection(&instance, 2), -150.0);
    close(dc_injection(&instance, 4), 0.0);

    let diagnostics = instance.hvdc_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code(), "BUILD.HVDC.FIXED_INJECTION");
    let message = diagnostics[0].message();
    assert!(
        message.starts_with("1 in service HVDC line(s)"),
        "{message}"
    );
    assert!(message.contains("100 MW withdrawn"), "{message}");
    assert!(message.contains("97 MW delivered"), "{message}");
}

#[test]
fn reactive_injections_follow_the_dcline_sign_convention() {
    let instance = AcPfInstance::from_network(two_islands()).unwrap();
    // QF and QT are injections into the bus, so converters absorbing
    // reactive power add to the demand.
    let (p2, q2) = ac_injection(&instance, 2);
    close(p2, -150.0);
    close(q2, -10.0 - 30.0);
    let (p4, q4) = ac_injection(&instance, 4);
    close(p4, 0.0);
    close(q4, -20.0 - 25.0);
}

#[test]
fn out_of_service_lines_inject_nothing() {
    let mut network = two_islands();
    for line in network.hvdc_mut() {
        line.in_service = false;
    }
    let instance = DcPfInstance::from_network(network).unwrap();
    close(dc_injection(&instance, 2), -50.0);
    close(dc_injection(&instance, 4), -97.0);
    assert!(instance.hvdc_diagnostics().is_empty());
}

#[test]
fn ignoring_hvdc_is_explicit_and_survives_a_network_replacement() {
    let instance =
        DcPfInstance::from_network_with_hvdc(two_islands(), HvdcTreatment::Ignore).unwrap();
    close(dc_injection(&instance, 2), -50.0);
    close(dc_injection(&instance, 4), -97.0);
    let diagnostics = instance.hvdc_diagnostics();
    assert_eq!(diagnostics[0].code(), "BUILD.HVDC.IGNORED");

    let replaced = instance.with_network(two_islands()).unwrap();
    assert_eq!(replaced.hvdc_treatment(), HvdcTreatment::Ignore);
    close(dc_injection(&replaced, 4), -97.0);

    let ac = AcPfInstance::from_network_with_hvdc(two_islands(), HvdcTreatment::Ignore).unwrap();
    close(ac_injection(&ac, 4).1, -20.0);
    let (dc, _) = ac.to_dc_pf();
    assert_eq!(dc.hvdc_treatment(), HvdcTreatment::Ignore);
}

#[test]
fn an_isolated_terminal_bus_blocks_the_line() {
    let mut network = two_islands();
    network.buses_mut()[3].kind = powerio_tx::BusType::Isolated;
    let instance = DcPfInstance::from_network(network).unwrap();
    // The rectifier end cannot send into a de-energized inverter.
    close(dc_injection(&instance, 2), -50.0);
    let codes: Vec<_> = instance
        .hvdc_diagnostics()
        .iter()
        .map(|d| d.code().to_owned())
        .collect();
    assert_eq!(codes, ["BUILD.HVDC.TERMINAL_INACTIVE"]);
}

/// MATPOWER's own `dcline` rows read unchanged, and their injections are the
/// dummy generators MATPOWER's `toggle_dcline` places: `-PF` at the from bus
/// and `+PT` at the to bus of each in service row.
#[test]
fn matpower_dclines_inject_what_toggle_dcline_injects() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/t_case9_dcline.m"
    );
    let network: BalancedNetwork = powerio_tx::parse(Source::open(path).unwrap())
        .unwrap()
        .into_value();
    let rows: Vec<_> = network
        .hvdc()
        .iter()
        .map(|d| (d.from.0, d.to.0, d.in_service, d.pf, d.pt, d.qf, d.qt))
        .collect();
    assert_eq!(
        rows,
        [
            (30, 4, true, 10.0, 8.9, 0.0, 0.0),
            (7, 9, true, 2.0, 1.96, 0.0, 0.0),
            (5, 8, false, 0.0, 0.0, 0.0, 0.0),
            (5, 9, true, 10.0, 9.5, 0.0, 0.0),
        ]
    );
    let instance = DcPfInstance::from_network(network).unwrap();
    let net_p = |bus: usize| -> f64 {
        let row = instance
            .network()
            .buses()
            .iter()
            .position(|b| b.id == BusId(bus))
            .unwrap();
        match instance.specifications()[row] {
            DcBusSpecification::NetActivePower { p_mw } => p_mw,
            other => panic!("bus {bus} states {other:?}"),
        }
    };
    close(net_p(30), 85.0 - 10.0);
    close(net_p(4), 8.9);
    close(net_p(5), -90.0 - 10.0);
    close(net_p(7), -100.0 - 2.0);
    close(net_p(8), 0.0);
    close(net_p(9), -125.0 + 1.96 + 9.5);
}
