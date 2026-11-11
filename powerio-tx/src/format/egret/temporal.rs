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

/// Append one scalar statement without retaining a full document per point.
/// Static fields are checked on every point; only supported varying fields grow.
fn append_point(
    output: &mut Value,
    next: &Value,
    point: usize,
    path: &mut Vec<String>,
) -> Result<()> {
    if path.len() == 4 && path[0] == "elements" && varying_field(&path[1], &path[3]) {
        if let Some(values) = output.get_mut("values").and_then(Value::as_array_mut) {
            values.push(next.clone());
        } else if output != next {
            let mut values = vec![output.clone(); point];
            values.push(next.clone());
            *output = json!({"data_type": "time_series", "values": values});
        }
        return Ok(());
    }
    if output == next {
        return Ok(());
    }
    if path.len() == 4 && path[0] == "elements" {
        return Err(refused(format!(
            "{} varies outside the supported electrical time-series profile",
            path.join(".")
        )));
    }
    let out = output
        .as_object_mut()
        .ok_or_else(|| refused(format!("{} must remain static", path.join("."))))?;
    let map = next
        .as_object()
        .ok_or_else(|| refused(format!("{} must remain static", path.join("."))))?;
    let keys: BTreeSet<_> = out.keys().chain(map.keys()).cloned().collect();
    for key in keys {
        let missing_rating = path.len() == 3
            && path[0] == "elements"
            && path[1] == "branch"
            && matches!(
                key.as_str(),
                "rating_long_term" | "rating_short_term" | "rating_emergency"
            );
        // Egret skips a thermal constraint only for None (JSON null).
        // A numeric zero would enforce zero flow at this time point.
        let unlimited = Value::Null;
        if missing_rating {
            out.entry(key.clone()).or_insert_with(|| unlimited.clone());
        }
        let value = map.get(&key).or(missing_rating.then_some(&unlimited));
        let Some((prior, value)) = out.get_mut(&key).zip(value) else {
            return Err(refused(format!(
                "{}.{} is missing at one or more time points",
                path.join("."),
                key
            )));
        };
        path.push(key);
        append_point(prior, value, point, path)?;
        path.pop();
    }
    Ok(())
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
    let mut warnings = Vec::new();
    let mut document = None;
    for (point, network) in series.values().iter().enumerate() {
        network.validate()?;
        let mut findings = Diagnostics::new();
        let scalar = Value::Object(write_document(network, &mut findings));
        super::super::warn_nonfinite_json(&F, &scalar, &mut findings);
        let mut checks = super::super::TextEmission::new(String::new(), findings);
        super::super::append_emit_warnings(
            network,
            super::super::TargetFormat::EgretJson,
            &mut checks,
        );
        for finding in checks.diagnostics {
            if !warnings.contains(&finding) {
                warnings.push(finding);
            }
        }
        if let Some(output) = &mut document {
            append_point(output, &scalar, point, &mut Vec::new())?;
        } else {
            document = Some(scalar);
        }
    }
    let Value::Object(mut root) = document.expect("series is nonempty") else {
        unreachable!("scalar writer states an object");
    };
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
    Ok(finish(&F, root, warnings.into()))
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
