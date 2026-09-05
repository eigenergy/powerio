//! Pre-solver structural and numerical readiness checks for distribution models.
//!
//! Parsing answers whether a source can be decoded. Readiness answers a
//! different question: whether the resulting typed network is safe to hand to
//! a numerical solver or lowering pass without silently repairing topology or
//! fabricating electrical data. The audit is deliberately read-only and does
//! not mutate or default the model.

use std::collections::BTreeSet;

use crate::model::{DistLineCode, MulticonductorNetwork};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessSeverity { Blocker, Warning }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadinessFinding {
    pub severity: ReadinessSeverity,
    pub code: &'static str,
    pub element: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElectricalReadiness { pub findings: Vec<ReadinessFinding> }

impl ElectricalReadiness {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.findings.iter().any(|f| f.severity == ReadinessSeverity::Blocker)
    }
    #[must_use]
    pub fn blockers(&self) -> impl Iterator<Item = &ReadinessFinding> {
        self.findings.iter().filter(|f| f.severity == ReadinessSeverity::Blocker)
    }
    #[must_use]
    pub fn warnings(&self) -> impl Iterator<Item = &ReadinessFinding> {
        self.findings.iter().filter(|f| f.severity == ReadinessSeverity::Warning)
    }
    fn block(&mut self, code: &'static str, element: &str, message: String) {
        self.findings.push(ReadinessFinding { severity: ReadinessSeverity::Blocker, code, element: element.to_owned(), message });
    }
    fn warn(&mut self, code: &'static str, element: &str, message: String) {
        self.findings.push(ReadinessFinding { severity: ReadinessSeverity::Warning, code, element: element.to_owned(), message });
    }
}

/// Audit a multiconductor network before numerical lowering or solving.
///
/// The audit is intentionally conservative. Unresolved topology, malformed
/// impedance matrices, non-finite frequency/length, and terminal-count
/// mismatches block the model. Parser-recorded defaults remain visible as
/// warnings instead of being mistaken for explicit source data.
#[must_use]
pub fn audit_electrical_readiness(net: &MulticonductorNetwork) -> ElectricalReadiness {
    let mut out = ElectricalReadiness::default();

    if !net.base_frequency().is_finite() || net.base_frequency() <= 0.0 {
        out.block("READINESS.FREQUENCY.INVALID", "network", format!("base frequency must be finite and greater than zero; got {}", net.base_frequency()));
    }

    let mut bus_ids = BTreeSet::new();
    for bus in net.buses() {
        if !bus_ids.insert(bus.id.to_ascii_lowercase()) {
            out.block("READINESS.BUS.DUPLICATE", &bus.id, "bus identifier is duplicated (case-insensitively)".into());
        }
        if bus.terminals.is_empty() {
            out.block("READINESS.BUS.TERMINALS_EMPTY", &bus.id, "bus has no terminals".into());
        }
    }

    let mut linecode_names = BTreeSet::new();
    for code in net.linecodes() {
        if !linecode_names.insert(code.name.to_ascii_lowercase()) {
            out.block("READINESS.LINECODE.DUPLICATE", &code.name, "linecode name is duplicated (case-insensitively)".into());
        }
        audit_linecode(code, &mut out);
    }

    let mut line_names = BTreeSet::new();
    for line in net.lines() {
        if !line_names.insert(line.name.to_ascii_lowercase()) {
            out.block("READINESS.LINE.DUPLICATE", &line.name, "line name is duplicated (case-insensitively)".into());
        }
        if !line.length.is_finite() || line.length <= 0.0 {
            out.block("READINESS.LINE.LENGTH_INVALID", &line.name, format!("line length must be finite and greater than zero; got {}", line.length));
        }
        if net.bus(&line.bus_from).is_none() {
            out.block("READINESS.LINE.BUS_FROM_UNRESOLVED", &line.name, format!("from-bus {:?} does not exist", line.bus_from));
        }
        if net.bus(&line.bus_to).is_none() {
            out.block("READINESS.LINE.BUS_TO_UNRESOLVED", &line.name, format!("to-bus {:?} does not exist", line.bus_to));
        }
        let Some(code) = net.linecode(&line.linecode) else {
            out.block("READINESS.LINE.LINECODE_UNRESOLVED", &line.name, format!("linecode {:?} does not exist", line.linecode));
            continue;
        };
        if line.terminal_map_from.len() != code.n_conductors || line.terminal_map_to.len() != code.n_conductors {
            out.block("READINESS.LINE.TERMINAL_COUNT_MISMATCH", &line.name, format!("terminal maps have lengths {}/{} but linecode requires {} conductors", line.terminal_map_from.len(), line.terminal_map_to.len(), code.n_conductors));
        }
    }

    for (element, fields) in net.defaulted() {
        for field in fields {
            out.warn("READINESS.SOURCE.DEFAULTED", element, format!("field {field:?} was materialized from a source-format default"));
        }
    }
    out
}

fn audit_linecode(code: &DistLineCode, out: &mut ElectricalReadiness) {
    if code.n_conductors == 0 {
        out.block("READINESS.LINECODE.CONDUCTOR_COUNT_ZERO", &code.name, "linecode has zero conductors".into());
        return;
    }
    audit_square_matrix(&code.name, "r_series", &code.r_series, code.n_conductors, out);
    audit_square_matrix(&code.name, "x_series", &code.x_series, code.n_conductors, out);
    audit_square_matrix(&code.name, "g_from", &code.g_from, code.n_conductors, out);
    audit_square_matrix(&code.name, "b_from", &code.b_from, code.n_conductors, out);
    audit_square_matrix(&code.name, "g_to", &code.g_to, code.n_conductors, out);
    audit_square_matrix(&code.name, "b_to", &code.b_to, code.n_conductors, out);
}

fn audit_square_matrix(element: &str, field: &str, matrix: &[Vec<f64>], n: usize, out: &mut ElectricalReadiness) {
    if matrix.len() != n || matrix.iter().any(|row| row.len() != n) {
        out.block("READINESS.MATRIX.SHAPE_MISMATCH", element, format!("{field} must be a {n}x{n} matrix"));
        return;
    }
    if matrix.iter().flatten().any(|value| !value.is_finite()) {
        out.block("READINESS.MATRIX.NONFINITE", element, format!("{field} contains a non-finite value"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DistBus, DistLine, DistLineCode, MulticonductorNetwork};

    fn linecode() -> DistLineCode { DistLineCode::new("lc", vec![vec![0.1]], vec![vec![0.2]]) }

    #[test]
    fn valid_network_is_ready() {
        let mut net = MulticonductorNetwork::new();
        net.buses_mut().push(DistBus::new("a", vec!["1".into()]));
        net.buses_mut().push(DistBus::new("b", vec!["1".into()]));
        net.linecodes_mut().push(linecode());
        net.lines_mut().push(DistLine::new("l", "a", "b", vec!["1".into()], vec!["1".into()], "lc", 100.0));
        assert!(audit_electrical_readiness(&net).is_ready());
    }

    #[test]
    fn unresolved_linecode_blocks() {
        let mut net = MulticonductorNetwork::new();
        net.buses_mut().push(DistBus::new("a", vec!["1".into()]));
        net.buses_mut().push(DistBus::new("b", vec!["1".into()]));
        net.lines_mut().push(DistLine::new("l", "a", "b", vec!["1".into()], vec!["1".into()], "missing", 100.0));
        let report = audit_electrical_readiness(&net);
        assert!(!report.is_ready());
        assert!(report.blockers().any(|f| f.code == "READINESS.LINE.LINECODE_UNRESOLVED"));
    }

    #[test]
    fn malformed_matrix_blocks() {
        let mut net = MulticonductorNetwork::new();
        net.buses_mut().push(DistBus::new("a", vec!["1", "2"]));
        net.buses_mut().push(DistBus::new("b", vec!["1", "2"]));
        let mut code = linecode();
        code.n_conductors = 2;
        net.linecodes_mut().push(code);
        let report = audit_electrical_readiness(&net);
        assert!(!report.is_ready());
        assert!(report.blockers().any(|f| f.code == "READINESS.MATRIX.SHAPE_MISMATCH"));
    }
}
