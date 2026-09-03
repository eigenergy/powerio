//! Parse and emit legacy GE PSLF `.epc` power flow cases.
//!
//! EPC files contain named data sections with colon separated record bodies.
//! The parser keeps raw physical lines plus token lists on both sides of each
//! colon, then maps the static power flow core into [`BalancedNetwork`]. Records outside
//! that model stay in retained source text and parse diagnostics. [`write_pslf`]
//! inverts the parser's column layout for cross format emission (same format
//! emission echoes the retained source).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use serde_json::{Number, Value};

use super::{TextEmission, sanitize_quoted, warn_extra_branch_rating_sets};
use crate::diagnostics::codes::EMIT_PSLF as F;
use crate::diagnostics::{Diagnostics, codes};
use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, Bus, BusId, BusType, Extras, Generator,
    GeneratorEnergySource, Hvdc, Impedance, Load, LoadVoltageModel, Shunt, SourceFormat,
    Transformer3W, Winding,
};
use crate::{Error, Result};

const FMT: &str = "PSLF .epc";

/// The double quote delimits an EPC name token, and the reader's tokenizer
/// toggles on it with no un-escaping, so an embedded quote would shift the record.
const NAME_FORBIDDEN: &[char] = &['"'];

/// Parse a PSLF `.epc` case into a [`BalancedNetwork`].
///
/// Parse diagnostics are available on the module returned by the `powerio`
/// facade's `parse` operation. This direct helper returns only the typed
/// network.
/// Parse retained source from the format hub.
pub(crate) fn parse_pslf_source(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let doc = parse_document(source, warnings);
    let base_mva = doc.base_mva(warnings);
    let name = doc.name(name_hint);
    let mut once = HashSet::new();

    let mut buses = Vec::new();
    let mut bus_schedule = HashMap::new();
    for rec in doc.records("bus data") {
        let bus = read_bus(rec)?;
        bus_schedule.insert(
            bus.id,
            BusSchedule {
                vm: bus.vm,
                vsched: num_at(&rec.rhs, 1, bus.vm, "bus vsched", rec)?,
                base_kv: bus.base_kv,
            },
        );
        buses.push(bus);
    }

    let mut loads = Vec::new();
    for rec in doc.records("load data") {
        loads.push(read_load(rec, warnings, &mut once)?);
    }

    let mut shunts = Vec::new();
    for rec in doc.records("shunt data") {
        shunts.push(read_shunt(rec, base_mva)?);
    }
    for rec in doc.records("svd data") {
        shunts.push(read_svd(rec, base_mva, warnings, &mut once)?);
    }

    let jump = doc.jump_threshold();
    let mut near_jump = 0usize;
    let mut branches = Vec::new();
    for rec in doc.records("branch data") {
        let branch = read_branch(rec)?;
        if let Some(threshold) = jump {
            if branch.x.abs() <= threshold {
                near_jump += 1;
            }
        }
        branches.push(branch);
    }
    if near_jump > 0 {
        warnings.push(
            &codes::READ_PSLF_VALUE_APPROXIMATED,
            format!("{near_jump} branch(es) have |x| at or below the PSLF jump threshold"),
        );
    }

    let mut transformers_3w = Vec::new();
    for rec in doc.records("transformer data") {
        match read_transformer(rec, base_mva)? {
            TransformerRecord::TwoWinding(branch) => branches.push(branch),
            TransformerRecord::ThreeWinding(t) => transformers_3w.push(t),
        }
    }
    if !transformers_3w.is_empty() {
        warnings.push(
            &codes::READ_PSLF_VALUE_DEFAULTED,
            "PSLF 3-winding transformer(s) mapped with the primary winding ratio/ratings; \
             secondary/tertiary winding ratios default to nominal",
        );
    }

    let mut generators = Vec::new();
    for rec in doc.records("generator data") {
        generators.push(read_generator(rec, &bus_schedule, warnings)?);
    }

    let dc_converters = read_dc_converters(&doc, warnings);
    let hvdc = read_dc_lines(&doc, &dc_converters, warnings);

    warn_unmodeled_sections(&doc, warnings);

    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: None,
        case_metadata: crate::network::CaseMetadata::default(),
        detailed_connectivity: None,
        buses: buses.into(),
        loads: loads.into(),
        shunts: shunts.into(),
        static_var_compensators: Vec::new().into(),
        branches: branches.into(),
        switches: Vec::new().into(),
        generators: generators.into(),
        storage: Vec::new().into(),
        hvdc: hvdc.into(),
        transformers_3w: transformers_3w.into(),
        areas: Vec::new().into(),
        solver: None,
        source_format: SourceFormat::Pslf,
    });
    net.check_references(FMT)?;
    Ok(net)
}

/// EPC source document: structural parse of the file before mapping to
/// [`BalancedNetwork`].
///
/// This intentionally keeps sections as raw records instead of making a PSLF
/// specific object model. The reader only maps the static power flow sections;
/// everything else remains in `source` and is surfaced through warnings.
#[derive(Debug)]
struct EpcDocument {
    title: Vec<String>,
    solution_parameters: Vec<String>,
    sections: BTreeMap<String, Section>,
}

impl EpcDocument {
    /// Choose the case name from the title block, falling back to the file stem.
    fn name(&self, name_hint: Option<&str>) -> String {
        self.title
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(|| name_hint.unwrap_or("case").to_string(), str::to_string)
    }

    /// Return records from a named EPC section, or an empty slice when absent.
    fn records(&self, section: &str) -> &[Record] {
        self.sections
            .get(section)
            .map_or(&[], |section| section.records.as_slice())
    }

    /// Read `sbase` from solution parameters, defaulting to 100 MVA.
    fn base_mva(&self, warnings: &mut Diagnostics) -> f64 {
        for line in &self.solution_parameters {
            let toks = tokens(line);
            if toks
                .first()
                .is_some_and(|tok| tok.eq_ignore_ascii_case("sbase"))
            {
                if let Some(base) = toks.get(1).and_then(|tok| tok.parse::<f64>().ok()) {
                    return base;
                }
            }
        }
        warnings.push(
            &codes::READ_PSLF_VALUE_DEFAULTED,
            "no PSLF sbase solution parameter found; defaulting baseMVA to 100",
        );
        100.0
    }

    /// Read the optional branch reactance jump threshold.
    fn jump_threshold(&self) -> Option<f64> {
        self.solution_parameters.iter().find_map(|line| {
            let toks = tokens(line);
            toks.first()
                .filter(|tok| tok.eq_ignore_ascii_case("jump"))
                .and_then(|_| toks.get(1))
                .and_then(|tok| tok.parse().ok())
        })
    }
}

/// One named `... data [count]` block.
///
/// `declared_count` is retained because count mismatches are useful evidence
/// when a variant section shape appears in a new EPC file.
#[derive(Debug)]
struct Section {
    declared_count: usize,
    header: String,
    records: Vec<Record>,
}

/// One logical EPC record assembled from one or more physical lines.
///
/// `lhs` is the identity side before `:`, and `rhs` is the numeric/status side.
/// Raw physical lines stay attached so conversion warnings and extras can point
/// back to the original text.
#[derive(Debug)]
struct Record {
    line_no: usize,
    raw: Vec<String>,
    lhs: Vec<String>,
    rhs: Vec<String>,
}

/// Parse EPC's section grammar without interpreting electrical fields.
///
/// The general structure is stable across observed files: free text blocks end
/// with `!`, data sections declare a count in brackets, and `/` continues a
/// record onto the next physical line. The parser stops early at the next
/// section header and reports the mismatch instead of consuming unrelated text.
#[expect(clippy::too_many_lines)]
fn parse_document(content: &str, warnings: &mut Diagnostics) -> EpcDocument {
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0usize;
    let mut title = Vec::new();
    let mut solution_parameters = Vec::new();
    let mut sections = BTreeMap::new();
    let mut end_seen = false;

    while i < lines.len() {
        let raw = lines[i].trim_end_matches('\r');
        let stripped = raw.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            i += 1;
            continue;
        }
        if stripped.eq_ignore_ascii_case("end") {
            end_seen = true;
            break;
        }

        let lower = stripped.to_ascii_lowercase();
        if matches!(lower.as_str(), "title" | "comments" | "solution parameters") {
            i += 1;
            let mut block = Vec::new();
            while i < lines.len() && lines[i].trim() != "!" {
                block.push(lines[i].trim_end_matches('\r').to_string());
                i += 1;
            }
            if i < lines.len() && lines[i].trim() == "!" {
                i += 1;
            }
            match lower.as_str() {
                "title" => title = block,
                "solution parameters" => solution_parameters = block,
                _ => {}
            }
            continue;
        }

        let Some((name, count, header)) = parse_section_header(stripped) else {
            warnings.push(
                &codes::READ_PSLF_RECORD_DROPPED,
                format!("line {} ignored outside a PSLF data section", i + 1),
            );
            i += 1;
            continue;
        };
        i += 1;

        let mut records = Vec::new();
        while records.len() < count && i < lines.len() {
            if lines[i].trim().is_empty() {
                i += 1;
                continue;
            }
            let next = lines[i].trim();
            if parse_section_header(next).is_some() || next.eq_ignore_ascii_case("end") {
                break;
            }

            let line_no = i + 1;
            let mut raw_lines = Vec::new();
            loop {
                let (line, continued) = clean_line(lines[i]);
                if !line.trim().is_empty() {
                    raw_lines.push(line);
                }
                i += 1;
                if !continued || i >= lines.len() {
                    break;
                }
            }
            let (lhs, rhs) = split_record(&raw_lines);
            records.push(Record {
                line_no,
                raw: raw_lines,
                lhs,
                rhs,
            });
        }

        if records.len() != count {
            warnings.push(
                &codes::READ_PSLF_SOURCE_MALFORMED,
                format!("{}: declared {count}, parsed {}", name, records.len()),
            );
        }
        if sections
            .insert(
                name.clone(),
                Section {
                    declared_count: count,
                    header,
                    records,
                },
            )
            .is_some()
        {
            warnings.push(
                &codes::READ_PSLF_SOURCE_MALFORMED,
                format!("{name}: duplicate section replaced earlier records"),
            );
        }
    }

    if !end_seen {
        warnings.push(
            &codes::READ_PSLF_SOURCE_MALFORMED,
            "PSLF file has no end marker",
        );
    }

    EpcDocument {
        title,
        solution_parameters,
        sections,
    }
}

/// Parse a `name data [count] ...` section header.
///
/// The returned name is lower case so callers can use stable section keys
/// across files that vary capitalization.
fn parse_section_header(line: &str) -> Option<(String, usize, String)> {
    let lower = line.to_ascii_lowercase();
    let data_at = lower.find(" data")?;
    let open = line[data_at + 5..].find('[')? + data_at + 5;
    let close = line[open + 1..].find(']')? + open + 1;
    let name = line[..data_at + 5].trim().to_ascii_lowercase();
    let count = line[open + 1..close].trim().parse().ok()?;
    let header = line[close + 1..].trim_end().to_string();
    Some((name, count, header))
}

/// Strip line endings and detect EPC continuation lines.
///
/// A trailing `/` outside a quoted string joins the next physical line into
/// the same logical record.
fn clean_line(raw: &str) -> (String, bool) {
    let raw = raw.trim_end_matches('\r');
    let trimmed = raw.trim_end();
    let continued = ends_with_unquoted_slash(trimmed);
    if continued {
        let without = &trimmed[..trimmed.len() - 1];
        (without.trim_end().to_string(), true)
    } else {
        (raw.to_string(), false)
    }
}

fn ends_with_unquoted_slash(line: &str) -> bool {
    if !line.ends_with('/') {
        return false;
    }
    let before = &line[..line.len() - 1];
    let mut quoted = false;
    let mut chars = before.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            if quoted && chars.peek() == Some(&'"') {
                chars.next();
            } else {
                quoted = !quoted;
            }
        }
    }
    !quoted
}

/// Tokenize a logical record and split it into identity and value sides.
fn split_record(raw_lines: &[String]) -> (Vec<String>, Vec<String>) {
    let toks = tokens(&raw_lines.join(" "));
    split_tokens(toks)
}

/// Split already tokenized fields at the first unquoted `:`.
fn split_tokens(toks: Vec<String>) -> (Vec<String>, Vec<String>) {
    if let Some(colon) = toks.iter().position(|tok| tok == ":") {
        (toks[..colon].to_vec(), toks[colon + 1..].to_vec())
    } else {
        (toks, Vec::new())
    }
}

/// Tokenize an EPC line while preserving quoted strings as one token.
///
/// Double quotes inside a quoted string are escaped by doubling them.
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if quoted && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = !quoted;
                    if !quoted {
                        out.push(std::mem::take(&mut cur));
                    }
                }
            }
            ':' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(":".into());
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Return the right side tokens for one physical line in a multi-line record.
fn line_rhs(rec: &Record, line: usize) -> Vec<String> {
    rec.raw
        .get(line)
        .map(|line| split_tokens(tokens(line)).1)
        .unwrap_or_default()
}

/// Return all tokens for one physical line in a multi-line record.
fn line_tokens(rec: &Record, line: usize) -> Vec<String> {
    rec.raw.get(line).map_or_else(Vec::new, |line| tokens(line))
}

/// Map one `bus data` record into a [`Bus`].
fn read_bus(rec: &Record) -> Result<Bus> {
    let id = BusId(req_id(&rec.lhs, 0, "bus id", rec)?);
    // An empty token (what the writer emits for an unnamed bus) is no name.
    let name = rec
        .lhs
        .get(1)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    Ok(Bus {
        id,
        kind: pslf_bus_type(int_at(&rec.rhs, 0, 1, "bus type", rec)?),
        vm: num_at(&rec.rhs, 2, 1.0, "bus voltage", rec)?,
        va: num_at(&rec.rhs, 3, 0.0, "bus angle", rec)?,
        base_kv: num_at(&rec.lhs, 2, 0.0, "bus nominal kV", rec)?,
        vmax: num_at(&rec.rhs, 6, 1.1, "bus vmax", rec)?,
        vmin: num_at(&rec.rhs, 7, 0.9, "bus vmin", rec)?,
        evhi: None,
        evlo: None,
        area: id_at(&rec.rhs, 4, 1, "bus area", rec)?,
        zone: id_at(&rec.rhs, 5, 1, "bus zone", rec)?,
        name,
        uid: None,
        location: None,
        extras: extras(rec, 3, 21),
    })
}

/// Convert PSLF bus type codes to the format neutral bus type enum.
fn pslf_bus_type(code: i64) -> BusType {
    match code {
        0 => BusType::Ref,
        2 => BusType::Pv,
        4 => BusType::Isolated,
        _ => BusType::Pq,
    }
}

/// Map one `branch data` record into a line [`Branch`].
fn read_branch(rec: &Record) -> Result<Branch> {
    let mut extras = extras(rec, 9, 10);
    // `1` is the id the writer allocates and the section it emits when none is
    // retained, so those tokens restate the default and are not kept.
    if let Some(circuit) = rec.lhs.get(6).filter(|c| c.trim() != "1") {
        extras.insert("pslf_circuit".into(), Value::String(circuit.clone()));
    }
    if let Some(section) = rec.lhs.get(7).filter(|t| t.trim() != "1") {
        extras.insert("pslf_section_id".into(), string_or_number(section));
    }
    Ok(Branch {
        name: None,
        from: BusId(req_id(&rec.lhs, 0, "branch from bus", rec)?),
        to: BusId(req_id(&rec.lhs, 3, "branch to bus", rec)?),
        r: num_at(&rec.rhs, 1, 0.0, "branch r", rec)?,
        x: num_at(&rec.rhs, 2, 0.0, "branch x", rec)?,
        b: num_at(&rec.rhs, 3, 0.0, "branch b", rec)?,
        charging: None,
        rate_a: num_at(&rec.rhs, 4, 0.0, "branch rate1", rec)?,
        rate_b: num_at(&rec.rhs, 5, 0.0, "branch rate2", rec)?,
        rate_c: num_at(&rec.rhs, 6, 0.0, "branch rate3", rec)?,
        rating_sets: Vec::new(),
        current_ratings: None,
        tap: 0.0,
        shift: 0.0,
        in_service: on_at(&rec.rhs, 0, true, "branch status", rec)?,
        angmin: -360.0,
        angmax: 360.0,
        control: None,
        solution: None,
        uid: None,
        route: None,
        extras,
    })
}

/// One mapped `transformer data` record: a 2-winding becomes a [`Branch`], a
/// 3-winding becomes a [`Transformer3W`].
// The 3-winding variant is the larger; boxing it to equalize the variants would
// add an allocation per record for no real benefit at this size.
#[allow(clippy::large_enum_variant)]
enum TransformerRecord {
    TwoWinding(Branch),
    ThreeWinding(Transformer3W),
}

/// Map one `transformer data` record. A tertiary winding (a nonzero tertiary bus
/// or any primary-tertiary / secondary-tertiary impedance) makes it a
/// [`Transformer3W`]; otherwise it is a two-winding [`Branch`].
///
/// The `.epc` record carries the three pairwise impedances and the primary
/// winding's ratio/ratings; the secondary and tertiary winding ratios are not
/// represented at these column positions, so they default to nominal.
#[allow(clippy::too_many_lines)]
fn read_transformer(rec: &Record, base_mva: f64) -> Result<TransformerRecord> {
    let rhs1 = line_rhs(rec, 0);
    let line2 = line_tokens(rec, 1);
    let tertiary = id_at(&rhs1, 9, 0, "transformer tertiary bus", rec)?;
    let pt_r = num_at(&rhs1, 17, 0.0, "transformer pt_r", rec)?;
    let pt_x = num_at(&rhs1, 18, 0.0, "transformer pt_x", rec)?;
    let ts_r = num_at(&rhs1, 19, 0.0, "transformer ts_r", rec)?;
    let ts_x = num_at(&rhs1, 20, 0.0, "transformer ts_x", rec)?;
    let from = BusId(req_id(&rec.lhs, 0, "transformer from bus", rec)?);
    let to = BusId(req_id(&rec.lhs, 3, "transformer to bus", rec)?);
    let r = num_at(&rhs1, 15, 0.0, "transformer r", rec)?;
    let x = num_at(&rhs1, 16, 0.0, "transformer x", rec)?;
    let tbase = num_at(&rhs1, 14, 0.0, "transformer base", rec)?;
    let tap = num_at(&line2, 16, 1.0, "transformer tap", rec)?;
    let shift = num_at(&line2, 10, 0.0, "transformer shift", rec)?;
    let rate_a = num_at(&line2, 6, 0.0, "transformer rate1", rec)?;
    let rate_b = num_at(&line2, 7, 0.0, "transformer rate2", rec)?;
    let rate_c = num_at(&line2, 8, 0.0, "transformer rate3", rec)?;
    let in_service = on_at(&rhs1, 0, true, "transformer status", rec)?;
    let circuit = rec.lhs.get(6).cloned();
    let name = rec
        .lhs
        .get(8)
        .filter(|n| !n.trim().is_empty())
        .map(|n| n.trim().to_string());

    if tertiary != 0 || pt_r != 0.0 || pt_x != 0.0 || ts_r != 0.0 || ts_x != 0.0 {
        let mut extras = transformer_extras(rec, &[rate_a, rate_b, rate_c, shift, tap]);
        if let Some(c) = circuit.filter(|c| c.trim() != "1") {
            extras.insert("pslf_circuit".into(), Value::String(c));
        }
        let nominal = |bus| Winding {
            bus,
            tap: 1.0,
            shift: 0.0,
            nominal_kv: 0.0,
            rate_a: 0.0,
            rate_b: 0.0,
            rate_c: 0.0,
            control: None,
        };
        let imp = |r, x| Impedance {
            r,
            x,
            base_mva: tbase,
        };
        let t3 = Transformer3W {
            windings: [
                Winding {
                    bus: from,
                    tap: if tap == 0.0 { 1.0 } else { tap },
                    shift,
                    nominal_kv: 0.0,
                    rate_a,
                    rate_b,
                    rate_c,
                    control: None,
                },
                nominal(to),
                nominal(BusId(tertiary)),
            ],
            // z12 = primary-secondary, z23 = secondary-tertiary, z31 = tertiary-primary.
            z: [imp(r, x), imp(ts_r, ts_x), imp(pt_r, pt_x)],
            star_vm: 1.0,
            star_va: 0.0,
            mag_g: 0.0,
            mag_b: 0.0,
            in_service,
            name,
            uid: None,
            extras,
        };
        return Ok(TransformerRecord::ThreeWinding(t3));
    }

    let mut extras = transformer_extras(rec, &[rate_a, rate_b, rate_c, shift, tap]);
    if let Some(c) = circuit.filter(|c| c.trim() != "1") {
        extras.insert("pslf_circuit".into(), Value::String(c));
    }
    // Exact comparison on purpose: the writer emits the case base verbatim
    // when no tbase is retained, so only a bit-different stated base is data.
    #[allow(clippy::float_cmp)]
    if tbase != base_mva {
        extras.insert("pslf_tbase".into(), number_value(tbase));
    }
    Ok(TransformerRecord::TwoWinding(Branch {
        // `lhs[8]` is the transformer record type (`xfmr`), not a component
        // name. The two winding record has no separate name field here.
        name: None,
        from,
        to,
        r,
        x,
        b: 0.0,
        charging: None,
        rate_a,
        rate_b,
        rate_c,
        rating_sets: Vec::new(),
        current_ratings: None,
        tap: if tap == 0.0 { 1.0 } else { tap },
        shift,
        in_service,
        angmin: -360.0,
        angmax: 360.0,
        control: None,
        solution: None,
        uid: None,
        route: None,
        extras,
    }))
}

/// What a `bus data` record says about one bus's voltage, for the generator
/// rows that key off it: the solved magnitude, the scheduled magnitude, and the
/// nominal kV that converts between per unit and the `reg_kv` column.
#[derive(Clone, Copy)]
struct BusSchedule {
    vm: f64,
    vsched: f64,
    base_kv: f64,
}

/// Map one `generator data` record.
///
/// EPC states generator voltage setpoints as controlled kV (`reg_kv`), which a
/// bus with no nominal kV cannot express. The bus's own `vsched` column is the
/// scheduled magnitude in per unit and needs no base, so it carries the
/// setpoint for those buses; a row with neither falls back to the solved bus
/// voltage.
fn read_generator(
    rec: &Record,
    bus_schedule: &HashMap<BusId, BusSchedule>,
    warnings: &mut Diagnostics,
) -> Result<Generator> {
    let bus = BusId(req_id(&rec.lhs, 0, "generator bus", rec)?);
    let schedule = bus_schedule.get(&bus).copied().unwrap_or(BusSchedule {
        vm: 1.0,
        vsched: 1.0,
        base_kv: 0.0,
    });
    let reg_kv = num_at(&rec.rhs, 3, 0.0, "generator reg_kv", rec)?;
    let vg = if schedule.base_kv > 0.0 {
        // `reg_kv` is the setpoint whenever the bus states a base to divide by.
        // A row leaving it zero states no setpoint, so the solved bus voltage
        // stands in; `vsched` is not consulted here, because on such a bus it
        // is the bus's own schedule and not this generator's.
        if reg_kv > 0.0 {
            reg_kv / schedule.base_kv
        } else {
            schedule.vm
        }
    } else {
        // No base kV to divide by: `vsched` is the only per unit column that
        // can carry a setpoint, and the solved voltage is the last resort.
        let (source, vg) = if schedule.vsched > 0.0 {
            ("the bus vsched setpoint", schedule.vsched)
        } else {
            ("bus voltage", schedule.vm)
        };
        if reg_kv > 0.0 {
            warnings.push(&codes::READ_PSLF_VALUE_DEFAULTED, format!(
                "PSLF generator at bus {bus}: reg_kv present but bus base kV is missing; used {source}"
            ));
        }
        vg
    };
    Ok(Generator {
        bus,
        energy_source: GeneratorEnergySource::default(),
        pg: num_at(&rec.rhs, 8, 0.0, "generator pgen", rec)?,
        qg: num_at(&rec.rhs, 11, 0.0, "generator qgen", rec)?,
        pmax: num_at(&rec.rhs, 9, 0.0, "generator pmax", rec)?,
        pmin: num_at(&rec.rhs, 10, 0.0, "generator pmin", rec)?,
        qmax: num_at(&rec.rhs, 12, 0.0, "generator qmax", rec)?,
        qmin: num_at(&rec.rhs, 13, 0.0, "generator qmin", rec)?,
        vg,
        mbase: num_at(&rec.rhs, 14, 100.0, "generator mbase", rec)?,
        in_service: on_at(&rec.rhs, 0, true, "generator status", rec)?,
        cost: None,
        caps: Default::default(),
        voltage_regulation_on: true,
        regulating_terminal: None,
        regulated_bus: None,
        active_power_control: None,
        uid: None,
    })
}

/// Map one `load data` record.
///
/// Constant current and impedance components are folded into total P/Q for
/// static load aggregation and preserved in the typed voltage model.
fn read_load(
    rec: &Record,
    warnings: &mut Diagnostics,
    once: &mut HashSet<&'static str>,
) -> Result<Load> {
    let p_const = num_at(&rec.rhs, 1, 0.0, "load mw", rec)?;
    let q_const = num_at(&rec.rhs, 2, 0.0, "load mvar", rec)?;
    let p_i = num_at(&rec.rhs, 3, 0.0, "load mw_i", rec)?;
    let q_i = num_at(&rec.rhs, 4, 0.0, "load mvar_i", rec)?;
    let p_z = num_at(&rec.rhs, 5, 0.0, "load mw_z", rec)?;
    let q_z = num_at(&rec.rhs, 6, 0.0, "load mvar_z", rec)?;
    let has_zip_components = (p_i, q_i, p_z, q_z) != (0.0, 0.0, 0.0, 0.0);
    if has_zip_components && once.insert("zip_load") {
        // Fold components into P/Q so matrix builders see the total demand that
        // the solved power flow used. The split stays typed for richer writers.
        warnings.push(&codes::READ_PSLF_RETAINED_SOURCE_ONLY,
            "PSLF ZIP load components folded into BalancedNetwork load p/q; component fields retained in the typed load voltage model"
                ,
        );
    }
    let mut extras = extras(rec, 5, 20);
    capture_device_id(&mut extras, &rec.lhs);
    // With zero I/Z terms the record states the constant-power pair alone,
    // which is exactly the typed p/q the writer falls back to; the six
    // components say more only when the split distributes.
    if has_zip_components {
        extras.insert("pslf_mw".into(), number_value(p_const));
        extras.insert("pslf_mvar".into(), number_value(q_const));
        extras.insert("pslf_mw_i".into(), number_value(p_i));
        extras.insert("pslf_mvar_i".into(), number_value(q_i));
        extras.insert("pslf_mw_z".into(), number_value(p_z));
        extras.insert("pslf_mvar_z".into(), number_value(q_z));
    }
    Ok(Load {
        bus: BusId(req_id(&rec.lhs, 0, "load bus", rec)?),
        p: p_const + p_i + p_z,
        q: q_const + q_i + q_z,
        voltage_model: has_zip_components.then_some(LoadVoltageModel::Zip {
            p_constant_power: p_const,
            q_constant_power: q_const,
            p_constant_current: p_i,
            q_constant_current: q_i,
            p_constant_impedance: p_z,
            q_constant_impedance: q_z,
            v_nom: None,
            load_type: None,
            scaling: None,
        }),
        in_service: on_at(&rec.rhs, 0, true, "load status", rec)?,
        uid: None,
        extras,
    })
}

/// Map one fixed `shunt data` record and convert per unit G/B to MW/MVAr.
fn read_shunt(rec: &Record, base_mva: f64) -> Result<Shunt> {
    let g_pu = num_at(&rec.rhs, 3, 0.0, "shunt pu_mw", rec)?;
    let b_pu = num_at(&rec.rhs, 4, 0.0, "shunt pu_mvar", rec)?;
    let mut extras = extras(rec, 10, 29);
    capture_device_id(&mut extras, &rec.lhs);
    // The writer divides MW back by the base when no pu extra is retained;
    // keep the stated token only when that round trip is not bit-exact.
    #[allow(clippy::float_cmp)]
    for (key, pu) in [("pslf_pu_mw", g_pu), ("pslf_pu_mvar", b_pu)] {
        if safe_div(pu * base_mva, base_mva) != pu {
            extras.insert(key.into(), number_value(pu));
        }
    }
    Ok(Shunt {
        bus: BusId(req_id(&rec.lhs, 0, "shunt bus", rec)?),
        g: g_pu * base_mva,
        b: b_pu * base_mva,
        in_service: on_at(&rec.rhs, 0, true, "shunt status", rec)?,
        section_count: None,
        control: None,
        uid: None,
        extras,
    })
}

/// Map one `svd data` record as a fixed shunt at its initial G/B value.
///
/// The control target, limits, and switching fields stay in extras until
/// `BalancedNetwork` grows a typed controlled shunt model.
fn read_svd(
    rec: &Record,
    base_mva: f64,
    warnings: &mut Diagnostics,
    once: &mut HashSet<&'static str>,
) -> Result<Shunt> {
    if once.insert("svd") {
        warnings.push(&codes::READ_PSLF_RETAINED_SOURCE_ONLY,
            "PSLF controlled shunts (svd data) reduced to fixed shunts at initial g/b; control fields retained in extras"
                ,
        );
    }
    let g_pu = num_at(&rec.rhs, 7, 0.0, "svd g", rec)?;
    let b_pu = num_at(&rec.rhs, 8, 0.0, "svd b", rec)?;
    let mut extras = extras(rec, 5, 30);
    capture_device_id(&mut extras, &rec.lhs);
    extras.insert("pslf_device".into(), Value::String("svd".into()));
    extras.insert("pslf_pu_g".into(), number_value(g_pu));
    extras.insert("pslf_pu_b".into(), number_value(b_pu));
    Ok(Shunt {
        bus: BusId(req_id(&rec.lhs, 0, "svd bus", rec)?),
        g: g_pu * base_mva,
        b: b_pu * base_mva,
        in_service: on_at(&rec.rhs, 0, true, "svd status", rec)?,
        section_count: None,
        control: None,
        uid: None,
        extras,
    })
}

/// Converter side of a PSLF DC line.
///
/// EPC stores AC converter rows separately from the DC line row. This holds the
/// AC terminal and setpoints until the line join happens.
#[derive(Clone)]
struct DcConverter {
    ac_bus: BusId,
    dc_bus: usize,
    in_service: bool,
    p: f64,
    q: f64,
    /// Whether the record stated converter fields beyond the ones mapped here.
    states_detail: bool,
    extras: Extras,
}

/// Whether a DC record states numeric data beyond the values the reader maps.
///
/// `mapped` are the values read out of the record; every other nonzero number
/// on the rhs is converter or control data (firing angles, transformer taps, a
/// DC voltage schedule) the balanced model does not carry, kept only in extras.
/// A zero states nothing: EPC reads a zero field as absent (`reg_kv` and
/// `rate1` above lean on the same convention), and the writer below emits
/// exactly that shape, so a powerio-written file reads back without the
/// warning while a real GE export still reports what stays behind.
///
/// Counting rather than indexing keeps the test independent of how a producer
/// splits the record across continuation lines: `rhs` is the joined numeric
/// side, and `mapped` is a subset of it.
fn dc_states_detail(rhs: &[String], mapped: &[f64]) -> bool {
    let stated = rhs
        .iter()
        .filter(|tok| tok.parse::<f64>().is_ok_and(|v| v != 0.0))
        .count();
    let consumed = mapped.iter().filter(|v| **v != 0.0).count();
    stated > consumed
}

/// Read all `dc converter data` rows into a DC bus keyed map.
///
/// Malformed converter rows become warnings so unrelated AC data in the same
/// file can still be read.
fn read_dc_converters(
    doc: &EpcDocument,
    warnings: &mut Diagnostics,
) -> HashMap<usize, DcConverter> {
    let mut out = HashMap::new();
    for rec in doc.records("dc converter data") {
        let parsed = (|| -> Result<DcConverter> {
            let l2 = line_tokens(rec, 1);
            let extras = extras(rec, 8, 15);
            let in_service = on_at(&rec.rhs, 0, true, "dc converter status", rec)?;
            let p = num_at(&l2, 2, 0.0, "dc converter p", rec)?;
            let q = num_at(&l2, 3, 0.0, "dc converter q", rec)?;
            Ok(DcConverter {
                ac_bus: BusId(req_id(&rec.lhs, 0, "dc converter AC bus", rec)?),
                dc_bus: req_id(&rec.lhs, 3, "dc converter DC bus", rec)?,
                in_service,
                p,
                q,
                states_detail: dc_states_detail(
                    &rec.rhs,
                    &[f64::from(i32::from(in_service)), p, q],
                ),
                extras,
            })
        })();
        match parsed {
            Ok(conv) => {
                out.insert(conv.dc_bus, conv);
            }
            Err(err) => warnings.push(
                &codes::READ_PSLF_RECORD_DROPPED,
                format!("dc converter at line {} not mapped: {err}", rec.line_no),
            ),
        }
    }
    out
}

/// Map two-terminal DC lines through their converter rows.
///
/// EPC separates the DC line from each AC converter. `BalancedNetwork::Hvdc` needs AC
/// terminal buses and setpoints on one row, so this joins by DC bus id and
/// retains converter extras under the HVDC record.
fn read_dc_lines(
    doc: &EpcDocument,
    converters: &HashMap<usize, DcConverter>,
    warnings: &mut Diagnostics,
) -> Vec<Hvdc> {
    let mut out = Vec::new();
    for rec in doc.records("dc line data") {
        let parsed = (|| -> Result<(bool, Hvdc)> {
            let from_dc = req_id(&rec.lhs, 0, "dc line from bus", rec)?;
            let to_dc = req_id(&rec.lhs, 3, "dc line to bus", rec)?;
            let from = converters.get(&from_dc).ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!("dc line references DC bus {from_dc} with no converter"),
            })?;
            let to = converters.get(&to_dc).ok_or_else(|| Error::FormatRead {
                format: FMT,
                message: format!("dc line references DC bus {to_dc} with no converter"),
            })?;
            let in_service = on_at(&rec.rhs, 0, true, "dc line status", rec)?;
            let rate = num_at(&rec.rhs, 6, 0.0, "dc line rate1", rec)?;
            let pmax = if rate > 0.0 {
                rate
            } else {
                from.p.abs().max(to.p.abs())
            };
            let states_detail = from.states_detail
                || to.states_detail
                || dc_states_detail(&rec.rhs, &[f64::from(i32::from(in_service)), rate]);
            let mut extras = extras(rec, 8, 20);
            // Converter extras ride under the joined HVDC record, but only
            // when a converter actually stated tokens beyond the mapped ones.
            for (key, conv) in [("pslf_from_converter", from), ("pslf_to_converter", to)] {
                if !conv.extras.is_empty() {
                    extras.insert(
                        key.into(),
                        Value::Object(conv.extras.clone().into_iter().collect()),
                    );
                }
            }
            Ok((
                states_detail,
                Hvdc {
                    from: from.ac_bus,
                    to: to.ac_bus,
                    in_service: in_service && from.in_service && to.in_service,
                    pf: from.p,
                    pt: to.p,
                    qf: from.q,
                    qt: to.q,
                    vf: 1.0,
                    vt: 1.0,
                    pmin: -pmax,
                    pmax,
                    qminf: from.q.min(0.0),
                    qmaxf: from.q.max(0.0),
                    qmint: to.q.min(0.0),
                    qmaxt: to.q.max(0.0),
                    loss0: 0.0,
                    loss1: 0.0,
                    resistance_ohm: None,
                    nominal_voltage_kv: None,
                    converters_mode: None,
                    converter1: None,
                    converter2: None,
                    cost: None,
                    uid: None,
                    extras,
                },
            ))
        })();
        match parsed {
            // The record and its two converters carry the warning only when one
            // of them stated data the mapping left behind; see
            // [`dc_states_detail`].
            Ok((states_detail, line)) => {
                if states_detail {
                    warnings.push(&codes::READ_PSLF_RETAINED_SOURCE_ONLY,
                        "PSLF DC line/converter data mapped to BalancedNetwork HVDC with unsupported control fields retained in extras"
                            ,
                    );
                }
                out.push(line);
            }
            Err(err) => warnings.push(
                &codes::READ_PSLF_RECORD_DROPPED,
                format!("dc line at line {} not mapped: {err}", rec.line_no),
            ),
        }
    }
    out
}

/// Report nonempty EPC sections that are retained as source text only.
fn warn_unmodeled_sections(doc: &EpcDocument, warnings: &mut Diagnostics) {
    const MODELED: &[&str] = &[
        "bus data",
        "branch data",
        "transformer data",
        "generator data",
        "load data",
        "shunt data",
        "svd data",
        "dc line data",
        "dc converter data",
    ];
    for (name, section) in &doc.sections {
        if section.declared_count > 0 && !MODELED.contains(&name.as_str()) {
            warnings.push(
                &codes::READ_PSLF_RETAINED_SOURCE_ONLY,
                format!(
                    "{name}: {} record(s) retained in source text only ({})",
                    section.declared_count, section.header
                ),
            );
        }
    }
}

/// Common extras for mapped EPC rows.
///
/// The `used_*` bounds are the fields consumed by the typed reader. Only the
/// tokens beyond them are retained: the consumed fields live in the model and
/// the writer regenerates the record from it, so a raw echo, the line number,
/// and the section name would restate provenance the rewrite re-derives (and
/// every cross-format hop would then warn about dropping a restatement).
fn extras(rec: &Record, used_lhs: usize, used_rhs: usize) -> Extras {
    let mut extras = Extras::new();
    if rec.lhs.len() > used_lhs {
        extras.insert(
            "pslf_lhs_extra".into(),
            string_array(rec.lhs[used_lhs..].iter().cloned()),
        );
    }
    if rec.rhs.len() > used_rhs {
        extras.insert(
            "pslf_rhs_extra".into(),
            string_array(rec.rhs[used_rhs..].iter().cloned()),
        );
    }
    extras
}

/// Extras for a transformer record. The first-line rhs is consumed through
/// index 21 and the type token (`xfmr`/`xf3`, lhs index 8) is re-derived from
/// the impedance shape on write. The second physical line is consumed by
/// position (ratings 6-8, shift 10, tap 16), so its retained tail is dropped
/// when it states no more nonzero tokens than those mapped fields — the same
/// stated-versus-consumed rule the dc records use.
fn transformer_extras(rec: &Record, mapped_line2: &[f64]) -> Extras {
    let mut extras = extras(rec, 9, 21);
    let tail = rec.rhs.get(21..).unwrap_or_default();
    if !dc_states_detail(tail, mapped_line2) {
        extras.remove("pslf_rhs_extra");
    }
    extras
}

/// Capture a load/shunt/svd record's id (lhs token 3) into `extras["id"]` — the
/// key the PSS/E reader uses — so the id survives cross-format writes and
/// parallel devices on a bus stay distinguishable.
fn capture_device_id(extras: &mut Extras, lhs: &[String]) {
    // `1` is the positional default the writer's allocator re-derives, so it
    // restates nothing; parallel devices keep their explicit non-default ids.
    if let Some(id) = lhs
        .get(3)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "1")
    {
        extras.insert("id".into(), Value::String(id.to_string()));
    }
}

/// Convert strings to a JSON array for `extras`.
fn string_array(values: impl IntoIterator<Item = String>) -> Value {
    Value::Array(values.into_iter().map(Value::String).collect())
}

/// Preserve an EPC token as a number when it parses, otherwise as a string.
fn string_or_number(token: &str) -> Value {
    token
        .parse::<f64>()
        .ok()
        .map_or_else(|| Value::String(token.to_string()), number_value)
}

/// Convert a finite f64 to JSON, using null for nonfinite values.
fn number_value(value: f64) -> Value {
    Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// Read an optional floating point field with a default for omitted values.
fn num_at(tokens: &[String], i: usize, default: f64, field: &str, rec: &Record) -> Result<f64> {
    match tokens.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(tok) => tok.parse().map_err(|_| bad_field(field, i, tok, rec)),
    }
}

/// Read an optional integer field with a default for omitted values.
fn int_at(tokens: &[String], i: usize, default: i64, field: &str, rec: &Record) -> Result<i64> {
    match tokens.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(tok) => tok.parse().map_err(|_| bad_field(field, i, tok, rec)),
    }
}

/// Read an optional nonnegative numeric identifier.
fn id_at(tokens: &[String], i: usize, default: usize, field: &str, rec: &Record) -> Result<usize> {
    match tokens.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(tok) => parse_id(tok).ok_or_else(|| bad_field(field, i, tok, rec)),
    }
}

/// Read a required nonnegative numeric identifier.
fn req_id(tokens: &[String], i: usize, field: &str, rec: &Record) -> Result<usize> {
    match tokens.get(i) {
        Some(tok) => parse_id(tok).ok_or_else(|| bad_field(field, i, tok, rec)),
        None => Err(Error::FormatRead {
            format: FMT,
            message: format!("{field} missing at line {}", rec.line_no),
        }),
    }
}

/// Parse PSLF numeric identifiers, including integer-valued floating text.
/// Fractional values are refused here; the range bound is the shared
/// [`crate::format::id_from_f64`] policy.
fn parse_id(tok: &str) -> Option<usize> {
    if let Ok(value) = tok.parse::<usize>() {
        return (value <= BusId::MAX.0).then_some(value);
    }
    let value = tok.parse::<f64>().ok()?;
    if value.fract() != 0.0 {
        return None;
    }
    crate::format::id_from_f64(value, "id").ok()
}

/// Read a numeric status field as an in service boolean.
fn on_at(tokens: &[String], i: usize, default: bool, field: &str, rec: &Record) -> Result<bool> {
    Ok(num_at(tokens, i, if default { 1.0 } else { 0.0 }, field, rec)? != 0.0)
}

/// Build a field-level parse error with the source line number.
fn bad_field(field: &str, i: usize, tok: &str, rec: &Record) -> Error {
    Error::FormatRead {
        format: FMT,
        message: format!(
            "{field} field {i} value {tok:?} is invalid at line {}",
            rec.line_no
        ),
    }
}

// ---- Writer -----------------------------------------------------------------

/// Per-bus identity the EPC `lhs` carries on every element record.
#[derive(Clone, Copy)]
struct BusRef<'a> {
    name: &'a str,
    base_kv: f64,
    area: usize,
    zone: usize,
}

/// Serialize `net` to PSLF `.epc` text.
///
/// The inverse of the reader's column layout: it emits the same colon separated
/// `lhs : rhs` records, so a `.epc` -> [`BalancedNetwork`] -> `.epc` round trip preserves
/// the power flow core. Where a PSLF read stashed a field the neutral model does
/// not name under a `pslf_*` extras key (the ZIP load split, the per unit shunt
/// G/B, the branch circuit id, the transformer winding base), the writer replays
/// it; otherwise it synthesizes the column. Same-format byte-exact echo rides the
/// retained source (see [`crate::emit`]); this is the cross format path and
/// the fallback when the source text was dropped (e.g. after a JSON round trip).
#[must_use]
// A flat serializer: one stanza per EPC section; splitting it would add
// indirection without clarity.
#[expect(clippy::too_many_lines)]
pub fn write_pslf(net: &BalancedNetwork) -> TextEmission {
    let mut warnings = Diagnostics::new();
    let mut nonfinite = false;
    let mut sanitized_names = 0usize;
    let mut sanitized_ids = 0usize;
    let mut s = String::new();

    let mut num = |x: f64| -> String {
        if x.is_finite() {
            format!("{x}")
        } else {
            nonfinite = true;
            let sentinel = if x > 0.0 {
                1.0e10
            } else if x < 0.0 {
                -1.0e10
            } else {
                0.0
            };
            format!("{sentinel}")
        }
    };

    // Bus identity for the lhs of every downstream record, keyed by source id.
    let bus_refs: HashMap<BusId, BusRef> = net
        .buses()
        .iter()
        .map(|b| {
            (
                b.id,
                BusRef {
                    name: b.name.as_deref().unwrap_or(""),
                    base_kv: b.base_kv,
                    area: b.area,
                    zone: b.zone,
                },
            )
        })
        .collect();
    let bus_ref = |id: BusId| -> BusRef {
        bus_refs.get(&id).copied().unwrap_or(BusRef {
            name: "",
            base_kv: 0.0,
            area: 1,
            zone: 1,
        })
    };
    // A quoted, sanitized name token; counts substitutions for the warning.
    let mut name_tok = |name: &str| -> String {
        let clean = sanitize_quoted(name, NAME_FORBIDDEN, ' ');
        if matches!(clean, std::borrow::Cow::Owned(_)) {
            sanitized_names += 1;
        }
        format!("\"{clean}\"")
    };

    // ---- header blocks ----
    let _ = writeln!(s, "title");
    // The title is one record; a terminator would end it and put the rest of
    // the name where the parser expects the next block.
    let _ = writeln!(s, "{}", sanitize_quoted(net.name(), NAME_FORBIDDEN, ' '));
    let _ = writeln!(s, "!");
    let _ = writeln!(s, "comments");
    let _ = writeln!(s, "powerio export");
    let _ = writeln!(s, "!");
    let _ = writeln!(s, "solution parameters");
    let _ = writeln!(s, "sbase {}", num(net.base_mva()));
    let _ = writeln!(s, "!");

    // ---- bus data ----
    // `vsched` is the scheduled voltage in per unit, distinct from the solved
    // `volt`. A bus with no base kV cannot state its generators' setpoint as
    // `reg_kv` (kV), and this is the only per unit column that can, so the
    // setpoint rides here for those buses. Where the bus states a base kV,
    // `reg_kv` carries the setpoint per generator and `vsched` stays the bus
    // voltage: routing it through `reg_kv = vg * base_kv` and back is not exact
    // in binary, so writing `vg` here too would make a re-serialize differ in
    // the last digit.
    //
    // One column per bus, so generators that disagree on a base-kV-less bus
    // keep only the first; the rest are what the generator loop reports.
    let mut setpoint_of: HashMap<BusId, f64> = HashMap::new();
    for g in net.generators() {
        if g.vg.is_finite() && g.vg > 0.0 {
            setpoint_of.entry(g.bus).or_insert(g.vg);
        }
    }
    let _ = writeln!(
        s,
        "bus data [{}] ty vsched volt angle ar zone vmax vmin",
        net.buses().len()
    );
    for b in net.buses() {
        let _ = writeln!(
            s,
            "{} {} {} : {} {} {} {} {} {} {} {}",
            b.id,
            name_tok(b.name.as_deref().unwrap_or("")),
            num(b.base_kv),
            pslf_type(b.kind),
            num(if b.base_kv > 0.0 {
                b.vm
            } else {
                setpoint_of.get(&b.id).copied().unwrap_or(b.vm)
            }),
            num(b.vm),
            num(b.va),
            b.area,
            b.zone,
            num(b.vmax),
            num(b.vmin),
        );
    }

    // ---- load data ----
    if !net.loads().is_empty() {
        let _ = writeln!(
            s,
            "load data [{}] id long_id st mw mvar mw_i mvar_i mw_z mvar_z ar zone",
            net.loads().len()
        );
        // Parallel loads on one bus get distinct ids; a captured `extras["id"]`
        // (from a PSS/E or PSLF source) wins, else positional.
        let mut load_ids: BTreeMap<BusId, BTreeSet<String>> = BTreeMap::new();
        for l in net.loads() {
            let r = bus_ref(l.bus);
            let (mw, mvar, mw_i, mvar_i, mw_z, mvar_z) =
                load_components_for_write(l, &mut warnings);
            let id = device_id(&l.extras, l.bus, &mut load_ids, &mut sanitized_ids);
            let _ = writeln!(
                s,
                "{} {} {} \"{id}\" \"load\" : {} {} {} {} {} {} {} {} {}",
                l.bus,
                name_tok(r.name),
                num(r.base_kv),
                i32::from(l.in_service),
                num(mw),
                num(mvar),
                num(mw_i),
                num(mvar_i),
                num(mw_z),
                num(mvar_z),
                r.area,
                r.zone,
            );
        }
    }

    // ---- shunt data ----
    if !net.shunts().is_empty() {
        let _ = writeln!(
            s,
            "shunt data [{}] id ck se long_id st ar zone pu_mw pu_mvar",
            net.shunts().len()
        );
        // Same per-bus id rule as loads: `(bus, id)` must stay unique.
        let mut shunt_ids: BTreeMap<BusId, BTreeSet<String>> = BTreeMap::new();
        for sh in net.shunts() {
            let r = bus_ref(sh.bus);
            // PSLF stores shunt G/B per unit on the system base; replay the read
            // values when present, else divide the MW/MVAr-at-1pu back out.
            let pu_mw = extra_f64(&sh.extras, "pslf_pu_mw")
                .or_else(|| extra_f64(&sh.extras, "pslf_pu_g"))
                .unwrap_or_else(|| safe_div(sh.g, net.base_mva()));
            let pu_mvar = extra_f64(&sh.extras, "pslf_pu_mvar")
                .or_else(|| extra_f64(&sh.extras, "pslf_pu_b"))
                .unwrap_or_else(|| safe_div(sh.b, net.base_mva()));
            let id = device_id(&sh.extras, sh.bus, &mut shunt_ids, &mut sanitized_ids);
            let _ = writeln!(
                s,
                "{} {} {} \"{id}\" : {} {} {} {} {}",
                sh.bus,
                name_tok(r.name),
                num(r.base_kv),
                i32::from(sh.in_service),
                r.area,
                r.zone,
                num(pu_mw),
                num(pu_mvar),
            );
        }
    }

    // ---- branch data (non-transformer) ----
    let lines: Vec<&Branch> = net
        .branches()
        .iter()
        .filter(|b| !b.is_transformer())
        .collect();
    if !lines.is_empty() {
        let _ = writeln!(
            s,
            "branch data [{}] ck se long_id st resist react charge rate1 rate2 rate3",
            lines.len()
        );
        // Parallel branches between the same bus pair get distinct circuit ids; a
        // captured `pslf_circuit` (from a PSLF source) wins, else positional.
        let mut branch_ids: BTreeMap<(BusId, BusId), BTreeSet<String>> = BTreeMap::new();
        for br in lines {
            let f = bus_ref(br.from);
            let t = bus_ref(br.to);
            let ck = super::allocate_circuit_id(
                br.extras.get("pslf_circuit").and_then(Value::as_str),
                (br.from, br.to),
                &mut branch_ids,
            );
            let se = section_tok(&br.extras);
            let _ = writeln!(
                s,
                "{} {} {} {} {} {} \"{ck}\" {se} \"line\" : {} {} {} {} {} {} {}",
                br.from,
                name_tok(f.name),
                num(f.base_kv),
                br.to,
                name_tok(t.name),
                num(t.base_kv),
                i32::from(br.in_service),
                num(br.r),
                num(br.x),
                num(br.calc_total_charging_b()),
                num(br.rate_a),
                num(br.rate_b),
                num(br.rate_c),
            );
        }
    }

    // ---- transformer data (2- and 3-winding, one section) ----
    let xfmrs: Vec<&Branch> = net
        .branches()
        .iter()
        .filter(|b| b.is_transformer())
        .collect();
    let n_xfmr = xfmrs.len() + net.transformers_3w().len();
    if n_xfmr > 0 {
        let _ = writeln!(s, "transformer data [{n_xfmr}]");
        for br in xfmrs {
            let f = bus_ref(br.from);
            let t = bus_ref(br.to);
            let tbase = extra_f64(&br.extras, "pslf_tbase").unwrap_or(net.base_mva());
            // First physical line: identity lhs, then the 21-field rhs the reader
            // indexes (status 0, tertiary 9 = 0, base 14, R 15, X 16, and the
            // pt/ts tertiary impedances 17-20 = 0 to mark a 2-winding unit). The
            // trailing `/` continues the record onto the second line.
            let mut rhs1 = vec!["0".to_string(); 21];
            rhs1[0] = i32::from(br.in_service).to_string();
            rhs1[14] = num(tbase);
            rhs1[15] = num(br.r);
            rhs1[16] = num(br.x);
            let _ = writeln!(
                s,
                "{} {} {} {} {} {} {} 1 \"xfmr\" : {} /",
                br.from,
                name_tok(f.name),
                num(f.base_kv),
                br.to,
                name_tok(t.name),
                num(t.base_kv),
                circuit_tok(&br.extras),
                rhs1.join(" "),
            );
            // Second physical line: ratings at 6-8, phase shift at 10, tap at 16.
            let mut line2 = vec!["0".to_string(); 17];
            line2[6] = num(br.rate_a);
            line2[7] = num(br.rate_b);
            line2[8] = num(br.rate_c);
            line2[10] = num(br.shift);
            line2[16] = num(br.calc_effective_tap());
            let _ = writeln!(s, "{}", line2.join(" "));
        }
        for tr in net.transformers_3w() {
            let p = bus_ref(tr.windings[0].bus);
            let sec = bus_ref(tr.windings[1].bus);
            let [z12, z23, z31] = tr.z;
            // The tertiary bus rides field 9; the pairwise impedances fill the
            // primary-secondary slot (15-16) and the primary-tertiary (17-18) and
            // secondary-tertiary (19-20) slots the reader keys off to detect a 3W.
            let mut rhs1 = vec!["0".to_string(); 21];
            rhs1[0] = i32::from(tr.in_service).to_string();
            rhs1[9] = tr.windings[2].bus.to_string();
            rhs1[14] = num(z12.base_mva);
            rhs1[15] = num(z12.r);
            rhs1[16] = num(z12.x);
            rhs1[17] = num(z31.r);
            rhs1[18] = num(z31.x);
            rhs1[19] = num(z23.r);
            rhs1[20] = num(z23.x);
            let _ = writeln!(
                s,
                "{} {} {} {} {} {} {} 1 \"xf3\" : {} /",
                tr.windings[0].bus,
                name_tok(p.name),
                num(p.base_kv),
                tr.windings[1].bus,
                name_tok(sec.name),
                num(sec.base_kv),
                circuit_tok(&tr.extras),
                rhs1.join(" "),
            );
            // Only the primary winding's ratio/ratings have a column here.
            let mut line2 = vec!["0".to_string(); 17];
            line2[6] = num(tr.windings[0].rate_a);
            line2[7] = num(tr.windings[0].rate_b);
            line2[8] = num(tr.windings[0].rate_c);
            line2[10] = num(tr.windings[0].shift);
            line2[16] = num(tr.windings[0].tap);
            let _ = writeln!(s, "{}", line2.join(" "));
        }
    }

    // ---- generator data ----
    if !net.generators().is_empty() {
        let _ = writeln!(
            s,
            "generator data [{}] id long_id st no reg_name reg_kv prf qrf ar zone \
             pgen pmax pmin qgen qmax qmin mbase",
            net.generators().len()
        );
        for g in net.generators() {
            let r = bus_ref(g.bus);
            // rhs indices the reader reads: status 0, reg_kv 3, pgen 8, pmax 9,
            // pmin 10, qgen 11, qmax 12, qmin 13, mbase 14. `reg_name` is left as
            // 0 because this writer only represents own-terminal regulation.
            // Without a base kV the setpoint rides the bus `vsched` column
            // instead, so it is lost only where that column carries a different
            // generator's.
            let reg_kv = if g.vg.is_finite() && r.base_kv > 0.0 {
                g.vg * r.base_kv
            } else {
                let scheduled = setpoint_of.get(&g.bus);
                if g.vg.is_finite()
                    && scheduled.is_some_and(|written| (written - g.vg).abs() > 1e-9)
                {
                    warnings.push(&F.value_substituted, format!(
                        "PSLF generator at bus {}: voltage setpoint {} p.u. could not be written because bus base kV is missing and the bus schedules a different setpoint",
                        g.bus, g.vg
                    ));
                }
                0.0
            };
            let _ = writeln!(
                s,
                "{} {} \"1\" \"gen\" : {} 1 0 {} 1 1 {} {} {} {} {} {} {} {} {}",
                g.bus,
                name_tok(r.name),
                i32::from(g.in_service),
                num(reg_kv),
                r.area,
                r.zone,
                num(g.pg),
                num(g.pmax),
                num(g.pmin),
                num(g.qg),
                num(g.qmax),
                num(g.qmin),
                num(g.mbase),
            );
        }
    }

    // ---- dc converter + dc line data (two-terminal HVDC) ----
    // EPC keeps the AC converter rows separate from the DC line that joins them,
    // keyed by a DC bus number. Synthesize a distinct DC bus per converter (these
    // are internal join keys, not AC buses) and emit the from/to converter rows
    // plus the line row that read_dc_converters/read_dc_lines rejoin into one
    // `BalancedNetwork::Hvdc`. Every unmapped field is written as 0, the shape
    // `dc_states_detail` reads as "nothing stated", so this writer's own output
    // reads back without the retained-control-fields warning.
    if !net.hvdc().is_empty() {
        let _ = writeln!(
            s,
            "dc converter data [{}] id name kv dc_bus",
            net.hvdc().len() * 2
        );
        for (k, d) in net.hvdc().iter().enumerate() {
            for (ac, dc_bus, p, q) in [
                (d.from, 2 * k + 1, d.pf, d.qf),
                (d.to, 2 * k + 2, d.pt, d.qt),
            ] {
                let r = bus_ref(ac);
                // Line 1 carries the AC bus (lhs 0), the DC bus (lhs 3), and the
                // status (rhs 0); the trailing `/` continues onto line 2, which
                // carries p (token 2) and q (token 3).
                let _ = writeln!(
                    s,
                    "{} {} {} {} : {} /",
                    ac,
                    name_tok(r.name),
                    num(r.base_kv),
                    dc_bus,
                    i32::from(d.in_service),
                );
                let _ = writeln!(s, "0 0 {} {}", num(p), num(q));
            }
        }
        let _ = writeln!(
            s,
            "dc line data [{}] from name kv to st rate1",
            net.hvdc().len()
        );
        for (k, d) in net.hvdc().iter().enumerate() {
            // The reader reads the status (rhs 0) and rate1 (rhs 6); rate1 sets the
            // power limit, falling back to |p| when nonpositive, so emit pmax.
            let _ = writeln!(
                s,
                "{} \"dc\" 0 {} : {} 0 0 0 0 0 {}",
                2 * k + 1,
                2 * k + 2,
                i32::from(d.in_service),
                num(d.pmax),
            );
        }
    }

    let _ = writeln!(s, "end");

    // ---- fidelity warnings ----
    let asymmetric_hvdc = net
        .hvdc()
        .iter()
        .filter(|d| (d.pmin + d.pmax).abs() > 1e-9)
        .count();
    if asymmetric_hvdc > 0 {
        warnings.push(
            &F.field_dropped,
            format!(
                "{asymmetric_hvdc} HVDC line(s) have asymmetric power limits (pmin != -pmax); \
             the PSLF .epc dc record carries only rate1 (= pmax), so pmin reads back as -pmax"
            ),
        );
    }
    if !net.storage().is_empty() {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} storage unit(s) dropped: PSLF .epc has no storage record",
                net.storage().len()
            ),
        );
    }
    if net.generators().iter().any(|g| g.cost.is_some()) {
        warnings.push(
            &F.field_dropped,
            "generator cost curves dropped: PSLF .epc carries no cost data",
        );
    }
    let with_caps = net.generators().iter().filter(|g| g.has_caps()).count();
    if with_caps > 0 {
        warnings.push(&F.field_dropped, format!(
            "generator capability/ramp columns dropped for {with_caps} generator(s): a PSLF .epc generator record states no reactive capability curve point and no ramp column"
        ));
    }
    if net.hvdc().iter().any(|d| d.cost.is_some()) {
        warnings.push(
            &F.field_dropped,
            "DC line cost curves dropped: PSLF .epc carries no cost data",
        );
    }
    // Transformer branches drop their charging entirely (warned separately
    // below), so exclude them here: only line records carry the collapsed total
    // susceptance this message describes.
    let terminal_charging = net
        .branches()
        .iter()
        .filter(|b| b.has_non_matpower_charging() && !b.is_transformer())
        .count();
    if terminal_charging > 0 {
        warnings.push(&F.value_collapsed, format!(
            "{terminal_charging} branch terminal admittance record(s) collapsed to total susceptance: PSLF branch records written here cannot carry conductance or asymmetric terminal charging"
        ));
    }
    let transformer_charging = net
        .branches()
        .iter()
        .filter(|b| {
            b.is_transformer()
                && (b.calc_terminal_charging().calc_total_g().abs() > 1e-12
                    || b.calc_terminal_charging().calc_total_b().abs() > 1e-12)
        })
        .count();
    if transformer_charging > 0 {
        warnings.push(&F.field_dropped, format!(
            "{transformer_charging} transformer charging admittance record(s) dropped: PSLF transformer records written here carry series impedance, tap, shift, and ratings only"
        ));
    }
    let current_ratings = net
        .branches()
        .iter()
        .filter(|b| b.current_ratings.is_some())
        .count();
    if current_ratings > 0 {
        warnings.push(&F.field_dropped, format!(
            "{current_ratings} branch current rating record(s) dropped: a PSLF .epc branch record states its ratings in MVA through rate1, rate2, and rate3"
        ));
    }
    warn_extra_branch_rating_sets(&F, "PSLF .epc", net, &mut warnings);
    // The keys this writer replays, spelled out rather than `pslf_*`: the
    // record tails (`pslf_lhs_extra`/`pslf_rhs_extra`) and the DC converter
    // stash are retained by the reader and never written back, so a prefix
    // rule would declare them replayed when they are not (#330).
    super::warn_dropped_extras(
        &F,
        "PSLF .epc",
        net,
        |key| {
            matches!(
                key,
                "id" | "pslf_circuit"
                    | "pslf_section_id"
                    | "pslf_tbase"
                    | "pslf_mw"
                    | "pslf_mvar"
                    | "pslf_mw_i"
                    | "pslf_mvar_i"
                    | "pslf_mw_z"
                    | "pslf_mvar_z"
                    | "pslf_pu_mw"
                    | "pslf_pu_mvar"
                    | "pslf_pu_g"
                    | "pslf_pu_b"
            )
        },
        &mut warnings,
    );
    super::warn_dropped_areas(&F, "PSLF .epc", true, net, &mut warnings);
    let branch_solutions = net
        .branches()
        .iter()
        .filter(|b| b.solution.is_some())
        .count();
    if branch_solutions > 0 {
        warnings.push(&F.field_dropped, format!(
            "{branch_solutions} branch solution value set(s) dropped: a PSLF .epc branch record states impedance, taps, and ratings and no solved flow"
        ));
    }
    // The generator record this writer emits regulates the unit's own terminal, so
    // a generator pointing at a remote regulated bus loses that target.
    let dropped_reg = net
        .generators()
        .iter()
        .filter(|g| g.regulated_bus.is_some())
        .count();
    if dropped_reg > 0 {
        warnings.push(&F.field_dropped, format!(
            "{dropped_reg} generator(s) lost their remote regulated bus: the PSLF .epc generator \
             record this writer emits controls the unit's own terminal"
        ));
    }
    // A 3-winding record here carries only the primary winding's ratio/ratings, so
    // report any non-nominal secondary/tertiary winding as a fidelity loss.
    let drops_winding_detail = net.transformers_3w().iter().any(|t| {
        t.windings[1..]
            .iter()
            .any(|w| (w.tap - 1.0).abs() > 1e-9 || w.rate_a.abs() > 1e-9)
    });
    if drops_winding_detail {
        warnings.push(
            &F.field_dropped,
            "PSLF 3-winding export carries the primary winding ratio/ratings only; \
             secondary/tertiary winding ratios/ratings dropped",
        );
    }
    // The `.epc` transformer record this writer emits has no regulating-control
    // columns (mode/limits/regulated bus), so a Branch carrying control loses it.
    let dropped_control = net
        .branches()
        .iter()
        .filter(|b| b.control.is_some())
        .count();
    if dropped_control > 0 {
        warnings.push(
            &F.field_dropped,
            format!(
                "{dropped_control} transformer(s) lost their regulating control (mode/tap limits/\
             regulated bus): the PSLF .epc transformer record carries no control columns"
            ),
        );
    }
    // Switched shunts write as fixed `.epc` shunts (G/B); the switching control
    // has no column in the shunt record this writer emits.
    let dropped_sw = net.shunts().iter().filter(|s| s.control.is_some()).count();
    if dropped_sw > 0 {
        warnings.push(
            &F.field_dropped,
            format!(
                "{dropped_sw} switched shunt(s) written as fixed: the PSLF .epc shunt record this \
             writer emits has no switching-control columns (mode/band/step blocks)"
            ),
        );
    }
    let sanitized = sanitized_names + sanitized_ids;
    if sanitized > 0 {
        warnings.push(
            &F.value_substituted,
            format!(
                "{sanitized} quoted field(s) contained a double quote that would corrupt an EPC \
             record; replaced with spaces"
            ),
        );
    }
    if nonfinite {
        warnings.push(
            &F.not_a_number,
            "non-finite values written as ±1e10 sentinels (PSLF has no Inf/NaN)",
        );
    }

    TextEmission::new(s, warnings)
}

/// Neutral bus kind -> PSLF bus type code (inverse of [`pslf_bus_type`]).
fn pslf_type(kind: BusType) -> u8 {
    match kind {
        BusType::Ref => 0,
        BusType::Pv => 2,
        BusType::Isolated => 4,
        BusType::Pq => 1,
    }
}

/// A per-bus unique load/shunt id: the captured `extras["id"]` (trimmed,
/// sanitized) when still free on this bus, else the lowest free positional id,
/// so parallel devices keep the `(bus, id)` uniqueness the EPC section requires.
fn device_id(
    extras: &Extras,
    bus: BusId,
    used: &mut BTreeMap<BusId, BTreeSet<String>>,
    sanitized: &mut usize,
) -> String {
    let preferred = extras
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| {
            let clean = sanitize_quoted(id, NAME_FORBIDDEN, ' ');
            if matches!(clean, std::borrow::Cow::Owned(_)) {
                *sanitized += 1;
            }
            clean.into_owned()
        });
    super::allocate_circuit_id(preferred.as_deref(), bus, used)
}

/// The branch section number token, replayed from `pslf_section_id` when a
/// PSLF read kept one (a multi-section line), else `1` — the writer used to
/// hardcode `1`, silently renumbering retained sections on write-back.
fn section_tok(extras: &Extras) -> String {
    match extras.get("pslf_section_id") {
        Some(Value::String(t)) => sanitize_quoted(t, &['"', ':', ' ', '\t', '/'], '_').into_owned(),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite())
            .map_or_else(|| "1".into(), |x| x.to_string()),
        None => "1".into(),
    }
}

/// The branch/transformer circuit id token, replayed from `pslf_circuit` when a
/// PSLF read kept it, else `"1"`.
fn circuit_tok(extras: &Extras) -> String {
    let ck = extras
        .get("pslf_circuit")
        .and_then(Value::as_str)
        .unwrap_or("1");
    // Sanitized like `device_id` and `name_tok`: an unfiltered quote or
    // separator here shifts every later column of the branch record. `:`
    // splits the EPC lhs/rhs, so it breaks the record too.
    let clean = sanitize_quoted(ck, &['"', ':', ' ', '\t', '/'], '_');
    format!("\"{clean}\"")
}

/// A numeric `pslf_*` extra, if present and finite. A non-finite value yields
/// `None` so the caller falls back to its synthesized default rather than
/// replaying a `NaN`/`±Inf` into the record.
fn extra_f64(extras: &Extras, key: &str) -> Option<f64> {
    extras
        .get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
}

fn same_load_total(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn load_components_for_write(
    l: &Load,
    warnings: &mut Diagnostics,
) -> (f64, f64, f64, f64, f64, f64) {
    if let Some(LoadVoltageModel::Zip {
        p_constant_power,
        q_constant_power,
        p_constant_current,
        q_constant_current,
        p_constant_impedance,
        q_constant_impedance,
        v_nom,
        load_type,
        scaling,
        ..
    }) = &l.voltage_model
    {
        if same_load_total(
            p_constant_power + p_constant_current + p_constant_impedance,
            l.p,
        ) && same_load_total(
            q_constant_power + q_constant_current + q_constant_impedance,
            l.q,
        ) {
            if v_nom.is_some() {
                warnings.push(
                    &F.field_dropped,
                    format!(
                        "PSLF load at bus {}: nominal voltage has no load data field; dropped",
                        l.bus
                    ),
                );
            }
            if load_type.is_some() || scaling.is_some() {
                warnings.push(&F.field_dropped, format!(
                    "PSLF load at bus {}: PSS/E load type/scaling has no load data field; dropped",
                    l.bus
                ));
            }
            return (
                *p_constant_power,
                *q_constant_power,
                *p_constant_current,
                *q_constant_current,
                *p_constant_impedance,
                *q_constant_impedance,
            );
        }
        warnings.push(&F.value_substituted, format!(
            "PSLF load at bus {}: stale voltage model components did not match typed p/q; wrote typed p/q as constant power",
            l.bus
        ));
        return (l.p, l.q, 0.0, 0.0, 0.0, 0.0);
    }
    if matches!(l.voltage_model, Some(LoadVoltageModel::Exponential { .. })) {
        warnings.push(&F.field_dropped, format!(
            "PSLF load at bus {}: exponential voltage model has no PSLF load data columns; wrote typed p/q as constant power",
            l.bus
        ));
        return (l.p, l.q, 0.0, 0.0, 0.0, 0.0);
    }

    // Replay the ZIP split a PSLF read preserved before this release; otherwise
    // put the whole demand in the constant power column.
    let mw = extra_f64(&l.extras, "pslf_mw").unwrap_or(l.p);
    let mvar = extra_f64(&l.extras, "pslf_mvar").unwrap_or(l.q);
    let mw_i = extra_f64(&l.extras, "pslf_mw_i").unwrap_or(0.0);
    let mvar_i = extra_f64(&l.extras, "pslf_mvar_i").unwrap_or(0.0);
    let mw_z = extra_f64(&l.extras, "pslf_mw_z").unwrap_or(0.0);
    let mvar_z = extra_f64(&l.extras, "pslf_mvar_z").unwrap_or(0.0);
    if l.extras.keys().any(|key| {
        matches!(
            key.as_str(),
            "pslf_mw" | "pslf_mvar" | "pslf_mw_i" | "pslf_mvar_i" | "pslf_mw_z" | "pslf_mvar_z"
        )
    }) && (!same_load_total(mw + mw_i + mw_z, l.p)
        || !same_load_total(mvar + mvar_i + mvar_z, l.q))
    {
        warnings.push(&F.value_substituted, format!(
            "PSLF load at bus {}: stale PSLF load extras did not match typed p/q; wrote typed p/q as constant power",
            l.bus
        ));
        return (l.p, l.q, 0.0, 0.0, 0.0, 0.0);
    }
    (mw, mvar, mw_i, mvar_i, mw_z, mvar_z)
}

/// `a / b`, or 0 when `b` is not a usable divisor (the identity for an absent base).
fn safe_div(a: f64, b: f64) -> f64 {
    if b.is_finite() && b != 0.0 {
        a / b
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    /// A DC record whose converter or line rows state real control fields —
    /// firing angles, taps, a voltage schedule — warns that they stay only in
    /// extras; the same records with those fields zeroed (the shape our writer
    /// emits) do not, because a zero states nothing in EPC.
    #[test]
    fn dc_control_detail_warns_and_the_neutral_shape_does_not() {
        let epc = |converter_tail: &str| {
            r#"title
d
!
solution parameters
sbase 100
!
bus data [2] ty vsched volt angle ar zone vmax vmin
1 "A" 230 : 0 1 1 0 1 1 1.1 0.9
2 "B" 230 : 1 1 1 0 1 1 1.1 0.9
dc converter data [2] id name kv dc_bus
1 "A" 230 11 : 1 /
0 0 10 2 TAIL
2 "B" 230 12 : 1 /
0 0 -9.5 -1.5
dc line data [1] from name kv to st rate1
11 "dc" 0 12 : 1 0 0 0 0 0 10
end
"#
            .replace(" TAIL", converter_tail)
        };

        // Firing angle limits stated on the rectifier: retained-only data.
        let mut warnings = Diagnostics::new();
        let net = parse_pslf_source(&epc(" 15 90"), None, &mut warnings).unwrap();
        assert_eq!(net.hvdc().len(), 1);
        assert!(
            warnings
                .lines()
                .iter()
                .any(|w| w.contains("unsupported control fields")),
            "stated firing angles must be reported: {warnings:?}"
        );

        // The neutral shape the writer emits: nothing beyond status/p/q/rate.
        let mut warnings = Diagnostics::new();
        let net = parse_pslf_source(&epc(""), None, &mut warnings).unwrap();
        assert_eq!(net.hvdc().len(), 1);
        assert!(
            !warnings
                .lines()
                .iter()
                .any(|w| w.contains("unsupported control fields")),
            "zeros state nothing: {warnings:?}"
        );
        let dc = &net.hvdc()[0];
        assert!((dc.pf - 10.0).abs() < 1e-12 && (dc.pmax - 10.0).abs() < 1e-12);
    }

    #[test]
    fn reads_minimal_epc_core() {
        let epc = r#"title
minimal
!
solution parameters
sbase 100.0000
jump  0.000290
!
bus data  [2] ty vsched volt angle ar zone vmax vmin date_in date_out pid L own st
1 "Slack       " 230.0000 : 0 1.0000 1.0000 0.0 1 1 1.1 0.9 400101 391231 0 0 1 0
2 "Load        " 230.0000 : 1 1.0000 1.0000 -1.0 1 1 1.1 0.9 400101 391231 0 0 1 0
branch data  [1] ck se long_id st resist react charge rate1 rate2 rate3 rate4 aloss lngth
1 "Slack       " 230.00 2 "Load        " 230.00 "1 " 1 "line" : 1 0.01 0.05 0.001 100 90 80 0 0 1 /
1 1 0 0
generator data  [1] id long_id st no reg_name prf qrf ar zone pgen pmax pmin qgen qmax qmin mbase
1 "Slack       " 230.00 "1 " "gen" : 1 1 "Slack       " 230.00 0 1 1 1 50 80 0 5 30 -20 100 /
0
load data  [1] id long_id st mw mvar mw_i mvar_i mw_z mvar_z ar zone
2 "Load        " 230.00 "1 " "load" : 1 10 3 1 0.5 2 1.5 1 1
shunt data  [1] id ck se long_id st ar zone pu_mw pu_mvar
2 "Load        " 230.00 "b " 0 "" 0.00 "  " 0 "" : 1 1 1 0.00 0.10
end
"#;

        let mut warnings = Diagnostics::new();
        let net = parse_pslf_source(epc, None, &mut warnings).unwrap();

        assert_eq!(net.source_format(), SourceFormat::Pslf);
        assert_eq!(net.buses().len(), 2);
        assert_eq!(net.branches().len(), 1);
        assert_eq!(net.loads().len(), 1);
        assert_eq!(net.generators().len(), 1);
        assert_eq!(net.shunts().len(), 1);
        assert_eq!(net.buses()[0].kind, BusType::Ref);
        close(net.loads()[0].p, 13.0);
        close(net.loads()[0].q, 5.0);
        close(net.shunts()[0].b, 10.0);
        assert!(warnings.lines().iter().any(|w| w.contains("ZIP load")));
    }

    #[test]
    fn transformer_charging_drop_is_warned_on_write() {
        let mut net = BalancedNetwork::in_memory(
            "charging",
            100.0,
            vec![
                Bus {
                    id: BusId(1),
                    kind: BusType::Ref,
                    vm: 1.0,
                    va: 0.0,
                    base_kv: 230.0,
                    vmax: 1.1,
                    vmin: 0.9,
                    evhi: None,
                    evlo: None,
                    area: 1,
                    zone: 1,
                    name: None,
                    uid: None,
                    location: None,
                    extras: Extras::new(),
                },
                Bus {
                    id: BusId(2),
                    kind: BusType::Pq,
                    vm: 1.0,
                    va: 0.0,
                    base_kv: 230.0,
                    vmax: 1.1,
                    vmin: 0.9,
                    evhi: None,
                    evlo: None,
                    area: 1,
                    zone: 1,
                    name: None,
                    uid: None,
                    location: None,
                    extras: Extras::new(),
                },
            ],
            Vec::new(),
        );
        net.branches_mut().push(Branch {
            name: None,
            from: BusId(1),
            to: BusId(2),
            r: 0.01,
            x: 0.1,
            b: 0.02,
            charging: None,
            rate_a: 100.0,
            rate_b: 100.0,
            rate_c: 100.0,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: 1.0,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            solution: None,
            uid: None,
            route: None,
            extras: Extras::new(),
        });

        let conv = write_pslf(&net);
        assert!(
            conv.render_diagnostics()
                .iter()
                .any(|w| w.contains("transformer charging admittance")),
            "{:?}",
            conv.render_diagnostics()
        );
    }

    #[test]
    fn clean_line_continuation_slash_respects_quotes() {
        assert_eq!(clean_line(r#"1 "A" : 0 /"#), (r#"1 "A" : 0"#.into(), true));
        assert_eq!(
            clean_line(r#"1 "name/" : 0"#),
            (r#"1 "name/" : 0"#.into(), false)
        );
        assert_eq!(
            clean_line(r#"1 "unterminated /"#),
            (r#"1 "unterminated /"#.into(), false)
        );
        assert_eq!(
            clean_line(r#"1 "has ""quote""" : 0 /"#),
            (r#"1 "has ""quote""" : 0"#.into(), true)
        );
    }

    #[test]
    fn pslf_tokens_keep_slashes_inside_quoted_names() {
        assert_eq!(
            tokens(r#"1 "A/B" 230.0 : 0"#),
            vec!["1", "A/B", "230.0", ":", "0"]
        );
    }

    #[test]
    fn parse_id_accepts_only_integer_values() {
        assert_eq!(parse_id("12"), Some(12));
        assert_eq!(parse_id("12.0"), Some(12));
        assert_eq!(parse_id("1e3"), Some(1000));
        assert_eq!(parse_id("12.9"), None);
        assert_eq!(parse_id("-1"), None);
        assert_eq!(parse_id("NaN"), None);
    }
}
