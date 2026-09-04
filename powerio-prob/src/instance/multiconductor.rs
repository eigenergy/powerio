//! The multiconductor calculation instances: `McAcPfInstance` and
//! `McAcOpfInstance`.
//!
//! Both share the reusable [`MulticonductorNetwork`] handle. The power flow
//! instance contains the partial boundary specification a distribution power
//! flow needs — prescribed terminal powers, prescribed source terminal
//! voltages, isolated terminals, load voltage models, and the active
//! equipment control modes — never a required complete operating point. The
//! OPF instance adds the typed per phase objective and the active constraint
//! selections.

use powerio_core::Error;
use powerio_dist::MulticonductorNetwork;
use serde::{Deserialize, Serialize};

use crate::OperatingPoint;
use crate::diagnostics::codes;
use crate::instance::balanced::transform_discarded;
use crate::instance::constraints::MulticonductorActiveConstraints;
use crate::instance::objective::{Objective, ObjectiveTerm};

/// One load's prescribed terminal power: the load's stated per phase complex
/// power, keyed by the load's name and its terminal map. Watts and vars, as
/// the network states them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrescribedTerminalPower {
    pub load: String,
    /// The load's terminals, in its stated terminal map order.
    pub terminals: Vec<String>,
    /// Watts per terminal, aligned to `terminals`.
    pub p_w: Vec<f64>,
    /// Vars per terminal, aligned to `terminals`.
    pub q_var: Vec<f64>,
    /// The load's stated voltage dependence.
    pub voltage_model: powerio_dist::DistLoadVoltageModel,
}

/// One source's prescribed terminal complex voltage: volts and radians per
/// terminal, as the network states them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrescribedSourceVoltage {
    pub source: String,
    /// The source's terminals, in its stated terminal map order.
    pub terminals: Vec<String>,
    /// Volts per terminal (0.0 on grounded terminals).
    pub v_magnitude: Vec<f64>,
    /// Radians per terminal.
    pub v_angle: Vec<f64>,
}

/// One equipment control that is active for the calculation, by element name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum ActiveControlMode {
    /// A regulating transformer's tap control.
    RegulatorTap { transformer: String },
    /// A switched capacitor's step control.
    CapacitorSteps { capacitor: String },
}

/// The multiconductor AC power flow instance.
#[derive(Clone, Debug)]
pub struct McAcPfInstance {
    network: MulticonductorNetwork,
    loads: Vec<PrescribedTerminalPower>,
    sources: Vec<PrescribedSourceVoltage>,
    isolated_terminals: Vec<(String, String)>,
    control_modes: Vec<ActiveControlMode>,
    initial_point: Option<OperatingPoint<MulticonductorNetwork>>,
}

impl McAcPfInstance {
    /// Build the instance from the network's stated data: every in service
    /// load contributes its prescribed per phase powers and voltage model,
    /// every voltage source its prescribed terminal complex voltage, and
    /// every stated regulator and capacitor control an active control mode.
    ///
    /// # Errors
    /// A network with no voltage source: a distribution power flow has no
    /// boundary condition without one.
    pub fn from_network(network: MulticonductorNetwork) -> Result<Self, Error> {
        if network.sources().is_empty() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                "the multiconductor network states no voltage source to anchor the calculation",
            ));
        }
        let loads = network
            .loads()
            .iter()
            .map(|load| PrescribedTerminalPower {
                load: load.name.clone(),
                terminals: load.terminal_map.clone(),
                p_w: load.p_nom.clone(),
                q_var: load.q_nom.clone(),
                voltage_model: load.voltage_model.clone(),
            })
            .collect();
        let sources = network
            .sources()
            .iter()
            .map(|source| PrescribedSourceVoltage {
                source: source.name.clone(),
                terminals: source.terminal_map.clone(),
                v_magnitude: source.v_magnitude.clone(),
                v_angle: source.v_angle.clone(),
            })
            .collect();
        // Controllers live in the retained untyped objects: a `regcontrol`
        // regulates the transformer its `transformer` property names, and a
        // `capcontrol` switches the capacitor its `capacitor` property names.
        let controlled_element = |object: &powerio_dist::UntypedObject, key: &str| {
            object
                .props
                .iter()
                .find(|(name, _)| name.as_deref() == Some(key))
                .map(|(_, value)| value.clone())
        };
        let mut control_modes = Vec::new();
        for object in network.untyped_objects() {
            match object.class.to_ascii_lowercase().as_str() {
                "regcontrol" => {
                    if let Some(transformer) = controlled_element(object, "transformer") {
                        control_modes.push(ActiveControlMode::RegulatorTap { transformer });
                    }
                }
                "capcontrol" => {
                    if let Some(capacitor) = controlled_element(object, "capacitor") {
                        control_modes.push(ActiveControlMode::CapacitorSteps { capacitor });
                    }
                }
                _ => {}
            }
        }
        Ok(Self {
            network,
            loads,
            sources,
            isolated_terminals: Vec::new(),
            control_modes,
            initial_point: None,
        })
    }

    /// Supply an optional solver initial point.
    #[must_use]
    pub fn with_initial_point(mut self, point: OperatingPoint<MulticonductorNetwork>) -> Self {
        self.initial_point = Some(point);
        self
    }

    /// Replace the network and recalculate its prescribed powers, source
    /// voltages, and active controls while preserving a compatible initial
    /// point.
    pub fn with_network(mut self, network: MulticonductorNetwork) -> Result<Self, Error> {
        let mut replacement = Self::from_network(network.clone())?;
        if let Some(initial) = self.initial_point.take() {
            replacement.initial_point = Some(initial.rebind_network(network)?);
        }
        Ok(replacement)
    }

    /// The network this instance calculates on. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &MulticonductorNetwork {
        &self.network
    }

    /// The prescribed load terminal powers, in load table order.
    #[must_use]
    pub fn loads(&self) -> &[PrescribedTerminalPower] {
        &self.loads
    }

    /// The prescribed source terminal voltages, in source table order.
    #[must_use]
    pub fn sources(&self) -> &[PrescribedSourceVoltage] {
        &self.sources
    }

    /// Terminals with no equation, as `(bus, terminal)` pairs.
    #[must_use]
    pub fn isolated_terminals(&self) -> &[(String, String)] {
        &self.isolated_terminals
    }

    /// The equipment controls active for the calculation.
    #[must_use]
    pub fn control_modes(&self) -> &[ActiveControlMode] {
        &self.control_modes
    }

    /// The optional solver initial point.
    #[must_use]
    pub const fn initial_point(&self) -> Option<&OperatingPoint<MulticonductorNetwork>> {
        self.initial_point.as_ref()
    }
}

/// The multiconductor AC optimal power flow instance: the shared network,
/// the typed per phase objective, and the active terminal voltage, conductor,
/// and per phase generator constraint selections.
#[derive(Clone, Debug)]
pub struct McAcOpfInstance {
    network: MulticonductorNetwork,
    objective: Objective,
    constraints: MulticonductorActiveConstraints,
    initial_point: Option<OperatingPoint<MulticonductorNetwork>>,
}

impl McAcOpfInstance {
    /// Build the instance with the default per phase objective and every
    /// stated limit active.
    ///
    /// # Errors
    /// A network with no voltage source.
    pub fn from_network(network: MulticonductorNetwork) -> Result<Self, Error> {
        if network.sources().is_empty() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                "the multiconductor network states no voltage source to anchor the calculation",
            ));
        }
        Ok(Self {
            network,
            objective: Objective::active_power_dispatch_cost(),
            constraints: MulticonductorActiveConstraints::default(),
            initial_point: None,
        })
    }

    /// Replace the objective, consuming the instance. The shared network
    /// moves; no table is copied.
    #[must_use]
    pub fn with_objective(mut self, objective: Objective) -> Self {
        self.objective = objective;
        self
    }

    /// Append one objective term, consuming the instance.
    #[must_use]
    pub fn with_objective_term(mut self, term: ObjectiveTerm) -> Self {
        self.objective = std::mem::take(&mut self.objective).with_term(term);
        self
    }

    /// Replace the active constraint selections, consuming the instance.
    #[must_use]
    pub fn with_constraints(mut self, constraints: MulticonductorActiveConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Supply an optional solver initial point.
    #[must_use]
    pub fn with_initial_point(mut self, point: OperatingPoint<MulticonductorNetwork>) -> Self {
        self.initial_point = Some(point);
        self
    }

    /// Replace the network while preserving this instance's objective,
    /// constraint selections, and a compatible initial point.
    pub fn with_network(mut self, network: MulticonductorNetwork) -> Result<Self, Error> {
        if network.sources().is_empty() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                "the multiconductor network states no voltage source to anchor the calculation",
            ));
        }
        if let Some(initial) = self.initial_point.take() {
            self.initial_point = Some(initial.rebind_network(network.clone())?);
        }
        self.network = network;
        Ok(self)
    }

    /// The network this instance calculates on. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &MulticonductorNetwork {
        &self.network
    }

    /// The typed objective.
    #[must_use]
    pub const fn objective(&self) -> &Objective {
        &self.objective
    }

    /// The active constraint selections.
    #[must_use]
    pub const fn constraints(&self) -> &MulticonductorActiveConstraints {
        &self.constraints
    }

    /// The optional solver initial point.
    #[must_use]
    pub const fn initial_point(&self) -> Option<&OperatingPoint<MulticonductorNetwork>> {
        self.initial_point.as_ref()
    }

    /// The multiconductor power flow instance for this problem's network at
    /// its stated injections: the objective and constraint selections are
    /// discarded, and the discard is recorded.
    ///
    /// # Errors
    /// As [`McAcPfInstance::from_network`].
    pub fn to_mc_ac_pf(&self) -> Result<(McAcPfInstance, Vec<powerio_core::Diagnostic>), Error> {
        let instance = McAcPfInstance::from_network(self.network.clone())?;
        Ok((
            instance,
            vec![transform_discarded(
                "the objective and the active constraint selections",
            )],
        ))
    }
}
