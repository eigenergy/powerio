//! Typed objective terms for the optimal power flow instances.
//!
//! A term is a typed reference to costs or penalties stored on the network or
//! the calculation record; the numerical curves themselves stay on the
//! network so power flow and other calculations reuse them. A solver never
//! adds a term silently: changing the mathematical objective constructs a
//! different instance, and a derived instance or a stored document can state
//! every term and its weight exactly.

use serde::{Deserialize, Serialize};

/// One typed objective term.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "term")]
#[non_exhaustive]
pub enum ObjectiveTerm {
    /// The generator cost curves the network states, summed over the in
    /// service generators the instance dispatches.
    NetworkGeneratorCost,
    /// The per phase cost references a multiconductor calculation record
    /// states (the BMOPF objective).
    NetworkPerPhaseCost,
    /// A differentiability regularization with its stated nonnegative weight,
    /// the term Tellegen adds explicitly rather than a solver adding it
    /// silently.
    DifferentiabilityRegularization { weight: f64 },
}

/// The complete typed objective of one OPF instance: a sum of terms.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Objective {
    terms: Vec<ObjectiveTerm>,
}

impl Objective {
    /// The empty objective; a feasibility problem.
    #[must_use]
    pub const fn none() -> Self {
        Self { terms: Vec::new() }
    }

    /// The default OPF objective: the network's generator cost curves.
    #[must_use]
    pub fn network_generator_cost() -> Self {
        Self {
            terms: vec![ObjectiveTerm::NetworkGeneratorCost],
        }
    }

    /// The default multiconductor OPF objective: the per phase cost
    /// references the calculation record states.
    #[must_use]
    pub fn network_per_phase_cost() -> Self {
        Self {
            terms: vec![ObjectiveTerm::NetworkPerPhaseCost],
        }
    }

    /// Append one term, consuming the objective.
    #[must_use]
    pub fn with_term(mut self, term: ObjectiveTerm) -> Self {
        self.terms.push(term);
        self
    }

    /// The terms, in the order they were stated.
    #[must_use]
    pub fn terms(&self) -> &[ObjectiveTerm] {
        &self.terms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_objective_states_its_terms_in_order() {
        let objective = Objective::network_generator_cost()
            .with_term(ObjectiveTerm::DifferentiabilityRegularization { weight: 1e-6 });
        assert_eq!(objective.terms().len(), 2);
        assert_eq!(objective.terms()[0], ObjectiveTerm::NetworkGeneratorCost);
        let wire = serde_json::to_value(&objective).unwrap();
        assert_eq!(wire["terms"][1]["term"], "differentiability_regularization");
        assert_eq!(wire["terms"][1]["weight"], 1e-6);
    }
}
