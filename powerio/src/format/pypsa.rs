//! Read and write PyPSA CSV folders.
//!
//! PyPSA's CSV folder is a directory format, so it does not fit the
//! `Conversion { text }` API used by single-file formats. The reader and writer
//! are exposed as path-based helpers and through `parse_file(..., "pypsa-csv")`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::network::{
    Branch, Bus, BusId, BusType, Extras, GenCost, Generator, Load, Network, Shunt, SourceFormat,
    Storage,
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

#[allow(clippy::too_many_lines)] // direct static-component CSV mapper; each block is one PyPSA table
pub fn read_pypsa_csv_folder(path: impl AsRef<Path>) -> Result<Network> {
    let path = path.as_ref();
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
    let mut buses = Vec::with_capacity(bus_table.rows.len());
    let mut id_of_name = HashMap::with_capacity(bus_table.rows.len());
    for (i, row) in bus_table.rows.iter().enumerate() {
        let raw_name = row
            .get("name")
            .cloned()
            .unwrap_or_else(|| (i + 1).to_string());
        let id = raw_name
            .parse::<usize>()
            .ok()
            .filter(|x| *x > 0)
            .unwrap_or(i + 1);
        let bus_id = BusId(id);
        id_of_name.insert(raw_name.clone(), bus_id);
        buses.push(Bus {
            id: bus_id,
            kind: BusType::Pq,
            vm: row.f("v_mag_pu_set").unwrap_or(1.0),
            va: 0.0,
            base_kv: row.f("v_nom").unwrap_or(0.0),
            vmax: row.f("v_mag_pu_max").unwrap_or(1.1),
            vmin: row.f("v_mag_pu_min").unwrap_or(0.9),
            area: 1,
            zone: 1,
            name: raw_name
                .parse::<usize>()
                .ok()
                .map_or(Some(raw_name), |_| None),
            extras: Extras::default(),
        });
    }
    let bus_pos: HashMap<BusId, usize> = buses.iter().enumerate().map(|(i, b)| (b.id, i)).collect();

    let mut loads = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("loads.csv"))? {
        for row in &table.rows {
            loads.push(Load {
                bus: bus_name_ref(row, "bus", &id_of_name),
                p: row.f("p_set").unwrap_or(0.0),
                q: row.f("q_set").unwrap_or(0.0),
                in_service: row.bool("active").unwrap_or(true),
                extras: Extras::default(),
            });
        }
    }

    let mut shunts = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("shunt_impedances.csv"))? {
        for row in &table.rows {
            let bus = bus_name_ref(row, "bus", &id_of_name);
            let zb = zbase(bus_kv(&buses, &bus_pos, bus), base_mva);
            shunts.push(Shunt {
                bus,
                g: row.f("g").unwrap_or(0.0) * zb * base_mva,
                b: row.f("b").unwrap_or(0.0) * zb * base_mva,
                in_service: row.bool("active").unwrap_or(true),
                extras: Extras::default(),
            });
        }
    }

    let mut generators = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("generators.csv"))? {
        for row in &table.rows {
            let bus = bus_name_ref(row, "bus", &id_of_name);
            let control = row.get("control").map_or("", String::as_str);
            set_bus_kind(
                &mut buses,
                &bus_pos,
                bus,
                if control.eq_ignore_ascii_case("Slack") {
                    BusType::Ref
                } else {
                    BusType::Pv
                },
            );
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
                    (Some(q), Some(c)) => Some(GenCost {
                        model: 2,
                        startup: 0.0,
                        shutdown: 0.0,
                        ncost: 3,
                        coeffs: vec![q, c, 0.0],
                    }),
                    (None, Some(c)) => Some(GenCost {
                        model: 2,
                        startup: 0.0,
                        shutdown: 0.0,
                        ncost: 2,
                        coeffs: vec![c, 0.0],
                    }),
                    _ => None,
                },
                caps: [None; crate::network::GEN_EXTRA_KEYS.len()],
            });
        }
    }

    let mut branches = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("lines.csv"))? {
        for row in &table.rows {
            let from = bus_name_ref(row, "bus0", &id_of_name);
            let to = bus_name_ref(row, "bus1", &id_of_name);
            let zb = zbase(bus_kv(&buses, &bus_pos, to), base_mva);
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
                extras: Extras::default(),
            });
        }
    }
    if let Some(table) = read_csv_optional(&path.join("transformers.csv"))? {
        for row in &table.rows {
            branches.push(Branch {
                from: bus_name_ref(row, "bus0", &id_of_name),
                to: bus_name_ref(row, "bus1", &id_of_name),
                r: row.f("r").unwrap_or(0.0),
                x: row.f("x").unwrap_or(0.0),
                b: 0.0,
                rate_a: row.f("s_nom").unwrap_or(0.0),
                rate_b: 0.0,
                rate_c: 0.0,
                tap: row.f("tap_ratio").unwrap_or(1.0),
                shift: row.f("phase_shift").unwrap_or(0.0),
                in_service: row.bool("active").unwrap_or(true),
                angmin: -360.0,
                angmax: 360.0,
                extras: Extras::default(),
            });
        }
    }

    let mut storage = Vec::new();
    if let Some(table) = read_csv_optional(&path.join("storage_units.csv"))? {
        for row in &table.rows {
            let p_nom = row.f("p_nom").unwrap_or(0.0);
            let max_hours = row.f("max_hours").unwrap_or(0.0);
            storage.push(Storage {
                bus: bus_name_ref(row, "bus", &id_of_name),
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

    let net = Network {
        name,
        base_mva,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage,
        hvdc: Vec::new(),
        source_format: SourceFormat::PypsaCsv,
        source: None,
    };
    net.check_references(FMT)?;
    Ok(net)
}

pub fn write_pypsa_csv_folder(net: &Network, out_dir: impl AsRef<Path>) -> Result<PypsaCsvOutputs> {
    let out_dir = out_dir.as_ref();
    std::fs::create_dir_all(out_dir)?;
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    if !net.hvdc.is_empty() {
        warnings.push(format!(
            "{} dcline(s) dropped: PyPSA CSV writer v1 does not model HVDC links",
            net.hvdc.len()
        ));
    }
    if net.generators.iter().any(Generator::has_caps) {
        warnings.push("generator capability/ramp columns dropped: PyPSA generator CSV has no MATPOWER capability columns".into());
    }
    if net
        .storage
        .iter()
        .any(|s| s.energy_rating != 0.0 || s.ps != 0.0 || s.qs != 0.0)
    {
        warnings
            .push("storage fields are mapped to PyPSA storage_units with reduced fidelity".into());
    }

    write_file(out_dir, "network.csv", &network_csv(net), &mut files)?;
    write_file(out_dir, "snapshots.csv", ",snapshot\n0,now\n", &mut files)?;
    write_file(out_dir, "buses.csv", &buses_csv(net), &mut files)?;
    write_file(out_dir, "generators.csv", &generators_csv(net), &mut files)?;
    write_file(out_dir, "loads.csv", &loads_csv(net), &mut files)?;
    write_file(out_dir, "lines.csv", &lines_csv(net), &mut files)?;
    let transformers = transformers_csv(net);
    if transformers.lines().count() > 1 {
        write_file(out_dir, "transformers.csv", &transformers, &mut files)?;
    }
    if !net.shunts.is_empty() {
        write_file(
            out_dir,
            "shunt_impedances.csv",
            &shunts_csv(net),
            &mut files,
        )?;
    }
    if !net.storage.is_empty() {
        write_file(out_dir, "storage_units.csv", &storage_csv(net), &mut files)?;
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

fn buses_csv(net: &Network) -> String {
    let mut s = String::from("name,v_nom,v_mag_pu_set,v_mag_pu_min,v_mag_pu_max\n");
    for b in &net.buses {
        let _ = writeln!(
            s,
            "{},{},{},{},{}",
            bus_name(b),
            b.base_kv,
            b.vm,
            b.vmin,
            b.vmax
        );
    }
    s
}

fn generators_csv(net: &Network) -> String {
    let mut s = String::from(
        "name,bus,control,p_nom,p_set,q_set,p_min_pu,p_max_pu,marginal_cost,marginal_cost_quadratic,active,v_mag_pu_set\n",
    );
    let bus_kind: HashMap<BusId, BusType> = net.buses.iter().map(|b| (b.id, b.kind)).collect();
    for (i, g) in net.generators.iter().enumerate() {
        let p_nom = if g.pmax.is_finite() && g.pmax > 0.0 {
            g.pmax
        } else {
            g.pg.abs().max(1.0)
        };
        let (c2, c1) = g
            .cost
            .as_ref()
            .and_then(|c| match c.coeffs.as_slice() {
                [c2, c1, ..] if c.model == 2 => Some((*c2, *c1)),
                [c1, ..] if c.model == 2 => Some((0.0, *c1)),
                _ => None,
            })
            .unwrap_or((0.0, 0.0));
        let _ = writeln!(
            s,
            "gen_{},{},{},{},{},{},{},{},{},{},{},{}",
            i + 1,
            g.bus.0,
            if bus_kind.get(&g.bus).copied() == Some(BusType::Ref) {
                "Slack"
            } else {
                "PV"
            },
            p_nom,
            g.pg,
            g.qg,
            if p_nom == 0.0 { 0.0 } else { g.pmin / p_nom },
            if p_nom == 0.0 || !g.pmax.is_finite() {
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
    s
}

fn loads_csv(net: &Network) -> String {
    let mut s = String::from("name,bus,p_set,q_set,active\n");
    for (i, l) in net.loads.iter().enumerate() {
        let _ = writeln!(
            s,
            "load_{},{},{},{},{}",
            i + 1,
            l.bus.0,
            l.p,
            l.q,
            l.in_service
        );
    }
    s
}

fn lines_csv(net: &Network) -> String {
    let mut s = String::from("name,bus0,bus1,r,x,b,s_nom,v_ang_min,v_ang_max,active\n");
    let bus_kv: HashMap<BusId, f64> = net.buses.iter().map(|b| (b.id, b.base_kv)).collect();
    for (i, br) in net
        .branches
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.is_transformer())
    {
        let zb = zbase(*bus_kv.get(&br.to).unwrap_or(&0.0), net.base_mva);
        let _ = writeln!(
            s,
            "line_{},{},{},{},{},{},{},{},{},{}",
            i + 1,
            br.from.0,
            br.to.0,
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

fn transformers_csv(net: &Network) -> String {
    let mut s = String::from("name,bus0,bus1,r,x,s_nom,tap_ratio,phase_shift,active\n");
    for (i, br) in net
        .branches
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_transformer())
    {
        let _ = writeln!(
            s,
            "transformer_{},{},{},{},{},{},{},{},{}",
            i + 1,
            br.from.0,
            br.to.0,
            br.r,
            br.x,
            br.rate_a,
            br.effective_tap(),
            br.shift,
            br.in_service
        );
    }
    s
}

fn shunts_csv(net: &Network) -> String {
    let mut s = String::from("name,bus,g,b,active\n");
    let bus_kv: HashMap<BusId, f64> = net.buses.iter().map(|b| (b.id, b.base_kv)).collect();
    for (i, sh) in net.shunts.iter().enumerate() {
        let zb = zbase(*bus_kv.get(&sh.bus).unwrap_or(&0.0), net.base_mva);
        let _ = writeln!(
            s,
            "shunt_{},{},{},{},{}",
            i + 1,
            sh.bus.0,
            sh.g / (zb * net.base_mva),
            sh.b / (zb * net.base_mva),
            sh.in_service
        );
    }
    s
}

fn storage_csv(net: &Network) -> String {
    let mut s = String::from(
        "name,bus,p_nom,max_hours,efficiency_store,efficiency_dispatch,cyclic_state_of_charge\n",
    );
    for (i, st) in net.storage.iter().enumerate() {
        let p_nom = st
            .charge_rating
            .max(st.discharge_rating)
            .max(st.energy_rating);
        let max_hours = if p_nom > 0.0 {
            st.energy_rating / p_nom
        } else {
            0.0
        };
        let _ = writeln!(
            s,
            "storage_{},{},{},{},{},{},true",
            i + 1,
            st.bus.0,
            p_nom,
            max_hours,
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

fn read_csv_required(path: &Path, label: &'static str) -> Result<CsvTable> {
    read_csv_optional(path)?.ok_or_else(|| Error::FormatRead {
        format: FMT,
        message: format!("missing required `{label}`"),
    })
}

fn read_csv_optional(path: &Path) -> Result<Option<CsvTable>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();
    let Some(header_line) = lines.next() else {
        return Ok(Some(CsvTable { rows: Vec::new() }));
    };
    let headers = parse_csv_line(header_line);
    let mut rows = Vec::new();
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let fields = parse_csv_line(line);
        let vals = headers
            .iter()
            .enumerate()
            .map(|(i, h)| (h.clone(), fields.get(i).cloned().unwrap_or_default()))
            .collect();
        rows.push(CsvRow { vals });
    }
    Ok(Some(CsvTable { rows }))
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                let _ = chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(cur);
                cur = String::new();
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn bus_name(b: &Bus) -> String {
    esc(b.name.as_deref().unwrap_or(&b.id.0.to_string()))
}

fn esc(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn bus_name_ref(row: &CsvRow, key: &str, id_of_name: &HashMap<String, BusId>) -> BusId {
    let raw = row.get(key).cloned().unwrap_or_default();
    id_of_name
        .get(&raw)
        .copied()
        .or_else(|| raw.parse::<usize>().ok().map(BusId))
        .unwrap_or(BusId(0))
}

fn set_bus_kind(buses: &mut [Bus], bus_pos: &HashMap<BusId, usize>, bus: BusId, kind: BusType) {
    if let Some(&idx) = bus_pos.get(&bus) {
        if buses[idx].kind != BusType::Isolated {
            buses[idx].kind = kind;
        }
    }
}

fn bus_kv(buses: &[Bus], bus_pos: &HashMap<BusId, usize>, bus: BusId) -> f64 {
    bus_pos
        .get(&bus)
        .and_then(|&i| buses.get(i))
        .map_or(0.0, |b| b.base_kv)
}

fn zbase(v_kv: f64, base_mva: f64) -> f64 {
    if v_kv > 0.0 && base_mva > 0.0 {
        v_kv * v_kv / base_mva
    } else {
        1.0
    }
}
