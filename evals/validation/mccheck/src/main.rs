//! Dump powerio's native multiconductor nodal admittance as JSON, for the
//! `validate_opendss_admittance.py` validation leg to compare against
//! OpenDSS's own per element primitive admittances (`CktElement.YPrim`).
//!
//! `calc_multiconductor_admittance_matrix` lives in `powerio-matrix` with no Python
//! binding, so this binary is the only way an external oracle can reach it:
//! it parses one distribution deck, builds the admittance, and prints the
//! node list, powerio's own bus/terminal to dense row resolution (so the
//! oracle can fold its own per element rows onto powerio's merged nodes),
//! the nonzero conductance and susceptance entries, the builder's
//! diagnostics, and the ideal equipment constraint row labels.
//!
//! Usage: `powerio-eval-mccheck <path-to-deck.dss>`, one JSON object on
//! stdout.

use powerio_core::Source;
use powerio_matrix::{NodeRef, calc_multiconductor_admittance_matrix};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: powerio-eval-mccheck <deck.dss>");
    let module = powerio_dist::parse(Source::open(&path).unwrap()).unwrap();
    let net = module.value();
    let y = calc_multiconductor_admittance_matrix(net).unwrap();
    let idx = y.index();
    let nodes: Vec<String> = idx
        .nodes()
        .iter()
        .map(|n| format!("{}.{}", n.bus, n.terminal))
        .collect();

    // Every declared (bus, terminal) and where it lands: a dense row, ground,
    // or nothing. This is what lets the oracle fold its own per element rows
    // (which use OpenDSS's unmerged node numbering) onto powerio's nodes,
    // which have already merged closed switch pairs into one electrical node.
    let mut resolution = Vec::new();
    for bus in net.buses() {
        for terminal in &bus.terminals {
            let label = format!("{}.{}", bus.id, terminal);
            let target = match idx.resolve(&bus.id, terminal) {
                Some(NodeRef::Node(row)) => format!("{row}"),
                Some(NodeRef::Ground) => "\"ground\"".to_owned(),
                None => "null".to_owned(),
            };
            resolution.push(format!("[{label:?},{target}]"));
        }
    }

    let mut entries = Vec::new();
    for (r, row) in y.conductance().outer_iterator().enumerate() {
        for (c, &v) in row.iter() {
            entries.push(format!("[{r},{c},{v:e},0.0]"));
        }
    }
    for (r, row) in y.susceptance().outer_iterator().enumerate() {
        for (c, &v) in row.iter() {
            entries.push(format!("[{r},{c},0.0,{v:e}]"));
        }
    }

    let diags: Vec<String> = y
        .diagnostics()
        .iter()
        .map(|d| format!("{:?}", d.message()))
        .collect();
    println!(
        "{{\"nodes\": {:?}, \"resolution\": [{}], \"entries\": [{}], \"diagnostics\": [{}], \"constraint_labels\": {:?} }}",
        nodes,
        resolution.join(","),
        entries.join(","),
        diags.join(","),
        y.augmented().labels
    );
}
