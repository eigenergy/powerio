//! Map a parsed [`AuxFile`] to the typed [`Network`], and write a `Network`
//! back out as aux text.
//!
//! Only the power flow core object types (Bus, Load, Shunt, Gen, Branch) feed
//! the typed model; every other `DATA` section stays reachable through the
//! generic layer (see [`super::aux`]) and survives the same format round trip
//! via the retained source.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use super::aux::{AuxFile, AuxObject, parse_aux};
use crate::format::Conversion;
use crate::network::{
    Branch, Bus, BusId, BusType, Extras, Generator, Load, Network, Shunt, SourceFormat,
};
use crate::{Error, Result};

const FMT: &str = "PowerWorld .aux";

// ---- Reader -----------------------------------------------------------------

/// Owned-source entry used by the format hub: parse by borrowing `source`, then
/// move the buffer into the retained source (no copy). `name_hint` (e.g. a file
/// stem) names the network when the `.aux` carries no export marker.
pub(crate) fn parse_powerworld_source(
    source: Arc<String>,
    name_hint: Option<&str>,
) -> Result<Network> {
    let content: &str = &source;
    // PowerWorld `.aux` does not carry the system base in the case data, so we
    // default to 100 MVA (the de-facto standard, and what our own writer records
    // in the `// baseMVA` marker below). Reading a real base from PowerWorld's
    // project files is tracked separately; defaulting here is deliberate, not a
    // silent guess — erroring would reject every base-less third-party `.aux`.
    let mut base_mva = 100.0;
    let mut name = name_hint.unwrap_or("case").to_string();
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("// baseMVA ") {
            if let Ok(v) = rest.trim().parse::<f64>() {
                base_mva = v;
            }
        } else if let Some((_, n)) = t.split_once("powerio export: ") {
            name = n.trim().to_string();
        }
    }

    let aux = parse_aux(content)?;
    let mut buses = Vec::new();
    let mut loads = Vec::new();
    let mut shunts = Vec::new();
    let mut generators = Vec::new();
    let mut branches = Vec::new();
    let mut saw_any = false;

    for blk in aux.data() {
        saw_any = true;
        match blk.object_type.as_str() {
            "Bus" => {
                for r in rows(blk) {
                    buses.push(read_bus(&r)?);
                }
            }
            "Load" => {
                for r in rows(blk) {
                    loads.push(read_load(&r)?);
                }
            }
            "Shunt" => {
                for r in rows(blk) {
                    shunts.push(read_shunt(&r)?);
                }
            }
            "Gen" => {
                for r in rows(blk) {
                    generators.push(read_gen(&r)?);
                }
            }
            "Branch" => {
                for r in rows(blk) {
                    branches.push(read_branch(&r)?);
                }
            }
            _ => {} // unmodeled object block: retained via the generic layer
        }
    }
    if !saw_any {
        return Err(Error::FormatRead {
            format: FMT,
            message: "no DATA blocks found".into(),
        });
    }

    let net = Network {
        name,
        base_mva,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage: Vec::new(),
        hvdc: Vec::new(),
        source_format: SourceFormat::PowerWorld,
        source: Some(source),
    };
    net.check_references(FMT)?;
    Ok(net)
}

/// Parse the auxiliary sections of a PowerWorld-sourced [`Network`]'s retained
/// source. The typed model carries the power flow core; everything else in the
/// original file (contingencies, limit sets, substations, ...) is reachable
/// here.
///
/// Returns `None` when the network was not read from a `.aux` source.
///
/// # Errors
/// As [`parse_aux`], on a retained source that no longer parses.
pub fn aux_sections(net: &Network) -> Option<Result<AuxFile>> {
    if net.source_format != SourceFormat::PowerWorld {
        return None;
    }
    net.source.as_ref().map(|s| parse_aux(s))
}

type Row<'a> = HashMap<&'a str, &'a str>;

/// Each row of `blk` as a field-name → value map keyed by the declared field
/// list.
fn rows(blk: &AuxObject) -> impl Iterator<Item = Row<'_>> {
    blk.rows.iter().map(|row| {
        blk.fields
            .iter()
            .map(String::as_str)
            .zip(row.values.iter().map(String::as_str))
            .collect()
    })
}

fn bad_field(key: &str, tok: &str) -> Error {
    Error::FormatRead {
        format: FMT,
        message: format!("field {key} {tok:?} is not a number"),
    }
}

/// Field `key` as f64, defaulting to 0.0 when absent. Present but unparseable is
/// a hard error: a malformed number must not silently become a plausible default
/// and corrupt the matrices downstream.
fn f(r: &Row, key: &str) -> Result<f64> {
    f_or(r, key, 0.0)
}
/// Field `key` as f64, absent → `default`, present-but-unparseable → error.
fn f_or(r: &Row, key: &str, default: f64) -> Result<f64> {
    match r.get(key).copied() {
        None | Some("") => Ok(default),
        Some(s) => s.trim().parse().map_err(|_| bad_field(key, s)),
    }
}
/// Field `key` as a bus id (parsed as f64 then truncated). Absent → 0;
/// present-but-unparseable → error.
fn uid(r: &Row, key: &str) -> Result<usize> {
    match r.get(key).copied() {
        None | Some("") => Ok(0),
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(s) => s
            .trim()
            .parse::<f64>()
            .map(|v| v as usize)
            .map_err(|_| bad_field(key, s)),
    }
}
fn on(r: &Row, key: &str) -> bool {
    !matches!(r.get(key).copied(), Some("Open" | "OPEN" | "0"))
}

fn bus_kind(cat: Option<&str>) -> BusType {
    match cat {
        Some("PV") => BusType::Pv,
        Some("Slack") => BusType::Ref,
        Some("Disconnected") => BusType::Isolated,
        _ => BusType::Pq,
    }
}

fn read_bus(r: &Row) -> Result<Bus> {
    let id = r
        .get("BusNum")
        .and_then(|v| v.parse::<f64>().ok())
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "Bus block row missing numeric BusNum".into(),
        })? as usize;
    let name = r
        .get("BusName")
        .filter(|n| !n.is_empty())
        .map(ToString::to_string);
    Ok(Bus {
        id: BusId(id),
        kind: bus_kind(r.get("BusCat").copied()),
        vm: f_or(r, "BusPUVolt", 1.0)?,
        va: f(r, "BusAngle")?,
        base_kv: f(r, "BusNomVolt")?,
        vmax: f_or(r, "BusVMax", 1.1)?,
        vmin: f_or(r, "BusVMin", 0.9)?,
        area: uid(r, "AreaNum")?,
        zone: uid(r, "ZoneNum")?,
        name,
        extras: Extras::new(),
    })
}

fn read_load(r: &Row) -> Result<Load> {
    Ok(Load {
        bus: BusId(uid(r, "BusNum")?),
        p: f(r, "LoadMW")?,
        q: f(r, "LoadMVR")?,
        in_service: on(r, "LoadStatus"),
        extras: Extras::new(),
    })
}

fn read_shunt(r: &Row) -> Result<Shunt> {
    Ok(Shunt {
        bus: BusId(uid(r, "BusNum")?),
        g: f(r, "ShuntMW")?,
        b: f(r, "ShuntMVR")?,
        in_service: on(r, "ShuntStatus"),
        extras: Extras::new(),
    })
}

fn read_gen(r: &Row) -> Result<Generator> {
    Ok(Generator {
        bus: BusId(uid(r, "BusNum")?),
        pg: f(r, "GenMW")?,
        qg: f(r, "GenMVR")?,
        pmax: f(r, "GenMWMax")?,
        pmin: f(r, "GenMWMin")?,
        qmax: f(r, "GenMVRMax")?,
        qmin: f(r, "GenMVRMin")?,
        vg: f_or(r, "GenVoltSet", 1.0)?,
        mbase: f_or(r, "GenMVABase", 100.0)?,
        in_service: on(r, "GenStatus"),
        cost: None,
        caps: Default::default(),
    })
}

fn read_branch(r: &Row) -> Result<Branch> {
    let is_xf = matches!(r.get("BranchDeviceType").copied(), Some("Transformer"));
    let tap = f_or(r, "LineXFRatio", 1.0)?;
    Ok(Branch {
        from: BusId(uid(r, "BusNum")?),
        to: BusId(uid(r, "BusNum:1")?),
        r: f(r, "LineR")?,
        x: f(r, "LineX")?,
        b: f(r, "LineC")?,
        rate_a: f(r, "LineAMVA")?,
        rate_b: f(r, "LineBMVA")?,
        rate_c: f(r, "LineCMVA")?,
        tap: if is_xf { tap } else { 0.0 },
        shift: f(r, "LinePhase")?,
        in_service: on(r, "LineStatus"),
        angmin: -360.0,
        angmax: 360.0,
        extras: Extras::new(),
    })
}

// ---- Writer -----------------------------------------------------------------

#[must_use]
// A flat serializer: one section per PowerWorld object type; splitting it would
// add indirection without clarity.
#[expect(clippy::too_many_lines)]
pub fn write_powerworld(net: &Network) -> Conversion {
    let mut warnings = Vec::new();
    let mut nonfinite = false;
    let mut n = |x: f64| -> String {
        if x.is_finite() {
            format!("{x}")
        } else {
            nonfinite = true;
            format!(
                "{}",
                if x > 0.0 {
                    1.0e10
                } else if x < 0.0 {
                    -1.0e10
                } else {
                    0.0
                }
            )
        }
    };
    let mut s = String::new();
    let _ = writeln!(
        s,
        "// PowerWorld auxiliary file — powerio export: {}",
        net.name
    );
    let _ = writeln!(s, "// baseMVA {}", net.base_mva);
    let _ = writeln!(s);

    block(
        &mut s,
        "Bus",
        "[BusNum, BusName, BusNomVolt, BusPUVolt, BusAngle, AreaNum, ZoneNum, BusVMax, BusVMin, BusCat]",
        |rows| {
            for b in &net.buses {
                rows.push(format!(
                    "{} \"{}\" {} {} {} {} {} {} {} \"{}\"",
                    b.id,
                    b.name.as_deref().unwrap_or(""),
                    n(b.base_kv),
                    n(b.vm),
                    n(b.va),
                    b.area,
                    b.zone,
                    n(b.vmax),
                    n(b.vmin),
                    bus_cat(b.kind)
                ));
            }
        },
    );

    block(
        &mut s,
        "Load",
        "[BusNum, LoadID, LoadMW, LoadMVR, LoadStatus]",
        |rows| {
            for (i, l) in net.loads.iter().enumerate() {
                rows.push(format!(
                    "{} \"{}\" {} {} \"{}\"",
                    l.bus,
                    i + 1,
                    n(l.p),
                    n(l.q),
                    status(l.in_service)
                ));
            }
        },
    );

    block(
        &mut s,
        "Shunt",
        "[BusNum, ShuntID, ShuntMW, ShuntMVR, ShuntStatus]",
        |rows| {
            for (i, sh) in net.shunts.iter().enumerate() {
                rows.push(format!(
                    "{} \"{}\" {} {} \"{}\"",
                    sh.bus,
                    i + 1,
                    n(sh.g),
                    n(sh.b),
                    status(sh.in_service)
                ));
            }
        },
    );

    block(
        &mut s,
        "Gen",
        "[BusNum, GenID, GenMW, GenMVR, GenMWMax, GenMWMin, GenMVRMax, GenMVRMin, GenVoltSet, GenMVABase, GenStatus]",
        |rows| {
            for (i, g) in net.generators.iter().enumerate() {
                rows.push(format!(
                    "{} \"{}\" {} {} {} {} {} {} {} {} \"{}\"",
                    g.bus,
                    i + 1,
                    n(g.pg),
                    n(g.qg),
                    n(g.pmax),
                    n(g.pmin),
                    n(g.qmax),
                    n(g.qmin),
                    n(g.vg),
                    n(g.mbase),
                    status(g.in_service)
                ));
            }
        },
    );

    block(
        &mut s,
        "Branch",
        "[BusNum, BusNum:1, LineCircuit, LineR, LineX, LineC, LineAMVA, LineBMVA, LineCMVA, LineXFRatio, LinePhase, LineStatus, BranchDeviceType]",
        |rows| {
            for br in &net.branches {
                let kind = if br.is_transformer() {
                    "Transformer"
                } else {
                    "Line"
                };
                rows.push(format!(
                    "{} {} \"1\" {} {} {} {} {} {} {} {} \"{}\" \"{}\"",
                    br.from,
                    br.to,
                    n(br.r),
                    n(br.x),
                    n(br.b),
                    n(br.rate_a),
                    n(br.rate_b),
                    n(br.rate_c),
                    n(br.effective_tap()),
                    n(br.shift),
                    status(br.in_service),
                    kind
                ));
            }
        },
    );

    if net.generators.iter().any(|g| g.cost.is_some()) {
        warnings.push("generator cost curves dropped: not written to PowerWorld .aux".into());
    }
    if !net.hvdc.is_empty() {
        warnings.push(format!(
            "{} dcline(s) dropped: PowerWorld HVDC not modeled",
            net.hvdc.len()
        ));
    }
    if !net.storage.is_empty() {
        warnings.push(format!(
            "{} storage unit(s) dropped: PowerWorld storage not modeled",
            net.storage.len()
        ));
    }
    if net.branches.iter().any(Branch::has_angle_limits) {
        warnings.push(
            "branch angle limits (angmin/angmax) dropped: not written to PowerWorld .aux".into(),
        );
    }
    if net.generators.iter().any(Generator::has_caps) {
        warnings.push(
            "generator ramp/capability columns dropped: not written to PowerWorld .aux".into(),
        );
    }
    if nonfinite {
        warnings.push("non-finite values written as ±1e10 sentinels".into());
    }

    Conversion { text: s, warnings }
}

fn block(s: &mut String, object: &str, fields: &str, fill: impl FnOnce(&mut Vec<String>)) {
    let mut rows = Vec::new();
    fill(&mut rows);
    let _ = writeln!(s, "DATA ({object}, {fields})");
    let _ = writeln!(s, "{{");
    for r in &rows {
        let _ = writeln!(s, "  {r}");
    }
    let _ = writeln!(s, "}}");
    let _ = writeln!(s);
}

fn status(on: bool) -> &'static str {
    if on { "Closed" } else { "Open" }
}

fn bus_cat(kind: BusType) -> &'static str {
    match kind {
        BusType::Pq => "PQ",
        BusType::Pv => "PV",
        BusType::Ref => "Slack",
        BusType::Isolated => "Disconnected",
    }
}
