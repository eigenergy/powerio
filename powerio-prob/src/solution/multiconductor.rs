//! The multiconductor solutions: `McAcPfSolution` and `McAcOpfSolution`.
//!
//! Terminal values are stored in the shared network's bus table order, each
//! bus's terminals in its stated order — the layout the operating point builders use —
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

/// The number of generator coils of a network: per generator, the coils of
/// its connection under `powerio_dist::calc_element_connection`, the rule
/// that orders a generator's per coil bounds and costs.
fn generator_coil_count(network: &MulticonductorNetwork) -> Result<usize, Error> {
    let mut coils = 0;
    for generator in network.generators() {
        coils += powerio_dist::calc_element_connection(
            network,
            &generator.bus,
            &generator.terminal_map,
            generator.configuration,
            None,
        )
        .map_err(|reason| {
            Error::new(
                &codes::BUILD_SOLUTION_SHAPE_MISMATCH,
                format!(
                    "generator `{}` does not resolve to coils: {reason}",
                    generator.name
                ),
            )
        })?
        .len();
    }
    Ok(coils)
}

/// Bus-and-terminal to flat column position, built once per solution so
/// repeated reads never rescan the bus table: powerio-dist's one flat
/// terminal index.
#[derive(Clone, Debug, Default)]
struct TerminalIndex(powerio_dist::MulticonductorTerminalIndex);

impl TerminalIndex {
    fn build(network: &MulticonductorNetwork) -> Result<Self, Error> {
        powerio_dist::MulticonductorTerminalIndex::build(network)
            .map(Self)
            .map_err(|error| {
                Error::new(
                    &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
                    error.to_string(),
                )
            })
    }
}

fn terminal_position(index: &TerminalIndex, bus: &str, terminal: &str) -> Option<usize> {
    index.0.resolve(bus, terminal)
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

        fn terminal_index(&self) -> &TerminalIndex {
            &self.index
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
            Some(
                self.terminal_voltage_magnitude
                    [terminal_position(self.terminal_index(), bus, terminal)?],
            )
        }

        /// Voltage angle at one bus terminal, radians.
        #[must_use]
        pub fn terminal_voltage_angle(&self, bus: &str, terminal: &str) -> Option<f64> {
            Some(
                self.terminal_voltage_angle
                    [terminal_position(self.terminal_index(), bus, terminal)?],
            )
        }

        /// Current into the network at one bus terminal, amperes, when the
        /// producer reported currents.
        #[must_use]
        pub fn terminal_current_magnitude(&self, bus: &str, terminal: &str) -> Option<f64> {
            let values = self.terminal_current_magnitude.as_ref()?;
            Some(values[terminal_position(self.terminal_index(), bus, terminal)?])
        }

        /// Active power into the network at one bus terminal, watts, when
        /// the producer reported terminal powers.
        #[must_use]
        pub fn terminal_active_power(&self, bus: &str, terminal: &str) -> Option<f64> {
            let values = self.terminal_active_power.as_ref()?;
            Some(values[terminal_position(self.terminal_index(), bus, terminal)?])
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
    index: TerminalIndex,
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
        let index = TerminalIndex::build(instance.network())?;
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
            index,
        })
    }

    shared_mc_solution_accessors!(McAcPfInstance);
}

/// The multiconductor AC optimal power flow solution: the power flow results
/// plus generator dispatch and the objective value.
///
/// Generator dispatch has two columns. The required one,
/// [`Self::generator_active_powers`], is per terminal of each generator's
/// terminal map, neutral included. The optional one,
/// [`Self::generator_coil_active_powers`], is per coil, the layout of a
/// generator's cost and bounds.
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
    /// Per generator, per terminal active power, generator table order with
    /// each generator's terminal map order (neutral included), watts.
    generator_active_power: Vec<f64>,
    /// Per generator, per coil active dispatch, generator table order with
    /// each generator's coil order, watts, when the producer reported it.
    generator_coil_active_power: Option<Vec<f64>>,
    objective: f64,
    index: TerminalIndex,
}

impl McAcOpfSolution {
    /// Assemble the results: the power flow columns plus the optimized
    /// generator active power per terminal (watts, generator table order
    /// with each generator's terminal map order, one entry per mapped
    /// terminal, neutral included) and the objective value.
    ///
    /// The per terminal column is not the layout of generator cost and
    /// bounds; record the per coil dispatch with
    /// [`Self::with_generator_coil_active_powers`].
    ///
    /// # Errors
    /// A column whose length disagrees with the instance's resolved
    /// terminals or generator terminal maps.
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
            "per terminal generator active powers",
            generator_active_power.len(),
            generator_conductors,
        )?;
        let index = TerminalIndex::build(instance.network())?;
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
            generator_coil_active_power: None,
            objective,
            index,
        })
    }

    shared_mc_solution_accessors!(McAcOpfInstance);

    /// The optimized objective value.
    #[must_use]
    pub const fn objective(&self) -> f64 {
        self.objective
    }

    /// The optimized generator active power per terminal, watts, generator
    /// table order with each generator's terminal map order: one entry per
    /// mapped terminal, neutral included.
    ///
    /// A generator's cost and bounds are per coil, not per terminal. For a
    /// DELTA generator, or a WYE generator whose neutral voltage is not
    /// zero, this column differs from the per coil dispatch; read
    /// [`Self::generator_coil_active_powers`] for that.
    #[must_use]
    pub fn generator_active_powers(&self) -> &[f64] {
        &self.generator_active_power
    }

    /// The optimized generator active dispatch per coil, watts, positive
    /// for generation, when the producer reported it.
    ///
    /// Generators are in generator table order. Within a generator the
    /// coils are in the order of [`powerio_dist::calc_element_connection`]
    /// over its bus, terminal map and configuration with no stated channel
    /// count, which is the order of its `p_min`, `p_max`, `q_min`, `q_max`,
    /// `s_max` and `cost` entries:
    ///
    /// - WYE: one coil per phase terminal, in terminal map order, from that
    ///   terminal to the neutral (to ground when the map states no
    ///   neutral). The neutral has no entry.
    /// - DELTA with three terminals: terminal 1 to 2, 2 to 3, 3 to 1 of the
    ///   terminal map (ab, bc, ca for a map a, b, c).
    /// - DELTA or SINGLE_PHASE with two terminals: one coil from the first
    ///   terminal to the second.
    ///
    /// Each entry is the power that coil delivers to the network, so cost
    /// times this column is the generator part of an active power dispatch
    /// cost.
    #[must_use]
    pub fn generator_coil_active_powers(&self) -> Option<&[f64]> {
        self.generator_coil_active_power.as_deref()
    }

    /// Record the per coil generator active dispatch, watts, positive for
    /// generation, in the order [`Self::generator_coil_active_powers`]
    /// states.
    ///
    /// # Errors
    /// A generator whose connection does not resolve to coils, or a column
    /// whose length disagrees with the generators' coils.
    pub fn with_generator_coil_active_powers(mut self, values: Vec<f64>) -> Result<Self, Error> {
        let coils = generator_coil_count(self.network())?;
        if values.len() != coils {
            return Err(Error::new(
                &codes::BUILD_SOLUTION_SHAPE_MISMATCH,
                format!(
                    "per coil generator dispatch carries {} values; the instance's generators have {coils} coils",
                    values.len()
                ),
            ));
        }
        self.generator_coil_active_power = Some(values);
        Ok(self)
    }
}
