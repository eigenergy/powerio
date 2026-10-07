//! Explicit family selection and source retention at the public reader boundary.

use powerio_core::{Diagnostic, Error, FormatId, PioModule, Source};
use powerio_sincal::{AcquiredProject, DatabaseSnapshot, TableRecords};

use crate::{DistSourceFormat, MulticonductorNetwork, diagnostics::codes};

/// Selection for the conductor-resolved SINCAL reader. No external tool runs
/// during parsing. Native SQLite/archive and explicitly acquired Access tables
/// are distinct input paths; both retain the original primary source bytes.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SincalReadOptions {
    /// Native variant ID. Omit only when the source has one selectable variant.
    pub variant: Option<i64>,
    /// Explicit daily-profile snapshot in hours; no implicit midnight selection.
    pub snapshot_hours: Option<f64>,
    /// Relative source companion containing the output of import_access.py.
    /// The primary source must be the original MDB whose length/hash it names.
    /// Memory sources supply this companion using Source::with_named_buffer;
    /// file sources acquire it beneath their configured acquisition root.
    pub acquired_tables: Option<String>,
    /// Experimental schema-11.5 compatibility: assume NULL Flag_LfLimit,
    /// Flag_LfCtrl, Flag_Qctrl, Flag_Macro and Kr mean inactive (zero).
    /// Active values and missing columns still reject. Applied assumptions emit
    /// warnings and survive IR serialization in network extras. Default: false.
    pub assume_inactive_source_controls: bool,
}

fn failure(error: impl std::fmt::Display, source: &Source) -> Error {
    Error::new(&codes::PARSE_SINCAL_MULTICONDUCTOR, error.to_string()).with_source(source.clone())
}

/// Parse a declared conductor-resolved SINCAL profile. Balanced-looking powers
/// do not change the network family, and unsupported modes reject atomically.
///
/// # Errors
/// Unverified input/schema, unsupported electrical modes, missing snapshot or
/// acquisition companion, mismatched MDB identity, or inconsistent values.
pub fn parse_sincal(
    source: Source,
    options: &SincalReadOptions,
) -> Result<PioModule<MulticonductorNetwork>, Error> {
    if source
        .format()
        .is_some_and(|f| !is_multiconductor_token(f.as_str()))
    {
        return Err(failure(
            "SINCAL multiconductor reader conflicts with the declared source format",
            &source,
        ));
    }
    let source = source.with_format(FormatId::new("sincal-multiconductor")?);
    let (snapshot, retained) = if let Some(name) = &options.acquired_tables {
        let primary = source.primary_buffer().map_err(|e| failure(e, &source))?;
        let records = source
            .referenced_buffer(&primary, name)
            .map_err(|e| failure(e, &source))?;
        let tables = TableRecords::decode(records.bytes()).map_err(|e| failure(e, &source))?;
        tables
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
            .with_format(FormatId::new("sincal-multiconductor")?);
        let snapshot = DatabaseSnapshot::decode(acquired.database.bytes(), options.variant)
            .map_err(|e| failure(e, &retained))?;
        (snapshot, retained)
    };
    let inventory = source_inventory(&snapshot).map_err(|e| failure(e, &retained))?;
    let mut database = super::schema::NativeDatabase::from_snapshot(snapshot)
        .map_err(|e| failure(e, &retained))?;
    database.assume_inactive_source_controls = options.assume_inactive_source_controls;
    let result = match options.snapshot_hours {
        Some(hours) => database.network_at(hours),
        None => database.network(),
    };
    let mut network = result.map_err(|e| failure(e, &retained))?;
    *network.source_format_mut() = Some(DistSourceFormat::Sincal);
    let mut diagnostics = vec![Diagnostic::of(
        &codes::READ_SINCAL_MULTICONDUCTOR_RETAINED_SOURCE_ONLY,
        "Only the selected conductor-resolved load-flow profile is typed. Fault, dynamic, protection, diagram, result and other native project data remain in retained source. An acquired-table companion is caller-supplied input, not a native export or independent attestation.",
    )];
    diagnostics.extend(inventory);
    add_provenance(&mut network, database.version, &mut diagnostics)?;
    PioModule::parsed(network, retained, diagnostics)
}

fn source_inventory(snapshot: &DatabaseSnapshot) -> powerio_sincal::Result<Vec<Diagnostic>> {
    snapshot.retention_diagnostics(
        &codes::READ_SINCAL_MULTICONDUCTOR_RETAINED_SOURCE_ONLY,
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
            "LineSeg",
            "CouplingData",
            "CoupledLine",
            "NeutralPointImp",
            "ShuntImpedance",
            "ShuntReactor",
            "ThreeWindingTransformer",
            "TransformerTap",
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

fn add_provenance(
    network: &mut MulticonductorNetwork,
    version: f64,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), Error> {
    let assumptions: std::collections::BTreeMap<_, Vec<_>> = network
        .defaulted()
        .iter()
        .filter(|(component, _)| component.starts_with("Infeeder."))
        .filter_map(|(component, fields)| {
            let controls: Vec<_> = fields
                .iter()
                .copied()
                .filter(|field| {
                    matches!(
                        *field,
                        "Flag_LfLimit" | "Flag_LfCtrl" | "Flag_Qctrl" | "Flag_Macro" | "Kr"
                    )
                })
                .collect();
            (!controls.is_empty()).then(|| (component.clone(), controls))
        })
        .collect();
    if let Some(nodes) = network
        .extras()
        .get("sincal_unconnected_nodes")
        .and_then(|v| v.as_array())
    {
        diagnostics.push(Diagnostic::of(&codes::READ_SINCAL_UNCONNECTED_NODES,
            format!("{} native nodes have no declared equipment conductors; preserved as sincal_unconnected_nodes in extras, without inventing electrical terminals", nodes.len())));
    }
    for (component, fields) in &assumptions {
        diagnostics.push(Diagnostic::of(
            &codes::READ_SINCAL_ASSUMED_INACTIVE_SOURCE_CONTROLS,
            format!("Experimental compatibility: {component} NULL fields {} assumed zero/inactive; native SINCAL behavior is unverified", fields.join(", ")),
        ).with_details(serde_json::json!({"component":component,"fields":fields,"source_value":null,"assumed_value":0,"native_semantics_verified":false}).as_object().unwrap().clone())?);
    }
    if !assumptions.is_empty() {
        network.extras_mut().insert("sincal_compatibility_assumptions".into(),
            serde_json::json!({"policy": "schema_11_5_null_source_controls_inactive", "assumed_value": 0, "components": &assumptions}));
    }
    let defaults: std::collections::BTreeMap<_, _> = network
        .defaulted()
        .iter()
        .filter_map(|(component, fields)| {
            let fields: Vec<_> = fields
                .iter()
                .copied()
                .filter(|field| {
                    !assumptions
                        .get(component)
                        .is_some_and(|assumed| assumed.contains(field))
                })
                .collect();
            (!fields.is_empty()).then(|| (component.clone(), fields))
        })
        .collect();
    if !defaults.is_empty() {
        let detail = serde_json::json!(defaults);
        network
            .extras_mut()
            .insert("sincal_defaulted_fields".into(), detail.clone());
        let mut diagnostic = Diagnostic::of(
            &codes::READ_MULTICONDUCTOR_VALUE_DEFAULTED,
            format!(
                "Versioned SINCAL field defaults applied to {} records; see structured components",
                defaults.len()
            ),
        );
        diagnostic.insert_detail("components", detail)?;
        diagnostic.insert_detail("schema_version", serde_json::json!(version))?;
        diagnostics.push(diagnostic);
    }
    Ok(())
}

pub(crate) fn is_multiconductor_token(name: &str) -> bool {
    name.chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .collect::<String>()
        .eq_ignore_ascii_case("sincalmulticonductor")
}
