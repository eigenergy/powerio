//! PSS/E RAWX revision 35.
//!
//! RAW and RAWX use the same PSS/E records. RAWX makes each record group a
//! JSON table whose `fields` array selects and orders the columns. This module
//! only translates that table syntax. The electrical conversion stays in the
//! `.raw` reader and writer in [`super::psse`].

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use powerio_core::ComponentId;

use super::{TextEmission, jnum};
use crate::diagnostics::{Diagnostics, codes};
use crate::network::{
    BalancedNetwork, BusId, BusbarSection, ComponentMetadata, ConnectivityNode,
    DetailedConnectivity, SourceFormat, Substation, Switch, SwitchKind, Terminal,
    TerminalReference, TopologyEndpoint, TopologyKind, TopologySwitch, VoltageLevel,
};
use crate::{Error, Result};

const FMT: &str = "PSS/E RAWX 35";

const CASE_FIELDS: &[&str] = &[
    "ic", "sbase", "rev", "xfrrat", "nxfrat", "basfrq", "title1", "title2",
];
const CASE_NUMERIC_DEFAULTS: &[(&str, &str)] = &[
    ("ic", "0"),
    ("sbase", "100"),
    ("rev", "35"),
    ("xfrrat", "0"),
    ("nxfrat", "1"),
    ("basfrq", "60"),
];
const BUS_FIELDS: &[&str] = &[
    "ibus", "name", "baskv", "ide", "area", "zone", "owner", "vm", "va", "nvhi", "nvlo", "evhi",
    "evlo",
];
const LOAD_FIELDS: &[&str] = &[
    "ibus", "loadid", "stat", "area", "zone", "pl", "ql", "ip", "iq", "yp", "yq", "owner", "scale",
    "intrpt", "dgenp", "dgenq", "dgenm", "loadtype",
];
const FIXED_SHUNT_FIELDS: &[&str] = &["ibus", "shntid", "stat", "gl", "bl"];
const GENERATOR_FIELDS: &[&str] = &[
    "ibus", "machid", "pg", "qg", "qt", "qb", "vs", "ireg", "nreg", "mbase", "zr", "zx", "rt",
    "xt", "gtap", "stat", "rmpct", "pt", "pb", "baslod", "o1", "f1", "o2", "f2", "o3", "f3", "o4",
    "f4", "wmod", "wpf",
];
const AC_LINE_FIELDS: &[&str] = &[
    "ibus", "jbus", "ckt", "rpu", "xpu", "bpu", "name", "rate1", "rate2", "rate3", "rate4",
    "rate5", "rate6", "rate7", "rate8", "rate9", "rate10", "rate11", "rate12", "gi", "bi", "gj",
    "bj", "stat", "met", "len", "o1", "f1", "o2", "f2", "o3", "f3", "o4", "f4",
];
const TRANSFORMER_MAIN_FIELDS: &[&str] = &[
    "ibus", "jbus", "kbus", "ckt", "cw", "cz", "cm", "mag1", "mag2", "nmet", "name", "stat", "o1",
    "f1", "o2", "f2", "o3", "f3", "o4", "f4", "vecgrp", "zcod",
];
const TRANSFORMER_IMPEDANCE_FIELDS: &[&str] = &[
    "r1_2", "x1_2", "sbase1_2", "r2_3", "x2_3", "sbase2_3", "r3_1", "x3_1", "sbase3_1", "vmstar",
    "anstar",
];
const AREA_FIELDS: &[&str] = &["iarea", "isw", "pdes", "ptol", "arname"];
const TWO_TERMINAL_DC_FIELDS: &[&str] = &[
    "name", "mdc", "rdc", "setvl", "vschd", "vcmod", "rcomp", "delti", "met", "dcvmin", "cccitmx",
    "cccacc", "ipr", "nbr", "anmxr", "anmnr", "rcr", "xcr", "ebasr", "trr", "tapr", "tmxr", "tmnr",
    "stpr", "icr", "ndr", "ifr", "itr", "idr", "xcapr", "ipi", "nbi", "anmxi", "anmni", "rci",
    "xci", "ebasi", "tri", "tapi", "tmxi", "tmni", "stpi", "ici", "ndi", "ifi", "iti", "idi",
    "xcapi",
];
const SWITCHED_SHUNT_FIELDS: &[&str] = &[
    "ibus", "shntid", "modsw", "adjm", "stat", "vswhi", "vswlo", "swreg", "nreg", "rmpct",
    "rmidnt", "binit", "s1", "n1", "b1", "s2", "n2", "b2", "s3", "n3", "b3", "s4", "n4", "b4",
    "s5", "n5", "b5", "s6", "n6", "b6", "s7", "n7", "b7", "s8", "n8", "b8",
];
const SYSTEM_SWITCH_RATING_FIELDS: &[&str] = &[
    "ibus", "jbus", "ckt", "xpu", "rate1", "rate2", "rate3", "rate4", "rate5", "rate6", "rate7",
    "rate8", "rate9", "rate10", "rate11", "rate12", "stat", "nstat", "met", "stype", "name",
];
const SYSTEM_SWITCH_RSET_FIELDS: &[&str] = &[
    "ibus", "jbus", "ckt", "xpu", "rsetnam", "stat", "nstat", "met", "stype", "name",
];
const SYSTEM_SWITCH_FIELDS: &[&str] = &[
    "ibus", "jbus", "ckt", "xpu", "rsetnam", "rate1", "rate2", "rate3", "rate4", "rate5", "rate6",
    "rate7", "rate8", "rate9", "rate10", "rate11", "rate12", "stat", "nstat", "met", "stype",
    "name",
];
const SUBSTATION_FIELDS: &[&str] = &["isub", "name", "lati", "long", "srg"];
const SUBSTATION_NODE_FIELDS: &[&str] = &["isub", "inode", "name", "ibus", "stat", "vm", "va"];
const SUBSTATION_SWITCH_FIELDS: &[&str] = &[
    "isub", "inode", "jnode", "swdid", "name", "type", "stat", "nstat", "xpu", "rate1", "rate2",
    "rate3",
];
const SUBSTATION_TERMINAL_FIELDS: &[&str] =
    &["isub", "inode", "type", "eqid", "ibus", "jbus", "kbus"];
const EXPLICIT_NULL: &str = "null";

const UNSUPPORTED_TABLES: &[(&str, &str)] = &[
    ("vscdc", "voltage source converter DC lines"),
    ("impcor", "transformer impedance correction tables"),
    ("ntermdc", "multi-terminal DC lines"),
    ("ntermdcconv", "multi-terminal DC converters"),
    ("ntermdcbus", "multi-terminal DC buses"),
    ("ntermdclink", "multi-terminal DC links"),
    ("msline", "multi-section line groups"),
    ("zone", "zones"),
    ("iatransfer", "interarea transfers"),
    ("owner", "owners"),
    ("facts", "FACTS devices"),
    ("gne", "GNE devices"),
    ("indmach", "induction machines"),
];

const KNOWN_NETWORK_MEMBERS: &[&str] = &[
    "caseid",
    "bus",
    "load",
    "fixshunt",
    "generator",
    "acline",
    "transformer",
    "area",
    "twotermdc",
    "swshunt",
    "sysswd",
    "sub",
    "subnode",
    "subswd",
    "subterm",
    "vscdc",
    "impcor",
    "ntermdc",
    "ntermdcconv",
    "ntermdcbus",
    "ntermdclink",
    "msline",
    "zone",
    "iatransfer",
    "owner",
    "facts",
    "gne",
    "indmach",
];

#[derive(Debug)]
struct Table<'a> {
    name: &'static str,
    fields: Vec<String>,
    columns: BTreeMap<String, usize>,
    rows: &'a [Value],
    repaired_powysbl_subswd_ratings: bool,
}

impl<'a> Table<'a> {
    fn parse(network: &'a Map<String, Value>, name: &'static str) -> Result<Option<Self>> {
        let Some(value) = network.get(name) else {
            return Ok(None);
        };
        let object = value
            .as_object()
            .ok_or_else(|| malformed(format!("RAWX table `{name}` must be an object")))?;
        let mut fields = object
            .get("fields")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed(format!("RAWX table `{name}` has no fields array")))?
            .iter()
            .enumerate()
            .map(|(index, value)| {
                value.as_str().map(str::to_ascii_lowercase).ok_or_else(|| {
                    malformed(format!(
                        "RAWX table `{name}` field name {index} is not a string"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let rows = object
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed(format!("RAWX table `{name}` has no data array")))?;

        // PowSybl's twoSubstations_rev35 fixture declares `rsetnam` as the
        // tenth and final field but supplies three numeric values after XPU.
        // Its own reader accepts the rows and ignores the extra values. Read
        // those values as RATE1/RATE2/RATE3, which are the PSS/E switching
        // device fields at those positions.
        let repaired_powysbl_subswd_ratings = name == "subswd"
            && fields
                == [
                    "isub", "inode", "jnode", "swdid", "name", "type", "stat", "nstat", "xpu",
                    "rsetnam",
                ]
            && rows
                .iter()
                .all(|row| row.as_array().is_some_and(|values| values.len() == 12));
        if repaired_powysbl_subswd_ratings {
            fields.pop();
            fields.extend(["rate1", "rate2", "rate3"].map(str::to_owned));
        }

        let mut columns = BTreeMap::new();
        for (index, field) in fields.iter().enumerate() {
            if field.is_empty() {
                return Err(malformed(format!(
                    "RAWX table `{name}` contains an empty field name"
                )));
            }
            if columns.insert(field.clone(), index).is_some() {
                return Err(malformed(format!(
                    "RAWX table `{name}` repeats field `{field}`"
                )));
            }
        }
        for (row_index, row) in rows.iter().enumerate() {
            let row = row.as_array().ok_or_else(|| {
                malformed(format!(
                    "RAWX table `{name}` row {row_index} is not an array"
                ))
            })?;
            if row.len() != fields.len() {
                return Err(malformed(format!(
                    "RAWX table `{name}` row {row_index} has {} values for {} fields",
                    row.len(),
                    fields.len()
                )));
            }
        }
        Ok(Some(Self {
            name,
            fields,
            columns,
            rows,
            repaired_powysbl_subswd_ratings,
        }))
    }

    fn value<'r>(&self, row: &'r Value, field: &str) -> Option<&'r Value> {
        let index = *self.columns.get(field)?;
        row.as_array()?.get(index)
    }

    fn warn_unknown_fields(&self, supported: &[&str], warnings: &mut Diagnostics) {
        let supported: BTreeSet<&str> = supported.iter().copied().collect();
        let unknown: Vec<&str> = self
            .fields
            .iter()
            .map(String::as_str)
            .filter(|field| !supported.contains(field))
            .collect();
        if !unknown.is_empty() {
            warnings.push(
                &codes::READ_PSSE_FIELD_DROPPED,
                format!(
                    "PSS/E RAWX table `{}` fields {} are not modeled; retained only in the same format source",
                    self.name,
                    unknown.join(", ")
                ),
            );
        }
    }
}

fn malformed(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}

/// Parse RAWX JSON, then run the shared PSS/E revision 35 electrical reader.
#[expect(clippy::too_many_lines)]
pub(super) fn parse_rawx_source(
    source: &str,
    name_hint: Option<&str>,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let root: Value = serde_json::from_str(source.trim_start_matches('\u{feff}'))
        .map_err(|error| malformed(format!("invalid JSON: {error}")))?;
    let network = root
        .get("network")
        .and_then(Value::as_object)
        .ok_or_else(|| malformed("RAWX root must contain a `network` object"))?;
    let case_object = network
        .get("caseid")
        .ok_or_else(|| malformed("RAWX network has no `caseid` parameter set"))?
        .as_object()
        .ok_or_else(|| malformed("RAWX `caseid` parameter set must be an object"))?;
    let case_fields = case_object
        .get("fields")
        .ok_or_else(|| malformed("RAWX caseid has no fields array"))?
        .as_array()
        .ok_or_else(|| malformed("RAWX caseid fields must be an array"))?;
    let case_data = case_object
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("RAWX caseid has no data array"))?;
    if case_fields.len() != case_data.len() {
        return Err(malformed(format!(
            "RAWX caseid has {} values for {} fields",
            case_data.len(),
            case_fields.len()
        )));
    }
    let mut case_columns = BTreeMap::new();
    for (index, field) in case_fields.iter().enumerate() {
        let field = field
            .as_str()
            .ok_or_else(|| malformed(format!("RAWX caseid field {index} is not a string")))?
            .to_ascii_lowercase();
        if case_columns.insert(field.clone(), index).is_some() {
            return Err(malformed(format!("RAWX caseid repeats field `{field}`")));
        }
    }
    warn_caseid_fields(case_object, &case_columns, warnings);
    let case_value = |field: &str| {
        case_columns
            .get(field)
            .and_then(|index| case_data.get(*index))
    };
    for (field, default) in [("ic", 0.0), ("xfrrat", 0.0), ("nxfrat", 1.0)] {
        let value = numeric_value(case_value(field), &format!("caseid.{field}"), Some(default))?;
        if !value.eq(&default) {
            warnings.push(
                &codes::READ_PSSE_FIELD_DROPPED,
                format!(
                    "PSS/E RAWX `caseid.{field}` value {value} is not retained in the balanced network; retained only in the same format source"
                ),
            );
        }
    }
    if let Some(title2) = string_value(case_value("title2"), "caseid.title2")?
        && !title2.is_empty()
    {
        warnings.push(
            &codes::READ_PSSE_FIELD_DROPPED,
            "PSS/E RAWX `caseid.title2` is not retained in the balanced network; retained only in the same format source",
        );
    }
    let revision = integer_value(case_value("rev"), "caseid.rev", Some(35))?;
    if revision != 35 {
        return Err(malformed(format!(
            "RAWX revision {revision} is unsupported; expected revision 35"
        )));
    }
    let mut substituted_strings = 0usize;
    let mut case_tokens = Vec::with_capacity(6);
    for (field, default) in CASE_NUMERIC_DEFAULTS {
        let token = raw_token(
            case_value(field),
            false,
            "caseid",
            field,
            &mut substituted_strings,
        )?;
        case_tokens.push(if token.is_empty() {
            (*default).to_owned()
        } else {
            token
        });
    }
    let title = string_value(case_value("title1"), "caseid.title1")?.unwrap_or("");
    let title = sanitize_record_text(title, &mut substituted_strings);
    let mut raw = format!("{}\n{}\n\n", case_tokens.join(", "), title);
    raw.push_str("0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA\n");

    append_simple_table(
        network,
        "bus",
        BUS_FIELDS,
        &["name"],
        &mut raw,
        warnings,
        &mut substituted_strings,
    )?;
    raw.push_str("0 / END OF BUS DATA, BEGIN LOAD DATA\n");
    append_simple_table(
        network,
        "load",
        LOAD_FIELDS,
        &["loadid", "loadtype"],
        &mut raw,
        warnings,
        &mut substituted_strings,
    )?;
    raw.push_str("0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\n");
    append_simple_table(
        network,
        "fixshunt",
        FIXED_SHUNT_FIELDS,
        &["shntid"],
        &mut raw,
        warnings,
        &mut substituted_strings,
    )?;
    raw.push_str("0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA\n");
    append_simple_table(
        network,
        "generator",
        GENERATOR_FIELDS,
        &["machid"],
        &mut raw,
        warnings,
        &mut substituted_strings,
    )?;
    raw.push_str("0 / END OF GENERATOR DATA, BEGIN BRANCH DATA\n");
    append_simple_table(
        network,
        "acline",
        AC_LINE_FIELDS,
        &["ckt", "name"],
        &mut raw,
        warnings,
        &mut substituted_strings,
    )?;
    raw.push_str("0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA\n");
    append_transformers(network, &mut raw, warnings, &mut substituted_strings)?;
    raw.push_str("0 / END OF TRANSFORMER DATA, BEGIN AREA DATA\n");
    append_simple_table(
        network,
        "area",
        AREA_FIELDS,
        &["arname"],
        &mut raw,
        warnings,
        &mut substituted_strings,
    )?;
    raw.push_str("0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA\n");
    append_two_terminal_dc(network, &mut raw, warnings, &mut substituted_strings)?;
    raw.push_str("0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA\n");
    raw.push_str("0 / END OF VSC DC LINE DATA, BEGIN IMPEDANCE CORRECTION DATA\n");
    raw.push_str("0 / END OF IMPEDANCE CORRECTION DATA, BEGIN MULTI-TERMINAL DC DATA\n");
    raw.push_str("0 / END OF MULTI-TERMINAL DC DATA, BEGIN MULTI-SECTION LINE DATA\n");
    raw.push_str("0 / END OF MULTI-SECTION LINE DATA, BEGIN ZONE DATA\n");
    raw.push_str("0 / END OF ZONE DATA, BEGIN INTER-AREA TRANSFER DATA\n");
    raw.push_str("0 / END OF INTER-AREA TRANSFER DATA, BEGIN OWNER DATA\n");
    raw.push_str("0 / END OF OWNER DATA, BEGIN FACTS DEVICE DATA\n");
    raw.push_str("0 / END OF FACTS DEVICE DATA, BEGIN SWITCHED SHUNT DATA\n");
    append_simple_table(
        network,
        "swshunt",
        SWITCHED_SHUNT_FIELDS,
        &["shntid", "rmidnt"],
        &mut raw,
        warnings,
        &mut substituted_strings,
    )?;
    raw.push_str("0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA\n");
    raw.push_str("0 / END OF GNE DEVICE DATA, BEGIN INDUCTION MACHINE DATA\n");
    raw.push_str("0 / END OF INDUCTION MACHINE DATA\nQ\n");

    if substituted_strings > 0 {
        warnings.push(
            &codes::READ_PSSE_VALUE_SUBSTITUTED,
            format!(
                "{substituted_strings} RAWX string value(s) contained an apostrophe, newline, or carriage return that the shared PSS/E record reader cannot carry; replaced with spaces"
            ),
        );
    }
    warn_unsupported_tables(network, warnings)?;
    warn_unknown_tables(network, warnings);

    // The shared reader decodes text synthesized here, not the RAWX document,
    // so its record offsets name nothing in the retained buffer and no span is
    // attached while it runs.
    let location = warnings.suspend_location();
    let parsed =
        super::psse::parse_psse_source_deferred_regulating_nodes(&raw, name_hint, warnings);
    warnings.resume_location(location);
    let mut parsed = parsed?;
    *parsed.source_format_mut() = SourceFormat::PsseRawx;
    read_system_switches(network, &mut parsed, warnings)?;
    parsed.assign_missing_component_ids();
    read_detailed_connectivity(network, &mut parsed, warnings)?;
    let detailed = parsed.detailed_connectivity().clone();
    apply_regulating_terminals(network, &mut parsed, detailed.as_deref(), warnings)?;
    parsed.check_references(FMT)?;
    Ok(parsed)
}

fn append_simple_table(
    network: &Map<String, Value>,
    name: &'static str,
    fields: &[&str],
    strings: &[&str],
    out: &mut String,
    warnings: &mut Diagnostics,
    substituted_strings: &mut usize,
) -> Result<()> {
    let Some(table) = Table::parse(network, name)? else {
        return Ok(());
    };
    table.warn_unknown_fields(fields, warnings);
    for row in table.rows {
        out.push_str(&record_line(
            &table,
            row,
            fields,
            strings,
            substituted_strings,
        )?);
        out.push('\n');
    }
    Ok(())
}

fn record_line(
    table: &Table<'_>,
    row: &Value,
    fields: &[&str],
    strings: &[&str],
    substituted_strings: &mut usize,
) -> Result<String> {
    fields
        .iter()
        .map(|field| {
            raw_token(
                table.value(row, field),
                strings.contains(field),
                table.name,
                field,
                substituted_strings,
            )
        })
        .collect::<Result<Vec<_>>>()
        .map(|tokens| tokens.join(", "))
}

fn append_transformers(
    network: &Map<String, Value>,
    out: &mut String,
    warnings: &mut Diagnostics,
    substituted_strings: &mut usize,
) -> Result<()> {
    let Some(table) = Table::parse(network, "transformer")? else {
        return Ok(());
    };
    let all_fields = transformer_fields();
    let supported: Vec<&str> = all_fields.iter().map(String::as_str).collect();
    table.warn_unknown_fields(&supported, warnings);
    for row in table.rows {
        out.push_str(&record_line(
            &table,
            row,
            TRANSFORMER_MAIN_FIELDS,
            &["ckt", "name", "vecgrp"],
            substituted_strings,
        )?);
        out.push('\n');
        out.push_str(&record_line(
            &table,
            row,
            TRANSFORMER_IMPEDANCE_FIELDS,
            &[],
            substituted_strings,
        )?);
        out.push('\n');
        for winding in 1..=2 {
            let fields = winding_fields(winding);
            let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
            out.push_str(&record_line(&table, row, &refs, &[], substituted_strings)?);
            out.push('\n');
        }
        if integer_value(table.value(row, "kbus"), "transformer.kbus", Some(0))? != 0 {
            let fields = winding_fields(3);
            let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
            out.push_str(&record_line(&table, row, &refs, &[], substituted_strings)?);
            out.push('\n');
        }
    }
    Ok(())
}

fn append_two_terminal_dc(
    network: &Map<String, Value>,
    out: &mut String,
    warnings: &mut Diagnostics,
    substituted_strings: &mut usize,
) -> Result<()> {
    let Some(table) = Table::parse(network, "twotermdc")? else {
        return Ok(());
    };
    table.warn_unknown_fields(TWO_TERMINAL_DC_FIELDS, warnings);
    for row in table.rows {
        for (fields, strings) in [
            (&TWO_TERMINAL_DC_FIELDS[..12], &["name", "met"][..]),
            (&TWO_TERMINAL_DC_FIELDS[12..30], &["idr"][..]),
            (&TWO_TERMINAL_DC_FIELDS[30..], &["idi"][..]),
        ] {
            out.push_str(&record_line(
                &table,
                row,
                fields,
                strings,
                substituted_strings,
            )?);
            out.push('\n');
        }
    }
    Ok(())
}

fn raw_token(
    value: Option<&Value>,
    string_field: bool,
    table: &str,
    field: &str,
    substituted_strings: &mut usize,
) -> Result<String> {
    let Some(value) = value else {
        return Ok(String::new());
    };
    if value.is_null() {
        return Ok(String::new());
    }
    if string_field {
        let text = match value {
            Value::String(text) => text.as_str(),
            Value::Number(number) => return Ok(format!("'{number}'")),
            _ => {
                return Err(malformed(format!(
                    "RAWX `{table}.{field}` must be a string or number"
                )));
            }
        };
        return Ok(format!(
            "'{}'",
            sanitize_record_text(text, substituted_strings)
        ));
    }
    match value {
        Value::Number(number) => Ok(number.to_string()),
        // PSS/E tools in the wild write numeric fields as JSON strings. The
        // shared record reader validates their numeric spelling.
        Value::String(text) => {
            let text = text.trim();
            text.parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(|_| text.to_owned())
                .ok_or_else(|| malformed(format!("RAWX `{table}.{field}` is not a finite number")))
        }
        _ => Err(malformed(format!(
            "RAWX `{table}.{field}` must be numeric or null"
        ))),
    }
}

fn sanitize_record_text<'a>(text: &'a str, substitutions: &mut usize) -> std::borrow::Cow<'a, str> {
    if text.contains(['\'', '\n', '\r']) {
        *substitutions += 1;
        std::borrow::Cow::Owned(text.replace(['\'', '\n', '\r'], " "))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

fn numeric_value(value: Option<&Value>, field: &str, default: Option<f64>) -> Result<f64> {
    match value {
        None | Some(Value::Null) => default.ok_or_else(|| malformed(format!("missing `{field}`"))),
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| malformed(format!("`{field}` is not a finite number"))),
        Some(Value::String(text)) => text
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| malformed(format!("`{field}` is not a finite number"))),
        Some(_) => Err(malformed(format!("`{field}` is not numeric"))),
    }
}

fn integer_value(value: Option<&Value>, field: &str, default: Option<i64>) -> Result<i64> {
    let value = numeric_value(value, field, default.map(|value| value as f64))?;
    if value.fract() != 0.0 || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(malformed(format!("`{field}` is not an integer")));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(value as i64)
}

fn string_value<'a>(value: Option<&'a Value>, field: &str) -> Result<Option<&'a str>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(malformed(format!("`{field}` is not a string"))),
    }
}

fn warn_caseid_fields(
    case_object: &Map<String, Value>,
    columns: &BTreeMap<String, usize>,
    warnings: &mut Diagnostics,
) {
    let known: BTreeSet<&str> = CASE_FIELDS.iter().copied().collect();
    let unknown = columns
        .keys()
        .map(String::as_str)
        .filter(|field| !known.contains(field))
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        warnings.push(
            &codes::READ_PSSE_FIELD_DROPPED,
            format!(
                "PSS/E RAWX `caseid` fields {} are not modeled; retained only in the same format source",
                unknown.join(", ")
            ),
        );
    }

    let unknown_members = case_object
        .keys()
        .map(String::as_str)
        .filter(|member| !matches!(*member, "fields" | "data"))
        .collect::<Vec<_>>();
    if !unknown_members.is_empty() {
        warnings.push(
            &codes::READ_PSSE_FIELD_DROPPED,
            format!(
                "PSS/E RAWX `caseid` object members {} are not modeled; retained only in the same format source",
                unknown_members.join(", ")
            ),
        );
    }
}

fn warn_unknown_tables(network: &Map<String, Value>, warnings: &mut Diagnostics) {
    let known: BTreeSet<&str> = KNOWN_NETWORK_MEMBERS.iter().copied().collect();
    for (name, value) in network {
        if known.contains(name.as_str()) || !rawx_member_has_content(value) {
            continue;
        }
        let count = value
            .as_object()
            .and_then(|object| object.get("data"))
            .and_then(Value::as_array)
            .map(Vec::len);
        let message = count.map_or_else(
            || {
                format!(
                    "PSS/E RAWX network member `{name}` is not modeled; retained only in the same format source"
                )
            },
            |count| {
                format!(
                    "PSS/E RAWX `{name}` table contains {count} unmodeled record(s); retained only in the same format source"
                )
            },
        );
        warnings.push(&codes::READ_PSSE_SECTION_UNSUPPORTED, message);
    }
}

fn rawx_member_has_content(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(_) => true,
        Value::String(value) => !value.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(object) => object
            .get("data")
            .and_then(Value::as_array)
            .map_or(!object.is_empty(), |rows| !rows.is_empty()),
    }
}

fn warn_unsupported_tables(network: &Map<String, Value>, warnings: &mut Diagnostics) -> Result<()> {
    for (name, description) in UNSUPPORTED_TABLES {
        let Some(table) = Table::parse(network, name)? else {
            continue;
        };
        if !table.rows.is_empty() {
            warnings.push(
                &codes::READ_PSSE_SECTION_UNSUPPORTED,
                format!(
                    "PSS/E RAWX `{name}` table contains {} {description} record(s); retained only in a same format source echo",
                    table.rows.len()
                ),
            );
        }
    }
    Ok(())
}

fn read_system_switches(
    network: &Map<String, Value>,
    parsed: &mut BalancedNetwork,
    warnings: &mut Diagnostics,
) -> Result<()> {
    let Some(table) = Table::parse(network, "sysswd")? else {
        return Ok(());
    };
    table.warn_unknown_fields(SYSTEM_SWITCH_FIELDS, warnings);
    for (index, row) in table.rows.iter().enumerate() {
        let from = numeric_value(table.value(row, "ibus"), "sysswd.ibus", None)?;
        let to = numeric_value(table.value(row, "jbus"), "sysswd.jbus", None)?;
        let from = crate::format::id_from_f64(from, "sysswd.ibus").map_err(malformed)?;
        let to = crate::format::id_from_f64(to, "sysswd.jbus").map_err(malformed)?;
        let mut switch = Switch::new(
            BusId(from),
            BusId(to),
            integer_value(table.value(row, "stat"), "sysswd.stat", Some(1))? != 0,
        );
        let rate = numeric_value(table.value(row, "rate1"), "sysswd.rate1", Some(0.0))?;
        switch.thermal_rating = (rate > 0.0).then_some(rate);
        for field in [
            "ckt", "xpu", "rate2", "rate3", "rate4", "rate5", "rate6", "rate7", "rate8", "rate9",
            "rate10", "rate11", "rate12", "rsetnam", "nstat", "met", "stype", "name",
        ] {
            if let Some(value) = table.value(row, field).filter(|value| !value.is_null()) {
                switch.extras.insert(format!("psse_{field}"), value.clone());
            }
        }
        if switch.uid.is_none() {
            let ckt = table
                .value(row, "ckt")
                .and_then(Value::as_str)
                .unwrap_or("1");
            switch.uid = Some(format!("{}-{}-{ckt}", switch.from, switch.to));
        }
        parsed.switches_mut().push(switch);
        if table
            .value(row, "xpu")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            != 0.0
        {
            warnings.push(
                &codes::READ_PSSE_RETAINED_SOURCE_ONLY,
                format!(
                    "PSS/E RAWX system switching device {} ({} to {}) carries XPU in extras; the typed switch model has no impedance field",
                    index + 1,
                    from,
                    to
                ),
            );
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RawxEquipmentKey {
    equipment_type: String,
    buses: [usize; 3],
    source_id: String,
}

impl RawxEquipmentKey {
    fn new(equipment_type: &str, ibus: usize, jbus: usize, kbus: usize, source_id: String) -> Self {
        let mut buses = [ibus, jbus, kbus];
        buses.sort_unstable();
        Self {
            equipment_type: equipment_type.to_ascii_uppercase(),
            buses,
            source_id,
        }
    }

    fn local_id(&self) -> String {
        format!(
            "{}.{}.{}.{}.{}",
            self.equipment_type, self.buses[0], self.buses[1], self.buses[2], self.source_id
        )
    }
}

fn component_id(component_type: &str, local_id: impl Into<String>) -> Result<ComponentId> {
    ComponentId::new(component_type, local_id).map_err(|error| malformed(error.to_string()))
}

fn table_bus(table: &Table<'_>, row: &Value, field: &str) -> Result<usize> {
    let value = numeric_value(
        table.value(row, field),
        &format!("{}.{field}", table.name),
        Some(0.0),
    )?;
    crate::format::id_from_f64(value, format!("{}.{field}", table.name)).map_err(malformed)
}

fn table_text(table: &Table<'_>, row: &Value, field: &str, default: &str) -> Result<String> {
    match table.value(row, field) {
        None | Some(Value::Null) => Ok(default.to_owned()),
        Some(Value::String(value)) => Ok(value.trim().to_owned()),
        Some(Value::Number(value)) => Ok(value.to_string()),
        Some(_) => Err(malformed(format!(
            "RAWX `{}.{field}` must be a string or number",
            table.name
        ))),
    }
}

fn table_equipment_key(
    table: &Table<'_>,
    row: &Value,
    equipment_type: &str,
    id_field: &str,
    bus_fields: &[&str],
) -> Result<RawxEquipmentKey> {
    let ibus = bus_fields
        .first()
        .map(|field| table_bus(table, row, field))
        .transpose()?
        .unwrap_or(0);
    let jbus = bus_fields
        .get(1)
        .map(|field| table_bus(table, row, field))
        .transpose()?
        .unwrap_or(0);
    let kbus = bus_fields
        .get(2)
        .map(|field| table_bus(table, row, field))
        .transpose()?
        .unwrap_or(0);
    Ok(RawxEquipmentKey::new(
        equipment_type,
        ibus,
        jbus,
        kbus,
        table_text(table, row, id_field, "1")?,
    ))
}

fn stable_id(
    component_type: &str,
    uid: Option<&str>,
    table: &str,
    row: usize,
) -> Result<ComponentId> {
    let uid = uid.ok_or_else(|| {
        malformed(format!(
            "internal RAWX {table} row {} has no stable component identity",
            row + 1
        ))
    })?;
    component_id(component_type, uid)
}

fn insert_equipment_mapping(
    mappings: &mut BTreeMap<RawxEquipmentKey, ComponentId>,
    key: RawxEquipmentKey,
    component: ComponentId,
) -> Result<()> {
    if mappings.contains_key(&key) {
        return Err(malformed(format!(
            "RAWX equipment identity `{}` is repeated",
            key.local_id()
        )));
    }
    mappings.insert(key, component);
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn rawx_equipment_mappings(
    source: &Map<String, Value>,
    parsed: &BalancedNetwork,
) -> Result<BTreeMap<RawxEquipmentKey, ComponentId>> {
    let mut mappings = BTreeMap::new();

    if let Some(table) = Table::parse(source, "load")? {
        if table.rows.len() != parsed.loads().len() {
            return Err(malformed("RAWX load table did not map one for one"));
        }
        for (index, (row, load)) in table.rows.iter().zip(parsed.loads()).enumerate() {
            insert_equipment_mapping(
                &mut mappings,
                table_equipment_key(&table, row, "L", "loadid", &["ibus"])?,
                stable_id("load", load.uid.as_deref(), "load", index)?,
            )?;
        }
    }

    let fixed_shunt_count = Table::parse(source, "fixshunt")?.map_or(0, |table| table.rows.len());
    if let Some(table) = Table::parse(source, "fixshunt")? {
        if fixed_shunt_count > parsed.shunts().len() {
            return Err(malformed("RAWX fixed shunt table did not map one for one"));
        }
        for (index, (row, shunt)) in table
            .rows
            .iter()
            .zip(parsed.shunts().iter().take(fixed_shunt_count))
            .enumerate()
        {
            insert_equipment_mapping(
                &mut mappings,
                table_equipment_key(&table, row, "F", "shntid", &["ibus"])?,
                stable_id("shunt", shunt.uid.as_deref(), "fixshunt", index)?,
            )?;
        }
    }

    if let Some(table) = Table::parse(source, "generator")? {
        if table.rows.len() != parsed.generators().len() {
            return Err(malformed("RAWX generator table did not map one for one"));
        }
        for (index, (row, generator)) in table.rows.iter().zip(parsed.generators()).enumerate() {
            insert_equipment_mapping(
                &mut mappings,
                table_equipment_key(&table, row, "M", "machid", &["ibus"])?,
                stable_id("generator", generator.uid.as_deref(), "generator", index)?,
            )?;
        }
    }

    let line_count = Table::parse(source, "acline")?.map_or(0, |table| table.rows.len());
    if let Some(table) = Table::parse(source, "acline")? {
        if line_count > parsed.branches().len() {
            return Err(malformed("RAWX AC line table did not map one for one"));
        }
        for (index, (row, branch)) in table
            .rows
            .iter()
            .zip(parsed.branches().iter().take(line_count))
            .enumerate()
        {
            insert_equipment_mapping(
                &mut mappings,
                table_equipment_key(&table, row, "B", "ckt", &["ibus", "jbus"])?,
                stable_id("branch", branch.uid.as_deref(), "acline", index)?,
            )?;
        }
    }

    if let Some(table) = Table::parse(source, "transformer")? {
        let mut two_winding = parsed.branches().iter().skip(line_count);
        let mut three_winding = parsed.transformers_3w().iter();
        for (index, row) in table.rows.iter().enumerate() {
            let kbus = table_bus(&table, row, "kbus")?;
            let component = if kbus == 0 {
                let transformer = two_winding.next().ok_or_else(|| {
                    malformed("RAWX two winding transformer table did not map one for one")
                })?;
                stable_id(
                    "transformer",
                    transformer.uid.as_deref(),
                    "transformer",
                    index,
                )?
            } else {
                let transformer = three_winding.next().ok_or_else(|| {
                    malformed("RAWX three winding transformer table did not map one for one")
                })?;
                stable_id(
                    "transformer",
                    transformer.uid.as_deref(),
                    "transformer",
                    index,
                )?
            };
            insert_equipment_mapping(
                &mut mappings,
                table_equipment_key(
                    &table,
                    row,
                    if kbus == 0 { "2" } else { "3" },
                    "ckt",
                    &["ibus", "jbus", "kbus"],
                )?,
                component,
            )?;
        }
        if two_winding.next().is_some() || three_winding.next().is_some() {
            return Err(malformed("RAWX transformer table did not map one for one"));
        }
    }

    if let Some(table) = Table::parse(source, "swshunt")? {
        let switched = parsed.shunts().iter().skip(fixed_shunt_count);
        if table.rows.len() != switched.clone().count() {
            return Err(malformed(
                "RAWX switched shunt table did not map one for one",
            ));
        }
        for (index, (row, shunt)) in table.rows.iter().zip(switched).enumerate() {
            insert_equipment_mapping(
                &mut mappings,
                table_equipment_key(&table, row, "S", "shntid", &["ibus"])?,
                stable_id("shunt", shunt.uid.as_deref(), "swshunt", index)?,
            )?;
        }
    }

    if let Some(table) = Table::parse(source, "twotermdc")? {
        if table.rows.len() != parsed.hvdc().len() {
            return Err(malformed(
                "RAWX two terminal DC table did not map one for one",
            ));
        }
        for (index, (row, line)) in table.rows.iter().zip(parsed.hvdc()).enumerate() {
            insert_equipment_mapping(
                &mut mappings,
                table_equipment_key(&table, row, "D", "name", &["ipr", "ipi"])?,
                stable_id("hvdc", line.uid.as_deref(), "twotermdc", index)?,
            )?;
        }
    }

    Ok(mappings)
}

#[derive(Clone)]
struct RawxNode {
    substation: i64,
    source_node: i32,
    component: ComponentId,
    voltage_level: ComponentId,
}

pub(super) fn regulating_terminal(
    detailed: &DetailedConnectivity,
    bus: BusId,
    source_node: i32,
) -> Option<TerminalReference> {
    let node = detailed
        .connectivity_nodes
        .iter()
        .find(|node| node.calculated_bus == Some(bus) && node.node_number == Some(source_node))?;
    detailed
        .terminals
        .iter()
        .filter(|terminal| terminal.node.as_ref() == Some(&node.component))
        // A busbar section is the exact logical target when one represents the
        // PSS/E node. Otherwise use an equipment terminal connected to it.
        .min_by_key(|terminal| usize::from(terminal.equipment.component_type() != "busbar_section"))
        .map(|terminal| TerminalReference {
            equipment: terminal.equipment.clone(),
            terminal: terminal.terminal,
        })
}

#[allow(clippy::too_many_lines)]
fn apply_regulating_terminals(
    source: &Map<String, Value>,
    parsed: &mut BalancedNetwork,
    detailed: Option<&DetailedConnectivity>,
    warnings: &mut Diagnostics,
) -> Result<()> {
    let mut assign = |target: &mut Option<TerminalReference>,
                      bus: BusId,
                      node: i32,
                      description: String| {
        if node == 0 {
            return;
        }
        if let Some(reference) =
            detailed.and_then(|detailed| regulating_terminal(detailed, bus, node))
        {
            *target = Some(reference);
        } else {
            warnings.push(
                &codes::READ_PSSE_REFERENCE_DROPPED,
                format!(
                    "{description}: node {node} at regulated bus {bus} has no detailed connectivity terminal"
                ),
            );
        }
    };

    if let Some(table) = Table::parse(source, "generator")? {
        for (index, row) in table.rows.iter().zip(parsed.generators_mut()) {
            let ireg = BusId(table_bus(&table, index, "ireg")?);
            let node = integer_value(table.value(index, "nreg"), "generator.nreg", Some(0))?;
            let node = i32::try_from(node)
                .map_err(|_| malformed("RAWX generator.nreg is outside the i32 range"))?;
            let bus = if ireg.0 == 0 { row.bus } else { ireg };
            assign(
                &mut row.regulating_terminal,
                bus,
                node,
                format!("PSS/E generator at bus {}", row.bus),
            );
        }
    }

    let fixed_shunts = Table::parse(source, "fixshunt")?.map_or(0, |table| table.rows.len());
    if let Some(table) = Table::parse(source, "swshunt")? {
        for (index, shunt) in table
            .rows
            .iter()
            .zip(parsed.shunts_mut().iter_mut().skip(fixed_shunts))
        {
            let Some(control) = shunt.control.as_mut() else {
                continue;
            };
            let swreg = BusId(table_bus(&table, index, "swreg")?);
            let node = integer_value(table.value(index, "nreg"), "swshunt.nreg", Some(0))?;
            let node = i32::try_from(node)
                .map_err(|_| malformed("RAWX swshunt.nreg is outside the i32 range"))?;
            let bus = if swreg.0 == 0 { shunt.bus } else { swreg };
            assign(
                &mut control.regulating_terminal,
                bus,
                node,
                format!("PSS/E switched shunt at bus {}", shunt.bus),
            );
        }
    }

    let line_count = Table::parse(source, "acline")?.map_or(0, |table| table.rows.len());
    if let Some(table) = Table::parse(source, "transformer")? {
        let mut two_winding_index = 0;
        let mut three_winding_index = 0;
        for row in table.rows {
            let kbus = table_bus(&table, row, "kbus")?;
            if kbus == 0 {
                let transformer = parsed
                    .branches_mut()
                    .get_mut(line_count + two_winding_index)
                    .ok_or_else(|| {
                        malformed("RAWX two winding transformer controls did not map one for one")
                    })?;
                two_winding_index += 1;
                if let Some(control) = transformer.control.as_mut() {
                    let node =
                        integer_value(table.value(row, "node1"), "transformer.node1", Some(0))?;
                    let node = i32::try_from(node).map_err(|_| {
                        malformed("RAWX transformer.node1 is outside the i32 range")
                    })?;
                    let bus = control.controlled_bus.unwrap_or(transformer.from);
                    assign(
                        &mut control.regulating_terminal,
                        bus,
                        node,
                        format!(
                            "PSS/E two winding transformer {}-{}",
                            transformer.from, transformer.to
                        ),
                    );
                }
            } else {
                let transformer = parsed
                    .transformers_3w_mut()
                    .get_mut(three_winding_index)
                    .ok_or_else(|| {
                        malformed("RAWX three winding transformer controls did not map one for one")
                    })?;
                three_winding_index += 1;
                for (winding_index, winding) in transformer.windings.iter_mut().enumerate() {
                    let Some(control) = winding.control.as_mut() else {
                        continue;
                    };
                    let field = format!("node{}", winding_index + 1);
                    let node = integer_value(
                        table.value(row, &field),
                        "transformer winding node",
                        Some(0),
                    )?;
                    let node = i32::try_from(node).map_err(|_| {
                        malformed("RAWX transformer winding node is outside the i32 range")
                    })?;
                    let bus = control.controlled_bus.unwrap_or(winding.bus);
                    assign(
                        &mut control.regulating_terminal,
                        bus,
                        node,
                        format!(
                            "PSS/E three winding transformer winding {} at bus {}",
                            winding_index + 1,
                            winding.bus
                        ),
                    );
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn read_detailed_connectivity(
    source: &Map<String, Value>,
    parsed: &mut BalancedNetwork,
    warnings: &mut Diagnostics,
) -> Result<()> {
    let substation_table = Table::parse(source, "sub")?;
    let node_table = Table::parse(source, "subnode")?;
    let switch_table = Table::parse(source, "subswd")?;
    let terminal_table = Table::parse(source, "subterm")?;
    if substation_table.is_none()
        && node_table.is_none()
        && switch_table.is_none()
        && terminal_table.is_none()
    {
        return Ok(());
    }
    let Some(substation_table) = substation_table else {
        return Err(malformed("RAWX node breaker tables require a `sub` table"));
    };
    substation_table.warn_unknown_fields(SUBSTATION_FIELDS, warnings);
    if let Some(table) = &node_table {
        table.warn_unknown_fields(SUBSTATION_NODE_FIELDS, warnings);
    }
    if let Some(table) = &switch_table {
        if table.repaired_powysbl_subswd_ratings {
            warnings.push(
                &codes::READ_PSSE_VALUE_SUBSTITUTED,
                "PSS/E RAWX `subswd` declared `rsetnam` as its final field but supplied three numeric values after `xpu`; read them as `rate1`, `rate2`, and `rate3`",
            );
        }
        table.warn_unknown_fields(SUBSTATION_SWITCH_FIELDS, warnings);
    }
    if let Some(table) = &terminal_table {
        table.warn_unknown_fields(SUBSTATION_TERMINAL_FIELDS, warnings);
    }

    let mut detailed = DetailedConnectivity::default();
    let mut metadata = parsed
        .detailed_connectivity()
        .as_deref()
        .map(|detailed| {
            detailed
                .component_metadata
                .iter()
                .cloned()
                .map(|metadata| (metadata.component.clone(), metadata))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut substations = BTreeMap::<i64, ComponentId>::new();
    for row in substation_table.rows {
        let number = integer_value(substation_table.value(row, "isub"), "sub.isub", None)?;
        let component = component_id("substation", number.to_string())?;
        if substations.insert(number, component.clone()).is_some() {
            return Err(malformed(format!("RAWX substation {number} is repeated")));
        }
        detailed.substations.push(Substation {
            component: component.clone(),
            country: None,
            operator: None,
            geographical_tags: Vec::new(),
        });
        let mut properties = BTreeMap::new();
        properties.insert("psse_isub".into(), number.to_string());
        for field in ["lati", "long", "srg"] {
            if let Some(value) = substation_table
                .value(row, field)
                .filter(|value| !value.is_null())
            {
                properties.insert(format!("psse_{field}"), value.to_string());
            }
        }
        metadata.insert(
            component.clone(),
            ComponentMetadata {
                component,
                name: string_value(substation_table.value(row, "name"), "sub.name")?
                    .map(str::to_owned),
                equipment_container: None,
                aliases: Vec::new(),
                external_identifiers: Vec::new(),
                properties,
                fictitious: false,
            },
        );
    }

    let bus_by_id = parsed
        .buses()
        .iter()
        .map(|bus| (bus.id, bus))
        .collect::<BTreeMap<_, _>>();
    let mut nodes = BTreeMap::<(i64, i32), RawxNode>::new();
    let mut level_buses = BTreeMap::<ComponentId, BTreeSet<BusId>>::new();
    let mut level_substation = BTreeMap::<ComponentId, ComponentId>::new();
    let mut level_nominal = BTreeMap::<ComponentId, f64>::new();
    let mut level_low = BTreeMap::<ComponentId, f64>::new();
    let mut level_high = BTreeMap::<ComponentId, f64>::new();
    if let Some(table) = &node_table {
        for row in table.rows {
            let substation = integer_value(table.value(row, "isub"), "subnode.isub", None)?;
            let substation_component = substations.get(&substation).ok_or_else(|| {
                malformed(format!(
                    "RAWX node refers to unknown substation {substation}"
                ))
            })?;
            let source_node_i64 = integer_value(table.value(row, "inode"), "subnode.inode", None)?;
            let source_node = i32::try_from(source_node_i64)
                .map_err(|_| malformed("RAWX subnode.inode is outside the i32 range"))?;
            let bus = BusId(table_bus(table, row, "ibus")?);
            let bus_row = bus_by_id
                .get(&bus)
                .ok_or_else(|| malformed(format!("RAWX node refers to unknown bus {bus}")))?;
            let voltage_level =
                component_id("voltage_level", format!("{substation}/{}", bus_row.base_kv))?;
            let component =
                component_id("connectivity_node", format!("{substation}/{source_node}"))?;
            if nodes
                .insert(
                    (substation, source_node),
                    RawxNode {
                        substation,
                        source_node,
                        component: component.clone(),
                        voltage_level: voltage_level.clone(),
                    },
                )
                .is_some()
            {
                return Err(malformed(format!(
                    "RAWX substation {substation} repeats node {source_node}"
                )));
            }
            level_buses
                .entry(voltage_level.clone())
                .or_default()
                .insert(bus);
            level_substation.insert(voltage_level.clone(), substation_component.clone());
            level_nominal.insert(voltage_level.clone(), bus_row.base_kv);
            level_low
                .entry(voltage_level.clone())
                .and_modify(|limit| *limit = limit.min(bus_row.vmin * bus_row.base_kv))
                .or_insert(bus_row.vmin * bus_row.base_kv);
            level_high
                .entry(voltage_level.clone())
                .and_modify(|limit| *limit = limit.max(bus_row.vmax * bus_row.base_kv))
                .or_insert(bus_row.vmax * bus_row.base_kv);
            detailed.connectivity_nodes.push(ConnectivityNode {
                component: component.clone(),
                voltage_level,
                node_number: Some(source_node),
                calculated_bus: Some(bus),
            });
            let mut properties = BTreeMap::new();
            for field in ["stat", "vm", "va"] {
                match table.value(row, field) {
                    Some(Value::Null) if matches!(field, "vm" | "va") => {
                        // Preserve an explicit RAWX null through PowerIO IR.
                        // Missing columns remain unmarked and retain the
                        // existing synthesized-output defaults.
                        properties.insert(format!("psse_{field}"), EXPLICIT_NULL.to_owned());
                    }
                    Some(value) if !value.is_null() => {
                        properties.insert(format!("psse_{field}"), value.to_string());
                    }
                    _ => {}
                }
            }
            metadata.insert(
                component.clone(),
                ComponentMetadata {
                    component,
                    name: string_value(table.value(row, "name"), "subnode.name")?
                        .map(str::to_owned),
                    equipment_container: None,
                    aliases: Vec::new(),
                    external_identifiers: Vec::new(),
                    properties,
                    fictitious: false,
                },
            );
        }
    }
    for (component, buses) in level_buses {
        detailed.voltage_levels.push(VoltageLevel {
            substation: level_substation.get(&component).cloned(),
            nominal_kv: level_nominal[&component],
            low_voltage_limit_kv: level_low.get(&component).copied(),
            high_voltage_limit_kv: level_high.get(&component).copied(),
            topology_kind: TopologyKind::NodeBreaker,
            buses: buses.into_iter().collect(),
            component,
        });
    }

    if let Some(table) = &switch_table {
        for row in table.rows {
            let substation = integer_value(table.value(row, "isub"), "subswd.isub", None)?;
            let inode_i64 = integer_value(table.value(row, "inode"), "subswd.inode", None)?;
            let jnode_i64 = integer_value(table.value(row, "jnode"), "subswd.jnode", None)?;
            let inode = i32::try_from(inode_i64)
                .map_err(|_| malformed("RAWX subswd.inode is outside the i32 range"))?;
            let jnode = i32::try_from(jnode_i64)
                .map_err(|_| malformed("RAWX subswd.jnode is outside the i32 range"))?;
            let first = nodes.get(&(substation, inode)).ok_or_else(|| {
                malformed(format!(
                    "RAWX switch refers to unknown node {substation}/{inode}"
                ))
            })?;
            let second = nodes.get(&(substation, jnode)).ok_or_else(|| {
                malformed(format!(
                    "RAWX switch refers to unknown node {substation}/{jnode}"
                ))
            })?;
            if first.voltage_level != second.voltage_level {
                return Err(malformed(format!(
                    "RAWX switch joins nodes {substation}/{inode} and {substation}/{jnode} in different voltage levels"
                )));
            }
            let switch_id = table_text(table, row, "swdid", "1")?;
            let component = component_id(
                "switch",
                format!(
                    "{substation}/{}-{}/{switch_id}",
                    inode.min(jnode),
                    inode.max(jnode)
                ),
            )?;
            let switch_type = integer_value(table.value(row, "type"), "subswd.type", Some(1))?;
            let status = integer_value(table.value(row, "stat"), "subswd.stat", Some(1))?;
            detailed.switches.push(TopologySwitch {
                component: component.clone(),
                voltage_level: first.voltage_level.clone(),
                kind: if switch_type == 2 {
                    SwitchKind::Breaker
                } else {
                    SwitchKind::Disconnector
                },
                endpoint1: TopologyEndpoint::Node(first.component.clone()),
                endpoint2: TopologyEndpoint::Node(second.component.clone()),
                open: status != 1,
                retained: false,
            });
            let mut properties = BTreeMap::new();
            properties.insert("psse_swdid".into(), switch_id);
            for field in [
                "type", "stat", "nstat", "xpu", "rate1", "rate2", "rate3", "rsetnam",
            ] {
                if let Some(value) = table.value(row, field).filter(|value| !value.is_null()) {
                    properties.insert(format!("psse_{field}"), value.to_string());
                }
            }
            metadata.insert(
                component.clone(),
                ComponentMetadata {
                    component,
                    name: string_value(table.value(row, "name"), "subswd.name")?.map(str::to_owned),
                    equipment_container: None,
                    aliases: Vec::new(),
                    external_identifiers: Vec::new(),
                    properties,
                    fictitious: false,
                },
            );
        }
    }

    let equipment_mappings = rawx_equipment_mappings(source, parsed)?;
    let mut terminal_occurrence = BTreeMap::<(RawxEquipmentKey, usize), usize>::new();
    let mut nodes_with_equipment = BTreeSet::<ComponentId>::new();
    if let Some(table) = &terminal_table {
        for row in table.rows {
            let substation = integer_value(table.value(row, "isub"), "subterm.isub", None)?;
            let source_node_i64 = integer_value(table.value(row, "inode"), "subterm.inode", None)?;
            let source_node = i32::try_from(source_node_i64)
                .map_err(|_| malformed("RAWX subterm.inode is outside the i32 range"))?;
            let node = nodes.get(&(substation, source_node)).ok_or_else(|| {
                malformed(format!(
                    "RAWX equipment terminal refers to unknown node {substation}/{source_node}"
                ))
            })?;
            let equipment_type = table_text(table, row, "type", "")?;
            if equipment_type.is_empty() {
                return Err(malformed("RAWX subterm.type is empty"));
            }
            let key = table_equipment_key(
                table,
                row,
                &equipment_type,
                "eqid",
                &["ibus", "jbus", "kbus"],
            )?;
            let equipment = if let Some(component) = equipment_mappings.get(&key) {
                component.clone()
            } else {
                warnings.push(
                    &codes::READ_PSSE_RETAINED_SOURCE_ONLY,
                    format!(
                        "PSS/E RAWX terminal refers to equipment `{}` outside the typed balanced calculation view",
                        key.local_id()
                    ),
                );
                component_id("equipment", key.local_id())?
            };
            let ibus = table_bus(table, row, "ibus")?;
            let position = key
                .buses
                .iter()
                .filter(|bus| **bus != 0)
                .position(|bus| *bus == ibus)
                .ok_or_else(|| malformed("RAWX subterm.ibus is absent from its equipment buses"))?;
            let occurrence = terminal_occurrence.entry((key.clone(), ibus)).or_insert(0);
            let terminal_number = position + 1 + *occurrence;
            *occurrence += 1;
            let terminal = u8::try_from(terminal_number)
                .map_err(|_| malformed("RAWX equipment has more than 255 terminals"))?;
            detailed.terminals.push(Terminal {
                component: None,
                equipment: equipment.clone(),
                terminal,
                voltage_level: node.voltage_level.clone(),
                bus: None,
                connectable_bus: None,
                node: Some(node.component.clone()),
                connected: true,
                active_power_mw: None,
                reactive_power_mvar: None,
            });
            nodes_with_equipment.insert(node.component.clone());
            let entry = metadata
                .entry(equipment.clone())
                .or_insert_with(|| ComponentMetadata {
                    component: equipment,
                    name: None,
                    equipment_container: None,
                    aliases: Vec::new(),
                    external_identifiers: Vec::new(),
                    properties: BTreeMap::new(),
                    fictitious: false,
                });
            entry
                .properties
                .insert("psse_type".into(), key.equipment_type.clone());
            entry
                .properties
                .insert("psse_eqid".into(), key.source_id.clone());
            for (index, bus) in key.buses.iter().copied().enumerate() {
                entry
                    .properties
                    .insert(format!("psse_bus_{}", index + 1), bus.to_string());
            }
        }
    }

    for node in nodes.values() {
        if nodes_with_equipment.contains(&node.component) {
            continue;
        }
        let component = component_id(
            "busbar_section",
            format!("{}/{}", node.substation, node.source_node),
        )?;
        detailed.busbar_sections.push(BusbarSection {
            component: component.clone(),
            voltage_level: node.voltage_level.clone(),
            node: node.component.clone(),
        });
        detailed.terminals.push(Terminal {
            component: None,
            equipment: component.clone(),
            terminal: 1,
            voltage_level: node.voltage_level.clone(),
            bus: None,
            connectable_bus: None,
            node: Some(node.component.clone()),
            connected: true,
            active_power_mw: None,
            reactive_power_mvar: None,
        });
        let name = metadata
            .get(&node.component)
            .and_then(|value| value.name.clone());
        metadata.insert(
            component.clone(),
            ComponentMetadata {
                component,
                name,
                equipment_container: None,
                aliases: Vec::new(),
                external_identifiers: Vec::new(),
                properties: BTreeMap::new(),
                fictitious: false,
            },
        );
    }

    detailed.component_metadata = metadata.into_values().collect();
    *parsed.detailed_connectivity_mut() = Some(std::sync::Arc::new(detailed));
    Ok(())
}

/// Read revision 34+ nested substation records from a legacy RAW source and
/// attach the same detailed connectivity produced by the RAWX tables.
#[allow(clippy::too_many_lines)]
pub(super) fn read_raw_detailed_connectivity(
    source: &str,
    parsed: &mut BalancedNetwork,
    warnings: &mut Diagnostics,
) -> Result<bool> {
    #[derive(Clone, Copy)]
    enum Stage {
        Substation,
        Node,
        Switch,
        Terminal,
    }

    let lines = source.lines().collect::<Vec<_>>();
    let Some(start) = lines
        .iter()
        .position(|line| section_after_marker(line.trim()).as_deref() == Some("SUBSTATION"))
    else {
        return Ok(false);
    };

    let mut substation_rows = Vec::new();
    let mut node_rows = Vec::new();
    let mut switch_rows = Vec::new();
    let mut terminal_rows = Vec::new();
    let mut current_substation = None;
    let mut stage = Stage::Substation;

    for raw_line in &lines[start + 1..] {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('@') || line.starts_with('/') {
            continue;
        }
        if super::psse::fields(line).first().map(AsRef::as_ref) == Some("0") {
            let upper = line.to_ascii_uppercase();
            if upper.contains("END OF SUBSTATION NODE DATA") {
                stage = Stage::Switch;
            } else if upper.contains("END OF SUBSTATION SWITCHING DEVICE DATA") {
                stage = Stage::Terminal;
            } else if upper.contains("END OF SUBSTATION EQUIPMENT TERMINAL DATA")
                || upper.contains("END OF SUBSTATION TERMINAL DATA")
            {
                stage = Stage::Substation;
                current_substation = None;
            } else if upper.contains("END OF SUBSTATION DATA") {
                break;
            }
            continue;
        }

        let tokens = line_tokens(line);
        match stage {
            Stage::Substation => {
                let row = tokens_as_values(&tokens, SUBSTATION_FIELDS, &["name"])?;
                current_substation = row.first().and_then(Value::as_i64);
                if current_substation.is_none() {
                    return Err(malformed("PSS/E substation record has no integer IS value"));
                }
                substation_rows.push(Value::Array(row));
                stage = Stage::Node;
            }
            Stage::Node => {
                let isub = current_substation.ok_or_else(|| {
                    malformed("PSS/E substation node has no enclosing substation")
                })?;
                let mut row = vec![Value::from(isub)];
                row.extend(tokens_as_values(
                    &tokens,
                    &SUBSTATION_NODE_FIELDS[1..],
                    &["name"],
                )?);
                node_rows.push(Value::Array(row));
            }
            Stage::Switch => {
                let isub = current_substation.ok_or_else(|| {
                    malformed("PSS/E substation switching device has no enclosing substation")
                })?;
                let mut row = vec![Value::from(isub)];
                row.extend(tokens_as_values(
                    &tokens,
                    &SUBSTATION_SWITCH_FIELDS[1..],
                    &["swdid", "name"],
                )?);
                switch_rows.push(Value::Array(row));
            }
            Stage::Terminal => {
                let isub = current_substation.ok_or_else(|| {
                    malformed("PSS/E substation terminal has no enclosing substation")
                })?;
                let fields: &[&str] = match tokens.len() {
                    4 => &["ibus", "inode", "type", "eqid"],
                    5 => &["ibus", "inode", "type", "jbus", "eqid"],
                    6 => &["ibus", "inode", "type", "jbus", "kbus", "eqid"],
                    count => {
                        return Err(malformed(format!(
                            "PSS/E substation terminal has {count} fields; expected 4, 5, or 6"
                        )));
                    }
                };
                let values = tokens_as_values(&tokens, fields, &["type", "eqid"])?;
                let column = |name: &str| {
                    fields
                        .iter()
                        .position(|field| *field == name)
                        .and_then(|index| values.get(index))
                        .cloned()
                        .unwrap_or(Value::Null)
                };
                terminal_rows.push(Value::Array(vec![
                    Value::from(isub),
                    column("inode"),
                    column("type"),
                    column("eqid"),
                    column("ibus"),
                    column("jbus"),
                    column("kbus"),
                ]));
            }
        }
    }

    if substation_rows.is_empty() {
        return Ok(false);
    }

    let (_, _, sections) = split_raw(source)?;
    let mut network = Map::new();
    add_simple_output_table(&mut network, "bus", BUS_FIELDS, &["name"], &sections);
    add_simple_output_table(
        &mut network,
        "load",
        LOAD_FIELDS,
        &["loadid", "loadtype"],
        &sections,
    );
    add_simple_output_table(
        &mut network,
        "fixshunt",
        FIXED_SHUNT_FIELDS,
        &["shntid"],
        &sections,
    );
    add_simple_output_table(
        &mut network,
        "generator",
        GENERATOR_FIELDS,
        &["machid"],
        &sections,
    );
    add_simple_output_table(
        &mut network,
        "acline",
        AC_LINE_FIELDS,
        &["ckt", "name"],
        &sections,
    );
    add_transformer_output_table(&mut network, &sections)?;
    add_simple_output_table(
        &mut network,
        "swshunt",
        SWITCHED_SHUNT_FIELDS,
        &["shntid", "rmidnt"],
        &sections,
    );
    add_two_terminal_output_table(&mut network, &sections)?;
    network.insert(
        "sub".to_owned(),
        table_object(SUBSTATION_FIELDS, Value::Array(substation_rows)),
    );
    network.insert(
        "subnode".to_owned(),
        table_object(SUBSTATION_NODE_FIELDS, Value::Array(node_rows)),
    );
    network.insert(
        "subswd".to_owned(),
        table_object(SUBSTATION_SWITCH_FIELDS, Value::Array(switch_rows)),
    );
    network.insert(
        "subterm".to_owned(),
        table_object(SUBSTATION_TERMINAL_FIELDS, Value::Array(terminal_rows)),
    );

    parsed.assign_missing_component_ids();
    read_detailed_connectivity(&network, parsed, warnings)?;
    Ok(true)
}

fn winding_fields(winding: usize) -> Vec<String> {
    let mut fields = vec![
        format!("windv{winding}"),
        format!("nomv{winding}"),
        format!("ang{winding}"),
    ];
    fields.extend((1..=12).map(|rating| format!("wdg{winding}rate{rating}")));
    fields.extend(
        [
            "cod", "cont", "node", "rma", "rmi", "vma", "vmi", "ntp", "tab", "cr", "cx", "cnxa",
        ]
        .into_iter()
        .map(|field| format!("{field}{winding}")),
    );
    fields
}

fn transformer_fields() -> Vec<String> {
    let mut fields: Vec<String> = TRANSFORMER_MAIN_FIELDS
        .iter()
        .chain(TRANSFORMER_IMPEDANCE_FIELDS)
        .map(|field| (*field).to_owned())
        .collect();
    for winding in 1..=3 {
        fields.extend(winding_fields(winding));
    }
    fields
}

/// Emit a neutral network through the shared PSS/E revision 35 writer, then
/// encode those records as RAWX tables.
pub(crate) fn write_rawx(net: &BalancedNetwork) -> Result<TextEmission> {
    let allocated_node_numbers = net
        .detailed_connectivity()
        .as_deref()
        .map_or(0, |detailed| {
            detailed
                .connectivity_nodes
                .iter()
                .filter(|node| node.node_number.is_none())
                .count()
        });
    let prepared = with_rawx_node_numbers(net)?;
    let net = prepared.as_ref().unwrap_or(net);
    let raw = super::psse::write_psse_rev_without_detailed_connectivity(net, 35);
    let mut diagnostics = Diagnostics::from(raw.diagnostics);
    if allocated_node_numbers > 0 {
        diagnostics.push(
            &codes::EMIT_PSSE.value_defaulted,
            format!(
                "{allocated_node_numbers} connectivity node(s) state no PSS/E node number; RAWX requires one, so deterministic positive numbers were allocated within each substation"
            ),
        );
    }
    let text = raw_to_rawx(net, &raw.text, &mut diagnostics).map_err(|error| Error::Emit {
        format: FMT,
        message: error.to_string(),
    })?;
    if net.solver().is_some() {
        diagnostics.push(
            &codes::EMIT_PSSE.field_dropped,
            "solver parameters dropped: PSS/E RAWX 35 has no system wide table",
        );
    }
    Ok(TextEmission {
        text,
        diagnostics: diagnostics.into_records(),
        fidelity: powerio_core::Fidelity::Canonical,
    })
}

fn raw_to_rawx(net: &BalancedNetwork, raw: &str, diagnostics: &mut Diagnostics) -> Result<String> {
    let (header, title, sections) = split_raw(raw)?;
    let mut network = Map::new();
    let mut case_data = tokens_as_values(&header, &CASE_FIELDS[..6], &[])?;
    case_data.push(Value::String(title));
    case_data.push(Value::String(String::new()));
    network.insert(
        "caseid".to_owned(),
        table_object(CASE_FIELDS, Value::Array(case_data)),
    );
    add_simple_output_table(&mut network, "bus", BUS_FIELDS, &["name"], &sections);
    add_simple_output_table(
        &mut network,
        "load",
        LOAD_FIELDS,
        &["loadid", "loadtype"],
        &sections,
    );
    add_simple_output_table(
        &mut network,
        "fixshunt",
        FIXED_SHUNT_FIELDS,
        &["shntid"],
        &sections,
    );
    add_simple_output_table(
        &mut network,
        "generator",
        GENERATOR_FIELDS,
        &["machid"],
        &sections,
    );
    add_simple_output_table(
        &mut network,
        "acline",
        AC_LINE_FIELDS,
        &["ckt", "name"],
        &sections,
    );
    add_transformer_output_table(&mut network, &sections)?;
    add_simple_output_table(&mut network, "area", AREA_FIELDS, &["arname"], &sections);
    add_two_terminal_output_table(&mut network, &sections)?;
    add_simple_output_table(
        &mut network,
        "swshunt",
        SWITCHED_SHUNT_FIELDS,
        &["shntid", "rmidnt"],
        &sections,
    );
    add_system_switch_output_table(&mut network, net, diagnostics);
    apply_detailed_equipment_ids(&mut network, net)?;
    add_detailed_connectivity_output_tables(&mut network, net, diagnostics)?;

    let mut root = Map::new();
    root.insert("network".to_owned(), Value::Object(network));
    let mut text = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|error| malformed(format!("cannot encode RAWX JSON: {error}")))?;
    text.push('\n');
    Ok(text)
}

/// Assign deterministic positive node numbers before the electrical writer
/// resolves exact regulation targets. PSS/E node numbers are unique within a
/// substation, while source-neutral connectivity nodes need not state one.
fn with_rawx_node_numbers(net: &BalancedNetwork) -> Result<Option<BalancedNetwork>> {
    let Some(detailed) = net.detailed_connectivity().as_deref() else {
        return Ok(None);
    };
    if detailed
        .connectivity_nodes
        .iter()
        .all(|node| node.node_number.is_some())
    {
        return Ok(None);
    }
    let level_substations = detailed
        .voltage_levels
        .iter()
        .filter_map(|level| {
            level
                .substation
                .as_ref()
                .map(|substation| (level.component.clone(), substation.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut used = BTreeMap::<ComponentId, BTreeSet<i32>>::new();
    for node in &detailed.connectivity_nodes {
        let substation = level_substations.get(&node.voltage_level).ok_or_else(|| {
            malformed(format!(
                "connectivity node {} has no substation through voltage level {}",
                node.component, node.voltage_level
            ))
        })?;
        if let Some(number) = node.node_number
            && !used.entry(substation.clone()).or_default().insert(number)
        {
            return Err(malformed(format!(
                "detailed connectivity repeats PSS/E node number {number} in substation {substation}"
            )));
        }
    }

    // Copy the connectivity directly rather than through
    // `detailed_connectivity_mut`, which clears the source omission markers
    // the writer still reports as dropped.
    let mut numbered = detailed.clone();
    let mut next = BTreeMap::<ComponentId, i32>::new();
    for node in &mut numbered.connectivity_nodes {
        if node.node_number.is_some() {
            continue;
        }
        let substation = level_substations
            .get(&node.voltage_level)
            .expect("voltage levels were checked before cloning");
        let used = used.entry(substation.clone()).or_default();
        let candidate = next.entry(substation.clone()).or_insert(1);
        while used.contains(candidate) {
            *candidate = candidate.checked_add(1).ok_or_else(|| {
                malformed(format!(
                    "RAWX substation {substation} has no available node number"
                ))
            })?;
        }
        node.node_number = Some(*candidate);
        used.insert(*candidate);
        *candidate = candidate.checked_add(1).unwrap_or(i32::MAX);
    }
    let mut prepared = net.clone();
    prepared.tables_mut().detailed_connectivity = Some(std::sync::Arc::new(numbered));
    Ok(Some(prepared))
}

type RawSections = BTreeMap<String, Vec<String>>;
type SplitRaw = (Vec<String>, String, RawSections);

fn split_raw(raw: &str) -> Result<SplitRaw> {
    let mut lines = raw.lines();
    let header = lines
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| malformed("internal PSS/E writer returned no header"))?;
    let header: Vec<String> = super::psse::fields(header)
        .into_iter()
        .map(std::borrow::Cow::into_owned)
        .collect();
    let title = lines.next().unwrap_or_default().trim().to_owned();
    let mut section = String::new();
    let mut sections = RawSections::new();
    for line in lines {
        let line = line.trim();
        if line.eq_ignore_ascii_case("q") {
            break;
        }
        if line.starts_with('@') || line.starts_with('/') {
            continue;
        }
        if let Some(next) = section_after_marker(line) {
            section = next;
            continue;
        }
        if !section.is_empty() {
            sections
                .entry(section.clone())
                .or_default()
                .push(line.to_owned());
        }
    }
    Ok((header, title, sections))
}

fn section_after_marker(line: &str) -> Option<String> {
    if super::psse::fields(line).first().map(AsRef::as_ref) != Some("0") {
        return None;
    }
    let upper = line.to_ascii_uppercase();
    let start = upper.find("BEGIN ")? + "BEGIN ".len();
    let rest = &upper[start..];
    let end = rest.find(" DATA")?;
    Some(rest[..end].trim().to_owned())
}

fn add_simple_output_table(
    network: &mut Map<String, Value>,
    name: &str,
    fields: &[&str],
    strings: &[&str],
    sections: &BTreeMap<String, Vec<String>>,
) {
    let section_name = match name {
        "bus" => "BUS",
        "load" => "LOAD",
        "fixshunt" => "FIXED SHUNT",
        "generator" => "GENERATOR",
        "acline" => "BRANCH",
        "area" => "AREA",
        "swshunt" => "SWITCHED SHUNT",
        _ => return,
    };
    let rows: Vec<Value> = sections
        .get(section_name)
        .into_iter()
        .flatten()
        .map(|line| {
            let tokens: Vec<String> = super::psse::fields(line)
                .into_iter()
                .map(std::borrow::Cow::into_owned)
                .collect();
            Value::Array(tokens_as_values(&tokens, fields, strings).unwrap())
        })
        .collect();
    if !rows.is_empty() {
        network.insert(name.to_owned(), table_object(fields, Value::Array(rows)));
    }
}

fn add_transformer_output_table(
    network: &mut Map<String, Value>,
    sections: &BTreeMap<String, Vec<String>>,
) -> Result<()> {
    let Some(lines) = sections.get("TRANSFORMER") else {
        return Ok(());
    };
    let fields = transformer_fields();
    let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
    let mut rows = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let main = line_tokens(&lines[index]);
        let kbus = main
            .get(2)
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0);
        let count = if kbus == 0.0 { 4 } else { 5 };
        if index + count > lines.len() {
            return Err(malformed("internal PSS/E transformer record is truncated"));
        }
        let impedance = line_tokens(&lines[index + 1]);
        let winding1 = line_tokens(&lines[index + 2]);
        let winding2 = line_tokens(&lines[index + 3]);
        let winding3 = (count == 5).then(|| line_tokens(&lines[index + 4]));
        let mut flat = Vec::new();
        append_padded(&mut flat, &main, TRANSFORMER_MAIN_FIELDS.len());
        append_padded(&mut flat, &impedance, TRANSFORMER_IMPEDANCE_FIELDS.len());
        append_padded(&mut flat, &winding1, winding_fields(1).len());
        append_padded(&mut flat, &winding2, winding_fields(2).len());
        append_padded(
            &mut flat,
            winding3.as_deref().unwrap_or_default(),
            winding_fields(3).len(),
        );
        rows.push(Value::Array(tokens_as_values(
            &flat,
            &refs,
            &["ckt", "name", "vecgrp"],
        )?));
        index += count;
    }
    if !rows.is_empty() {
        network.insert(
            "transformer".to_owned(),
            table_object(&refs, Value::Array(rows)),
        );
    }
    Ok(())
}

fn add_two_terminal_output_table(
    network: &mut Map<String, Value>,
    sections: &BTreeMap<String, Vec<String>>,
) -> Result<()> {
    let Some(lines) = sections.get("TWO-TERMINAL DC") else {
        return Ok(());
    };
    if lines.len() % 3 != 0 {
        return Err(malformed(
            "internal PSS/E two-terminal DC record is truncated",
        ));
    }
    let mut rows = Vec::new();
    for record in lines.chunks(3) {
        let mut flat = line_tokens(&record[0]);
        flat.extend(line_tokens(&record[1]));
        flat.extend(line_tokens(&record[2]));
        rows.push(Value::Array(tokens_as_values(
            &flat,
            TWO_TERMINAL_DC_FIELDS,
            &["name", "met", "idr", "idi"],
        )?));
    }
    if !rows.is_empty() {
        network.insert(
            "twotermdc".to_owned(),
            table_object(TWO_TERMINAL_DC_FIELDS, Value::Array(rows)),
        );
    }
    Ok(())
}

fn add_system_switch_output_table(
    network: &mut Map<String, Value>,
    net: &BalancedNetwork,
    diagnostics: &mut Diagnostics,
) {
    if net.switches().is_empty() {
        return;
    }
    let has_ratings = net.switches().iter().any(|switch| {
        switch.thermal_rating.is_some()
            || (2..=12).any(|rating| switch.extras.contains_key(&format!("psse_rate{rating}")))
    });
    let has_rating_set_names = net
        .switches()
        .iter()
        .any(|switch| switch.extras.contains_key("psse_rsetnam"));
    let use_rating_set_names = has_rating_set_names && !has_ratings;
    if has_rating_set_names && has_ratings {
        let count = net
            .switches()
            .iter()
            .filter(|switch| switch.extras.contains_key("psse_rsetnam"))
            .count();
        diagnostics.push(
            &codes::EMIT_PSSE.field_dropped,
            format!(
                "{count} system switching device rating set name(s) dropped: one RAWX `sysswd` table cannot mix `rsetnam` rows with explicit RATE1-RATE12 rows"
            ),
        );
    }

    let mut switch_ids = BTreeMap::new();
    let rows = net
        .switches()
        .iter()
        .map(|switch| {
            let extra = |field: &str| switch.extras.get(&format!("psse_{field}")).cloned();
            let preferred_ckt = switch.extras.get("psse_ckt").and_then(Value::as_str);
            let ckt = super::allocate_circuit_id(
                preferred_ckt,
                (switch.from, switch.to),
                &mut switch_ids,
            );
            let mut row = vec![
                Value::from(switch.from.0),
                Value::from(switch.to.0),
                Value::String(ckt),
                extra("xpu").unwrap_or_else(|| Value::from(0.0)),
            ];
            if use_rating_set_names {
                row.push(extra("rsetnam").unwrap_or_else(|| Value::String(String::new())));
            } else {
                row.push(switch.thermal_rating.map_or(Value::from(0.0), jnum));
                row.extend((2..=12).map(|rating| {
                    extra(&format!("rate{rating}")).unwrap_or_else(|| Value::from(0.0))
                }));
            }
            row.extend([
                Value::from(i64::from(switch.closed)),
                extra("nstat").unwrap_or_else(|| Value::from(1)),
                extra("met").unwrap_or_else(|| Value::from(1)),
                extra("stype").unwrap_or_else(|| Value::from(1)),
                extra("name").unwrap_or_else(|| Value::String(String::new())),
            ]);
            Value::Array(row)
        })
        .collect();
    network.insert(
        "sysswd".to_owned(),
        table_object(
            if use_rating_set_names {
                SYSTEM_SWITCH_RSET_FIELDS
            } else {
                SYSTEM_SWITCH_RATING_FIELDS
            },
            Value::Array(rows),
        ),
    );
}

fn metadata_by_component(
    detailed: &DetailedConnectivity,
) -> BTreeMap<&ComponentId, &ComponentMetadata> {
    detailed
        .component_metadata
        .iter()
        .map(|metadata| (&metadata.component, metadata))
        .collect()
}

fn replace_table_string(
    network: &mut Map<String, Value>,
    table: &str,
    row: usize,
    field: &str,
    value: &str,
) -> Result<()> {
    let table_value = network
        .get_mut(table)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| malformed(format!("internal RAWX output has no `{table}` table")))?;
    let field_index = table_value
        .get("fields")
        .and_then(Value::as_array)
        .and_then(|fields| {
            fields
                .iter()
                .position(|candidate| candidate.as_str() == Some(field))
        })
        .ok_or_else(|| {
            malformed(format!(
                "internal RAWX `{table}` table has no `{field}` field"
            ))
        })?;
    let row = table_value
        .get_mut("data")
        .and_then(Value::as_array_mut)
        .and_then(|rows| rows.get_mut(row))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| malformed(format!("internal RAWX `{table}` row is absent")))?;
    row[field_index] = Value::String(value.to_owned());
    Ok(())
}

fn apply_detailed_equipment_ids(
    network: &mut Map<String, Value>,
    net: &BalancedNetwork,
) -> Result<()> {
    let Some(detailed) = net.detailed_connectivity().as_deref() else {
        return Ok(());
    };
    let source_id = |component_type: &str, uid: Option<&str>| {
        let uid = uid?;
        detailed.component_metadata.iter().find_map(|metadata| {
            (metadata.component.component_type() == component_type
                && metadata.component.local_id() == uid)
                .then(|| metadata.properties.get("psse_eqid"))
                .flatten()
                .cloned()
        })
    };

    for (row, generator) in net.generators().iter().enumerate() {
        if let Some(id) = source_id("generator", generator.uid.as_deref()) {
            replace_table_string(network, "generator", row, "machid", &id)?;
        }
    }

    let mut row = 0usize;
    for transformer in net
        .branches()
        .iter()
        .filter(|branch| branch.is_transformer())
    {
        if let Some(id) = source_id("transformer", transformer.uid.as_deref()) {
            replace_table_string(network, "transformer", row, "ckt", &id)?;
        }
        row += 1;
    }
    for transformer in net.transformers_3w() {
        if let Some(id) = source_id("transformer", transformer.uid.as_deref()) {
            replace_table_string(network, "transformer", row, "ckt", &id)?;
        }
        row += 1;
    }
    Ok(())
}

fn metadata_text(
    metadata: Option<&ComponentMetadata>,
    property: &str,
    default: impl FnOnce() -> String,
) -> String {
    metadata
        .and_then(|value| value.properties.get(property))
        .cloned()
        .unwrap_or_else(default)
}

fn metadata_number(metadata: Option<&ComponentMetadata>, property: &str, default: f64) -> Value {
    metadata
        .and_then(|value| value.properties.get(property))
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .map_or_else(|| jnum(default), jnum)
}

fn metadata_number_or_null(
    metadata: Option<&ComponentMetadata>,
    property: &str,
    default: f64,
) -> Value {
    if metadata
        .and_then(|value| value.properties.get(property))
        .is_some_and(|value| value == EXPLICIT_NULL)
    {
        Value::Null
    } else {
        metadata_number(metadata, property, default)
    }
}

fn metadata_integer(metadata: Option<&ComponentMetadata>, property: &str, default: i64) -> Value {
    metadata
        .and_then(|value| value.properties.get(property))
        .and_then(|value| {
            value.parse::<i64>().ok().or_else(|| {
                value
                    .parse::<f64>()
                    .ok()
                    .filter(|number| {
                        number.is_finite()
                            && number.fract() == 0.0
                            && *number >= i64::MIN as f64
                            && *number <= i64::MAX as f64
                    })
                    .map(|number| number as i64)
            })
        })
        .map_or_else(|| Value::from(default), Value::from)
}

fn psse_equipment_type(
    equipment: &ComponentId,
    terminal_count: usize,
    metadata: Option<&ComponentMetadata>,
) -> Option<String> {
    metadata
        .and_then(|value| value.properties.get("psse_type"))
        .cloned()
        .or_else(|| {
            Some(
                match (equipment.component_type(), terminal_count) {
                    ("load", 1) => "L",
                    ("shunt", 1) => "F",
                    ("generator", 1) => "M",
                    ("branch", 2) => "B",
                    ("transformer", 2) => "2",
                    ("transformer", 3) => "3",
                    ("hvdc", 2) => "D",
                    _ => return None,
                }
                .to_owned(),
            )
        })
}

fn insert_output_equipment_key(
    keys: &mut BTreeMap<ComponentId, RawxEquipmentKey>,
    component_types: &[&str],
    uid: Option<&str>,
    key: &RawxEquipmentKey,
) -> Result<()> {
    let Some(uid) = uid else {
        return Ok(());
    };
    for component_type in component_types {
        keys.insert(component_id(component_type, uid)?, key.clone());
    }
    Ok(())
}

/// Map source-neutral equipment identities to the identifiers selected by the
/// electrical PSS/E writer. A `subterm` row must use the same type, buses, and
/// local identifier as the corresponding RAWX equipment row.
#[allow(clippy::too_many_lines)] // one ordered pass aligns every PSS/E equipment table
fn output_equipment_keys(
    network: &Map<String, Value>,
    net: &BalancedNetwork,
) -> Result<BTreeMap<ComponentId, RawxEquipmentKey>> {
    let mut keys = BTreeMap::new();
    if let Some(table) = Table::parse(network, "load")? {
        for (row, load) in table.rows.iter().zip(net.loads()) {
            insert_output_equipment_key(
                &mut keys,
                &["load"],
                load.uid.as_deref(),
                &table_equipment_key(&table, row, "L", "loadid", &["ibus"])?,
            )?;
        }
    }
    if let Some(table) = Table::parse(network, "fixshunt")? {
        for (row, shunt) in table
            .rows
            .iter()
            .zip(net.shunts().iter().filter(|shunt| shunt.control.is_none()))
        {
            insert_output_equipment_key(
                &mut keys,
                &["shunt"],
                shunt.uid.as_deref(),
                &table_equipment_key(&table, row, "F", "shntid", &["ibus"])?,
            )?;
        }
    }
    if let Some(table) = Table::parse(network, "generator")? {
        for (row, generator) in table.rows.iter().zip(net.generators()) {
            insert_output_equipment_key(
                &mut keys,
                &["generator"],
                generator.uid.as_deref(),
                &table_equipment_key(&table, row, "M", "machid", &["ibus"])?,
            )?;
        }
    }
    if let Some(table) = Table::parse(network, "acline")? {
        for (row, branch) in table.rows.iter().zip(
            net.branches()
                .iter()
                .filter(|branch| !branch.is_transformer()),
        ) {
            insert_output_equipment_key(
                &mut keys,
                &["branch"],
                branch.uid.as_deref(),
                &table_equipment_key(&table, row, "B", "ckt", &["ibus", "jbus"])?,
            )?;
        }
    }
    if let Some(table) = Table::parse(network, "transformer")? {
        let mut rows = table.rows.iter();
        for branch in net
            .branches()
            .iter()
            .filter(|branch| branch.is_transformer())
        {
            if let Some(row) = rows.next() {
                insert_output_equipment_key(
                    &mut keys,
                    &["branch", "transformer"],
                    branch.uid.as_deref(),
                    &table_equipment_key(&table, row, "2", "ckt", &["ibus", "jbus", "kbus"])?,
                )?;
            }
        }
        for transformer in net.transformers_3w() {
            if let Some(row) = rows.next() {
                insert_output_equipment_key(
                    &mut keys,
                    &["branch", "transformer", "transformer_3w"],
                    transformer.uid.as_deref(),
                    &table_equipment_key(&table, row, "3", "ckt", &["ibus", "jbus", "kbus"])?,
                )?;
            }
        }
    }
    if let Some(table) = Table::parse(network, "swshunt")? {
        for (row, shunt) in table
            .rows
            .iter()
            .zip(net.shunts().iter().filter(|shunt| shunt.control.is_some()))
        {
            insert_output_equipment_key(
                &mut keys,
                &["shunt"],
                shunt.uid.as_deref(),
                &table_equipment_key(&table, row, "S", "shntid", &["ibus"])?,
            )?;
        }
    }
    if let Some(table) = Table::parse(network, "twotermdc")? {
        for (row, line) in table.rows.iter().zip(net.hvdc()) {
            insert_output_equipment_key(
                &mut keys,
                &["hvdc"],
                line.uid.as_deref(),
                &table_equipment_key(&table, row, "D", "name", &["ipr", "ipi"])?,
            )?;
        }
    }
    Ok(keys)
}

// Keep this as one inventory so every detailed record and field is counted once
// before RAWX projection.
#[allow(clippy::too_many_lines)]
fn warn_detailed_output_losses(detailed: &DetailedConnectivity, warnings: &mut Diagnostics) {
    let unsupported_records = [
        ("subnetwork", detailed.subnetworks.len()),
        ("bus breaker bus", detailed.bus_breaker_buses.len()),
        ("calculated bus", detailed.calculated_buses.len()),
        ("junction", detailed.junctions.len()),
        ("internal connection", detailed.internal_connections.len()),
        (
            "operational limit group",
            detailed.operational_limit_groups.len(),
        ),
        ("tap changer", detailed.tap_changers.len()),
        (
            "equipment reactive limit",
            detailed.equipment_reactive_limits.len(),
        ),
        ("boundary line", detailed.boundary_lines.len()),
        ("tie line", detailed.tie_lines.len()),
        ("DC converter unit", detailed.dc_converter_units.len()),
        ("DC topological node", detailed.dc_topological_nodes.len()),
        ("DC node", detailed.dc_nodes.len()),
        ("DC ground", detailed.dc_grounds.len()),
        ("DC busbar", detailed.dc_busbars.len()),
        ("DC line detail", detailed.dc_lines.len()),
        ("DC series device", detailed.dc_series_devices.len()),
        ("DC topology switch", detailed.dc_switches.len()),
        (
            "voltage source converter detail",
            detailed.voltage_source_converters.len(),
        ),
        (
            "line commutated converter detail",
            detailed.line_commutated_converters.len(),
        ),
    ];
    for (record, count) in unsupported_records {
        if count > 0 {
            warnings.push(
                &codes::EMIT_PSSE.record_dropped,
                format!(
                    "{count} detailed {record} record(s) dropped: PSS/E RAWX substation tables cannot represent them"
                ),
            );
        }
    }

    if !detailed.omitted_fields.is_empty() {
        warnings.push(
            &codes::EMIT_PSSE.field_dropped,
            format!(
                "{} detailed source omission marker(s) dropped: PSS/E RAWX cannot retain source absence markers",
                detailed.omitted_fields.len()
            ),
        );
    }

    let substation_fields = detailed
        .substations
        .iter()
        .map(|substation| {
            usize::from(substation.country.is_some())
                + usize::from(substation.operator.is_some())
                + substation.geographical_tags.len()
        })
        .sum::<usize>();
    if substation_fields > 0 {
        warnings.push(
            &codes::EMIT_PSSE.field_dropped,
            format!(
                "{substation_fields} substation country, operator, or geographical tag value(s) dropped: PSS/E RAWX `sub` has no corresponding fields"
            ),
        );
    }

    let metadata_fields = detailed
        .component_metadata
        .iter()
        .map(|metadata| {
            usize::from(metadata.equipment_container.is_some())
                + metadata.aliases.len()
                + metadata.external_identifiers.len()
                + usize::from(metadata.fictitious)
                + metadata
                    .properties
                    .keys()
                    .filter(|property| !rawx_metadata_property_is_emitted(property))
                    .count()
        })
        .sum::<usize>();
    if metadata_fields > 0 {
        warnings.push(
            &codes::EMIT_PSSE.extras_dropped,
            format!(
                "{metadata_fields} detailed component metadata value(s) dropped: PSS/E RAWX emits only its named substation, node, switch, and terminal fields"
            ),
        );
    }

    let retained_switches = detailed
        .switches
        .iter()
        .filter(|switch| switch.retained)
        .count();
    if retained_switches > 0 {
        warnings.push(
            &codes::EMIT_PSSE.field_dropped,
            format!(
                "{retained_switches} topology switch retained flag(s) dropped: PSS/E RAWX switching device records have no retained field"
            ),
        );
    }
    let load_break_switches = detailed
        .switches
        .iter()
        .filter(|switch| switch.kind == SwitchKind::LoadBreakSwitch)
        .count();
    if load_break_switches > 0 {
        warnings.push(
            &codes::EMIT_PSSE.value_collapsed,
            format!(
                "{load_break_switches} load break switch kind(s) emitted as disconnectors: PSS/E RAWX has no distinct load break switch code"
            ),
        );
    }

    let terminal_identities = detailed
        .terminals
        .iter()
        .filter(|terminal| terminal.component.is_some())
        .count();
    let terminal_bus_references = detailed
        .terminals
        .iter()
        .map(|terminal| {
            usize::from(terminal.bus.is_some()) + usize::from(terminal.connectable_bus.is_some())
        })
        .sum::<usize>();
    let disconnected = detailed
        .terminals
        .iter()
        .filter(|terminal| !terminal.connected)
        .count();
    let terminal_powers = detailed
        .terminals
        .iter()
        .map(|terminal| {
            usize::from(terminal.active_power_mw.is_some())
                + usize::from(terminal.reactive_power_mvar.is_some())
        })
        .sum::<usize>();
    let mut terminal_numbers = BTreeMap::<&ComponentId, Vec<u8>>::new();
    for terminal in &detailed.terminals {
        terminal_numbers
            .entry(&terminal.equipment)
            .or_default()
            .push(terminal.terminal);
    }
    let noncanonical_terminal_numbers = terminal_numbers
        .values_mut()
        .map(|numbers| {
            numbers.sort_unstable();
            numbers
                .iter()
                .enumerate()
                .filter(|(index, number)| usize::from(**number) != index + 1)
                .count()
        })
        .sum::<usize>();
    for (count, description) in [
        (terminal_identities, "terminal component identities"),
        (
            terminal_bus_references,
            "terminal bus and connectable bus references",
        ),
        (disconnected, "disconnected terminal flags"),
        (terminal_powers, "terminal active and reactive power values"),
        (
            noncanonical_terminal_numbers,
            "noncanonical terminal sequence numbers",
        ),
    ] {
        if count > 0 {
            warnings.push(
                &codes::EMIT_PSSE.field_dropped,
                format!(
                    "{count} {description} dropped: PSS/E RAWX `subterm` has no corresponding field"
                ),
            );
        }
    }
}

fn rawx_metadata_property_is_emitted(property: &str) -> bool {
    matches!(
        property,
        "psse_isub"
            | "psse_lati"
            | "psse_long"
            | "psse_srg"
            | "psse_stat"
            | "psse_vm"
            | "psse_va"
            | "psse_swdid"
            | "psse_type"
            | "psse_nstat"
            | "psse_xpu"
            | "psse_rate1"
            | "psse_rate2"
            | "psse_rate3"
            | "psse_eqid"
            | "psse_zr"
            | "psse_zx"
            | "psse_rt"
            | "psse_xt"
            | "psse_gtap"
            | "psse_rmpct"
            | "psse_baslod"
            | "psse_o1"
            | "psse_f1"
            | "psse_o2"
            | "psse_f2"
            | "psse_o3"
            | "psse_f3"
            | "psse_o4"
            | "psse_f4"
            | "psse_wmod"
            | "psse_wpf"
            | "psse_bus_1"
            | "psse_bus_2"
            | "psse_bus_3"
    )
}

#[allow(clippy::too_many_lines)]
fn add_detailed_connectivity_output_tables(
    network: &mut Map<String, Value>,
    net: &BalancedNetwork,
    warnings: &mut Diagnostics,
) -> Result<()> {
    let Some(detailed) = net.detailed_connectivity().as_deref() else {
        return Ok(());
    };
    warn_detailed_output_losses(detailed, warnings);
    if detailed.connectivity_nodes.is_empty() {
        let dropped = detailed.substations.len()
            + detailed.voltage_levels.len()
            + detailed.busbar_sections.len()
            + detailed.switches.len()
            + detailed.terminals.len();
        if dropped > 0 {
            warnings.push(
                &codes::EMIT_PSSE.record_dropped,
                format!(
                    "{dropped} detailed substation, voltage level, busbar, switch, or terminal record(s) dropped: PSS/E RAWX substation tables require connectivity nodes"
                ),
            );
        }
        return Ok(());
    }
    let metadata = metadata_by_component(detailed);
    let mut substation_numbers = BTreeMap::<ComponentId, i64>::new();
    let mut used_substation_numbers = BTreeSet::new();
    let mut next_substation_number = 1_i64;
    let mut substation_rows = Vec::new();
    for substation in &detailed.substations {
        let row_metadata = metadata.get(&substation.component).copied();
        let requested = row_metadata
            .and_then(|value| value.properties.get("psse_isub"))
            .and_then(|value| value.parse::<i64>().ok())
            .or_else(|| substation.component.local_id().parse::<i64>().ok());
        let number = if let Some(number) = requested.filter(|number| *number > 0) {
            if !used_substation_numbers.insert(number) {
                return Err(malformed(format!(
                    "detailed connectivity repeats PSS/E substation number {number}"
                )));
            }
            number
        } else {
            while used_substation_numbers.contains(&next_substation_number) {
                next_substation_number += 1;
            }
            let number = next_substation_number;
            used_substation_numbers.insert(number);
            next_substation_number += 1;
            number
        };
        substation_numbers.insert(substation.component.clone(), number);
        substation_rows.push(Value::Array(vec![
            Value::from(number),
            Value::String(
                row_metadata
                    .and_then(|value| value.name.clone())
                    .unwrap_or_else(|| substation.component.local_id().to_owned()),
            ),
            metadata_number(row_metadata, "psse_lati", 0.0),
            metadata_number(row_metadata, "psse_long", 0.0),
            metadata_number(row_metadata, "psse_srg", 0.0),
        ]));
    }
    if !substation_rows.is_empty() {
        network.insert(
            "sub".to_owned(),
            table_object(SUBSTATION_FIELDS, Value::Array(substation_rows)),
        );
    }

    let voltage_levels = detailed
        .voltage_levels
        .iter()
        .map(|level| (&level.component, level))
        .collect::<BTreeMap<_, _>>();
    let buses = net
        .buses()
        .iter()
        .map(|bus| (bus.id, bus))
        .collect::<BTreeMap<_, _>>();
    let mut node_substations = BTreeMap::<ComponentId, i64>::new();
    let mut node_numbers = BTreeMap::<ComponentId, i32>::new();
    let mut used_node_numbers = BTreeMap::<i64, BTreeSet<i32>>::new();
    for node in &detailed.connectivity_nodes {
        let level = voltage_levels.get(&node.voltage_level).ok_or_else(|| {
            malformed(format!(
                "connectivity node {} refers to unknown voltage level {}",
                node.component, node.voltage_level
            ))
        })?;
        let substation = level.substation.as_ref().ok_or_else(|| {
            malformed(format!(
                "node breaker voltage level {} has no substation",
                level.component
            ))
        })?;
        let substation_number = *substation_numbers.get(substation).ok_or_else(|| {
            malformed(format!(
                "voltage level {} refers to unknown substation {substation}",
                level.component
            ))
        })?;
        node_substations.insert(node.component.clone(), substation_number);
        if let Some(number) = node.node_number {
            if !used_node_numbers
                .entry(substation_number)
                .or_default()
                .insert(number)
            {
                return Err(malformed(format!(
                    "RAWX substation {substation_number} repeats node {number}"
                )));
            }
            node_numbers.insert(node.component.clone(), number);
        }
    }
    let mut next_node_numbers = BTreeMap::<i64, i32>::new();
    for node in &detailed.connectivity_nodes {
        if node.node_number.is_some() {
            continue;
        }
        let substation_number = node_substations[&node.component];
        let used = used_node_numbers.entry(substation_number).or_default();
        let next = next_node_numbers.entry(substation_number).or_insert(1);
        while used.contains(next) {
            *next = next.checked_add(1).ok_or_else(|| {
                malformed(format!(
                    "RAWX substation {substation_number} has no available node number"
                ))
            })?;
        }
        let number = *next;
        used.insert(number);
        node_numbers.insert(node.component.clone(), number);
        *next = next.checked_add(1).unwrap_or(i32::MAX);
    }
    let mut node_info = BTreeMap::<ComponentId, (i64, i32, BusId)>::new();
    let mut node_voltage_levels = BTreeMap::<ComponentId, ComponentId>::new();
    let mut node_rows = Vec::new();
    for node in &detailed.connectivity_nodes {
        let substation_number = node_substations[&node.component];
        let node_number = node_numbers[&node.component];
        let bus_id = node.calculated_bus.ok_or_else(|| {
            malformed(format!(
                "connectivity node {} has no calculated bus for RAWX output",
                node.component
            ))
        })?;
        let bus = buses.get(&bus_id).ok_or_else(|| {
            malformed(format!(
                "connectivity node {} refers to unknown calculated bus {bus_id}",
                node.component
            ))
        })?;
        node_info.insert(
            node.component.clone(),
            (substation_number, node_number, bus_id),
        );
        node_voltage_levels.insert(node.component.clone(), node.voltage_level.clone());
        let row_metadata = metadata.get(&node.component).copied();
        node_rows.push(Value::Array(vec![
            Value::from(substation_number),
            Value::from(node_number),
            Value::String(
                row_metadata
                    .and_then(|value| value.name.clone())
                    .unwrap_or_default(),
            ),
            Value::from(bus_id.0),
            metadata_integer(row_metadata, "psse_stat", 1),
            metadata_number_or_null(row_metadata, "psse_vm", bus.vm),
            metadata_number_or_null(row_metadata, "psse_va", bus.va),
        ]));
    }
    network.insert(
        "subnode".to_owned(),
        table_object(SUBSTATION_NODE_FIELDS, Value::Array(node_rows)),
    );

    let mut switch_rows = Vec::new();
    let mut non_node_switches = 0usize;
    for switch in &detailed.switches {
        let (TopologyEndpoint::Node(first), TopologyEndpoint::Node(second)) =
            (&switch.endpoint1, &switch.endpoint2)
        else {
            non_node_switches += 1;
            continue;
        };
        let (first_substation, first_node, _) = node_info.get(first).ok_or_else(|| {
            malformed(format!("RAWX switch endpoint {first} is not a known node"))
        })?;
        let (second_substation, second_node, _) = node_info.get(second).ok_or_else(|| {
            malformed(format!("RAWX switch endpoint {second} is not a known node"))
        })?;
        let first_level = &node_voltage_levels[first];
        let second_level = &node_voltage_levels[second];
        if first_level != second_level || &switch.voltage_level != first_level {
            return Err(malformed(format!(
                "RAWX switch {} does not stay within its declared voltage level",
                switch.component
            )));
        }
        if first_substation != second_substation {
            return Err(malformed(format!(
                "RAWX switch {} joins two substations",
                switch.component
            )));
        }
        let row_metadata = metadata.get(&switch.component).copied();
        let switch_id = metadata_text(row_metadata, "psse_swdid", || {
            switch.component.local_id().to_owned()
        });
        switch_rows.push(Value::Array(vec![
            Value::from(*first_substation),
            Value::from(*first_node),
            Value::from(*second_node),
            Value::String(switch_id),
            Value::String(
                row_metadata
                    .and_then(|value| value.name.clone())
                    .unwrap_or_default(),
            ),
            metadata_integer(
                row_metadata,
                "psse_type",
                if switch.kind == SwitchKind::Breaker {
                    2
                } else {
                    3
                },
            ),
            Value::from(i64::from(!switch.open)),
            metadata_integer(row_metadata, "psse_nstat", 1),
            metadata_number(row_metadata, "psse_xpu", 0.0),
            metadata_number(row_metadata, "psse_rate1", 0.0),
            metadata_number(row_metadata, "psse_rate2", 0.0),
            metadata_number(row_metadata, "psse_rate3", 0.0),
        ]));
    }
    if non_node_switches > 0 {
        warnings.push(
            &codes::EMIT_PSSE.record_dropped,
            format!(
                "{non_node_switches} detailed topology switch record(s) dropped: PSS/E RAWX `subswd` requires two connectivity node endpoints"
            ),
        );
    }
    if !switch_rows.is_empty() {
        network.insert(
            "subswd".to_owned(),
            table_object(SUBSTATION_SWITCH_FIELDS, Value::Array(switch_rows)),
        );
    }

    let mut terminals_by_equipment = BTreeMap::<ComponentId, Vec<&Terminal>>::new();
    for terminal in &detailed.terminals {
        terminals_by_equipment
            .entry(terminal.equipment.clone())
            .or_default()
            .push(terminal);
    }
    let output_equipment = output_equipment_keys(network, net)?;
    let mut terminal_rows = Vec::new();
    let mut unsupported_terminal_equipment = 0usize;
    for (equipment, mut terminals) in terminals_by_equipment {
        terminals.sort_by_key(|terminal| terminal.terminal);
        let row_metadata = metadata.get(&equipment).copied();
        let selected_output = output_equipment.get(&equipment);
        let equipment_type = selected_output
            .map(|key| key.equipment_type.clone())
            .or_else(|| psse_equipment_type(&equipment, terminals.len(), row_metadata));
        let Some(equipment_type) = equipment_type else {
            let implicit_busbar = detailed.busbar_sections.iter().any(|busbar| {
                busbar.component == equipment
                    && terminals.len() == 1
                    && terminals[0].terminal == 1
                    && terminals[0].node.as_ref() == Some(&busbar.node)
            });
            if !implicit_busbar {
                unsupported_terminal_equipment += terminals.len();
            }
            continue;
        };
        let equipment_id = selected_output.map_or_else(
            || {
                metadata_text(row_metadata, "psse_eqid", || {
                    equipment.local_id().to_owned()
                })
            },
            |key| key.source_id.clone(),
        );
        let terminal_buses = terminals
            .iter()
            .map(|terminal| {
                terminal
                    .node
                    .as_ref()
                    .and_then(|node| {
                        let level = node_voltage_levels.get(node)?;
                        if level != &terminal.voltage_level {
                            return None;
                        }
                        node_info.get(node)
                    })
                    .map(|(_, _, bus)| *bus)
                    .ok_or_else(|| {
                        malformed(format!(
                            "RAWX equipment terminal for {equipment} has no known node in its declared voltage level"
                        ))
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        let source_buses = row_metadata
            .map(|metadata| {
                (1..=3)
                    .filter_map(|index| {
                        metadata
                            .properties
                            .get(&format!("psse_bus_{index}"))
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .filter(|bus| *bus != 0)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (index, terminal) in terminals.iter().enumerate() {
            let node = terminal.node.as_ref().expect("validated above");
            let (substation, source_node, bus) = node_info[node];
            let mut other_buses = if source_buses.is_empty() {
                terminal_buses
                    .iter()
                    .enumerate()
                    .filter_map(|(other_index, bus)| (other_index != index).then_some(bus.0))
                    .collect::<Vec<_>>()
            } else {
                let mut buses = source_buses.clone();
                if let Some(position) = buses.iter().position(|source_bus| *source_bus == bus.0) {
                    buses.remove(position);
                }
                buses
            }
            .into_iter();
            let jbus = other_buses.next().map_or(Value::Null, Value::from);
            let kbus = other_buses.next().map_or(Value::Null, Value::from);
            terminal_rows.push(Value::Array(vec![
                Value::from(substation),
                Value::from(source_node),
                Value::String(equipment_type.clone()),
                Value::String(equipment_id.clone()),
                Value::from(bus.0),
                jbus,
                kbus,
            ]));
        }
    }
    if unsupported_terminal_equipment > 0 {
        warnings.push(
            &codes::EMIT_PSSE.record_dropped,
            format!(
                "{unsupported_terminal_equipment} detailed terminal record(s) dropped: their equipment type has no PSS/E RAWX `subterm.type` code"
            ),
        );
    }
    if !terminal_rows.is_empty() {
        network.insert(
            "subterm".to_owned(),
            table_object(SUBSTATION_TERMINAL_FIELDS, Value::Array(terminal_rows)),
        );
    }
    Ok(())
}

/// Encode detailed connectivity as the nested revision 35 RAW substation
/// records. RAWX stores the same records in four flat tables.
pub(super) fn write_raw_substation_data(
    net: &BalancedNetwork,
) -> Result<(Option<String>, Diagnostics)> {
    let mut network = Map::new();
    let mut warnings = Diagnostics::new();
    add_detailed_connectivity_output_tables(&mut network, net, &mut warnings)?;
    let Some(substations) = Table::parse(&network, "sub")? else {
        return Ok((None, warnings));
    };
    let nodes = Table::parse(&network, "subnode")?;
    let switches = Table::parse(&network, "subswd")?;
    let terminals = Table::parse(&network, "subterm")?;
    let mut output = String::new();
    let mut substitutions = 0usize;

    for substation in substations.rows {
        let isub = integer_value(substations.value(substation, "isub"), "sub.isub", None)?;
        output.push_str(&record_line(
            &substations,
            substation,
            SUBSTATION_FIELDS,
            &["name"],
            &mut substitutions,
        )?);
        output.push('\n');

        if let Some(table) = &nodes {
            for row in table.rows.iter().filter(|row| {
                integer_value(table.value(row, "isub"), "subnode.isub", None)
                    .is_ok_and(|row_isub| row_isub == isub)
            }) {
                output.push_str(&record_line(
                    table,
                    row,
                    &SUBSTATION_NODE_FIELDS[1..],
                    &["name"],
                    &mut substitutions,
                )?);
                output.push('\n');
            }
        }
        output
            .push_str("0 / END OF SUBSTATION NODE DATA, BEGIN SUBSTATION SWITCHING DEVICE DATA\n");

        if let Some(table) = &switches {
            for row in table.rows.iter().filter(|row| {
                integer_value(table.value(row, "isub"), "subswd.isub", None)
                    .is_ok_and(|row_isub| row_isub == isub)
            }) {
                output.push_str(&record_line(
                    table,
                    row,
                    &SUBSTATION_SWITCH_FIELDS[1..],
                    &["swdid", "name"],
                    &mut substitutions,
                )?);
                output.push('\n');
            }
        }
        output.push_str(
            "0 / END OF SUBSTATION SWITCHING DEVICE DATA, BEGIN SUBSTATION EQUIPMENT TERMINAL DATA\n",
        );

        if let Some(table) = &terminals {
            for row in table.rows.iter().filter(|row| {
                integer_value(table.value(row, "isub"), "subterm.isub", None)
                    .is_ok_and(|row_isub| row_isub == isub)
            }) {
                let mut fields = vec!["ibus", "inode", "type"];
                let jbus = integer_value(table.value(row, "jbus"), "subterm.jbus", Some(0))?;
                let kbus = integer_value(table.value(row, "kbus"), "subterm.kbus", Some(0))?;
                if jbus != 0 {
                    fields.push("jbus");
                }
                if kbus != 0 {
                    fields.push("kbus");
                }
                fields.push("eqid");
                output.push_str(&record_line(
                    table,
                    row,
                    &fields,
                    &["type", "eqid"],
                    &mut substitutions,
                )?);
                output.push('\n');
            }
        }
        output.push_str("0 / END OF SUBSTATION EQUIPMENT TERMINAL DATA\n");
    }
    if substitutions > 0 {
        warnings.push(
            &codes::EMIT_PSSE.value_substituted,
            format!(
                "{substitutions} quoted PSS/E substation field(s) (`sub.name`, `subnode.name`, `subswd.swdid`/`name`, or `subterm.type`/`eqid`) contained a quote or line break; replaced with spaces"
            ),
        );
    }
    Ok((Some(output), warnings))
}

fn table_object(fields: &[&str], data: Value) -> Value {
    let mut object = Map::new();
    object.insert(
        "fields".to_owned(),
        Value::Array(
            fields
                .iter()
                .map(|field| Value::String((*field).to_owned()))
                .collect(),
        ),
    );
    object.insert("data".to_owned(), data);
    Value::Object(object)
}

fn line_tokens(line: &str) -> Vec<String> {
    super::psse::fields(line)
        .into_iter()
        .map(std::borrow::Cow::into_owned)
        .collect()
}

fn append_padded(target: &mut Vec<String>, source: &[String], width: usize) {
    target.extend(source.iter().take(width).cloned());
    target.resize(
        target.len() + width.saturating_sub(source.len()),
        String::new(),
    );
}

fn tokens_as_values(tokens: &[String], fields: &[&str], strings: &[&str]) -> Result<Vec<Value>> {
    fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let token = tokens.get(index).map_or("", String::as_str);
            if strings.contains(field) {
                return Ok(Value::String(token.to_owned()));
            }
            if token.is_empty() {
                return Ok(Value::Null);
            }
            if let Ok(value) = token.parse::<i64>() {
                return Ok(Value::from(value));
            }
            let value = token
                .parse::<f64>()
                .map_err(|_| malformed(format!("internal PSS/E field `{field}` is not numeric")))?;
            Ok(jnum(value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    fn table_value(root: &Value, table: &str, row: usize, field: &str) -> Value {
        let table = &root["network"][table];
        let column = table["fields"]
            .as_array()
            .unwrap()
            .iter()
            .position(|value| value == field)
            .unwrap();
        table["data"][row][column].clone()
    }

    const MINIMAL: &str = r#"{
      "network": {
        "caseid": {"fields":["rev","sbase","basfrq","title1"],"data":[35,100,60,"rawx-small"]},
        "bus": {"fields":["ide","ibus","name","baskv","vm","va"],"data":[[3,1,"Slack",230,1,0],[1,2,"Load",230,1,-1]]},
        "load": {"fields":["ql","ibus","pl","loadid","stat","ip","iq","yp","yq"],"data":[[5,2,20,"L1",1,1,2,3,4]]},
        "generator": {"fields":["ibus","machid","pg","qg","qt","qb","vs","ireg","mbase","stat","pt","pb"],"data":[[1,"G1",20,5,50,-50,1,2,100,1,80,0]]},
        "acline": {"fields":["ibus","jbus","ckt","rpu","xpu","bpu","rate1","stat"],"data":[[1,2,"1",0.01,0.1,0.02,100,1]]}
      }
    }"#;

    const TRANSFORMERS_AND_SWITCH: &str = r#"{
      "network": {
        "caseid": {"fields":["sbase","rev","basfrq","title1"],"data":[100,35,60,"rawx-transformers"]},
        "bus": {"fields":["ibus","name","baskv","ide","vm","va"],"data":[[1,"B1",230,3,1,0],[2,"B2",115,1,1,0],[3,"B3",69,1,1,0]]},
        "transformer": {
          "fields":["ibus","jbus","kbus","ckt","cw","cz","cm","mag1","mag2","name","stat","r1_2","x1_2","sbase1_2","r2_3","x2_3","sbase2_3","r3_1","x3_1","sbase3_1","vmstar","anstar","windv1","nomv1","ang1","wdg1rate1","wdg1rate2","wdg1rate3","cod1","cont1","rma1","rmi1","vma1","vmi1","ntp1","windv2","nomv2","ang2","wdg2rate1","wdg2rate2","wdg2rate3","windv3","nomv3","ang3","wdg3rate1","wdg3rate2","wdg3rate3"],
          "data":[
            [1,2,0,"T1",2,2,1,0.001,-0.002,"TWO",1,0.01,0.1,50,null,null,null,null,null,null,null,null,230,230,5,90,80,70,1,2,1.1,0.9,1.05,0.95,17,115,115,0,60,50,40,null,null,null,null,null,null],
            [1,2,3,"T3",1,1,1,0,0,"THREE",1,0.01,0.10,100,0.02,0.20,100,0.03,0.30,100,1.01,2,1,230,0,100,90,80,0,0,1.1,0.9,1.1,0.9,33,1,115,1,70,60,50,1,69,-2,40,30,20]
          ]
        },
        "sysswd": {"fields":["jbus","ibus","ckt","xpu","rate1","stat","nstat","met","stype","name"],"data":[[3,2,"S1",0.0001,55,0,1,1,2,"breaker"]]},
        "sub": {"fields":["isub","name","lati","long","srg"],"data":[[1,"SUB",42,-83,0.1]]},
        "subnode": {"fields":["isub","inode","name","ibus","stat","vm","va"],"data":[[1,1,"BUSBAR",1,1,1,0],[1,2,"T1-H",1,1,1,0],[1,3,"T1-L",2,1,1,0],[1,4,"T3-T",3,1,1,0]]},
        "subswd": {"fields":["isub","inode","jnode","swdid","name","type","stat","nstat","xpu","rate1","rate2","rate3","rsetnam"],"data":[[1,1,2,"BR1","BREAKER",2,1,1,0.0001,100,90,80,""]]},
        "subterm": {"fields":["isub","inode","type","eqid","ibus","jbus","kbus"],"data":[[1,2,"2","T1",1,2,0],[1,3,"2","T1",2,1,0],[1,2,"3","T3",1,2,3],[1,3,"3","T3",2,1,3],[1,4,"3","T3",3,1,2]]}
      }
    }"#;

    #[test]
    fn reads_arbitrary_field_order_and_zip_loads() {
        let mut diagnostics = Diagnostics::new();
        let net = parse_rawx_source(MINIMAL, None, &mut diagnostics).unwrap();
        assert_eq!(net.buses().len(), 2);
        close(net.loads()[0].p, 24.0);
        close(net.loads()[0].q, 11.0);
        assert_eq!(net.generators()[0].regulated_bus, Some(BusId(2)));
        close(net.branches()[0].rate_a, 100.0);
    }

    #[test]
    fn applies_psse_defaults_to_omitted_case_parameters() {
        let minimal = MINIMAL.replace(
            r#"["rev","sbase","basfrq","title1"],"data":[35,100,60,"rawx-small"]"#,
            r#"["rev","title1"],"data":[35,"rawx-small"]"#,
        );
        let net = parse_rawx_source(&minimal, None, &mut Diagnostics::new()).unwrap();
        close(net.base_mva(), 100.0);
        close(net.base_frequency(), 60.0);
    }

    #[test]
    fn emits_parseable_revision_35_rawx() {
        let mut diagnostics = Diagnostics::new();
        let mut net = parse_rawx_source(MINIMAL, None, &mut diagnostics).unwrap();
        net.generators_mut()[0].energy_source = crate::network::GeneratorEnergySource::Wind;
        let emitted = write_rawx(&net).unwrap();
        assert!(emitted.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic.message().contains("generator energy source")
                && diagnostic.message().contains("wind=1")
        }));
        let root: Value = serde_json::from_str(&emitted.text).unwrap();
        assert_eq!(root["network"]["caseid"]["data"][2], 35);
        let mut diagnostics = Diagnostics::new();
        let back = parse_rawx_source(&emitted.text, None, &mut diagnostics).unwrap();
        assert_eq!(back.buses().len(), 2);
        close(back.loads()[0].p, 24.0);
    }

    #[test]
    fn rejects_other_revisions_and_bad_row_widths() {
        let wrong_revision = MINIMAL.replace("[35,100", "[34,100");
        assert!(parse_rawx_source(&wrong_revision, None, &mut Diagnostics::new()).is_err());
        let short = MINIMAL.replace("[3,1,\"Slack\",230,1,0]", "[3,1]");
        assert!(parse_rawx_source(&short, None, &mut Diagnostics::new()).is_err());
    }

    #[test]
    fn rejects_nonfinite_and_invalid_numeric_values() {
        let nonfinite = MINIMAL.replace("0.01", "1e9999");
        assert!(parse_rawx_source(&nonfinite, None, &mut Diagnostics::new()).is_err());
        let bad = MINIMAL.replace("0.01", "\"not-a-number\"");
        assert!(parse_rawx_source(&bad, None, &mut Diagnostics::new()).is_err());
        let string_nonfinite = MINIMAL.replace("0.01", "\"NaN\"");
        assert!(parse_rawx_source(&string_nonfinite, None, &mut Diagnostics::new()).is_err());
    }

    #[test]
    fn diagnoses_unknown_tables_case_fields_and_retained_header_values() {
        let mut root: Value = serde_json::from_str(MINIMAL).unwrap();
        let network = root["network"].as_object_mut().unwrap();
        network.insert(
            "vendor_table".to_owned(),
            serde_json::json!({"fields": ["id"], "data": [[1], [2]]}),
        );
        network.insert(
            "empty_vendor_table".to_owned(),
            serde_json::json!({"fields": ["id"], "data": []}),
        );
        let caseid = network["caseid"].as_object_mut().unwrap();
        caseid["fields"]
            .as_array_mut()
            .unwrap()
            .extend(["vendorflag", "ic", "xfrrat", "nxfrat", "title2"].map(Value::from));
        caseid["data"].as_array_mut().unwrap().extend([
            Value::from(7),
            Value::from(1),
            Value::from(2),
            Value::from(3),
            Value::from("second title"),
        ]);
        caseid.insert("vendor_member".to_owned(), Value::Bool(true));

        let mut diagnostics = Diagnostics::new();
        parse_rawx_source(
            &serde_json::to_string(&root).unwrap(),
            None,
            &mut diagnostics,
        )
        .unwrap();
        let lines = diagnostics.lines();
        assert!(lines.iter().any(|line| {
            line.contains("READ.PSSE.SECTION_UNSUPPORTED")
                && line.contains("vendor_table")
                && line.contains('2')
        }));
        assert!(
            lines
                .iter()
                .all(|line| !line.contains("empty_vendor_table"))
        );
        assert!(lines.iter().any(|line| {
            line.contains("READ.PSSE.FIELD_DROPPED")
                && line.contains("caseid")
                && line.contains("vendorflag")
        }));
        assert!(lines.iter().any(|line| line.contains("vendor_member")));
        for field in [
            "caseid.ic",
            "caseid.xfrrat",
            "caseid.nxfrat",
            "caseid.title2",
        ] {
            assert!(
                lines.iter().any(|line| line.contains(field)),
                "missing diagnostic for {field}: {lines:?}"
            );
        }
    }

    #[test]
    fn preserves_string_encoded_transformer_zcod() {
        let mut root: Value = serde_json::from_str(TRANSFORMERS_AND_SWITCH).unwrap();
        let transformer = root["network"]["transformer"].as_object_mut().unwrap();
        transformer["fields"]
            .as_array_mut()
            .unwrap()
            .push(Value::from("zcod"));
        let rows = transformer["data"].as_array_mut().unwrap();
        rows[0].as_array_mut().unwrap().push(Value::from("1"));
        rows[1].as_array_mut().unwrap().push(Value::Null);

        let mut diagnostics = Diagnostics::new();
        let parsed = parse_rawx_source(
            &serde_json::to_string(&root).unwrap(),
            None,
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(parsed.branches()[0].extras["psse_zcod"], Value::from(1));
        assert!(
            diagnostics
                .lines()
                .iter()
                .all(|line| !line.contains("ZCOD")),
            "unexpected diagnostic: {:?}",
            diagnostics.lines()
        );

        let emitted = write_rawx(&parsed).unwrap();
        let reparsed = parse_rawx_source(&emitted.text, None, &mut Diagnostics::new()).unwrap();
        assert_eq!(reparsed.branches()[0].extras["psse_zcod"], Value::from(1));
    }

    #[test]
    fn rawx_output_returns_detailed_connectivity_errors_instead_of_panicking() {
        let mut net =
            parse_rawx_source(TRANSFORMERS_AND_SWITCH, None, &mut Diagnostics::new()).unwrap();
        let detailed = std::sync::Arc::make_mut(net.detailed_connectivity_mut().as_mut().unwrap());
        detailed.substations.push(detailed.substations[0].clone());

        let error = write_rawx(&net).unwrap_err();
        assert!(matches!(error, Error::Emit { .. }));
        assert!(
            error
                .to_string()
                .contains("repeats PSS/E substation number")
        );
    }

    #[test]
    fn raw_substation_output_diagnoses_sanitized_names_and_identifiers() {
        let mut net =
            parse_rawx_source(TRANSFORMERS_AND_SWITCH, None, &mut Diagnostics::new()).unwrap();
        let detailed = std::sync::Arc::make_mut(net.detailed_connectivity_mut().as_mut().unwrap());
        let substation = detailed.substations[0].component.clone();
        let node = detailed.connectivity_nodes[0].component.clone();
        let switch = detailed.switches[0].component.clone();
        let equipment = detailed.terminals[0].equipment.clone();

        detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| metadata.component == substation)
            .unwrap()
            .name = Some("SUB'NAME".into());
        detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| metadata.component == node)
            .unwrap()
            .name = Some("NODE'NAME".into());
        let switch_metadata = detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| metadata.component == switch)
            .unwrap();
        switch_metadata.name = Some("SWITCH'NAME".into());
        switch_metadata
            .properties
            .insert("psse_swdid".into(), "SW'ID".into());
        detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| metadata.component == equipment)
            .unwrap()
            .properties
            .insert("psse_eqid".into(), "EQ'ID".into());

        let (records, warnings) = write_raw_substation_data(&net).unwrap();
        let records = records.unwrap();
        for sanitized in [
            "'SUB NAME'",
            "'NODE NAME'",
            "'SW ID'",
            "'SWITCH NAME'",
            "'EQ ID'",
        ] {
            assert!(
                records.contains(sanitized),
                "missing {sanitized}: {records}"
            );
        }
        assert!(warnings.records().iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.VALUE_SUBSTITUTED"
                && diagnostic.message().contains("sub.name")
                && diagnostic.message().contains("subswd.swdid")
                && diagnostic.message().contains("subterm.type`/`eqid")
                && diagnostic.message().contains("replaced with spaces")
        }));
    }

    #[test]
    fn rawx_output_diagnoses_unrepresentable_switch_and_terminal_data() {
        let mut net =
            parse_rawx_source(TRANSFORMERS_AND_SWITCH, None, &mut Diagnostics::new()).unwrap();
        let detailed = std::sync::Arc::make_mut(net.detailed_connectivity_mut().as_mut().unwrap());
        let switch = &mut detailed.switches[0];
        switch.endpoint1 = TopologyEndpoint::Bus(ComponentId::new("bus", "1").unwrap());
        switch.kind = SwitchKind::LoadBreakSwitch;
        switch.retained = true;

        let terminal = &mut detailed.terminals[0];
        terminal.component = Some(ComponentId::new("terminal", "t1").unwrap());
        terminal.bus = Some(ComponentId::new("bus", "configured").unwrap());
        terminal.connectable_bus = Some(ComponentId::new("bus", "connectable").unwrap());
        terminal.connected = false;
        terminal.active_power_mw = Some(1.0);
        terminal.reactive_power_mvar = Some(2.0);

        let mut unsupported = detailed.terminals[1].clone();
        unsupported.equipment = ComponentId::new("junction", "unsupported").unwrap();
        unsupported.terminal = 1;
        detailed.terminals.push(unsupported);

        let emitted = write_rawx(&net).unwrap();
        let diagnostics = emitted.render_diagnostics();
        for expected in [
            "requires two connectivity node endpoints",
            "load break switch kind",
            "retained flag",
            "terminal component identities",
            "terminal bus and connectable bus references",
            "disconnected terminal flags",
            "terminal active and reactive power values",
            "equipment type has no PSS/E RAWX `subterm.type` code",
        ] {
            assert!(
                diagnostics.iter().any(|line| line.contains(expected)),
                "missing {expected:?}: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn rawx_output_diagnoses_detailed_records_without_connectivity_nodes() {
        let mut net =
            parse_rawx_source(TRANSFORMERS_AND_SWITCH, None, &mut Diagnostics::new()).unwrap();
        let detailed = std::sync::Arc::make_mut(net.detailed_connectivity_mut().as_mut().unwrap());
        detailed.connectivity_nodes.clear();

        let emitted = write_rawx(&net).unwrap();
        assert!(emitted.render_diagnostics().iter().any(|line| {
            line.contains("EMIT.PSSE.RECORD_DROPPED") && line.contains("require connectivity nodes")
        }));
    }

    #[test]
    fn rejects_json_past_the_parser_nesting_limit() {
        let mut nested =
            String::from(r#"{"network":{"caseid":{"fields":["rev"],"data":[35]},"extra":"#);
        nested.extend(std::iter::repeat_n('[', 160));
        nested.push('0');
        nested.extend(std::iter::repeat_n(']', 160));
        nested.push_str("}}");
        assert!(parse_rawx_source(&nested, None, &mut Diagnostics::new()).is_err());
    }

    #[test]
    fn reads_transformer_basis_codes_controls_and_system_switches() {
        let mut diagnostics = Diagnostics::new();
        let net = parse_rawx_source(TRANSFORMERS_AND_SWITCH, None, &mut diagnostics).unwrap();
        assert_eq!(net.branches().len(), 1);
        let transformer = &net.branches()[0];
        assert_eq!(transformer.name.as_deref(), Some("TWO"));
        assert!((transformer.r - 0.02).abs() < 1e-12);
        assert!((transformer.x - 0.20).abs() < 1e-12);
        assert!((transformer.tap - 1.0).abs() < 1e-12);
        close(transformer.shift, 5.0);
        let control = transformer.control.as_ref().unwrap();
        assert_eq!(control.controlled_bus, Some(BusId(2)));
        assert_eq!(control.ntp, 17);
        assert_eq!(net.transformers_3w().len(), 1);
        assert_eq!(net.transformers_3w()[0].windings[2].bus, BusId(3));
        assert_eq!(net.switches().len(), 1);
        assert!(!net.switches()[0].closed);
        assert_eq!(net.switches()[0].thermal_rating, Some(55.0));
        let detailed = net.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.substations.len(), 1);
        assert_eq!(detailed.voltage_levels.len(), 3);
        assert_eq!(detailed.connectivity_nodes.len(), 4);
        assert_eq!(detailed.switches.len(), 1);
        assert_eq!(detailed.switches[0].kind, SwitchKind::Breaker);
        assert!(!detailed.switches[0].open);
        assert_eq!(detailed.terminals.len(), 6);
        assert_eq!(detailed.busbar_sections.len(), 1);
        assert!(diagnostics.into_records().iter().all(|diagnostic| {
            diagnostic.code() != "READ.PSSE.SECTION_UNSUPPORTED"
                || !diagnostic.message().contains("substation")
        }));
    }

    #[test]
    fn powysbl_system_switch_rating_set_name_round_trips() {
        let mut root: Value = serde_json::from_str(TRANSFORMERS_AND_SWITCH).unwrap();
        root["network"]["sysswd"] = serde_json::json!({
            "fields": ["ibus", "jbus", "ckt", "xpu", "rsetnam", "stat", "nstat", "met", "stype", "name"],
            "data": [[3, 2, "S1", 0.0001, "RATESET", 0, 1, 1, 2, "breaker"]]
        });
        let source = serde_json::to_string(&root).unwrap();
        let mut parsed = parse_rawx_source(&source, None, &mut Diagnostics::new()).unwrap();
        let switch = &parsed.switches()[0];
        assert_eq!(switch.thermal_rating, None);
        assert_eq!(switch.extras["psse_rsetnam"], Value::from("RATESET"));
        parsed
            .switches_mut()
            .push(Switch::new(BusId(1), BusId(2), true));
        parsed
            .switches_mut()
            .push(Switch::new(BusId(2), BusId(3), true));
        parsed
            .switches_mut()
            .push(Switch::new(BusId(1), BusId(2), false));

        let emitted = write_rawx(&parsed).unwrap();
        assert!(!emitted.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic.message().contains("rating set name")
        }));
        let output: Value = serde_json::from_str(&emitted.text).unwrap();
        assert_eq!(
            output["network"]["sysswd"]["fields"],
            serde_json::json!([
                "ibus", "jbus", "ckt", "xpu", "rsetnam", "stat", "nstat", "met", "stype", "name"
            ])
        );
        assert_eq!(output["network"]["sysswd"]["data"][0][4], "RATESET");
        assert_eq!(output["network"]["sysswd"]["data"][1][2], "1");
        assert_eq!(output["network"]["sysswd"]["data"][2][2], "1");
        assert_eq!(output["network"]["sysswd"]["data"][3][2], "2");

        let reparsed = parse_rawx_source(&emitted.text, None, &mut Diagnostics::new()).unwrap();
        assert_eq!(
            reparsed.switches()[0].extras["psse_rsetnam"],
            Value::from("RATESET")
        );

        let mut mixed = parsed;
        mixed.switches_mut()[0].thermal_rating = Some(55.0);
        let emitted = write_rawx(&mixed).unwrap();
        assert!(emitted.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic.message().contains("rating set name")
                && diagnostic.message().contains("RATE1-RATE12")
        }));
    }

    #[test]
    fn transformer_and_switch_output_reloads() {
        let mut diagnostics = Diagnostics::new();
        let mut net = parse_rawx_source(TRANSFORMERS_AND_SWITCH, None, &mut diagnostics).unwrap();
        net.switches_mut()[0].current_rating = Some(2_000.0);
        net.switches_mut()[0].pf = Some(10.0);
        net.switches_mut()[0].qf = Some(2.0);
        net.switches_mut()[0].pt = Some(-9.8);
        net.switches_mut()[0].qt = Some(-1.9);
        let emitted = write_rawx(&net).unwrap();
        assert!(emitted.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic.message().contains("switch current rating")
        }));
        assert!(emitted.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                && diagnostic.message().contains("switch power flow result")
        }));
        let root: Value = serde_json::from_str(&emitted.text).unwrap();
        assert_eq!(
            root["network"]["transformer"]["data"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(root["network"]["transformer"]["data"][0][3], "T1");
        assert_eq!(root["network"]["transformer"]["data"][1][3], "T3");
        assert_eq!(
            root["network"]["sysswd"]["data"].as_array().unwrap().len(),
            1
        );
        assert_eq!(root["network"]["sub"]["data"].as_array().unwrap().len(), 1);
        assert_eq!(
            root["network"]["subnode"]["data"].as_array().unwrap().len(),
            4
        );
        assert_eq!(
            root["network"]["subswd"]["data"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            root["network"]["subterm"]["data"].as_array().unwrap().len(),
            5
        );
        assert_eq!(root["network"]["subnode"]["data"][0][4].as_i64(), Some(1));
        assert_eq!(root["network"]["subswd"]["data"][0][5].as_i64(), Some(2));
        assert_eq!(root["network"]["subswd"]["data"][0][6].as_i64(), Some(1));
        assert_eq!(root["network"]["subswd"]["data"][0][7].as_i64(), Some(1));
        let mut diagnostics = Diagnostics::new();
        let back = parse_rawx_source(&emitted.text, None, &mut diagnostics).unwrap();
        assert_eq!(back.branches().len(), 1);
        assert_eq!(back.branches()[0].name.as_deref(), Some("TWO"));
        assert_eq!(back.transformers_3w().len(), 1);
        assert_eq!(back.transformers_3w()[0].name.as_deref(), Some("THREE"));
        assert_eq!(back.switches().len(), 1);
        let detailed = back.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.connectivity_nodes.len(), 4);
        assert_eq!(detailed.switches.len(), 1);
        assert_eq!(detailed.terminals.len(), 6);
    }

    #[test]
    fn exact_node_regulation_targets_survive_rawx_output() {
        let mut root: Value = serde_json::from_str(TRANSFORMERS_AND_SWITCH).unwrap();
        root["network"]["generator"] = serde_json::json!({
            "fields": ["ibus", "machid", "pg", "qg", "qt", "qb", "vs", "ireg", "nreg", "mbase", "stat", "pt", "pb"],
            "data": [[1, "G1", 20, 0, 50, -50, 1, 1, 1, 100, 1, 80, 0]]
        });
        root["network"]["swshunt"] = serde_json::json!({
            "fields": ["ibus", "shntid", "modsw", "adjm", "stat", "vswhi", "vswlo", "swreg", "nreg", "rmpct", "rmidnt", "binit", "s1", "n1", "b1"],
            "data": [[1, "S1", 1, 0, 1, 1.05, 0.95, 1, 1, 100, "", 0, 1, 1, 0.01]]
        });
        let transformer = root["network"]["transformer"].as_object_mut().unwrap();
        transformer
            .get_mut("fields")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .extend(["node1", "cod2", "cont2", "node2", "cod3", "cont3", "node3"].map(Value::from));
        let rows = transformer.get_mut("data").unwrap().as_array_mut().unwrap();
        rows[0]
            .as_array_mut()
            .unwrap()
            .extend([3, 0, 0, 0, 0, 0, 0].map(Value::from));
        rows[1].as_array_mut().unwrap()[28] = Value::from(1);
        rows[1].as_array_mut().unwrap()[29] = Value::from(1);
        rows[1]
            .as_array_mut()
            .unwrap()
            .extend([2, 1, 2, 3, 1, 3, 4].map(Value::from));

        let source = serde_json::to_string(&root).unwrap();
        let net = parse_rawx_source(&source, None, &mut Diagnostics::new()).unwrap();
        assert!(net.generators()[0].regulating_terminal.is_some());
        assert!(
            net.shunts()
                .last()
                .unwrap()
                .control
                .as_ref()
                .unwrap()
                .regulating_terminal
                .is_some()
        );
        assert!(
            net.branches()[0]
                .control
                .as_ref()
                .unwrap()
                .regulating_terminal
                .is_some()
        );
        assert!(net.transformers_3w()[0].windings.iter().all(|winding| {
            winding
                .control
                .as_ref()
                .and_then(|control| control.regulating_terminal.as_ref())
                .is_some()
        }));

        let emitted = write_rawx(&net).unwrap();
        let output: Value = serde_json::from_str(&emitted.text).unwrap();
        assert_eq!(table_value(&output, "generator", 0, "ireg"), 1);
        assert_eq!(table_value(&output, "generator", 0, "nreg"), 1);
        assert_eq!(table_value(&output, "swshunt", 0, "swreg"), 1);
        assert_eq!(table_value(&output, "swshunt", 0, "nreg"), 1);
        assert_eq!(table_value(&output, "transformer", 0, "cont1"), 2);
        assert_eq!(table_value(&output, "transformer", 0, "node1"), 3);
        assert_eq!(table_value(&output, "transformer", 1, "cont1"), 1);
        assert_eq!(table_value(&output, "transformer", 1, "node1"), 2);
        assert_eq!(table_value(&output, "transformer", 1, "cont2"), 2);
        assert_eq!(table_value(&output, "transformer", 1, "node2"), 3);
        assert_eq!(table_value(&output, "transformer", 1, "cont3"), 3);
        assert_eq!(table_value(&output, "transformer", 1, "node3"), 4);

        let back = parse_rawx_source(&emitted.text, None, &mut Diagnostics::new()).unwrap();
        assert!(back.generators()[0].regulating_terminal.is_some());
        assert!(back.transformers_3w()[0].windings.iter().all(|winding| {
            winding
                .control
                .as_ref()
                .and_then(|control| control.regulating_terminal.as_ref())
                .is_some()
        }));
    }

    #[test]
    fn nonzero_regulating_nodes_without_connectivity_are_diagnosed() {
        let mut root: Value = serde_json::from_str(TRANSFORMERS_AND_SWITCH).unwrap();
        let network = root["network"].as_object_mut().unwrap();
        for table in ["sub", "subnode", "subswd", "subterm"] {
            network.remove(table);
        }
        network.insert(
            "generator".to_owned(),
            serde_json::json!({
                "fields": ["ibus", "machid", "pg", "qg", "qt", "qb", "vs", "ireg", "nreg", "mbase", "stat", "pt", "pb"],
                "data": [[1, "G1", 20, 0, 50, -50, 1, 1, 7, 100, 1, 80, 0]]
            }),
        );
        let transformer = network
            .get_mut("transformer")
            .unwrap()
            .as_object_mut()
            .unwrap();
        transformer
            .get_mut("fields")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(Value::from("node1"));
        for row in transformer.get_mut("data").unwrap().as_array_mut().unwrap() {
            row.as_array_mut().unwrap().push(Value::from(8));
        }

        let mut diagnostics = Diagnostics::new();
        let parsed = parse_rawx_source(
            &serde_json::to_string(&root).unwrap(),
            None,
            &mut diagnostics,
        )
        .unwrap();

        assert!(parsed.generators()[0].regulating_terminal.is_none());
        assert!(
            parsed.branches()[0]
                .control
                .as_ref()
                .unwrap()
                .regulating_terminal
                .is_none()
        );
        let lines = diagnostics.lines();
        assert!(lines.iter().any(|line| {
            line.contains("generator at bus 1")
                && line.contains("node 7")
                && line.contains("no detailed connectivity terminal")
        }));
        assert!(lines.iter().any(|line| {
            line.contains("two winding transformer 1-2")
                && line.contains("node 8")
                && line.contains("no detailed connectivity terminal")
        }));
    }

    #[test]
    fn output_allocates_missing_node_numbers_without_collisions() {
        let mut net =
            parse_rawx_source(TRANSFORMERS_AND_SWITCH, None, &mut Diagnostics::new()).unwrap();
        let detailed = std::sync::Arc::make_mut(net.detailed_connectivity_mut().as_mut().unwrap());
        for (index, node) in detailed.connectivity_nodes.iter_mut().enumerate() {
            node.node_number = (index == 0).then_some(9);
        }

        let emitted = write_rawx(&net).unwrap();
        let back = parse_rawx_source(&emitted.text, None, &mut Diagnostics::new()).unwrap();
        let numbers = back
            .detailed_connectivity()
            .as_ref()
            .unwrap()
            .connectivity_nodes
            .iter()
            .filter_map(|node| node.node_number)
            .collect::<BTreeSet<_>>();
        assert_eq!(numbers, BTreeSet::from([1, 2, 3, 9]));
        assert!(emitted.diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "EMIT.PSSE.VALUE_DEFAULTED"
                && diagnostic
                    .message()
                    .contains("3 connectivity node(s) state no PSS/E node number")
        }));
    }

    #[test]
    fn reads_powysbl_substation_switch_rows_with_undeclared_ratings() {
        let mut root: Value = serde_json::from_str(TRANSFORMERS_AND_SWITCH).unwrap();
        root["network"]["subswd"]["fields"] = serde_json::json!([
            "isub", "inode", "jnode", "swdid", "name", "type", "stat", "nstat", "xpu", "rsetnam"
        ]);
        root["network"]["subswd"]["data"] =
            serde_json::json!([[1, 1, 2, "BR1", "BREAKER", 2, 1, 1, 0.0001, 100, 90, 80]]);
        let source = serde_json::to_string(&root).unwrap();
        let mut diagnostics = Diagnostics::new();

        let net = parse_rawx_source(&source, None, &mut diagnostics).unwrap();

        let detailed = net.detailed_connectivity().as_deref().unwrap();
        let switch_metadata = detailed
            .component_metadata
            .iter()
            .find(|metadata| metadata.component.component_type() == "switch")
            .unwrap();
        assert_eq!(switch_metadata.properties.get("psse_rate1").unwrap(), "100");
        assert_eq!(switch_metadata.properties.get("psse_rate2").unwrap(), "90");
        assert_eq!(switch_metadata.properties.get("psse_rate3").unwrap(), "80");
        assert!(diagnostics.lines().iter().any(|line| {
            line.contains("READ.PSSE.VALUE_SUBSTITUTED")
                && line.contains("read them as `rate1`, `rate2`, and `rate3`")
        }));
    }

    #[test]
    fn detailed_terminals_use_the_rawx_equipment_ids() {
        let mut root: Value = serde_json::from_str(MINIMAL).unwrap();
        root["network"]["sub"] = serde_json::json!({
            "fields": ["isub", "name", "lati", "long", "srg"],
            "data": [[1, "SUB", 0, 0, 0]]
        });
        root["network"]["subnode"] = serde_json::json!({
            "fields": ["isub", "inode", "name", "ibus", "stat", "vm", "va"],
            "data": [[1, 1, "GEN", 1, 1, 1, 0], [1, 2, "LINE", 1, 1, 1, 0]]
        });
        root["network"]["subterm"] = serde_json::json!({
            "fields": ["isub", "inode", "type", "eqid", "ibus", "jbus", "kbus"],
            "data": [
                [1, 1, "M", "G1", 1, null, null],
                [1, 2, "B", "1", 1, 2, null]
            ]
        });
        let source = serde_json::to_string(&root).unwrap();
        let mut net = parse_rawx_source(&source, None, &mut Diagnostics::new()).unwrap();

        let emitted = write_rawx(&net).unwrap();
        let root: Value = serde_json::from_str(&emitted.text).unwrap();

        assert_eq!(root["network"]["generator"]["data"][0][1], "G1");
        let line_terminal = root["network"]["subterm"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row[2] == "B")
            .unwrap();
        assert_eq!(line_terminal[4], 1);
        assert_eq!(line_terminal[5], 2);
        assert!(line_terminal[6].is_null());

        let detailed = std::sync::Arc::make_mut(
            net.detailed_connectivity_mut()
                .as_mut()
                .expect("the source states detailed connectivity"),
        );
        for metadata in &mut detailed.component_metadata {
            metadata.properties.remove("psse_eqid");
        }
        let emitted = write_rawx(&net).unwrap();
        let root: Value = serde_json::from_str(&emitted.text).unwrap();
        let terminals = root["network"]["subterm"]["data"].as_array().unwrap();
        let generator_terminal = terminals.iter().find(|row| row[2] == "M").unwrap();
        let line_terminal = terminals.iter().find(|row| row[2] == "B").unwrap();
        assert_eq!(
            generator_terminal[3],
            root["network"]["generator"]["data"][0][1]
        );
        assert_eq!(line_terminal[3], root["network"]["acline"]["data"][0][2]);

        let mut diagnostics = Diagnostics::new();
        parse_rawx_source(&emitted.text, None, &mut diagnostics).unwrap();
        assert!(
            diagnostics
                .lines()
                .iter()
                .all(|line| !line.contains("outside the typed balanced calculation view"))
        );
    }
}
