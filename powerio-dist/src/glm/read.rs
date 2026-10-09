use std::collections::{BTreeMap, BTreeSet};

use num_complex::Complex64;
use powerio_core::{Diagnostic, Error, FormatId, PioModule, Source};

use super::{
    GlmObject, GlmProject, GlmProperty, GlmPropertyValue, GlmQuantityKind, ObjectIndex, ObjectKey,
    PreprocessOptions, Statement, TokenKind, parse_project,
};
use crate::diagnostics::codes;
use crate::{
    Configuration, DistBus, DistLine, DistLineCode, DistLoad, DistLoadVoltageModel,
    MulticonductorNetwork, VoltageSource,
};

const METERS_PER_MILE: f64 = 1609.344;

mod cables;
mod capacitors;
mod delta_grounded;
mod geometry;
mod loads;
mod regulators;
mod sources;
mod split_phase;
mod switches;
mod templates;
mod transformers;
mod triplex_lines;

/// Read the initial static GLM electrical profile. Unknown physical
/// classes, properties and globals fail explicitly; they are not silently
/// omitted from an apparently usable network. The source project is retained.
///
/// This initial profile covers ordinary phase-to-earth node/load/meter objects,
/// fixed source phasors, P/I/Z loads, overhead and underground line matrices
/// or conductor geometry, and fixed ordinary-phase switch contacts. Finite
/// contacts use exact line equivalents; ideal and open contacts remain typed switches.
/// Ordinary single-phase and three-phase grounded-wye transformers use winding
/// records; FBS additionally preserves its finite default winding-2 core shunt.
/// A connected directed FBS tree may use its implicit upstream voltage source.
/// Wye regulators preserve fixed phase taps, NR resistance and FBS ideal
/// ratios in winding equivalents.
/// Wye capacitors use fixed per-phase shunt admittances. Active controls require
/// explicit individual switch states and produce a frozen-control diagnostic.
/// Delta–grounded-wye transformers use phase-pair primary coils with grounded
/// secondaries, preserving native step-up/down polarity in both solvers.
/// Center-tapped services with grounded neutrals use typed windings and an
/// internal core impedance load; triplex P/I/Z loads retain leg polarity.
/// Triplex geometry uses full conductor matrices with both endpoint neutrals
/// grounded, preserving native hot-conductor and neutral-current behavior.
/// Direct triplex matrices and canonical GLM service emission remain unfinished.
/// Other conductor/device profiles remain under implementation.
pub fn parse(
    source: Source,
    options: PreprocessOptions,
) -> Result<PioModule<MulticonductorNetwork>, Error> {
    let preprocessing =
        serde_json::json!({"defines": options.defines, "reinclude": options.reinclude});
    let source_format = match options.dialect {
        super::GlmDialect::Pnnl5_3 => crate::DistTargetFormat::Gridlabd,
        super::GlmDialect::Arras4_3 => crate::DistTargetFormat::GridlabdArras,
    };
    let project = parse_project(source, options)?;
    let index = project.index_objects()?;
    let mut reader = Reader {
        project: &project,
        index,
        net: MulticonductorNetwork::new(),
        diagnostics: Vec::new(),
        roots: BTreeMap::new(),
        names: BTreeMap::new(),
        identities: BTreeSet::new(),
        quantities: BTreeMap::new(),
        phases: BTreeMap::new(),
        nominal: BTreeMap::new(),
        triplex: BTreeMap::new(),
        attachments: BTreeMap::new(),
        line_capacitance: false,
        newton_raphson: false,
        switch_default_resistance: 1e-4,
    };
    // Profile identity belongs to the module's source descriptors and context.
    // Extending the legacy network source-format enum would change a frozen
    // IR 2 record solely to carry non-electrical provenance.
    reader.top_level()?;
    reader.inventory()?;
    reader.buses()?;
    reader.loads_and_sources()?;
    reader.lines()?;
    reader.triplex_lines()?;
    reader.switches()?;
    reader.transformers()?;
    reader.regulators()?;
    reader.capacitors()?;
    reader.coordinates()?;
    crate::require_electrical_readiness(&reader.net)
        .map_err(|e| e.with_source(project.source().clone()))?;
    let mut module = PioModule::parsed(
        reader.net,
        project
            .source()
            .clone()
            .with_format(FormatId::new(source_format.name())?),
        reader.diagnostics,
    )?;
    module.insert_extension("org.powerio.gridlabd", serde_json::json!({
        "version": 1,
        "dialect": source_format.name(),
        "dependencies": project.dependencies().iter().map(powerio_core::SourceId::as_str).collect::<Vec<_>>(),
        "absolute_include": project.has_absolute_include(),
        "preprocessing": preprocessing,
    }))?;
    Ok(module)
}

struct Reader<'a> {
    project: &'a GlmProject,
    index: ObjectIndex<'a>,
    net: MulticonductorNetwork,
    diagnostics: Vec<Diagnostic>,
    roots: BTreeMap<ObjectKey, ObjectKey>,
    names: BTreeMap<ObjectKey, String>,
    identities: BTreeSet<String>,
    quantities: BTreeMap<(ObjectKey, String), Complex64>,
    phases: BTreeMap<ObjectKey, Vec<usize>>,
    nominal: BTreeMap<ObjectKey, f64>,
    triplex: BTreeMap<ObjectKey, usize>,
    attachments: BTreeMap<ObjectKey, Vec<(crate::DistLocationTarget, String)>>,
    line_capacitance: bool,
    newton_raphson: bool,
    switch_default_resistance: f64,
}

impl Reader<'_> {
    fn error(&self, key: ObjectKey, message: impl Into<String>) -> Error {
        self.project
            .electrical_error(self.index.objects()[key.ordinal()].span(), message)
    }

    fn quantity(&self, key: ObjectKey, name: &str) -> Option<Complex64> {
        self.quantities.get(&(key, name.to_owned())).copied()
    }

    fn real(&self, key: ObjectKey, name: &str) -> Option<f64> {
        self.quantity(key, name).map(|v| v.re)
    }

    fn text(&self, key: ObjectKey, name: &str) -> Result<Option<String>, Error> {
        property(&self.index.objects()[key.ordinal()], name)
            .map(|p| literal(p).map_err(|m| self.error(key, m)))
            .transpose()
    }

    fn top_level(&mut self) -> Result<(), Error> {
        let mut powerflow = false;
        for statement in self.project.statements() {
            match statement {
                Statement::Block { header, body, span }
                    if header.first().is_some_and(|t| t.text() == "module") =>
                {
                    if header.len() != 2 || header[1].text() != "powerflow" {
                        return Err(self
                            .project
                            .electrical_error(span, "unsupported module configuration"));
                    }
                    powerflow = true;
                    for entry in body {
                        let Statement::Simple { tokens, span } = entry else {
                            return Err(self
                                .project
                                .electrical_error(span, "unsupported powerflow module syntax"));
                        };
                        if tokens.len() != 2 {
                            return Err(self
                                .project
                                .electrical_error(span, "powerflow setting requires one value"));
                        }
                        self.powerflow_setting(tokens[0].text(), tokens[1].text(), span)?;
                    }
                }
                Statement::Simple { tokens, span }
                    if tokens.first().is_some_and(|t| t.text() == "module") =>
                {
                    if tokens.len() != 2
                        || !matches!(tokens[1].text(), "powerflow" | "tape" | "assert")
                    {
                        return Err(self.project.electrical_error(
                            span,
                            "module may introduce unmodeled electrical behavior",
                        ));
                    }
                    powerflow |= tokens[1].text() == "powerflow";
                }
                Statement::Block { header, .. }
                    if header
                        .first()
                        .is_some_and(|t| matches!(t.text(), "object" | "clock")) => {}
                Statement::Directive(token) => self.global_directive(token)?,
                Statement::Simple { span, .. } | Statement::Block { span, .. } => {
                    return Err(self
                        .project
                        .electrical_error(span, "unsupported top-level electrical declaration"));
                }
            }
        }
        if !powerflow {
            return Err(Error::new(
                &codes::READ_GLM_ELECTRICAL_UNSUPPORTED,
                "GLM static network requires the powerflow module",
            )
            .with_source(self.project.source().clone()));
        }
        Ok(())
    }

    fn global_directive(&mut self, token: &super::Token) -> Result<(), Error> {
        let text = token.text().trim();
        let setting = text.strip_prefix("#set ").and_then(|s| s.split_once('='));
        if let Some(("iteration_limit", value)) =
            setting.map(|(k, v)| (k.trim(), v.trim().trim_end_matches(';')))
        {
            if value.parse::<u32>().ok().is_none_or(|v| v == 0) {
                return Err(self.project.electrical_error(
                    &token.span(),
                    "iteration_limit requires a positive integer",
                ));
            }
            self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_SOURCE_ONLY,
                "iteration_limit is retained in source; canonical output uses its own solver settings"));
            return Ok(());
        }
        if let Some((name @ ("profiler" | "relax_naming_rules"), value)) =
            setting.map(|(k, v)| (k.trim(), v.trim().trim_end_matches(';')))
        {
            if !matches!(value, "0" | "1" | "TRUE" | "FALSE" | "true" | "false") {
                return Err(self
                    .project
                    .electrical_error(&token.span(), format!("{name} requires a boolean")));
            }
            self.diagnostics.push(Diagnostic::of(
                &codes::READ_GLM_SOURCE_ONLY,
                format!("{name} is a non-electrical engine setting retained in source; object identities remain literal"),
            ));
            return Ok(());
        }
        if !setting.is_some_and(|(name, _)| {
            matches!(
                name.trim(),
                "complex_format"
                    | "complex_output_format"
                    | "double_format"
                    | "savefile"
                    | "reinclude"
                    | "literal_if"
                    | "relax_undefined_if"
            )
        }) {
            return Err(self.project.electrical_error(
                &token.span(),
                "unqualified global directive in static GLM input",
            ));
        }
        Ok(())
    }

    fn powerflow_setting(
        &mut self,
        name: &str,
        value: &str,
        span: &powerio_core::SourceSpan,
    ) -> Result<(), Error> {
        match (name, value) {
            ("solver_method", "NR") => self.newton_raphson = true,
            ("solver_method", "FBS") => self.newton_raphson = false,
            ("line_limits", value)
                if matches!(
                    value.to_ascii_lowercase().as_str(),
                    "true" | "false" | "0" | "1"
                ) =>
            {
                self.diagnostics.push(Diagnostic::of(
                    &codes::READ_GLM_SOURCE_ONLY,
                    "line_limits controls native limit reporting; the setting remains in source",
                ));
            }
            ("default_maximum_voltage_error" | "maximum_voltage_error", value) => {
                if !value.parse::<f64>().is_ok_and(|v| v.is_finite() && v > 0.0) {
                    return Err(self.project.electrical_error(
                        span,
                        "default voltage convergence tolerance requires a finite positive scalar",
                    ));
                }
                self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_SOURCE_ONLY,
                    "native voltage convergence tolerance remains in source; canonical output uses its own solver settings"));
            }
            ("default_resistance", value) => {
                self.switch_default_resistance = value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .ok_or_else(|| {
                        self.project.electrical_error(
                            span,
                            "default_resistance requires a finite positive scalar in ohms",
                        )
                    })?;
            }
            ("nominal_frequency", value) => {
                *self.net.base_frequency_mut() = value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| [50.0, 60.0].contains(v))
                    .ok_or_else(|| {
                        self.project.electrical_error(
                            span,
                            "nominal_frequency must be 50 or 60 Hz in this profile",
                        )
                    })?;
            }
            ("line_capacitance", v) if v.eq_ignore_ascii_case("true") => {
                self.line_capacitance = true;
            }
            ("line_capacitance", v) if v.eq_ignore_ascii_case("false") => {
                self.line_capacitance = false;
            }
            _ => {
                return Err(self
                    .project
                    .electrical_error(span, "unqualified powerflow global setting"));
            }
        }
        Ok(())
    }

    fn reserve_explicit_names(&mut self) -> Result<(), Error> {
        let mut components = BTreeSet::new();
        for object in self.index.objects() {
            if let Some(name) = object.name() {
                let folded = name.to_ascii_lowercase();
                // Passive templates resolve by native case-sensitive identity.
                // Their names never become typed component/linecode identities.
                // Still reserve them against generated names for canonical output.
                self.identities.insert(folded.clone());
                if !templates::is_template(class(object)) && !components.insert(folded) {
                    return Err(self.error(
                        object.key(),
                        "object identities collide under current canonical case-insensitive lookup",
                    ));
                }
            }
        }
        Ok(())
    }

    fn inventory(&mut self) -> Result<(), Error> {
        let used_templates = self.used_templates()?;
        self.reserve_explicit_names()?;
        for object in self.index.objects() {
            let name = object.name().map_or_else(
                || {
                    allocate_name(
                        &mut self.identities,
                        &format!("__glm_object_{}", object.key().ordinal()),
                    )
                },
                str::to_owned,
            );
            self.names.insert(object.key(), name);
            let class = class(object);
            if templates::is_template(class) && !used_templates.contains(&object.key()) {
                self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_SOURCE_ONLY,
                    format!("unused {class} {} is retained in source; it contributes no network equipment", self.names[&object.key()])));
                continue;
            }
            if matches!(
                class,
                "recorder"
                    | "multi_recorder"
                    | "impedance_dump"
                    | "voltdump"
                    | "currdump"
                    | "complex_assert"
                    | "assert"
            ) {
                self.diagnostics.push(Diagnostic::of(
                    &codes::READ_GLM_SOURCE_ONLY,
                    format!(
                        "{class} {} remains in source; no recording/assertion behavior is executed",
                        self.names[&object.key()]
                    ),
                ));
                continue;
            }
            if !supported_class(class) {
                return Err(self.error(
                    object.key(),
                    format!("class {class} has no verified static mapping yet"),
                ));
            }
            if !object.other_statements().is_empty() {
                return Err(self.error(object.key(), "uninterpreted object-body syntax"));
            }
            for property in object.properties() {
                let name = property.name;
                if !allowed_property(class, name) {
                    return Err(self.project.electrical_error(
                        property.span,
                        format!("unmapped property {class}.{name}"),
                    ));
                }
                if let Some((kind, unit)) = quantity_spec(class, name, self.project.dialect()) {
                    // Native setters run in source order, even if a later
                    // assignment overwrites the value or its phase is absent.
                    let result = self.project.quantity(property, kind, unit)?;
                    self.diagnostics.extend(result.diagnostics);
                    Self::assign_quantity(
                        &mut self.quantities,
                        object.key(),
                        class,
                        name,
                        result.value,
                    );
                }
            }
            if let Some(diagnostic) = self.rating_metadata(object.key())? {
                self.diagnostics.push(diagnostic);
            }
        }
        let keys: Vec<_> = self
            .index
            .objects()
            .iter()
            .filter(|o| bus_class(class(o)))
            .map(GlmObject::key)
            .collect();
        for key in keys {
            let mut root = key;
            let mut path = Vec::new();
            loop {
                if let Some(&known) = self.roots.get(&root) {
                    root = known;
                    break;
                }
                path.push(root);
                let Some(parent) = self.index.parent(root) else {
                    break;
                };
                if !bus_class(class(&self.index.objects()[parent.ordinal()])) {
                    return Err(self.error(key, "electrical parent must be a node, meter or load"));
                }
                root = parent;
            }
            for descendant in path {
                self.roots.insert(descendant, root);
            }
        }
        self.validate_parent_profile()
    }

    fn validate_parent_profile(&self) -> Result<(), Error> {
        if self.newton_raphson && self.project.dialect() == super::GlmDialect::Arras4_3 {
            for (&key, &root) in &self.roots {
                if key != root && self.index.parent(key) != Some(root) {
                    return Err(self.error(key, format!(
                        "Arras 4.3 NR nested electrical parents are not qualified: {} has a parent below root {}; the pinned engine can omit descendant loads or fail; flattening would change source behavior",
                        self.names[&key], self.names[&root]
                    )));
                }
            }
        }
        Ok(())
    }

    fn buses(&mut self) -> Result<(), Error> {
        let roots: BTreeSet<_> = self.roots.values().copied().collect();
        for key in roots {
            let flags = self
                .text(key, "phases")?
                .ok_or_else(|| self.error(key, "explicit phases are required"))?;
            let phases = if split_phase::bus_class(class(&self.index.objects()[key.ordinal()])) {
                let anchor = split_phase::anchor(&flags).map_err(|m| self.error(key, m))?;
                self.triplex.insert(key, anchor);
                vec![0, 1]
            } else {
                parse_bus_phases(&flags).map_err(|m| self.error(key, m))?
            };
            let nominal = self
                .real(key, "nominal_voltage")
                .ok_or_else(|| self.error(key, "nominal_voltage is required"))?;
            if nominal <= 0.0 {
                return Err(self.error(key, "nominal_voltage must be positive"));
            }
            let mut terminals: Vec<_> = phases.iter().map(|p| (p + 1).to_string()).collect();
            terminals.push("n".into());
            let mut bus = DistBus::new(&self.names[&key], terminals);
            bus.grounded.push("n".into());
            self.net.buses_mut().push(bus);
            self.attachments
                .entry(key)
                .or_default()
                .push((crate::DistLocationTarget::Bus, self.names[&key].clone()));
            self.phases.insert(key, phases);
            self.nominal.insert(key, nominal);
        }
        Ok(())
    }

    fn assign_quantity(
        quantities: &mut BTreeMap<(ObjectKey, String), Complex64>,
        key: ObjectKey,
        class: &str,
        name: &str,
        value: Complex64,
    ) {
        if let Some((canonical, imaginary, default)) = (class == "transformer_configuration")
            .then(|| transformers::alias(name))
            .flatten()
        {
            let previous = quantities.entry((key, canonical.into())).or_insert(default);
            if imaginary {
                previous.im = value.re;
            } else {
                previous.re = value.re;
            }
        } else if let Some((canonical, Some(imaginary), _, _)) = if class == "load" {
            loads::property(name)
        } else if split_phase::bus_class(class) {
            split_phase::property(class, name)
        } else {
            None
        } {
            let previous = quantities.entry((key, canonical.to_owned())).or_default();
            if imaginary {
                previous.im = value.re;
            } else {
                previous.re = value.re;
            }
        } else {
            quantities.insert((key, name.to_owned()), value);
        }
    }

    fn loads_and_sources(&mut self) -> Result<(), Error> {
        let implicit_source = self.implicit_fbs_source()?;
        let entries: Vec<_> = self.roots.iter().map(|(&k, &r)| (k, r)).collect();
        for (key, root) in entries {
            if self.triplex.contains_key(&root) {
                if implicit_source == Some(key) {
                    return Err(self.error(key, "implicit triplex voltage source is not qualified"));
                }
                self.triplex_loads(key, root)?;
                continue;
            }
            if split_phase::bus_class(class(&self.index.objects()[key.ordinal()])) {
                return Err(self.error(key, "triplex child requires a triplex parent"));
            }
            let phases = if key == root {
                self.phases[&root].clone()
            } else {
                let phases = parse_bus_phases(
                    self.text(key, "phases")?
                        .as_deref()
                        .ok_or_else(|| self.error(key, "child requires explicit phases"))?,
                )
                .map_err(|m| self.error(key, m))?;
                if phases.iter().any(|p| !self.phases[&root].contains(p)) {
                    return Err(self.error(key, "child phases are outside its parent bus"));
                }
                phases
            };
            let nominal = self.nominal[&root];
            if key != root
                && self
                    .real(key, "nominal_voltage")
                    .is_some_and(|v| v.total_cmp(&nominal).is_ne())
            {
                return Err(self.error(key, "child nominal voltage conflicts with its parent"));
            }
            let bustype = self.text(key, "bustype")?.unwrap_or_else(|| "PQ".into());
            if bustype == "SWING" || (bustype == "PQ" && implicit_source == Some(key)) {
                if key != root {
                    return Err(self.error(key, "parented SWING source is not qualified"));
                }
                let mut magnitudes = Vec::new();
                let mut angles = Vec::new();
                for &phase in &phases {
                    let value = self
                        .quantity(key, &format!("voltage_{}", phase_name(phase)))
                        .filter(|v| *v != Complex64::default())
                        .unwrap_or_else(|| nominal_phasor(nominal, phase));
                    magnitudes.push(value.norm());
                    angles.push(value.arg());
                }
                self.net.sources_mut().push(VoltageSource::new(
                    &self.names[&key],
                    &self.names[&root],
                    phases.iter().map(|p| (p + 1).to_string()).collect(),
                    magnitudes,
                    angles,
                ));
            } else if bustype != "PQ" {
                return Err(self.error(
                    key,
                    format!("bustype {bustype} needs an additional static model"),
                ));
            }
            if class(&self.index.objects()[key.ordinal()]) != "load" {
                continue;
            }
            let delta = self
                .text(key, "phases")?
                .is_some_and(|flags| flags.contains('D'));
            if !delta {
                self.load_terms(key, root, &phases, nominal)?;
            }
            self.connected_load_terms(key, root, &phases, nominal, delta)?;
        }
        Ok(())
    }

    fn load_terms(
        &mut self,
        key: ObjectKey,
        root: ObjectKey,
        phases: &[usize],
        nominal: f64,
    ) -> Result<(), Error> {
        let laws = ["power", "current", "impedance"];
        let law_count = laws
            .iter()
            .filter(|law| {
                phases.iter().any(|&phase| {
                    self.quantity(key, &format!("constant_{law}_{}", phase_name(phase)))
                        .is_some()
                })
            })
            .count()
            + self.explicit_load_term_count(key);
        for law in laws {
            let mut values = Vec::new();
            let mut stated = false;
            for &phase in phases {
                let value = self.quantity(key, &format!("constant_{law}_{}", phase_name(phase)));
                stated |= value.is_some();
                let value = value.unwrap_or_default();
                let power = match law {
                    "power" => value,
                    "current" => nominal_phasor(nominal, phase) * value.conj(),
                    _ if value == Complex64::default() => Complex64::default(),
                    _ => nominal * nominal / value.conj(),
                };
                if !power.re.is_finite() || !power.im.is_finite() {
                    return Err(self.error(key, "load power normalization overflows"));
                }
                if value != Complex64::default() && power == Complex64::default() {
                    return Err(self.error(key, "load power normalization underflows to zero"));
                }
                values.push(power);
            }
            if !stated {
                continue;
            }
            let mut terminals: Vec<_> = phases.iter().map(|p| (p + 1).to_string()).collect();
            terminals.push("n".into());
            let name = if key != root && law_count == 1 {
                self.names[&key].clone()
            } else {
                allocate_name(
                    &mut self.identities,
                    &format!("{}__glm_{law}", self.names[&key]),
                )
            };
            let mut load = DistLoad::new(
                name,
                &self.names[&root],
                terminals,
                Configuration::Wye,
                values.iter().map(|v| v.re).collect(),
                values.iter().map(|v| v.im).collect(),
            );
            let v_nom = vec![nominal; phases.len()];
            load.voltage_model = match law {
                "power" => DistLoadVoltageModel::ConstantPower { v_nom },
                "current" => DistLoadVoltageModel::ConstantCurrent { v_nom },
                _ => DistLoadVoltageModel::ConstantImpedance { v_nom },
            };
            if key != root {
                self.attachments
                    .entry(key)
                    .or_default()
                    .push((crate::DistLocationTarget::Load, load.name.clone()));
            }
            self.net.loads_mut().push(load);
        }
        Ok(())
    }

    fn has_explicit_matrix(&self, config: ObjectKey) -> bool {
        (1..=3).any(|i| {
            self.quantity(config, &format!("z{i}{i}"))
                .is_some_and(|v| v != Complex64::default())
        })
    }

    fn lines(&mut self) -> Result<(), Error> {
        let keys: Vec<_> = self
            .index
            .objects()
            .iter()
            .filter(|o| matches!(class(o), "overhead_line" | "underground_line"))
            .map(GlmObject::key)
            .collect();
        for key in keys {
            let phases = parse_phases(
                self.text(key, "phases")?
                    .as_deref()
                    .ok_or_else(|| self.error(key, "line requires explicit phases"))?,
            )
            .map_err(|m| self.error(key, m))?;
            let reference = |name| {
                self.index
                    .resolve_property(key, name)?
                    .ok_or_else(|| self.error(key, format!("missing {name} reference")))
            };
            let from = reference("from")?;
            let to = reference("to")?;
            let config = reference("configuration")?;
            let from = *self
                .roots
                .get(&from)
                .ok_or_else(|| self.error(key, "line from is not an electrical bus"))?;
            let to = *self
                .roots
                .get(&to)
                .ok_or_else(|| self.error(key, "line to is not an electrical bus"))?;
            if self.triplex.contains_key(&from) || self.triplex.contains_key(&to) {
                return Err(self.error(key, "ordinary line cannot connect triplex buses"));
            }
            if phases
                .iter()
                .any(|p| !self.phases[&from].contains(p) || !self.phases[&to].contains(p))
            {
                return Err(self.error(key, "line phases are absent at an endpoint"));
            }
            if class(&self.index.objects()[config.ordinal()]) != "line_configuration" {
                return Err(self.error(key, "line configuration has the wrong class"));
            }
            let length = self
                .real(key, "length")
                .ok_or_else(|| self.error(key, "line length is required"))?
                * 0.3048;
            if length <= 0.0 || !length.is_finite() {
                return Err(self.error(key, "line length must be finite and positive"));
            }
            let code_name = allocate_name(
                &mut self.identities,
                &format!("{}__glm_linecode", self.names[&key]),
            );
            let code = if self.has_explicit_matrix(config) {
                self.line_code(config, &phases, &code_name)?
            } else {
                let neutral = self
                    .text(key, "phases")?
                    .is_some_and(|flags| flags.contains('N'));
                if class(&self.index.objects()[key.ordinal()]) == "underground_line" {
                    self.cable_line_code(config, &phases, neutral, &code_name)?
                } else {
                    self.geometry_line_code(config, &phases, neutral, &code_name)?
                }
            };
            if !super::line_physics::line_shunts_commute(&code) {
                self.diagnostics.push(Diagnostic::of(&codes::READ_GLM_NATIVE_LINE_DISCREPANCY,
                    format!("line {}: this source has noncommuting impedance/capacitance matrices; the typed network preserves the physical pi-line law, while pinned PNNL/Arras native reported line currents and losses differ in NR and FBS; canonical output uses exact end-shunt equivalents", self.names[&key])));
            }
            self.net.line_codes_mut().push(code);
            self.attach(
                config,
                crate::DistLocationTarget::LineCode,
                code_name.clone(),
            );
            self.attach(
                key,
                crate::DistLocationTarget::Line,
                self.names[&key].clone(),
            );
            let terminals = phases
                .iter()
                .map(|p| (p + 1).to_string())
                .collect::<Vec<_>>();
            self.net.lines_mut().push(DistLine::new(
                &self.names[&key],
                &self.names[&from],
                &self.names[&to],
                terminals.clone(),
                terminals,
                code_name,
                length,
            ));
        }
        Ok(())
    }

    fn line_code(
        &self,
        config: ObjectKey,
        phases: &[usize],
        name: &str,
    ) -> Result<DistLineCode, Error> {
        let n = phases.len();
        let mut resistance = vec![vec![0.0; n]; n];
        let mut reactance = resistance.clone();
        let mut susceptance = resistance.clone();
        for (row, &a) in phases.iter().enumerate() {
            for (col, &c) in phases.iter().enumerate() {
                let z = self
                    .quantity(config, &format!("z{}{}", a + 1, c + 1))
                    .unwrap_or_default()
                    / METERS_PER_MILE;
                let capacitance = self
                    .real(config, &format!("c{}{}", a + 1, c + 1))
                    .unwrap_or_default();
                if capacitance != 0.0 && !self.line_capacitance {
                    return Err(self.error(config, "stated line capacitance with the native switch disabled needs explicit ignored-capacitance handling"));
                }
                resistance[row][col] = z.re;
                reactance[row][col] = z.im;
                susceptance[row][col] =
                    std::f64::consts::PI * self.net.base_frequency() * capacitance * 1e-9
                        / METERS_PER_MILE;
            }
        }
        let mut code = DistLineCode::new(name, resistance, reactance);
        code.b_from.clone_from(&susceptance);
        code.b_to = susceptance;
        Ok(code)
    }

    fn coordinates(&mut self) -> Result<(), Error> {
        if !self.index.objects().iter().any(|o| {
            o.properties()
                .iter()
                .any(|p| matches!(p.name, "latitude" | "longitude"))
        }) {
            return Ok(());
        }
        let points = self.index.declared_coordinates()?;
        for object in self.index.objects() {
            let point = points[object.key().ordinal()];
            if point.latitude.is_none() && point.longitude.is_none() {
                continue;
            }
            let Some(targets) = self.attachments.get(&object.key()) else {
                return Err(self.error(
                    object.key(),
                    "coordinate-bearing object has no typed component attachment yet",
                ));
            };
            for (target, name) in targets {
                let mut location = crate::DistComponentLocation::new(
                    *target,
                    name,
                    crate::DistGeographicPoint::new(point.latitude, point.longitude),
                );
                location.kind = Some(crate::DistCoordsKind::Source);
                self.net.component_locations_mut().push(location);
            }
        }
        self.net.validate_component_locations()
    }

    fn attach(&mut self, key: ObjectKey, target: crate::DistLocationTarget, name: String) {
        self.attachments
            .entry(key)
            .or_default()
            .push((target, name));
    }
}

// Reserve explicit source identities first so generated components can never
// shadow a source name (including an object declared later in the document).
fn allocate_name(identities: &mut BTreeSet<String>, base: &str) -> String {
    if identities.insert(base.to_ascii_lowercase()) {
        return base.to_owned();
    }
    for suffix in 1usize.. {
        let name = format!("{base}_{suffix}");
        if identities.insert(name.to_ascii_lowercase()) {
            return name;
        }
    }
    unreachable!("bounded object inventory cannot exhaust usize names")
}

fn property<'a>(object: &'a GlmObject<'_>, name: &str) -> Option<&'a GlmProperty<'a>> {
    object.properties().iter().rev().find(|p| p.name == name)
}

fn literal(property: &GlmProperty<'_>) -> Result<String, &'static str> {
    let GlmPropertyValue::Tokens(tokens) = &property.value else {
        return Err("expected a literal property value");
    };
    let [token] = *tokens else {
        return Err("expected one literal token");
    };
    if token.kind() == TokenKind::Quoted {
        token
            .text()
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .map(str::to_owned)
            .ok_or("expected a double-quoted literal")
    } else {
        Ok(token.text().to_owned())
    }
}

fn class<'a>(object: &GlmObject<'a>) -> &'a str {
    object
        .class_name()
        .strip_prefix("powerflow.")
        .unwrap_or(object.class_name())
}
fn supported_class(name: &str) -> bool {
    bus_class(name)
        || matches!(
            name,
            "overhead_line"
                | "underground_line"
                | "underground_line_conductor"
                | "triplex_line"
                | "triplex_line_configuration"
                | "triplex_line_conductor"
                | "line_configuration"
                | "switch"
                | "overhead_line_conductor"
                | "line_spacing"
                | "transformer"
                | "transformer_configuration"
                | "regulator"
                | "regulator_configuration"
        )
}
fn bus_class(name: &str) -> bool {
    matches!(name, "node" | "load" | "meter" | "capacitor") || split_phase::bus_class(name)
}
fn matrix_property(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 3
        && matches!(bytes[0], b'z' | b'c')
        && (b'1'..=b'3').contains(&bytes[1])
        && (b'1'..=b'3').contains(&bytes[2])
}
fn allowed_property(class: &str, name: &str) -> bool {
    matches!(
        name,
        "name" | "parent" | "groupid" | "latitude" | "longitude"
    ) || (bus_class(class)
        && matches!(
            name,
            "phases"
                | "bustype"
                | "nominal_voltage"
                | "maximum_voltage_error"
                | "voltage_A"
                | "voltage_B"
                | "voltage_C"
        ))
        || (class == "load" && loads::property(name).is_some())
        || (matches!(class, "overhead_line" | "underground_line")
            && matches!(name, "phases" | "from" | "to" | "length" | "configuration"))
        || (class == "line_configuration" && matrix_property(name))
        || split_phase::allowed(class, name)
        || triplex_lines::allowed(class, name)
        || geometry::rating_property(class, name)
        || capacitors::allowed(class, name)
        || regulators::allowed(class, name)
        || transformers::allowed(class, name)
        || geometry::allowed(class, name)
        || cables::unit(class, name).is_some()
        || (class == "switch"
            && matches!(
                name,
                "phases"
                    | "from"
                    | "to"
                    | "status"
                    | "operating_mode"
                    | "phase_A_state"
                    | "phase_B_state"
                    | "phase_C_state"
                    | "switch_impedance"
                    | "switch_resistance"
                    | "switch_reactance"
            ))
}
fn quantity_spec(
    class: &str,
    name: &str,
    dialect: super::GlmDialect,
) -> Option<(GlmQuantityKind, Option<&'static str>)> {
    use GlmQuantityKind::{Complex, Real};
    if let Some(spec) = split_phase::quantity_spec(class, name)
        .or_else(|| triplex_lines::quantity_spec(class, name))
    {
        return Some((spec.0, Some(spec.1)));
    }
    if let Some(spec) = transformers::quantity_spec(class, name)
        .map(|(kind, unit)| (kind, Some(unit)))
        .or_else(|| regulators::quantity_spec(class, name))
    {
        // Arras publishes regulation in percent; PNNL has no unit.
        return Some(
            if class == "regulator_configuration"
                && name == "regulation"
                && dialect == super::GlmDialect::Arras4_3
            {
                (spec.0, Some("%"))
            } else {
                spec
            },
        );
    }
    if let Some(unit) = capacitors::unit(class, name)
        .or_else(|| geometry::unit(class, name))
        .or_else(|| cables::unit(class, name))
    {
        return Some((Real, Some(unit)));
    }
    if class == "switch" {
        match name {
            "switch_impedance" => return Some((Complex, Some("Ohm"))),
            "switch_resistance" | "switch_reactance" => return Some((Real, Some("Ohm"))),
            _ => {}
        }
    }
    if bus_class(class) {
        match name {
            "nominal_voltage" | "maximum_voltage_error" => return Some((Real, Some("V"))),
            "voltage_A" | "voltage_B" | "voltage_C" => return Some((Complex, Some("V"))),
            _ => {}
        }
    }
    if class == "load" {
        return loads::property(name).map(|(_, _, kind, unit)| (kind, Some(unit)));
    }
    if matches!(class, "overhead_line" | "underground_line") && name == "length" {
        return Some((Real, Some("ft")));
    }
    if class == "line_configuration" && matrix_property(name) {
        return Some(if name.starts_with('z') {
            (Complex, Some("Ohm/mile"))
        } else {
            (Real, Some("nF/mile"))
        });
    }
    None
}
pub(super) fn phase_name(phase: usize) -> char {
    ['A', 'B', 'C'][phase]
}
pub(super) fn nominal_phasor(magnitude: f64, phase: usize) -> Complex64 {
    Complex64::from_polar(
        magnitude,
        [
            0.0,
            -2.0 * std::f64::consts::PI / 3.0,
            2.0 * std::f64::consts::PI / 3.0,
        ][phase],
    )
}
fn parse_bus_phases(text: &str) -> Result<Vec<usize>, &'static str> {
    if text.bytes().filter(|&c| c == b'D').count() > 1 {
        return Err("duplicate delta phase flag");
    }
    parse_phases(&text.replace('D', ""))
}

fn parse_phases(text: &str) -> Result<Vec<usize>, &'static str> {
    let mut phases = BTreeSet::new();
    let mut neutral = false;
    for phase in text.bytes() {
        match phase {
            b'A'..=b'C' => {
                if !phases.insert(usize::from(phase - b'A')) {
                    return Err("duplicate phase flag");
                }
            }
            b'N' if !neutral => neutral = true,
            _ => {
                return Err(
                    "phase flags require additional delta, split-phase or conductor interpretation",
                );
            }
        }
    }
    if phases.is_empty() {
        return Err("no active phase declared");
    }
    Ok(phases.into_iter().collect())
}
