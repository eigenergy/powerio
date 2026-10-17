//! Native binary acquisition and module assembly for the explicit profile.
use powerio_core::{Diagnostic, Error, FormatId, PioModule, Source};
use powerio_sincal::{AcquiredProject, DatabaseSnapshot, TableRecords};

use crate::BalancedNetwork;
use crate::diagnostics::codes;
use crate::format::routing::{TransmissionFormat, parse_transmission_format};

/// Explicit variant, snapshot and Access acquisition for the balanced reader.
/// Acquisition records are caller-supplied; parsing never runs external tools.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SincalBalancedReadOptions {
    /// Native variant ID; omission requires one selectable variant.
    pub variant: Option<i64>,
    /// Daily-profile snapshot in hours, without an implicit midnight default.
    pub snapshot_hours: Option<f64>,
    /// Relative companion containing import_access.py records. The primary
    /// source must be the original MDB matching the records' length and digest.
    pub acquired_tables: Option<String>,
}

fn failure(error: impl std::fmt::Display, source: &Source) -> Error {
    Error::new(&codes::PARSE_SINCAL_MALFORMED, error.to_string()).with_source(source.clone())
}

pub(in crate::format) fn handles(source: &Source) -> bool {
    match source.format() {
        Some(format) => {
            format.as_str().eq_ignore_ascii_case("sincal")
                || parse_transmission_format(format.as_str())
                    == Some(TransmissionFormat::SincalBalanced)
        }
        None => std::path::Path::new(source.name())
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("sinx")),
    }
}

pub(in crate::format) fn parse(source: Source) -> Result<PioModule<BalancedNetwork>, Error> {
    parse_with_options(source, &SincalBalancedReadOptions::default())
}

/// Parse an explicitly declared balanced SINCAL source with native selections.
///
/// # Errors
/// Missing or conflicting family selection, unsupported electrical semantics,
/// invalid snapshot, or missing/mismatched acquisition records.
pub fn parse_with_options(
    source: Source,
    options: &SincalBalancedReadOptions,
) -> Result<PioModule<BalancedNetwork>, Error> {
    let selected = source
        .format()
        .and_then(|f| parse_transmission_format(f.as_str()));
    if selected != Some(TransmissionFormat::SincalBalanced) {
        return Err(Error::new(&codes::REQUEST_SINCAL_PROFILE_REQUIRED,
            "SINCAL can contain balanced or multiconductor networks; select 'sincal-balanced' for positive sequence or 'sincal-multiconductor' for conductor-resolved interpretation. There is no balancing fallback.")
            .with_source(source));
    }
    let source = source.with_format(FormatId::new("sincal-balanced")?);
    let (snapshot, retained) = if let Some(name) = &options.acquired_tables {
        let primary = source.primary_buffer().map_err(|e| failure(e, &source))?;
        let records = source
            .referenced_buffer(&primary, name)
            .map_err(|e| failure(e, &source))?;
        TableRecords::decode(records.bytes())
            .map_err(|e| failure(e, &source))?
            .verify_source(primary.bytes())
            .map_err(|e| failure(e, &source))?;
        let snapshot = DatabaseSnapshot::decode_records(records.bytes(), options.variant)
            .map_err(|e| failure(e, &source))?;
        (snapshot, source)
    } else {
        if source
            .primary_buffer()
            .map_err(|e| failure(e, &source))?
            .bytes()
            .starts_with(b"\0\x01\0\0Standard Jet DB\0")
        {
            return Err(failure(
                "Access MDB parsing requires an explicitly supplied acquired_tables companion from import_access.py; the library does not run external tools",
                &source,
            ));
        }
        let acquired = AcquiredProject::read(&source).map_err(|e| failure(e, &source))?;
        let retained = acquired
            .source
            .with_format(FormatId::new("sincal-balanced")?);
        let snapshot = DatabaseSnapshot::decode(acquired.database.bytes(), options.variant)
            .map_err(|e| failure(e, &retained))?;
        (snapshot, retained)
    };
    let geometry = snapshot
        .drawing_geometry()
        .map_err(|e| failure(e, &retained))?;
    let inventory = retention_inventory(&snapshot).map_err(|e| failure(e, &retained))?;
    let mut network =
        super::read_balanced_snapshot_at(&snapshot, retained.name(), options.snapshot_hours)
            .map_err(|e| failure(e, &retained))?;
    let mut diagnostics = vec![
        Diagnostic::of(
            &codes::READ_SINCAL_CONVERSION_BASE,
            "The balanced profile maps native powers, voltages and impedances on a 100 MVA conversion base; this is not a native system base.",
        ),
        Diagnostic::of(
            &codes::READ_SINCAL_RETAINED_SOURCE_ONLY,
            "Fault, dynamic, protection, economic and untyped graphic data, stored calculation results, and native settings outside the selected load-flow snapshot remain only in retained source. They are not part of the typed balanced network. An acquired-table companion is caller-supplied input, not a native export or independent attestation.",
        ),
    ];
    diagnostics.extend(inventory);
    super::geometry::attach(&mut network, &geometry, &mut diagnostics)?;
    diagnostics.extend(default_diagnostics(&network)?);
    if !network.generators().is_empty() {
        diagnostics.push(Diagnostic::of(&codes::READ_SINCAL_LIMITS_UNSPECIFIED,
            "Native generator capability limits are disabled in this profile; typed P/Q limits are unbounded. This does not establish OPF capability data."));
    }
    let mut module = PioModule::parsed(network, retained, diagnostics)?;
    geometry.retain_view(&mut module)?;
    Ok(module)
}

fn default_diagnostics(
    network: &crate::network::BalancedNetwork,
) -> Result<Vec<Diagnostic>, Error> {
    let defaults = network
        .buses()
        .iter()
        .filter_map(|bus| {
            bus.extras
                .get("sincal_voltage_basis")
                .map(|value| (&bus.uid, value))
        })
        .chain(network.branches().iter().filter_map(|branch| {
            branch
                .extras
                .get("sincal_defaulted_temperature")
                .map(|value| (&branch.uid, value))
        }));
    defaults.map(|(component, value)| {
        let mut diagnostic = Diagnostic::of(&codes::READ_SINCAL_VALUE_DEFAULTED,
            format!("Versioned SINCAL field default applied to {component:?}; see structured native field and value"));
        diagnostic.insert_detail("component", serde_json::json!(component))?;
        diagnostic.insert_detail("default", value.clone())?;
        Ok(diagnostic)
    }).collect()
}

fn retention_inventory(snapshot: &DatabaseSnapshot) -> powerio_sincal::Result<Vec<Diagnostic>> {
    snapshot.retention_diagnostics(
        &codes::READ_SINCAL_RETAINED_SOURCE_ONLY,
        &[
            "Version",
            "Variant",
            "Node",
            "Element",
            "Terminal",
            "VoltageLevel",
            "CalcParameter",
            "Line",
            "Load",
            "Infeeder",
            "TwoWindingTransformer",
            "DCInfeeder",
            "OpSer",
            "OpSerVal",
            "ShuntCondensator",
        ],
        &[
            (
                "Infeeder",
                &[
                    "Flag_Har",
                    "HarImp_ID",
                    "HarVolt_ID",
                    "HarCur_ID",
                    "Flag_Reliability",
                    "SupplyType_ID",
                ],
            ),
            ("Line", &["Flag_Har", "Flag_Reliability"]),
            ("TwoWindingTransformer", &["Flag_Har", "Flag_Reliability"]),
        ],
    )
}
