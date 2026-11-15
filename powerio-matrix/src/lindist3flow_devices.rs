//! Device, objective, and nodal-balance preparation for LinDist3Flow.

use num_complex::Complex64;
use powerio_dist::{Configuration, DistLoad, DistLoadVoltageModel};
use powerio_prob::{LinDist3FlowOpfInstance, ObjectiveTerm};

use crate::{
    ConnectionPowerMap, Error, LinDist3FlowNetworkData, Result, build_lindist3flow_network_data,
    connection_power_map, cross_voltage_coefficients, winding_voltage_coefficients,
};

/// One coefficient on a squared-voltage variable.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowVoltageTerm {
    pub node: usize,
    pub coefficient: f64,
}

/// An affine scalar over the prepared squared-voltage variables.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowAffineExpression {
    pub constant: f64,
    pub voltage_terms: Vec<LinDist3FlowVoltageTerm>,
}

/// Active and reactive affine power expressions.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowComplexAffinePower {
    pub active: LinDist3FlowAffineExpression,
    pub reactive: LinDist3FlowAffineExpression,
}

/// One physical load and its reference-frozen terminal allocation.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowLoadData {
    pub load: String,
    pub source_load_row: usize,
    pub terminal_nodes: Vec<usize>,
    pub incidence: Vec<Vec<f64>>,
    pub terminal_power_map: ConnectionPowerMap,
    pub channel_power: Vec<LinDist3FlowComplexAffinePower>,
}

/// One affine terminal shunt-power row.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowShuntData {
    pub shunt: String,
    pub source_shunt_row: usize,
    pub terminal_nodes: Vec<usize>,
    pub terminal_power: Vec<LinDist3FlowComplexAffinePower>,
}

/// Bounds, objective coefficient, and SOC data for one dispatch channel.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowDispatchChannel {
    pub active_min: Option<f64>,
    pub active_max: Option<f64>,
    pub reactive_min: Option<f64>,
    pub reactive_max: Option<f64>,
    pub apparent_power_limit: Option<f64>,
    pub current_limit: Option<f64>,
    /// Affine live `|D v|²` used by the rotated current cone.
    pub squared_winding_voltage: LinDist3FlowAffineExpression,
    pub reference_winding_voltage: f64,
    /// Linear objective coefficient on active watts.
    pub active_objective_coefficient: f64,
}

/// One dispatchable generator and its terminal allocation.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowGeneratorData {
    pub generator: String,
    pub source_generator_row: usize,
    pub terminal_nodes: Vec<usize>,
    pub incidence: Vec<Vec<f64>>,
    pub terminal_power_map: ConnectionPowerMap,
    pub channels: Vec<LinDist3FlowDispatchChannel>,
}

/// One voltage source. Its channel powers inject directly at corresponding
/// terminals; the squared voltages are fixed in [`LinDist3FlowNetworkData`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowSourceData {
    pub source: String,
    pub source_source_row: usize,
    pub terminal_nodes: Vec<usize>,
    pub active_objective_coefficient: Vec<f64>,
}

/// Semantic scalar variables used by nodal-balance rows.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum LinDist3FlowVariable {
    LineActive { line: usize, conductor: usize },
    LineReactive { line: usize, conductor: usize },
    GeneratorActive { generator: usize, channel: usize },
    GeneratorReactive { generator: usize, channel: usize },
    SourceActive { source: usize, channel: usize },
    SourceReactive { source: usize, channel: usize },
}

/// One coefficient on a semantic decision variable.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowBalanceTerm {
    pub variable: LinDist3FlowVariable,
    pub coefficient: f64,
}

/// One equality expression constrained to zero.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowBalanceEquation {
    pub affine: LinDist3FlowAffineExpression,
    pub variable_terms: Vec<LinDist3FlowBalanceTerm>,
}

/// Active and reactive KCL at one retained bus terminal.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowBalanceRow {
    pub node: usize,
    pub active: LinDist3FlowBalanceEquation,
    pub reactive: LinDist3FlowBalanceEquation,
}

/// All device rows and the fully expanded nodal balances.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowDeviceData {
    pub loads: Vec<LinDist3FlowLoadData>,
    pub shunts: Vec<LinDist3FlowShuntData>,
    pub generators: Vec<LinDist3FlowGeneratorData>,
    pub sources: Vec<LinDist3FlowSourceData>,
    pub balances: Vec<LinDist3FlowBalanceRow>,
}

/// Complete solver-neutral preparation for the currently supported L3F slice.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LinDist3FlowPreparation {
    pub network: LinDist3FlowNetworkData,
    pub devices: LinDist3FlowDeviceData,
}

fn invalid(reason: impl Into<String>) -> Error {
    Error::InvalidLinDist3FlowCoefficients {
        reason: reason.into(),
    }
}

/// Build the physical-channel incidence matrix for a typed connection.
///
/// # Errors
/// The terminal and channel arities do not describe a supported grounded-wye,
/// single-phase, or delta connection.
pub fn lindist3flow_connection_incidence(
    configuration: Configuration,
    terminal_count: usize,
    channel_count: usize,
) -> Result<Vec<Vec<f64>>> {
    let carried =
        powerio_dist::lindist3flow_carries_connection(configuration, terminal_count, channel_count);
    let unsupported = || {
        invalid(format!(
            "{configuration:?} connection with {terminal_count} terminals and {channel_count} channels is unsupported"
        ))
    };
    if !carried {
        return Err(unsupported());
    }
    let channels =
        powerio_dist::calc_connection_channels(configuration, terminal_count, channel_count, None)
            .map_err(|_| unsupported())?;
    Ok(channels
        .iter()
        .map(|channel| {
            let mut row = vec![0.0; terminal_count];
            row[channel.from] = 1.0;
            if let Some(to) = channel.to {
                row[to] = -1.0;
            }
            row
        })
        .collect())
}

fn resolve_nodes(
    positions: &powerio_dist::MulticonductorTerminalIndex,
    bus: &str,
    terminals: &[String],
    family: &str,
    name: &str,
) -> Result<Vec<usize>> {
    terminals
        .iter()
        .map(|terminal| {
            positions.resolve(bus, terminal).ok_or_else(|| {
                invalid(format!(
                    "{family} `{name}` names unknown terminal `{bus}/{terminal}`"
                ))
            })
        })
        .collect()
}

fn references(instance: &LinDist3FlowOpfInstance, nodes: &[usize]) -> Vec<Complex64> {
    nodes
        .iter()
        .map(|&node| {
            let reference = &instance.reference().voltages[node];
            Complex64::from_polar(reference.magnitude, reference.angle)
        })
        .collect()
}

fn global_affine(
    constant: f64,
    scale: f64,
    local: &crate::AffineScalarCoefficients,
    nodes: &[usize],
) -> LinDist3FlowAffineExpression {
    LinDist3FlowAffineExpression {
        constant: constant + scale * local.constant,
        voltage_terms: local
            .coefficients
            .iter()
            .zip(nodes)
            .filter_map(|(coefficient, node)| {
                let coefficient = scale * coefficient;
                (coefficient.abs() > f64::EPSILON).then_some(LinDist3FlowVoltageTerm {
                    node: *node,
                    coefficient,
                })
            })
            .collect(),
    }
}

fn channel_value(values: &[f64], channel: usize, channels: usize, label: &str) -> Result<f64> {
    let value = match values.len() {
        1 => values[0],
        length if length == channels => values[channel],
        length => {
            return Err(invalid(format!(
                "{label} has length {length}, expected 1 or {channels}"
            )));
        }
    };
    if !value.is_finite() {
        return Err(invalid(format!("{label} entry {channel} is non-finite")));
    }
    Ok(value)
}

fn load_channel_power(
    load: &DistLoad,
    incidence: &[Vec<f64>],
    reference: &[Complex64],
    terminal_nodes: &[usize],
    channel: usize,
) -> Result<LinDist3FlowComplexAffinePower> {
    let channels = load.p_nom.len();
    let p_nom = load.p_nom[channel];
    let q_nom = load.q_nom[channel];
    if !p_nom.is_finite() || !q_nom.is_finite() {
        return Err(invalid(format!(
            "load `{}` channel {channel} nominal power is non-finite",
            load.name
        )));
    }
    if matches!(
        load.voltage_model,
        DistLoadVoltageModel::ConstantPower { .. }
    ) {
        return Ok(LinDist3FlowComplexAffinePower {
            active: LinDist3FlowAffineExpression {
                constant: p_nom,
                voltage_terms: Vec::new(),
            },
            reactive: LinDist3FlowAffineExpression {
                constant: q_nom,
                voltage_terms: Vec::new(),
            },
        });
    }
    let winding = winding_voltage_coefficients(&incidence[channel], reference)?;
    let (v_nom, alpha_z, alpha_p, beta_z, beta_p) = match &load.voltage_model {
        DistLoadVoltageModel::ConstantImpedance { v_nom } => (
            channel_value(v_nom, channel, channels, "load v_nom")?,
            1.0,
            0.0,
            1.0,
            0.0,
        ),
        DistLoadVoltageModel::Zip {
            v_nom,
            alpha_z,
            alpha_i,
            alpha_p,
            beta_z,
            beta_i,
            beta_p,
        } => {
            let alpha_i = channel_value(alpha_i, channel, channels, "load alpha_i")?;
            let beta_i = channel_value(beta_i, channel, channels, "load beta_i")?;
            if alpha_i.abs() > f64::EPSILON || beta_i.abs() > f64::EPSILON {
                return Err(invalid(format!(
                    "load `{}` channel {channel} has a nonzero current fraction",
                    load.name
                )));
            }
            (
                channel_value(v_nom, channel, channels, "load v_nom")?,
                channel_value(alpha_z, channel, channels, "load alpha_z")?,
                channel_value(alpha_p, channel, channels, "load alpha_p")?,
                channel_value(beta_z, channel, channels, "load beta_z")?,
                channel_value(beta_p, channel, channels, "load beta_p")?,
            )
        }
        _ => {
            return Err(invalid(format!(
                "load `{}` voltage model is outside the supported ZP slice",
                load.name
            )));
        }
    };
    if !v_nom.is_finite() || v_nom <= 0.0 {
        return Err(invalid(format!(
            "load `{}` channel {channel} v_nom must be finite and positive",
            load.name
        )));
    }
    Ok(LinDist3FlowComplexAffinePower {
        active: global_affine(
            p_nom * alpha_p,
            powerio_core::electrical::calc_rated_admittance(p_nom * alpha_z, v_nom),
            &winding,
            terminal_nodes,
        ),
        reactive: global_affine(
            q_nom * beta_p,
            powerio_core::electrical::calc_rated_admittance(q_nom * beta_z, v_nom),
            &winding,
            terminal_nodes,
        ),
    })
}

fn shunt_terminal_power(
    name: &str,
    g: &[Vec<f64>],
    b: &[Vec<f64>],
    reference: &[Complex64],
    terminal_nodes: &[usize],
    terminal: usize,
) -> Result<LinDist3FlowComplexAffinePower> {
    let n = terminal_nodes.len();
    if g.len() != n || b.len() != n {
        return Err(invalid(format!(
            "shunt `{name}` admittance does not have {n} rows"
        )));
    }
    let mut active = LinDist3FlowAffineExpression::default();
    let mut reactive = LinDist3FlowAffineExpression::default();
    for other in 0..n {
        if g[terminal].len() != n || b[terminal].len() != n {
            return Err(invalid(format!(
                "shunt `{name}` admittance row {terminal} does not have {n} entries"
            )));
        }
        let admittance = Complex64::new(g[terminal][other], b[terminal][other]);
        if !admittance.re.is_finite() || !admittance.im.is_finite() {
            return Err(invalid(format!(
                "shunt `{name}` admittance entry ({terminal}, {other}) is non-finite"
            )));
        }
        let cross = cross_voltage_coefficients(reference[terminal], reference[other])?;
        let scale = admittance.conj();
        let constant = scale * cross.constant;
        let own = scale * cross.coefficient_phi;
        let other_coefficient = scale * cross.coefficient_psi;
        active.constant += constant.re;
        reactive.constant += constant.im;
        active.voltage_terms.push(LinDist3FlowVoltageTerm {
            node: terminal_nodes[terminal],
            coefficient: own.re,
        });
        reactive.voltage_terms.push(LinDist3FlowVoltageTerm {
            node: terminal_nodes[terminal],
            coefficient: own.im,
        });
        active.voltage_terms.push(LinDist3FlowVoltageTerm {
            node: terminal_nodes[other],
            coefficient: other_coefficient.re,
        });
        reactive.voltage_terms.push(LinDist3FlowVoltageTerm {
            node: terminal_nodes[other],
            coefficient: other_coefficient.im,
        });
    }
    Ok(LinDist3FlowComplexAffinePower { active, reactive })
}

fn checked_vector_value(
    values: Option<&[f64]>,
    channel: usize,
    channels: usize,
    label: &str,
) -> Result<Option<f64>> {
    let Some(values) = values else {
        return Ok(None);
    };
    if values.len() != channels {
        return Err(invalid(format!(
            "{label} has length {}, expected {channels}",
            values.len()
        )));
    }
    let value = values[channel];
    if !value.is_finite() {
        return Err(invalid(format!("{label} entry {channel} is non-finite")));
    }
    Ok(Some(value))
}

fn positive_limit(
    values: Option<&[f64]>,
    channel: usize,
    channels: usize,
    label: &str,
) -> Result<Option<f64>> {
    let value = checked_vector_value(values, channel, channels, label)?;
    if value.is_some_and(|value| value <= 0.0) {
        return Err(invalid(format!("{label} entry {channel} must be positive")));
    }
    Ok(value)
}

fn generator_bounds(
    nominal: f64,
    lower: Option<&[f64]>,
    upper: Option<&[f64]>,
    channel: usize,
    channels: usize,
    label: &str,
    selected: bool,
) -> Result<(Option<f64>, Option<f64>)> {
    if !selected {
        return Ok((None, None));
    }
    let lower = checked_vector_value(lower, channel, channels, &format!("{label} minimum"))?;
    let upper = checked_vector_value(upper, channel, channels, &format!("{label} maximum"))?;
    let (lower, upper) = if lower.is_none() && upper.is_none() {
        if !nominal.is_finite() {
            return Err(invalid(format!("{label} nominal value is non-finite")));
        }
        (Some(nominal), Some(nominal))
    } else {
        (lower, upper)
    };
    if lower.zip(upper).is_some_and(|(lower, upper)| lower > upper) {
        return Err(invalid(format!("{label} bounds are inverted")));
    }
    Ok((lower, upper))
}

fn cost_value(values: Option<&[f64]>, channel: usize, channels: usize, label: &str) -> Result<f64> {
    let Some(values) = values else {
        return Ok(0.0);
    };
    Ok(channel_value(values, channel, channels, label)? / 1000.0)
}

fn uses_dispatch_cost(instance: &LinDist3FlowOpfInstance) -> Result<bool> {
    let mut enabled = false;
    for term in instance.base_instance().objective().terms() {
        match term {
            ObjectiveTerm::ActivePowerDispatchCost => enabled = true,
            ObjectiveTerm::NetworkGeneratorCost => {
                return Err(invalid(
                    "LinDist3Flow does not compile balanced-network generator cost curves",
                ));
            }
            _ => {
                return Err(invalid(
                    "the LinDist3Flow objective contains an unknown term",
                ));
            }
        }
    }
    Ok(enabled)
}

fn add_affine(
    target: &mut LinDist3FlowAffineExpression,
    source: &LinDist3FlowAffineExpression,
    scale: f64,
) {
    target.constant += scale * source.constant;
    target
        .voltage_terms
        .extend(source.voltage_terms.iter().filter_map(|term| {
            let coefficient = scale * term.coefficient;
            (coefficient.abs() > f64::EPSILON).then_some(LinDist3FlowVoltageTerm {
                node: term.node,
                coefficient,
            })
        }));
}

fn add_variable(
    equation: &mut LinDist3FlowBalanceEquation,
    variable: LinDist3FlowVariable,
    coefficient: f64,
) {
    if coefficient.abs() > f64::EPSILON {
        equation.variable_terms.push(LinDist3FlowBalanceTerm {
            variable,
            coefficient,
        });
    }
}

/// Prepare load, shunt, generator, source, objective, and KCL rows against an
/// already prepared L3F node/line axis.
///
/// Every balance equation is stated as an affine expression plus semantic
/// decision-variable terms, all constrained to zero. This keeps row numbering
/// and cone storage decisions in the solver adapter.
///
/// # Errors
/// A device has an unsupported connection or voltage model, inconsistent
/// dimensions, an invalid bound/rating, or an unresolved terminal identity.
#[allow(clippy::too_many_lines)]
pub fn build_lindist3flow_device_data(
    instance: &LinDist3FlowOpfInstance,
    network_data: &LinDist3FlowNetworkData,
) -> Result<LinDist3FlowDeviceData> {
    let network = instance.network();
    // The network data's nodes are powerio-dist's flat terminal index.
    let positions = powerio_dist::MulticonductorTerminalIndex::build(network)
        .map_err(|error| invalid(error.to_string()))?;
    if positions.len() != network_data.nodes.len() {
        return Err(invalid(
            "the network data does not index this instance's terminals".to_owned(),
        ));
    }
    let dispatch_cost = uses_dispatch_cost(instance)?;

    let mut loads = Vec::with_capacity(network.loads().len());
    for (row, load) in network.loads().iter().enumerate() {
        if load.p_nom.len() != load.q_nom.len() || load.p_nom.is_empty() {
            return Err(invalid(format!(
                "load `{}` active/reactive channel counts are empty or unequal",
                load.name
            )));
        }
        let terminal_nodes = resolve_nodes(
            &positions,
            &load.bus,
            &load.terminal_map,
            "load",
            &load.name,
        )?;
        let incidence = lindist3flow_connection_incidence(
            load.configuration,
            terminal_nodes.len(),
            load.p_nom.len(),
        )?;
        let reference = references(instance, &terminal_nodes);
        let terminal_power_map = connection_power_map(&incidence, &reference)?;
        let channel_power = (0..load.p_nom.len())
            .map(|channel| {
                load_channel_power(load, &incidence, &reference, &terminal_nodes, channel)
            })
            .collect::<Result<Vec<_>>>()?;
        loads.push(LinDist3FlowLoadData {
            load: load.name.clone(),
            source_load_row: row,
            terminal_nodes,
            incidence,
            terminal_power_map,
            channel_power,
        });
    }

    let mut shunts = Vec::with_capacity(network.shunts().len());
    for (row, shunt) in network.shunts().iter().enumerate() {
        let terminal_nodes = resolve_nodes(
            &positions,
            &shunt.bus,
            &shunt.terminal_map,
            "shunt",
            &shunt.name,
        )?;
        let reference = references(instance, &terminal_nodes);
        let terminal_power = (0..terminal_nodes.len())
            .map(|terminal| {
                shunt_terminal_power(
                    &shunt.name,
                    &shunt.g,
                    &shunt.b,
                    &reference,
                    &terminal_nodes,
                    terminal,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        shunts.push(LinDist3FlowShuntData {
            shunt: shunt.name.clone(),
            source_shunt_row: row,
            terminal_nodes,
            terminal_power,
        });
    }

    let mut generators = Vec::with_capacity(network.generators().len());
    for (row, generator) in network.generators().iter().enumerate() {
        let channels = generator.p_nom.len();
        if channels == 0 || generator.q_nom.len() != channels {
            return Err(invalid(format!(
                "generator `{}` active/reactive channel counts are empty or unequal",
                generator.name
            )));
        }
        let terminal_nodes = resolve_nodes(
            &positions,
            &generator.bus,
            &generator.terminal_map,
            "generator",
            &generator.name,
        )?;
        let incidence = lindist3flow_connection_incidence(
            generator.configuration,
            terminal_nodes.len(),
            channels,
        )?;
        let reference = references(instance, &terminal_nodes);
        let terminal_power_map = connection_power_map(&incidence, &reference)?;
        let capability_selected = instance
            .base_instance()
            .constraints()
            .generator_capability
            .selects(&generator.name);
        let mut prepared_channels = Vec::with_capacity(channels);
        for (channel, incidence_row) in incidence.iter().enumerate() {
            let winding = winding_voltage_coefficients(incidence_row, &reference)?;
            let squared_winding_voltage = global_affine(0.0, 1.0, &winding, &terminal_nodes);
            let reference_winding_voltage =
                terminal_power_map.reference_winding_voltage[channel].norm();
            let (active_min, active_max) = generator_bounds(
                generator.p_nom[channel],
                generator.p_min.as_deref(),
                generator.p_max.as_deref(),
                channel,
                channels,
                "generator active power",
                capability_selected,
            )?;
            let (reactive_min, reactive_max) = generator_bounds(
                generator.q_nom[channel],
                generator.q_min.as_deref(),
                generator.q_max.as_deref(),
                channel,
                channels,
                "generator reactive power",
                capability_selected,
            )?;
            prepared_channels.push(LinDist3FlowDispatchChannel {
                active_min,
                active_max,
                reactive_min,
                reactive_max,
                apparent_power_limit: capability_selected
                    .then(|| {
                        positive_limit(
                            generator.s_max.as_deref(),
                            channel,
                            channels,
                            "generator apparent-power limit",
                        )
                    })
                    .transpose()?
                    .flatten(),
                current_limit: capability_selected
                    .then(|| {
                        positive_limit(
                            generator.i_max.as_deref(),
                            channel,
                            channels,
                            "generator current limit",
                        )
                    })
                    .transpose()?
                    .flatten(),
                squared_winding_voltage,
                reference_winding_voltage,
                active_objective_coefficient: if dispatch_cost {
                    cost_value(
                        generator.cost.as_deref(),
                        channel,
                        channels,
                        "generator cost",
                    )?
                } else {
                    0.0
                },
            });
        }
        generators.push(LinDist3FlowGeneratorData {
            generator: generator.name.clone(),
            source_generator_row: row,
            terminal_nodes,
            incidence,
            terminal_power_map,
            channels: prepared_channels,
        });
    }

    let mut sources = Vec::with_capacity(network.sources().len());
    for (row, source) in network.sources().iter().enumerate() {
        let terminal_nodes = resolve_nodes(
            &positions,
            &source.bus,
            &source.terminal_map,
            "voltage source",
            &source.name,
        )?;
        let channels = terminal_nodes.len();
        let active_objective_coefficient = (0..channels)
            .map(|channel| {
                if dispatch_cost {
                    cost_value(
                        source.energy_cost_rate.as_deref(),
                        channel,
                        channels,
                        "voltage source energy cost",
                    )
                } else {
                    Ok(0.0)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        sources.push(LinDist3FlowSourceData {
            source: source.name.clone(),
            source_source_row: row,
            terminal_nodes,
            active_objective_coefficient,
        });
    }

    let mut balances = (0..network_data.nodes.len())
        .map(|node| LinDist3FlowBalanceRow {
            node,
            active: LinDist3FlowBalanceEquation::default(),
            reactive: LinDist3FlowBalanceEquation::default(),
        })
        .collect::<Vec<_>>();
    for (line_index, line) in network_data.lines.iter().enumerate() {
        for conductor in 0..line.parent_nodes.len() {
            add_variable(
                &mut balances[line.parent_nodes[conductor]].active,
                LinDist3FlowVariable::LineActive {
                    line: line_index,
                    conductor,
                },
                -1.0,
            );
            add_variable(
                &mut balances[line.parent_nodes[conductor]].reactive,
                LinDist3FlowVariable::LineReactive {
                    line: line_index,
                    conductor,
                },
                -1.0,
            );
            add_variable(
                &mut balances[line.child_nodes[conductor]].active,
                LinDist3FlowVariable::LineActive {
                    line: line_index,
                    conductor,
                },
                1.0,
            );
            add_variable(
                &mut balances[line.child_nodes[conductor]].reactive,
                LinDist3FlowVariable::LineReactive {
                    line: line_index,
                    conductor,
                },
                1.0,
            );
        }
    }
    for load in &loads {
        for terminal in 0..load.terminal_nodes.len() {
            let balance = &mut balances[load.terminal_nodes[terminal]];
            for channel in 0..load.channel_power.len() {
                let power = &load.channel_power[channel];
                let real = load.terminal_power_map.real_part[terminal][channel];
                let imag = load.terminal_power_map.imag_part[terminal][channel];
                add_affine(&mut balance.active.affine, &power.active, -real);
                add_affine(&mut balance.active.affine, &power.reactive, imag);
                add_affine(&mut balance.reactive.affine, &power.active, -imag);
                add_affine(&mut balance.reactive.affine, &power.reactive, -real);
            }
        }
    }
    for shunt in &shunts {
        for (terminal, &node) in shunt.terminal_nodes.iter().enumerate() {
            add_affine(
                &mut balances[node].active.affine,
                &shunt.terminal_power[terminal].active,
                -1.0,
            );
            add_affine(
                &mut balances[node].reactive.affine,
                &shunt.terminal_power[terminal].reactive,
                -1.0,
            );
        }
    }
    for (generator_index, generator) in generators.iter().enumerate() {
        for terminal in 0..generator.terminal_nodes.len() {
            let balance = &mut balances[generator.terminal_nodes[terminal]];
            for channel in 0..generator.channels.len() {
                let real = generator.terminal_power_map.real_part[terminal][channel];
                let imag = generator.terminal_power_map.imag_part[terminal][channel];
                add_variable(
                    &mut balance.active,
                    LinDist3FlowVariable::GeneratorActive {
                        generator: generator_index,
                        channel,
                    },
                    real,
                );
                add_variable(
                    &mut balance.active,
                    LinDist3FlowVariable::GeneratorReactive {
                        generator: generator_index,
                        channel,
                    },
                    -imag,
                );
                add_variable(
                    &mut balance.reactive,
                    LinDist3FlowVariable::GeneratorActive {
                        generator: generator_index,
                        channel,
                    },
                    imag,
                );
                add_variable(
                    &mut balance.reactive,
                    LinDist3FlowVariable::GeneratorReactive {
                        generator: generator_index,
                        channel,
                    },
                    real,
                );
            }
        }
    }
    for (source_index, source) in sources.iter().enumerate() {
        for (channel, &node) in source.terminal_nodes.iter().enumerate() {
            add_variable(
                &mut balances[node].active,
                LinDist3FlowVariable::SourceActive {
                    source: source_index,
                    channel,
                },
                1.0,
            );
            add_variable(
                &mut balances[node].reactive,
                LinDist3FlowVariable::SourceReactive {
                    source: source_index,
                    channel,
                },
                1.0,
            );
        }
    }

    Ok(LinDist3FlowDeviceData {
        loads,
        shunts,
        generators,
        sources,
        balances,
    })
}

/// Build the complete solver-neutral preparation for the supported L3F slice.
///
/// # Errors
/// As [`build_lindist3flow_network_data`] and
/// [`build_lindist3flow_device_data`].
pub fn build_lindist3flow_preparation(
    instance: &LinDist3FlowOpfInstance,
) -> Result<LinDist3FlowPreparation> {
    let network = build_lindist3flow_network_data(instance)?;
    let devices = build_lindist3flow_device_data(instance, &network)?;
    Ok(LinDist3FlowPreparation { network, devices })
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use approx::assert_relative_eq;
    use powerio_dist::{
        Configuration, DistBus, DistGenerator, DistLine, DistLineCode, DistLoad, DistShunt,
        MulticonductorNetwork, VoltageSource,
    };
    use powerio_prob::{
        ConstraintSelection, LinDist3FlowBuildOptions, LinDist3FlowOpfInstance, McAcOpfInstance,
        MulticonductorActiveConstraints,
    };

    use super::*;

    fn one_phase_network() -> MulticonductorNetwork {
        let terminal = vec!["1".to_owned()];
        let mut network = MulticonductorNetwork::named("one-phase");
        network
            .buses_mut()
            .push(DistBus::new("source", terminal.clone()));
        network
            .buses_mut()
            .push(DistBus::new("load", terminal.clone()));
        network
            .line_codes_mut()
            .push(DistLineCode::new("one", vec![vec![0.1]], vec![vec![0.2]]));
        network.lines_mut().push(DistLine::new(
            "line",
            "source",
            "load",
            terminal.clone(),
            terminal.clone(),
            "one",
            1.0,
        ));
        network.sources_mut().push(VoltageSource::new(
            "grid",
            "source",
            terminal,
            vec![230.0],
            vec![0.0],
        ));
        network
    }

    fn three_phase_network() -> MulticonductorNetwork {
        let terminals = vec!["1".to_owned(), "2".to_owned(), "3".to_owned()];
        let mut network = MulticonductorNetwork::named("three-phase");
        network
            .buses_mut()
            .push(DistBus::new("source", terminals.clone()));
        network
            .buses_mut()
            .push(DistBus::new("load", terminals.clone()));
        network.line_codes_mut().push(DistLineCode::new(
            "three",
            vec![
                vec![0.1, 0.0, 0.0],
                vec![0.0, 0.1, 0.0],
                vec![0.0, 0.0, 0.1],
            ],
            vec![
                vec![0.2, 0.0, 0.0],
                vec![0.0, 0.2, 0.0],
                vec![0.0, 0.0, 0.2],
            ],
        ));
        network.lines_mut().push(DistLine::new(
            "line",
            "source",
            "load",
            terminals.clone(),
            terminals.clone(),
            "three",
            1.0,
        ));
        network.sources_mut().push(VoltageSource::new(
            "grid",
            "source",
            terminals,
            vec![230.0; 3],
            vec![0.0, -2.0 * PI / 3.0, 2.0 * PI / 3.0],
        ));
        network
    }

    fn evaluate(expression: &LinDist3FlowAffineExpression, squared_voltage: &[f64]) -> f64 {
        expression.constant
            + expression
                .voltage_terms
                .iter()
                .map(|term| term.coefficient * squared_voltage[term.node])
                .sum::<f64>()
    }

    #[test]
    fn connection_incidence_matches_supported_physical_channels() {
        assert_eq!(
            lindist3flow_connection_incidence(Configuration::SinglePhase, 2, 1).unwrap(),
            [vec![1.0, -1.0]]
        );
        assert_eq!(
            lindist3flow_connection_incidence(Configuration::Wye, 2, 2).unwrap(),
            [vec![1.0, 0.0], vec![0.0, 1.0]]
        );
        assert_eq!(
            lindist3flow_connection_incidence(Configuration::Delta, 3, 3).unwrap()[2],
            [-1.0, 0.0, 1.0]
        );
        assert!(lindist3flow_connection_incidence(Configuration::Delta, 3, 1).is_err());
    }

    #[test]
    fn constant_power_load_and_line_flows_have_the_reference_kcl_signs() {
        let mut network = one_phase_network();
        network.loads_mut().push(DistLoad::new(
            "demand",
            "load",
            vec!["1".to_owned()],
            Configuration::Wye,
            vec![1_000.0],
            vec![200.0],
        ));
        let instance =
            LinDist3FlowOpfInstance::from_network(network, LinDist3FlowBuildOptions::default())
                .unwrap();
        let preparation = build_lindist3flow_preparation(&instance).unwrap();

        let source = &preparation.devices.balances[0];
        assert!(
            source
                .active
                .variable_terms
                .contains(&LinDist3FlowBalanceTerm {
                    variable: LinDist3FlowVariable::SourceActive {
                        source: 0,
                        channel: 0,
                    },
                    coefficient: 1.0,
                })
        );
        assert!(
            source
                .active
                .variable_terms
                .contains(&LinDist3FlowBalanceTerm {
                    variable: LinDist3FlowVariable::LineActive {
                        line: 0,
                        conductor: 0,
                    },
                    coefficient: -1.0,
                })
        );
        let load = &preparation.devices.balances[1];
        assert_relative_eq!(load.active.affine.constant, -1_000.0, epsilon = 1e-12);
        assert_relative_eq!(load.reactive.affine.constant, -200.0, epsilon = 1e-12);
        assert!(
            load.active
                .variable_terms
                .contains(&LinDist3FlowBalanceTerm {
                    variable: LinDist3FlowVariable::LineActive {
                        line: 0,
                        conductor: 0,
                    },
                    coefficient: 1.0,
                })
        );
    }

    #[test]
    fn delta_zp_load_is_affine_and_exact_at_the_reference() {
        let mut network = three_phase_network();
        let mut load = DistLoad::new(
            "delta",
            "load",
            vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
            Configuration::Delta,
            vec![1_000.0, 2_000.0, 3_000.0],
            vec![100.0, 200.0, 300.0],
        );
        load.voltage_model = DistLoadVoltageModel::Zip {
            v_nom: vec![230.0 * 3.0_f64.sqrt()],
            alpha_z: vec![0.4],
            alpha_i: vec![0.0],
            alpha_p: vec![0.6],
            beta_z: vec![0.25],
            beta_i: vec![0.0],
            beta_p: vec![0.75],
        };
        network.loads_mut().push(load);
        let instance =
            LinDist3FlowOpfInstance::from_network(network, LinDist3FlowBuildOptions::default())
                .unwrap();
        let preparation = build_lindist3flow_preparation(&instance).unwrap();
        let squared_voltage = preparation
            .network
            .nodes
            .iter()
            .map(|node| node.reference_magnitude.powi(2))
            .collect::<Vec<_>>();
        let load = &preparation.devices.loads[0];

        for channel in 0..3 {
            assert_relative_eq!(
                evaluate(&load.channel_power[channel].active, &squared_voltage),
                [1_000.0, 2_000.0, 3_000.0][channel],
                epsilon = 1e-9
            );
            let allocation_sum = (0..3)
                .map(|terminal| load.terminal_power_map.matrix[terminal][channel])
                .sum::<Complex64>();
            assert_relative_eq!(allocation_sum.re, 1.0, epsilon = 1e-12);
            assert_relative_eq!(allocation_sum.im, 0.0, epsilon = 1e-12);
        }
    }

    #[test]
    fn generator_rows_keep_bounds_cost_soc_voltage_and_terminal_map() {
        let mut network = one_phase_network();
        let mut generator = DistGenerator::new(
            "der",
            "load",
            vec!["1".to_owned()],
            Configuration::Wye,
            vec![500.0],
            vec![0.0],
        );
        generator.p_min = Some(vec![0.0]);
        generator.p_max = Some(vec![1_000.0]);
        generator.q_min = Some(vec![-300.0]);
        generator.q_max = Some(vec![300.0]);
        generator.cost = Some(vec![0.2]);
        generator.s_max = Some(vec![1_100.0]);
        generator.i_max = Some(vec![5.0]);
        network.generators_mut().push(generator);
        let instance =
            LinDist3FlowOpfInstance::from_network(network, LinDist3FlowBuildOptions::default())
                .unwrap();
        let preparation = build_lindist3flow_preparation(&instance).unwrap();
        let channel = &preparation.devices.generators[0].channels[0];

        assert_eq!(channel.active_min, Some(0.0));
        assert_eq!(channel.active_max, Some(1_000.0));
        assert_eq!(channel.reactive_min, Some(-300.0));
        assert_eq!(channel.reactive_max, Some(300.0));
        assert_eq!(channel.apparent_power_limit, Some(1_100.0));
        assert_eq!(channel.current_limit, Some(5.0));
        assert_relative_eq!(
            channel.active_objective_coefficient,
            0.0002,
            epsilon = 1e-15
        );
        assert_relative_eq!(channel.reference_winding_voltage, 230.0, epsilon = 1e-12);
        assert_relative_eq!(
            evaluate(
                &channel.squared_winding_voltage,
                &[230.0_f64.powi(2), 230.0_f64.powi(2)]
            ),
            230.0_f64.powi(2),
            epsilon = 1e-9
        );
    }

    #[test]
    fn shunt_power_enters_balance_as_an_affine_absorption() {
        let mut network = one_phase_network();
        network.shunts_mut().push(DistShunt::new(
            "capacitive",
            "load",
            vec!["1".to_owned()],
            vec![vec![0.0]],
            vec![vec![0.01]],
        ));
        let instance =
            LinDist3FlowOpfInstance::from_network(network, LinDist3FlowBuildOptions::default())
                .unwrap();
        let preparation = build_lindist3flow_preparation(&instance).unwrap();
        let squared_voltage = vec![230.0_f64.powi(2); 2];
        let shunt = &preparation.devices.shunts[0].terminal_power[0];

        assert_relative_eq!(
            evaluate(&shunt.active, &squared_voltage),
            0.0,
            epsilon = 1e-9
        );
        assert_relative_eq!(
            evaluate(&shunt.reactive, &squared_voltage),
            -0.01 * 230.0_f64.powi(2),
            epsilon = 1e-9
        );
        assert_relative_eq!(
            evaluate(
                &preparation.devices.balances[1].reactive.affine,
                &squared_voltage
            ),
            0.01 * 230.0_f64.powi(2),
            epsilon = 1e-9
        );
    }

    #[test]
    fn inactive_constraint_families_remove_only_their_numerical_limits() {
        let mut network = one_phase_network();
        network.buses_mut()[1].v_min = Some(210.0);
        network.lines_mut()[0].i_max = Some(vec![100.0]);
        let mut generator = DistGenerator::new(
            "der",
            "load",
            vec!["1".to_owned()],
            Configuration::Wye,
            vec![500.0],
            vec![0.0],
        );
        generator.p_min = Some(vec![0.0]);
        generator.p_max = Some(vec![1_000.0]);
        network.generators_mut().push(generator);
        let mut constraints = MulticonductorActiveConstraints::default();
        constraints.terminal_voltage_bounds = ConstraintSelection::None;
        constraints.conductor_limits = ConstraintSelection::None;
        constraints.generator_capability = ConstraintSelection::None;
        let base = McAcOpfInstance::from_network(network)
            .unwrap()
            .with_constraints(constraints);
        let instance =
            LinDist3FlowOpfInstance::from_mc_ac(base, LinDist3FlowBuildOptions::default()).unwrap();
        let preparation = build_lindist3flow_preparation(&instance).unwrap();

        assert!(preparation.network.nodes[1].squared_voltage_min.is_none());
        assert_eq!(preparation.network.lines[0].current_limit, [None]);
        assert!(
            preparation.devices.generators[0].channels[0]
                .active_min
                .is_none()
        );
    }
}
