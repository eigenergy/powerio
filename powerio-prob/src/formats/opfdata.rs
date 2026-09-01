//! Map DeepMind OPFData JSON to [`AcOpfSolution`].

use std::sync::Arc;

use powerio_core::{Diagnostic, Error, PioModule, Source, TimePoint};

use super::source_text;
use crate::instance::AcOpfInstance;
use crate::operating::BalancedOperatingPointBuilder;
use crate::solution::{AcOpfSolution, Residuals, Termination};

/// Decode one OPFData document into the AC OPF solution it explicitly
/// represents. The instance is built from the document's network, with the
/// source supplied solver initial `pg`, `qg`, and `vg` recorded as its
/// initial point; the solution carries the solved columns and the stated
/// objective with termination [`Termination::NotReported`] (the source claims
/// a solution and reports nothing about how it was reached), and residuals
/// computed here from the solved voltages, flows, and injections.
///
/// # Errors
/// An invalid document, or solved columns whose shapes disagree with the
/// network's tables; every failure retains the source.
#[doc(hidden)]
pub fn __decode_opfdata_solution(source: Source) -> Result<PioModule<AcOpfSolution>, Error> {
    let source = match source.format() {
        Some(_) => source,
        None => source.with_format(powerio_core::FormatId::new("opfdata-json")?),
    };
    match parse_opfdata_text(&source) {
        Ok((solution, diagnostics)) => PioModule::parsed(solution, source, diagnostics),
        Err(error) => Err(error.with_source(source)),
    }
}

fn parse_opfdata_text(source: &Source) -> Result<(AcOpfSolution, Vec<Diagnostic>), Error> {
    let buffer = source.primary_buffer()?;
    let content = source_text(&buffer)?;
    let (network, solved, diagnostics) = powerio_tx::__parse_opfdata_json(content)
        .map_err(|error| Error::new(error.code(), error.to_string()))?;

    let initial = BalancedOperatingPointBuilder::new(
        network.clone(),
        vec![TimePoint::new("solver initial", None)?],
    )
    .generator_active_powers(solved.initial_generator_active_power.clone())
    .generator_reactive_powers(solved.initial_generator_reactive_power.clone())
    .generator_voltage_setpoints(solved.initial_generator_voltage_setpoint.clone())
    .build()?
    .values()[0]
        .clone();

    let residuals = power_balance_residuals(&network, &solved);
    let (bus_active_injection, bus_reactive_injection) = bus_injections(&network);

    let instance = Arc::new(AcOpfInstance::from_network(network)?.with_initial_point(initial));
    let solution = AcOpfSolution::new(
        instance,
        Termination::NotReported,
        solved.bus_voltage_magnitude,
        solved.bus_voltage_angle,
        bus_active_injection,
        bus_reactive_injection,
        solved.branch_from_active_flow,
        solved.branch_from_reactive_flow,
        solved.branch_to_active_flow,
        solved.branch_to_reactive_flow,
        solved.generator_active_power,
        solved.generator_reactive_power,
        solved.objective,
        Vec::new(),
    )?
    .with_residuals(residuals);

    Ok((solution, diagnostics))
}

/// Net bus injections from the solved dispatch and the stated demand:
/// generation minus load per bus, MW and MVAr, bus order. Shunts are network
/// elements evaluated at the solved voltage, so they enter the balance in
/// [`power_balance_residuals`] rather than the injection.
fn bus_injections(network: &powerio_tx::BalancedNetwork) -> (Vec<f64>, Vec<f64>) {
    let position: std::collections::HashMap<_, _> = network
        .buses()
        .iter()
        .enumerate()
        .map(|(index, bus)| (bus.id, index))
        .collect();
    let mut p = vec![0.0; network.buses().len()];
    let mut q = vec![0.0; network.buses().len()];
    for generator in network.generators() {
        if let Some(&at) = position.get(&generator.bus) {
            p[at] += generator.pg;
            q[at] += generator.qg;
        }
    }
    for load in network.loads() {
        if let Some(&at) = position.get(&load.bus) {
            p[at] -= load.p;
            q[at] -= load.q;
        }
    }
    (p, q)
}

/// The largest absolute per bus power balance mismatch of the stated
/// solution: net injection, minus the shunt consumption at the solved
/// voltage, minus the solved flows leaving the bus.
fn power_balance_residuals(
    network: &powerio_tx::BalancedNetwork,
    solved: &powerio_tx::format::OpfDataSolution,
) -> Residuals {
    let position: std::collections::HashMap<_, _> = network
        .buses()
        .iter()
        .enumerate()
        .map(|(index, bus)| (bus.id, index))
        .collect();
    let (mut p, mut q) = bus_injections(network);
    for shunt in network.shunts() {
        if let Some(&at) = position.get(&shunt.bus) {
            let vm2 = solved.bus_voltage_magnitude[at].powi(2);
            p[at] -= shunt.g * vm2;
            q[at] += shunt.b * vm2;
        }
    }
    for (index, branch) in network.branches().iter().enumerate() {
        if let Some(&at) = position.get(&branch.from) {
            p[at] -= solved.branch_from_active_flow[index];
            q[at] -= solved.branch_from_reactive_flow[index];
        }
        if let Some(&at) = position.get(&branch.to) {
            p[at] -= solved.branch_to_active_flow[index];
            q[at] -= solved.branch_to_reactive_flow[index];
        }
    }
    let largest = |values: &[f64]| {
        values
            .iter()
            .fold(0.0_f64, |largest, value| largest.max(value.abs()))
    };
    Residuals {
        max_active_power_mismatch: Some(largest(&p)),
        max_reactive_power_mismatch: Some(largest(&q)),
    }
}
