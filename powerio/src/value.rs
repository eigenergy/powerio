//! The dynamic Rust boundary for values produced by universal parsing.

use powerio_core::{Scenario, ScenarioSet, TimePoint, TimeSeries};
use powerio_dist::MulticonductorNetwork;
use powerio_prob::OperatingPoint;
use powerio_tx::BalancedNetwork;

/// A dynamically typed time series at the universal parser boundary.
///
/// Typed Rust code uses `TimeSeries<T>` directly. This wrapper preserves the
/// element type for empty collections and lets C and PowerIO IR report the
/// same structural type name without a flattened collection registry.
#[derive(Clone, Debug)]
pub struct PioTimeSeries {
    element_type: Box<str>,
    type_name: Box<str>,
    values: TimeSeries<PioValue>,
}

impl PioTimeSeries {
    fn from_typed<T>(element_type: &'static str, values: TimeSeries<T>) -> Self
    where
        T: Clone + Into<PioValue>,
    {
        let values = values.map_values(Into::into);
        debug_assert!(
            values
                .values()
                .iter()
                .all(|value| value.type_name() == element_type)
        );
        Self {
            element_type: element_type.into(),
            type_name: format!("powerio.TimeSeries<{element_type}>").into_boxed_str(),
            values,
        }
    }

    /// Construct the dynamic form from values that already crossed the
    /// universal parser boundary.
    ///
    /// Typed Rust code should use [`TimeSeries<T>`] and its ordinary
    /// [`From`] conversion. This constructor exists for dynamic language
    /// bindings. An empty input has no value from which to infer `T` and is
    /// rejected.
    pub fn from_values(
        time_points: Vec<TimePoint>,
        values: Vec<PioValue>,
    ) -> Result<Self, powerio_core::Error> {
        let Some(first) = values.first() else {
            return Err(powerio_core::Error::new(
                &crate::codes::VALIDATE_COLLECTION_EMPTY,
                "a time series needs at least one value to infer its element type",
            ));
        };
        let element_type = first.type_name().to_owned();
        if !is_time_series_element_type(&element_type) {
            return Err(powerio_core::Error::new(
                &crate::codes::VALIDATE_COLLECTION_ELEMENT_TYPE,
                format!("PowerIO IR does not define a time series of `{element_type}`"),
            ));
        }
        require_element_type(&values, &element_type)?;
        let values = TimeSeries::new(time_points, values)?;
        Ok(Self {
            type_name: format!("powerio.TimeSeries<{element_type}>").into_boxed_str(),
            element_type: element_type.into_boxed_str(),
            values,
        })
    }

    #[must_use]
    pub fn element_type(&self) -> &str {
        &self.element_type
    }

    #[must_use]
    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    #[must_use]
    pub fn time_points(&self) -> &[TimePoint] {
        self.values.time_points()
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&PioValue> {
        self.values.get(index)
    }

    /// Mutably borrow one entry through the collection's copy on write
    /// storage.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut PioValue> {
        self.values.get_mut(index)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&TimePoint, &PioValue)> {
        self.values.iter()
    }

    /// Iterate over mutable entries through one copy on write split.
    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = (&TimePoint, &mut PioValue)> {
        self.values.iter_mut()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn values(&self) -> &TimeSeries<PioValue> {
        &self.values
    }
}

/// A dynamically typed scenario set at the universal parser boundary.
/// Typed Rust code uses `ScenarioSet<T>` directly.
#[derive(Clone, Debug)]
pub struct PioScenarioSet {
    element_type: Box<str>,
    type_name: Box<str>,
    values: ScenarioSet<PioValue>,
}

impl PioScenarioSet {
    fn from_typed<T>(element_type: &'static str, values: ScenarioSet<T>) -> Self
    where
        T: Clone + Into<PioValue>,
    {
        let values = values.map_values(Into::into);
        debug_assert!(
            values
                .iter()
                .all(|scenario| scenario.value().type_name() == element_type)
        );
        Self {
            element_type: element_type.into(),
            type_name: format!("powerio.ScenarioSet<{element_type}>").into_boxed_str(),
            values,
        }
    }

    /// Construct the dynamic form from scenarios that already crossed the
    /// universal parser boundary.
    ///
    /// Typed Rust code should use [`ScenarioSet<T>`] and its ordinary
    /// [`From`] conversion. An empty input has no value from which to infer
    /// `T` and is rejected.
    pub fn from_scenarios(scenarios: Vec<Scenario<PioValue>>) -> Result<Self, powerio_core::Error> {
        let Some(first) = scenarios.first() else {
            return Err(powerio_core::Error::new(
                &crate::codes::VALIDATE_COLLECTION_EMPTY,
                "a scenario set needs at least one value to infer its element type",
            ));
        };
        let element_type = first.value().type_name().to_owned();
        if !is_scenario_element_type(&element_type) {
            return Err(powerio_core::Error::new(
                &crate::codes::VALIDATE_COLLECTION_ELEMENT_TYPE,
                format!("PowerIO IR does not define a scenario set of `{element_type}`"),
            ));
        }
        require_element_type(
            &scenarios
                .iter()
                .map(|scenario| scenario.value().clone())
                .collect::<Vec<_>>(),
            &element_type,
        )?;
        let values = ScenarioSet::new(scenarios)?;
        Ok(Self {
            type_name: format!("powerio.ScenarioSet<{element_type}>").into_boxed_str(),
            element_type: element_type.into_boxed_str(),
            values,
        })
    }

    #[must_use]
    pub fn element_type(&self) -> &str {
        &self.element_type
    }

    #[must_use]
    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&PioValue> {
        self.values.get(id)
    }

    /// Mutably borrow one entry by scenario ID through copy on write.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut PioValue> {
        self.values.get_mut(id)
    }

    #[must_use]
    pub fn get_at(&self, position: usize) -> Option<&PioValue> {
        self.values.get_at(position)
    }

    /// Mutably borrow one entry by insertion position through copy on write.
    pub fn get_at_mut(&mut self, position: usize) -> Option<&mut PioValue> {
        self.values.get_at_mut(position)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Scenario<PioValue>> {
        self.values.iter()
    }

    /// Iterate over mutable scenario entries through one copy on write split.
    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = &mut Scenario<PioValue>> {
        self.values.iter_mut()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn values(&self) -> &ScenarioSet<PioValue> {
        &self.values
    }
}

const BALANCED_NETWORK_TYPE: &str = "powerio.BalancedNetwork";
const MULTICONDUCTOR_NETWORK_TYPE: &str = "powerio.MulticonductorNetwork";
const BALANCED_OPERATING_POINT_TYPE: &str = "powerio.OperatingPoint<powerio.BalancedNetwork>";
const MULTICONDUCTOR_OPERATING_POINT_TYPE: &str =
    "powerio.OperatingPoint<powerio.MulticonductorNetwork>";

fn is_time_series_element_type(type_name: &str) -> bool {
    matches!(
        type_name,
        BALANCED_NETWORK_TYPE
            | MULTICONDUCTOR_NETWORK_TYPE
            | BALANCED_OPERATING_POINT_TYPE
            | MULTICONDUCTOR_OPERATING_POINT_TYPE
    )
}

fn is_scenario_element_type(type_name: &str) -> bool {
    is_time_series_element_type(type_name)
        || matches!(
            type_name,
            "powerio.TimeSeries<powerio.BalancedNetwork>"
                | "powerio.TimeSeries<powerio.MulticonductorNetwork>"
                | "powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>"
                | "powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>"
        )
}

fn require_element_type(
    values: &[PioValue],
    element_type: &str,
) -> Result<(), powerio_core::Error> {
    if let Some(value) = values
        .iter()
        .find(|value| value.type_name() != element_type)
    {
        return Err(powerio_core::Error::new(
            &crate::codes::VALIDATE_COLLECTION_ELEMENT_TYPE,
            format!(
                "collection starts with `{element_type}` but also contains `{}`",
                value.type_name()
            ),
        ));
    }
    Ok(())
}

/// A value produced by PowerIO's universal parser or decoded from PowerIO IR.
/// Application values remain ordinary `PioModule<T>` values.
#[derive(Clone, Debug)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum PioValue {
    BalancedNetwork(BalancedNetwork),
    MulticonductorNetwork(MulticonductorNetwork),
    BalancedOperatingPoint(OperatingPoint<BalancedNetwork>),
    MulticonductorOperatingPoint(OperatingPoint<MulticonductorNetwork>),
    TimeSeries(PioTimeSeries),
    ScenarioSet(PioScenarioSet),
    DcPfInstance(powerio_prob::DcPfInstance),
    AcPfInstance(powerio_prob::AcPfInstance),
    DcOpfInstance(powerio_prob::DcOpfInstance),
    AcOpfInstance(powerio_prob::AcOpfInstance),
    McAcPfInstance(powerio_prob::McAcPfInstance),
    McAcOpfInstance(powerio_prob::McAcOpfInstance),
    AcScucInstance(powerio_prob::AcScucInstance),
    DcPfSolution(powerio_prob::DcPfSolution),
    AcPfSolution(powerio_prob::AcPfSolution),
    DcOpfSolution(powerio_prob::DcOpfSolution),
    AcOpfSolution(powerio_prob::AcOpfSolution),
    SocwrOpfSolution(powerio_prob::solution::SocwrOpfSolution),
    McAcPfSolution(powerio_prob::McAcPfSolution),
    McAcOpfSolution(powerio_prob::McAcOpfSolution),
    AcScucSolution(powerio_prob::AcScucSolution),
}

impl PioValue {
    /// Canonical structural type name used by C and PowerIO IR.
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::BalancedNetwork(_) => "powerio.BalancedNetwork",
            Self::MulticonductorNetwork(_) => "powerio.MulticonductorNetwork",
            Self::BalancedOperatingPoint(_) => "powerio.OperatingPoint<powerio.BalancedNetwork>",
            Self::MulticonductorOperatingPoint(_) => {
                "powerio.OperatingPoint<powerio.MulticonductorNetwork>"
            }
            Self::TimeSeries(series) => series.type_name(),
            Self::ScenarioSet(scenarios) => scenarios.type_name(),
            Self::DcPfInstance(_) => "powerio.DcPfInstance",
            Self::AcPfInstance(_) => "powerio.AcPfInstance",
            Self::DcOpfInstance(_) => "powerio.DcOpfInstance",
            Self::AcOpfInstance(_) => "powerio.AcOpfInstance",
            Self::McAcPfInstance(_) => "powerio.McAcPfInstance",
            Self::McAcOpfInstance(_) => "powerio.McAcOpfInstance",
            Self::AcScucInstance(_) => "powerio.AcScucInstance",
            Self::DcPfSolution(_) => "powerio.DcPfSolution",
            Self::AcPfSolution(_) => "powerio.AcPfSolution",
            Self::DcOpfSolution(_) => "powerio.DcOpfSolution",
            Self::AcOpfSolution(_) => "powerio.AcOpfSolution",
            Self::SocwrOpfSolution(_) => "powerio.SocwrOpfSolution",
            Self::McAcPfSolution(_) => "powerio.McAcPfSolution",
            Self::McAcOpfSolution(_) => "powerio.McAcOpfSolution",
            Self::AcScucSolution(_) => "powerio.AcScucSolution",
        }
    }
}

macro_rules! value_conversion {
    ($ty:ty, $variant:ident) => {
        impl From<$ty> for PioValue {
            fn from(value: $ty) -> Self {
                Self::$variant(value)
            }
        }
    };
}

impl From<BalancedNetwork> for PioValue {
    fn from(mut value: BalancedNetwork) -> Self {
        value.assign_missing_component_ids();
        Self::BalancedNetwork(value)
    }
}
value_conversion!(MulticonductorNetwork, MulticonductorNetwork);
value_conversion!(OperatingPoint<BalancedNetwork>, BalancedOperatingPoint);
value_conversion!(
    OperatingPoint<MulticonductorNetwork>,
    MulticonductorOperatingPoint
);
value_conversion!(powerio_prob::DcPfInstance, DcPfInstance);
value_conversion!(powerio_prob::AcPfInstance, AcPfInstance);
value_conversion!(powerio_prob::DcOpfInstance, DcOpfInstance);
value_conversion!(powerio_prob::AcOpfInstance, AcOpfInstance);
value_conversion!(powerio_prob::McAcPfInstance, McAcPfInstance);
value_conversion!(powerio_prob::McAcOpfInstance, McAcOpfInstance);
value_conversion!(powerio_prob::AcScucInstance, AcScucInstance);
value_conversion!(powerio_prob::DcPfSolution, DcPfSolution);
value_conversion!(powerio_prob::AcPfSolution, AcPfSolution);
value_conversion!(powerio_prob::DcOpfSolution, DcOpfSolution);
value_conversion!(powerio_prob::AcOpfSolution, AcOpfSolution);
value_conversion!(powerio_prob::solution::SocwrOpfSolution, SocwrOpfSolution);
value_conversion!(powerio_prob::McAcPfSolution, McAcPfSolution);
value_conversion!(powerio_prob::McAcOpfSolution, McAcOpfSolution);
value_conversion!(powerio_prob::AcScucSolution, AcScucSolution);

macro_rules! time_series_conversion {
    ($ty:ty, $name:literal) => {
        impl From<TimeSeries<$ty>> for PioValue {
            fn from(value: TimeSeries<$ty>) -> Self {
                Self::TimeSeries(PioTimeSeries::from_typed($name, value))
            }
        }
    };
}

time_series_conversion!(BalancedNetwork, "powerio.BalancedNetwork");
time_series_conversion!(MulticonductorNetwork, "powerio.MulticonductorNetwork");
time_series_conversion!(
    OperatingPoint<BalancedNetwork>,
    "powerio.OperatingPoint<powerio.BalancedNetwork>"
);
time_series_conversion!(
    OperatingPoint<MulticonductorNetwork>,
    "powerio.OperatingPoint<powerio.MulticonductorNetwork>"
);

macro_rules! scenario_set_conversion {
    ($ty:ty, $name:literal) => {
        impl From<ScenarioSet<$ty>> for PioValue {
            fn from(value: ScenarioSet<$ty>) -> Self {
                Self::ScenarioSet(PioScenarioSet::from_typed($name, value))
            }
        }
    };
}

scenario_set_conversion!(BalancedNetwork, "powerio.BalancedNetwork");
scenario_set_conversion!(MulticonductorNetwork, "powerio.MulticonductorNetwork");
scenario_set_conversion!(
    OperatingPoint<BalancedNetwork>,
    "powerio.OperatingPoint<powerio.BalancedNetwork>"
);
scenario_set_conversion!(
    OperatingPoint<MulticonductorNetwork>,
    "powerio.OperatingPoint<powerio.MulticonductorNetwork>"
);
scenario_set_conversion!(
    TimeSeries<BalancedNetwork>,
    "powerio.TimeSeries<powerio.BalancedNetwork>"
);
scenario_set_conversion!(
    TimeSeries<MulticonductorNetwork>,
    "powerio.TimeSeries<powerio.MulticonductorNetwork>"
);
scenario_set_conversion!(
    TimeSeries<OperatingPoint<BalancedNetwork>>,
    "powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>"
);
scenario_set_conversion!(
    TimeSeries<OperatingPoint<MulticonductorNetwork>>,
    "powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>"
);

#[cfg(test)]
mod tests {
    use super::*;

    fn small_balanced() -> BalancedNetwork {
        use powerio_tx::{Bus, BusId, BusType};
        BalancedNetwork::in_memory(
            "facade",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            Vec::new(),
        )
    }

    #[test]
    fn rust_uses_enum_matching_and_structural_names() {
        let value = PioValue::from(small_balanced());
        assert!(matches!(value, PioValue::BalancedNetwork(_)));
        assert_eq!(value.type_name(), "powerio.BalancedNetwork");
    }

    #[test]
    fn collections_keep_generic_structural_names() {
        let series = TimeSeries::new(
            vec![TimePoint::new("now", None).unwrap()],
            vec![small_balanced()],
        )
        .unwrap();
        let value = PioValue::from(series);
        let PioValue::TimeSeries(series) = &value else {
            unreachable!();
        };
        assert_eq!(series.len(), 1);
        assert_eq!(
            value.type_name(),
            "powerio.TimeSeries<powerio.BalancedNetwork>"
        );
    }

    #[test]
    fn scenario_sets_compose_with_time_series() {
        let series = TimeSeries::new(
            vec![TimePoint::new("now", None).unwrap()],
            vec![small_balanced()],
        )
        .unwrap();
        let set = ScenarioSet::new(vec![Scenario::new(
            powerio_core::ScenarioId::new("base").unwrap(),
            None,
            series,
        )])
        .unwrap();
        let value = PioValue::from(set);
        assert_eq!(
            value.type_name(),
            "powerio.ScenarioSet<powerio.TimeSeries<powerio.BalancedNetwork>>"
        );
    }
}
