//! Read and write PyPSA CSV folders.
//!
//! PyPSA's CSV folder is a directory format, so it does not fit the
//! `TextEmission { text }` API used by single-file formats. The universal
//! facade acquires the directory through `Source` and routes it through
//! `parse(..., Some("pypsa-csv"))`.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use super::{bus_kv, set_bus_kind, warn_extra_branch_rating_sets, zbase};
use crate::diagnostics::codes::EMIT_PYPSA as F;
use crate::diagnostics::{Diagnostics, codes};
use crate::network::{
    BalancedNetwork, BalancedNetworkTables, Branch, BranchCharging, Bus, BusId, BusType, Extras,
    GenCost, Generator, GeneratorEnergySource, Hvdc, Load, LoadVoltageModel, Shunt, SourceFormat,
    Storage,
};
use crate::{Error, Result};

const FMT: &str = "PyPSA CSV";

#[cfg(test)]
#[derive(Debug, Clone)]
struct PypsaCsvOutputs {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
    /// The writer's findings as structured records.
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
}

#[cfg(test)]
impl PypsaCsvOutputs {
    /// The findings as `CODE: message` lines, rendered on request.
    #[must_use]
    pub fn render_diagnostics(&self) -> Vec<String> {
        crate::diagnostics::render_diagnostics(&self.diagnostics)
    }
}

/// A directory source's file listing plus lazy acquisition, the reader's view
/// of the folder: table names resolve against the listing, and bytes come
/// through the source so the reader never touches the filesystem itself.
struct PypsaFolder<'a> {
    source: &'a powerio_core::Source,
    entries: Vec<powerio_core::ArtifactPath>,
}

impl PypsaFolder<'_> {
    fn optional(&self, name: &str) -> Result<Option<CsvTable>> {
        if !self.entries.iter().any(|entry| entry.as_str() == name) {
            return Ok(None);
        }
        let path =
            powerio_core::ArtifactPath::new(name).map_err(|error| acquisition_error(&error))?;
        let buffer = self
            .source
            .buffer(&path)
            .map_err(|error| acquisition_error(&error))?;
        let text = std::str::from_utf8(buffer.content_bytes()).map_err(|e| Error::FormatRead {
            format: FMT,
            message: format!("`{name}` is not valid UTF-8: {e}"),
        })?;
        parse_csv_table(text, name)
    }

    fn required(&self, name: &'static str) -> Result<CsvTable> {
        self.optional(name)?
            .ok_or_else(|| bad(format!("missing required `{name}`")))
    }
}

fn acquisition_error(error: &powerio_core::Error) -> Error {
    Error::FormatRead {
        format: FMT,
        message: error.to_string(),
    }
}

/// Read a PyPSA CSV folder source into the typed network.
#[allow(clippy::too_many_lines)] // direct static-component CSV mapper; each block is one PyPSA table
pub(crate) fn read_pypsa_csv_source(
    source: &powerio_core::Source,
    warnings: &mut Diagnostics,
) -> Result<BalancedNetwork> {
    // A directory source yields its walk once, so the one listing threads
    // through every consumer of it.
    let entries = source
        .entry_names()
        .map_err(|error| acquisition_error(&error))?;
    read_pypsa_csv_static(source, entries, warnings, &HashSet::new())
}

/// The static read body; `entries` is the directory's one listing and
/// `series_consumed` names the sibling series files a sequence entry
/// interprets itself, so they are not reported as ignored.
#[allow(clippy::too_many_lines)] // direct static-component CSV mapper; each block is one PyPSA table
fn read_pypsa_csv_static(
    source: &powerio_core::Source,
    entries: Vec<powerio_core::ArtifactPath>,
    warnings: &mut Diagnostics,
    series_consumed: &HashSet<String>,
) -> Result<BalancedNetwork> {
    let folder = PypsaFolder { source, entries };
    let path = Path::new(source.name());
    let network = folder.optional("network.csv")?;
    let network_row = network.as_ref().and_then(|t| t.rows.first());
    let name = network_row
        .and_then(|r| r.get("name"))
        .filter(|s| !s.is_empty())
        .cloned()
        .or_else(|| {
            path.file_name()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "pypsa".to_string());
    let base_mva = network_row
        .and_then(|r| r.f("powerio_base_mva"))
        .unwrap_or(1.0);

    let bus_table = folder.required("buses.csv")?;
    let mut raw_names = Vec::with_capacity(bus_table.rows.len());
    let mut seen = HashSet::with_capacity(bus_table.rows.len());
    for (i, row) in bus_table.rows.iter().enumerate() {
        let raw = row
            .get("name")
            .cloned()
            .ok_or_else(|| bad(format!("buses.csv row {}: missing bus name", i + 1)))?;
        if !seen.insert(raw.clone()) {
            return Err(bad(format!("buses.csv: duplicate bus name `{raw}`")));
        }
        raw_names.push(raw);
    }
    // Scheme A iff every name is a distinct positive integer: ids are the names
    // and `bus.name` stays empty. Otherwise scheme B for ALL buses: ids are
    // positions and every raw name is kept. Never mixed, so an element
    // reference resolves by name only — no numeric fallback.
    let numeric: Option<Vec<usize>> = raw_names
        .iter()
        .map(|s| s.parse::<usize>().ok().filter(|x| *x > 0))
        .collect();
    let numeric = numeric.filter(|ids| ids.iter().collect::<HashSet<_>>().len() == ids.len());

    let mut buses = Vec::with_capacity(bus_table.rows.len());
    let mut id_of_name = HashMap::with_capacity(bus_table.rows.len());
    for (i, row) in bus_table.rows.iter().enumerate() {
        let (id, bus_name) = match &numeric {
            Some(ids) => (BusId(ids[i]), None),
            None => (BusId(i + 1), Some(raw_names[i].clone())),
        };
        id_of_name.insert(raw_names[i].clone(), id);
        // v_nom drives every ohm <-> per unit conversion; defaulting it would
        // silently read line ohms as per unit (the pandapower reader holds the
        // same line for vn_kv). PyPSA omits the column only when every bus
        // keeps the default v_nom = 1, and erroring there beats misreading.
        let v_nom = row.f("v_nom").filter(|v| v.is_finite()).ok_or_else(|| {
            bad(format!(
                "buses.csv row {}: required column `v_nom` is missing or not numeric",
                i + 1
            ))
        })?;
        buses.push(Bus {
            id,
            kind: BusType::Pq,
            vm: row.f("v_mag_pu_set").unwrap_or(1.0),
            va: 0.0,
            base_kv: v_nom,
            vmax: row.f("v_mag_pu_max").unwrap_or(1.1),
            vmin: row.f("v_mag_pu_min").unwrap_or(0.9),
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: bus_name,
            uid: None,
            // PyPSA `x`/`y` are longitude/latitude; both must be present, so
            // a folder without the columns keeps `location = None`.
            location: match (
                row.f("x").filter(|v| v.is_finite()),
                row.f("y").filter(|v| v.is_finite()),
            ) {
                (Some(x), Some(y)) => Some(crate::geo::Location { x, y, kind: None }),
                _ => None,
            },
            extras: Extras::default(),
        });
    }
    let bus_pos: HashMap<BusId, usize> = buses.iter().enumerate().map(|(i, b)| (b.id, i)).collect();

    let mut loads = Vec::new();
    if let Some(table) = folder.optional("loads.csv")? {
        for (i, row) in table.rows.iter().enumerate() {
            loads.push(Load {
                bus: bus_ref("loads.csv", i + 1, row, "bus", &id_of_name)?,
                p: row.f("p_set").unwrap_or(0.0),
                q: row.f("q_set").unwrap_or(0.0),
                voltage_model: None,
                in_service: row.bool("active").unwrap_or(true),
                uid: None,
                extras: Extras::default(),
            });
        }
    }

    let mut shunts = Vec::new();
    if let Some(table) = folder.optional("shunt_impedances.csv")? {
        for (i, row) in table.rows.iter().enumerate() {
            let bus = bus_ref("shunt_impedances.csv", i + 1, row, "bus", &id_of_name)?;
            let zb = zbase(bus_kv(&buses, &bus_pos, bus), base_mva);
            shunts.push(Shunt {
                bus,
                g: row.f("g").unwrap_or(0.0) * zb * base_mva,
                b: row.f("b").unwrap_or(0.0) * zb * base_mva,
                in_service: row.bool("active").unwrap_or(true),
                section_count: None,
                control: None,
                uid: None,
                extras: Extras::default(),
            });
        }
    }

    let mut generators = Vec::new();
    if let Some(table) = folder.optional("generators.csv")? {
        for (i, row) in table.rows.iter().enumerate() {
            let bus = bus_ref("generators.csv", i + 1, row, "bus", &id_of_name)?;
            let control = row.get("control").map_or("", String::as_str);
            // "PQ", empty, and anything unrecognized leave the bus kind alone.
            if control.eq_ignore_ascii_case("slack") {
                set_bus_kind(&mut buses, &bus_pos, bus, BusType::Ref);
            } else if control.eq_ignore_ascii_case("pv") {
                set_bus_kind(&mut buses, &bus_pos, bus, BusType::Pv);
            }
            let p_nom = row
                .f("p_nom")
                .unwrap_or_else(|| row.f("p_set").unwrap_or(0.0).abs());
            let pmax = p_nom * row.f("p_max_pu").unwrap_or(1.0);
            let pmin = p_nom * row.f("p_min_pu").unwrap_or(0.0);
            let c1 = row.f("marginal_cost");
            let c2 = row.f("marginal_cost_quadratic");
            generators.push(Generator {
                bus,
                energy_source: GeneratorEnergySource::default(),
                pg: row.f("p_set").unwrap_or(0.0),
                qg: row.f("q_set").unwrap_or(0.0),
                pmax,
                pmin,
                qmax: f64::INFINITY,
                qmin: f64::NEG_INFINITY,
                vg: row.f("v_mag_pu_set").unwrap_or(1.0),
                mbase: base_mva,
                in_service: row.bool("active").unwrap_or(true),
                cost: match (c2, c1) {
                    (Some(q), c) => Some(GenCost {
                        model: 2,
                        startup: 0.0,
                        shutdown: 0.0,
                        ncost: 3,
                        // PyPSA defaults marginal_cost to 0, so a quadratic
                        // without a linear column keeps the quadratic term.
                        coeffs: vec![q, c.unwrap_or(0.0), 0.0],
                    }),
                    (None, Some(c)) => Some(GenCost {
                        model: 2,
                        startup: 0.0,
                        shutdown: 0.0,
                        ncost: 2,
                        coeffs: vec![c, 0.0],
                    }),
                    (None, None) => None,
                },
                caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
                voltage_regulation_on: true,
                regulating_terminal: None,
                regulated_bus: None,
                active_power_control: None,
                uid: None,
            });
        }
    }

    let mut branches = Vec::new();
    if let Some(table) = folder.optional("lines.csv")? {
        for (i, row) in table.rows.iter().enumerate() {
            let from = bus_ref("lines.csv", i + 1, row, "bus0", &id_of_name)?;
            let to = bus_ref("lines.csv", i + 1, row, "bus1", &id_of_name)?;
            // PyPSA per-unitizes line ohms on the BUS0 v_nom
            // (BalancedNetwork.calculate_dependent_values), not bus1.
            let zb = zbase(bus_kv(&buses, &bus_pos, from), base_mva);
            let b = row.f("b").unwrap_or(0.0) * zb;
            let g = row.f("g").unwrap_or(0.0) * zb;
            branches.push(Branch {
                name: None,
                from,
                to,
                r: row.f("r").unwrap_or(0.0) / zb,
                x: row.f("x").unwrap_or(0.0) / zb,
                b,
                charging: Some(BranchCharging {
                    g_fr: g / 2.0,
                    b_fr: b / 2.0,
                    g_to: g / 2.0,
                    b_to: b / 2.0,
                }),
                rate_a: row.f("s_nom").unwrap_or(0.0),
                rate_b: 0.0,
                rate_c: 0.0,
                rating_sets: Vec::new(),
                current_ratings: None,
                tap: 0.0,
                shift: 0.0,
                in_service: row.bool("active").unwrap_or(true),
                angmin: row.f("v_ang_min").unwrap_or(-360.0),
                angmax: row.f("v_ang_max").unwrap_or(360.0),
                control: None,
                solution: None,
                uid: None,
                route: None,
                extras: Extras::default(),
            });
        }
    }
    if let Some(table) = folder.optional("transformers.csv")? {
        for (i, row) in table.rows.iter().enumerate() {
            let from = bus_ref("transformers.csv", i + 1, row, "bus0", &id_of_name)?;
            let to = bus_ref("transformers.csv", i + 1, row, "bus1", &id_of_name)?;
            // PyPSA stores transformer impedances per unit on the transformer's
            // own s_nom base; rebase to the system base.
            let s_nom = row.f("s_nom").unwrap_or(0.0);
            if s_nom <= 0.0 {
                let xf_name = row.get("name").cloned().unwrap_or_default();
                return Err(bad(format!(
                    "transformers.csv row {} (`{xf_name}`): s_nom must be positive to rebase impedances (got {s_nom})",
                    i + 1
                )));
            }
            let k = base_mva / s_nom;
            let b = row.f("b").unwrap_or(0.0) * s_nom / base_mva;
            let g = row.f("g").unwrap_or(0.0) * s_nom / base_mva;
            branches.push(Branch {
                name: None,
                from,
                to,
                r: row.f("r").unwrap_or(0.0) * k,
                x: row.f("x").unwrap_or(0.0) * k,
                b,
                charging: Some(BranchCharging {
                    g_fr: g,
                    b_fr: b,
                    g_to: 0.0,
                    b_to: 0.0,
                }),
                rate_a: s_nom,
                rate_b: 0.0,
                rate_c: 0.0,
                rating_sets: Vec::new(),
                current_ratings: None,
                tap: row.f("tap_ratio").unwrap_or(1.0),
                shift: row.f("phase_shift").unwrap_or(0.0),
                in_service: row.bool("active").unwrap_or(true),
                angmin: -360.0,
                angmax: 360.0,
                control: None,
                solution: None,
                uid: None,
                route: None,
                extras: Extras::default(),
            });
        }
    }

    let mut storage = Vec::new();
    if let Some(table) = folder.optional("storage_units.csv")? {
        for (i, row) in table.rows.iter().enumerate() {
            let p_nom = row.f("p_nom").unwrap_or(0.0);
            let max_hours = row.f("max_hours").unwrap_or(0.0);
            storage.push(Storage {
                bus: bus_ref("storage_units.csv", i + 1, row, "bus", &id_of_name)?,
                ps: row.f("p_set").unwrap_or(0.0),
                qs: row.f("q_set").unwrap_or(0.0),
                energy: row.f("state_of_charge_initial").unwrap_or(0.0),
                energy_rating: p_nom * max_hours,
                charge_rating: p_nom,
                discharge_rating: p_nom,
                charge_efficiency: row.f("efficiency_store").unwrap_or(1.0),
                discharge_efficiency: row.f("efficiency_dispatch").unwrap_or(1.0),
                thermal_rating: p_nom,
                current_rating: None,
                qmin: f64::NEG_INFINITY,
                qmax: f64::INFINITY,
                r: 0.0,
                x: 0.0,
                p_loss: 0.0,
                q_loss: 0.0,
                in_service: row.bool("active").unwrap_or(true),
                active_power_control: None,
                uid: None,
                extras: Extras::default(),
            });
        }
    }

    let mut hvdc = Vec::new();
    if let Some(table) = folder.optional("links.csv")? {
        for (i, row) in table.rows.iter().enumerate() {
            let from = bus_ref("links.csv", i + 1, row, "bus0", &id_of_name)?;
            let to = bus_ref("links.csv", i + 1, row, "bus1", &id_of_name)?;
            let efficiency = row.f("efficiency").unwrap_or(1.0);
            let p_nom = row.f("p_nom").unwrap_or(0.0);
            let pf = row.f("p_set").unwrap_or(0.0);
            hvdc.push(Hvdc {
                from,
                to,
                in_service: row.bool("active").unwrap_or(true),
                pf,
                pt: Hvdc::calc_delivered_power(pf, 0.0, 1.0 - efficiency),
                qf: 0.0,
                qt: 0.0,
                vf: 1.0,
                vt: 1.0,
                pmin: p_nom * row.f("p_min_pu").unwrap_or(0.0),
                pmax: p_nom * row.f("p_max_pu").unwrap_or(1.0),
                qminf: 0.0,
                qmaxf: 0.0,
                qmint: 0.0,
                qmaxt: 0.0,
                loss0: 0.0,
                loss1: 1.0 - efficiency,
                resistance_ohm: None,
                nominal_voltage_kv: None,
                converters_mode: None,
                converter1: None,
                converter2: None,
                cost: None,
                uid: None,
                extras: Extras::default(),
            });
        }
        if !table.rows.is_empty() {
            warnings.push(&codes::READ_PYPSA_VALUE_APPROXIMATED, format!(
                "links.csv: {} links read as HVDC lines; PyPSA links carry no reactive or voltage data (q limits 0, voltage setpoints 1.0)",
                table.rows.len()
            ));
        }
    }
    if let Some(table) = folder.optional("stores.csv")? {
        if !table.rows.is_empty() {
            warnings.push(
                &codes::READ_PYPSA_TABLE_UNSUPPORTED,
                format!(
                    "stores.csv ignored ({} rows): PyPSA stores are not mapped",
                    table.rows.len()
                ),
            );
        }
    }

    // A real PyPSA export can carry its data in time series siblings
    // (`loads-p_set.csv`, `generators-p_max_pu.csv`, ...); reading only the
    // static tables and saying nothing would present a zero-load network as a
    // clean parse. Name every CSV this reader did not open.
    let consumed = [
        "network.csv",
        "snapshots.csv",
        "buses.csv",
        "loads.csv",
        "shunt_impedances.csv",
        "generators.csv",
        "lines.csv",
        "transformers.csv",
        "storage_units.csv",
        "links.csv",
        "stores.csv",
    ];
    let mut unread: Vec<String> = folder
        .entries
        .iter()
        .map(powerio_core::ArtifactPath::as_str)
        .filter(|name| {
            !name.contains('/')
                && Path::new(name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
                && !consumed.contains(name)
                && !series_consumed.contains(*name)
        })
        .map(str::to_owned)
        .collect();
    unread.sort();
    for file in unread {
        warnings.push(&codes::READ_PYPSA_TABLE_UNSUPPORTED, format!(
            "`{file}` ignored: only the static element tables are read (time series and other tables are not modeled)"
        ));
    }

    let net = BalancedNetwork::from_tables(BalancedNetworkTables {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        geo: super::geographic_meta(&buses),
        case_metadata: crate::network::CaseMetadata::default(),
        detailed_connectivity: None,
        buses: buses.into(),
        loads: loads.into(),
        shunts: shunts.into(),
        static_var_compensators: Vec::new().into(),
        branches: branches.into(),
        switches: Vec::new().into(),
        generators: generators.into(),
        storage: storage.into(),
        hvdc: hvdc.into(),
        transformers_3w: Vec::new().into(),
        areas: Vec::new().into(),
        solver: None,
        source_format: SourceFormat::PypsaCsv,
    });
    // This reader bypasses the read_source funnel (directory input), so it
    // guards against a hollow case itself.
    crate::format::reject_empty_case(&net, FMT)?;
    net.check_references(FMT)?;
    Ok(net)
}

/// Write the complete PyPSA CSV folder at `out_dir` through the no-replace
/// destination commit: the inventory is staged completely and moved onto
/// `out_dir` only when no entry exists there, so a refused write leaves the
/// caller's filesystem byte for byte as it was.
///
/// # Errors
/// A refused commit: `out_dir` already exists, cannot be staged, or the
/// destination cannot commit without risking replacement.
#[allow(clippy::missing_panics_doc)] // the destination kind is ours by construction
#[cfg(test)]
fn write_pypsa_csv_folder(
    net: &BalancedNetwork,
    out_dir: impl AsRef<Path>,
) -> std::result::Result<PypsaCsvOutputs, powerio_core::Error> {
    let (artifacts, diagnostics) = pypsa_csv_artifacts(net);
    let inventory = artifacts
        .into_iter()
        .map(|(name, text)| {
            powerio_core::MemoryArtifact::new(
                powerio_core::ArtifactPath::new(name).expect("the writer emits fixed valid names"),
                text.into_bytes(),
            )
        })
        .collect();
    let result = powerio_core::Destination::path(out_dir.as_ref()).__commit_artifacts(
        true,
        powerio_core::Fidelity::Canonical,
        inventory,
        Vec::new(),
    )?;
    let powerio_core::EmittedOutput::Path { root, artifacts } = result.into_output() else {
        unreachable!("a path destination returns a path output")
    };
    Ok(PypsaCsvOutputs {
        dir: root,
        files: artifacts,
        diagnostics,
    })
}

/// The complete PyPSA CSV folder as an in-memory artifact inventory:
/// `(file name, content)` pairs in emission order, plus the writer's
/// findings. Both the folder writer and the `Destination` write commit these
/// as one atomic inventory.
// One emission body shared by the streaming and inventory writers; the
// length is the format's table count, not branching depth.
#[allow(clippy::too_many_lines)]
pub(crate) fn pypsa_csv_artifacts(
    net: &BalancedNetwork,
) -> (
    Vec<(&'static str, String)>,
    Vec<crate::diagnostics::Diagnostic>,
) {
    let mut files: Vec<(&'static str, String)> = Vec::new();
    let mut write_file = |name: &'static str, text: String| {
        files.push((name, text));
    };
    let mut warnings = Diagnostics::new();
    // Element tables must reference buses by the same key buses.csv is indexed
    // on, and PyPSA requires those keys to be unique for its joins. A bus is
    // keyed by its name only when the name collides with no other bus's name
    // or id string; colliding buses fall back to their numeric id, which is
    // unique by construction and (per the same rule) cannot displace a kept
    // name.
    let mut name_counts: HashMap<&str, usize> = HashMap::new();
    for b in net.buses() {
        if let Some(n) = &b.name {
            *name_counts.entry(n.as_str()).or_insert(0) += 1;
        }
    }
    let id_owner: HashMap<String, BusId> = net
        .buses()
        .iter()
        .map(|b| (b.id.0.to_string(), b.id))
        .collect();
    let mut displaced: Vec<String> = Vec::new();
    let key_of: HashMap<BusId, String> = net
        .buses()
        .iter()
        .map(|b| {
            let key = match &b.name {
                Some(n)
                    if name_counts[n.as_str()] == 1
                        && id_owner.get(n).is_none_or(|&owner| owner == b.id) =>
                {
                    n.clone()
                }
                Some(n) => {
                    displaced.push(format!("`{n}`"));
                    b.id.0.to_string()
                }
                None => b.id.0.to_string(),
            };
            (b.id, key)
        })
        .collect();
    if !displaced.is_empty() {
        displaced.sort();
        displaced.dedup();
        warnings.push(&codes::READ_PYPSA_NAME_REMAPPED, format!(
            "buses.csv: bus names {} collide with another bus name or id; those buses are keyed by their numeric id instead",
            displaced.join(", ")
        ));
    }
    if !net.hvdc().is_empty() {
        warnings.push(
            &F.record_dropped,
            format!(
                "{} dcline(s) dropped: the PyPSA CSV writer does not model HVDC links",
                net.hvdc().len()
            ),
        );
    }
    if !net.transformers_3w().is_empty() {
        warnings.push(&F.record_dropped, format!(
            "{} 3-winding transformer(s) dropped: the PyPSA CSV writer emits no 3-winding transformer",
            net.transformers_3w().len()
        ));
    }
    if net
        .buses()
        .iter()
        .any(|b| b.evhi.is_some() || b.evlo.is_some())
    {
        warnings.push(
            &F.field_dropped,
            "emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band",
        );
    }
    if net.generators().iter().any(Generator::has_caps) {
        warnings.push(&F.field_dropped, "generator capability/ramp columns dropped: a PyPSA generators.csv row states p_nom and no reactive capability curve point");
    }
    let voltage_loads = net
        .loads()
        .iter()
        .filter(|l| {
            l.voltage_model
                .as_ref()
                .is_some_and(LoadVoltageModel::has_non_matpower_fields)
        })
        .count();
    if voltage_loads > 0 {
        warnings.push(&F.field_dropped, format!(
            "{voltage_loads} voltage dependent load model(s) dropped: PyPSA loads.csv carries static p_set/q_set only"
        ));
    }
    let isolated = net
        .buses()
        .iter()
        .filter(|b| b.kind == BusType::Isolated)
        .count();
    if isolated > 0 {
        warnings.push(&F.field_dropped, format!(
            "{isolated} isolated bus(es) written without status: PyPSA buses carry no active flag, they read back in service"
        ));
    }
    let xf_angles = net
        .branches()
        .iter()
        .filter(|b| b.is_transformer() && b.has_angle_limits())
        .count();
    if xf_angles > 0 {
        warnings.push(&F.field_dropped, format!(
            "{xf_angles} transformer angle limit(s) dropped: transformers.csv carries no v_ang_min/v_ang_max"
        ));
    }
    let rate_bc = net
        .branches()
        .iter()
        .filter(|b| {
            super::nonzero_differs(b.rate_b, b.rate_a) || super::nonzero_differs(b.rate_c, b.rate_a)
        })
        .count();
    if rate_bc > 0 {
        warnings.push(&F.field_dropped, format!(
            "{rate_bc} branch rate_b/rate_c value set(s) dropped: PyPSA carries one s_nom rating"
        ));
    }
    let current_ratings = net
        .branches()
        .iter()
        .filter(|b| b.current_ratings.is_some())
        .count();
    if current_ratings > 0 {
        warnings.push(&F.field_dropped, format!(
            "{current_ratings} branch current rating record(s) dropped: a PyPSA lines.csv row states s_nom in MVA and no current rating"
        ));
    }
    warn_extra_branch_rating_sets(&F, "PyPSA CSV", net, &mut warnings);
    super::warn_dropped_areas(&F, "PyPSA CSV", net, &mut warnings);
    let branch_solutions = net
        .branches()
        .iter()
        .filter(|b| b.solution.is_some())
        .count();
    if branch_solutions > 0 {
        warnings.push(&F.field_dropped, format!(
            "{branch_solutions} branch solution value set(s) dropped: a PyPSA component CSV states case data, and a flow belongs to a result time series over snapshots this profile does not state"
        ));
    }
    let terminal_charging = net
        .branches()
        .iter()
        .filter(|b| pypsa_loses_terminal_charging(b))
        .count();
    if terminal_charging > 0 {
        warnings.push(&F.value_collapsed, format!(
            "{terminal_charging} branch terminal admittance record(s) collapsed: PyPSA CSV supports symmetric line shunts and one-sided transformer shunts only"
        ));
    }
    if let Some(message) = super::missing_reference_warning(net) {
        warnings.push(&F.reference_missing, message);
    }
    if let Some(message) = super::normalized_tap_warning(net) {
        warnings.push(&F.element_relabeled, message);
    }
    // Exact compares are the point: any deviation from the symmetric, no-loss
    // shape the round trip preserves means a field is dropped on write.
    #[allow(clippy::float_cmp)]
    let lossy = net
        .storage()
        .iter()
        .filter(|st| {
            let p_nom = st.charge_rating.max(st.discharge_rating);
            st.charge_rating != st.discharge_rating
                || st.thermal_rating != p_nom
                || st.qmin.is_finite()
                || st.qmax.is_finite()
                || st.r != 0.0
                || st.x != 0.0
                || st.p_loss != 0.0
                || st.q_loss != 0.0
        })
        .count();
    if lossy > 0 {
        warnings.push(&F.value_collapsed, format!(
            "{lossy} storage units lose fields PyPSA storage_units cannot carry (asymmetric charge/discharge ratings collapse to p_nom = max; thermal_rating, qmin/qmax, r/x, p_loss/q_loss dropped)"
        ));
    }

    write_file("network.csv", network_csv(net));
    write_file("snapshots.csv", ",snapshot\n0,now\n".to_owned());
    write_file("buses.csv", buses_csv(net, &key_of));
    write_file(
        "generators.csv",
        generators_csv(net, &key_of, &mut warnings),
    );
    // The v_nom per bus, shared by the writers that rebase impedances.
    let kv_of: HashMap<BusId, f64> = net.buses().iter().map(|b| (b.id, b.base_kv)).collect();
    write_file("loads.csv", loads_csv(net, &key_of));
    write_file("lines.csv", lines_csv(net, &key_of, &kv_of));
    let transformers = transformers_csv(net, &key_of);
    if transformers.lines().count() > 1 {
        write_file("transformers.csv", transformers);
    }
    if !net.shunts().is_empty() {
        write_file("shunt_impedances.csv", shunts_csv(net, &key_of, &kv_of));
    }
    if !net.storage().is_empty() {
        write_file("storage_units.csv", storage_csv(net, &key_of));
    }
    (files, warnings.into_records())
}

fn network_csv(net: &BalancedNetwork) -> String {
    format!(
        "name,srid,powerio_base_mva\n{},4326,{}\n",
        esc(net.name()),
        net.base_mva()
    )
}

fn buses_csv(net: &BalancedNetwork, key_of: &HashMap<BusId, String>) -> String {
    // The coordinate columns appear only when the case carries locations, so
    // a case without geometry writes exactly as before. PyPSA defaults a
    // missing cell to 0, so a located case writes empty cells for the odd
    // bus without a point.
    let write_locations = net.buses().iter().any(|b| b.location.is_some());
    let mut s = String::from(if write_locations {
        "name,v_nom,v_mag_pu_set,v_mag_pu_min,v_mag_pu_max,x,y\n"
    } else {
        "name,v_nom,v_mag_pu_set,v_mag_pu_min,v_mag_pu_max\n"
    });
    for b in net.buses() {
        let _ = write!(
            s,
            "{},{},{},{},{}",
            key_for(key_of, b.id),
            b.base_kv,
            b.vm,
            b.vmin,
            b.vmax
        );
        if write_locations {
            match b.location {
                Some(location) => {
                    let _ = write!(s, ",{},{}", location.x, location.y);
                }
                None => s.push_str(",,"),
            }
        }
        s.push('\n');
    }
    s
}

#[allow(clippy::too_many_lines)]
// one column expression per PyPSA generator attribute
// The exact mbase compare is the point: any deviation from the system base is
// information the PyPSA table cannot carry.
#[allow(clippy::float_cmp)]
fn generators_csv(
    net: &BalancedNetwork,
    key_of: &HashMap<BusId, String>,
    warnings: &mut Diagnostics,
) -> String {
    let mut s = String::from(
        "name,bus,control,p_nom,p_set,q_set,p_min_pu,p_max_pu,marginal_cost,marginal_cost_quadratic,active,v_mag_pu_set\n",
    );
    let bus_kind: HashMap<BusId, BusType> = net.buses().iter().map(|b| (b.id, b.kind)).collect();
    let mut dropped = 0usize;
    let mut truncated = 0usize;
    let mut empty = 0usize;
    let mut unbounded = 0usize;
    for (i, g) in net.generators().iter().enumerate() {
        let p_nom = if g.pmax.is_finite() && g.pmax > 0.0 {
            g.pmax
        } else {
            g.pg.abs().max(1.0)
        };
        // Keep the LOWEST order terms: a polynomial's coeffs run high to low.
        let (c2, c1) = match g.cost.as_ref() {
            Some(c) if c.model == 2 => {
                let n = c.coeffs.len();
                if n == 0 {
                    empty += 1;
                } else if n > 3 {
                    truncated += 1;
                }
                (
                    if n >= 3 { c.coeffs[n - 3] } else { 0.0 },
                    if n >= 2 { c.coeffs[n - 2] } else { 0.0 },
                )
            }
            Some(_) => {
                dropped += 1;
                (0.0, 0.0)
            }
            None => (0.0, 0.0),
        };
        let _ = writeln!(
            s,
            "gen_{},{},{},{},{},{},{},{},{},{},{},{}",
            i + 1,
            key_for(key_of, g.bus),
            match bus_kind.get(&g.bus).copied() {
                Some(BusType::Ref) => "Slack",
                Some(BusType::Pv) => "PV",
                _ => "PQ",
            },
            p_nom,
            g.pg,
            g.qg,
            if p_nom == 0.0 || !g.pmin.is_finite() {
                if !g.pmin.is_finite() {
                    unbounded += 1;
                }
                0.0
            } else {
                g.pmin / p_nom
            },
            if p_nom == 0.0 || !g.pmax.is_finite() {
                if !g.pmax.is_finite() {
                    unbounded += 1;
                }
                1.0
            } else {
                g.pmax / p_nom
            },
            c1,
            c2,
            g.in_service,
            g.vg
        );
    }
    if dropped > 0 {
        warnings.push(&F.field_dropped, format!(
            "{dropped} generator costs dropped: PyPSA carries marginal_cost/marginal_cost_quadratic (model 2) only"
        ));
    }
    if truncated > 0 {
        warnings.push(
            &F.value_truncated,
            format!(
                "{truncated} generator costs truncated to quadratic for PyPSA marginal cost columns"
            ),
        );
    }
    if empty > 0 {
        warnings.push(
            &F.value_defaulted,
            format!("{empty} generator costs had no coefficients and were written as zero"),
        );
    }
    if unbounded > 0 {
        warnings.push(&F.value_defaulted, format!(
            "{unbounded} non-finite generator p limit(s) written as the PyPSA defaults (p_min_pu 0, p_max_pu 1)"
        ));
    }
    let q_limited = net
        .generators()
        .iter()
        .filter(|g| g.qmin.is_finite() || g.qmax.is_finite())
        .count();
    if q_limited > 0 {
        warnings.push(&F.field_dropped, format!(
            "{q_limited} generator reactive limit(s) dropped: PyPSA generators carry no q bounds"
        ));
    }
    let off_base = net
        .generators()
        .iter()
        .filter(|g| g.mbase != 0.0 && g.mbase != net.base_mva())
        .count();
    if off_base > 0 {
        warnings.push(&F.field_dropped, format!(
            "{off_base} generator machine base(s) (mbase) dropped: PyPSA carries no per generator MVA base"
        ));
    }
    s
}

fn loads_csv(net: &BalancedNetwork, key_of: &HashMap<BusId, String>) -> String {
    let mut s = String::from("name,bus,p_set,q_set,active\n");
    for (i, l) in net.loads().iter().enumerate() {
        let _ = writeln!(
            s,
            "load_{},{},{},{},{}",
            i + 1,
            key_for(key_of, l.bus),
            l.p,
            l.q,
            l.in_service
        );
    }
    s
}

fn pypsa_loses_terminal_charging(br: &Branch) -> bool {
    let charging = br.calc_terminal_charging();
    if br.is_transformer() {
        charging.g_to.abs() > f64::EPSILON || charging.b_to.abs() > f64::EPSILON
    } else {
        (charging.g_fr - charging.g_to).abs() > f64::EPSILON
            || (charging.b_fr - charging.b_to).abs() > f64::EPSILON
    }
}

fn lines_csv(
    net: &BalancedNetwork,
    key_of: &HashMap<BusId, String>,
    kv_of: &HashMap<BusId, f64>,
) -> String {
    let mut s = String::from("name,bus0,bus1,r,x,b,g,s_nom,v_ang_min,v_ang_max,active\n");
    for (i, br) in net
        .branches()
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.is_transformer())
    {
        // PyPSA per-unitizes line ohms on the BUS0 v_nom, not bus1.
        let zb = zbase(*kv_of.get(&br.from).unwrap_or(&0.0), net.base_mva());
        let charging = br.calc_terminal_charging();
        let _ = writeln!(
            s,
            "line_{},{},{},{},{},{},{},{},{},{},{}",
            i + 1,
            key_for(key_of, br.from),
            key_for(key_of, br.to),
            br.r * zb,
            br.x * zb,
            charging.calc_total_b() / zb,
            (charging.g_fr + charging.g_to) / zb,
            br.rate_a,
            br.angmin,
            br.angmax,
            br.in_service
        );
    }
    s
}

fn transformers_csv(net: &BalancedNetwork, key_of: &HashMap<BusId, String>) -> String {
    let mut s = String::from("name,bus0,bus1,r,x,b,g,s_nom,tap_ratio,phase_shift,active\n");
    for (i, br) in net
        .branches()
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_transformer())
    {
        // PyPSA wants impedances per unit on the transformer's own s_nom base
        // and a positive s_nom; rate_a == 0 (unlimited) falls back to the
        // system base so the rebase is the identity.
        let s_nom = if br.rate_a > 0.0 {
            br.rate_a
        } else {
            net.base_mva()
        };
        let charging = br.charging.unwrap_or(BranchCharging {
            g_fr: 0.0,
            b_fr: br.calc_total_charging_b(),
            g_to: 0.0,
            b_to: 0.0,
        });
        let _ = writeln!(
            s,
            "transformer_{},{},{},{},{},{},{},{},{},{},{}",
            i + 1,
            key_for(key_of, br.from),
            key_for(key_of, br.to),
            br.r * s_nom / net.base_mva(),
            br.x * s_nom / net.base_mva(),
            charging.b_fr * net.base_mva() / s_nom,
            charging.g_fr * net.base_mva() / s_nom,
            s_nom,
            br.calc_effective_tap(),
            br.shift,
            br.in_service
        );
    }
    s
}

fn shunts_csv(
    net: &BalancedNetwork,
    key_of: &HashMap<BusId, String>,
    kv_of: &HashMap<BusId, f64>,
) -> String {
    let mut s = String::from("name,bus,g,b,active\n");
    for (i, sh) in net.shunts().iter().enumerate() {
        let zb = zbase(*kv_of.get(&sh.bus).unwrap_or(&0.0), net.base_mva());
        let _ = writeln!(
            s,
            "shunt_{},{},{},{},{}",
            i + 1,
            key_for(key_of, sh.bus),
            sh.g / (zb * net.base_mva()),
            sh.b / (zb * net.base_mva()),
            sh.in_service
        );
    }
    s
}

fn storage_csv(net: &BalancedNetwork, key_of: &HashMap<BusId, String>) -> String {
    let mut s = String::from(
        "name,bus,p_nom,max_hours,p_set,q_set,state_of_charge_initial,efficiency_store,efficiency_dispatch,cyclic_state_of_charge\n",
    );
    for (i, st) in net.storage().iter().enumerate() {
        let p_nom = st.charge_rating.max(st.discharge_rating);
        let max_hours = if p_nom > 0.0 {
            st.energy_rating / p_nom
        } else {
            0.0
        };
        let _ = writeln!(
            s,
            "storage_{},{},{},{},{},{},{},{},{},false",
            i + 1,
            key_for(key_of, st.bus),
            p_nom,
            max_hours,
            st.ps,
            st.qs,
            st.energy,
            st.charge_efficiency,
            st.discharge_efficiency
        );
    }
    s
}

#[derive(Debug)]
struct CsvTable {
    headers: Vec<String>,
    rows: Vec<CsvRow>,
}

/// One record: the fields actually present, resolved through the table's
/// shared header index, so a table's retained size is its own field count
/// rather than header count times row count.
#[derive(Debug)]
struct CsvRow {
    column_of: std::sync::Arc<HashMap<String, usize>>,
    fields: Vec<String>,
}

impl CsvRow {
    fn get(&self, key: &str) -> Option<&String> {
        let column = *self.column_of.get(key)?;
        self.fields.get(column).filter(|s| !s.is_empty())
    }
    fn f(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|s| s.parse().ok())
    }
    fn bool(&self, key: &str) -> Option<bool> {
        self.get(key)
            .and_then(|s| match s.to_ascii_lowercase().as_str() {
                "true" | "1" => Some(true),
                "false" | "0" => Some(false),
                _ => None,
            })
    }
}

fn bad(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}

/// Which per point field one supported `{component}-{attribute}.csv` column
/// patches.
#[derive(Clone, Copy)]
enum SeriesField {
    LoadP,
    LoadQ,
    GenPg,
    GenQg,
    GenPmax,
    GenPmin,
    GenVg,
    BusVm,
    BusVa,
}

/// A parsed PyPSA sequence: the per snapshot networks, whether any calculation
/// input varied (otherwise only solution quantities varied), and the reader's
/// findings.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PypsaCsvSequence {
    pub series: powerio_core::TimeSeries<BalancedNetwork>,
    /// False when every varying column is a solution quantity, so the
    /// sequence is one fixed network with changing operating points.
    pub inputs_vary: bool,
    /// Whether any recognized series column varied at all. A declared
    /// snapshot axis with no series siblings preserves the axis as networks
    /// sharing every table; it is not an operating point series.
    pub has_varying_columns: bool,
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
}

/// The declared snapshot axis of a PyPSA CSV folder, probed from entry names
/// and `snapshots.csv` alone: a recognized series sibling or more than one
/// declared snapshot selects the sequence reader, and one snapshot with no
/// series is the scalar profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PypsaAxis {
    SingleSnapshot,
    Series,
}

/// Probe the folder's declared snapshot axis without reading the component
/// tables.
///
/// # Errors
/// The folder listing or `snapshots.csv` could not be acquired or decoded.
pub fn pypsa_axis(source: &powerio_core::Source) -> Result<PypsaAxis> {
    let entries = source
        .entry_names()
        .map_err(|error| acquisition_error(&error))?;
    for entry in &entries {
        let name = entry.as_str();
        if name.contains('/') {
            continue;
        }
        let Some(stem) = name.strip_suffix(".csv") else {
            continue;
        };
        let Some((component, attribute)) = stem.split_once('-') else {
            continue;
        };
        if series_field(component, attribute).is_some() {
            return Ok(PypsaAxis::Series);
        }
    }
    let folder = PypsaFolder { source, entries };
    let Some(snapshot_table) = folder.optional("snapshots.csv")? else {
        return Ok(PypsaAxis::SingleSnapshot);
    };
    if snapshot_table.rows.len() > 1 {
        Ok(PypsaAxis::Series)
    } else {
        Ok(PypsaAxis::SingleSnapshot)
    }
}

/// The snapshot-local series files the sequence reader interprets: input
/// setpoints and bounds, and complete voltage and dispatch output. Everything
/// else stays reported rather than silently reduced.
/// The field plus whether the column is problem input (a setpoint or bound,
/// the `*_set`/`*_pu` spellings) rather than voltage or dispatch output
/// (the bare `p`/`q`/voltage spellings). The distinction picks the
/// sequence's value type: input changes produce a network per point, while a
/// fixed network with only solution quantities varying is an operating point
/// series.
fn series_field(component: &str, attribute: &str) -> Option<(SeriesField, bool)> {
    match (component, attribute) {
        ("loads", "p_set") => Some((SeriesField::LoadP, true)),
        ("loads", "p") => Some((SeriesField::LoadP, false)),
        ("loads", "q_set") => Some((SeriesField::LoadQ, true)),
        ("loads", "q") => Some((SeriesField::LoadQ, false)),
        ("generators", "p_set") => Some((SeriesField::GenPg, true)),
        ("generators", "p") => Some((SeriesField::GenPg, false)),
        ("generators", "q_set") => Some((SeriesField::GenQg, true)),
        ("generators", "q") => Some((SeriesField::GenQg, false)),
        ("generators", "p_max_pu") => Some((SeriesField::GenPmax, true)),
        ("generators", "p_min_pu") => Some((SeriesField::GenPmin, true)),
        ("generators", "v_mag_pu_set") => Some((SeriesField::GenVg, true)),
        ("buses", "v_mag_pu") => Some((SeriesField::BusVm, false)),
        ("buses", "v_ang") => Some((SeriesField::BusVa, false)),
        _ => None,
    }
}

/// One resolved series column: the field it patches, the element row, and one
/// value per snapshot.
struct SeriesColumn {
    field: SeriesField,
    /// Calculation input rather than a solution quantity.
    input: bool,
    row: usize,
    values: Vec<f64>,
}

/// Read a PyPSA CSV folder with time series siblings into a balanced network
/// time series: one network handle per snapshot, static tables shared across
/// the whole series, and the supported snapshot-local columns patched per
/// point: load and generator setpoints, per unit dispatch bounds scaled by
/// `p_nom`, voltage setpoints, and solved bus voltages. A series
/// file outside that profile is reported and retained rather than silently
/// reduced; a series column naming an unknown element, a non-numeric value,
/// or a row axis that disagrees with `snapshots.csv` is refused.
///
/// # Errors
/// A folder without `snapshots.csv`, a malformed series table, or any static
/// profile error.
// The listing scan, snapshot axis, column resolution, and per point patching
// read as one sequence; splitting them would thread six locals through
// helpers.
#[allow(clippy::too_many_lines)]
pub fn parse_pypsa_csv_time_series(source: &powerio_core::Source) -> Result<PypsaCsvSequence> {
    let mut warnings = Diagnostics::new();
    // A directory source yields its walk once; this one listing serves the
    // series scan, the static read, and the series tables.
    let entries = source
        .entry_names()
        .map_err(|error| acquisition_error(&error))?;

    // Series siblings by name shape, before the static read so it does not
    // report the interpreted ones as ignored.
    let mut series_files: Vec<(String, String, String)> = Vec::new();
    let mut consumed = HashSet::new();
    for entry in &entries {
        let name = entry.as_str();
        if name.contains('/') {
            continue;
        }
        let Some(stem) = name.strip_suffix(".csv") else {
            continue;
        };
        let Some((component, attribute)) = stem.split_once('-') else {
            continue;
        };
        if series_field(component, attribute).is_some() {
            consumed.insert(name.to_string());
        }
        series_files.push((
            name.to_string(),
            component.to_string(),
            attribute.to_string(),
        ));
    }

    let base = read_pypsa_csv_static(source, entries.clone(), &mut warnings, &consumed)?;
    let folder = PypsaFolder { source, entries };

    // The snapshot axis. The label column is `snapshot` (the writer's and
    // pandas' spelling), else `name`, else the leading index column.
    let snapshot_table = folder
        .optional("snapshots.csv")?
        .ok_or_else(|| bad("a time series folder needs `snapshots.csv`"))?;
    let label_column = ["snapshot", "name"]
        .into_iter()
        .find(|c| snapshot_table.headers.iter().any(|h| h == c))
        .map(str::to_string)
        .or_else(|| snapshot_table.headers.first().cloned())
        .ok_or_else(|| bad("`snapshots.csv` has no columns"))?;
    let snapshots: Vec<String> = snapshot_table
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            row.get(&label_column)
                .cloned()
                .ok_or_else(|| bad(format!("snapshots.csv row {}: empty snapshot label", i + 1)))
        })
        .collect::<Result<_>>()?;
    if snapshots.is_empty() {
        return Err(bad("`snapshots.csv` states no snapshots"));
    }

    // Element rows resolve by name in table order — the same order the static
    // read built each table in.
    let name_rows = |file: &str| -> Result<HashMap<String, usize>> {
        let Some(table) = folder.optional(file)? else {
            return Ok(HashMap::new());
        };
        Ok(table
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| row.get("name").map(|n| (n.clone(), i)))
            .collect())
    };
    let bus_rows: HashMap<String, usize> = folder
        .required("buses.csv")?
        .rows
        .iter()
        .enumerate()
        .filter_map(|(i, row)| row.get("name").map(|n| (n.clone(), i)))
        .collect();
    let load_rows = name_rows("loads.csv")?;
    let generator_rows = name_rows("generators.csv")?;
    // `p_nom` scales the per unit dispatch bound series, with the static
    // read's own fallback.
    let p_nom: Vec<f64> = folder
        .optional("generators.csv")?
        .map_or_else(Vec::new, |t| {
            t.rows
                .iter()
                .map(|row| {
                    row.f("p_nom")
                        .unwrap_or_else(|| row.f("p_set").unwrap_or(0.0).abs())
                })
                .collect()
        });

    let mut columns: Vec<SeriesColumn> = Vec::new();
    for (file, component, attribute) in &series_files {
        let Some((field, input)) = series_field(component, attribute) else {
            warnings.push(&codes::READ_PYPSA_TABLE_UNSUPPORTED, format!(
                "`{file}` is outside the snapshot-local series profile; retained for exact same format writing"
            ));
            continue;
        };
        let table = folder
            .optional(file)?
            .ok_or_else(|| bad(format!("`{file}` vanished between listing and read")))?;
        if table.rows.len() != snapshots.len() {
            return Err(bad(format!(
                "`{file}` states {} rows for {} snapshots",
                table.rows.len(),
                snapshots.len()
            )));
        }
        let rows_of: &HashMap<String, usize> = match field {
            SeriesField::BusVm | SeriesField::BusVa => &bus_rows,
            SeriesField::LoadP | SeriesField::LoadQ => &load_rows,
            _ => &generator_rows,
        };
        for header in table.headers.iter().skip(1) {
            if header.is_empty() {
                continue;
            }
            let Some(&row) = rows_of.get(header) else {
                return Err(bad(format!(
                    "`{file}` column `{header}` names no element of its table"
                )));
            };
            let values = table
                .rows
                .iter()
                .enumerate()
                .map(|(k, r)| {
                    r.f(header).ok_or_else(|| {
                        bad(format!(
                            "`{file}` column `{header}` row {}: not a number",
                            k + 1
                        ))
                    })
                })
                .collect::<Result<Vec<f64>>>()?;
            columns.push(SeriesColumn {
                field,
                input,
                row,
                values,
            });
        }
    }

    let mut networks = Vec::with_capacity(snapshots.len());
    for point in 0..snapshots.len() {
        let mut network = base.clone();
        for column in &columns {
            let value = column.values[point];
            match column.field {
                SeriesField::LoadP => network.loads_mut()[column.row].p = value,
                SeriesField::LoadQ => network.loads_mut()[column.row].q = value,
                SeriesField::GenPg => network.generators_mut()[column.row].pg = value,
                SeriesField::GenQg => network.generators_mut()[column.row].qg = value,
                SeriesField::GenPmax => {
                    network.generators_mut()[column.row].pmax =
                        value * p_nom.get(column.row).copied().unwrap_or(0.0);
                }
                SeriesField::GenPmin => {
                    network.generators_mut()[column.row].pmin =
                        value * p_nom.get(column.row).copied().unwrap_or(0.0);
                }
                SeriesField::GenVg => network.generators_mut()[column.row].vg = value,
                SeriesField::BusVm => network.buses_mut()[column.row].vm = value,
                SeriesField::BusVa => {
                    network.buses_mut()[column.row].va = value * crate::normalize::RAD_TO_DEG;
                }
            }
        }
        networks.push(network);
    }

    let time_points = snapshots
        .iter()
        .map(|label| powerio_core::TimePoint::new(label.clone(), None))
        .collect::<std::result::Result<Vec<_>, powerio_core::Error>>()
        .map_err(|e| bad(e.to_string()))?;
    let series =
        powerio_core::TimeSeries::new(time_points, networks).map_err(|e| bad(e.to_string()))?;
    let inputs_vary = columns.iter().any(|column| column.input);
    let has_varying_columns = !columns.is_empty();
    Ok(PypsaCsvSequence {
        series,
        inputs_vary,
        has_varying_columns,
        diagnostics: warnings.into_records(),
    })
}

fn parse_csv_table(text: &str, name: &str) -> Result<Option<CsvTable>> {
    let mut records = parse_csv(text, name)?
        .into_iter()
        .filter(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    let Some(headers) = records.next() else {
        return Ok(Some(CsvTable {
            headers: Vec::new(),
            rows: Vec::new(),
        }));
    };
    let column_of: std::sync::Arc<HashMap<String, usize>> = std::sync::Arc::new(
        headers
            .iter()
            .enumerate()
            .map(|(column, header)| (header.clone(), column))
            .collect(),
    );
    let mut rows = Vec::new();
    for fields in records {
        rows.push(CsvRow {
            column_of: std::sync::Arc::clone(&column_of),
            fields,
        });
    }
    Ok(Some(CsvTable { headers, rows }))
}

/// Split a whole CSV file into records, honoring quoted fields: an embedded
/// newline or comma inside `"..."` stays in the field (the writer's `esc` emits
/// those), and `""` is an escaped quote. A quote left open at end of input is
/// malformed CSV — everything after it would silently parse as one literal
/// field — so it is an error, not a best-effort record.
fn parse_csv(text: &str, name: &str) -> Result<Vec<Vec<String>>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                let _ = chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => record.push(std::mem::take(&mut cur)),
            '\r' if !quoted && chars.peek() == Some(&'\n') => {}
            '\n' if !quoted => {
                record.push(std::mem::take(&mut cur));
                records.push(std::mem::take(&mut record));
            }
            _ => cur.push(c),
        }
    }
    if quoted {
        return Err(bad(format!(
            "{name}: unterminated quoted field (unbalanced `\"`)"
        )));
    }
    if !cur.is_empty() || !record.is_empty() {
        record.push(cur);
        records.push(record);
    }
    Ok(records)
}

/// The collision-free PyPSA key for a bus: its name when it has one, else its
/// numeric id. Tests build `key_of` maps with it; the writer derives keys with
/// the collision fallback in `write_pypsa_csv_folder` instead.
#[cfg(test)]
fn bus_key(b: &Bus) -> String {
    b.name.clone().unwrap_or_else(|| b.id.0.to_string())
}

/// The bus column an element table writes, escaped: the same key `buses.csv`
/// is indexed on, falling back to the raw id for a reference to a missing bus.
fn key_for(key_of: &HashMap<BusId, String>, bus: BusId) -> String {
    key_of
        .get(&bus)
        .map_or_else(|| bus.0.to_string(), |k| esc(k))
}

fn esc(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn bus_ref(
    file: &'static str,
    n: usize,
    row: &CsvRow,
    key: &str,
    id_of_name: &HashMap<String, BusId>,
) -> Result<BusId> {
    let raw = row
        .get(key)
        .ok_or_else(|| bad(format!("{file} row {n}: missing bus reference `{key}`")))?;
    id_of_name.get(raw).copied().ok_or_else(|| {
        bad(format!(
            "{file} row {n}: column `{key}` references unknown bus `{raw}`"
        ))
    })
}

#[cfg(test)]
// Exact float compares are the point: a mapped value deviating from the
// fixture arithmetic means a column was misread.
#[allow(clippy::float_cmp)]
mod tests {
    #[derive(Debug)]
    struct Parsed {
        network: BalancedNetwork,
        diagnostics: Vec<crate::diagnostics::Diagnostic>,
    }

    impl Parsed {
        fn render_diagnostics(&self) -> Vec<String> {
            crate::diagnostics::render_diagnostics(&self.diagnostics)
        }
    }

    fn read_pypsa_csv_folder(path: impl AsRef<Path>) -> Result<Parsed> {
        let source =
            powerio_core::Source::open(path.as_ref()).map_err(|error| acquisition_error(&error))?;
        let mut warnings = Diagnostics::new();
        let network = read_pypsa_csv_source(&source, &mut warnings)?;
        Ok(Parsed {
            network,
            diagnostics: warnings.into_records(),
        })
    }

    use super::*;
    use std::fs;

    /// A fresh, nonexistent target for the folder writer: the destination
    /// commit refuses an existing entry, so the target must not exist yet.
    fn tmp_dir(label: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("powerio-pypsa-unit-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    fn folder(label: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = tmp_dir(label);
        fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            fs::write(dir.join(name), text).unwrap();
        }
        dir
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "{a} vs {b}");
    }

    fn bus(id: usize, name: Option<&str>) -> Bus {
        Bus {
            id: BusId(id),
            kind: BusType::Pq,
            vm: 1.0,
            va: 0.0,
            base_kv: 110.0,
            vmax: 1.1,
            vmin: 0.9,
            evhi: None,
            evlo: None,
            area: 1,
            zone: 1,
            name: name.map(str::to_string),
            uid: None,
            location: None,
            extras: Extras::default(),
        }
    }

    fn make_gen(bus: usize, cost: Option<GenCost>) -> Generator {
        Generator {
            bus: BusId(bus),
            energy_source: GeneratorEnergySource::default(),
            pg: 1.0,
            qg: 0.0,
            pmax: 10.0,
            pmin: 0.0,
            qmax: f64::INFINITY,
            qmin: f64::NEG_INFINITY,
            vg: 1.0,
            mbase: 100.0,
            in_service: true,
            cost,
            caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
            voltage_regulation_on: true,
            regulating_terminal: None,
            regulated_bus: None,
            active_power_control: None,
            uid: None,
        }
    }

    fn storage_unit(bus: usize) -> Storage {
        Storage {
            bus: BusId(bus),
            ps: 3.0,
            qs: 1.5,
            energy: 20.0,
            energy_rating: 100.0,
            charge_rating: 25.0,
            discharge_rating: 25.0,
            charge_efficiency: 0.91,
            discharge_efficiency: 0.92,
            thermal_rating: 25.0,
            current_rating: None,
            qmin: f64::NEG_INFINITY,
            qmax: f64::INFINITY,
            r: 0.0,
            x: 0.0,
            p_loss: 0.0,
            q_loss: 0.0,
            in_service: true,
            active_power_control: None,
            uid: None,
            extras: Extras::default(),
        }
    }

    fn xfmr(from: usize, to: usize, rate_a: f64) -> Branch {
        Branch {
            name: None,
            from: BusId(from),
            to: BusId(to),
            r: 0.125,
            x: 0.5,
            b: 0.25,
            charging: None,
            rate_a,
            rate_b: 0.0,
            rate_c: 0.0,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: 1.05,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            solution: None,
            uid: None,
            route: None,
            extras: Extras::default(),
        }
    }

    fn line(from: usize, to: usize) -> Branch {
        Branch {
            name: None,
            from: BusId(from),
            to: BusId(to),
            r: 0.01,
            x: 0.1,
            b: 0.2,
            charging: None,
            rate_a: 100.0,
            rate_b: 0.0,
            rate_c: 0.0,
            rating_sets: Vec::new(),
            current_ratings: None,
            tap: 0.0,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            solution: None,
            uid: None,
            route: None,
            extras: Extras::default(),
        }
    }

    fn net_with(buses: Vec<Bus>) -> BalancedNetwork {
        BalancedNetwork::in_memory("t", 100.0, buses, Vec::new())
    }

    #[test]
    fn scheme_a_keeps_numeric_ids() {
        let dir = folder(
            "scheme-a",
            &[
                ("buses.csv", "name,v_nom\n5,110\n2,110\n"),
                ("loads.csv", "name,bus,p_set\nd1,5,7\n"),
            ],
        );
        let net = read_pypsa_csv_folder(&dir).unwrap().network;
        assert_eq!(net.buses()[0].id, BusId(5));
        assert_eq!(net.buses()[1].id, BusId(2));
        assert!(net.buses()[0].name.is_none());
        assert_eq!(net.loads()[0].bus, BusId(5));
    }

    #[test]
    fn scheme_b_on_mixed_names_never_mixes() {
        let dir = folder(
            "scheme-b",
            &[
                ("buses.csv", "name,v_nom\n2,110\nb,110\n"),
                ("loads.csv", "name,bus,p_set\nd1,2,7\n"),
            ],
        );
        let net = read_pypsa_csv_folder(&dir).unwrap().network;
        assert_eq!(net.buses()[0].id, BusId(1));
        assert_eq!(net.buses()[1].id, BusId(2));
        assert_eq!(net.buses()[0].name.as_deref(), Some("2"));
        assert_eq!(net.buses()[1].name.as_deref(), Some("b"));
        // "2" resolves by name to the first bus, not numerically to the second.
        assert_eq!(net.loads()[0].bus, BusId(1));
    }

    #[test]
    fn duplicate_bus_name_errors() {
        let dir = folder("dup-name", &[("buses.csv", "name,v_nom\nn1,110\nn1,110\n")]);
        let err = read_pypsa_csv_folder(&dir).unwrap_err().to_string();
        assert!(err.contains("duplicate bus name `n1`"), "{err}");
    }

    #[test]
    fn missing_bus_name_errors() {
        let dir = folder("no-name", &[("buses.csv", "name,v_nom\n,110\n")]);
        let err = read_pypsa_csv_folder(&dir).unwrap_err().to_string();
        assert!(err.contains("buses.csv row 1: missing bus name"), "{err}");
    }

    #[test]
    fn unknown_bus_reference_errors_no_numeric_fallback() {
        let dir = folder(
            "unknown-ref",
            &[
                ("buses.csv", "name,v_nom\n1,110\n"),
                ("loads.csv", "name,bus,p_set\nd1,7,5\n"),
            ],
        );
        let err = read_pypsa_csv_folder(&dir).unwrap_err().to_string();
        assert!(
            err.contains("loads.csv row 1: column `bus` references unknown bus `7`"),
            "{err}"
        );
    }

    #[test]
    fn missing_bus_reference_errors() {
        let dir = folder(
            "missing-ref",
            &[
                ("buses.csv", "name,v_nom\n1,110\n"),
                ("loads.csv", "name,p_set\nd1,5\n"),
            ],
        );
        let err = read_pypsa_csv_folder(&dir).unwrap_err().to_string();
        assert!(
            err.contains("loads.csv row 1: missing bus reference `bus`"),
            "{err}"
        );
    }

    #[test]
    fn control_sets_bus_kind_pq_untouched() {
        let dir = folder(
            "control",
            &[
                ("buses.csv", "name,v_nom\n1,110\n2,110\n3,110\n"),
                (
                    "generators.csv",
                    "name,bus,control,p_set\ng1,1,slack,1\ng2,2,pv,1\ng3,3,PQ,1\n",
                ),
            ],
        );
        let net = read_pypsa_csv_folder(&dir).unwrap().network;
        assert_eq!(net.buses()[0].kind, BusType::Ref);
        assert_eq!(net.buses()[1].kind, BusType::Pv);
        assert_eq!(net.buses()[2].kind, BusType::Pq);
    }

    #[test]
    fn transformer_read_rebases_to_system_base() {
        let dir = folder(
            "xf-read",
            &[
                ("network.csv", "name,powerio_base_mva\nt,100\n"),
                ("buses.csv", "name,v_nom\n1,110\n2,110\n"),
                (
                    "transformers.csv",
                    "name,bus0,bus1,r,x,b,g,s_nom,tap_ratio,phase_shift,active\nt1,1,2,0.0625,0.25,0.5,0.1,50,1.05,0,True\n",
                ),
            ],
        );
        let parsed = read_pypsa_csv_folder(&dir).unwrap();
        let br = &parsed.network.branches()[0];
        close(br.r, 0.125); // 0.0625 * 100/50
        close(br.x, 0.5);
        close(br.b, 0.25); // 0.5 * 50/100
        close(br.calc_terminal_charging().g_fr, 0.05);
        close(br.calc_terminal_charging().b_fr, 0.25);
        close(br.calc_terminal_charging().g_to, 0.0);
        assert_eq!(br.rate_a, 50.0);
        assert_eq!(br.tap, 1.05);
        assert!(
            parsed.render_diagnostics().is_empty(),
            "{:?}",
            parsed.render_diagnostics()
        );
    }

    #[test]
    fn transformer_read_rejects_nonpositive_s_nom() {
        let dir = folder(
            "xf-snom",
            &[
                ("buses.csv", "name,v_nom\n1,110\n2,110\n"),
                (
                    "transformers.csv",
                    "name,bus0,bus1,r,x,s_nom,tap_ratio\nt1,1,2,0.1,0.2,0,1.05\n",
                ),
            ],
        );
        let err = read_pypsa_csv_folder(&dir).unwrap_err().to_string();
        assert!(
            err.contains(
                "transformers.csv row 1 (`t1`): s_nom must be positive to rebase impedances (got 0)"
            ),
            "{err}"
        );
    }

    #[test]
    fn line_g_maps_to_terminal_conductance() {
        let dir = folder(
            "line-g",
            &[
                ("buses.csv", "name,v_nom\n1,110\n2,110\n"),
                (
                    "lines.csv",
                    "name,bus0,bus1,r,x,g,s_nom\nl1,1,2,0.1,0.2,0.3,100\n",
                ),
            ],
        );
        let parsed = read_pypsa_csv_folder(&dir).unwrap();
        let charging = parsed.network.branches()[0].calc_terminal_charging();
        close(charging.g_fr, 1815.0);
        close(charging.g_to, 1815.0);
        assert!(
            parsed.render_diagnostics().is_empty(),
            "{:?}",
            parsed.render_diagnostics()
        );
    }

    #[test]
    fn transformer_write_rebases_to_s_nom_base() {
        let mut net = net_with(vec![bus(1, None), bus(2, None)]);
        *net.branches_mut() = vec![xfmr(1, 2, 50.0)];
        let key_of: HashMap<BusId, String> =
            net.buses().iter().map(|b| (b.id, bus_key(b))).collect();
        let csv = transformers_csv(&net, &key_of);
        assert_eq!(
            csv.lines().nth(1).unwrap(),
            "transformer_1,1,2,0.0625,0.25,0.5,0,50,1.05,0,true"
        );
    }

    #[test]
    fn transformer_write_zero_rate_a_uses_base_mva() {
        let mut net = net_with(vec![bus(1, None), bus(2, None)]);
        *net.branches_mut() = vec![xfmr(1, 2, 0.0)];
        let key_of: HashMap<BusId, String> =
            net.buses().iter().map(|b| (b.id, bus_key(b))).collect();
        let csv = transformers_csv(&net, &key_of);
        assert_eq!(
            csv.lines().nth(1).unwrap(),
            "transformer_1,1,2,0.125,0.5,0.25,0,100,1.05,0,true"
        );
    }

    #[test]
    fn a_folder_write_never_replaces_an_existing_entry() {
        let net = net_with(vec![bus(1, None), bus(2, None)]);

        // A regular file at a produced table name: the write is refused and
        // the file keeps its bytes.
        let blocked = tmp_dir("no-clobber-file");
        fs::create_dir_all(&blocked).unwrap();
        fs::write(blocked.join("buses.csv"), b"precious").unwrap();
        let error = write_pypsa_csv_folder(&net, &blocked).unwrap_err();
        assert_eq!(error.category(), powerio_core::ErrorCategory::Request);
        assert_eq!(fs::read(blocked.join("buses.csv")).unwrap(), b"precious");

        // A symbolic link at a produced table name: the link survives and the
        // file it designates keeps its bytes and its length.
        #[cfg(unix)]
        {
            let linked = tmp_dir("no-clobber-link");
            fs::create_dir_all(&linked).unwrap();
            let designated = tmp_dir("no-clobber-designated");
            fs::create_dir_all(&designated).unwrap();
            let real = designated.join("real.csv");
            fs::write(&real, b"designated bytes").unwrap();
            std::os::unix::fs::symlink(&real, linked.join("buses.csv")).unwrap();
            let error = write_pypsa_csv_folder(&net, &linked).unwrap_err();
            assert_eq!(error.category(), powerio_core::ErrorCategory::Request);
            assert!(
                fs::symlink_metadata(linked.join("buses.csv"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(fs::read(&real).unwrap(), b"designated bytes");
            assert_eq!(fs::metadata(&real).unwrap().len(), 16);
            let _ = fs::remove_dir_all(&linked);
            let _ = fs::remove_dir_all(&designated);
        }

        // The same write into a fresh directory produces the complete table
        // inventory the `Destination` write commits.
        let fresh = tmp_dir("no-clobber-fresh");
        let out = write_pypsa_csv_folder(&net, &fresh).unwrap();
        let mut folder_names: Vec<String> = out
            .files
            .iter()
            .map(|path| {
                path.strip_prefix(&out.dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        folder_names.sort();
        let module = powerio_core::PioModule::new(net.clone());
        let committed = crate::format::__emit_pypsa_csv(
            &module,
            powerio_core::Destination::memory("case").unwrap(),
        )
        .unwrap();
        let powerio_core::EmittedOutput::Memory { artifacts } = committed.into_output() else {
            panic!("memory output")
        };
        let mut memory_names: Vec<String> = artifacts
            .iter()
            .map(|artifact| {
                artifact
                    .name()
                    .as_str()
                    .trim_start_matches("case/")
                    .to_owned()
            })
            .collect();
        memory_names.sort();
        assert_eq!(folder_names, memory_names);
        let _ = fs::remove_dir_all(&blocked);
        let _ = fs::remove_dir_all(&fresh);
    }

    #[test]
    fn transformer_legacy_b_warns_about_terminal_charging_collapse() {
        let mut net = net_with(vec![bus(1, None), bus(2, None)]);
        *net.branches_mut() = vec![xfmr(1, 2, 50.0)];
        let out = write_pypsa_csv_folder(&net, tmp_dir("xf-legacy-b-warning")).unwrap();

        assert!(
            out.render_diagnostics()
                .iter()
                .any(|w| w.contains("terminal admittance")),
            "{:?}",
            out.render_diagnostics()
        );
    }

    #[test]
    fn line_conductance_writes_and_round_trips() {
        let mut net = net_with(vec![bus(1, None), bus(2, None)]);
        let mut br = line(1, 2);
        br.charging = Some(BranchCharging {
            g_fr: 0.4,
            b_fr: 0.1,
            g_to: 0.4,
            b_to: 0.1,
        });
        *net.branches_mut() = vec![br];
        let dir = tmp_dir("line-g-write");
        let out = write_pypsa_csv_folder(&net, &dir).unwrap();
        assert!(
            !out.render_diagnostics()
                .iter()
                .any(|w| w.contains("terminal admittance")),
            "{:?}",
            out.render_diagnostics()
        );
        let text = fs::read_to_string(dir.join("lines.csv")).unwrap();
        assert_eq!(
            text.lines().next().unwrap(),
            "name,bus0,bus1,r,x,b,g,s_nom,v_ang_min,v_ang_max,active"
        );

        let back = read_pypsa_csv_folder(&dir).unwrap().network;
        let charging = back.branches()[0].calc_terminal_charging();
        close(charging.g_fr, 0.4);
        close(charging.g_to, 0.4);
        close(charging.b_fr, 0.1);
        close(charging.b_to, 0.1);
    }

    #[test]
    fn transformer_conductance_writes_and_round_trips() {
        let mut net = net_with(vec![bus(1, None), bus(2, None)]);
        let mut br = xfmr(1, 2, 50.0);
        br.charging = Some(BranchCharging {
            g_fr: 0.05,
            b_fr: 0.25,
            g_to: 0.0,
            b_to: 0.0,
        });
        *net.branches_mut() = vec![br];
        let dir = tmp_dir("xf-g-write");
        let out = write_pypsa_csv_folder(&net, &dir).unwrap();
        assert!(
            !out.render_diagnostics()
                .iter()
                .any(|w| w.contains("terminal admittance")),
            "{:?}",
            out.render_diagnostics()
        );

        let back = read_pypsa_csv_folder(&dir).unwrap().network;
        let charging = back.branches()[0].calc_terminal_charging();
        close(charging.g_fr, 0.05);
        close(charging.g_to, 0.0);
        close(charging.b_fr, 0.25);
        close(charging.b_to, 0.0);
    }

    #[test]
    fn storage_write_fields_and_round_trip() {
        let mut net = net_with(vec![bus(1, None)]);
        *net.storage_mut() = vec![storage_unit(1)];
        let dir = tmp_dir("storage-rt");
        let out = write_pypsa_csv_folder(&net, &dir).unwrap();
        assert!(
            !out.render_diagnostics()
                .iter()
                .any(|w| w.contains("storage units")),
            "{:?}",
            out.render_diagnostics()
        );
        let text = fs::read_to_string(dir.join("storage_units.csv")).unwrap();
        assert_eq!(
            text.lines().next().unwrap(),
            "name,bus,p_nom,max_hours,p_set,q_set,state_of_charge_initial,efficiency_store,efficiency_dispatch,cyclic_state_of_charge"
        );
        assert_eq!(
            text.lines().nth(1).unwrap(),
            "storage_1,1,25,4,3,1.5,20,0.91,0.92,false"
        );
        let back = read_pypsa_csv_folder(&dir).unwrap().network;
        let st = &back.storage()[0];
        assert_eq!(st.charge_rating, 25.0);
        assert_eq!(st.discharge_rating, 25.0);
        assert_eq!(st.energy_rating, 100.0);
        assert_eq!(st.ps, 3.0);
        assert_eq!(st.qs, 1.5);
        assert_eq!(st.energy, 20.0);
    }

    #[test]
    fn storage_write_lossy_warning_counts() {
        let mut net = net_with(vec![bus(1, None)]);
        let mut st = storage_unit(1);
        st.charge_rating = 10.0;
        st.discharge_rating = 20.0;
        st.thermal_rating = 20.0;
        *net.storage_mut() = vec![st];
        let out = write_pypsa_csv_folder(&net, tmp_dir("storage-lossy")).unwrap();
        assert!(
            out.diagnostics.iter().any(|d| d.message()
                == "1 storage units lose fields PyPSA storage_units cannot carry (asymmetric charge/discharge ratings collapse to p_nom = max; thermal_rating, qmin/qmax, r/x, p_loss/q_loss dropped)"),
            "{:?}",
            out.render_diagnostics()
        );
    }

    #[test]
    fn named_buses_join_on_write() {
        let mut net = net_with(vec![bus(1, Some("North")), bus(2, None)]);
        *net.generators_mut() = vec![make_gen(1, None)];
        *net.loads_mut() = vec![Load {
            bus: BusId(2),
            p: 5.0,
            q: 1.0,
            voltage_model: None,
            in_service: true,
            uid: None,
            extras: Extras::default(),
        }];
        let dir = tmp_dir("named-join");
        write_pypsa_csv_folder(&net, &dir).unwrap();
        let buses = fs::read_to_string(dir.join("buses.csv")).unwrap();
        assert!(buses.lines().nth(1).unwrap().starts_with("North,"));
        let gens = fs::read_to_string(dir.join("generators.csv")).unwrap();
        assert!(gens.lines().nth(1).unwrap().contains(",North,"), "{gens}");
        let back = read_pypsa_csv_folder(&dir).unwrap().network;
        assert_eq!(back.buses()[0].name.as_deref(), Some("North"));
        assert_eq!(back.loads()[0].bus, back.buses()[1].id);
    }

    #[test]
    fn duplicate_bus_names_fall_back_to_ids() {
        let mut net = net_with(vec![bus(1, Some("X")), bus(2, Some("X"))]);
        *net.loads_mut() = vec![Load {
            bus: BusId(2),
            p: 5.0,
            q: 1.0,
            voltage_model: None,
            in_service: true,
            uid: None,
            extras: Extras::default(),
        }];
        let dir = tmp_dir("dup-keys");
        let out = write_pypsa_csv_folder(&net, &dir).unwrap();
        assert!(
            out.diagnostics.iter().any(|d| d.message()
                == "buses.csv: bus names `X` collide with another bus name or id; those buses are keyed by their numeric id instead"),
            "{:?}",
            out.render_diagnostics()
        );
        let buses = fs::read_to_string(dir.join("buses.csv")).unwrap();
        let keys: Vec<&str> = buses
            .lines()
            .skip(1)
            .map(|l| l.split(',').next().unwrap())
            .collect();
        assert_eq!(keys, ["1", "2"]);
        // The folder is importable: elements join on the fallback keys.
        let back = read_pypsa_csv_folder(&dir).unwrap().network;
        assert_eq!(back.loads()[0].bus, back.buses()[1].id);
    }

    #[test]
    fn unterminated_quote_is_an_error() {
        let dir = folder(
            "bad-quote",
            &[("buses.csv", "name,v_nom\n\"bus one,110\n2,110\n")],
        );
        let msg = read_pypsa_csv_folder(&dir).unwrap_err().to_string();
        assert!(
            msg.contains("buses.csv: unterminated quoted field (unbalanced `\"`)"),
            "{msg}"
        );
    }

    #[test]
    fn quadratic_only_marginal_cost_is_kept() {
        // PyPSA defaults marginal_cost to 0; a quadratic-only file still
        // carries a real cost curve.
        let dir = folder(
            "quad-cost",
            &[
                ("buses.csv", "name,v_nom\n1,110\n"),
                (
                    "generators.csv",
                    "name,bus,p_nom,marginal_cost_quadratic\ng1,1,50,0.25\n",
                ),
            ],
        );
        let parsed = read_pypsa_csv_folder(&dir).unwrap();
        let cost = parsed.network.generators()[0].cost.as_ref().unwrap();
        assert_eq!(cost.coeffs, vec![0.25, 0.0, 0.0]);
    }

    #[test]
    fn bus_name_matching_another_bus_id_falls_back() {
        // A bus literally named "2" would collide with bus id 2's key.
        let net = net_with(vec![bus(1, Some("2")), bus(2, None)]);
        let dir = tmp_dir("name-id-clash");
        let out = write_pypsa_csv_folder(&net, &dir).unwrap();
        assert!(
            out.render_diagnostics().iter().any(|w| w.contains("`2`")),
            "{:?}",
            out.render_diagnostics()
        );
        let buses = fs::read_to_string(dir.join("buses.csv")).unwrap();
        let keys: Vec<&str> = buses
            .lines()
            .skip(1)
            .map(|l| l.split(',').next().unwrap())
            .collect();
        assert_eq!(keys, ["1", "2"]);
    }

    #[test]
    fn links_read_as_hvdc_with_warning() {
        let dir = folder(
            "links",
            &[
                ("buses.csv", "name,v_nom\n1,110\n2,110\n"),
                (
                    "links.csv",
                    "name,bus0,bus1,p_set,p_nom,p_min_pu,p_max_pu,efficiency,active\nl1,1,2,10,50,-1,1,0.97,True\n",
                ),
            ],
        );
        let parsed = read_pypsa_csv_folder(&dir).unwrap();
        let h = &parsed.network.hvdc()[0];
        assert_eq!(h.from, BusId(1));
        assert_eq!(h.to, BusId(2));
        assert_eq!(h.pf, 10.0);
        close(h.pt, 9.7);
        close(h.pmin, -50.0);
        close(h.pmax, 50.0);
        assert_eq!(h.loss0, 0.0);
        close(h.loss1, 0.03);
        assert_eq!(h.vf, 1.0);
        assert_eq!(h.qf, 0.0);
        assert!(h.in_service);
        assert!(
            parsed.diagnostics.iter().any(|d| d.message()
                == "links.csv: 1 links read as HVDC lines; PyPSA links carry no reactive or voltage data (q limits 0, voltage setpoints 1.0)"),
            "{:?}",
            parsed.render_diagnostics()
        );
    }

    #[test]
    fn stores_warning_gated_on_nonempty() {
        let dir = folder(
            "stores-empty",
            &[
                ("buses.csv", "name,v_nom\n1,110\n"),
                ("stores.csv", "name,bus,e_nom\n"),
            ],
        );
        assert!(
            read_pypsa_csv_folder(&dir)
                .unwrap()
                .render_diagnostics()
                .is_empty()
        );
        let dir = folder(
            "stores-nonempty",
            &[
                ("buses.csv", "name,v_nom\n1,110\n"),
                ("stores.csv", "name,bus,e_nom\ns1,1,10\n"),
            ],
        );
        let parsed = read_pypsa_csv_folder(&dir).unwrap();
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|d| d.message() == "stores.csv ignored (1 rows): PyPSA stores are not mapped"),
            "{:?}",
            parsed.render_diagnostics()
        );
    }

    #[test]
    fn header_only_buses_is_an_empty_case() {
        let dir = folder("empty", &[("buses.csv", "name,v_nom\n")]);
        let err = read_pypsa_csv_folder(&dir).unwrap_err().to_string();
        assert!(err.contains("case has no buses"), "{err}");
    }

    #[test]
    fn cost_write_keeps_low_order_terms_and_warns() {
        let mut net = net_with(vec![bus(1, None), bus(2, None)]);
        *net.generators_mut() = vec![
            make_gen(
                1,
                Some(GenCost {
                    model: 2,
                    startup: 0.0,
                    shutdown: 0.0,
                    ncost: 4,
                    coeffs: vec![5.0, 4.0, 3.0, 2.0], // cubic: keep (c2, c1) = (4, 3)
                }),
            ),
            make_gen(
                2,
                Some(GenCost {
                    model: 1,
                    startup: 0.0,
                    shutdown: 0.0,
                    ncost: 2,
                    coeffs: vec![1.0, 2.0, 3.0, 4.0],
                }),
            ),
            make_gen(
                1,
                Some(GenCost {
                    model: 2,
                    startup: 0.0,
                    shutdown: 0.0,
                    ncost: 0,
                    coeffs: Vec::new(),
                }),
            ),
        ];
        let key_of: HashMap<BusId, String> =
            net.buses().iter().map(|b| (b.id, bus_key(b))).collect();
        let mut warnings = Diagnostics::new();
        let csv = generators_csv(&net, &key_of, &mut warnings);
        assert_eq!(
            csv.lines().nth(1).unwrap(),
            "gen_1,1,PQ,10,1,0,0,1,3,4,true,1"
        );
        assert_eq!(
            csv.lines().nth(2).unwrap(),
            "gen_2,2,PQ,10,1,0,0,1,0,0,true,1"
        );
        assert_eq!(
            csv.lines().nth(3).unwrap(),
            "gen_3,1,PQ,10,1,0,0,1,0,0,true,1"
        );
        for expected in [
            "1 generator costs dropped: PyPSA carries marginal_cost/marginal_cost_quadratic (model 2) only",
            "1 generator costs truncated to quadratic for PyPSA marginal cost columns",
            "1 generator costs had no coefficients and were written as zero",
        ] {
            assert!(
                warnings.records().iter().any(|d| d.message() == expected),
                "missing {expected:?} in {warnings:?}"
            );
        }
    }
}
