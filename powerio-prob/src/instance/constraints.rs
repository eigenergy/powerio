//! Active constraint selections for the optimal power flow instances.
//!
//! Physical limit values stay on the network so every calculation reuses
//! them; an OPF instance selects which of those limits are active
//! constraints, by stable element identity, without copying the numerical
//! bounds.

use serde::{Deserialize, Serialize};

/// Which elements of one constraint family are active, by stable element
/// identity (the payload `uid`, else `{table}:{row}`; buses by their id).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "select", content = "identities")]
#[non_exhaustive]
pub enum ConstraintSelection {
    /// Every element with a stated limit.
    #[default]
    All,
    /// No element; the family is relaxed.
    None,
    /// Exactly the named elements.
    Only(Vec<String>),
}

impl ConstraintSelection {
    /// Whether the named element's limit is an active constraint.
    #[must_use]
    pub fn selects(&self, identity: &str) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Only(identities) => identities.iter().any(|selected| selected == identity),
        }
    }
}

/// The active constraint families of a balanced OPF instance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ActiveConstraints {
    /// Generator capability bounds (active and reactive limits).
    pub generator_capability: ConstraintSelection,
    /// Bus voltage magnitude bounds.
    pub voltage_bounds: ConstraintSelection,
    /// Branch thermal limits.
    pub thermal_limits: ConstraintSelection,
    /// Branch angle difference bounds.
    pub angle_bounds: ConstraintSelection,
}

/// The active constraint families of a multiconductor OPF instance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct MulticonductorActiveConstraints {
    /// Terminal voltage magnitude bounds.
    pub terminal_voltage_bounds: ConstraintSelection,
    /// Conductor current or apparent power limits.
    pub conductor_limits: ConstraintSelection,
    /// Per phase generator capability bounds.
    pub generator_capability: ConstraintSelection,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_answer_by_identity() {
        assert!(ConstraintSelection::All.selects("branches:3"));
        assert!(!ConstraintSelection::None.selects("branches:3"));
        let only = ConstraintSelection::Only(vec!["branches:3".to_owned()]);
        assert!(only.selects("branches:3"));
        assert!(!only.selects("branches:4"));
    }
}
