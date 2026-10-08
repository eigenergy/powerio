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

/// Read the initial static GLM electrical profile. Unknown physical
/// classes, properties and globals fail explicitly; they are not silently
/// omitted from an apparently usable network. The source project is retained.
///
/// This initial profile covers ordinary phase-to-earth node/load/meter objects,
/// fixed source phasors, P/I/Z loads and explicitly parameterized overhead lines.
/// Other conductor/device profiles remain under implementation.
pub fn parse(
    source: Source,
    options: PreprocessOptions,
) -> Result<PioModule<MulticonductorNetwork>, Error> {
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
        line_capacitance: false,
    };
    reader.top_level()?;
    reader.inventory()?;
    reader.buses()?;
    reader.loads_and_sources()?;
    reader.lines()?;
    crate::require_electrical_readiness(&reader.net)
        .map_err(|e| e.with_source(project.source().clone()))?;
    PioModule::parsed(
        reader.net,
        project
            .source()
            .clone()
            .with_format(FormatId::new("gridlabd")?),
        reader.diagnostics,
    )
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
    line_capacitance: bool,
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
                        match (tokens[0].text(), tokens[1].text()) {
                            ("solver_method", "NR" | "FBS") => {}
                            ("line_capacitance", "TRUE") => self.line_capacitance = true,
                            ("line_capacitance", "FALSE") => self.line_capacitance = false,
                            _ => {
                                return Err(self.project.electrical_error(
                                    span,
                                    "unqualified powerflow global setting",
                                ));
                            }
                        }
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
                Statement::Directive(token) => {
                    let text = token.text().trim();
                    let setting = text.strip_prefix("#set ").and_then(|s| s.split_once('='));
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
                }
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

    fn inventory(&mut self) -> Result<(), Error> {
        for object in self.index.objects() {
            if let Some(name) = object.name()
                && !self.identities.insert(name.to_ascii_lowercase())
            {
                return Err(self.error(
                    object.key(),
                    "object identities collide under current canonical case-insensitive lookup",
                ));
            }
        }
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
            if matches!(
                class,
                "recorder" | "impedance_dump" | "complex_assert" | "assert"
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
            if !matches!(
                class,
                "node" | "meter" | "load" | "overhead_line" | "line_configuration"
            ) {
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
                let allowed = allowed_property(class, name);
                if !allowed {
                    return Err(self.project.electrical_error(
                        property.span,
                        format!("unmapped property {class}.{name}"),
                    ));
                }
                if let Some((kind, unit)) = quantity_spec(class, name) {
                    // Native setters run in source order, even if a later
                    // assignment overwrites the value or its phase is absent.
                    let result = self.project.quantity(property, kind, Some(unit))?;
                    self.diagnostics.extend(result.diagnostics);
                    self.quantities
                        .insert((object.key(), name.to_owned()), result.value);
                }
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
        Ok(())
    }

    fn buses(&mut self) -> Result<(), Error> {
        let roots: BTreeSet<_> = self.roots.values().copied().collect();
        for key in roots {
            let phases = parse_phases(
                self.text(key, "phases")?
                    .as_deref()
                    .ok_or_else(|| self.error(key, "explicit phases are required"))?,
            )
            .map_err(|m| self.error(key, m))?;
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
            self.phases.insert(key, phases);
            self.nominal.insert(key, nominal);
        }
        Ok(())
    }

    fn loads_and_sources(&mut self) -> Result<(), Error> {
        let entries: Vec<_> = self.roots.iter().map(|(&k, &r)| (k, r)).collect();
        for (key, root) in entries {
            let phases = if key == root {
                self.phases[&root].clone()
            } else {
                let phases = parse_phases(
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
            if bustype == "SWING" {
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
            self.load_terms(key, root, &phases, nominal)?;
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
            .count();
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
            self.net.loads_mut().push(load);
        }
        Ok(())
    }

    fn require_explicit_matrix(&self, config: ObjectKey) -> Result<(), Error> {
        let mut explicit = false;
        for i in 0..3 {
            explicit |= self
                .quantity(config, &format!("z{}{}", i + 1, i + 1))
                .is_some_and(|v| v != Complex64::default());
        }
        if !explicit {
            return Err(self.error(config, "native configuration selects geometry; explicit nonzero diagonal impedance is required by this profile"));
        }
        Ok(())
    }

    fn lines(&mut self) -> Result<(), Error> {
        let keys: Vec<_> = self
            .index
            .objects()
            .iter()
            .filter(|o| class(o) == "overhead_line")
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
            let n = phases.len();
            let mut resistance = vec![vec![0.0; n]; n];
            let mut reactance = resistance.clone();
            let mut susceptance = resistance.clone();
            self.require_explicit_matrix(config)?;
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
            let code_name = allocate_name(
                &mut self.identities,
                &format!("{}__glm_linecode", self.names[&key]),
            );
            let mut code = DistLineCode::new(&code_name, resistance, reactance);
            code.b_from.clone_from(&susceptance);
            code.b_to = susceptance;
            self.net.line_codes_mut().push(code);
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
fn bus_class(name: &str) -> bool {
    matches!(name, "node" | "load" | "meter")
}
fn matrix_property(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 3
        && matches!(bytes[0], b'z' | b'c')
        && (b'1'..=b'3').contains(&bytes[1])
        && (b'1'..=b'3').contains(&bytes[2])
}
fn allowed_property(class: &str, name: &str) -> bool {
    matches!(name, "name" | "parent" | "groupid")
        || (matches!(class, "node" | "meter" | "load")
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
        || (class == "load"
            && [
                "constant_power_",
                "constant_current_",
                "constant_impedance_",
            ]
            .iter()
            .any(|prefix| {
                name.strip_prefix(prefix)
                    .is_some_and(|v| matches!(v, "A" | "B" | "C"))
            }))
        || (class == "overhead_line"
            && matches!(name, "phases" | "from" | "to" | "length" | "configuration"))
        || (class == "line_configuration" && matrix_property(name))
}
fn quantity_spec(class: &str, name: &str) -> Option<(GlmQuantityKind, &'static str)> {
    use GlmQuantityKind::{Complex, Real};
    if bus_class(class) {
        match name {
            "nominal_voltage" | "maximum_voltage_error" => return Some((Real, "V")),
            "voltage_A" | "voltage_B" | "voltage_C" => return Some((Complex, "V")),
            _ => {}
        }
    }
    if class == "load" {
        for (prefix, unit) in [
            ("constant_power_", "VA"),
            ("constant_current_", "A"),
            ("constant_impedance_", "Ohm"),
        ] {
            if name
                .strip_prefix(prefix)
                .is_some_and(|phase| matches!(phase, "A" | "B" | "C"))
            {
                return Some((Complex, unit));
            }
        }
    }
    if class == "overhead_line" && name == "length" {
        return Some((Real, "ft"));
    }
    if class == "line_configuration" && matrix_property(name) {
        return Some(if name.starts_with('z') {
            (Complex, "Ohm/mile")
        } else {
            (Real, "nF/mile")
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
