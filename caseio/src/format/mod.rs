//! Convert a parsed [`Network`](crate::Network) into other interchange formats.
//!
//! Each converter is an independent writer over the shared `Network`: every
//! input format and every output format meet at the hub, so a new target is
//! one writer here, not a change to any parser. Non-finite numeric values (a
//! MATPOWER `Inf`/`NaN` angle limit, say) are written as JSON `null`.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::network::{Network, SourceFormat};

mod egret;
mod powermodels;
mod powerworld;
mod psse;

pub use egret::write_egret_json;
pub use powermodels::{parse_powermodels_json, write_powermodels_json};
pub use powerworld::{parse_powerworld, write_powerworld};
pub use psse::{parse_psse, write_psse};

/// A target interchange format. See [`write_as`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFormat {
    /// PowerModels.jl network data JSON.
    PowerModelsJson,
    /// EGRET `ModelData` JSON.
    EgretJson,
    /// PSS/E `.raw` (v33).
    Psse,
    /// PowerWorld auxiliary `.aux`.
    PowerWorld,
    /// MATPOWER `.m` (round-trip; byte-exact when the case kept its source).
    Matpower,
}

impl TargetFormat {
    /// Conventional file extension for this format (no leading dot).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            TargetFormat::PowerModelsJson | TargetFormat::EgretJson => "json",
            TargetFormat::Psse => "raw",
            TargetFormat::PowerWorld => "aux",
            TargetFormat::Matpower => "m",
        }
    }
}

/// Output of a conversion: the serialized text plus any fidelity warnings —
/// data the target can't represent, defaults synthesized, or blocks mapped
/// best-effort. An empty `warnings` means a faithful conversion.
#[derive(Debug, Clone)]
pub struct Conversion {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Convert a [`Network`] to `format`. Writing back to the source format echoes
/// the retained source byte-for-byte (the same-format leg of the fidelity
/// contract); otherwise the network is serialized into the target.
#[must_use]
pub fn write_as(net: &Network, format: TargetFormat) -> Conversion {
    if same_format(format, net.source_format) {
        if let Some(src) = &net.source {
            return Conversion { text: src.to_string(), warnings: Vec::new() };
        }
    }
    match format {
        TargetFormat::PowerModelsJson => write_powermodels_json(net),
        TargetFormat::EgretJson => write_egret_json(net),
        TargetFormat::Psse => write_psse(net),
        TargetFormat::PowerWorld => write_powerworld(net),
        // From another source (or no retained source): canonical MATPOWER from
        // the folded model.
        TargetFormat::Matpower => Conversion {
            text: crate::write_matpower(net),
            warnings: Vec::new(),
        },
    }
}

/// Whether a write target is the same format the network was read from.
fn same_format(target: TargetFormat, source: SourceFormat) -> bool {
    matches!(
        (target, source),
        (TargetFormat::Matpower, SourceFormat::Matpower)
            | (TargetFormat::PowerModelsJson, SourceFormat::PowerModelsJson)
            | (TargetFormat::EgretJson, SourceFormat::EgretJson)
            | (TargetFormat::Psse, SourceFormat::Psse)
            | (TargetFormat::PowerWorld, SourceFormat::PowerWorld)
    )
}

/// JSON number for a finite `f64`; `Value::Null` for `NaN`/`±Inf`.
pub(crate) fn jnum(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

/// Serialize a built JSON tree into a [`Conversion`], appending one warning that
/// names every field where a non-finite `f64` was written as `null` (JSON has no
/// `±Inf`/`NaN`). Shared by the JSON writers.
pub(crate) fn finish(root: Map<String, Value>, mut warnings: Vec<String>) -> Conversion {
    let value = Value::Object(root);
    let mut nulls = BTreeSet::new();
    collect_null_keys(&value, &mut nulls);
    if !nulls.is_empty() {
        warnings.push(format!(
            "non-finite numeric values written as JSON null in field(s): {}",
            nulls.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let text =
        serde_json::to_string_pretty(&value).expect("a serde_json::Value always serializes");
    Conversion { text, warnings }
}

/// Collect the names of object keys whose value is `null`, anywhere in the tree.
fn collect_null_keys(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                if val.is_null() {
                    out.insert(key.clone());
                } else {
                    collect_null_keys(val, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_null_keys(v, out)),
        _ => {}
    }
}
