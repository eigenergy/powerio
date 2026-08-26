//! The one way upgrade from released 0.9.x `NetworkPackage` documents to the
//! runtime module. The pre 0.9 `schema_version` lineage is refused (0.9
//! already required regenerating it), and a nonempty legacy `study` is
//! refused with a directed migration instruction: its unapplied cumulative
//! commits cannot become descriptive history without choosing a revision.

use std::collections::BTreeMap;

use powerio_core::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, HistoryEntry, HistoryId, HistoryKind,
    PioModule, Producer, TimePoint,
};

use crate::package::diagnostics::codes;
use crate::package::{ModelPayload, NetworkPackage, OperatingPointSeries};
use crate::value::PioValue;

type Result<T> = std::result::Result<T, powerio_core::Error>;

fn refused(code: &'static powerio_core::DiagnosticInfo, message: String) -> powerio_core::Error {
    powerio_core::Error::new(code, message)
}

/// Upgrade legacy `.pio.json` text. `powerio_version` is the header's stated
/// producer version; the 0.9 shape is the only one positively identified, so
/// anything else is refused with what it claimed to be.
pub(super) fn upgrade_legacy(
    text: &str,
    powerio_version: Option<&str>,
) -> Result<PioModule<PioValue>> {
    let identified_09 = powerio_version.is_some_and(version_is_09);
    if !identified_09 {
        return Err(refused(
            &codes::READ_MODULE_UNSUPPORTED,
            format!(
                "not a stored powerio module: no `schema` header and `powerio_version` {} does \
                 not identify a released 0.9 package; the pre 0.9 lineage must be regenerated",
                powerio_version.map_or_else(|| "<absent>".to_string(), |v| format!("`{v}`"))
            ),
        ));
    }
    let legacy = NetworkPackage::from_json(text)
        .map_err(|error| refused(&codes::READ_MODULE_INVALID, error.to_string()))?;

    if legacy.study.as_ref().is_some_and(|study| !study.is_empty()) {
        return Err(refused(
            &codes::READ_MODULE_LEGACY_STUDY,
            "this 0.9 package carries a nonempty `study` block, whose unapplied cumulative \
             commits and selectable base state cannot be upgraded without choosing a revision; \
             materialize the selected commit with the 0.9 migration command \
             (`powerio package --materialize`) and upgrade that static package"
                .to_string(),
        ));
    }

    let series = legacy
        .operating_points
        .clone()
        .filter(|series| !series.points.is_empty());
    let value = match (&legacy.model, series) {
        (ModelPayload::Balanced { balanced_network }, Some(series)) => {
            upgrade_series(balanced_network, &series)?
        }
        (ModelPayload::Balanced { balanced_network }, None) => {
            PioValue::BalancedNetwork((**balanced_network).clone())
        }
        (
            ModelPayload::Multiconductor {
                multiconductor_network,
            },
            Some(_),
        ) => {
            return Err(refused(
                &codes::READ_MODULE_INVALID,
                format!(
                    "a 0.9 multiconductor package with operating points has no upgrade; the \
                     0.9 shape never produced one ({} buses)",
                    multiconductor_network.buses().len()
                ),
            ));
        }
        (
            ModelPayload::Multiconductor {
                multiconductor_network,
            },
            None,
        ) => PioValue::MulticonductorNetwork((**multiconductor_network).clone()),
    };

    let producer = Producer::new(
        legacy.producer.tool.clone(),
        legacy.producer.version.clone(),
    )
    .unwrap_or_else(|_| {
        Producer::new("powerio", crate::VERSION).expect("static producer is valid")
    });
    let mut module = PioModule::new(value).with_producer(producer);
    for diagnostic in &legacy.diagnostics {
        module
            .add_diagnostic(upgrade_diagnostic(diagnostic))
            .map_err(|error| refused(&codes::READ_MODULE_INVALID, error.to_string()))?;
    }
    let upgrade_note = HistoryEntry::new(
        HistoryId::new("upgrade-0.9").expect("static id is valid"),
        HistoryKind::Upgrade,
        "upgrade_network_package_0_9",
    )
    .expect("static name is valid")
    .with_assumption(
        "legacy summaries, validation counts, origin, and derived metadata were \
         nonauthoritative and were recomputed or dropped",
    )
    .expect("static assumption is valid");
    module
        .add_history_entry(upgrade_note)
        .map_err(|error| refused(&codes::READ_MODULE_INVALID, error.to_string()))?;
    module
        .add_diagnostic(Diagnostic::of(
            &codes::READ_MODULE_UPGRADED,
            format!(
                "upgraded a powerio {} package: legacy summaries, validation counts, and derived \
             metadata are nonauthoritative and were not carried",
                legacy.powerio_version
            ),
        ))
        .map_err(|error| refused(&codes::READ_MODULE_INVALID, error.to_string()))?;
    Ok(module)
}

fn version_is_09(version: &str) -> bool {
    version
        .split('.')
        .take(2)
        .collect::<Vec<_>>()
        .as_slice()
        .first()
        .zip(version.split('.').nth(1))
        .is_some_and(|(major, minor)| *major == "0" && minor == "9")
}

/// Legacy operating points become the primary
/// `TimeSeries<OperatingPoint<BalancedNetwork>>` value. Legacy updates state
/// JSON fields on payload rows; each maps onto the fixed state vocabulary,
/// and a field outside it refuses the upgrade by name rather than dropping
/// data silently.
fn upgrade_series(
    network: &crate::BalancedNetwork,
    series: &OperatingPointSeries,
) -> Result<PioValue> {
    let periods = series.time_axis.periods.max(series.points.len());
    let mut time_points = Vec::with_capacity(periods);
    for index in 0..periods {
        let label = series
            .time_axis
            .labels
            .get(index)
            .cloned()
            .unwrap_or_else(|| format!("period {index}"));
        let duration = series
            .time_axis
            .duration_hours
            .get(index)
            .and_then(|hours| {
                (hours.is_finite() && *hours >= 0.0)
                    .then(|| std::time::Duration::from_secs_f64(hours * 3600.0))
            });
        time_points.push(
            TimePoint::new(label, duration)
                .map_err(|error| refused(&codes::READ_MODULE_INVALID, error.to_string()))?,
        );
    }

    // Sparse columns: base row from the static payload, overrides per point.
    let mut overrides: BTreeMap<&'static str, Vec<Vec<(String, f64)>>> = BTreeMap::new();
    for point in &series.points {
        if point.index >= periods {
            return Err(refused(
                &codes::READ_MODULE_INVALID,
                format!(
                    "operating point index {} is outside the {periods} period time axis",
                    point.index
                ),
            ));
        }
        for update in &point.updates {
            let identity = update
                .element
                .source_uid
                .clone()
                .or_else(|| {
                    update
                        .element
                        .row
                        .map(|row| format!("{}:{row}", update.element.table))
                })
                .unwrap_or_default();
            for (field, value) in &update.fields {
                let Some(quantity) = legacy_quantity(&update.element.table, field) else {
                    return Err(refused(
                        &codes::READ_MODULE_LEGACY_FIELD,
                        format!(
                            "operating point update field `{}.{field}` has no state quantity; \
                             this 0.9 series cannot upgrade losslessly",
                            update.element.table
                        ),
                    ));
                };
                let Some(value) = value.as_f64().or_else(|| match value {
                    serde_json::Value::Bool(flag) => Some(f64::from(u8::from(*flag))),
                    _ => None,
                }) else {
                    return Err(refused(
                        &codes::READ_MODULE_INVALID,
                        format!(
                            "operating point update `{}.{field}` is not numeric",
                            update.element.table
                        ),
                    ));
                };
                overrides
                    .entry(quantity)
                    .or_insert_with(|| vec![Vec::new(); periods])[point.index]
                    .push((identity.clone(), value));
            }
        }
    }

    let mut builder = powerio_prob::BalancedStateBuilder::new(network.clone(), time_points);
    for (quantity, changes) in overrides {
        let base = base_row(network, quantity)?;
        builder = builder
            .sparse_by_name(quantity, base, changes)
            .map_err(|error| refused(&codes::READ_MODULE_INVALID, error.to_string()))?;
    }
    Ok(PioValue::BalancedOperatingPointTimeSeries(
        builder
            .build()
            .map_err(|error| refused(&codes::READ_MODULE_INVALID, error.to_string()))?,
    ))
}

/// The 0.9 update vocabulary: payload table and serde field name to the state
/// quantity that stores it.
fn legacy_quantity(table: &str, field: &str) -> Option<&'static str> {
    Some(match (table, field) {
        ("loads", "p") => "load_active_power",
        ("loads", "q") => "load_reactive_power",
        ("generators", "pg") => "generator_active_power",
        ("generators", "qg") => "generator_reactive_power",
        ("generators", "vg") => "generator_voltage_setpoint",
        ("generators", "in_service") => "generator_in_service",
        ("buses", "vm") => "bus_voltage_magnitude",
        ("buses", "va") => "bus_voltage_angle",
        ("branches", "in_service") => "branch_in_service",
        ("branches", "tap") => "branch_tap_ratio",
        ("branches", "shift") => "branch_phase_shift",
        _ => return None,
    })
}

/// The static payload's row values for one quantity, the sparse base row.
fn base_row(network: &crate::BalancedNetwork, quantity: &str) -> Result<Vec<f64>> {
    let row = match quantity {
        "load_active_power" => network.loads().iter().map(|l| l.p).collect(),
        "load_reactive_power" => network.loads().iter().map(|l| l.q).collect(),
        "generator_active_power" => network.generators().iter().map(|g| g.pg).collect(),
        "generator_reactive_power" => network.generators().iter().map(|g| g.qg).collect(),
        "generator_voltage_setpoint" => network.generators().iter().map(|g| g.vg).collect(),
        "generator_in_service" => network
            .generators()
            .iter()
            .map(|g| f64::from(u8::from(g.in_service)))
            .collect(),
        "bus_voltage_magnitude" => network.buses().iter().map(|b| b.vm).collect(),
        "bus_voltage_angle" => network.buses().iter().map(|b| b.va).collect(),
        "branch_in_service" => network
            .branches()
            .iter()
            .map(|b| f64::from(u8::from(b.in_service)))
            .collect(),
        "branch_tap_ratio" => network.branches().iter().map(|b| b.tap).collect(),
        "branch_phase_shift" => network.branches().iter().map(|b| b.shift).collect(),
        other => {
            return Err(refused(
                &codes::READ_MODULE_INVALID,
                format!("no base row for quantity `{other}`"),
            ));
        }
    };
    Ok(row)
}

fn upgrade_diagnostic(legacy: &crate::package::StructuredDiagnostic) -> Diagnostic {
    let code = DiagnosticCode::new(legacy.code.as_str())
        .unwrap_or_else(|_| DiagnosticCode::new("LEGACY.UNKNOWN").expect("static code is valid"));
    let severity = match legacy.severity {
        crate::package::DiagnosticSeverity::Error => DiagnosticSeverity::Error,
        crate::package::DiagnosticSeverity::Warning => DiagnosticSeverity::Warning,
        _ => DiagnosticSeverity::Note,
    };
    let mut diagnostic = Diagnostic::new(code, severity, legacy.message.clone());
    if let Some(target) = &legacy.element_path
        && let Ok(with) = diagnostic.clone().with_target(target.clone())
    {
        diagnostic = with;
    }
    diagnostic
}
