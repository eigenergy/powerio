//! Transformations of facade values, preserving their module records.
use crate::codes;
use crate::{BalancedNetwork, GeoApplyReport, GeoLayer};
use powerio_core::{Diagnostic, HistoryEntry, HistoryId, HistoryKind};
use powerio_dist::{MulticonductorNetwork, NeutralKronOptions, NeutralKronReport};
#[cfg(test)]
use powerio_tnd::transform::capped_history_notes;
#[doc(inline)]
pub use powerio_tnd::transform::{
    MulticonductorToBalancedError, MulticonductorToBalancedOptions, MulticonductorToBalancedReport,
    MulticonductorToBalancedTransformation, SequenceTransformConvention, to_balanced_network,
    to_balanced_network_report,
};
use std::collections::BTreeMap;
/// Readiness of one module's value for the balanced lowering: the #398
/// inspect operation. The value must be a multiconductor network.
///
/// # Errors
/// A value of any other kind, named.
pub fn to_balanced_report(
    module: &powerio_core::PioModule<crate::PioValue>,
    options: MulticonductorToBalancedOptions,
) -> Result<MulticonductorToBalancedReport, powerio_core::Error> {
    let crate::PioValue::MulticonductorNetwork(net) = &module.value() else {
        return Err(wrong_kind_error(module.value()));
    };
    Ok(to_balanced_network_report(net, options))
}

/// Lower a multiconductor module to a balanced module: the #398 transform
/// operation. The module's common records carry over, the retained source is
/// severed because its bytes describe the input value, and the pass appends
/// its structured findings as module diagnostics and one Transform history
/// entry stating the chosen base power, every assumption and approximation,
/// the dropped fields, and the removed bus and switch identities.
///
/// # Errors
/// A value of any other kind (the module comes back untouched), or the
/// lowering's structured refusal.
///
/// # Panics
/// Only on a broken internal invariant: the pass's diagnostics carry no
/// identity and no span, the note lists are capped, and the history id is
/// minted unused, so every record append succeeds.
#[allow(clippy::result_large_err)]
pub fn to_balanced(
    module: powerio_core::PioModule<crate::PioValue>,
    options: MulticonductorToBalancedOptions,
) -> Result<
    powerio_core::PioModule<crate::PioValue>,
    (
        powerio_core::PioModule<crate::PioValue>,
        Box<MulticonductorToBalancedError>,
    ),
> {
    let crate::PioValue::MulticonductorNetwork(net) = &module.value() else {
        let error = MulticonductorToBalancedError::new(
            options,
            &[Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_WRONG_MODEL_KIND,
                format!(
                    "the module carries a {} value; the balanced lowering takes a \
                     multiconductor network",
                    module.value().type_name()
                ),
            )],
        );
        return Err((module, Box::new(error)));
    };
    let lowering = match to_balanced_network(net, options) {
        Ok(lowering) => lowering,
        Err(error) => return Err((module, Box::new(error))),
    };
    let MulticonductorToBalancedTransformation {
        network,
        diagnostics,
        history,
        ..
    } = lowering;
    // Room for the pass's own records is checked against the module maxima
    // before the value is consumed, so the additions below hold by
    // construction and a cap-edge input is refused with its module intact.
    let diagnostics_room =
        powerio_core::limits::MAX_MODULE_DIAGNOSTICS.saturating_sub(module.diagnostics().len());
    let history_room =
        powerio_core::limits::MAX_MODULE_HISTORY_ENTRIES.saturating_sub(module.history().len());
    if diagnostics.len() > diagnostics_room || history_room == 0 {
        let error = MulticonductorToBalancedError::new(
            options,
            &[Diagnostic::of(
                &codes::TRANSFORM_MULTI_TO_BALANCED_RECORD_CAP,
                "the module cannot hold the lowering's findings and history entry; export a \
                 fresh module before lowering"
                    .to_string(),
            )],
        );
        return Err((module, Box::new(error)));
    }
    let mut module = module
        .map_value(|_| crate::PioValue::BalancedNetwork(network))
        .sever_source();
    // The value's kind changed, so no RFC 6901 target survives the
    // transform: pre-existing diagnostic targets and the source map pointed
    // into the consumed multiconductor value and are severed here; the
    // pass's own findings are emitted with no target for the same reason.
    module.sever_value_targets();
    for diagnostic in &diagnostics {
        let mut diagnostic = diagnostic.clone();
        diagnostic.clear_target();
        module
            .add_diagnostic(diagnostic)
            .expect("room was checked; pass diagnostics carry no identity and no span");
    }
    let entry = copy_history_with_id(
        &history,
        unused_history_id(&module, "multiconductor-to-balanced"),
    );
    module
        .add_history_entry(entry)
        .expect("room was checked and the history id is unique by construction");
    Ok(module)
}

fn derive_balanced_calculation<I>(
    module: &powerio_core::PioModule<crate::PioValue>,
    operation: &'static str,
    output_type: &'static str,
    build: impl FnOnce(BalancedNetwork) -> Result<I, powerio_core::Error>,
) -> Result<powerio_core::PioModule<I>, powerio_core::Error> {
    if !matches!(module.value(), crate::PioValue::BalancedNetwork(_)) {
        return Err(powerio_core::Error::new(
            &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
            format!(
                "{operation} requires powerio.BalancedNetwork; the module contains {}",
                module.value().type_name()
            ),
        ));
    }
    let history = HistoryEntry::new(
        unused_history_id(module, operation),
        HistoryKind::Transform,
        operation,
    )?
    .with_input_type("powerio.BalancedNetwork")?
    .with_output_type(output_type)?;
    let producer = powerio_core::Producer::new("powerio", crate::VERSION)?;
    module.clone().try_derive_value(producer, history, |value| {
        let crate::PioValue::BalancedNetwork(network) = value else {
            unreachable!("the value type was checked before derivation")
        };
        build(network)
    })
}

fn derive_multiconductor_calculation<I>(
    module: &powerio_core::PioModule<crate::PioValue>,
    operation: &'static str,
    output_type: &'static str,
    build: impl FnOnce(MulticonductorNetwork) -> Result<I, powerio_core::Error>,
) -> Result<powerio_core::PioModule<I>, powerio_core::Error> {
    if !matches!(module.value(), crate::PioValue::MulticonductorNetwork(_)) {
        return Err(powerio_core::Error::new(
            &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
            format!(
                "{operation} requires powerio.MulticonductorNetwork; the module contains {}",
                module.value().type_name()
            ),
        ));
    }
    let history = HistoryEntry::new(
        unused_history_id(module, operation),
        HistoryKind::Transform,
        operation,
    )?
    .with_input_type("powerio.MulticonductorNetwork")?
    .with_output_type(output_type)?;
    let producer = powerio_core::Producer::new("powerio", crate::VERSION)?;
    module.clone().try_derive_value(producer, history, |value| {
        let crate::PioValue::MulticonductorNetwork(network) = value else {
            unreachable!("the value type was checked before derivation")
        };
        build(network)
    })
}

/// Apply one geographic layer to a network module.
///
/// Balanced bus points and branch routes use
/// [`BalancedNetwork::apply_geo_layer`]. Multiconductor coordinates use the
/// same shared matching rules through [`crate::dist_geo::apply_dist_geo_layer`].
/// The source module is unchanged. The returned module clears retained bytes
/// and source mappings, preserves its other records, and appends one
/// `apply_geo_layer` history entry.
///
/// # Errors
/// The module does not contain a balanced or multiconductor network, or its
/// records cannot accept the new history entry.
pub fn apply_geo_layer(
    module: &powerio_core::PioModule<crate::PioValue>,
    layer: &GeoLayer,
) -> Result<(powerio_core::PioModule<crate::PioValue>, GeoApplyReport), powerio_core::Error> {
    let type_name = match &module.value() {
        crate::PioValue::BalancedNetwork(_) => "powerio.BalancedNetwork",
        crate::PioValue::MulticonductorNetwork(_) => "powerio.MulticonductorNetwork",
        value => {
            return Err(powerio_core::Error::new(
                &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
                format!(
                    "apply_geo_layer requires powerio.BalancedNetwork or \
                     powerio.MulticonductorNetwork; the module contains {}",
                    value.type_name()
                ),
            ));
        }
    };
    let history = HistoryEntry::new(
        unused_history_id(module, "apply-geo-layer"),
        HistoryKind::Transform,
        "apply_geo_layer",
    )?
    .with_input_type(type_name)?
    .with_output_type(type_name)?;
    let producer = powerio_core::Producer::new("powerio", crate::VERSION)?;
    let mut report = None;
    let derived = module
        .clone()
        .try_derive_value(producer, history, |mut value| {
            let applied = match &mut value {
                crate::PioValue::BalancedNetwork(network) => network.apply_geo_layer(layer),
                crate::PioValue::MulticonductorNetwork(network) => {
                    crate::dist_geo::apply_dist_geo_layer(network, layer)
                }
                value => {
                    return Err(powerio_core::Error::new(
                        &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
                        format!(
                            "apply_geo_layer requires powerio.BalancedNetwork or \
                             powerio.MulticonductorNetwork; the module contains {}",
                            value.type_name()
                        ),
                    ));
                }
            };
            report = Some(applied);
            Ok(value)
        })?;
    let report = report.ok_or_else(|| {
        powerio_core::Error::new(
            &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
            "apply_geo_layer did not receive a network value",
        )
    })?;
    Ok((derived, report))
}

/// Eliminate explicitly grounded neutral conductors with the default Kron
/// projection options.
///
/// The source module is left unchanged. The returned module retains its
/// records, clears locators into the pre-projection value, appends the
/// projection findings, and records one same-family transform history entry.
pub fn neutral_kron(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<(powerio_core::PioModule<crate::PioValue>, NeutralKronReport), powerio_core::Error> {
    neutral_kron_with_options(module, &NeutralKronOptions::default())
}

/// Eliminate explicitly grounded neutral conductors with explicit projection
/// options.
///
/// The returned report keeps its input-network targets. Copies appended to the
/// output module have those targets cleared because terminal arrays changed.
///
/// # Errors
/// The module does not contain a multiconductor network, the neutral
/// projection cannot preserve its semantics, or the derived module cannot
/// accept the new history and diagnostic records.
pub fn neutral_kron_with_options(
    module: &powerio_core::PioModule<crate::PioValue>,
    options: &NeutralKronOptions,
) -> Result<(powerio_core::PioModule<crate::PioValue>, NeutralKronReport), powerio_core::Error> {
    let crate::PioValue::MulticonductorNetwork(network) = module.value() else {
        return Err(powerio_core::Error::new(
            &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
            format!(
                "neutral_kron requires powerio.MulticonductorNetwork; the module contains {}",
                module.value().type_name()
            ),
        ));
    };
    let reduction = powerio_dist::neutral_kron_reduce(network, options).map_err(|cause| {
        powerio_core::Error::new(cause.code(), cause.to_string()).with_cause(cause)
    })?;
    let (network, report) = reduction.into_parts();
    let neutral_terminals = options
        .neutral_terminals
        .iter()
        .map(|(bus, terminal)| (bus.clone(), serde_json::Value::String(terminal.clone())))
        .collect();
    let parameters = BTreeMap::from([
        (
            "allow_forced_ideal_ground".to_owned(),
            serde_json::Value::Bool(options.allow_forced_ideal_ground),
        ),
        (
            "neutral_terminals".to_owned(),
            serde_json::Value::Object(neutral_terminals),
        ),
    ]);
    let history = HistoryEntry::new(
        unused_history_id(module, "neutral_kron"),
        HistoryKind::Transform,
        "neutral_kron",
    )?
    .with_input_type("powerio.MulticonductorNetwork")?
    .with_output_type("powerio.MulticonductorNetwork")?
    .with_parameters(parameters)?;
    let producer = powerio_core::Producer::new("powerio", crate::VERSION)?;
    let mut derived = module.clone().try_derive_value(
        producer,
        history,
        move |value| match value {
            crate::PioValue::MulticonductorNetwork(_) => {
                Ok(crate::PioValue::MulticonductorNetwork(network))
            }
            value => Err(powerio_core::Error::new(
                &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
                format!(
                    "neutral_kron requires powerio.MulticonductorNetwork; the module contains {}",
                    value.type_name()
                ),
            )),
        },
    )?;
    for diagnostic in &report.diagnostics {
        let mut diagnostic = diagnostic.clone();
        diagnostic.clear_target();
        derived.add_diagnostic(diagnostic)?;
    }
    Ok((derived, report))
}

/// The balanced network an operating point states: the point's network with
/// the point's bus voltages, generator dispatch and setpoints, load powers,
/// service flags, taps, phase shifts, and switch positions applied. The
/// collection's shared network is not changed. Net bus injection columns have
/// no network field and are not applied; `emit` reports that omission.
#[must_use]
pub fn network_with_operating_point(
    point: &powerio_prob::OperatingPoint<BalancedNetwork>,
) -> BalancedNetwork {
    crate::write::network_with_balanced_operating_point(point, "network").0
}

/// Construct a DC power flow calculation from a balanced network module.
/// Module diagnostics, source descriptions, provenance, and prior history are
/// preserved. Retained source bytes and value locators are cleared because
/// they describe the network rather than the calculation instance.
pub fn to_dc_pf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::DcPfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::DcPfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::DcPfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_balanced_calculation(
        module,
        "to_dc_pf_instance",
        "powerio.DcPfInstance",
        powerio_prob::DcPfInstance::from_network,
    )
}

/// Construct an AC power flow calculation from a balanced network module.
pub fn to_ac_pf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::AcPfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::AcPfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::AcPfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_balanced_calculation(
        module,
        "to_ac_pf_instance",
        "powerio.AcPfInstance",
        powerio_prob::AcPfInstance::from_network,
    )
}

/// Construct a DC optimal power flow calculation from a balanced network
/// module.
pub fn to_dc_opf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::DcOpfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::DcOpfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::DcOpfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_balanced_calculation(
        module,
        "to_dc_opf_instance",
        "powerio.DcOpfInstance",
        powerio_prob::DcOpfInstance::from_network,
    )
}

/// Construct an AC optimal power flow calculation from a balanced network
/// module.
pub fn to_ac_opf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::AcOpfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::AcOpfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::AcOpfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_balanced_calculation(
        module,
        "to_ac_opf_instance",
        "powerio.AcOpfInstance",
        powerio_prob::AcOpfInstance::from_network,
    )
}

/// Construct a multiconductor AC power flow calculation from a
/// multiconductor network module.
pub fn to_mc_ac_pf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::McAcPfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::McAcPfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::McAcPfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_multiconductor_calculation(
        module,
        "to_mc_ac_pf_instance",
        "powerio.McAcPfInstance",
        powerio_prob::McAcPfInstance::from_network,
    )
}

/// Construct a multiconductor AC optimal power flow calculation from a
/// multiconductor network module.
pub fn to_mc_ac_opf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::McAcOpfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::McAcOpfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::McAcOpfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    derive_multiconductor_calculation(
        module,
        "to_mc_ac_opf_instance",
        "powerio.McAcOpfInstance",
        powerio_prob::McAcOpfInstance::from_network,
    )
}

/// Construct a LinDist3Flow optimal power flow calculation from a
/// multiconductor network module using the default formulation options.
///
/// An already typed LinDist3Flow instance is extracted without reconstruction
/// or another history entry.
pub fn to_lindist3flow_opf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::LinDist3FlowOpfInstance>, powerio_core::Error> {
    to_lindist3flow_opf_instance_with_options(
        module,
        powerio_prob::LinDist3FlowBuildOptions::default(),
    )
}

/// Construct a LinDist3Flow optimal power flow calculation with explicit
/// formulation options.
///
/// The options are recorded on the transform history entry. An already typed
/// LinDist3Flow instance is extracted as-is; its own stored options remain
/// authoritative.
pub fn to_lindist3flow_opf_instance_with_options(
    module: &powerio_core::PioModule<crate::PioValue>,
    options: powerio_prob::LinDist3FlowBuildOptions,
) -> Result<powerio_core::PioModule<powerio_prob::LinDist3FlowOpfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::LinDist3FlowOpfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::LinDist3FlowOpfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    if !matches!(module.value(), crate::PioValue::MulticonductorNetwork(_)) {
        return Err(powerio_core::Error::new(
            &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
            format!(
                "to_lindist3flow_opf_instance requires powerio.MulticonductorNetwork; the module contains {}",
                module.value().type_name()
            ),
        ));
    }
    let options_value = serde_json::to_value(options).map_err(|cause| {
        powerio_core::Error::new(
            &codes::TRANSFORM_LINDIST3FLOW_OPTIONS_SERIALIZE_FAILED,
            "could not record LinDist3Flow build options in transform history",
        )
        .with_cause(cause)
    })?;
    let serde_json::Value::Object(parameters) = options_value else {
        return Err(powerio_core::Error::new(
            &codes::TRANSFORM_LINDIST3FLOW_OPTIONS_SERIALIZE_FAILED,
            "LinDist3Flow build options did not serialize as an object",
        ));
    };
    let history = HistoryEntry::new(
        unused_history_id(module, "to_lindist3flow_opf_instance"),
        HistoryKind::Transform,
        "to_lindist3flow_opf_instance",
    )?
    .with_input_type("powerio.MulticonductorNetwork")?
    .with_output_type("powerio.LinDist3FlowOpfInstance")?
    .with_parameters(parameters.into_iter().collect())?;
    let producer = powerio_core::Producer::new("powerio", crate::VERSION)?;
    module.clone().try_derive_value(producer, history, |value| {
        let crate::PioValue::MulticonductorNetwork(network) = value else {
            unreachable!("the value type was checked before derivation")
        };
        powerio_prob::LinDist3FlowOpfInstance::from_network(network, options)
    })
}

/// Construct a fixed-dispatch LinDist3Flow power-flow calculation from a
/// multiconductor network using the default formulation options.
pub fn to_lindist3flow_pf_instance(
    module: &powerio_core::PioModule<crate::PioValue>,
) -> Result<powerio_core::PioModule<powerio_prob::LinDist3FlowPfInstance>, powerio_core::Error> {
    to_lindist3flow_pf_instance_with_options(
        module,
        powerio_prob::LinDist3FlowBuildOptions::default(),
    )
}

/// Construct a fixed-dispatch LinDist3Flow power-flow calculation with
/// explicit formulation options.
pub fn to_lindist3flow_pf_instance_with_options(
    module: &powerio_core::PioModule<crate::PioValue>,
    options: powerio_prob::LinDist3FlowBuildOptions,
) -> Result<powerio_core::PioModule<powerio_prob::LinDist3FlowPfInstance>, powerio_core::Error> {
    if matches!(module.value(), crate::PioValue::LinDist3FlowPfInstance(_)) {
        return Ok(module.clone().map_value(|value| match value {
            crate::PioValue::LinDist3FlowPfInstance(instance) => instance,
            _ => unreachable!("the value type was checked before extraction"),
        }));
    }
    if !matches!(module.value(), crate::PioValue::MulticonductorNetwork(_)) {
        return Err(powerio_core::Error::new(
            &codes::REQUEST_MODULE_WRONG_MODEL_KIND,
            format!(
                "to_lindist3flow_pf_instance requires powerio.MulticonductorNetwork; the module contains {}",
                module.value().type_name()
            ),
        ));
    }
    let options_value = serde_json::to_value(options).map_err(|cause| {
        powerio_core::Error::new(
            &codes::TRANSFORM_LINDIST3FLOW_OPTIONS_SERIALIZE_FAILED,
            "could not record LinDist3Flow build options in transform history",
        )
        .with_cause(cause)
    })?;
    let serde_json::Value::Object(parameters) = options_value else {
        return Err(powerio_core::Error::new(
            &codes::TRANSFORM_LINDIST3FLOW_OPTIONS_SERIALIZE_FAILED,
            "LinDist3Flow build options did not serialize as an object",
        ));
    };
    let history = HistoryEntry::new(
        unused_history_id(module, "to_lindist3flow_pf_instance"),
        HistoryKind::Transform,
        "to_lindist3flow_pf_instance",
    )?
    .with_input_type("powerio.MulticonductorNetwork")?
    .with_output_type("powerio.LinDist3FlowPfInstance")?
    .with_parameters(parameters.into_iter().collect())?;
    let producer = powerio_core::Producer::new("powerio", crate::VERSION)?;
    module.clone().try_derive_value(producer, history, |value| {
        let crate::PioValue::MulticonductorNetwork(network) = value else {
            unreachable!("the value type was checked before derivation")
        };
        powerio_prob::LinDist3FlowPfInstance::from_network(network, options)
    })
}

/// Cap a history note list at the record limit, replacing the overflow with
/// one note stating how many entries were elided, and normalize every kept
/// note to the record layer's requirements: NUL replaced, never empty, and
/// truncated at a character boundary within the identifier bound with a
/// visible marker. Truncation and elision are always visible, never silent.
fn copy_history_with_id(history: &HistoryEntry, id: HistoryId) -> HistoryEntry {
    let mut copied = HistoryEntry::new(id, history.kind(), history.name())
        .expect("the existing history name is valid")
        .with_parameters(history.parameters().clone())
        .expect("the existing parameter set is valid");
    if let Some(type_name) = history.input_type() {
        copied = copied
            .with_input_type(type_name)
            .expect("the existing input type is valid");
    }
    if let Some(type_name) = history.output_type() {
        copied = copied
            .with_output_type(type_name)
            .expect("the existing output type is valid");
    }
    for assumption in history.assumptions() {
        copied = copied
            .with_assumption(assumption.clone())
            .expect("the existing assumption list is valid");
    }
    for loss in history.losses() {
        copied = copied
            .with_loss(loss.clone())
            .expect("the existing loss list is valid");
    }
    copied
}

/// One history note made valid for the record layer, whatever the source
/// element names carried: nonempty, free of NUL, within the identifier
/// bound.
fn unused_history_id(
    module: &powerio_core::PioModule<crate::PioValue>,
    base: &str,
) -> powerio_core::HistoryId {
    use powerio_core::HistoryId;
    let taken: std::collections::BTreeSet<&str> = module
        .history()
        .iter()
        .map(|entry| entry.id().as_str())
        .collect();
    if !taken.contains(base) {
        return HistoryId::new(base).expect("static id is valid");
    }
    let mut counter = 2usize;
    loop {
        let candidate = format!("{base}-{counter}");
        if !taken.contains(candidate.as_str()) {
            return HistoryId::new(candidate).expect("numbered id is valid");
        }
        counter += 1;
    }
}

fn wrong_kind_error(value: &crate::PioValue) -> powerio_core::Error {
    powerio_core::Error::new(
        &codes::TRANSFORM_MULTI_TO_BALANCED_WRONG_MODEL_KIND,
        format!(
            "the module carries a {} value; the balanced lowering takes a multiconductor \
             network",
            value.type_name()
        ),
    )
}

#[cfg(test)]
mod history_record_tests {
    use super::{capped_history_notes, unused_history_id};

    #[test]
    fn note_overflow_is_stated_within_the_cap() {
        let cap = powerio_core::limits::MAX_HISTORY_NOTES;
        let notes: Vec<String> = (0..cap + 40).map(|index| format!("note {index}")).collect();
        let kept = capped_history_notes(notes, "assumptions");
        assert_eq!(kept.len(), cap);
        assert_eq!(kept.last().unwrap(), "41 more assumptions elided");

        let short: Vec<String> = (0..3).map(|index| format!("note {index}")).collect();
        assert_eq!(capped_history_notes(short.clone(), "assumptions"), short);
    }

    #[test]
    fn the_history_id_is_minted_unused() {
        use powerio_core::{HistoryEntry, HistoryId, HistoryKind, PioModule};
        let mut module = PioModule::new(crate::PioValue::BalancedNetwork(
            crate::BalancedNetwork::in_memory("t", 100.0, Vec::new(), Vec::new()),
        ));
        assert_eq!(
            unused_history_id(&module, "multiconductor-to-balanced").as_str(),
            "multiconductor-to-balanced"
        );
        for id in ["multiconductor-to-balanced", "multiconductor-to-balanced-2"] {
            module
                .add_history_entry(
                    HistoryEntry::new(
                        HistoryId::new(id).unwrap(),
                        HistoryKind::Transform,
                        "to_balanced",
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        assert_eq!(
            unused_history_id(&module, "multiconductor-to-balanced").as_str(),
            "multiconductor-to-balanced-3"
        );
    }
}
