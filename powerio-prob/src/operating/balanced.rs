//! Balanced operating points over one shared
//! [`BalancedNetwork`] handle.

use std::sync::Arc;

use powerio_core::{Error, TimePoint, TimeSeries};
use powerio_tx::{BalancedNetwork, BusId};

use super::{
    OperatingPoint, OperatingPointColumns, OperatingPointFlags, OperatingPointValues,
    QuantityLayout, SharedColumns, dense_quantity, row_identity, sparse_quantity,
};
use crate::diagnostics::codes;

/// The balanced instantaneous quantity names. Bus quantities are keyed by the
/// bus ID's decimal spelling; element quantities by the element's payload
/// persistent identity. The row based fallback exists only for released
/// stored documents that predate parser assigned component identities; new
/// parsers assign and retain `uid` values before building an operating point.
const BUS_VOLTAGE_MAGNITUDE: &str = "bus_voltage_magnitude";
const BUS_VOLTAGE_ANGLE: &str = "bus_voltage_angle";
const BUS_ACTIVE_INJECTION: &str = "bus_active_injection";
const BUS_REACTIVE_INJECTION: &str = "bus_reactive_injection";
pub(crate) const GENERATOR_ACTIVE_POWER: &str = "generator_active_power";
pub(crate) const GENERATOR_REACTIVE_POWER: &str = "generator_reactive_power";
pub(crate) const GENERATOR_VOLTAGE_SETPOINT: &str = "generator_voltage_setpoint";
pub(crate) const GENERATOR_IN_SERVICE: &str = "generator_in_service";
pub(crate) const LOAD_ACTIVE_POWER: &str = "load_active_power";
pub(crate) const LOAD_REACTIVE_POWER: &str = "load_reactive_power";
pub(crate) const BRANCH_IN_SERVICE: &str = "branch_in_service";
pub(crate) const BRANCH_TAP_RATIO: &str = "branch_tap_ratio";
pub(crate) const BRANCH_PHASE_SHIFT: &str = "branch_phase_shift";
pub(crate) const SWITCH_CLOSED: &str = "switch_closed";

/// A numeric balanced operating point quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BalancedOperatingPointQuantity {
    BusVoltageMagnitude,
    BusVoltageAngle,
    BusActiveInjection,
    BusReactiveInjection,
    GeneratorActivePower,
    GeneratorReactivePower,
    GeneratorVoltageSetpoint,
    LoadActivePower,
    LoadReactivePower,
    BranchTapRatio,
    BranchPhaseShift,
}

impl BalancedOperatingPointQuantity {
    /// The stable PowerIO IR spelling.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BusVoltageMagnitude => BUS_VOLTAGE_MAGNITUDE,
            Self::BusVoltageAngle => BUS_VOLTAGE_ANGLE,
            Self::BusActiveInjection => BUS_ACTIVE_INJECTION,
            Self::BusReactiveInjection => BUS_REACTIVE_INJECTION,
            Self::GeneratorActivePower => GENERATOR_ACTIVE_POWER,
            Self::GeneratorReactivePower => GENERATOR_REACTIVE_POWER,
            Self::GeneratorVoltageSetpoint => GENERATOR_VOLTAGE_SETPOINT,
            Self::LoadActivePower => LOAD_ACTIVE_POWER,
            Self::LoadReactivePower => LOAD_REACTIVE_POWER,
            Self::BranchTapRatio => BRANCH_TAP_RATIO,
            Self::BranchPhaseShift => BRANCH_PHASE_SHIFT,
        }
    }
}

/// A boolean balanced operating point quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BalancedOperatingPointFlag {
    GeneratorInService,
    BranchInService,
    SwitchClosed,
}

impl BalancedOperatingPointFlag {
    /// The stable PowerIO IR spelling.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::GeneratorInService => GENERATOR_IN_SERVICE,
            Self::BranchInService => BRANCH_IN_SERVICE,
            Self::SwitchClosed => SWITCH_CLOSED,
        }
    }
}

impl OperatingPoint<BalancedNetwork> {
    /// Rebind this point to an edited network with the same element identity
    /// layout. Used when an instance changes parameters such as branch ratings
    /// without changing which columns an initial point addresses.
    pub(crate) fn rebind_network(mut self, network: BalancedNetwork) -> Result<Self, Error> {
        let layout = BalancedOperatingPointBuilder::new(network.clone(), Vec::new());
        for quantity in self.columns.quantities.keys() {
            let expected = layout.identity_order(quantity)?;
            let actual: Vec<&str> = self
                .identity_order(quantity)
                .expect("the quantity came from this point")
                .collect();
            if actual.len() != expected.len() || actual.iter().zip(&expected).any(|(a, b)| *a != b)
            {
                return Err(Error::new(
                    &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                    format!(
                        "{quantity}: edited network changes the initial point's element identity order"
                    ),
                ));
            }
        }
        self.network = network;
        Ok(self)
    }

    /// Iterate one numeric quantity in stable component identity order.
    #[must_use]
    pub fn values(
        &self,
        quantity: BalancedOperatingPointQuantity,
    ) -> Option<OperatingPointValues<'_>> {
        self.iter_values(quantity.name())
    }

    /// Iterate one boolean quantity in stable component identity order.
    #[must_use]
    pub fn flags(&self, quantity: BalancedOperatingPointFlag) -> Option<OperatingPointFlags<'_>> {
        self.iter_flags(quantity.name())
    }

    /// Bus voltage magnitude in per unit, `None` when the point contains no
    /// voltages or the bus is unknown.
    #[must_use]
    pub fn bus_voltage_magnitude(&self, bus: BusId) -> Option<f64> {
        self.value(BUS_VOLTAGE_MAGNITUDE, &bus.0.to_string())
    }

    /// Bus voltage angle in radians.
    #[must_use]
    pub fn bus_voltage_angle(&self, bus: BusId) -> Option<f64> {
        self.value(BUS_VOLTAGE_ANGLE, &bus.0.to_string())
    }

    /// Net active injection at the bus in MW.
    #[must_use]
    pub fn bus_active_injection(&self, bus: BusId) -> Option<f64> {
        self.value(BUS_ACTIVE_INJECTION, &bus.0.to_string())
    }

    /// Net reactive injection at the bus in MVAr.
    #[must_use]
    pub fn bus_reactive_injection(&self, bus: BusId) -> Option<f64> {
        self.value(BUS_REACTIVE_INJECTION, &bus.0.to_string())
    }

    /// Generator active power output in MW, by payload identity.
    #[must_use]
    pub fn generator_active_power(&self, identity: &str) -> Option<f64> {
        self.value(GENERATOR_ACTIVE_POWER, identity)
    }

    /// Generator reactive power output in MVAr, by payload identity.
    #[must_use]
    pub fn generator_reactive_power(&self, identity: &str) -> Option<f64> {
        self.value(GENERATOR_REACTIVE_POWER, identity)
    }

    /// Generator voltage setpoint in per unit, by payload identity.
    #[must_use]
    pub fn generator_voltage_setpoint(&self, identity: &str) -> Option<f64> {
        self.value(GENERATOR_VOLTAGE_SETPOINT, identity)
    }

    /// Whether the generator is in service at this point.
    #[must_use]
    pub fn generator_in_service(&self, identity: &str) -> Option<bool> {
        self.value(GENERATOR_IN_SERVICE, identity)
            .map(|value| value != 0.0)
    }

    /// Load active power in MW, by payload identity.
    #[must_use]
    pub fn load_active_power(&self, identity: &str) -> Option<f64> {
        self.value(LOAD_ACTIVE_POWER, identity)
    }

    /// Load reactive power in MVAr, by payload identity.
    #[must_use]
    pub fn load_reactive_power(&self, identity: &str) -> Option<f64> {
        self.value(LOAD_REACTIVE_POWER, identity)
    }

    /// Whether the branch is in service at this point.
    #[must_use]
    pub fn branch_in_service(&self, identity: &str) -> Option<bool> {
        self.value(BRANCH_IN_SERVICE, identity)
            .map(|value| value != 0.0)
    }

    /// Branch off-nominal tap ratio at this point.
    #[must_use]
    pub fn branch_tap_ratio(&self, identity: &str) -> Option<f64> {
        self.value(BRANCH_TAP_RATIO, identity)
    }

    /// Branch phase shift in degrees at this point.
    #[must_use]
    pub fn branch_phase_shift(&self, identity: &str) -> Option<f64> {
        self.value(BRANCH_PHASE_SHIFT, identity)
    }

    /// Whether the switch is closed at this point.
    #[must_use]
    pub fn switch_closed(&self, identity: &str) -> Option<bool> {
        self.value(SWITCH_CLOSED, identity)
            .map(|value| value != 0.0)
    }
}

/// Bulk constructor for a balanced operating point series. Identities resolve
/// once against the network's stable order — buses in bus table order by ID,
/// elements in table order by payload identity — and every column is
/// validated against that order and the point count. Dense columns are point
/// major; sparse columns override one base row per point by identity.
#[derive(Debug)]
pub struct BalancedOperatingPointBuilder {
    network: BalancedNetwork,
    time_points: Vec<TimePoint>,
    quantities: Vec<(&'static str, ColumnsInput)>,
}

#[derive(Debug)]
enum ColumnsInput {
    Dense(Vec<f64>),
    Sparse {
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyFamily {
    Bus,
    Generator,
    Load,
    Branch,
    Switch,
}

fn family_of(quantity: &'static str) -> KeyFamily {
    match quantity {
        BUS_VOLTAGE_MAGNITUDE
        | BUS_VOLTAGE_ANGLE
        | BUS_ACTIVE_INJECTION
        | BUS_REACTIVE_INJECTION => KeyFamily::Bus,
        GENERATOR_ACTIVE_POWER
        | GENERATOR_REACTIVE_POWER
        | GENERATOR_VOLTAGE_SETPOINT
        | GENERATOR_IN_SERVICE => KeyFamily::Generator,
        LOAD_ACTIVE_POWER | LOAD_REACTIVE_POWER => KeyFamily::Load,
        BRANCH_IN_SERVICE | BRANCH_TAP_RATIO | BRANCH_PHASE_SHIFT => KeyFamily::Branch,
        SWITCH_CLOSED => KeyFamily::Switch,
        _ => unreachable!("builder methods name registered quantities"),
    }
}

impl BalancedOperatingPointBuilder {
    #[must_use]
    pub fn new(mut network: BalancedNetwork, time_points: Vec<TimePoint>) -> Self {
        network.assign_missing_component_ids();
        Self {
            network,
            time_points,
            quantities: Vec::new(),
        }
    }

    /// Start a builder for one operating point.
    #[must_use]
    pub fn for_point(network: BalancedNetwork) -> Self {
        Self::new(network, Vec::new())
    }

    fn dense(mut self, quantity: &'static str, values: Vec<f64>) -> Self {
        self.quantities
            .push((quantity, ColumnsInput::Dense(values)));
        self
    }

    fn sparse(
        mut self,
        quantity: &'static str,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.quantities
            .push((quantity, ColumnsInput::Sparse { base, changes }));
        self
    }

    fn sparse_flags(
        self,
        quantity: &'static str,
        base: Vec<bool>,
        changes: Vec<Vec<(String, bool)>>,
    ) -> Self {
        self.sparse(
            quantity,
            encode_flags(base),
            changes
                .into_iter()
                .map(|point| {
                    point
                        .into_iter()
                        .map(|(identity, value)| (identity, encode_flag(value)))
                        .collect()
                })
                .collect(),
        )
    }

    /// Dense bus voltage magnitudes: `point_count * n_buses` values in bus
    /// table order, point major.
    #[must_use]
    pub fn bus_voltage_magnitudes(self, values: Vec<f64>) -> Self {
        self.dense(BUS_VOLTAGE_MAGNITUDE, values)
    }

    #[must_use]
    pub fn bus_voltage_angles(self, values: Vec<f64>) -> Self {
        self.dense(BUS_VOLTAGE_ANGLE, values)
    }

    #[must_use]
    pub fn bus_active_injections(self, values: Vec<f64>) -> Self {
        self.dense(BUS_ACTIVE_INJECTION, values)
    }

    #[must_use]
    pub fn bus_reactive_injections(self, values: Vec<f64>) -> Self {
        self.dense(BUS_REACTIVE_INJECTION, values)
    }

    #[must_use]
    pub fn generator_active_powers(self, values: Vec<f64>) -> Self {
        self.dense(GENERATOR_ACTIVE_POWER, values)
    }

    #[must_use]
    pub fn generator_reactive_powers(self, values: Vec<f64>) -> Self {
        self.dense(GENERATOR_REACTIVE_POWER, values)
    }

    #[must_use]
    pub fn generator_voltage_setpoints(self, values: Vec<f64>) -> Self {
        self.dense(GENERATOR_VOLTAGE_SETPOINT, values)
    }

    #[must_use]
    pub fn generator_in_service(self, values: Vec<bool>) -> Self {
        self.dense(GENERATOR_IN_SERVICE, encode_flags(values))
    }

    #[must_use]
    pub fn load_active_powers(self, values: Vec<f64>) -> Self {
        self.dense(LOAD_ACTIVE_POWER, values)
    }

    #[must_use]
    pub fn load_reactive_powers(self, values: Vec<f64>) -> Self {
        self.dense(LOAD_REACTIVE_POWER, values)
    }

    #[must_use]
    pub fn branch_in_service(self, values: Vec<bool>) -> Self {
        self.dense(BRANCH_IN_SERVICE, encode_flags(values))
    }

    #[must_use]
    pub fn branch_tap_ratios(self, values: Vec<f64>) -> Self {
        self.dense(BRANCH_TAP_RATIO, values)
    }

    #[must_use]
    pub fn branch_phase_shifts(self, values: Vec<f64>) -> Self {
        self.dense(BRANCH_PHASE_SHIFT, values)
    }

    #[must_use]
    pub fn switch_closed(self, values: Vec<bool>) -> Self {
        self.dense(SWITCH_CLOSED, encode_flags(values))
    }

    /// Sparse bus voltage magnitudes: one base row plus per point overrides
    /// keyed by bus identity.
    #[must_use]
    pub fn sparse_bus_voltage_magnitudes(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(BUS_VOLTAGE_MAGNITUDE, base, changes)
    }

    #[must_use]
    pub fn sparse_bus_voltage_angles(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(BUS_VOLTAGE_ANGLE, base, changes)
    }

    #[must_use]
    pub fn sparse_bus_active_injections(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(BUS_ACTIVE_INJECTION, base, changes)
    }

    #[must_use]
    pub fn sparse_bus_reactive_injections(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(BUS_REACTIVE_INJECTION, base, changes)
    }

    #[must_use]
    pub fn sparse_generator_active_powers(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(GENERATOR_ACTIVE_POWER, base, changes)
    }

    #[must_use]
    pub fn sparse_generator_reactive_powers(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(GENERATOR_REACTIVE_POWER, base, changes)
    }

    #[must_use]
    pub fn sparse_generator_voltage_setpoints(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(GENERATOR_VOLTAGE_SETPOINT, base, changes)
    }

    #[must_use]
    pub fn sparse_generator_in_service(
        self,
        base: Vec<bool>,
        changes: Vec<Vec<(String, bool)>>,
    ) -> Self {
        self.sparse_flags(GENERATOR_IN_SERVICE, base, changes)
    }

    /// Sparse load active powers: one base row in load table order plus per
    /// point overrides keyed by payload identity.
    #[must_use]
    pub fn sparse_load_active_powers(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(LOAD_ACTIVE_POWER, base, changes)
    }

    #[must_use]
    pub fn sparse_load_reactive_powers(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(LOAD_REACTIVE_POWER, base, changes)
    }

    #[must_use]
    pub fn sparse_branch_in_service(
        self,
        base: Vec<bool>,
        changes: Vec<Vec<(String, bool)>>,
    ) -> Self {
        self.sparse_flags(BRANCH_IN_SERVICE, base, changes)
    }

    #[must_use]
    pub fn sparse_branch_tap_ratios(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(BRANCH_TAP_RATIO, base, changes)
    }

    #[must_use]
    pub fn sparse_branch_phase_shifts(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(BRANCH_PHASE_SHIFT, base, changes)
    }

    #[must_use]
    pub fn sparse_switch_closed(self, base: Vec<bool>, changes: Vec<Vec<(String, bool)>>) -> Self {
        self.sparse_flags(SWITCH_CLOSED, base, changes)
    }

    /// The identity order the network resolves for a quantity: the exact
    /// sequence a dense column binds to.
    fn identity_order(&self, quantity: &'static str) -> Result<Vec<String>, Error> {
        Ok(self
            .layout_for(quantity)?
            .order()
            .map(str::to_string)
            .collect())
    }

    fn layout_for(&self, quantity: &'static str) -> Result<QuantityLayout, Error> {
        let network = &self.network;
        match family_of(quantity) {
            KeyFamily::Bus => QuantityLayout::from_order(
                quantity,
                network.buses().iter().map(|bus| bus.id.0.to_string()),
            ),
            KeyFamily::Generator => QuantityLayout::from_order(
                quantity,
                network
                    .generators()
                    .iter()
                    .enumerate()
                    .map(|(row, g)| row_identity(g.uid.as_deref(), "generators", row)),
            ),
            KeyFamily::Load => QuantityLayout::from_order(
                quantity,
                network
                    .loads()
                    .iter()
                    .enumerate()
                    .map(|(row, l)| row_identity(l.uid.as_deref(), "loads", row)),
            ),
            KeyFamily::Branch => QuantityLayout::from_order(
                quantity,
                network
                    .branches()
                    .iter()
                    .enumerate()
                    .map(|(row, b)| row_identity(b.uid.as_deref(), "branches", row)),
            ),
            KeyFamily::Switch => QuantityLayout::from_order(
                quantity,
                network
                    .switches()
                    .iter()
                    .enumerate()
                    .map(|(row, s)| row_identity(s.uid.as_deref(), "switches", row)),
            ),
        }
    }

    fn build_points(
        &self,
        point_count: usize,
    ) -> Result<Vec<OperatingPoint<BalancedNetwork>>, Error> {
        let mut quantities = std::collections::HashMap::new();
        for (quantity, input) in &self.quantities {
            let layout = self.layout_for(quantity)?;
            let built = match input {
                ColumnsInput::Dense(values) => {
                    dense_quantity(quantity, layout, point_count, values.clone())?
                }
                ColumnsInput::Sparse { base, changes } => {
                    sparse_quantity(quantity, layout, point_count, base.clone(), changes.clone())?
                }
            };
            if quantities.insert(*quantity, built).is_some() {
                return Err(Error::new(
                    &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                    format!("{quantity} was supplied twice"),
                ));
            }
        }
        let columns: SharedColumns = Arc::new(OperatingPointColumns {
            point_count,
            quantities,
        });
        Ok((0..point_count)
            .map(|index| OperatingPoint {
                network: self.network.clone(),
                columns: Arc::clone(&columns),
                index,
            })
            .collect())
    }

    /// Resolve every identity once, validate every column, and build the
    /// series. The points share the network handle and the columns.
    ///
    /// # Errors
    /// A shape mismatch, a duplicate or unknown identity, or a time axis
    /// whose length disagrees with the columns.
    pub fn build(self) -> Result<TimeSeries<OperatingPoint<BalancedNetwork>>, Error> {
        let point_count = self.time_points.len();
        if point_count == 0 {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                "an operating point series needs at least one time point",
            ));
        }
        let points = self.build_points(point_count)?;
        TimeSeries::new(self.time_points, points)
    }

    /// Build one operating point.
    ///
    /// # Errors
    /// The builder does not contain exactly one point, or a quantity is invalid.
    pub fn build_point(self) -> Result<OperatingPoint<BalancedNetwork>, Error> {
        if self.time_points.len() > 1 {
            return Err(Error::new(
                &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                "a scalar operating point builder cannot contain several time points",
            ));
        }
        self.build_points(1)?.pop().ok_or_else(|| {
            Error::new(
                &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                "the scalar operating point builder produced no point",
            )
        })
    }
}

fn encode_flags(values: Vec<bool>) -> Vec<f64> {
    values.into_iter().map(encode_flag).collect()
}

fn encode_flag(value: bool) -> f64 {
    if value { 1.0 } else { 0.0 }
}
