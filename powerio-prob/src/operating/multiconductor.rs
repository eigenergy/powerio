//! Multiconductor operating points over one shared
//! [`MulticonductorNetwork`] handle, with per terminal addressing.

use std::sync::Arc;

use powerio_core::{Error, TimePoint, TimeSeries};
use powerio_dist::MulticonductorNetwork;

use super::{
    OperatingPointColumns, OperatingPointFlags, OperatingPointValues, QuantityLayout,
    SharedColumns, dense_quantity, sparse_quantity,
};
use crate::diagnostics::codes;

/// Multiconductor quantity names. Terminal quantities are keyed
/// `bus_id/terminal`; per element phase quantities `element_name/terminal`;
/// whole element quantities by the element's name. Names keep the case the
/// source supplied.
const TERMINAL_VOLTAGE_MAGNITUDE: &str = "terminal_voltage_magnitude";
const TERMINAL_VOLTAGE_ANGLE: &str = "terminal_voltage_angle";
pub(crate) const LOAD_ACTIVE_POWER: &str = "load_active_power";
pub(crate) const LOAD_REACTIVE_POWER: &str = "load_reactive_power";
pub(crate) const GENERATOR_ACTIVE_POWER: &str = "generator_active_power";
pub(crate) const GENERATOR_REACTIVE_POWER: &str = "generator_reactive_power";
pub(crate) const SWITCH_CLOSED: &str = "switch_closed";
const TRANSFORMER_TAP: &str = "transformer_tap";
const CAPACITOR_STEPS: &str = "capacitor_steps";

/// A numeric multiconductor operating point quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MulticonductorOperatingPointQuantity {
    TerminalVoltageMagnitude,
    TerminalVoltageAngle,
    LoadActivePower,
    LoadReactivePower,
    GeneratorActivePower,
    GeneratorReactivePower,
    TransformerTap,
    CapacitorSteps,
}

impl MulticonductorOperatingPointQuantity {
    /// The stable PowerIO IR spelling.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TerminalVoltageMagnitude => TERMINAL_VOLTAGE_MAGNITUDE,
            Self::TerminalVoltageAngle => TERMINAL_VOLTAGE_ANGLE,
            Self::LoadActivePower => LOAD_ACTIVE_POWER,
            Self::LoadReactivePower => LOAD_REACTIVE_POWER,
            Self::GeneratorActivePower => GENERATOR_ACTIVE_POWER,
            Self::GeneratorReactivePower => GENERATOR_REACTIVE_POWER,
            Self::TransformerTap => TRANSFORMER_TAP,
            Self::CapacitorSteps => CAPACITOR_STEPS,
        }
    }
}

/// A boolean multiconductor operating point quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MulticonductorOperatingPointFlag {
    SwitchClosed,
}

impl MulticonductorOperatingPointFlag {
    /// The stable PowerIO IR spelling.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SwitchClosed => SWITCH_CLOSED,
        }
    }
}

pub use super::OperatingPoint;

impl OperatingPoint<MulticonductorNetwork> {
    /// Rebind this point to an edited network with the same component and
    /// terminal identity layouts.
    pub(crate) fn rebind_network(mut self, network: MulticonductorNetwork) -> Result<Self, Error> {
        let layout = MulticonductorOperatingPointBuilder::new(network.clone(), Vec::new());
        for quantity in self.columns.quantities.keys() {
            let expected: Vec<String> = layout
                .layout_for(quantity)?
                .order()
                .map(str::to_owned)
                .collect();
            let actual: Vec<&str> = self
                .identity_order(quantity)
                .expect("the quantity came from this point")
                .collect();
            if actual.len() != expected.len()
                || actual
                    .iter()
                    .zip(&expected)
                    .any(|(left, right)| *left != right)
            {
                return Err(Error::new(
                    &codes::BUILD_OPERATING_POINT_SHAPE_MISMATCH,
                    format!(
                        "{quantity}: edited network changes the initial point's component identity order"
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
        quantity: MulticonductorOperatingPointQuantity,
    ) -> Option<OperatingPointValues<'_>> {
        self.iter_values(quantity.name())
    }

    /// Iterate one boolean quantity in stable component identity order.
    #[must_use]
    pub fn flags(
        &self,
        quantity: MulticonductorOperatingPointFlag,
    ) -> Option<OperatingPointFlags<'_>> {
        self.iter_flags(quantity.name())
    }

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

    /// Generator active power on one conductor in watts.
    #[must_use]
    pub fn generator_active_power(&self, generator: &str, terminal: &str) -> Option<f64> {
        self.value_pair(GENERATOR_ACTIVE_POWER, generator, terminal)
    }

    /// Generator reactive power on one conductor in vars.
    #[must_use]
    pub fn generator_reactive_power(&self, generator: &str, terminal: &str) -> Option<f64> {
        self.value_pair(GENERATOR_REACTIVE_POWER, generator, terminal)
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

/// Bulk constructor for a multiconductor operating point series. Identities
/// resolve once against the network's stable order: bus terminals in bus
/// table order with each bus's terminal order, load conductors in load table
/// order with each terminal map's order, and named elements in their table
/// order.
#[derive(Debug)]
pub struct MulticonductorOperatingPointBuilder {
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

impl MulticonductorOperatingPointBuilder {
    #[must_use]
    pub fn new(network: MulticonductorNetwork, time_points: Vec<TimePoint>) -> Self {
        Self {
            network,
            time_points,
            quantities: Vec::new(),
        }
    }

    /// Start a builder for one operating point.
    #[must_use]
    pub fn for_point(network: MulticonductorNetwork) -> Self {
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
    pub fn generator_active_powers(self, values: Vec<f64>) -> Self {
        self.dense(GENERATOR_ACTIVE_POWER, values)
    }

    #[must_use]
    pub fn generator_reactive_powers(self, values: Vec<f64>) -> Self {
        self.dense(GENERATOR_REACTIVE_POWER, values)
    }

    #[must_use]
    pub fn switch_closed(self, values: Vec<bool>) -> Self {
        self.dense(SWITCH_CLOSED, encode_flags(values))
    }

    #[must_use]
    pub fn transformer_taps(self, values: Vec<f64>) -> Self {
        self.dense(TRANSFORMER_TAP, values)
    }

    #[must_use]
    pub fn capacitor_steps(self, values: Vec<f64>) -> Self {
        self.dense(CAPACITOR_STEPS, values)
    }

    /// Sparse terminal voltage magnitudes: one base row plus per point
    /// overrides keyed `bus_id/terminal`.
    #[must_use]
    pub fn sparse_terminal_voltage_magnitudes(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(TERMINAL_VOLTAGE_MAGNITUDE, base, changes)
    }

    #[must_use]
    pub fn sparse_terminal_voltage_angles(
        self,
        base: Vec<f64>,
        changes: Vec<Vec<(String, f64)>>,
    ) -> Self {
        self.sparse(TERMINAL_VOLTAGE_ANGLE, base, changes)
    }

    /// Sparse per conductor load active powers: one base row plus per point
    /// overrides keyed `load_name/terminal`.
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
    pub fn sparse_switch_closed(self, base: Vec<bool>, changes: Vec<Vec<(String, bool)>>) -> Self {
        self.sparse_flags(SWITCH_CLOSED, base, changes)
    }

    #[must_use]
    pub fn sparse_transformer_taps(self, base: Vec<f64>, changes: Vec<Vec<(String, f64)>>) -> Self {
        self.sparse(TRANSFORMER_TAP, base, changes)
    }

    #[must_use]
    pub fn sparse_capacitor_steps(self, base: Vec<f64>, changes: Vec<Vec<(String, f64)>>) -> Self {
        self.sparse(CAPACITOR_STEPS, base, changes)
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
            GENERATOR_ACTIVE_POWER | GENERATOR_REACTIVE_POWER => QuantityLayout::from_order(
                quantity,
                network.generators().iter().flat_map(|generator| {
                    generator
                        .terminal_map
                        .iter()
                        .map(move |terminal| format!("{}/{terminal}", generator.name))
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

    fn build_points(
        &self,
        point_count: usize,
    ) -> Result<Vec<OperatingPoint<MulticonductorNetwork>>, Error> {
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
    /// series. The points share the network handle and the columns; QSTS
    /// result import goes through here without cloning the network per
    /// sample.
    ///
    /// # Errors
    /// A shape mismatch, a duplicate or unknown identity, or an empty time
    /// axis.
    pub fn build(self) -> Result<TimeSeries<OperatingPoint<MulticonductorNetwork>>, Error> {
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
    pub fn build_point(self) -> Result<OperatingPoint<MulticonductorNetwork>, Error> {
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
