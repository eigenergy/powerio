//! Read and write PowerWorld auxiliary `.aux` files.
//!
//! Emits `DATA (Object, [fields]) { … }` blocks for Bus, Load, Shunt, Gen, and
//! Branch — the core transmission objects — with values in MW/MVAr/degrees and
//! status as `Closed`/`Open`. The reader maps each block by its declared field
//! list, so column order doesn't matter. Generator cost, HVDC, and storage are
//! not represented and are reported on write. Same-format round-trip is byte-exact
//! via the retained source (see [`crate::write_as`]); this is the cross-format
//! path. The `.pwb` binary format is proprietary and out of scope.

use std::fmt::Write as _;
use std::sync::Arc;

use super::Conversion;
use crate::network::{Branch, Bus, BusType, Extras, Generator, Load, Network, Shunt, SourceFormat};
use crate::{Error, Result};

const FMT: &str = "PowerWorld .aux";

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
        "// PowerWorld auxiliary file — caseio export: {}",
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

// ---- Reader -----------------------------------------------------------------

/// Parse a PowerWorld `.aux` into a [`Network`], reading the Bus/Load/Shunt/Gen/
/// Branch `DATA` blocks by their declared field lists.
pub fn parse_powerworld(content: &str) -> Result<Network> {
    parse_powerworld_source(Arc::new(content.to_owned()), None)
}

/// Owned-source entry used by the format hub: parse by borrowing `source`, then
/// move the buffer into the retained source (no copy). `name_hint` (e.g. a file
/// stem) names the network when the `.aux` carries no export marker.
pub(crate) fn parse_powerworld_source(
    source: Arc<String>,
    name_hint: Option<&str>,
) -> Result<Network> {
    let content: &str = &source;
    let mut base_mva = 100.0;
    let mut name = name_hint.unwrap_or("case").to_string();
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("// baseMVA ") {
            if let Ok(v) = rest.trim().parse::<f64>() {
                base_mva = v;
            }
        } else if let Some((_, n)) = t.split_once("caseio export: ") {
            name = n.trim().to_string();
        }
    }

    let mut buses = Vec::new();
    let mut loads = Vec::new();
    let mut shunts = Vec::new();
    let mut generators = Vec::new();
    let mut branches = Vec::new();
    let mut saw_any = false;

    for blk in DataBlocks::new(content) {
        saw_any = true;
        match blk.object.as_str() {
            "Bus" => {
                for r in blk.rows() {
                    buses.push(read_bus(&r)?);
                }
            }
            "Load" => loads.extend(blk.rows().map(|r| read_load(&r))),
            "Shunt" => shunts.extend(blk.rows().map(|r| read_shunt(&r))),
            "Gen" => generators.extend(blk.rows().map(|r| read_gen(&r))),
            "Branch" => branches.extend(blk.rows().map(|r| read_branch(&r))),
            _ => {} // unmodeled object block: skipped
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

/// One `DATA (Object, [fields]) { … }` block: the object name, its field list,
/// and the raw value rows.
struct Block {
    object: String,
    fields: Vec<String>,
    raw_rows: Vec<String>,
}

impl Block {
    /// Each row as a field-name → value map (keyed by the declared field list).
    fn rows(&self) -> impl Iterator<Item = std::collections::HashMap<&str, String>> + '_ {
        self.raw_rows.iter().map(move |line| {
            let vals = split_values(line);
            self.fields
                .iter()
                .zip(vals)
                .map(|(k, v)| (k.as_str(), v))
                .collect()
        })
    }
}

/// Iterate the `DATA` blocks in a `.aux` file.
struct DataBlocks<'a> {
    lines: std::iter::Peekable<std::str::Lines<'a>>,
}

impl<'a> DataBlocks<'a> {
    fn new(content: &'a str) -> Self {
        Self {
            lines: content.lines().peekable(),
        }
    }
}

impl Iterator for DataBlocks<'_> {
    type Item = Block;

    fn next(&mut self) -> Option<Block> {
        // Find the next `DATA (Object, [fields])` header.
        let (object, fields) = loop {
            let line = self.lines.next()?;
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("DATA") {
                let inner = rest.trim().trim_start_matches('(').trim_end_matches(')');
                let (obj, flds) = inner.split_once(',')?;
                let fields = flds
                    .trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(|f| f.trim().to_string())
                    .collect();
                break (obj.trim().to_string(), fields);
            }
        };
        // Body between `{` and `}`.
        while self.lines.peek().map(|l| l.trim()) != Some("{") {
            self.lines.next()?;
        }
        self.lines.next(); // consume `{`
        let mut raw_rows = Vec::new();
        for line in self.lines.by_ref() {
            let t = line.trim();
            if t == "}" {
                break;
            }
            if !t.is_empty() {
                raw_rows.push(t.to_string());
            }
        }
        Some(Block {
            object,
            fields,
            raw_rows,
        })
    }
}

/// Split a value row on whitespace, keeping quoted strings intact (quotes
/// removed). An empty quoted token (`""`) is preserved as an empty field so it
/// doesn't shift later columns.
fn split_values(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut started = false; // a token has begun, including an empty quoted one
    for c in line.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                started = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() || started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if !cur.is_empty() || started {
        out.push(cur);
    }
    out
}

type Row<'a> = std::collections::HashMap<&'a str, String>;

fn f(r: &Row, key: &str) -> f64 {
    r.get(key)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0)
}
fn f_or(r: &Row, key: &str, default: f64) -> f64 {
    r.get(key)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}
fn uid(r: &Row, key: &str) -> usize {
    r.get(key)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0) as usize
}
fn on(r: &Row, key: &str) -> bool {
    !matches!(r.get(key).map(String::as_str), Some("Open" | "OPEN" | "0"))
}

fn bus_kind(cat: Option<&String>) -> BusType {
    match cat.map(String::as_str) {
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
    let name = r.get("BusName").filter(|n| !n.is_empty()).cloned();
    Ok(Bus {
        id,
        kind: bus_kind(r.get("BusCat")),
        vm: f_or(r, "BusPUVolt", 1.0),
        va: f(r, "BusAngle"),
        base_kv: f(r, "BusNomVolt"),
        vmax: f_or(r, "BusVMax", 1.1),
        vmin: f_or(r, "BusVMin", 0.9),
        area: uid(r, "AreaNum"),
        zone: uid(r, "ZoneNum"),
        name,
        extras: Extras::new(),
    })
}

fn read_load(r: &Row) -> Load {
    Load {
        bus: uid(r, "BusNum"),
        p: f(r, "LoadMW"),
        q: f(r, "LoadMVR"),
        in_service: on(r, "LoadStatus"),
        extras: Extras::new(),
    }
}

fn read_shunt(r: &Row) -> Shunt {
    Shunt {
        bus: uid(r, "BusNum"),
        g: f(r, "ShuntMW"),
        b: f(r, "ShuntMVR"),
        in_service: on(r, "ShuntStatus"),
        extras: Extras::new(),
    }
}

fn read_gen(r: &Row) -> Generator {
    Generator {
        bus: uid(r, "BusNum"),
        pg: f(r, "GenMW"),
        qg: f(r, "GenMVR"),
        pmax: f(r, "GenMWMax"),
        pmin: f(r, "GenMWMin"),
        qmax: f(r, "GenMVRMax"),
        qmin: f(r, "GenMVRMin"),
        vg: f_or(r, "GenVoltSet", 1.0),
        mbase: f_or(r, "GenMVABase", 100.0),
        in_service: on(r, "GenStatus"),
        cost: None,
        caps: Default::default(),
    }
}

fn read_branch(r: &Row) -> Branch {
    let is_xf = matches!(
        r.get("BranchDeviceType").map(String::as_str),
        Some("Transformer")
    );
    let tap = f_or(r, "LineXFRatio", 1.0);
    Branch {
        from: uid(r, "BusNum"),
        to: uid(r, "BusNum:1"),
        r: f(r, "LineR"),
        x: f(r, "LineX"),
        b: f(r, "LineC"),
        rate_a: f(r, "LineAMVA"),
        rate_b: f(r, "LineBMVA"),
        rate_c: f(r, "LineCMVA"),
        tap: if is_xf { tap } else { 0.0 },
        shift: f(r, "LinePhase"),
        in_service: on(r, "LineStatus"),
        angmin: -360.0,
        angmax: 360.0,
        extras: Extras::new(),
    }
}
