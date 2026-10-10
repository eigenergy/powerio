use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;

use num_complex::Complex64;
use powerio_core::{ArtifactPath, Destination, EmitResult, Error, MemoryArtifact};

use crate::convert::TextEmission;
use crate::diagnostics::{Diagnostics, codes};
use crate::{
    Configuration, DistBus, DistLine, DistLineCode, DistLoadVoltageModel, MulticonductorNetwork,
};

use super::read::{nominal_phasor, phase_name};

const METERS_PER_MILE: f64 = 1609.344;

mod coil_banks;
mod ideal;
mod loads;
mod regulators;
mod services;
mod shunts;
mod transformers;

/// Emit fresh GLM from the current typed network, without retained-source echo.
///
/// The initial profile supports earth/phase-pair P/I/Z loads, ideal fixed sources,
/// explicit phase line matrices, ordinary grounded-wye transformers and fixed
/// contacts. Grounded center-tapped services with private impedance core loads
/// emit native transformers; triplex P/I/Z terms and qualified full-neutral or
/// direct hot-conductor line matrices emit from current typed coefficients.
/// Coherent three-coil phase-pair banks emit native delta-grounded-wye
/// configurations using current effective winding voltages and impedances.
/// Fixed resistive regulator equivalents emit manual Type B settings.
/// Ideal closed contacts and grounded ideal phase ratios select exact FBS
/// output on a checked forest with one source bus per component. Independent
/// phase ratios use native series stages; private service core circuits are
/// contracted for the forest check. Redundant fully open ties use unloaded
/// parented endpoint aliases to preserve their voltages and zero port currents.
/// FBS uses a 1,000-iteration budget with unchanged node stopping precision.
/// Symmetric line end shunts use exact admittance-load equivalents when native
/// line parameters cannot preserve their port equations. Returned diagnostics
/// identify the generated loads that contribute to physical line port flows.
/// Unsupported electrical behavior fails
/// before the destination is written; omitted non-electrical metadata is
/// reported. This entry point deliberately takes a value, not a source module.
pub fn emit_canonical(
    network: &MulticonductorNetwork,
    destination: Destination,
) -> Result<EmitResult, Error> {
    let emission = emit_text(network)?;
    destination.__commit_artifacts(
        false,
        emission.fidelity,
        vec![MemoryArtifact::new(
            ArtifactPath::new("case.glm")?,
            emission.text.into_bytes(),
        )],
        emission.diagnostics,
    )
}

pub(crate) fn emit_text(network: &MulticonductorNetwork) -> Result<TextEmission, Error> {
    crate::require_electrical_readiness(network)?;
    network.validate_component_locations()?;
    let mut writer = Writer {
        net: network,
        out: String::new(),
        diagnostics: Diagnostics::new(),
        identities: BTreeSet::new(),
        phases: BTreeMap::new(),
        source_voltages: BTreeMap::new(),
        nominal: BTreeMap::new(),
        locations: network
            .component_locations()
            .iter()
            .map(|p| ((p.target, p.name.as_str()), p))
            .collect(),
        written_locations: BTreeSet::new(),
        ideal_plan: None,
        services: services::Services::discover(network)?,
        coil_banks: coil_banks::CoilBanks::discover(network)?,
        regulators: regulators::Regulators::discover(network)?,
    };
    writer.inventory()?;
    writer.sources()?;
    writer.prepare_ideal_contacts()?;
    writer.validate_transformers()?;
    writer.nominal_voltages()?;
    let solver = if writer.ideal_plan.is_some() {
        "FBS"
    } else {
        "NR"
    };
    let _ = writeln!(
        writer.out,
        "// Canonical PowerIO static network.\nmodule powerflow {{ solver_method {solver}; line_capacitance TRUE; nominal_frequency {}; }}\n",
        network.base_frequency()
    );
    if writer.ideal_plan.is_some() {
        writer.out.push_str("#set iteration_limit=1000;\n");
    }
    if writer
        .identities
        .iter()
        .any(|name| !name.as_bytes()[0].is_ascii_alphabetic() && name.as_bytes()[0] != b'_')
    {
        writer.out.push_str("#set relax_naming_rules=1;\n");
    }
    writer.buses()?;
    writer.lines()?;
    writer.switches()?;
    writer.transformers()?;
    writer.loads()?;
    writer.capacitor_shunts()?;
    writer.remaining_locations();
    Ok(TextEmission::new(
        writer.out,
        Vec::new(),
        writer.diagnostics,
    ))
}

struct Writer<'a> {
    net: &'a MulticonductorNetwork,
    out: String,
    diagnostics: Diagnostics,
    identities: BTreeSet<String>,
    phases: BTreeMap<&'a str, Vec<usize>>,
    source_voltages: BTreeMap<&'a str, BTreeMap<usize, Complex64>>,
    nominal: BTreeMap<&'a str, f64>,
    locations: BTreeMap<(crate::DistLocationTarget, &'a str), &'a crate::DistComponentLocation>,
    written_locations: BTreeSet<(crate::DistLocationTarget, &'a str)>,
    ideal_plan: Option<ideal::IdealPlan<'a>>,
    services: services::Services<'a>,
    coil_banks: coil_banks::CoilBanks<'a>,
    regulators: regulators::Regulators<'a>,
}

fn unsupported(message: impl Into<String>) -> Error {
    Error::new(&codes::EMIT_GLM_UNSUPPORTED, message)
}

impl Writer<'_> {
    fn coordinates(&mut self, target: crate::DistLocationTarget, name: &str) {
        let Some(location) = self.locations.get(&(target, name)).copied() else {
            return;
        };
        if let Some(latitude) = location.point.latitude {
            let _ = writeln!(self.out, "    latitude {latitude};");
        }
        if let Some(longitude) = location.point.longitude {
            let _ = writeln!(self.out, "    longitude {longitude};");
        }
        self.written_locations
            .insert((target, location.name.as_str()));
        if location.point.datum.is_some()
            || location
                .kind
                .is_some_and(|k| k != crate::DistCoordsKind::Source)
        {
            self.dropped(format!("{target:?} {name}: GLM has no declared coordinate datum or point-origin field; numeric angles are unchanged"));
        }
    }

    fn remaining_locations(&mut self) {
        for &key in self.locations.keys() {
            if !self.written_locations.contains(&key) {
                self.diagnostics.push(
                    &codes::EMIT_DIST_COMPONENT_COORDINATES_DROPPED,
                    format!(
                        "{:?} {} has no point attachment in this GLM output profile",
                        key.0, key.1
                    ),
                );
            }
        }
    }

    fn inventory(&mut self) -> Result<(), Error> {
        if self.net.buses().is_empty() {
            return Err(unsupported("GLM output needs at least one electrical bus"));
        }
        if ![50.0, 60.0].contains(&self.net.base_frequency()) {
            return Err(unsupported(
                "nominal frequencies other than 50 or 60 Hz are not qualified",
            ));
        }
        if !self.net.generators().is_empty()
            || !self.net.ibrs().is_empty()
            || !self.net.capacitors().is_empty()
            || !self.net.control_profiles().is_empty()
            || !self.net.untyped_objects().is_empty()
        {
            return Err(unsupported(
                "additional device tables need a qualified GLM mapping",
            ));
        }
        if !self.net.extras().is_empty()
            || !self.net.options().is_empty()
            || !self.net.commands().is_empty()
        {
            return Err(unsupported(
                "uninterpreted network extras, options or commands may affect physics",
            ));
        }
        if self.net.name().is_some() || self.net.geo().is_some() {
            self.dropped("network name/coordinate metadata is not emitted by this GLM profile");
        }
        for bus in self.net.buses() {
            self.reserve(&bus.id)?;
            if !bus.extras.is_empty() {
                return Err(unsupported(format!(
                    "bus {} has uninterpreted extras",
                    bus.id
                )));
            }
            if bus.location.is_some() || has_bus_bounds(bus) {
                self.dropped(format!(
                    "bus {}: coordinates/optimization voltage bounds are not emitted",
                    bus.id
                ));
            }
            if bus.grounded.len() != 1
                || bus.terminals.iter().filter(|t| t.as_str() == "n").count() != 1
                || bus.grounded.iter().any(|t| t != "n")
                || !bus.grounded.iter().any(|t| t == "n")
                || !bus.terminals.iter().any(|t| t == "n")
            {
                return Err(unsupported(format!(
                    "bus {} requires explicit conductor/ground interpretation",
                    bus.id
                )));
            }
            if self.services.buses.contains_key(bus.id.as_str()) && bus.terminals != ["1", "2", "n"]
            {
                return Err(unsupported(
                    "triplex buses need exactly two hot terminals and a grounded neutral",
                ));
            }
            let phases = phases(
                &bus.terminals
                    .iter()
                    .filter(|t| *t != "n")
                    .cloned()
                    .collect::<Vec<_>>(),
            )?;
            self.phases.insert(&bus.id, phases);
        }
        for line in self.net.lines() {
            self.reserve(&line.name)?;
        }
        for t in self.net.transformers() {
            self.reserve(&t.name)?;
        }
        for load in self.net.loads() {
            self.reserve(&load.name)?;
        }
        for shunt in self.net.shunts() {
            self.reserve(&shunt.name)?;
        }
        for switch in self.net.switches() {
            self.reserve(&switch.name)?;
        }
        Ok(())
    }

    fn sources(&mut self) -> Result<(), Error> {
        let buses: BTreeMap<_, _> = self
            .net
            .buses()
            .iter()
            .map(|b| (b.id.as_str(), b))
            .collect();
        for source in self.net.sources() {
            let bus = buses.get(source.bus.as_str()).ok_or_else(|| {
                unsupported(format!("source {} has no exact bus identity", source.name))
            })?;
            if !source.extras.is_empty() {
                return Err(unsupported(format!(
                    "source {} has uninterpreted electrical extras",
                    source.name
                )));
            }
            if source
                .reference_terminal
                .as_ref()
                .is_some_and(|t| !bus.grounded.contains(t))
            {
                return Err(unsupported(format!(
                    "source {} has a floating reference terminal",
                    source.name
                )));
            }
            if source.name != source.bus || source.energy_cost_rate.is_some() {
                self.dropped(format!("source {} becomes the SWING state of bus {}; its independent name/cost is not emitted", source.name, source.bus));
            }
            let source_phases = phases(&source.terminal_map)?;
            if source_phases.len() != source.v_magnitude.len()
                || source_phases.len() != source.v_angle.len()
            {
                return Err(unsupported(format!(
                    "source {} has incomplete phasors",
                    source.name
                )));
            }
            for (i, phase) in source_phases.into_iter().enumerate() {
                let magnitude = source.v_magnitude[i];
                let angle = source.v_angle[i];
                if !magnitude.is_finite() || magnitude <= 0.0 || !angle.is_finite() {
                    return Err(unsupported(
                        "GLM SWING zero voltages select nominal defaults; nonpositive/nonfinite source phasors cannot be emitted",
                    ));
                }
                if !self.phases[bus.id.as_str()].contains(&phase) {
                    return Err(unsupported("source phase is absent from its bus"));
                }
                let value = Complex64::from_polar(magnitude, angle);
                let voltages = self.source_voltages.entry(&bus.id).or_default();
                if voltages.insert(phase, value).is_some() {
                    return Err(unsupported(
                        "multiple source constraints on one phase require explicit equivalence checking",
                    ));
                }
            }
        }
        for (&bus, voltages) in &self.source_voltages {
            if voltages.len() != self.phases[bus].len() {
                return Err(unsupported(format!(
                    "bus {bus}: partial SWING constraints would energize additional phases"
                )));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Connected propagation includes distinct native winding bases.
    fn nominal_voltages(&mut self) -> Result<(), Error> {
        let mut neighbors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for line in self.net.lines() {
            if self.services.buses.contains_key(line.bus_from.as_str()) {
                neighbors
                    .entry(&line.bus_from)
                    .or_default()
                    .push(&line.bus_to);
                neighbors
                    .entry(&line.bus_to)
                    .or_default()
                    .push(&line.bus_from);
                continue;
            }
            self.link_phases(
                &line.name,
                &line.bus_from,
                &line.bus_to,
                &line.terminal_map_from,
                &line.terminal_map_to,
            )?;
            neighbors
                .entry(&line.bus_from)
                .or_default()
                .push(&line.bus_to);
            neighbors
                .entry(&line.bus_to)
                .or_default()
                .push(&line.bus_from);
        }
        if let Some(plan) = &self.ideal_plan {
            for group in &plan.groups {
                if plan.open_aliases.contains(&group.name) {
                    continue;
                }
                neighbors.entry(group.from).or_default().push(group.to);
                neighbors.entry(group.to).or_default().push(group.from);
            }
        }
        let mut queue = VecDeque::new();
        for (&bus, voltages) in &self.source_voltages {
            let nominal = voltages
                .values()
                .next()
                .expect("nonempty qualified source")
                .norm();
            self.nominal.insert(bus, nominal);
            queue.push_back(bus);
        }
        let mut ratios: BTreeMap<&str, Vec<(&str, f64)>> = BTreeMap::new();
        for t in self.net.transformers() {
            if let Some(leader) = self.coil_banks.leaders.get(t.name.as_str()) {
                if *leader != t.name {
                    continue;
                }
                let bank = &self.coil_banks.banks[leader];
                let a = &t.windings[0];
                let b = &t.windings[1];
                ratios
                    .entry(&a.bus)
                    .or_default()
                    .push((&b.bus, bank.secondary / bank.primary));
                ratios
                    .entry(&b.bus)
                    .or_default()
                    .push((&a.bus, bank.primary / bank.secondary));
                continue;
            }
            let a = &t.windings[0];
            let b = &t.windings[1];
            ratios
                .entry(&a.bus)
                .or_default()
                .push((&b.bus, (b.v_ref * b.tap) / (a.v_ref * a.tap)));
            ratios
                .entry(&b.bus)
                .or_default()
                .push((&a.bus, (a.v_ref * a.tap) / (b.v_ref * b.tap)));
        }
        while let Some(bus) = queue.pop_front() {
            let nominal = self.nominal[bus];
            for &(neighbor, ratio) in ratios.get(bus).into_iter().flatten() {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    self.nominal.entry(neighbor)
                {
                    entry.insert(nominal * ratio);
                    queue.push_back(neighbor);
                }
            }
            for &neighbor in neighbors.get(bus).into_iter().flatten() {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    self.nominal.entry(neighbor)
                {
                    entry.insert(nominal);
                    queue.push_back(neighbor);
                }
            }
        }
        if self.nominal.len() + self.services.core_buses.len() != self.net.buses().len() {
            return Err(unsupported(
                "a GLM bus island has no fixed source from which to initialize voltage",
            ));
        }
        self.diagnostics.push(&codes::EMIT_GLM_VALUE_REBASED,
            "bus nominal voltages are initialized from connected source magnitudes; P/I/Z coefficients retain their original voltage law");
        Ok(())
    }

    fn buses(&mut self) -> Result<(), Error> {
        for bus in self.net.buses() {
            if self.services.core_buses.contains(bus.id.as_str()) {
                continue;
            }
            if let Some(&phase) = self.services.buses.get(bus.id.as_str()) {
                self.service_bus(bus, phase)?;
                continue;
            }
            let phase_flags = flags(&self.phases[bus.id.as_str()]);
            let _ = writeln!(
                self.out,
                "object node {{ name \"{}\"; phases {phase_flags}N; nominal_voltage {}; maximum_voltage_error 1e-9;",
                bus.id,
                self.nominal[bus.id.as_str()]
            );
            if let Some(voltages) = self.source_voltages.get(bus.id.as_str()) {
                self.out.push_str("    bustype SWING;\n");
                for (&phase, &voltage) in voltages {
                    let literal = complex_literal(voltage)?;
                    let _ = writeln!(self.out, "    voltage_{} {literal};", phase_name(phase));
                }
            }
            self.coordinates(crate::DistLocationTarget::Bus, &bus.id);
            self.out.push_str("}\n");
        }
        Ok(())
    }

    fn link_phases(
        &self,
        name: &str,
        from: &str,
        to: &str,
        from_map: &[String],
        to_map: &[String],
    ) -> Result<Vec<usize>, Error> {
        if from_map != to_map {
            return Err(unsupported(format!(
                "link {name} permutes endpoint phase identities"
            )));
        }
        let terminals = phases(from_map)?;
        for bus in [from, to] {
            let available = self
                .phases
                .get(bus)
                .ok_or_else(|| unsupported(format!("link {name} has no exact endpoint {bus}")))?;
            if terminals.iter().any(|p| !available.contains(p)) {
                return Err(unsupported(format!(
                    "link {name} uses an absent endpoint phase"
                )));
            }
        }
        Ok(terminals)
    }

    fn link_endpoints<'b>(&self, name: &str, from: &'b str, to: &'b str) -> (&'b str, &'b str) {
        if self
            .ideal_plan
            .as_ref()
            .is_some_and(|p| p.reversed.contains(name))
        {
            (to, from)
        } else {
            (from, to)
        }
    }

    fn switches(&mut self) -> Result<(), Error> {
        if let Some(plan) = self.ideal_plan.take() {
            self.ideal_contacts(&plan);
            self.ideal_plan = Some(plan);
            return Ok(());
        }
        for switch in self.net.switches() {
            if !switch.extras.is_empty() {
                return Err(unsupported(format!(
                    "switch {} has uninterpreted electrical extras",
                    switch.name
                )));
            }
            let phase_order = self.link_phases(
                &switch.name,
                &switch.bus_from,
                &switch.bus_to,
                &switch.terminal_map_from,
                &switch.terminal_map_to,
            )?;
            let phase_flags: String = phase_order
                .iter()
                .map(|&p| super::read::phase_name(p))
                .collect();
            let _ = writeln!(
                self.out,
                "object switch {{\n    name {};\n    phases {phase_flags}N;\n    from {};\n    to {};\n    operating_mode INDIVIDUAL;\n    status OPEN;",
                switch.name, switch.bus_from, switch.bus_to
            );
            for phase in phase_flags.chars() {
                let _ = writeln!(self.out, "    phase_{phase}_state OPEN;");
            }
            self.coordinates(crate::DistLocationTarget::Switch, &switch.name);
            self.out.push_str("}\n\n");
            if switch.i_max.is_some() {
                self.dropped(format!(
                    "switch {}: contact current ratings are not emitted",
                    switch.name
                ));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // A line and its coefficient/endpoint expansions stay together.
    fn lines(&mut self) -> Result<(), Error> {
        let codes: BTreeMap<_, _> = self
            .net
            .line_codes()
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();
        let mut used_codes = BTreeSet::new();
        for line in self.net.lines() {
            if self.services.buses.contains_key(line.bus_from.as_str()) {
                let code = codes
                    .get(line.linecode.as_str())
                    .ok_or_else(|| unsupported("service line has no linecode"))?;
                used_codes.insert(code.name.as_str());
                self.service_line(line, code)?;
                continue;
            }
            let phase_order = self.link_phases(
                &line.name,
                &line.bus_from,
                &line.bus_to,
                &line.terminal_map_from,
                &line.terminal_map_to,
            )?;
            let code = codes.get(line.linecode.as_str()).ok_or_else(|| {
                unsupported(format!("line {} has no exact linecode identity", line.name))
            })?;
            used_codes.insert(code.name.as_str());
            if !line.extras.is_empty() || !code.extras.is_empty() {
                return Err(unsupported(format!(
                    "line {} has uninterpreted electrical extras",
                    line.name
                )));
            }
            if line.route.is_some()
                || line.i_max.is_some()
                || line.s_max.is_some()
                || code.i_max.is_some()
                || code.s_max.is_some()
                || code.source.is_some()
            {
                self.dropped(format!(
                    "line {}: route, ratings and linecode provenance are not emitted",
                    line.name
                ));
            }
            validate_line_matrices(line, code, phase_order.len())?;
            let expanded_shunts = Self::expand_line_shunts(code);
            let config = self.generated_name(&format!("{}__glm_configuration", line.name));
            let _ = writeln!(self.out, "object line_configuration {{ name \"{config}\";");
            self.coordinates(crate::DistLocationTarget::LineCode, &code.name);
            for (row, &a) in phase_order.iter().enumerate() {
                for (col, &b) in phase_order.iter().enumerate() {
                    let z = Complex64::new(code.r_series[row][col], code.x_series[row][col])
                        * METERS_PER_MILE;
                    let literal = complex_literal(z)?;
                    let capacitance = if expanded_shunts {
                        0.0
                    } else {
                        code.b_from[row][col]
                    } * METERS_PER_MILE
                        / (std::f64::consts::PI * self.net.base_frequency() * 1e-9);
                    if !capacitance.is_finite() {
                        return Err(unsupported("line capacitance conversion overflows"));
                    }
                    let _ = writeln!(
                        self.out,
                        "    z{}{} {literal}; c{}{} {capacitance};",
                        a + 1,
                        b + 1,
                        a + 1,
                        b + 1
                    );
                }
            }
            self.out.push_str("}\n");
            let (from, to) = self.link_endpoints(&line.name, &line.bus_from, &line.bus_to);
            let _ = writeln!(
                self.out,
                "object overhead_line {{ name \"{}\"; phases {}N; from \"{}\"; to \"{}\"; length {} m; configuration \"{config}\";",
                line.name,
                flags(&phase_order),
                from,
                to,
                line.length
            );
            self.coordinates(crate::DistLocationTarget::Line, &line.name);
            self.out.push_str("}\n");
            if expanded_shunts {
                self.line_shunts(line, code, &phase_order)?;
            }
            self.diagnostics.push(
                &codes::EMIT_GLM_VALUE_REBASED,
                format!(
                    "line {}: linecode {} is materialized as phase-indexed configuration {config}",
                    line.name, code.name
                ),
            );
        }
        for code in self.net.line_codes() {
            if !used_codes.contains(code.name.as_str()) {
                self.dropped(format!("unused linecode {} is not emitted", code.name));
            }
        }
        Ok(())
    }

    fn loads(&mut self) -> Result<(), Error> {
        for load in self.net.loads() {
            if self.services.core_loads.contains(load.name.as_str()) {
                continue;
            }
            if self.services.buses.contains_key(load.bus.as_str()) {
                self.service_load(load)?;
                continue;
            }
            if load.configuration != Configuration::Wye {
                self.pair_load(load)?;
                continue;
            }
            if load.configuration != Configuration::Wye
                || load.terminal_map.last().map(String::as_str) != Some("n")
            {
                return Err(unsupported(format!(
                    "load {} requires an additional connection model",
                    load.name
                )));
            }
            if !load.extras.is_empty() {
                return Err(unsupported(format!(
                    "load {} has uninterpreted electrical extras",
                    load.name
                )));
            }
            let phase_order = phases(&load.terminal_map[..load.terminal_map.len() - 1])?;
            let available = self.phases.get(load.bus.as_str()).ok_or_else(|| {
                unsupported(format!("load {} has no exact bus identity", load.name))
            })?;
            if phase_order.iter().any(|p| !available.contains(p))
                || load.p_nom.len() != phase_order.len()
                || load.q_nom.len() != phase_order.len()
            {
                return Err(unsupported(format!(
                    "load {} has incomplete terminal/power vectors",
                    load.name
                )));
            }
            let (law, v_nom) = match &load.voltage_model {
                DistLoadVoltageModel::ConstantPower { v_nom } => ("power", v_nom),
                DistLoadVoltageModel::ConstantCurrent { v_nom } => ("current", v_nom),
                DistLoadVoltageModel::ConstantImpedance { v_nom } => ("impedance", v_nom),
                _ => {
                    return Err(unsupported(format!(
                        "load {} requires another voltage-law mapping",
                        load.name
                    )));
                }
            };
            if law != "power"
                && (v_nom.len() != phase_order.len()
                    || v_nom.iter().any(|v| !v.is_finite() || *v <= 0.0))
            {
                return Err(unsupported(format!(
                    "load {} needs positive finite per-phase nominal voltages",
                    load.name
                )));
            }
            let _ = writeln!(
                self.out,
                "object load {{ name \"{}\"; parent \"{}\"; phases {}N; nominal_voltage {};",
                load.name,
                load.bus,
                flags(&phase_order),
                self.nominal[load.bus.as_str()]
            );
            self.coordinates(crate::DistLocationTarget::Load, &load.name);
            for (i, &phase) in phase_order.iter().enumerate() {
                let power = Complex64::new(load.p_nom[i], load.q_nom[i]);
                let value = match law {
                    "power" => power,
                    "current" => nominal_phasor(1.0, phase) * power.conj() / v_nom[i],
                    _ if power == Complex64::default() => Complex64::default(),
                    _ => v_nom[i] * v_nom[i] / power.conj(),
                };
                if power != Complex64::default() && value == Complex64::default() {
                    return Err(unsupported(
                        "load coefficient conversion underflows to zero",
                    ));
                }
                let literal = complex_literal(value)?;
                let _ = writeln!(
                    self.out,
                    "    constant_{law}_{} {literal};",
                    phase_name(phase)
                );
            }
            self.out.push_str("}\n");
        }
        Ok(())
    }

    fn reserve(&mut self, name: &str) -> Result<(), Error> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(unsupported(format!(
                "object name {name:?} needs qualified GLM escaping"
            )));
        }
        if !self.identities.insert(name.to_ascii_lowercase()) {
            return Err(unsupported(format!(
                "object name {name:?} collides in the GLM namespace"
            )));
        }
        Ok(())
    }

    fn generated_name(&mut self, base: &str) -> String {
        if self.identities.insert(base.to_ascii_lowercase()) {
            return base.to_owned();
        }
        for i in 1usize.. {
            let name = format!("{base}_{i}");
            if self.identities.insert(name.to_ascii_lowercase()) {
                return name;
            }
        }
        unreachable!("bounded network cannot exhaust usize names")
    }

    fn dropped(&mut self, message: impl Into<String>) {
        self.diagnostics
            .push(&codes::EMIT_GLM_FIELD_DROPPED, message);
    }
}

fn validate_line_matrices(line: &DistLine, code: &DistLineCode, n: usize) -> Result<(), Error> {
    if code.n_conductors != n
        || !line.length.is_finite()
        || line.length <= 0.0
        || !(line.length / 0.3048).is_finite()
    {
        return Err(unsupported(format!(
            "line {} has invalid conductor count or length",
            line.name
        )));
    }
    for matrix in [
        &code.r_series,
        &code.x_series,
        &code.g_from,
        &code.b_from,
        &code.g_to,
        &code.b_to,
    ] {
        if matrix.len() != n
            || matrix
                .iter()
                .any(|r| r.len() != n || r.iter().any(|v| !v.is_finite()))
        {
            return Err(unsupported(format!(
                "line {} has an invalid conductor matrix",
                line.name
            )));
        }
    }
    if (0..n).all(|i| code.r_series[i][i] == 0.0 && code.x_series[i][i] == 0.0) {
        return Err(unsupported(format!(
            "line {} has no nonzero diagonal to select native explicit-matrix mode",
            line.name
        )));
    }
    Ok(())
}

fn phases(terminals: &[String]) -> Result<Vec<usize>, Error> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for terminal in terminals {
        let phase = match terminal.as_str() {
            "1" => 0,
            "2" => 1,
            "3" => 2,
            _ => {
                return Err(unsupported(format!(
                    "terminal {terminal:?} needs additional phase/neutral interpretation"
                )));
            }
        };
        if !seen.insert(phase) {
            return Err(unsupported("repeated phase terminal"));
        }
        result.push(phase);
    }
    if result.is_empty() {
        return Err(unsupported("empty phase set"));
    }
    Ok(result)
}

fn flags(phases: &[usize]) -> String {
    // Native sets have fixed A/B/C order; matrices and properties are emitted
    // with explicit phase indices, independently of canonical terminal order.
    let set: BTreeSet<_> = phases.iter().copied().collect();
    set.into_iter().map(phase_name).collect()
}

fn complex_literal(value: Complex64) -> Result<String, Error> {
    if !value.re.is_finite() || !value.im.is_finite() {
        return Err(unsupported(
            "GLM electrical literal is nonfinite or its unit conversion overflowed",
        ));
    }
    Ok(format!("{}{:+}j", value.re, value.im))
}

fn has_bus_bounds(bus: &DistBus) -> bool {
    bus.v_min.is_some()
        || bus.v_max.is_some()
        || bus.v_min_phase.is_some()
        || bus.v_max_phase.is_some()
        || bus.vpn_min.is_some()
        || bus.vpn_max.is_some()
        || bus.vpp_min.is_some()
        || bus.vpp_max.is_some()
        || bus.vpos_min.is_some()
        || bus.vpos_max.is_some()
        || bus.vneg_max.is_some()
        || bus.vzero_max.is_some()
        || bus.vn_max.is_some()
}
