#![allow(dead_code)]

pub mod schema;

use std::collections::BTreeSet;
use std::fmt;
use std::marker::PhantomData;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Error {
    ShapeMismatch {
        what: &'static str,
        expected: usize,
        actual: usize,
    },
    DimensionOverflow {
        what: &'static str,
        rows: usize,
        columns: usize,
    },
    EmptyTimePointLabel {
        index: usize,
    },
    EmptyScenarioId,
    DuplicateScenarioId {
        id: String,
    },
    MissingScenarioProbability {
        id: String,
    },
    InvalidScenarioProbability {
        id: String,
        value: f64,
    },
    ScenarioProbabilitySum {
        sum: f64,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShapeMismatch {
                what,
                expected,
                actual,
            } => write!(f, "{what} has {actual} values, expected {expected}"),
            Self::DimensionOverflow {
                what,
                rows,
                columns,
            } => write!(f, "{what} dimensions {rows} by {columns} overflow usize"),
            Self::EmptyTimePointLabel { index } => {
                write!(f, "time point at index {index} has an empty label")
            }
            Self::EmptyScenarioId => f.write_str("scenario ID cannot be empty"),
            Self::DuplicateScenarioId { id } => write!(f, "duplicate scenario ID `{id}`"),
            Self::MissingScenarioProbability { id } => {
                write!(f, "scenario `{id}` has no probability")
            }
            Self::InvalidScenarioProbability { id, value } => write!(
                f,
                "scenario `{id}` probability must be finite and nonnegative; found {value}"
            ),
            Self::ScenarioProbabilitySum { sum } => {
                write!(f, "scenario probabilities must sum to 1; found {sum}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// An immutable network value. The table owners are private so unchanged
/// tables can be shared by later revisions without changing the public type.
#[derive(Clone, Debug, PartialEq)]
pub struct BalancedNetwork {
    bus_ids: Arc<[u64]>,
    load_p: Arc<[f64]>,
}

impl BalancedNetwork {
    pub fn new(bus_ids: Vec<u64>, load_p: Vec<f64>) -> Result<Self, Error> {
        if bus_ids.len() != load_p.len() {
            return Err(Error::ShapeMismatch {
                what: "balanced network load column",
                expected: bus_ids.len(),
                actual: load_p.len(),
            });
        }
        Ok(Self {
            bus_ids: bus_ids.into(),
            load_p: load_p.into(),
        })
    }

    pub fn bus_ids(&self) -> &[u64] {
        &self.bus_ids
    }

    pub fn load_p(&self) -> &[f64] {
        &self.load_p
    }

    /// A persistent edit that reuses the unchanged identity table.
    pub fn with_load_p(&self, load_p: Vec<f64>) -> Result<Self, Error> {
        if self.bus_ids.len() != load_p.len() {
            return Err(Error::ShapeMismatch {
                what: "balanced network load column",
                expected: self.bus_ids.len(),
                actual: load_p.len(),
            });
        }
        Ok(Self {
            bus_ids: Arc::clone(&self.bus_ids),
            load_p: load_p.into(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MulticonductorNetwork {
    bus_ids: Arc<[u64]>,
    load_p: Arc<[f64]>,
}

impl MulticonductorNetwork {
    pub fn new(bus_ids: Vec<u64>, load_p: Vec<f64>) -> Result<Self, Error> {
        if bus_ids.len() != load_p.len() {
            return Err(Error::ShapeMismatch {
                what: "multiconductor network load column",
                expected: bus_ids.len(),
                actual: load_p.len(),
            });
        }
        Ok(Self {
            bus_ids: bus_ids.into(),
            load_p: load_p.into(),
        })
    }

    pub fn bus_ids(&self) -> &[u64] {
        &self.bus_ids
    }

    pub fn load_p(&self) -> &[f64] {
        &self.load_p
    }
}

/// One representative instance. Sharing belongs to the relationship between
/// the instance and network, not to an extra public network data type.
#[derive(Clone, Debug, PartialEq)]
pub struct DcPfInstance {
    network: BalancedNetwork,
}

impl DcPfInstance {
    pub fn new(network: BalancedNetwork) -> Self {
        Self { network }
    }

    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AcPfInstance {
    network: BalancedNetwork,
}

impl AcPfInstance {
    pub fn new(network: BalancedNetwork) -> Self {
        Self { network }
    }

    pub fn network(&self) -> &BalancedNetwork {
        &self.network
    }
}

#[derive(Clone, Debug)]
pub struct DcPfSolution {
    instance: DcPfInstance,
}

impl DcPfSolution {
    pub fn new(instance: DcPfInstance) -> Self {
        Self { instance }
    }

    pub fn instance(&self) -> &DcPfInstance {
        &self.instance
    }
}

macro_rules! marker_values {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Clone, Debug, PartialEq)]
            pub struct $name;
        )+
    };
}

marker_values!(
    DcOpfInstance,
    AcOpfInstance,
    McAcPfInstance,
    McAcOpfInstance,
    AcScucInstance,
    AcPfSolution,
    DcOpfSolution,
    AcOpfSolution,
    McAcPfSolution,
    McAcOpfSolution,
    AcScucSolution,
);

#[derive(Clone, Debug, PartialEq)]
pub struct TimePoint {
    pub label: String,
    pub duration: Option<Duration>,
}

#[derive(Debug)]
struct BalancedOperatingPointColumns {
    network: BalancedNetwork,
    load_p: Arc<[f64]>,
}

#[derive(Debug)]
struct MulticonductorOperatingPointColumns {
    network: MulticonductorNetwork,
    load_p: Arc<[f64]>,
}

#[derive(Clone, Debug)]
enum OperatingPointData {
    Balanced(Arc<BalancedOperatingPointColumns>),
    Multiconductor(Arc<MulticonductorOperatingPointColumns>),
}

/// One complete electrical state. In a series it is a small owning handle into
/// shared numerical columns, so retaining a point neither keeps the parent series
/// alive nor materializes a network.
#[derive(Clone, Debug)]
pub struct OperatingPoint<N> {
    data: OperatingPointData,
    index: usize,
    marker: PhantomData<N>,
}

impl OperatingPoint<BalancedNetwork> {
    pub fn network(&self) -> &BalancedNetwork {
        let OperatingPointData::Balanced(columns) = &self.data else {
            unreachable!("private constructor maintains the network marker")
        };
        &columns.network
    }

    pub fn load_p(&self) -> &[f64] {
        let OperatingPointData::Balanced(columns) = &self.data else {
            unreachable!("private constructor maintains the network marker")
        };
        row(&columns.load_p, self.index, columns.network.bus_ids().len())
            .expect("private constructor checked the column shape")
    }
}

impl OperatingPoint<MulticonductorNetwork> {
    pub fn network(&self) -> &MulticonductorNetwork {
        let OperatingPointData::Multiconductor(columns) = &self.data else {
            unreachable!("private constructor maintains the network marker")
        };
        &columns.network
    }

    pub fn load_p(&self) -> &[f64] {
        let OperatingPointData::Multiconductor(columns) = &self.data else {
            unreachable!("private constructor maintains the network marker")
        };
        row(&columns.load_p, self.index, columns.network.bus_ids().len())
            .expect("private constructor checked the column shape")
    }
}

/// An ordinary generic sequence. Type specific constructors can place cheap
/// handles over shared numerical columns in `values`; no public trait for memory
/// representation is needed.
#[derive(Clone, Debug)]
pub struct TimeSeries<T> {
    time_points: Arc<[TimePoint]>,
    values: Arc<[T]>,
}

impl<T> TimeSeries<T> {
    pub fn new(time_points: Vec<TimePoint>, values: Vec<T>) -> Result<Self, Error> {
        if time_points.len() != values.len() {
            return Err(Error::ShapeMismatch {
                what: "time series values",
                expected: time_points.len(),
                actual: values.len(),
            });
        }
        if let Some((index, _)) = time_points
            .iter()
            .enumerate()
            .find(|(_, point)| point.label.is_empty())
        {
            return Err(Error::EmptyTimePointLabel { index });
        }
        Ok(Self {
            time_points: time_points.into(),
            values: values.into(),
        })
    }

    pub fn time_points(&self) -> &[TimePoint] {
        &self.time_points
    }

    pub fn values(&self) -> &[T] {
        &self.values
    }

    pub fn get(&self, index: usize) -> Option<(&TimePoint, &T)> {
        Some((self.time_points.get(index)?, self.values.get(index)?))
    }

    pub fn time_point(&self, index: usize) -> Option<&TimePoint> {
        self.time_points.get(index)
    }

    pub fn value(&self, index: usize) -> Option<&T> {
        self.values.get(index)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl TimeSeries<OperatingPoint<BalancedNetwork>> {
    pub fn from_load_columns(
        network: BalancedNetwork,
        time_points: Vec<TimePoint>,
        load_p: Vec<f64>,
    ) -> Result<Self, Error> {
        let expected = checked_product(time_points.len(), network.bus_ids().len())?;
        if load_p.len() != expected {
            return Err(Error::ShapeMismatch {
                what: "balanced operating point load column",
                expected,
                actual: load_p.len(),
            });
        }
        let count = time_points.len();
        let columns = Arc::new(BalancedOperatingPointColumns {
            network,
            load_p: load_p.into(),
        });
        let values = (0..count)
            .map(|index| OperatingPoint {
                data: OperatingPointData::Balanced(Arc::clone(&columns)),
                index,
                marker: PhantomData,
            })
            .collect();
        Self::new(time_points, values)
    }
}

impl TimeSeries<OperatingPoint<MulticonductorNetwork>> {
    pub fn from_load_columns(
        network: MulticonductorNetwork,
        time_points: Vec<TimePoint>,
        load_p: Vec<f64>,
    ) -> Result<Self, Error> {
        let expected = checked_product(time_points.len(), network.bus_ids().len())?;
        if load_p.len() != expected {
            return Err(Error::ShapeMismatch {
                what: "multiconductor operating point load column",
                expected,
                actual: load_p.len(),
            });
        }
        let count = time_points.len();
        let columns = Arc::new(MulticonductorOperatingPointColumns {
            network,
            load_p: load_p.into(),
        });
        let values = (0..count)
            .map(|index| OperatingPoint {
                data: OperatingPointData::Multiconductor(Arc::clone(&columns)),
                index,
                marker: PhantomData,
            })
            .collect();
        Self::new(time_points, values)
    }
}

fn checked_product(left: usize, right: usize) -> Result<usize, Error> {
    left.checked_mul(right).ok_or(Error::DimensionOverflow {
        what: "operating point column",
        rows: left,
        columns: right,
    })
}

fn row(values: &[f64], index: usize, width: usize) -> Option<&[f64]> {
    let start = index.checked_mul(width)?;
    values.get(start..start.checked_add(width)?)
}

/// A stable scenario key. Entry order is preserved for reproducible writing,
/// but position is not the scenario's semantic identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScenarioId(String);

impl ScenarioId {
    pub fn new(id: impl Into<String>) -> Result<Self, Error> {
        let id = id.into();
        if id.is_empty() {
            return Err(Error::EmptyScenarioId);
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct Scenario<T> {
    id: ScenarioId,
    probability: Option<f64>,
    value: T,
}

impl<T> Scenario<T> {
    pub fn new(id: ScenarioId, probability: Option<f64>, value: T) -> Self {
        Self {
            id,
            probability,
            value,
        }
    }

    pub fn id(&self) -> &ScenarioId {
        &self.id
    }

    pub fn probability(&self) -> Option<f64> {
        self.probability
    }

    pub fn value(&self) -> &T {
        &self.value
    }
}

#[derive(Clone, Debug)]
pub struct ScenarioSet<T> {
    scenarios: Arc<[Scenario<T>]>,
}

impl<T> ScenarioSet<T> {
    pub fn new(scenarios: Vec<Scenario<T>>) -> Result<Self, Error> {
        let mut ids = BTreeSet::new();
        for scenario in &scenarios {
            if !ids.insert(scenario.id.as_str()) {
                return Err(Error::DuplicateScenarioId {
                    id: scenario.id.as_str().to_owned(),
                });
            }
        }
        let probability_count = scenarios.iter().filter(|s| s.probability.is_some()).count();
        if probability_count != 0 && probability_count != scenarios.len() {
            let id = scenarios
                .iter()
                .find(|scenario| scenario.probability.is_none())
                .expect("a partial probability set has a missing entry")
                .id
                .as_str()
                .to_owned();
            return Err(Error::MissingScenarioProbability { id });
        }
        if probability_count != 0 {
            if let Some(scenario) = scenarios.iter().find(|scenario| {
                scenario
                    .probability
                    .is_some_and(|value| !value.is_finite() || value < 0.0)
            }) {
                return Err(Error::InvalidScenarioProbability {
                    id: scenario.id.as_str().to_owned(),
                    value: scenario.probability.expect("probabilities are complete"),
                });
            }
            let sum: f64 = scenarios.iter().filter_map(|s| s.probability).sum();
            if (sum - 1.0).abs() > 1e-12 {
                return Err(Error::ScenarioProbabilitySum { sum });
            }
        }
        Ok(Self {
            scenarios: scenarios.into(),
        })
    }

    pub fn get(&self, id: &str) -> Option<&Scenario<T>> {
        self.scenarios
            .iter()
            .find(|scenario| scenario.id.as_str() == id)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Scenario<T>> {
        self.scenarios.iter()
    }

    pub fn len(&self) -> usize {
        self.scenarios.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scenarios.is_empty()
    }
}

/// The finite built in values recognized at automatic parse, stored JSON, and
/// language binding boundaries. A typed `PioModule<T>` does not store this enum.
#[non_exhaustive]
#[derive(Debug)]
pub enum PioValue {
    BalancedNetwork(BalancedNetwork),
    MulticonductorNetwork(MulticonductorNetwork),
    BalancedNetworkTimeSeries(TimeSeries<BalancedNetwork>),
    BalancedOperatingPointTimeSeries(TimeSeries<OperatingPoint<BalancedNetwork>>),
    MulticonductorOperatingPointTimeSeries(TimeSeries<OperatingPoint<MulticonductorNetwork>>),
    BalancedNetworkScenarioSet(ScenarioSet<BalancedNetwork>),
    DcPfInstance(DcPfInstance),
    AcPfInstance(AcPfInstance),
    DcOpfInstance(DcOpfInstance),
    AcOpfInstance(AcOpfInstance),
    McAcPfInstance(McAcPfInstance),
    McAcOpfInstance(McAcOpfInstance),
    AcScucInstance(AcScucInstance),
    DcPfSolution(DcPfSolution),
    AcPfSolution(AcPfSolution),
    DcOpfSolution(DcOpfSolution),
    AcOpfSolution(AcOpfSolution),
    McAcPfSolution(McAcPfSolution),
    McAcOpfSolution(McAcOpfSolution),
    AcScucSolution(AcScucSolution),
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PioValueKind {
    BalancedNetwork,
    MulticonductorNetwork,
    BalancedNetworkTimeSeries,
    BalancedOperatingPointTimeSeries,
    MulticonductorOperatingPointTimeSeries,
    BalancedNetworkScenarioSet,
    DcPfInstance,
    AcPfInstance,
    DcOpfInstance,
    AcOpfInstance,
    McAcPfInstance,
    McAcOpfInstance,
    AcScucInstance,
    DcPfSolution,
    AcPfSolution,
    DcOpfSolution,
    AcOpfSolution,
    McAcPfSolution,
    McAcOpfSolution,
    AcScucSolution,
}

impl PioValueKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BalancedNetwork => "balanced_network",
            Self::MulticonductorNetwork => "multiconductor_network",
            Self::BalancedNetworkTimeSeries => "balanced_network_time_series",
            Self::BalancedOperatingPointTimeSeries => "balanced_operating_point_time_series",
            Self::MulticonductorOperatingPointTimeSeries => {
                "multiconductor_operating_point_time_series"
            }
            Self::BalancedNetworkScenarioSet => "balanced_network_scenario_set",
            Self::DcPfInstance => "dc_pf_instance",
            Self::AcPfInstance => "ac_pf_instance",
            Self::DcOpfInstance => "dc_opf_instance",
            Self::AcOpfInstance => "ac_opf_instance",
            Self::McAcPfInstance => "mc_ac_pf_instance",
            Self::McAcOpfInstance => "mc_ac_opf_instance",
            Self::AcScucInstance => "ac_scuc_instance",
            Self::DcPfSolution => "dc_pf_solution",
            Self::AcPfSolution => "ac_pf_solution",
            Self::DcOpfSolution => "dc_opf_solution",
            Self::AcOpfSolution => "ac_opf_solution",
            Self::McAcPfSolution => "mc_ac_pf_solution",
            Self::McAcOpfSolution => "mc_ac_opf_solution",
            Self::AcScucSolution => "ac_scuc_solution",
        }
    }
}

impl PioValue {
    pub fn kind(&self) -> PioValueKind {
        match self {
            Self::BalancedNetwork(_) => PioValueKind::BalancedNetwork,
            Self::MulticonductorNetwork(_) => PioValueKind::MulticonductorNetwork,
            Self::BalancedNetworkTimeSeries(_) => PioValueKind::BalancedNetworkTimeSeries,
            Self::BalancedOperatingPointTimeSeries(_) => {
                PioValueKind::BalancedOperatingPointTimeSeries
            }
            Self::MulticonductorOperatingPointTimeSeries(_) => {
                PioValueKind::MulticonductorOperatingPointTimeSeries
            }
            Self::BalancedNetworkScenarioSet(_) => PioValueKind::BalancedNetworkScenarioSet,
            Self::DcPfInstance(_) => PioValueKind::DcPfInstance,
            Self::AcPfInstance(_) => PioValueKind::AcPfInstance,
            Self::DcOpfInstance(_) => PioValueKind::DcOpfInstance,
            Self::AcOpfInstance(_) => PioValueKind::AcOpfInstance,
            Self::McAcPfInstance(_) => PioValueKind::McAcPfInstance,
            Self::McAcOpfInstance(_) => PioValueKind::McAcOpfInstance,
            Self::AcScucInstance(_) => PioValueKind::AcScucInstance,
            Self::DcPfSolution(_) => PioValueKind::DcPfSolution,
            Self::AcPfSolution(_) => PioValueKind::AcPfSolution,
            Self::DcOpfSolution(_) => PioValueKind::DcOpfSolution,
            Self::AcOpfSolution(_) => PioValueKind::AcOpfSolution,
            Self::McAcPfSolution(_) => PioValueKind::McAcPfSolution,
            Self::McAcOpfSolution(_) => PioValueKind::McAcOpfSolution,
            Self::AcScucSolution(_) => PioValueKind::AcScucSolution,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Source(Arc<SourceData>);

#[derive(Debug)]
struct SourceData {
    name: String,
    bytes: Arc<[u8]>,
}

impl Source {
    pub fn from_bytes(name: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self(Arc::new(SourceData {
            name: name.into(),
            bytes: bytes.into(),
        }))
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0.bytes
    }
}

#[derive(Debug, Default)]
struct ModuleRecords {
    diagnostics: Vec<String>,
    source: Option<Source>,
}

/// One typed compiler unit. `T` is ordinary static Rust type information and is
/// deliberately unconstrained.
#[derive(Debug)]
pub struct PioModule<T> {
    value: T,
    records: ModuleRecords,
}

impl<T> PioModule<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            records: ModuleRecords::default(),
        }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn into_value(self) -> T {
        self.value
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.records.diagnostics
    }

    pub fn source(&self) -> Option<&Source> {
        self.records.source.as_ref()
    }

    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.records.diagnostics.push(diagnostic.into());
        self
    }

    pub fn with_source(mut self, name: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Self {
        self.records.source = Some(Source::from_bytes(name, bytes));
        self
    }

    pub fn map_value<U>(self, convert: impl FnOnce(T) -> U) -> PioModule<U> {
        PioModule {
            value: convert(self.value),
            records: self.records,
        }
    }

    /// Internal cross-crate support for a recoverable value conversion.
    #[doc(hidden)]
    pub fn __try_map_value<U>(
        self,
        convert: impl FnOnce(T) -> Result<U, T>,
    ) -> Result<PioModule<U>, PioModule<T>> {
        let Self { value, records } = self;
        match convert(value) {
            Ok(value) => Ok(PioModule { value, records }),
            Err(value) => Err(PioModule { value, records }),
        }
    }
}

/// A recoverable checked conversion failure. The unexpected module remains
/// available for inspection or another narrowing attempt.
#[derive(Debug)]
pub struct ValueKindMismatch {
    expected: PioValueKind,
    module: PioModule<PioValue>,
}

impl ValueKindMismatch {
    pub fn expected(&self) -> PioValueKind {
        self.expected
    }

    pub fn actual(&self) -> PioValueKind {
        self.module.value.kind()
    }

    pub fn module(&self) -> &PioModule<PioValue> {
        &self.module
    }

    pub fn into_module(self) -> PioModule<PioValue> {
        self.module
    }
}

impl fmt::Display for ValueKindMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "expected `{}`, found `{}`",
            self.expected.as_str(),
            self.actual().as_str()
        )
    }
}

impl std::error::Error for ValueKindMismatch {}

mod private {
    pub trait Sealed {}
}

/// Internal connection between one built in concrete type and its dynamic
/// variant. This is a conversion behavior, not a bound on `PioModule<T>`.
#[doc(hidden)]
pub trait FromPioValue: private::Sealed + Sized {
    const KIND: PioValueKind;
    fn try_from_pio_value(value: PioValue) -> Result<Self, PioValue>;
}

/// Checks the dynamic kind, then moves the value and every module record into
/// a concrete module without importing an extension trait.
pub fn try_into_typed<T: FromPioValue>(
    module: PioModule<PioValue>,
) -> Result<PioModule<T>, ValueKindMismatch> {
    match module.__try_map_value(T::try_from_pio_value) {
        Ok(module) => Ok(module),
        Err(module) => Err(ValueKindMismatch {
            expected: T::KIND,
            module,
        }),
    }
}

macro_rules! dynamic_value {
    ($ty:ty, $variant:ident, $kind:ident) => {
        impl private::Sealed for $ty {}

        impl FromPioValue for $ty {
            const KIND: PioValueKind = PioValueKind::$kind;

            fn try_from_pio_value(value: PioValue) -> Result<Self, PioValue> {
                match value {
                    PioValue::$variant(value) => Ok(value),
                    value => Err(value),
                }
            }
        }

        impl From<$ty> for PioValue {
            fn from(value: $ty) -> Self {
                Self::$variant(value)
            }
        }
    };
}

dynamic_value!(BalancedNetwork, BalancedNetwork, BalancedNetwork);
dynamic_value!(
    MulticonductorNetwork,
    MulticonductorNetwork,
    MulticonductorNetwork
);
dynamic_value!(
    TimeSeries<BalancedNetwork>,
    BalancedNetworkTimeSeries,
    BalancedNetworkTimeSeries
);
dynamic_value!(
    TimeSeries<OperatingPoint<BalancedNetwork>>,
    BalancedOperatingPointTimeSeries,
    BalancedOperatingPointTimeSeries
);
dynamic_value!(
    TimeSeries<OperatingPoint<MulticonductorNetwork>>,
    MulticonductorOperatingPointTimeSeries,
    MulticonductorOperatingPointTimeSeries
);
dynamic_value!(
    ScenarioSet<BalancedNetwork>,
    BalancedNetworkScenarioSet,
    BalancedNetworkScenarioSet
);
dynamic_value!(DcPfInstance, DcPfInstance, DcPfInstance);
dynamic_value!(AcPfInstance, AcPfInstance, AcPfInstance);
dynamic_value!(DcOpfInstance, DcOpfInstance, DcOpfInstance);
dynamic_value!(AcOpfInstance, AcOpfInstance, AcOpfInstance);
dynamic_value!(McAcPfInstance, McAcPfInstance, McAcPfInstance);
dynamic_value!(McAcOpfInstance, McAcOpfInstance, McAcOpfInstance);
dynamic_value!(AcScucInstance, AcScucInstance, AcScucInstance);
dynamic_value!(DcPfSolution, DcPfSolution, DcPfSolution);
dynamic_value!(AcPfSolution, AcPfSolution, AcPfSolution);
dynamic_value!(DcOpfSolution, DcOpfSolution, DcOpfSolution);
dynamic_value!(AcOpfSolution, AcOpfSolution, AcOpfSolution);
dynamic_value!(McAcPfSolution, McAcPfSolution, McAcPfSolution);
dynamic_value!(McAcOpfSolution, McAcOpfSolution, McAcOpfSolution);
dynamic_value!(AcScucSolution, AcScucSolution, AcScucSolution);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemoFormat {
    OneFile,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArtifactPath(String);

impl ArtifactPath {
    pub fn new(name: impl Into<String>) -> Result<Self, WriteError> {
        let name = name.into();
        let valid = !name.is_empty()
            && !name.contains('\\')
            && name
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        if !valid {
            return Err(WriteError(format!("invalid artifact path `{name}`")));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn join(&self, name: &str) -> Result<Self, WriteError> {
        Self::new(format!("{}/{name}", self.0))
    }
}

#[derive(Debug)]
enum DestinationKind {
    Path(PathBuf),
    Memory { root: ArtifactPath },
}

#[derive(Debug)]
pub struct Destination {
    kind: DestinationKind,
}

impl Destination {
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self {
            kind: DestinationKind::Path(path.into()),
        }
    }

    pub fn memory(root: impl Into<String>) -> Result<Self, WriteError> {
        Ok(Self {
            kind: DestinationKind::Memory {
                root: ArtifactPath::new(root)?,
            },
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct MemoryArtifact {
    pub name: ArtifactPath,
    pub bytes: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WrittenOutput {
    Path {
        root: PathBuf,
        artifacts: Vec<PathBuf>,
    },
    Memory {
        artifacts: Vec<MemoryArtifact>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct WriteResult {
    pub output: WrittenOutput,
    pub diagnostics: Vec<String>,
}

#[derive(Debug)]
pub struct WriteError(String);

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WriteError {}

pub fn write_demo(format: DemoFormat, destination: Destination) -> Result<WriteResult, WriteError> {
    let files: Vec<(&str, &[u8])> = match format {
        DemoFormat::OneFile => vec![("case.m", b"mpc.version = '2';\n")],
        DemoFormat::Directory => vec![
            ("buses.csv", b"name,v_nom\nbus,110\n"),
            ("lines.csv", b"name,bus0,bus1\nline,bus,bus\n"),
        ],
    };
    let output = match destination.kind {
        DestinationKind::Memory { root } => WrittenOutput::Memory {
            artifacts: files
                .into_iter()
                .map(|(name, bytes)| {
                    let name = match format {
                        DemoFormat::OneFile => Ok(root.clone()),
                        DemoFormat::Directory => root.join(name),
                    }?;
                    Ok(MemoryArtifact {
                        name,
                        bytes: bytes.to_vec(),
                    })
                })
                .collect::<Result<Vec<_>, WriteError>>()?,
        },
        DestinationKind::Path(path) => match format {
            DemoFormat::OneFile => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(io_error)?;
                }
                // The staging entry exists only through an exclusive create
                // (its unique name still cannot collide with a caller's
                // file), and the commit is a hard link of that entry to the
                // destination name: the link fails if anything appeared at
                // the destination, so nothing is ever replaced.
                let staging = staging_path(&path);
                let result = (|| -> std::io::Result<()> {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&staging)?;
                    std::io::Write::write_all(&mut file, files[0].1)?;
                    std::fs::hard_link(&staging, &path)
                })();
                let _ = std::fs::remove_file(&staging);
                if let Err(error) = result {
                    return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
                        WriteError(format!("output `{}` already exists", path.display()))
                    } else {
                        io_error(error)
                    });
                }
                WrittenOutput::Path {
                    root: path.clone(),
                    artifacts: vec![path],
                }
            }
            DemoFormat::Directory => {
                // The destination directory itself is the reservation: an
                // exclusive create fails if the name is already present, and
                // the artifacts land inside the reserved object, so an
                // existing entry is refused and never replaced.
                if let Err(error) = std::fs::create_dir(&path) {
                    return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
                        WriteError(format!("output `{}` already exists", path.display()))
                    } else {
                        io_error(error)
                    });
                }
                let result = (|| -> Result<(), WriteError> {
                    for (name, bytes) in files {
                        std::fs::write(path.join(checked_relative(name)?), bytes)
                            .map_err(io_error)?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = std::fs::remove_dir_all(&path);
                    return Err(error);
                }
                WrittenOutput::Path {
                    root: path.clone(),
                    artifacts: vec![path.join("buses.csv"), path.join("lines.csv")],
                }
            }
        },
    };
    Ok(WriteResult {
        output,
        diagnostics: Vec::new(),
    })
}

fn staging_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("powerio-output");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    path.with_file_name(format!(
        ".{name}.powerio-tmp-{}-{nonce}",
        std::process::id()
    ))
}

fn checked_relative(name: &str) -> Result<&Path, WriteError> {
    let path = Path::new(name);
    if path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(WriteError(format!("invalid output name `{name}`")));
    }
    Ok(path)
}

fn io_error(error: std::io::Error) -> WriteError {
    WriteError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn balanced() -> BalancedNetwork {
        BalancedNetwork::new(vec![10, 20], vec![1.0, 2.0]).unwrap()
    }

    fn multiconductor() -> MulticonductorNetwork {
        MulticonductorNetwork::new(vec![10, 20], vec![1.0, 2.0]).unwrap()
    }

    fn points() -> Vec<TimePoint> {
        vec![
            TimePoint {
                label: "t0".to_owned(),
                duration: Some(Duration::from_secs(3600)),
            },
            TimePoint {
                label: "t1".to_owned(),
                duration: Some(Duration::from_secs(3600)),
            },
        ]
    }

    #[test]
    fn module_accepts_application_values_without_dynamic_registration() {
        struct ApplicationValue(Rc<()>);
        let module = PioModule::new(ApplicationValue(Rc::new(())));
        assert_eq!(Rc::strong_count(&module.value().0), 1);
    }

    #[test]
    fn built_in_dynamic_values_are_send_sync_and_static() {
        fn assert_bounds<T: Send + Sync + 'static>() {}

        assert_bounds::<PioValue>();
        assert_bounds::<PioModule<PioValue>>();
    }

    #[test]
    fn consuming_narrowing_moves_the_network_and_records() {
        let network = balanced();
        let load_ptr = network.load_p().as_ptr();
        let dynamic = PioModule::new(PioValue::BalancedNetwork(network))
            .with_diagnostic("kept")
            .with_source("case.m", Arc::<[u8]>::from(&b"source"[..]));
        let diagnostics_ptr = dynamic.diagnostics().as_ptr();
        let concrete: PioModule<BalancedNetwork> = try_into_typed(dynamic).unwrap();
        assert_eq!(concrete.value().load_p().as_ptr(), load_ptr);
        assert_eq!(concrete.diagnostics().as_ptr(), diagnostics_ptr);
    }

    #[test]
    fn failed_narrowing_returns_the_original_module() {
        let dynamic = PioModule::new(PioValue::MulticonductorNetwork(multiconductor()))
            .with_diagnostic("kept")
            .with_source("case.dss", Arc::<[u8]>::from(&b"source"[..]));
        let source_ptr = dynamic.source().unwrap().bytes().as_ptr();
        let error = try_into_typed::<BalancedNetwork>(dynamic).unwrap_err();
        assert_eq!(error.actual(), PioValueKind::MulticonductorNetwork);
        let recovered = error.into_module();
        assert_eq!(recovered.diagnostics(), ["kept"]);
        assert_eq!(recovered.source().unwrap().bytes().as_ptr(), source_ptr);
    }

    #[test]
    fn every_built_in_conversion_uses_the_same_move_path() {
        let dynamic: PioModule<PioValue> =
            PioModule::new(AcPfInstance::new(balanced())).map_value(PioValue::from);
        let concrete: PioModule<AcPfInstance> = try_into_typed(dynamic).unwrap();
        assert_eq!(concrete.value().network().bus_ids(), [10, 20]);
    }

    #[test]
    fn time_point_labels_are_nonempty() {
        let result = TimeSeries::new(
            vec![TimePoint {
                label: String::new(),
                duration: None,
            }],
            vec![balanced()],
        );
        assert!(matches!(
            result,
            Err(Error::EmptyTimePointLabel { index: 0 })
        ));
    }

    #[test]
    fn value_kind_schema_ids_are_exact_and_unique() {
        let kinds = [
            PioValueKind::BalancedNetwork,
            PioValueKind::MulticonductorNetwork,
            PioValueKind::BalancedNetworkTimeSeries,
            PioValueKind::BalancedOperatingPointTimeSeries,
            PioValueKind::MulticonductorOperatingPointTimeSeries,
            PioValueKind::BalancedNetworkScenarioSet,
            PioValueKind::DcPfInstance,
            PioValueKind::AcPfInstance,
            PioValueKind::DcOpfInstance,
            PioValueKind::AcOpfInstance,
            PioValueKind::McAcPfInstance,
            PioValueKind::McAcOpfInstance,
            PioValueKind::AcScucInstance,
            PioValueKind::DcPfSolution,
            PioValueKind::AcPfSolution,
            PioValueKind::DcOpfSolution,
            PioValueKind::AcOpfSolution,
            PioValueKind::McAcPfSolution,
            PioValueKind::McAcOpfSolution,
            PioValueKind::AcScucSolution,
        ];
        let ids = kinds.map(PioValueKind::as_str);
        assert_eq!(
            ids,
            [
                "balanced_network",
                "multiconductor_network",
                "balanced_network_time_series",
                "balanced_operating_point_time_series",
                "multiconductor_operating_point_time_series",
                "balanced_network_scenario_set",
                "dc_pf_instance",
                "ac_pf_instance",
                "dc_opf_instance",
                "ac_opf_instance",
                "mc_ac_pf_instance",
                "mc_ac_opf_instance",
                "ac_scuc_instance",
                "dc_pf_solution",
                "ac_pf_solution",
                "dc_opf_solution",
                "ac_opf_solution",
                "mc_ac_pf_solution",
                "mc_ac_opf_solution",
                "ac_scuc_solution",
            ]
        );
        assert_eq!(ids.len(), ids.into_iter().collect::<BTreeSet<_>>().len());
    }

    #[test]
    fn balanced_time_series_uses_shared_columns_and_network() {
        let network = balanced();
        let bus_ptr = network.bus_ids().as_ptr();
        let series = TimeSeries::<OperatingPoint<BalancedNetwork>>::from_load_columns(
            network,
            points(),
            vec![1.0, 2.0, 3.0, 4.0],
        )
        .unwrap();
        let retained = series.value(1).unwrap().clone();
        assert_eq!(retained.network().bus_ids().as_ptr(), bus_ptr);
        drop(series);
        assert_eq!(retained.load_p(), [3.0, 4.0]);
    }

    #[test]
    fn multiconductor_time_series_is_a_supported_concrete_value() {
        let network = multiconductor();
        let series = TimeSeries::<OperatingPoint<MulticonductorNetwork>>::from_load_columns(
            network,
            points(),
            vec![1.0, 2.0, 3.0, 4.0],
        )
        .unwrap();
        assert_eq!(series.value(1).unwrap().load_p(), [3.0, 4.0]);
        assert_eq!(
            PioValue::MulticonductorOperatingPointTimeSeries(series).kind(),
            PioValueKind::MulticonductorOperatingPointTimeSeries
        );
    }

    #[test]
    fn scenario_lookup_uses_identity_not_position() {
        let scenarios = ScenarioSet::new(vec![
            Scenario::new(ScenarioId::new("base").unwrap(), Some(0.75), balanced()),
            Scenario::new(
                ScenarioId::new("stress").unwrap(),
                Some(0.25),
                balanced().with_load_p(vec![3.0, 4.0]).unwrap(),
            ),
        ])
        .unwrap();
        assert_eq!(
            scenarios.get("stress").unwrap().value().load_p(),
            [3.0, 4.0]
        );
    }

    #[test]
    fn sibling_instances_and_solutions_share_owners() {
        let network = balanced();
        let bus_ptr = network.bus_ids().as_ptr();
        let dc = DcPfInstance::new(network.clone());
        let ac = AcPfInstance::new(network);
        assert_eq!(dc.network().bus_ids().as_ptr(), bus_ptr);
        assert_eq!(ac.network().bus_ids().as_ptr(), bus_ptr);
        let solution = DcPfSolution::new(dc);
        assert_eq!(solution.instance().network().bus_ids().as_ptr(), bus_ptr);
    }

    #[test]
    fn invalid_collection_shapes_return_errors() {
        assert!(BalancedNetwork::new(vec![1], vec![]).is_err());
        assert!(
            TimeSeries::<OperatingPoint<BalancedNetwork>>::from_load_columns(
                balanced(),
                points(),
                vec![1.0],
            )
            .is_err()
        );
        assert!(
            ScenarioSet::new(vec![
                Scenario::new(ScenarioId::new("same").unwrap(), None, balanced()),
                Scenario::new(ScenarioId::new("same").unwrap(), None, balanced()),
            ])
            .is_err()
        );
    }

    #[test]
    fn persistent_network_edit_reuses_unchanged_table_data() {
        let network = balanced();
        let bus_ptr = network.bus_ids().as_ptr();
        let edited = network.with_load_p(vec![4.0, 5.0]).unwrap();
        assert_eq!(edited.bus_ids().as_ptr(), bus_ptr);
        assert_ne!(edited.load_p().as_ptr(), network.load_p().as_ptr());
    }

    #[test]
    fn owned_memory_destination_returns_one_or_many_buffers() {
        let single =
            write_demo(DemoFormat::OneFile, Destination::memory("case.m").unwrap()).unwrap();
        let WrittenOutput::Memory { artifacts: single } = single.output else {
            panic!("memory output")
        };
        assert_eq!(single[0].name.as_str(), "case.m");

        let multiple =
            write_demo(DemoFormat::Directory, Destination::memory("case").unwrap()).unwrap();
        let WrittenOutput::Memory {
            artifacts: multiple,
        } = multiple.output
        else {
            panic!("memory output")
        };
        assert_eq!(
            multiple.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["case/buses.csv", "case/lines.csv"]
        );
    }

    #[test]
    fn owned_path_destination_handles_directory_output() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "powerio-v1-api-prototype-{}-{unique}",
            std::process::id()
        ));
        let result = write_demo(DemoFormat::Directory, Destination::path(&dir)).unwrap();
        assert_eq!(
            result.output,
            WrittenOutput::Path {
                root: dir.clone(),
                artifacts: vec![dir.join("buses.csv"), dir.join("lines.csv")],
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn destination_rejects_collisions_and_traversal() {
        assert!(Destination::memory("../escape").is_err());
        assert!(Destination::memory("a/./b").is_err());
        assert!(Destination::memory("a/../b").is_err());
        assert!(Destination::memory("a//b").is_err());
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "powerio-v1-api-prototype-existing-{}-{unique}",
            std::process::id()
        ));
        std::fs::write(&path, b"existing").unwrap();
        assert!(write_demo(DemoFormat::OneFile, Destination::path(&path)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_target_appearing_before_the_commit_is_refused_untouched() {
        // The commit is a non-replacing link: a file created at the
        // destination after the parent exists but before the commit lands is
        // refused, its bytes intact. The staged entry never survives.
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "powerio-v1-api-prototype-race-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("case.m");
        // Simulate the race by pre-creating the target; the exclusive link
        // refuses whatever the check-then-write ordering was.
        std::fs::write(&path, b"raced").unwrap();
        assert!(write_demo(DemoFormat::OneFile, Destination::path(&path)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"raced");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("case.m")]);

        // The directory reservation is the exclusive create itself.
        let existing_dir = dir.join("out");
        std::fs::create_dir(&existing_dir).unwrap();
        std::fs::write(existing_dir.join("keep.txt"), b"keep").unwrap();
        assert!(write_demo(DemoFormat::Directory, Destination::path(&existing_dir)).is_err());
        assert_eq!(
            std::fs::read(existing_dir.join("keep.txt")).unwrap(),
            b"keep"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
