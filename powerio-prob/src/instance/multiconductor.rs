//! The multiconductor calculation instances: `McAcPfInstance` and
//! `McAcOpfInstance`.
//!
//! Both share the reusable [`MulticonductorNetwork`] handle. The power flow
//! instance contains the partial boundary specification a distribution power
//! flow needs — prescribed terminal powers, prescribed source terminal
//! voltages, isolated terminals, load voltage models, and the active
//! equipment control modes — never a required complete operating point. The
//! OPF instance adds the typed per phase objective and the active constraint
//! selections.

use std::collections::BTreeMap;

use powerio_core::Error;
use powerio_dist::MulticonductorNetwork;
use serde::{Deserialize, Serialize};

use crate::OperatingPoint;
use crate::diagnostics::codes;
use crate::instance::balanced::transform_discarded;
use crate::instance::constraints::MulticonductorActiveConstraints;
use crate::instance::objective::{Objective, ObjectiveTerm};

/// A source elsewhere in the document cannot anchor a disconnected island.
/// This is a necessary bus-level check only: line coupling, winding maps and
/// per-conductor voltage constraints still determine numerical solvability.
fn require_source_covered_islands(network: &MulticonductorNetwork) -> Result<(), Error> {
    let components = crate::update::multiconductor_network_connectivity(network);
    let positions: std::collections::BTreeMap<_, _> = network
        .buses()
        .iter()
        .enumerate()
        .map(|(index, bus)| (bus.id.to_ascii_lowercase(), index))
        .collect();
    let mut covered = std::collections::BTreeSet::new();
    for source in network.sources() {
        if let Some(&index) = positions.get(&source.bus.to_ascii_lowercase()) {
            covered.insert(components[index]);
        }
    }
    for (index, bus) in network.buses().iter().enumerate() {
        if !covered.contains(&components[index]) {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                format!(
                    "multiconductor island containing bus `{}` has no voltage source; explicitly resolve de-energized islands before constructing a calculation instance",
                    bus.id
                ),
            ));
        }
    }
    Ok(())
}

/// One load's prescribed terminal power: the load's stated per phase complex
/// power, keyed by the load's name and its terminal map. Watts and vars, as
/// the network states them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrescribedTerminalPower {
    pub load: String,
    /// The load's terminals, in its stated terminal map order.
    pub terminals: Vec<String>,
    /// Watts per terminal, aligned to `terminals`.
    pub p_w: Vec<f64>,
    /// Vars per terminal, aligned to `terminals`.
    pub q_var: Vec<f64>,
    /// The load's stated voltage dependence.
    pub voltage_model: powerio_dist::DistLoadVoltageModel,
}

/// One source's prescribed phasor parameters, keyed to its network source.
/// This record does not carry the bus or voltage reference. Use
/// [`McAcPfInstance::source_boundary`] for the complete borrowed boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrescribedSourceVoltage {
    pub source: String,
    /// The source's terminals, in its stated terminal map order.
    pub terminals: Vec<String>,
    /// Source voltage magnitudes in volts; their reference is on the network source.
    pub v_magnitude: Vec<f64>,
    /// Radians per terminal.
    pub v_angle: Vec<f64>,
}

/// Complete borrowed source boundary: V(terminal) - V(reference) is the
/// stated phasor. None means earth; a named reference is on `bus` and does
/// not imply a connection to earth. Every phase current has an equal
/// opposite contribution at the reference.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct PrescribedSourceBoundary<'a> {
    pub source: &'a str,
    pub bus: &'a str,
    pub terminals: &'a [String],
    pub reference_terminal: Option<&'a str>,
    pub v_magnitude: &'a [f64],
    pub v_angle: &'a [f64],
}

/// One equipment control that is active for the calculation, by element name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum ActiveControlMode {
    /// A regulating transformer's tap control.
    RegulatorTap { transformer: String },
    /// A switched capacitor's step control.
    CapacitorSteps { capacitor: String },
}

/// The multiconductor AC power flow instance.
#[derive(Clone, Debug)]
pub struct McAcPfInstance {
    network: MulticonductorNetwork,
    loads: Vec<PrescribedTerminalPower>,
    sources: Vec<PrescribedSourceVoltage>,
    isolated_terminals: Vec<(String, String)>,
    control_modes: Vec<ActiveControlMode>,
    initial_point: Option<OperatingPoint<MulticonductorNetwork>>,
}

impl McAcPfInstance {
    /// Build the instance from the network's stated data: every in service
    /// load contributes its prescribed per phase powers and voltage model,
    /// every voltage source its prescribed terminal complex voltage, and
    /// every stated regulator and capacitor control an active control mode.
    ///
    /// # Errors
    /// A network with no voltage source, or a bus island without a voltage
    /// source. This necessary connectivity check does not establish conductor
    /// energization or numerical solvability.
    pub fn from_network(network: MulticonductorNetwork) -> Result<Self, Error> {
        powerio_dist::require_electrical_readiness(&network)?;
        if network.sources().is_empty() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                "the multiconductor network states no voltage source to anchor the calculation",
            ));
        }
        require_source_covered_islands(&network)?;
        let loads = network
            .loads()
            .iter()
            .map(|load| PrescribedTerminalPower {
                load: load.name.clone(),
                terminals: load.terminal_map.clone(),
                p_w: load.p_nom.clone(),
                q_var: load.q_nom.clone(),
                voltage_model: load.voltage_model.clone(),
            })
            .collect();
        let sources = network
            .sources()
            .iter()
            .map(|source| PrescribedSourceVoltage {
                source: source.name.clone(),
                terminals: source.terminal_map.clone(),
                v_magnitude: source.v_magnitude.clone(),
                v_angle: source.v_angle.clone(),
            })
            .collect();
        // Controllers live in the retained untyped objects: a `regcontrol`
        // regulates the transformer its `transformer` property names, and a
        // `capcontrol` switches the capacitor its `capacitor` property names.
        let controlled_element = |object: &powerio_dist::UntypedObject, key: &str| {
            object
                .props
                .iter()
                .find(|(name, _)| name.as_deref() == Some(key))
                .map(|(_, value)| value.clone())
        };
        let mut control_modes = Vec::new();
        for object in network.untyped_objects() {
            match object.class.to_ascii_lowercase().as_str() {
                "regcontrol" => {
                    if let Some(transformer) = controlled_element(object, "transformer") {
                        control_modes.push(ActiveControlMode::RegulatorTap { transformer });
                    }
                }
                "capcontrol" => {
                    if let Some(capacitor) = controlled_element(object, "capacitor") {
                        control_modes.push(ActiveControlMode::CapacitorSteps { capacitor });
                    }
                }
                _ => {}
            }
        }
        Ok(Self {
            network,
            loads,
            sources,
            isolated_terminals: Vec::new(),
            control_modes,
            initial_point: None,
        })
    }

    /// Supply an optional solver initial point.
    #[must_use]
    pub fn with_initial_point(mut self, point: OperatingPoint<MulticonductorNetwork>) -> Self {
        self.initial_point = Some(point);
        self
    }

    /// Replace the network and recalculate its prescribed powers, source
    /// voltages, and active controls while preserving a compatible initial
    /// point.
    pub fn with_network(mut self, network: MulticonductorNetwork) -> Result<Self, Error> {
        let mut replacement = Self::from_network(network.clone())?;
        if let Some(initial) = self.initial_point.take() {
            replacement.initial_point = Some(initial.rebind_network(network)?);
        }
        Ok(replacement)
    }

    /// The network this instance calculates on. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &MulticonductorNetwork {
        &self.network
    }

    /// The prescribed load terminal powers, in load table order.
    #[must_use]
    pub fn loads(&self) -> &[PrescribedTerminalPower] {
        &self.loads
    }

    /// Source phasor parameters in source table order. Resolve each record
    /// against the associated network source, or use [`Self::source_boundary`]
    /// to obtain both endpoints with the phasors.
    #[must_use]
    pub fn sources(&self) -> &[PrescribedSourceVoltage] {
        &self.sources
    }

    /// Both voltage endpoints and their phasor differences, borrowed from
    /// this instance. No network cloning or serialization is involved.
    pub fn source_boundary(&self, index: usize) -> Option<PrescribedSourceBoundary<'_>> {
        let values = self.sources.get(index)?;
        let source = self.network.sources().get(index)?;
        Some(PrescribedSourceBoundary {
            source: &values.source,
            bus: &source.bus,
            terminals: &values.terminals,
            reference_terminal: source.reference_terminal.as_deref(),
            v_magnitude: &values.v_magnitude,
            v_angle: &values.v_angle,
        })
    }

    /// Terminals with no equation, as `(bus, terminal)` pairs.
    #[must_use]
    pub fn isolated_terminals(&self) -> &[(String, String)] {
        &self.isolated_terminals
    }

    /// The equipment controls active for the calculation.
    #[must_use]
    pub fn control_modes(&self) -> &[ActiveControlMode] {
        &self.control_modes
    }

    /// The optional solver initial point.
    #[must_use]
    pub const fn initial_point(&self) -> Option<&OperatingPoint<MulticonductorNetwork>> {
        self.initial_point.as_ref()
    }
}

/// The multiconductor AC optimal power flow instance: the shared network,
/// the typed per phase objective, and the active terminal voltage, conductor,
/// and per phase generator constraint selections.
#[derive(Clone, Debug)]
pub struct McAcOpfInstance {
    network: MulticonductorNetwork,
    objective: Objective,
    constraints: MulticonductorActiveConstraints,
    initial_point: Option<OperatingPoint<MulticonductorNetwork>>,
}

impl McAcOpfInstance {
    /// Build the instance with the default per phase objective and every
    /// stated limit active.
    ///
    /// # Errors
    /// A network with no voltage source, or a bus island without one.
    pub fn from_network(network: MulticonductorNetwork) -> Result<Self, Error> {
        powerio_dist::require_electrical_readiness(&network)?;
        if network.sources().is_empty() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                "the multiconductor network states no voltage source to anchor the calculation",
            ));
        }
        require_source_covered_islands(&network)?;
        Ok(Self {
            network,
            objective: Objective::active_power_dispatch_cost(),
            constraints: MulticonductorActiveConstraints::default(),
            initial_point: None,
        })
    }

    /// Replace the objective, consuming the instance. The shared network
    /// moves; no table is copied.
    #[must_use]
    pub fn with_objective(mut self, objective: Objective) -> Self {
        self.objective = objective;
        self
    }

    /// Append one objective term, consuming the instance.
    #[must_use]
    pub fn with_objective_term(mut self, term: ObjectiveTerm) -> Self {
        self.objective = std::mem::take(&mut self.objective).with_term(term);
        self
    }

    /// Replace the active constraint selections, consuming the instance.
    #[must_use]
    pub fn with_constraints(mut self, constraints: MulticonductorActiveConstraints) -> Self {
        self.constraints = constraints;
        self
    }

    /// Supply an optional solver initial point.
    #[must_use]
    pub fn with_initial_point(mut self, point: OperatingPoint<MulticonductorNetwork>) -> Self {
        self.initial_point = Some(point);
        self
    }

    /// Replace the network while preserving this instance's objective,
    /// constraint selections, and a compatible initial point.
    pub fn with_network(mut self, network: MulticonductorNetwork) -> Result<Self, Error> {
        powerio_dist::require_electrical_readiness(&network)?;
        if network.sources().is_empty() {
            return Err(Error::new(
                &codes::BUILD_INSTANCE_SHAPE_MISMATCH,
                "the multiconductor network states no voltage source to anchor the calculation",
            ));
        }
        require_source_covered_islands(&network)?;
        if let Some(initial) = self.initial_point.take() {
            self.initial_point = Some(initial.rebind_network(network.clone())?);
        }
        self.network = network;
        Ok(self)
    }

    /// The network this instance calculates on. Borrowed; never a copy.
    #[must_use]
    pub fn network(&self) -> &MulticonductorNetwork {
        &self.network
    }

    /// The typed objective.
    #[must_use]
    pub const fn objective(&self) -> &Objective {
        &self.objective
    }

    /// The active constraint selections.
    #[must_use]
    pub const fn constraints(&self) -> &MulticonductorActiveConstraints {
        &self.constraints
    }

    /// The optional solver initial point.
    #[must_use]
    pub const fn initial_point(&self) -> Option<&OperatingPoint<MulticonductorNetwork>> {
        self.initial_point.as_ref()
    }

    /// The multiconductor power flow instance for this problem's network at
    /// its stated injections: the objective and constraint selections are
    /// discarded, and the discard is recorded.
    ///
    /// # Errors
    /// As [`McAcPfInstance::from_network`].
    pub fn to_mc_ac_pf(&self) -> Result<(McAcPfInstance, Vec<powerio_core::Diagnostic>), Error> {
        let instance = McAcPfInstance::from_network(self.network.clone())?;
        Ok((
            instance,
            vec![transform_discarded(
                "the objective and the active constraint selections",
            )],
        ))
    }
}

/// The relative difference under which two nominal voltage anchors of one
/// zone count as the same level. Ratings imported from different formats
/// commonly mix rounded line to line and line to neutral values (415/√3
/// against 240 V, say).
const NOMINAL_ANCHOR_TOLERANCE: f64 = 1e-2;

impl McAcPfInstance {
    /// The branch nominal voltage, volts, of every constant power load whose
    /// source stated none, keyed by load name with one value per stated
    /// power. Every other load is absent from the map.
    ///
    /// The voltage level is a property of a zone: the buses that lines and
    /// closed switches join, never crossing a transformer. A zone's line to
    /// neutral level is anchored by the nameplates in it (each transformer
    /// winding's rated voltage, each capacitor's and each load's stated
    /// nominal voltage), or, in a zone with no nameplate, by the median
    /// prescribed magnitude of a voltage source the instance states there.
    /// Anchors of one zone must agree within one percent; the first stated
    /// anchor is the level. A wye load's branch voltage is the zone's line
    /// to neutral level, a delta load's (or a single phase load across two
    /// phase terminals) its line to line level.
    ///
    /// This is not the PMD MATHEMATICAL writer's `calc_voltage_bases`
    /// reproduction (powerio-dist), and the two share only their zone
    /// labeling (`powerio_core::union_find::calc_component_labels`). The
    /// writer floods a stated base (`settings.vbases_default` or a read
    /// document's `pmd_vbase`) across transformers by their nominal ratio,
    /// over the lowered buses joined by every branch and every switch,
    /// open or closed, as PMD does, and falls back to a load's rating or
    /// 1 kV for an island no seed reaches. Here no level crosses a
    /// transformer, an open switch splits a zone, and every zone is anchored
    /// by its own nameplates or source, never by a neighbour's base. Sharing
    /// the flood would change either result.
    ///
    /// # Errors
    /// An element naming an unknown bus, an anchor that is not finite and
    /// positive, two anchors of one zone further apart than one percent, or
    /// a load to infer whose zone has no anchor or whose configuration has
    /// no branch voltage rule.
    #[allow(clippy::too_many_lines)] // one anchor family per block, as documented
    pub fn calc_inferred_load_nominal_voltages(&self) -> Result<BTreeMap<String, Vec<f64>>, Error> {
        use powerio_dist::{Configuration, DistLoadVoltageModel};
        let network = &self.network;
        let unresolved = |message: String| {
            Error::new(&codes::BUILD_INSTANCE_NOMINAL_VOLTAGE_UNRESOLVED, message)
        };
        let to_infer = |load: &powerio_dist::DistLoad| {
            load.voltage_model.v_nom().is_empty()
                && matches!(
                    load.voltage_model,
                    DistLoadVoltageModel::ConstantPower { .. }
                )
        };
        if !network.loads().iter().any(to_infer) {
            return Ok(BTreeMap::new());
        }
        let bus_position: BTreeMap<_, _> = network
            .buses()
            .iter()
            .enumerate()
            .map(|(index, bus)| (bus.id.to_ascii_lowercase(), index))
            .collect();
        let position_of = |bus: &str, label: &str| {
            bus_position
                .get(&bus.to_ascii_lowercase())
                .copied()
                .ok_or_else(|| unresolved(format!("{label} references unknown bus `{bus}`")))
        };
        // Voltage zones: the buses lines and closed switches join.
        let zone =
            powerio_dist::voltage_zones::calc_voltage_zone_labels(network).map_err(unresolved)?;

        let conventions = network.terminal_conventions();
        let phase_to_phase = |position: usize, terminals: &[String]| {
            let bus = &network.buses()[position];
            let phases: std::collections::BTreeSet<_> = bus
                .phase_indices(conventions)
                .into_iter()
                .map(|index| bus.terminals[index].as_str())
                .collect();
            terminals.len() == 2
                && terminals
                    .iter()
                    .all(|terminal| phases.contains(terminal.as_str()))
        };
        // The line to line factor of a branch: √3 for a delta branch or a
        // single phase branch across two phase terminals, one otherwise.
        let line_to_line =
            |configuration: Configuration, across_phases: bool, label: &str| match configuration {
                Configuration::Delta => Ok(3f64.sqrt()),
                Configuration::SinglePhase if across_phases => Ok(3f64.sqrt()),
                Configuration::Wye | Configuration::SinglePhase => Ok(1.0),
                _ => Err(unresolved(format!(
                    "{label} has an unsupported configuration for nominal voltage inference"
                ))),
            };

        let mut rated = BTreeMap::<usize, Vec<(String, f64)>>::new();
        let mut operating = BTreeMap::<usize, Vec<(String, f64)>>::new();
        let add = |position: usize,
                   label: String,
                   value: f64,
                   target: &mut BTreeMap<usize, Vec<(String, f64)>>|
         -> Result<(), Error> {
            if !value.is_finite() || value <= 0.0 {
                return Err(unresolved(format!(
                    "{label} provides an invalid nominal voltage"
                )));
            }
            target
                .entry(zone[position])
                .or_default()
                .push((label, value));
            Ok(())
        };

        for transformer in network.transformers() {
            for (index, winding) in transformer.windings.iter().enumerate() {
                let label = format!("transformer `{}` winding {}", transformer.name, index + 1);
                let line_neutral = if transformer.phases >= 2 {
                    winding.v_ref / 3f64.sqrt()
                } else {
                    winding.v_ref
                };
                add(
                    position_of(&winding.bus, &label)?,
                    label,
                    line_neutral,
                    &mut rated,
                )?;
            }
        }
        for capacitor in network.capacitors() {
            let label = format!("capacitor `{}`", capacitor.name);
            let position = position_of(&capacitor.bus, &label)?;
            let factor = line_to_line(
                capacitor.configuration,
                phase_to_phase(position, &capacitor.terminal_map),
                &label,
            )?;
            add(position, label, capacitor.v_nom / factor, &mut rated)?;
        }
        for load in network.loads() {
            let values = load.voltage_model.v_nom();
            if values.is_empty() {
                continue;
            }
            let label = format!("load `{}`", load.name);
            let position = position_of(&load.bus, &label)?;
            let factor = line_to_line(
                load.configuration,
                phase_to_phase(position, &load.terminal_map),
                &label,
            )?;
            for (branch, value) in values.iter().copied().enumerate() {
                add(
                    position,
                    format!("load `{}` branch {branch}", load.name),
                    value / factor,
                    &mut rated,
                )?;
            }
        }
        for prescribed in &self.sources {
            let voltage_source = network
                .sources()
                .iter()
                .find(|source| source.name == prescribed.source)
                .ok_or_else(|| {
                    unresolved(format!(
                        "instance voltage source `{}` is absent from its network",
                        prescribed.source
                    ))
                })?;
            let mut magnitudes = prescribed
                .v_magnitude
                .iter()
                .copied()
                .filter(|value| value.is_finite() && *value > 0.0)
                .collect::<Vec<_>>();
            if magnitudes.is_empty() {
                continue;
            }
            magnitudes.sort_by(f64::total_cmp);
            let label = format!("voltage source `{}`", voltage_source.name);
            add(
                position_of(&voltage_source.bus, &label)?,
                label,
                magnitudes[magnitudes.len() / 2],
                &mut operating,
            )?;
        }

        let mut bases = BTreeMap::<usize, f64>::new();
        for &root in &zone {
            if bases.contains_key(&root) {
                continue;
            }
            let Some(candidates) = rated.get(&root).or_else(|| operating.get(&root)) else {
                continue;
            };
            let base = candidates[0].1;
            for (label, candidate) in &candidates[1..] {
                let relative = (candidate - base).abs() / base.max(*candidate);
                if relative > NOMINAL_ANCHOR_TOLERANCE {
                    return Err(unresolved(format!(
                        "conflicting nominal voltage anchors in one line-connected component: {base} V and {label}={candidate} V"
                    )));
                }
            }
            bases.insert(root, base);
        }

        let mut inferred = BTreeMap::new();
        for load in network.loads().iter().filter(|load| to_infer(load)) {
            let label = format!("load `{}`", load.name);
            let position = position_of(&load.bus, &label)?;
            let line_neutral = *bases.get(&zone[position]).ok_or_else(|| {
                unresolved(format!(
                    "load `{}` omits nominal voltage and bus `{}` has no source, transformer, or nameplate voltage anchor",
                    load.name, load.bus
                ))
            })?;
            let factor = line_to_line(
                load.configuration,
                phase_to_phase(position, &load.terminal_map),
                &label,
            )?;
            inferred.insert(
                load.name.clone(),
                vec![line_neutral * factor; load.p_nom.len()],
            );
        }
        Ok(inferred)
    }
}
