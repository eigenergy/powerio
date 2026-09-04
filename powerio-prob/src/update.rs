//! Typed, atomic updates to electrical assignments, network limits, and
//! calculation instances.

use std::collections::HashSet;

use powerio_core::{ComponentId, Error, HistoryEntry, HistoryId, HistoryKind, PioModule, Producer};
use powerio_dist::MulticonductorNetwork;
use powerio_tx::{BalancedNetwork, BusId};
use serde::{Deserialize, Serialize};

use crate::diagnostics::codes;
use crate::instance::{
    AcOpfInstance, AcPfInstance, DcOpfInstance, DcPfInstance, McAcOpfInstance, McAcPfInstance,
};
use crate::operating::balanced::{
    BRANCH_IN_SERVICE, BRANCH_PHASE_SHIFT, BRANCH_TAP_RATIO, GENERATOR_ACTIVE_POWER,
    GENERATOR_IN_SERVICE, GENERATOR_REACTIVE_POWER, GENERATOR_VOLTAGE_SETPOINT, LOAD_ACTIVE_POWER,
    LOAD_REACTIVE_POWER, SWITCH_CLOSED,
};
use crate::operating::multiconductor::{
    GENERATOR_ACTIVE_POWER as MC_GENERATOR_ACTIVE_POWER,
    GENERATOR_REACTIVE_POWER as MC_GENERATOR_REACTIVE_POWER,
    LOAD_ACTIVE_POWER as MC_LOAD_ACTIVE_POWER, LOAD_REACTIVE_POWER as MC_LOAD_REACTIVE_POWER,
    SWITCH_CLOSED as MC_SWITCH_CLOSED,
};
use crate::operating::{OperatingPoint, QuantityLayout, row_identity};

const LOAD: &str = "load";
const GENERATOR: &str = "generator";
const BRANCH: &str = "branch";
const TRANSFORMER: &str = "transformer";
const SWITCH: &str = "switch";
const LINE: &str = "line";

/// Unit carried by an active power replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ActivePowerUnit {
    Watts,
    Megawatts,
}

/// An absolute active power replacement with an explicit unit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ActivePower {
    value: f64,
    unit: ActivePowerUnit,
}

impl ActivePower {
    #[must_use]
    pub const fn from_watts(value: f64) -> Self {
        Self {
            value,
            unit: ActivePowerUnit::Watts,
        }
    }

    #[must_use]
    pub const fn from_megawatts(value: f64) -> Self {
        Self {
            value,
            unit: ActivePowerUnit::Megawatts,
        }
    }

    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }

    #[must_use]
    pub const fn unit(self) -> ActivePowerUnit {
        self.unit
    }

    fn watts_value(self) -> Result<f64, Error> {
        let factor = if self.unit == ActivePowerUnit::Megawatts {
            1_000_000.0
        } else {
            1.0
        };
        convert_power(self.value, factor, "active power")
    }

    fn megawatts_value(self) -> Result<f64, Error> {
        let factor = if self.unit == ActivePowerUnit::Watts {
            1.0 / 1_000_000.0
        } else {
            1.0
        };
        convert_power(self.value, factor, "active power")
    }
}

/// How an aggregate active demand replacement is divided among the in service
/// loads connected to one bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoadAllocation {
    /// Give every participating load the same share of the aggregate demand.
    Equal,
    /// Preserve each load's share of the current aggregate active demand.
    /// Every participating load must have a persistent identity and a
    /// nonnegative active demand, and their sum must be positive.
    ProportionalToCurrentActivePower,
}

/// Unit carried by a reactive power replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReactivePowerUnit {
    Vars,
    Megavars,
}

/// An absolute reactive power replacement with an explicit unit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReactivePower {
    value: f64,
    unit: ReactivePowerUnit,
}

impl ReactivePower {
    #[must_use]
    pub const fn from_vars(value: f64) -> Self {
        Self {
            value,
            unit: ReactivePowerUnit::Vars,
        }
    }

    #[must_use]
    pub const fn from_megavars(value: f64) -> Self {
        Self {
            value,
            unit: ReactivePowerUnit::Megavars,
        }
    }

    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }

    #[must_use]
    pub const fn unit(self) -> ReactivePowerUnit {
        self.unit
    }

    fn vars_value(self) -> Result<f64, Error> {
        let factor = if self.unit == ReactivePowerUnit::Megavars {
            1_000_000.0
        } else {
            1.0
        };
        convert_power(self.value, factor, "reactive power")
    }

    fn megavars_value(self) -> Result<f64, Error> {
        let factor = if self.unit == ReactivePowerUnit::Vars {
            1.0 / 1_000_000.0
        } else {
            1.0
        };
        convert_power(self.value, factor, "reactive power")
    }
}

/// Unit carried by an apparent power rating replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ApparentPowerUnit {
    VoltAmperes,
    MegavoltAmperes,
}

/// An absolute apparent power rating with an explicit unit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApparentPower {
    value: f64,
    unit: ApparentPowerUnit,
}

impl ApparentPower {
    #[must_use]
    pub const fn from_volt_amperes(value: f64) -> Self {
        Self {
            value,
            unit: ApparentPowerUnit::VoltAmperes,
        }
    }

    #[must_use]
    pub const fn from_megavolt_amperes(value: f64) -> Self {
        Self {
            value,
            unit: ApparentPowerUnit::MegavoltAmperes,
        }
    }

    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }

    #[must_use]
    pub const fn unit(self) -> ApparentPowerUnit {
        self.unit
    }

    fn volt_amperes_value(self) -> Result<f64, Error> {
        let factor = if self.unit == ApparentPowerUnit::MegavoltAmperes {
            1_000_000.0
        } else {
            1.0
        };
        let value = convert_power(self.value, factor, "apparent power rating")?;
        require_nonnegative(value, "apparent power rating")
    }

    fn megavolt_amperes_value(self) -> Result<f64, Error> {
        let factor = if self.unit == ApparentPowerUnit::VoltAmperes {
            1.0 / 1_000_000.0
        } else {
            1.0
        };
        let value = convert_power(self.value, factor, "apparent power rating")?;
        require_nonnegative(value, "apparent power rating")
    }
}

fn convert_power(value: f64, factor: f64, field: &str) -> Result<f64, Error> {
    require_finite(value, field)?;
    let converted = value * factor;
    require_finite(converted, field)
}

fn require_finite(value: f64, field: &str) -> Result<f64, Error> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Error::new(
            &codes::VALIDATE_UPDATE_VALUE_INVALID,
            format!("{field} must be finite"),
        ))
    }
}

fn require_positive(value: f64, field: &str) -> Result<f64, Error> {
    let value = require_finite(value, field)?;
    if value > 0.0 {
        Ok(value)
    } else {
        Err(Error::new(
            &codes::VALIDATE_UPDATE_VALUE_INVALID,
            format!("{field} must be greater than zero"),
        ))
    }
}

fn require_nonnegative(value: f64, field: &str) -> Result<f64, Error> {
    let value = require_finite(value, field)?;
    if value >= 0.0 {
        Ok(value)
    } else {
        Err(Error::new(
            &codes::VALIDATE_UPDATE_VALUE_INVALID,
            format!("{field} must be nonnegative"),
        ))
    }
}

fn number_changed(previous: f64, replacement: f64) -> bool {
    previous.to_bits() != replacement.to_bits()
}

/// An update to an electrical assignment. Every replacement is absolute.
/// A conductor resolved power update names one terminal; a balanced update
/// leaves `terminal` as `None`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "field")]
#[non_exhaustive]
pub enum OperatingPointUpdate {
    LoadActivePower {
        load: ComponentId,
        terminal: Option<String>,
        p: ActivePower,
    },
    LoadReactivePower {
        load: ComponentId,
        terminal: Option<String>,
        q: ReactivePower,
    },
    GeneratorActivePower {
        generator: ComponentId,
        terminal: Option<String>,
        p: ActivePower,
    },
    GeneratorReactivePower {
        generator: ComponentId,
        terminal: Option<String>,
        q: ReactivePower,
    },
    GeneratorVoltageMagnitude {
        generator: ComponentId,
        vm_pu: f64,
    },
    GeneratorInService {
        generator: ComponentId,
        in_service: bool,
    },
    BranchInService {
        branch: ComponentId,
        in_service: bool,
    },
    TransformerTapRatio {
        transformer: ComponentId,
        tap_ratio: f64,
    },
    TransformerPhaseShift {
        transformer: ComponentId,
        shift_degrees: f64,
    },
    SwitchClosed {
        switch: ComponentId,
        closed: bool,
    },
}

/// An update to a physical network parameter or limit. Every replacement is
/// absolute. A conductor resolved line rating names one terminal; a balanced
/// branch rating leaves `terminal` as `None`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "field")]
#[non_exhaustive]
pub enum NetworkUpdate {
    BranchThermalRating {
        branch: ComponentId,
        terminal: Option<String>,
        rating: ApparentPower,
    },
}

/// One update to a calculation instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "data_role", content = "update")]
#[non_exhaustive]
pub enum CalculationUpdate {
    OperatingPoint(OperatingPointUpdate),
    Network(NetworkUpdate),
}

impl From<OperatingPointUpdate> for CalculationUpdate {
    fn from(update: OperatingPointUpdate) -> Self {
        Self::OperatingPoint(update)
    }
}

impl From<NetworkUpdate> for CalculationUpdate {
    fn from(update: NetworkUpdate) -> Self {
        Self::Network(update)
    }
}

/// A field changed by an update batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UpdatedField {
    LoadActivePower,
    LoadReactivePower,
    GeneratorActivePower,
    GeneratorReactivePower,
    GeneratorVoltageMagnitude,
    GeneratorInService,
    BranchThermalRating,
    BranchInService,
    TransformerTapRatio,
    TransformerPhaseShift,
    SwitchClosed,
}

/// One exact component field changed by an update batch.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateChange {
    component_id: ComponentId,
    field: UpdatedField,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal: Option<String>,
}

impl UpdateChange {
    #[must_use]
    pub const fn component_id(&self) -> &ComponentId {
        &self.component_id
    }

    #[must_use]
    pub const fn field(&self) -> UpdatedField {
        self.field
    }

    #[must_use]
    pub fn terminal(&self) -> Option<&str> {
        self.terminal.as_deref()
    }
}

/// Exact changes made by one atomic update batch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateReport {
    changes: Vec<UpdateChange>,
    connectivity_changed: bool,
}

impl UpdateReport {
    #[must_use]
    pub fn changes(&self) -> &[UpdateChange] {
        &self.changes
    }

    /// True only when a changed branch service flag or switch position changes
    /// which terminal pairs are electrically connected.
    #[must_use]
    pub const fn connectivity_changed(&self) -> bool {
        self.connectivity_changed
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

#[derive(Default)]
struct ReportBuilder {
    seen: HashSet<UpdateChange>,
    report: UpdateReport,
}

impl ReportBuilder {
    fn record(
        &mut self,
        component_id: ComponentId,
        field: UpdatedField,
        terminal: Option<&str>,
        changed: bool,
        connectivity: bool,
    ) -> Result<(), Error> {
        let change = UpdateChange {
            component_id,
            field,
            terminal: terminal.map(str::to_owned),
        };
        if !self.seen.insert(change.clone()) {
            return Err(Error::new(
                &codes::VALIDATE_UPDATE_DUPLICATE_FIELD,
                format!(
                    "the batch assigns {} {:?} more than once",
                    change.component_id, change.field
                ),
            ));
        }
        if changed {
            self.report.connectivity_changed |= connectivity;
            self.report.changes.push(change);
        }
        Ok(())
    }
}

/// Implementation trait for [`apply_updates`]. It is public only so the free
/// function can be generic across the built in targets.
#[doc(hidden)]
pub trait UpdateTarget<U> {
    fn apply_update_batch(&mut self, updates: &[U]) -> Result<UpdateReport, Error>;
}

/// Validate a complete batch, then apply it atomically to `target`.
///
/// If any update is invalid, `target` is unchanged.
pub fn apply_updates<T, U>(target: &mut T, updates: &[U]) -> Result<UpdateReport, Error>
where
    T: UpdateTarget<U>,
{
    target.apply_update_batch(updates)
}

/// A balanced calculation instance whose network can receive typed updates.
///
/// This trait lets one PowerIO operation cover the balanced power flow and
/// optimal power flow instance types without copying their tables.
pub trait BalancedCalculationInstance: Clone + UpdateTarget<CalculationUpdate> {
    /// The instance's balanced electrical network.
    fn network(&self) -> &BalancedNetwork;
}

macro_rules! impl_balanced_calculation_instance {
    ($($instance:ty),+ $(,)?) => {
        $(
            impl BalancedCalculationInstance for $instance {
                fn network(&self) -> &BalancedNetwork {
                    self.network()
                }
            }
        )+
    };
}

impl_balanced_calculation_instance!(DcPfInstance, AcPfInstance, DcOpfInstance, AcOpfInstance);

/// Replace the aggregate active demand at one bus using an explicit allocation
/// rule, then apply the resulting load updates atomically to the module.
///
/// Only in service loads participate. The returned report names every load
/// whose active demand changed. No load is selected by table position.
///
/// # Errors
/// The bus is unknown, the bus has no in service load, a participating load
/// lacks a persistent identity, or the selected allocation rule has no valid
/// basis.
pub fn apply_bus_load_active_power<T>(
    module: &mut PioModule<T>,
    bus: BusId,
    total: ActivePower,
    allocation: LoadAllocation,
) -> Result<UpdateReport, Error>
where
    T: BalancedCalculationInstance,
{
    let total_mw = require_nonnegative(total.megawatts_value()?, "bus active demand")?;
    let network = module.value().network();
    if !network.buses().iter().any(|candidate| candidate.id == bus) {
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_COMPONENT_UNKNOWN,
            format!("the calculation contains no bus `{bus}`"),
        ));
    }

    let loads: Vec<_> = network
        .loads()
        .iter()
        .filter(|load| load.in_service && load.bus == bus)
        .collect();
    if loads.is_empty() {
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_ALLOCATION_FAILED,
            format!("bus `{bus}` has no in service load to receive active demand"),
        ));
    }

    for load in &loads {
        if load.uid.is_none() {
            return Err(Error::new(
                &codes::VALIDATE_UPDATE_STABLE_ID_REQUIRED,
                format!("an in service load at bus `{bus}` has no persistent identity"),
            ));
        }
    }

    let (weights, weight_sum) = match allocation {
        LoadAllocation::Equal => {
            let weights = vec![1.0; loads.len()];
            let sum = weights.iter().sum();
            (weights, sum)
        }
        LoadAllocation::ProportionalToCurrentActivePower => {
            let mut sum = 0.0;
            for load in &loads {
                require_nonnegative(load.p, "current load active demand")?;
                sum += load.p;
            }
            let sum = require_positive(sum, "current aggregate active demand")?;
            (loads.iter().map(|load| load.p).collect(), sum)
        }
    };

    let mut assigned_mw = 0.0;
    let mut updates = Vec::with_capacity(loads.len());
    for (index, load) in loads.iter().enumerate() {
        let load_id = load.uid.as_deref().ok_or_else(|| {
            Error::new(
                &codes::VALIDATE_UPDATE_STABLE_ID_REQUIRED,
                format!("an in service load at bus `{bus}` has no persistent identity"),
            )
        })?;
        let replacement_mw = if index + 1 == loads.len() {
            // Rounding can leave the remainder a hair below zero.
            (total_mw - assigned_mw).max(0.0)
        } else {
            let replacement = total_mw * (weights[index] / weight_sum);
            assigned_mw += replacement;
            replacement
        };
        updates.push(CalculationUpdate::OperatingPoint(
            OperatingPointUpdate::LoadActivePower {
                load: ComponentId::new(LOAD, load_id)?,
                terminal: None,
                p: ActivePower::from_megawatts(replacement_mw),
            },
        ));
    }

    apply_updates(module, &updates)
}

impl<T, U> UpdateTarget<U> for PioModule<T>
where
    T: Clone + UpdateTarget<U>,
    U: Serialize,
{
    fn apply_update_batch(&mut self, updates: &[U]) -> Result<UpdateReport, Error> {
        let history_id = unused_history_id(self, "apply-updates");
        let mut edit = self.stage_edit();
        let report = edit.value_mut().apply_update_batch(updates)?;
        if report.is_empty() {
            return Ok(report);
        }

        let mut parameters = std::collections::BTreeMap::new();
        parameters.insert(
            "updates".to_owned(),
            serde_json::to_value(updates).map_err(|error| {
                Error::new(
                    &codes::VALIDATE_UPDATE_VALUE_INVALID,
                    format!("cannot record the applied updates: {error}"),
                )
            })?,
        );
        parameters.insert(
            "changes".to_owned(),
            serde_json::to_value(report.changes()).map_err(|error| {
                Error::new(
                    &codes::VALIDATE_UPDATE_VALUE_INVALID,
                    format!("cannot record the applied changes: {error}"),
                )
            })?,
        );
        parameters.insert(
            "connectivity_changed".to_owned(),
            serde_json::Value::Bool(report.connectivity_changed()),
        );
        let history = HistoryEntry::new(history_id, HistoryKind::Edit, "apply_updates")?
            .with_parameters(parameters)?;
        let producer = Producer::new("powerio", env!("CARGO_PKG_VERSION"))?;
        edit.commit(producer, history)?;
        Ok(report)
    }
}

fn unused_history_id<T>(module: &PioModule<T>, base: &str) -> HistoryId {
    let taken: HashSet<&str> = module
        .history()
        .iter()
        .map(|entry| entry.id().as_str())
        .collect();
    if !taken.contains(base) {
        return HistoryId::new(base).expect("the static update history ID is valid");
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !taken.contains(candidate.as_str()) {
            return HistoryId::new(candidate).expect("the numbered update history ID is valid");
        }
        suffix += 1;
    }
}

fn apply_atomically<T: Clone, U>(
    target: &mut T,
    updates: &[U],
    mut apply_one: impl FnMut(&mut T, &U, &mut ReportBuilder) -> Result<(), Error>,
) -> Result<UpdateReport, Error> {
    let mut candidate = target.clone();
    let mut report = ReportBuilder::default();
    for update in updates {
        apply_one(&mut candidate, update, &mut report)?;
    }
    *target = candidate;
    Ok(report.report)
}

fn apply_atomically_with_connectivity<T: Clone, U>(
    target: &mut T,
    updates: &[U],
    mut apply_one: impl FnMut(&mut T, &U, &mut ReportBuilder) -> Result<(), Error>,
    connectivity: impl Fn(&T) -> Vec<usize>,
) -> Result<UpdateReport, Error> {
    let before = connectivity(target);
    let mut candidate = target.clone();
    let mut report = ReportBuilder::default();
    for update in updates {
        apply_one(&mut candidate, update, &mut report)?;
    }
    report.report.connectivity_changed = before != connectivity(&candidate);
    *target = candidate;
    Ok(report.report)
}

fn connectivity_partition(
    node_count: usize,
    edges: impl IntoIterator<Item = (usize, usize)>,
) -> Vec<usize> {
    fn root(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }

    let mut parent: Vec<usize> = (0..node_count).collect();
    for (left, right) in edges {
        let left = root(&mut parent, left);
        let right = root(&mut parent, right);
        if left != right {
            let (keep, replace) = if left < right {
                (left, right)
            } else {
                (right, left)
            };
            parent[replace] = keep;
        }
    }
    (0..node_count)
        .map(|node| root(&mut parent, node))
        .collect()
}

fn balanced_connectivity(
    network: &BalancedNetwork,
    branch_in_service: impl Fn(usize) -> bool,
    switch_closed: impl Fn(usize) -> bool,
) -> Vec<usize> {
    let bus_rows: std::collections::BTreeMap<_, _> = network
        .buses()
        .iter()
        .enumerate()
        .map(|(row, bus)| (bus.id, row))
        .collect();
    let mut edges = Vec::new();
    for (row, branch) in network.branches().iter().enumerate() {
        if branch_in_service(row)
            && let (Some(&from), Some(&to)) = (bus_rows.get(&branch.from), bus_rows.get(&branch.to))
        {
            edges.push((from, to));
        }
    }
    for (row, switch) in network.switches().iter().enumerate() {
        if switch_closed(row)
            && let (Some(&from), Some(&to)) = (bus_rows.get(&switch.from), bus_rows.get(&switch.to))
        {
            edges.push((from, to));
        }
    }
    for transformer in network.transformers_3w() {
        for left in 0..transformer.windings.len() {
            for right in (left + 1)..transformer.windings.len() {
                if let (Some(&from), Some(&to)) = (
                    bus_rows.get(&transformer.windings[left].bus),
                    bus_rows.get(&transformer.windings[right].bus),
                ) {
                    edges.push((from, to));
                }
            }
        }
    }
    connectivity_partition(network.buses().len(), edges)
}

fn balanced_network_connectivity(network: &BalancedNetwork) -> Vec<usize> {
    balanced_connectivity(
        network,
        |row| network.branches()[row].in_service,
        |row| network.switches()[row].closed,
    )
}

fn balanced_point_connectivity(point: &OperatingPoint<BalancedNetwork>) -> Vec<usize> {
    let network = point.network();
    balanced_connectivity(
        network,
        |row| {
            let branch = &network.branches()[row];
            let identity = row_identity(branch.uid.as_deref(), "branches", row);
            point
                .branch_in_service(&identity)
                .unwrap_or(branch.in_service)
        },
        |row| {
            let switch = &network.switches()[row];
            let identity = row_identity(switch.uid.as_deref(), "switches", row);
            point.switch_closed(&identity).unwrap_or(switch.closed)
        },
    )
}

fn multiconductor_connectivity(
    network: &MulticonductorNetwork,
    switch_closed: impl Fn(usize) -> bool,
) -> Vec<usize> {
    let bus_rows: std::collections::BTreeMap<_, _> = network
        .buses()
        .iter()
        .enumerate()
        .map(|(row, bus)| (bus.id.to_ascii_lowercase(), row))
        .collect();
    let row = |bus: &str| bus_rows.get(&bus.to_ascii_lowercase()).copied();
    let mut edges = Vec::new();
    for line in network.lines() {
        if let (Some(from), Some(to)) = (row(&line.bus_from), row(&line.bus_to)) {
            edges.push((from, to));
        }
    }
    for (index, switch) in network.switches().iter().enumerate() {
        if switch_closed(index)
            && let (Some(from), Some(to)) = (row(&switch.bus_from), row(&switch.bus_to))
        {
            edges.push((from, to));
        }
    }
    for transformer in network.transformers() {
        for left in 0..transformer.windings.len() {
            for right in (left + 1)..transformer.windings.len() {
                if let (Some(from), Some(to)) = (
                    row(&transformer.windings[left].bus),
                    row(&transformer.windings[right].bus),
                ) {
                    edges.push((from, to));
                }
            }
        }
    }
    connectivity_partition(network.buses().len(), edges)
}

fn multiconductor_network_connectivity(network: &MulticonductorNetwork) -> Vec<usize> {
    multiconductor_connectivity(network, |row| !network.switches()[row].open)
}

fn multiconductor_point_connectivity(point: &OperatingPoint<MulticonductorNetwork>) -> Vec<usize> {
    let network = point.network();
    multiconductor_connectivity(network, |row| {
        let switch = &network.switches()[row];
        point.switch_closed(&switch.name).unwrap_or(!switch.open)
    })
}

fn require_component_type(component: &ComponentId, expected: &str) -> Result<(), Error> {
    if component.component_type() == expected {
        Ok(())
    } else {
        Err(Error::new(
            &codes::VALIDATE_UPDATE_COMPONENT_TYPE,
            format!(
                "{} has component type `{}`; this field requires `{expected}`",
                component,
                component.component_type()
            ),
        ))
    }
}

fn resolve_index<T>(
    values: &[T],
    component: &ComponentId,
    expected_type: &str,
    mut local_id: impl FnMut(&T) -> Option<&str>,
) -> Result<usize, Error> {
    require_component_type(component, expected_type)?;
    let mut matches = values.iter().enumerate().filter_map(|(index, value)| {
        (local_id(value) == Some(component.local_id())).then_some(index)
    });
    let Some(index) = matches.next() else {
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_COMPONENT_UNKNOWN,
            format!("the target contains no component `{component}`"),
        ));
    };
    if matches.next().is_some() {
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_COMPONENT_AMBIGUOUS,
            format!("the target contains more than one component `{component}`"),
        ));
    }
    Ok(index)
}

fn resolve_persisted_index<T>(
    values: &[T],
    component: &ComponentId,
    expected_type: &str,
    table: &str,
    mut local_id: impl FnMut(&T) -> Option<&str>,
) -> Result<usize, Error> {
    require_component_type(component, expected_type)?;
    let row_prefix = format!("{table}:");
    if component
        .local_id()
        .strip_prefix(&row_prefix)
        .is_some_and(|row| !row.is_empty() && row.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_STABLE_ID_REQUIRED,
            format!(
                "`{}` is a table row spelling; assign a persistent component identity first",
                component.local_id()
            ),
        ));
    }
    let mut found = None;
    let mut unidentified = false;
    for (index, value) in values.iter().enumerate() {
        match local_id(value) {
            Some(local) if local == component.local_id() => {
                if found.replace(index).is_some() {
                    return Err(Error::new(
                        &codes::VALIDATE_UPDATE_COMPONENT_AMBIGUOUS,
                        format!("the target contains more than one component `{component}`"),
                    ));
                }
            }
            None => unidentified = true,
            Some(_) => {}
        }
    }
    match found {
        Some(index) => Ok(index),
        None if unidentified => Err(Error::new(
            &codes::VALIDATE_UPDATE_STABLE_ID_REQUIRED,
            format!(
                "the target contains an unidentified {expected_type}; assign persistent identities before applying updates"
            ),
        )),
        None => Err(Error::new(
            &codes::VALIDATE_UPDATE_COMPONENT_UNKNOWN,
            format!("the target contains no component `{component}`"),
        )),
    }
}

fn require_no_terminal(terminal: Option<&str>, field: &str) -> Result<(), Error> {
    if terminal.is_none() {
        Ok(())
    } else {
        Err(Error::new(
            &codes::VALIDATE_UPDATE_FIELD_UNSUPPORTED,
            format!("a balanced {field} update does not name a terminal"),
        ))
    }
}

fn unsupported(field: &str, model: &str) -> Error {
    Error::new(
        &codes::VALIDATE_UPDATE_FIELD_UNSUPPORTED,
        format!("{model} does not define {field}"),
    )
}

fn resolve_balanced_transformer(
    network: &BalancedNetwork,
    component: &ComponentId,
) -> Result<usize, Error> {
    require_component_type(component, TRANSFORMER)?;
    if component
        .local_id()
        .strip_prefix("branches:")
        .is_some_and(|row| !row.is_empty() && row.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_STABLE_ID_REQUIRED,
            format!(
                "`{}` is a table row spelling; assign a persistent component identity first",
                component.local_id()
            ),
        ));
    }
    let unidentified = network
        .branches()
        .iter()
        .any(|branch| branch.is_transformer() && branch.uid.is_none());
    let mut matches = network
        .branches()
        .iter()
        .enumerate()
        .filter_map(|(index, branch)| {
            (branch.is_transformer() && branch.uid.as_deref() == Some(component.local_id()))
                .then_some(index)
        });
    let Some(index) = matches.next() else {
        if unidentified {
            return Err(Error::new(
                &codes::VALIDATE_UPDATE_STABLE_ID_REQUIRED,
                "the target contains an unidentified transformer; assign persistent identities before applying updates",
            ));
        }
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_COMPONENT_UNKNOWN,
            format!("the target contains no transformer `{component}`"),
        ));
    };
    if matches.next().is_some() {
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_COMPONENT_AMBIGUOUS,
            format!("the target contains more than one transformer `{component}`"),
        ));
    }
    Ok(index)
}

#[allow(clippy::too_many_lines)] // exhaustive typed dispatch; splitting it would duplicate validation
fn apply_balanced_assignment(
    network: &mut BalancedNetwork,
    update: &OperatingPointUpdate,
    report: &mut ReportBuilder,
) -> Result<(), Error> {
    match update {
        OperatingPointUpdate::LoadActivePower { load, terminal, p } => {
            require_no_terminal(terminal.as_deref(), "load active power")?;
            let replacement = p.megawatts_value()?;
            let index = resolve_persisted_index(network.loads(), load, LOAD, "loads", |row| {
                row.uid.as_deref()
            })?;
            let previous = network.loads()[index].p;
            powerio_tx::__internal::edit_load_assignment(
                network,
                index,
                load,
                powerio_tx::OmittedFieldName::ActivePower,
                |row| row.p = replacement,
            );
            report.record(
                load.clone(),
                UpdatedField::LoadActivePower,
                None,
                number_changed(previous, replacement),
                false,
            )
        }
        OperatingPointUpdate::LoadReactivePower { load, terminal, q } => {
            require_no_terminal(terminal.as_deref(), "load reactive power")?;
            let replacement = q.megavars_value()?;
            let index = resolve_persisted_index(network.loads(), load, LOAD, "loads", |row| {
                row.uid.as_deref()
            })?;
            let previous = network.loads()[index].q;
            powerio_tx::__internal::edit_load_assignment(
                network,
                index,
                load,
                powerio_tx::OmittedFieldName::ReactivePower,
                |row| row.q = replacement,
            );
            report.record(
                load.clone(),
                UpdatedField::LoadReactivePower,
                None,
                number_changed(previous, replacement),
                false,
            )
        }
        OperatingPointUpdate::GeneratorActivePower {
            generator,
            terminal,
            p,
        } => {
            require_no_terminal(terminal.as_deref(), "generator active power")?;
            let replacement = p.megawatts_value()?;
            let index = resolve_persisted_index(
                network.generators(),
                generator,
                GENERATOR,
                "generators",
                |row| row.uid.as_deref(),
            )?;
            let previous = network.generators()[index].pg;
            powerio_tx::__internal::edit_generator_assignment(
                network,
                index,
                generator,
                powerio_tx::OmittedFieldName::ActivePower,
                |row| row.pg = replacement,
            );
            report.record(
                generator.clone(),
                UpdatedField::GeneratorActivePower,
                None,
                number_changed(previous, replacement),
                false,
            )
        }
        OperatingPointUpdate::GeneratorReactivePower {
            generator,
            terminal,
            q,
        } => {
            require_no_terminal(terminal.as_deref(), "generator reactive power")?;
            let replacement = q.megavars_value()?;
            let index = resolve_persisted_index(
                network.generators(),
                generator,
                GENERATOR,
                "generators",
                |row| row.uid.as_deref(),
            )?;
            let previous = network.generators()[index].qg;
            powerio_tx::__internal::edit_generator_assignment(
                network,
                index,
                generator,
                powerio_tx::OmittedFieldName::ReactivePower,
                |row| row.qg = replacement,
            );
            report.record(
                generator.clone(),
                UpdatedField::GeneratorReactivePower,
                None,
                number_changed(previous, replacement),
                false,
            )
        }
        OperatingPointUpdate::GeneratorVoltageMagnitude { generator, vm_pu } => {
            let replacement = require_positive(*vm_pu, "generator voltage magnitude")?;
            let index = resolve_persisted_index(
                network.generators(),
                generator,
                GENERATOR,
                "generators",
                |row| row.uid.as_deref(),
            )?;
            let previous = network.generators()[index].vg;
            powerio_tx::__internal::edit_generator_assignment(
                network,
                index,
                generator,
                powerio_tx::OmittedFieldName::VoltageSetpoint,
                |row| row.vg = replacement,
            );
            report.record(
                generator.clone(),
                UpdatedField::GeneratorVoltageMagnitude,
                None,
                number_changed(previous, replacement),
                false,
            )
        }
        OperatingPointUpdate::GeneratorInService {
            generator,
            in_service,
        } => {
            let index = resolve_persisted_index(
                network.generators(),
                generator,
                GENERATOR,
                "generators",
                |row| row.uid.as_deref(),
            )?;
            let previous = network.generators()[index].in_service;
            network.generators_mut()[index].in_service = *in_service;
            report.record(
                generator.clone(),
                UpdatedField::GeneratorInService,
                None,
                previous != *in_service,
                false,
            )
        }
        OperatingPointUpdate::BranchInService { branch, in_service } => {
            let index =
                resolve_persisted_index(network.branches(), branch, BRANCH, "branches", |row| {
                    row.uid.as_deref()
                })?;
            let previous = network.branches()[index].in_service;
            network.branches_mut()[index].in_service = *in_service;
            report.record(
                branch.clone(),
                UpdatedField::BranchInService,
                None,
                previous != *in_service,
                previous != *in_service,
            )
        }
        OperatingPointUpdate::TransformerTapRatio {
            transformer,
            tap_ratio,
        } => {
            let replacement = require_positive(*tap_ratio, "transformer tap ratio")?;
            let index = resolve_balanced_transformer(network, transformer)?;
            let previous = network.branches()[index].calc_effective_tap();
            network.branches_mut()[index].tap = replacement;
            report.record(
                transformer.clone(),
                UpdatedField::TransformerTapRatio,
                None,
                number_changed(previous, replacement),
                false,
            )
        }
        OperatingPointUpdate::TransformerPhaseShift {
            transformer,
            shift_degrees,
        } => {
            let replacement = require_finite(*shift_degrees, "transformer phase shift")?;
            let index = resolve_balanced_transformer(network, transformer)?;
            let previous = network.branches()[index].shift;
            network.branches_mut()[index].shift = replacement;
            report.record(
                transformer.clone(),
                UpdatedField::TransformerPhaseShift,
                None,
                number_changed(previous, replacement),
                false,
            )
        }
        OperatingPointUpdate::SwitchClosed { switch, closed } => {
            let index =
                resolve_persisted_index(network.switches(), switch, SWITCH, "switches", |row| {
                    row.uid.as_deref()
                })?;
            let previous = network.switches()[index].closed;
            network.switches_mut()[index].closed = *closed;
            report.record(
                switch.clone(),
                UpdatedField::SwitchClosed,
                None,
                previous != *closed,
                previous != *closed,
            )
        }
    }
}

fn apply_balanced_network_update(
    network: &mut BalancedNetwork,
    update: &NetworkUpdate,
    report: &mut ReportBuilder,
) -> Result<(), Error> {
    match update {
        NetworkUpdate::BranchThermalRating {
            branch,
            terminal,
            rating,
        } => {
            require_no_terminal(terminal.as_deref(), "branch thermal rating")?;
            let replacement = rating.megavolt_amperes_value()?;
            let index =
                resolve_persisted_index(network.branches(), branch, BRANCH, "branches", |row| {
                    row.uid.as_deref()
                })?;
            let previous = network.branches()[index].rate_a;
            network.branches_mut()[index].rate_a = replacement;
            report.record(
                branch.clone(),
                UpdatedField::BranchThermalRating,
                None,
                number_changed(previous, replacement),
                false,
            )
        }
    }
}

impl UpdateTarget<OperatingPointUpdate> for BalancedNetwork {
    fn apply_update_batch(
        &mut self,
        updates: &[OperatingPointUpdate],
    ) -> Result<UpdateReport, Error> {
        apply_atomically_with_connectivity(
            self,
            updates,
            apply_balanced_assignment,
            balanced_network_connectivity,
        )
    }
}

impl UpdateTarget<NetworkUpdate> for BalancedNetwork {
    fn apply_update_batch(&mut self, updates: &[NetworkUpdate]) -> Result<UpdateReport, Error> {
        apply_atomically(self, updates, apply_balanced_network_update)
    }
}

#[allow(clippy::too_many_lines)] // one exhaustive map from registered fields to network defaults
fn balanced_layout_and_defaults(
    network: &BalancedNetwork,
    quantity: &'static str,
) -> Result<(QuantityLayout, Vec<f64>), Error> {
    let (identities, defaults): (Vec<String>, Vec<f64>) = match quantity {
        LOAD_ACTIVE_POWER => network
            .loads()
            .iter()
            .enumerate()
            .map(|(row, value)| (row_identity(value.uid.as_deref(), "loads", row), value.p))
            .unzip(),
        LOAD_REACTIVE_POWER => network
            .loads()
            .iter()
            .enumerate()
            .map(|(row, value)| (row_identity(value.uid.as_deref(), "loads", row), value.q))
            .unzip(),
        GENERATOR_ACTIVE_POWER => network
            .generators()
            .iter()
            .enumerate()
            .map(|(row, value)| {
                (
                    row_identity(value.uid.as_deref(), "generators", row),
                    value.pg,
                )
            })
            .unzip(),
        GENERATOR_REACTIVE_POWER => network
            .generators()
            .iter()
            .enumerate()
            .map(|(row, value)| {
                (
                    row_identity(value.uid.as_deref(), "generators", row),
                    value.qg,
                )
            })
            .unzip(),
        GENERATOR_VOLTAGE_SETPOINT => network
            .generators()
            .iter()
            .enumerate()
            .map(|(row, value)| {
                (
                    row_identity(value.uid.as_deref(), "generators", row),
                    value.vg,
                )
            })
            .unzip(),
        GENERATOR_IN_SERVICE => network
            .generators()
            .iter()
            .enumerate()
            .map(|(row, value)| {
                (
                    row_identity(value.uid.as_deref(), "generators", row),
                    f64::from(u8::from(value.in_service)),
                )
            })
            .unzip(),
        BRANCH_IN_SERVICE | BRANCH_TAP_RATIO | BRANCH_PHASE_SHIFT => network
            .branches()
            .iter()
            .enumerate()
            .map(|(row, value)| {
                let default = match quantity {
                    BRANCH_IN_SERVICE => f64::from(u8::from(value.in_service)),
                    BRANCH_TAP_RATIO => value.calc_effective_tap(),
                    BRANCH_PHASE_SHIFT => value.shift,
                    _ => unreachable!(),
                };
                (row_identity(value.uid.as_deref(), "branches", row), default)
            })
            .unzip(),
        SWITCH_CLOSED => network
            .switches()
            .iter()
            .enumerate()
            .map(|(row, value)| {
                (
                    row_identity(value.uid.as_deref(), "switches", row),
                    f64::from(u8::from(value.closed)),
                )
            })
            .unzip(),
        _ => unreachable!("the update names a registered balanced quantity"),
    };
    Ok((QuantityLayout::from_order(quantity, identities)?, defaults))
}

fn replace_balanced_point_value(
    point: &mut OperatingPoint<BalancedNetwork>,
    quantity: &'static str,
    identity: &str,
    replacement: f64,
) -> Result<bool, Error> {
    let (layout, defaults) = balanced_layout_and_defaults(point.network(), quantity)?;
    point.replace_value(quantity, layout, &defaults, identity, replacement)
}

#[allow(clippy::too_many_lines)] // exhaustive typed dispatch; splitting it would duplicate validation
fn apply_balanced_point_update(
    point: &mut OperatingPoint<BalancedNetwork>,
    update: &OperatingPointUpdate,
    report: &mut ReportBuilder,
) -> Result<(), Error> {
    let (component, field, quantity, replacement, connectivity) = match update {
        OperatingPointUpdate::LoadActivePower { load, terminal, p } => {
            require_no_terminal(terminal.as_deref(), "load active power")?;
            resolve_persisted_index(point.network().loads(), load, LOAD, "loads", |row| {
                row.uid.as_deref()
            })?;
            (
                load,
                UpdatedField::LoadActivePower,
                LOAD_ACTIVE_POWER,
                p.megawatts_value()?,
                false,
            )
        }
        OperatingPointUpdate::LoadReactivePower { load, terminal, q } => {
            require_no_terminal(terminal.as_deref(), "load reactive power")?;
            resolve_persisted_index(point.network().loads(), load, LOAD, "loads", |row| {
                row.uid.as_deref()
            })?;
            (
                load,
                UpdatedField::LoadReactivePower,
                LOAD_REACTIVE_POWER,
                q.megavars_value()?,
                false,
            )
        }
        OperatingPointUpdate::GeneratorActivePower {
            generator,
            terminal,
            p,
        } => {
            require_no_terminal(terminal.as_deref(), "generator active power")?;
            resolve_persisted_index(
                point.network().generators(),
                generator,
                GENERATOR,
                "generators",
                |row| row.uid.as_deref(),
            )?;
            (
                generator,
                UpdatedField::GeneratorActivePower,
                GENERATOR_ACTIVE_POWER,
                p.megawatts_value()?,
                false,
            )
        }
        OperatingPointUpdate::GeneratorReactivePower {
            generator,
            terminal,
            q,
        } => {
            require_no_terminal(terminal.as_deref(), "generator reactive power")?;
            resolve_persisted_index(
                point.network().generators(),
                generator,
                GENERATOR,
                "generators",
                |row| row.uid.as_deref(),
            )?;
            (
                generator,
                UpdatedField::GeneratorReactivePower,
                GENERATOR_REACTIVE_POWER,
                q.megavars_value()?,
                false,
            )
        }
        OperatingPointUpdate::GeneratorVoltageMagnitude { generator, vm_pu } => {
            resolve_persisted_index(
                point.network().generators(),
                generator,
                GENERATOR,
                "generators",
                |row| row.uid.as_deref(),
            )?;
            (
                generator,
                UpdatedField::GeneratorVoltageMagnitude,
                GENERATOR_VOLTAGE_SETPOINT,
                require_positive(*vm_pu, "generator voltage magnitude")?,
                false,
            )
        }
        OperatingPointUpdate::GeneratorInService {
            generator,
            in_service,
        } => {
            resolve_persisted_index(
                point.network().generators(),
                generator,
                GENERATOR,
                "generators",
                |row| row.uid.as_deref(),
            )?;
            (
                generator,
                UpdatedField::GeneratorInService,
                GENERATOR_IN_SERVICE,
                f64::from(u8::from(*in_service)),
                false,
            )
        }
        OperatingPointUpdate::BranchInService { branch, in_service } => {
            resolve_persisted_index(
                point.network().branches(),
                branch,
                BRANCH,
                "branches",
                |row| row.uid.as_deref(),
            )?;
            (
                branch,
                UpdatedField::BranchInService,
                BRANCH_IN_SERVICE,
                f64::from(u8::from(*in_service)),
                true,
            )
        }
        OperatingPointUpdate::TransformerTapRatio {
            transformer,
            tap_ratio,
        } => {
            resolve_balanced_transformer(point.network(), transformer)?;
            (
                transformer,
                UpdatedField::TransformerTapRatio,
                BRANCH_TAP_RATIO,
                require_positive(*tap_ratio, "transformer tap ratio")?,
                false,
            )
        }
        OperatingPointUpdate::TransformerPhaseShift {
            transformer,
            shift_degrees,
        } => {
            resolve_balanced_transformer(point.network(), transformer)?;
            (
                transformer,
                UpdatedField::TransformerPhaseShift,
                BRANCH_PHASE_SHIFT,
                require_finite(*shift_degrees, "transformer phase shift")?,
                false,
            )
        }
        OperatingPointUpdate::SwitchClosed { switch, closed } => {
            resolve_persisted_index(
                point.network().switches(),
                switch,
                SWITCH,
                "switches",
                |row| row.uid.as_deref(),
            )?;
            (
                switch,
                UpdatedField::SwitchClosed,
                SWITCH_CLOSED,
                f64::from(u8::from(*closed)),
                true,
            )
        }
    };
    let changed = replace_balanced_point_value(point, quantity, component.local_id(), replacement)?;
    report.record(
        component.clone(),
        field,
        None,
        changed,
        changed && connectivity,
    )
}

impl UpdateTarget<OperatingPointUpdate> for OperatingPoint<BalancedNetwork> {
    fn apply_update_batch(
        &mut self,
        updates: &[OperatingPointUpdate],
    ) -> Result<UpdateReport, Error> {
        apply_atomically_with_connectivity(
            self,
            updates,
            apply_balanced_point_update,
            balanced_point_connectivity,
        )
    }
}

fn require_terminal<'a>(terminal: Option<&'a str>, field: &str) -> Result<&'a str, Error> {
    terminal.ok_or_else(|| {
        Error::new(
            &codes::VALIDATE_UPDATE_TERMINAL_UNKNOWN,
            format!("a conductor resolved {field} update must name a terminal"),
        )
    })
}

fn resolve_terminal(
    terminals: &[String],
    terminal: &str,
    component: &ComponentId,
) -> Result<usize, Error> {
    let mut matches = terminals
        .iter()
        .enumerate()
        .filter_map(|(index, name)| (name == terminal).then_some(index));
    let Some(index) = matches.next() else {
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_TERMINAL_UNKNOWN,
            format!("{component} has no terminal `{terminal}`"),
        ));
    };
    if matches.next().is_some() {
        return Err(Error::new(
            &codes::VALIDATE_UPDATE_TERMINAL_UNKNOWN,
            format!("{component} repeats terminal `{terminal}`"),
        ));
    }
    Ok(index)
}

fn resolve_power_terminal(
    terminals: &[String],
    value_count: usize,
    terminal: &str,
    component: &ComponentId,
    field: &str,
) -> Result<usize, Error> {
    if value_count > terminals.len() {
        return Err(Error::new(
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
            format!(
                "{component} has {value_count} {field} values for {} declared terminals",
                terminals.len()
            ),
        ));
    }
    resolve_terminal(&terminals[..value_count], terminal, component)
}

fn replace_terminal_value(
    values: &mut [f64],
    terminal_count: usize,
    terminal: usize,
    replacement: f64,
    component: &ComponentId,
    field: &str,
) -> Result<bool, Error> {
    if values.len() != terminal_count {
        return Err(Error::new(
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
            format!(
                "{component} has {} {field} values for {terminal_count} terminals",
                values.len()
            ),
        ));
    }
    let previous = values[terminal];
    values[terminal] = replacement;
    Ok(number_changed(previous, replacement))
}

#[allow(clippy::too_many_lines)] // exhaustive typed dispatch; splitting it would duplicate validation
fn apply_multiconductor_assignment(
    network: &mut MulticonductorNetwork,
    update: &OperatingPointUpdate,
    report: &mut ReportBuilder,
) -> Result<(), Error> {
    match update {
        OperatingPointUpdate::LoadActivePower { load, terminal, p } => {
            let terminal = require_terminal(terminal.as_deref(), "load active power")?;
            let replacement = p.watts_value()?;
            let index = resolve_index(network.loads(), load, LOAD, |row| Some(row.name.as_str()))?;
            let terminal_count = network.loads()[index].p_nom.len();
            let terminal_index = resolve_power_terminal(
                &network.loads()[index].terminal_map,
                terminal_count,
                terminal,
                load,
                "active power",
            )?;
            let changed = replace_terminal_value(
                &mut network.loads_mut()[index].p_nom,
                terminal_count,
                terminal_index,
                replacement,
                load,
                "active power",
            )?;
            report.record(
                load.clone(),
                UpdatedField::LoadActivePower,
                Some(terminal),
                changed,
                false,
            )
        }
        OperatingPointUpdate::LoadReactivePower { load, terminal, q } => {
            let terminal = require_terminal(terminal.as_deref(), "load reactive power")?;
            let replacement = q.vars_value()?;
            let index = resolve_index(network.loads(), load, LOAD, |row| Some(row.name.as_str()))?;
            let terminal_count = network.loads()[index].q_nom.len();
            let terminal_index = resolve_power_terminal(
                &network.loads()[index].terminal_map,
                terminal_count,
                terminal,
                load,
                "reactive power",
            )?;
            let changed = replace_terminal_value(
                &mut network.loads_mut()[index].q_nom,
                terminal_count,
                terminal_index,
                replacement,
                load,
                "reactive power",
            )?;
            report.record(
                load.clone(),
                UpdatedField::LoadReactivePower,
                Some(terminal),
                changed,
                false,
            )
        }
        OperatingPointUpdate::GeneratorActivePower {
            generator,
            terminal,
            p,
        } => {
            let terminal = require_terminal(terminal.as_deref(), "generator active power")?;
            let replacement = p.watts_value()?;
            let index = resolve_index(network.generators(), generator, GENERATOR, |row| {
                Some(row.name.as_str())
            })?;
            let terminal_count = network.generators()[index].p_nom.len();
            let terminal_index = resolve_power_terminal(
                &network.generators()[index].terminal_map,
                terminal_count,
                terminal,
                generator,
                "active power",
            )?;
            let changed = replace_terminal_value(
                &mut network.generators_mut()[index].p_nom,
                terminal_count,
                terminal_index,
                replacement,
                generator,
                "active power",
            )?;
            report.record(
                generator.clone(),
                UpdatedField::GeneratorActivePower,
                Some(terminal),
                changed,
                false,
            )
        }
        OperatingPointUpdate::GeneratorReactivePower {
            generator,
            terminal,
            q,
        } => {
            let terminal = require_terminal(terminal.as_deref(), "generator reactive power")?;
            let replacement = q.vars_value()?;
            let index = resolve_index(network.generators(), generator, GENERATOR, |row| {
                Some(row.name.as_str())
            })?;
            let terminal_count = network.generators()[index].q_nom.len();
            let terminal_index = resolve_power_terminal(
                &network.generators()[index].terminal_map,
                terminal_count,
                terminal,
                generator,
                "reactive power",
            )?;
            let changed = replace_terminal_value(
                &mut network.generators_mut()[index].q_nom,
                terminal_count,
                terminal_index,
                replacement,
                generator,
                "reactive power",
            )?;
            report.record(
                generator.clone(),
                UpdatedField::GeneratorReactivePower,
                Some(terminal),
                changed,
                false,
            )
        }
        OperatingPointUpdate::SwitchClosed { switch, closed } => {
            let index = resolve_index(network.switches(), switch, SWITCH, |row| {
                Some(row.name.as_str())
            })?;
            let previous = !network.switches()[index].open;
            network.switches_mut()[index].open = !*closed;
            report.record(
                switch.clone(),
                UpdatedField::SwitchClosed,
                None,
                previous != *closed,
                previous != *closed,
            )
        }
        OperatingPointUpdate::GeneratorVoltageMagnitude { .. } => Err(unsupported(
            "a generator voltage setpoint",
            "MulticonductorNetwork",
        )),
        OperatingPointUpdate::GeneratorInService { .. } => Err(unsupported(
            "generator service status",
            "MulticonductorNetwork",
        )),
        OperatingPointUpdate::BranchInService { .. } => Err(unsupported(
            "branch service status",
            "MulticonductorNetwork",
        )),
        OperatingPointUpdate::TransformerTapRatio { .. } => Err(unsupported(
            "one transformer tap ratio without a winding number",
            "MulticonductorNetwork",
        )),
        OperatingPointUpdate::TransformerPhaseShift { .. } => Err(unsupported(
            "a transformer phase shift",
            "MulticonductorNetwork",
        )),
    }
}

fn apply_multiconductor_network_update(
    network: &mut MulticonductorNetwork,
    update: &NetworkUpdate,
    report: &mut ReportBuilder,
) -> Result<(), Error> {
    match update {
        NetworkUpdate::BranchThermalRating {
            branch,
            terminal,
            rating,
        } => {
            require_component_type(branch, LINE)?;
            let terminal = require_terminal(terminal.as_deref(), "line thermal rating")?;
            let replacement = rating.volt_amperes_value()?;
            let index =
                resolve_index(network.lines(), branch, LINE, |row| Some(row.name.as_str()))?;
            let terminal_index =
                resolve_terminal(&network.lines()[index].terminal_map_from, terminal, branch)?;
            let terminal_count = network.lines()[index].terminal_map_from.len();
            if network.lines()[index]
                .s_max
                .as_ref()
                .is_some_and(|values| values.len() != terminal_count)
            {
                return Err(Error::new(
                    &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                    format!(
                        "{branch} has a thermal rating count that differs from its terminal count"
                    ),
                ));
            }
            let values = network.lines_mut()[index]
                .s_max
                .get_or_insert_with(|| vec![f64::INFINITY; terminal_count]);
            let previous = values[terminal_index];
            values[terminal_index] = replacement;
            report.record(
                branch.clone(),
                UpdatedField::BranchThermalRating,
                Some(terminal),
                number_changed(previous, replacement),
                false,
            )
        }
    }
}

impl UpdateTarget<OperatingPointUpdate> for MulticonductorNetwork {
    fn apply_update_batch(
        &mut self,
        updates: &[OperatingPointUpdate],
    ) -> Result<UpdateReport, Error> {
        apply_atomically_with_connectivity(
            self,
            updates,
            apply_multiconductor_assignment,
            multiconductor_network_connectivity,
        )
    }
}

impl UpdateTarget<NetworkUpdate> for MulticonductorNetwork {
    fn apply_update_batch(&mut self, updates: &[NetworkUpdate]) -> Result<UpdateReport, Error> {
        apply_atomically(self, updates, apply_multiconductor_network_update)
    }
}

fn multiconductor_layout_and_defaults(
    network: &MulticonductorNetwork,
    quantity: &'static str,
) -> Result<(QuantityLayout, Vec<f64>), Error> {
    let (identities, defaults): (Vec<String>, Vec<f64>) = match quantity {
        MC_LOAD_ACTIVE_POWER | MC_LOAD_REACTIVE_POWER => network
            .loads()
            .iter()
            .flat_map(|load| {
                load.terminal_map
                    .iter()
                    .enumerate()
                    .map(move |(index, terminal)| {
                        let values = if quantity == MC_LOAD_ACTIVE_POWER {
                            &load.p_nom
                        } else {
                            &load.q_nom
                        };
                        (
                            format!("{}/{terminal}", load.name),
                            values.get(index).copied().unwrap_or(0.0),
                        )
                    })
            })
            .unzip(),
        MC_GENERATOR_ACTIVE_POWER | MC_GENERATOR_REACTIVE_POWER => network
            .generators()
            .iter()
            .flat_map(|generator| {
                generator
                    .terminal_map
                    .iter()
                    .enumerate()
                    .map(move |(index, terminal)| {
                        let values = if quantity == MC_GENERATOR_ACTIVE_POWER {
                            &generator.p_nom
                        } else {
                            &generator.q_nom
                        };
                        (
                            format!("{}/{terminal}", generator.name),
                            values.get(index).copied().unwrap_or(0.0),
                        )
                    })
            })
            .unzip(),
        MC_SWITCH_CLOSED => network
            .switches()
            .iter()
            .map(|switch| (switch.name.clone(), f64::from(u8::from(!switch.open))))
            .unzip(),
        _ => unreachable!("the update names a registered multiconductor quantity"),
    };
    if defaults.iter().any(|value| !value.is_finite()) {
        return Err(Error::new(
            &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
            format!("{quantity} defaults do not align with the component terminals"),
        ));
    }
    Ok((QuantityLayout::from_order(quantity, identities)?, defaults))
}

fn replace_multiconductor_point_value(
    point: &mut OperatingPoint<MulticonductorNetwork>,
    quantity: &'static str,
    identity: &str,
    replacement: f64,
) -> Result<bool, Error> {
    let (layout, defaults) = multiconductor_layout_and_defaults(point.network(), quantity)?;
    point.replace_value(quantity, layout, &defaults, identity, replacement)
}

#[allow(clippy::too_many_lines)] // exhaustive typed dispatch; splitting it would duplicate validation
fn apply_multiconductor_point_update(
    point: &mut OperatingPoint<MulticonductorNetwork>,
    update: &OperatingPointUpdate,
    report: &mut ReportBuilder,
) -> Result<(), Error> {
    let (component, terminal, field, quantity, replacement, connectivity) = match update {
        OperatingPointUpdate::LoadActivePower { load, terminal, p } => {
            let terminal = require_terminal(terminal.as_deref(), "load active power")?;
            let index = resolve_index(point.network().loads(), load, LOAD, |row| {
                Some(row.name.as_str())
            })?;
            resolve_power_terminal(
                &point.network().loads()[index].terminal_map,
                point.network().loads()[index].p_nom.len(),
                terminal,
                load,
                "active power",
            )?;
            (
                load,
                Some(terminal),
                UpdatedField::LoadActivePower,
                MC_LOAD_ACTIVE_POWER,
                p.watts_value()?,
                false,
            )
        }
        OperatingPointUpdate::LoadReactivePower { load, terminal, q } => {
            let terminal = require_terminal(terminal.as_deref(), "load reactive power")?;
            let index = resolve_index(point.network().loads(), load, LOAD, |row| {
                Some(row.name.as_str())
            })?;
            resolve_power_terminal(
                &point.network().loads()[index].terminal_map,
                point.network().loads()[index].q_nom.len(),
                terminal,
                load,
                "reactive power",
            )?;
            (
                load,
                Some(terminal),
                UpdatedField::LoadReactivePower,
                MC_LOAD_REACTIVE_POWER,
                q.vars_value()?,
                false,
            )
        }
        OperatingPointUpdate::GeneratorActivePower {
            generator,
            terminal,
            p,
        } => {
            let terminal = require_terminal(terminal.as_deref(), "generator active power")?;
            let index = resolve_index(point.network().generators(), generator, GENERATOR, |row| {
                Some(row.name.as_str())
            })?;
            resolve_power_terminal(
                &point.network().generators()[index].terminal_map,
                point.network().generators()[index].p_nom.len(),
                terminal,
                generator,
                "active power",
            )?;
            (
                generator,
                Some(terminal),
                UpdatedField::GeneratorActivePower,
                MC_GENERATOR_ACTIVE_POWER,
                p.watts_value()?,
                false,
            )
        }
        OperatingPointUpdate::GeneratorReactivePower {
            generator,
            terminal,
            q,
        } => {
            let terminal = require_terminal(terminal.as_deref(), "generator reactive power")?;
            let index = resolve_index(point.network().generators(), generator, GENERATOR, |row| {
                Some(row.name.as_str())
            })?;
            resolve_power_terminal(
                &point.network().generators()[index].terminal_map,
                point.network().generators()[index].q_nom.len(),
                terminal,
                generator,
                "reactive power",
            )?;
            (
                generator,
                Some(terminal),
                UpdatedField::GeneratorReactivePower,
                MC_GENERATOR_REACTIVE_POWER,
                q.vars_value()?,
                false,
            )
        }
        OperatingPointUpdate::SwitchClosed { switch, closed } => {
            resolve_index(point.network().switches(), switch, SWITCH, |row| {
                Some(row.name.as_str())
            })?;
            (
                switch,
                None,
                UpdatedField::SwitchClosed,
                MC_SWITCH_CLOSED,
                f64::from(u8::from(*closed)),
                true,
            )
        }
        OperatingPointUpdate::GeneratorVoltageMagnitude { .. } => {
            return Err(unsupported(
                "a generator voltage setpoint",
                "OperatingPoint<MulticonductorNetwork>",
            ));
        }
        OperatingPointUpdate::GeneratorInService { .. } => {
            return Err(unsupported(
                "generator service status",
                "OperatingPoint<MulticonductorNetwork>",
            ));
        }
        OperatingPointUpdate::BranchInService { .. } => {
            return Err(unsupported(
                "branch service status",
                "OperatingPoint<MulticonductorNetwork>",
            ));
        }
        OperatingPointUpdate::TransformerTapRatio { .. } => {
            return Err(unsupported(
                "one transformer tap ratio without a winding number",
                "OperatingPoint<MulticonductorNetwork>",
            ));
        }
        OperatingPointUpdate::TransformerPhaseShift { .. } => {
            return Err(unsupported(
                "a transformer phase shift",
                "OperatingPoint<MulticonductorNetwork>",
            ));
        }
    };
    let identity = match terminal {
        Some(terminal) => format!("{}/{terminal}", component.local_id()),
        None => component.local_id().to_owned(),
    };
    let changed = replace_multiconductor_point_value(point, quantity, &identity, replacement)?;
    report.record(
        component.clone(),
        field,
        terminal,
        changed,
        changed && connectivity,
    )
}

impl UpdateTarget<OperatingPointUpdate> for OperatingPoint<MulticonductorNetwork> {
    fn apply_update_batch(
        &mut self,
        updates: &[OperatingPointUpdate],
    ) -> Result<UpdateReport, Error> {
        apply_atomically_with_connectivity(
            self,
            updates,
            apply_multiconductor_point_update,
            multiconductor_point_connectivity,
        )
    }
}

macro_rules! impl_balanced_calculation_updates {
    ($($instance:ty),+ $(,)?) => {
        $(
            impl UpdateTarget<CalculationUpdate> for $instance {
                fn apply_update_batch(
                    &mut self,
                    updates: &[CalculationUpdate],
                ) -> Result<UpdateReport, Error> {
                    if updates.is_empty() {
                        return Ok(UpdateReport::default());
                    }
                    let candidate = self.clone();
                    let before_connectivity = balanced_network_connectivity(candidate.network());
                    let mut network = candidate.network().clone();
                    let mut report = ReportBuilder::default();
                    for update in updates {
                        match update {
                            CalculationUpdate::OperatingPoint(update) => {
                                apply_balanced_assignment(&mut network, update, &mut report)?;
                            }
                            CalculationUpdate::Network(update) => {
                                apply_balanced_network_update(&mut network, update, &mut report)?;
                            }
                        }
                    }
                    report.report.connectivity_changed =
                        before_connectivity != balanced_network_connectivity(&network);
                    let candidate = candidate.with_network(network)?;
                    *self = candidate;
                    Ok(report.report)
                }
            }
        )+
    };
}

impl_balanced_calculation_updates!(DcPfInstance, AcPfInstance, DcOpfInstance, AcOpfInstance);

macro_rules! impl_multiconductor_calculation_updates {
    ($($instance:ty),+ $(,)?) => {
        $(
            impl UpdateTarget<CalculationUpdate> for $instance {
                fn apply_update_batch(
                    &mut self,
                    updates: &[CalculationUpdate],
                ) -> Result<UpdateReport, Error> {
                    if updates.is_empty() {
                        return Ok(UpdateReport::default());
                    }
                    let candidate = self.clone();
                    let before_connectivity =
                        multiconductor_network_connectivity(candidate.network());
                    let mut network = candidate.network().clone();
                    let mut report = ReportBuilder::default();
                    for update in updates {
                        match update {
                            CalculationUpdate::OperatingPoint(update) => {
                                apply_multiconductor_assignment(&mut network, update, &mut report)?;
                            }
                            CalculationUpdate::Network(update) => {
                                apply_multiconductor_network_update(&mut network, update, &mut report)?;
                            }
                        }
                    }
                    report.report.connectivity_changed =
                        before_connectivity != multiconductor_network_connectivity(&network);
                    let candidate = candidate.with_network(network)?;
                    *self = candidate;
                    Ok(report.report)
                }
            }
        )+
    };
}

impl_multiconductor_calculation_updates!(McAcPfInstance, McAcOpfInstance);

#[cfg(test)]
mod tests {
    use powerio_core::{FormatId, Source};
    use powerio_dist::{DistBus, DistSwitch};
    use powerio_tx::{
        Branch, Bus, BusId, BusType, DetailedConnectivity, Load, OmittedField, OmittedFieldName,
        Switch,
    };

    use super::*;
    use crate::BalancedOperatingPointBuilder;

    fn component(component_type: &str, local_id: &str) -> ComponentId {
        ComponentId::new(component_type, local_id).unwrap()
    }

    fn assert_number_eq(actual: f64, expected: f64) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }

    fn balanced_network() -> BalancedNetwork {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
        let mut network = powerio_tx::parse(Source::open(path).unwrap())
            .unwrap()
            .into_value();
        for (index, load) in network.loads_mut().iter_mut().enumerate() {
            load.uid = Some(format!("load-{index}"));
        }
        for (index, generator) in network.generators_mut().iter_mut().enumerate() {
            generator.uid = Some(format!("generator-{index}"));
        }
        for (index, branch) in network.branches_mut().iter_mut().enumerate() {
            branch.uid = Some(format!("branch-{index}"));
        }
        network.branches_mut()[0].tap = 1.0;
        network.branches_mut()[0].uid = Some("transformer-0".to_owned());
        let mut switch = Switch::new(network.buses()[0].id, network.buses()[1].id, true);
        switch.uid = Some("switch-0".to_owned());
        network.switches_mut().push(switch);
        network
    }

    fn multiconductor_network() -> MulticonductorNetwork {
        let dss = "New Circuit.c basekv=12.47 pu=1 phases=3 bus1=a\n\
                   New Line.l1 bus1=a.1.2.3 bus2=b.1.2.3 phases=3 r1=0.1 x1=0.2 length=1 units=km\n\
                   New Load.ld bus1=b.1.2.3 phases=3 conn=wye kv=7.2 kw=30 kvar=9\n\
                   New Generator.g bus1=b.1.2.3 phases=3 conn=wye kv=7.2 kw=12 kvar=3\n";
        let source = Source::from_memory("<memory>", dss.as_bytes().to_vec())
            .unwrap()
            .with_format(FormatId::new("dss").unwrap());
        let mut network = powerio_dist::parse(source).unwrap().into_value();
        network.switches_mut().push(DistSwitch::new(
            "sw",
            "a",
            "b",
            vec!["1".to_owned()],
            vec!["1".to_owned()],
            false,
        ));
        network
    }

    #[test]
    fn invalid_batch_leaves_the_network_unchanged() {
        let mut network = balanced_network();
        let previous = network.loads()[0].p;
        let error = apply_updates(
            &mut network,
            &[
                OperatingPointUpdate::LoadActivePower {
                    load: component(LOAD, "load-0"),
                    terminal: None,
                    p: ActivePower::from_megawatts(previous + 10.0),
                },
                OperatingPointUpdate::GeneratorActivePower {
                    generator: component(GENERATOR, "missing"),
                    terminal: None,
                    p: ActivePower::from_megawatts(1.0),
                },
            ],
        )
        .unwrap_err();
        assert_eq!(
            error.info().unwrap().code,
            "VALIDATE.UPDATE.COMPONENT_UNKNOWN"
        );
        assert_number_eq(network.loads()[0].p, previous);
    }

    #[test]
    fn balanced_update_removes_only_the_exact_source_omissions() {
        let mut network = balanced_network();
        let load = component(LOAD, "load-0");
        let mut second_load = network.loads()[0].clone();
        second_load.uid = Some("load-1".to_owned());
        network.loads_mut().push(second_load);
        let second_load = component(LOAD, "load-1");
        let generator = component(GENERATOR, "generator-0");
        let mut detailed = DetailedConnectivity::default();
        detailed.omitted_fields = vec![
            OmittedField::new(load.clone(), OmittedFieldName::ActivePower),
            OmittedField::new(load.clone(), OmittedFieldName::ReactivePower),
            OmittedField::new(second_load.clone(), OmittedFieldName::ActivePower),
            OmittedField::new(generator.clone(), OmittedFieldName::ActivePower),
            OmittedField::new(generator.clone(), OmittedFieldName::ReactivePower),
            OmittedField::new(generator.clone(), OmittedFieldName::VoltageSetpoint),
        ];
        *network.detailed_connectivity_mut() = Some(std::sync::Arc::new(detailed));

        apply_updates(
            &mut network,
            &[
                OperatingPointUpdate::LoadActivePower {
                    load: load.clone(),
                    terminal: None,
                    p: ActivePower::from_megawatts(12.0),
                },
                OperatingPointUpdate::GeneratorReactivePower {
                    generator: generator.clone(),
                    terminal: None,
                    q: ReactivePower::from_megavars(3.0),
                },
            ],
        )
        .unwrap();

        let omitted = &network
            .detailed_connectivity()
            .as_deref()
            .unwrap()
            .omitted_fields;
        assert!(!omitted.contains(&OmittedField::new(
            load.clone(),
            OmittedFieldName::ActivePower,
        )));
        assert!(!omitted.contains(&OmittedField::new(
            generator.clone(),
            OmittedFieldName::ReactivePower,
        )));
        assert!(omitted.contains(&OmittedField::new(load, OmittedFieldName::ReactivePower,)));
        assert!(omitted.contains(&OmittedField::new(
            second_load,
            OmittedFieldName::ActivePower,
        )));
        assert!(omitted.contains(&OmittedField::new(
            generator.clone(),
            OmittedFieldName::ActivePower,
        )));
        assert!(omitted.contains(&OmittedField::new(
            generator,
            OmittedFieldName::VoltageSetpoint,
        )));
        assert_number_eq(network.loads()[0].p, 12.0);
        assert_number_eq(network.generators()[0].qg, 3.0);
    }

    #[test]
    fn module_update_records_exact_changes_and_invalidates_retained_bytes() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
        let mut module = powerio_tx::parse(Source::open(path).unwrap()).unwrap();
        let load_id = module.value().loads()[0].uid.clone().unwrap();
        let previous = module.value().loads()[0].p;
        assert!(module.source().is_some());

        let update = OperatingPointUpdate::LoadActivePower {
            load: component(LOAD, &load_id),
            terminal: None,
            p: ActivePower::from_megawatts(previous + 1.0),
        };
        let report = apply_updates(&mut module, std::slice::from_ref(&update)).unwrap();
        assert_eq!(report.changes().len(), 1);
        assert_number_eq(module.value().loads()[0].p, previous + 1.0);
        assert!(module.source().is_none());
        assert_eq!(module.producer().name(), "powerio");
        assert_eq!(module.history().len(), 1);
        assert_eq!(module.history()[0].kind(), HistoryKind::Edit);
        assert_eq!(module.history()[0].name(), "apply_updates");
        assert_eq!(
            module.history()[0].parameters()["updates"],
            serde_json::to_value([&update]).unwrap()
        );
        assert_eq!(
            module.history()[0].parameters()["changes"],
            serde_json::to_value(report.changes()).unwrap()
        );

        apply_updates(
            &mut module,
            &[OperatingPointUpdate::LoadActivePower {
                load: component(LOAD, &load_id),
                terminal: None,
                p: ActivePower::from_megawatts(previous + 2.0),
            }],
        )
        .unwrap();
        assert_eq!(module.history()[1].id().as_str(), "apply-updates-2");
    }

    #[test]
    fn no_op_module_update_keeps_retained_bytes_and_history() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
        let mut module = powerio_tx::parse(Source::open(path).unwrap()).unwrap();
        let load_id = module.value().loads()[0].uid.clone().unwrap();
        let current = module.value().loads()[0].p;
        let report = apply_updates(
            &mut module,
            &[OperatingPointUpdate::LoadActivePower {
                load: component(LOAD, &load_id),
                terminal: None,
                p: ActivePower::from_megawatts(current),
            }],
        )
        .unwrap();
        assert!(report.is_empty());
        assert!(module.source().is_some());
        assert!(module.history().is_empty());
    }

    #[test]
    fn load_updates_require_a_load_identity() {
        let mut network = balanced_network();
        let previous = network.loads()[0].p;
        let bus_id = network.loads()[0].bus.0.to_string();
        let error = apply_updates(
            &mut network,
            &[OperatingPointUpdate::LoadActivePower {
                load: component("bus", &bus_id),
                terminal: None,
                p: ActivePower::from_megawatts(99.0),
            }],
        )
        .unwrap_err();
        assert_eq!(error.info().unwrap().code, "VALIDATE.UPDATE.COMPONENT_TYPE");
        assert_number_eq(network.loads()[0].p, previous);
    }

    #[test]
    fn bus_load_update_uses_the_explicit_allocation_rule() {
        let mut network = balanced_network();
        let bus = network.loads()[0].bus;
        network.loads_mut()[0].uid = Some("load-a".to_owned());
        network.loads_mut()[0].p = 30.0;

        let mut second = Load::new(bus, 10.0, 2.0);
        second.uid = Some("load-b".to_owned());
        network.loads_mut().push(second);

        let mut out_of_service = Load::new(bus, 70.0, 14.0);
        out_of_service.uid = Some("load-off".to_owned());
        out_of_service.in_service = false;
        network.loads_mut().push(out_of_service);

        let instance = DcOpfInstance::from_network(network).unwrap();
        let mut module = PioModule::new(instance);
        let report = apply_bus_load_active_power(
            &mut module,
            bus,
            ActivePower::from_megawatts(100.0),
            LoadAllocation::ProportionalToCurrentActivePower,
        )
        .unwrap();

        let network = module.value().network();
        let active_power = |uid: &str| {
            network
                .loads()
                .iter()
                .find(|load| load.uid.as_deref() == Some(uid))
                .unwrap()
                .p
        };
        assert_number_eq(active_power("load-a"), 75.0);
        assert_number_eq(active_power("load-b"), 25.0);
        assert_number_eq(active_power("load-off"), 70.0);
        assert_eq!(report.changes().len(), 2);
        assert!(
            report
                .changes()
                .iter()
                .all(|change| change.field() == UpdatedField::LoadActivePower)
        );
        assert_eq!(module.history().len(), 1);
        assert_eq!(module.history()[0].name(), "apply_updates");
    }

    #[test]
    fn bus_load_update_refuses_an_undefined_proportional_basis() {
        let mut network = balanced_network();
        let bus = network.loads()[0].bus;
        network.loads_mut()[0].p = 0.0;
        let instance = DcOpfInstance::from_network(network).unwrap();
        let mut module = PioModule::new(instance);
        let error = apply_bus_load_active_power(
            &mut module,
            bus,
            ActivePower::from_megawatts(20.0),
            LoadAllocation::ProportionalToCurrentActivePower,
        )
        .unwrap_err();
        assert_eq!(error.info().unwrap().code, "VALIDATE.UPDATE.VALUE_INVALID");
        assert_number_eq(module.value().network().loads()[0].p, 0.0);
        assert!(module.history().is_empty());
    }

    #[test]
    fn equal_bus_load_allocation_restores_zero_demand() {
        let mut network = balanced_network();
        let bus = network.loads()[0].bus;
        network.loads_mut()[0].uid = Some("load-a".to_owned());
        network.loads_mut()[0].p = 0.0;

        let mut second = Load::new(bus, 0.0, 0.0);
        second.uid = Some("load-b".to_owned());
        network.loads_mut().push(second);

        let instance = DcOpfInstance::from_network(network).unwrap();
        let mut module = PioModule::new(instance);
        let report = apply_bus_load_active_power(
            &mut module,
            bus,
            ActivePower::from_megawatts(20.0),
            LoadAllocation::Equal,
        )
        .unwrap();

        let active_power = module
            .value()
            .network()
            .loads()
            .iter()
            .filter(|load| load.in_service && load.bus == bus)
            .map(|load| load.p)
            .collect::<Vec<_>>();
        assert_eq!(active_power, vec![10.0, 10.0]);
        assert_eq!(report.changes().len(), 2);
    }

    #[test]
    fn row_spellings_and_unidentified_records_are_not_update_targets() {
        let mut network = balanced_network();
        let previous = network.loads()[0].p;
        let error = apply_updates(
            &mut network,
            &[OperatingPointUpdate::LoadActivePower {
                load: component(LOAD, "loads:0"),
                terminal: None,
                p: ActivePower::from_megawatts(previous + 1.0),
            }],
        )
        .unwrap_err();
        assert_eq!(
            error.info().unwrap().code,
            "VALIDATE.UPDATE.STABLE_ID_REQUIRED"
        );
        assert_number_eq(network.loads()[0].p, previous);

        network.loads_mut()[0].uid = None;
        let error = apply_updates(
            &mut network,
            &[OperatingPointUpdate::LoadActivePower {
                load: component(LOAD, "load-0"),
                terminal: None,
                p: ActivePower::from_megawatts(previous + 1.0),
            }],
        )
        .unwrap_err();
        assert_eq!(
            error.info().unwrap().code,
            "VALIDATE.UPDATE.STABLE_ID_REQUIRED"
        );
        assert_number_eq(network.loads()[0].p, previous);
    }

    #[test]
    fn balanced_initial_update_fields_apply_as_absolute_replacements() {
        let mut network = balanced_network();
        let updates = [
            OperatingPointUpdate::LoadReactivePower {
                load: component(LOAD, "load-0"),
                terminal: None,
                q: ReactivePower::from_vars(4_000_000.0),
            },
            OperatingPointUpdate::GeneratorActivePower {
                generator: component(GENERATOR, "generator-0"),
                terminal: None,
                p: ActivePower::from_megawatts(71.0),
            },
            OperatingPointUpdate::GeneratorReactivePower {
                generator: component(GENERATOR, "generator-0"),
                terminal: None,
                q: ReactivePower::from_megavars(8.0),
            },
            OperatingPointUpdate::GeneratorVoltageMagnitude {
                generator: component(GENERATOR, "generator-0"),
                vm_pu: 1.02,
            },
            OperatingPointUpdate::TransformerTapRatio {
                transformer: component(TRANSFORMER, "transformer-0"),
                tap_ratio: 1.05,
            },
            OperatingPointUpdate::TransformerPhaseShift {
                transformer: component(TRANSFORMER, "transformer-0"),
                shift_degrees: 3.0,
            },
            OperatingPointUpdate::SwitchClosed {
                switch: component(SWITCH, "switch-0"),
                closed: false,
            },
        ];
        let report = apply_updates(&mut network, &updates).unwrap();
        assert_number_eq(network.loads()[0].q, 4.0);
        assert_number_eq(network.generators()[0].pg, 71.0);
        assert_number_eq(network.generators()[0].qg, 8.0);
        assert_number_eq(network.generators()[0].vg, 1.02);
        assert_number_eq(network.branches()[0].tap, 1.05);
        assert_number_eq(network.branches()[0].shift, 3.0);
        assert!(!network.switches()[0].closed);
        assert_eq!(report.changes().len(), updates.len());
        assert!(!report.connectivity_changed());
    }

    #[test]
    fn report_names_exact_fields_and_only_connectivity_edits() {
        let mut network = balanced_network();
        let generator = component(GENERATOR, "generator-0");
        let report = apply_updates(
            &mut network,
            &[OperatingPointUpdate::GeneratorInService {
                generator: generator.clone(),
                in_service: false,
            }],
        )
        .unwrap();
        assert_eq!(report.changes().len(), 1);
        assert_eq!(report.changes()[0].component_id(), &generator);
        assert_eq!(
            report.changes()[0].field(),
            UpdatedField::GeneratorInService
        );
        assert!(!report.connectivity_changed());

        let branch = component(BRANCH, "branch-1");
        let report = apply_updates(
            &mut network,
            &[OperatingPointUpdate::BranchInService {
                branch: branch.clone(),
                in_service: false,
            }],
        )
        .unwrap();
        assert_eq!(report.changes()[0].component_id(), &branch);
        assert!(!report.connectivity_changed());

        let report = apply_updates(
            &mut network,
            &[OperatingPointUpdate::BranchInService {
                branch,
                in_service: false,
            }],
        )
        .unwrap();
        assert!(report.is_empty());
        assert!(!report.connectivity_changed());
    }

    #[test]
    fn connectivity_report_compares_the_complete_energized_graph() {
        let buses = vec![
            Bus::new(BusId(1), BusType::Ref, 230.0),
            Bus::new(BusId(2), BusType::Pq, 230.0),
        ];
        let branches = vec![
            Branch::new(BusId(1), BusId(2), 0.0, 0.1),
            Branch::new(BusId(1), BusId(2), 0.0, 0.2),
        ];
        let mut network = BalancedNetwork::in_memory("parallel", 100.0, buses, branches);
        let first = network.branches()[0].uid.clone().unwrap();
        let second = network.branches()[1].uid.clone().unwrap();

        let report = apply_updates(
            &mut network,
            &[OperatingPointUpdate::BranchInService {
                branch: component(BRANCH, &first),
                in_service: false,
            }],
        )
        .unwrap();
        assert!(!report.connectivity_changed());

        let report = apply_updates(
            &mut network,
            &[OperatingPointUpdate::BranchInService {
                branch: component(BRANCH, &second),
                in_service: false,
            }],
        )
        .unwrap();
        assert!(report.connectivity_changed());
    }

    #[test]
    fn network_updates_are_copy_on_write_and_convert_units() {
        let mut network = balanced_network();
        let untouched = network.clone();
        let report = apply_updates(
            &mut network,
            &[NetworkUpdate::BranchThermalRating {
                branch: component(BRANCH, "branch-1"),
                terminal: None,
                rating: ApparentPower::from_volt_amperes(125_000_000.0),
            }],
        )
        .unwrap();
        assert_number_eq(network.branches()[1].rate_a, 125.0);
        assert_ne!(
            untouched.branches()[1].rate_a.to_bits(),
            125.0_f64.to_bits()
        );
        assert_eq!(
            report.changes()[0].field(),
            UpdatedField::BranchThermalRating
        );
    }

    #[test]
    fn point_update_adds_one_typed_override_without_changing_its_network() {
        let network = balanced_network();
        let original = network.loads()[0].p;
        let mut point = BalancedOperatingPointBuilder::for_point(network.clone())
            .build_point()
            .unwrap();
        let untouched = point.clone();
        let report = apply_updates(
            &mut point,
            &[OperatingPointUpdate::LoadActivePower {
                load: component(LOAD, "load-0"),
                terminal: None,
                p: ActivePower::from_megawatts(original + 7.0),
            }],
        )
        .unwrap();
        assert_eq!(point.load_active_power("load-0"), Some(original + 7.0));
        assert_eq!(untouched.load_active_power("load-0"), None);
        assert_number_eq(point.network().loads()[0].p, original);
        assert_eq!(report.changes()[0].field(), UpdatedField::LoadActivePower);
    }

    #[test]
    fn duplicate_assignments_are_rejected_atomically() {
        let mut network = balanced_network();
        let previous = network.loads()[0].p;
        let load = component(LOAD, "load-0");
        let error = apply_updates(
            &mut network,
            &[
                OperatingPointUpdate::LoadActivePower {
                    load: load.clone(),
                    terminal: None,
                    p: ActivePower::from_megawatts(previous + 1.0),
                },
                OperatingPointUpdate::LoadActivePower {
                    load,
                    terminal: None,
                    p: ActivePower::from_megawatts(previous + 2.0),
                },
            ],
        )
        .unwrap_err();
        assert_eq!(
            error.info().unwrap().code,
            "VALIDATE.UPDATE.DUPLICATE_FIELD"
        );
        assert_number_eq(network.loads()[0].p, previous);
    }

    #[test]
    fn calculation_updates_preserve_the_instance_type_and_replace_its_inputs() {
        let network = balanced_network();
        let mut instance = DcPfInstance::from_network(network).unwrap();
        let previous = instance.network().loads()[0].p;
        let report = apply_updates(
            &mut instance,
            &[CalculationUpdate::OperatingPoint(
                OperatingPointUpdate::LoadActivePower {
                    load: component(LOAD, "load-0"),
                    terminal: None,
                    p: ActivePower::from_megawatts(previous + 5.0),
                },
            )],
        )
        .unwrap();
        assert_number_eq(instance.network().loads()[0].p, previous + 5.0);
        assert_eq!(report.changes()[0].field(), UpdatedField::LoadActivePower);
    }

    #[test]
    fn conductor_resolved_updates_name_the_exact_terminal() {
        let mut network = multiconductor_network();
        let load = component(LOAD, "ld");
        let terminal = network.loads()[0].terminal_map[0].clone();
        let original = network.loads()[0].p_nom[0];
        let report = apply_updates(
            &mut network,
            &[OperatingPointUpdate::LoadActivePower {
                load: load.clone(),
                terminal: Some(terminal.clone()),
                p: ActivePower::from_watts(original + 1_000.0),
            }],
        )
        .unwrap();
        assert_number_eq(network.loads()[0].p_nom[0], original + 1_000.0);
        assert_eq!(report.changes()[0].component_id(), &load);
        assert_eq!(report.changes()[0].terminal(), Some(terminal.as_str()));

        let report = apply_updates(
            &mut network,
            &[OperatingPointUpdate::SwitchClosed {
                switch: component(SWITCH, "sw"),
                closed: false,
            }],
        )
        .unwrap();
        assert!(!report.connectivity_changed());

        network
            .buses_mut()
            .push(DistBus::new("c", vec!["1".to_owned()]));
        network.switches_mut().push(DistSwitch::new(
            "bridge",
            "b",
            "c",
            vec!["1".to_owned()],
            vec!["1".to_owned()],
            false,
        ));
        let report = apply_updates(
            &mut network,
            &[OperatingPointUpdate::SwitchClosed {
                switch: component(SWITCH, "bridge"),
                closed: false,
            }],
        )
        .unwrap();
        assert!(report.connectivity_changed());
    }
}
