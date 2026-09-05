//! Pre-solver structural readiness checks for distribution networks.
//!
//! Parsing and electrical readiness are deliberately separate. A source can be
//! syntactically valid while the resulting typed network is not safe to hand
//! to a solver or semantic writer. In particular, OpenDSS line geometry is
//! still deferred: until Carson/geometry lowering exists, a geometry-defined
//! line must not be treated as electrically complete merely because the
//! parser has a placeholder linecode.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{DistLineCode, MulticonductorNetwork};

/// Severity of an electrical-readiness finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessSeverity {
    /// The network must not be passed to numerical lowering or solving.
    Blocker,
    /// The network can be used, but the caller should inspect the finding.
    Warning,
}

/// A deterministic finding emitted by the readiness audit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadinessFinding {
    pub severity: ReadinessSeverity,
    pub code: &'static str,
    pub element: String,
    pub message: String,
}

/// Result of [`audit_electrical_readiness`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElectricalReadiness {
    pub findings: Vec<ReadinessFinding>,
}

impl ElectricalReadiness {
    /// Returns `true` only when no blocker is present.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self
            .findings
            .iter()
            .any(|finding| finding.severity == ReadinessSeverity::Blocker)
    }

    /// Returns the findings that make the network unsafe to solve.
    pub fn blockers(&self) -> impl Iterator<Item = &ReadinessFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == ReadinessSeverity::Blocker)
    }

    /// Returns non-fatal findings.
    pub fn warnings(&self) -> impl Iterator<Item = &ReadinessFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == ReadinessSeverity::Warning)
    }

    fn block(&mut self, code: &'static str, element: &str, message: impl Into<String>) {
        self.findings.push(ReadinessFinding {
            severity: ReadinessSeverity::Blocker,
            code,
            element: element.to_owned(),
            message: message.into(),
        });
    }
}

/// Audit a multiconductor network before numerical lowering or solving.
///
/// This check is intentionally conservative and read-only. It does not repair
/// topology, invent conductor counts, or replace missing electrical data with
/// source-format defaults. OpenDSS `geometry=`, `spacing=`, `wires=`,
/// `cncables=`, and `tscables=` metadata are treated as a hard blocker because
/// the geometry family is not yet lowered into the canonical impedance
/// matrices. The audit therefore provides an explicit fail-closed boundary
/// for applications that need to decide whether a parsed network is safe to
/// analyse.
#[must_use]
pub fn audit_electrical_readiness(net: &MulticonductorNetwork) -> ElectricalReadiness {
    let mut report = ElectricalReadiness::default();

    if !net.base_frequency().is_finite() || net.base_frequency() <= 0.0 {
        report.block(
            "READINESS.FREQUENCY.INVALID",
            "network",
            format!(
                "base frequency must be finite and greater than zero; got {}",
                net.base_frequency()
            ),
        );
    }

    let mut buses = BTreeSet::new();
    for bus in net.buses() {
        if !buses.insert(bus.id.to_ascii_lowercase()) {
            report.block(
                "READINESS.BUS.DUPLICATE",
                &bus.id,
                "bus identifier is duplicated case-insensitively",
            );
        }
        if bus.terminals.is_empty() {
            report.block(
                "READINESS.BUS.TERMINALS_EMPTY",
                &bus.id,
                "bus has no terminals",
            );
        }
    }

    let mut linecodes = BTreeMap::new();
    for code in net.line_codes() {
        let key = code.name.to_ascii_lowercase();
        if linecodes.insert(key, code).is_some() {
            report.block(
                "READINESS.LINECODE.DUPLICATE",
                &code.name,
                "linecode name is duplicated case-insensitively",
            );
        }
        audit_linecode(code, &mut report);
    }

    for line in net.lines() {
        if !line.length.is_finite() || line.length <= 0.0 {
            report.block(
                "READINESS.LINE.LENGTH_INVALID",
                &line.name,
                format!(
                    "line length must be finite and greater than zero; got {}",
                    line.length
                ),
            );
        }

        if !buses.contains(&line.bus_from.to_ascii_lowercase()) {
            report.block(
                "READINESS.LINE.BUS_FROM_UNRESOLVED",
                &line.name,
                format!("from-bus {:?} does not exist", line.bus_from),
            );
        }
        if !buses.contains(&line.bus_to.to_ascii_lowercase()) {
            report.block(
                "READINESS.LINE.BUS_TO_UNRESOLVED",
                &line.name,
                format!("to-bus {:?} does not exist", line.bus_to),
            );
        }

        audit_deferred_geometry(line, &mut report);

        let Some(code) = linecodes.get(&line.linecode.to_ascii_lowercase()) else {
            report.block(
                "READINESS.LINE.LINECODE_UNRESOLVED",
                &line.name,
                format!("linecode {:?} does not exist", line.linecode),
            );
            continue;
        };

        if line.terminal_map_from.len() != code.n_conductors
            || line.terminal_map_to.len() != code.n_conductors
        {
            report.block(
                "READINESS.LINE.TERMINAL_COUNT_MISMATCH",
                &line.name,
                format!(
                    "terminal maps have lengths {}/{} but linecode requires {} conductors",
                    line.terminal_map_from.len(),
                    line.terminal_map_to.len(),
                    code.n_conductors
                ),
            );
        }
    }

    report
}

fn audit_deferred_geometry(line: &crate::model::DistLine, report: &mut ElectricalReadiness) {
    const GEOMETRY_KEYS: [&str; 5] = ["geometry", "spacing", "wires", "cncables", "tscables"];
    let keys: Vec<&str> = GEOMETRY_KEYS
        .into_iter()
        .filter(|key| line.extras.contains_key(*key))
        .collect();

    if !keys.is_empty() {
        report.block(
            "READINESS.DSS.GEOMETRY_DEFERRED",
            &line.name,
            format!(
                "OpenDSS geometry-family properties {keys:?} are deferred; no geometry-derived impedance may be assumed"
            ),
        );
    }
}

fn audit_linecode(code: &DistLineCode, report: &mut ElectricalReadiness) {
    if code.n_conductors == 0 {
        report.block(
            "READINESS.LINECODE.CONDUCTOR_COUNT_ZERO",
            &code.name,
            "linecode has zero conductors",
        );
        return;
    }

    audit_square_matrix(
        &code.name,
        "r_series",
        &code.r_series,
        code.n_conductors,
        report,
    );
    audit_square_matrix(
        &code.name,
        "x_series",
        &code.x_series,
        code.n_conductors,
        report,
    );
    audit_square_matrix(
        &code.name,
        "g_from",
        &code.g_from,
        code.n_conductors,
        report,
    );
    audit_square_matrix(
        &code.name,
        "b_from",
        &code.b_from,
        code.n_conductors,
        report,
    );
    audit_square_matrix(&code.name, "g_to", &code.g_to, code.n_conductors, report);
    audit_square_matrix(&code.name, "b_to", &code.b_to, code.n_conductors, report);
}

fn audit_square_matrix(
    element: &str,
    field: &str,
    matrix: &[Vec<f64>],
    n: usize,
    report: &mut ElectricalReadiness,
) {
    if matrix.len() != n || matrix.iter().any(|row| row.len() != n) {
        report.block(
            "READINESS.MATRIX.SHAPE_MISMATCH",
            element,
            format!("{field} must be a {n}x{n} matrix"),
        );
        return;
    }
    if matrix.iter().flatten().any(|value| !value.is_finite()) {
        report.block(
            "READINESS.MATRIX.NONFINITE",
            element,
            format!("{field} contains a non-finite value"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DistBus, DistLine, DistLineCode, Extras, MulticonductorNetwork};

    fn network_with_line(extras: Extras) -> MulticonductorNetwork {
        let mut net = MulticonductorNetwork::new();
        net.buses_mut()
            .push(DistBus::new("source", vec!["1".into()]));
        net.buses_mut().push(DistBus::new("load", vec!["1".into()]));
        net.line_codes_mut().push(DistLineCode::new(
            "explicit",
            vec![vec![0.1]],
            vec![vec![0.2]],
        ));
        let mut line = DistLine::new(
            "l1",
            "source",
            "load",
            vec!["1".into()],
            vec!["1".into()],
            "explicit",
            100.0,
        );
        line.extras = extras;
        net.lines_mut().push(line);
        net
    }

    #[test]
    fn explicit_line_is_ready() {
        let report = audit_electrical_readiness(&network_with_line(Extras::new()));
        assert!(report.is_ready());
        assert_eq!(report.blockers().count(), 0);
    }

    #[test]
    fn geometry_metadata_is_a_hard_blocker() {
        let mut extras = Extras::new();
        extras.insert("geometry".into(), serde_json::json!("g601"));
        let report = audit_electrical_readiness(&network_with_line(extras));
        assert!(!report.is_ready());
        assert!(
            report
                .blockers()
                .any(|finding| finding.code == "READINESS.DSS.GEOMETRY_DEFERRED")
        );
    }

    #[test]
    fn all_deferred_geometry_families_are_hard_blockers() {
        for key in ["geometry", "spacing", "wires", "cncables", "tscables"] {
            let mut extras = Extras::new();
            extras.insert(key.into(), serde_json::json!("deferred"));
            let report = audit_electrical_readiness(&network_with_line(extras));
            assert!(!report.is_ready(), "{key} must block readiness");
            assert_eq!(
                report
                    .blockers()
                    .filter(|finding| finding.code == "READINESS.DSS.GEOMETRY_DEFERRED")
                    .count(),
                1,
                "{key} must produce exactly one deferred-geometry blocker"
            );
        }
    }

    #[test]
    fn swer_geometry_cannot_be_hidden_by_a_one_phase_linecode() {
        let mut extras = Extras::new();
        extras.insert("geometry".into(), serde_json::json!("gswer"));
        let mut net = network_with_line(extras);
        net.lines_mut()[0].terminal_map_from = vec!["1".into()];
        net.lines_mut()[0].terminal_map_to = vec!["1".into()];
        let report = audit_electrical_readiness(&net);
        assert!(!report.is_ready());
        assert!(
            report
                .blockers()
                .any(|finding| finding.code == "READINESS.DSS.GEOMETRY_DEFERRED")
        );
    }

    #[test]
    fn malformed_impedance_matrix_blocks() {
        let mut net = network_with_line(Extras::new());
        net.line_codes_mut()[0].n_conductors = 2;
        let report = audit_electrical_readiness(&net);
        assert!(!report.is_ready());
        assert!(
            report
                .blockers()
                .any(|finding| finding.code == "READINESS.MATRIX.SHAPE_MISMATCH")
        );
    }
}
