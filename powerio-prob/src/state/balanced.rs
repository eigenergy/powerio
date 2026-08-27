//! Balanced operating points: instantaneous states over one shared
//! [`BalancedNetwork`] handle.

use std::sync::Arc;

use powerio_core::{Error, TimePoint, TimeSeries};
use powerio_tx::{BalancedNetwork, BusId};

use super::{
    OperatingPoint, QuantityLayout, SharedColumns, StateColumns, dense_quantity, row_identity,
    sparse_quantity,
};
use crate::diagnostics::codes;

/// The balanced instantaneous quantity names. Bus quantities are keyed by the
/// bus ID's decimal spelling; element quantities by the element's payload
/// identity (its `uid`, or `{table}:{row}` where it states none).
const BUS_VOLTAGE_MAGNITUDE: &str = "bus_voltage_magnitude";
const BUS_VOLTAGE_ANGLE: &str = "bus_voltage_angle";
const BUS_ACTIVE_INJECTION: &str = "bus_active_injection";
const BUS_REACTIVE_INJECTION: &str = "bus_reactive_injection";
const GENERATOR_ACTIVE_POWER: &str = "generator_active_power";
const GENERATOR_REACTIVE_POWER: &str = "generator_reactive_power";
const GENERATOR_VOLTAGE_SETPOINT: &str = "generator_voltage_setpoint";
const GENERATOR_IN_SERVICE: &str = "generator_in_service";
const LOAD_ACTIVE_POWER: &str = "load_active_power";
const LOAD_REACTIVE_POWER: &str = "load_reactive_power";
const BRANCH_IN_SERVICE: &str = "branch_in_service";
const BRANCH_TAP_RATIO: &str = "branch_tap_ratio";
const BRANCH_PHASE_SHIFT: &str = "branch_phase_shift";
const SWITCH_CLOSED: &str = "switch_closed";

/// The complete balanced instantaneous vocabulary: the set of quantities the
/// stored wire writes is exactly the set the builder accepts, from this one
/// definition.
pub const BALANCED_STATE_QUANTITIES: [&str; 14] = [
    BUS_VOLTAGE_MAGNITUDE,
    BUS_VOLTAGE_ANGLE,
    BUS_ACTIVE_INJECTION,
    BUS_REACTIVE_INJECTION,
    GENERATOR_ACTIVE_POWER,
    GENERATOR_REACTIVE_POWER,
    GENERATOR_VOLTAGE_SETPOINT,
    GENERATOR_IN_SERVICE,
    LOAD_ACTIVE_POWER,
    LOAD_REACTIVE_POWER,
    BRANCH_IN_SERVICE,
    BRANCH_TAP_RATIO,
    BRANCH_PHASE_SHIFT,
    SWITCH_CLOSED,
];

impl OperatingPoint<BalancedNetwork> {
    /// Bus voltage magnitude in per unit, `None` when the series states no
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

    /// Whether the generator is in service at this point (a stated 0 is out,
    /// anything else is in).
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

    /// Whether the series states this quantity at all, spelled with the
    /// quantity's accessor name (`"bus_voltage_magnitude"`, ...).
    #[must_use]
    pub fn states(&self, quantity: &str) -> bool {
        match quantity {
            BUS_VOLTAGE_MAGNITUDE => self.stated(BUS_VOLTAGE_MAGNITUDE),
            BUS_VOLTAGE_ANGLE => self.stated(BUS_VOLTAGE_ANGLE),
            BUS_ACTIVE_INJECTION => self.stated(BUS_ACTIVE_INJECTION),
            BUS_REACTIVE_INJECTION => self.stated(BUS_REACTIVE_INJECTION),
            GENERATOR_ACTIVE_POWER => self.stated(GENERATOR_ACTIVE_POWER),
            GENERATOR_REACTIVE_POWER => self.stated(GENERATOR_REACTIVE_POWER),
            GENERATOR_VOLTAGE_SETPOINT => self.stated(GENERATOR_VOLTAGE_SETPOINT),
            GENERATOR_IN_SERVICE => self.stated(GENERATOR_IN_SERVICE),
            LOAD_ACTIVE_POWER => self.stated(LOAD_ACTIVE_POWER),
            LOAD_REACTIVE_POWER => self.stated(LOAD_REACTIVE_POWER),
            BRANCH_IN_SERVICE => self.stated(BRANCH_IN_SERVICE),
            BRANCH_TAP_RATIO => self.stated(BRANCH_TAP_RATIO),
            BRANCH_PHASE_SHIFT => self.stated(BRANCH_PHASE_SHIFT),
            SWITCH_CLOSED => self.stated(SWITCH_CLOSED),
            _ => false,
        }
    }
}

impl OperatingPoint<BalancedNetwork> {
    /// Write this point's stated quantities into an independent static
    /// network. Every write is typed: no JSON round trip, no update map, and
    /// no wholesale network clone — the shared handle copies only the tables
    /// a stated quantity touches. Angles convert from the vocabulary's
    /// radians to the table's degrees.
    ///
    /// # Errors
    /// A stated quantity the static tables cannot carry (net bus injections
    /// have no bus field), or a column count that disagrees with the table.
    pub fn materialize_network(&self) -> Result<BalancedNetwork, Error> {
        let mut network = self.network.clone();
        let mut names: Vec<&'static str> = self.columns.quantities.keys().copied().collect();
        names.sort_unstable();
        for name in names {
            if name == BUS_ACTIVE_INJECTION || name == BUS_REACTIVE_INJECTION {
                return Err(Error::new(
                    &codes::TRANSFORM_STATE_UNREPRESENTED,
                    format!(
                        "`{name}` is instantaneous state with no static network field; \
                         export cannot carry it"
                    ),
                ));
            }
            let Some(row) = self.quantity_values(name) else {
                continue;
            };
            write_quantity(&mut network, name, &row)?;
        }
        Ok(network)
    }
}

fn write_quantity(
    network: &mut BalancedNetwork,
    quantity: &'static str,
    row: &[f64],
) -> Result<(), Error> {
    let expected = match family_of(quantity) {
        KeyFamily::Bus => network.buses().len(),
        KeyFamily::Generator => network.generators().len(),
        KeyFamily::Load => network.loads().len(),
        KeyFamily::Branch => network.branches().len(),
        KeyFamily::Switch => network.switches().len(),
    };
    if row.len() != expected {
        return Err(Error::new(
            &codes::BUILD_STATE_SHAPE_MISMATCH,
            format!(
                "{quantity}: {} stated values for a {expected} row table",
                row.len()
            ),
        ));
    }
    match quantity {
        BUS_VOLTAGE_MAGNITUDE => {
            for (bus, value) in network.buses_mut().iter_mut().zip(row) {
                bus.vm = *value;
            }
        }
        BUS_VOLTAGE_ANGLE => {
            for (bus, value) in network.buses_mut().iter_mut().zip(row) {
                bus.va = value.to_degrees();
            }
        }
        GENERATOR_ACTIVE_POWER => {
            for (generator, value) in network.generators_mut().iter_mut().zip(row) {
                generator.pg = *value;
            }
        }
        GENERATOR_REACTIVE_POWER => {
            for (generator, value) in network.generators_mut().iter_mut().zip(row) {
                generator.qg = *value;
            }
        }
        GENERATOR_VOLTAGE_SETPOINT => {
            for (generator, value) in network.generators_mut().iter_mut().zip(row) {
                generator.vg = *value;
            }
        }
        GENERATOR_IN_SERVICE => {
            for (generator, value) in network.generators_mut().iter_mut().zip(row) {
                generator.in_service = *value != 0.0;
            }
        }
        LOAD_ACTIVE_POWER => {
            for (load, value) in network.loads_mut().iter_mut().zip(row) {
                load.p = *value;
            }
        }
        LOAD_REACTIVE_POWER => {
            for (load, value) in network.loads_mut().iter_mut().zip(row) {
                load.q = *value;
            }
        }
        BRANCH_IN_SERVICE => {
            for (branch, value) in network.branches_mut().iter_mut().zip(row) {
                branch.in_service = *value != 0.0;
            }
        }
        BRANCH_TAP_RATIO => {
            for (branch, value) in network.branches_mut().iter_mut().zip(row) {
                branch.tap = *value;
            }
        }
        BRANCH_PHASE_SHIFT => {
            for (branch, value) in network.branches_mut().iter_mut().zip(row) {
                branch.shift = *value;
            }
        }
        SWITCH_CLOSED => {
            for (switch, value) in network.switches_mut().iter_mut().zip(row) {
                switch.closed = *value != 0.0;
            }
        }
        other => {
            return Err(Error::new(
                &codes::TRANSFORM_STATE_UNREPRESENTED,
                format!("`{other}` has no static network field"),
            ));
        }
    }
    Ok(())
}

/// The balanced series entry: alias for what the builder produces.
pub type BalancedOperatingPoints = TimeSeries<OperatingPoint<BalancedNetwork>>;

/// Bulk constructor for a balanced operating point series. Identities resolve
/// once against the network's stable order — buses in bus table order by ID,
/// elements in table order by payload identity — and every column is
/// validated against that order and the point count. Dense columns are point
/// major; sparse columns override one base row per point by identity.
#[derive(Debug)]
pub struct BalancedStateBuilder {
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

impl BalancedStateBuilder {
    #[must_use]
    pub fn new(network: BalancedNetwork, time_points: Vec<TimePoint>) -> Self {
        Self {
            network,
            time_points,
            quantities: Vec::new(),
        }
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
    pub fn generator_in_service(self, values: Vec<f64>) -> Self {
        self.dense(GENERATOR_IN_SERVICE, values)
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
    pub fn branch_in_service(self, values: Vec<f64>) -> Self {
        self.dense(BRANCH_IN_SERVICE, values)
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
    pub fn switch_closed(self, values: Vec<f64>) -> Self {
        self.dense(SWITCH_CLOSED, values)
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

    /// Sparse generator active powers, as
    /// [`sparse_load_active_powers`](Self::sparse_load_active_powers).
    #[must_use]
    pub fn sparse_generator_active_powers(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(GENERATOR_ACTIVE_POWER, base, changes)
    }

    /// Dense columns for a quantity by its accessor name, for stored decode:
    /// an unknown name is refused rather than silently registered.
    ///
    /// # Errors
    /// A name outside the balanced instantaneous vocabulary.
    pub fn dense_by_name(self, quantity: &str, values: Vec<f64>) -> Result<Self, Error> {
        let quantity = resolve_quantity(quantity)?;
        Ok(self.dense(quantity, values))
    }

    /// The identity order the network resolves for a quantity: the exact
    /// sequence a dense column binds to, so a stored document's stated
    /// identity list can be checked before its values are accepted.
    ///
    /// # Errors
    /// A name outside the balanced instantaneous vocabulary.
    pub fn identity_order(&self, quantity: &str) -> Result<Vec<String>, Error> {
        let quantity = resolve_quantity(quantity)?;
        Ok(self
            .layout_for(quantity)?
            .order()
            .map(str::to_string)
            .collect())
    }

    /// Sparse columns for a quantity by its accessor name, as
    /// [`Self::dense_by_name`].
    ///
    /// # Errors
    /// A name outside the balanced instantaneous vocabulary.
    pub fn sparse_by_name(
        self,
        quantity: &str,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Result<Self, Error> {
        let quantity = resolve_quantity(quantity)?;
        Ok(self.sparse(quantity, base, changes))
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

    /// Resolve every identity once, validate every column, and build the
    /// series. The points share the network handle and the columns.
    ///
    /// # Errors
    /// A shape mismatch, a duplicate or unknown identity, or a time axis
    /// whose length disagrees with the columns.
    pub fn build(self) -> Result<BalancedOperatingPoints, Error> {
        let point_count = self.time_points.len();
        if point_count == 0 {
            return Err(Error::new(
                &codes::BUILD_STATE_SHAPE_MISMATCH,
                "an operating point series needs at least one time point",
            ));
        }
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
                    &codes::BUILD_STATE_SHAPE_MISMATCH,
                    format!("{quantity} was supplied twice"),
                ));
            }
        }
        let columns: SharedColumns = Arc::new(StateColumns { quantities });
        let network = self.network;
        let points = (0..point_count)
            .map(|index| OperatingPoint {
                network: network.clone(),
                columns: Arc::clone(&columns),
                index,
            })
            .collect();
        TimeSeries::new(self.time_points, points)
    }
}

/// The static vocabulary name for a stored quantity spelling.
/// The complete balanced instantaneous vocabulary, in declaration order.
pub const BALANCED_STATE_QUANTITIES: [&str; 14] = [
    BUS_VOLTAGE_MAGNITUDE,
    BUS_VOLTAGE_ANGLE,
    BUS_ACTIVE_INJECTION,
    BUS_REACTIVE_INJECTION,
    GENERATOR_ACTIVE_POWER,
    GENERATOR_REACTIVE_POWER,
    GENERATOR_VOLTAGE_SETPOINT,
    GENERATOR_IN_SERVICE,
    LOAD_ACTIVE_POWER,
    LOAD_REACTIVE_POWER,
    BRANCH_IN_SERVICE,
    BRANCH_TAP_RATIO,
    BRANCH_PHASE_SHIFT,
    SWITCH_CLOSED,
];

fn resolve_quantity(name: &str) -> Result<&'static str, Error> {
    BALANCED_STATE_QUANTITIES
        .iter()
        .find(|known| **known == name)
        .copied()
        .ok_or_else(|| {
            Error::new(
                &codes::BUILD_STATE_SHAPE_MISMATCH,
                format!("`{name}` is not a balanced instantaneous quantity"),
            )
        })
}
