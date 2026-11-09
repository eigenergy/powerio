//! Canonical emission of Egret's scalar electrical snapshot sequence.

use std::collections::BTreeSet;
use std::time::Duration;

use powerio_core::{ArtifactPath, Destination, EmitResult, MemoryArtifact, TimeSeries};
use serde_json::{Map, Value, json};

use super::{BalancedNetwork, Diagnostics, F, FMT, bad, finish, write_document};
use crate::{Error, Result};

pub(super) fn read_duration(system: &Map<String, Value>) -> Result<Option<Duration>> {
    let Some(value) = system.get("time_period_length_minutes") else {
        return Ok(None);
    };
    let minutes = value
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| bad("system.time_period_length_minutes must be positive and numeric"))?;
    let duration = Duration::try_from_secs_f64(minutes * 60.0)
        .map_err(|_| bad("system.time_period_length_minutes is outside the duration range"))?;
    if duration.is_zero() {
        return Err(bad(
            "system.time_period_length_minutes is below the duration resolution",
        ));
    }
    Ok(Some(duration))
}

pub(super) fn read_time_points(
    system: &Map<String, Value>,
) -> Result<Vec<powerio_core::TimePoint>> {
    let time_keys: Vec<String> = match system.get("time_keys") {
        Some(Value::Array(keys)) => keys
            .iter()
            .map(|k| match k {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        Some(other) => {
            return Err(bad(format!("`system.time_keys` is not an array: {other}")));
        }
        None => {
            return Err(bad(
                "`system.time_keys` is absent; parse the document as one scalar snapshot",
            ));
        }
    };
    if time_keys
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != time_keys.len()
    {
        return Err(bad("system.time_keys contains duplicate labels"));
    }
    let duration = read_duration(system)?;
    for (key, value) in system {
        if super::is_time_series(value) {
            return Err(bad(format!(
                "time varying system.{key} is outside the supported scalar network profile"
            )));
        }
    }
    if time_keys.is_empty() {
        return Err(bad("`system.time_keys` is empty"));
    }

    time_keys
        .into_iter()
        .map(|key| powerio_core::TimePoint::new(key, duration))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| bad(error.to_string()))
}

fn refused(message: impl Into<String>) -> Error {
    Error::Emit {
        format: FMT,
        message: message.into(),
    }
}

fn varying_field(section: &str, attribute: &str) -> bool {
    match section {
        "load" => matches!(attribute, "p_load" | "q_load" | "in_service"),
        "generator" => matches!(
            attribute,
            "pg" | "qg" | "p_min" | "p_max" | "q_min" | "q_max" | "vg" | "in_service"
        ),
        "shunt" => matches!(attribute, "gs" | "bs" | "in_service"),
        "branch" => matches!(
            attribute,
            "rating_long_term"
                | "rating_short_term"
                | "rating_emergency"
                | "transformer_tap_ratio"
                | "transformer_phase_shift"
                | "in_service"
                | "pf"
                | "qf"
                | "pt"
                | "qt"
        ),
        "bus" => matches!(attribute, "vm" | "va" | "v_min" | "v_max"),
        _ => false,
    }
}

/// Combine complete scalar statements. Only explicitly supported element
/// attributes may vary; changing identities, topology, or an unmodeled field
/// is refused rather than flattened to the first point.
fn combine(values: &[&Value], path: &mut Vec<String>) -> Result<Value> {
    let first = values[0];
    if values.iter().all(|value| *value == first) {
        return Ok(first.clone());
    }
    if path.len() == 4 && path[0] == "elements" {
        if varying_field(&path[1], &path[3]) {
            return Ok(json!({"data_type": "time_series", "values": values}));
        }
        return Err(refused(format!(
            "{} varies outside the supported electrical time-series profile",
            path.join(".")
        )));
    }
    let maps = values
        .iter()
        .map(|value| {
            value
                .as_object()
                .ok_or_else(|| refused(format!("{} must remain static", path.join("."))))
        })
        .collect::<Result<Vec<_>>>()?;
    let keys: BTreeSet<_> = maps.iter().flat_map(|map| map.keys()).collect();
    let mut out = Map::new();
    for key in keys {
        let missing_rating = path.len() == 3
            && path[0] == "elements"
            && path[1] == "branch"
            && matches!(
                key.as_str(),
                "rating_long_term" | "rating_short_term" | "rating_emergency"
            );
        let unlimited = json!(0.0);
        let fields = maps
            .iter()
            .map(|map| {
                map.get(key)
                    .or(missing_rating.then_some(&unlimited))
                    .ok_or_else(|| {
                        refused(format!(
                            "{}.{} is missing at one or more time points",
                            path.join("."),
                            key
                        ))
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        path.push(key.clone());
        let value = combine(&fields, path)?;
        path.pop();
        out.insert(key.clone(), value);
    }
    Ok(Value::Object(out))
}

fn write(series: &TimeSeries<BalancedNetwork>) -> Result<super::super::TextEmission> {
    if series.is_empty() {
        return Err(refused("an Egret time series needs at least one point"));
    }
    let labels = series
        .time_points()
        .iter()
        .map(powerio_core::TimePoint::label)
        .collect::<Vec<_>>();
    if labels.iter().collect::<BTreeSet<_>>().len() != labels.len() {
        return Err(refused("Egret time keys must be unique"));
    }
    let duration = series.time_points()[0].duration();
    if series
        .time_points()
        .iter()
        .any(|point| point.duration() != duration)
    {
        return Err(refused(
            "Egret's electrical profile requires one common time-period duration",
        ));
    }
    let mut warnings = Diagnostics::new();
    let documents = series
        .values()
        .iter()
        .map(|network| {
            network.validate()?;
            Ok(Value::Object(write_document(network, &mut warnings)))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut root = combine(&documents.iter().collect::<Vec<_>>(), &mut Vec::new())?
        .as_object()
        .expect("scalar documents are objects")
        .clone();
    let system = root
        .get_mut("system")
        .and_then(Value::as_object_mut)
        .expect("writer states system");
    system.insert("time_keys".into(), json!(labels));
    if let Some(duration) = duration {
        system.insert(
            "time_period_length_minutes".into(),
            json!(duration.as_secs_f64() / 60.0),
        );
        if read_duration(system).ok().flatten() != Some(duration) {
            return Err(refused(
                "time-period duration cannot be represented exactly in Egret minutes",
            ));
        }
    }
    Ok(finish(&F, root, warnings))
}

/// Internal facade bridge for canonical Egret electrical time-series output.
///
/// # Errors
/// Refuses changing topology, unsupported variation, or incompatible durations;
/// also returns destination errors. Same-format byte replay belongs to the facade.
pub fn emit_time_series(
    series: &TimeSeries<BalancedNetwork>,
    destination: Destination,
) -> std::result::Result<EmitResult, powerio_core::Error> {
    let output = write(series).map_err(super::super::core_error)?;
    destination.__commit_artifacts(
        false,
        output.fidelity,
        vec![MemoryArtifact::new(
            ArtifactPath::new("case.egret.json")?,
            output.text.into_bytes(),
        )],
        output.diagnostics,
    )
}
