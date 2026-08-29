//! Multiconductor operating points: instantaneous states over one shared
//! [`MulticonductorNetwork`] handle, with per terminal addressing.

use std::sync::Arc;

use powerio_core::{Error, TimePoint, TimeSeries};
use powerio_dist::MulticonductorNetwork;

use super::{QuantityLayout, SharedColumns, StateColumns, dense_quantity, sparse_quantity};
use crate::diagnostics::codes;

/// Multiconductor quantity names. Terminal quantities are keyed
/// `bus_id/terminal`; per element phase quantities `element_name/terminal`;
/// whole element quantities by the element's name. Names keep the case the
/// model states.
const TERMINAL_VOLTAGE_MAGNITUDE: &str = "terminal_voltage_magnitude";
const TERMINAL_VOLTAGE_ANGLE: &str = "terminal_voltage_angle";
const LOAD_ACTIVE_POWER: &str = "load_active_power";
const LOAD_REACTIVE_POWER: &str = "load_reactive_power";
const SWITCH_CLOSED: &str = "switch_closed";
const TRANSFORMER_TAP: &str = "transformer_tap";
const CAPACITOR_STEPS: &str = "capacitor_steps";

pub use super::OperatingPoint;

impl OperatingPoint<MulticonductorNetwork> {
    /// Voltage magnitude at one bus terminal in volts, keyed
    /// `bus_id/terminal`.
    #[must_use]
    pub fn terminal_voltage_magnitude(&self, bus: &str, terminal: &str) -> Option<f64> {
        self.value_pair(TERMINAL_VOLTAGE_MAGNITUDE, bus, terminal)
    }

    /// Voltage angle at one bus terminal in radians.
    #[must_use]
    pub fn terminal_voltage_angle(&self, bus: &str, terminal: &str) -> Option<f64> {
        self.value_pair(TERMINAL_VOLTAGE_ANGLE, bus, terminal)
    }

    /// Load active power on one conductor in watts, keyed
    /// `load_name/terminal`.
    #[must_use]
    pub fn load_active_power(&self, load: &str, terminal: &str) -> Option<f64> {
        self.value_pair(LOAD_ACTIVE_POWER, load, terminal)
    }

    /// Load reactive power on one conductor in vars.
    #[must_use]
    pub fn load_reactive_power(&self, load: &str, terminal: &str) -> Option<f64> {
        self.value_pair(LOAD_REACTIVE_POWER, load, terminal)
    }

    /// Whether the named switch is closed at this point.
    #[must_use]
    pub fn switch_closed(&self, switch: &str) -> Option<bool> {
        self.value_single(SWITCH_CLOSED, switch)
            .map(|value| value != 0.0)
    }

    /// The named transformer's regulator tap position at this point.
    #[must_use]
    pub fn transformer_tap(&self, transformer: &str) -> Option<f64> {
        self.value_single(TRANSFORMER_TAP, transformer)
    }

    /// The named capacitor's engaged step count at this point.
    #[must_use]
    pub fn capacitor_steps(&self, capacitor: &str) -> Option<f64> {
        self.value_single(CAPACITOR_STEPS, capacitor)
    }

    fn value_pair(&self, quantity: &'static str, element: &str, terminal: &str) -> Option<f64> {
        self.columns
            .quantities
            .get(quantity)?
            .value(self.index, &format!("{element}/{terminal}"))
    }

    fn value_single(&self, quantity: &'static str, element: &str) -> Option<f64> {
        self.columns
            .quantities
            .get(quantity)?
            .value(self.index, element)
    }
}

/// The multiconductor series entry: alias for what the builder produces.
pub type MulticonductorOperatingPoints = TimeSeries<OperatingPoint<MulticonductorNetwork>>;

/// Bulk constructor for a multiconductor operating point series. Identities
/// resolve once against the network's stable order: bus terminals in bus
/// table order with each bus's terminal order, load conductors in load table
/// order with each terminal map's order, and named elements in their table
/// order.
#[derive(Debug)]
pub struct MulticonductorStateBuilder {
    network: MulticonductorNetwork,
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

impl MulticonductorStateBuilder {
    #[must_use]
    pub fn new(network: MulticonductorNetwork, time_points: Vec<TimePoint>) -> Self {
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

    /// Dense terminal voltage magnitudes: one value per bus terminal in bus
    /// table order (each bus's terminals in its stated order), point major.
    #[must_use]
    pub fn terminal_voltage_magnitudes(self, values: Vec<f64>) -> Self {
        self.dense(TERMINAL_VOLTAGE_MAGNITUDE, values)
    }

    #[must_use]
    pub fn terminal_voltage_angles(self, values: Vec<f64>) -> Self {
        self.dense(TERMINAL_VOLTAGE_ANGLE, values)
    }

    /// Dense per conductor load active powers: one value per load terminal in
    /// load table order (each load's terminal map order), point major.
    #[must_use]
    pub fn load_active_powers(self, values: Vec<f64>) -> Self {
        self.dense(LOAD_ACTIVE_POWER, values)
    }

    #[must_use]
    pub fn load_reactive_powers(self, values: Vec<f64>) -> Self {
        self.dense(LOAD_REACTIVE_POWER, values)
    }

    #[must_use]
    pub fn switch_closed(self, values: Vec<f64>) -> Self {
        self.dense(SWITCH_CLOSED, values)
    }

    #[must_use]
    pub fn transformer_taps(self, values: Vec<f64>) -> Self {
        self.dense(TRANSFORMER_TAP, values)
    }

    #[must_use]
    pub fn capacitor_steps(self, values: Vec<f64>) -> Self {
        self.dense(CAPACITOR_STEPS, values)
    }

    /// Sparse per conductor load active powers: one base row plus per point
    /// overrides keyed `load_name/terminal`.
    #[must_use]
    pub fn sparse_load_active_powers(
        mut self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.quantities
            .push((LOAD_ACTIVE_POWER, ColumnsInput::Sparse { base, changes }));
        self
    }

    /// The identity order the network resolves for a quantity: the exact
    /// sequence a dense column binds to, so a stored document's stated
    /// identity list can be checked before its values are accepted.
    ///
    /// # Errors
    /// A name outside the multiconductor state vocabulary.
    pub fn identity_order(&self, quantity: &str) -> Result<Vec<String>, Error> {
        let quantity = match quantity {
            "terminal_voltage_magnitude" => TERMINAL_VOLTAGE_MAGNITUDE,
            "terminal_voltage_angle" => TERMINAL_VOLTAGE_ANGLE,
            "load_active_power" => LOAD_ACTIVE_POWER,
            "load_reactive_power" => LOAD_REACTIVE_POWER,
            "switch_closed" => SWITCH_CLOSED,
            "transformer_tap" => TRANSFORMER_TAP,
            "capacitor_steps" => CAPACITOR_STEPS,
            other => {
                return Err(Error::new(
                    &codes::BUILD_STATE_SHAPE_MISMATCH,
                    format!("`{other}` is not a multiconductor state quantity"),
                ));
            }
        };
        Ok(self
            .layout_for(quantity)?
            .order()
            .map(str::to_string)
            .collect())
    }

    fn layout_for(&self, quantity: &'static str) -> Result<QuantityLayout, Error> {
        let network = &self.network;
        match quantity {
            TERMINAL_VOLTAGE_MAGNITUDE | TERMINAL_VOLTAGE_ANGLE => QuantityLayout::from_order(
                quantity,
                network.buses().iter().flat_map(|bus| {
                    bus.terminals
                        .iter()
                        .map(move |terminal| format!("{}/{terminal}", bus.id))
                }),
            ),
            LOAD_ACTIVE_POWER | LOAD_REACTIVE_POWER => QuantityLayout::from_order(
                quantity,
                network.loads().iter().flat_map(|load| {
                    load.terminal_map
                        .iter()
                        .map(move |terminal| format!("{}/{terminal}", load.name))
                }),
            ),
            SWITCH_CLOSED => QuantityLayout::from_order(
                quantity,
                network.switches().iter().map(|s| s.name.clone()),
            ),
            TRANSFORMER_TAP => QuantityLayout::from_order(
                quantity,
                network.transformers().iter().map(|t| t.name.clone()),
            ),
            CAPACITOR_STEPS => QuantityLayout::from_order(
                quantity,
                network.capacitors().iter().map(|c| c.name.clone()),
            ),
            _ => unreachable!("builder methods name registered quantities"),
        }
    }

    /// Resolve every identity once, validate every column, and build the
    /// series. The points share the network handle and the columns; QSTS
    /// result import goes through here without cloning the network per
    /// sample.
    ///
    /// # Errors
    /// A shape mismatch, a duplicate or unknown identity, or an empty time
    /// axis.
    pub fn build(self) -> Result<MulticonductorOperatingPoints, Error> {
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
