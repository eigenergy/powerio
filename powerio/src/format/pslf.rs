//! Read legacy GE PSLF `.epc` power flow cases.
//!
//! EPC files contain named data sections with colon separated record bodies.
//! The reader keeps raw physical lines plus token lists on both sides of each
//! colon, then maps the static power flow core into [`Network`]. Records outside
//! that model stay in retained source text and read warnings.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Number, Value};

use crate::network::{
    Branch, Bus, BusId, BusType, Extras, Generator, Hvdc, Load, Network, Shunt, SourceFormat,
};
use crate::{Error, Result};

const FMT: &str = "PSLF .epc";

/// Parse a PSLF `.epc` case into a [`Network`].
///
/// Read warnings are available through the shared [`crate::parse_file`] /
/// [`crate::parse_str`] entry points. This direct helper keeps the older
/// format-module convention and returns only the typed network.
pub fn parse_pslf(content: &str) -> Result<Network> {
    let mut warnings = Vec::new();
    parse_pslf_source(Arc::new(content.to_owned()), None, &mut warnings)
}

/// Parse retained source from the format hub.
pub(crate) fn parse_pslf_source(
    source: Arc<String>,
    name_hint: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Network> {
    let doc = parse_document(&source, warnings);
    let base_mva = doc.base_mva(warnings);
    let name = doc.name(name_hint);
    let mut once = HashSet::new();

    let mut buses = Vec::new();
    let mut bus_vm = HashMap::new();
    for rec in doc.records("bus data") {
        let bus = read_bus(rec)?;
        bus_vm.insert(bus.id, bus.vm);
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
        warnings.push(format!(
            "{near_jump} branch(es) have |x| at or below the PSLF jump threshold"
        ));
    }

    for rec in doc.records("transformer data") {
        if let Some(branch) = read_transformer(rec, warnings)? {
            branches.push(branch);
        }
    }

    let mut generators = Vec::new();
    for rec in doc.records("generator data") {
        generators.push(read_generator(rec, &bus_vm)?);
    }

    let dc_converters = read_dc_converters(&doc, warnings);
    let hvdc = read_dc_lines(&doc, &dc_converters, warnings);

    warn_unmodeled_sections(&doc, warnings);

    let net = Network {
        name,
        base_mva,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage: Vec::new(),
        hvdc,
        source_format: SourceFormat::Pslf,
        source: Some(source),
    };
    net.check_references(FMT)?;
    Ok(net)
}

#[derive(Debug)]
struct EpcDocument {
    title: Vec<String>,
    solution_parameters: Vec<String>,
    sections: BTreeMap<String, Section>,
}

impl EpcDocument {
    fn name(&self, name_hint: Option<&str>) -> String {
        self.title
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(|| name_hint.unwrap_or("case").to_string(), str::to_string)
    }

    fn records(&self, section: &str) -> &[Record] {
        self.sections
            .get(section)
            .map_or(&[], |section| section.records.as_slice())
    }

    fn base_mva(&self, warnings: &mut Vec<String>) -> f64 {
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
        warnings.push("no PSLF sbase solution parameter found; defaulting baseMVA to 100".into());
        100.0
    }

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

#[derive(Debug)]
struct Section {
    declared_count: usize,
    header: String,
    records: Vec<Record>,
}

#[derive(Debug)]
struct Record {
    line_no: usize,
    raw: Vec<String>,
    lhs: Vec<String>,
    rhs: Vec<String>,
}

#[expect(clippy::too_many_lines)]
fn parse_document(content: &str, warnings: &mut Vec<String>) -> EpcDocument {
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
            warnings.push(format!(
                "line {} ignored outside a PSLF data section",
                i + 1
            ));
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
            warnings.push(format!(
                "{}: declared {count}, parsed {}",
                name,
                records.len()
            ));
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
            warnings.push(format!(
                "{name}: duplicate section replaced earlier records"
            ));
        }
    }

    if !end_seen {
        warnings.push("PSLF file has no end marker".into());
    }

    EpcDocument {
        title,
        solution_parameters,
        sections,
    }
}

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

fn clean_line(raw: &str) -> (String, bool) {
    let raw = raw.trim_end_matches('\r');
    let trimmed = raw.trim_end();
    let continued = trimmed.ends_with('/');
    if continued {
        let without = &trimmed[..trimmed.len() - 1];
        (without.trim_end().to_string(), true)
    } else {
        (raw.to_string(), false)
    }
}

fn split_record(raw_lines: &[String]) -> (Vec<String>, Vec<String>) {
    let toks = tokens(&raw_lines.join(" "));
    split_tokens(toks)
}

fn split_tokens(toks: Vec<String>) -> (Vec<String>, Vec<String>) {
    if let Some(colon) = toks.iter().position(|tok| tok == ":") {
        (toks[..colon].to_vec(), toks[colon + 1..].to_vec())
    } else {
        (toks, Vec::new())
    }
}

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

fn line_rhs(rec: &Record, line: usize) -> Vec<String> {
    rec.raw
        .get(line)
        .map(|line| split_tokens(tokens(line)).1)
        .unwrap_or_default()
}

fn line_tokens(rec: &Record, line: usize) -> Vec<String> {
    rec.raw.get(line).map_or_else(Vec::new, |line| tokens(line))
}

fn read_bus(rec: &Record) -> Result<Bus> {
    let id = BusId(req_id(&rec.lhs, 0, "bus id", rec)?);
    let name = rec.lhs.get(1).map(|name| name.trim().to_string());
    Ok(Bus {
        id,
        kind: pslf_bus_type(int_at(&rec.rhs, 0, 1, "bus type", rec)?),
        vm: num_at(&rec.rhs, 2, 1.0, "bus voltage", rec)?,
        va: num_at(&rec.rhs, 3, 0.0, "bus angle", rec)?,
        base_kv: num_at(&rec.lhs, 2, 0.0, "bus nominal kV", rec)?,
        vmax: num_at(&rec.rhs, 6, 1.1, "bus vmax", rec)?,
        vmin: num_at(&rec.rhs, 7, 0.9, "bus vmin", rec)?,
        area: id_at(&rec.rhs, 4, 1, "bus area", rec)?,
        zone: id_at(&rec.rhs, 5, 1, "bus zone", rec)?,
        name,
        extras: extras(rec, "bus data", 3, 21),
    })
}

fn pslf_bus_type(code: i64) -> BusType {
    match code {
        0 => BusType::Ref,
        2 => BusType::Pv,
        4 => BusType::Isolated,
        _ => BusType::Pq,
    }
}

fn read_branch(rec: &Record) -> Result<Branch> {
    let mut extras = extras(rec, "branch data", 9, 10);
    if let Some(circuit) = rec.lhs.get(6) {
        extras.insert("pslf_circuit".into(), Value::String(circuit.clone()));
    }
    if let Some(section) = rec.lhs.get(7) {
        extras.insert("pslf_section_id".into(), string_or_number(section));
    }
    Ok(Branch {
        from: BusId(req_id(&rec.lhs, 0, "branch from bus", rec)?),
        to: BusId(req_id(&rec.lhs, 3, "branch to bus", rec)?),
        r: num_at(&rec.rhs, 1, 0.0, "branch r", rec)?,
        x: num_at(&rec.rhs, 2, 0.0, "branch x", rec)?,
        b: num_at(&rec.rhs, 3, 0.0, "branch b", rec)?,
        rate_a: num_at(&rec.rhs, 4, 0.0, "branch rate1", rec)?,
        rate_b: num_at(&rec.rhs, 5, 0.0, "branch rate2", rec)?,
        rate_c: num_at(&rec.rhs, 6, 0.0, "branch rate3", rec)?,
        tap: 0.0,
        shift: 0.0,
        in_service: on_at(&rec.rhs, 0, true, "branch status", rec)?,
        angmin: -360.0,
        angmax: 360.0,
        extras,
    })
}

fn read_transformer(rec: &Record, warnings: &mut Vec<String>) -> Result<Option<Branch>> {
    let rhs1 = line_rhs(rec, 0);
    let line2 = line_tokens(rec, 1);
    let tertiary = id_at(&rhs1, 9, 0, "transformer tertiary bus", rec)?;
    let pt_r = num_at(&rhs1, 17, 0.0, "transformer pt_r", rec)?;
    let pt_x = num_at(&rhs1, 18, 0.0, "transformer pt_x", rec)?;
    let ts_r = num_at(&rhs1, 19, 0.0, "transformer ts_r", rec)?;
    let ts_x = num_at(&rhs1, 20, 0.0, "transformer ts_x", rec)?;
    if tertiary != 0 || pt_r != 0.0 || pt_x != 0.0 || ts_r != 0.0 || ts_x != 0.0 {
        warnings.push(format!(
            "transformer record at line {} is three winding; no neutral Network equivalent",
            rec.line_no
        ));
        return Ok(None);
    }

    let tap = num_at(&line2, 16, 1.0, "transformer tap", rec)?;
    let mut extras = extras(rec, "transformer data", 8, 21);
    if let Some(circuit) = rec.lhs.get(6) {
        extras.insert("pslf_circuit".into(), Value::String(circuit.clone()));
    }
    extras.insert(
        "pslf_tbase".into(),
        number_value(num_at(&rhs1, 14, 0.0, "transformer base", rec)?),
    );
    Ok(Some(Branch {
        from: BusId(req_id(&rec.lhs, 0, "transformer from bus", rec)?),
        to: BusId(req_id(&rec.lhs, 3, "transformer to bus", rec)?),
        r: num_at(&rhs1, 15, 0.0, "transformer r", rec)?,
        x: num_at(&rhs1, 16, 0.0, "transformer x", rec)?,
        b: 0.0,
        rate_a: num_at(&line2, 6, 0.0, "transformer rate1", rec)?,
        rate_b: num_at(&line2, 7, 0.0, "transformer rate2", rec)?,
        rate_c: num_at(&line2, 8, 0.0, "transformer rate3", rec)?,
        tap: if tap == 0.0 { 1.0 } else { tap },
        shift: num_at(&line2, 10, 0.0, "transformer shift", rec)?,
        in_service: on_at(&rhs1, 0, true, "transformer status", rec)?,
        angmin: -360.0,
        angmax: 360.0,
        extras,
    }))
}

fn read_generator(rec: &Record, bus_vm: &HashMap<BusId, f64>) -> Result<Generator> {
    let bus = BusId(req_id(&rec.lhs, 0, "generator bus", rec)?);
    Ok(Generator {
        bus,
        pg: num_at(&rec.rhs, 8, 0.0, "generator pgen", rec)?,
        qg: num_at(&rec.rhs, 11, 0.0, "generator qgen", rec)?,
        pmax: num_at(&rec.rhs, 9, 0.0, "generator pmax", rec)?,
        pmin: num_at(&rec.rhs, 10, 0.0, "generator pmin", rec)?,
        qmax: num_at(&rec.rhs, 12, 0.0, "generator qmax", rec)?,
        qmin: num_at(&rec.rhs, 13, 0.0, "generator qmin", rec)?,
        vg: bus_vm.get(&bus).copied().unwrap_or(1.0),
        mbase: num_at(&rec.rhs, 14, 100.0, "generator mbase", rec)?,
        in_service: on_at(&rec.rhs, 0, true, "generator status", rec)?,
        cost: None,
        caps: Default::default(),
    })
}

fn read_load(
    rec: &Record,
    warnings: &mut Vec<String>,
    once: &mut HashSet<&'static str>,
) -> Result<Load> {
    let p_const = num_at(&rec.rhs, 1, 0.0, "load mw", rec)?;
    let q_const = num_at(&rec.rhs, 2, 0.0, "load mvar", rec)?;
    let p_i = num_at(&rec.rhs, 3, 0.0, "load mw_i", rec)?;
    let q_i = num_at(&rec.rhs, 4, 0.0, "load mvar_i", rec)?;
    let p_z = num_at(&rec.rhs, 5, 0.0, "load mw_z", rec)?;
    let q_z = num_at(&rec.rhs, 6, 0.0, "load mvar_z", rec)?;
    if (p_i, q_i, p_z, q_z) != (0.0, 0.0, 0.0, 0.0) && once.insert("zip_load") {
        warnings.push(
            "PSLF ZIP load components folded into Network load p/q; component fields retained in extras"
                .into(),
        );
    }
    let mut extras = extras(rec, "load data", 5, 20);
    extras.insert("pslf_mw".into(), number_value(p_const));
    extras.insert("pslf_mvar".into(), number_value(q_const));
    extras.insert("pslf_mw_i".into(), number_value(p_i));
    extras.insert("pslf_mvar_i".into(), number_value(q_i));
    extras.insert("pslf_mw_z".into(), number_value(p_z));
    extras.insert("pslf_mvar_z".into(), number_value(q_z));
    Ok(Load {
        bus: BusId(req_id(&rec.lhs, 0, "load bus", rec)?),
        p: p_const + p_i + p_z,
        q: q_const + q_i + q_z,
        in_service: on_at(&rec.rhs, 0, true, "load status", rec)?,
        extras,
    })
}

fn read_shunt(rec: &Record, base_mva: f64) -> Result<Shunt> {
    let g_pu = num_at(&rec.rhs, 3, 0.0, "shunt pu_mw", rec)?;
    let b_pu = num_at(&rec.rhs, 4, 0.0, "shunt pu_mvar", rec)?;
    let mut extras = extras(rec, "shunt data", 10, 29);
    extras.insert("pslf_pu_mw".into(), number_value(g_pu));
    extras.insert("pslf_pu_mvar".into(), number_value(b_pu));
    Ok(Shunt {
        bus: BusId(req_id(&rec.lhs, 0, "shunt bus", rec)?),
        g: g_pu * base_mva,
        b: b_pu * base_mva,
        in_service: on_at(&rec.rhs, 0, true, "shunt status", rec)?,
        extras,
    })
}

fn read_svd(
    rec: &Record,
    base_mva: f64,
    warnings: &mut Vec<String>,
    once: &mut HashSet<&'static str>,
) -> Result<Shunt> {
    if once.insert("svd") {
        warnings.push(
            "PSLF controlled shunts (svd data) reduced to fixed shunts at initial g/b; control fields retained in extras"
                .into(),
        );
    }
    let g_pu = num_at(&rec.rhs, 7, 0.0, "svd g", rec)?;
    let b_pu = num_at(&rec.rhs, 8, 0.0, "svd b", rec)?;
    let mut extras = extras(rec, "svd data", 5, 30);
    extras.insert("pslf_device".into(), Value::String("svd".into()));
    extras.insert("pslf_pu_g".into(), number_value(g_pu));
    extras.insert("pslf_pu_b".into(), number_value(b_pu));
    Ok(Shunt {
        bus: BusId(req_id(&rec.lhs, 0, "svd bus", rec)?),
        g: g_pu * base_mva,
        b: b_pu * base_mva,
        in_service: on_at(&rec.rhs, 0, true, "svd status", rec)?,
        extras,
    })
}

#[derive(Clone)]
struct DcConverter {
    ac_bus: BusId,
    dc_bus: usize,
    in_service: bool,
    p: f64,
    q: f64,
    extras: Extras,
}

fn read_dc_converters(
    doc: &EpcDocument,
    warnings: &mut Vec<String>,
) -> HashMap<usize, DcConverter> {
    let mut out = HashMap::new();
    for rec in doc.records("dc converter data") {
        let parsed = (|| -> Result<DcConverter> {
            let l2 = line_tokens(rec, 1);
            let mut extras = extras(rec, "dc converter data", 8, 15);
            extras.insert("pslf_device".into(), Value::String("dc_converter".into()));
            Ok(DcConverter {
                ac_bus: BusId(req_id(&rec.lhs, 0, "dc converter AC bus", rec)?),
                dc_bus: req_id(&rec.lhs, 3, "dc converter DC bus", rec)?,
                in_service: on_at(&rec.rhs, 0, true, "dc converter status", rec)?,
                p: num_at(&l2, 2, 0.0, "dc converter p", rec)?,
                q: num_at(&l2, 3, 0.0, "dc converter q", rec)?,
                extras,
            })
        })();
        match parsed {
            Ok(conv) => {
                out.insert(conv.dc_bus, conv);
            }
            Err(err) => warnings.push(format!(
                "dc converter at line {} not mapped: {err}",
                rec.line_no
            )),
        }
    }
    out
}

fn read_dc_lines(
    doc: &EpcDocument,
    converters: &HashMap<usize, DcConverter>,
    warnings: &mut Vec<String>,
) -> Vec<Hvdc> {
    let mut out = Vec::new();
    for rec in doc.records("dc line data") {
        let parsed = (|| -> Result<Hvdc> {
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
            let rate = num_at(&rec.rhs, 6, 0.0, "dc line rate1", rec)?;
            let pmax = if rate > 0.0 {
                rate
            } else {
                from.p.abs().max(to.p.abs())
            };
            let mut extras = extras(rec, "dc line data", 8, 20);
            extras.insert("pslf_device".into(), Value::String("dc_line".into()));
            extras.insert(
                "pslf_from_converter".into(),
                Value::Object(from.extras.clone().into_iter().collect()),
            );
            extras.insert(
                "pslf_to_converter".into(),
                Value::Object(to.extras.clone().into_iter().collect()),
            );
            Ok(Hvdc {
                from: from.ac_bus,
                to: to.ac_bus,
                in_service: on_at(&rec.rhs, 0, true, "dc line status", rec)?
                    && from.in_service
                    && to.in_service,
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
                extras,
            })
        })();
        match parsed {
            Ok(line) => {
                warnings.push(
                    "PSLF DC line/converter data mapped to Network HVDC with unsupported control fields retained in extras"
                        .into(),
                );
                out.push(line);
            }
            Err(err) => warnings.push(format!("dc line at line {} not mapped: {err}", rec.line_no)),
        }
    }
    out
}

fn warn_unmodeled_sections(doc: &EpcDocument, warnings: &mut Vec<String>) {
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
            warnings.push(format!(
                "{name}: {} record(s) retained in source text only ({})",
                section.declared_count, section.header
            ));
        }
    }
}

fn extras(rec: &Record, section: &str, used_lhs: usize, used_rhs: usize) -> Extras {
    let mut extras = Extras::new();
    extras.insert("pslf_section".into(), Value::String(section.into()));
    extras.insert("pslf_line".into(), number_value(rec.line_no as f64));
    extras.insert("pslf_raw".into(), string_array(rec.raw.iter().cloned()));
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

fn string_array(values: impl IntoIterator<Item = String>) -> Value {
    Value::Array(values.into_iter().map(Value::String).collect())
}

fn string_or_number(token: &str) -> Value {
    token
        .parse::<f64>()
        .ok()
        .map_or_else(|| Value::String(token.to_string()), number_value)
}

fn number_value(value: f64) -> Value {
    Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn num_at(tokens: &[String], i: usize, default: f64, field: &str, rec: &Record) -> Result<f64> {
    match tokens.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(tok) => tok.parse().map_err(|_| bad_field(field, i, tok, rec)),
    }
}

fn int_at(tokens: &[String], i: usize, default: i64, field: &str, rec: &Record) -> Result<i64> {
    match tokens.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(tok) => tok.parse().map_err(|_| bad_field(field, i, tok, rec)),
    }
}

fn id_at(tokens: &[String], i: usize, default: usize, field: &str, rec: &Record) -> Result<usize> {
    match tokens.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(tok) => parse_id(tok).ok_or_else(|| bad_field(field, i, tok, rec)),
    }
}

fn req_id(tokens: &[String], i: usize, field: &str, rec: &Record) -> Result<usize> {
    tokens
        .get(i)
        .and_then(|tok| parse_id(tok))
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: format!("{field} missing or invalid at line {}", rec.line_no),
        })
}

fn parse_id(tok: &str) -> Option<usize> {
    let value = tok.parse::<f64>().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value as usize)
}

fn on_at(tokens: &[String], i: usize, default: bool, field: &str, rec: &Record) -> Result<bool> {
    Ok(num_at(tokens, i, if default { 1.0 } else { 0.0 }, field, rec)? != 0.0)
}

fn bad_field(field: &str, i: usize, tok: &str, rec: &Record) -> Error {
    Error::FormatRead {
        format: FMT,
        message: format!(
            "{field} field {i} value {tok:?} is invalid at line {}",
            rec.line_no
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
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

        let mut warnings = Vec::new();
        let net = parse_pslf_source(Arc::new(epc.to_string()), None, &mut warnings).unwrap();

        assert_eq!(net.source_format, SourceFormat::Pslf);
        assert_eq!(net.buses.len(), 2);
        assert_eq!(net.branches.len(), 1);
        assert_eq!(net.loads.len(), 1);
        assert_eq!(net.generators.len(), 1);
        assert_eq!(net.shunts.len(), 1);
        assert_eq!(net.buses[0].kind, BusType::Ref);
        close(net.loads[0].p, 13.0);
        close(net.loads[0].q, 5.0);
        close(net.shunts[0].b, 10.0);
        assert!(warnings.iter().any(|w| w.contains("ZIP load")));
    }

    #[test]
    fn same_source_text_is_retained() {
        let epc = "title\nx\n!\nsolution parameters\nsbase 100\n!\nbus data [1]\n1 \"A\" 1 : 0 1 1 0 1 1 1.1 0.9\nend\n";
        let mut warnings = Vec::new();
        let net = parse_pslf_source(Arc::new(epc.to_string()), None, &mut warnings).unwrap();
        assert_eq!(net.source.as_deref().map(String::as_str), Some(epc));
    }
}
