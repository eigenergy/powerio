//! Classify a PyPSA CSV sequence: input changes produce a network per
//! snapshot; a fixed network with only solved electrical state varying
//! produces an operating point series over one shared network.

use powerio_core::{Diagnostic, Error, TimeSeries};
use powerio_tx::BalancedNetwork;

use crate::state::{BalancedStateBuilder, OperatingPoint};

/// A parsed PyPSA sequence, classified by what varies.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum PypsaSequence {
    /// Problem input varies (setpoints, bounds): one network per snapshot,
    /// static tables shared across the series.
    Networks(TimeSeries<BalancedNetwork>),
    /// Only solved electrical state varies: one fixed network under an
    /// operating point per snapshot.
    OperatingPoints(TimeSeries<OperatingPoint<BalancedNetwork>>),
}

/// Parse a PyPSA CSV folder with time series siblings and classify the
/// result: input parameter changes (with or without state output) produce
/// [`PypsaSequence::Networks`]; a fixed network with only complete electrical
/// state output varying produces [`PypsaSequence::OperatingPoints`], holding
/// the state as columns over one shared network instead of a patched network
/// per snapshot.
///
/// # Errors
/// As the hub's sequence reader, or an operating point assembly whose shapes
/// disagree.
pub fn parse_pypsa_sequence(
    source: &powerio_core::Source,
) -> Result<(PypsaSequence, Vec<Diagnostic>), Error> {
    let sequence = powerio_tx::parse_pypsa_csv_time_series(source)
        .map_err(|error| Error::new(error.code(), error.to_string()))?;
    if sequence.inputs_vary {
        return Ok((
            PypsaSequence::Networks(sequence.series),
            sequence.diagnostics,
        ));
    }

    // Only state varied: lift the per snapshot voltages and injections off
    // the patched networks into dense point major columns over the first
    // point's network, so the series holds one network and one set of
    // columns rather than copied bus and generator tables per snapshot.
    let series = sequence.series;
    let network = series.values()[0].clone();
    let points = series.len();
    let buses = network.buses().len();
    let generators = network.generators().len();
    let loads = network.loads().len();
    let mut bus_vm = Vec::with_capacity(points * buses);
    let mut bus_va = Vec::with_capacity(points * buses);
    let mut gen_p = Vec::with_capacity(points * generators);
    let mut gen_q = Vec::with_capacity(points * generators);
    let mut load_p = Vec::with_capacity(points * loads);
    let mut load_q = Vec::with_capacity(points * loads);
    for point in series.values() {
        bus_vm.extend(point.buses().iter().map(|bus| bus.vm));
        bus_va.extend(point.buses().iter().map(|bus| bus.va));
        gen_p.extend(point.generators().iter().map(|g| g.pg));
        gen_q.extend(point.generators().iter().map(|g| g.qg));
        load_p.extend(point.loads().iter().map(|l| l.p));
        load_q.extend(point.loads().iter().map(|l| l.q));
    }
    let states = BalancedStateBuilder::new(network, series.time_points().to_vec())
        .bus_voltage_magnitudes(bus_vm)
        .bus_voltage_angles(bus_va)
        .generator_active_powers(gen_p)
        .generator_reactive_powers(gen_q)
        .load_active_powers(load_p)
        .load_reactive_powers(load_q)
        .build()?;
    Ok((PypsaSequence::OperatingPoints(states), sequence.diagnostics))
}
