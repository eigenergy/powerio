//! Source neutral inputs for an AC security constrained unit commitment calculation.
//!
//! These records describe the scheduling, reserve, and contingency data that
//! do not belong on [`powerio_tx::BalancedNetwork`]. They use stable component
//! identities and preserve the nested time structure of the source problem.
//! Solver row numbers, packed arrays, and derived window memberships belong in
//! solver preparation code, not in a calculation instance.

use powerio_core::ComponentId;
use serde::{Deserialize, Serialize};

/// Whether a simple dispatchable device produces or consumes power.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScucDeviceKind {
    Producer,
    Consumer,
}

/// One downtime dependent startup cost adjustment.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucStartupCostAdjustment {
    /// Cost adjustment in dollars.
    pub cost: f64,
    /// Maximum preceding down time for this adjustment, in hours.
    pub maximum_down_time: f64,
}

/// A limit on how many times one device can start within a time window.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucStartupLimit {
    /// Window start, in hours from the beginning of the horizon.
    pub start_time: f64,
    /// Window end, in hours from the beginning of the horizon.
    pub end_time: f64,
    pub maximum_startups: u64,
}

/// An energy requirement over one time window.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucEnergyRequirement {
    /// Window start, in hours from the beginning of the horizon.
    pub start_time: f64,
    /// Window end, in hours from the beginning of the horizon.
    pub end_time: f64,
    /// Energy bound in per unit, as defined by the GO Challenge 3 data model.
    pub energy: f64,
}

/// Initial commitment duration data not carried by the electrical network.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucInitialCommitment {
    /// Accumulated in service time before the first interval, in hours.
    pub accumulated_up_time: f64,
    /// Accumulated out of service time before the first interval, in hours.
    pub accumulated_down_time: f64,
}

/// Active power ramp limits for one dispatchable device, in per unit per hour.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucRampLimits {
    pub up: f64,
    pub down: f64,
    pub startup: f64,
    pub shutdown: f64,
}

/// Reserve quantity limits for one dispatchable device, in per unit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucReserveLimits {
    pub regulation_up: f64,
    pub regulation_down: f64,
    pub synchronized: f64,
    pub nonsynchronized: f64,
    pub ramping_up_online: f64,
    pub ramping_down_online: f64,
    pub ramping_up_offline: f64,
    pub ramping_down_offline: f64,
}

/// Additional active and reactive power capability relation for one device.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum ScucReactiveCapability {
    None,
    Linear {
        /// Reactive power at zero active power, in per unit.
        reactive_power_at_zero_active_power: f64,
        slope: f64,
    },
    Bounded {
        /// Lower reactive power intercept at zero active power, in per unit.
        reactive_power_at_zero_active_power_min: f64,
        /// Upper reactive power intercept at zero active power, in per unit.
        reactive_power_at_zero_active_power_max: f64,
        slope_min: f64,
        slope_max: f64,
    },
}

/// One piecewise linear active energy cost block.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucEnergyCostBlock {
    /// Marginal cost in $/(p.u. h).
    pub marginal_cost: f64,
    /// Active power width of the block, in per unit.
    pub block_size: f64,
}

/// Reserve costs for one device and one interval, in $/(p.u. h).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucReserveCosts {
    pub regulation_up: f64,
    pub regulation_down: f64,
    pub synchronized: f64,
    pub nonsynchronized: f64,
    pub ramping_up_online: f64,
    pub ramping_down_online: f64,
    pub ramping_up_offline: f64,
    pub ramping_down_offline: f64,
    pub reactive_up: f64,
    pub reactive_down: f64,
}

/// Time varying inputs for one dispatchable device and one interval.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucDevicePeriod {
    pub on_status_min: bool,
    pub on_status_max: bool,
    /// Active power lower bound in per unit on the instance system base.
    pub active_power_min: f64,
    /// Active power upper bound in per unit on the instance system base.
    pub active_power_max: f64,
    /// Reactive power lower bound in per unit on the instance system base.
    pub reactive_power_min: f64,
    /// Reactive power upper bound in per unit on the instance system base.
    pub reactive_power_max: f64,
    pub energy_cost_blocks: Vec<ScucEnergyCostBlock>,
    pub reserve_costs: ScucReserveCosts,
}

/// Scheduling and cost inputs for one simple dispatchable device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucDevice {
    /// The corresponding `generator` or `load` in the balanced network.
    pub id: ComponentId,
    pub kind: ScucDeviceKind,
    /// Fixed operating cost in dollars.
    pub on_cost: f64,
    /// Base startup cost in dollars.
    pub startup_cost: f64,
    pub startup_cost_adjustments: Vec<ScucStartupCostAdjustment>,
    /// Shutdown cost in dollars.
    pub shutdown_cost: f64,
    pub startup_limits: Vec<ScucStartupLimit>,
    pub energy_upper_bounds: Vec<ScucEnergyRequirement>,
    pub energy_lower_bounds: Vec<ScucEnergyRequirement>,
    /// Minimum in service time after startup, in hours.
    pub minimum_up_time: f64,
    /// Minimum out of service time after shutdown, in hours.
    pub minimum_down_time: f64,
    pub ramp_limits: ScucRampLimits,
    pub reserve_limits: ScucReserveLimits,
    /// Commitment immediately before the first interval.
    pub initial_on_status: bool,
    pub initial_commitment: ScucInitialCommitment,
    pub reactive_capability: ScucReactiveCapability,
    /// Values in the same chronological order as [`ScucInputs::interval_durations`].
    pub periods: Vec<ScucDevicePeriod>,
}

/// Discrete step limits for one shunt in the balanced network.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucShunt {
    pub id: ComponentId,
    /// Conductance added by one step, in per unit on the instance system base.
    pub conductance_per_step: f64,
    /// Susceptance added by one step, in per unit on the instance system base.
    pub susceptance_per_step: f64,
    pub step_min: i64,
    pub step_max: i64,
    pub initial_step: i64,
}

/// Connection and disconnection costs for one switchable AC branch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucBranchSwitchingCost {
    /// The corresponding `branch` or `transformer` in the balanced network.
    pub id: ComponentId,
    /// Connection cost in dollars.
    pub connection_cost: f64,
    /// Disconnection cost in dollars.
    pub disconnection_cost: f64,
}

/// Tap ratio and phase shift bounds for one two winding transformer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucTransformerControl {
    pub id: ComponentId,
    /// Off nominal tap ratio lower bound in per unit.
    pub tap_ratio_min: f64,
    /// Off nominal tap ratio upper bound in per unit.
    pub tap_ratio_max: f64,
    /// Phase shift bounds in radians.
    pub phase_shift_min: f64,
    pub phase_shift_max: f64,
}

/// Active reserve requirements and violation costs for one zone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucActiveReserveZone {
    pub id: ComponentId,
    /// Buses assigned to this zone by stable source identity.
    pub buses: Vec<ComponentId>,
    pub regulation_up_requirement_fraction: f64,
    pub regulation_down_requirement_fraction: f64,
    pub synchronized_requirement_fraction: f64,
    pub nonsynchronized_requirement_fraction: f64,
    pub ramping_up_requirement: Vec<f64>,
    pub ramping_down_requirement: Vec<f64>,
    pub regulation_up_violation_cost: f64,
    pub regulation_down_violation_cost: f64,
    pub synchronized_violation_cost: f64,
    pub nonsynchronized_violation_cost: f64,
    pub ramping_up_violation_cost: f64,
    pub ramping_down_violation_cost: f64,
}

/// Reactive reserve requirements and violation costs for one zone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucReactiveReserveZone {
    pub id: ComponentId,
    /// Buses assigned to this zone by stable source identity.
    pub buses: Vec<ComponentId>,
    pub reactive_up_requirement: Vec<f64>,
    pub reactive_down_requirement: Vec<f64>,
    pub reactive_up_violation_cost: f64,
    pub reactive_down_violation_cost: f64,
}

/// One named equipment outage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucContingency {
    pub id: ComponentId,
    /// Challenge 3 requires exactly one AC line, transformer, or DC line.
    pub components: Vec<ComponentId>,
}

/// The four required violation costs from a Challenge 3 input/problem data file.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucViolationCosts {
    /// Active power balance violation cost in $/(p.u. h).
    pub active_power_balance: f64,
    /// Reactive power balance violation cost in $/(p.u. h).
    pub reactive_power_balance: f64,
    /// Branch thermal limit violation cost in $/(p.u. h).
    pub branch_thermal_limit: f64,
    /// Energy requirement violation cost in $/(p.u. h).
    pub energy_requirement: f64,
}

/// Scheduling, reserve, and contingency inputs for an AC SCUC calculation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ScucInputs {
    /// Interval durations in chronological order, in hours.
    pub interval_durations: Vec<f64>,
    pub devices: Vec<ScucDevice>,
    pub shunts: Vec<ScucShunt>,
    pub branch_switching_costs: Vec<ScucBranchSwitchingCost>,
    pub transformer_controls: Vec<ScucTransformerControl>,
    pub active_reserve_zones: Vec<ScucActiveReserveZone>,
    pub reactive_reserve_zones: Vec<ScucReactiveReserveZone>,
    pub contingencies: Vec<ScucContingency>,
    pub violation_costs: ScucViolationCosts,
}

impl ScucInputs {
    /// Producing devices in source order.
    pub fn producers(&self) -> impl Iterator<Item = &ScucDevice> {
        self.devices
            .iter()
            .filter(|device| device.kind == ScucDeviceKind::Producer)
    }

    /// Consuming devices in source order.
    pub fn consumers(&self) -> impl Iterator<Item = &ScucDevice> {
        self.devices
            .iter()
            .filter(|device| device.kind == ScucDeviceKind::Consumer)
    }

    /// Find a dispatchable device by its source UID.
    #[must_use]
    pub fn device(&self, uid: &str) -> Option<&ScucDevice> {
        self.devices
            .iter()
            .find(|device| device.id.local_id() == uid)
    }

    /// Find a shunt by its source UID.
    #[must_use]
    pub fn shunt(&self, uid: &str) -> Option<&ScucShunt> {
        self.shunts.iter().find(|shunt| shunt.id.local_id() == uid)
    }

    /// Find a contingency by its source UID.
    #[must_use]
    pub fn contingency(&self, uid: &str) -> Option<&ScucContingency> {
        self.contingencies
            .iter()
            .find(|contingency| contingency.id.local_id() == uid)
    }

    /// Interval durations in chronological order, in hours.
    #[must_use]
    pub fn interval_durations(&self) -> &[f64] {
        &self.interval_durations
    }
}
