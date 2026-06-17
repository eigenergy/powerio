//! Read and write PyPSA CSV folders.
//!
//! PyPSA's CSV folder is a directory format, so it does not fit the
//! `Conversion { text }` API used by single-file formats. The reader and writer
//! are exposed as path-based helpers and through `parse_file(..., "pypsa-csv")`.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::{Parsed, bus_kv, set_bus_kind, zbase};
use crate::network::{
    Branch, Bus, BusId, BusType, Extras, GenCost, Generator, Hvdc, Load, Network, Shunt,
    SourceFormat, Storage,
};
use crate::{Error, Result};

const FMT: &str = "PyPSA CSV";

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PypsaCsvOutputs {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Read a PyPSA CSV folder at `path`. Returns [`Parsed`]: the network plus the
/// reader's fidelity warnings.
pub fn read_pypsa_csv_folder(path: impl AsRef<Path>) -> Result<Parsed> {
    let mut warnings = Vec::new();
    let network = read_pypsa_csv_folder_inner(path.as_ref(), &mut warnings)?;
    Ok(Parsed { network, warnings })
}

#[allow(clippy::too_many_lines)] // direct static-component CSV mapper; each block is one PyPSA table
fn read_pypsa_csv_folder_inner(path: &Path, warnings: &mut Vec<String>) -> Result<Network> {
    let network = read_csv_optional(&path.join("network.csv"))?;
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

    let bus_table = read_csv_required(&path.join("buses.csv"), "buses.csv")?;
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
            area: 1,
            zone: 1,
            name: bus_name,
            extras: Extras::default(),
        });
    }
    let bus_pos: HashMap<BusId, usize> = buses.iter().enumerate().map(|(i, b)| (b.id, i)).collect();

    let mut loads = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("loads.csv"))? {
        for (i, row) in table.rows.iter().enumerate() {
            loads.push(Load {
                bus: bus_ref("loads.csv", i + 1, row, "bus", &id_of_name)?,
                p: row.f("p_set").unwrap_or(0.0),
                q: row.f("q_set").unwrap_or(0.0),
                in_service: row.bool("active").unwrap_or(true),
                extras: Extras::default(),
            });
        }
    }

    let mut shunts = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("shunt_impedances.csv"))? {
        for (i, row) in table.rows.iter().enumerate() {
            let bus = bus_ref("shunt_impedances.csv", i + 1, row, "bus", &id_of_name)?;
            let zb = zbase(bus_kv(&buses, &bus_pos, bus), base_mva);
            shunts.push(Shunt {
                bus,
                g: row.f("g").unwrap_or(0.0) * zb * base_mva,
                b: row.f("b").unwrap_or(0.0) * zb * base_mva,
                in_service: row.bool("active").unwrap_or(true),
                control: None,
                extras: Extras::default(),
            });
        }
    }

    let mut generators = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("generators.csv"))? {
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
            });
        }
    }

    let mut branches = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("lines.csv"))? {
        let mut g_rows = 0usize;
        for (i, row) in table.rows.iter().enumerate() {
            let from = bus_ref("lines.csv", i + 1, row, "bus0", &id_of_name)?;
            let to = bus_ref("lines.csv", i + 1, row, "bus1", &id_of_name)?;
            if row.f("g").unwrap_or(0.0) != 0.0 {
                g_rows += 1;
            }
            // PyPSA per-unitizes line ohms on the BUS0 v_nom
            // (Network.calculate_dependent_values), not bus1.
            let zb = zbase(bus_kv(&buses, &bus_pos, from), base_mva);
            branches.push(Branch {
                from,
                to,
                r: row.f("r").unwrap_or(0.0) / zb,
                x: row.f("x").unwrap_or(0.0) / zb,
                b: row.f("b").unwrap_or(0.0) * zb,
                rate_a: row.f("s_nom").unwrap_or(0.0),
                rate_b: 0.0,
                rate_c: 0.0,
                tap: 0.0,
                shift: 0.0,
                in_service: row.bool("active").unwrap_or(true),
                angmin: row.f("v_ang_min").unwrap_or(-360.0),
                angmax: row.f("v_ang_max").unwrap_or(360.0),
                control: None,
                extras: Extras::default(),
            });
        }
        if g_rows > 0 {
            warnings.push(format!(
                "lines.csv: g nonzero on {g_rows} rows; line shunt conductance is not representable and was ignored"
            ));
        }
    }
    if let Some(table) = read_csv_optional(&path.join("transformers.csv"))? {
        let mut g_rows = 0usize;
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
            if row.f("g").unwrap_or(0.0) != 0.0 {
                g_rows += 1;
            }
            let k = base_mva / s_nom;
            branches.push(Branch {
                from,
                to,
                r: row.f("r").unwrap_or(0.0) * k,
                x: row.f("x").unwrap_or(0.0) * k,
                b: row.f("b").unwrap_or(0.0) * s_nom / base_mva,
                rate_a: s_nom,
                rate_b: 0.0,
                rate_c: 0.0,
                tap: row.f("tap_ratio").unwrap_or(1.0),
                shift: row.f("phase_shift").unwrap_or(0.0),
                in_service: row.bool("active").unwrap_or(true),
                angmin: -360.0,
                angmax: 360.0,
                control: None,
                extras: Extras::default(),
            });
        }
        if g_rows > 0 {
            warnings.push(format!(
                "transformers.csv: g nonzero on {g_rows} rows; transformer shunt conductance is not representable and was ignored"
            ));
        }
    }

    let mut storage = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("storage_units.csv"))? {
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
                qmin: f64::NEG_INFINITY,
                qmax: f64::INFINITY,
                r: 0.0,
                x: 0.0,
                p_loss: 0.0,
                q_loss: 0.0,
                in_service: row.bool("active").unwrap_or(true),
                extras: Extras::default(),
            });
        }
    }

    let mut hvdc = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("links.csv"))? {
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
                pt: pf * efficiency,
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
                extras: Extras::default(),
            });
        }
        if !table.rows.is_empty() {
            warnings.push(format!(
                "links.csv: {} links read as HVDC lines; PyPSA links carry no reactive or voltage data (q limits 0, voltage setpoints 1.0)",
                table.rows.len()
            ));
        }
    }
    if let Some(table) = read_csv_optional(&path.join("stores.csv"))? {
        if !table.rows.is_empty() {
            warnings.push(format!(
                "stores.csv ignored ({} rows): PyPSA stores are not mapped",
                table.rows.len()
            ));
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
    let mut unread: Vec<String> = std::fs::read_dir(path)?
        .filter_map(std::result::Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            Path::new(n)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
                && !consumed.contains(&n.as_str())
        })
        .collect();
    unread.sort();
    for file in unread {
        warnings.push(format!(
            "`{file}` ignored: only the static element tables are read (time series and other tables are not modeled)"
        ));
    }

    let net = Network {
        name,
        base_mva,
        base_frequency: crate::network::DEFAULT_BASE_FREQUENCY,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage,
        hvdc,
        transformers_3w: Vec::new(),
        areas: Vec::new(),
        source_format: SourceFormat::PypsaCsv,
        source: None,
    };
    // This reader bypasses the read_source funnel (directory input), so it
    // guards against a hollow case itself.
    crate::format::reject_empty_case(&net, FMT)?;
    net.check_references(FMT)?;
    Ok(net)
}

#[allow(clippy::too_many_lines)] // one fidelity warning block per dropped field family, then the table writes
pub fn write_pypsa_csv_folder(net: &Network, out_dir: impl AsRef<Path>) -> Result<PypsaCsvOutputs> {
    let out_dir = out_dir.as_ref();
    std::fs::create_dir_all(out_dir)?;
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    // Element tables must reference buses by the same key buses.csv is indexed
    // on, and PyPSA requires those keys to be unique for its joins. A bus is
    // keyed by its name only when the name collides with no other bus's name
    // or id string; colliding buses fall back to their numeric id, which is
    // unique by construction and (per the same rule) cannot displace a kept
    // name.
    let mut name_counts: HashMap<&str, usize> = HashMap::new();
    for b in &net.buses {
        if let Some(n) = &b.name {
            *name_counts.entry(n.as_str()).or_insert(0) += 1;
        }
    }
    let id_owner: HashMap<String, BusId> = net
        .buses
        .iter()
        .map(|b| (b.id.0.to_string(), b.id))
        .collect();
    let mut displaced: Vec<String> = Vec::new();
    let key_of: HashMap<BusId, String> = net
        .buses
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
        warnings.push(format!(
            "buses.csv: bus names {} collide with another bus name or id; those buses are keyed by their numeric id instead",
            displaced.join(", ")
        ));
    }
    if !net.hvdc.is_empty() {
        warnings.push(format!(
            "{} dcline(s) dropped: the PyPSA CSV writer does not model HVDC links",
            net.hvdc.len()
        ));
    }
    if net.generators.iter().any(Generator::has_caps) {
        warnings.push("generator capability/ramp columns dropped: PyPSA generator CSV has no MATPOWER capability columns".into());
    }
    let isolated = net
        .buses
        .iter()
        .filter(|b| b.kind == BusType::Isolated)
        .count();
    if isolated > 0 {
        warnings.push(format!(
            "{isolated} isolated bus(es) written without status: PyPSA buses carry no active flag, they read back in service"
        ));
    }
    let xf_angles = net
        .branches
        .iter()
        .filter(|b| b.is_transformer() && b.has_angle_limits())
        .count();
    if xf_angles > 0 {
        warnings.push(format!(
            "{xf_angles} transformer angle limit(s) dropped: transformers.csv carries no v_ang_min/v_ang_max"
        ));
    }
    let rate_bc = net
        .branches
        .iter()
        .filter(|b| {
            super::nonzero_differs(b.rate_b, b.rate_a) || super::nonzero_differs(b.rate_c, b.rate_a)
        })
        .count();
    if rate_bc > 0 {
        warnings.push(format!(
            "{rate_bc} branch rate_b/rate_c value set(s) dropped: PyPSA carries one s_nom rating"
        ));
    }
    warnings.extend(super::missing_reference_warning(net));
    warnings.extend(super::normalized_tap_warning(net));
    // Exact compares are the point: any deviation from the symmetric, no-loss
    // shape the round trip preserves means a field is dropped on write.
    #[allow(clippy::float_cmp)]
    let lossy = net
        .storage
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
        warnings.push(format!(
            "{lossy} storage units lose fields PyPSA storage_units cannot carry (asymmetric charge/discharge ratings collapse to p_nom = max; thermal_rating, qmin/qmax, r/x, p_loss/q_loss dropped)"
        ));
    }

    write_file(out_dir, "network.csv", &network_csv(net), &mut files)?;
    write_file(out_dir, "snapshots.csv", ",snapshot\n0,now\n", &mut files)?;
    write_file(out_dir, "buses.csv", &buses_csv(net, &key_of), &mut files)?;
    write_file(
        out_dir,
        "generators.csv",
        &generators_csv(net, &key_of, &mut warnings),
        &mut files,
    )?;
    // The v_nom per bus, shared by the writers that rebase impedances.
    let kv_of: HashMap<BusId, f64> = net.buses.iter().map(|b| (b.id, b.base_kv)).collect();
    write_file(out_dir, "loads.csv", &loads_csv(net, &key_of), &mut files)?;
    write_file(
        out_dir,
        "lines.csv",
        &lines_csv(net, &key_of, &kv_of),
        &mut files,
    )?;
    let transformers = transformers_csv(net, &key_of);
    if transformers.lines().count() > 1 {
        write_file(out_dir, "transformers.csv", &transformers, &mut files)?;
    }
    if !net.shunts.is_empty() {
        write_file(
            out_dir,
            "shunt_impedances.csv",
            &shunts_csv(net, &key_of, &kv_of),
            &mut files,
        )?;
    }
    if !net.storage.is_empty() {
        write_file(
            out_dir,
            "storage_units.csv",
            &storage_csv(net, &key_of),
            &mut files,
        )?;
    }
    Ok(PypsaCsvOutputs {
        dir: out_dir.to_path_buf(),
        files,
        warnings,
    })
}

fn network_csv(net: &Network) -> String {
    format!(
        "name,srid,powerio_base_mva\n{},4326,{}\n",
        esc(&net.name),
        net.base_mva
    )
}

fn buses_csv(net: &Network, key_of: &HashMap<BusId, String>) -> String {
    let mut s = String::from("name,v_nom,v_mag_pu_set,v_mag_pu_min,v_mag_pu_max\n");
    for b in &net.buses {
        let _ = writeln!(
            s,
            "{},{},{},{},{}",
            key_for(key_of, b.id),
            b.base_kv,
            b.vm,
            b.vmin,
            b.vmax
        );
    }
    s
}

#[allow(clippy::too_many_lines)]
// one column expression per PyPSA generator attribute
// The exact mbase compare is the point: any deviation from the system base is
// information the PyPSA table cannot carry.
#[allow(clippy::float_cmp)]
fn generators_csv(
    net: &Network,
    key_of: &HashMap<BusId, String>,
    warnings: &mut Vec<String>,
) -> String {
    let mut s = String::from(
        "name,bus,control,p_nom,p_set,q_set,p_min_pu,p_max_pu,marginal_cost,marginal_cost_quadratic,active,v_mag_pu_set\n",
    );
    let bus_kind: HashMap<BusId, BusType> = net.buses.iter().map(|b| (b.id, b.kind)).collect();
    let mut dropped = 0usize;
    let mut truncated = 0usize;
    let mut empty = 0usize;
    let mut unbounded = 0usize;
    for (i, g) in net.generators.iter().enumerate() {
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
        warnings.push(format!(
            "{dropped} generator costs dropped: PyPSA carries marginal_cost/marginal_cost_quadratic (model 2) only"
        ));
    }
    if truncated > 0 {
        warnings.push(format!(
            "{truncated} generator costs truncated to quadratic for PyPSA marginal cost columns"
        ));
    }
    if empty > 0 {
        warnings.push(format!(
            "{empty} generator costs had no coefficients and were written as zero"
        ));
    }
    if unbounded > 0 {
        warnings.push(format!(
            "{unbounded} non-finite generator p limit(s) written as the PyPSA defaults (p_min_pu 0, p_max_pu 1)"
        ));
    }
    let q_limited = net
        .generators
        .iter()
        .filter(|g| g.qmin.is_finite() || g.qmax.is_finite())
        .count();
    if q_limited > 0 {
        warnings.push(format!(
            "{q_limited} generator reactive limit(s) dropped: PyPSA generators carry no q bounds"
        ));
    }
    let off_base = net
        .generators
        .iter()
        .filter(|g| g.mbase != 0.0 && g.mbase != net.base_mva)
        .count();
    if off_base > 0 {
        warnings.push(format!(
            "{off_base} generator machine base(s) (mbase) dropped: PyPSA carries no per generator MVA base"
        ));
    }
    s
}

fn loads_csv(net: &Network, key_of: &HashMap<BusId, String>) -> String {
    let mut s = String::from("name,bus,p_set,q_set,active\n");
    for (i, l) in net.loads.iter().enumerate() {
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

fn lines_csv(
    net: &Network,
    key_of: &HashMap<BusId, String>,
    kv_of: &HashMap<BusId, f64>,
) -> String {
    let mut s = String::from("name,bus0,bus1,r,x,b,s_nom,v_ang_min,v_ang_max,active\n");
    for (i, br) in net
        .branches
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.is_transformer())
    {
        // PyPSA per-unitizes line ohms on the BUS0 v_nom, not bus1.
        let zb = zbase(*kv_of.get(&br.from).unwrap_or(&0.0), net.base_mva);
        let _ = writeln!(
            s,
            "line_{},{},{},{},{},{},{},{},{},{}",
            i + 1,
            key_for(key_of, br.from),
            key_for(key_of, br.to),
            br.r * zb,
            br.x * zb,
            br.b / zb,
            br.rate_a,
            br.angmin,
            br.angmax,
            br.in_service
        );
    }
    s
}

fn transformers_csv(net: &Network, key_of: &HashMap<BusId, String>) -> String {
    let mut s = String::from("name,bus0,bus1,r,x,b,s_nom,tap_ratio,phase_shift,active\n");
    for (i, br) in net
        .branches
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
            net.base_mva
        };
        let _ = writeln!(
            s,
            "transformer_{},{},{},{},{},{},{},{},{},{}",
            i + 1,
            key_for(key_of, br.from),
            key_for(key_of, br.to),
            br.r * s_nom / net.base_mva,
            br.x * s_nom / net.base_mva,
            br.b * net.base_mva / s_nom,
            s_nom,
            br.effective_tap(),
            br.shift,
            br.in_service
        );
    }
    s
}

fn shunts_csv(
    net: &Network,
    key_of: &HashMap<BusId, String>,
    kv_of: &HashMap<BusId, f64>,
) -> String {
    let mut s = String::from("name,bus,g,b,active\n");
    for (i, sh) in net.shunts.iter().enumerate() {
        let zb = zbase(*kv_of.get(&sh.bus).unwrap_or(&0.0), net.base_mva);
        let _ = writeln!(
            s,
            "shunt_{},{},{},{},{}",
            i + 1,
            key_for(key_of, sh.bus),
            sh.g / (zb * net.base_mva),
            sh.b / (zb * net.base_mva),
            sh.in_service
        );
    }
    s
}

fn storage_csv(net: &Network, key_of: &HashMap<BusId, String>) -> String {
    let mut s = String::from(
        "name,bus,p_nom,max_hours,p_set,q_set,state_of_charge_initial,efficiency_store,efficiency_dispatch,cyclic_state_of_charge\n",
    );
    for (i, st) in net.storage.iter().enumerate() {
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

fn write_file(dir: &Path, name: &str, text: &str, files: &mut Vec<PathBuf>) -> Result<()> {
    let path = dir.join(name);
    std::fs::write(&path, text)?;
    files.push(path);
    Ok(())
}

#[derive(Debug)]
struct CsvTable {
    rows: Vec<CsvRow>,
}

#[derive(Debug)]
struct CsvRow {
    vals: HashMap<String, String>,
}

impl CsvRow {
    fn get(&self, key: &str) -> Option<&String> {
        self.vals.get(key).filter(|s| !s.is_empty())
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

fn read_csv_required(path: &Path, label: &'static str) -> Result<CsvTable> {
    read_csv_optional(path)?.ok_or_else(|| bad(format!("missing required `{label}`")))
}

fn read_csv_optional(path: &Path) -> Result<Option<CsvTable>> {
    // Only a missing file means an absent table; any other error (permissions,
    // a directory in the file's place) must surface, not read as an empty net.
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("csv");
    let mut records = parse_csv(&text, name)?
        .into_iter()
        .filter(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    let Some(headers) = records.next() else {
        return Ok(Some(CsvTable { rows: Vec::new() }));
    };
    let mut rows = Vec::new();
    for fields in records {
        let vals = headers
            .iter()
            .enumerate()
            .map(|(i, h)| (h.clone(), fields.get(i).cloned().unwrap_or_default()))
            .collect();
        rows.push(CsvRow { vals });
    }
    Ok(Some(CsvTable { rows }))
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
    use super::*;
    use std::fs;

    fn tmp_dir(label: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("powerio-pypsa-unit-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn folder(label: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = tmp_dir(label);
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
            area: 1,
            zone: 1,
            name: name.map(str::to_string),
            extras: Extras::default(),
        }
    }

    fn make_gen(bus: usize, cost: Option<GenCost>) -> Generator {
        Generator {
            bus: BusId(bus),
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
            qmin: f64::NEG_INFINITY,
            qmax: f64::INFINITY,
            r: 0.0,
            x: 0.0,
            p_loss: 0.0,
            q_loss: 0.0,
            in_service: true,
            extras: Extras::default(),
        }
    }

    fn xfmr(from: usize, to: usize, rate_a: f64) -> Branch {
        Branch {
            from: BusId(from),
            to: BusId(to),
            r: 0.125,
            x: 0.5,
            b: 0.25,
            rate_a,
            rate_b: 0.0,
            rate_c: 0.0,
            tap: 1.05,
            shift: 0.0,
            in_service: true,
            angmin: -360.0,
            angmax: 360.0,
            control: None,
            extras: Extras::default(),
        }
    }

    fn net_with(buses: Vec<Bus>) -> Network {
        Network::in_memory("t", 100.0, buses, Vec::new())
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
        assert_eq!(net.buses[0].id, BusId(5));
        assert_eq!(net.buses[1].id, BusId(2));
        assert!(net.buses[0].name.is_none());
        assert_eq!(net.loads[0].bus, BusId(5));
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
        assert_eq!(net.buses[0].id, BusId(1));
        assert_eq!(net.buses[1].id, BusId(2));
        assert_eq!(net.buses[0].name.as_deref(), Some("2"));
        assert_eq!(net.buses[1].name.as_deref(), Some("b"));
        // "2" resolves by name to the first bus, not numerically to the second.
        assert_eq!(net.loads[0].bus, BusId(1));
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
        assert_eq!(net.buses[0].kind, BusType::Ref);
        assert_eq!(net.buses[1].kind, BusType::Pv);
        assert_eq!(net.buses[2].kind, BusType::Pq);
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
        let br = &parsed.network.branches[0];
        close(br.r, 0.125); // 0.0625 * 100/50
        close(br.x, 0.5);
        close(br.b, 0.25); // 0.5 * 50/100
        assert_eq!(br.rate_a, 50.0);
        assert_eq!(br.tap, 1.05);
        assert!(
            parsed.warnings.iter().any(|w| w
                == "transformers.csv: g nonzero on 1 rows; transformer shunt conductance is not representable and was ignored"),
            "{:?}",
            parsed.warnings
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
    fn line_g_warns() {
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
        assert!(
            parsed.warnings.iter().any(|w| w
                == "lines.csv: g nonzero on 1 rows; line shunt conductance is not representable and was ignored"),
            "{:?}",
            parsed.warnings
        );
    }

    #[test]
    fn transformer_write_rebases_to_s_nom_base() {
        let mut net = net_with(vec![bus(1, None), bus(2, None)]);
        net.branches = vec![xfmr(1, 2, 50.0)];
        let key_of: HashMap<BusId, String> = net.buses.iter().map(|b| (b.id, bus_key(b))).collect();
        let csv = transformers_csv(&net, &key_of);
        assert_eq!(
            csv.lines().nth(1).unwrap(),
            "transformer_1,1,2,0.0625,0.25,0.5,50,1.05,0,true"
        );
    }

    #[test]
    fn transformer_write_zero_rate_a_uses_base_mva() {
        let mut net = net_with(vec![bus(1, None), bus(2, None)]);
        net.branches = vec![xfmr(1, 2, 0.0)];
        let key_of: HashMap<BusId, String> = net.buses.iter().map(|b| (b.id, bus_key(b))).collect();
        let csv = transformers_csv(&net, &key_of);
        assert_eq!(
            csv.lines().nth(1).unwrap(),
            "transformer_1,1,2,0.125,0.5,0.25,100,1.05,0,true"
        );
    }

    #[test]
    fn storage_write_fields_and_round_trip() {
        let mut net = net_with(vec![bus(1, None)]);
        net.storage = vec![storage_unit(1)];
        let dir = tmp_dir("storage-rt");
        let out = write_pypsa_csv_folder(&net, &dir).unwrap();
        assert!(
            !out.warnings.iter().any(|w| w.contains("storage units")),
            "{:?}",
            out.warnings
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
        let st = &back.storage[0];
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
        net.storage = vec![st];
        let out = write_pypsa_csv_folder(&net, tmp_dir("storage-lossy")).unwrap();
        assert!(
            out.warnings.iter().any(|w| w
                == "1 storage units lose fields PyPSA storage_units cannot carry (asymmetric charge/discharge ratings collapse to p_nom = max; thermal_rating, qmin/qmax, r/x, p_loss/q_loss dropped)"),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn named_buses_join_on_write() {
        let mut net = net_with(vec![bus(1, Some("North")), bus(2, None)]);
        net.generators = vec![make_gen(1, None)];
        net.loads = vec![Load {
            bus: BusId(2),
            p: 5.0,
            q: 1.0,
            in_service: true,
            extras: Extras::default(),
        }];
        let dir = tmp_dir("named-join");
        write_pypsa_csv_folder(&net, &dir).unwrap();
        let buses = fs::read_to_string(dir.join("buses.csv")).unwrap();
        assert!(buses.lines().nth(1).unwrap().starts_with("North,"));
        let gens = fs::read_to_string(dir.join("generators.csv")).unwrap();
        assert!(gens.lines().nth(1).unwrap().contains(",North,"), "{gens}");
        let back = read_pypsa_csv_folder(&dir).unwrap().network;
        assert_eq!(back.buses[0].name.as_deref(), Some("North"));
        assert_eq!(back.loads[0].bus, back.buses[1].id);
    }

    #[test]
    fn duplicate_bus_names_fall_back_to_ids() {
        let mut net = net_with(vec![bus(1, Some("X")), bus(2, Some("X"))]);
        net.loads = vec![Load {
            bus: BusId(2),
            p: 5.0,
            q: 1.0,
            in_service: true,
            extras: Extras::default(),
        }];
        let dir = tmp_dir("dup-keys");
        let out = write_pypsa_csv_folder(&net, &dir).unwrap();
        assert!(
            out.warnings.iter().any(|w| w
                == "buses.csv: bus names `X` collide with another bus name or id; those buses are keyed by their numeric id instead"),
            "{:?}",
            out.warnings
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
        assert_eq!(back.loads[0].bus, back.buses[1].id);
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
        let cost = parsed.network.generators[0].cost.as_ref().unwrap();
        assert_eq!(cost.coeffs, vec![0.25, 0.0, 0.0]);
    }

    #[test]
    fn bus_name_matching_another_bus_id_falls_back() {
        // A bus literally named "2" would collide with bus id 2's key.
        let net = net_with(vec![bus(1, Some("2")), bus(2, None)]);
        let dir = tmp_dir("name-id-clash");
        let out = write_pypsa_csv_folder(&net, &dir).unwrap();
        assert!(
            out.warnings.iter().any(|w| w.contains("`2`")),
            "{:?}",
            out.warnings
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
        let h = &parsed.network.hvdc[0];
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
            parsed.warnings.iter().any(|w| w
                == "links.csv: 1 links read as HVDC lines; PyPSA links carry no reactive or voltage data (q limits 0, voltage setpoints 1.0)"),
            "{:?}",
            parsed.warnings
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
        assert!(read_pypsa_csv_folder(&dir).unwrap().warnings.is_empty());
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
                .warnings
                .iter()
                .any(|w| w == "stores.csv ignored (1 rows): PyPSA stores are not mapped"),
            "{:?}",
            parsed.warnings
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
        net.generators = vec![
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
        let key_of: HashMap<BusId, String> = net.buses.iter().map(|b| (b.id, bus_key(b))).collect();
        let mut warnings = Vec::new();
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
                warnings.iter().any(|w| w == expected),
                "missing {expected:?} in {warnings:?}"
            );
        }
    }
}
