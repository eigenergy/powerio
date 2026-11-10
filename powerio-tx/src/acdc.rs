//! Balanced resistive DC equivalents and full converter-station declarations.
//!
//! These are based declarations, independent of a calculation. DC pole count
//! multiplies branch power; it does not describe grounding or physical poles.
//! Station impedances use the source's AC per-unit coordinates. The station
//! voltage base is not implicitly replaced by the PCC bus voltage base.
use crate::{BalancedNetwork, BusId, Error, Result};
use powerio_core::ComponentId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// IR readers must implement this capability before accepting this equipment.
pub const CAPABILITY: &str = "balanced-acdc-equivalent-v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BalancedAcDcEquipment {
    /// Source power base, retained even if the AC network is transformed.
    pub base_mva: f64,
    /// One or two symmetric poles in the scalar DC equivalent.
    pub pole_count: u8,
    pub buses: Vec<DcEquivalentBus>,
    pub branches: Vec<DcEquivalentBranch>,
    pub stations: Vec<AcDcConverterStation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcEquivalentBus {
    pub component: ComponentId,
    /// External identity in the DC namespace, never an AC BusId.
    pub external_id: u32,
    /// Source grouping metadata, not an electrical connectivity partition.
    pub source_grid: u32,
    pub base_kv: f64,
    pub withdrawal_mw: f64,
    pub initial_voltage_pu: f64,
    pub voltage_limits_pu: AcDcLimits,
    /// Declared per-unit capacitance; no conversion to farads is implied.
    pub capacitance_pu: f64,
    pub area: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcEquivalentBranch {
    pub component: ComponentId,
    pub from: ComponentId,
    pub to: ComponentId,
    /// Per unit on equipment.base_mva and the declared endpoint DC base kV.
    pub resistance_pu: f64,
    /// Source coefficient in pu/second, retained for IO only.
    pub inductance_pu_per_second: f64,
    /// Source coefficient in pu*second, retained for IO only.
    pub capacitance_pu_seconds: f64,
    /// A/B/C endpoint active-power bounds. Zero means zero, not unlimited.
    pub ratings_mw: [f64; 3],
    pub in_service: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcDcLimits {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConverterTechnology {
    Vsc,
    Lcc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AcConverterControl {
    ReactivePowerAtPcc,
    VoltageAtPcc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DcConverterControl {
    ActivePowerAtPcc,
    DcVoltage,
    DcPowerDroop,
    ValveAcPowerDroop,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConverterSeries {
    /// Bypassing a component retains its supplied impedance and tap.
    pub enabled: bool,
    pub resistance_pu: f64,
    pub reactance_pu: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcCurrentLosses {
    pub constant_mw: f64,
    /// Source LossB in kV, based on converter AC RMS current, not DC current.
    pub linear_kv: f64,
    pub rectifier_quadratic_ohm: f64,
    pub inverter_quadratic_ohm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConverterSettings {
    /// Passive PCC withdrawal; MATPOWER P_g/Q_g have the opposite sign.
    pub pcc_withdrawal_mw: f64,
    pub pcc_withdrawal_mvar: f64,
    pub declared_ac_voltage_pu: f64,
    /// Declared pu-voltage/pu-power coefficient; retain unused zero values.
    pub droop: f64,
    pub dc_withdrawal_target_mw: f64,
    pub dc_voltage_target_pu: f64,
    pub dc_voltage_deadband_pu: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcDcConverterStation {
    pub component: ComponentId,
    pub ac_bus: BusId,
    pub dc_bus: ComponentId,
    pub technology: ConverterTechnology,
    pub ac_control: AcConverterControl,
    pub dc_control: DcConverterControl,
    /// Converter AC base; independent of the PCC bus's voltage coordinates.
    pub ac_base_kv: f64,
    pub transformer: ConverterSeries,
    pub transformer_tap_ratio: f64,
    pub filter_enabled: bool,
    pub filter_susceptance_pu: f64,
    pub reactor: ConverterSeries,
    pub valve_voltage_pu: AcDcLimits,
    pub valve_current_limit_pu: f64,
    /// Passive valve-side limits; these are not PCC setpoint limits.
    pub valve_active_mw: AcDcLimits,
    pub valve_reactive_mvar: AcDcLimits,
    pub losses: AcCurrentLosses,
    pub settings: ConverterSettings,
    pub in_service: bool,
}

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: "matpower-acdc",
        message: message.into(),
    }
}

impl BalancedAcDcEquipment {
    /// Check all records, including disabled equipment, without altering data.
    pub fn validate(&self, network: &BalancedNetwork) -> Result<()> {
        let finite = |v: f64| v.is_finite();
        let positive = |v: f64| v.is_finite() && v > 0.0;
        let limits = |v: AcDcLimits| finite(v.min) && finite(v.max) && v.min <= v.max;
        if !positive(self.base_mva) || !matches!(self.pole_count, 1 | 2) {
            return Err(invalid(
                "AC/DC equivalent needs a positive base MVA and pole count 1 or 2",
            ));
        }
        let mut ids = HashSet::new();
        let mut external = HashSet::new();
        let dc: HashSet<_> = self.buses.iter().map(|b| &b.component).collect();
        let ac: HashSet<_> = network.buses().iter().map(|b| b.id).collect();
        for b in &self.buses {
            if !ids.insert(&b.component)
                || !external.insert(b.external_id)
                || b.external_id == 0
                || !positive(b.base_kv)
                || !positive(b.initial_voltage_pu)
                || !limits(b.voltage_limits_pu)
                || !positive(b.voltage_limits_pu.min)
                || !finite(b.withdrawal_mw)
                || !finite(b.capacitance_pu)
                || b.capacitance_pu < 0.0
            {
                return Err(invalid(format!(
                    "invalid or duplicate DC bus {}",
                    b.component
                )));
            }
        }
        for b in &self.branches {
            if !ids.insert(&b.component)
                || !dc.contains(&b.from)
                || !dc.contains(&b.to)
                || b.from == b.to
                || !positive(b.resistance_pu)
                || ![b.inductance_pu_per_second, b.capacitance_pu_seconds]
                    .into_iter()
                    .chain(b.ratings_mw)
                    .all(|v| finite(v) && v >= 0.0)
            {
                return Err(invalid(format!(
                    "invalid DC branch {} or endpoint",
                    b.component
                )));
            }
        }
        for c in &self.stations {
            let s = &c.settings;
            if !ids.insert(&c.component)
                || !dc.contains(&c.dc_bus)
                || !ac.contains(&c.ac_bus)
                || !positive(c.ac_base_kv)
                || !finite(c.transformer_tap_ratio)
                || (c.transformer.enabled && !positive(c.transformer_tap_ratio))
                || !limits(c.valve_voltage_pu)
                || !positive(c.valve_voltage_pu.min)
                || !limits(c.valve_active_mw)
                || !limits(c.valve_reactive_mvar)
                || ![
                    c.valve_current_limit_pu,
                    c.losses.constant_mw,
                    c.losses.linear_kv,
                    c.losses.rectifier_quadratic_ohm,
                    c.losses.inverter_quadratic_ohm,
                    s.dc_voltage_deadband_pu,
                ]
                .into_iter()
                .all(|v| finite(v) && v >= 0.0)
                || ![
                    c.transformer.resistance_pu,
                    c.transformer.reactance_pu,
                    c.reactor.resistance_pu,
                    c.reactor.reactance_pu,
                    c.filter_susceptance_pu,
                    s.pcc_withdrawal_mw,
                    s.pcc_withdrawal_mvar,
                    s.declared_ac_voltage_pu,
                    s.droop,
                    s.dc_withdrawal_target_mw,
                    s.dc_voltage_target_pu,
                ]
                .into_iter()
                .all(finite)
            {
                return Err(invalid(format!(
                    "invalid converter {} or endpoint",
                    c.component
                )));
            }
        }
        Ok(())
    }
}

impl BalancedNetwork {
    /// Replace equipment atomically after checking all AC/DC references.
    pub fn set_ac_dc_equivalent(&mut self, equipment: Option<BalancedAcDcEquipment>) -> Result<()> {
        if let Some(ref equipment) = equipment {
            equipment.validate(self)?;
        }
        self.tables_mut().ac_dc_equivalent = equipment.map(std::sync::Arc::new);
        Ok(())
    }
    /// Refuse a legacy calculation or transformation before it can erase equipment.
    pub fn require_no_ac_dc_equivalent(&self) -> Result<()> {
        if self.ac_dc_equivalent().is_some_and(|e| {
            !e.buses.is_empty() || !e.branches.is_empty() || !e.stations.is_empty()
        }) {
            return Err(Error::UnsupportedAcDc);
        }
        Ok(())
    }
}

impl BalancedNetwork {
    pub(crate) fn update_acdc_stations(
        &mut self,
        touches: impl Fn(&AcDcConverterStation) -> bool,
        mut update: impl FnMut(&mut AcDcConverterStation),
    ) {
        if self
            .ac_dc_equivalent()
            .is_some_and(|e| e.stations.iter().any(&touches))
        {
            let e = self
                .tables_mut()
                .ac_dc_equivalent
                .as_mut()
                .expect("checked equipment");
            for c in &mut std::sync::Arc::make_mut(e).stations {
                if touches(c) {
                    update(c);
                }
            }
        }
    }
}
