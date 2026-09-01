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
//! [`OperatingPoint`] can be supplied as an optional solver initial point.
//! Zero impedance branches are preserved; a projection that cannot represent
//! them refuses at its own boundary and
//! [`merge_zero_impedance_buses`](super::merge_zero_impedance_buses) is the
//! explicit, checked resolution.

use std::collections::{BTreeMap, BTreeSet};

use powerio_core::Error;
use powerio_tx::{BalancedNetwork, BranchSusceptanceFormula, BusId, BusType};
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
/// specifications and the selected branch susceptance formula.
#[derive(Clone, Debug)]
pub struct DcPfInstance {
    network: BalancedNetwork,
    specifications: Vec<DcBusSpecification>,
    branch_susceptance_formula: BranchSusceptanceFormula,
    initial_point: Option<OperatingPoint<BalancedNetwork>>,
}

impl DcPfInstance {
    /// Build the instance from the network's stated data: reference buses
    /// contribute their stated angle, isolated buses no equation, and every
    /// other bus its net active injection over in service generators and
    /// loads.
    ///
    /// # Errors
    /// A network with no reference bus.
    pub fn from_network(mut network: BalancedNetwork) -> Result<Self, Error> {
        network.assign_missing_component_ids();
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
            branch_susceptance_formula: BranchSusceptanceFormula::default(),
            initial_point: None,
        })
    }

    /// Select the branch susceptance formula, consuming the instance. The
    /// network handle moves; no table is copied.
    #[must_use]
    pub fn with_branch_susceptance_formula(mut self, formula: BranchSusceptanceFormula) -> Self {
        self.branch_susceptance_formula = formula;
        self
    }

    /// Supply an optional solver initial point.
    #[must_use]
    pub fn with_initial_point(mut self, point: OperatingPoint<BalancedNetwork>) -> Self {
        self.initial_point = Some(point);
        self
    }

    /// Replace the network and recalculate the fixed bus specifications while
    /// preserving the branch susceptance formula and a compatible initial
    /// point.
    ///
    /// # Errors
    /// The replacement has no reference bus or changes an identity layout used
    /// by the initial point.
    pub fn with_network(mut self, mut network: BalancedNetwork) -> Result<Self, Error> {
        network.assign_missing_component_ids();
        let mut replacement = Self::from_network(network.clone())?
            .with_branch_susceptance_formula(self.branch_susceptance_formula);
        if let Some(initial) = self.initial_point.take() {
            replacement.initial_point = Some(initial.rebind_network(network)?);
        }
        Ok(replacement)
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

    /// The selected branch susceptance formula.
    #[must_use]
    pub const fn branch_susceptance_formula(&self) -> BranchSusceptanceFormula {
        self.branch_susceptance_formula
    }

    /// The optional solver initial point.
    #[must_use]
    pub const fn initial_point(&self) -> Option<&OperatingPoint<BalancedNetwork>> {
        self.initial_point.as_ref()
    }
}

/// The AC power flow instance: the shared network plus one
/// [`AcBusSpecification`] per bus.
#[derive(Clone, Debug)]
pub struct AcPfInstance {
    network: BalancedNetwork,
    specifications: Vec<AcBusSpecification>,
    initial_point: Option<OperatingPoint<BalancedNetwork>>,
}

impl AcPfInstance {
    /// Build an AC power flow instance from explicit bus specifications.
    /// The specification vector follows bus table order and is retained
    /// exactly; it is not inferred again from bus types, loads, or generator
    /// schedules.
    ///
    /// # Errors
    /// The specification count differs from the bus count, or no
    /// specification declares a reference bus.
    pub fn new(
        mut network: BalancedNetwork,
        specifications: Vec<AcBusSpecification>,
    ) -> Result<Self, Error> {
        network.assign_missing_component_ids();
        if specifications.len() != network.buses().len() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                format!(
                    "AC power flow specifications carry {} rows; the network has {} buses",
                    specifications.len(),
                    network.buses().len()
                ),
            ));
        }
        if !specifications
            .iter()
            .any(|specification| matches!(specification, AcBusSpecification::Reference { .. }))
        {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_NO_REFERENCE_BUS,
                "the AC power flow specifications state no reference (slack) bus",
            ));
        }
        Ok(Self {
            network,
            specifications,
            initial_point: None,
        })
    }

    /// Build the instance from the network's stated data. A PQ bus states
    /// its net injections; a PV bus its net active injection and the
    /// regulating generator's voltage setpoint; a reference bus its setpoint
    /// magnitude and stated angle.
    ///
    /// # Errors
    /// A network with no reference bus, or conflicting active voltage
    /// controllers: two in service generators at one bus stating different
    /// voltage setpoints are refused until an explicit edit resolves them.
    pub fn from_network(mut network: BalancedNetwork) -> Result<Self, Error> {
        network.assign_missing_component_ids();
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
        Self::new(network, specifications)
    }

    /// Supply an optional solver initial point.
    #[must_use]
    pub fn with_initial_point(mut self, point: OperatingPoint<BalancedNetwork>) -> Self {
        self.initial_point = Some(point);
        self
    }

    /// Replace the network and recalculate the fixed bus specifications while
    /// preserving a compatible initial point.
    ///
    /// # Errors
    /// The replacement has no reference bus, has conflicting voltage
    /// controllers, or changes an identity layout used by the initial point.
    pub fn with_network(mut self, mut network: BalancedNetwork) -> Result<Self, Error> {
        network.assign_missing_component_ids();
        let mut replacement = Self::from_network(network.clone())?;
        if let Some(initial) = self.initial_point.take() {
            replacement.initial_point = Some(initial.rebind_network(network)?);
        }
        Ok(replacement)
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

    /// The optional solver initial point.
    #[must_use]
    pub const fn initial_point(&self) -> Option<&OperatingPoint<BalancedNetwork>> {
        self.initial_point.as_ref()
    }

    /// The DC power flow instance this AC problem implies: reactive data and
    /// voltage magnitudes are discarded, and the DC model's flat voltage
    /// assumption is recorded as a diagnostic.
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
            branch_susceptance_formula: BranchSusceptanceFormula::default(),
            initial_point: self.initial_point.clone(),
        };
        let diagnostics = vec![
            transform_discarded("reactive power and voltage magnitude specifications"),
            transform_assumption(
                "the DC power flow model holds every voltage magnitude at one per unit",
            ),
        ];
        (instance, diagnostics)
    }
}

/// The DC optimal power flow instance: the shared network, the typed
/// objective, the active constraint selections, the selected DC branch
/// susceptance formula, and the reference conditions the network states.
#[derive(Clone, Debug)]
pub struct DcOpfInstance {
    network: BalancedNetwork,
    objective: Objective,
    constraints: ActiveConstraints,
    branch_susceptance_formula: BranchSusceptanceFormula,
    initial_point: Option<OperatingPoint<BalancedNetwork>>,
}

impl DcOpfInstance {
    /// Build the instance with every stated limit active. The default
    /// objective is the network's generator cost curves when at least one
    /// dispatchable generator carries cost data. A network with no applicable
    /// cost rows becomes an explicit feasibility problem instead of inventing
    /// zero cost curves. Partially populated costs still select the network
    /// objective so preparation reports the missing row.
    ///
    /// # Errors
    /// A network with no reference bus or no in service generator attached to
    /// a non-isolated bus.
    pub fn from_network(mut network: BalancedNetwork) -> Result<Self, Error> {
        network.assign_missing_component_ids();
        require_reference(&network)?;
        require_dispatchable(&network)?;
        let objective = default_opf_objective(&network);
        Ok(Self {
            network,
            objective,
            constraints: ActiveConstraints::default(),
            branch_susceptance_formula: BranchSusceptanceFormula::default(),
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
    pub fn with_constraints(mut self, constraints: ActiveConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Select the branch susceptance formula, consuming the instance.
    #[must_use]
    pub fn with_branch_susceptance_formula(mut self, formula: BranchSusceptanceFormula) -> Self {
        self.branch_susceptance_formula = formula;
        self
    }

    /// Supply an optional solver initial point.
    #[must_use]
    pub fn with_initial_point(mut self, point: OperatingPoint<BalancedNetwork>) -> Self {
        self.initial_point = Some(point);
        self
    }

    /// Replace the network while preserving this instance's objective,
    /// constraint selections, branch susceptance formula, and compatible initial point.
    /// This is the checked path for a parameter edit such as a branch rating
    /// change; callers do not have to reconstruct the problem and risk
    /// dropping its semantics.
    ///
    /// # Errors
    /// The replacement has no reference bus or dispatchable generator, or it
    /// changes an identity layout used by the initial point.
    pub fn with_network(mut self, mut network: BalancedNetwork) -> Result<Self, Error> {
        network.assign_missing_component_ids();
        require_reference(&network)?;
        require_dispatchable(&network)?;
        if let Some(initial) = self.initial_point.take() {
            self.initial_point = Some(initial.rebind_network(network.clone())?);
        }
        self.network = network;
        Ok(self)
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

    /// The selected branch susceptance formula.
    #[must_use]
    pub const fn branch_susceptance_formula(&self) -> BranchSusceptanceFormula {
        self.branch_susceptance_formula
    }

    /// The optional solver initial point.
    #[must_use]
    pub const fn initial_point(&self) -> Option<&OperatingPoint<BalancedNetwork>> {
        self.initial_point.as_ref()
    }

    /// The DC power flow instance for this problem's network at its stated
    /// injections: the objective and the constraint selections are
    /// discarded, and the discard is recorded.
    ///
    /// # Errors
    /// As [`DcPfInstance::from_network`].
    pub fn to_dc_pf(&self) -> Result<(DcPfInstance, Vec<powerio_core::Diagnostic>), Error> {
        let instance = DcPfInstance::from_network(self.network.clone())?
            .with_branch_susceptance_formula(self.branch_susceptance_formula);
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
    initial_point: Option<OperatingPoint<BalancedNetwork>>,
}

impl AcOpfInstance {
    /// Build the instance with every stated limit active. The default
    /// objective is the network's generator cost curves when at least one
    /// dispatchable generator carries cost data. A network with no applicable
    /// cost rows becomes an explicit feasibility problem instead of inventing
    /// zero cost curves. Partially populated costs still select the network
    /// objective so preparation reports the missing row.
    ///
    /// # Errors
    /// A network with no reference bus or no in service generator attached to
    /// a non-isolated bus.
    pub fn from_network(mut network: BalancedNetwork) -> Result<Self, Error> {
        network.assign_missing_component_ids();
        require_reference(&network)?;
        require_dispatchable(&network)?;
        let objective = default_opf_objective(&network);
        Ok(Self {
            network,
            objective,
            constraints: ActiveConstraints::default(),
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
    pub fn with_constraints(mut self, constraints: ActiveConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Supply an optional solver initial point.
    #[must_use]
    pub fn with_initial_point(mut self, point: OperatingPoint<BalancedNetwork>) -> Self {
        self.initial_point = Some(point);
        self
    }

    /// Replace the network while preserving this instance's objective,
    /// constraint selections, and compatible initial point.
    ///
    /// # Errors
    /// The replacement has no reference bus or dispatchable generator, or it
    /// changes an identity layout used by the initial point.
    pub fn with_network(mut self, mut network: BalancedNetwork) -> Result<Self, Error> {
        network.assign_missing_component_ids();
        require_reference(&network)?;
        require_dispatchable(&network)?;
        if let Some(initial) = self.initial_point.take() {
            self.initial_point = Some(initial.rebind_network(network.clone())?);
        }
        self.network = network;
        Ok(self)
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

    /// The optional solver initial point.
    #[must_use]
    pub const fn initial_point(&self) -> Option<&OperatingPoint<BalancedNetwork>> {
        self.initial_point.as_ref()
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
            branch_susceptance_formula: BranchSusceptanceFormula::default(),
            initial_point: self.initial_point.clone(),
        };
        let diagnostics = vec![
            transform_discarded("the voltage bound constraint selection"),
            transform_assumption(
                "the DC power flow model holds every voltage magnitude at one per unit",
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
    let active_buses = active_bus_ids(network);
    if network
        .generators()
        .iter()
        .any(|generator| generator.in_service && active_buses.contains(&generator.bus))
    {
        Ok(())
    } else {
        Err(Error::new(
            &codes::BUILD_INSTANCE_NO_GENERATORS,
            "the network has no in service generator for the problem to dispatch",
        ))
    }
}

fn default_opf_objective(network: &BalancedNetwork) -> Objective {
    let active_buses = active_bus_ids(network);
    if network.generators().iter().any(|generator| {
        generator.in_service && active_buses.contains(&generator.bus) && generator.cost.is_some()
    }) {
        Objective::network_generator_cost()
    } else {
        Objective::none()
    }
}

fn active_bus_ids(network: &BalancedNetwork) -> BTreeSet<BusId> {
    network
        .buses()
        .iter()
        .filter(|bus| bus.kind != BusType::Isolated)
        .map(|bus| bus.id)
        .collect()
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
