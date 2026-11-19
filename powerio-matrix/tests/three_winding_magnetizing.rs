//! Where a three winding transformer's magnetizing admittance connects (#741).
//!
//! PSS/E connects `Ym = MAG1 + jMAG2` from winding 1's bus to ground, outside
//! that winding's ratio. IIDM and CGMES state it at the star point. The two
//! are different circuits, and the lowering has to build the one the record
//! states.

use powerio_matrix::matrix::{BuildOptions, YbusParts, calc_admittance_matrix};
use powerio_tx::{
    BalancedNetwork, Bus, BusId, BusType, Impedance, IndexedNetwork, MagnetizingConnection, Shunt,
    Transformer3W, Winding,
};

const MAG_G: f64 = 0.002;
const MAG_B: f64 = -0.03;
const BASE_MVA: f64 = 100.0;

fn network(mag: (f64, f64), connection: MagnetizingConnection) -> BalancedNetwork {
    let buses = vec![
        Bus::new(BusId(1), BusType::Ref, 230.0),
        Bus::new(BusId(2), BusType::Pq, 115.0),
        Bus::new(BusId(3), BusType::Pq, 13.8),
    ];
    let mut windings = [
        Winding::new(BusId(1)),
        Winding::new(BusId(2)),
        Winding::new(BusId(3)),
    ];
    // An off-nominal winding 1 ratio: the case the star placement got wrong.
    windings[0].tap = 1.05;
    windings[1].tap = 0.98;
    let mut transformer = Transformer3W::new(
        windings,
        [
            Impedance::new(0.002, 0.08, BASE_MVA),
            Impedance::new(0.003, 0.12, BASE_MVA),
            Impedance::new(0.0025, 0.10, BASE_MVA),
        ],
    );
    transformer.mag_g = mag.0;
    transformer.mag_b = mag.1;
    transformer.magnetizing_connection = connection;
    let mut net = BalancedNetwork::in_memory("three-winding", BASE_MVA, buses, Vec::new());
    net.transformers_3w_mut().push(transformer);
    net
}

fn ybus(net: &BalancedNetwork) -> YbusParts {
    calc_admittance_matrix(&IndexedNetwork::new(net), &BuildOptions::default()).unwrap()
}

fn assert_same(left: &YbusParts, right: &YbusParts) {
    for (name, a, b) in [("G", &left.g, &right.g), ("B", &left.b, &right.b)] {
        assert_eq!(a.shape(), b.shape(), "{name} shape");
        let (rows, cols) = a.shape();
        for i in 0..rows {
            for j in 0..cols {
                let x = a.get(i, j).copied().unwrap_or(0.0);
                let y = b.get(i, j).copied().unwrap_or(0.0);
                assert!((x - y).abs() < 1e-12, "{name}[{i},{j}]: {x} != {y}");
            }
        }
    }
}

#[test]
fn a_winding_1_magnetizing_admittance_is_seen_at_bus_1_through_the_ratio() {
    let at_terminal = network((MAG_G, MAG_B), MagnetizingConnection::Winding1Terminal);
    // The same transformer without its admittance, plus Ym as a bus shunt at
    // bus 1 (a raw network states shunts in MW and MVAr at 1 p.u. voltage).
    let mut reference = network((0.0, 0.0), MagnetizingConnection::Winding1Terminal);
    reference
        .shunts_mut()
        .push(Shunt::new(BusId(1), MAG_G * BASE_MVA, MAG_B * BASE_MVA));
    assert_same(&ybus(&at_terminal), &ybus(&reference));
}

#[test]
fn a_star_point_magnetizing_admittance_stays_at_the_star_bus() {
    let at_star = network((MAG_G, MAG_B), MagnetizingConnection::StarPoint);
    let at_terminal = network((MAG_G, MAG_B), MagnetizingConnection::Winding1Terminal);
    let (star, terminal) = (ybus(&at_star), ybus(&at_terminal));
    // The star bus is the analysis row after the three terminal buses.
    let star_row = 3;
    let without = ybus(&network((0.0, 0.0), MagnetizingConnection::StarPoint));
    let diagonal = |parts: &YbusParts, row: usize| {
        (
            parts.g.get(row, row).copied().unwrap_or(0.0),
            parts.b.get(row, row).copied().unwrap_or(0.0),
        )
    };
    let (g, b) = diagonal(&star, star_row);
    let (g0, b0) = diagonal(&without, star_row);
    assert!((g - g0 - MAG_G).abs() < 1e-12 && (b - b0 - MAG_B).abs() < 1e-12);
    // The two placements are different circuits.
    let (g1, b1) = diagonal(&terminal, 0);
    let (g1_star, b1_star) = diagonal(&star, 0);
    assert!((g1 - g1_star - MAG_G).abs() < 1e-12 && (b1 - b1_star - MAG_B).abs() < 1e-12);
}

#[test]
fn a_winding_1_admittance_leaves_with_its_winding() {
    // Winding 1 out of service disconnects its branch, and the magnetizing
    // admittance it carries with it.
    let mut net = network((MAG_G, MAG_B), MagnetizingConnection::Winding1Terminal);
    net.transformers_3w_mut()[0].set_winding_in_service([false, true, true]);
    let mut without = network((0.0, 0.0), MagnetizingConnection::Winding1Terminal);
    without.transformers_3w_mut()[0].set_winding_in_service([false, true, true]);
    assert_same(&ybus(&net), &ybus(&without));
}
