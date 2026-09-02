//! The UCTE-DEF reader: fixed column records into the balanced network.

use super::{
    AngleRegulation, Area, BASE_FREQUENCY, BASE_MVA, BTreeSet, BalancedNetwork, Branch,
    BranchCharging, BranchCurrentRatings, Bus, BusId, BusType, CONTROL_AREA, CROSS_BORDER_AREA,
    DEFAULT_POWER_LIMIT, DiagnosticInfo, Diagnostics, ElementId, Error, Extras, FMT, Generator,
    GeneratorEnergySource, HashMap, Load, MIN_REACTANCE_OHM, NodeCode, ORDER_CODES, PLANT_TYPES,
    PhaseRegulation, REVISION, REVISIONS, Result, SQRT_3, SourceFormat, Switch, TransformerControl,
    TransformerControlMode, Value, codes, country_iso, json,
};

/// One data line, addressed by character column.
struct Record<'a> {
    chars: Vec<char>,
    text: &'a str,
    line: usize,
    byte_start: usize,
    byte_end: usize,
}

impl Record<'_> {
    fn error(&self, message: impl Into<String>) -> Error {
        Error::FormatRead {
            format: FMT,
            message: format!("line {}: {}", self.line, message.into()),
        }
    }

    /// The characters in `begin..end`, untrimmed; `None` when the line ends
    /// before `begin`. A line ending inside the range yields the shorter text,
    /// which is how a 2003.09.01 record reads with the 2007.05.01 layout.
    fn raw(&self, begin: usize, end: usize) -> Option<String> {
        let end = end.min(self.chars.len());
        (end >= begin).then(|| self.chars[begin..end].iter().collect())
    }

    fn field(&self, begin: usize, end: usize) -> Option<String> {
        self.raw(begin, end)
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    }

    fn char_at(&self, index: usize) -> Option<char> {
        self.chars.get(index).copied().filter(|c| *c != ' ')
    }

    fn number(&self, begin: usize, end: usize, what: &str) -> Result<Option<f64>> {
        self.field(begin, end)
            .map(|text| {
                text.parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| self.error(format!("{what} {text:?} is not a number")))
            })
            .transpose()
    }

    fn integer(&self, begin: usize, end: usize, what: &str) -> Result<Option<i64>> {
        self.field(begin, end)
            .map(|text| {
                text.parse::<i64>()
                    .ok()
                    .ok_or_else(|| self.error(format!("{what} {text:?} is not an integer")))
            })
            .transpose()
    }

    fn digit(&self, index: usize, what: &str) -> Result<Option<u8>> {
        self.char_at(index)
            .map(|c| {
                c.to_digit(10)
                    .and_then(|d| u8::try_from(d).ok())
                    .ok_or_else(|| self.error(format!("{what} {c:?} is not a digit")))
            })
            .transpose()
    }

    fn node_code(&self, begin: usize, what: &str) -> Result<NodeCode> {
        let text = self.raw(begin, begin + 8).unwrap_or_default();
        NodeCode::parse(&text).ok_or_else(|| {
            self.error(format!(
                "{what} {text:?} is not a UCTE node code (country letter, five character \
                 geographical spot, voltage level digit, busbar)"
            ))
        })
    }

    fn element_id(&self) -> Result<ElementId> {
        let node1 = self.node_code(0, "node 1 code")?;
        let node2 = self.node_code(9, "node 2 code")?;
        let order = self.chars.get(18).copied().unwrap_or(' ');
        if self.chars.get(8) != Some(&' ')
            || self.chars.get(17) != Some(&' ')
            || !(ORDER_CODES.contains(order) || order == ' ')
        {
            return Err(self.error(format!(
                "{:?} is not a UCTE element id (node code, blank, node code, blank, order code)",
                self.raw(0, 19).unwrap_or_default()
            )));
        }
        Ok(ElementId {
            node1,
            node2,
            order,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Comment,
    Node,
    Line,
    Transformer,
    Regulation,
    SpecialDescription,
    Exchange,
}

struct NodeRecord {
    code: NodeCode,
    zone_iso: String,
    geographical_name: Option<String>,
    equivalent: bool,
    type_code: Option<u8>,
    voltage_reference: Option<f64>,
    active_load: Option<f64>,
    reactive_load: Option<f64>,
    active_generation: Option<f64>,
    reactive_generation: Option<f64>,
    min_p: Option<f64>,
    max_p: Option<f64>,
    min_q: Option<f64>,
    max_q: Option<f64>,
    primary_control_static: Option<f64>,
    primary_control_power: Option<f64>,
    short_circuit_power: Option<f64>,
    xr_ratio: Option<f64>,
    plant_type: Option<char>,
    line: usize,
    byte_start: usize,
    byte_end: usize,
}

struct Reader<'a> {
    warnings: &'a mut Diagnostics,
}

impl Reader<'_> {
    fn warn_at(
        &mut self,
        info: &'static DiagnosticInfo,
        line: Option<(usize, usize)>,
        message: String,
    ) {
        self.warnings.leave_record();
        if let Some((start, end)) = line {
            self.warnings.enter_record(start, end);
        }
        self.warnings.push(info, message);
        self.warnings.leave_record();
    }

    fn warn(
        &mut self,
        info: &'static DiagnosticInfo,
        record: &Record<'_>,
        message: impl Into<String>,
    ) {
        self.warn_at(
            info,
            Some((record.byte_start, record.byte_end)),
            format!("line {}: {}", record.line, message.into()),
        );
    }
}

fn zone_iso(record: &Record<'_>) -> String {
    record.field(3, 5).unwrap_or_default()
}

fn parse_node(record: &Record<'_>, zone_iso: &str) -> Result<NodeRecord> {
    let code = record.node_code(0, "node code")?;
    let plant_type = record.char_at(127);
    if let Some(letter) = plant_type
        && !PLANT_TYPES.contains(letter)
    {
        return Err(record.error(format!(
            "power plant type {letter:?} is not one of H, N, L, C, G, O, W, F"
        )));
    }
    let status = record.digit(22, "node status")?;
    if status.is_some_and(|status| status > 1) {
        return Err(record.error("node status must be 0 (real) or 1 (equivalent)"));
    }
    let type_code = record.digit(24, "node type code")?;
    if type_code.is_some_and(|code| code > 3) {
        return Err(record.error("node type code must be 0 (PQ), 1 (QT), 2 (PU), or 3 (UT)"));
    }
    Ok(NodeRecord {
        code,
        zone_iso: zone_iso.to_owned(),
        geographical_name: record.field(9, 21),
        equivalent: status == Some(1),
        type_code,
        voltage_reference: record.number(26, 32, "voltage reference")?,
        active_load: record.number(33, 40, "active load")?,
        reactive_load: record.number(41, 48, "reactive load")?,
        active_generation: record.number(49, 56, "active power generation")?,
        reactive_generation: record.number(57, 64, "reactive power generation")?,
        min_p: record.number(65, 72, "minimum permissible active power generation")?,
        max_p: record.number(73, 80, "maximum permissible active power generation")?,
        min_q: record.number(81, 88, "minimum permissible reactive power generation")?,
        max_q: record.number(89, 96, "maximum permissible reactive power generation")?,
        primary_control_static: record.number(97, 102, "static of primary control")?,
        primary_control_power: record.number(103, 110, "nominal power for primary control")?,
        short_circuit_power: record.number(111, 118, "three phase short circuit power")?,
        xr_ratio: record.number(119, 126, "X/R ratio")?,
        plant_type,
        line: record.line,
        byte_start: record.byte_start,
        byte_end: record.byte_end,
    })
}

/// The element status digit: `0`/`8` real, `1`/`9` equivalent, `2`/`7`
/// busbar coupler; the second of each pair is out of operation.
fn element_status(record: &Record<'_>) -> Result<u8> {
    let status = record
        .digit(20, "element status")?
        .ok_or_else(|| record.error("element status is blank"))?;
    if matches!(status, 0 | 8 | 1 | 9 | 2 | 7) {
        Ok(status)
    } else {
        Err(record.error(format!(
            "element status {status} is not one of 0, 8 (real), 1, 9 (equivalent), 2, 7 (busbar coupler)"
        )))
    }
}

fn is_valid_value(value: Option<f64>) -> bool {
    value.is_some_and(|value| value != 0.0)
}

fn extra_number(extras: &mut Extras, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        extras.insert(key.to_owned(), json!(value));
    }
}

/// One `##R` record before it attaches to its transformer.
struct RegulationRecord {
    id: ElementId,
    phase: Option<PhaseRegulation>,
    angle: Option<AngleRegulation>,
    line: usize,
    byte_start: usize,
    byte_end: usize,
}

struct BranchInput {
    id: ElementId,
    branch: Branch,
    /// The from bus voltage level base, for regulation voltage targets.
    from_kv: f64,
    nominal_power: Option<f64>,
    line: usize,
}

/// Parse UCTE-DEF text into a balanced network. `name_hint` (the file stem)
/// names the network and, when it follows the UCTE file name convention
/// `<yyyymmdd>_<HHMM>_<TY><w>_<cc><v>`, dates the case. The caller locates
/// `warnings` in the decoded source so record findings carry spans.
#[expect(clippy::too_many_lines)]
pub(crate) fn parse_ucte_source(
    text: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let mut reader = Reader { warnings };
    let mut block: Option<Block> = None;
    let mut revision: Option<String> = None;
    let mut zone: Option<String> = None;
    let mut nodes: Vec<NodeRecord> = Vec::new();
    let mut node_index: HashMap<NodeCode, usize> = HashMap::new();
    let mut lines: Vec<BranchInput> = Vec::new();
    let mut switches: Vec<Switch> = Vec::new();
    let mut transformers: Vec<BranchInput> = Vec::new();
    let mut transformer_index: HashMap<ElementId, usize> = HashMap::new();
    let mut regulations: Vec<RegulationRecord> = Vec::new();
    let mut special_descriptions: Vec<(ElementId, String, usize, usize, usize)> = Vec::new();
    let mut exchanges: Vec<String> = Vec::new();
    let mut exchange_span: Option<(usize, usize)> = None;
    let mut special_span: Option<(usize, usize)> = None;

    let mut offset = 0usize;
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        let byte_start = offset;
        offset += raw.len();
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let byte_end = byte_start + line.len();
        if line.trim().is_empty() {
            continue;
        }
        let record = Record {
            chars: line.chars().collect(),
            text: line,
            line: index + 1,
            byte_start,
            byte_end,
        };
        if line.starts_with("##") {
            let key = if line.starts_with("##TT") {
                Block::SpecialDescription
            } else {
                match record.chars.get(2) {
                    Some('C') => Block::Comment,
                    Some('N') => Block::Node,
                    Some('L') => Block::Line,
                    Some('T') => Block::Transformer,
                    Some('R') => Block::Regulation,
                    Some('E') => Block::Exchange,
                    Some('Z') => {
                        if block != Some(Block::Node) {
                            return Err(
                                record.error("a ##Z country line belongs inside the ##N block")
                            );
                        }
                        zone = Some(zone_iso(&record));
                        continue;
                    }
                    _ => {
                        return Err(record.error(format!(
                            "unknown block key {:?}; expected ##C, ##N, ##Z, ##L, ##T, ##R, ##TT, or ##E",
                            record.raw(0, 4).unwrap_or_default()
                        )));
                    }
                }
            };
            if block.is_none() && key != Block::Comment {
                return Err(record.error("the first block must be a ##C comment block"));
            }
            if key == Block::Comment && revision.is_none() {
                let declared = record.field(4, 14).unwrap_or_default();
                if !REVISIONS.contains(&declared.as_str()) {
                    return Err(record.error(format!(
                        "revision {declared:?} is not supported; expected ##C 2003.09.01 or ##C 2007.05.01"
                    )));
                }
                revision = Some(declared);
            }
            if key == Block::Node {
                zone = None;
            }
            block = Some(key);
            continue;
        }
        match block {
            None => {
                return Err(record.error("expected a ## key line before the first record"));
            }
            Some(Block::Comment) => {}
            Some(Block::Node) => {
                let Some(zone_iso) = zone.as_deref() else {
                    return Err(record.error("a node must be defined under a ##Z country line"));
                };
                let node = parse_node(&record, zone_iso)?;
                if let Some(first) = node_index.get(&node.code).copied() {
                    reader.warn(
                        &codes::READ_UCTE_VALUE_SUBSTITUTED,
                        &record,
                        format!(
                            "node {:?} repeats the node of line {}; the later record replaces the earlier one",
                            node.code.text(),
                            nodes[first].line
                        ),
                    );
                    nodes[first] = node;
                } else {
                    node_index.insert(node.code, nodes.len());
                    nodes.push(node);
                }
            }
            Some(Block::Line) => {
                let id = record.element_id()?;
                let status = element_status(&record)?;
                let from = node_bus(&node_index, id.node1, &record)?;
                let to = node_bus(&node_index, id.node2, &record)?;
                let current_limit = record.integer(45, 51, "current limit")?;
                let name = record.field(52, 64);
                if matches!(status, 2 | 7) {
                    if from == to {
                        reader.warn(
                            &codes::READ_UCTE_RECORD_IGNORED,
                            &record,
                            format!(
                                "busbar coupler {:?} names the same node at both ends; PowSybl UcteImporter ignores it",
                                id.text()
                            ),
                        );
                        continue;
                    }
                    let mut switch = Switch::new(from, to, status == 2);
                    switch.current_rating = current_limit
                        .filter(|limit| *limit > 0)
                        .map(|limit| limit as f64);
                    switch
                        .extras
                        .insert("ucte_order_code".into(), json!(id.order.to_string()));
                    if let Some(name) = name {
                        switch
                            .extras
                            .insert("ucte_element_name".into(), json!(name));
                    }
                    if let Some(existing) = switches.iter().position(|s| {
                        s.from == from
                            && s.to == to
                            && s.extras.get("ucte_order_code") == Some(&json!(id.order.to_string()))
                    }) {
                        reader.warn(
                            &codes::READ_UCTE_VALUE_SUBSTITUTED,
                            &record,
                            format!("busbar coupler {:?} repeats an earlier record; the later record replaces it", id.text()),
                        );
                        switches[existing] = switch;
                    } else {
                        switches.push(switch);
                    }
                    continue;
                }
                if id.node1.level() != id.node2.level() {
                    return Err(record.error(format!(
                        "line {:?} joins two different voltage levels ({} kV and {} kV); UCTE-DEF states such an element as a transformer",
                        id.text(),
                        id.node1.base_kv(),
                        id.node2.base_kv()
                    )));
                }
                let kv = id.node1.base_kv();
                let zbase = kv * kv / BASE_MVA;
                let r = record.number(22, 28, "resistance")?;
                let x = record.number(29, 35, "reactance")?;
                let b = record.number(36, 44, "susceptance")?;
                let (r_ohm, x_ohm) = series_impedance(&mut reader, &record, &id, r, x);
                let mut branch = Branch::new(from, to, r_ohm / zbase, x_ohm / zbase);
                branch.name = name;
                branch.in_service = matches!(status, 0 | 1);
                branch.b = b.unwrap_or(0.0) * 1e-6 * zbase;
                branch.charging = Some(BranchCharging::from_total_b(branch.b));
                apply_current_limit(&mut reader, &record, &id, &mut branch, current_limit, kv);
                branch
                    .extras
                    .insert("ucte_order_code".into(), json!(id.order.to_string()));
                if matches!(status, 1 | 9) {
                    branch.extras.insert("ucte_equivalent".into(), json!(true));
                }
                if let Some(existing) = lines.iter().position(|l| l.id == id) {
                    reader.warn(
                        &codes::READ_UCTE_VALUE_SUBSTITUTED,
                        &record,
                        format!(
                            "line {:?} repeats the record of line {}; the later record replaces it",
                            id.text(),
                            lines[existing].line
                        ),
                    );
                    lines[existing].branch = branch;
                } else {
                    lines.push(BranchInput {
                        id,
                        branch,
                        from_kv: kv,
                        nominal_power: None,
                        line: record.line,
                    });
                }
            }
            Some(Block::Transformer) => {
                let id = record.element_id()?;
                let status = element_status(&record)?;
                if matches!(status, 2 | 7) {
                    return Err(record.error(format!(
                        "transformer {:?} carries busbar coupler status {status}",
                        id.text()
                    )));
                }
                // Node 2 is the regulated winding; the branch runs from it to
                // node 1, whose voltage base carries the impedance.
                let from = node_bus(&node_index, id.node2, &record)?;
                let to = node_bus(&node_index, id.node1, &record)?;
                let kv1 = id.node1.base_kv();
                let kv2 = id.node2.base_kv();
                let zbase = kv1 * kv1 / BASE_MVA;
                let rated_u1 = rated_voltage(
                    &mut reader,
                    &record,
                    &id,
                    record.number(22, 27, "rated voltage 1")?,
                    1,
                    kv1,
                );
                let rated_u2 = rated_voltage(
                    &mut reader,
                    &record,
                    &id,
                    record.number(28, 33, "rated voltage 2")?,
                    2,
                    kv2,
                );
                let nominal_power = record.number(34, 39, "nominal power")?;
                let r = record.number(40, 46, "resistance")?;
                let x = record.number(47, 53, "reactance")?;
                let b = record.number(54, 62, "susceptance")?;
                let g = record.number(63, 69, "conductance")?;
                let current_limit = record.integer(70, 76, "current limit")?;
                let (r_ohm, x_ohm) = series_impedance(&mut reader, &record, &id, r, x);
                let mut branch = Branch::new(from, to, r_ohm / zbase, x_ohm / zbase);
                branch.name = record.field(77, 89);
                branch.in_service = matches!(status, 0 | 1);
                branch.tap = (rated_u2 / kv2) / (rated_u1 / kv1);
                let b_fr = b.unwrap_or(0.0) * 1e-6 * zbase;
                let g_fr = g.unwrap_or(0.0) * 1e-6 * zbase;
                branch.b = b_fr;
                branch.charging = Some(BranchCharging::new(g_fr, b_fr, 0.0, 0.0));
                apply_current_limit(&mut reader, &record, &id, &mut branch, current_limit, kv1);
                branch
                    .extras
                    .insert("ucte_order_code".into(), json!(id.order.to_string()));
                branch
                    .extras
                    .insert("ucte_rated_voltage_1".into(), json!(rated_u1));
                extra_number(&mut branch.extras, "ucte_nominal_power", nominal_power);
                if matches!(status, 1 | 9) {
                    branch.extras.insert("ucte_equivalent".into(), json!(true));
                }
                if let Some(existing) = transformer_index.get(&id).copied() {
                    reader.warn(
                        &codes::READ_UCTE_VALUE_SUBSTITUTED,
                        &record,
                        format!("transformer {:?} repeats the record of line {}; the later record replaces it", id.text(), transformers[existing].line),
                    );
                    transformers[existing].branch = branch;
                    transformers[existing].nominal_power = nominal_power;
                } else {
                    transformer_index.insert(id, transformers.len());
                    transformers.push(BranchInput {
                        id,
                        branch,
                        from_kv: kv2,
                        nominal_power,
                        line: record.line,
                    });
                }
            }
            Some(Block::Regulation) => {
                let id = record.element_id()?;
                let phase = parse_phase_regulation(&record)?;
                let angle = parse_angle_regulation(&record)?;
                let parsed = RegulationRecord {
                    id,
                    phase,
                    angle,
                    line: record.line,
                    byte_start: record.byte_start,
                    byte_end: record.byte_end,
                };
                if let Some(existing) = regulations.iter().position(|r| r.id == id) {
                    reader.warn(
                        &codes::READ_UCTE_VALUE_SUBSTITUTED,
                        &record,
                        format!("regulation of {:?} repeats the record of line {}; the later record replaces it", id.text(), regulations[existing].line),
                    );
                    regulations[existing] = parsed;
                } else {
                    regulations.push(parsed);
                }
            }
            Some(Block::SpecialDescription) => {
                let id = record.element_id()?;
                special_span = Some((
                    special_span.map_or(record.byte_start, |(start, _)| start),
                    record.byte_end,
                ));
                special_descriptions.push((
                    id,
                    record.text.to_owned(),
                    record.line,
                    record.byte_start,
                    record.byte_end,
                ));
            }
            Some(Block::Exchange) => {
                exchange_span = Some((
                    exchange_span.map_or(record.byte_start, |(start, _)| start),
                    record.byte_end,
                ));
                exchanges.push(describe_exchange(&record));
            }
        }
    }
    if block.is_none() {
        return Err(Error::FormatRead {
            format: FMT,
            message: "empty file: expected a ##C comment block".into(),
        });
    }

    // Areas: one per country letter in node block order; the cross border
    // country X is its own area so tie lines keep both ends.
    let mut areas: Vec<Area> = Vec::new();
    let mut area_by_letter: HashMap<char, usize> = HashMap::new();
    let mut buses = Vec::with_capacity(nodes.len());
    let mut loads = Vec::new();
    let mut generators = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let letter = node.code.country();
        let iso = country_iso(letter).unwrap_or("XX");
        let area_number = *area_by_letter.entry(letter).or_insert_with(|| {
            let mut area = Area::new(areas.len() + 1);
            area.name = Some(iso.to_owned());
            area.uid = Some(iso.to_owned());
            area.area_type = Some(
                if node.code.is_cross_border() {
                    CROSS_BORDER_AREA
                } else {
                    CONTROL_AREA
                }
                .to_owned(),
            );
            areas.push(area);
            areas.len()
        });
        if !node.zone_iso.eq_ignore_ascii_case(iso) {
            reader.warn_at(
                &codes::READ_UCTE_VALUE_SUBSTITUTED,
                Some((node.byte_start, node.byte_end)),
                format!(
                    "line {}: node {:?} sits under ##Z{} but its country letter {letter:?} means {iso}; the letter decides the area",
                    node.line,
                    node.code.text(),
                    node.zone_iso
                ),
            );
        }
        let bus_id = BusId(index + 1);
        let base_kv = node.code.base_kv();
        let mut bus = Bus::new(bus_id, BusType::Pq, base_kv);
        bus.name = Some(node.code.text());
        bus.area = area_number;
        if let Some(name) = &node.geographical_name {
            bus.extras
                .insert("ucte_geographical_name".into(), json!(name));
        }
        if node.equivalent {
            bus.extras.insert("ucte_node_status".into(), json!(1));
        }
        if let Some(letter) = node.plant_type {
            bus.extras
                .insert("ucte_power_plant_type".into(), json!(letter.to_string()));
        }
        extra_number(
            &mut bus.extras,
            "ucte_primary_control_static",
            node.primary_control_static,
        );
        extra_number(
            &mut bus.extras,
            "ucte_primary_control_power",
            node.primary_control_power,
        );
        extra_number(
            &mut bus.extras,
            "ucte_short_circuit_power",
            node.short_circuit_power,
        );
        extra_number(&mut bus.extras, "ucte_xr_ratio", node.xr_ratio);
        if let Some(reference) = node.voltage_reference.filter(|v| *v > 0.0) {
            bus.vm = reference / base_kv;
        }
        map_node_kind_and_generation(&mut reader, node, &mut bus, &mut generators, base_kv);
        if is_valid_value(node.active_load) || is_valid_value(node.reactive_load) {
            loads.push(Load::new(
                bus_id,
                node.active_load.unwrap_or(0.0),
                node.reactive_load.unwrap_or(0.0),
            ));
        }
        buses.push(bus);
    }

    // Regulation and special description rows attach to their transformer.
    for regulation in regulations {
        let id = regulation.id;
        let Some(index) = transformer_index.get(&id).copied() else {
            reader.warn_at(
                &codes::READ_UCTE_REFERENCE_DROPPED,
                Some((regulation.byte_start, regulation.byte_end)),
                format!("line {}: ##R regulation names transformer {:?}, which the ##T block does not declare; dropped", regulation.line, id.text()),
            );
            continue;
        };
        let record = (regulation.line, regulation.byte_start, regulation.byte_end);
        let transformer = &mut transformers[index];
        let phase = regulation
            .phase
            .and_then(|phase| fix_phase_regulation(&mut reader, record, &id, phase));
        let angle = regulation
            .angle
            .and_then(|angle| fix_angle_regulation(&mut reader, record, &id, angle));
        apply_regulation(
            &mut reader,
            record,
            transformer,
            phase.as_ref(),
            angle.as_ref(),
        );
    }
    if !special_descriptions.is_empty() {
        let mut named = BTreeSet::new();
        for (id, text, line, byte_start, byte_end) in &special_descriptions {
            named.insert(id.text());
            match transformer_index.get(id).copied() {
                Some(index) => {
                    let extras = &mut transformers[index].branch.extras;
                    let rows = extras
                        .entry("ucte_special_description".to_owned())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Value::Array(rows) = rows {
                        rows.push(json!(text));
                    }
                }
                None => reader.warn_at(
                    &codes::READ_UCTE_REFERENCE_DROPPED,
                    Some((*byte_start, *byte_end)),
                    format!("line {line}: ##TT special description names transformer {:?}, which the ##T block does not declare; kept in the retained source only", id.text()),
                ),
            }
        }
        reader.warn_at(
            &codes::READ_UCTE_RETAINED_SOURCE_ONLY,
            special_span,
            format!(
                "##TT block: {} special transformer description record(s) for {} have no typed field; they survive in the retained source and in the transformer extras `ucte_special_description`, which only fresh UCTE output replays",
                special_descriptions.len(),
                named.into_iter().map(|id| format!("{id:?}")).collect::<Vec<_>>().join(", ")
            ),
        );
    }
    if !exchanges.is_empty() {
        reader.warn_at(
            &codes::READ_UCTE_RETAINED_SOURCE_ONLY,
            exchange_span,
            format!(
                "##E block: {} scheduled exchange record(s) ({}) have no typed field; they survive in the retained source only and no other target carries them",
                exchanges.len(),
                exchanges.join("; ")
            ),
        );
    }

    let mut net = BalancedNetwork::new(name_hint.unwrap_or("ucte"), BASE_MVA);
    *net.base_frequency_mut() = BASE_FREQUENCY;
    *net.source_format_mut() = SourceFormat::Ucte;
    if let Some(date) = name_hint.and_then(case_date_from_name) {
        net.case_metadata_mut().case_date = Some(date);
    }
    *net.buses_mut() = buses;
    *net.loads_mut() = loads;
    *net.generators_mut() = generators;
    *net.branches_mut() = lines
        .into_iter()
        .chain(transformers)
        .map(|input| input.branch)
        .collect();
    *net.switches_mut() = switches;
    *net.areas_mut() = areas;
    reader.warnings.push_at(
        &codes::READ_UCTE_VALUE_DEFAULTED,
        crate::diagnostics::DiagnosticSeverity::Remark,
        format!(
            "UCTE-DEF {} states physical units and no system base; the balanced view uses {BASE_MVA} MVA and the {BASE_FREQUENCY} Hz synchronous area frequency",
            revision.as_deref().unwrap_or(REVISION)
        ),
    );
    Ok(net)
}

fn node_bus(
    index: &HashMap<NodeCode, usize>,
    code: NodeCode,
    record: &Record<'_>,
) -> Result<BusId> {
    index
        .get(&code)
        .map(|position| BusId(position + 1))
        .ok_or_else(|| {
            record.error(format!(
                "node {:?} is not declared in the ##N block",
                code.text()
            ))
        })
}

/// The series impedance in ohm after the reactance floor PowSybl applies; a
/// blank value reads as zero and is reported.
fn series_impedance(
    reader: &mut Reader<'_>,
    record: &Record<'_>,
    id: &ElementId,
    r: Option<f64>,
    x: Option<f64>,
) -> (f64, f64) {
    let r_ohm = r.unwrap_or_else(|| {
        reader.warn(
            &codes::READ_UCTE_VALUE_DEFAULTED,
            record,
            format!(
                "element {:?} states no resistance; read as 0 ohm",
                id.text()
            ),
        );
        0.0
    });
    let mut x_ohm = x.unwrap_or_else(|| {
        reader.warn(
            &codes::READ_UCTE_VALUE_DEFAULTED,
            record,
            format!("element {:?} states no reactance; read as 0 ohm", id.text()),
        );
        0.0
    });
    if x_ohm.abs() < MIN_REACTANCE_OHM {
        let floored = if x_ohm >= 0.0 {
            MIN_REACTANCE_OHM
        } else {
            -MIN_REACTANCE_OHM
        };
        reader.warn(
            &codes::READ_UCTE_VALUE_SUBSTITUTED,
            record,
            format!(
                "element {:?} reactance {x_ohm} ohm is below the {MIN_REACTANCE_OHM} ohm floor the reference importer applies; read as {floored} ohm",
                id.text()
            ),
        );
        x_ohm = floored;
    }
    (r_ohm, x_ohm)
}

fn rated_voltage(
    reader: &mut Reader<'_>,
    record: &Record<'_>,
    id: &ElementId,
    value: Option<f64>,
    winding: u8,
    nominal_kv: f64,
) -> f64 {
    match value {
        Some(value) if value > 0.0 => value,
        _ => {
            reader.warn(
                &codes::READ_UCTE_VALUE_DEFAULTED,
                record,
                format!(
                    "transformer {:?} states no positive rated voltage {winding}; read as the node voltage level {nominal_kv} kV",
                    id.text()
                ),
            );
            nominal_kv
        }
    }
}

/// The permanent current limit in ampere becomes `rate_a` in MVA at the
/// element's voltage level and stays exact in `current_ratings`.
fn apply_current_limit(
    reader: &mut Reader<'_>,
    record: &Record<'_>,
    id: &ElementId,
    branch: &mut Branch,
    current_limit: Option<i64>,
    kv: f64,
) {
    match current_limit {
        Some(limit) if limit > 0 => {
            let amps = limit as f64;
            branch.rate_a = SQRT_3 * kv * amps / 1000.0;
            branch.current_ratings = Some(BranchCurrentRatings::new(amps, 0.0, 0.0));
        }
        Some(limit) => reader.warn(
            &codes::READ_UCTE_VALUE_SUBSTITUTED,
            record,
            format!(
                "element {:?} current limit {limit} A is not positive; read as unrated",
                id.text()
            ),
        ),
        None => {}
    }
}

fn parse_phase_regulation(record: &Record<'_>) -> Result<Option<PhaseRegulation>> {
    let du = record.number(20, 25, "phase regulation voltage step")?;
    let n = record.integer(26, 28, "phase regulation tap count")?;
    let np = record.integer(29, 32, "phase regulation tap position")?;
    let u = record.number(33, 38, "phase regulation voltage target")?;
    if du.is_none() && n.is_none() && np.is_none() && u.is_none() {
        return Ok(None);
    }
    // Incomplete data is kept until the fix step, which reports and drops it.
    Ok(Some(PhaseRegulation {
        du: du.unwrap_or(f64::NAN),
        n: n.unwrap_or(0),
        np: np.unwrap_or(i64::MIN),
        u,
    }))
}

fn parse_angle_regulation(record: &Record<'_>) -> Result<Option<AngleRegulation>> {
    let du = record.number(39, 44, "angle regulation voltage step")?;
    let theta = record.number(45, 50, "angle regulation angle")?;
    let n = record.integer(51, 53, "angle regulation tap count")?;
    let np = record.integer(54, 57, "angle regulation tap position")?;
    let p = record.number(58, 63, "angle regulation active power target")?;
    let kind = record.field(64, 68);
    if du.is_none()
        && theta.is_none()
        && n.is_none()
        && np.is_none()
        && p.is_none()
        && kind.is_none()
    {
        return Ok(None);
    }
    // A blank type reads as ASYM; the fix step reports that default.
    let symmetrical = match kind.as_deref() {
        Some("SYMM") => true,
        Some("ASYM") | None => false,
        Some(other) => {
            return Err(record.error(format!(
                "angle regulation type {other:?} is not ASYM or SYMM"
            )));
        }
    };
    Ok(Some(AngleRegulation {
        du: du.unwrap_or(f64::NAN),
        theta: theta.unwrap_or(f64::NAN),
        n: n.unwrap_or(0),
        np: np.unwrap_or(i64::MIN),
        p,
        symmetrical,
        type_stated: kind.is_some(),
    }))
}

fn fix_phase_regulation(
    reader: &mut Reader<'_>,
    (line, byte_start, byte_end): (usize, usize, usize),
    id: &ElementId,
    mut regulation: PhaseRegulation,
) -> Option<PhaseRegulation> {
    if regulation.u.is_some_and(|u| u <= 0.0) {
        reader.warn_at(
            &codes::READ_UCTE_VALUE_SUBSTITUTED,
            Some((byte_start, byte_end)),
            format!(
                "line {line}: phase regulation of {:?} has voltage target {} kV, which is not positive; the target is dropped",
                id.text(),
                regulation.u.unwrap_or_default()
            ),
        );
        regulation.u = None;
    }
    if regulation.n == 0 || regulation.np == i64::MIN || regulation.du.is_nan() {
        reader.warn_at(
            &codes::READ_UCTE_VALUE_SUBSTITUTED,
            Some((byte_start, byte_end)),
            format!(
                "line {line}: phase regulation of {:?} is incomplete (needs a nonzero tap count, a tap position, and a voltage step); dropped",
                id.text()
            ),
        );
        return None;
    }
    Some(regulation)
}

fn fix_angle_regulation(
    reader: &mut Reader<'_>,
    (line, byte_start, byte_end): (usize, usize, usize),
    id: &ElementId,
    regulation: AngleRegulation,
) -> Option<AngleRegulation> {
    if regulation.n == 0
        || regulation.np == i64::MIN
        || regulation.du.is_nan()
        || regulation.theta.is_nan()
    {
        reader.warn_at(
            &codes::READ_UCTE_VALUE_SUBSTITUTED,
            Some((byte_start, byte_end)),
            format!(
                "line {line}: angle regulation of {:?} is incomplete (needs a nonzero tap count, a tap position, a voltage step, and an angle); dropped",
                id.text()
            ),
        );
        return None;
    }
    if !regulation.type_stated {
        reader.warn_at(
            &codes::READ_UCTE_VALUE_DEFAULTED,
            Some((byte_start, byte_end)),
            format!(
                "line {line}: angle regulation of {:?} states no type; read as ASYM",
                id.text()
            ),
        );
    }
    Some(regulation)
}

/// Fold the regulation at its current tap position into the branch tap and
/// shift, record the control, and keep the regulation itself in the extras.
fn apply_regulation(
    reader: &mut Reader<'_>,
    (line, byte_start, byte_end): (usize, usize, usize),
    transformer: &mut BranchInput,
    phase: Option<&PhaseRegulation>,
    angle: Option<&AngleRegulation>,
) {
    let id = transformer.id;
    let branch = &mut transformer.branch;
    let nominal_tap = branch.tap;
    let mut control = TransformerControl::new(TransformerControlMode::Fixed);
    control.controlled_bus = Some(branch.from);
    control.mva_base = transformer.nominal_power.unwrap_or(0.0);
    let mut has_control = false;
    if let Some(phase) = phase {
        let factor = 1.0 + phase.np as f64 * phase.du / 100.0;
        if factor <= 0.0 {
            reader.warn_at(
                &codes::READ_UCTE_VALUE_SUBSTITUTED,
                Some((byte_start, byte_end)),
                format!(
                    "line {line}: phase regulation of {:?} at tap {} with step {} % gives a non positive ratio; the ratio is ignored",
                    id.text(),
                    phase.np,
                    phase.du
                ),
            );
        } else {
            branch.tap *= factor;
        }
        has_control = true;
        control.ntp = u32::try_from(2 * phase.n.unsigned_abs() + 1).unwrap_or(u32::MAX);
        control.tap_min = nominal_tap * (1.0 - phase.n.unsigned_abs() as f64 * phase.du / 100.0);
        control.tap_max = nominal_tap * (1.0 + phase.n.unsigned_abs() as f64 * phase.du / 100.0);
        if let Some(u) = phase.u {
            control.mode = TransformerControlMode::Voltage;
            control.enabled = true;
            control.band_min = u / transformer.from_kv;
            control.band_max = u / transformer.from_kv;
        }
        branch.extras.insert(
            "ucte_phase_regulation".into(),
            json!({ "du": phase.du, "n": phase.n, "np": phase.np, "u": phase.u }),
        );
    }
    if let Some(angle) = angle {
        let (rho, alpha) = angle.rho_alpha(angle.np);
        if rho.is_finite() && rho > 0.0 {
            branch.tap /= rho;
            branch.shift = alpha;
        } else {
            reader.warn_at(
                &codes::READ_UCTE_VALUE_SUBSTITUTED,
                Some((byte_start, byte_end)),
                format!(
                    "line {line}: angle regulation of {:?} at tap {} gives a degenerate ratio; the regulation is ignored",
                    id.text(),
                    angle.np
                ),
            );
        }
        if !has_control {
            has_control = true;
            control.ntp = u32::try_from(2 * angle.n.unsigned_abs() + 1).unwrap_or(u32::MAX);
            let n = i64::try_from(angle.n.unsigned_abs()).unwrap_or(i64::MAX);
            control.tap_min = angle.rho_alpha(-n).1;
            control.tap_max = angle.rho_alpha(n).1;
            if let Some(p) = angle.p {
                control.mode = TransformerControlMode::ActiveFlow;
                control.enabled = false;
                control.band_min = -p;
                control.band_max = -p;
            }
        }
        branch.extras.insert(
            "ucte_angle_regulation".into(),
            json!({
                "du": angle.du,
                "theta": angle.theta,
                "n": angle.n,
                "np": angle.np,
                "p": angle.p,
                "type": if angle.symmetrical { "SYMM" } else { "ASYM" },
            }),
        );
    }
    if has_control {
        branch.control = Some(control);
    }
}

/// Map the node type, the voltage reference, and the generation fields onto
/// the bus kind and one generator, applying the consistency rules PowSybl's
/// `UcteNode.fix` applies so both readers describe the same machine.
// The limit comparisons are the exact ones PowSybl's `UcteNode.fix` makes on
// the stated values, so an epsilon would change which records it reports.
#[expect(clippy::too_many_lines, clippy::float_cmp)]
fn map_node_kind_and_generation(
    reader: &mut Reader<'_>,
    node: &NodeRecord,
    bus: &mut Bus,
    generators: &mut Vec<Generator>,
    base_kv: f64,
) {
    let code = node.code.text();
    let span = Some((node.byte_start, node.byte_end));
    let mut regulating = matches!(node.type_code, Some(2 | 3));
    match node.type_code {
        Some(1) => reader.warn_at(
            &codes::READ_UCTE_VALUE_SUBSTITUTED,
            span,
            format!(
                "line {}: node {code:?} has type code 1 (Q and angle constant), which the balanced model has no bus type for; read as PQ",
                node.line
            ),
        ),
        None => reader.warn_at(
            &codes::READ_UCTE_VALUE_DEFAULTED,
            span,
            format!("line {}: node {code:?} states no type code; read as PQ", node.line),
        ),
        _ => {}
    }
    let is_generator = regulating
        || is_valid_value(node.active_generation)
        || is_valid_value(node.reactive_generation)
        || limits_declare_generator(node.min_p, node.max_p)
        || limits_declare_generator(node.min_q, node.max_q);
    if regulating
        && !node
            .voltage_reference
            .is_some_and(|reference| reference >= 0.0001)
    {
        reader.warn_at(
            &codes::READ_UCTE_VALUE_SUBSTITUTED,
            span,
            format!(
                "line {}: node {code:?} regulates voltage but states no voltage reference; read as PQ",
                node.line
            ),
        );
        regulating = false;
    }
    bus.kind = match (node.type_code, regulating) {
        (Some(3), true) => BusType::Ref,
        (Some(2), true) => BusType::Pv,
        _ => BusType::Pq,
    };
    if !is_generator {
        return;
    }
    let warn = |reader: &mut Reader<'_>, message: String| {
        reader.warn_at(
            &codes::READ_UCTE_VALUE_SUBSTITUTED,
            span,
            format!("line {}: node {code:?} {message}", node.line),
        );
    };
    // Active power, in the UCTE sign (an injection is negative).
    let mut p = node.active_generation.unwrap_or_else(|| {
        reader.warn_at(
            &codes::READ_UCTE_VALUE_DEFAULTED,
            span,
            format!(
                "line {}: node {code:?} hosts a generator with no active power; read as 0 MW",
                node.line
            ),
        );
        0.0
    });
    let mut min_p = node.min_p.unwrap_or(DEFAULT_POWER_LIMIT);
    let mut max_p = node.max_p.unwrap_or(-DEFAULT_POWER_LIMIT);
    if min_p < max_p {
        warn(
            reader,
            format!("active power limits {min_p} and {max_p} MW are inverted; swapped"),
        );
        std::mem::swap(&mut min_p, &mut max_p);
    }
    if p < max_p {
        warn(
            reader,
            format!(
                "active power {p} MW exceeds the maximum permissible generation {max_p} MW; the limit follows the value"
            ),
        );
        max_p = p;
    }
    if p != 0.0 && p > min_p {
        warn(
            reader,
            format!(
                "active power {p} MW is under the minimum permissible generation {min_p} MW; the limit follows the value"
            ),
        );
        min_p = p;
    }
    if min_p == 0.0 && max_p == 0.0 && p != 0.0 {
        warn(
            reader,
            "active power limits are both zero; read as the +/-9999 MW defaults".to_owned(),
        );
        min_p = DEFAULT_POWER_LIMIT;
        max_p = -DEFAULT_POWER_LIMIT;
    }
    // Reactive power.
    let mut q = node.reactive_generation;
    if !regulating && q.is_none() {
        reader.warn_at(
            &codes::READ_UCTE_VALUE_DEFAULTED,
            span,
            format!("line {}: node {code:?} hosts a generator that does not regulate voltage and states no reactive power; read as 0 MVAr", node.line),
        );
        q = Some(0.0);
    }
    let mut min_q = node.min_q.unwrap_or(DEFAULT_POWER_LIMIT);
    let mut max_q = node.max_q.unwrap_or(-DEFAULT_POWER_LIMIT);
    if min_q < max_q {
        warn(
            reader,
            format!("reactive power limits {min_q} and {max_q} MVAr are inverted; swapped"),
        );
        std::mem::swap(&mut min_q, &mut max_q);
    }
    if let Some(q) = q {
        if q < max_q {
            warn(
                reader,
                format!(
                    "reactive power {q} MVAr exceeds the maximum permissible generation {max_q} MVAr; the limit follows the value"
                ),
            );
            max_q = q;
        }
        if q > min_q {
            warn(
                reader,
                format!(
                    "reactive power {q} MVAr is under the minimum permissible generation {min_q} MVAr; the limit follows the value"
                ),
            );
            min_q = q;
        }
    }
    if min_q == max_q {
        warn(
            reader,
            format!(
                "reactive power limits are both {min_q} MVAr; read as the +/-9999 MVAr defaults"
            ),
        );
        min_q = DEFAULT_POWER_LIMIT;
        max_q = -DEFAULT_POWER_LIMIT;
    }
    if p.is_nan() {
        p = 0.0;
    }
    let mut generator = Generator::new(bus.id);
    generator.pg = -p;
    generator.qg = -q.unwrap_or(0.0);
    generator.pmin = -min_p;
    generator.pmax = -max_p;
    generator.qmin = -min_q;
    generator.qmax = -max_q;
    generator.mbase = BASE_MVA;
    generator.voltage_regulation_on = regulating;
    generator.vg = if regulating {
        node.voltage_reference.unwrap_or(base_kv) / base_kv
    } else {
        1.0
    };
    generator.energy_source = match node.plant_type {
        Some('H') => GeneratorEnergySource::Hydro,
        Some('N') => GeneratorEnergySource::Nuclear,
        Some('L' | 'C' | 'G' | 'O') => GeneratorEnergySource::Thermal,
        Some('W') => GeneratorEnergySource::Wind,
        _ => GeneratorEnergySource::Other,
    };
    generators.push(generator);
}

/// Both limits stated, not both zero, and different: the PowSybl rule for a
/// node record that declares a machine through its limits alone.
// Exact, as in PowSybl's `UcteNode.isGenerator`.
#[expect(clippy::float_cmp)]
fn limits_declare_generator(min: Option<f64>, max: Option<f64>) -> bool {
    matches!((min, max), (Some(min), Some(max)) if (min != 0.0 || max != 0.0) && min != max)
}

/// A one line description of an `##E` exchange record: the two ISO country
/// codes and the scheduled power when the row parses, else the raw text.
fn describe_exchange(record: &Record<'_>) -> String {
    match (
        record.field(0, 2),
        record.field(3, 5),
        record.number(6, 13, "exchange power").ok().flatten(),
    ) {
        (Some(from), Some(to), Some(power)) => format!("{from}-{to} {power} MW"),
        _ => format!("{:?}", record.text.trim_end()),
    }
}

/// The case date in the UCTE file name convention
/// `<yyyymmdd>_<HHMM>_<TY><w>_<cc><v>`, as `YYYY-MM-DDTHH:MM`.
fn case_date_from_name(stem: &str) -> Option<String> {
    let bytes = stem.as_bytes();
    if bytes.len() < 13 || bytes[8] != b'_' {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| {
        bytes[range.clone()]
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| &stem[range])
    };
    let (year, month, day, hour, minute) = (
        digits(0..4)?,
        digits(4..6)?,
        digits(6..8)?,
        digits(9..11)?,
        digits(11..13)?,
    );
    let in_range = |text: &str, low: u32, high: u32| {
        text.parse::<u32>().is_ok_and(|v| (low..=high).contains(&v))
    };
    (in_range(month, 1, 12)
        && in_range(day, 1, 31)
        && in_range(hour, 0, 23)
        && in_range(minute, 0, 59))
    .then(|| format!("{year}-{month}-{day}T{hour}:{minute}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_dates_come_from_the_file_name_convention() {
        assert_eq!(
            case_date_from_name("20170322_1844_SN3_FR2").as_deref(),
            Some("2017-03-22T18:44")
        );
        assert_eq!(case_date_from_name("elementName"), None);
        assert_eq!(case_date_from_name("20171399_1844_SN3_FR2"), None);
    }
}
