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

    /// Mutably borrow the scenario value. Identity and probability stay
    /// immutable so a set's validated index and probability sum remain valid.
    pub const fn value_mut(&mut self) -> &mut T {
        &mut self.value
    }

    #[must_use]
    pub fn into_value(self) -> T {
        self.value
    }

    /// Consume the entry and map its value while retaining its identity and
    /// probability.
    #[must_use]
    pub fn map_value<U>(self, map: impl FnOnce(T) -> U) -> Scenario<U> {
        Scenario {
            id: self.id,
            probability: self.probability,
            value: map(self.value),
        }
    }
}

/// Named alternatives with no implied time order.
pub struct ScenarioSet<T> {
    scenarios: Arc<Vec<Scenario<T>>>,
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
        if probability_count != 0
            && probability_count != scenarios.len()
            && let Some(missing) = scenarios
                .iter()
                .find(|scenario| scenario.probability.is_none())
        {
            return Err(Error::new(
                &crate::codes::VALIDATE_SCENARIO_MISSING_PROBABILITY,
                format!("scenario `{}` has no probability", missing.id),
            ));
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
            scenarios: Arc::new(scenarios),
            index: Arc::new(ids),
        })
    }

    #[must_use]
    /// Look one scenario value up by its stable ID. Constant time: IDs are
    /// case sensitive and never normalized, and the index is built once.
    pub fn get(&self, id: &str) -> Option<&T> {
        self.entry(id).map(Scenario::value)
    }

    /// Look up the complete scenario entry by its stable ID.
    #[must_use]
    pub fn entry(&self, id: &str) -> Option<&Scenario<T>> {
        self.entry_at(*self.index.get(id)?)
    }

    /// Look up one scenario value by its insertion position.
    #[must_use]
    pub fn get_at(&self, position: usize) -> Option<&T> {
        self.entry_at(position).map(Scenario::value)
    }

    /// Look up one complete scenario entry by its insertion position.
    #[must_use]
    pub fn entry_at(&self, position: usize) -> Option<&Scenario<T>> {
        self.scenarios.get(position)
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

impl<T: Clone> ScenarioSet<T> {
    /// Mutably borrow one scenario value by ID through copy on write.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut T> {
        self.entry_mut(id).map(Scenario::value_mut)
    }

    /// Mutably borrow one complete scenario entry by ID through copy on
    /// write. Its public mutable surface exposes only the value, preserving
    /// the validated ID index and probability sum.
    pub fn entry_mut(&mut self, id: &str) -> Option<&mut Scenario<T>> {
        let position = *self.index.get(id)?;
        self.entry_at_mut(position)
    }

    /// Mutably borrow one scenario value by insertion position.
    pub fn get_at_mut(&mut self, position: usize) -> Option<&mut T> {
        self.entry_at_mut(position).map(Scenario::value_mut)
    }

    /// Mutably borrow one complete scenario entry by insertion position.
    pub fn entry_at_mut(&mut self, position: usize) -> Option<&mut Scenario<T>> {
        Arc::make_mut(&mut self.scenarios).get_mut(position)
    }

    /// Iterate over mutable scenario entries through one copy on write
    /// detachment. Entry identities and probabilities remain immutable.
    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = &mut Scenario<T>> {
        Arc::make_mut(&mut self.scenarios).iter_mut()
    }

    /// Consume the set and map each scenario value while retaining identities,
    /// probabilities, order, and the already validated identity index. A
    /// uniquely owned set moves its values without cloning them; a shared set
    /// first performs the copy on write split.
    #[must_use]
    pub fn map_values<U>(self, mut map: impl FnMut(T) -> U) -> ScenarioSet<U> {
        let scenarios = Arc::unwrap_or_clone(self.scenarios)
            .into_iter()
            .map(|scenario| scenario.map_value(&mut map))
            .collect();
        ScenarioSet {
            scenarios: Arc::new(scenarios),
            index: self.index,
        }
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
        assert_eq!(set.get("s19999"), Some(&19_999));
        assert_eq!(set.get("s0"), Some(&0));
        assert!(set.get("S0").is_none(), "IDs are case sensitive");
        assert!(set.get("missing").is_none());
        // Cloning shares the index rather than rebuilding it.
        assert_eq!(set.clone().get("s12345"), Some(&12_345),);
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

    #[test]
    fn entry_and_mutable_value_access_preserve_validated_metadata() {
        let mut edited = ScenarioSet::new(vec![
            Scenario::new(ScenarioId::new("base").unwrap(), Some(0.25), 1_u8),
            Scenario::new(ScenarioId::new("high").unwrap(), Some(0.75), 2_u8),
        ])
        .unwrap();
        let original = edited.clone();
        assert!(Arc::ptr_eq(&edited.scenarios, &original.scenarios));
        assert!(Arc::ptr_eq(&edited.index, &original.index));

        *edited.get_mut("high").unwrap() = 9;
        assert_eq!(edited.get("high"), Some(&9));
        assert_eq!(original.get("high"), Some(&2));
        assert_eq!(edited.entry("high").unwrap().probability(), Some(0.75));
        assert_eq!(edited.entry_at(0).unwrap().id().as_str(), "base");
        assert_eq!(edited.get_at(0), Some(&1));
        assert!(Arc::ptr_eq(&edited.index, &original.index));
        assert!(!Arc::ptr_eq(&edited.scenarios, &original.scenarios));

        *edited.entry_at_mut(0).unwrap().value_mut() = 7;
        for scenario in edited.iter_mut() {
            *scenario.value_mut() += 1;
        }
        assert_eq!(edited.get("base"), Some(&8));
        assert_eq!(edited.get("high"), Some(&10));
    }

    #[test]
    fn consuming_map_moves_unique_values_and_reuses_the_index() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static CLONES: AtomicUsize = AtomicUsize::new(0);

        #[derive(Debug)]
        struct Counted(u8);

        impl Clone for Counted {
            fn clone(&self) -> Self {
                CLONES.fetch_add(1, Ordering::Relaxed);
                Self(self.0)
            }
        }

        CLONES.store(0, Ordering::Relaxed);
        let set = ScenarioSet::new(vec![Scenario::new(
            ScenarioId::new("base").unwrap(),
            Some(1.0),
            Counted(3),
        )])
        .unwrap();
        let index = Arc::clone(&set.index);
        let mapped = set.map_values(|value| usize::from(value.0));
        assert_eq!(mapped.get("base"), Some(&3));
        assert_eq!(mapped.entry("base").unwrap().probability(), Some(1.0));
        assert!(Arc::ptr_eq(&mapped.index, &index));
        assert_eq!(CLONES.load(Ordering::Relaxed), 0);
    }
}
