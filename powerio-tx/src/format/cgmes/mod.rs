//! CGMES (IEC 61970-600) CIMXML import and emission for
//! [`BalancedNetwork`](crate::network::BalancedNetwork).
//!
//! A CGMES case is a set of RDF/XML instance files, one per profile: EQ
//! (Equipment), TP (Topology), SSH (Steady State Hypothesis), SV (State
//! Variables), plus boundary, diagram, geography, and dynamics parts, tied together
//! by `md:FullModel` headers. The reader takes a directory (or an explicit
//! file list), classifies each file by its header profile URIs, merges the
//! object descriptions across files (`rdf:ID` defines, `rdf:about` extends),
//! and maps equipment, hierarchy, detailed connectivity, and the bus branch
//! calculation view onto `BalancedNetwork`.
//!
//! Both CGMES 2.4.15 (CIM16, `http://iec.ch/TC57/2013/CIM-schema-cim16#`,
//! ENTSO-E extensions under `entsoe:`) and CGMES 3.0 (CIM100,
//! `http://iec.ch/TC57/CIM100#`, `eu:` extensions) parse; the version is
//! detected from the `cim` namespace, tolerating vendor spellings of the CIM16
//! URI year. Fresh emission writes a deterministic CGMES 3.0 EQ, TP, SSH,
//! and SV profile set. The shared format dispatcher reads a profile directory,
//! a directory containing profile ZIP files, or one ZIP archive.
//!
//! CGMES has no system MVA base (values are MW/MVAr/kV/ohm); the reader
//! normalizes onto 100 MVA. The base frequency comes from the EQ
//! `BaseFrequency` record when present, else 50 Hz (the ENTSO-E default),
//! reported as an assumption.

mod read;
mod write;
mod xml;

use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use std::path::{Component, Path};

use powerio_core::{ArtifactPath, MemoryArtifact, Source};

use crate::diagnostics::{Diagnostics, codes};
use crate::network::BalancedNetwork;
use crate::{Error, Result};

const MAX_FILES: usize = 4_096;
const MAX_BYTES: u64 = 64 << 20;
const MAX_COMPRESSION_RATIO: u64 = 200;
const CGMES_CLASS_PROPERTY: &str = "cgmes_class";

pub(crate) fn looks_like_profile_set(source: &Source) -> bool {
    if source.is_directory() {
        return source.entry_names().is_ok_and(|entries| {
            entries.iter().any(|entry| match extension(entry.as_str()) {
                Some("zip") => true,
                Some("xml") => source
                    .buffer(entry)
                    .is_ok_and(|buffer| looks_like_cgmes_xml(buffer.content_bytes())),
                _ => false,
            })
        });
    }
    match extension(source.name()) {
        Some("zip") => true,
        Some("xml") => source
            .primary_buffer()
            .is_ok_and(|buffer| looks_like_cgmes_xml(buffer.content_bytes())),
        _ => false,
    }
}

fn looks_like_cgmes_xml(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(16_384)];
    let text = String::from_utf8_lossy(head);
    text.contains("rdf:RDF")
        && (text.contains("CIM-schema-cim16") || text.contains("CIM100#"))
        && text.contains("FullModel")
}

pub(crate) fn parse_source(
    source: &Source,
    diagnostics: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let documents = acquire_documents(source)?;
    let name = Path::new(source.name())
        .file_stem()
        .and_then(|stem| stem.to_str());
    let parsed = read::read_cgmes_documents(documents, name)?;
    for warning in parsed.warnings {
        let code = if warning.contains("assuming") || warning.contains("100 MVA") {
            &codes::READ_CGMES_VALUE_DEFAULTED
        } else if warning.contains("approximat") || warning.contains("mapped to") {
            &codes::READ_CGMES_VALUE_APPROXIMATED
        } else {
            &codes::READ_CGMES_RECORD_UNMAPPED
        };
        diagnostics.push(code, warning);
    }
    Ok(parsed.network)
}

pub(crate) fn parse_text(
    name: &str,
    text: &str,
    diagnostics: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    reject_unsafe_xml(text.as_bytes())?;
    let parsed = read::read_cgmes_documents(vec![(name.to_string(), text.to_string())], None)?;
    for warning in parsed.warnings {
        diagnostics.push(&codes::READ_CGMES_RECORD_UNMAPPED, warning);
    }
    Ok(parsed.network)
}

fn acquire_documents(source: &Source) -> Result<Vec<(String, String)>> {
    let mut documents = Vec::new();
    let mut names = BTreeSet::new();
    let mut total = 0_u64;
    if source.is_directory() {
        for name in source.entry_names().map_err(|error| source_error(&error))? {
            match extension(name.as_str()) {
                Some("xml") => {
                    let buffer = source.buffer(&name).map_err(|error| source_error(&error))?;
                    push_xml(
                        name.as_str(),
                        name.as_str(),
                        buffer.content_bytes(),
                        &mut documents,
                        &mut names,
                        &mut total,
                    )?;
                }
                Some("zip") => {
                    let buffer = source.buffer(&name).map_err(|error| source_error(&error))?;
                    push_zip(
                        name.as_str(),
                        buffer.content_bytes(),
                        &mut documents,
                        &mut names,
                        &mut total,
                    )?;
                }
                _ => {}
            }
        }
    } else {
        let buffer = source
            .primary_buffer()
            .map_err(|error| source_error(&error))?;
        if extension(source.name()) == Some("zip")
            || buffer.content_bytes().starts_with(b"PK\x03\x04")
        {
            push_zip(
                source.name(),
                buffer.content_bytes(),
                &mut documents,
                &mut names,
                &mut total,
            )?;
        } else {
            push_xml(
                source.name(),
                source.name(),
                buffer.content_bytes(),
                &mut documents,
                &mut names,
                &mut total,
            )?;
        }
    }
    if documents.is_empty() {
        return Err(format_error(
            "the source contains no CGMES XML profile documents",
        ));
    }
    Ok(documents)
}

fn push_zip(
    archive_name: &str,
    bytes: &[u8],
    documents: &mut Vec<(String, String)>,
    names: &mut BTreeSet<String>,
    total: &mut u64,
) -> Result<()> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format_error(format!(
            "archive {archive_name} exceeds the {MAX_BYTES} byte input limit"
        )));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format_error(format!("cannot read archive {archive_name}: {error}")))?;
    if archive.len() > MAX_FILES {
        return Err(format_error(format!(
            "archive {archive_name} contains more than {MAX_FILES} entries"
        )));
    }
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| {
            format_error(format!(
                "cannot read entry {index} from {archive_name}: {error}"
            ))
        })?;
        if file.is_dir() {
            continue;
        }
        let raw_name = file.name().to_string();
        let path = strict_archive_path(&raw_name)?;
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            return Err(format_error(format!(
                "archive {archive_name} contains symbolic link {raw_name}"
            )));
        }
        let ext = extension(path.as_str());
        if ext == Some("zip") {
            return Err(format_error(format!(
                "archive {archive_name} contains nested archive {raw_name}"
            )));
        }
        if ext != Some("xml") {
            continue;
        }
        let size = file.size();
        let compressed = file.compressed_size();
        if size > MAX_BYTES
            || (size > 0 && (compressed == 0 || size / compressed.max(1) > MAX_COMPRESSION_RATIO))
        {
            return Err(format_error(format!(
                "archive entry {raw_name} exceeds the CGMES decompression limits"
            )));
        }
        let remaining = MAX_BYTES.saturating_sub(*total);
        if size > remaining {
            return Err(format_error(
                "CGMES profile data exceeds the 64 MiB input limit",
            ));
        }
        let mut content = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        file.by_ref()
            .take(remaining + 1)
            .read_to_end(&mut content)
            .map_err(|error| format_error(format!("cannot decompress {raw_name}: {error}")))?;
        push_xml(
            &format!("{archive_name}/{raw_name}"),
            path.as_str(),
            &content,
            documents,
            names,
            total,
        )?;
    }
    Ok(())
}

fn push_xml(
    name: &str,
    normalized_name: &str,
    bytes: &[u8],
    documents: &mut Vec<(String, String)>,
    names: &mut BTreeSet<String>,
    total: &mut u64,
) -> Result<()> {
    if documents.len() >= MAX_FILES {
        return Err(format_error(format!(
            "CGMES profile set contains more than {MAX_FILES} XML documents"
        )));
    }
    let size = bytes.len() as u64;
    *total = total
        .checked_add(size)
        .filter(|sum| *sum <= MAX_BYTES)
        .ok_or_else(|| format_error("CGMES profile data exceeds the 64 MiB input limit"))?;
    reject_unsafe_xml(bytes)?;
    let key = normalized_name.replace('\\', "/").to_ascii_lowercase();
    if !names.insert(key) {
        return Err(format_error(format!(
            "CGMES profile set contains duplicate normalized name {normalized_name}"
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format_error(format!("{name} is not UTF-8 XML: {error}")))?;
    documents.push((name.to_string(), text.to_string()));
    Ok(())
}

fn strict_archive_path(name: &str) -> Result<ArtifactPath> {
    if name.contains('\\') || name.contains('\0') || name.contains(':') {
        return Err(format_error(format!("unsafe archive entry name {name}")));
    }
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format_error(format!("unsafe archive entry name {name}")));
    }
    ArtifactPath::new(name.to_string()).map_err(|error| source_error(&error))
}

fn reject_unsafe_xml(bytes: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    if text.contains("<!doctype") || text.contains("<!entity") {
        return Err(format_error(
            "CGMES XML must not contain a DTD or entity declaration",
        ));
    }
    Ok(())
}

fn extension(name: &str) -> Option<&'static str> {
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())?;
    if extension.eq_ignore_ascii_case("xml") {
        Some("xml")
    } else if extension.eq_ignore_ascii_case("zip") {
        Some("zip")
    } else {
        None
    }
}

fn source_error(error: &powerio_core::Error) -> Error {
    format_error(error.to_string())
}

fn format_error(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: "CGMES",
        message: message.into(),
    }
}

pub(crate) fn artifacts(network: &BalancedNetwork) -> Result<(Vec<MemoryArtifact>, Diagnostics)> {
    let output = write::write_cgmes(network, CgmesVersion::V3_0)?;
    let artifacts = output
        .files
        .into_iter()
        .map(|(name, text)| {
            MemoryArtifact::new(
                ArtifactPath::new(name).expect("CGMES writer emits portable fixed names"),
                text.into_bytes(),
            )
        })
        .collect();
    let mut diagnostics = Diagnostics::new();
    for warning in output.warnings {
        let code = if warning.contains("no reference bus") {
            &codes::EMIT_CGMES.reference_missing
        } else if warning.contains("rating set") || warning.contains("A/B/C") {
            &codes::EMIT_CGMES.rating_set_dropped
        } else if warning.contains("placed in") || warning.contains("base_kv 0") {
            &codes::EMIT_CGMES.value_defaulted
        } else if warning.contains("folded")
            || warning.contains("written as")
            || warning.contains("reparse")
        {
            &codes::EMIT_CGMES.value_collapsed
        } else if warning.contains("field") || warning.contains("cost curve") {
            &codes::EMIT_CGMES.field_dropped
        } else {
            &codes::EMIT_CGMES.record_dropped
        };
        diagnostics.push(code, warning);
    }
    Ok((artifacts, diagnostics))
}

/// The CGMES release family a file set declares, from its `cim` namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CgmesVersion {
    /// CGMES 2.4.15 on CIM16 (`…/CIM-schema-cim16#`, any vintage year).
    V2_4_15,
    /// CGMES 3.0 on CIM100 (`…/CIM100#`).
    V3_0,
}

impl CgmesVersion {
    pub(crate) fn from_namespace(ns: &str) -> Option<Self> {
        if ns.contains("CIM100") {
            Some(CgmesVersion::V3_0)
        } else if ns.contains("CIM-schema-cim16") {
            Some(CgmesVersion::V2_4_15)
        } else {
            None
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            CgmesVersion::V2_4_15 => "CGMES 2.4.15 (CIM16)",
            CgmesVersion::V3_0 => "CGMES 3.0 (CIM100)",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::io::{Cursor, Write};
    use std::sync::Arc;

    use powerio_core::{ComponentId, Destination, EmittedOutput, PioModule, Source};

    use super::*;
    use crate::TargetFormat;
    use crate::network::{
        AcDcConverterControlMode, ActivePowerControl, Branch, Bus, BusBreakerBus, BusId, BusType,
        BusbarSection, ComponentMetadata, ConnectivityNode, CurveStyle, DcBusbar,
        DcConverterOperatingMode, DcConverterUnit, DcGround, DcLine, DcNode, DcPolarity,
        DcSeriesDevice, DcSwitch, DcSwitchKind, DcTerminal, DcTopologicalNode,
        DetailedConnectivity, ExternalIdentifier, Generator, Impedance, LineCommutatedConverter,
        LineCommutatedConverterOperatingMode, LineCommutatedConverterReactiveModel, Load,
        LoadVoltageModel, LoadingLimits, OperationalLimitGroup, ReactiveCapabilityCurve,
        ReactiveCapabilityCurvePoint, ReactiveLimits, Shunt, ShuntBlock, StaticVarCompensator,
        StaticVarCompensatorRegulationMode, Substation, SwitchKind, SwitchedShuntControl,
        SwitchedShuntMode, TapChanger, TapChangerKind, TapChangerRegulationMode, TapChangerStep,
        TemporaryLimit, Terminal, TerminalReference, TopologyEndpoint, TopologyKind,
        TopologySwitch, Transformer3W, VoltageLevel, VoltageSourceConverter, Winding,
    };

    fn component(component_type: &str, local_id: &str) -> ComponentId {
        ComponentId::new(component_type, local_id).unwrap()
    }

    fn network() -> BalancedNetwork {
        let mut network = BalancedNetwork::new("cgmes-test", 100.0);
        let mut first = Bus::new(BusId(1), BusType::Ref, 230.0);
        first.uid = Some("bus-1".into());
        let mut second = Bus::new(BusId(2), BusType::Pq, 230.0);
        second.uid = Some("bus-2".into());
        *network.buses_mut() = vec![first, second];
        network.loads_mut().push(Load::new(BusId(2), 20.0, 5.0));
        let mut generator = Generator::new(BusId(1));
        generator.pg = 20.0;
        generator.pmax = 100.0;
        generator.qmin = -50.0;
        generator.qmax = 50.0;
        network.generators_mut().push(generator);
        network
            .branches_mut()
            .push(Branch::new(BusId(1), BusId(2), 0.01, 0.1));
        network.assign_missing_component_ids();
        network
    }

    fn append_vs_converter_records(
        records: &mut String,
        id_prefix: &str,
        dc_unit_id: &str,
        rated_voltages_kv: &[f64],
    ) {
        for (index, value) in rated_voltages_kv.iter().enumerate() {
            let _ = write!(
                records,
                r##"  <cim:VsConverter rdf:ID="_{id_prefix}converter-{index}">
    <cim:Equipment.EquipmentContainer rdf:resource="#{dc_unit_id}"/>
    <cim:ACDCConverter.ratedUdc>{value}</cim:ACDCConverter.ratedUdc>
  </cim:VsConverter>
"##
            );
        }
    }

    fn insert_profile_records(
        documents: Vec<(String, String)>,
        equipment: &str,
        topology: Option<&str>,
        steady_state_hypothesis: Option<&str>,
    ) -> Vec<(String, String)> {
        documents
            .into_iter()
            .map(|(name, text)| {
                let records = if name.ends_with("_EQ.xml") {
                    Some(equipment)
                } else if name.ends_with("_TP.xml") {
                    topology
                } else if name.ends_with("_SSH.xml") {
                    steady_state_hypothesis
                } else {
                    None
                };
                match records {
                    Some(records) => (
                        name,
                        text.replace("</rdf:RDF>", &format!("{records}</rdf:RDF>")),
                    ),
                    None => (name, text),
                }
            })
            .collect()
    }

    #[allow(clippy::too_many_lines)] // the fixture names every linked topology table explicitly
    fn detailed_network() -> BalancedNetwork {
        let mut network = network();
        let substation = component("substation", "sub-A");
        let voltage_level = component("voltage_level", "vl-A");
        let first_node = component("connectivity_node", "node-A");
        let second_node = component("connectivity_node", "node-B");
        let first_bus = component("bus", "tn-A");
        let second_bus = component("bus", "tn-B");
        let busbar = component("busbar_section", "bbs-A");
        let switch = component("switch", "breaker-A");
        let load = component("load", network.loads()[0].uid.as_deref().unwrap());
        let generator = component("generator", network.generators()[0].uid.as_deref().unwrap());
        let branch = component("branch", network.branches()[0].uid.as_deref().unwrap());
        let terminal =
            |equipment: ComponentId, number: u8, bus: ComponentId, node: ComponentId| Terminal {
                equipment,
                terminal: number,
                voltage_level: voltage_level.clone(),
                bus: Some(bus.clone()),
                connectable_bus: Some(bus),
                node: Some(node),
                connected: true,
                active_power_mw: None,
                reactive_power_mvar: None,
            };
        let named_components = [
            (&substation, "North substation"),
            (&voltage_level, "North 230 kV"),
            (&first_node, "North node A"),
            (&second_node, "North node B"),
            (&first_bus, "North bus A"),
            (&second_bus, "North bus B"),
            (&busbar, "North busbar"),
            (&switch, "North breaker"),
        ];
        let detailed = DetailedConnectivity {
            component_metadata: named_components
                .into_iter()
                .map(|(component, name)| ComponentMetadata {
                    component: component.clone(),
                    name: Some(name.into()),
                    aliases: Vec::new(),
                    external_identifiers: Vec::new(),
                    properties: std::collections::BTreeMap::new(),
                    fictitious: false,
                })
                .collect(),
            subnetworks: Vec::new(),
            substations: vec![Substation {
                component: substation.clone(),
                country: None,
                operator: None,
                geographical_tags: Vec::new(),
            }],
            voltage_levels: vec![VoltageLevel {
                component: voltage_level.clone(),
                substation: Some(substation),
                nominal_kv: 230.0,
                low_voltage_limit_kv: Some(210.0),
                high_voltage_limit_kv: Some(250.0),
                topology_kind: TopologyKind::NodeBreaker,
                buses: vec![BusId(1), BusId(2)],
            }],
            bus_breaker_buses: vec![
                BusBreakerBus {
                    component: first_bus.clone(),
                    voltage_level: voltage_level.clone(),
                    calculated_bus: Some(BusId(1)),
                    voltage_kv: None,
                    angle_degrees: None,
                },
                BusBreakerBus {
                    component: second_bus.clone(),
                    voltage_level: voltage_level.clone(),
                    calculated_bus: Some(BusId(2)),
                    voltage_kv: None,
                    angle_degrees: None,
                },
            ],
            calculated_buses: Vec::new(),
            connectivity_nodes: vec![
                ConnectivityNode {
                    component: first_node.clone(),
                    voltage_level: voltage_level.clone(),
                    node_number: None,
                    calculated_bus: Some(BusId(1)),
                },
                ConnectivityNode {
                    component: second_node.clone(),
                    voltage_level: voltage_level.clone(),
                    node_number: None,
                    calculated_bus: Some(BusId(2)),
                },
            ],
            busbar_sections: vec![BusbarSection {
                component: busbar.clone(),
                voltage_level: voltage_level.clone(),
                node: first_node.clone(),
            }],
            terminals: vec![
                terminal(busbar, 1, first_bus.clone(), first_node.clone()),
                terminal(load, 1, second_bus.clone(), second_node.clone()),
                terminal(generator, 1, first_bus.clone(), first_node.clone()),
                terminal(branch.clone(), 1, first_bus.clone(), first_node.clone()),
                terminal(branch, 2, second_bus.clone(), second_node.clone()),
                terminal(switch.clone(), 1, first_bus.clone(), first_node.clone()),
                terminal(switch.clone(), 2, second_bus.clone(), second_node.clone()),
            ],
            switches: vec![TopologySwitch {
                component: switch,
                voltage_level,
                kind: SwitchKind::Breaker,
                endpoint1: TopologyEndpoint::Node(first_node),
                endpoint2: TopologyEndpoint::Node(second_node),
                open: true,
                retained: true,
            }],
            internal_connections: Vec::new(),
            operational_limit_groups: Vec::new(),
            tap_changers: Vec::new(),
            equipment_reactive_limits: Vec::new(),
            boundary_lines: Vec::new(),
            tie_lines: Vec::new(),
            dc_converter_units: Vec::new(),
            dc_topological_nodes: Vec::new(),
            dc_nodes: Vec::new(),
            dc_grounds: Vec::new(),
            dc_lines: Vec::new(),
            dc_switches: Vec::new(),
            dc_busbars: Vec::new(),
            dc_series_devices: Vec::new(),
            voltage_source_converters: Vec::new(),
            line_commutated_converters: Vec::new(),
        };
        *network.detailed_connectivity_mut() = Some(Arc::new(detailed));
        network
    }

    fn dc_ground_documents(
        version: CgmesVersion,
        converter_rated_voltages_kv: &[f64],
        ground_rated_voltage_kv: Option<f64>,
    ) -> Vec<(String, String)> {
        let namespace = match version {
            CgmesVersion::V2_4_15 => "http://iec.ch/TC57/2013/CIM-schema-cim16#",
            CgmesVersion::V3_0 => "http://iec.ch/TC57/CIM100#",
        };
        let mut converters = String::new();
        append_vs_converter_records(&mut converters, "", "_dc-unit", converter_rated_voltages_kv);
        let ground_voltage = ground_rated_voltage_kv.map_or_else(String::new, |value| {
            format!(
                "    <cim:DCConductingEquipment.ratedUdc>{value}</cim:DCConductingEquipment.ratedUdc>\n"
            )
        });
        let records = format!(
            r##"  <cim:DCConverterUnit rdf:ID="_dc-unit">
    <cim:DCConverterUnit.operationMode rdf:resource="{namespace}DCConverterOperatingModeKind.bipolar"/>
  </cim:DCConverterUnit>
  <cim:DCNode rdf:ID="_dc-node">
    <cim:DCNode.DCEquipmentContainer rdf:resource="#_dc-unit"/>
  </cim:DCNode>
  <cim:DCGround rdf:ID="_dc-ground">
    <cim:Equipment.EquipmentContainer rdf:resource="#_dc-unit"/>
{ground_voltage}  </cim:DCGround>
  <cim:DCTerminal rdf:ID="_dc-ground-terminal">
    <cim:DCTerminal.DCConductingEquipment rdf:resource="#_dc-ground"/>
    <cim:DCBaseTerminal.DCNode rdf:resource="#_dc-node"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:DCTerminal>
{converters}"##
        );
        let documents = write::write_cgmes(&network(), version).unwrap().files;
        insert_profile_records(documents, &records, None, None)
    }

    fn dc_line_documents(
        version: CgmesVersion,
        first_unit_converter_voltages_kv: &[f64],
        second_unit_converter_voltages_kv: &[f64],
        line_rated_voltage_kv: Option<f64>,
    ) -> Vec<(String, String)> {
        let namespace = match version {
            CgmesVersion::V2_4_15 => "http://iec.ch/TC57/2013/CIM-schema-cim16#",
            CgmesVersion::V3_0 => "http://iec.ch/TC57/CIM100#",
        };
        let mut converters = String::new();
        append_vs_converter_records(
            &mut converters,
            "first-",
            "_first-dc-unit",
            first_unit_converter_voltages_kv,
        );
        append_vs_converter_records(
            &mut converters,
            "second-",
            "_second-dc-unit",
            second_unit_converter_voltages_kv,
        );
        let line_voltage = line_rated_voltage_kv.map_or_else(String::new, |value| {
            format!(
                "    <cim:DCConductingEquipment.ratedUdc>{value}</cim:DCConductingEquipment.ratedUdc>\n"
            )
        });
        let records = format!(
            r##"  <cim:DCConverterUnit rdf:ID="_first-dc-unit">
    <cim:DCConverterUnit.operationMode rdf:resource="{namespace}DCConverterOperatingModeKind.bipolar"/>
  </cim:DCConverterUnit>
  <cim:DCConverterUnit rdf:ID="_second-dc-unit">
    <cim:DCConverterUnit.operationMode rdf:resource="{namespace}DCConverterOperatingModeKind.bipolar"/>
  </cim:DCConverterUnit>
  <cim:DCNode rdf:ID="_first-dc-node">
    <cim:DCNode.DCEquipmentContainer rdf:resource="#_first-dc-unit"/>
  </cim:DCNode>
  <cim:DCNode rdf:ID="_second-dc-node">
    <cim:DCNode.DCEquipmentContainer rdf:resource="#_second-dc-unit"/>
  </cim:DCNode>
  <cim:DCLine rdf:ID="_dc-line-container">
    <cim:IdentifiedObject.name>DC line container</cim:IdentifiedObject.name>
  </cim:DCLine>
  <cim:DCLineSegment rdf:ID="_dc-line">
    <cim:Equipment.EquipmentContainer rdf:resource="#_dc-line-container"/>
{line_voltage}    <cim:DCLineSegment.resistance>1.25</cim:DCLineSegment.resistance>
    <cim:DCLineSegment.inductance>0.02</cim:DCLineSegment.inductance>
    <cim:DCLineSegment.capacitance>0.000003</cim:DCLineSegment.capacitance>
  </cim:DCLineSegment>
  <cim:DCTerminal rdf:ID="_dc-line-terminal-1">
    <cim:DCTerminal.DCConductingEquipment rdf:resource="#_dc-line"/>
    <cim:DCBaseTerminal.DCNode rdf:resource="#_first-dc-node"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:DCTerminal>
  <cim:DCTerminal rdf:ID="_dc-line-terminal-2">
    <cim:DCTerminal.DCConductingEquipment rdf:resource="#_dc-line"/>
    <cim:DCBaseTerminal.DCNode rdf:resource="#_second-dc-node"/>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
  </cim:DCTerminal>
{converters}"##
        );
        let topology_records = r##"  <cim:DCTopologicalNode rdf:ID="_first-dc-topological-node">
    <cim:DCTopologicalNode.DCEquipmentContainer rdf:resource="#_first-dc-unit"/>
  </cim:DCTopologicalNode>
  <cim:DCTopologicalNode rdf:ID="_second-dc-topological-node">
    <cim:DCTopologicalNode.DCEquipmentContainer rdf:resource="#_second-dc-unit"/>
  </cim:DCTopologicalNode>
  <cim:DCNode rdf:about="#_first-dc-node">
    <cim:DCNode.DCTopologicalNode rdf:resource="#_first-dc-topological-node"/>
  </cim:DCNode>
  <cim:DCNode rdf:about="#_second-dc-node">
    <cim:DCNode.DCTopologicalNode rdf:resource="#_second-dc-topological-node"/>
  </cim:DCNode>
  <cim:DCTerminal rdf:about="#_dc-line-terminal-1">
    <cim:DCBaseTerminal.DCTopologicalNode rdf:resource="#_first-dc-topological-node"/>
  </cim:DCTerminal>
  <cim:DCTerminal rdf:about="#_dc-line-terminal-2">
    <cim:DCBaseTerminal.DCTopologicalNode rdf:resource="#_second-dc-topological-node"/>
  </cim:DCTerminal>
"##;
        let steady_state_records = r##"  <cim:DCTerminal rdf:about="#_dc-line-terminal-1">
    <cim:ACDCTerminal.connected>true</cim:ACDCTerminal.connected>
  </cim:DCTerminal>
  <cim:DCTerminal rdf:about="#_dc-line-terminal-2">
    <cim:ACDCTerminal.connected>true</cim:ACDCTerminal.connected>
  </cim:DCTerminal>
"##;
        let documents = write::write_cgmes(&network(), version).unwrap().files;
        insert_profile_records(
            documents,
            &records,
            Some(topology_records),
            Some(steady_state_records),
        )
    }

    #[test]
    fn fresh_output_is_four_cgmes_3_profiles_and_reparses() {
        let module = PioModule::new(network());
        let result = crate::format::emit(
            &module,
            TargetFormat::Cgmes,
            Destination::memory("cgmes").unwrap(),
        )
        .unwrap();
        let EmittedOutput::Memory { artifacts } = result.into_output() else {
            panic!("memory destination returned paths");
        };
        assert_eq!(artifacts.len(), 4);
        let documents = artifacts
            .iter()
            .map(|artifact| {
                let text = std::str::from_utf8(artifact.bytes()).unwrap();
                assert!(text.contains("http://iec.ch/TC57/CIM100#"));
                (artifact.name().to_string(), text.to_string())
            })
            .collect();
        let parsed = read::read_cgmes_documents(documents, Some("case")).unwrap();
        assert_eq!(parsed.network.buses().len(), 2);
        assert_eq!(parsed.network.branches().len(), 1);
        assert_eq!(parsed.network.loads().len(), 1);
        assert_eq!(parsed.network.generators().len(), 1);
    }

    #[test]
    fn synchronous_machine_emits_the_required_generator_kind() {
        for (version, namespace) in [
            (
                CgmesVersion::V2_4_15,
                "http://iec.ch/TC57/2013/CIM-schema-cim16#",
            ),
            (CgmesVersion::V3_0, "http://iec.ch/TC57/CIM100#"),
        ] {
            let output = write::write_cgmes(&network(), version).unwrap();
            let eq = output
                .files
                .iter()
                .find(|(name, _)| name.ends_with("_EQ.xml"))
                .map(|(_, text)| text)
                .unwrap();
            assert!(eq.contains(&format!(
                "<cim:SynchronousMachine.type rdf:resource=\"{namespace}SynchronousMachineKind.generator\"/>"
            )));
        }
    }

    #[test]
    fn generating_unit_normal_pf_maps_to_active_power_control() {
        let mut network = network();
        let mut control = ActivePowerControl::new(true);
        control.participation_factor = Some(0.35);
        network.generators_mut()[0].active_power_control = Some(control.clone());
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let ssh = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SSH.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(ssh.contains("<cim:GeneratingUnit.normalPF>0.35</cim:GeneratingUnit.normalPF>"));
        assert!(
            !output
                .warnings
                .iter()
                .any(|warning| warning.contains("active power control"))
        );

        let parsed = read::read_cgmes_documents(output.files.clone(), Some("normal-pf")).unwrap();
        assert_eq!(
            parsed.network.generators()[0].active_power_control,
            Some(control)
        );

        let invalid = output
            .files
            .into_iter()
            .map(|(name, xml)| {
                (
                    name,
                    xml.replace(
                        "<cim:GeneratingUnit.normalPF>0.35</cim:GeneratingUnit.normalPF>",
                        "<cim:GeneratingUnit.normalPF>-0.1</cim:GeneratingUnit.normalPF>",
                    ),
                )
            })
            .collect();
        assert!(read::read_cgmes_documents(invalid, Some("invalid-normal-pf")).is_err());

        network.generators_mut()[0]
            .active_power_control
            .as_mut()
            .unwrap()
            .participate = false;
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let ssh = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SSH.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(!ssh.contains("GeneratingUnit.normalPF"));
        assert!(
            output.warnings.iter().any(|warning| {
                warning.contains("participate=false with a participation factor")
            })
        );
    }

    #[test]
    fn source_line_topology_container_remains_a_line() {
        const TOPOLOGICAL_NODE_MRID: &str = "11111111-1111-4111-8111-111111111111";
        const CONNECTIVITY_NODE_MRID: &str = "22222222-2222-4222-8222-222222222222";
        const SOURCE_LINE_MRID: &str = "33333333-3333-4333-8333-333333333333";

        let mut network = detailed_network();
        let source_container = component("line", SOURCE_LINE_MRID);
        let topological_node = component("bus", TOPOLOGICAL_NODE_MRID);
        let connectivity_node = component("connectivity_node", CONNECTIVITY_NODE_MRID);
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let old_topological_node = detailed.bus_breaker_buses[0].component.clone();
        let old_connectivity_node = detailed.connectivity_nodes[0].component.clone();

        detailed.bus_breaker_buses[0].component = topological_node.clone();
        detailed.bus_breaker_buses[0].voltage_level = source_container.clone();
        detailed.connectivity_nodes[0].component = connectivity_node.clone();
        detailed.connectivity_nodes[0].voltage_level = source_container.clone();
        detailed.busbar_sections[0].node = connectivity_node.clone();
        for terminal in &mut detailed.terminals {
            if terminal.bus.as_ref() == Some(&old_topological_node) {
                terminal.bus = Some(topological_node.clone());
            }
            if terminal.connectable_bus.as_ref() == Some(&old_topological_node) {
                terminal.connectable_bus = Some(topological_node.clone());
            }
            if terminal.node.as_ref() == Some(&old_connectivity_node) {
                terminal.node = Some(connectivity_node.clone());
                terminal.voltage_level = source_container.clone();
            }
        }
        if let TopologyEndpoint::Node(node) = &mut detailed.switches[0].endpoint1
            && *node == old_connectivity_node
        {
            *node = connectivity_node.clone();
        }
        for (component, mrid, name) in [
            (
                source_container.clone(),
                SOURCE_LINE_MRID,
                "Boundary line container",
            ),
            (
                topological_node.clone(),
                TOPOLOGICAL_NODE_MRID,
                "Boundary topological node",
            ),
            (
                connectivity_node.clone(),
                CONNECTIVITY_NODE_MRID,
                "Boundary connectivity node",
            ),
        ] {
            detailed.component_metadata.push(ComponentMetadata {
                component,
                name: Some(name.into()),
                aliases: Vec::new(),
                external_identifiers: vec![ExternalIdentifier {
                    value: mrid.into(),
                    authority: Some("CGMES".into()),
                }],
                properties: std::collections::BTreeMap::new(),
                fictitious: false,
            });
        }

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(!output.warnings.iter().any(|warning| {
            warning.contains(&source_container.to_string())
                && warning.contains("generated VoltageLevel")
        }));

        let all_xml = output
            .files
            .iter()
            .map(|(_, xml)| xml.as_str())
            .collect::<String>();
        assert!(all_xml.contains(&format!("rdf:ID=\"_{TOPOLOGICAL_NODE_MRID}\"")));
        assert!(all_xml.contains(&format!("rdf:ID=\"_{CONNECTIVITY_NODE_MRID}\"")));
        assert!(all_xml.contains(&format!("<cim:Line rdf:ID=\"_{SOURCE_LINE_MRID}\"")));
        assert!(all_xml.contains(&format!("rdf:resource=\"#_{SOURCE_LINE_MRID}\"")));

        let parsed = read::read_cgmes_documents(output.files, Some("boundary-line")).unwrap();
        let identifiers = parsed
            .network
            .detailed_connectivity()
            .as_deref()
            .unwrap()
            .component_metadata
            .iter()
            .flat_map(|metadata| &metadata.external_identifiers)
            .map(|identifier| identifier.value.as_str())
            .collect::<Vec<_>>();
        assert!(identifiers.contains(&TOPOLOGICAL_NODE_MRID));
        assert!(identifiers.contains(&CONNECTIVITY_NODE_MRID));
        assert!(identifiers.contains(&SOURCE_LINE_MRID));
        let parsed_detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert!(parsed_detailed.connectivity_nodes.iter().any(|node| {
            node.component.local_id() == CONNECTIVITY_NODE_MRID
                && node.voltage_level.component_type() == "line"
                && node.voltage_level.local_id() == SOURCE_LINE_MRID
        }));
    }

    #[test]
    fn unrelated_boundary_connectivity_node_and_line_are_omitted() {
        const CONNECTIVITY_NODE_MRID: &str = "44444444-4444-4444-8444-444444444444";
        const SOURCE_LINE_MRID: &str = "55555555-5555-4555-8555-555555555555";

        let mut network = detailed_network();
        let source_line = component("line", SOURCE_LINE_MRID);
        let connectivity_node = component("connectivity_node", CONNECTIVITY_NODE_MRID);
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.connectivity_nodes.push(ConnectivityNode {
            component: connectivity_node.clone(),
            voltage_level: source_line.clone(),
            node_number: None,
            calculated_bus: None,
        });
        for (component, name) in [
            (source_line.clone(), "Unrelated boundary line"),
            (connectivity_node.clone(), "Unrelated boundary node"),
        ] {
            detailed.component_metadata.push(ComponentMetadata {
                external_identifiers: vec![ExternalIdentifier {
                    value: component.local_id().into(),
                    authority: Some("CGMES".into()),
                }],
                component,
                name: Some(name.into()),
                aliases: Vec::new(),
                properties: std::collections::BTreeMap::new(),
                fictitious: false,
            });
        }

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(output.warnings.iter().any(|warning| {
            warning.contains(&connectivity_node.to_string())
                && warning.contains(&source_line.to_string())
                && warning.contains("not connected to a calculated bus or retained equipment")
                && warning.contains("omitted from fresh CGMES emission")
        }));
        let all_xml = output
            .files
            .iter()
            .map(|(_, xml)| xml.as_str())
            .collect::<String>();
        assert!(!all_xml.contains(CONNECTIVITY_NODE_MRID));
        assert!(!all_xml.contains(SOURCE_LINE_MRID));
    }

    #[test]
    fn emitted_terminal_without_node_number_has_a_matching_connectivity_node() {
        const UNRELATED_NODE_MRID: &str = "66666666-6666-4666-8666-666666666666";
        const UNRELATED_LINE_MRID: &str = "77777777-7777-4777-8777-777777777777";

        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let load_terminal = detailed
            .terminals
            .iter_mut()
            .find(|terminal| terminal.equipment.component_type() == "load")
            .unwrap();
        assert_eq!(
            detailed
                .voltage_levels
                .iter()
                .find(|level| level.component == load_terminal.voltage_level)
                .unwrap()
                .topology_kind,
            TopologyKind::NodeBreaker
        );
        load_terminal.node = None;

        let unrelated_line = component("line", UNRELATED_LINE_MRID);
        let unrelated_node = component("connectivity_node", UNRELATED_NODE_MRID);
        detailed.connectivity_nodes.push(ConnectivityNode {
            component: unrelated_node.clone(),
            voltage_level: unrelated_line.clone(),
            node_number: None,
            calculated_bus: None,
        });
        for (component, name) in [
            (unrelated_line, "Unrelated boundary line"),
            (unrelated_node, "Unrelated boundary node"),
        ] {
            detailed.component_metadata.push(ComponentMetadata {
                external_identifiers: vec![ExternalIdentifier {
                    value: component.local_id().into(),
                    authority: Some("CGMES".into()),
                }],
                component,
                name: Some(name.into()),
                aliases: Vec::new(),
                properties: std::collections::BTreeMap::new(),
                fictitious: false,
            });
        }

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let all_xml = output
            .files
            .iter()
            .map(|(_, xml)| xml.as_str())
            .collect::<String>();
        assert!(!all_xml.contains(UNRELATED_NODE_MRID));
        assert!(!all_xml.contains(UNRELATED_LINE_MRID));

        let reparsed =
            read::read_cgmes_documents(output.files, Some("terminal-connectivity")).unwrap();
        let detailed = reparsed.network.detailed_connectivity().as_deref().unwrap();
        let load_terminal = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.component_type() == "load")
            .unwrap();
        let emitted_node = load_terminal.node.as_ref().unwrap();
        assert!(
            detailed
                .connectivity_nodes
                .iter()
                .any(|node| node.component == *emitted_node)
        );
    }

    #[test]
    fn emission_rejects_unknown_voltage_bases_instead_of_inventing_one_kv() {
        let mut invalid_bus = network();
        invalid_bus.buses_mut()[0].base_kv = 0.0;
        let error = write::write_cgmes(&invalid_bus, CgmesVersion::V3_0).unwrap_err();
        assert!(error.to_string().contains("bus 1"));
        assert!(error.to_string().contains("base_kv 0"));
        assert!(
            error
                .to_string()
                .contains("requires an exact positive voltage base")
        );

        let mut missing_topological_node_base = detailed_network();
        let detailed = Arc::make_mut(
            missing_topological_node_base
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        let topological_node = detailed.bus_breaker_buses[0].component.clone();
        let source_container = component("voltage_level", "untyped-topology-container");
        detailed.bus_breaker_buses[0].voltage_level = source_container.clone();
        detailed.bus_breaker_buses[0].calculated_bus = None;
        let error =
            write::write_cgmes(&missing_topological_node_base, CgmesVersion::V3_0).unwrap_err();
        let message = error.to_string();
        assert!(message.contains(&source_container.to_string()));
        assert!(message.contains("TopologicalNode"));
        assert!(message.contains(&topological_node.to_string()));
        assert!(message.contains("has no calculated bus"));

        let mut missing_connectivity_node_base = detailed_network();
        let detailed = Arc::make_mut(
            missing_connectivity_node_base
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        let connectivity_node = detailed.connectivity_nodes[0].component.clone();
        let source_container = component("voltage_level", "untyped-connectivity-container");
        detailed.connectivity_nodes[0].voltage_level = source_container.clone();
        detailed.connectivity_nodes[0].calculated_bus = None;
        let error =
            write::write_cgmes(&missing_connectivity_node_base, CgmesVersion::V3_0).unwrap_err();
        let message = error.to_string();
        assert!(message.contains(&source_container.to_string()));
        assert!(message.contains("ConnectivityNode"));
        assert!(message.contains(&connectivity_node.to_string()));
        assert!(message.contains("has no calculated bus"));
    }

    #[test]
    fn three_winding_transformer_round_trips_without_star_lowering() {
        let mut network = network();
        let mut third = Bus::new(BusId(3), BusType::Pq, 115.0);
        third.uid = Some("bus-3".into());
        network.buses_mut().push(third);

        let mut winding1 = Winding::new(BusId(1));
        winding1.nominal_kv = 230.0;
        winding1.tap = 1.03;
        winding1.shift = 2.5;
        winding1.rate_a = 120.0;
        winding1.rate_b = 135.0;
        winding1.rate_c = 150.0;
        let mut winding2 = Winding::new(BusId(2));
        winding2.nominal_kv = 230.0;
        winding2.rate_a = 100.0;
        let mut winding3 = Winding::new(BusId(3));
        winding3.nominal_kv = 115.0;
        winding3.rate_a = 80.0;
        let mut transformer = Transformer3W::new(
            [winding1, winding2, winding3],
            [
                Impedance::new(0.01, 0.10, 100.0),
                Impedance::new(0.02, 0.20, 100.0),
                Impedance::new(0.03, 0.30, 100.0),
            ],
        );
        transformer.mag_g = 0.001;
        transformer.mag_b = -0.01;
        transformer.name = Some("three winding transformer".into());
        transformer.uid = Some("three-winding-1".into());
        network.transformers_3w_mut().push(transformer);

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(
            output
                .files
                .iter()
                .find(|(name, _)| name.ends_with("_EQ.xml"))
                .is_some_and(|(_, text)| text.matches("<cim:PowerTransformerEnd").count() >= 3)
        );
        let parsed = read::read_cgmes_documents(output.files, Some("three-winding")).unwrap();
        assert_eq!(parsed.network.transformers_3w().len(), 1);
        let parsed = &parsed.network.transformers_3w()[0];
        assert_eq!(parsed.windings[2].bus, BusId(3));
        assert!((parsed.windings[0].tap - 1.03).abs() < 1e-10);
        assert!((parsed.windings[0].shift - 2.5).abs() < 1e-10);
        assert!((parsed.windings[0].rate_a - 120.0).abs() < 1e-10);
        assert!((parsed.windings[0].rate_b - 135.0).abs() < 1e-10);
        assert!((parsed.windings[0].rate_c - 150.0).abs() < 1e-10);
        for (actual, expected) in parsed.z.iter().zip([0.10, 0.20, 0.30]) {
            assert!((actual.x - expected).abs() < 1e-10);
        }
        assert!((parsed.mag_g - 0.001).abs() < 1e-10);
        assert!((parsed.mag_b + 0.01).abs() < 1e-10);
    }

    #[test]
    fn every_transformer_end_emits_zero_susceptance_when_it_has_none() {
        let mut network = network();
        network.branches_mut()[0].tap = 1.05;

        let mut third = Bus::new(BusId(3), BusType::Pq, 115.0);
        third.uid = Some("bus-3".into());
        network.buses_mut().push(third);
        let mut winding1 = Winding::new(BusId(1));
        winding1.nominal_kv = 230.0;
        let mut winding2 = Winding::new(BusId(2));
        winding2.nominal_kv = 230.0;
        let mut winding3 = Winding::new(BusId(3));
        winding3.nominal_kv = 115.0;
        network.transformers_3w_mut().push(Transformer3W::new(
            [winding1, winding2, winding3],
            [
                Impedance::new(0.01, 0.10, 100.0),
                Impedance::new(0.02, 0.20, 100.0),
                Impedance::new(0.03, 0.30, 100.0),
            ],
        ));

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert_eq!(eq.matches("<cim:PowerTransformerEnd ").count(), 5);
        assert_eq!(
            eq.matches("<cim:PowerTransformerEnd.b>0</cim:PowerTransformerEnd.b>")
                .count(),
            5
        );

        let reparsed = read::read_cgmes_documents(output.files, Some("zero-end-b")).unwrap();
        assert_eq!(
            reparsed
                .network
                .branches()
                .iter()
                .filter(|branch| branch.is_transformer())
                .count(),
            1
        );
        assert_eq!(reparsed.network.transformers_3w().len(), 1);
    }

    #[test]
    fn nonlinear_shunt_sections_round_trip_with_conductance() {
        let mut network = network();
        let mut shunt = Shunt::new(BusId(2), 0.3, 5.0);
        shunt.uid = Some("nonlinear-shunt-1".into());
        shunt.control = Some(SwitchedShuntControl {
            mode: SwitchedShuntMode::Discrete,
            vhigh: 1.04,
            vlow: 0.96,
            control_bus: None,
            regulating_terminal: None,
            rmpct: 100.0,
            blocks: vec![
                ShuntBlock::with_admittance(1, 0.1, 2.0),
                ShuntBlock::with_admittance(1, 0.2, 3.0),
                ShuntBlock::with_admittance(1, 0.4, 4.0),
            ],
        });
        network.shunts_mut().push(shunt);

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(eq.contains("<cim:NonlinearShuntCompensator "));
        assert_eq!(
            eq.matches("<cim:NonlinearShuntCompensatorPoint ").count(),
            3
        );

        let parsed = read::read_cgmes_documents(output.files, Some("nonlinear-shunt")).unwrap();
        let shunt = &parsed.network.shunts()[0];
        assert!((shunt.g - 0.3).abs() < 1e-10);
        assert!((shunt.b - 5.0).abs() < 1e-10);
        assert!(shunt.in_service);
        let control = shunt.control.as_ref().unwrap();
        assert_eq!(control.mode, SwitchedShuntMode::Discrete);
        assert_eq!(control.blocks.len(), 3);
        assert!((control.blocks[2].g - 0.4).abs() < 1e-10);
        assert!((control.blocks[2].b - 4.0).abs() < 1e-10);
    }

    #[test]
    fn load_response_characteristics_round_trip() {
        let mut network = network();
        network.loads_mut()[0].voltage_model = Some(LoadVoltageModel::Zip {
            p_constant_power: 10.0,
            q_constant_power: 2.5,
            p_constant_current: 4.0,
            q_constant_current: 1.0,
            p_constant_impedance: 6.0,
            q_constant_impedance: 1.5,
            v_nom: None,
            load_type: None,
            scaling: None,
        });
        let mut exponential = Load::new(BusId(2), 8.0, 3.0);
        exponential.uid = Some("9db15afd-90c9-5aad-b2a0-0e0ac3a64772".into());
        exponential.voltage_model = Some(LoadVoltageModel::Exponential {
            p: 8.0,
            q: 3.0,
            v_nom: None,
            gamma_p: 1.2,
            gamma_q: 2.1,
        });
        network.loads_mut().push(exponential);

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(eq.contains("<cim:EnergyConsumer.LoadResponse "));
        assert!(eq.contains("<cim:LoadResponseCharacteristic.exponentModel>"));

        let parsed = read::read_cgmes_documents(output.files, Some("load-response")).unwrap();
        let zip = parsed
            .network
            .loads()
            .iter()
            .find_map(|load| match &load.voltage_model {
                Some(LoadVoltageModel::Zip {
                    p_constant_power,
                    q_constant_power,
                    p_constant_current,
                    q_constant_current,
                    p_constant_impedance,
                    q_constant_impedance,
                    ..
                }) => Some([
                    *p_constant_power,
                    *q_constant_power,
                    *p_constant_current,
                    *q_constant_current,
                    *p_constant_impedance,
                    *q_constant_impedance,
                ]),
                _ => None,
            })
            .unwrap();
        for (actual, expected) in zip.into_iter().zip([10.0, 2.5, 4.0, 1.0, 6.0, 1.5]) {
            assert!((actual - expected).abs() < 1e-12);
        }
        let exponential = parsed
            .network
            .loads()
            .iter()
            .find_map(|load| match &load.voltage_model {
                Some(LoadVoltageModel::Exponential {
                    gamma_p, gamma_q, ..
                }) => Some((*gamma_p, *gamma_q)),
                _ => None,
            })
            .unwrap();
        assert!((exponential.0 - 1.2).abs() < 1e-12);
        assert!((exponential.1 - 2.1).abs() < 1e-12);
    }

    #[test]
    fn detailed_hierarchy_and_connectivity_round_trip() {
        let output = write::write_cgmes(&detailed_network(), CgmesVersion::V3_0).unwrap();
        let parsed = read::read_cgmes_documents(output.files, Some("detailed")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.substations.len(), 1);
        assert_eq!(detailed.voltage_levels.len(), 1);
        assert_eq!(detailed.connectivity_nodes.len(), 2);
        assert_eq!(detailed.busbar_sections.len(), 1);
        assert_eq!(detailed.switches.len(), 1);
        assert!(
            detailed
                .component_metadata
                .iter()
                .any(|metadata| { metadata.name.as_deref() == Some("North substation") })
        );
    }

    #[test]
    fn series_compensator_terminals_survive_fresh_emission() {
        let documents = write::write_cgmes(&detailed_network(), CgmesVersion::V3_0)
            .unwrap()
            .files
            .into_iter()
            .map(|(name, text)| {
                let mut text = text
                    .replace("ACLineSegment", "SeriesCompensator")
                    .replace(
                        "    <cim:SeriesCompensator.bch>0</cim:SeriesCompensator.bch>\n",
                        "",
                    );
                if name.ends_with("_EQ.xml") {
                    text = text.replace(
                        "</cim:SeriesCompensator>",
                        "    <cim:SeriesCompensator.r0>0.2</cim:SeriesCompensator.r0>\n\
                         <cim:SeriesCompensator.x0>0.3</cim:SeriesCompensator.x0>\n\
                         <cim:SeriesCompensator.varistorPresent>true</cim:SeriesCompensator.varistorPresent>\n\
                         <cim:SeriesCompensator.varistorRatedCurrent>500</cim:SeriesCompensator.varistorRatedCurrent>\n\
                         <cim:SeriesCompensator.varistorVoltageThreshold>250</cim:SeriesCompensator.varistorVoltageThreshold>\n\
                         </cim:SeriesCompensator>",
                    );
                }
                (name, text)
            })
            .collect();

        let parsed = read::read_cgmes_documents(documents, Some("series-compensator")).unwrap();
        assert!(
            parsed
                .warnings
                .iter()
                .all(|warning| !warning.contains("SeriesCompensator")),
            "SeriesCompensator is mapped electrical equipment"
        );
        let branch_terminal_nodes = |network: &BalancedNetwork| {
            let branch = network.branches().first().unwrap();
            let branch = component("branch", branch.uid.as_deref().unwrap());
            let detailed = network.detailed_connectivity().as_deref().unwrap();
            let terminals = detailed
                .terminals
                .iter()
                .filter(|terminal| terminal.equipment == branch)
                .collect::<Vec<_>>();
            assert_eq!(terminals.len(), 2);
            [
                terminals[0].node.clone().unwrap(),
                terminals[1].node.clone().unwrap(),
            ]
        };
        let source_nodes = branch_terminal_nodes(&parsed.network);
        assert_ne!(source_nodes[0], source_nodes[1]);

        let fresh = write::write_cgmes(&parsed.network, CgmesVersion::V3_0).unwrap();
        assert!(
            fresh
                .warnings
                .iter()
                .all(|warning| !warning.contains("SeriesCompensator")),
            "a retained SeriesCompensator should emit without projection"
        );
        let eq = fresh
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(eq.contains("<cim:SeriesCompensator "));
        assert!(!eq.contains("<cim:ACLineSegment "));
        for field in [
            "SeriesCompensator.r0",
            "SeriesCompensator.x0",
            "SeriesCompensator.varistorPresent",
            "SeriesCompensator.varistorRatedCurrent",
            "SeriesCompensator.varistorVoltageThreshold",
        ] {
            assert!(eq.contains(&format!("<cim:{field}>")), "missing `{field}`");
        }
        let reparsed = read::read_cgmes_documents(fresh.files, Some("fresh")).unwrap();
        assert_eq!(branch_terminal_nodes(&reparsed.network), source_nodes);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // the fixture names every linked DC equipment table explicitly
    fn physical_dc_equipment_round_trips_through_official_cgmes_classes() {
        const DC_LINE_CONTAINER_MRID: &str = "44444444-4444-4444-8444-444444444444";
        const DC_BUSBAR_MRID: &str = "55555555-5555-4555-8555-555555555555";
        const DC_SERIES_DEVICE_MRID: &str = "66666666-6666-4666-8666-666666666666";

        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let converter_unit = component("dc_converter_unit", "dc-unit-1");
        let dc_line_container = component("dc_line_container", "dc-line-container-1");
        let dc_busbar = component("dc_busbar", "dc-busbar-1");
        let dc_series_device = component("dc_series_device", "dc-series-device-1");
        let first_dc_node = component("dc_node", "dc-node-1");
        let second_dc_node = component("dc_node", "dc-node-2");
        let third_dc_node = component("dc_node", "dc-node-3");
        let first_dc_topological_node = component("dc_topological_node", "dc-tn-1");
        let second_dc_topological_node = component("dc_topological_node", "dc-tn-2");
        let third_dc_topological_node = component("dc_topological_node", "dc-tn-3");
        for (component, mrid, name) in [
            (
                dc_line_container.clone(),
                DC_LINE_CONTAINER_MRID,
                "Retained DC line container",
            ),
            (dc_busbar.clone(), DC_BUSBAR_MRID, "North DC busbar"),
            (
                dc_series_device.clone(),
                DC_SERIES_DEVICE_MRID,
                "North DC series device",
            ),
        ] {
            detailed.component_metadata.push(ComponentMetadata {
                component,
                name: Some(name.into()),
                aliases: Vec::new(),
                external_identifiers: vec![ExternalIdentifier {
                    value: mrid.into(),
                    authority: Some("CGMES".into()),
                }],
                properties: std::collections::BTreeMap::new(),
                fictitious: false,
            });
        }
        detailed.dc_converter_units.push(DcConverterUnit {
            component: converter_unit.clone(),
            substation: Some(component("substation", "sub-A")),
            operation_mode: DcConverterOperatingMode::Bipolar,
        });
        detailed.dc_topological_nodes = vec![
            DcTopologicalNode {
                component: first_dc_topological_node.clone(),
                dc_converter_unit: Some(converter_unit.clone()),
            },
            DcTopologicalNode {
                component: second_dc_topological_node.clone(),
                dc_converter_unit: Some(converter_unit.clone()),
            },
            DcTopologicalNode {
                component: third_dc_topological_node.clone(),
                dc_converter_unit: Some(converter_unit.clone()),
            },
        ];
        detailed.dc_nodes = vec![
            DcNode {
                component: first_dc_node.clone(),
                nominal_voltage_kv: Some(320.0),
                dc_converter_unit: Some(converter_unit.clone()),
                dc_topological_node: Some(first_dc_topological_node.clone()),
                voltage_kv: Some(320.0),
            },
            DcNode {
                component: second_dc_node.clone(),
                nominal_voltage_kv: Some(320.0),
                dc_converter_unit: Some(converter_unit.clone()),
                dc_topological_node: Some(second_dc_topological_node.clone()),
                voltage_kv: Some(0.0),
            },
            DcNode {
                component: third_dc_node.clone(),
                nominal_voltage_kv: Some(320.0),
                dc_converter_unit: Some(converter_unit.clone()),
                dc_topological_node: Some(third_dc_topological_node.clone()),
                voltage_kv: Some(-320.0),
            },
        ];
        let dc_terminal = |id: &str,
                           sequence_number: u32,
                           node: &ComponentId,
                           topological_node: &ComponentId,
                           polarity: Option<DcPolarity>,
                           connected: bool,
                           current_a: Option<f64>| DcTerminal {
            component: Some(component("dc_terminal", id)),
            sequence_number: Some(sequence_number),
            dc_node: Some(node.clone()),
            dc_topological_node: Some(topological_node.clone()),
            polarity,
            connected: Some(connected),
            active_power_mw: None,
            current_a,
        };
        detailed.dc_grounds.push(DcGround {
            component: component("dc_ground", "dc-ground-1"),
            equipment_container: Some(converter_unit.clone()),
            dc_terminal: dc_terminal(
                "ground-t1",
                1,
                &first_dc_node,
                &first_dc_topological_node,
                None,
                true,
                None,
            ),
            rated_dc_voltage_kv: Some(320.0),
            resistance_ohm: Some(0.8),
            inductance_h: Some(0.01),
        });
        detailed.dc_lines.push(DcLine {
            component: component("dc_line", "dc-line-1"),
            equipment_container: Some(dc_line_container.clone()),
            dc_terminal1: dc_terminal(
                "line-t1",
                1,
                &first_dc_node,
                &first_dc_topological_node,
                None,
                true,
                None,
            ),
            dc_terminal2: dc_terminal(
                "line-t2",
                2,
                &second_dc_node,
                &second_dc_topological_node,
                None,
                true,
                None,
            ),
            rated_dc_voltage_kv: Some(320.0),
            resistance_ohm: Some(1.25),
            inductance_h: Some(0.02),
            capacitance_f: Some(3.0e-6),
            length_km: Some(100.0),
        });
        detailed.dc_busbars.push(DcBusbar {
            component: dc_busbar,
            equipment_container: Some(converter_unit.clone()),
            dc_terminal: dc_terminal(
                "busbar-t1",
                1,
                &first_dc_node,
                &first_dc_topological_node,
                None,
                true,
                None,
            ),
            rated_dc_voltage_kv: Some(320.0),
        });
        detailed.dc_series_devices.push(DcSeriesDevice {
            component: dc_series_device,
            equipment_container: Some(converter_unit.clone()),
            dc_terminal1: dc_terminal(
                "series-t1",
                1,
                &second_dc_node,
                &second_dc_topological_node,
                None,
                true,
                None,
            ),
            dc_terminal2: dc_terminal(
                "series-t2",
                2,
                &third_dc_node,
                &third_dc_topological_node,
                None,
                true,
                None,
            ),
            rated_dc_voltage_kv: Some(315.0),
            resistance_ohm: Some(0.75),
            inductance_h: Some(0.015),
        });
        detailed.dc_switches.push(DcSwitch {
            component: component("dc_switch", "dc-breaker-1"),
            equipment_container: Some(converter_unit.clone()),
            dc_terminal1: dc_terminal(
                "switch-t1",
                1,
                &second_dc_node,
                &second_dc_topological_node,
                None,
                false,
                None,
            ),
            dc_terminal2: dc_terminal(
                "switch-t2",
                2,
                &third_dc_node,
                &third_dc_topological_node,
                None,
                false,
                None,
            ),
            kind: DcSwitchKind::Breaker,
            rated_dc_voltage_kv: Some(320.0),
            open: Some(true),
            resistance_ohm: None,
        });

        let voltage_source = component("voltage_source_converter", "vsc-1");
        let line_commutated = component("line_commutated_converter", "lcc-1");
        let voltage_level = component("voltage_level", "vl-A");
        let first_bus = component("bus", "tn-A");
        let second_bus = component("bus", "tn-B");
        let first_node = component("connectivity_node", "node-A");
        let second_node = component("connectivity_node", "node-B");
        detailed.terminals.extend([
            Terminal {
                equipment: voltage_source.clone(),
                terminal: 1,
                voltage_level: voltage_level.clone(),
                bus: Some(first_bus.clone()),
                connectable_bus: Some(first_bus),
                node: Some(first_node),
                connected: true,
                active_power_mw: Some(150.0),
                reactive_power_mvar: Some(20.0),
            },
            Terminal {
                equipment: line_commutated.clone(),
                terminal: 1,
                voltage_level,
                bus: Some(second_bus.clone()),
                connectable_bus: Some(second_bus),
                node: Some(second_node),
                connected: true,
                active_power_mw: Some(-145.0),
                reactive_power_mvar: Some(35.0),
            },
        ]);
        detailed
            .voltage_source_converters
            .push(VoltageSourceConverter {
                component: voltage_source.clone(),
                dc_converter_unit: Some(converter_unit.clone()),
                dc_terminal1: dc_terminal(
                    "vsc-t1",
                    1,
                    &first_dc_node,
                    &first_dc_topological_node,
                    Some(DcPolarity::Positive),
                    true,
                    Some(470.0),
                ),
                dc_terminal2: dc_terminal(
                    "vsc-t2",
                    2,
                    &second_dc_node,
                    &second_dc_topological_node,
                    Some(DcPolarity::Negative),
                    true,
                    Some(-470.0),
                ),
                base_apparent_power_mva: Some(200.0),
                minimum_active_power_mw: Some(-200.0),
                maximum_active_power_mw: Some(200.0),
                minimum_dc_voltage_kv: Some(280.0),
                maximum_dc_voltage_kv: Some(350.0),
                rated_dc_voltage_kv: Some(320.0),
                valve_u0_kv: Some(0.5),
                number_of_valves: Some(6),
                idle_loss_mw: Some(0.5),
                switching_loss_mw_per_ampere: Some(0.001),
                resistive_loss_ohm: Some(0.05),
                control_mode: Some(AcDcConverterControlMode::DcVoltage),
                target_active_power_mw: Some(150.0),
                target_dc_voltage_kv: Some(320.0),
                active_power_at_pcc_mw: Some(150.0),
                reactive_power_at_pcc_mvar: Some(20.0),
                pcc_terminal: Some(TerminalReference {
                    equipment: voltage_source,
                    terminal: 1,
                }),
                droop_curve: None,
                droop: None,
                droop_compensation: None,
                q_share: None,
                maximum_modulation_index: Some(1.1),
                maximum_valve_current_a: Some(1_000.0),
                voltage_regulator_on: Some(false),
                voltage_setpoint_kv: Some(230.0),
                reactive_power_setpoint_mvar: Some(20.0),
                reactive_limits: Some(ReactiveLimits::CapabilityCurve(ReactiveCapabilityCurve {
                    curve_style: CurveStyle::StraightLineYValues,
                    properties: std::collections::BTreeMap::default(),
                    points: vec![ReactiveCapabilityCurvePoint {
                        active_power_mw: 150.0,
                        minimum_reactive_power_mvar: -80.0,
                        maximum_reactive_power_mvar: 90.0,
                        properties: std::collections::BTreeMap::default(),
                    }],
                })),
                pole_loss_active_power_mw: Some(0.981),
                dc_current_a: Some(470.0),
                ac_voltage_kv: Some(230.0),
                dc_voltage_kv: Some(320.0),
                delta_degrees: Some(2.0),
                uf_kv: None,
                uv_kv: Some(231.0),
            });
        detailed
            .line_commutated_converters
            .push(LineCommutatedConverter {
                component: line_commutated.clone(),
                dc_converter_unit: Some(converter_unit),
                dc_terminal1: dc_terminal(
                    "lcc-t1",
                    1,
                    &second_dc_node,
                    &second_dc_topological_node,
                    Some(DcPolarity::Positive),
                    true,
                    Some(450.0),
                ),
                dc_terminal2: dc_terminal(
                    "lcc-t2",
                    2,
                    &third_dc_node,
                    &third_dc_topological_node,
                    Some(DcPolarity::Negative),
                    true,
                    Some(-450.0),
                ),
                base_apparent_power_mva: Some(200.0),
                minimum_active_power_mw: Some(-200.0),
                maximum_active_power_mw: Some(200.0),
                minimum_dc_voltage_kv: Some(280.0),
                maximum_dc_voltage_kv: Some(350.0),
                rated_dc_voltage_kv: Some(320.0),
                valve_u0_kv: Some(0.6),
                number_of_valves: Some(12),
                idle_loss_mw: Some(0.6),
                switching_loss_mw_per_ampere: Some(0.002),
                resistive_loss_ohm: Some(0.06),
                control_mode: Some(AcDcConverterControlMode::ActivePowerAtPcc),
                target_active_power_mw: Some(-145.0),
                target_dc_voltage_kv: Some(320.0),
                active_power_at_pcc_mw: Some(-145.0),
                reactive_power_at_pcc_mvar: Some(35.0),
                pcc_terminal: Some(TerminalReference {
                    equipment: line_commutated,
                    terminal: 1,
                }),
                droop_curve: None,
                reactive_model: Some(LineCommutatedConverterReactiveModel::CalculatedPowerFactor),
                power_factor: Some(0.972),
                operating_mode: Some(LineCommutatedConverterOperatingMode::Inverter),
                rated_dc_current_a: Some(1_000.0),
                minimum_alpha_degrees: Some(5.0),
                maximum_alpha_degrees: Some(25.0),
                minimum_gamma_degrees: Some(10.0),
                maximum_gamma_degrees: Some(30.0),
                target_alpha_degrees: Some(12.0),
                target_gamma_degrees: Some(18.0),
                target_dc_current_a: Some(450.0),
                pole_loss_active_power_mw: Some(1.512),
                dc_current_a: Some(450.0),
                ac_voltage_kv: Some(230.0),
                dc_voltage_kv: Some(320.0),
                alpha_degrees: Some(12.5),
                gamma_degrees: Some(18.5),
            });

        let mut missing_line_value = network.clone();
        Arc::make_mut(
            missing_line_value
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .dc_lines[0]
            .capacitance_f = None;
        let error = write::write_cgmes(&missing_line_value, CgmesVersion::V3_0).unwrap_err();
        assert!(error.to_string().contains("DCLineSegment.capacitance"));

        let mut missing_line_container = network.clone();
        Arc::make_mut(
            missing_line_container
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .dc_lines[0]
            .equipment_container = None;
        let error = write::write_cgmes(&missing_line_container, CgmesVersion::V3_0).unwrap_err();
        assert!(error.to_string().contains("dc-line-1"));
        assert!(error.to_string().contains("Equipment.EquipmentContainer"));

        let mut missing_busbar_voltage = network.clone();
        Arc::make_mut(
            missing_busbar_voltage
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .dc_busbars[0]
            .rated_dc_voltage_kv = None;
        let error = write::write_cgmes(&missing_busbar_voltage, CgmesVersion::V3_0).unwrap_err();
        assert!(error.to_string().contains("DCConductingEquipment.ratedUdc"));
        assert!(error.to_string().contains("dc-busbar-1"));

        let mut missing_series_resistance = network.clone();
        Arc::make_mut(
            missing_series_resistance
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .dc_series_devices[0]
            .resistance_ohm = None;
        let error = write::write_cgmes(&missing_series_resistance, CgmesVersion::V3_0).unwrap_err();
        assert!(error.to_string().contains("DCSeriesDevice.resistance"));

        let mut missing_series_container = network.clone();
        Arc::make_mut(
            missing_series_container
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .dc_series_devices[0]
            .equipment_container = None;
        let error = write::write_cgmes(&missing_series_container, CgmesVersion::V3_0).unwrap_err();
        assert!(error.to_string().contains("Equipment.EquipmentContainer"));

        let mut version_2_network = network.clone();
        let version_2_detailed = Arc::make_mut(
            version_2_network
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        version_2_detailed.voltage_source_converters.clear();
        version_2_detailed.line_commutated_converters.clear();
        let version_2 = write::write_cgmes(&version_2_network, CgmesVersion::V2_4_15).unwrap();
        let version_2_xml = version_2
            .files
            .iter()
            .map(|(_, xml)| xml.as_str())
            .collect::<String>();
        assert!(version_2_xml.contains("<cim:DCSeriesDevice.ratedUdc>315"));
        assert!(!version_2_xml.contains("<cim:DCConductingEquipment.ratedUdc>315"));
        let mut missing_version_2_series_voltage = version_2_network;
        Arc::make_mut(
            missing_version_2_series_voltage
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .dc_series_devices[0]
            .rated_dc_voltage_kv = None;
        let error = write::write_cgmes(&missing_version_2_series_voltage, CgmesVersion::V2_4_15)
            .unwrap_err();
        assert!(error.to_string().contains("DCSeriesDevice.ratedUdc"));

        let mut missing_converter_voltage = network.clone();
        Arc::make_mut(
            missing_converter_voltage
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .voltage_source_converters[0]
            .rated_dc_voltage_kv = None;
        let error = write::write_cgmes(&missing_converter_voltage, CgmesVersion::V3_0).unwrap_err();
        assert!(error.to_string().contains("ACDCConverter.ratedUdc"));

        let mut incomplete_sv = network.clone();
        Arc::make_mut(incomplete_sv.detailed_connectivity_mut().as_mut().unwrap())
            .voltage_source_converters[0]
            .uv_kv = None;
        let incomplete_output = write::write_cgmes(&incomplete_sv, CgmesVersion::V3_0).unwrap();
        assert!(
            incomplete_output
                .warnings
                .iter()
                .any(|warning| warning.contains("VsConverter.uv"))
        );
        let incomplete_sv_xml = incomplete_output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(!incomplete_sv_xml.contains("<cim:VsConverter"));

        let mut droop_control = network.clone();
        let vsc = &mut Arc::make_mut(droop_control.detailed_connectivity_mut().as_mut().unwrap())
            .voltage_source_converters[0];
        vsc.control_mode = Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroop);
        vsc.droop = Some(0.125);
        vsc.droop_compensation = Some(0.25);
        let droop_output = write::write_cgmes(&droop_control, CgmesVersion::V3_0).unwrap();
        let droop_xml = droop_output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SSH.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(droop_xml.contains("VsPpccControlKind.pPccAndUdcDroop"));
        assert!(!droop_xml.contains("VsPpccControlKind.pPccAndUdcDroopWithCompensation"));
        assert!(droop_xml.contains("<cim:VsConverter.droop>0.125"));

        let mut compensated_control = network.clone();
        let vsc = &mut Arc::make_mut(
            compensated_control
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .voltage_source_converters[0];
        vsc.control_mode =
            Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopWithCompensation);
        vsc.droop = Some(0.125);
        vsc.droop_compensation = Some(0.25);
        let compensated = write::write_cgmes(&compensated_control, CgmesVersion::V3_0).unwrap();
        let compensated_xml = compensated
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SSH.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(compensated_xml.contains("VsPpccControlKind.pPccAndUdcDroopWithCompensation"));
        assert!(compensated_xml.contains("<cim:VsConverter.droopCompensation>0.25"));

        let mut pilot_control = network.clone();
        let vsc = &mut Arc::make_mut(pilot_control.detailed_connectivity_mut().as_mut().unwrap())
            .voltage_source_converters[0];
        vsc.control_mode = Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopPilot);
        vsc.droop = Some(0.125);
        let pilot = write::write_cgmes(&pilot_control, CgmesVersion::V3_0).unwrap();
        let pilot_xml = pilot
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SSH.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(pilot_xml.contains("VsPpccControlKind.pPccAndUdcDroopPilot"));

        let mut missing_compensation = compensated_control.clone();
        Arc::make_mut(
            missing_compensation
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .voltage_source_converters[0]
            .droop_compensation = None;
        let error = write::write_cgmes(&missing_compensation, CgmesVersion::V3_0).unwrap_err();
        assert!(error.to_string().contains("VsConverter.droopCompensation"));

        let mut xiidm_curve_control = network.clone();
        Arc::make_mut(
            xiidm_curve_control
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .voltage_source_converters[0]
            .control_mode = Some(AcDcConverterControlMode::ActivePowerAtPccAndDcVoltageDroopCurve);
        let error = write::write_cgmes(&xiidm_curve_control, CgmesVersion::V3_0).unwrap_err();
        assert!(error.to_string().contains("XIIDM piecewise droop curve"));
        assert!(error.to_string().contains("no CGMES VsPpccControlKind"));

        let mut constant_network = network.clone();
        let constant_vsc = &mut Arc::make_mut(
            constant_network
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .voltage_source_converters[0];
        let Some(ReactiveLimits::CapabilityCurve(constant_curve)) =
            constant_vsc.reactive_limits.as_mut()
        else {
            panic!("expected reactive capability curve");
        };
        constant_curve.curve_style = CurveStyle::ConstantYValue;
        let constant_output = write::write_cgmes(&constant_network, CgmesVersion::V3_0).unwrap();
        let constant_xml = constant_output
            .files
            .iter()
            .map(|(_, xml)| xml.as_str())
            .collect::<String>();
        assert!(constant_xml.contains("CurveStyle.constantYValue"));
        assert!(!constant_xml.contains("CurveStyle.straightLineYValues"));
        let constant_parsed =
            read::read_cgmes_documents(constant_output.files, Some("constant-curve")).unwrap();
        let constant_vsc = &constant_parsed
            .network
            .detailed_connectivity()
            .as_deref()
            .unwrap()
            .voltage_source_converters[0];
        let Some(ReactiveLimits::CapabilityCurve(constant_curve)) = &constant_vsc.reactive_limits
        else {
            panic!("expected reactive capability curve");
        };
        assert_eq!(constant_curve.curve_style, CurveStyle::ConstantYValue);

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let all_xml = output
            .files
            .iter()
            .map(|(_, xml)| xml.as_str())
            .collect::<String>();
        for class in [
            "DCConverterUnit",
            "DCNode",
            "DCTopologicalNode",
            "DCGround",
            "DCBusbar",
            "DCLine",
            "DCLineSegment",
            "DCSeriesDevice",
            "DCBreaker",
            "VsConverter",
            "CsConverter",
            "DCTerminal",
            "ACDCConverterDCTerminal",
        ] {
            assert!(
                all_xml.contains(&format!("<cim:{class}")),
                "missing {class}"
            );
        }
        assert!(all_xml.contains("DCConverterOperatingModeKind.bipolar"));
        assert!(!all_xml.contains("PowerIO DC converter unit"));
        assert!(all_xml.contains("DCPolarityKind.positive"));
        assert!(all_xml.contains("DCPolarityKind.negative"));
        assert!(all_xml.contains("<cim:DCLineSegment.inductance>0.02"));
        assert!(all_xml.contains("<cim:DCLineSegment.capacitance>0.000003"));
        assert!(all_xml.contains(&format!("<cim:DCLine rdf:ID=\"_{DC_LINE_CONTAINER_MRID}\"")));
        assert!(all_xml.contains(&format!("<cim:DCBusbar rdf:ID=\"_{DC_BUSBAR_MRID}\"")));
        assert!(all_xml.contains(&format!(
            "<cim:DCSeriesDevice rdf:ID=\"_{DC_SERIES_DEVICE_MRID}\""
        )));
        assert!(all_xml.contains(
            "<cim:IdentifiedObject.name>Retained DC line container</cim:IdentifiedObject.name>"
        ));
        assert!(
            all_xml
                .contains("<cim:IdentifiedObject.name>North DC busbar</cim:IdentifiedObject.name>")
        );
        assert!(all_xml.contains(
            "<cim:IdentifiedObject.name>North DC series device</cim:IdentifiedObject.name>"
        ));
        assert!(all_xml.contains(&format!(
            "<cim:Equipment.EquipmentContainer rdf:resource=\"#_{DC_LINE_CONTAINER_MRID}\""
        )));
        assert!(
            all_xml.find("<cim:DCLine rdf:ID").unwrap()
                < all_xml.find("<cim:DCLineSegment rdf:ID").unwrap()
        );
        assert!(all_xml.contains("<cim:DCConductingEquipment.ratedUdc>315"));
        assert!(all_xml.contains("<cim:DCSeriesDevice.resistance>0.75"));
        assert!(all_xml.contains("<cim:DCSeriesDevice.inductance>0.015"));
        assert!(all_xml.contains("<cim:CsConverter.targetAlpha>12"));
        assert!(all_xml.contains("<cim:CsConverter.targetGamma>18"));
        assert!(all_xml.contains("<cim:CsConverter.targetIdc>450"));
        assert_eq!(
            all_xml
                .matches("<cim:ACDCTerminal.connected>false</cim:ACDCTerminal.connected>")
                .count(),
            2
        );
        assert!(!all_xml.contains("<cim:VsConverter.droop>0</cim:VsConverter.droop>"));
        assert!(!all_xml.contains("<cim:ACDCConverter.ratedUdc>1</"));
        assert!(all_xml.contains("CurveStyle.straightLineYValues"));
        assert!(all_xml.contains(
            "<cim:Curve.xUnit rdf:resource=\"http://iec.ch/TC57/CIM100#UnitSymbol.W\"/>"
        ));
        assert!(all_xml.contains(
            "<cim:Curve.y1Unit rdf:resource=\"http://iec.ch/TC57/CIM100#UnitSymbol.VAr\"/>"
        ));
        assert!(all_xml.contains(
            "<cim:Curve.y2Unit rdf:resource=\"http://iec.ch/TC57/CIM100#UnitSymbol.VAr\"/>"
        ));

        let mut unknown_style_files = output.files.clone();
        let eq = &mut unknown_style_files
            .iter_mut()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .unwrap()
            .1;
        *eq = eq.replacen(
            "CurveStyle.straightLineYValues",
            "CurveStyle.unsupported",
            1,
        );
        let unknown_style =
            read::read_cgmes_documents(unknown_style_files, Some("unknown-curve-style")).unwrap();
        let unknown_style_vsc = &unknown_style
            .network
            .detailed_connectivity()
            .as_deref()
            .unwrap()
            .voltage_source_converters[0];
        assert!(unknown_style_vsc.reactive_limits.is_none());
        assert!(unknown_style.warnings.iter().any(|warning| {
            warning.contains("Curve.curveStyle `unsupported` is unknown")
                && warning.contains("reactive limits were not assigned")
        }));

        for (property, expected, unsupported) in [
            ("Curve.xUnit", "W", "V"),
            ("Curve.y1Unit", "VAr", "A"),
            ("Curve.y2Unit", "VAr", "A"),
        ] {
            let mut unsupported_unit_files = output.files.clone();
            let eq = &mut unsupported_unit_files
                .iter_mut()
                .find(|(name, _)| name.ends_with("_EQ.xml"))
                .unwrap()
                .1;
            let supported = format!(
                "<cim:{property} rdf:resource=\"http://iec.ch/TC57/CIM100#UnitSymbol.{expected}\"/>"
            );
            let unsupported_element = format!(
                "<cim:{property} rdf:resource=\"http://iec.ch/TC57/CIM100#UnitSymbol.{unsupported}\"/>"
            );
            assert!(eq.contains(&supported));
            *eq = eq.replacen(&supported, &unsupported_element, 1);
            let parsed =
                read::read_cgmes_documents(unsupported_unit_files, Some("unsupported-curve-unit"))
                    .unwrap();
            let vsc = &parsed
                .network
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .voltage_source_converters[0];
            assert!(vsc.reactive_limits.is_none());
            assert!(
                parsed.warnings.iter().any(|warning| {
                    warning.contains(&format!(
                        "{property} `UnitSymbol.{unsupported}` is unsupported"
                    )) && warning.contains(&format!("expected `UnitSymbol.{expected}`"))
                        && warning.contains("reactive limits were not assigned")
                }),
                "warnings for {property}: {:?}",
                parsed.warnings
            );
        }

        let parsed = read::read_cgmes_documents(output.files, Some("physical-dc")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.dc_converter_units.len(), 1);
        assert_eq!(
            detailed.dc_converter_units[0].operation_mode,
            DcConverterOperatingMode::Bipolar
        );
        assert_eq!(detailed.dc_nodes.len(), 3);
        assert_eq!(detailed.dc_grounds.len(), 1);
        assert_eq!(detailed.dc_busbars.len(), 1);
        assert_eq!(detailed.dc_lines.len(), 1);
        assert_eq!(detailed.dc_series_devices.len(), 1);
        assert_eq!(detailed.dc_switches.len(), 1);
        assert_eq!(detailed.voltage_source_converters.len(), 1);
        assert_eq!(detailed.line_commutated_converters.len(), 1);
        assert!((detailed.dc_lines[0].resistance_ohm.unwrap() - 1.25).abs() < 1e-12);
        assert!((detailed.dc_lines[0].inductance_h.unwrap() - 0.02).abs() < 1e-12);
        assert!((detailed.dc_lines[0].capacitance_f.unwrap() - 3.0e-6).abs() < 1e-12);
        assert_eq!(
            detailed.dc_lines[0]
                .equipment_container
                .as_ref()
                .unwrap()
                .component_type(),
            "dc_line_container"
        );
        assert!((detailed.dc_busbars[0].rated_dc_voltage_kv.unwrap() - 320.0).abs() < 1e-12);
        assert_eq!(detailed.dc_busbars[0].dc_terminal.sequence_number, Some(1));
        assert!((detailed.dc_series_devices[0].rated_dc_voltage_kv.unwrap() - 315.0).abs() < 1e-12);
        assert!((detailed.dc_series_devices[0].resistance_ohm.unwrap() - 0.75).abs() < 1e-12);
        assert!((detailed.dc_series_devices[0].inductance_h.unwrap() - 0.015).abs() < 1e-12);
        assert_eq!(
            detailed.dc_series_devices[0]
                .dc_terminal2
                .dc_topological_node
                .as_ref()
                .unwrap(),
            detailed.dc_nodes[2].dc_topological_node.as_ref().unwrap()
        );
        for (component_type, mrid, name) in [
            (
                "dc_line_container",
                DC_LINE_CONTAINER_MRID,
                "Retained DC line container",
            ),
            ("dc_busbar", DC_BUSBAR_MRID, "North DC busbar"),
            (
                "dc_series_device",
                DC_SERIES_DEVICE_MRID,
                "North DC series device",
            ),
        ] {
            let metadata = detailed
                .component_metadata
                .iter()
                .find(|metadata| metadata.component.component_type() == component_type)
                .unwrap();
            assert_eq!(metadata.name.as_deref(), Some(name));
            assert!(metadata.external_identifiers.iter().any(|identifier| {
                identifier.value == mrid && identifier.authority.as_deref() == Some("CGMES")
            }));
        }
        assert_eq!(detailed.dc_switches[0].open, Some(true));
        assert_eq!(
            detailed.voltage_source_converters[0].control_mode,
            Some(AcDcConverterControlMode::DcVoltage)
        );
        let Some(ReactiveLimits::CapabilityCurve(curve)) =
            &detailed.voltage_source_converters[0].reactive_limits
        else {
            panic!("expected reactive capability curve");
        };
        assert_eq!(curve.curve_style, CurveStyle::StraightLineYValues);
        assert_eq!(detailed.line_commutated_converters[0].reactive_model, None);
        assert_eq!(detailed.line_commutated_converters[0].power_factor, None);
        assert_eq!(
            detailed.line_commutated_converters[0].operating_mode,
            Some(LineCommutatedConverterOperatingMode::Inverter)
        );
    }

    #[test]
    fn static_var_compensator_round_trips() {
        let mut network = network();
        let mut svc = StaticVarCompensator::new(BusId(2), -0.02, 0.04);
        svc.voltage_setpoint_kv = 228.0;
        svc.reactive_power_setpoint_mvar = 12.5;
        svc.regulation_mode = StaticVarCompensatorRegulationMode::ReactivePower;
        svc.regulating = true;
        svc.regulating_terminal = Some(TerminalReference {
            equipment: component("load", network.loads()[0].uid.as_deref().unwrap()),
            terminal: 1,
        });
        svc.p = 1.25;
        svc.q = 12.5;
        svc.uid = Some("68fe2cdf-43b4-5549-a7ce-95f3425edb19".into());
        network.static_var_compensators_mut().push(svc);

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let parsed = read::read_cgmes_documents(output.files, Some("svc")).unwrap();
        let svc = &parsed.network.static_var_compensators()[0];
        assert_eq!(
            svc.regulation_mode,
            StaticVarCompensatorRegulationMode::ReactivePower
        );
        assert!(svc.regulating);
        assert_eq!(svc.regulating_terminal.as_ref().unwrap().terminal, 1);
        assert!((svc.b_min_siemens + 0.02).abs() < 1e-12);
        assert!((svc.b_max_siemens - 0.04).abs() < 1e-12);
        assert!((svc.voltage_setpoint_kv - 228.0).abs() < 1e-12);
        assert!((svc.reactive_power_setpoint_mvar - 12.5).abs() < 1e-12);
        assert!((svc.p - 1.25).abs() < 1e-12);
        assert!((svc.q - 12.5).abs() < 1e-12);
    }

    #[test]
    fn operational_limit_group_round_trips_without_collapsing_limit_types() {
        let mut network = detailed_network();
        let branch = component("branch", network.branches()[0].uid.as_deref().unwrap());
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed
            .operational_limit_groups
            .push(OperationalLimitGroup {
                equipment: branch,
                terminal: 1,
                id: "5600bceb-a97b-5060-80d4-d546bbc85741".into(),
                properties: std::collections::BTreeMap::new(),
                selected: false,
                current_limits: Some(LoadingLimits {
                    permanent_limit: Some(1000.0),
                    permanent_limit_name: Some("permanent current".into()),
                    temporary_limits: vec![TemporaryLimit {
                        name: "five minute current".into(),
                        value: 1200.0,
                        acceptable_duration_seconds: 300,
                        fictitious: true,
                    }],
                }),
                active_power_limits: Some(LoadingLimits {
                    permanent_limit: Some(80.0),
                    permanent_limit_name: Some("permanent active power".into()),
                    temporary_limits: Vec::new(),
                }),
                apparent_power_limits: Some(LoadingLimits {
                    permanent_limit: Some(90.0),
                    permanent_limit_name: Some("permanent apparent power".into()),
                    temporary_limits: Vec::new(),
                }),
            });

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let parsed = read::read_cgmes_documents(output.files, Some("limits")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        let group = detailed
            .operational_limit_groups
            .iter()
            .find(|value| value.id == "5600bceb-a97b-5060-80d4-d546bbc85741")
            .unwrap();
        assert_eq!(group.terminal, 1);
        let current = group.current_limits.as_ref().unwrap();
        assert_eq!(current.permanent_limit, Some(1000.0));
        assert_eq!(
            current.permanent_limit_name.as_deref(),
            Some("permanent current")
        );
        assert_eq!(current.temporary_limits.len(), 1);
        assert_eq!(current.temporary_limits[0].acceptable_duration_seconds, 300);
        assert!(current.temporary_limits[0].fictitious);
        assert_eq!(
            group
                .active_power_limits
                .as_ref()
                .and_then(|limits| limits.permanent_limit),
            Some(80.0)
        );
        assert_eq!(
            group
                .apparent_power_limits
                .as_ref()
                .and_then(|limits| limits.permanent_limit),
            Some(90.0)
        );
    }

    #[test]
    fn real_cgmes_tap_associations_and_table_steps_round_trip() {
        let mut network = detailed_network();
        network.branches_mut()[0].tap = 1.05;
        network.branches_mut()[0].shift = 2.0;
        let transformer = component("branch", network.branches()[0].uid.as_deref().unwrap());
        let step = |position, rho, alpha_degrees| TapChangerStep {
            position,
            rho,
            alpha_degrees,
            resistance_deviation_percent: 0.0,
            reactance_deviation_percent: 0.0,
            conductance_deviation_percent: 0.0,
            susceptance_deviation_percent: 0.0,
        };
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.tap_changers = vec![
            TapChanger {
                transformer: transformer.clone(),
                winding: 1,
                kind: TapChangerKind::Ratio,
                tap_position: Some(1),
                solved_tap_position: Some(0),
                low_tap_position: -1,
                load_tap_changing_capabilities: true,
                regulating: true,
                regulation_mode: Some(TapChangerRegulationMode::Voltage),
                regulation_value: Some(228.0),
                target_deadband: Some(2.0),
                regulation_terminal: Some(TerminalReference {
                    equipment: transformer.clone(),
                    terminal: 2,
                }),
                steps: vec![step(-1, 0.95, 0.0), step(0, 1.0, 0.0), step(1, 1.05, 0.0)],
            },
            TapChanger {
                transformer,
                winding: 1,
                kind: TapChangerKind::Phase,
                tap_position: Some(1),
                solved_tap_position: Some(-1),
                low_tap_position: -1,
                load_tap_changing_capabilities: true,
                regulating: true,
                regulation_mode: Some(TapChangerRegulationMode::ActivePower),
                regulation_value: Some(25.0),
                target_deadband: Some(1.0),
                regulation_terminal: None,
                steps: vec![step(-1, 1.0, -2.0), step(0, 1.0, 0.0), step(1, 1.0, 2.0)],
            },
        ];

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(eq.contains("<cim:RatioTapChanger.TransformerEnd "));
        assert!(eq.contains("<cim:PhaseTapChanger.TransformerEnd "));
        assert!(eq.contains("<cim:TapChangerTablePoint.step>"));
        assert!(eq.contains("<cim:TapChangerTablePoint.ratio>"));
        assert!(eq.contains("<cim:PhaseTapChangerTablePoint.angle>"));
        assert!(!eq.contains("<cim:TransformerEnd.RatioTapChanger "));
        assert!(!eq.contains("<cim:TransformerEnd.PhaseTapChanger "));

        let parsed = read::read_cgmes_documents(output.files, Some("tap-table")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.tap_changers.len(), 2);
        let ratio = detailed
            .tap_changers
            .iter()
            .find(|tap| tap.kind == TapChangerKind::Ratio)
            .unwrap();
        assert_eq!(ratio.tap_position, Some(1));
        assert_eq!(ratio.solved_tap_position, Some(0));
        assert_eq!(ratio.steps.len(), 3);
        assert!((ratio.steps[0].rho - 0.95).abs() < 1e-12);
        let phase = detailed
            .tap_changers
            .iter()
            .find(|tap| tap.kind == TapChangerKind::Phase)
            .unwrap();
        assert_eq!(phase.solved_tap_position, Some(-1));
        assert!((phase.steps[2].alpha_degrees - 2.0).abs() < 1e-12);
        assert!((parsed.network.branches()[0].tap - 1.05).abs() < 1e-12);
        assert!((parsed.network.branches()[0].shift - 2.0).abs() < 1e-12);
    }

    #[test]
    fn names_are_xml_escaped_once_and_round_trip() {
        let mut network = network();
        *network.name_mut() = "A & B <North>".into();
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(eq.contains("A &amp; B &lt;North&gt;"));
        assert!(!eq.contains("&amp;amp;"));

        let parsed = read::read_cgmes_documents(output.files, Some("escaped-name")).unwrap();
        assert_eq!(parsed.network.name(), "A & B <North>");
    }

    #[test]
    fn artifact_names_are_bounded_deterministic_and_independent_of_model_description() {
        let descriptions = [
            ("CGMES model legal notice: ".to_string() + &"all rights and conditions; ".repeat(40))
                .trim_end()
                .to_string(),
            r#"../../north\west:*?\"<>| & grid"#.to_string(),
        ];
        for description in descriptions {
            let mut network = network();
            *network.name_mut() = description.clone();
            let mut expected_names = write::write_cgmes(&network, CgmesVersion::V3_0)
                .unwrap()
                .files
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>();
            let repeated_names = write::write_cgmes(&network, CgmesVersion::V3_0)
                .unwrap()
                .files
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>();
            assert_eq!(expected_names, repeated_names);
            expected_names.sort();

            let result = crate::format::emit(
                &PioModule::new(network),
                TargetFormat::Cgmes,
                Destination::memory("cgmes").unwrap(),
            )
            .unwrap();
            let EmittedOutput::Memory { artifacts } = result.into_output() else {
                panic!("memory destination returned paths");
            };
            let names = artifacts
                .iter()
                .map(|artifact| {
                    artifact
                        .name()
                        .as_str()
                        .strip_prefix("cgmes/")
                        .unwrap()
                        .to_string()
                })
                .collect::<Vec<_>>();
            assert_eq!(names, expected_names);
            assert!(names.iter().all(|name| {
                name.starts_with("powerio_")
                    && name.len() <= 52
                    && name.chars().all(|character| {
                        character.is_ascii_alphanumeric() || "_-.".contains(character)
                    })
            }));

            let documents = artifacts
                .iter()
                .map(|artifact| {
                    (
                        artifact.name().to_string(),
                        std::str::from_utf8(artifact.bytes()).unwrap().to_string(),
                    )
                })
                .collect();
            let parsed = read::read_cgmes_documents(documents, Some("bounded-name")).unwrap();
            assert_eq!(parsed.network.name().as_str(), description);
        }
    }

    #[test]
    fn cim100_equipment_extensions_merge_into_concrete_eq_objects() {
        let output = write::write_cgmes(&network(), CgmesVersion::V3_0).unwrap();
        let mut documents: Vec<_> = output
            .files
            .into_iter()
            .map(|(name, text)| {
                if name.ends_with("_SSH.xml") {
                    (
                        name,
                        text.replace(
                            "<cim:EnergyConsumer rdf:about=",
                            "<cim:Equipment rdf:about=",
                        )
                        .replace("</cim:EnergyConsumer>", "</cim:Equipment>"),
                    )
                } else {
                    (name, text)
                }
            })
            .collect();
        documents.reverse();
        let parsed = read::read_cgmes_documents(documents, Some("generic-equipment")).unwrap();
        assert_eq!(parsed.network.loads().len(), 1);
        assert!(parsed.network.loads()[0].in_service);
    }

    #[test]
    fn zip_profile_set_parses_and_unsafe_entries_are_refused() {
        let output = write::write_cgmes(&network(), CgmesVersion::V3_0).unwrap();
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in output.files {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        let source = Source::from_memory("case.zip", bytes.clone()).unwrap();
        let parsed = parse_source(&source, &mut Diagnostics::new()).unwrap();
        assert_eq!(parsed.buses().len(), 2);

        let module = crate::format::parse(source.clone()).unwrap();
        let result = crate::format::emit(
            &module,
            TargetFormat::Cgmes,
            powerio_core::Destination::memory("copy").unwrap(),
        )
        .unwrap();
        assert_eq!(result.fidelity(), powerio_core::Fidelity::ExactSameFormat);
        let powerio_core::EmittedOutput::Memory { artifacts } = result.output() else {
            panic!("a memory destination returned paths")
        };
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].bytes(), bytes);

        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("profiles.zip"),
            source.primary_buffer().unwrap().bytes(),
        )
        .unwrap();
        let source = Source::open(directory.path()).unwrap();
        let parsed = parse_source(&source, &mut Diagnostics::new()).unwrap();
        assert_eq!(parsed.branches().len(), 1);

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("../EQ.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<rdf:RDF/>").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let source = Source::from_memory("unsafe.zip", bytes).unwrap();
        assert!(parse_source(&source, &mut Diagnostics::new()).is_err());
    }

    #[test]
    fn duplicate_xml_names_across_archives_are_refused() {
        let archive = || {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            writer
                .start_file("profiles/EQ.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"<rdf:RDF/>").unwrap();
            writer.finish().unwrap().into_inner()
        };
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("first.zip"), archive()).unwrap();
        std::fs::write(directory.path().join("second.zip"), archive()).unwrap();
        let source = Source::open(directory.path()).unwrap();
        let error = acquire_documents(&source).unwrap_err();
        assert!(error.to_string().contains("duplicate normalized name"));
    }

    #[test]
    fn xml_declarations_that_enable_entities_are_refused() {
        for xml in [
            "<!DOCTYPE rdf:RDF SYSTEM \"file:///etc/passwd\"><rdf:RDF/>",
            "<!DOCTYPE rdf:RDF [<!ENTITY x \"expanded\">]><rdf:RDF>&x;</rdf:RDF>",
        ] {
            assert!(reject_unsafe_xml(xml.as_bytes()).is_err());
        }
    }

    #[test]
    fn cim16_profile_set_parses() {
        let output = write::write_cgmes(&network(), CgmesVersion::V2_4_15).unwrap();
        let parsed = read::read_cgmes_documents(output.files, Some("cim16")).unwrap();
        assert_eq!(parsed.network.source_format(), crate::SourceFormat::Cgmes);
        assert_eq!(parsed.network.buses().len(), 2);
    }

    #[test]
    fn cim16_ground_voltage_uses_the_units_unique_converter_voltage() {
        let parsed = read::read_cgmes_documents(
            dc_ground_documents(CgmesVersion::V2_4_15, &[320.0, 320.0], None),
            Some("cim16-dc-ground"),
        )
        .unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.dc_grounds.len(), 1);
        assert_eq!(detailed.dc_grounds[0].rated_dc_voltage_kv, Some(320.0));
        assert!(parsed.warnings.iter().any(|warning| {
            warning.contains("DCGround `dc-ground`")
                && warning.contains("DCConverterUnit `dc-unit`")
                && warning.contains("derived 320 kV")
                && warning.contains("unique positive ACDCConverter.ratedUdc")
        }));
    }

    #[test]
    fn cim16_ground_voltage_rejects_missing_or_conflicting_converter_values() {
        let missing = read::read_cgmes_documents(
            dc_ground_documents(CgmesVersion::V2_4_15, &[], None),
            Some("cim16-dc-ground-missing"),
        )
        .err()
        .unwrap();
        assert!(missing.to_string().contains("DCGround `dc-ground`"));
        assert!(missing.to_string().contains("DCConverterUnit `dc-unit`"));
        assert!(
            missing
                .to_string()
                .contains("no explicit positive ACDCConverter.ratedUdc")
        );

        let conflicting = read::read_cgmes_documents(
            dc_ground_documents(CgmesVersion::V2_4_15, &[320.0, 400.0], None),
            Some("cim16-dc-ground-conflicting"),
        )
        .err()
        .unwrap();
        assert!(conflicting.to_string().contains("DCGround `dc-ground`"));
        assert!(
            conflicting
                .to_string()
                .contains("DCConverterUnit `dc-unit`")
        );
        assert!(
            conflicting
                .to_string()
                .contains("conflicting ACDCConverter.ratedUdc values: 320, 400")
        );
    }

    #[test]
    fn cim100_ground_voltage_is_not_derived_from_converter_voltage() {
        let parsed = read::read_cgmes_documents(
            dc_ground_documents(CgmesVersion::V3_0, &[320.0], None),
            Some("cim100-dc-ground"),
        )
        .unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.dc_grounds.len(), 1);
        assert_eq!(detailed.dc_grounds[0].rated_dc_voltage_kv, None);
        assert!(!parsed.warnings.iter().any(|warning| {
            warning.contains("DCGround `dc-ground`") && warning.contains("derived")
        }));
    }

    #[test]
    fn cim16_dc_line_voltage_uses_matching_endpoint_unit_converter_voltages() {
        let parsed = read::read_cgmes_documents(
            dc_line_documents(CgmesVersion::V2_4_15, &[320.0], &[320.0, 320.0], None),
            Some("cim16-dc-line"),
        )
        .unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.dc_lines.len(), 1);
        assert_eq!(detailed.dc_lines[0].rated_dc_voltage_kv, Some(320.0));
        assert!(parsed.warnings.iter().any(|warning| {
            warning.contains("DCLineSegment `dc-line`")
                && warning.contains("first-dc-unit")
                && warning.contains("second-dc-unit")
                && warning.contains("derived 320 kV")
                && warning.contains("both units have the same unique positive")
        }));

        let source_dc = parsed
            .network
            .detailed_connectivity()
            .as_deref()
            .unwrap()
            .clone();
        let mut emitted_network = detailed_network();
        let emitted_dc = Arc::make_mut(
            emitted_network
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        emitted_dc.dc_converter_units = source_dc.dc_converter_units;
        emitted_dc.dc_topological_nodes = source_dc.dc_topological_nodes;
        emitted_dc.dc_nodes = source_dc.dc_nodes;
        emitted_dc.dc_lines = source_dc.dc_lines;
        let output = write::write_cgmes(&emitted_network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(eq.contains("<cim:DCConductingEquipment.ratedUdc>320"));
        let reparsed = read::read_cgmes_documents(output.files, Some("cim100-dc-line")).unwrap();
        assert_eq!(
            reparsed
                .network
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .dc_lines[0]
                .rated_dc_voltage_kv,
            Some(320.0)
        );
    }

    #[test]
    fn cim16_dc_line_voltage_rejects_missing_or_conflicting_endpoint_values() {
        let missing = read::read_cgmes_documents(
            dc_line_documents(CgmesVersion::V2_4_15, &[], &[320.0], None),
            Some("cim16-dc-line-missing"),
        )
        .err()
        .unwrap();
        assert!(missing.to_string().contains("DCLineSegment `dc-line`"));
        assert!(missing.to_string().contains("first-dc-unit"));
        assert!(
            missing
                .to_string()
                .contains("no explicit positive ACDCConverter.ratedUdc")
        );

        let conflicting = read::read_cgmes_documents(
            dc_line_documents(CgmesVersion::V2_4_15, &[320.0], &[400.0], None),
            Some("cim16-dc-line-conflicting"),
        )
        .err()
        .unwrap();
        let message = conflicting.to_string();
        assert!(message.contains("DCLineSegment `dc-line`"));
        assert!(message.contains("first-dc-unit"));
        assert!(message.contains("320 kV"));
        assert!(message.contains("second-dc-unit"));
        assert!(message.contains("400 kV"));
    }

    #[test]
    fn cim100_dc_line_voltage_is_not_derived_from_endpoint_converter_voltages() {
        let parsed = read::read_cgmes_documents(
            dc_line_documents(CgmesVersion::V3_0, &[320.0], &[320.0], None),
            Some("cim100-dc-line"),
        )
        .unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.dc_lines.len(), 1);
        assert_eq!(detailed.dc_lines[0].rated_dc_voltage_kv, None);
        assert!(!parsed.warnings.iter().any(|warning| {
            warning.contains("DCLineSegment `dc-line`") && warning.contains("derived")
        }));
    }

    #[test]
    fn missing_profiles_malformed_xml_and_dangling_references_are_refused() {
        let output = write::write_cgmes(&network(), CgmesVersion::V3_0).unwrap();
        let eq_only = output
            .files
            .iter()
            .filter(|(name, _)| name.ends_with("_EQ.xml"))
            .cloned()
            .collect();
        assert!(read::read_cgmes_documents(eq_only, Some("missing-tp")).is_err());

        assert!(xml::parse_cimxml("<rdf:RDF><cim:TopologicalNode>").is_err());

        let dangling = output
            .files
            .into_iter()
            .map(|(name, text)| {
                if name.ends_with("_TP.xml") {
                    (
                        name,
                        text.replacen("rdf:resource=\"#_", "rdf:resource=\"#_missing-", 1),
                    )
                } else {
                    (name, text)
                }
            })
            .collect();
        assert!(read::read_cgmes_documents(dangling, Some("dangling")).is_err());

        let duplicate = write::write_cgmes(&network(), CgmesVersion::V3_0)
            .unwrap()
            .files
            .into_iter()
            .map(|(name, text)| {
                if name.ends_with("_EQ.xml") {
                    (
                        name,
                        text.replace(
                            "</rdf:RDF>",
                            "  <cim:Junction rdf:ID=\"_duplicate\"><cim:IdentifiedObject.name>first</cim:IdentifiedObject.name></cim:Junction>\n  <cim:Junction rdf:ID=\"_duplicate\"><cim:IdentifiedObject.name>second</cim:IdentifiedObject.name></cim:Junction>\n</rdf:RDF>",
                        ),
                    )
                } else {
                    (name, text)
                }
            })
            .collect();
        assert!(read::read_cgmes_documents(duplicate, Some("duplicate")).is_err());
    }
}
