//! Pre-solver structural readiness checks for distribution networks.
//!
//! Parsing and electrical readiness are deliberately separate. A source can be
//! syntactically valid while the resulting typed network is not safe to hand
//! to a solver or semantic writer. Unresolved OpenDSS geometry metadata
//! must remain blocked: a geometry-defined
//! line must not be treated as electrically complete merely because the
//! parser has a placeholder linecode.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{DistLineCode, MulticonductorNetwork};

pub(crate) const DEFERRED_GEOMETRY_KEYS: [&str; 5] =
    ["geometry", "spacing", "wires", "cncables", "tscables"];

/// The line extra the CIM reader sets on a segment whose source states no
/// impedance it can read. The segment's line code is a zero placeholder.
pub(crate) const UNRESOLVED_IMPEDANCE_KEY: &str = "cim_impedance_unresolved";

/// Blocker codes that stop canonical conversion: the line exists, but its
/// impedance is not in the canonical matrices.
const CONVERSION_BLOCKERS: [&str; 2] = [
    "READINESS.DSS.GEOMETRY_DEFERRED",
    "READINESS.CIM_DIST.IMPEDANCE_UNRESOLVED",
];

/// Reject incomplete electrical data before numerical use.
pub fn require_electrical_readiness(
    net: &MulticonductorNetwork,
) -> Result<(), powerio_core::Error> {
    let report = audit_electrical_readiness(net);
    if let Some(finding) = report.blockers().next() {
        return Err(powerio_core::Error::new(
            &crate::diagnostics::codes::BUILD_DIST_ELECTRICAL_INCOMPLETE,
            format!("{} {}: {}", finding.code, finding.element, finding.message),
        ));
    }
    Ok(())
}

/// Refuse a projection that has not implemented terminal-referenced sources.
/// Consumers that support them must instead retain both voltage endpoints.
pub fn require_earth_referenced_sources(
    net: &MulticonductorNetwork,
) -> Result<(), powerio_core::Error> {
    if let Some(source) = net.sources().iter().find(|s| {
        s.reference_terminal.as_ref().is_some_and(|reference| {
            !net.buses()
                .iter()
                .any(|bus| bus.id.eq_ignore_ascii_case(&s.bus) && bus.grounded.contains(reference))
        })
    }) {
        return Err(powerio_core::Error::new(
            &crate::diagnostics::codes::BUILD_DIST_ELECTRICAL_INCOMPLETE,
            format!(
                "source `{}`: this projection does not support reference-terminal voltage constraints",
                source.name
            ),
        ));
    }
    Ok(())
}

/// Reject source-only electrical equipment that a canonical writer cannot
/// reconstruct: OpenDSS geometry lines, and CIM segments whose impedance the
/// reader could not read.
pub(crate) fn require_resolved_geometry(
    net: &MulticonductorNetwork,
) -> Result<(), powerio_core::Error> {
    let report = audit_electrical_readiness(net);
    if let Some(finding) = report
        .blockers()
        .find(|finding| CONVERSION_BLOCKERS.contains(&finding.code))
    {
        return Err(powerio_core::Error::new(
            &crate::diagnostics::codes::BUILD_DIST_ELECTRICAL_INCOMPLETE,
            format!(
                "{}: {}; retain the source module or provide explicit conductor impedances before canonical conversion",
                finding.element, finding.message
            ),
        ));
    }
    Ok(())
}

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

    fn warn(&mut self, code: &'static str, element: &str, message: impl Into<String>) {
        self.findings.push(ReadinessFinding {
            severity: ReadinessSeverity::Warning,
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
/// those retained references have not been lowered into canonical impedance
/// matrices. Resolved geometry carries explicit matrices instead. A CIM segment
/// whose source states no readable impedance is a hard blocker for the same
/// reason. The audit provides a fail-closed boundary for numerical consumers.
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

    let identity = |value: &str| {
        if *net.source_format() == Some(crate::model::DistSourceFormat::Dss) {
            value.to_ascii_lowercase()
        } else {
            value.to_owned()
        }
    };
    audit_untyped(net, &mut report);
    audit_load_voltage(net, &mut report);

    for (element, fields) in net.defaulted() {
        report.warn(
            "READINESS.SOURCE.DEFAULTED",
            element,
            format!("source defaults supplied {}", fields.join(", ")),
        );
    }
    let mut buses = BTreeMap::new();
    for bus in net.buses() {
        if buses.insert(identity(&bus.id), bus).is_some() {
            report.block(
                "READINESS.BUS.DUPLICATE",
                &bus.id,
                "bus identifier is duplicated under the source identifier convention",
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
        let key = identity(&code.name);
        if linecodes.insert(key, code).is_some() {
            report.block(
                "READINESS.LINECODE.DUPLICATE",
                &code.name,
                "linecode name is duplicated under the source identifier convention",
            );
        }
        audit_linecode(code, &mut report);
    }

    audit_lines(net, &buses, &linecodes, identity, &mut report);

    for source in net.sources() {
        let Some(reference) = &source.reference_terminal else {
            continue;
        };
        let declared = buses.get(&identity(&source.bus)).is_some_and(|bus| {
            bus.terminals.contains(reference)
                && source
                    .terminal_map
                    .iter()
                    .all(|t| bus.terminals.contains(t))
        });
        let n = source.terminal_map.len();
        if !declared
            || n == 0
            || source.terminal_map.contains(reference)
            || source.terminal_map.iter().collect::<BTreeSet<_>>().len() != n
            || source.v_magnitude.len() != n
            || source.v_angle.len() != n
            || source
                .v_magnitude
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0)
            || source.v_angle.iter().any(|v| !v.is_finite())
        {
            report.block("READINESS.SOURCE.REFERENCE_INVALID", &source.name,
                "referenced source needs distinct declared terminals and complete finite voltage differences");
        }
    }

    report
}

fn audit_untyped(net: &MulticonductorNetwork, report: &mut ElectricalReadiness) {
    let autotrans_state_command = net.commands().iter().any(|(verb, args)| {
        matches!(verb.as_str(), "enable" | "disable" | "open" | "close")
            && crate::dss::lex::Scanner::new(args, None)
                .next_param()
                .is_some_and(|p| p.value.text.to_ascii_lowercase().starts_with("autotrans."))
    });
    let reactor_state_command = net.commands().iter().any(|(verb, args)| {
        matches!(verb.as_str(), "enable" | "disable" | "open" | "close")
            && crate::dss::lex::Scanner::new(args, None)
                .next_param()
                .is_some_and(|p| p.value.text.to_ascii_lowercase().starts_with("reactor."))
    });
    let storage_state_command = net.commands().iter().any(|(verb, args)| {
        matches!(verb.as_str(), "enable" | "disable" | "open" | "close")
            && crate::dss::lex::Scanner::new(args, None)
                .next_param()
                .is_some_and(|p| p.value.text.to_ascii_lowercase().starts_with("storage."))
    });
    for object in net.untyped_objects() {
        let explicitly_disabled = object
            .props
            .iter()
            .rev()
            .find(|(key, _)| {
                key.as_ref()
                    .is_some_and(|key| key.eq_ignore_ascii_case("enabled"))
            })
            .is_some_and(|(_, value)| {
                value.eq_ignore_ascii_case("no") || value.eq_ignore_ascii_case("false")
            });
        if object.class.eq_ignore_ascii_case("autotrans")
            && (!explicitly_disabled || autotrans_state_command)
        {
            report.block("READINESS.DSS.AUTOTRANS_UNRESOLVED", &object.name,
                "untyped autotransformer has unresolved winding behavior; omitting it can disconnect the feeder");
        }
        if object.class.eq_ignore_ascii_case("reactor")
            && (!explicitly_disabled || reactor_state_command)
        {
            report.block("READINESS.DSS.REACTOR_UNRESOLVED", &object.name,
                "untyped reactor has unresolved electrical behavior; omitting a series reactor can disconnect the feeder");
        }
        if object.class.eq_ignore_ascii_case("storage")
            && (!explicitly_disabled || storage_state_command)
        {
            report.block(
                "READINESS.DSS.STORAGE_UNRESOLVED",
                &object.name,
                "untyped storage has unresolved static power exchange, including idling losses",
            );
        }
        if object.class.eq_ignore_ascii_case("vsource") && !explicitly_disabled {
            report.block("READINESS.DSS.SOURCE_UNRESOLVED", &object.name,
                "untyped voltage source must remain explicitly disabled; enabling it requires electrical source conversion");
        }
        if object.class.eq_ignore_ascii_case("line")
            && !explicitly_disabled
            && object.props.iter().any(|(key, _)| {
                key.as_ref().is_some_and(|key| {
                    DEFERRED_GEOMETRY_KEYS
                        .iter()
                        .any(|candidate| key.eq_ignore_ascii_case(candidate))
                })
            })
        {
            report.block("READINESS.DSS.GEOMETRY_DEFERRED", &object.name,
                "source line geometry has no calculated conductor impedances or resolved terminal map");
        } else if object.class.eq_ignore_ascii_case("line") && !explicitly_disabled {
            report.block("READINESS.DSS.LINE_UNRESOLVED", &object.name,
                "untyped line must remain explicitly disabled; enabling it requires electrical line conversion");
        }
    }
}

fn audit_lines(
    net: &MulticonductorNetwork,
    buses: &BTreeMap<String, &crate::model::DistBus>,
    linecodes: &BTreeMap<String, &DistLineCode>,
    identity: impl Fn(&str) -> String,
    report: &mut ElectricalReadiness,
) {
    let mut lines = BTreeSet::new();
    for line in net.lines() {
        if !lines.insert(identity(&line.name)) {
            report.block(
                "READINESS.LINE.DUPLICATE",
                &line.name,
                "line identifier is duplicated",
            );
        }
        for (bus_id, terminals) in [
            (&line.bus_from, &line.terminal_map_from),
            (&line.bus_to, &line.terminal_map_to),
        ] {
            if let Some(bus) = buses.get(&identity(bus_id)) {
                for terminal in terminals {
                    if !bus.terminals.contains(terminal) {
                        report.block(
                            "READINESS.LINE.TERMINAL_UNRESOLVED",
                            &line.name,
                            format!("bus {bus_id} does not declare terminal {terminal}"),
                        );
                    }
                }
            }
        }
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

        if !buses.contains_key(&identity(&line.bus_from)) {
            report.block(
                "READINESS.LINE.BUS_FROM_UNRESOLVED",
                &line.name,
                format!("from-bus {:?} does not exist", line.bus_from),
            );
        }
        if !buses.contains_key(&identity(&line.bus_to)) {
            report.block(
                "READINESS.LINE.BUS_TO_UNRESOLVED",
                &line.name,
                format!("to-bus {:?} does not exist", line.bus_to),
            );
        }

        audit_deferred_geometry(line, report);
        if line
            .extras
            .get(UNRESOLVED_IMPEDANCE_KEY)
            .is_some_and(|v| v.as_bool() == Some(true))
        {
            report.block(
                "READINESS.CIM_DIST.IMPEDANCE_UNRESOLVED",
                &line.name,
                "the CIM source states no impedance the reader can use; its zero placeholder \
                 impedance would be a short circuit",
            );
        }

        let Some(code) = linecodes.get(&identity(&line.linecode)) else {
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
}

fn audit_deferred_geometry(line: &crate::model::DistLine, report: &mut ElectricalReadiness) {
    let keys: Vec<&str> = DEFERRED_GEOMETRY_KEYS
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

fn audit_load_voltage(net: &MulticonductorNetwork, report: &mut ElectricalReadiness) {
    for load in net.loads() {
        if matches!(
            load.voltage_model,
            crate::DistLoadVoltageModel::BoundedZip { .. }
                | crate::DistLoadVoltageModel::BoundedExponential { .. }
                | crate::DistLoadVoltageModel::BoundedConstantPower { .. }
        ) {
            let n = load.p_nom.len().max(load.q_nom.len());
            if n == 0 || load.voltage_model.v_nom().len() != n {
                report.block(
                    "READINESS.LOAD.VOLTAGE_LAW",
                    &load.name,
                    "bounded load law must align with nominal power branches",
                );
            }
            for phase in 0..n {
                let v = load
                    .voltage_model
                    .v_nom()
                    .get(phase)
                    .copied()
                    .unwrap_or(f64::NAN);
                if let Err(error) = load.voltage_model.power_scale(phase, v) {
                    report.block("READINESS.LOAD.VOLTAGE_LAW", &load.name, error);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DistBus, DistLine, DistLineCode, Extras, MulticonductorNetwork};

    #[test]
    fn unresolved_series_reactor_is_a_blocker_unless_disabled() {
        let mut net = MulticonductorNetwork::new();
        net.untyped_objects_mut()
            .push(crate::model::UntypedObject::new(
                "reactor",
                "feed",
                vec![
                    (Some("bus1".into()), "source".into()),
                    (Some("bus2".into()), "feeder".into()),
                    (Some("x".into()), "15".into()),
                ],
            ));
        let report = audit_electrical_readiness(&net);
        assert!(
            report
                .blockers()
                .any(|f| f.code == "READINESS.DSS.REACTOR_UNRESOLVED")
        );
        assert!(require_electrical_readiness(&net).is_err());
        net.untyped_objects_mut()[0]
            .props
            .push((Some("enabled".into()), "no".into()));
        assert!(audit_electrical_readiness(&net).is_ready());
    }

    #[test]
    fn inactive_source_metadata_cannot_be_reenabled_without_electrical_conversion() {
        let mut net = MulticonductorNetwork::new();
        net.untyped_objects_mut()
            .push(crate::model::UntypedObject::new(
                "vsource",
                "off",
                vec![(Some("enabled".into()), "no".into())],
            ));
        assert!(audit_electrical_readiness(&net).is_ready());
        net.untyped_objects_mut()[0]
            .props
            .push((Some("enabled".into()), "yes".into()));
        assert!(!audit_electrical_readiness(&net).is_ready());
        net.untyped_objects_mut()[0].props.clear();
        assert!(!audit_electrical_readiness(&net).is_ready());
    }

    #[test]
    fn grounded_voltage_reference_is_earth_but_a_floating_reference_is_not() {
        let mut net = MulticonductorNetwork::new();
        let mut bus = DistBus::new("feed", vec!["1".into(), "4".into()]);
        bus.grounded.push("4".into());
        net.buses_mut().push(bus);
        let mut source =
            crate::VoltageSource::new("s", "feed", vec!["1".into()], vec![240.0], vec![0.0]);
        source.reference_terminal = Some("4".into());
        net.sources_mut().push(source);
        assert!(require_earth_referenced_sources(&net).is_ok());
        net.buses_mut()[0].grounded.clear();
        assert!(require_earth_referenced_sources(&net).is_err());
    }

    #[test]
    fn inactive_line_metadata_cannot_be_reenabled_without_electrical_conversion() {
        let mut net = MulticonductorNetwork::new();
        net.untyped_objects_mut()
            .push(crate::model::UntypedObject::new(
                "line",
                "off",
                vec![(Some("enabled".into()), "no".into())],
            ));
        assert!(audit_electrical_readiness(&net).is_ready());
        net.untyped_objects_mut()[0]
            .props
            .push((Some("enabled".into()), "yes".into()));
        let report = audit_electrical_readiness(&net);
        assert!(
            report
                .blockers()
                .any(|f| f.code == "READINESS.DSS.LINE_UNRESOLVED")
        );
        net.untyped_objects_mut()[0].props.clear();
        assert!(!audit_electrical_readiness(&net).is_ready());
    }

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
    fn unresolved_cim_impedance_blocks_conversion() {
        let mut extras = Extras::new();
        extras.insert(UNRESOLVED_IMPEDANCE_KEY.into(), serde_json::json!(true));
        let net = network_with_line(extras);
        let report = audit_electrical_readiness(&net);
        assert!(
            report
                .blockers()
                .any(|finding| finding.code == "READINESS.CIM_DIST.IMPEDANCE_UNRESOLVED")
        );
        assert!(require_resolved_geometry(&net).is_err());
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
