//! The multiconductor solutions: `McAcPfSolution` and `McAcOpfSolution`.
//!
//! Terminal values are stored in the shared network's bus table order, each
//! bus's terminals in its stated order — the layout the state builders use —
//! and read back by `(bus, terminal)`. Source injections are per source
//! terminal in source table order.

use std::sync::Arc;

use powerio_core::Error;
use powerio_dist::MulticonductorNetwork;

use crate::diagnostics::codes;
use crate::instance::{McAcOpfInstance, McAcPfInstance};
use crate::solution::{Producer, Residuals, Termination};

fn check_length(what: &'static str, got: usize, expected: usize) -> Result<(), Error> {
    if got == expected {
        Ok(())
    } else {
        Err(Error::new(
            &codes::BUILD_SOLUTION_SHAPE_MISMATCH,
            format!("{what} carries {got} values; the instance resolves {expected} terminals"),
        ))
    }
}

fn terminal_count(network: &MulticonductorNetwork) -> usize {
    network.buses().iter().map(|bus| bus.terminals.len()).sum()
}

fn source_terminal_count(network: &MulticonductorNetwork) -> usize {
    network
        .sources()
        .iter()
        .map(|source| source.terminal_map.len())
        .sum()
}

fn terminal_position(network: &MulticonductorNetwork, bus: &str, terminal: &str) -> Option<usize> {
    let mut offset = 0usize;
    for row in network.buses() {
        if row.id == bus {
            return row
                .terminals
                .iter()
                .position(|candidate| candidate == terminal)
                .map(|position| offset + position);
        }
        offset += row.terminals.len();
    }
    None
}

macro_rules! shared_mc_solution_accessors {
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
        pub fn network(&self) -> &MulticonductorNetwork {
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

        /// Voltage magnitude at one bus terminal, volts.
        #[must_use]
        pub fn terminal_voltage_magnitude(&self, bus: &str, terminal: &str) -> Option<f64> {
            Some(self.terminal_voltage_magnitude[terminal_position(self.network(), bus, terminal)?])
        }

        /// Voltage angle at one bus terminal, radians.
        #[must_use]
        pub fn terminal_voltage_angle(&self, bus: &str, terminal: &str) -> Option<f64> {
            Some(self.terminal_voltage_angle[terminal_position(self.network(), bus, terminal)?])
        }

        /// Current into the network at one bus terminal, amperes, when the
        /// producer reported currents.
        #[must_use]
        pub fn terminal_current_magnitude(&self, bus: &str, terminal: &str) -> Option<f64> {
            let values = self.terminal_current_magnitude.as_ref()?;
            Some(values[terminal_position(self.network(), bus, terminal)?])
        }

        /// Active power into the network at one bus terminal, watts, when
        /// the producer reported terminal powers.
        #[must_use]
        pub fn terminal_active_power(&self, bus: &str, terminal: &str) -> Option<f64> {
            let values = self.terminal_active_power.as_ref()?;
            Some(values[terminal_position(self.network(), bus, terminal)?])
        }

        /// Record per terminal current magnitudes, amperes.
        ///
        /// # Errors
        /// A column whose length disagrees with the resolved terminals.
        pub fn with_terminal_currents(mut self, values: Vec<f64>) -> Result<Self, Error> {
            check_length(
                "terminal current magnitudes",
                values.len(),
                terminal_count(self.network()),
            )?;
            self.terminal_current_magnitude = Some(values);
            Ok(self)
        }

        /// Record per terminal active powers, watts.
        ///
        /// # Errors
        /// A column whose length disagrees with the resolved terminals.
        pub fn with_terminal_powers(mut self, values: Vec<f64>) -> Result<Self, Error> {
            check_length(
                "terminal active powers",
                values.len(),
                terminal_count(self.network()),
            )?;
            self.terminal_active_power = Some(values);
            Ok(self)
        }

        /// Per source terminal active injections, watts, in source table
        /// order with each source's terminal map order.
        #[must_use]
        pub fn source_active_injections(&self) -> &[f64] {
            &self.source_active_injection
        }
    };
}

/// The multiconductor AC power flow solution: terminal complex voltages,
/// optional terminal currents and powers, and source injections over the
/// shared instance.
#[derive(Clone, Debug)]
pub struct McAcPfSolution {
    instance: Arc<McAcPfInstance>,
    termination: Termination,
    residuals: Residuals,
    producer: Producer,
    terminal_voltage_magnitude: Vec<f64>,
    terminal_voltage_angle: Vec<f64>,
    terminal_current_magnitude: Option<Vec<f64>>,
    terminal_active_power: Option<Vec<f64>>,
    source_active_injection: Vec<f64>,
}

impl McAcPfSolution {
    /// Assemble the required results: per terminal voltage magnitudes
    /// (volts) and angles (radians) in bus table and stated terminal order,
    /// and per source terminal active injections (watts) in source table
    /// order.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's resolved
    /// terminals.
    pub fn new(
        instance: Arc<McAcPfInstance>,
        termination: Termination,
        terminal_voltage_magnitude: Vec<f64>,
        terminal_voltage_angle: Vec<f64>,
        source_active_injection: Vec<f64>,
    ) -> Result<Self, Error> {
        let terminals = terminal_count(instance.network());
        check_length(
            "terminal voltage magnitudes",
            terminal_voltage_magnitude.len(),
            terminals,
        )?;
        check_length(
            "terminal voltage angles",
            terminal_voltage_angle.len(),
            terminals,
        )?;
        check_length(
            "source active injections",
            source_active_injection.len(),
            source_terminal_count(instance.network()),
        )?;
        Ok(Self {
            instance,
            termination,
            residuals: Residuals::default(),
            producer: None,
            terminal_voltage_magnitude,
            terminal_voltage_angle,
            terminal_current_magnitude: None,
            terminal_active_power: None,
            source_active_injection,
        })
    }

    shared_mc_solution_accessors!(McAcPfInstance);
}

/// The multiconductor AC optimal power flow solution: the power flow results
/// plus per phase generator dispatch and the objective value.
#[derive(Clone, Debug)]
pub struct McAcOpfSolution {
    instance: Arc<McAcOpfInstance>,
    termination: Termination,
    residuals: Residuals,
    producer: Producer,
    terminal_voltage_magnitude: Vec<f64>,
    terminal_voltage_angle: Vec<f64>,
    terminal_current_magnitude: Option<Vec<f64>>,
    terminal_active_power: Option<Vec<f64>>,
    source_active_injection: Vec<f64>,
    /// Per generator, per phase active dispatch, generator table order with
    /// each generator's terminal map order, watts.
    generator_active_power: Vec<f64>,
    objective: f64,
}

impl McAcOpfSolution {
    /// Assemble the results: the power flow columns plus the optimized per
    /// phase generator dispatch (watts, generator table order with each
    /// generator's terminal map order) and the objective value.
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's resolved
    /// terminals or generator conductors.
    pub fn new(
        instance: Arc<McAcOpfInstance>,
        termination: Termination,
        terminal_voltage_magnitude: Vec<f64>,
        terminal_voltage_angle: Vec<f64>,
        source_active_injection: Vec<f64>,
        generator_active_power: Vec<f64>,
        objective: f64,
    ) -> Result<Self, Error> {
        let terminals = terminal_count(instance.network());
        check_length(
            "terminal voltage magnitudes",
            terminal_voltage_magnitude.len(),
            terminals,
        )?;
        check_length(
            "terminal voltage angles",
            terminal_voltage_angle.len(),
            terminals,
        )?;
        check_length(
            "source active injections",
            source_active_injection.len(),
            source_terminal_count(instance.network()),
        )?;
        let generator_conductors: usize = instance
            .network()
            .generators()
            .iter()
            .map(|generator| generator.terminal_map.len())
            .sum();
        check_length(
            "per phase generator dispatch",
            generator_active_power.len(),
            generator_conductors,
        )?;
        Ok(Self {
            instance,
            termination,
            residuals: Residuals::default(),
            producer: None,
            terminal_voltage_magnitude,
            terminal_voltage_angle,
            terminal_current_magnitude: None,
            terminal_active_power: None,
            source_active_injection,
            generator_active_power,
            objective,
        })
    }

    shared_mc_solution_accessors!(McAcOpfInstance);

    /// The optimized objective value.
    #[must_use]
    pub const fn objective(&self) -> f64 {
        self.objective
    }

    /// The optimized per phase generator dispatch, watts, generator table
    /// order with each generator's terminal map order.
    #[must_use]
    pub fn generator_active_powers(&self) -> &[f64] {
        &self.generator_active_power
    }
}
