use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::Error;
use crate::validation::valid_nonempty_text;

/// Absolute tolerance for a complete probability sum.
pub const SCENARIO_PROBABILITY_TOLERANCE: f64 = 1e-12;

/// Case-sensitive stable identity of one scenario.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScenarioId(Box<str>);

impl ScenarioId {
    pub fn new(id: impl Into<String>) -> Result<Self, Error> {
        let id = id.into();
        if !valid_nonempty_text(&id) {
            return Err(Error::new(
                &crate::codes::VALIDATE_SCENARIO_INVALID_ID,
                "a scenario ID must be nonempty and bounded",
            ));
        }
        Ok(Self(id.into_boxed_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ScenarioId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One named scenario and its optional probability.
#[derive(Clone, Debug)]
pub struct Scenario<T> {
    id: ScenarioId,
    probability: Option<f64>,
    value: T,
}

impl<T> Scenario<T> {
    #[must_use]
    pub const fn new(id: ScenarioId, probability: Option<f64>, value: T) -> Self {
        Self {
            id,
            probability,
            value,
        }
    }

    #[must_use]
    pub const fn id(&self) -> &ScenarioId {
        &self.id
    }

    #[must_use]
    pub const fn probability(&self) -> Option<f64> {
        self.probability
    }

    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
    }

    #[must_use]
    pub fn into_value(self) -> T {
        self.value
    }
}

/// Named alternatives with no implied time order.
pub struct ScenarioSet<T> {
    scenarios: Arc<[Scenario<T>]>,
    /// ID to position. Built once, so `get` does not scan the set.
    index: Arc<HashMap<Box<str>, usize>>,
}

impl<T> ScenarioSet<T> {
    pub fn new(scenarios: Vec<Scenario<T>>) -> Result<Self, Error> {
        let mut ids: HashMap<Box<str>, usize> = HashMap::new();
        ids.try_reserve(scenarios.len()).map_err(|cause| {
            Error::new(
                &crate::codes::VALIDATE_SCENARIO_ALLOCATION_REFUSED,
                format!(
                    "cannot reserve identity validation for {} scenarios",
                    scenarios.len()
                ),
            )
            .with_cause(cause)
        })?;
        for (position, scenario) in scenarios.iter().enumerate() {
            if ids.insert(scenario.id.as_str().into(), position).is_some() {
                return Err(Error::new(
                    &crate::codes::VALIDATE_SCENARIO_DUPLICATE_ID,
                    format!("duplicate scenario ID `{}`", scenario.id),
                ));
            }
        }

        let probability_count = scenarios
            .iter()
            .filter(|scenario| scenario.probability.is_some())
            .count();
        if probability_count != 0 && probability_count != scenarios.len() {
            if let Some(missing) = scenarios
                .iter()
                .find(|scenario| scenario.probability.is_none())
            {
                return Err(Error::new(
                    &crate::codes::VALIDATE_SCENARIO_MISSING_PROBABILITY,
                    format!("scenario `{}` has no probability", missing.id),
                ));
            }
        }

        if probability_count != 0 {
            for scenario in &scenarios {
                let Some(probability) = scenario.probability else {
                    return Err(Error::new(
                        &crate::codes::VALIDATE_SCENARIO_MISSING_PROBABILITY,
                        format!("scenario `{}` has no probability", scenario.id),
                    ));
                };
                if !probability.is_finite() || probability < 0.0 {
                    return Err(Error::new(
                        &crate::codes::VALIDATE_SCENARIO_INVALID_PROBABILITY,
                        format!(
                            "scenario `{}` probability must be finite and nonnegative; found {probability}",
                            scenario.id
                        ),
                    ));
                }
            }
            let sum = compensated_sum(scenarios.iter().filter_map(|scenario| scenario.probability));
            if !sum.is_finite() || (sum - 1.0).abs() > SCENARIO_PROBABILITY_TOLERANCE {
                return Err(Error::new(
                    &crate::codes::VALIDATE_SCENARIO_PROBABILITY_SUM,
                    format!("scenario probabilities must sum to one; found {sum}"),
                ));
            }
        }

        Ok(Self {
            scenarios: scenarios.into(),
            index: Arc::new(ids),
        })
    }

    #[must_use]
    /// Look one scenario up by its stable ID. Constant time: IDs are case
    /// sensitive and never normalized, and the index is built once.
    pub fn get(&self, id: &str) -> Option<&Scenario<T>> {
        self.scenarios.get(*self.index.get(id)?)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Scenario<T>> {
        self.scenarios.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.scenarios.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scenarios.is_empty()
    }
}

impl<T> Clone for ScenarioSet<T> {
    fn clone(&self) -> Self {
        Self {
            scenarios: Arc::clone(&self.scenarios),
            index: Arc::clone(&self.index),
        }
    }
}

// The ID index is a derived view of `scenarios`, so printing it would only
// repeat the IDs already shown.
#[expect(
    clippy::missing_fields_in_debug,
    reason = "the index is derived from the scenarios it points into"
)]
impl<T: fmt::Debug> fmt::Debug for ScenarioSet<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScenarioSet")
            .field("scenarios", &self.scenarios)
            .finish()
    }
}

fn compensated_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut sum = 0.0;
    let mut correction = 0.0;
    for value in values {
        let corrected = value - correction;
        let next = sum + corrected;
        correction = (next - sum) - corrected;
        sum = next;
    }
    sum
}

#[cfg(test)]
mod tests {

    #[test]
    fn lookup_is_indexed_and_stays_correct_at_scale() {
        // A linear scan is correct but not what the design allows, so this
        // exercises enough entries that a scan would be visible in a profile
        // and asserts the last one is reachable directly.
        let scenarios: Vec<_> = (0..20_000)
            .map(|index| Scenario::new(ScenarioId::new(format!("s{index}")).unwrap(), None, index))
            .collect();
        let set = ScenarioSet::new(scenarios).unwrap();
        assert_eq!(set.get("s19999").map(Scenario::value), Some(&19_999));
        assert_eq!(set.get("s0").map(Scenario::value), Some(&0));
        assert!(set.get("S0").is_none(), "IDs are case sensitive");
        assert!(set.get("missing").is_none());
        // Cloning shares the index rather than rebuilding it.
        assert_eq!(
            set.clone().get("s12345").map(Scenario::value),
            Some(&12_345)
        );
    }
    use super::*;

    fn scenario(id: &str, probability: Option<f64>) -> Scenario<u8> {
        Scenario::new(ScenarioId::new(id).unwrap(), probability, 1)
    }

    #[test]
    fn identities_are_exact_bounded_and_case_sensitive() {
        assert!(ScenarioId::new("").is_err());
        assert!(ScenarioId::new("x".repeat(65_537)).is_err());
        let set = ScenarioSet::new(vec![scenario("base", None), scenario("Base", None)]).unwrap();
        assert!(set.get("base").is_some());
        assert!(set.get("BASE").is_none());
        assert!(ScenarioSet::new(vec![scenario("same", None), scenario("same", None)]).is_err());
    }

    #[test]
    fn probabilities_are_all_or_none_and_sum_with_exact_tolerance() {
        assert!(ScenarioSet::new(vec![scenario("a", None), scenario("b", None)]).is_ok());
        assert!(ScenarioSet::new(vec![scenario("a", Some(1.0)), scenario("b", None)]).is_err());
        assert!(ScenarioSet::new(vec![scenario("a", Some(f64::NAN))]).is_err());
        assert!(
            ScenarioSet::new(vec![scenario("a", Some(-0.1)), scenario("b", Some(1.1))]).is_err()
        );
        assert!(ScenarioSet::new(vec![scenario("a", Some(0.4)), scenario("b", Some(0.6))]).is_ok());
        assert!(ScenarioSet::new(vec![scenario("a", Some(1.0 + 2e-12))]).is_err());
    }

    #[test]
    fn probability_accumulation_overflow_is_an_error() {
        let result = ScenarioSet::new(vec![
            scenario("a", Some(f64::MAX)),
            scenario("b", Some(f64::MAX)),
        ]);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().diagnostics()[0].code(),
            "VALIDATE.SCENARIO.PROBABILITY_SUM"
        );
    }

    #[test]
    fn an_empty_set_is_valid() {
        let set = ScenarioSet::<u8>::new(Vec::new()).unwrap();
        assert!(set.is_empty());
    }
}
