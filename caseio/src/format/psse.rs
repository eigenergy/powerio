//! Read and write PSS/E `.raw` (revision 33).
//!
//! Covers the core sections — bus, load, fixed shunt, generator, branch, and
//! 2-winding transformer — which together carry a transmission power flow case.
//! Impedances are written on the system base with per-unit turns ratios
//! (`CZ = 1`, `CW = 1`); the reader assumes the same and does not convert other
//! impedance/turns bases — a non-unit `CZ`/`CW` is read verbatim (so misread).
//! 3-winding transformers, two-terminal DC, switched shunts, and the other
//! advanced sections are not modeled: on write they're emitted as empty
//! sections, on read they're skipped, and HVDC/storage carried on the `Network`
//! are reported as dropped. Same-format round-trip is byte-exact via the retained
//! source (see [`crate::write_as`]); this serializer is the cross-format path.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use super::Conversion;
use crate::network::BusType;
use crate::network::{Branch, Bus, Extras, Generator, Load, Network, Shunt, SourceFormat};
use crate::{Error, Result};

const FMT: &str = "PSS/E .raw";
const REV: u32 = 33;

// ---- Writer -----------------------------------------------------------------

#[must_use]
pub fn write_psse(net: &Network) -> Conversion {
    let mut warnings = Vec::new();
    let mut nonfinite = false;
    let mut s = String::new();
    // A formatter that records when a value can't be represented (PSS/E is fixed
    // numeric — no Inf/NaN).
    let mut num = |x: f64| -> String {
        if x.is_finite() {
            let s = format!("{x}");
            // PSS/E v33 readers treat a record whose first field is exactly "0" as
            // a section terminator (PowerModels' pti.jl). A transformer impedance
            // line can start with R = 0, so never emit a bare integer "0": give it
            // a decimal, matching PSS/E's own numeric convention.
            if s.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                format!("{s}.0")
            } else {
                s
            }
        } else {
            nonfinite = true;
            let sentinel = if x > 0.0 { 1.0e10 } else if x < 0.0 { -1.0e10 } else { 0.0 };
            format!("{sentinel}.0")
        }
    };

    let _ = writeln!(s, "0, {}, {REV}, 0, 0, 60.00   / caseio export: {}", net.base_mva, net.name);
    let _ = writeln!(s, "{}", net.name);
    let _ = writeln!(s);

    // Bus, with area/zone kept for the load records that reference them.
    let mut bus_area: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    for b in &net.buses {
        bus_area.insert(b.id, (b.area, b.zone));
        let name = b.name.as_deref().unwrap_or("");
        let _ = writeln!(
            s,
            "{}, '{:<12}', {}, {}, {}, {}, 1, {}, {}, {}, {}, {}, {}",
            b.id, name, num(b.base_kv), ide(b.kind), b.area, b.zone,
            num(b.vm), num(b.va), num(b.vmax), num(b.vmin), num(b.vmax), num(b.vmin)
        );
    }
    let _ = writeln!(s, "0 / END OF BUS DATA, BEGIN LOAD DATA");

    for l in &net.loads {
        let (area, zone) = bus_area.get(&l.bus).copied().unwrap_or((1, 1));
        let _ = writeln!(
            s,
            "{}, '1', {}, {}, {}, {}, {}, 0, 0, 0, 0, 1, 1, 0",
            l.bus, i32::from(l.in_service), area, zone, num(l.p), num(l.q)
        );
    }
    let _ = writeln!(s, "0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA");

    for sh in &net.shunts {
        let _ = writeln!(
            s,
            "{}, '1', {}, {}, {}",
            sh.bus, i32::from(sh.in_service), num(sh.g), num(sh.b)
        );
    }
    let _ = writeln!(s, "0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA");

    for g in &net.generators {
        let _ = writeln!(
            s,
            "{}, '1', {}, {}, {}, {}, {}, 0, {}, 0, 1, 0, 0, 1, {}, 100, {}, {}, 1, 1",
            g.bus, num(g.pg), num(g.qg), num(g.qmax), num(g.qmin), num(g.vg),
            num(g.mbase), i32::from(g.in_service), num(g.pmax), num(g.pmin)
        );
    }
    let _ = writeln!(s, "0 / END OF GENERATOR DATA, BEGIN BRANCH DATA");

    // Non-transformer branches here; transformers go in their own section.
    for br in net.branches.iter().filter(|b| !b.is_transformer()) {
        let _ = writeln!(
            s,
            "{}, {}, '1', {}, {}, {}, {}, {}, {}, 0, 0, 0, 0, {}, 1, 0, 1, 1",
            br.from, br.to, num(br.r), num(br.x), num(br.b),
            num(br.rate_a), num(br.rate_b), num(br.rate_c), i32::from(br.in_service)
        );
    }
    let _ = writeln!(s, "0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA");

    for br in net.branches.iter().filter(|b| b.is_transformer()) {
        // 2-winding, 4-line record. CW=1 (turns ratio p.u.), CZ=1 (Z on system
        // base). Record 1 carries the full owner block (O1..O4,F1..F4) and the
        // VECGRP string: PSS/E v33 readers count a 2-winding transformer as a
        // fixed 43-field record (21 + 3 + 17 + 2), so the owner padding matters.
        let _ = writeln!(
            s,
            "{}, {}, 0, '1', 1, 1, 1, 0, 0, 2, '            ', {}, 1, 1, 0, 1, 0, 1, 0, 1, '            '",
            br.from, br.to, i32::from(br.in_service)
        );
        let _ = writeln!(s, "{}, {}, {}", num(br.r), num(br.x), net.base_mva);
        let _ = writeln!(
            s,
            "{}, 0, {}, {}, {}, {}, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0",
            num(br.effective_tap()), num(br.shift), num(br.rate_a), num(br.rate_b), num(br.rate_c)
        );
        let _ = writeln!(s, "1.0, 0");
    }
    let _ = writeln!(s, "0 / END OF TRANSFORMER DATA, BEGIN AREA DATA");

    // The remaining sections are not modeled; emit their terminators in order so
    // the file is a valid v33 case.
    for line in EMPTY_SECTIONS {
        let _ = writeln!(s, "{line}");
    }
    let _ = writeln!(s, "Q");

    if !net.hvdc.is_empty() {
        warnings.push(format!("{} dcline(s) dropped: PSS/E HVDC not modeled", net.hvdc.len()));
    }
    if !net.storage.is_empty() {
        warnings.push(format!("{} storage unit(s) dropped: PSS/E has no storage record", net.storage.len()));
    }
    if net.generators.iter().any(|g| g.cost.is_some()) {
        warnings.push("generator cost curves dropped: PSS/E .raw has no cost data".into());
    }
    if nonfinite {
        warnings.push("non-finite values written as ±1e10 sentinels (PSS/E has no Inf/NaN)".into());
    }

    Conversion { text: s, warnings }
}

/// MATPOWER/neutral bus kind → PSS/E bus type code (IDE).
fn ide(kind: BusType) -> u8 {
    kind as u8 // 1=PQ, 2=PV, 3=ref/swing, 4=isolated — same codes
}

const EMPTY_SECTIONS: [&str; 13] = [
    "0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA",
    "0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA",
    "0 / END OF VSC DC LINE DATA, BEGIN IMPEDANCE CORRECTION DATA",
    "0 / END OF IMPEDANCE CORRECTION DATA, BEGIN MULTI-TERMINAL DC DATA",
    "0 / END OF MULTI-TERMINAL DC DATA, BEGIN MULTI-SECTION LINE DATA",
    "0 / END OF MULTI-SECTION LINE DATA, BEGIN ZONE DATA",
    "0 / END OF ZONE DATA, BEGIN INTER-AREA TRANSFER DATA",
    "0 / END OF INTER-AREA TRANSFER DATA, BEGIN OWNER DATA",
    "0 / END OF OWNER DATA, BEGIN FACTS DEVICE DATA",
    "0 / END OF FACTS DEVICE DATA, BEGIN SWITCHED SHUNT DATA",
    "0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA",
    "0 / END OF GNE DEVICE DATA, BEGIN INDUCTION MACHINE DATA",
    "0 / END OF INDUCTION MACHINE DATA",
];

// ---- Reader -----------------------------------------------------------------

/// Parse a PSS/E v33 `.raw` into a [`Network`]. Reads bus/load/fixed-shunt/
/// generator/branch/2-winding-transformer; skips the advanced sections.
pub fn parse_psse(content: &str) -> Result<Network> {
    let mut lines = content.lines();

    // Header line 1: IC, SBASE, REV, ...
    let header = lines.next().ok_or_else(|| Error::FormatRead {
        format: FMT,
        message: "empty file".into(),
    })?;
    let base_mva = fields(header)
        .get(1)
        .and_then(|f| f.parse::<f64>().ok())
        .ok_or_else(|| Error::FormatRead { format: FMT, message: "missing SBASE in header".into() })?;
    // Line 2 is the case title; we write the network name there, so read it back.
    let title = lines.next().unwrap_or("").trim();
    let name = if title.is_empty() { "case".to_string() } else { title.to_string() };
    lines.next(); // line 3: second comment

    let mut buses = Vec::new();
    let mut loads = Vec::new();
    let mut shunts = Vec::new();
    let mut generators = Vec::new();
    let mut branches = Vec::new();

    // Sections appear in fixed order, each ended by a record whose first field is
    // `0`. We read the ones we model and treat the rest as skipped.
    let mut section = Section::Bus;
    let mut lines = lines.peekable();
    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line == "Q" {
            break;
        }
        if is_terminator(line) {
            section = section.next();
            continue;
        }
        let f = fields(line);
        match section {
            Section::Bus => buses.push(read_bus(&f)?),
            Section::Load => loads.push(read_load(&f)),
            Section::FixedShunt => shunts.push(read_shunt(&f)),
            Section::Generator => generators.push(read_gen(&f)),
            Section::Branch => branches.push(read_branch(&f)),
            Section::Transformer => {
                // 2-winding = 4 lines (K field == 0); 3-winding = 5 lines (skip).
                let two_winding = f.get(2).and_then(|x| x.parse::<i64>().ok()) == Some(0);
                let l2 = lines.next().map_or("", str::trim);
                let l3 = lines.next().map_or("", str::trim);
                let l4 = lines.next().map_or("", str::trim);
                if two_winding {
                    branches.push(read_transformer(&f, &fields(l2), &fields(l3), &fields(l4)));
                } else {
                    // 3-winding: consume its 5th line and skip (not modeled).
                    lines.next();
                }
            }
            Section::Skip => {}
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
        storage: Vec::new(),
        hvdc: Vec::new(),
        source_format: SourceFormat::Psse,
        source: Some(Arc::from(content)),
    };
    net.check_references(FMT)?;
    Ok(net)
}

#[derive(Clone, Copy)]
enum Section {
    Bus,
    Load,
    FixedShunt,
    Generator,
    Branch,
    Transformer,
    Skip,
}

impl Section {
    fn next(self) -> Self {
        match self {
            Section::Bus => Section::Load,
            Section::Load => Section::FixedShunt,
            Section::FixedShunt => Section::Generator,
            Section::Generator => Section::Branch,
            Section::Branch => Section::Transformer,
            // Everything past transformers is skipped.
            Section::Transformer | Section::Skip => Section::Skip,
        }
    }
}

/// A record line's first field is `0` (the section terminator).
fn is_terminator(line: &str) -> bool {
    fields(line).first().map(String::as_str) == Some("0")
}

/// Split a PSS/E record into trimmed, unquoted fields, dropping a trailing
/// `/comment`. Comma-delimited records keep empty fields (column position is
/// significant — a blank quoted name must not shift later columns); records with
/// no commas fall back to whitespace splitting.
fn fields(line: &str) -> Vec<String> {
    let code = line.split('/').next().unwrap_or(line);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let comma_delimited = code.contains(',');
    for c in code.chars() {
        match c {
            '\'' => quoted = !quoted,
            ',' if !quoted && comma_delimited => out.push(std::mem::take(&mut cur).trim().to_string()),
            c if c.is_whitespace() && !quoted && !comma_delimited => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    let last = cur.trim().to_string();
    if comma_delimited || !last.is_empty() {
        out.push(last);
    }
    out
}

fn num_at(f: &[String], i: usize) -> f64 {
    f.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0)
}
fn id_at(f: &[String], i: usize) -> usize {
    f.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) as usize
}
fn on_at(f: &[String], i: usize) -> bool {
    f.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(1.0) != 0.0
}

fn bustype(code: i64) -> BusType {
    match code {
        2 => BusType::Pv,
        3 => BusType::Ref,
        4 => BusType::Isolated,
        _ => BusType::Pq,
    }
}

fn read_bus(f: &[String]) -> Result<Bus> {
    // I, NAME, BASKV, IDE, AREA, ZONE, OWNER, VM, VA, NVHI, NVLO, EVHI, EVLO
    let id = f.first().and_then(|x| x.parse::<f64>().ok()).ok_or_else(|| Error::FormatRead {
        format: FMT,
        message: "bus record missing numeric id (field I)".into(),
    })? as usize;
    let name = f.get(1).filter(|n| !n.is_empty()).map(|n| n.trim().to_string());
    Ok(Bus {
        id,
        kind: bustype(f.get(3).and_then(|x| x.parse().ok()).unwrap_or(1)),
        vm: f.get(7).and_then(|x| x.parse().ok()).unwrap_or(1.0),
        va: num_at(f, 8),
        base_kv: num_at(f, 2),
        vmax: f.get(9).and_then(|x| x.parse().ok()).unwrap_or(1.1),
        vmin: f.get(10).and_then(|x| x.parse().ok()).unwrap_or(0.9),
        area: id_at(f, 4),
        zone: id_at(f, 5),
        name,
        extras: Extras::new(),
    })
}

fn read_load(f: &[String]) -> Load {
    // I, ID, STATUS, AREA, ZONE, PL, QL, ...
    Load { bus: id_at(f, 0), p: num_at(f, 5), q: num_at(f, 6), in_service: on_at(f, 2), extras: Extras::new() }
}

fn read_shunt(f: &[String]) -> Shunt {
    // I, ID, STATUS, GL, BL
    Shunt { bus: id_at(f, 0), g: num_at(f, 3), b: num_at(f, 4), in_service: on_at(f, 2), extras: Extras::new() }
}

fn read_gen(f: &[String]) -> Generator {
    // I, ID, PG, QG, QT, QB, VS, IREG, MBASE, ..., STAT(14), ..., PT(16), PB(17)
    Generator {
        bus: id_at(f, 0),
        pg: num_at(f, 2),
        qg: num_at(f, 3),
        qmax: num_at(f, 4),
        qmin: num_at(f, 5),
        vg: f.get(6).and_then(|x| x.parse().ok()).unwrap_or(1.0),
        mbase: f.get(8).and_then(|x| x.parse().ok()).unwrap_or(100.0),
        in_service: on_at(f, 14),
        pmax: num_at(f, 16),
        pmin: num_at(f, 17),
        cost: None,
        extras: Extras::new(),
    }
}

fn read_branch(f: &[String]) -> Branch {
    // I, J, CKT, R, X, B, RATEA, RATEB, RATEC, GI,BI,GJ,BJ, ST(13)
    Branch {
        from: id_at(f, 0),
        to: id_at(f, 1),
        r: num_at(f, 3),
        x: num_at(f, 4),
        b: num_at(f, 5),
        rate_a: num_at(f, 6),
        rate_b: num_at(f, 7),
        rate_c: num_at(f, 8),
        tap: 0.0,
        shift: 0.0,
        in_service: on_at(f, 13),
        angmin: -360.0,
        angmax: 360.0,
        extras: Extras::new(),
    }
}

fn read_transformer(l1: &[String], l2: &[String], l3: &[String], _l4: &[String]) -> Branch {
    // l1: I, J, K, CKT, CW, CZ, CM, MAG1, MAG2, NMETR, NAME, STAT(11)
    // l2: R1-2, X1-2, SBASE1-2
    // l3: WINDV1, NOMV1, ANG1, RATA1, RATB1, RATC1, ...
    let tap = l3.first().and_then(|x| x.parse().ok()).unwrap_or(1.0);
    Branch {
        from: id_at(l1, 0),
        to: id_at(l1, 1),
        r: num_at(l2, 0),
        x: num_at(l2, 1),
        b: 0.0,
        rate_a: num_at(l3, 3),
        rate_b: num_at(l3, 4),
        rate_c: num_at(l3, 5),
        tap,
        shift: num_at(l3, 2),
        in_service: on_at(l1, 11),
        angmin: -360.0,
        angmax: 360.0,
        extras: Extras::new(),
    }
}
