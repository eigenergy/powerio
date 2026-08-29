//! The `.pio.json` version 1 wire: one stored document version, decoded by
//! exact typed DTOs after header dispatch. Runtime types never derive this
//! layout; the mapping in [`super::convert`] is the one bridge.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{DeserializeSeed, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use powerio_core::limits;

use crate::BalancedNetwork;
use powerio_dist::MulticonductorNetwork;

#[cfg(feature = "schema")]
use schemars::JsonSchema;

pub const SCHEMA_NAME: &str = "powerio.module";
pub const SCHEMA_VERSION: u32 = 1;

/// JSON has no nonfinite number literals. PowerIO keeps the spellings already
/// shipped by 0.9 (`"Infinity"`, `"-Infinity"`, `"NaN"`) instead of turning
/// valid open bounds into `null`; `null` is not a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoredF64(pub f64);

impl Serialize for StoredF64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            value if value.is_finite() => serializer.serialize_f64(value),
            value if value == f64::INFINITY => serializer.serialize_str("Infinity"),
            value if value == f64::NEG_INFINITY => serializer.serialize_str("-Infinity"),
            _ => serializer.serialize_str("NaN"),
        }
    }
}

struct StoredF64Visitor;

impl Visitor<'_> for StoredF64Visitor {
    type Value = StoredF64;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON number or \"Infinity\", \"-Infinity\", or \"NaN\"")
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(StoredF64(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        #[allow(clippy::cast_precision_loss)]
        Ok(StoredF64(value as f64))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        #[allow(clippy::cast_precision_loss)]
        Ok(StoredF64(value as f64))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        match value {
            "Infinity" => Ok(StoredF64(f64::INFINITY)),
            "-Infinity" => Ok(StoredF64(f64::NEG_INFINITY)),
            "NaN" => Ok(StoredF64(f64::NAN)),
            _ => Err(E::custom(format!("invalid stored float `{value}`"))),
        }
    }
}

impl<'de> Deserialize<'de> for StoredF64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StoredF64Visitor)
    }
}

#[cfg(feature = "schema")]
impl JsonSchema for StoredF64 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("StoredF64")
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // The stated union: a JSON number, or exactly the three nonfinite
        // spellings. `null` is not a number.
        schemars::json_schema!({
            "anyOf": [
                {"type": "number"},
                {"enum": ["Infinity", "-Infinity", "NaN"]}
            ]
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProducerV1 {
    pub name: String,
    pub version: String,
}

// Decode time bounds: every sequence, map, and string on the version 1 record
// wire is refused or truncated at its limit while it is decoded, before the
// full collection has been retained, matching the core record wire.
fn bounded_identifier<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    limits::BoundedStr {
        what: "record identifier",
        max_bytes: limits::MAX_IDENTIFIER_BYTES,
    }
    .deserialize(deserializer)
}

fn bounded_code<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    limits::BoundedStr {
        what: "diagnostic code",
        max_bytes: limits::MAX_DIAGNOSTIC_CODE_BYTES,
    }
    .deserialize(deserializer)
}

fn truncated_message<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    deserializer.deserialize_str(limits::TruncatedStr {
        max_bytes: limits::MAX_DIAGNOSTIC_MESSAGE_DECODE_BYTES,
    })
}

fn bounded_target<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    limits::BoundedStr {
        what: "record target",
        max_bytes: limits::MAX_DIAGNOSTIC_TARGET_BYTES,
    }
    .deserialize(deserializer)
}

fn bounded_opt_target<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    struct OptTarget;
    impl<'de> Visitor<'de> for OptTarget {
        type Value = Option<String>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded target string or null")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(
            self,
            deserializer: D2,
        ) -> Result<Self::Value, D2::Error> {
            bounded_target(deserializer).map(Some)
        }
    }
    deserializer.deserialize_option(OptTarget)
}

fn bounded_name<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    bounded_identifier(deserializer)
}

fn bounded_opt_action<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    struct OptAction;
    impl<'de> Visitor<'de> for OptAction {
        type Value = Option<String>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded suggested action string or null")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D2: Deserializer<'de>>(
            self,
            deserializer: D2,
        ) -> Result<Self::Value, D2::Error> {
            truncated_message(deserializer).map(Some)
        }
    }
    deserializer.deserialize_option(OptAction)
}

fn bounded_btree_map<'de, D: Deserializer<'de>>(
    deserializer: D,
    what: &'static str,
    max_keys: usize,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    let map = limits::bounded_json_map(
        deserializer,
        what,
        max_keys,
        limits::MAX_IDENTIFIER_BYTES,
        |key| !key.is_empty() && !key.contains('\0'),
    )?;
    Ok(map.into_iter().collect())
}

fn bounded_details<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    bounded_btree_map(
        deserializer,
        "detail keys",
        limits::MAX_DIAGNOSTIC_DETAIL_KEYS,
    )
}

fn bounded_parameters<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    bounded_btree_map(
        deserializer,
        "parameter keys",
        limits::MAX_HISTORY_PARAMETERS,
    )
}

fn bounded_extensions<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, serde_json::Value>, D::Error> {
    bounded_btree_map(
        deserializer,
        "extension keys",
        limits::MAX_MODULE_EXTENSION_KEYS,
    )
}

fn bounded_diagnostic_spans<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SourceSpanV1>, D::Error> {
    limits::bounded_vec(deserializer, "source spans", limits::MAX_DIAGNOSTIC_SPANS)
}

fn bounded_map_spans<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SourceSpanV1>, D::Error> {
    limits::bounded_vec(deserializer, "source spans", limits::MAX_SOURCE_MAP_SPANS)
}

fn bounded_related<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<DiagnosticIdV1>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "related diagnostics",
        limits::MAX_DIAGNOSTIC_RELATED,
    )
}

fn bounded_notes<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    limits::bounded_vec(deserializer, "history notes", limits::MAX_HISTORY_NOTES)
}

fn bounded_sources<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SourceDescriptorV1>, D::Error> {
    limits::bounded_vec(deserializer, "sources", limits::MAX_MODULE_SOURCES)
}

fn bounded_source_map<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<SourceMapEntryV1>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "source map entries",
        limits::MAX_MODULE_SOURCE_MAP_ENTRIES,
    )
}

fn bounded_diagnostics<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<DiagnosticV1>, D::Error> {
    limits::bounded_vec(deserializer, "diagnostics", limits::MAX_MODULE_DIAGNOSTICS)
}

fn bounded_history<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<HistoryEntryV1>, D::Error> {
    limits::bounded_vec(
        deserializer,
        "history entries",
        limits::MAX_MODULE_HISTORY_ENTRIES,
    )
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct SourceIdV1(#[serde(deserialize_with = "bounded_identifier")] pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct DiagnosticIdV1(#[serde(deserialize_with = "bounded_identifier")] pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(transparent)]
pub struct HistoryIdV1(#[serde(deserialize_with = "bounded_identifier")] pub String);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TimePointV1 {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<DurationV1>,
}

/// A stored duration: unsigned seconds plus a nanosecond remainder below one
/// billion, exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DurationV1 {
    pub secs: u64,
    pub nanos: u32,
}

/// One state quantity's dense columns: the resolved identity order, then the
/// point major values (`time_points.len() × identities.len()`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredQuantityV1 {
    pub identities: Vec<String>,
    pub values: Vec<StoredF64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BalancedOperatingPointTimeSeriesV1 {
    pub network: Box<BalancedNetwork>,
    pub time_points: Vec<TimePointV1>,
    /// Quantity name → dense columns, the balanced instantaneous vocabulary.
    pub quantities: BTreeMap<String, StoredQuantityV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BalancedNetworkTimeSeriesV1 {
    pub time_points: Vec<TimePointV1>,
    pub values: Vec<BalancedNetwork>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BalancedNetworkScenarioV1 {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probability: Option<StoredF64>,
    pub value: BalancedNetwork,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BalancedNetworkScenarioSetV1 {
    pub scenarios: Vec<BalancedNetworkScenarioV1>,
}

/// One stored operating point: the state quantities of a single point,
/// keyed by the instantaneous vocabulary, each with its resolved identity
/// order and one row of values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredOperatingPointV1 {
    pub quantities: BTreeMap<String, StoredQuantityV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MulticonductorOperatingPointTimeSeriesV1 {
    pub network: Box<MulticonductorNetwork>,
    pub time_points: Vec<TimePointV1>,
    /// Quantity name → dense columns, the multiconductor instantaneous
    /// vocabulary.
    pub quantities: BTreeMap<String, StoredQuantityV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcPfInstanceV1 {
    pub network: Box<BalancedNetwork>,
    /// The selected branch susceptance formula's stable name.
    pub approximation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_state: Option<StoredOperatingPointV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcPfInstanceV1 {
    pub network: Box<BalancedNetwork>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_state: Option<StoredOperatingPointV1>,
}

/// One typed objective term, mirroring `powerio_prob::ObjectiveTerm` with its
/// weight wrapped for the nonfinite spelling: an internally tagged enum's
/// derived `Deserialize` re-decodes its variant from a buffered generic
/// value rather than the deserializer callers pass in, so wrapping the whole
/// document's deserializer (as [`StoredModuleV1`] does for every plain
/// struct field) never reaches this `f64`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case", tag = "term", deny_unknown_fields)]
pub enum ObjectiveTermV1 {
    NetworkGeneratorCost,
    NetworkPerPhaseCost,
    DifferentiabilityRegularization { weight: StoredF64 },
}

/// The complete typed objective, mirroring `powerio_prob::Objective`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveV1 {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<ObjectiveTermV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcOpfInstanceV1 {
    pub network: Box<BalancedNetwork>,
    pub approximation: String,
    /// The typed objective the instance states, in the calculation crate's
    /// own serialization.
    pub objective: ObjectiveV1,
    pub constraints: powerio_prob::ActiveConstraints,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_state: Option<StoredOperatingPointV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcOpfInstanceV1 {
    pub network: Box<BalancedNetwork>,
    pub objective: ObjectiveV1,
    pub constraints: powerio_prob::ActiveConstraints,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_state: Option<StoredOperatingPointV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct McAcPfInstanceV1 {
    pub network: Box<MulticonductorNetwork>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_state: Option<StoredOperatingPointV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct McAcOpfInstanceV1 {
    pub network: Box<MulticonductorNetwork>,
    pub objective: ObjectiveV1,
    pub constraints: powerio_prob::MulticonductorActiveConstraints,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_state: Option<StoredOperatingPointV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcScucInstanceV1 {
    pub network: Box<BalancedNetwork>,
    /// The complete SCUC inputs, in the calculation crate's own
    /// serialization, typed through this document's schema.
    pub inputs: Box<powerio_prob::ScucInputs>,
}

/// A solution's producer stated generator dispatch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GeneratorDispatchV1 {
    pub p_mw: Vec<StoredF64>,
    pub q_mvar: Vec<StoredF64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcPfSolutionV1 {
    pub instance: DcPfInstanceV1,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub bus_voltage_angle: Vec<StoredF64>,
    pub bus_active_injection: Vec<StoredF64>,
    pub branch_from_active_flow: Vec<StoredF64>,
    pub branch_to_active_flow: Vec<StoredF64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generator_dispatch: Option<GeneratorDispatchV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcPfSolutionV1 {
    pub instance: AcPfInstanceV1,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub bus_voltage_magnitude: Vec<StoredF64>,
    pub bus_voltage_angle: Vec<StoredF64>,
    pub bus_active_injection: Vec<StoredF64>,
    pub bus_reactive_injection: Vec<StoredF64>,
    pub branch_from_active_flow: Vec<StoredF64>,
    pub branch_from_reactive_flow: Vec<StoredF64>,
    pub branch_to_active_flow: Vec<StoredF64>,
    pub branch_to_reactive_flow: Vec<StoredF64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generator_dispatch: Option<GeneratorDispatchV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DcOpfSolutionV1 {
    pub instance: DcOpfInstanceV1,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub bus_voltage_angle: Vec<StoredF64>,
    pub bus_active_injection: Vec<StoredF64>,
    pub branch_from_active_flow: Vec<StoredF64>,
    pub branch_to_active_flow: Vec<StoredF64>,
    pub generator_active_power: Vec<StoredF64>,
    pub objective: StoredF64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcOpfSolutionV1 {
    pub instance: AcOpfInstanceV1,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub bus_voltage_magnitude: Vec<StoredF64>,
    pub bus_voltage_angle: Vec<StoredF64>,
    pub bus_active_injection: Vec<StoredF64>,
    pub bus_reactive_injection: Vec<StoredF64>,
    pub branch_from_active_flow: Vec<StoredF64>,
    pub branch_from_reactive_flow: Vec<StoredF64>,
    pub branch_to_active_flow: Vec<StoredF64>,
    pub branch_to_reactive_flow: Vec<StoredF64>,
    pub generator_active_power: Vec<StoredF64>,
    pub generator_reactive_power: Vec<StoredF64>,
    pub objective: StoredF64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct McAcPfSolutionV1 {
    pub instance: McAcPfInstanceV1,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub terminal_voltage_magnitude: Vec<StoredF64>,
    pub terminal_voltage_angle: Vec<StoredF64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_current_magnitude: Option<Vec<StoredF64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_active_power: Option<Vec<StoredF64>>,
    pub source_active_injection: Vec<StoredF64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct McAcOpfSolutionV1 {
    pub instance: McAcOpfInstanceV1,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub terminal_voltage_magnitude: Vec<StoredF64>,
    pub terminal_voltage_angle: Vec<StoredF64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_current_magnitude: Option<Vec<StoredF64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_active_power: Option<Vec<StoredF64>>,
    pub source_active_injection: Vec<StoredF64>,
    pub generator_active_power: Vec<StoredF64>,
    pub objective: StoredF64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ScucNetworkOutputsV1 {
    pub bus_vm: Vec<Vec<StoredF64>>,
    pub bus_va: Vec<Vec<StoredF64>>,
    pub shunt_step: Vec<Vec<StoredF64>>,
    pub ac_line_on_status: Vec<Vec<StoredF64>>,
    pub transformer_tm: Vec<Vec<StoredF64>>,
    pub transformer_ta: Vec<Vec<StoredF64>>,
    pub transformer_on_status: Vec<Vec<StoredF64>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dc_line_pdc_fr: Vec<Vec<StoredF64>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dc_line_qdc_fr: Vec<Vec<StoredF64>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dc_line_qdc_to: Vec<Vec<StoredF64>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ScucDeviceOutputsV1 {
    pub on_status: Vec<Vec<StoredF64>>,
    pub p_on: Vec<Vec<StoredF64>>,
    pub q: Vec<Vec<StoredF64>>,
    pub p_reg_res_up: Vec<Vec<StoredF64>>,
    pub p_reg_res_down: Vec<Vec<StoredF64>>,
    pub p_syn_res: Vec<Vec<StoredF64>>,
    pub p_nsyn_res: Vec<Vec<StoredF64>>,
    pub p_ramp_res_up_online: Vec<Vec<StoredF64>>,
    pub p_ramp_res_down_online: Vec<Vec<StoredF64>>,
    pub q_res_up: Vec<Vec<StoredF64>>,
    pub q_res_down: Vec<Vec<StoredF64>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AcScucSolutionV1 {
    pub instance: AcScucInstanceV1,
    pub termination: powerio_prob::Termination,
    pub residuals: powerio_prob::Residuals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    pub network_outputs: ScucNetworkOutputsV1,
    pub device_outputs: ScucDeviceOutputsV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<StoredF64>,
}

/// The tagged value DTO. `data` is a typed record, never an untyped JSON
/// catchall; the network payloads are the typed serializations the network
/// crates own (which carry the nonfinite spellings themselves).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(
    deny_unknown_fields,
    tag = "kind",
    content = "data",
    rename_all = "snake_case"
)]
pub enum StoredValueV1 {
    BalancedNetwork(Box<BalancedNetwork>),
    MulticonductorNetwork(Box<MulticonductorNetwork>),
    BalancedNetworkTimeSeries(BalancedNetworkTimeSeriesV1),
    BalancedOperatingPointTimeSeries(BalancedOperatingPointTimeSeriesV1),
    MulticonductorOperatingPointTimeSeries(MulticonductorOperatingPointTimeSeriesV1),
    BalancedNetworkScenarioSet(BalancedNetworkScenarioSetV1),
    DcPfInstance(DcPfInstanceV1),
    AcPfInstance(AcPfInstanceV1),
    DcOpfInstance(DcOpfInstanceV1),
    AcOpfInstance(AcOpfInstanceV1),
    McAcPfInstance(McAcPfInstanceV1),
    McAcOpfInstance(McAcOpfInstanceV1),
    AcScucInstance(AcScucInstanceV1),
    DcPfSolution(Box<DcPfSolutionV1>),
    AcPfSolution(Box<AcPfSolutionV1>),
    DcOpfSolution(Box<DcOpfSolutionV1>),
    AcOpfSolution(Box<AcOpfSolutionV1>),
    McAcPfSolution(Box<McAcPfSolutionV1>),
    McAcOpfSolution(Box<McAcOpfSolutionV1>),
    AcScucSolution(Box<AcScucSolutionV1>),
}

impl StoredValueV1 {
    /// The permanent kind identifier the tag serializes.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BalancedNetwork(_) => "balanced_network",
            Self::MulticonductorNetwork(_) => "multiconductor_network",
            Self::BalancedNetworkTimeSeries(_) => "balanced_network_time_series",
            Self::BalancedOperatingPointTimeSeries(_) => "balanced_operating_point_time_series",
            Self::MulticonductorOperatingPointTimeSeries(_) => {
                "multiconductor_operating_point_time_series"
            }
            Self::BalancedNetworkScenarioSet(_) => "balanced_network_scenario_set",
            Self::DcPfInstance(_) => "dc_pf_instance",
            Self::AcPfInstance(_) => "ac_pf_instance",
            Self::DcOpfInstance(_) => "dc_opf_instance",
            Self::AcOpfInstance(_) => "ac_opf_instance",
            Self::McAcPfInstance(_) => "mc_ac_pf_instance",
            Self::McAcOpfInstance(_) => "mc_ac_opf_instance",
            Self::AcScucInstance(_) => "ac_scuc_instance",
            Self::DcPfSolution(_) => "dc_pf_solution",
            Self::AcPfSolution(_) => "ac_pf_solution",
            Self::DcOpfSolution(_) => "dc_opf_solution",
            Self::AcOpfSolution(_) => "ac_opf_solution",
            Self::McAcPfSolution(_) => "mc_ac_pf_solution",
            Self::McAcOpfSolution(_) => "mc_ac_opf_solution",
            Self::AcScucSolution(_) => "ac_scuc_solution",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceDescriptorV1 {
    pub id: SourceIdV1,
    pub name: String,
    pub byte_length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<DigestV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithmV1 {
    Sha256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DigestV1 {
    pub algorithm: DigestAlgorithmV1,
    /// Lowercase hexadecimal, without a prefix.
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceSpanV1 {
    pub source: SourceIdV1,
    pub byte_start: u64,
    pub byte_end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceRelationV1 {
    Exact,
    Defaulted,
    Inferred,
    ConvertedUnits,
    Aggregated,
    Split,
    Synthetic,
    Transformed,
    RetainedExtra,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceMapEntryV1 {
    /// RFC 6901 pointer into `value.data`.
    #[serde(deserialize_with = "bounded_target")]
    pub target: String,
    pub relation: SourceRelationV1,
    /// Empty only for `defaulted`, `synthetic`, or `transformed`.
    #[serde(
        default,
        deserialize_with = "bounded_map_spans",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub spans: Vec<SourceSpanV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SeverityV1 {
    Error,
    Warning,
    Remark,
    Note,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DiagnosticV1 {
    pub id: DiagnosticIdV1,
    pub severity: SeverityV1,
    #[serde(deserialize_with = "bounded_code")]
    pub code: String,
    #[serde(deserialize_with = "truncated_message")]
    pub message: String,
    #[serde(
        default,
        deserialize_with = "bounded_opt_target",
        skip_serializing_if = "Option::is_none"
    )]
    pub target: Option<String>,
    #[serde(
        default,
        deserialize_with = "bounded_diagnostic_spans",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub spans: Vec<SourceSpanV1>,
    #[serde(
        default,
        deserialize_with = "bounded_related",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub related: Vec<DiagnosticIdV1>,
    #[serde(
        default,
        deserialize_with = "bounded_opt_action",
        skip_serializing_if = "Option::is_none"
    )]
    pub suggested_action: Option<String>,
    #[serde(
        default,
        deserialize_with = "bounded_details",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub details: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum HistoryKindV1 {
    Parse,
    Upgrade,
    Transform,
    Edit,
    Repair,
}

/// Describes an operation that produced the current value. Not a replay
/// program; replayable revisions require their own typed value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HistoryEntryV1 {
    pub id: HistoryIdV1,
    pub kind: HistoryKindV1,
    /// Stable registered operation name.
    #[serde(deserialize_with = "bounded_name")]
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_kind: Option<String>,
    #[serde(
        default,
        deserialize_with = "bounded_parameters",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub parameters: BTreeMap<String, serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "bounded_notes",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub assumptions: Vec<String>,
    #[serde(
        default,
        deserialize_with = "bounded_notes",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub losses: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, remote = "Self")]
pub struct StoredModuleV1 {
    pub schema: String,
    pub version: u32,
    pub producer: ProducerV1,
    pub value: StoredValueV1,
    #[serde(
        default,
        deserialize_with = "bounded_sources",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub sources: Vec<SourceDescriptorV1>,
    #[serde(
        default,
        deserialize_with = "bounded_source_map",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub source_map: Vec<SourceMapEntryV1>,
    #[serde(
        default,
        deserialize_with = "bounded_diagnostics",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub diagnostics: Vec<DiagnosticV1>,
    #[serde(
        default,
        deserialize_with = "bounded_history",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub history: Vec<HistoryEntryV1>,
    /// Nonsemantic third party annotations. Keys must be namespaced.
    #[serde(
        default,
        deserialize_with = "bounded_extensions",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

// Same mechanism as the two network models and NetworkPackage: the whole
// document, including `value`'s powerio-prob calculation payloads (whose own
// f64/Option<f64> fields, defined outside this crate, are not individually
// wrapped in StoredF64), spells a nonfinite float as a string, so nothing
// this crate writes ever refuses to read back.
impl Serialize for StoredModuleV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        StoredModuleV1::serialize(
            self,
            powerio_core::__implementation::nonfinite::NonFiniteSer(serializer),
        )
    }
}

impl<'de> Deserialize<'de> for StoredModuleV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        StoredModuleV1::deserialize(powerio_core::__implementation::nonfinite::NonFiniteDe(
            deserializer,
        ))
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct StoredHeader {
    #[serde(default)]
    pub(super) schema: Option<String>,
    #[serde(default)]
    pub(super) version: Option<u32>,
    /// The 0.9 legacy shape identifies itself by this field instead.
    #[serde(default)]
    pub(super) powerio_version: Option<String>,
}

/// Structural validation of one decoded document: identities, digests,
/// spans, pointers, relation rules, namespaced extensions, and value shapes.
pub fn validate(module: &StoredModuleV1) -> Result<(), String> {
    validate_value(&module.value)?;
    // Pointer existence checks re-inflate the value only when something
    // actually targets it.
    let needs_targets = !module.source_map.is_empty()
        || module
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.target.is_some());
    // One generic representation, borrowed by every target check: the tagged
    // value serializes with its `data` key, so targets resolve under a `/data`
    // prefix instead of cloning the subtree out.
    let stored_value = if needs_targets {
        Some(serde_json::to_value(&module.value).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let value_data = stored_value.as_ref();

    let mut sources = BTreeMap::new();
    for source in &module.sources {
        nonempty("source id", &source.id.0)?;
        if sources
            .insert(source.id.0.clone(), source.byte_length)
            .is_some()
        {
            return Err(format!("duplicate source id `{}`", source.id.0));
        }
        nonempty("source name", &source.name)?;
        if let Some(digest) = &source.digest
            && (digest.value.len() != 64
                || !digest
                    .value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        {
            return Err(format!("invalid SHA-256 `{}`", digest.value));
        }
    }

    let mut diagnostics = BTreeSet::new();
    for diagnostic in &module.diagnostics {
        nonempty("diagnostic id", &diagnostic.id.0)?;
        if !diagnostics.insert(diagnostic.id.0.as_str()) {
            return Err(format!("duplicate diagnostic id `{}`", diagnostic.id.0));
        }
        validate_target(diagnostic.target.as_deref(), value_data)?;
        validate_spans(&diagnostic.spans, &sources)?;
    }
    for diagnostic in &module.diagnostics {
        for related in &diagnostic.related {
            if !diagnostics.contains(related.0.as_str()) {
                return Err(format!(
                    "diagnostic `{}` refers to unknown diagnostic `{}`",
                    diagnostic.id.0, related.0
                ));
            }
        }
    }

    for entry in &module.source_map {
        validate_target(Some(&entry.target), value_data)?;
        validate_spans(&entry.spans, &sources)?;
        let empty_allowed = matches!(
            entry.relation,
            SourceRelationV1::Defaulted
                | SourceRelationV1::Synthetic
                | SourceRelationV1::Transformed
        );
        if entry.spans.is_empty() && !empty_allowed {
            return Err(format!(
                "source map relation `{:?}` requires at least one span",
                entry.relation
            ));
        }
    }

    let mut history = BTreeSet::new();
    for entry in &module.history {
        nonempty("history id", &entry.id.0)?;
        if !history.insert(entry.id.0.as_str()) {
            return Err(format!("duplicate history id `{}`", entry.id.0));
        }
        nonempty("history operation name", &entry.name)?;
    }

    for namespace in module.extensions.keys() {
        if namespace.starts_with('.') || !namespace.contains('.') {
            return Err(format!("extension key `{namespace}` is not namespaced"));
        }
    }

    Ok(())
}

// One arm per stored kind, as in the decoder.
#[allow(clippy::too_many_lines)]
fn validate_value(value: &StoredValueV1) -> Result<(), String> {
    let points: &[TimePointV1] = match value {
        StoredValueV1::BalancedNetworkTimeSeries(series) => {
            if series.time_points.len() != series.values.len() {
                return Err(format!(
                    "balanced network time series has {} values for {} time points",
                    series.values.len(),
                    series.time_points.len()
                ));
            }
            if series.time_points.is_empty() {
                return Err("a time series needs at least one time point".to_owned());
            }
            &series.time_points
        }
        StoredValueV1::BalancedOperatingPointTimeSeries(series) => {
            if series.time_points.is_empty() {
                return Err("a time series needs at least one time point".to_owned());
            }
            for (name, quantity) in &series.quantities {
                let expected = series
                    .time_points
                    .len()
                    .checked_mul(quantity.identities.len())
                    .ok_or_else(|| format!("quantity `{name}` dimensions overflow"))?;
                if quantity.values.len() != expected {
                    return Err(format!(
                        "quantity `{name}` has {} values; expected {expected}",
                        quantity.values.len()
                    ));
                }
            }
            &series.time_points
        }
        StoredValueV1::BalancedNetworkScenarioSet(set) => {
            let mut ids = BTreeSet::new();
            for scenario in &set.scenarios {
                nonempty("scenario id", &scenario.id)?;
                if !ids.insert(scenario.id.as_str()) {
                    return Err(format!("duplicate scenario id `{}`", scenario.id));
                }
            }
            return Ok(());
        }
        StoredValueV1::MulticonductorOperatingPointTimeSeries(series) => {
            if series.time_points.is_empty() {
                return Err("a time series needs at least one time point".to_owned());
            }
            for (name, quantity) in &series.quantities {
                let expected = series
                    .time_points
                    .len()
                    .checked_mul(quantity.identities.len())
                    .ok_or_else(|| format!("quantity `{name}` dimensions overflow"))?;
                if quantity.values.len() != expected {
                    return Err(format!(
                        "quantity `{name}` has {} values; expected {expected}",
                        quantity.values.len()
                    ));
                }
            }
            &series.time_points
        }
        StoredValueV1::DcPfInstance(instance) => {
            validate_stored_point(instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::AcPfInstance(instance) => {
            validate_stored_point(instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::DcOpfInstance(instance) => {
            validate_stored_point(instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::AcOpfInstance(instance) => {
            validate_stored_point(instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::McAcPfInstance(instance) => {
            validate_stored_point(instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::McAcOpfInstance(instance) => {
            validate_stored_point(instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::DcPfSolution(solution) => {
            validate_stored_point(solution.instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::AcPfSolution(solution) => {
            validate_stored_point(solution.instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::DcOpfSolution(solution) => {
            validate_stored_point(solution.instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::AcOpfSolution(solution) => {
            validate_stored_point(solution.instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::McAcPfSolution(solution) => {
            validate_stored_point(solution.instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::McAcOpfSolution(solution) => {
            validate_stored_point(solution.instance.initial_state.as_ref())?;
            return Ok(());
        }
        StoredValueV1::AcScucInstance(_) | StoredValueV1::AcScucSolution(_) => return Ok(()),
        StoredValueV1::BalancedNetwork(_) | StoredValueV1::MulticonductorNetwork(_) => {
            return Ok(());
        }
    };
    for point in points {
        nonempty("time point label", &point.label)?;
        if let Some(duration) = point.duration
            && duration.nanos >= 1_000_000_000
        {
            return Err(format!(
                "time point `{}` has an invalid nanosecond remainder {}",
                point.label, duration.nanos
            ));
        }
    }
    Ok(())
}

/// One stored operating point: each quantity carries exactly one row over
/// its identities.
fn validate_stored_point(point: Option<&StoredOperatingPointV1>) -> Result<(), String> {
    let Some(point) = point else {
        return Ok(());
    };
    for (name, quantity) in &point.quantities {
        if quantity.values.len() != quantity.identities.len() {
            return Err(format!(
                "stored operating point quantity `{name}` has {} values for {} identities",
                quantity.values.len(),
                quantity.identities.len()
            ));
        }
    }
    Ok(())
}

fn validate_spans(spans: &[SourceSpanV1], sources: &BTreeMap<String, u64>) -> Result<(), String> {
    for span in spans {
        let Some(byte_length) = sources.get(span.source.0.as_str()) else {
            return Err(format!("unknown source id `{}`", span.source.0));
        };
        if span.byte_start > span.byte_end {
            return Err(format!(
                "source span {}..{} is reversed",
                span.byte_start, span.byte_end
            ));
        }
        if span.byte_end > *byte_length {
            return Err(format!(
                "source span end {} exceeds source length {}",
                span.byte_end, byte_length
            ));
        }
    }
    Ok(())
}

fn validate_pointer(pointer: Option<&str>) -> Result<(), String> {
    if let Some(pointer) = pointer {
        if !pointer.is_empty() && !pointer.starts_with('/') {
            return Err(format!("`{pointer}` is not an RFC 6901 pointer"));
        }
        let bytes = pointer.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'~'
                && (index + 1 == bytes.len() || !matches!(bytes[index + 1], b'0' | b'1'))
            {
                return Err(format!("`{pointer}` is not an RFC 6901 pointer"));
            }
            index += 1;
        }
    }
    Ok(())
}

fn validate_target(
    pointer: Option<&str>,
    stored_value: Option<&serde_json::Value>,
) -> Result<(), String> {
    validate_pointer(pointer)?;
    if let (Some(pointer), Some(value)) = (pointer, stored_value)
        && value.pointer(&format!("/data{pointer}")).is_none()
    {
        return Err(format!(
            "`{pointer}` does not identify a field in value.data"
        ));
    }
    Ok(())
}

fn nonempty(what: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{what} cannot be empty"));
    }
    Ok(())
}
