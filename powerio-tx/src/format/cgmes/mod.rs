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

use powerio_core::{ArtifactPath, DiagnosticInfo, MemoryArtifact, Source};

use crate::diagnostics::{Diagnostics, codes};
use crate::network::BalancedNetwork;
use crate::{Error, Result};

const MAX_FILES: usize = 4_096;
const MAX_BYTES: u64 = 64 << 20;
const MAX_COMPRESSION_RATIO: u64 = 200;
/// The modeling authority set fresh CGMES output states. A header carrying it
/// was synthesized by this writer, so its identity, version, creation time,
/// and profile dependencies state nothing a source document stated.
const POWERIO_MODELING_AUTHORITY_SET: &str = "http://powerio.dev/cgmes";
const CGMES_CLASS_PROPERTY: &str = "cgmes_class";
const CGMES_SV_STATUS_PROPERTY: &str = "SvStatus.inService";
const CGMES_GENERATING_UNIT_PROPERTY: &str = "RotatingMachine.GeneratingUnit";
const CGMES_REGULATING_CONTROL_PROPERTY: &str = "RegulatingCondEq.RegulatingControl";
const CGMES_SV_VOLTAGE_AUTHORITY_MISMATCH_PROPERTY: &str =
    "powerio.cgmes.sv_voltage_authority_mismatch";

#[derive(Debug, Clone)]
struct CgmesDiagnostic {
    info: &'static DiagnosticInfo,
    message: String,
}

impl std::ops::Deref for CgmesDiagnostic {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.message
    }
}

#[derive(Debug, Clone)]
struct CgmesDiagnostics {
    default_info: &'static DiagnosticInfo,
    records: Vec<CgmesDiagnostic>,
}

impl CgmesDiagnostics {
    fn new(default_info: &'static DiagnosticInfo) -> Self {
        Self {
            default_info,
            records: Vec::new(),
        }
    }

    fn push(&mut self, message: impl Into<String>) {
        self.push_as(self.default_info, message);
    }

    fn push_as(&mut self, info: &'static DiagnosticInfo, message: impl Into<String>) {
        self.records.push(CgmesDiagnostic {
            info,
            message: message.into(),
        });
    }

    fn extend(&mut self, other: Self) {
        self.records.extend(other.records);
    }
}

impl std::ops::Deref for CgmesDiagnostics {
    type Target = [CgmesDiagnostic];

    fn deref(&self) -> &Self::Target {
        &self.records
    }
}

impl IntoIterator for CgmesDiagnostics {
    type Item = CgmesDiagnostic;
    type IntoIter = std::vec::IntoIter<CgmesDiagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.records.into_iter()
    }
}

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
    read_documents(documents, name, diagnostics)
}

/// Read the documents, handing every diagnostic to `diagnostics` whether or
/// not the read succeeds, so a coded refusal reaches the caller with its error.
fn read_documents(
    documents: Vec<(String, String)>,
    name: Option<&str>,
    diagnostics: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    let mut warnings = CgmesDiagnostics::new(&codes::READ_CGMES_RECORD_UNMAPPED);
    let result = read::read_cgmes_documents_into(documents, name, &mut warnings);
    for diagnostic in warnings {
        diagnostics.push(diagnostic.info, diagnostic.message);
    }
    result
}

pub(crate) fn parse_text(
    name: &str,
    text: &str,
    diagnostics: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    reject_unsafe_xml(text.as_bytes())?;
    read_documents(
        vec![(name.to_string(), text.to_string())],
        None,
        diagnostics,
    )
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
        let size = file.size();
        let compressed = file.compressed_size();
        if size > MAX_BYTES || exceeds_compression_ratio(size, compressed) {
            return Err(format_error(format!(
                "archive entry {raw_name} exceeds the CGMES decompression limits"
            )));
        }
        let prefix_length = usize::try_from(size.min(4)).expect("four bytes fit usize");
        let mut prefix = vec![0_u8; prefix_length];
        file.read_exact(&mut prefix)
            .map_err(|error| format_error(format!("cannot decompress {raw_name}: {error}")))?;
        if is_zip_signature(&prefix) {
            return Err(format_error(format!(
                "archive {archive_name} contains nested archive {raw_name}"
            )));
        }
        if extension(path.as_str()) != Some("xml") {
            continue;
        }
        let remaining = MAX_BYTES.saturating_sub(*total);
        if size > remaining {
            return Err(format_error(
                "CGMES profile data exceeds the 64 MiB input limit",
            ));
        }
        let mut content = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        content.extend_from_slice(&prefix);
        file.by_ref()
            .take(remaining.saturating_sub(prefix.len() as u64) + 1)
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

fn exceeds_compression_ratio(size: u64, compressed: u64) -> bool {
    size > 0 && (compressed == 0 || size > compressed.saturating_mul(MAX_COMPRESSION_RATIO))
}

fn is_zip_signature(bytes: &[u8]) -> bool {
    matches!(
        bytes,
        [b'P', b'K', 0x03, 0x04, ..] | [b'P', b'K', 0x05, 0x06, ..] | [b'P', b'K', 0x07, 0x08, ..]
    )
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
    for diagnostic in output.warnings {
        diagnostics.push(diagnostic.info, diagnostic.message);
    }
    Ok((artifacts, diagnostics))
}

/// The CGMES release family a file set declares, from its `cim` namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CgmesVersion {
    /// CGMES 2.4.15 on CIM16 (`.../CIM-schema-cim16#`, any vintage year).
    V2_4_15,
    /// CGMES 3.0 on CIM100 (`.../CIM100#`).
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
        AcDcConverterControlMode, ActivePowerControl, Area, BoundaryLine, Branch,
        BranchCurrentRatings, BranchSolution, Bus, BusBreakerBus, BusId, BusType, BusbarSection,
        CalculatedBus, CaseMetadata, ComponentMetadata, ConnectivityNode, CurveStyle, DcBusbar,
        DcConverterOperatingMode, DcConverterUnit, DcGround, DcLine, DcNode, DcPolarity,
        DcSeriesDevice, DcSwitch, DcSwitchKind, DcTerminal, DcTopologicalNode,
        DetailedConnectivity, EquipmentReactiveLimits, ExternalIdentifier, Generator,
        GeneratorEnergySource, Hvdc, Impedance, Junction, LineCommutatedConverter,
        LineCommutatedConverterOperatingMode, LineCommutatedConverterReactiveModel, Load,
        LoadVoltageModel, LoadingLimits, MinMaxReactiveLimits, OmittedField, OmittedFieldName,
        OperationalLimitGroup, ReactiveCapabilityCurve, ReactiveCapabilityCurvePoint,
        ReactiveLimits, Shunt, ShuntBlock, StaticVarCompensator,
        StaticVarCompensatorRegulationMode, Subnetwork, Substation, Switch, SwitchKind,
        SwitchedShuntControl, SwitchedShuntMode, TapChanger, TapChangerKind,
        TapChangerRegulationMode, TapChangerStep, TemporaryLimit, Terminal, TerminalReference,
        TieLine, TopologyEndpoint, TopologyKind, TopologySwitch, Transformer3W, VoltageLevel,
        VoltageSourceConverter, Winding,
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

    #[test]
    fn derived_cgmes_limits_report_separate_current_ratings() {
        let mut network = network();
        network.branches_mut()[0].rate_a = 100.0;
        network.branches_mut()[0].current_ratings =
            Some(BranchCurrentRatings::new(500.0, 600.0, 700.0));

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(output.warnings.iter().any(|warning| {
            warning.info.code == codes::EMIT_CGMES.field_dropped.code
                && warning.contains("current rating record dropped")
        }));
    }

    #[test]
    fn a_v2415_permanent_limit_states_no_acceptable_duration() {
        let mut network = network();
        network.branches_mut()[0].rate_a = 100.0;

        let output = write::write_cgmes(&network, CgmesVersion::V2_4_15).unwrap();
        let xml = output
            .files
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let permanent = xml
            .split("<cim:OperationalLimitType ")
            .nth(1)
            .and_then(|text| text.split("</cim:OperationalLimitType>").next())
            .expect("a positive rate_a emits one limit type");
        assert!(!permanent.contains("<cim:IdentifiedObject.name>"));
        assert!(!permanent.contains("acceptableDuration"), "{permanent}");
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

    fn voltage_limit_documents(
        version: CgmesVersion,
        voltage_level_limits: (f64, f64),
        operational_limits: (f64, f64),
    ) -> Vec<(String, String)> {
        const VOLTAGE_LEVEL_MRID: &str = "10000000-0000-4000-8000-000000000001";
        const BUSBAR_MRID: &str = "10000000-0000-4000-8000-000000000002";

        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.voltage_levels[0].low_voltage_limit_kv = None;
        detailed.voltage_levels[0].high_voltage_limit_kv = None;
        for (component, mrid) in [
            (&detailed.voltage_levels[0].component, VOLTAGE_LEVEL_MRID),
            (&detailed.busbar_sections[0].component, BUSBAR_MRID),
        ] {
            detailed
                .component_metadata
                .iter_mut()
                .find(|metadata| metadata.component == *component)
                .unwrap()
                .external_identifiers = vec![ExternalIdentifier {
                value: mrid.into(),
                authority: Some("CGMES".into()),
            }];
        }

        let (kind_property, kind_namespace, value_property) = match version {
            CgmesVersion::V2_4_15 => (
                "entsoe:OperationalLimitType.limitType",
                "http://entsoe.eu/CIM/SchemaExtension/3/1#LimitTypeKind",
                "VoltageLimit.value",
            ),
            CgmesVersion::V3_0 => (
                "eu:OperationalLimitType.kind",
                "http://iec.ch/TC57/CIM100-European#LimitKind",
                "VoltageLimit.normalValue",
            ),
        };
        let (level_low, level_high) = voltage_level_limits;
        let (operational_low, operational_high) = operational_limits;
        let records = format!(
            r##"  <cim:VoltageLevel rdf:about="#_{VOLTAGE_LEVEL_MRID}">
    <cim:VoltageLevel.lowVoltageLimit>{level_low}</cim:VoltageLevel.lowVoltageLimit>
    <cim:VoltageLevel.highVoltageLimit>{level_high}</cim:VoltageLevel.highVoltageLimit>
  </cim:VoltageLevel>
  <cim:OperationalLimitSet rdf:ID="_voltage-limit-set">
    <cim:OperationalLimitSet.Equipment rdf:resource="#_{BUSBAR_MRID}"/>
  </cim:OperationalLimitSet>
  <cim:OperationalLimitType rdf:ID="_low-voltage-limit-type">
    <{kind_property} rdf:resource="{kind_namespace}.lowVoltage"/>
  </cim:OperationalLimitType>
  <cim:OperationalLimitType rdf:ID="_high-voltage-limit-type">
    <{kind_property} rdf:resource="{kind_namespace}.highVoltage"/>
  </cim:OperationalLimitType>
  <cim:VoltageLimit rdf:ID="_low-voltage-limit">
    <cim:OperationalLimit.OperationalLimitSet rdf:resource="#_voltage-limit-set"/>
    <cim:OperationalLimit.OperationalLimitType rdf:resource="#_low-voltage-limit-type"/>
    <cim:{value_property}>{operational_low}</cim:{value_property}>
  </cim:VoltageLimit>
  <cim:VoltageLimit rdf:ID="_high-voltage-limit">
    <cim:OperationalLimit.OperationalLimitSet rdf:resource="#_voltage-limit-set"/>
    <cim:OperationalLimit.OperationalLimitType rdf:resource="#_high-voltage-limit-type"/>
    <cim:{value_property}>{operational_high}</cim:{value_property}>
  </cim:VoltageLimit>
"##
        );
        let documents = write::write_cgmes(&network, version).unwrap().files;
        insert_profile_records(documents, &records, None, None)
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
                component: None,
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
            omitted_fields: Vec::new(),
            component_metadata: named_components
                .into_iter()
                .map(|(component, name)| ComponentMetadata {
                    component: component.clone(),
                    name: Some(name.into()),
                    equipment_container: None,
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
            junctions: Vec::new(),
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

    fn mixed_topology_network() -> BalancedNetwork {
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let second_level = component("voltage_level", "vl-B");
        let second_bus = component("bus", "tn-B");
        let second_node = component("connectivity_node", "node-B");
        let switch = component("switch", "breaker-A");
        let substation = detailed.voltage_levels[0].substation.clone();
        detailed.voltage_levels[0]
            .buses
            .retain(|bus| *bus != BusId(2));
        detailed.voltage_levels.push(VoltageLevel {
            component: second_level.clone(),
            substation,
            nominal_kv: 230.0,
            low_voltage_limit_kv: Some(210.0),
            high_voltage_limit_kv: Some(250.0),
            topology_kind: TopologyKind::BusBreaker,
            buses: vec![BusId(2)],
        });
        detailed.component_metadata.push(ComponentMetadata {
            component: second_level.clone(),
            name: Some("South 230 kV".into()),
            equipment_container: None,
            aliases: Vec::new(),
            external_identifiers: Vec::new(),
            properties: std::collections::BTreeMap::new(),
            fictitious: false,
        });
        detailed.bus_breaker_buses[1].voltage_level = second_level.clone();
        detailed
            .connectivity_nodes
            .retain(|node| node.component != second_node);
        detailed.switches.clear();
        detailed
            .terminals
            .retain(|terminal| terminal.equipment != switch);
        for terminal in detailed
            .terminals
            .iter_mut()
            .filter(|terminal| terminal.bus.as_ref() == Some(&second_bus))
        {
            terminal.voltage_level = second_level.clone();
            terminal.node = None;
        }
        network.validate().unwrap();
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
        assert_eq!(
            parsed
                .network
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .substations
                .len(),
            2
        );
        assert!(parsed.network.case_metadata().source_model_format.is_none());
        assert!(parsed.warnings.is_empty(), "{:#?}", parsed.warnings);

        let emitted_again = write::write_cgmes(&parsed.network, CgmesVersion::V3_0).unwrap();
        let parsed_again = read::read_cgmes_documents(emitted_again.files, Some("case")).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed.network).unwrap(),
            serde_json::to_value(&parsed_again.network).unwrap()
        );
        assert!(
            parsed_again.warnings.is_empty(),
            "{:#?}",
            parsed_again.warnings
        );
    }

    #[test]
    fn simple_hvdc_reports_the_missing_physical_cgmes_data() {
        let mut network = detailed_network();
        let mut line = Hvdc::new(BusId(1), BusId(2));
        line.uid = Some("dc-link".into());
        line.resistance_ohm = Some(2.5);
        line.nominal_voltage_kv = Some(320.0);
        network.hvdc_mut().push(line);

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(output.warnings.iter().any(|warning| {
            warning.contains("HVDC line `dc-link` is a two terminal calculation record")
                && warning.contains("DCConverterUnit operating mode and containment")
                && warning.contains("DC node and terminal polarity identities")
                && warning.contains("without inventing data")
        }));
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
    fn synchronous_machine_reactive_capability_curve_round_trips() {
        let mut network = detailed_network();
        network.generators_mut()[0].pg = 50.0;
        network.generators_mut()[0].qmin = -1.0;
        network.generators_mut()[0].qmax = 1.0;
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let generator = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.component_type() == "generator")
            .map(|terminal| terminal.equipment.clone())
            .unwrap();
        let limits = ReactiveLimits::CapabilityCurve(ReactiveCapabilityCurve {
            curve_style: CurveStyle::StraightLineYValues,
            properties: std::collections::BTreeMap::new(),
            points: vec![
                ReactiveCapabilityCurvePoint {
                    active_power_mw: -100.0,
                    minimum_reactive_power_mvar: -20.0,
                    maximum_reactive_power_mvar: 20.0,
                    properties: std::collections::BTreeMap::new(),
                },
                ReactiveCapabilityCurvePoint {
                    active_power_mw: 100.0,
                    minimum_reactive_power_mvar: -40.0,
                    maximum_reactive_power_mvar: 40.0,
                    properties: std::collections::BTreeMap::new(),
                },
            ],
        });
        detailed
            .equipment_reactive_limits
            .push(EquipmentReactiveLimits {
                equipment: generator,
                limits: limits.clone(),
            });

        let first = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let second = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert_eq!(first.files, second.files);
        let eq = first
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(eq.contains("SynchronousMachine.InitialReactiveCapabilityCurve"));
        assert!(eq.contains("<cim:ReactiveCapabilityCurve rdf:ID="));
        assert_eq!(eq.matches("<cim:CurveData rdf:ID=").count(), 2);
        assert!(eq.contains("<cim:CurveData.xvalue>-100</cim:CurveData.xvalue>"));
        assert!(eq.contains("<cim:CurveData.y1value>-40</cim:CurveData.y1value>"));
        assert!(eq.contains("<cim:SynchronousMachine.minQ>-35</cim:SynchronousMachine.minQ>"));
        assert!(eq.contains("<cim:SynchronousMachine.maxQ>35</cim:SynchronousMachine.maxQ>"));
        assert!(first.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::EMIT_CGMES.value_substituted.code
                && warning.contains("typed reactive limits evaluate to minQ=-35")
                && warning.contains("balanced generator row contains minQ=-1")
        }));

        let parsed = read::read_cgmes_documents(first.files, Some("machine-curve")).unwrap();
        let machine = &parsed.network.generators()[0];
        assert!((machine.qmin - -35.0).abs() < 1e-12);
        assert!((machine.qmax - 35.0).abs() < 1e-12);
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.equipment_reactive_limits.len(), 1);
        assert_eq!(detailed.equipment_reactive_limits[0].limits, limits);
        assert!(!parsed.warnings.iter().any(|warning| {
            warning.contains("ReactiveCapabilityCurve object(s)")
                || warning.contains("CurveData object(s)")
        }));
    }

    #[test]
    fn typed_min_max_reactive_limits_override_the_balanced_generator_projection() {
        let mut network = detailed_network();
        network.generators_mut()[0].qmin = -1.0;
        network.generators_mut()[0].qmax = 1.0;
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let generator = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.component_type() == "generator")
            .map(|terminal| terminal.equipment.clone())
            .unwrap();
        let mut properties = std::collections::BTreeMap::new();
        properties.insert("source_extension".into(), "retained".into());
        detailed
            .equipment_reactive_limits
            .push(EquipmentReactiveLimits {
                equipment: generator,
                limits: ReactiveLimits::MinMax(MinMaxReactiveLimits {
                    minimum_reactive_power_mvar: -25.0,
                    maximum_reactive_power_mvar: 30.0,
                    properties,
                }),
            });
        detailed
            .equipment_reactive_limits
            .push(EquipmentReactiveLimits {
                equipment: component("storage", "not-emitted"),
                limits: ReactiveLimits::MinMax(MinMaxReactiveLimits {
                    minimum_reactive_power_mvar: -5.0,
                    maximum_reactive_power_mvar: 5.0,
                    properties: std::collections::BTreeMap::new(),
                }),
            });

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(eq.contains("<cim:SynchronousMachine.minQ>-25</cim:SynchronousMachine.minQ>"));
        assert!(eq.contains("<cim:SynchronousMachine.maxQ>30</cim:SynchronousMachine.maxQ>"));
        assert!(output.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::EMIT_CGMES.value_substituted.code
                && warning.contains("typed reactive limits evaluate to minQ=-25")
        }));
        assert!(output.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::EMIT_CGMES.field_dropped.code
                && warning.contains("min/max reactive limits properties")
        }));
        assert!(output.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::EMIT_CGMES.record_dropped.code
                && warning.contains("storage/not-emitted")
                && warning.contains("no CGMES record states the storage")
        }));

        let parsed = read::read_cgmes_documents(output.files, Some("machine-min-max")).unwrap();
        assert!((parsed.network.generators()[0].qmin + 25.0).abs() < 1e-12);
        assert!((parsed.network.generators()[0].qmax - 30.0).abs() < 1e-12);
    }

    #[test]
    fn sv_status_overrides_ssh_and_round_trips() {
        let mut network = detailed_network();
        network.loads_mut()[0].in_service = false;
        network.generators_mut()[0].in_service = false;
        network.branches_mut()[0].in_service = false;
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let busbar = detailed.busbar_sections[0].component.clone();
        detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| metadata.component == busbar)
            .unwrap()
            .properties
            .insert(CGMES_SV_STATUS_PROPERTY.into(), "false".into());

        let mut output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let sv = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert_eq!(sv.matches("<cim:SvStatus rdf:ID=").count(), 4);
        assert_eq!(
            sv.matches("<cim:SvStatus.inService>false</cim:SvStatus.inService>")
                .count(),
            4
        );

        let ssh = output
            .files
            .iter_mut()
            .find(|(name, _)| name.ends_with("_SSH.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        *ssh = ssh.replace(
            "<cim:Equipment.inService>false</cim:Equipment.inService>",
            "<cim:Equipment.inService>true</cim:Equipment.inService>",
        );
        let parsed = read::read_cgmes_documents(output.files.clone(), Some("sv-status")).unwrap();
        assert!(!parsed.network.loads()[0].in_service);
        assert!(!parsed.network.generators()[0].in_service);
        assert!(!parsed.network.branches()[0].in_service);
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert!(detailed.component_metadata.iter().any(|metadata| {
            metadata.component.component_type() == "busbar_section"
                && metadata
                    .properties
                    .get(CGMES_SV_STATUS_PROPERTY)
                    .is_some_and(|value| value == "false")
        }));
        assert!(
            !parsed
                .warnings
                .iter()
                .any(|warning| warning.contains("SvStatus object(s)"))
        );

        let sv = output
            .files
            .iter_mut()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        *sv = sv.replacen(
            "SvStatus.ConductingEquipment",
            "SvStatus.MissingConductingEquipment",
            1,
        );
        let Err(error) = read::read_cgmes_documents(output.files, Some("invalid-sv-status")) else {
            panic!("SvStatus without a conducting equipment reference parsed");
        };
        assert!(
            error
                .to_string()
                .contains("has no SvStatus.ConductingEquipment reference")
        );
    }

    const MAPPED_GENERATING_UNIT_MRID: &str = "11111111-1111-4111-8111-111111111111";

    fn network_with_mapped_equipment_metadata() -> BalancedNetwork {
        let mut network = detailed_network();
        let load = network.loads()[0].uid.clone().unwrap();
        let generator = network.generators()[0].uid.clone().unwrap();
        let branch = network.branches()[0].uid.clone().unwrap();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let voltage_level = detailed.voltage_levels[0].component.clone();
        let substation = detailed.substations[0].component.clone();
        for (component_type, local_id, name) in [
            ("load", load.as_str(), "Retained load name"),
            ("generator", generator.as_str(), "Retained generator name"),
            ("branch", branch.as_str(), "Retained line name"),
        ] {
            detailed.component_metadata.push(ComponentMetadata {
                component: component(component_type, local_id),
                name: Some(name.into()),
                equipment_container: Some(voltage_level.clone()),
                aliases: Vec::new(),
                external_identifiers: Vec::new(),
                properties: std::collections::BTreeMap::new(),
                fictitious: false,
            });
        }
        detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| {
                metadata.component.component_type() == "generator"
                    && metadata.component.local_id() == generator
            })
            .unwrap()
            .properties
            .insert(
                CGMES_GENERATING_UNIT_PROPERTY.into(),
                MAPPED_GENERATING_UNIT_MRID.into(),
            );
        detailed.component_metadata.push(ComponentMetadata {
            component: component("cgmes_object", MAPPED_GENERATING_UNIT_MRID),
            name: Some("Retained generating unit name".into()),
            equipment_container: Some(substation),
            aliases: Vec::new(),
            external_identifiers: vec![ExternalIdentifier {
                value: MAPPED_GENERATING_UNIT_MRID.into(),
                authority: Some("CGMES".into()),
            }],
            properties: std::collections::BTreeMap::from([
                (
                    "IdentifiedObject.description".into(),
                    "Retained unit description".into(),
                ),
                ("Equipment.aggregate".into(), "false".into()),
                (
                    "GeneratingUnit.genControlSource".into(),
                    "GeneratorControlSource.offAGC".into(),
                ),
                ("GeneratingUnit.nominalP".into(), "125".into()),
            ]),
            fictitious: false,
        });
        network.validate().unwrap();
        network
    }

    #[test]
    fn mapped_equipment_names_and_containers_round_trip() {
        let network = network_with_mapped_equipment_metadata();
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let equipment = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(equipment.contains(&format!(
            "<cim:GeneratingUnit rdf:ID=\"_{MAPPED_GENERATING_UNIT_MRID}\">"
        )));
        assert!(equipment.contains(
            "<cim:IdentifiedObject.name>Retained generating unit name</cim:IdentifiedObject.name>"
        ));
        assert!(equipment.contains(
            "<cim:IdentifiedObject.description>Retained unit description</cim:IdentifiedObject.description>"
        ));
        assert!(
            equipment.contains("<cim:GeneratingUnit.nominalP>125</cim:GeneratingUnit.nominalP>")
        );
        let reparsed = read::read_cgmes_documents(output.files, Some("mapped-metadata")).unwrap();
        let detailed = reparsed.network.detailed_connectivity().as_deref().unwrap();
        for name in [
            "Retained load name",
            "Retained generator name",
            "Retained line name",
        ] {
            let metadata = detailed
                .component_metadata
                .iter()
                .find(|metadata| metadata.name.as_deref() == Some(name))
                .unwrap();
            let container = metadata.equipment_container.as_ref().unwrap();
            assert!(detailed.voltage_levels.iter().any(|level| {
                level.component == *container
                    && detailed.component_metadata.iter().any(|metadata| {
                        metadata.component == level.component
                            && metadata.name.as_deref() == Some("North 230 kV")
                    })
            }));
        }
        let unit = detailed
            .component_metadata
            .iter()
            .find(|metadata| metadata.name.as_deref() == Some("Retained generating unit name"))
            .unwrap();
        let container = unit.equipment_container.as_ref().unwrap();
        assert!(detailed.substations.iter().any(|substation| {
            substation.component == *container
                && detailed.component_metadata.iter().any(|metadata| {
                    metadata.component == substation.component
                        && metadata.name.as_deref() == Some("North substation")
                })
        }));
    }

    #[test]
    fn generating_unit_classes_round_trip_as_generator_energy_sources() {
        for (energy_source, class) in [
            (GeneratorEnergySource::Hydro, "HydroGeneratingUnit"),
            (GeneratorEnergySource::Nuclear, "NuclearGeneratingUnit"),
            (GeneratorEnergySource::Wind, "WindGeneratingUnit"),
            (GeneratorEnergySource::Thermal, "ThermalGeneratingUnit"),
            (GeneratorEnergySource::Solar, "SolarGeneratingUnit"),
            (GeneratorEnergySource::Other, "GeneratingUnit"),
        ] {
            let mut network = network();
            network.generators_mut()[0].energy_source = energy_source;
            let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
            let equipment = output
                .files
                .iter()
                .find(|(name, _)| name.ends_with("_EQ.xml"))
                .map(|(_, xml)| xml)
                .unwrap();
            let steady_state_hypothesis = output
                .files
                .iter()
                .find(|(name, _)| name.ends_with("_SSH.xml"))
                .map(|(_, xml)| xml)
                .unwrap();
            assert!(equipment.contains(&format!("<cim:{class} rdf:ID=")));
            assert!(steady_state_hypothesis.contains(&format!("<cim:{class} rdf:about=")));

            let parsed = read::read_cgmes_documents(output.files, Some("energy-source")).unwrap();
            assert_eq!(parsed.network.generators()[0].energy_source, energy_source);
            let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
            assert!(detailed.component_metadata.iter().any(|metadata| {
                metadata
                    .properties
                    .get(CGMES_CLASS_PROPERTY)
                    .is_some_and(|value| value == class)
            }));
        }
    }

    #[test]
    fn dangling_component_metadata_container_is_rejected() {
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.component_metadata[0].equipment_container =
            Some(component("voltage_level", "missing"));
        let error = network.validate().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("references unknown equipment container `voltage_level/missing`")
        );
    }

    #[test]
    fn fresh_emission_replaces_an_unsupported_container_with_the_terminal_voltage_level() {
        let mut network = detailed_network();
        let load = component("load", network.loads()[0].uid.as_deref().unwrap());
        let unsupported_container = component("cgmes_object", "bay-A");
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.component_metadata.push(ComponentMetadata {
            component: unsupported_container.clone(),
            name: Some("Bay A".into()),
            equipment_container: None,
            aliases: Vec::new(),
            external_identifiers: Vec::new(),
            properties: std::collections::BTreeMap::new(),
            fictitious: false,
        });
        detailed.component_metadata.push(ComponentMetadata {
            component: load,
            name: Some("North load".into()),
            equipment_container: Some(unsupported_container),
            aliases: Vec::new(),
            external_identifiers: Vec::new(),
            properties: std::collections::BTreeMap::new(),
            fictitious: false,
        });
        network.validate().unwrap();

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(output.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::EMIT_CGMES.value_substituted.code
                && warning.contains("load/")
                && warning.contains("cgmes_object/bay-A")
                && warning.contains("terminal's VoltageLevel")
        }));

        let parsed = read::read_cgmes_documents(output.files, Some("container-fallback")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        let load = detailed
            .component_metadata
            .iter()
            .find(|metadata| metadata.name.as_deref() == Some("North load"))
            .unwrap();
        let container = load.equipment_container.as_ref().unwrap();
        assert_eq!(container.component_type(), "voltage_level");
        assert!(
            detailed
                .voltage_levels
                .iter()
                .any(|level| level.component == *container)
        );
    }

    #[test]
    fn dc_only_detailed_connectivity_emits_declared_substations() {
        let substation = component("substation", "dc-substation");
        let converter_unit = component("dc_converter_unit", "dc-unit");
        let mut network = BalancedNetwork::new("dc-only", 100.0);
        *network.detailed_connectivity_mut() = Some(Arc::new(DetailedConnectivity {
            component_metadata: vec![ComponentMetadata {
                component: substation.clone(),
                name: Some("DC substation".into()),
                equipment_container: None,
                aliases: Vec::new(),
                external_identifiers: Vec::new(),
                properties: std::collections::BTreeMap::new(),
                fictitious: false,
            }],
            substations: vec![Substation {
                component: substation,
                country: None,
                operator: None,
                geographical_tags: Vec::new(),
            }],
            dc_converter_units: vec![DcConverterUnit {
                component: converter_unit,
                substation: Some(component("substation", "dc-substation")),
                operation_mode: DcConverterOperatingMode::Bipolar,
            }],
            ..DetailedConnectivity::default()
        }));

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let equipment = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(equipment.contains("<cim:Substation rdf:ID="));
        assert!(equipment.contains("<cim:IdentifiedObject.name>DC substation"));
        assert!(equipment.contains("<cim:DCConverterUnit.Substation rdf:resource="));
    }

    #[test]
    fn busbar_containment_conflicts_are_rejected_and_switch_terminals_may_cross_containers() {
        let add_second_level = |network: &mut BalancedNetwork| {
            let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
            let second_level = component("voltage_level", "vl-B");
            let second_node = component("connectivity_node", "node-C");
            detailed.voltage_levels.push(VoltageLevel {
                component: second_level.clone(),
                substation: detailed.voltage_levels[0].substation.clone(),
                nominal_kv: 230.0,
                low_voltage_limit_kv: None,
                high_voltage_limit_kv: None,
                topology_kind: TopologyKind::NodeBreaker,
                buses: Vec::new(),
            });
            detailed.connectivity_nodes.push(ConnectivityNode {
                component: second_node.clone(),
                voltage_level: second_level,
                node_number: None,
                calculated_bus: None,
            });
            second_node
        };

        let mut busbar = detailed_network();
        let second_node = add_second_level(&mut busbar);
        Arc::make_mut(busbar.detailed_connectivity_mut().as_mut().unwrap()).busbar_sections[0]
            .node = second_node;
        assert!(
            busbar
                .validate()
                .unwrap_err()
                .to_string()
                .contains("busbar section")
        );

        let mut switch = detailed_network();
        let second_node = add_second_level(&mut switch);
        Arc::make_mut(switch.detailed_connectivity_mut().as_mut().unwrap()).switches[0].endpoint2 =
            TopologyEndpoint::Node(second_node);
        switch.validate().unwrap();
    }

    #[test]
    fn cim_junction_round_trips_with_name_container_and_terminal() {
        const JUNCTION_MRID: &str = "5249a78f-6642-4fc5-968f-06e2ed18fab7";
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let junction = component("junction", JUNCTION_MRID);
        let container = detailed.voltage_levels[0].component.clone();
        detailed.component_metadata.push(ComponentMetadata {
            component: junction.clone(),
            name: Some("Junction XJ1".into()),
            equipment_container: Some(container.clone()),
            aliases: Vec::new(),
            external_identifiers: vec![ExternalIdentifier {
                value: JUNCTION_MRID.into(),
                authority: Some("CGMES".into()),
            }],
            properties: std::collections::BTreeMap::new(),
            fictitious: false,
        });
        detailed.junctions.push(Junction {
            component: junction.clone(),
        });
        let mut terminal = detailed.terminals[0].clone();
        terminal.equipment = junction;
        terminal.terminal = 1;
        detailed.terminals.push(terminal);
        network.validate().unwrap();

        let network = crate::network::serde_round_trip(&network);
        for version in [CgmesVersion::V2_4_15, CgmesVersion::V3_0] {
            let output = write::write_cgmes(&network, version).unwrap();
            let equipment = output
                .files
                .iter()
                .find(|(name, _)| name.ends_with("_EQ.xml"))
                .map(|(_, xml)| xml)
                .unwrap();
            assert!(equipment.contains(&format!("<cim:Junction rdf:ID=\"_{JUNCTION_MRID}\">")));
            assert!(
                equipment.contains(
                    "<cim:IdentifiedObject.name>Junction XJ1</cim:IdentifiedObject.name>"
                )
            );

            let parsed = read::read_cgmes_documents(output.files, Some("junction")).unwrap();
            let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
            assert_eq!(detailed.junctions.len(), 1);
            let junction = &detailed.junctions[0].component;
            assert!(
                detailed
                    .terminals
                    .iter()
                    .any(|terminal| { terminal.equipment == *junction && terminal.terminal == 1 })
            );
            let metadata = detailed
                .component_metadata
                .iter()
                .find(|metadata| metadata.component == *junction)
                .unwrap();
            assert_eq!(metadata.name.as_deref(), Some("Junction XJ1"));
            let container = metadata.equipment_container.as_ref().unwrap();
            assert!(detailed.component_metadata.iter().any(|metadata| {
                metadata.component == *container && metadata.name.as_deref() == Some("North 230 kV")
            }));
        }
    }

    #[test]
    fn v2415_busbar_without_connectivity_node_is_preserved() {
        let network = detailed_network();
        let mut output = write::write_cgmes(&network, CgmesVersion::V2_4_15).unwrap();
        let eq = output
            .files
            .iter_mut()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        let connection = eq
            .lines()
            .find(|line| line.contains("<cim:Terminal.ConnectivityNode "))
            .unwrap()
            .to_owned();
        *eq = eq.replacen(&format!("{connection}\n"), "", 1);

        let parsed = read::read_cgmes_documents(output.files, Some("v2415-busbar")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(detailed.busbar_sections.len(), 1);
        let busbar = &detailed.busbar_sections[0];
        let node = detailed
            .connectivity_nodes
            .iter()
            .find(|node| node.component == busbar.node)
            .unwrap();
        assert!(node.component.local_id().starts_with("terminal-"));
        assert_eq!(node.calculated_bus, Some(BusId(1)));

        let fresh = write::write_cgmes(&parsed.network, CgmesVersion::V3_0).unwrap();
        let eq = fresh
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(eq.contains("<cim:IdentifiedObject.name>North busbar</cim:IdentifiedObject.name>"));
        assert!(eq.contains("<cim:Equipment.EquipmentContainer rdf:resource="));
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
                equipment_container: None,
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
    fn disconnected_terminal_with_a_calculated_bus_omits_its_tp_association() {
        const TERMINAL_MRID: &str = "89000000-0000-4000-8000-000000000001";
        let mut network = detailed_network();
        let terminal_component = component("terminal", TERMINAL_MRID);
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let terminal = detailed
            .terminals
            .iter_mut()
            .find(|terminal| terminal.equipment.component_type() == "load")
            .unwrap();
        assert!(terminal.node.is_some());
        terminal.component = Some(terminal_component.clone());
        terminal.connected = false;
        detailed.component_metadata.push(ComponentMetadata {
            component: terminal_component,
            name: Some("Disconnected load terminal".into()),
            equipment_container: None,
            aliases: Vec::new(),
            external_identifiers: vec![ExternalIdentifier {
                value: TERMINAL_MRID.into(),
                authority: Some("CGMES".into()),
            }],
            properties: std::collections::BTreeMap::new(),
            fictitious: false,
        });

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let profile = |suffix: &str| {
            output
                .files
                .iter()
                .find(|(name, _)| name.ends_with(suffix))
                .map(|(_, text)| text.as_str())
                .unwrap()
        };
        let terminal_id = format!("_{TERMINAL_MRID}");
        assert!(profile("_EQ.xml").contains(&format!("rdf:ID=\"{terminal_id}\"")));
        assert!(!profile("_TP.xml").contains(&format!("rdf:about=\"#{terminal_id}\"")));
        assert!(profile("_SSH.xml").contains(&format!("rdf:about=\"#{terminal_id}\"")));
        assert!(
            profile("_SSH.xml")
                .contains("<cim:ACDCTerminal.connected>false</cim:ACDCTerminal.connected>")
        );

        let parsed =
            read::read_cgmes_documents(output.files, Some("disconnected-terminal")).unwrap();
        let terminal = parsed
            .network
            .detailed_connectivity()
            .as_deref()
            .unwrap()
            .terminals
            .iter()
            .find(|terminal| {
                terminal
                    .component
                    .as_ref()
                    .is_some_and(|component| component.local_id() == TERMINAL_MRID)
            })
            .unwrap();
        assert!(!terminal.connected);
        assert!(terminal.bus.is_none());
        assert!(terminal.node.is_some());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn disconnected_detailed_equipment_round_trips_without_a_topological_node() {
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let voltage_level = detailed.voltage_levels[0].component.clone();
        let first_node = component("connectivity_node", "81000000-0000-4000-8000-000000000001");
        let second_node = component("connectivity_node", "81000000-0000-4000-8000-000000000002");
        let busbar = component("busbar_section", "82000000-0000-4000-8000-000000000001");
        let junction = component("junction", "83000000-0000-4000-8000-000000000001");
        let switch = component("switch", "84000000-0000-4000-8000-000000000001");
        let terminal_ids = [
            component("terminal", "85000000-0000-4000-8000-000000000001"),
            component("terminal", "85000000-0000-4000-8000-000000000002"),
            component("terminal", "85000000-0000-4000-8000-000000000003"),
            component("terminal", "85000000-0000-4000-8000-000000000004"),
        ];

        detailed.connectivity_nodes.extend([
            ConnectivityNode {
                component: first_node.clone(),
                voltage_level: voltage_level.clone(),
                node_number: None,
                calculated_bus: None,
            },
            ConnectivityNode {
                component: second_node.clone(),
                voltage_level: voltage_level.clone(),
                node_number: None,
                calculated_bus: None,
            },
        ]);
        detailed.busbar_sections.push(BusbarSection {
            component: busbar.clone(),
            voltage_level: voltage_level.clone(),
            node: first_node.clone(),
        });
        detailed.junctions.push(Junction {
            component: junction.clone(),
        });
        detailed.switches.push(TopologySwitch {
            component: switch.clone(),
            voltage_level: voltage_level.clone(),
            kind: SwitchKind::Breaker,
            endpoint1: TopologyEndpoint::Node(first_node.clone()),
            endpoint2: TopologyEndpoint::Node(second_node.clone()),
            open: true,
            retained: true,
        });
        let disconnected_terminal =
            |component: ComponentId, equipment: ComponentId, terminal: u8, node: ComponentId| {
                Terminal {
                    component: Some(component),
                    equipment,
                    terminal,
                    voltage_level: voltage_level.clone(),
                    bus: None,
                    connectable_bus: None,
                    node: Some(node),
                    connected: false,
                    active_power_mw: None,
                    reactive_power_mvar: None,
                }
            };
        detailed.terminals.extend([
            disconnected_terminal(
                terminal_ids[0].clone(),
                busbar.clone(),
                1,
                first_node.clone(),
            ),
            disconnected_terminal(
                terminal_ids[1].clone(),
                junction.clone(),
                1,
                second_node.clone(),
            ),
            disconnected_terminal(terminal_ids[2].clone(), switch.clone(), 1, first_node),
            disconnected_terminal(terminal_ids[3].clone(), switch.clone(), 2, second_node),
        ]);
        for value in [&busbar, &junction, &switch]
            .into_iter()
            .chain(terminal_ids.iter())
        {
            detailed.component_metadata.push(ComponentMetadata {
                component: value.clone(),
                name: Some(value.local_id().into()),
                equipment_container: None,
                aliases: Vec::new(),
                external_identifiers: vec![ExternalIdentifier {
                    value: value.local_id().into(),
                    authority: Some("CGMES".into()),
                }],
                properties: std::collections::BTreeMap::new(),
                fictitious: false,
            });
        }

        network.validate().unwrap();
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(
            !output
                .warnings
                .iter()
                .any(|warning| warning.contains("not emitted"))
        );
        let all_xml = output
            .files
            .iter()
            .map(|(_, xml)| xml.as_str())
            .collect::<String>();
        for value in [&busbar, &junction, &switch] {
            assert!(all_xml.contains(value.local_id()));
        }
        assert_eq!(
            all_xml.matches("<cim:ACDCTerminal.connected>false").count(),
            4
        );

        let parsed = read::read_cgmes_documents(output.files, Some("disconnected")).unwrap();
        let parsed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert!(
            parsed
                .busbar_sections
                .iter()
                .any(|value| value.component.local_id() == busbar.local_id())
        );
        assert!(
            parsed
                .junctions
                .iter()
                .any(|value| value.component.local_id() == junction.local_id())
        );
        assert!(
            parsed
                .switches
                .iter()
                .any(|value| value.component.local_id() == switch.local_id())
        );
        for id in &terminal_ids {
            assert!(parsed.terminals.iter().any(|terminal| {
                terminal
                    .component
                    .as_ref()
                    .is_some_and(|value| value.local_id() == id.local_id())
                    && !terminal.connected
            }));
        }
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
                equipment_container: None,
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
                equipment_container: None,
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
    fn missing_balanced_terminal_records_get_generated_connectivity_nodes() {
        for (equipment_type, terminal_number) in [("load", 1), ("branch", 2)] {
            let mut network = detailed_network();
            let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
            detailed.terminals.retain(|terminal| {
                terminal.equipment.component_type() != equipment_type
                    || usize::from(terminal.terminal) != terminal_number
            });
            network.validate().unwrap();

            for version in [CgmesVersion::V2_4_15, CgmesVersion::V3_0] {
                let output = write::write_cgmes(&network, version).unwrap();
                assert!(output.warnings.iter().any(|warning| {
                    warning.contains("balanced equipment terminal at bus 2")
                        && warning.contains("generated ConnectivityNode")
                }));
                let parsed =
                    read::read_cgmes_documents(output.files, Some("missing-balanced-terminal"))
                        .unwrap();
                let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
                let terminal = detailed
                    .terminals
                    .iter()
                    .find(|terminal| {
                        terminal.equipment.component_type() == equipment_type
                            && usize::from(terminal.terminal) == terminal_number
                    })
                    .unwrap();
                let node = terminal.node.as_ref().unwrap();
                assert!(detailed.connectivity_nodes.iter().any(|candidate| {
                    candidate.component == *node && candidate.calculated_bus == Some(BusId(2))
                }));
            }
        }
    }

    /// A CGMES BaseVoltage states a positive nominal voltage and a per-unit
    /// only case states none, so the substituted kilovolt is declared. It is
    /// the value PowSybl's own MATPOWER importer uses for a bus row with
    /// `BASE_KV = 0`, and it returns the same per-unit model because a reader
    /// divides by the same nominal voltage the writer multiplied by.
    #[test]
    fn emission_substitutes_and_declares_an_unstated_voltage_base() {
        let mut unstated_bus = network();
        unstated_bus.buses_mut()[0].base_kv = 0.0;
        let files = write::write_cgmes(&unstated_bus, CgmesVersion::V3_0).unwrap();
        let messages = files
            .warnings
            .iter()
            .map(|record| record.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            messages.contains("1 bus(es) state no nominal voltage"),
            "{messages}"
        );
        assert!(messages.contains("bus 1"), "{messages}");
        let eq = files
            .files
            .iter()
            .find(|(name, _)| name.contains("_EQ"))
            .map(|(_, text)| text.as_str())
            .unwrap();
        assert!(eq.contains("<cim:BaseVoltage.nominalVoltage>1</cim:BaseVoltage.nominalVoltage>"));

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
    fn imported_power_transformer_end_identities_round_trip_exactly() {
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
        let mut transformer = Transformer3W::new(
            [winding1, winding2, winding3],
            [
                Impedance::new(0.01, 0.10, 100.0),
                Impedance::new(0.02, 0.20, 100.0),
                Impedance::new(0.03, 0.30, 100.0),
            ],
        );
        transformer.uid = Some("three-winding-end-id-test".into());
        network.transformers_3w_mut().push(transformer);

        let mut files = write::write_cgmes(&network, CgmesVersion::V3_0)
            .unwrap()
            .files;
        let eq = files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        let generated = eq
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("<cim:PowerTransformerEnd rdf:ID=\"_")
                    .and_then(|tail| tail.split('\"').next())
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        assert_eq!(generated.len(), 5);
        let retained = [
            "10000000-0000-4000-8000-000000000001",
            "10000000-0000-4000-8000-000000000002",
            "10000000-0000-4000-8000-000000000003",
            "10000000-0000-4000-8000-000000000004",
            "10000000-0000-4000-8000-000000000005",
        ];
        for (generated, retained) in generated.iter().zip(retained) {
            for (_, xml) in &mut files {
                *xml = xml.replace(&format!("_{generated}"), &format!("_{retained}"));
            }
        }

        let parsed = read::read_cgmes_documents(files, Some("retained-transformer-ends")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        for retained in retained {
            let metadata = detailed
                .component_metadata
                .iter()
                .find(|metadata| {
                    metadata.external_identifiers.iter().any(|identifier| {
                        identifier.authority.as_deref() == Some("CGMES")
                            && identifier.value == retained
                    })
                })
                .unwrap();
            assert_eq!(
                metadata
                    .properties
                    .get(CGMES_CLASS_PROPERTY)
                    .map(String::as_str),
                Some("PowerTransformerEnd")
            );
            assert!(
                metadata
                    .properties
                    .contains_key("PowerTransformerEnd.PowerTransformer")
            );
            assert!(metadata.properties.contains_key("TransformerEnd.endNumber"));
        }

        let fresh = write::write_cgmes(&parsed.network, CgmesVersion::V3_0).unwrap();
        let eq = fresh
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        for retained in retained {
            assert!(
                eq.contains(&format!("<cim:PowerTransformerEnd rdf:ID=\"_{retained}\">")),
                "missing retained transformer end {retained}"
            );
        }
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
    fn missing_transformer_rated_voltage_uses_the_connected_bus_with_a_diagnostic() {
        let mut network = network();
        network.branches_mut()[0].tap = 1.05;
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let files = output
            .files
            .into_iter()
            .map(|(name, text)| {
                if !name.ends_with("_EQ.xml") {
                    return (name, text);
                }
                let mut filtered = text
                    .lines()
                    .filter(|line| !line.contains("PowerTransformerEnd.ratedU"))
                    .collect::<Vec<_>>()
                    .join("\n");
                filtered.push('\n');
                (name, filtered)
            })
            .collect();
        let parsed = read::read_cgmes_documents(files, Some("missing-rated-voltage")).unwrap();
        let actual = &parsed.network.branches()[0];
        assert!(actual.r.is_finite() && actual.r > 0.0 && actual.r < 1.0);
        assert!(actual.x.is_finite() && actual.x > 0.0 && actual.x < 1.0);
        let warnings = parsed
            .warnings
            .iter()
            .filter(|warning| {
                warning.info.code == crate::diagnostics::codes::READ_CGMES_VALUE_DEFAULTED.code
                    && warning.contains("PowerTransformerEnd.ratedU is absent")
            })
            .count();
        assert_eq!(warnings, 2);
    }

    #[test]
    fn nonlinear_shunt_sections_round_trip_with_conductance() {
        let mut network = network();
        let mut shunt = Shunt::new(BusId(2), 0.3, 5.0);
        shunt.uid = Some("nonlinear-shunt-1".into());
        shunt.section_count = Some(2);
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
        let sv = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(sv.contains("<cim:SvShuntCompensatorSections "));
        assert!(sv.contains(
            "<cim:SvShuntCompensatorSections.sections>2</cim:SvShuntCompensatorSections.sections>"
        ));

        let parsed = read::read_cgmes_documents(output.files, Some("nonlinear-shunt")).unwrap();
        let shunt = &parsed.network.shunts()[0];
        assert!((shunt.g - 0.3).abs() < 1e-10);
        assert!((shunt.b - 5.0).abs() < 1e-10);
        assert_eq!(shunt.section_count, Some(2));
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
    fn busbar_and_switch_terminals_round_trip_exactly() {
        let mut network = detailed_network();
        let terminal_ids = [
            "10000000-0000-4000-8000-000000000001",
            "10000000-0000-4000-8000-000000000002",
            "10000000-0000-4000-8000-000000000003",
        ];
        let expected = [
            ("busbar_section", 1_u8, terminal_ids[0], false, 1.25, -0.5),
            ("switch", 1_u8, terminal_ids[1], false, 2.0, 3.0),
            ("switch", 2_u8, terminal_ids[2], true, -2.0, -3.0),
        ];
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        for (equipment_type, number, id, connected, p, q) in expected {
            let terminal = detailed
                .terminals
                .iter_mut()
                .find(|terminal| {
                    terminal.equipment.component_type() == equipment_type
                        && terminal.terminal == number
                })
                .unwrap();
            let component = component("terminal", id);
            terminal.component = Some(component.clone());
            terminal.connected = connected;
            terminal.active_power_mw = Some(p);
            terminal.reactive_power_mvar = Some(q);
            detailed.component_metadata.push(ComponentMetadata {
                component,
                name: None,
                equipment_container: None,
                aliases: Vec::new(),
                external_identifiers: vec![ExternalIdentifier {
                    value: id.into(),
                    authority: Some("CGMES".into()),
                }],
                properties: std::collections::BTreeMap::new(),
                fictitious: false,
            });
        }

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let parsed = read::read_cgmes_documents(output.files, Some("terminal-identity")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        for (equipment_type, number, id, connected, p, q) in expected {
            let terminal = detailed
                .terminals
                .iter()
                .find(|terminal| {
                    terminal.equipment.component_type() == equipment_type
                        && terminal.terminal == number
                })
                .unwrap();
            assert_eq!(terminal.component.as_ref().unwrap().local_id(), id);
            assert_eq!(terminal.connected, connected);
            assert_eq!(terminal.active_power_mw, Some(p));
            assert_eq!(terminal.reactive_power_mvar, Some(q));
        }
    }

    #[test]
    fn voltage_level_buses_seed_topological_nodes() {
        let mut network = detailed_network();
        network.loads_mut().clear();
        network.generators_mut().clear();
        network.branches_mut().clear();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.bus_breaker_buses.clear();
        detailed.calculated_buses.clear();
        detailed.connectivity_nodes.clear();
        detailed.busbar_sections.clear();
        detailed.terminals.clear();
        detailed.switches.clear();

        network.validate().unwrap();
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let topology = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_TP.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert_eq!(topology.matches("<cim:TopologicalNode ").count(), 2);
        assert_eq!(
            topology
                .matches("<cim:TopologicalNode.ConnectivityNodeContainer ")
                .count(),
            2
        );

        let reparsed =
            read::read_cgmes_documents(output.files, Some("voltage-level-buses")).unwrap();
        assert_eq!(reparsed.network.buses().len(), 2);
    }

    #[test]
    fn partial_detailed_topology_preserves_every_balanced_bus() {
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let second_bus = component("bus", "tn-B");
        let second_node = component("connectivity_node", "node-B");
        detailed.voltage_levels[0]
            .buses
            .retain(|bus| *bus != BusId(2));
        detailed
            .bus_breaker_buses
            .retain(|bus| bus.component != second_bus);
        detailed
            .connectivity_nodes
            .retain(|node| node.component != second_node);
        detailed.terminals.retain(|terminal| {
            terminal.bus.as_ref() != Some(&second_bus)
                && terminal.node.as_ref() != Some(&second_node)
        });
        detailed.switches.clear();
        network.validate().unwrap();

        for version in [CgmesVersion::V2_4_15, CgmesVersion::V3_0] {
            let output = write::write_cgmes(&network, version).unwrap();
            assert!(output.warnings.iter().any(|warning| {
                warning.contains("balanced bus 2 is absent from detailed connectivity")
                    && warning.contains("generated VoltageLevel and ConnectivityNode")
            }));
            let topology = output
                .files
                .iter()
                .find(|(name, _)| name.ends_with("_TP.xml"))
                .map(|(_, text)| text)
                .unwrap();
            assert_eq!(topology.matches("<cim:TopologicalNode ").count(), 2);

            let parsed =
                read::read_cgmes_documents(output.files, Some("partial-topology")).unwrap();
            assert_eq!(parsed.network.buses().len(), 2);
            assert!(parsed.network.buses().iter().any(|bus| bus.id == BusId(2)));
            assert_eq!(parsed.network.loads().len(), 1);
            assert_eq!(parsed.network.branches().len(), 1);
        }
    }

    #[test]
    fn xiidm_node_breaker_calculated_bus_is_emitted_as_a_cgmes_topological_node() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<iidm:network xmlns:iidm="http://www.powsybl.org/schema/iidm/1_17" id="nodes" caseDate="2025-01-01T00:00:00Z" forecastDistance="0" sourceFormat="test" minimumValidationLevel="STEADY_STATE_HYPOTHESIS">
  <iidm:substation id="S">
    <iidm:voltageLevel id="VL" nominalV="110" topologyKind="NODE_BREAKER">
      <iidm:nodeBreakerTopology>
        <iidm:bus v="110" angle="0" nodes="0,1,2"/>
        <iidm:busbarSection id="BBS" node="2"/>
        <iidm:switch id="BR" kind="BREAKER" open="false" node1="1" node2="2"/>
        <iidm:internalConnection node1="0" node2="1"/>
      </iidm:nodeBreakerTopology>
      <iidm:generator id="G" energySource="OTHER" minP="0" maxP="10" voltageRegulatorOn="true" targetP="5" node="0">
        <iidm:minMaxReactiveLimits minQ="-2" maxQ="2"/>
      </iidm:generator>
    </iidm:voltageLevel>
  </iidm:substation>
</iidm:network>"#;
        let mut xiidm_diagnostics = Diagnostics::new();
        let mut network =
            crate::format::xiidm::parse_xiidm_source(source, &mut xiidm_diagnostics).unwrap();
        let detailed = network.detailed_connectivity().as_deref().unwrap();
        assert!(detailed.bus_breaker_buses.is_empty());
        assert_eq!(detailed.calculated_buses.len(), 1);
        assert_eq!(detailed.calculated_buses[0].nodes.len(), 3);
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        for node in &mut detailed.connectivity_nodes {
            node.calculated_bus = None;
        }
        assert!(
            detailed
                .connectivity_nodes
                .iter()
                .all(|node| node.calculated_bus.is_none())
        );

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let topology = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_TP.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert_eq!(topology.matches("<cim:TopologicalNode ").count(), 1);
        assert_eq!(
            topology
                .matches("<cim:ConnectivityNode.TopologicalNode ")
                .count(),
            3
        );

        let reparsed = read::read_cgmes_documents(output.files, Some("node-breaker")).unwrap();
        assert_eq!(reparsed.network.buses().len(), 1);
        let reparsed_detailed = reparsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(reparsed_detailed.bus_breaker_buses.len(), 1);
        assert_eq!(reparsed_detailed.connectivity_nodes.len(), 3);
        assert!(
            reparsed_detailed
                .connectivity_nodes
                .iter()
                .all(|node| node.calculated_bus == Some(BusId::new(1)))
        );
    }

    #[test]
    fn calculated_bus_node_conflicts_are_rejected() {
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let voltage_level = detailed.voltage_levels[0].component.clone();
        let node = detailed.connectivity_nodes[1].component.clone();
        detailed.calculated_buses.push(CalculatedBus {
            voltage_level: voltage_level.clone(),
            calculated_bus: BusId(1),
            nodes: vec![node.clone()],
            voltage_kv: None,
            angle_degrees: None,
        });

        let error = network.validate().unwrap_err().to_string();
        assert!(error.contains(&node.to_string()), "{error}");
        assert!(error.contains('1') && error.contains('2'), "{error}");

        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let node = detailed.connectivity_nodes[0].component.clone();
        for connectivity_node in &mut detailed.connectivity_nodes {
            connectivity_node.calculated_bus = None;
        }
        detailed.calculated_buses.extend([
            CalculatedBus {
                voltage_level: voltage_level.clone(),
                calculated_bus: BusId(1),
                nodes: vec![node.clone()],
                voltage_kv: None,
                angle_degrees: None,
            },
            CalculatedBus {
                voltage_level,
                calculated_bus: BusId(2),
                nodes: vec![node.clone()],
                voltage_kv: None,
                angle_degrees: None,
            },
        ]);
        let error = network.validate().unwrap_err().to_string();
        assert!(error.contains(&node.to_string()), "{error}");
        assert!(error.contains('1') && error.contains('2'), "{error}");
    }

    #[test]
    fn mixed_voltage_level_topologies_emit_complete_node_breaker_connectivity() {
        let network = mixed_topology_network();
        let original = network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(
            original
                .voltage_levels
                .iter()
                .filter(|level| level.topology_kind == TopologyKind::NodeBreaker)
                .count(),
            1
        );
        assert_eq!(
            original
                .voltage_levels
                .iter()
                .filter(|level| level.topology_kind == TopologyKind::BusBreaker)
                .count(),
            1
        );
        let expected_counts = (
            network.buses().len(),
            network.branches().len(),
            network.generators().len(),
            network.loads().len(),
        );

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(output.warnings.iter().any(|warning| {
            warning.contains("contains both node breaker and bus breaker VoltageLevels")
                && warning.contains("promotes 1 bus breaker VoltageLevel(s)")
                && warning.contains("one ConnectivityNode per TopologicalNode")
        }));
        let parsed = read::read_cgmes_documents(output.files, Some("mixed-topology")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        let generator_terminal = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.component_type() == "generator")
            .unwrap();
        let load_terminal = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.component_type() == "load")
            .unwrap();
        assert!(generator_terminal.node.is_some());
        assert!(load_terminal.node.is_some());
        assert_eq!(
            detailed
                .voltage_levels
                .iter()
                .find(|level| level.component == generator_terminal.voltage_level)
                .map(|level| level.topology_kind),
            Some(TopologyKind::NodeBreaker)
        );
        assert_eq!(
            detailed
                .voltage_levels
                .iter()
                .find(|level| level.component == load_terminal.voltage_level)
                .map(|level| level.topology_kind),
            Some(TopologyKind::NodeBreaker)
        );
        assert_eq!(
            (
                parsed.network.buses().len(),
                parsed.network.branches().len(),
                parsed.network.generators().len(),
                parsed.network.loads().len(),
            ),
            expected_counts
        );
    }

    #[test]
    fn projected_converter_only_transformer_connection_is_diagnosed() {
        let mut network = mixed_topology_network();
        network.branches_mut()[0].tap = 1.0;
        network.loads_mut().clear();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed
            .terminals
            .retain(|terminal| terminal.equipment.component_type() != "load");

        let converter = component("voltage_source_converter", "converter-A");
        let dc_node_1 = component("dc_node", "dc-node-1");
        let dc_node_2 = component("dc_node", "dc-node-2");
        let dc_terminal = |node: &ComponentId| DcTerminal {
            component: None,
            sequence_number: None,
            dc_node: Some(node.clone()),
            dc_topological_node: None,
            polarity: None,
            connected: Some(true),
            active_power_mw: None,
            current_a: None,
        };
        detailed.dc_series_devices.push(DcSeriesDevice {
            component: component("dc_series_device", "series-A"),
            equipment_container: None,
            dc_terminal1: dc_terminal(&dc_node_1),
            dc_terminal2: dc_terminal(&dc_node_2),
            rated_dc_voltage_kv: None,
            resistance_ohm: None,
            inductance_h: None,
        });
        detailed.voltage_source_converters.push(
            serde_json::from_value(serde_json::json!({
                "component": converter,
                "dc_terminal1": dc_terminal(&dc_node_1),
                "dc_terminal2": dc_terminal(&dc_node_2),
            }))
            .unwrap(),
        );
        detailed.terminals.push(Terminal {
            component: None,
            equipment: converter,
            terminal: 1,
            voltage_level: component("voltage_level", "vl-B"),
            bus: Some(component("bus", "tn-B")),
            connectable_bus: Some(component("bus", "tn-B")),
            node: None,
            connected: true,
            active_power_mw: None,
            reactive_power_mvar: None,
        });

        let detailed = network.detailed_connectivity().as_deref().unwrap();
        let mut warnings =
            CgmesDiagnostics::new(&crate::diagnostics::codes::EMIT_CGMES.record_dropped);
        write::warn_pow_sybl_projected_transformer_connections(&network, detailed, &mut warnings);
        let branch = &network.branches()[0];
        let branch_id = branch.uid.as_deref().unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains(&format!("transformer `branch/{branch_id}` terminal 2")));
        assert!(warnings[0].contains("configured bus `bus/tn-B`"));
        assert!(warnings[0].contains("voltage_source_converter/converter-A"));
        assert!(warnings[0].contains("unsupported BACK_TO_BACK"));
        assert!(warnings[0].contains("reloads this transformer terminal as disconnected"));
    }

    const PROJECTED_VOLTAGE_LEVEL_MRID: &str = "0f268ca2-545f-4acf-b01d-b223a0c4e30d";
    const PROJECTED_BUSBAR_MRID: &str = "9fa5c795-e6b1-4226-a9fc-e506b4fe68f4";

    fn mixed_topology_network_with_busbar() -> BalancedNetwork {
        let mut network = mixed_topology_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let voltage_level = component("voltage_level", "vl-B");
        let bus = component("bus", "tn-B");
        let node = component("connectivity_node", "terminal-busbar-b");
        let busbar = component("busbar_section", "bbs-B");
        detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| metadata.component == voltage_level)
            .unwrap()
            .external_identifiers
            .push(ExternalIdentifier {
                value: PROJECTED_VOLTAGE_LEVEL_MRID.into(),
                authority: Some("CGMES".into()),
            });
        detailed.component_metadata.push(ComponentMetadata {
            component: busbar.clone(),
            name: Some("South busbar".into()),
            equipment_container: Some(voltage_level.clone()),
            aliases: Vec::new(),
            external_identifiers: vec![ExternalIdentifier {
                value: PROJECTED_BUSBAR_MRID.into(),
                authority: Some("CGMES".into()),
            }],
            properties: std::collections::BTreeMap::new(),
            fictitious: false,
        });
        detailed.connectivity_nodes.push(ConnectivityNode {
            component: node.clone(),
            voltage_level: voltage_level.clone(),
            node_number: None,
            calculated_bus: Some(BusId(2)),
        });
        detailed.busbar_sections.push(BusbarSection {
            component: busbar.clone(),
            voltage_level: voltage_level.clone(),
            node: node.clone(),
        });
        detailed.terminals.push(Terminal {
            component: None,
            equipment: busbar,
            terminal: 1,
            voltage_level,
            bus: Some(bus.clone()),
            connectable_bus: Some(bus),
            node: Some(node),
            connected: true,
            active_power_mw: None,
            reactive_power_mvar: None,
        });
        assert_eq!(
            detailed
                .voltage_levels
                .iter()
                .find(|level| level.component == component("voltage_level", "vl-B"))
                .map(|level| level.topology_kind),
            Some(TopologyKind::BusBreaker)
        );
        network
    }

    #[test]
    fn bus_breaker_busbar_is_retained_during_mixed_topology_projection() {
        let network = mixed_topology_network_with_busbar();
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(eq.contains("<cim:IdentifiedObject.name>South busbar</"));
        assert!(eq.contains(&format!(
            "<cim:BusbarSection rdf:ID=\"_{PROJECTED_BUSBAR_MRID}\">"
        )));
        assert!(eq.contains(&format!(
            "<cim:Equipment.EquipmentContainer rdf:resource=\"#_{PROJECTED_VOLTAGE_LEVEL_MRID}\"/>"
        )));
        assert!(eq.contains("<cim:ConductingEquipment.BaseVoltage rdf:resource=\"#"));
        assert!(!output.warnings.iter().any(|warning| {
            warning.contains("BusbarSection `busbar_section/bbs-B`")
                && warning.contains("was omitted")
        }));
        assert!(output.warnings.iter().any(|warning| {
            warning.contains("contains both node breaker and bus breaker VoltageLevels")
                && warning.contains("one ConnectivityNode per TopologicalNode")
        }));

        let parsed = read::read_cgmes_documents(output.files, Some("bus-breaker-busbar")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        let level_component = detailed
            .component_metadata
            .iter()
            .find(|metadata| {
                metadata.external_identifiers.iter().any(|identifier| {
                    identifier.authority.as_deref() == Some("CGMES")
                        && identifier.value == PROJECTED_VOLTAGE_LEVEL_MRID
                })
            })
            .map(|metadata| &metadata.component)
            .unwrap();
        let level = detailed
            .voltage_levels
            .iter()
            .find(|level| level.component == *level_component)
            .unwrap();
        assert_eq!(level.topology_kind, TopologyKind::NodeBreaker);
        assert!((level.nominal_kv - 230.0).abs() < 1e-12);

        let busbar_metadata = detailed
            .component_metadata
            .iter()
            .find(|metadata| {
                metadata.external_identifiers.iter().any(|identifier| {
                    identifier.authority.as_deref() == Some("CGMES")
                        && identifier.value == PROJECTED_BUSBAR_MRID
                })
            })
            .unwrap();
        assert_eq!(busbar_metadata.name.as_deref(), Some("South busbar"));
        assert_eq!(
            busbar_metadata.equipment_container.as_ref(),
            Some(level_component)
        );
        let parsed_busbar = detailed
            .busbar_sections
            .iter()
            .find(|record| record.component == busbar_metadata.component)
            .unwrap();
        assert_eq!(&parsed_busbar.voltage_level, level_component);
        let parsed_terminal = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment == parsed_busbar.component)
            .unwrap();
        assert_eq!(parsed_terminal.node.as_ref(), Some(&parsed_busbar.node));
        let parsed_bus = detailed
            .bus_breaker_buses
            .iter()
            .find(|record| record.calculated_bus == Some(BusId(2)))
            .unwrap();
        assert_eq!(parsed_terminal.bus.as_ref(), Some(&parsed_bus.component));
        let parsed_node = detailed
            .connectivity_nodes
            .iter()
            .find(|record| record.component == parsed_busbar.node)
            .unwrap();
        assert_eq!(&parsed_node.voltage_level, level_component);
        assert_eq!(parsed_node.calculated_bus, Some(BusId(2)));
    }

    #[test]
    fn exact_sv_observations_round_trip_without_filling_absent_values() {
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.bus_breaker_buses[0].voltage_kv = Some(231.25);
        detailed.bus_breaker_buses[0].angle_degrees = Some(-2.75);
        let expected_flows = [
            ("load", 1, 20.25, 5.5),
            ("generator", 1, -20.5, -4.75),
            ("branch", 1, 18.0, 3.0),
            ("branch", 2, -17.75, -2.5),
        ];
        for (component_type, sequence, active, reactive) in expected_flows {
            let terminal = detailed
                .terminals
                .iter_mut()
                .find(|terminal| {
                    terminal.equipment.component_type() == component_type
                        && terminal.terminal == sequence
                })
                .unwrap();
            terminal.active_power_mw = Some(active);
            terminal.reactive_power_mvar = Some(reactive);
        }

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let sv = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert_eq!(sv.matches("<cim:SvPowerFlow rdf:ID=").count(), 4);
        assert_eq!(sv.matches("<cim:SvVoltage rdf:ID=").count(), 1);

        let parsed = read::read_cgmes_documents(output.files, Some("exact-sv")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        for (component_type, sequence, active, reactive) in expected_flows {
            let terminal = detailed
                .terminals
                .iter()
                .find(|terminal| {
                    terminal.equipment.component_type() == component_type
                        && terminal.terminal == sequence
                })
                .unwrap();
            assert_eq!(terminal.active_power_mw, Some(active));
            assert_eq!(terminal.reactive_power_mvar, Some(reactive));
        }
        let first = detailed
            .bus_breaker_buses
            .iter()
            .find(|bus| bus.calculated_bus == Some(BusId(1)))
            .unwrap();
        let second = detailed
            .bus_breaker_buses
            .iter()
            .find(|bus| bus.calculated_bus == Some(BusId(2)))
            .unwrap();
        assert_eq!(first.voltage_kv, Some(231.25));
        assert_eq!(first.angle_degrees, Some(-2.75));
        assert_eq!(second.voltage_kv, None);
        assert_eq!(second.angle_degrees, None);
    }

    #[test]
    fn partial_sv_power_flow_is_diagnosed_instead_of_silently_dropped() {
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let load_terminal = detailed
            .terminals
            .iter_mut()
            .find(|terminal| terminal.equipment.component_type() == "load")
            .unwrap();
        load_terminal.active_power_mw = Some(12.5);
        load_terminal.reactive_power_mvar = None;

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(output.warnings.iter().any(|warning| {
            warning.contains("retained active power field 12.5 MW without reactive power")
                && warning.contains("CGMES SvPowerFlow requires both p and q")
        }));
        let sv = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(!sv.contains("<cim:SvPowerFlow rdf:ID="));

        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.bus_breaker_buses[0].voltage_kv = Some(231.0);
        detailed.bus_breaker_buses[0].angle_degrees = None;
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(output.warnings.iter().any(|warning| {
            warning.contains("retained voltage field 231 kV without an angle")
                && warning.contains("CGMES SvVoltage requires both v and angle")
        }));
        let sv = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .map(|(_, xml)| xml)
            .unwrap();
        assert!(!sv.contains("<cim:SvVoltage rdf:ID="));
    }

    #[test]
    fn flat_branch_and_switch_solution_values_emit_as_sv_power_flow() {
        let mut network = network();
        network.branches_mut()[0].solution = Some(BranchSolution {
            pf: 18.0,
            qf: 3.0,
            pt: -17.75,
            qt: -2.5,
        });
        let mut switch = Switch::new(BusId(1), BusId(2), false);
        switch.uid = Some("coupler".into());
        switch.current_rating = Some(500.0);
        switch.thermal_rating = Some(100.0);
        switch.pf = Some(5.0);
        switch.qf = Some(1.0);
        switch.pt = Some(-4.9);
        switch.qt = Some(-0.9);
        network.switches_mut().push(switch);

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(output.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::EMIT_CGMES.field_dropped.code
                && warning.contains("switch `coupler` thermal rating 100 MVA")
                && warning.contains("rated current")
        }));
        let parsed = read::read_cgmes_documents(output.files, Some("flat-solutions")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert_eq!(parsed.network.branches().len(), 1);
        assert_eq!(parsed.network.switches().len(), 1);
        let branch_id = parsed.network.branches()[0].uid.as_deref().unwrap();
        let switch_id = parsed.network.switches()[0].uid.as_deref().unwrap();
        for (component_type, local_id, side, active, reactive) in [
            ("branch", branch_id, 1, 18.0, 3.0),
            ("branch", branch_id, 2, -17.75, -2.5),
            ("switch", switch_id, 1, 5.0, 1.0),
            ("switch", switch_id, 2, -4.9, -0.9),
        ] {
            let terminal = detailed
                .terminals
                .iter()
                .find(|terminal| {
                    terminal.equipment.component_type() == component_type
                        && terminal.equipment.local_id() == local_id
                        && terminal.terminal == side
                })
                .unwrap();
            assert_eq!(terminal.active_power_mw, Some(active));
            assert_eq!(terminal.reactive_power_mvar, Some(reactive));
        }
    }

    #[test]
    fn detailed_switch_uses_balanced_solution_values_when_terminal_values_are_absent() {
        let mut network = detailed_network();
        let mut switch = Switch::new(BusId(1), BusId(2), false);
        switch.uid = Some("breaker-A".into());
        switch.current_rating = Some(500.0);
        switch.thermal_rating = Some(100.0);
        switch.pf = Some(5.0);
        switch.qf = Some(1.0);
        switch.pt = Some(-4.9);
        switch.qt = Some(-0.9);
        network.switches_mut().push(switch);

        let check = |network: &BalancedNetwork, label: &str| {
            let output = write::write_cgmes(network, CgmesVersion::V3_0).unwrap();
            assert!(output.warnings.iter().any(|warning| {
                warning.info.code == crate::diagnostics::codes::EMIT_CGMES.field_dropped.code
                    && warning.contains("switch `breaker-A` thermal rating 100 MVA")
            }));
            let parsed = read::read_cgmes_documents(output.files, Some(label)).unwrap();
            let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
            assert_eq!(parsed.network.switches().len(), 1);
            let switch_id = parsed.network.switches()[0].uid.as_deref().unwrap();
            for (side, active, reactive) in [(1, 5.0, 1.0), (2, -4.9, -0.9)] {
                let terminal = detailed
                    .terminals
                    .iter()
                    .find(|terminal| {
                        terminal.equipment.component_type() == "switch"
                            && terminal.equipment.local_id() == switch_id
                            && terminal.terminal == side
                    })
                    .unwrap();
                assert_eq!(terminal.active_power_mw, Some(active));
                assert_eq!(terminal.reactive_power_mvar, Some(reactive));
            }
        };

        check(&network, "detailed-switch-solution");

        let mut without_topology_switch = network;
        Arc::make_mut(
            without_topology_switch
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        )
        .switches
        .clear();
        check(
            &without_topology_switch,
            "detailed-terminals-with-balanced-switch",
        );
    }

    #[test]
    fn generator_voltage_control_round_trips_with_exact_remote_terminal() {
        const TERMINAL_MRID: &str = "de305d54-75b4-431b-adb2-eb6b9e546099";
        let mut network = detailed_network();
        let regulating_terminal = TerminalReference {
            equipment: component("load", network.loads()[0].uid.as_deref().unwrap()),
            terminal: 1,
        };
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let terminal_component = component("terminal", "load-terminal");
        detailed
            .terminals
            .iter_mut()
            .find(|terminal| {
                terminal.equipment == regulating_terminal.equipment && terminal.terminal == 1
            })
            .unwrap()
            .component = Some(terminal_component.clone());
        detailed.component_metadata.push(ComponentMetadata {
            component: terminal_component,
            name: Some("Load terminal".into()),
            equipment_container: None,
            aliases: Vec::new(),
            external_identifiers: vec![ExternalIdentifier {
                value: TERMINAL_MRID.into(),
                authority: Some("CGMES".into()),
            }],
            properties: std::collections::BTreeMap::new(),
            fictitious: false,
        });
        let generator = &mut network.generators_mut()[0];
        generator.voltage_regulation_on = false;
        generator.regulated_bus = Some(BusId(2));
        generator.regulating_terminal = Some(regulating_terminal.clone());

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        let ssh = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SSH.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(eq.contains(&format!("<cim:Terminal rdf:ID=\"_{TERMINAL_MRID}\">")));
        assert!(eq.contains(&format!(
            "<cim:RegulatingControl.Terminal rdf:resource=\"#_{TERMINAL_MRID}\"/>"
        )));
        assert!(
            ssh.contains("<cim:RegulatingControl.enabled>false</cim:RegulatingControl.enabled>")
        );
        assert!(
            ssh.contains("<cim:RegulatingControl.discrete>false</cim:RegulatingControl.discrete>")
        );
        assert!(ssh.contains(
            "<cim:RegulatingControl.targetValueUnitMultiplier rdf:resource=\"http://iec.ch/TC57/CIM100#UnitMultiplier.k\"/>"
        ));
        assert!(ssh.contains(
            "<cim:RegulatingCondEq.controlEnabled>false</cim:RegulatingCondEq.controlEnabled>"
        ));
        assert!(ssh.contains(
            "<cim:SynchronousMachine.operatingMode rdf:resource=\"http://iec.ch/TC57/CIM100#SynchronousMachineOperatingMode.generator\"/>"
        ));

        let parsed = read::read_cgmes_documents(output.files, Some("generator-control")).unwrap();
        let generator = &parsed.network.generators()[0];
        assert!(!generator.voltage_regulation_on);
        assert_eq!(generator.regulated_bus, Some(BusId(2)));
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        let terminal = detailed
            .terminals
            .iter()
            .find(|terminal| {
                terminal
                    .component
                    .as_ref()
                    .is_some_and(|component| component.local_id() == TERMINAL_MRID)
            })
            .unwrap();
        assert_eq!(terminal.terminal, 1);
        assert_eq!(
            terminal.equipment.local_id(),
            parsed.network.loads()[0].uid.as_deref().unwrap()
        );
        let component = terminal.component.as_ref().unwrap();
        let metadata = detailed
            .component_metadata
            .iter()
            .find(|metadata| metadata.component == *component)
            .unwrap();
        assert!(metadata.external_identifiers.iter().any(|identifier| {
            identifier.value == TERMINAL_MRID && identifier.authority.as_deref() == Some("CGMES")
        }));
        let regulating_terminal = generator.regulating_terminal.as_ref().unwrap();
        assert_eq!(regulating_terminal.terminal, 1);
        assert_eq!(
            regulating_terminal.equipment.local_id(),
            parsed.network.loads()[0].uid.as_deref().unwrap()
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
                equipment_container: None,
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
                true,
                None,
            ),
            dc_terminal2: dc_terminal(
                "switch-t2",
                2,
                &third_dc_node,
                &third_dc_topological_node,
                None,
                true,
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
                component: None,
                equipment: voltage_source.clone(),
                terminal: 1,
                voltage_level: voltage_level.clone(),
                bus: Some(first_bus.clone()),
                connectable_bus: Some(first_bus),
                node: Some(first_node),
                connected: false,
                active_power_mw: Some(150.0),
                reactive_power_mvar: Some(20.0),
            },
            Terminal {
                component: None,
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

        let mut assignment_only = network.clone();
        let assignment_detailed = Arc::make_mut(
            assignment_only
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        for terminal in assignment_detailed.terminals.iter_mut().filter(|terminal| {
            matches!(
                terminal.equipment.component_type(),
                "voltage_source_converter" | "line_commutated_converter"
            )
        }) {
            terminal.active_power_mw = None;
            terminal.reactive_power_mvar = None;
        }
        let assignment_output = write::write_cgmes(&assignment_only, CgmesVersion::V3_0).unwrap();
        let assignment_parsed =
            read::read_cgmes_documents(assignment_output.files, Some("converter-assignment"))
                .unwrap();
        let assignment_detailed = assignment_parsed
            .network
            .detailed_connectivity()
            .as_deref()
            .unwrap();
        assert_eq!(
            assignment_detailed.voltage_source_converters[0].active_power_at_pcc_mw,
            Some(150.0)
        );
        assert_eq!(
            assignment_detailed.voltage_source_converters[0].reactive_power_at_pcc_mvar,
            Some(20.0)
        );
        assert!(
            assignment_detailed
                .terminals
                .iter()
                .filter(|terminal| {
                    matches!(
                        terminal.equipment.component_type(),
                        "voltage_source_converter" | "line_commutated_converter"
                    )
                })
                .all(|terminal| terminal.active_power_mw.is_none()
                    && terminal.reactive_power_mvar.is_none())
        );

        let mut generic_converter_limits = network.clone();
        let generic_detailed = Arc::make_mut(
            generic_converter_limits
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        let converter_component = generic_detailed.voltage_source_converters[0]
            .component
            .clone();
        let limits = generic_detailed.voltage_source_converters[0]
            .reactive_limits
            .take()
            .unwrap();
        generic_detailed
            .equipment_reactive_limits
            .push(EquipmentReactiveLimits {
                equipment: converter_component,
                limits,
            });
        let generic_output =
            write::write_cgmes(&generic_converter_limits, CgmesVersion::V3_0).unwrap();
        assert!(
            generic_output
                .files
                .iter()
                .any(|(_, xml)| xml.contains("<cim:VsCapabilityCurve "))
        );

        let mut conflicting_converter_limits = network.clone();
        let conflicting_detailed = Arc::make_mut(
            conflicting_converter_limits
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        let converter_component = conflicting_detailed.voltage_source_converters[0]
            .component
            .clone();
        conflicting_detailed
            .equipment_reactive_limits
            .push(EquipmentReactiveLimits {
                equipment: converter_component,
                limits: ReactiveLimits::MinMax(MinMaxReactiveLimits {
                    minimum_reactive_power_mvar: -1.0,
                    maximum_reactive_power_mvar: 1.0,
                    properties: std::collections::BTreeMap::new(),
                }),
            });
        let error =
            write::write_cgmes(&conflicting_converter_limits, CgmesVersion::V3_0).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("has conflicting direct and equipment reactive limits records")
        );

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
        assert!(all_xml.contains("<cim:Switch.open>true</cim:Switch.open>"));
        assert!(all_xml.contains("<cim:CsConverter.targetAlpha>12"));
        assert!(all_xml.contains("<cim:CsConverter.targetGamma>18"));
        assert!(all_xml.contains("<cim:CsConverter.targetIdc>450"));
        assert_eq!(
            all_xml
                .matches("<cim:ACDCTerminal.connected>false</cim:ACDCTerminal.connected>")
                .count(),
            1
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
        assert_eq!(all_xml.matches("<cim:SvPowerFlow rdf:ID=").count(), 2);
        assert!(all_xml.contains("<cim:SvPowerFlow.p>150</cim:SvPowerFlow.p>"));
        assert!(all_xml.contains("<cim:SvPowerFlow.q>20</cim:SvPowerFlow.q>"));
        assert!(all_xml.contains("<cim:SvPowerFlow.p>-145</cim:SvPowerFlow.p>"));
        assert!(all_xml.contains("<cim:SvPowerFlow.q>35</cim:SvPowerFlow.q>"));

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
        let voltage_source_terminal = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.component_type() == "voltage_source_converter")
            .unwrap();
        assert!(!voltage_source_terminal.connected);
        assert!(voltage_source_terminal.bus.is_none());
        assert!(voltage_source_terminal.node.is_some());
        assert_eq!(voltage_source_terminal.active_power_mw, Some(150.0));
        assert_eq!(voltage_source_terminal.reactive_power_mvar, Some(20.0));
        let line_commutated_terminal = detailed
            .terminals
            .iter()
            .find(|terminal| terminal.equipment.component_type() == "line_commutated_converter")
            .unwrap();
        assert_eq!(line_commutated_terminal.active_power_mw, Some(-145.0));
        assert_eq!(line_commutated_terminal.reactive_power_mvar, Some(35.0));

        let mut closed_network = network;
        Arc::make_mut(closed_network.detailed_connectivity_mut().as_mut().unwrap()).dc_switches
            [0]
        .open = Some(false);
        let closed = write::write_cgmes(&closed_network, CgmesVersion::V3_0).unwrap();
        let closed_xml = closed
            .files
            .iter()
            .map(|(_, xml)| xml.as_str())
            .collect::<String>();
        assert!(closed_xml.contains("<cim:Switch.open>false</cim:Switch.open>"));
        let reparsed = read::read_cgmes_documents(closed.files, Some("closed-dc-switch")).unwrap();
        assert_eq!(
            reparsed
                .network
                .detailed_connectivity()
                .as_deref()
                .unwrap()
                .dc_switches[0]
                .open,
            Some(false)
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
    fn retained_load_and_switch_classes_survive_fresh_emission() {
        let files = write::write_cgmes(&detailed_network(), CgmesVersion::V3_0)
            .unwrap()
            .files
            .into_iter()
            .map(|(name, text)| {
                (
                    name,
                    text.replace("cim:EnergyConsumer", "cim:ConformLoad")
                        .replace("cim:Breaker", "cim:Fuse"),
                )
            })
            .collect();
        let parsed = read::read_cgmes_documents(files, Some("retained-classes")).unwrap();
        let detailed = parsed.network.detailed_connectivity().as_deref().unwrap();
        assert!(detailed.component_metadata.iter().any(|metadata| {
            metadata.component.component_type() == "load"
                && metadata
                    .properties
                    .get(CGMES_CLASS_PROPERTY)
                    .map(String::as_str)
                    == Some("ConformLoad")
        }));
        assert!(detailed.component_metadata.iter().any(|metadata| {
            metadata.component.component_type() == "switch"
                && metadata
                    .properties
                    .get(CGMES_CLASS_PROPERTY)
                    .map(String::as_str)
                    == Some("Fuse")
        }));

        let fresh = write::write_cgmes(&parsed.network, CgmesVersion::V3_0).unwrap();
        let equipment = &fresh
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .unwrap()
            .1;
        assert!(equipment.contains("<cim:ConformLoad "));
        assert!(equipment.contains("<cim:Fuse "));
        assert!(!equipment.contains("<cim:EnergyConsumer "));
        assert!(!equipment.contains("<cim:Breaker "));
    }

    #[test]
    fn external_network_injection_substitution_is_explicit() {
        let files = write::write_cgmes(&detailed_network(), CgmesVersion::V3_0)
            .unwrap()
            .files;
        let topological_node = files
            .iter()
            .find(|(name, _)| name.ends_with("_TP.xml"))
            .and_then(|(_, text)| {
                text.lines()
                    .find(|line| line.contains("<cim:TopologicalNode rdf:ID="))
            })
            .and_then(|line| line.split("rdf:ID=\"").nth(1))
            .and_then(|value| value.split('"').next())
            .unwrap()
            .trim_start_matches('_')
            .to_string();
        let equipment = r##"  <cim:ExternalNetworkInjection rdf:ID="_external-1">
    <cim:ExternalNetworkInjection.maxP>100</cim:ExternalNetworkInjection.maxP>
    <cim:ExternalNetworkInjection.minP>-100</cim:ExternalNetworkInjection.minP>
    <cim:ExternalNetworkInjection.maxQ>50</cim:ExternalNetworkInjection.maxQ>
    <cim:ExternalNetworkInjection.minQ>-50</cim:ExternalNetworkInjection.minQ>
  </cim:ExternalNetworkInjection>
  <cim:Terminal rdf:ID="_external-terminal">
    <cim:Terminal.ConductingEquipment rdf:resource="#_external-1"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
"##;
        let topology = format!(
            r##"  <cim:Terminal rdf:about="#_external-terminal">
    <cim:Terminal.TopologicalNode rdf:resource="#_{topological_node}"/>
  </cim:Terminal>
"##
        );
        let ssh = r##"  <cim:ExternalNetworkInjection rdf:about="#_external-1">
    <cim:ExternalNetworkInjection.p>-30</cim:ExternalNetworkInjection.p>
    <cim:ExternalNetworkInjection.q>-5</cim:ExternalNetworkInjection.q>
  </cim:ExternalNetworkInjection>
"##;
        let files = insert_profile_records(files, equipment, Some(&topology), Some(ssh));
        let parsed = read::read_cgmes_documents(files, Some("external-injection")).unwrap();
        let generator = parsed
            .network
            .generators()
            .iter()
            .find(|generator| generator.uid.as_deref() == Some("external-1"))
            .unwrap();
        assert!((generator.pg - 30.0).abs() <= f64::EPSILON);
        assert!((generator.qg - 5.0).abs() <= f64::EPSILON);
        assert!(parsed.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::READ_CGMES_VALUE_APPROXIMATED.code
                && warning.contains("ExternalNetworkInjection `external-1`")
                && warning.contains("fresh CGMES output emits a SynchronousMachine")
        }));
        let fresh = write::write_cgmes(&parsed.network, CgmesVersion::V3_0).unwrap();
        let equipment = &fresh
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .unwrap()
            .1;
        assert!(equipment.contains("<cim:SynchronousMachine "));
        assert!(!equipment.contains("<cim:ExternalNetworkInjection "));
    }

    #[test]
    fn conflicting_solution_observations_are_rejected() {
        fn reference_in_first_block(text: &str, class: &str, property: &str) -> String {
            let start = text.find(&format!("<cim:{class} ")).unwrap();
            let end = text[start..]
                .find(&format!("</cim:{class}>"))
                .map(|offset| start + offset)
                .unwrap();
            text[start..end]
                .lines()
                .find(|line| line.contains(&format!("<cim:{property} ")))
                .and_then(|line| line.split("rdf:resource=\"#").nth(1))
                .and_then(|value| value.split('"').next())
                .unwrap()
                .to_string()
        }
        fn append_sv(files: &mut [(String, String)], record: &str) {
            let (_, text) = files
                .iter_mut()
                .find(|(name, _)| name.ends_with("_SV.xml"))
                .unwrap();
            *text = text.replace("</rdf:RDF>", &format!("{record}</rdf:RDF>"));
        }

        let mut voltage_network = detailed_network();
        let detailed = Arc::make_mut(
            voltage_network
                .detailed_connectivity_mut()
                .as_mut()
                .unwrap(),
        );
        detailed.bus_breaker_buses[0].voltage_kv = Some(230.0);
        detailed.bus_breaker_buses[0].angle_degrees = Some(0.0);
        let base = write::write_cgmes(&voltage_network, CgmesVersion::V3_0)
            .unwrap()
            .files;
        let sv = &base
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .unwrap()
            .1;
        let node = reference_in_first_block(sv, "SvVoltage", "SvVoltage.TopologicalNode");
        let mut voltage_files = base.clone();
        append_sv(
            &mut voltage_files,
            &format!(
                r##"  <cim:SvVoltage rdf:ID="_conflicting-voltage">
    <cim:SvVoltage.TopologicalNode rdf:resource="#{node}"/>
    <cim:SvVoltage.v>999</cim:SvVoltage.v>
    <cim:SvVoltage.angle>99</cim:SvVoltage.angle>
  </cim:SvVoltage>
"##
            ),
        );
        let message = read::read_cgmes_documents(voltage_files, Some("conflicting-voltage"))
            .err()
            .unwrap()
            .to_string();
        assert!(message.contains("conflicting SvVoltage observations"));

        let mut flow_network = detailed_network();
        let detailed = Arc::make_mut(flow_network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.terminals[0].active_power_mw = Some(1.0);
        detailed.terminals[0].reactive_power_mvar = Some(2.0);
        let mut flow_files = write::write_cgmes(&flow_network, CgmesVersion::V3_0)
            .unwrap()
            .files;
        let sv = &flow_files
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .unwrap()
            .1;
        let terminal = reference_in_first_block(sv, "SvPowerFlow", "SvPowerFlow.Terminal");
        append_sv(
            &mut flow_files,
            &format!(
                r##"  <cim:SvPowerFlow rdf:ID="_conflicting-flow">
    <cim:SvPowerFlow.Terminal rdf:resource="#{terminal}"/>
    <cim:SvPowerFlow.p>3</cim:SvPowerFlow.p>
    <cim:SvPowerFlow.q>4</cim:SvPowerFlow.q>
  </cim:SvPowerFlow>
"##
            ),
        );
        let message = read::read_cgmes_documents(flow_files, Some("conflicting-flow"))
            .err()
            .unwrap()
            .to_string();
        assert!(message.contains("conflicting SvPowerFlow observations"));
    }

    #[test]
    fn flat_network_voltage_limits_and_all_area_records_are_reported() {
        let mut network = network();
        network.buses_mut()[0].vmin = 0.91;
        network.buses_mut()[0].vmax = 1.09;
        network.areas_mut().push(Area::new(1));
        network.areas_mut().push(Area::new(2));
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let equipment = &output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .unwrap()
            .1;
        assert!(equipment.contains("<cim:VoltageLevel.lowVoltageLimit>209.3"));
        assert!(equipment.contains("<cim:VoltageLevel.highVoltageLimit>250.7"));
        assert!(output.warnings.iter().any(|warning| {
            warning.contains("2 area record(s) dropped: a CGMES ControlArea states")
        }));
    }

    #[test]
    fn voltage_limits_use_the_most_restrictive_valid_voltage_level_range() {
        const VOLTAGE_LEVEL_MRID: &str = "10000000-0000-4000-8000-000000000001";
        let cases = [
            (
                "operational-tighter",
                (380.0, 420.0),
                (390.0, 410.0),
                (390.0, 410.0),
            ),
            (
                "voltage-level-tighter",
                (380.0, 420.0),
                (370.0, 430.0),
                (380.0, 420.0),
            ),
            (
                "inconsistent-voltage-level",
                (420.0, 380.0),
                (380.0, 420.0),
                (380.0, 420.0),
            ),
        ];
        for version in [CgmesVersion::V2_4_15, CgmesVersion::V3_0] {
            for (name, voltage_level_limits, operational_limits, expected) in cases {
                let parsed = read::read_cgmes_documents(
                    voltage_limit_documents(version, voltage_level_limits, operational_limits),
                    Some(name),
                )
                .unwrap();
                let level = parsed
                    .network
                    .detailed_connectivity()
                    .as_deref()
                    .unwrap()
                    .voltage_levels
                    .iter()
                    .find(|level| level.component.local_id() == VOLTAGE_LEVEL_MRID)
                    .unwrap();
                assert_eq!(
                    level.low_voltage_limit_kv,
                    Some(expected.0),
                    "{version:?} {name}"
                );
                assert_eq!(
                    level.high_voltage_limit_kv,
                    Some(expected.1),
                    "{version:?} {name}"
                );
                assert!(parsed.warnings.iter().any(|warning| {
                    warning.info.code
                        == crate::diagnostics::codes::READ_CGMES_VALUE_APPROXIMATED.code
                        && warning.contains("most restrictive valid")
                        && warning.contains("low-voltage-limit")
                        && warning.contains("high-voltage-limit")
                }));
                if name == "inconsistent-voltage-level" {
                    assert!(parsed.warnings.iter().any(|warning| {
                        warning.info.code
                            == crate::diagnostics::codes::READ_CGMES_VALUE_APPROXIMATED.code
                            && warning
                                .contains("both inconsistent VoltageLevel limits were ignored")
                    }));
                }

                let fresh = write::write_cgmes(&parsed.network, CgmesVersion::V3_0).unwrap();
                let equipment = &fresh
                    .files
                    .iter()
                    .find(|(name, _)| name.ends_with("_EQ.xml"))
                    .unwrap()
                    .1;
                assert!(equipment.contains("<cim:VoltageLevel.lowVoltageLimit>"));
                assert!(equipment.contains("<cim:VoltageLevel.highVoltageLimit>"));
                assert!(!equipment.contains("<cim:VoltageLimit"));
                let reparsed =
                    read::read_cgmes_documents(fresh.files, Some("fresh-limits")).unwrap();
                let reparsed_level = reparsed
                    .network
                    .detailed_connectivity()
                    .as_deref()
                    .unwrap()
                    .voltage_levels
                    .iter()
                    .find(|level| level.component.local_id() == VOLTAGE_LEVEL_MRID)
                    .unwrap();
                assert_eq!(reparsed_level.low_voltage_limit_kv, Some(expected.0));
                assert_eq!(reparsed_level.high_voltage_limit_kv, Some(expected.1));
            }
        }
    }

    #[test]
    fn unmappable_voltage_limit_has_a_specific_diagnostic() {
        let documents = voltage_limit_documents(CgmesVersion::V3_0, (380.0, 420.0), (390.0, 410.0));
        let documents = insert_profile_records(
            documents,
            r##"  <cim:OperationalLimitSet rdf:ID="_orphan-voltage-limit-set"/>
  <cim:VoltageLimit rdf:ID="_orphan-voltage-limit">
    <cim:OperationalLimit.OperationalLimitSet rdf:resource="#_orphan-voltage-limit-set"/>
    <cim:OperationalLimit.OperationalLimitType rdf:resource="#_low-voltage-limit-type"/>
    <cim:VoltageLimit.normalValue>395</cim:VoltageLimit.normalValue>
  </cim:VoltageLimit>
"##,
            None,
            None,
        );
        let parsed = read::read_cgmes_documents(documents, Some("orphan-voltage-limit")).unwrap();
        assert!(parsed.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::READ_CGMES_RECORD_UNMAPPED.code
                && warning.contains("VoltageLimit `orphan-voltage-limit`")
                && warning.contains("does not target equipment or a terminal in one VoltageLevel")
                && warning.contains("was not mapped")
        }));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn real_cgmes_tap_associations_and_table_steps_round_trip() {
        const RATIO_TAP_MRID: &str = "de305d54-75b4-431b-adb2-eb6b9e546077";
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
        let ratio_tap_component = component("tap_changer", "source-ratio-tap");
        detailed.tap_changers = vec![
            TapChanger {
                component: Some(ratio_tap_component.clone()),
                transformer: transformer.clone(),
                winding: 1,
                kind: TapChangerKind::Ratio,
                tap_position: Some(1),
                solved_tap_position: Some(0),
                low_tap_position: -1,
                neutral_tap_position: Some(0),
                normal_tap_position: Some(0),
                voltage_step_increment_percent: Some(5.0),
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
                component: None,
                transformer,
                winding: 1,
                kind: TapChangerKind::Phase,
                tap_position: Some(1),
                solved_tap_position: Some(-1),
                low_tap_position: -1,
                neutral_tap_position: Some(0),
                normal_tap_position: Some(0),
                voltage_step_increment_percent: None,
                load_tap_changing_capabilities: true,
                regulating: true,
                regulation_mode: Some(TapChangerRegulationMode::ActivePower),
                regulation_value: Some(25.0),
                target_deadband: Some(1.0),
                regulation_terminal: None,
                steps: vec![step(-1, 1.0, -2.0), step(0, 1.0, 0.0), step(1, 1.0, 2.0)],
            },
        ];
        detailed.component_metadata.push(ComponentMetadata {
            component: ratio_tap_component,
            name: Some("Imported ratio tap".into()),
            equipment_container: None,
            aliases: Vec::new(),
            external_identifiers: vec![ExternalIdentifier {
                value: RATIO_TAP_MRID.into(),
                authority: Some("CGMES".into()),
            }],
            properties: std::collections::BTreeMap::new(),
            fictitious: false,
        });

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(eq.contains("<cim:RatioTapChanger.TransformerEnd "));
        assert!(eq.contains(&format!(
            "<cim:RatioTapChanger rdf:ID=\"_{RATIO_TAP_MRID}\">"
        )));
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
        assert_eq!(
            ratio.component.as_ref().map(ComponentId::local_id),
            Some(RATIO_TAP_MRID)
        );
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
    fn tap_sv_requires_a_solved_position_and_unknown_control_terminal_is_diagnosed() {
        const TAP_MRID: &str = "20000000-0000-4000-8000-000000000001";
        let mut network = detailed_network();
        let transformer = component("branch", network.branches()[0].uid.as_deref().unwrap());
        let tap_component = component("tap_changer", TAP_MRID);
        let step = |position, rho| TapChangerStep {
            position,
            rho,
            alpha_degrees: 0.0,
            resistance_deviation_percent: 0.0,
            reactance_deviation_percent: 0.0,
            conductance_deviation_percent: 0.0,
            susceptance_deviation_percent: 0.0,
        };
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.tap_changers.push(TapChanger {
            component: Some(tap_component.clone()),
            transformer: transformer.clone(),
            winding: 1,
            kind: TapChangerKind::Ratio,
            tap_position: Some(1),
            solved_tap_position: None,
            low_tap_position: -1,
            neutral_tap_position: Some(0),
            normal_tap_position: Some(0),
            voltage_step_increment_percent: Some(5.0),
            load_tap_changing_capabilities: true,
            regulating: true,
            regulation_mode: Some(TapChangerRegulationMode::Voltage),
            regulation_value: Some(228.0),
            target_deadband: Some(2.0),
            regulation_terminal: Some(TerminalReference {
                equipment: transformer,
                terminal: 2,
            }),
            steps: vec![step(-1, 0.95), step(0, 1.0), step(1, 1.05)],
        });
        detailed.component_metadata.push(ComponentMetadata {
            component: tap_component,
            name: None,
            equipment_container: None,
            aliases: Vec::new(),
            external_identifiers: vec![ExternalIdentifier {
                value: TAP_MRID.into(),
                authority: Some("CGMES".into()),
            }],
            properties: std::collections::BTreeMap::new(),
            fictitious: false,
        });

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let ssh = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SSH.xml"))
            .map(|(_, text)| text)
            .unwrap();
        let sv = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(ssh.contains("<cim:TapChanger.step>1</cim:TapChanger.step>"));
        assert!(!sv.contains("<cim:SvTapStep "));

        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.tap_changers[0].solved_tap_position = Some(-1);
        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let sv = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_SV.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(sv.contains("<cim:SvTapStep.position>-1</cim:SvTapStep.position>"));

        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.tap_changers[0].regulation_terminal = Some(TerminalReference {
            equipment: component("branch", "missing-transformer"),
            terminal: 1,
        });
        let error = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("regulates unknown equipment terminal"));
        assert!(message.contains("branch/missing-transformer"));
    }

    #[test]
    fn invalid_operational_limits_are_diagnosed() {
        let mut network = detailed_network();
        let branch = component("branch", network.branches()[0].uid.as_deref().unwrap());
        Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap())
            .operational_limit_groups
            .push(OperationalLimitGroup {
                equipment: branch,
                terminal: 1,
                id: "invalid-limits".into(),
                properties: std::collections::BTreeMap::new(),
                selected: false,
                current_limits: Some(LoadingLimits {
                    permanent_limit: Some(-1.0),
                    permanent_limit_name: Some("invalid permanent".into()),
                    temporary_limits: vec![
                        TemporaryLimit {
                            name: "invalid temporary".into(),
                            value: f64::NAN,
                            acceptable_duration_seconds: 300,
                            fictitious: false,
                        },
                        TemporaryLimit {
                            name: "valid temporary".into(),
                            value: 1200.0,
                            acceptable_duration_seconds: 60,
                            fictitious: false,
                        },
                    ],
                }),
                active_power_limits: None,
                apparent_power_limits: None,
            });

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let dropped = output
            .warnings
            .iter()
            .filter(|warning| {
                warning.info.code == crate::diagnostics::codes::EMIT_CGMES.record_dropped.code
            })
            .collect::<Vec<_>>();
        assert!(dropped.iter().any(|warning| {
            warning.contains("permanent limit `-1`")
                && warning.contains("must be positive and finite")
        }));
        assert!(dropped.iter().any(|warning| {
            warning.contains("temporary limit `invalid temporary`")
                && warning.contains("must be positive and finite")
        }));
        let all_xml = output
            .files
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<String>();
        assert!(!all_xml.contains("invalid permanent"));
        assert!(!all_xml.contains("invalid temporary"));
        assert!(all_xml.contains("valid temporary"));
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
    fn fresh_cgmes_rejects_duplicate_mrids() {
        const SHARED_MRID: &str = "30000000-0000-4000-8000-000000000001";
        let mut network = network();
        network.loads_mut()[0].uid = Some(SHARED_MRID.into());
        network.generators_mut()[0].uid = Some(SHARED_MRID.into());
        let error = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap_err();
        let message = error.to_string();
        assert!(message.contains(SHARED_MRID));
        assert!(message.contains("defines mRID"));
        assert!(message.contains("more than once"));
    }

    #[test]
    fn rdf_graph_validation_ignores_marker_text() {
        let xml = r##"<rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
            xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
            xmlns:cim="http://iec.ch/TC57/CIM100#">
          <md:FullModel rdf:about="urn:uuid:10000000-0000-4000-8000-000000000001"/>
          <cim:BaseVoltage rdf:ID="_20000000-0000-4000-8000-000000000001">
            <cim:IdentifiedObject.name>literal rdf:ID="_not-an-object" rdf:resource="#_missing"</cim:IdentifiedObject.name>
          </cim:BaseVoltage>
        </rdf:RDF>"##;
        write::validate_rdf_graph(&[("markers.xml".into(), xml.into())]).unwrap();
    }

    #[test]
    fn rdf_graph_validation_separates_full_model_and_fragment_identifiers() {
        const SHARED_UUID: &str = "30000000-0000-4000-8000-000000000001";
        let xml = format!(
            r##"<rdf:RDF
                xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                xmlns:model="http://iec.ch/TC57/61970-552/ModelDescription/1#"
                xmlns:cim="http://iec.ch/TC57/CIM100#">
              <model:FullModel rdf:about="urn:uuid:{SHARED_UUID}">
                <model:Model.DependentOn rdf:resource="urn:uuid:{SHARED_UUID}"/>
              </model:FullModel>
              <cim:BaseVoltage rdf:ID="_{SHARED_UUID}"/>
              <cim:BaseVoltage rdf:about="#_{SHARED_UUID}"/>
            </rdf:RDF>"##
        );
        write::validate_rdf_graph(&[("shared-uuid.xml".into(), xml.clone())]).unwrap();

        let dangling = xml.replace(
            &format!("rdf:about=\"#_{SHARED_UUID}\""),
            "rdf:about=\"#_missing\"",
        );
        let error = write::validate_rdf_graph(&[("dangling.xml".into(), dangling)]).unwrap_err();
        assert!(error.to_string().contains("dangling rdf:about reference"));
        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn short_names_and_fictitious_flags_round_trip() {
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let voltage_level = detailed.voltage_levels[0].component.clone();
        let metadata = detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| metadata.component == voltage_level)
            .unwrap();
        metadata.aliases = vec![
            crate::network::ComponentAlias {
                value: "N230".into(),
                alias_type: Some("short_name".into()),
            },
            crate::network::ComponentAlias {
                value: "legacy-vl".into(),
                alias_type: Some("legacy".into()),
            },
        ];
        metadata.fictitious = true;

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        assert!(output.warnings.iter().any(|warning| {
            warning.contains("alias `legacy-vl` of type `legacy`")
                && warning.contains("has no CGMES IdentifiedObject.shortName mapping")
        }));
        let eq = output
            .files
            .iter()
            .find(|(name, _)| name.ends_with("_EQ.xml"))
            .map(|(_, text)| text)
            .unwrap();
        assert!(
            eq.contains("<cim:IdentifiedObject.shortName>N230</cim:IdentifiedObject.shortName>")
        );
        assert!(eq.contains(
            "<cim:IdentifiedObject.isFictitious>true</cim:IdentifiedObject.isFictitious>"
        ));

        let reparsed = read::read_cgmes_documents(output.files, Some("metadata")).unwrap();
        let detailed = reparsed.network.detailed_connectivity().as_deref().unwrap();
        let metadata = detailed
            .component_metadata
            .iter()
            .find(|metadata| metadata.aliases.iter().any(|alias| alias.value == "N230"))
            .unwrap();
        assert!(metadata.fictitious);
        assert!(metadata.aliases.iter().any(|alias| {
            alias.value == "N230" && alias.alias_type.as_deref() == Some("short_name")
        }));
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
    fn renamed_nested_archives_and_fractional_ratio_overflow_are_refused() {
        let mut nested = zip::ZipWriter::new(Cursor::new(Vec::new()));
        nested
            .start_file("EQ.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        nested.write_all(b"<rdf:RDF/>").unwrap();
        let nested = nested.finish().unwrap().into_inner();

        let mut outer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        outer
            .start_file("renamed.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        outer.write_all(&nested).unwrap();
        let bytes = outer.finish().unwrap().into_inner();
        let source = Source::from_memory("nested.zip", bytes).unwrap();
        let error = acquire_documents(&source).unwrap_err();
        assert!(error.to_string().contains("nested archive"));

        assert!(!exceeds_compression_ratio(20_000, 100));
        assert!(exceeds_compression_ratio(20_001, 100));
        assert!(exceeds_compression_ratio(1, 0));
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
    fn topological_nodes_require_an_exact_positive_base_voltage() {
        let files = write::write_cgmes(&network(), CgmesVersion::V3_0)
            .unwrap()
            .files;

        let without_reference = files
            .iter()
            .cloned()
            .map(|(name, text)| {
                if !name.ends_with("_TP.xml") {
                    return (name, text);
                }
                let line = text
                    .lines()
                    .find(|line| line.contains("<cim:TopologicalNode.BaseVoltage "))
                    .unwrap();
                (name, text.replacen(&format!("{line}\n"), "", 1))
            })
            .collect();
        let error = read::read_cgmes_documents(without_reference, Some("missing-base-reference"))
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("has no TopologicalNode.BaseVoltage reference")
        );

        for replacement in [None, Some("0"), Some("-230")] {
            let changed = files
                .iter()
                .cloned()
                .map(|(name, text)| {
                    if !name.ends_with("_EQ.xml") {
                        return (name, text);
                    }
                    let line = text
                        .lines()
                        .find(|line| line.contains("<cim:BaseVoltage.nominalVoltage>"))
                        .unwrap();
                    let new_line = replacement.map(|replacement| {
                        let value_start = line.find('>').unwrap() + 1;
                        let value_end = line.rfind('<').unwrap();
                        format!(
                            "{}{replacement}{}",
                            &line[..value_start],
                            &line[value_end..]
                        )
                    });
                    let text = new_line.map_or_else(
                        || text.replacen(&format!("{line}\n"), "", 1),
                        |new_line| text.replacen(line, &new_line, 1),
                    );
                    (name, text)
                })
                .collect();
            let error = read::read_cgmes_documents(changed, Some("invalid-base-voltage"))
                .err()
                .unwrap();
            let message = error.to_string();
            if let Some(replacement) = replacement {
                assert!(message.contains("nonpositive nominal voltage"));
                assert!(message.contains(replacement));
            } else {
                assert!(message.contains("without BaseVoltage.nominalVoltage"));
            }
        }
    }

    #[test]
    fn sv_power_flow_supplies_missing_or_partial_ssh_assignments() {
        let mut network = detailed_network();
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        let load_terminal = detailed
            .terminals
            .iter_mut()
            .find(|terminal| terminal.equipment.component_type() == "load")
            .unwrap();
        load_terminal.active_power_mw = Some(21.0);
        load_terminal.reactive_power_mvar = Some(6.0);
        let files = write::write_cgmes(&network, CgmesVersion::V2_4_15)
            .unwrap()
            .files;

        let without_ssh = files
            .iter()
            .filter(|(name, _)| !name.ends_with("_SSH.xml"))
            .cloned()
            .collect();
        let parsed = read::read_cgmes_documents(without_ssh, Some("sv-only")).unwrap();
        assert!((parsed.network.loads()[0].p - 21.0).abs() < 1e-12);
        assert!((parsed.network.loads()[0].q - 6.0).abs() < 1e-12);
        assert!(parsed.warnings.iter().any(|warning| {
            warning.contains("has no SSH p or q assignment")
                && warning.contains("p=21 MW")
                && warning.contains("q=6 MVAr")
        }));

        let partial_ssh = files
            .into_iter()
            .map(|(name, text)| {
                if !name.ends_with("_SSH.xml") {
                    return (name, text);
                }
                let line = text
                    .lines()
                    .find(|line| line.contains("<cim:EnergyConsumer.q>"))
                    .unwrap();
                (name, text.replacen(&format!("{line}\n"), "", 1))
            })
            .collect();
        let parsed = read::read_cgmes_documents(partial_ssh, Some("partial-ssh")).unwrap();
        assert!((parsed.network.loads()[0].p - 20.0).abs() < 1e-12);
        assert!((parsed.network.loads()[0].q - 6.0).abs() < 1e-12);
        assert!(parsed.warnings.iter().any(|warning| {
            warning.contains("has SSH p=20 MW but no SSH q assignment")
                && warning.contains("used q=6 MVAr")
        }));
    }

    #[test]
    fn fresh_cgmes_preserves_absent_xiidm_assignments() {
        let mut network = detailed_network();
        let mut shunt = Shunt::new(BusId(2), 0.0, -0.01);
        shunt.uid = Some("omitted-shunt".into());
        network.shunts_mut().push(shunt);
        let load = component("load", network.loads()[0].uid.as_deref().unwrap());
        let generator = component("generator", network.generators()[0].uid.as_deref().unwrap());
        let shunt = component("shunt", "omitted-shunt");
        {
            let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
            detailed.terminals.push(Terminal {
                component: None,
                equipment: shunt.clone(),
                terminal: 1,
                voltage_level: detailed.voltage_levels[0].component.clone(),
                bus: Some(detailed.bus_breaker_buses[1].component.clone()),
                connectable_bus: Some(detailed.bus_breaker_buses[1].component.clone()),
                node: Some(detailed.connectivity_nodes[1].component.clone()),
                connected: true,
                active_power_mw: None,
                reactive_power_mvar: None,
            });
        }

        let explicit = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let explicit_xml = explicit
            .files
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<String>();
        for property in [
            "EnergyConsumer.p",
            "EnergyConsumer.q",
            "RotatingMachine.p",
            "RotatingMachine.q",
            "RegulatingControl.targetValue",
            "LinearShuntCompensator.gPerSection",
        ] {
            assert!(explicit_xml.contains(&format!("<cim:{property}>")));
        }

        Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap()).omitted_fields = vec![
            OmittedField::new(load.clone(), OmittedFieldName::ActivePower),
            OmittedField::new(load, OmittedFieldName::ReactivePower),
            OmittedField::new(generator.clone(), OmittedFieldName::ActivePower),
            OmittedField::new(generator.clone(), OmittedFieldName::ReactivePower),
            OmittedField::new(generator, OmittedFieldName::VoltageSetpoint),
            OmittedField::new(shunt, OmittedFieldName::ShuntConductancePerSection),
        ];
        let omitted = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let omitted_xml = omitted
            .files
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<String>();
        for property in [
            "EnergyConsumer.p",
            "EnergyConsumer.q",
            "RotatingMachine.p",
            "RotatingMachine.q",
            "RegulatingControl.targetValue",
            "LinearShuntCompensator.gPerSection",
        ] {
            assert!(!omitted_xml.contains(&format!("<cim:{property}>")));
        }
        assert!(
            omitted
                .warnings
                .iter()
                .all(|warning| !warning.contains("omitted field record")),
            "supported omission records must be consumed by the writer"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // the fixture exercises each diagnostic family explicitly
    fn fresh_output_diagnoses_boundary_case_and_metadata_projection() {
        let mut network = detailed_network();
        network.case_metadata_mut().forecast_distance = Some(2);
        network.case_metadata_mut().source_model_format = Some("XIIDM".into());
        network.case_metadata_mut().minimum_validation_level =
            Some("STEADY_STATE_HYPOTHESIS".into());

        let load = component("load", network.loads()[0].uid.as_deref().unwrap());
        let generator = component("generator", network.generators()[0].uid.as_deref().unwrap());
        let branch = component("branch", network.branches()[0].uid.as_deref().unwrap());
        let first_boundary = component("boundary_line", "boundary-a");
        let second_boundary = component("boundary_line", "boundary-b");
        let detailed = Arc::make_mut(network.detailed_connectivity_mut().as_mut().unwrap());
        detailed.connectivity_nodes[0].node_number = Some(17);
        detailed.boundary_lines = vec![
            BoundaryLine {
                component: first_boundary.clone(),
                voltage_level: detailed.voltage_levels[0].component.clone(),
                active_power_setpoint_mw: 10.0,
                reactive_power_setpoint_mvar: 2.0,
                resistance_ohm: 0.1,
                reactance_ohm: 1.0,
                conductance_siemens: 0.0,
                susceptance_siemens: 0.0,
                pairing_key: Some("pair-a".into()),
                generation: None,
                calculation_load: Some(load.clone()),
                calculation_generator: Some(generator.clone()),
            },
            BoundaryLine {
                component: second_boundary.clone(),
                voltage_level: detailed.voltage_levels[0].component.clone(),
                active_power_setpoint_mw: -10.0,
                reactive_power_setpoint_mvar: -2.0,
                resistance_ohm: 0.1,
                reactance_ohm: 1.0,
                conductance_siemens: 0.0,
                susceptance_siemens: 0.0,
                pairing_key: Some("pair-a".into()),
                generation: None,
                calculation_load: None,
                calculation_generator: None,
            },
        ];
        detailed.tie_lines.push(TieLine {
            component: component("tie_line", "tie-a"),
            boundary_line1: first_boundary,
            boundary_line2: second_boundary,
            calculation_branch: Some(branch.clone()),
        });
        detailed.subnetworks.push(Subnetwork {
            component: component("subnetwork", "child-a"),
            parent: component("network", "root"),
            case_metadata: CaseMetadata {
                case_date: Some("2025-01-02T03:04:05Z".into()),
                forecast_distance: Some(1),
                source_model_format: Some("XIIDM".into()),
                minimum_validation_level: Some("EQUIPMENT".into()),
            },
            components: vec![load.clone(), generator],
        });
        detailed
            .component_metadata
            .iter_mut()
            .find(|metadata| metadata.component.component_type() == "voltage_level")
            .unwrap()
            .properties
            .insert("vendor.unmappedProperty".into(), "retained".into());
        detailed
            .operational_limit_groups
            .push(OperationalLimitGroup {
                equipment: branch,
                terminal: 1,
                id: "limit-group-with-metadata".into(),
                properties: std::collections::BTreeMap::from([(
                    "vendor.limitProperty".into(),
                    "retained".into(),
                )]),
                selected: false,
                current_limits: None,
                active_power_limits: None,
                apparent_power_limits: None,
            });
        detailed
            .omitted_fields
            .push(OmittedField::new(load, OmittedFieldName::VoltageSetpoint));

        let output = write::write_cgmes(&network, CgmesVersion::V3_0).unwrap();
        let has = |code: &str, text: &str| {
            output
                .warnings
                .iter()
                .any(|warning| warning.info.code == code && warning.contains(text))
        };
        assert!(has(
            "EMIT.CGMES.RECORD_DROPPED",
            "BoundaryLine `boundary_line/boundary-a`"
        ));
        assert!(has("EMIT.CGMES.RECORD_DROPPED", "TieLine `tie_line/tie-a`"));
        assert!(has("EMIT.CGMES.FIELD_DROPPED", "source node number 17"));
        assert!(has(
            "EMIT.CGMES.VALUE_COLLAPSED",
            "subnetwork `subnetwork/child-a`"
        ));
        assert!(has("EMIT.CGMES.FIELD_DROPPED", "forecast_distance=2"));
        assert!(has(
            "EMIT.CGMES.FIELD_DROPPED",
            "source_model_format=`XIIDM`"
        ));
        assert!(has(
            "EMIT.CGMES.FIELD_DROPPED",
            "minimum_validation_level=`STEADY_STATE_HYPOTHESIS`"
        ));
        assert!(has("EMIT.CGMES.FIELD_DROPPED", "vendor.unmappedProperty"));
        assert!(has("EMIT.CGMES.FIELD_DROPPED", "vendor.limitProperty"));
        assert!(has("EMIT.CGMES.FIELD_DROPPED", "field `voltage_setpoint`"));
    }

    #[test]
    fn dc_projection_fields_receive_exact_emission_diagnostics() {
        let network = network();
        let mut detailed = DetailedConnectivity::default();
        let dc_node = component("dc_node", "dc-node-with-voltage");
        detailed.dc_nodes.push(DcNode {
            component: dc_node.clone(),
            nominal_voltage_kv: Some(320.0),
            dc_converter_unit: None,
            dc_topological_node: None,
            voltage_kv: Some(318.0),
        });
        let dc_terminal = DcTerminal {
            component: Some(component("dc_terminal", "ground-terminal")),
            sequence_number: Some(1),
            dc_node: Some(dc_node),
            dc_topological_node: None,
            polarity: Some(DcPolarity::Positive),
            connected: Some(true),
            active_power_mw: None,
            current_a: None,
        };
        detailed.dc_grounds.push(DcGround {
            component: component("dc_ground", "ground-with-polarity"),
            equipment_container: None,
            dc_terminal: dc_terminal.clone(),
            rated_dc_voltage_kv: None,
            resistance_ohm: None,
            inductance_h: None,
        });
        detailed
            .voltage_source_converters
            .push(VoltageSourceConverter {
                component: component("voltage_source_converter", "vsc-conflicting-voltage"),
                dc_converter_unit: None,
                dc_terminal1: dc_terminal.clone(),
                dc_terminal2: dc_terminal,
                base_apparent_power_mva: None,
                minimum_active_power_mw: None,
                maximum_active_power_mw: None,
                minimum_dc_voltage_kv: None,
                maximum_dc_voltage_kv: None,
                rated_dc_voltage_kv: None,
                valve_u0_kv: None,
                number_of_valves: None,
                idle_loss_mw: None,
                switching_loss_mw_per_ampere: None,
                resistive_loss_ohm: None,
                control_mode: None,
                active_power_at_pcc_mw: None,
                reactive_power_at_pcc_mvar: None,
                target_active_power_mw: None,
                target_dc_voltage_kv: None,
                pcc_terminal: None,
                droop_curve: None,
                droop: None,
                droop_compensation: None,
                q_share: None,
                maximum_modulation_index: None,
                maximum_valve_current_a: None,
                voltage_regulator_on: None,
                voltage_setpoint_kv: None,
                reactive_power_setpoint_mvar: None,
                reactive_limits: None,
                pole_loss_active_power_mw: None,
                dc_current_a: None,
                ac_voltage_kv: None,
                dc_voltage_kv: None,
                delta_degrees: None,
                uf_kv: Some(229.0),
                uv_kv: Some(231.0),
            });
        let mut warnings =
            CgmesDiagnostics::new(&crate::diagnostics::codes::EMIT_CGMES.record_dropped);
        write::warn_unemitted_detailed_fields(
            &network,
            &detailed,
            CgmesVersion::V3_0,
            &mut warnings,
        );

        let has = |code: &str, text: &str| {
            warnings
                .iter()
                .any(|warning| warning.info.code == code && warning.contains(text))
        };
        assert!(has("EMIT.CGMES.FIELD_DROPPED", "nominal_voltage_kv=320"));
        assert!(has("EMIT.CGMES.FIELD_DROPPED", "voltage_kv=318"));
        assert!(has("EMIT.CGMES.FIELD_DROPPED", "polarity `positive`"));
        assert!(has(
            "EMIT.CGMES.VALUE_SUBSTITUTED",
            "uf=229 kV and uv=231 kV"
        ));
        assert!(has(
            "EMIT.CGMES.VALUE_SUBSTITUTED",
            "writes VsConverter.uv=231 kV"
        ));
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
        emitted_dc
            .component_metadata
            .extend(source_dc.component_metadata.clone());
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

    /// The document without its ConnectivityNode elements and without the
    /// terminal references to them.
    fn strip_connectivity_nodes(text: &str) -> String {
        let mut stripped = String::new();
        let mut inside_node = false;
        for line in text.lines() {
            if line.contains("<cim:ConnectivityNode rdf:ID=") {
                inside_node = true;
            }
            if !inside_node && !line.contains("Terminal.ConnectivityNode") {
                stripped.push_str(line);
                stripped.push('\n');
            }
            if line.contains("</cim:ConnectivityNode>") {
                inside_node = false;
            }
        }
        stripped
    }

    #[test]
    fn missing_profiles_malformed_xml_and_dangling_references_are_refused() {
        let output = write::write_cgmes(&network(), CgmesVersion::V3_0).unwrap();
        // A CGMES 3.0 EQ carries ConnectivityNodes, so it reads without TP
        // through calculated buses; stripping those nodes leaves nothing to
        // calculate from.
        let eq_only: Vec<(String, String)> = output
            .files
            .iter()
            .filter(|(name, _)| name.ends_with("_EQ.xml"))
            .cloned()
            .collect();
        let calculated = read::read_cgmes_documents(eq_only.clone(), Some("missing-tp")).unwrap();
        assert_eq!(calculated.network.buses().len(), 2);
        assert!(calculated.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::READ_CGMES_TOPOLOGY_CALCULATED.code
        }));
        let without_nodes = eq_only
            .into_iter()
            .map(|(name, text)| (name, strip_connectivity_nodes(&text)))
            .collect();
        let error = read::read_cgmes_documents(without_nodes, Some("missing-tp"))
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("the set declares no TP profile data"),
            "{error}"
        );

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

    const NODE_BREAKER_EQ: &str =
        include_str!("../../../../tests/data/cgmes/node-breaker/NodeBreaker_EQ.xml");
    const NODE_BREAKER_SSH: &str =
        include_str!("../../../../tests/data/cgmes/node-breaker/NodeBreaker_SSH.xml");

    fn node_breaker_documents() -> Vec<(String, String)> {
        vec![
            (
                "NodeBreaker_EQ.xml".to_string(),
                NODE_BREAKER_EQ.to_string(),
            ),
            (
                "NodeBreaker_SSH.xml".to_string(),
                NODE_BREAKER_SSH.to_string(),
            ),
        ]
    }

    fn bus_named<'a>(network: &'a BalancedNetwork, name: &str) -> &'a Bus {
        network
            .buses()
            .iter()
            .find(|bus| bus.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no bus named {name}"))
    }

    fn load_with_uid<'a>(network: &'a BalancedNetwork, uid: &str) -> &'a Load {
        network
            .loads()
            .iter()
            .find(|load| load.uid.as_deref() == Some(uid))
            .unwrap_or_else(|| panic!("no load with the requested identifier"))
    }

    #[test]
    fn node_breaker_set_without_tp_calculates_buses_from_switch_positions() {
        let parsed =
            read::read_cgmes_documents(node_breaker_documents(), Some("node-breaker")).unwrap();
        let network = &parsed.network;
        assert_eq!(network.buses().len(), 3);
        let bb1 = bus_named(network, "BB1");
        let n3 = bus_named(network, "N3");
        let bb2 = bus_named(network, "BB2");
        assert_eq!(bb1.kind, BusType::Ref);
        assert!(
            network
                .buses()
                .iter()
                .all(|bus| (bus.base_kv - 110.0).abs() < 1e-12)
        );

        assert_eq!(
            load_with_uid(network, "ec000000-0000-4000-8000-000000000001").bus,
            bb1.id
        );
        assert_eq!(
            load_with_uid(network, "ec000000-0000-4000-8000-000000000002").bus,
            n3.id
        );
        let disconnected = load_with_uid(network, "ec000000-0000-4000-8000-000000000003");
        assert_eq!(disconnected.bus, bb2.id);
        assert!(!disconnected.in_service);
        assert_eq!(network.generators()[0].bus, bb1.id);
        assert_eq!(
            (network.branches()[0].from, network.branches()[0].to),
            (bb1.id, bb2.id)
        );
        assert_eq!(network.switches().len(), 1);
        let open = &network.switches()[0];
        assert_eq!((open.from, open.to, open.closed), (bb1.id, n3.id, false));

        // Identities derive from the joined ConnectivityNode mRIDs: valid
        // UUIDs, never a node's own mRID, and identical on every read.
        for bus in network.buses() {
            let uid = bus.uid.as_deref().unwrap();
            assert!(uuid::Uuid::parse_str(uid).is_ok());
            assert!(!uid.starts_with("c0000000"));
        }
        let again =
            read::read_cgmes_documents(node_breaker_documents(), Some("node-breaker")).unwrap();
        let uids = |network: &BalancedNetwork| {
            network
                .buses()
                .iter()
                .map(|bus| bus.uid.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(uids(network), uids(&again.network));

        let detailed = network.detailed_connectivity().as_deref().unwrap();
        assert!(detailed.bus_breaker_buses.is_empty());
        assert_eq!(detailed.calculated_buses.len(), 3);
        let joined = detailed
            .calculated_buses
            .iter()
            .find(|bus| bus.calculated_bus == bb1.id)
            .unwrap();
        // N2 sits in a Bay, which resolves to VL1 like N1.
        assert_eq!(joined.nodes.len(), 2);
        assert_eq!(
            joined.voltage_level.local_id(),
            "a1000000-0000-4000-8000-000000000001"
        );
        assert!(
            detailed
                .voltage_levels
                .iter()
                .all(|level| level.topology_kind == TopologyKind::NodeBreaker)
        );
        let vl1 = detailed
            .voltage_levels
            .iter()
            .find(|level| level.component.local_id() == "a1000000-0000-4000-8000-000000000001")
            .unwrap();
        assert_eq!(vl1.buses, vec![bb1.id, n3.id]);
        assert!(parsed.warnings.iter().any(|warning| {
            warning.info.code == crate::diagnostics::codes::READ_CGMES_TOPOLOGY_CALCULATED.code
                && warning.contains(
                    "3 calculated bus(es) joined 5 ConnectivityNode(s) through 2 closed switch(es); 1 open switch(es)",
                )
        }));
    }

    #[test]
    fn node_breaker_ssh_switch_position_closes_the_disconnector() {
        let closed = node_breaker_documents()
            .into_iter()
            .map(|(name, text)| {
                if name.ends_with("_SSH.xml") {
                    (
                        name,
                        text.replace(
                            "<cim:Switch.open>true</cim:Switch.open>",
                            "<cim:Switch.open>false</cim:Switch.open>",
                        ),
                    )
                } else {
                    (name, text)
                }
            })
            .collect();
        let parsed = read::read_cgmes_documents(closed, Some("closed")).unwrap();
        assert_eq!(parsed.network.buses().len(), 2);
        assert!(parsed.network.switches().is_empty());
        let bb1 = bus_named(&parsed.network, "BB1").id;
        assert_eq!(
            parsed
                .network
                .loads()
                .iter()
                .filter(|load| load.bus == bb1)
                .count(),
            2
        );
    }

    #[test]
    fn node_breaker_set_without_connectivity_is_refused_with_the_missing_data_named() {
        let bus_branch = node_breaker_documents()
            .into_iter()
            .map(|(name, text)| {
                (
                    name,
                    text.replace(
                        "    <md:Model.profile>http://entsoe.eu/CIM/EquipmentOperation/3/1</md:Model.profile>\n",
                        "",
                    ),
                )
            })
            .collect();
        let mut warnings =
            CgmesDiagnostics::new(&crate::diagnostics::codes::READ_CGMES_RECORD_UNMAPPED);
        let error = read::read_cgmes_documents_into(bus_branch, Some("bus-branch"), &mut warnings)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("the set declares no TP profile data"));
        assert!(error.contains("bus branch equipment"));
        assert!(warnings.iter().any(|warning| {
            warning.info.code
                == crate::diagnostics::codes::READ_CGMES_CONNECTIVITY_INSUFFICIENT.code
        }));

        let without_nodes = node_breaker_documents()
            .into_iter()
            .map(|(name, text)| {
                if name.ends_with("_EQ.xml") {
                    (name, strip_connectivity_nodes(&text))
                } else {
                    (name, text)
                }
            })
            .collect();
        let error = read::read_cgmes_documents(without_nodes, Some("no-nodes"))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("define no ConnectivityNode records"));
    }
}
