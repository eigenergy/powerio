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

fn bus_position(network: &BalancedNetwork, bus: BusId) -> Option<usize> {
    network.buses().iter().position(|row| row.id == bus)
}

fn branch_position(network: &BalancedNetwork, identity: &str) -> Option<usize> {
    network
        .branches()
        .iter()
        .enumerate()
        .position(|(row, branch)| row_identity(branch.uid.as_deref(), "branches", row) == identity)
}

fn generator_position(network: &BalancedNetwork, identity: &str) -> Option<usize> {
    network
        .generators()
        .iter()
        .enumerate()
        .position(|(row, generator)| {
            row_identity(generator.uid.as_deref(), "generators", row) == identity
        })
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
        })
    }

    shared_solution_accessors!(DcPfInstance);
    optional_dispatch_accessors!();

    /// Voltage angle at one bus, degrees.
    #[must_use]
    pub fn bus_voltage_angle(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_angle[bus_position(self.network(), bus)?])
    }

    /// Net active injection at one bus, MW.
    #[must_use]
    pub fn bus_active_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_injection[bus_position(self.network(), bus)?])
    }

    /// Active flow into the branch at its from terminal, MW, by stable
    /// branch identity.
    #[must_use]
    pub fn branch_from_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_active_flow[branch_position(self.network(), identity)?])
    }

    /// Active flow into the branch at its to terminal, MW.
    #[must_use]
    pub fn branch_to_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_active_flow[branch_position(self.network(), identity)?])
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
        })
    }

    shared_solution_accessors!(AcPfInstance);
    optional_dispatch_accessors!();

    /// Voltage magnitude at one bus, per unit.
    #[must_use]
    pub fn bus_voltage_magnitude(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_magnitude[bus_position(self.network(), bus)?])
    }

    /// Voltage angle at one bus, degrees.
    #[must_use]
    pub fn bus_voltage_angle(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_angle[bus_position(self.network(), bus)?])
    }

    /// Net active injection at one bus, MW.
    #[must_use]
    pub fn bus_active_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_injection[bus_position(self.network(), bus)?])
    }

    /// Net reactive injection at one bus, MVAr.
    #[must_use]
    pub fn bus_reactive_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_reactive_injection[bus_position(self.network(), bus)?])
    }

    /// Active flow into the branch at its from terminal, MW, by stable
    /// branch identity.
    #[must_use]
    pub fn branch_from_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_active_flow[branch_position(self.network(), identity)?])
    }

    /// Reactive flow into the branch at its from terminal, MVAr.
    #[must_use]
    pub fn branch_from_reactive_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_reactive_flow[branch_position(self.network(), identity)?])
    }

    /// Active flow into the branch at its to terminal, MW.
    #[must_use]
    pub fn branch_to_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_active_flow[branch_position(self.network(), identity)?])
    }

    /// Reactive flow into the branch at its to terminal, MVAr.
    #[must_use]
    pub fn branch_to_reactive_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_reactive_flow[branch_position(self.network(), identity)?])
    }
}

/// The DC optimal power flow solution: the DC power flow results plus the
/// optimized generator active dispatch and the objective value.
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
        })
    }

    shared_solution_accessors!(DcOpfInstance);

    /// The optimized objective value.
    #[must_use]
    pub const fn objective(&self) -> f64 {
        self.objective
    }

    /// Optimized active power of one generator, MW, by stable identity.
    #[must_use]
    pub fn generator_active_power(&self, identity: &str) -> Option<f64> {
        Some(self.generator_active_power[generator_position(self.network(), identity)?])
    }

    /// Voltage angle at one bus, degrees.
    #[must_use]
    pub fn bus_voltage_angle(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_angle[bus_position(self.network(), bus)?])
    }

    /// Net active injection at one bus, MW.
    #[must_use]
    pub fn bus_active_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_injection[bus_position(self.network(), bus)?])
    }

    /// Active flow into the branch at its from terminal, MW.
    #[must_use]
    pub fn branch_from_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_active_flow[branch_position(self.network(), identity)?])
    }

    /// Active flow into the branch at its to terminal, MW.
    #[must_use]
    pub fn branch_to_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_active_flow[branch_position(self.network(), identity)?])
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
        })
    }

    shared_solution_accessors!(AcOpfInstance);

    /// The optimized objective value.
    #[must_use]
    pub const fn objective(&self) -> f64 {
        self.objective
    }

    /// Optimized active power of one generator, MW, by stable identity.
    #[must_use]
    pub fn generator_active_power(&self, identity: &str) -> Option<f64> {
        Some(self.generator_active_power[generator_position(self.network(), identity)?])
    }

    /// Optimized reactive power of one generator, MVAr.
    #[must_use]
    pub fn generator_reactive_power(&self, identity: &str) -> Option<f64> {
        Some(self.generator_reactive_power[generator_position(self.network(), identity)?])
    }

    /// Voltage magnitude at one bus, per unit.
    #[must_use]
    pub fn bus_voltage_magnitude(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_magnitude[bus_position(self.network(), bus)?])
    }

    /// Voltage angle at one bus, degrees.
    #[must_use]
    pub fn bus_voltage_angle(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_voltage_angle[bus_position(self.network(), bus)?])
    }

    /// Net active injection at one bus, MW.
    #[must_use]
    pub fn bus_active_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_active_injection[bus_position(self.network(), bus)?])
    }

    /// Net reactive injection at one bus, MVAr.
    #[must_use]
    pub fn bus_reactive_injection(&self, bus: BusId) -> Option<f64> {
        Some(self.bus_reactive_injection[bus_position(self.network(), bus)?])
    }

    /// Active flow into the branch at its from terminal, MW.
    #[must_use]
    pub fn branch_from_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_active_flow[branch_position(self.network(), identity)?])
    }

    /// Reactive flow into the branch at its from terminal, MVAr.
    #[must_use]
    pub fn branch_from_reactive_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_from_reactive_flow[branch_position(self.network(), identity)?])
    }

    /// Active flow into the branch at its to terminal, MW.
    #[must_use]
    pub fn branch_to_active_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_active_flow[branch_position(self.network(), identity)?])
    }

    /// Reactive flow into the branch at its to terminal, MVAr.
    #[must_use]
    pub fn branch_to_reactive_flow(&self, identity: &str) -> Option<f64> {
        Some(self.branch_to_reactive_flow[branch_position(self.network(), identity)?])
    }
}
