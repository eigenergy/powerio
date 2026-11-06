//! The PowerModelsDistribution MATHEMATICAL model as JSON (`pmd-math-json`).
//!
//! PMD's `transform_data_model` lowers an ENGINEERING model to the index
//! based, per unit MATHEMATICAL model its optimization problems are built
//! on: integer keyed `bus`, `branch`, `switch`, `transformer`, `gen`,
//! `load`, `shunt`, and `storage` tables, every value per unit on a bus
//! voltage base and the system power base, angles in radians, and
//! transformers decomposed into ideal two-winding units joined by a loss
//! model of virtual buses and branches. The `map` table records which
//! MATHEMATICAL objects each ENGINEERING object became, and `bus_lookup`
//! names each bus's index.
//!
//! The writer performs that lowering from the typed network,
//! following `eng2math.jl`, `make_per_unit!`, and `calc_voltage_bases` of PMD
//! v0.16: the same object names, `source_id`s, virtual element names, and
//! per unit conventions, so PMD reads the output as its own and the oracle
//! tests compare the two value by value. The reader inverts the
//! per unit scaling with the document's own bases and folds the virtual
//! elements back through `map`, so a MATHEMATICAL document parses to the
//! same [`MulticonductorNetwork`] its ENGINEERING source would, with the
//! irreversible parts (winding power ratings, line lengths, Kron reduced
//! neutrals) restated and reported.
//!
//! [`MulticonductorNetwork`]: crate::MulticonductorNetwork

pub(crate) mod read;
pub(crate) mod write;

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// The emission policy of the MATHEMATICAL writer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PmdMathEmitOptions {
    /// Kron reduce the grounded neutral before lowering, as PMD's
    /// `transform_data_model` does by default. `false` keeps every conductor
    /// (PMD's `kron_reduce=false`).
    pub kron_reduce: bool,
}

impl Default for PmdMathEmitOptions {
    fn default() -> Self {
        Self { kron_reduce: true }
    }
}

/// The ENGINEERING `settings` record that fixes the per unit system: the
/// power base in `power_scale_factor` units (kVA), the voltage scale factor
/// (kV), the frequency, and the stated voltage bases per bus (kV, line to
/// neutral).
#[derive(Clone, Debug)]
pub(crate) struct Settings {
    pub(crate) sbase: f64,
    pub(crate) voltage_scale_factor: f64,
    pub(crate) power_scale_factor: f64,
    pub(crate) base_frequency: f64,
    /// Stated voltage bases by lowercased bus id, kV line to neutral.
    pub(crate) vbases_default: BTreeMap<String, f64>,
    /// The record as stated, for the output's `settings`.
    pub(crate) raw: Map<String, Value>,
}

impl Settings {
    /// The settings an ENGINEERING read retained, else the ones the
    /// ENGINEERING writer synthesizes for a dss or BMOPF sourced network.
    pub(crate) fn of(net: &crate::MulticonductorNetwork) -> Self {
        let raw = match net.extras().get("pmd_settings") {
            Some(Value::Object(settings)) => settings.clone(),
            _ => match super::write::synthesized_settings(net) {
                Value::Object(settings) => settings,
                _ => Map::new(),
            },
        };
        let number = |key: &str, default: f64| {
            raw.get(key)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(default)
        };
        let vbases_default = raw
            .get("vbases_default")
            .and_then(Value::as_object)
            .map(|table| {
                table
                    .iter()
                    .filter_map(|(bus, kv)| Some((bus.to_ascii_lowercase(), kv.as_f64()?)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            sbase: number("sbase", number("sbase_default", 1.0)),
            voltage_scale_factor: number("voltage_scale_factor", 1000.0),
            power_scale_factor: number("power_scale_factor", 1000.0),
            base_frequency: number("base_frequency", net.base_frequency()),
            vbases_default,
            raw,
        }
    }
}

/// The MATHEMATICAL `model` enumeration of a load law.
pub(crate) const LOAD_MODEL_POWER: &str = "POWER";
pub(crate) const LOAD_MODEL_IMPEDANCE: &str = "IMPEDANCE";
pub(crate) const LOAD_MODEL_CURRENT: &str = "CURRENT";

/// PMD's null restoration applies to MATHEMATICAL documents too: an absent
/// or `null` bound restores by its key's suffix.
pub(crate) fn restore(key: &str, v: &Value) -> f64 {
    super::read::restore_null(key, v)
}
