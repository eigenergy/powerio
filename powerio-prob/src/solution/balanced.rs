//! The balanced solutions: `DcPfSolution`, `AcPfSolution`, `DcOpfSolution`,
//! and `AcOpfSolution`.
//!
//! Values are stored in the shared network's table order and read back by
//! stable identity: buses by [`BusId`], branches and generators by payload
//! identity (`uid`, else `{table}:{row}`). Bus injections, bus voltages, and
//! branch flows are required on the power flow solutions; individual
//! generator outputs stay optional unless the instance determines them
//! uniquely or the source records an explicit allocation, and the OPF
//! solutions require the dispatch they optimized.

use std::sync::Arc;

use powerio_core::Error;
use powerio_tx::{BalancedNetwork, BusId};

use crate::diagnostics::codes;
use crate::instance::{AcOpfInstance, AcPfInstance, DcOpfInstance, DcPfInstance};
use crate::solution::{Producer, Residuals, Termination};
use crate::state::row_identity;

/// Optional per generator dispatch, in generator table order.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct GeneratorDispatch {
    /// Active power per generator, MW.
    pub p_mw: Vec<f64>,
    /// Reactive power per generator, MVAr; empty for a DC result.
    pub q_mvar: Vec<f64>,
}

fn check_length(what: &'static str, got: usize, expected: usize) -> Result<(), Error> {
    if got == expected {
        Ok(())
    } else {
        Err(Error::new(
            &codes::BUILD_SOLUTION_SHAPE_MISMATCH,
            format!("{what} carries {got} values; the instance's table has {expected} rows"),
        ))
    }
}

fn check_nonnegative_multipliers(what: &'static str, values: &[f64]) -> Result<(), Error> {
    if let Some((row, value)) = values
        .iter()
        .copied()
        .enumerate()
        .find(|(_, value)| !value.is_finite() || *value < 0.0)
    {
        return Err(Error::new(
            &codes::BUILD_SOLUTION_MULTIPLIER_INVALID,
            format!("{what} row {row} is {value}; multipliers must be finite and nonnegative"),
        ));
    }
    Ok(())
}

/// Identity to row position over one network's tables, built once per
/// solution on first keyed access so repeated reads never rescan a table.
#[derive(Clone, Debug, Default)]
struct SolutionIndex {
    bus: std::collections::BTreeMap<BusId, usize>,
    branch: std::collections::BTreeMap<String, usize>,
    generator: std::collections::BTreeMap<String, usize>,
}

impl SolutionIndex {
    fn build(network: &BalancedNetwork) -> Result<Self, Error> {
        let mut index = Self::default();
        for (row, bus) in network.buses().iter().enumerate() {
            if index.bus.insert(bus.id, row).is_some() {
                return Err(duplicate_identity("bus", &bus.id.to_string()));
            }
        }
        for (row, branch) in network.branches().iter().enumerate() {
            let identity = row_identity(branch.uid.as_deref(), "branches", row);
            if index.branch.insert(identity.clone(), row).is_some() {
                return Err(duplicate_identity("branch", &identity));
            }
        }
        for (row, generator) in network.generators().iter().enumerate() {
            let identity = row_identity(generator.uid.as_deref(), "generators", row);
            if index.generator.insert(identity.clone(), row).is_some() {
                return Err(duplicate_identity("generator", &identity));
            }
        }
        Ok(index)
    }
}

/// The identity index a solution constructor builds once, refusing a network
/// whose resolved identities are not all distinct so every keyed accessor
/// reads its own row.
fn solution_index<I>(instance: &std::sync::Arc<I>) -> Result<SolutionIndex, Error>
where
    I: NetworkCarrier,
{
    SolutionIndex::build(instance.network())
}

/// The one thing solution_index needs from each instance type.
trait NetworkCarrier {
    fn network(&self) -> &BalancedNetwork;
}

impl NetworkCarrier for DcPfInstance {
    fn network(&self) -> &BalancedNetwork {
        DcPfInstance::network(self)
    }
}
impl NetworkCarrier for AcPfInstance {
    fn network(&self) -> &BalancedNetwork {
        AcPfInstance::network(self)
    }
}
impl NetworkCarrier for DcOpfInstance {
    fn network(&self) -> &BalancedNetwork {
        DcOpfInstance::network(self)
    }
}
impl NetworkCarrier for AcOpfInstance {
    fn network(&self) -> &BalancedNetwork {
        AcOpfInstance::network(self)
    }
}

fn duplicate_identity(kind: &str, identity: &str) -> Error {
    Error::new(
        &codes::BUILD_STATE_IDENTITY_UNKNOWN,
        format!("{kind}: duplicate element identity `{identity}`"),
    )
}

fn bus_position(index: &SolutionIndex, bus: BusId) -> Option<usize> {
    index.bus.get(&bus).copied()
}

fn branch_position(index: &SolutionIndex, identity: &str) -> Option<usize> {
    index.branch.get(identity).copied()
}

fn generator_position(index: &SolutionIndex, identity: &str) -> Option<usize> {
    index.generator.get(identity).copied()
}

macro_rules! shared_solution_accessors {
    ($instance_type:ty) => {
        /// The immutable instance this solution solves. Borrowed; never a
        /// copy.
        #[must_use]
        pub fn instance(&self) -> &$instance_type {
            &self.instance
        }

        /// The shared instance owner, for another solution of the same
        /// problem.
        #[must_use]
        pub fn shared_instance(&self) -> Arc<$instance_type> {
            Arc::clone(&self.instance)
        }

        /// The network the solved instance calculates on.
        #[must_use]
        pub fn network(&self) -> &BalancedNetwork {
            self.instance.network()
        }

        fn row_index(&self) -> &SolutionIndex {
            &self.index
        }

        /// Bus IDs in the column order every bulk accessor uses.
        #[must_use]
        pub fn bus_order(&self) -> Vec<BusId> {
            self.network().buses().iter().map(|bus| bus.id).collect()
        }

        /// Stable branch identities in bulk column order.
        #[must_use]
        pub fn branch_order(&self) -> Vec<String> {
            self.network()
                .branches()
                .iter()
                .enumerate()
                .map(|(row, branch)| row_identity(branch.uid.as_deref(), "branches", row))
                .collect()
        }

        /// Stable generator identities in bulk column order.
        #[must_use]
        pub fn generator_order(&self) -> Vec<String> {
            self.network()
                .generators()
                .iter()
                .enumerate()
                .map(|(row, generator)| row_identity(generator.uid.as_deref(), "generators", row))
                .collect()
        }

        /// How the producing calculation ended.
        #[must_use]
        pub fn termination(&self) -> &Termination {
            &self.termination
        }

        /// The reported numerical residuals.
        #[must_use]
        pub fn residuals(&self) -> &Residuals {
            &self.residuals
        }

        /// The producer or solver identity, when recorded.
        #[must_use]
        pub fn producer(&self) -> Option<&str> {
            self.producer.as_deref()
        }

        /// Record the producer identity.
        #[must_use]
        pub fn with_producer(mut self, producer: impl Into<String>) -> Self {
            self.producer = Some(producer.into());
            self
        }

        /// Record the numerical residuals.
        #[must_use]
        pub fn with_residuals(mut self, residuals: Residuals) -> Self {
            self.residuals = residuals;
            self
        }

        /// The branch identities the flow columns follow, in table order.
        pub fn branch_identity_order(&self) -> impl Iterator<Item = String> + '_ {
            self.network()
                .branches()
                .iter()
                .enumerate()
                .map(|(row, branch)| row_identity(branch.uid.as_deref(), "branches", row))
        }
    };
}

macro_rules! optional_dispatch_accessors {
    () => {
        /// Per generator dispatch, when the instance determines it uniquely
        /// or the source records an explicit allocation.
        #[must_use]
        pub fn generator_dispatch(&self) -> Option<&GeneratorDispatch> {
            self.generator_dispatch.as_ref()
        }

        /// Record an explicit per generator allocation.
        ///
        /// # Errors
        /// A dispatch whose length disagrees with the generator table.
        pub fn with_generator_dispatch(
            mut self,
            dispatch: GeneratorDispatch,
        ) -> Result<Self, Error> {
            check_length(
                "generator dispatch",
                dispatch.p_mw.len(),
                self.network().generators().len(),
            )?;
            if !dispatch.q_mvar.is_empty() {
                check_length(
                    "generator reactive dispatch",
                    dispatch.q_mvar.len(),
                    self.network().generators().len(),
                )?;
            }
            self.generator_dispatch = Some(dispatch);
            Ok(self)
        }
    };
}

/// The DC power flow solution: bus angles and injections and branch terminal
/// active flows over the shared instance.
#[derive(Clone, Debug)]
pub struct DcPfSolution {
    instance: Arc<DcPfInstance>,
    termination: Termination,
    residuals: Residuals,
    producer: Producer,
    bus_voltage_angle: Vec<f64>,
    bus_active_injection: Vec<f64>,
    branch_from_active_flow: Vec<f64>,
    branch_to_active_flow: Vec<f64>,
    generator_dispatch: Option<GeneratorDispatch>,
    index: SolutionIndex,
}

impl DcPfSolution {
    /// Assemble the required results: per bus voltage angles (degrees) and
    /// net active injections (MW) in bus table order, and per branch terminal
    /// active flows (MW, into the branch at each terminal) in branch table
    /// order.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's tables.
    pub fn new(
        instance: Arc<DcPfInstance>,
        termination: Termination,
        bus_voltage_angle: Vec<f64>,
        bus_active_injection: Vec<f64>,
        branch_from_active_flow: Vec<f64>,
        branch_to_active_flow: Vec<f64>,
    ) -> Result<Self, Error> {
        let buses = instance.network().buses().len();
        let branches = instance.network().branches().len();
        check_length("bus voltage angles", bus_voltage_angle.len(), buses)?;
        check_length("bus active injections", bus_active_injection.len(), buses)?;
        check_length(
            "branch from-side flows",
            branch_from_active_flow.len(),
            branches,
        )?;
        check_length(
            "branch to-side flows",
            branch_to_active_flow.len(),
            branches,
        )?;
        let index = solution_index(&instance)?;
        Ok(Self {
            instance,
            termination,
            residuals: Residuals::default(),
            producer: None,
            bus_voltage_angle,
            bus_active_injection,
            branch_from_active_flow,
            branch_to_active_flow,
            generator_dispatch: None,
            index,
        })
    }

    shared_solution_accessors!(DcPfInstance);
    optional_dispatch_accessors!();

    /// Voltage angle at one bus, degrees.
    #[must_use]
    pub fn bus_voltage_angle(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_angle[bus_position(self.row_index(), bus)?])
    }

    /// Net active injection at one bus, MW.
    #[must_use]
    pub fn bus_active_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_injection[bus_position(self.row_index(), bus)?])
    }

    /// Active flow into the branch at its from terminal, MW, by stable
    /// branch identity.
    #[must_use]
    pub fn branch_from_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_active_flow[branch_position(self.row_index(), identity)?])
    }

    /// Active flow into the branch at its to terminal, MW.
    #[must_use]
    pub fn branch_to_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_active_flow[branch_position(self.row_index(), identity)?])
    }

    /// The complete `bus_voltage_angle` column, in `bus_order`.
    #[must_use]
    pub fn bus_voltage_angles(&self) -> &[f64] {
        &self.bus_voltage_angle
    }
    /// The complete `bus_active_injection` column, in `bus_order`.
    #[must_use]
    pub fn bus_active_injections(&self) -> &[f64] {
        &self.bus_active_injection
    }
    /// The complete `branch_from_active_flow` column, in `branch_order`.
    #[must_use]
    pub fn branch_from_active_flows(&self) -> &[f64] {
        &self.branch_from_active_flow
    }
    /// The complete `branch_to_active_flow` column, in `branch_order`.
    #[must_use]
    pub fn branch_to_active_flows(&self) -> &[f64] {
        &self.branch_to_active_flow
    }
}

/// The AC power flow solution: complex bus voltages, active and reactive bus
/// injections, and terminal branch flows over the shared instance.
#[derive(Clone, Debug)]
pub struct AcPfSolution {
    instance: Arc<AcPfInstance>,
    termination: Termination,
    residuals: Residuals,
    producer: Producer,
    bus_voltage_magnitude: Vec<f64>,
    bus_voltage_angle: Vec<f64>,
    bus_active_injection: Vec<f64>,
    bus_reactive_injection: Vec<f64>,
    branch_from_active_flow: Vec<f64>,
    branch_from_reactive_flow: Vec<f64>,
    branch_to_active_flow: Vec<f64>,
    branch_to_reactive_flow: Vec<f64>,
    generator_dispatch: Option<GeneratorDispatch>,
    index: SolutionIndex,
}

impl AcPfSolution {
    /// Assemble the required results: per bus voltage magnitudes (per unit)
    /// and angles (degrees), net injections (MW, MVAr) in bus table order,
    /// and per branch terminal flows (MW, MVAr into the branch at each
    /// terminal) in branch table order.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's tables.
    #[allow(clippy::too_many_arguments)] // the required result set is the signature
    pub fn new(
        instance: Arc<AcPfInstance>,
        termination: Termination,
        bus_voltage_magnitude: Vec<f64>,
        bus_voltage_angle: Vec<f64>,
        bus_active_injection: Vec<f64>,
        bus_reactive_injection: Vec<f64>,
        branch_from_active_flow: Vec<f64>,
        branch_from_reactive_flow: Vec<f64>,
        branch_to_active_flow: Vec<f64>,
        branch_to_reactive_flow: Vec<f64>,
    ) -> Result<Self, Error> {
        let buses = instance.network().buses().len();
        let branches = instance.network().branches().len();
        check_length("bus voltage magnitudes", bus_voltage_magnitude.len(), buses)?;
        check_length("bus voltage angles", bus_voltage_angle.len(), buses)?;
        check_length("bus active injections", bus_active_injection.len(), buses)?;
        check_length(
            "bus reactive injections",
            bus_reactive_injection.len(),
            buses,
        )?;
        check_length(
            "branch from-side active flows",
            branch_from_active_flow.len(),
            branches,
        )?;
        check_length(
            "branch from-side reactive flows",
            branch_from_reactive_flow.len(),
            branches,
        )?;
        check_length(
            "branch to-side active flows",
            branch_to_active_flow.len(),
            branches,
        )?;
        check_length(
            "branch to-side reactive flows",
            branch_to_reactive_flow.len(),
            branches,
        )?;
        let index = solution_index(&instance)?;
        Ok(Self {
            instance,
            termination,
            residuals: Residuals::default(),
            producer: None,
            bus_voltage_magnitude,
            bus_voltage_angle,
            bus_active_injection,
            bus_reactive_injection,
            branch_from_active_flow,
            branch_from_reactive_flow,
            branch_to_active_flow,
            branch_to_reactive_flow,
            generator_dispatch: None,
            index,
        })
    }

    shared_solution_accessors!(AcPfInstance);
    optional_dispatch_accessors!();

    /// Voltage magnitude at one bus, per unit.
    #[must_use]
    pub fn bus_voltage_magnitude(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_magnitude[bus_position(self.row_index(), bus)?])
    }

    /// Voltage angle at one bus, degrees.
    #[must_use]
    pub fn bus_voltage_angle(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_angle[bus_position(self.row_index(), bus)?])
    }

    /// Net active injection at one bus, MW.
    #[must_use]
    pub fn bus_active_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_injection[bus_position(self.row_index(), bus)?])
    }

    /// Net reactive injection at one bus, MVAr.
    #[must_use]
    pub fn bus_reactive_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_reactive_injection[bus_position(self.row_index(), bus)?])
    }

    /// Active flow into the branch at its from terminal, MW, by stable
    /// branch identity.
    #[must_use]
    pub fn branch_from_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_active_flow[branch_position(self.row_index(), identity)?])
    }

    /// Reactive flow into the branch at its from terminal, MVAr.
    #[must_use]
    pub fn branch_from_reactive_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_reactive_flow[branch_position(self.row_index(), identity)?])
    }

    /// Active flow into the branch at its to terminal, MW.
    #[must_use]
    pub fn branch_to_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_active_flow[branch_position(self.row_index(), identity)?])
    }

    /// Reactive flow into the branch at its to terminal, MVAr.
    #[must_use]
    pub fn branch_to_reactive_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_reactive_flow[branch_position(self.row_index(), identity)?])
    }
}

/// The DC optimal power flow solution: the DC power flow results plus the
/// optimized generator active dispatch, the objective value, and the
/// optional economic outputs an optimizing producer can attach.
#[derive(Clone, Debug)]
pub struct DcOpfSolution {
    instance: Arc<DcOpfInstance>,
    termination: Termination,
    residuals: Residuals,
    producer: Producer,
    bus_voltage_angle: Vec<f64>,
    bus_active_injection: Vec<f64>,
    branch_from_active_flow: Vec<f64>,
    branch_to_active_flow: Vec<f64>,
    generator_active_power: Vec<f64>,
    objective: f64,
    bus_active_power_marginal: Option<Vec<f64>>,
    branch_from_limit_multiplier: Option<Vec<f64>>,
    branch_to_limit_multiplier: Option<Vec<f64>>,
    index: SolutionIndex,
}

impl DcOpfSolution {
    /// Assemble the results: the DC power flow columns, the optimized per
    /// generator active dispatch (MW, generator table order), and the
    /// objective value.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's tables.
    #[allow(clippy::too_many_arguments)] // the required result set is the signature
    pub fn new(
        instance: Arc<DcOpfInstance>,
        termination: Termination,
        bus_voltage_angle: Vec<f64>,
        bus_active_injection: Vec<f64>,
        branch_from_active_flow: Vec<f64>,
        branch_to_active_flow: Vec<f64>,
        generator_active_power: Vec<f64>,
        objective: f64,
    ) -> Result<Self, Error> {
        let buses = instance.network().buses().len();
        let branches = instance.network().branches().len();
        check_length("bus voltage angles", bus_voltage_angle.len(), buses)?;
        check_length("bus active injections", bus_active_injection.len(), buses)?;
        check_length(
            "branch from-side flows",
            branch_from_active_flow.len(),
            branches,
        )?;
        check_length(
            "branch to-side flows",
            branch_to_active_flow.len(),
            branches,
        )?;
        check_length(
            "generator active dispatch",
            generator_active_power.len(),
            instance.network().generators().len(),
        )?;
        let index = solution_index(&instance)?;
        Ok(Self {
            instance,
            termination,
            residuals: Residuals::default(),
            producer: None,
            bus_voltage_angle,
            bus_active_injection,
            branch_from_active_flow,
            branch_to_active_flow,
            generator_active_power,
            objective,
            bus_active_power_marginal: None,
            branch_from_limit_multiplier: None,
            branch_to_limit_multiplier: None,
            index,
        })
    }

    shared_solution_accessors!(DcOpfInstance);

    /// Attach the derivative of the optimal objective with respect to added
    /// active demand at each bus, in objective units per MW and bus table
    /// order. A network generator cost objective gives the usual active power
    /// locational marginal price; a different objective does not imply money.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's bus table.
    pub fn with_bus_active_power_marginals(mut self, marginals: Vec<f64>) -> Result<Self, Error> {
        check_length(
            "bus active power marginals",
            marginals.len(),
            self.instance.network().buses().len(),
        )?;
        self.bus_active_power_marginal = Some(marginals);
        Ok(self)
    }

    /// Attach the two nonnegative KKT multipliers for every branch thermal
    /// bound, in objective units per MW and branch table order. `from` is the
    /// multiplier on `flow <= rating`; `to` is the multiplier on
    /// `-flow <= rating`. Increasing one symmetric rating by one MW changes
    /// the local optimal objective by the negative sum of the two values. A
    /// branch omitted by the approximation carries zero in both columns.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's branch table.
    pub fn with_branch_thermal_limit_multipliers(
        mut self,
        from: Vec<f64>,
        to: Vec<f64>,
    ) -> Result<Self, Error> {
        check_length(
            "branch from-side thermal limit multipliers",
            from.len(),
            self.instance.network().branches().len(),
        )?;
        check_length(
            "branch to-side thermal limit multipliers",
            to.len(),
            self.instance.network().branches().len(),
        )?;
        check_nonnegative_multipliers("branch from-side thermal limit multipliers", &from)?;
        check_nonnegative_multipliers("branch to-side thermal limit multipliers", &to)?;
        self.branch_from_limit_multiplier = Some(from);
        self.branch_to_limit_multiplier = Some(to);
        Ok(self)
    }

    /// The optimized objective value.
    #[must_use]
    pub const fn objective(&self) -> f64 {
        self.objective
    }

    /// Optimized active power of one generator, MW, by stable identity.
    #[must_use]
    pub fn generator_active_power(&self, identity: &str) -> Option<f64> {
        Some(self.generator_active_power[generator_position(self.row_index(), identity)?])
    }

    /// Optimal objective derivative per added MW of active demand at one bus.
    #[must_use]
    pub fn bus_active_power_marginal(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_power_marginal.as_ref()?[bus_position(self.row_index(), bus)?])
    }

    /// From-side thermal bound multiplier by stable branch identity.
    #[must_use]
    pub fn branch_from_limit_multiplier(&self, identity: &str) -> Option<f64> {
        Some(
            self.branch_from_limit_multiplier.as_ref()?
                [branch_position(self.row_index(), identity)?],
        )
    }

    /// To-side thermal bound multiplier by stable branch identity.
    #[must_use]
    pub fn branch_to_limit_multiplier(&self, identity: &str) -> Option<f64> {
        Some(
            self.branch_to_limit_multiplier.as_ref()?[branch_position(self.row_index(), identity)?],
        )
    }

    /// All active demand marginals in bus table order, when attached.
    #[must_use]
    pub fn bus_active_power_marginals(&self) -> Option<&[f64]> {
        self.bus_active_power_marginal.as_deref()
    }

    /// All from-side thermal bound multipliers in branch table order.
    #[must_use]
    pub fn branch_from_limit_multipliers(&self) -> Option<&[f64]> {
        self.branch_from_limit_multiplier.as_deref()
    }

    /// All to-side thermal bound multipliers in branch table order.
    #[must_use]
    pub fn branch_to_limit_multipliers(&self) -> Option<&[f64]> {
        self.branch_to_limit_multiplier.as_deref()
    }

    /// Voltage angle at one bus, degrees.
    #[must_use]
    pub fn bus_voltage_angle(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_angle[bus_position(self.row_index(), bus)?])
    }

    /// Net active injection at one bus, MW.
    #[must_use]
    pub fn bus_active_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_injection[bus_position(self.row_index(), bus)?])
    }

    /// Active flow into the branch at its from terminal, MW.
    #[must_use]
    pub fn branch_from_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_active_flow[branch_position(self.row_index(), identity)?])
    }

    /// Active flow into the branch at its to terminal, MW.
    #[must_use]
    pub fn branch_to_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_active_flow[branch_position(self.row_index(), identity)?])
    }
}

/// The AC optimal power flow solution: the AC power flow results plus the
/// optimized generator active and reactive dispatch and the objective value.
#[derive(Clone, Debug)]
pub struct AcOpfSolution {
    instance: Arc<AcOpfInstance>,
    termination: Termination,
    residuals: Residuals,
    producer: Producer,
    bus_voltage_magnitude: Vec<f64>,
    bus_voltage_angle: Vec<f64>,
    bus_active_injection: Vec<f64>,
    bus_reactive_injection: Vec<f64>,
    branch_from_active_flow: Vec<f64>,
    branch_from_reactive_flow: Vec<f64>,
    branch_to_active_flow: Vec<f64>,
    branch_to_reactive_flow: Vec<f64>,
    generator_active_power: Vec<f64>,
    generator_reactive_power: Vec<f64>,
    objective: f64,
    bus_active_power_marginal: Option<Vec<f64>>,
    bus_reactive_power_marginal: Option<Vec<f64>>,
    branch_from_limit_multiplier: Option<Vec<f64>>,
    branch_to_limit_multiplier: Option<Vec<f64>>,
    index: SolutionIndex,
}

impl AcOpfSolution {
    /// Assemble the results: the AC power flow columns, the optimized per
    /// generator dispatch (MW and MVAr, generator table order), and the
    /// objective value.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's tables.
    #[allow(clippy::too_many_arguments)] // the required result set is the signature
    pub fn new(
        instance: Arc<AcOpfInstance>,
        termination: Termination,
        bus_voltage_magnitude: Vec<f64>,
        bus_voltage_angle: Vec<f64>,
        bus_active_injection: Vec<f64>,
        bus_reactive_injection: Vec<f64>,
        branch_from_active_flow: Vec<f64>,
        branch_from_reactive_flow: Vec<f64>,
        branch_to_active_flow: Vec<f64>,
        branch_to_reactive_flow: Vec<f64>,
        generator_active_power: Vec<f64>,
        generator_reactive_power: Vec<f64>,
        objective: f64,
    ) -> Result<Self, Error> {
        let buses = instance.network().buses().len();
        let branches = instance.network().branches().len();
        let generators = instance.network().generators().len();
        check_length("bus voltage magnitudes", bus_voltage_magnitude.len(), buses)?;
        check_length("bus voltage angles", bus_voltage_angle.len(), buses)?;
        check_length("bus active injections", bus_active_injection.len(), buses)?;
        check_length(
            "bus reactive injections",
            bus_reactive_injection.len(),
            buses,
        )?;
        check_length(
            "branch from-side active flows",
            branch_from_active_flow.len(),
            branches,
        )?;
        check_length(
            "branch from-side reactive flows",
            branch_from_reactive_flow.len(),
            branches,
        )?;
        check_length(
            "branch to-side active flows",
            branch_to_active_flow.len(),
            branches,
        )?;
        check_length(
            "branch to-side reactive flows",
            branch_to_reactive_flow.len(),
            branches,
        )?;
        check_length(
            "generator active dispatch",
            generator_active_power.len(),
            generators,
        )?;
        check_length(
            "generator reactive dispatch",
            generator_reactive_power.len(),
            generators,
        )?;
        let index = solution_index(&instance)?;
        Ok(Self {
            instance,
            termination,
            residuals: Residuals::default(),
            producer: None,
            bus_voltage_magnitude,
            bus_voltage_angle,
            bus_active_injection,
            bus_reactive_injection,
            branch_from_active_flow,
            branch_from_reactive_flow,
            branch_to_active_flow,
            branch_to_reactive_flow,
            generator_active_power,
            generator_reactive_power,
            objective,
            bus_active_power_marginal: None,
            bus_reactive_power_marginal: None,
            branch_from_limit_multiplier: None,
            branch_to_limit_multiplier: None,
            index,
        })
    }

    shared_solution_accessors!(AcOpfInstance);

    /// Attach the derivative of the optimal objective with respect to added
    /// active demand, in objective units per MW and bus table order.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's bus table.
    pub fn with_bus_active_power_marginals(mut self, marginals: Vec<f64>) -> Result<Self, Error> {
        check_length(
            "bus active power marginals",
            marginals.len(),
            self.instance.network().buses().len(),
        )?;
        self.bus_active_power_marginal = Some(marginals);
        Ok(self)
    }

    /// Attach the derivative of the optimal objective with respect to added
    /// reactive demand, in objective units per MVAr and bus table order.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's bus table.
    pub fn with_bus_reactive_power_marginals(mut self, marginals: Vec<f64>) -> Result<Self, Error> {
        check_length(
            "bus reactive power marginals",
            marginals.len(),
            self.instance.network().buses().len(),
        )?;
        self.bus_reactive_power_marginal = Some(marginals);
        Ok(self)
    }

    /// Attach the two nonnegative apparent power limit multipliers, in
    /// objective units per MVA and branch table order. Increasing the shared
    /// rating by one MVA changes the local optimal objective by the negative
    /// sum of the from and to terminal multipliers.
    pub fn with_branch_thermal_limit_multipliers(
        mut self,
        from: Vec<f64>,
        to: Vec<f64>,
    ) -> Result<Self, Error> {
        check_length(
            "branch from-terminal thermal limit multipliers",
            from.len(),
            self.instance.network().branches().len(),
        )?;
        check_length(
            "branch to-terminal thermal limit multipliers",
            to.len(),
            self.instance.network().branches().len(),
        )?;
        check_nonnegative_multipliers("branch from-terminal thermal limit multipliers", &from)?;
        check_nonnegative_multipliers("branch to-terminal thermal limit multipliers", &to)?;
        self.branch_from_limit_multiplier = Some(from);
        self.branch_to_limit_multiplier = Some(to);
        Ok(self)
    }

    /// The optimized objective value.
    #[must_use]
    pub const fn objective(&self) -> f64 {
        self.objective
    }

    /// Optimal objective derivative per added MW of active demand at one bus.
    #[must_use]
    pub fn bus_active_power_marginal(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_power_marginal.as_ref()?[bus_position(self.row_index(), bus)?])
    }

    /// Optimal objective derivative per added MVAr of reactive demand.
    #[must_use]
    pub fn bus_reactive_power_marginal(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_reactive_power_marginal.as_ref()?[bus_position(self.row_index(), bus)?])
    }

    /// From-terminal apparent power bound multiplier by branch identity.
    #[must_use]
    pub fn branch_from_limit_multiplier(&self, identity: &str) -> Option<f64> {
        Some(
            self.branch_from_limit_multiplier.as_ref()?
                [branch_position(self.row_index(), identity)?],
        )
    }

    /// To-terminal apparent power bound multiplier by branch identity.
    #[must_use]
    pub fn branch_to_limit_multiplier(&self, identity: &str) -> Option<f64> {
        Some(
            self.branch_to_limit_multiplier.as_ref()?[branch_position(self.row_index(), identity)?],
        )
    }

    /// All active demand marginals in bus table order, when attached.
    #[must_use]
    pub fn bus_active_power_marginals(&self) -> Option<&[f64]> {
        self.bus_active_power_marginal.as_deref()
    }

    /// All reactive demand marginals in bus table order, when attached.
    #[must_use]
    pub fn bus_reactive_power_marginals(&self) -> Option<&[f64]> {
        self.bus_reactive_power_marginal.as_deref()
    }

    /// All from-terminal thermal bound multipliers in branch table order.
    #[must_use]
    pub fn branch_from_limit_multipliers(&self) -> Option<&[f64]> {
        self.branch_from_limit_multiplier.as_deref()
    }

    /// All to-terminal thermal bound multipliers in branch table order.
    #[must_use]
    pub fn branch_to_limit_multipliers(&self) -> Option<&[f64]> {
        self.branch_to_limit_multiplier.as_deref()
    }

    /// Optimized active power of one generator, MW, by stable identity.
    #[must_use]
    pub fn generator_active_power(&self, identity: &str) -> Option<f64> {
        Some(self.generator_active_power[generator_position(self.row_index(), identity)?])
    }

    /// Optimized reactive power of one generator, MVAr.
    #[must_use]
    pub fn generator_reactive_power(&self, identity: &str) -> Option<f64> {
        Some(self.generator_reactive_power[generator_position(self.row_index(), identity)?])
    }

    /// Voltage magnitude at one bus, per unit.
    #[must_use]
    pub fn bus_voltage_magnitude(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_magnitude[bus_position(self.row_index(), bus)?])
    }

    /// Voltage angle at one bus, degrees.
    #[must_use]
    pub fn bus_voltage_angle(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_angle[bus_position(self.row_index(), bus)?])
    }

    /// Net active injection at one bus, MW.
    #[must_use]
    pub fn bus_active_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_injection[bus_position(self.row_index(), bus)?])
    }

    /// Net reactive injection at one bus, MVAr.
    #[must_use]
    pub fn bus_reactive_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_reactive_injection[bus_position(self.row_index(), bus)?])
    }

    /// Active flow into the branch at its from terminal, MW.
    #[must_use]
    pub fn branch_from_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_active_flow[branch_position(self.row_index(), identity)?])
    }

    /// Reactive flow into the branch at its from terminal, MVAr.
    #[must_use]
    pub fn branch_from_reactive_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_reactive_flow[branch_position(self.row_index(), identity)?])
    }

    /// Active flow into the branch at its to terminal, MW.
    #[must_use]
    pub fn branch_to_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_active_flow[branch_position(self.row_index(), identity)?])
    }

    /// Reactive flow into the branch at its to terminal, MVAr.
    #[must_use]
    pub fn branch_to_reactive_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_reactive_flow[branch_position(self.row_index(), identity)?])
    }

    /// The complete `bus_voltage_magnitude` column, in `bus_order`.
    #[must_use]
    pub fn bus_voltage_magnitudes(&self) -> &[f64] {
        &self.bus_voltage_magnitude
    }
    /// The complete `bus_voltage_angle` column, in `bus_order`.
    #[must_use]
    pub fn bus_voltage_angles(&self) -> &[f64] {
        &self.bus_voltage_angle
    }
    /// The complete `bus_active_injection` column, in `bus_order`.
    #[must_use]
    pub fn bus_active_injections(&self) -> &[f64] {
        &self.bus_active_injection
    }
    /// The complete `bus_reactive_injection` column, in `bus_order`.
    #[must_use]
    pub fn bus_reactive_injections(&self) -> &[f64] {
        &self.bus_reactive_injection
    }
    /// The complete `branch_from_active_flow` column, in `branch_order`.
    #[must_use]
    pub fn branch_from_active_flows(&self) -> &[f64] {
        &self.branch_from_active_flow
    }
    /// The complete `branch_from_reactive_flow` column, in `branch_order`.
    #[must_use]
    pub fn branch_from_reactive_flows(&self) -> &[f64] {
        &self.branch_from_reactive_flow
    }
    /// The complete `branch_to_active_flow` column, in `branch_order`.
    #[must_use]
    pub fn branch_to_active_flows(&self) -> &[f64] {
        &self.branch_to_active_flow
    }
    /// The complete `branch_to_reactive_flow` column, in `branch_order`.
    #[must_use]
    pub fn branch_to_reactive_flows(&self) -> &[f64] {
        &self.branch_to_reactive_flow
    }
    /// The complete `generator_active_power` column, in `generator_order`.
    #[must_use]
    pub fn generator_active_powers(&self) -> &[f64] {
        &self.generator_active_power
    }
    /// The complete `generator_reactive_power` column, in `generator_order`.
    #[must_use]
    pub fn generator_reactive_powers(&self) -> &[f64] {
        &self.generator_reactive_power
    }
}
