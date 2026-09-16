//! Audited, format-neutral network preparation for LinDist3Flow.
//!
//! The input network is never mutated. The returned network contains only
//! transformations selected by the caller's policy, and every material
//! transformation or omission has one typed action in the report.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Configuration, DistCapacitor, DistGenerator, DistIbr, DistLine, DistLineCode,
    DistLoadVoltageModel, DistShunt, Error, IbrPrimeMover, IbrTopology, MulticonductorNetwork,
    NeutralKronOptions, NeutralKronReport, Result, neutral_kron_reduce,
};

/// How aggressively the distribution network may be prepared for the
/// LinDist3Flow formulation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LinDist3FlowPreparationPolicy {
    /// Do not transform unsupported data.
    #[default]
    Reject,
    /// Apply representation-preserving static lowerings.
    Lower,
    /// Additionally apply named affine/static approximations.
    Approximate,
    /// Additionally omit typed or retained data that has no formulation role.
    Permissive,
}

impl LinDist3FlowPreparationPolicy {
    const fn level(self) -> u8 {
        match self {
            Self::Reject => 0,
            Self::Lower => 1,
            Self::Approximate => 2,
            Self::Permissive => 3,
        }
    }

    const fn at_least(self, required: Self) -> bool {
        self.level() >= required.level()
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Lower => "lower",
            Self::Approximate => "approximate",
            Self::Permissive => "permissive",
        }
    }
}

/// One auditable preparation decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LinDist3FlowPreparationActionKind {
    NeutralKronReduced,
    OpenSwitchOmitted,
    ClosedSwitchLowered,
    LineShuntLowered,
    CapacitorLowered,
    LoadApproximated,
    IbrApproximated,
    StaticControlFrozen,
    UntypedObjectOmitted,
    InitialPointOmitted,
}

impl LinDist3FlowPreparationActionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NeutralKronReduced => "neutral_kron_reduced",
            Self::OpenSwitchOmitted => "open_switch_omitted",
            Self::ClosedSwitchLowered => "closed_switch_lowered",
            Self::LineShuntLowered => "line_shunt_lowered",
            Self::CapacitorLowered => "capacitor_lowered",
            Self::LoadApproximated => "load_approximated",
            Self::IbrApproximated => "ibr_approximated",
            Self::StaticControlFrozen => "static_control_frozen",
            Self::UntypedObjectOmitted => "untyped_object_omitted",
            Self::InitialPointOmitted => "initial_point_omitted",
        }
    }

    /// Whether this action changes component behavior rather than only its
    /// representation.
    #[must_use]
    pub const fn is_approximation(self) -> bool {
        matches!(
            self,
            Self::LoadApproximated
                | Self::IbrApproximated
                | Self::StaticControlFrozen
                | Self::UntypedObjectOmitted
                | Self::InitialPointOmitted
        )
    }

    /// Whether the source record is intentionally absent from the prepared
    /// formulation network.
    #[must_use]
    pub const fn is_omission(self) -> bool {
        matches!(
            self,
            Self::OpenSwitchOmitted
                | Self::StaticControlFrozen
                | Self::UntypedObjectOmitted
                | Self::InitialPointOmitted
        )
    }
}

/// One source component and the prepared representation chosen for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct LinDist3FlowPreparationAction {
    pub kind: LinDist3FlowPreparationActionKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

impl LinDist3FlowPreparationAction {
    /// Create one typed action. Callers may add machine-readable `details`
    /// before appending it to a report.
    #[must_use]
    pub fn new(
        kind: LinDist3FlowPreparationActionKind,
        source: impl Into<String>,
        target: Option<String>,
    ) -> Self {
        Self {
            kind,
            source: source.into(),
            target,
            details: BTreeMap::new(),
        }
    }
}

/// Complete provenance for a LinDist3Flow preparation pass.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LinDist3FlowPreparationReport {
    pub policy: LinDist3FlowPreparationPolicy,
    pub actions: Vec<LinDist3FlowPreparationAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neutral_kron: Option<NeutralKronReport>,
}

/// Independently owned prepared network and its audit report.
#[derive(Clone, Debug)]
pub struct LinDist3FlowPreparedNetwork {
    network: MulticonductorNetwork,
    report: LinDist3FlowPreparationReport,
}

impl LinDist3FlowPreparedNetwork {
    #[must_use]
    pub fn network(&self) -> &MulticonductorNetwork {
        &self.network
    }

    #[must_use]
    pub const fn report(&self) -> &LinDist3FlowPreparationReport {
        &self.report
    }

    #[must_use]
    pub fn into_parts(self) -> (MulticonductorNetwork, LinDist3FlowPreparationReport) {
        (self.network, self.report)
    }
}

fn fail(message: impl Into<String>) -> Error {
    Error::LinDist3FlowPreparation {
        message: message.into(),
    }
}

fn unique_name(prefix: &str, names: &mut BTreeSet<String>) -> String {
    let mut candidate = prefix.to_owned();
    let mut suffix = 2usize;
    while !names.insert(candidate.to_ascii_lowercase()) {
        candidate = format!("{prefix}-{suffix}");
        suffix += 1;
    }
    candidate
}

fn zero_matrix(n: usize) -> Vec<Vec<f64>> {
    vec![vec![0.0; n]; n]
}

fn scaled(matrix: &[Vec<f64>], scale: f64) -> Vec<Vec<f64>> {
    matrix
        .iter()
        .map(|row| row.iter().map(|value| value * scale).collect())
        .collect()
}

fn nonzero(matrix: &[Vec<f64>]) -> bool {
    matrix.iter().flatten().any(|value| *value != 0.0)
}

fn lower_line_shunts(
    network: &mut MulticonductorNetwork,
    report: &mut LinDist3FlowPreparationReport,
) -> Result<()> {
    let lines = network.lines().clone();
    let mut names = network
        .shunts()
        .iter()
        .map(|shunt| shunt.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut additions = Vec::new();
    for line in &lines {
        let code = network.linecode(&line.linecode).ok_or_else(|| {
            fail(format!(
                "line `{}` names missing linecode `{}`",
                line.name, line.linecode
            ))
        })?;
        let n = line.terminal_map_from.len();
        if line.terminal_map_to.len() != n
            || [&code.g_from, &code.b_from, &code.g_to, &code.b_to]
                .into_iter()
                .any(|matrix| matrix.len() != n || matrix.iter().any(|row| row.len() != n))
        {
            return Err(fail(format!(
                "line `{}` shunt matrices do not align with its terminal maps",
                line.name
            )));
        }
        for (side, bus, terminals, g, b) in [
            (
                "from",
                &line.bus_from,
                &line.terminal_map_from,
                &code.g_from,
                &code.b_from,
            ),
            (
                "to",
                &line.bus_to,
                &line.terminal_map_to,
                &code.g_to,
                &code.b_to,
            ),
        ] {
            if !nonzero(g) && !nonzero(b) {
                continue;
            }
            let name = unique_name(
                &format!("__l3f-line-{}-{side}-shunt", line.name),
                &mut names,
            );
            additions.push(DistShunt::new(
                &name,
                bus,
                terminals.clone(),
                scaled(g, line.length),
                scaled(b, line.length),
            ));
            report.actions.push(LinDist3FlowPreparationAction::new(
                LinDist3FlowPreparationActionKind::LineShuntLowered,
                format!("line.{}", line.name),
                Some(format!("shunt.{name}")),
            ));
        }
    }
    network.shunts_mut().extend(additions);
    for code in network.line_codes_mut() {
        code.g_from = zero_matrix(code.n_conductors);
        code.b_from = zero_matrix(code.n_conductors);
        code.g_to = zero_matrix(code.n_conductors);
        code.b_to = zero_matrix(code.n_conductors);
    }
    Ok(())
}

fn lower_switches(
    network: &mut MulticonductorNetwork,
    report: &mut LinDist3FlowPreparationReport,
) -> Result<()> {
    let switches = std::mem::take(network.switches_mut());
    let mut line_names = network
        .lines()
        .iter()
        .map(|line| line.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut code_names = network
        .line_codes()
        .iter()
        .map(|code| code.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for switch in switches {
        let source = format!("switch.{}", switch.name);
        if switch.open {
            report.actions.push(LinDist3FlowPreparationAction::new(
                LinDist3FlowPreparationActionKind::OpenSwitchOmitted,
                source,
                None,
            ));
            continue;
        }
        let n = switch.terminal_map_from.len();
        if n == 0 || switch.terminal_map_to.len() != n {
            return Err(fail(format!(
                "closed switch `{}` has empty or unequal terminal maps",
                switch.name
            )));
        }
        if switch
            .i_max
            .as_ref()
            .is_some_and(|limits| limits.len() != n)
        {
            return Err(fail(format!(
                "closed switch `{}` current limits do not align with its terminal maps",
                switch.name
            )));
        }
        let line_name = unique_name(&format!("__l3f-switch-{}", switch.name), &mut line_names);
        let code_name = unique_name(
            &format!("__l3f-switch-{}-ideal", switch.name),
            &mut code_names,
        );
        network.line_codes_mut().push(DistLineCode::new(
            &code_name,
            zero_matrix(n),
            zero_matrix(n),
        ));
        let mut line = DistLine::new(
            &line_name,
            switch.bus_from,
            switch.bus_to,
            switch.terminal_map_from,
            switch.terminal_map_to,
            code_name,
            1.0,
        );
        line.i_max = switch.i_max;
        network.lines_mut().push(line);
        let mut action = LinDist3FlowPreparationAction::new(
            LinDist3FlowPreparationActionKind::ClosedSwitchLowered,
            source,
            Some(format!("line.{line_name}")),
        );
        action.details.insert(
            "flow_identity".to_owned(),
            json!("independent_contact_flow"),
        );
        report.actions.push(action);
    }
    Ok(())
}

fn capacitor_matrix(capacitor: &DistCapacitor) -> Result<Vec<Vec<f64>>> {
    if !capacitor.q_rated.is_finite()
        || !capacitor.v_nom.is_finite()
        || capacitor.v_nom <= 0.0
        || capacitor.terminal_map.is_empty()
    {
        return Err(fail(format!(
            "capacitor `{}` has invalid rating, nominal voltage, or terminal map",
            capacitor.name
        )));
    }
    let n = capacitor.terminal_map.len();
    match capacitor.configuration {
        Configuration::Wye if n == 1 => Ok(vec![vec![capacitor.q_rated / capacitor.v_nom.powi(2)]]),
        Configuration::Wye if n == 3 => {
            let value = capacitor.q_rated / capacitor.v_nom.powi(2);
            Ok((0..3)
                .map(|row| {
                    (0..3)
                        .map(|column| if row == column { value } else { 0.0 })
                        .collect()
                })
                .collect())
        }
        Configuration::SinglePhase if n == 2 => {
            let value = capacitor.q_rated / capacitor.v_nom.powi(2);
            Ok(vec![vec![value, -value], vec![-value, value]])
        }
        Configuration::Delta if n == 3 => {
            let branch = capacitor.q_rated / (3.0 * capacitor.v_nom.powi(2));
            Ok(vec![
                vec![2.0 * branch, -branch, -branch],
                vec![-branch, 2.0 * branch, -branch],
                vec![-branch, -branch, 2.0 * branch],
            ])
        }
        _ => Err(fail(format!(
            "capacitor `{}` {:?} connection with {n} terminals is not supported",
            capacitor.name, capacitor.configuration
        ))),
    }
}

fn lower_capacitors(
    network: &mut MulticonductorNetwork,
    report: &mut LinDist3FlowPreparationReport,
) -> Result<()> {
    let capacitors = std::mem::take(network.capacitors_mut());
    let mut names = network
        .shunts()
        .iter()
        .map(|shunt| shunt.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for capacitor in capacitors {
        let matrix = capacitor_matrix(&capacitor)?;
        let name = unique_name(&format!("__l3f-capacitor-{}", capacitor.name), &mut names);
        network.shunts_mut().push(DistShunt::new(
            &name,
            &capacitor.bus,
            capacitor.terminal_map.clone(),
            zero_matrix(matrix.len()),
            matrix,
        ));
        report.actions.push(LinDist3FlowPreparationAction::new(
            LinDist3FlowPreparationActionKind::CapacitorLowered,
            format!("capacitor.{}", capacitor.name),
            Some(format!("shunt.{name}")),
        ));
    }
    Ok(())
}

fn approximate_loads(
    network: &mut MulticonductorNetwork,
    report: &mut LinDist3FlowPreparationReport,
) -> Result<()> {
    let expand = |values: &[f64], channels: usize, label: &str| -> Result<Vec<f64>> {
        match values.len() {
            1 => Ok(vec![values[0]; channels]),
            length if length == channels => Ok(values.to_vec()),
            length => Err(fail(format!(
                "{label} has length {length}, expected 1 or {channels}"
            ))),
        }
    };
    for load in network.loads_mut() {
        let approximation = match &load.voltage_model {
            DistLoadVoltageModel::ConstantCurrent { v_nom } => Some(DistLoadVoltageModel::Zip {
                v_nom: v_nom.clone(),
                alpha_z: vec![0.5; load.p_nom.len()],
                alpha_i: vec![0.0; load.p_nom.len()],
                alpha_p: vec![0.5; load.p_nom.len()],
                beta_z: vec![0.5; load.q_nom.len()],
                beta_i: vec![0.0; load.q_nom.len()],
                beta_p: vec![0.5; load.q_nom.len()],
            }),
            DistLoadVoltageModel::Zip {
                v_nom,
                alpha_z,
                alpha_i,
                alpha_p,
                beta_z,
                beta_i,
                beta_p,
            } if alpha_i.iter().any(|value| value.abs() > f64::EPSILON)
                || beta_i.iter().any(|value| value.abs() > f64::EPSILON) =>
            {
                let combine = |z: &[f64],
                               i: &[f64],
                               p: &[f64],
                               channels: usize|
                 -> Result<(Vec<f64>, Vec<f64>)> {
                    let z = expand(z, channels, "ZIP impedance fraction")?;
                    let i = expand(i, channels, "ZIP current fraction")?;
                    let p = expand(p, channels, "ZIP power fraction")?;
                    Ok((
                        z.iter().zip(&i).map(|(z, i)| z + 0.5 * i).collect(),
                        p.iter().zip(&i).map(|(p, i)| p + 0.5 * i).collect(),
                    ))
                };
                let (alpha_z, alpha_p) = combine(alpha_z, alpha_i, alpha_p, load.p_nom.len())?;
                let (beta_z, beta_p) = combine(beta_z, beta_i, beta_p, load.q_nom.len())?;
                Some(DistLoadVoltageModel::Zip {
                    v_nom: v_nom.clone(),
                    alpha_z,
                    alpha_i: vec![0.0; load.p_nom.len()],
                    alpha_p,
                    beta_z,
                    beta_i: vec![0.0; load.q_nom.len()],
                    beta_p,
                })
            }
            DistLoadVoltageModel::Exponential {
                v_nom,
                gamma_p,
                gamma_q,
            } => {
                let gamma_p = expand(gamma_p, load.p_nom.len(), "active exponent")?;
                let gamma_q = expand(gamma_q, load.q_nom.len(), "reactive exponent")?;
                Some(DistLoadVoltageModel::Zip {
                    v_nom: v_nom.clone(),
                    alpha_z: gamma_p.iter().map(|gamma| 0.5 * gamma).collect(),
                    alpha_i: vec![0.0; load.p_nom.len()],
                    alpha_p: gamma_p.iter().map(|gamma| 1.0 - 0.5 * gamma).collect(),
                    beta_z: gamma_q.iter().map(|gamma| 0.5 * gamma).collect(),
                    beta_i: vec![0.0; load.q_nom.len()],
                    beta_p: gamma_q.iter().map(|gamma| 1.0 - 0.5 * gamma).collect(),
                })
            }
            _ => None,
        };
        if let Some(approximation) = approximation {
            load.voltage_model = approximation;
            let mut action = LinDist3FlowPreparationAction::new(
                LinDist3FlowPreparationActionKind::LoadApproximated,
                format!("load.{}", load.name),
                Some(format!("load.{}", load.name)),
            );
            action.details.insert(
                "method".to_owned(),
                json!("first_order_squared_voltage_at_nominal"),
            );
            report.actions.push(action);
        }
    }
    Ok(())
}

fn ibr_configuration(ibr: &DistIbr) -> Result<Configuration> {
    match (ibr.topology, ibr.terminal_map.len(), ibr.s_max.len()) {
        (IbrTopology::SinglePhase, 1, 1) => Ok(Configuration::Wye),
        (IbrTopology::SinglePhase, 2, 1) => Ok(Configuration::SinglePhase),
        (IbrTopology::ThreeLeg | IbrTopology::FourLeg, terminals, phases)
            if terminals == phases =>
        {
            Ok(Configuration::Wye)
        }
        _ => Err(fail(format!(
            "IBR `{}` topology and terminal/rating dimensions cannot be mapped to a static generator",
            ibr.name
        ))),
    }
}

fn approximate_ibrs(
    network: &mut MulticonductorNetwork,
    report: &mut LinDist3FlowPreparationReport,
) -> Result<()> {
    let ibrs = std::mem::take(network.ibrs_mut());
    let mut names = network
        .generators()
        .iter()
        .map(|generator| generator.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for ibr in ibrs {
        let channels = ibr.s_max.len();
        if channels == 0
            || ibr
                .s_max
                .iter()
                .any(|rating| !rating.is_finite() || *rating <= 0.0)
        {
            return Err(fail(format!(
                "IBR `{}` has invalid phase ratings",
                ibr.name
            )));
        }
        let configuration = ibr_configuration(&ibr)?;
        let name = unique_name(&format!("__l3f-ibr-{}", ibr.name), &mut names);
        let total_rating = ibr.s_max.iter().sum::<f64>();
        let p_nom = ibr.p_avail.map_or_else(
            || vec![0.0; channels],
            |available| {
                ibr.s_max
                    .iter()
                    .map(|rating| available * rating / total_rating)
                    .collect()
            },
        );
        let mut generator = DistGenerator::new(
            &name,
            &ibr.bus,
            ibr.terminal_map.clone(),
            configuration,
            p_nom.clone(),
            vec![0.0; channels],
        );
        generator.p_min = ibr.p_min.clone().or_else(|| {
            (ibr.p_avail.is_some()
                && matches!(ibr.prime_mover, IbrPrimeMover::Pv | IbrPrimeMover::Generic))
            .then(|| vec![0.0; channels])
        });
        generator.p_max = ibr.p_max.clone().or_else(|| ibr.p_avail.map(|_| p_nom));
        if generator.p_min.is_some() != generator.p_max.is_some() {
            return Err(fail(format!(
                "IBR `{}` supplies only one side of its active-power bounds",
                ibr.name
            )));
        }
        generator.q_min.clone_from(&ibr.q_min);
        generator.q_max.clone_from(&ibr.q_max);
        if generator.q_min.is_some() != generator.q_max.is_some() {
            return Err(fail(format!(
                "IBR `{}` supplies only one side of its reactive-power bounds",
                ibr.name
            )));
        }
        generator.s_max = Some(ibr.s_max.clone());
        generator.i_max.clone_from(&ibr.i_max);
        network.generators_mut().push(generator);
        let mut action = LinDist3FlowPreparationAction::new(
            LinDist3FlowPreparationActionKind::IbrApproximated,
            format!("ibr.{}", ibr.name),
            Some(format!("generator.{name}")),
        );
        action
            .details
            .insert("method".to_owned(), json!("static_dispatchable_generator"));
        action
            .details
            .insert("control_profile".to_owned(), json!(&ibr.control_profile));
        report.actions.push(action);
        if let Some(profile) = &ibr.control_profile {
            report.actions.push(LinDist3FlowPreparationAction::new(
                LinDist3FlowPreparationActionKind::StaticControlFrozen,
                format!("control_profile.{profile}"),
                Some(format!("generator.{name}")),
            ));
        }
    }
    Ok(())
}

fn omit_controls_and_untyped(
    network: &mut MulticonductorNetwork,
    policy: LinDist3FlowPreparationPolicy,
    report: &mut LinDist3FlowPreparationReport,
) {
    let objects = std::mem::take(network.untyped_objects_mut());
    let mut retained = Vec::new();
    for object in objects {
        let control = matches!(
            object.class.to_ascii_lowercase().as_str(),
            "regcontrol" | "capcontrol" | "invcontrol" | "swtcontrol"
        );
        if control && policy.at_least(LinDist3FlowPreparationPolicy::Approximate) {
            report.actions.push(LinDist3FlowPreparationAction::new(
                LinDist3FlowPreparationActionKind::StaticControlFrozen,
                format!("{}.{}", object.class, object.name),
                None,
            ));
        } else if policy.at_least(LinDist3FlowPreparationPolicy::Permissive) {
            report.actions.push(LinDist3FlowPreparationAction::new(
                LinDist3FlowPreparationActionKind::UntypedObjectOmitted,
                format!("{}.{}", object.class, object.name),
                None,
            ));
        } else {
            retained.push(object);
        }
    }
    *network.untyped_objects_mut() = retained;
    if policy.at_least(LinDist3FlowPreparationPolicy::Approximate) {
        for profile in network.control_profiles() {
            let source = format!("control_profile.{}", profile.name);
            if !report.actions.iter().any(|action| action.source == source) {
                report.actions.push(LinDist3FlowPreparationAction::new(
                    LinDist3FlowPreparationActionKind::StaticControlFrozen,
                    source,
                    None,
                ));
            }
        }
        network.control_profiles_mut().clear();
    }
}

/// Prepare an independently owned LinDist3Flow network according to `policy`.
///
/// The input remains unchanged. Static switch contacts become independent
/// zero-impedance line-flow variables, avoiding any aggregation of parallel
/// contact ratings. Line charging and capacitors become explicit terminal
/// shunts. Current and exponential load behavior is linearized in squared
/// voltage at nominal voltage. IBRs become static generators; their dynamic
/// controls and storage state are not represented.
///
/// # Errors
/// A selected transformation cannot preserve or explicitly approximate the
/// component's dimensions, identities, bounds, or electrical parameters.
pub fn prepare_lindist3flow_network(
    network: &MulticonductorNetwork,
    policy: LinDist3FlowPreparationPolicy,
) -> Result<LinDist3FlowPreparedNetwork> {
    let mut prepared = network.clone();
    let mut report = LinDist3FlowPreparationReport {
        policy,
        ..LinDist3FlowPreparationReport::default()
    };
    if policy.at_least(LinDist3FlowPreparationPolicy::Lower) {
        let reduction = neutral_kron_reduce(&prepared, &NeutralKronOptions::default())?;
        let (network, neutral_report) = reduction.into_parts();
        prepared = network;
        if !neutral_report.buses.is_empty() {
            let mut action = LinDist3FlowPreparationAction::new(
                LinDist3FlowPreparationActionKind::NeutralKronReduced,
                "network",
                Some("network".to_owned()),
            );
            action
                .details
                .insert("bus_count".to_owned(), json!(neutral_report.buses.len()));
            report.actions.push(action);
            report.neutral_kron = Some(neutral_report);
        }
        lower_switches(&mut prepared, &mut report)?;
        lower_line_shunts(&mut prepared, &mut report)?;
        lower_capacitors(&mut prepared, &mut report)?;
    }
    if policy.at_least(LinDist3FlowPreparationPolicy::Approximate) {
        approximate_loads(&mut prepared, &mut report)?;
        approximate_ibrs(&mut prepared, &mut report)?;
    }
    omit_controls_and_untyped(&mut prepared, policy, &mut report);
    prepared.extras_mut().insert(
        "powerio_lindist3flow_preparation".to_owned(),
        json!({
            "policy": report.policy,
            "actions": report.actions,
        }),
    );
    Ok(LinDist3FlowPreparedNetwork {
        network: prepared,
        report,
    })
}
