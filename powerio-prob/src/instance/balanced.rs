//! The balanced calculation instances: `DcPfInstance`, `AcPfInstance`,
//! `DcOpfInstance`, and `AcOpfInstance`.
//!
//! Every instance shares its reusable electrical network as a cheap owning
//! handle rather than duplicating it into solver preparation arrays: cloning
//! an instance clones no network table. Fields are private; each instance
//! exposes a borrowed `network()` accessor and typed calculation data.
//! Physical limit values stay on the network; an OPF instance selects active
//! constraints by stable element identity and states its objective as typed
//! terms.
//!
//! A power flow instance contains partial boundary specifications, never a
//! required complete operating point: the unknown voltages, injections, and
//! flows are what the calculation solves. A complete
//! [`OperatingPoint`] can be supplied as an optional solver initial state.
//! Zero impedance branches are preserved; a projection that cannot represent
//! them refuses at its own boundary and
//! [`merge_zero_impedance_buses`](super::merge_zero_impedance_buses) is the
//! explicit, checked resolution.

use std::collections::BTreeMap;

use powerio_core::Error;
use powerio_tx::{BalancedNetwork, BusId, BusType, DcConvention};
use serde::{Deserialize, Serialize};

use crate::OperatingPoint;
use crate::diagnostics::codes;
use crate::instance::constraints::ActiveConstraints;
use crate::instance::objective::{Objective, ObjectiveTerm};

/// One bus of a DC power flow problem: what the calculation is told, never
/// what it solves.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum DcBusSpecification {
    /// The net active power injection the bus states, MW (generation minus
    /// demand over in service elements).
    NetActivePower { p_mw: f64 },
    /// A reference bus with its stated voltage angle, degrees.
    Reference { va_degrees: f64 },
    /// An isolated bus: no equation.
    Isolated,
}

/// One bus of an AC power flow problem, the standard partial specification.
/// Powers are MW and MVAr, voltage magnitudes per unit, angles degrees, as
/// the network states them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum AcBusSpecification {
    /// Prescribed net active and reactive injection.
    Pq { p: f64, q: f64 },
    /// Prescribed net active injection and voltage magnitude.
    Pv { p: f64, vm: f64 },
    /// Prescribed voltage magnitude and angle.
    Reference { vm: f64, va: f64 },
    /// No equation.
    Isolated,
}

/// The DC power flow instance: the shared network plus per bus boundary
/// specifications and the selected DC branch approximation.
#[derive(Clone, Debug)]
pub struct DcPfInstance {
    network: BalancedNetwork,
    specifications: Vec<DcBusSpecification>,
    approximation: DcConvention,
    initial_state: Option<OperatingPoint<BalancedNetwork>>,
}

impl DcPfInstance {
    /// Build the instance from the network's stated data: reference buses
    /// contribute their stated angle, isolated buses no equation, and every
    /// other bus its net active injection over in service generators and
    /// loads.
    ///
    /// # Errors
    /// A network with no reference bus.
    pub fn from_network(network: BalancedNetwork) -> Result<Self, Error> {
        require_reference(&network)?;
        let totals = aggregate_bus_elements(&network);
        let specifications = network
            .buses()
            .iter()
            .map(|bus| match bus.kind {
                BusType::Ref => DcBusSpecification::Reference { va_degrees: bus.va },
                BusType::Isolated => DcBusSpecification::Isolated,
                // Every other declared kind states a net injection.
                _ => DcBusSpecification::NetActivePower {
                    p_mw: net_active_power(&totals, bus.id),
                },
            })
            .collect();
        Ok(Self {
            network,
            specifications,
            approximation: DcConvention::default(),
            initial_state: None,
        })
    }

    /// Select the DC branch approximation, consuming the instance. The
    /// network handle moves; no table is copied.
    #[must_use]
    pub fn with_approximation(mut self, approximation: DcConvention) -> Self {
        self.approximation = approximation;
        self
    }

    /// Supply an optional solver initial state.
    #[must_use]
    pub fn with_initial_state(mut self, state: OperatingPoint<BalancedNetwork>) -> Self {
        self.initial_state = Some(state);
        self
    }

    /// The network this instance calculates on. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }

    /// The per bus boundary specifications, in bus table order.
    #[must_use]
    pub fn specifications(&self) -> &[DcBusSpecification] {
        &self.specifications
    }

    /// The selected DC branch approximation.
    #[must_use]
    pub const fn approximation(&self) -> DcConvention {
        self.approximation
    }

    /// The optional solver initial state.
    #[must_use]
    pub const fn initial_state(&self) -> Option<&OperatingPoint<BalancedNetwork>> {
        self.initial_state.as_ref()
    }
}

/// The AC power flow instance: the shared network plus one
/// [`AcBusSpecification`] per bus.
#[derive(Clone, Debug)]
pub struct AcPfInstance {
    network: BalancedNetwork,
    specifications: Vec<AcBusSpecification>,
    initial_state: Option<OperatingPoint<BalancedNetwork>>,
}

impl AcPfInstance {
    /// Build the instance from the network's stated data. A PQ bus states
    /// its net injections; a PV bus its net active injection and the
    /// regulating generator's voltage setpoint; a reference bus its setpoint
    /// magnitude and stated angle.
    ///
    /// # Errors
    /// A network with no reference bus, or conflicting active voltage
    /// controllers: two in service generators at one bus stating different
    /// voltage setpoints are refused until an explicit edit resolves them.
    pub fn from_network(network: BalancedNetwork) -> Result<Self, Error> {
        require_reference(&network)?;
        let totals = aggregate_bus_elements(&network);
        let specifications = network
            .buses()
            .iter()
            .map(|bus| {
                let spec = match bus.kind {
                    BusType::Isolated => AcBusSpecification::Isolated,
                    BusType::Pv => AcBusSpecification::Pv {
                        p: net_active_power(&totals, bus.id),
                        vm: controlled_magnitude(&totals, bus.id, bus.vm)?,
                    },
                    BusType::Ref => AcBusSpecification::Reference {
                        vm: controlled_magnitude(&totals, bus.id, bus.vm)?,
                        va: bus.va,
                    },
                    // Every other declared kind is the PQ specification.
                    _ => AcBusSpecification::Pq {
                        p: net_active_power(&totals, bus.id),
                        q: net_reactive_power(&totals, bus.id),
                    },
                };
                Ok(spec)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Self {
            network,
            specifications,
            initial_state: None,
        })
    }

    /// Supply an optional solver initial state.
    #[must_use]
    pub fn with_initial_state(mut self, state: OperatingPoint<BalancedNetwork>) -> Self {
        self.initial_state = Some(state);
        self
    }

    /// The network this instance calculates on. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }

    /// The per bus specifications, in bus table order.
    #[must_use]
    pub fn specifications(&self) -> &[AcBusSpecification] {
        &self.specifications
    }

    /// The optional solver initial state.
    #[must_use]
    pub const fn initial_state(&self) -> Option<&OperatingPoint<BalancedNetwork>> {
        self.initial_state.as_ref()
    }

    /// The DC power flow instance this AC problem implies: reactive data and
    /// voltage magnitudes are discarded, and the flat voltage assumption of
    /// the DC approximation is recorded as a diagnostic.
    #[must_use]
    pub fn to_dc_pf(&self) -> (DcPfInstance, Vec<powerio_core::Diagnostic>) {
        let instance = DcPfInstance {
            network: self.network.clone(),
            specifications: self
                .specifications
                .iter()
                .map(|specification| match *specification {
                    AcBusSpecification::Pq { p, .. } | AcBusSpecification::Pv { p, .. } => {
                        DcBusSpecification::NetActivePower { p_mw: p }
                    }
                    AcBusSpecification::Reference { va, .. } => {
                        DcBusSpecification::Reference { va_degrees: va }
                    }
                    AcBusSpecification::Isolated => DcBusSpecification::Isolated,
                })
                .collect(),
            approximation: DcConvention::default(),
            initial_state: self.initial_state.clone(),
        };
        let diagnostics = vec![
            transform_discarded("reactive power and voltage magnitude specifications"),
            transform_assumption(
                "the DC approximation holds every voltage magnitude at one per unit",
            ),
        ];
        (instance, diagnostics)
    }
}

/// The DC optimal power flow instance: the shared network, the typed
/// objective, the active constraint selections, the selected DC branch
/// approximation, and the reference conditions the network states.
#[derive(Clone, Debug)]
pub struct DcOpfInstance {
    network: BalancedNetwork,
    objective: Objective,
    constraints: ActiveConstraints,
    approximation: DcConvention,
    initial_state: Option<OperatingPoint<BalancedNetwork>>,
}

impl DcOpfInstance {
    /// Build the instance with the default objective (the network's
    /// generator cost curves) and every stated limit active.
    ///
    /// # Errors
    /// A network with no reference bus or no in service generator to
    /// dispatch.
    pub fn from_network(network: BalancedNetwork) -> Result<Self, Error> {
        require_reference(&network)?;
        require_dispatchable(&network)?;
        Ok(Self {
            network,
            objective: Objective::network_generator_cost(),
            constraints: ActiveConstraints::default(),
            approximation: DcConvention::default(),
            initial_state: None,
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
    pub fn with_constraints(mut self, constraints: ActiveConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Select the DC branch approximation, consuming the instance.
    #[must_use]
    pub fn with_approximation(mut self, approximation: DcConvention) -> Self {
        self.approximation = approximation;
        self
    }

    /// Supply an optional solver initial state.
    #[must_use]
    pub fn with_initial_state(mut self, state: OperatingPoint<BalancedNetwork>) -> Self {
        self.initial_state = Some(state);
        self
    }

    /// The network this instance calculates on. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }

    /// The typed objective.
    #[must_use]
    pub const fn objective(&self) -> &Objective {
        &self.objective
    }

    /// The active constraint selections.
    #[must_use]
    pub const fn constraints(&self) -> &ActiveConstraints {
        &self.constraints
    }

    /// The selected DC branch approximation.
    #[must_use]
    pub const fn approximation(&self) -> DcConvention {
        self.approximation
    }

    /// The optional solver initial state.
    #[must_use]
    pub const fn initial_state(&self) -> Option<&OperatingPoint<BalancedNetwork>> {
        self.initial_state.as_ref()
    }

    /// The DC power flow instance for this problem's network at its stated
    /// injections: the objective and the constraint selections are
    /// discarded, and the discard is recorded.
    ///
    /// # Errors
    /// As [`DcPfInstance::from_network`].
    pub fn to_dc_pf(&self) -> Result<(DcPfInstance, Vec<powerio_core::Diagnostic>), Error> {
        let instance = DcPfInstance::from_network(self.network.clone())?
            .with_approximation(self.approximation);
        Ok((
            instance,
            vec![transform_discarded(
                "the objective and the active constraint selections",
            )],
        ))
    }
}

/// The AC optimal power flow instance: the shared network, the typed
/// objective, and the active generator capability, voltage, thermal, and
/// angle constraint selections.
#[derive(Clone, Debug)]
pub struct AcOpfInstance {
    network: BalancedNetwork,
    objective: Objective,
    constraints: ActiveConstraints,
    initial_state: Option<OperatingPoint<BalancedNetwork>>,
}

impl AcOpfInstance {
    /// Build the instance with the default objective (the network's
    /// generator cost curves) and every stated limit active.
    ///
    /// # Errors
    /// A network with no reference bus or no in service generator to
    /// dispatch.
    pub fn from_network(network: BalancedNetwork) -> Result<Self, Error> {
        require_reference(&network)?;
        require_dispatchable(&network)?;
        Ok(Self {
            network,
            objective: Objective::network_generator_cost(),
            constraints: ActiveConstraints::default(),
            initial_state: None,
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
    pub fn with_constraints(mut self, constraints: ActiveConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Supply an optional solver initial state.
    #[must_use]
    pub fn with_initial_state(mut self, state: OperatingPoint<BalancedNetwork>) -> Self {
        self.initial_state = Some(state);
        self
    }

    /// The network this instance calculates on. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }

    /// The typed objective.
    #[must_use]
    pub const fn objective(&self) -> &Objective {
        &self.objective
    }

    /// The active constraint selections.
    #[must_use]
    pub const fn constraints(&self) -> &ActiveConstraints {
        &self.constraints
    }

    /// The optional solver initial state.
    #[must_use]
    pub const fn initial_state(&self) -> Option<&OperatingPoint<BalancedNetwork>> {
        self.initial_state.as_ref()
    }

    /// The AC power flow instance for this problem's network at its stated
    /// injections and setpoints: the objective and the constraint selections
    /// are discarded, and the discard is recorded.
    ///
    /// # Errors
    /// As [`AcPfInstance::from_network`].
    pub fn to_ac_pf(&self) -> Result<(AcPfInstance, Vec<powerio_core::Diagnostic>), Error> {
        let instance = AcPfInstance::from_network(self.network.clone())?;
        Ok((
            instance,
            vec![transform_discarded(
                "the objective and the active constraint selections",
            )],
        ))
    }

    /// The DC optimal power flow instance this AC problem implies: the
    /// objective and the generator capability, thermal, and angle selections
    /// carry over; the voltage bound selection has no DC variable and is
    /// discarded, and the flat voltage assumption is recorded.
    #[must_use]
    pub fn to_dc_opf(&self) -> (DcOpfInstance, Vec<powerio_core::Diagnostic>) {
        let constraints = ActiveConstraints {
            generator_capability: self.constraints.generator_capability.clone(),
            voltage_bounds: crate::instance::ConstraintSelection::None,
            thermal_limits: self.constraints.thermal_limits.clone(),
            angle_bounds: self.constraints.angle_bounds.clone(),
        };
        let instance = DcOpfInstance {
            network: self.network.clone(),
            objective: self.objective.clone(),
            constraints,
            approximation: DcConvention::default(),
            initial_state: self.initial_state.clone(),
        };
        let diagnostics = vec![
            transform_discarded("the voltage bound constraint selection"),
            transform_assumption(
                "the DC approximation holds every voltage magnitude at one per unit",
            ),
        ];
        (instance, diagnostics)
    }
}

/// Net stated active injection at one bus, MW, over in service elements.
/// Per bus totals of the in service generators and loads, plus the voltage
/// setpoint agreement, gathered in one pass so instance construction stays
/// linear in bus plus generator plus load count.
#[derive(Default)]
struct BusAggregate {
    p_gen: f64,
    q_gen: f64,
    p_load: f64,
    q_load: f64,
    setpoint: Option<f64>,
    conflicting: Option<f64>,
}

fn aggregate_bus_elements(network: &BalancedNetwork) -> BTreeMap<BusId, BusAggregate> {
    let mut totals: BTreeMap<BusId, BusAggregate> = BTreeMap::new();
    for generator in network
        .generators()
        .iter()
        .filter(|generator| generator.in_service)
    {
        let entry = totals.entry(generator.bus).or_default();
        entry.p_gen += generator.pg;
        entry.q_gen += generator.qg;
        match entry.setpoint {
            None => entry.setpoint = Some(generator.vg),
            Some(existing) if existing.to_bits() == generator.vg.to_bits() => {}
            Some(_) => {
                if entry.conflicting.is_none() {
                    entry.conflicting = Some(generator.vg);
                }
            }
        }
    }
    for load in network.loads().iter().filter(|load| load.in_service) {
        let entry = totals.entry(load.bus).or_default();
        entry.p_load += load.p;
        entry.q_load += load.q;
    }
    totals
}

fn net_active_power(totals: &BTreeMap<BusId, BusAggregate>, bus: BusId) -> f64 {
    totals
        .get(&bus)
        .map_or(0.0, |entry| entry.p_gen - entry.p_load)
}

/// Net stated reactive injection at one bus, MVAr, over in service elements.
fn net_reactive_power(totals: &BTreeMap<BusId, BusAggregate>, bus: BusId) -> f64 {
    totals
        .get(&bus)
        .map_or(0.0, |entry| entry.q_gen - entry.q_load)
}

/// The controlled voltage magnitude at one bus: the in service generators'
/// shared setpoint, else the bus's stated magnitude. Two in service
/// generators stating different setpoints at one bus are conflicting active
/// voltage controllers and are refused.
fn controlled_magnitude(
    totals: &BTreeMap<BusId, BusAggregate>,
    bus: BusId,
    stated: f64,
) -> Result<f64, Error> {
    let Some(entry) = totals.get(&bus) else {
        return Ok(stated);
    };
    if let (Some(existing), Some(other)) = (entry.setpoint, entry.conflicting) {
        return Err(Error::new(
            &codes::BUILD_INSTANCE_VOLTAGE_CONTROL_CONFLICT,
            format!(
                "bus {bus} has in service generators stating voltage setpoints {existing} and {other}; resolve the conflict explicitly before constructing the power flow instance"
            ),
        ));
    }
    Ok(entry.setpoint.unwrap_or(stated))
}

fn require_reference(network: &BalancedNetwork) -> Result<(), Error> {
    if network.buses().iter().any(|bus| bus.kind == BusType::Ref) {
        Ok(())
    } else {
        Err(Error::new(
            &codes::BUILD_INSTANCE_NO_REFERENCE_BUS,
            "the network states no reference (slack) bus",
        ))
    }
}

fn require_dispatchable(network: &BalancedNetwork) -> Result<(), Error> {
    if network
        .generators()
        .iter()
        .any(|generator| generator.in_service)
    {
        Ok(())
    } else {
        Err(Error::new(
            &codes::BUILD_INSTANCE_NO_GENERATORS,
            "the network has no in service generator for the problem to dispatch",
        ))
    }
}

pub(crate) fn transform_discarded(what: &str) -> powerio_core::Diagnostic {
    powerio_core::Diagnostic::of(
        &codes::TRANSFORM_INSTANCE_DATA_DISCARDED,
        format!("{what} of the source instance are not part of the derived calculation"),
    )
}

pub(crate) fn transform_assumption(what: &str) -> powerio_core::Diagnostic {
    powerio_core::Diagnostic::of(&codes::TRANSFORM_INSTANCE_ASSUMPTION, what)
}
