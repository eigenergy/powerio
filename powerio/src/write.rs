//! The module emission dispatcher. One dynamic module emits a named target
//! format; the concrete value routes to the family emitter that owns it.
//!
//! Same format emission keeps the family writers' echo tier: an unchanged
//! parsed module emits its retained source bytes exactly. Cross format
//! emission serializes the typed value and reports what the target cannot
//! represent through the returned diagnostics.

use powerio_core::{Destination, Diagnostic, EmitResult, Error, PioModule};
use powerio_prob::{
    BalancedOperatingPointFlag, BalancedOperatingPointQuantity, MulticonductorOperatingPointFlag,
    MulticonductorOperatingPointQuantity, OperatingPoint,
};
use powerio_tx::{BalancedNetwork, BranchSolution};

use crate::PioValue;

pub mod codes {
    powerio_core::diagnostic_codes! {
        REQUEST_EMIT_UNKNOWN_FORMAT = "REQUEST.EMIT.UNKNOWN_FORMAT", Error,
            "the requested target format name is not recognized", category = Request;
        REQUEST_EMIT_UNSUPPORTED_VALUE_TYPE = "REQUEST.EMIT.UNSUPPORTED_VALUE_TYPE", Error,
            "the module's value type cannot be emitted in the requested format", category = Request;
        EMIT_CALCULATION_DATA_OMITTED = "EMIT.CALCULATION.DATA_OMITTED", Warning,
            "the target grid exchange format does not represent the complete calculation definition", category = Output;
        EMIT_SOLUTION_DATA_OMITTED = "EMIT.SOLUTION.DATA_OMITTED", Warning,
            "the target grid exchange format does not represent every solution result", category = Output;
        EMIT_RELAXATION_DATA_OMITTED = "EMIT.RELAXATION.DATA_OMITTED", Warning,
            "the target grid exchange format does not represent an SOCWR relaxation result", category = Output;
        EMIT_OPERATING_POINT_DATA_OMITTED = "EMIT.OPERATING_POINT.DATA_OMITTED", Warning,
            "the target grid exchange format does not represent every operating point quantity", category = Output;
    }
}

/// True when `format` names the PyPSA CSV folder target.
fn is_pypsa_dir(format: &str) -> bool {
    crate::resolve_format(format).is_some_and(|info| info.token == "pypsa-csv")
}

fn is_cgmes_dir(format: &str) -> bool {
    crate::resolve_format(format).is_some_and(|info| info.token == "cgmes")
}

fn is_goc3(format: &str) -> bool {
    crate::resolve_format(format).is_some_and(|info| info.token == "goc3-json")
}

/// True when `format` names the GridFM directory target.
fn is_gridfm_dir(format: &str) -> bool {
    crate::resolve_format(format).is_some_and(|info| info.token == "gridfm")
}

/// True when two names identify the same supported directory format. The
/// retained source carries the format selected by the parser; this comparison
/// admits the public aliases at emission without treating an arbitrary
/// declared directory token as an echoable format.
fn same_directory_format(source: &str, requested: &str) -> bool {
    if is_pypsa_dir(source) && is_pypsa_dir(requested) {
        return true;
    }
    if is_cgmes_dir(source) && is_cgmes_dir(requested) {
        return true;
    }
    #[cfg(feature = "gridfm")]
    if source.eq_ignore_ascii_case("gridfm") && requested.eq_ignore_ascii_case("gridfm") {
        return true;
    }
    false
}

/// The exact artifact inventory of an unchanged directory source when the
/// requested format is that source's format. Entry names come from the
/// source's bounded portable directory walk, and every byte is acquired
/// through the source's confined, no-symlink path rather than by reopening a
/// caller-controlled path here.
fn echo_retained_directory(
    module: &PioModule<PioValue>,
    format: &str,
) -> Result<Option<Vec<powerio_core::MemoryArtifact>>, Error> {
    let Some(source) = module.source().filter(|source| source.is_directory()) else {
        return Ok(None);
    };
    let Some(source_format) = source.format() else {
        return Ok(None);
    };
    if !same_directory_format(source_format.as_str(), format) {
        return Ok(None);
    }

    let mut artifacts = Vec::new();
    for name in source.entry_names()? {
        let buffer = source.buffer(&name)?;
        artifacts.push(powerio_core::MemoryArtifact::new(
            name,
            buffer.bytes().to_vec(),
        ));
    }
    Ok(Some(artifacts))
}

/// A typed sibling module over `value` carrying every common record and the
/// retained source from the dynamic module. Source descriptors are added
/// first because source map and diagnostic spans validate against them.
fn typed_sibling<T>(module: &PioModule<PioValue>, value: T) -> Result<PioModule<T>, Error> {
    let mut out = PioModule::new(value).with_producer(module.producer().clone());
    for descriptor in module.sources() {
        out.add_source_descriptor(descriptor.clone())?;
    }
    for entry in module.source_map() {
        out.add_source_map_entry(entry.clone())?;
    }
    for diagnostic in &module.diagnostics {
        out.add_diagnostic(diagnostic.clone())?;
    }
    for entry in module.history() {
        out.add_history_entry(entry.clone())?;
    }
    for (namespace, value) in module.extensions() {
        out.insert_extension(namespace.clone(), value.clone())?;
    }
    Ok(match module.source() {
        Some(source) => out.with_source(source.clone()),
        None => out,
    })
}

/// The module's retained source's exact original bytes, when that source's
/// content is already `format`: the byte exact echo tier every kind should
/// get, not just [`PioValue::BalancedNetwork`] and
/// [`PioValue::MulticonductorNetwork`] (whose family emitters already carry
/// it through `powerio_tx::emit` and `powerio_dist::emit`).
/// `None` when there is no retained source, or its content is not `format`.
///
/// A source declared with an explicit format token (the C ABI and bindings
/// route this way) is trusted directly; a source routed here by content (a
/// bare `.json` file with no declared format, the common case for the CLI
/// and a plain `Source::open`) is reclassified the same way `powerio::parse`'s
/// own routing already did once to land on this kind in the first place.
fn retained_source_matches_case_format(module: &PioModule<PioValue>, format: &str) -> bool {
    use powerio_tx::format::routing::{
        Detection, JsonClass, classify_format_name, classify_json_text,
    };

    let Some(source) = module.source() else {
        return false;
    };
    // A one file representation can echo its primary buffer. A source set
    // cannot: its primary file is only one input to the typed value (for
    // example, the problem file beside a GO Challenge 3 solution).
    if source.acquired_buffers().len() != 1 {
        return false;
    }
    let Some(requested) = classify_format_name(format).known() else {
        return false;
    };
    let actual = if let Some(declared) = source.format() {
        let Some(actual) = classify_format_name(declared.as_str()).known() else {
            return false;
        };
        actual
    } else {
        let Ok(buffer) = source.primary_buffer() else {
            return false;
        };
        let Ok(text) = std::str::from_utf8(buffer.content_bytes()) else {
            return false;
        };
        match classify_json_text(text) {
            JsonClass::Case(Detection::Known(found)) => found,
            _ => return false,
        }
    };
    requested == actual
}

fn echo_retained_source(module: &PioModule<PioValue>, format: &str) -> Option<Vec<u8>> {
    if !retained_source_matches_case_format(module, format) {
        return None;
    }
    let source = module.source()?;
    let buffer = source.primary_buffer().ok()?;
    Some(buffer.bytes().to_vec())
}

fn unsupported_type(module: &PioModule<PioValue>, format: &str) -> Error {
    Error::new(
        &codes::REQUEST_EMIT_UNSUPPORTED_VALUE_TYPE,
        format!(
            "a {} module cannot be emitted as {format}; serialize writes PowerIO IR",
            module.value.type_name()
        ),
    )
}

fn unknown_format(format: &str) -> Error {
    // The descriptor still resolves the token in a build without the GridFM
    // emitter, so the refusal names the missing feature rather than the name.
    if cfg!(not(feature = "gridfm")) && is_gridfm_dir(format) {
        return Error::new(
            &codes::REQUEST_EMIT_UNKNOWN_FORMAT,
            format!(
                "{format} names the GridFM Parquet directory format, which this build compiled without the `gridfm` feature"
            ),
        );
    }
    Error::new(
        &codes::REQUEST_EMIT_UNKNOWN_FORMAT,
        format!("{format} is not a recognized target format name"),
    )
}

/// Emit one dynamic module as `format` into `destination`. The concrete value
/// routes to its grid exchange format implementation. PowerIO IR uses
/// [`crate::serialize`] instead.
///
/// # Errors
/// `REQUEST.EMIT.UNKNOWN_FORMAT` for a format name nothing recognizes,
/// `REQUEST.EMIT.UNSUPPORTED_VALUE_TYPE` for a value the
/// named format cannot state, and the family writer's own failure otherwise.
///
/// # Panics
/// Never on external input: the stored document's fixed artifact name is
/// valid by construction.
pub fn emit<T>(
    module: &PioModule<T>,
    format: &str,
    destination: Destination,
) -> Result<EmitResult, Error>
where
    T: Clone + Into<PioValue>,
{
    let module = module.clone().map_value(Into::into);
    emit_dynamic(&module, format, destination)
}

fn calculation_data_omitted(value_type: &str, format: &str) -> Diagnostic {
    Diagnostic::of(
        &codes::EMIT_CALCULATION_DATA_OMITTED,
        format!(
            "{format} represents a grid case, not the complete {value_type}; emitted its electrical network"
        ),
    )
}

fn solution_data_omitted(value_type: &str, format: &str) -> Diagnostic {
    Diagnostic::of(
        &codes::EMIT_SOLUTION_DATA_OMITTED,
        format!(
            "{format} cannot represent every {value_type} result; emitted the network values it supports, while PowerIO IR retains the complete solution"
        ),
    )
}

fn relaxation_data_omitted(format: &str) -> Diagnostic {
    Diagnostic::of(
        &codes::EMIT_RELAXATION_DATA_OMITTED,
        format!(
            "{format} cannot represent SOCWR W-space values or the objective lower bound; emitted the instance network without treating the relaxation as an AC power flow solution"
        ),
    )
}

fn operating_point_data_omitted(format: &str) -> Diagnostic {
    Diagnostic::of(
        &codes::EMIT_OPERATING_POINT_DATA_OMITTED,
        format!(
            "{format} has no source-neutral field for net bus injection columns; emitted the other operating point quantities"
        ),
    )
}

fn network_with_balanced_operating_point(
    point: &OperatingPoint<BalancedNetwork>,
    format: &str,
) -> (BalancedNetwork, Vec<Diagnostic>) {
    let mut network = point.network().clone();

    if let Some(values) = point.values(BalancedOperatingPointQuantity::BusVoltageMagnitude) {
        for (bus, (_, value)) in network.buses_mut().iter_mut().zip(values) {
            bus.vm = value;
        }
    }
    if let Some(values) = point.values(BalancedOperatingPointQuantity::BusVoltageAngle) {
        for (bus, (_, value)) in network.buses_mut().iter_mut().zip(values) {
            bus.va = value.to_degrees();
        }
    }
    if let Some(values) = point.values(BalancedOperatingPointQuantity::GeneratorActivePower) {
        for (generator, (_, value)) in network.generators_mut().iter_mut().zip(values) {
            generator.pg = value;
        }
    }
    if let Some(values) = point.values(BalancedOperatingPointQuantity::GeneratorReactivePower) {
        for (generator, (_, value)) in network.generators_mut().iter_mut().zip(values) {
            generator.qg = value;
        }
    }
    if let Some(values) = point.values(BalancedOperatingPointQuantity::GeneratorVoltageSetpoint) {
        for (generator, (_, value)) in network.generators_mut().iter_mut().zip(values) {
            generator.vg = value;
        }
    }
    if let Some(flags) = point.flags(BalancedOperatingPointFlag::GeneratorInService) {
        for (generator, (_, value)) in network.generators_mut().iter_mut().zip(flags) {
            generator.in_service = value;
        }
    }
    if let Some(values) = point.values(BalancedOperatingPointQuantity::LoadActivePower) {
        for (load, (_, value)) in network.loads_mut().iter_mut().zip(values) {
            load.p = value;
        }
    }
    if let Some(values) = point.values(BalancedOperatingPointQuantity::LoadReactivePower) {
        for (load, (_, value)) in network.loads_mut().iter_mut().zip(values) {
            load.q = value;
        }
    }
    if let Some(flags) = point.flags(BalancedOperatingPointFlag::BranchInService) {
        for (branch, (_, value)) in network.branches_mut().iter_mut().zip(flags) {
            branch.in_service = value;
        }
    }
    if let Some(values) = point.values(BalancedOperatingPointQuantity::BranchTapRatio) {
        for (branch, (_, value)) in network.branches_mut().iter_mut().zip(values) {
            branch.tap = value;
        }
    }
    if let Some(values) = point.values(BalancedOperatingPointQuantity::BranchPhaseShift) {
        for (branch, (_, value)) in network.branches_mut().iter_mut().zip(values) {
            branch.shift = value;
        }
    }
    if let Some(flags) = point.flags(BalancedOperatingPointFlag::SwitchClosed) {
        for (switch, (_, value)) in network.switches_mut().iter_mut().zip(flags) {
            switch.closed = value;
        }
    }

    let has_unrepresented_injections = point
        .values(BalancedOperatingPointQuantity::BusActiveInjection)
        .is_some()
        || point
            .values(BalancedOperatingPointQuantity::BusReactiveInjection)
            .is_some();
    let diagnostics = has_unrepresented_injections
        .then(|| operating_point_data_omitted(format))
        .into_iter()
        .collect();
    (network, diagnostics)
}

fn network_with_multiconductor_operating_point(
    point: &OperatingPoint<powerio_dist::MulticonductorNetwork>,
    format: &str,
) -> (powerio_dist::MulticonductorNetwork, Vec<Diagnostic>) {
    let mut network = point.network().clone();

    if let Some(values) = point.values(MulticonductorOperatingPointQuantity::LoadActivePower) {
        let mut values = values.map(|(_, value)| value);
        for load in network.loads_mut() {
            for value in &mut load.p_nom {
                *value = values
                    .next()
                    .expect("an operating point has one value per load terminal");
            }
        }
        debug_assert!(values.next().is_none());
    }
    if let Some(values) = point.values(MulticonductorOperatingPointQuantity::LoadReactivePower) {
        let mut values = values.map(|(_, value)| value);
        for load in network.loads_mut() {
            for value in &mut load.q_nom {
                *value = values
                    .next()
                    .expect("an operating point has one value per load terminal");
            }
        }
        debug_assert!(values.next().is_none());
    }
    if let Some(values) = point.values(MulticonductorOperatingPointQuantity::GeneratorActivePower) {
        let mut values = values.map(|(_, value)| value);
        for generator in network.generators_mut() {
            for value in &mut generator.p_nom {
                *value = values
                    .next()
                    .expect("an operating point has one value per generator terminal");
            }
        }
        debug_assert!(values.next().is_none());
    }
    if let Some(values) = point.values(MulticonductorOperatingPointQuantity::GeneratorReactivePower)
    {
        let mut values = values.map(|(_, value)| value);
        for generator in network.generators_mut() {
            for value in &mut generator.q_nom {
                *value = values
                    .next()
                    .expect("an operating point has one value per generator terminal");
            }
        }
        debug_assert!(values.next().is_none());
    }
    if let Some(flags) = point.flags(MulticonductorOperatingPointFlag::SwitchClosed) {
        for (switch, (_, closed)) in network.switches_mut().iter_mut().zip(flags) {
            switch.open = !closed;
        }
    }

    let mut omitted = Vec::new();
    if point
        .values(MulticonductorOperatingPointQuantity::TerminalVoltageMagnitude)
        .is_some()
    {
        omitted.push("terminal voltage magnitude");
    }
    if point
        .values(MulticonductorOperatingPointQuantity::TerminalVoltageAngle)
        .is_some()
    {
        omitted.push("terminal voltage angle");
    }
    if point
        .values(MulticonductorOperatingPointQuantity::TransformerTap)
        .is_some()
    {
        omitted.push("transformer tap");
    }
    if point
        .values(MulticonductorOperatingPointQuantity::CapacitorSteps)
        .is_some()
    {
        omitted.push("capacitor steps");
    }
    let diagnostics = if omitted.is_empty() {
        Vec::new()
    } else {
        vec![Diagnostic::of(
            &codes::EMIT_OPERATING_POINT_DATA_OMITTED,
            format!(
                "{format} cannot represent these operating point quantities in the source-neutral network model: {}; emitted the other quantities",
                omitted.join(", ")
            ),
        )]
    };
    (network, diagnostics)
}

fn network_with_dc_pf_solution(solution: &powerio_prob::DcPfSolution) -> BalancedNetwork {
    let mut network = solution.network().clone();
    for bus in network.buses_mut() {
        bus.va = solution
            .bus_voltage_angle(bus.id)
            .expect("a solution contains one angle per network bus");
    }
    if let Some(dispatch) = solution.generator_dispatch() {
        for (generator, active_power) in network.generators_mut().iter_mut().zip(&dispatch.p_mw) {
            generator.pg = *active_power;
        }
        if !dispatch.q_mvar.is_empty() {
            for (generator, reactive_power) in
                network.generators_mut().iter_mut().zip(&dispatch.q_mvar)
            {
                generator.qg = *reactive_power;
            }
        }
    }
    network
}

fn network_with_ac_pf_solution(solution: &powerio_prob::AcPfSolution) -> BalancedNetwork {
    let mut network = solution.network().clone();
    for bus in network.buses_mut() {
        bus.vm = solution
            .bus_voltage_magnitude(bus.id)
            .expect("a solution contains one magnitude per network bus");
        bus.va = solution
            .bus_voltage_angle(bus.id)
            .expect("a solution contains one angle per network bus");
    }
    for (branch, identity) in network
        .branches_mut()
        .iter_mut()
        .zip(solution.branch_order())
    {
        branch.solution = Some(BranchSolution::new(
            solution
                .branch_from_active_flow(&identity)
                .expect("a solution contains one from active flow per branch"),
            solution
                .branch_from_reactive_flow(&identity)
                .expect("a solution contains one from reactive flow per branch"),
            solution
                .branch_to_active_flow(&identity)
                .expect("a solution contains one to active flow per branch"),
            solution
                .branch_to_reactive_flow(&identity)
                .expect("a solution contains one to reactive flow per branch"),
        ));
    }
    if let Some(dispatch) = solution.generator_dispatch() {
        for (generator, active_power) in network.generators_mut().iter_mut().zip(&dispatch.p_mw) {
            generator.pg = *active_power;
        }
        if !dispatch.q_mvar.is_empty() {
            for (generator, reactive_power) in
                network.generators_mut().iter_mut().zip(&dispatch.q_mvar)
            {
                generator.qg = *reactive_power;
            }
        }
    }
    network
}

fn network_with_dc_opf_solution(solution: &powerio_prob::DcOpfSolution) -> BalancedNetwork {
    let mut network = solution.network().clone();
    for bus in network.buses_mut() {
        bus.va = solution
            .bus_voltage_angle(bus.id)
            .expect("a solution contains one angle per network bus");
    }
    for (generator, identity) in network
        .generators_mut()
        .iter_mut()
        .zip(solution.generator_order())
    {
        generator.pg = solution
            .generator_active_power(&identity)
            .expect("a solution contains one active power per generator");
    }
    network
}

fn network_with_ac_opf_solution(solution: &powerio_prob::AcOpfSolution) -> BalancedNetwork {
    let mut network = solution.network().clone();
    for bus in network.buses_mut() {
        bus.vm = solution
            .bus_voltage_magnitude(bus.id)
            .expect("a solution contains one magnitude per network bus");
        bus.va = solution
            .bus_voltage_angle(bus.id)
            .expect("a solution contains one angle per network bus");
    }
    for (branch, identity) in network
        .branches_mut()
        .iter_mut()
        .zip(solution.branch_order())
    {
        branch.solution = Some(BranchSolution::new(
            solution
                .branch_from_active_flow(&identity)
                .expect("a solution contains one from active flow per branch"),
            solution
                .branch_from_reactive_flow(&identity)
                .expect("a solution contains one from reactive flow per branch"),
            solution
                .branch_to_active_flow(&identity)
                .expect("a solution contains one to active flow per branch"),
            solution
                .branch_to_reactive_flow(&identity)
                .expect("a solution contains one to reactive flow per branch"),
        ));
    }
    for (generator, identity) in network
        .generators_mut()
        .iter_mut()
        .zip(solution.generator_order())
    {
        generator.pg = solution
            .generator_active_power(&identity)
            .expect("a solution contains one active power per generator");
        generator.qg = solution
            .generator_reactive_power(&identity)
            .expect("a solution contains one reactive power per generator");
    }
    network
}

fn emit_balanced_network(
    module: &PioModule<PioValue>,
    network: &BalancedNetwork,
    format: &str,
    destination: Destination,
    preserve_retained_source: bool,
    diagnostics: Vec<Diagnostic>,
) -> Result<EmitResult, Error> {
    let typed = typed_sibling(module, network.clone())?;
    let typed = if preserve_retained_source && retained_source_matches_case_format(module, format) {
        typed
    } else {
        typed.sever_source()
    };
    let result = if is_pypsa_dir(format) {
        powerio_tx::__emit_pypsa_csv(&typed, destination)
    } else {
        #[cfg(feature = "gridfm")]
        if is_gridfm_dir(format) {
            let dataset = powerio_matrix::build_gridfm_dataset(
                network,
                0,
                &powerio_matrix::GridfmOptions::default(),
            )
            .map_err(|error| Error::new(error.code(), error.to_string()).with_cause(error))?;
            return destination
                .__commit_artifacts(
                    true,
                    powerio_core::Fidelity::Canonical,
                    dataset.artifacts,
                    Vec::new(),
                )
                .map(|result| result.__with_diagnostics(diagnostics));
        }
        let Some(target) = powerio_tx::format::parse_target_format(format) else {
            return Err(unknown_format(format));
        };
        powerio_tx::emit(&typed, target, destination)
    }?;
    Ok(result.__with_diagnostics(diagnostics))
}

fn emit_multiconductor_network(
    module: &PioModule<PioValue>,
    network: powerio_dist::MulticonductorNetwork,
    format: &str,
    destination: Destination,
    preserve_retained_source: bool,
    diagnostics: Vec<Diagnostic>,
) -> Result<EmitResult, Error> {
    // A multiconductor network read from a DGS export has no distribution
    // writer for that format; an unchanged module returns its retained source.
    if crate::dgs::is_dgs_token(format) {
        if preserve_retained_source
            && let Some(bytes) = echo_retained_source(module, format)
        {
            let artifact = powerio_core::MemoryArtifact::new(
                powerio_core::ArtifactPath::new("case.dgs")
                    .expect("static name is a valid artifact path"),
                bytes,
            );
            return destination
                .__commit_artifacts(
                    false,
                    powerio_core::Fidelity::ExactSameFormat,
                    vec![artifact],
                    Vec::new(),
                )
                .map(|result| result.__with_diagnostics(diagnostics));
        }
        return Err(Error::new(
            &codes::REQUEST_EMIT_UNKNOWN_FORMAT,
            "dgs is read only; a multiconductor network with no retained DGS source cannot be \
             written as DGS",
        ));
    }
    let Some(target) = powerio_dist::parse_dist_target_format(format) else {
        return Err(unknown_format(format));
    };
    let typed = typed_sibling(module, network)?;
    let typed = if preserve_retained_source && retained_source_matches_case_format(module, format) {
        typed
    } else {
        typed.sever_source()
    };
    powerio_dist::emit(&typed, target, destination)
        .map(|result| result.__with_diagnostics(diagnostics))
}

fn emit_goc3_solution(
    solution: &powerio_prob::AcScucSolution,
    destination: Destination,
) -> Result<EmitResult, Error> {
    let text = powerio_prob::__internal::__emit_goc3_output(solution)?;
    let artifact = powerio_core::MemoryArtifact::new(
        powerio_core::ArtifactPath::new("solution.json")
            .expect("static name is a valid artifact path"),
        text.into_bytes(),
    );
    destination.__commit_artifacts(
        false,
        powerio_core::Fidelity::Canonical,
        vec![artifact],
        Vec::new(),
    )
}

fn balanced_calculation_network(value: &PioValue) -> Option<&BalancedNetwork> {
    match value {
        PioValue::DcPfInstance(instance) => Some(instance.network()),
        PioValue::AcPfInstance(instance) => Some(instance.network()),
        PioValue::DcOpfInstance(instance) => Some(instance.network()),
        PioValue::AcOpfInstance(instance) => Some(instance.network()),
        PioValue::AcScucInstance(instance) => Some(instance.network()),
        _ => None,
    }
}

fn multiconductor_calculation_network(
    value: &PioValue,
) -> Option<&powerio_dist::MulticonductorNetwork> {
    match value {
        PioValue::McAcPfInstance(instance) => Some(instance.network()),
        PioValue::McAcOpfInstance(instance) => Some(instance.network()),
        _ => None,
    }
}

fn emit_network_or_calculation(
    module: &PioModule<PioValue>,
    format: &str,
    destination: Destination,
) -> Result<EmitResult, Error> {
    if matches!(module.value, PioValue::AcScucInstance(_)) && is_goc3(format) {
        return Err(unsupported_type(module, format));
    }
    if let Some(network) = balanced_calculation_network(&module.value) {
        return emit_balanced_network(
            module,
            network,
            format,
            destination,
            false,
            vec![calculation_data_omitted(module.value.type_name(), format)],
        );
    }
    if let Some(network) = multiconductor_calculation_network(&module.value) {
        return emit_multiconductor_network(
            module,
            network.clone(),
            format,
            destination,
            false,
            vec![calculation_data_omitted(module.value.type_name(), format)],
        );
    }
    match &module.value {
        PioValue::BalancedNetwork(network) => {
            emit_balanced_network(module, network, format, destination, true, Vec::new())
        }
        PioValue::MulticonductorNetwork(network) => emit_multiconductor_network(
            module,
            network.clone(),
            format,
            destination,
            true,
            Vec::new(),
        ),
        PioValue::BalancedOperatingPoint(point) => {
            let (network, diagnostics) = network_with_balanced_operating_point(point, format);
            emit_balanced_network(module, &network, format, destination, false, diagnostics)
        }
        PioValue::MulticonductorOperatingPoint(point) => {
            let (network, diagnostics) = network_with_multiconductor_operating_point(point, format);
            emit_multiconductor_network(module, network, format, destination, false, diagnostics)
        }
        _ => unreachable!("caller selected a value that is not a network or calculation"),
    }
}

fn emit_balanced_solution_network(
    module: &PioModule<PioValue>,
    network: &BalancedNetwork,
    format: &str,
    destination: Destination,
) -> Result<EmitResult, Error> {
    emit_balanced_network(
        module,
        network,
        format,
        destination,
        false,
        vec![solution_data_omitted(module.value.type_name(), format)],
    )
}

fn emit_multiconductor_solution_network(
    module: &PioModule<PioValue>,
    network: &powerio_dist::MulticonductorNetwork,
    format: &str,
    destination: Destination,
) -> Result<EmitResult, Error> {
    emit_multiconductor_network(
        module,
        network.clone(),
        format,
        destination,
        false,
        vec![solution_data_omitted(module.value.type_name(), format)],
    )
}

fn emit_solution(
    module: &PioModule<PioValue>,
    format: &str,
    destination: Destination,
) -> Result<EmitResult, Error> {
    match &module.value {
        PioValue::DcPfSolution(solution) => emit_balanced_solution_network(
            module,
            &network_with_dc_pf_solution(solution),
            format,
            destination,
        ),
        PioValue::AcPfSolution(solution) => emit_balanced_solution_network(
            module,
            &network_with_ac_pf_solution(solution),
            format,
            destination,
        ),
        PioValue::DcOpfSolution(solution) => emit_balanced_solution_network(
            module,
            &network_with_dc_opf_solution(solution),
            format,
            destination,
        ),
        PioValue::AcOpfSolution(solution) => emit_balanced_solution_network(
            module,
            &network_with_ac_opf_solution(solution),
            format,
            destination,
        ),
        PioValue::SocwrOpfSolution(solution) => emit_balanced_network(
            module,
            solution.network(),
            format,
            destination,
            false,
            vec![relaxation_data_omitted(format)],
        ),
        PioValue::McAcPfSolution(solution) => {
            emit_multiconductor_solution_network(module, solution.network(), format, destination)
        }
        PioValue::McAcOpfSolution(solution) => {
            emit_multiconductor_solution_network(module, solution.network(), format, destination)
        }
        PioValue::AcScucSolution(solution) if is_goc3(format) => {
            emit_goc3_solution(solution, destination)
        }
        PioValue::AcScucSolution(solution) => emit_balanced_solution_network(
            module,
            solution.instance().network(),
            format,
            destination,
        ),
        _ => unreachable!("caller selected a value that is not a solution"),
    }
}

fn emit_dynamic(
    module: &PioModule<PioValue>,
    format: &str,
    destination: Destination,
) -> Result<EmitResult, Error> {
    if let Some(artifacts) = echo_retained_directory(module, format)? {
        return destination.__commit_artifacts(
            true,
            powerio_core::Fidelity::ExactSameFormat,
            artifacts,
            Vec::new(),
        );
    }

    // A parse-only calculation format can still reproduce an unchanged
    // retained source exactly. A derived module has no retained source, so a
    // solved result never falls through to stale input bytes here.
    if !matches!(
        &module.value,
        PioValue::BalancedNetwork(_) | PioValue::MulticonductorNetwork(_)
    ) && let Some(bytes) = echo_retained_source(module, format)
    {
        let artifact = powerio_core::MemoryArtifact::new(
            powerio_core::ArtifactPath::new("case").expect("static name is a valid artifact path"),
            bytes,
        );
        return destination.__commit_artifacts(
            false,
            powerio_core::Fidelity::ExactSameFormat,
            vec![artifact],
            Vec::new(),
        );
    }

    match &module.value {
        PioValue::BalancedNetwork(_)
        | PioValue::MulticonductorNetwork(_)
        | PioValue::BalancedOperatingPoint(_)
        | PioValue::MulticonductorOperatingPoint(_)
        | PioValue::DcPfInstance(_)
        | PioValue::AcPfInstance(_)
        | PioValue::DcOpfInstance(_)
        | PioValue::AcOpfInstance(_)
        | PioValue::McAcPfInstance(_)
        | PioValue::McAcOpfInstance(_)
        | PioValue::AcScucInstance(_) => emit_network_or_calculation(module, format, destination),
        PioValue::DcPfSolution(_)
        | PioValue::AcPfSolution(_)
        | PioValue::DcOpfSolution(_)
        | PioValue::AcOpfSolution(_)
        | PioValue::SocwrOpfSolution(_)
        | PioValue::McAcPfSolution(_)
        | PioValue::McAcOpfSolution(_)
        | PioValue::AcScucSolution(_) => emit_solution(module, format, destination),
        _ => {
            if known_format_name(format) {
                Err(unsupported_type(module, format))
            } else {
                Err(unknown_format(format))
            }
        }
    }
}

/// True when `format` is a name some family recognizes, used to tell "wrong
/// kind for this format" apart from "no such format".
fn known_format_name(format: &str) -> bool {
    crate::resolve_format(format).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerio_core::{
        DiagnosticCode, DiagnosticSeverity, HistoryEntry, HistoryId, HistoryKind, Producer,
        SourceDescriptor, SourceId, SourceMapEntry, SourceRelation, SourceSpan,
    };
    use powerio_tx::{BalancedNetwork, Bus, BusId, BusType};

    #[test]
    fn a_typed_writer_sibling_preserves_every_common_record() {
        let network = BalancedNetwork::in_memory(
            "records",
            100.0,
            vec![Bus::new(BusId(1), BusType::Ref, 230.0)],
            vec![],
        );
        let source = powerio_core::Source::from_memory("case.m", b"case bytes".to_vec())
            .unwrap()
            .with_format(powerio_core::FormatId::new("matpower").unwrap());
        let source_id = SourceId::new("source-1").unwrap();
        let mut module = PioModule::new(PioValue::BalancedNetwork(network.clone()))
            .with_producer(Producer::new("records-test", "1").unwrap())
            .with_source(source);
        module
            .add_source_descriptor(SourceDescriptor::new(source_id.clone(), "case.m", 10).unwrap())
            .unwrap();
        module
            .add_source_map_entry(
                SourceMapEntry::new(
                    "/buses/0",
                    SourceRelation::Exact,
                    vec![SourceSpan::new(source_id, 0, 4).unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
        module
            .add_diagnostic(powerio_core::Diagnostic::new(
                DiagnosticCode::new("READ.TEST.RECORD").unwrap(),
                DiagnosticSeverity::Remark,
                "record carried to the family writer",
            ))
            .unwrap();
        module
            .add_history_entry(
                HistoryEntry::new(
                    HistoryId::new("history-1").unwrap(),
                    HistoryKind::Parse,
                    "parse",
                )
                .unwrap(),
            )
            .unwrap();
        module
            .insert_extension("test.writer", serde_json::json!({"kept": true}))
            .unwrap();

        let sibling = typed_sibling(&module, network).unwrap();
        assert_eq!(sibling.producer(), module.producer());
        assert_eq!(sibling.sources(), module.sources());
        assert_eq!(sibling.source_map(), module.source_map());
        assert_eq!(sibling.diagnostics, module.diagnostics);
        assert_eq!(sibling.history(), module.history());
        assert_eq!(sibling.extensions(), module.extensions());
        let sibling_source = sibling.source().unwrap();
        assert_eq!(sibling_source.name(), "case.m");
        assert_eq!(
            sibling_source.primary_buffer().unwrap().bytes(),
            b"case bytes"
        );
    }
}
