//! Composition of balanced transmission and conductor-resolved feeder modules.
use num_complex::Complex64;
use powerio_core::{Error, PioModule};
use powerio_dist::MulticonductorNetwork;
use powerio_tx::{BalancedNetwork, BusId, BusType};
use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use crate::transform::UnionFind;

/// How the feeder's computed demand enters the transmission balance.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransmissionDemand {
    /// Additional demand, with every existing transmission load retained.
    Additional,
    /// Substitute feeder demand for these transmission load UIDs in coupled equations.
    /// The component modules retain the original load records.
    ReplaceLoads(Vec<String>),
}

/// A three-phase interface from a balanced bus to an earth-referenced feeder source.
///
/// The consumer replaces the selected source's three phase voltage prescriptions
/// with the coupled voltage equations. Extra source terminals remain fixed at zero.
/// Source names and terminal names are exact identifiers within the named feeder.
/// The source's stated phase voltages must agree with the interface: each within
/// 0.5 to 2 times the bus voltage times `voltage_ratio`, in A, B, C rotation.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Interface {
    pub name: String,
    pub transmission_bus: BusId,
    pub feeder: String,
    pub voltage_source: String,
    /// Feeder terminals corresponding to positive-sequence A, B and C, in order.
    pub phase_terminals: [String; 3],
    /// Feeder voltage divided by transmission voltage, on the same phase-to-earth basis.
    pub voltage_ratio: f64,
    /// Feeder phase A angle minus transmission positive-sequence angle, radians.
    pub angle_shift_radians: f64,
    pub demand: TransmissionDemand,
}
impl Interface {
    /// `voltage_ratio` is feeder phase-to-earth voltage divided by transmission
    /// phase-to-earth voltage; there is no default, since equal bases are rare.
    pub fn new(
        name: impl Into<String>,
        transmission_bus: BusId,
        feeder: impl Into<String>,
        voltage_source: impl Into<String>,
        phase_terminals: [String; 3],
        voltage_ratio: f64,
        demand: TransmissionDemand,
    ) -> Self {
        Self {
            name: name.into(),
            transmission_bus,
            feeder: feeder.into(),
            voltage_source: voltage_source.into(),
            phase_terminals,
            voltage_ratio,
            angle_shift_radians: 0.0,
            demand,
        }
    }
}

/// Validated immutable composition. Component modules retain source bytes and records.
///
/// This is an input model for coupled calculations. Constructing it neither solves
/// the networks nor changes their standalone equations. A consumer must apply each
/// interface's source substitution and explicit transmission demand policy.
/// Interfaces do not imply an automatic voltage-based partition of a source model.
#[derive(Clone, Debug)]
pub struct IntegratedNetwork {
    transmission: PioModule<BalancedNetwork>,
    feeders: BTreeMap<String, PioModule<MulticonductorNetwork>>,
    interfaces: Vec<Interface>,
}
impl IntegratedNetwork {
    pub fn new(
        transmission: PioModule<BalancedNetwork>,
        feeders: BTreeMap<String, PioModule<MulticonductorNetwork>>,
        interfaces: Vec<Interface>,
    ) -> Result<Self, Error> {
        let value = Self {
            transmission,
            feeders,
            interfaces,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn transmission(&self) -> &PioModule<BalancedNetwork> {
        &self.transmission
    }
    pub fn feeders(&self) -> &BTreeMap<String, PioModule<MulticonductorNetwork>> {
        &self.feeders
    }
    pub fn interfaces(&self) -> &[Interface] {
        &self.interfaces
    }

    /// Convert positive-sequence transmission voltage (per unit) to feeder A/B/C
    /// phase-to-earth phasors in volts. Uses the bus line-to-line kV base and the
    /// interface ratio; component MVA bases do not enter this voltage conversion.
    pub fn boundary_voltages(
        &self,
        interface: &str,
        voltage_pu: Complex64,
    ) -> Result<[Complex64; 3], Error> {
        let link = self
            .interfaces
            .iter()
            .find(|i| i.name == interface)
            .ok_or_else(|| invalid("unknown interface"))?;
        let bus = self
            .transmission
            .value()
            .buses()
            .iter()
            .find(|b| b.id == link.transmission_bus)
            .ok_or_else(|| invalid("interface transmission bus is missing"))?;
        let a = voltage_pu
            * (bus.base_kv * 1000.0 / 3.0_f64.sqrt() * link.voltage_ratio)
            * Complex64::from_polar(1.0, link.angle_shift_radians);
        let phases = [
            a,
            a * Complex64::from_polar(1.0, -TAU / 3.0),
            a * Complex64::from_polar(1.0, TAU / 3.0),
        ];
        if phases.iter().any(|v| !finite(*v)) {
            return Err(invalid("boundary voltage is nonfinite or overflows"));
        }
        Ok(phases)
    }

    #[allow(clippy::too_many_lines)]
    fn validate(&self) -> Result<(), Error> {
        let tx = self.transmission.value();
        tx.validate().map_err(invalid)?;
        tx.check_base_mva().map_err(invalid)?;
        if !positive(tx.base_frequency()) || self.feeders.is_empty() || self.interfaces.is_empty() {
            return Err(invalid(
                "composition needs a positive frequency, feeders, and interfaces",
            ));
        }
        let mut classes = BTreeMap::new();
        for (id, module) in &self.feeders {
            identifier(id)?;
            let net = module.value();
            powerio_dist::require_electrical_readiness(net)?;
            let refs = powerio_dist::find_unresolved_references(net);
            if !refs.is_empty() {
                return Err(invalid(format!("feeder {id}: {refs:?}")));
            }
            if !positive(net.base_frequency())
                || (net.base_frequency() - tx.base_frequency()).abs() > 1e-9
            {
                return Err(invalid(format!(
                    "feeder {id} frequency differs from transmission"
                )));
            }
            classes.insert(id, terminal_classes(net)?);
        }
        let mut names = BTreeSet::new();
        let mut sources = BTreeSet::new();
        let mut terminals = BTreeSet::new();
        let mut replaced = BTreeSet::new();
        let mut connected = BTreeSet::new();
        for link in &self.interfaces {
            identifier(&link.name)?;
            if !names.insert(&link.name) {
                return Err(invalid("duplicate interface name"));
            }
            if !positive(link.voltage_ratio) || !link.angle_shift_radians.is_finite() {
                return Err(invalid(
                    "interface voltage ratio must be positive and angle finite",
                ));
            }
            let bus = tx
                .buses()
                .iter()
                .find(|b| b.id == link.transmission_bus)
                .ok_or_else(|| invalid("unknown transmission bus"))?;
            if bus.kind == BusType::Isolated || !positive(bus.base_kv) {
                return Err(invalid(
                    "interface requires an active bus with positive voltage base",
                ));
            }
            let nominal = bus.base_kv * 1000.0 / 3.0_f64.sqrt() * link.voltage_ratio;
            let net = self
                .feeders
                .get(&link.feeder)
                .ok_or_else(|| invalid("unknown feeder"))?
                .value();
            connected.insert(&link.feeder);
            let matches: Vec<_> = net
                .sources()
                .iter()
                .filter(|s| s.name == link.voltage_source)
                .collect();
            if matches.len() != 1 || !sources.insert((&link.feeder, &link.voltage_source)) {
                return Err(invalid(
                    "interface source must exist exactly once and belong to one interface",
                ));
            }
            let source = matches[0];
            let bus = net
                .bus(&source.bus)
                .ok_or_else(|| invalid("unknown source bus"))?;
            if source.reference_terminal.is_some() {
                return Err(invalid(
                    "this interface requires an earth-referenced source",
                ));
            }
            let count = source.terminal_map.len();
            if count != source.v_magnitude.len()
                || count != source.v_angle.len()
                || source.terminal_map.iter().collect::<BTreeSet<_>>().len() != count
            {
                return Err(invalid(
                    "source phasor vectors or terminal identities are inconsistent",
                ));
            }
            if link.phase_terminals.iter().collect::<BTreeSet<_>>().len() != 3 {
                return Err(invalid("phase map must name three distinct terminals"));
            }
            for terminal in &link.phase_terminals {
                if !bus.terminals.contains(terminal)
                    || !source.terminal_map.contains(terminal)
                    || bus.grounded.contains(terminal)
                {
                    return Err(invalid(
                        "phase terminal must be declared, sourced, and ungrounded",
                    ));
                }
            }
            // Terminal identity follows closed ideal switches by terminal map,
            // so a voltage fixed elsewhere can still reach an interface phase.
            let classes = &classes[&link.feeder];
            let class = |bus: &str, terminal: &str| {
                classes
                    .get(&(bus.to_ascii_lowercase(), terminal.to_owned()))
                    .copied()
            };
            let phases: BTreeSet<_> = link
                .phase_terminals
                .iter()
                .filter_map(|terminal| class(&bus.id, terminal))
                .collect();
            if phases.len() != 3 {
                return Err(invalid(
                    "closed switches join interface phase terminals together",
                ));
            }
            for phase in &phases {
                if !terminals.insert((&link.feeder, *phase)) {
                    return Err(invalid(
                        "interfaces overlap at a feeder terminal, directly or through closed switches",
                    ));
                }
            }
            let mut fixed = net
                .sources()
                .iter()
                .flat_map(|s| s.terminal_map.iter().map(move |t| (s, t)))
                .filter(|(s, t)| s.name != source.name || !link.phase_terminals.contains(t))
                .filter_map(|(s, t)| class(&s.bus, t))
                .chain(
                    net.buses()
                        .iter()
                        .flat_map(|b| b.grounded.iter().filter_map(|t| class(&b.id, t))),
                );
            if fixed.any(|c| phases.contains(&c)) {
                return Err(invalid(
                    "another source terminal or a ground fixes an interface phase, directly or through closed switches",
                ));
            }
            // The stated phase voltages must describe the voltage the coupled
            // equations will impose: a wrong ratio or phase map fails here.
            let phasor = |terminal: &String| {
                let index = source
                    .terminal_map
                    .iter()
                    .position(|t| t == terminal)
                    .expect("phase terminals were checked to be sourced");
                (source.v_magnitude[index], source.v_angle[index])
            };
            let [a, b, c] = link.phase_terminals.each_ref().map(phasor);
            if [a, b, c]
                .iter()
                .any(|(magnitude, _)| !(0.5 * nominal..=2.0 * nominal).contains(magnitude))
            {
                return Err(invalid(
                    "stated source voltages disagree with the interface voltage ratio",
                ));
            }
            if !near_angle(b.1 - a.1, -TAU / 3.0) || !near_angle(c.1 - a.1, TAU / 3.0) {
                return Err(invalid(
                    "stated source rotation disagrees with the A, B, C phase map",
                ));
            }
            for (index, terminal) in source.terminal_map.iter().enumerate() {
                if !bus.terminals.contains(terminal)
                    || !source.v_magnitude[index].is_finite()
                    || source.v_magnitude[index] < 0.0
                    || !source.v_angle[index].is_finite()
                {
                    return Err(invalid("source has invalid terminal or phasor data"));
                }
                if !link.phase_terminals.contains(terminal) && source.v_magnitude[index] != 0.0 {
                    return Err(invalid(
                        "unbound source terminals must remain zero-volt earth clamps",
                    ));
                }
            }
            if let TransmissionDemand::ReplaceLoads(loads) = &link.demand {
                if loads.is_empty() {
                    return Err(invalid("replacement policy must name at least one load"));
                }
                for uid in loads {
                    let matches: Vec<_> = tx
                        .loads()
                        .iter()
                        .filter(|l| l.uid.as_ref() == Some(uid))
                        .collect();
                    if matches.len() != 1
                        || !matches[0].in_service
                        || matches[0].bus != link.transmission_bus
                        || !replaced.insert(uid)
                    {
                        return Err(invalid(
                            "replacement must name a unique active load at the interface bus, once",
                        ));
                    }
                }
            }
        }
        if connected.len() != self.feeders.len() {
            return Err(invalid("every feeder needs an interface"));
        }
        Ok(())
    }
}

/// Sum V times conjugate(I) in MW + j MVAr. Voltages are phase-to-earth volts,
/// currents are amperes positive into the feeder, in the same A/B/C order.
/// Export from the feeder produces negative active power. Lossless interface
/// power is invariant to the voltage ratio and does not use an MVA base.
pub fn boundary_power(
    voltage: [Complex64; 3],
    current: [Complex64; 3],
) -> Result<Complex64, Error> {
    if voltage.iter().chain(&current).any(|v| !finite(*v)) {
        return Err(invalid("boundary phasors must be finite"));
    }
    let power: Complex64 = voltage
        .into_iter()
        .zip(current)
        .map(|(v, i)| v * i.conj() / 1e6)
        .sum();
    if !finite(power) {
        return Err(invalid("boundary power overflows"));
    }
    Ok(power)
}
/// Class of every `(bus, terminal)` node joined by closed ideal switches,
/// pairing conductors by terminal map. Bus names compare case-insensitively,
/// as `MulticonductorNetwork::bus` does; terminal names are exact.
fn terminal_classes(
    net: &MulticonductorNetwork,
) -> Result<BTreeMap<(String, String), usize>, Error> {
    let key = |bus: &str, terminal: &str| (bus.to_ascii_lowercase(), terminal.to_owned());
    let closed: Vec<_> = net.switches().iter().filter(|s| !s.open).collect();
    let mut index = BTreeMap::new();
    let nodes = net
        .buses()
        .iter()
        .flat_map(|b| b.terminals.iter().chain(&b.grounded).map(|t| key(&b.id, t)))
        .chain(
            net.sources()
                .iter()
                .flat_map(|s| s.terminal_map.iter().map(|t| key(&s.bus, t))),
        )
        .chain(closed.iter().flat_map(|s| {
            let from = s.terminal_map_from.iter().map(|t| key(&s.bus_from, t));
            from.chain(s.terminal_map_to.iter().map(|t| key(&s.bus_to, t)))
        }));
    for node in nodes {
        let next = index.len();
        index.entry(node).or_insert(next);
    }
    let mut union = UnionFind::new(index.len());
    for switch in closed {
        if switch.terminal_map_from.is_empty()
            || switch.terminal_map_from.len() != switch.terminal_map_to.len()
        {
            return Err(invalid(format!(
                "closed switch {} must pair its terminals one to one",
                switch.name
            )));
        }
        for (a, b) in switch.terminal_map_from.iter().zip(&switch.terminal_map_to) {
            union.join(
                index[&key(&switch.bus_from, a)],
                index[&key(&switch.bus_to, b)],
            );
        }
    }
    Ok(index
        .into_iter()
        .map(|(node, row)| (node, union.root(row)))
        .collect())
}
/// Within 30 degrees of `expected`, modulo a full turn. Opposite rotation is
/// 120 degrees away, so a swapped phase map cannot pass.
fn near_angle(actual: f64, expected: f64) -> bool {
    (actual - expected)
        .rem_euclid(TAU)
        .min((expected - actual).rem_euclid(TAU))
        <= TAU / 12.0
}
fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}
fn finite(value: Complex64) -> bool {
    value.re.is_finite() && value.im.is_finite()
}
fn identifier(value: &str) -> Result<(), Error> {
    if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        Err(invalid(
            "identifier must be nonempty, bounded, and free of control characters",
        ))
    } else {
        Ok(())
    }
}
fn invalid(message: impl std::fmt::Display) -> Error {
    Error::new(
        &crate::diagnostics::VALIDATE_TND_INTERFACE,
        message.to_string(),
    )
}
