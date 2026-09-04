//! Solution values for the PowerModels SOCWR relaxation of AC optimal power
//! flow.

use std::collections::BTreeMap;
use std::sync::Arc;

use powerio_core::Error;
use powerio_tx::{BalancedNetwork, BusId};

use crate::ThreeWindingTransformerTerminalPower;
use crate::diagnostics::codes;
use crate::instance::AcOpfInstance;
use crate::operating::row_identity;
use crate::solution::{Producer, Residuals, Termination};

fn check_length(what: &str, actual: usize, expected: usize) -> Result<(), Error> {
    if actual == expected {
        Ok(())
    } else {
        Err(Error::new(
            &codes::BUILD_SOLUTION_SHAPE_MISMATCH,
            format!("{what} carries {actual} values; expected {expected}"),
        ))
    }
}

fn check_nonnegative(what: &str, values: &[f64]) -> Result<(), Error> {
    if let Some((position, value)) = values
        .iter()
        .copied()
        .enumerate()
        .find(|(_, value)| !value.is_finite() || *value < 0.0)
    {
        return Err(Error::new(
            &codes::BUILD_SOLUTION_MULTIPLIER_INVALID,
            format!(
                "{what} value {position} is {value}; multipliers must be finite and nonnegative"
            ),
        ));
    }
    Ok(())
}

/// The primal quantities reported by a SOCWR AC OPF solve.
///
/// Bus, branch, and generator vectors follow their source network table
/// order. Power quantities use MW, MVAr, or MVA. The three voltage product
/// quantities use per unit squared.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SocwrOpfValues {
    /// `w[i] = |V_i|²`, by bus.
    pub bus_voltage_magnitude_squared: Vec<f64>,
    /// `wr[e] = Re(V_from * conj(V_to))`, by oriented branch.
    pub branch_voltage_product_real: Vec<f64>,
    /// `wi[e] = Im(V_from * conj(V_to))`, by oriented branch.
    pub branch_voltage_product_imaginary: Vec<f64>,
    /// Active generator dispatch, MW.
    pub generator_active_power: Vec<f64>,
    /// Reactive generator dispatch, MVAr.
    pub generator_reactive_power: Vec<f64>,
    /// Active power into each branch at its from terminal, MW.
    pub branch_from_active_power: Vec<f64>,
    /// Reactive power into each branch at its from terminal, MVAr.
    pub branch_from_reactive_power: Vec<f64>,
    /// Active power into each branch at its to terminal, MW.
    pub branch_to_active_power: Vec<f64>,
    /// Reactive power into each branch at its to terminal, MVAr.
    pub branch_to_reactive_power: Vec<f64>,
    /// Terminal powers for each three winding transformer, in transformer
    /// table order.
    pub three_winding_transformer_terminal_powers: Vec<ThreeWindingTransformerTerminalPower>,
}

/// Optional dual quantities reported by a SOCWR AC OPF solve.
///
/// `None` means the producer did not report that column. Present bus columns
/// follow bus table order; present branch columns follow branch table order.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SocwrOpfDuals {
    /// Objective derivative per added MW of active demand.
    pub bus_active_power_marginal: Option<Vec<f64>>,
    /// Objective derivative per added MVAr of reactive demand.
    pub bus_reactive_power_marginal: Option<Vec<f64>>,
    /// Multiplier on the from terminal apparent power limit.
    pub branch_from_thermal_limit_multiplier: Option<Vec<f64>>,
    /// Multiplier on the to terminal apparent power limit.
    pub branch_to_thermal_limit_multiplier: Option<Vec<f64>>,
}

#[derive(Clone, Debug)]
struct SolutionIndex {
    buses: BTreeMap<BusId, usize>,
    branches: BTreeMap<String, usize>,
    generators: BTreeMap<String, usize>,
}

impl SolutionIndex {
    fn new(network: &BalancedNetwork) -> Result<Self, Error> {
        let mut buses = BTreeMap::new();
        let mut branches = BTreeMap::new();
        let mut generators = BTreeMap::new();
        for (position, bus) in network.buses().iter().enumerate() {
            if buses.insert(bus.id, position).is_some() {
                return Err(duplicate_identity("bus", &bus.id.to_string()));
            }
        }
        for (position, branch) in network.branches().iter().enumerate() {
            let id = row_identity(branch.uid.as_deref(), "branches", position);
            if branches.insert(id.clone(), position).is_some() {
                return Err(duplicate_identity("branch", &id));
            }
        }
        for (position, generator) in network.generators().iter().enumerate() {
            let id = row_identity(generator.uid.as_deref(), "generators", position);
            if generators.insert(id.clone(), position).is_some() {
                return Err(duplicate_identity("generator", &id));
            }
        }
        Ok(Self {
            buses,
            branches,
            generators,
        })
    }
}

fn duplicate_identity(component: &str, id: &str) -> Error {
    Error::new(
        &codes::BUILD_OPERATING_POINT_IDENTITY_UNKNOWN,
        format!("duplicate {component} identity `{id}`"),
    )
}

/// A solution of the PowerModels SOCWR relaxation of AC optimal power flow.
///
/// This is a relaxation result and its objective is a lower bound. It is not
/// an [`crate::AcOpfSolution`] and makes no claim that its voltage products
/// recover an AC feasible voltage phasor.
#[derive(Clone, Debug)]
pub struct SocwrOpfSolution {
    instance: Arc<AcOpfInstance>,
    termination: Termination,
    residuals: Residuals,
    producer: Producer,
    values: SocwrOpfValues,
    duals: SocwrOpfDuals,
    objective_lower_bound: f64,
    index: SolutionIndex,
}

impl SocwrOpfSolution {
    pub const FORMULATION: &'static str = "socwr";

    /// Construct a SOCWR solution from table ordered primal values and its
    /// objective lower bound.
    ///
    /// # Errors
    /// Any value column has a length different from its network table.
    pub fn new(
        instance: Arc<AcOpfInstance>,
        termination: Termination,
        values: SocwrOpfValues,
        objective_lower_bound: f64,
    ) -> Result<Self, Error> {
        let network = instance.network();
        let buses = network.buses().len();
        let branches = network.branches().len();
        let generators = network.generators().len();
        check_length(
            "bus voltage magnitude squared",
            values.bus_voltage_magnitude_squared.len(),
            buses,
        )?;
        check_length(
            "branch voltage product real part",
            values.branch_voltage_product_real.len(),
            branches,
        )?;
        check_length(
            "branch voltage product imaginary part",
            values.branch_voltage_product_imaginary.len(),
            branches,
        )?;
        check_length(
            "generator active power",
            values.generator_active_power.len(),
            generators,
        )?;
        check_length(
            "generator reactive power",
            values.generator_reactive_power.len(),
            generators,
        )?;
        check_length(
            "branch from terminal active power",
            values.branch_from_active_power.len(),
            branches,
        )?;
        check_length(
            "branch from terminal reactive power",
            values.branch_from_reactive_power.len(),
            branches,
        )?;
        check_length(
            "branch to terminal active power",
            values.branch_to_active_power.len(),
            branches,
        )?;
        check_length(
            "branch to terminal reactive power",
            values.branch_to_reactive_power.len(),
            branches,
        )?;
        check_length(
            "three winding transformer terminal powers",
            values.three_winding_transformer_terminal_powers.len(),
            network.transformers_3w().len(),
        )?;
        let index = SolutionIndex::new(network)?;
        Ok(Self {
            instance,
            termination,
            residuals: Residuals::default(),
            producer: None,
            values,
            duals: SocwrOpfDuals::default(),
            objective_lower_bound,
            index,
        })
    }

    #[must_use]
    pub const fn formulation(&self) -> &'static str {
        Self::FORMULATION
    }

    #[must_use]
    pub fn instance(&self) -> &AcOpfInstance {
        &self.instance
    }

    #[must_use]
    pub fn shared_instance(&self) -> Arc<AcOpfInstance> {
        Arc::clone(&self.instance)
    }

    #[must_use]
    pub fn network(&self) -> &BalancedNetwork {
        self.instance.network()
    }

    #[must_use]
    pub const fn termination(&self) -> &Termination {
        &self.termination
    }

    #[must_use]
    pub const fn residuals(&self) -> &Residuals {
        &self.residuals
    }

    #[must_use]
    pub fn producer(&self) -> Option<&str> {
        self.producer.as_deref()
    }

    #[must_use]
    pub const fn values(&self) -> &SocwrOpfValues {
        &self.values
    }

    #[must_use]
    pub const fn duals(&self) -> &SocwrOpfDuals {
        &self.duals
    }

    #[must_use]
    pub const fn objective_lower_bound(&self) -> f64 {
        self.objective_lower_bound
    }

    #[must_use]
    pub fn with_producer(mut self, producer: impl Into<String>) -> Self {
        self.producer = Some(producer.into());
        self
    }

    #[must_use]
    pub const fn with_residuals(mut self, residuals: Residuals) -> Self {
        self.residuals = residuals;
        self
    }

    /// Attach optional dual columns.
    ///
    /// # Errors
    /// A present column has the wrong length, or a thermal limit multiplier
    /// is negative or nonfinite.
    pub fn with_duals(mut self, duals: SocwrOpfDuals) -> Result<Self, Error> {
        let buses = self.network().buses().len();
        let branches = self.network().branches().len();
        if let Some(values) = &duals.bus_active_power_marginal {
            check_length("bus active power marginal", values.len(), buses)?;
        }
        if let Some(values) = &duals.bus_reactive_power_marginal {
            check_length("bus reactive power marginal", values.len(), buses)?;
        }
        if let Some(values) = &duals.branch_from_thermal_limit_multiplier {
            check_length(
                "branch from terminal thermal limit multiplier",
                values.len(),
                branches,
            )?;
            check_nonnegative("branch from terminal thermal limit multiplier", values)?;
        }
        if let Some(values) = &duals.branch_to_thermal_limit_multiplier {
            check_length(
                "branch to terminal thermal limit multiplier",
                values.len(),
                branches,
            )?;
            check_nonnegative("branch to terminal thermal limit multiplier", values)?;
        }
        self.duals = duals;
        Ok(self)
    }

    pub fn bus_order(&self) -> impl ExactSizeIterator<Item = BusId> + '_ {
        self.network().buses().iter().map(|bus| bus.id)
    }

    pub fn branch_order(&self) -> impl ExactSizeIterator<Item = String> + '_ {
        self.network()
            .branches()
            .iter()
            .enumerate()
            .map(|(position, branch)| row_identity(branch.uid.as_deref(), "branches", position))
    }

    pub fn generator_order(&self) -> impl ExactSizeIterator<Item = String> + '_ {
        self.network()
            .generators()
            .iter()
            .enumerate()
            .map(|(position, generator)| {
                row_identity(generator.uid.as_deref(), "generators", position)
            })
    }

    #[must_use]
    pub fn bus_voltage_magnitude_squared(&self, bus: BusId) -> Option<f64> {
        Some(self.values.bus_voltage_magnitude_squared[*self.index.buses.get(&bus)?])
    }

    #[must_use]
    pub fn branch_voltage_product_real(&self, branch: &str) -> Option<f64> {
        Some(self.values.branch_voltage_product_real[*self.index.branches.get(branch)?])
    }

    #[must_use]
    pub fn branch_voltage_product_imaginary(&self, branch: &str) -> Option<f64> {
        Some(self.values.branch_voltage_product_imaginary[*self.index.branches.get(branch)?])
    }

    #[must_use]
    pub fn generator_active_power(&self, generator: &str) -> Option<f64> {
        Some(self.values.generator_active_power[*self.index.generators.get(generator)?])
    }

    #[must_use]
    pub fn generator_reactive_power(&self, generator: &str) -> Option<f64> {
        Some(self.values.generator_reactive_power[*self.index.generators.get(generator)?])
    }

    #[must_use]
    pub fn branch_from_active_power(&self, branch: &str) -> Option<f64> {
        Some(self.values.branch_from_active_power[*self.index.branches.get(branch)?])
    }

    #[must_use]
    pub fn branch_from_reactive_power(&self, branch: &str) -> Option<f64> {
        Some(self.values.branch_from_reactive_power[*self.index.branches.get(branch)?])
    }

    #[must_use]
    pub fn branch_to_active_power(&self, branch: &str) -> Option<f64> {
        Some(self.values.branch_to_active_power[*self.index.branches.get(branch)?])
    }

    #[must_use]
    pub fn branch_to_reactive_power(&self, branch: &str) -> Option<f64> {
        Some(self.values.branch_to_reactive_power[*self.index.branches.get(branch)?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerio_core::Source;

    fn instance() -> Arc<AcOpfInstance> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
        let network = powerio_tx::parse(Source::open(path).unwrap())
            .unwrap()
            .into_value();
        Arc::new(AcOpfInstance::from_network(network).unwrap())
    }

    fn values(instance: &AcOpfInstance) -> SocwrOpfValues {
        let buses = instance.network().buses().len();
        let branches = instance.network().branches().len();
        let generators = instance.network().generators().len();
        SocwrOpfValues {
            bus_voltage_magnitude_squared: vec![1.0; buses],
            branch_voltage_product_real: vec![0.99; branches],
            branch_voltage_product_imaginary: vec![0.01; branches],
            generator_active_power: vec![10.0; generators],
            generator_reactive_power: vec![2.0; generators],
            branch_from_active_power: vec![3.0; branches],
            branch_from_reactive_power: vec![0.5; branches],
            branch_to_active_power: vec![-2.9; branches],
            branch_to_reactive_power: vec![-0.4; branches],
            three_winding_transformer_terminal_powers: Vec::new(),
        }
    }

    #[test]
    fn result_is_explicitly_a_relaxation_lower_bound() {
        let instance = instance();
        let solution = SocwrOpfSolution::new(
            Arc::clone(&instance),
            Termination::Converged,
            values(&instance),
            5_000.0,
        )
        .unwrap()
        .with_producer("test-solver");
        assert_eq!(solution.formulation(), "socwr");
        assert!((solution.objective_lower_bound() - 5_000.0).abs() < f64::EPSILON);
        assert_eq!(solution.producer(), Some("test-solver"));
        assert!(
            (solution.bus_voltage_magnitude_squared(BusId(1)).unwrap() - 1.0).abs() < f64::EPSILON
        );
        let branch = solution.branch_order().next().unwrap();
        assert!(
            (solution.branch_voltage_product_real(&branch).unwrap() - 0.99).abs() < f64::EPSILON
        );
        assert!(std::ptr::eq(solution.instance(), instance.as_ref()));
    }

    #[test]
    fn dimensions_and_multiplier_signs_are_checked() {
        let instance = instance();
        let mut wrong = values(&instance);
        wrong.bus_voltage_magnitude_squared.push(1.0);
        assert!(
            SocwrOpfSolution::new(Arc::clone(&instance), Termination::Converged, wrong, 0.0,)
                .is_err()
        );

        let solution = SocwrOpfSolution::new(
            Arc::clone(&instance),
            Termination::Converged,
            values(&instance),
            0.0,
        )
        .unwrap();
        let branches = instance.network().branches().len();
        assert!(
            solution
                .with_duals(SocwrOpfDuals {
                    branch_from_thermal_limit_multiplier: Some(vec![-1.0; branches]),
                    ..SocwrOpfDuals::default()
                })
                .is_err()
        );
    }
}
