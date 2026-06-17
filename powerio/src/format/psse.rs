//! Read and write PSS/E `.raw` (revision 33).
//!
//! Covers the core sections — bus, load, fixed shunt, generator, branch, and
//! 2-winding transformer — which together carry a transmission power flow case.
//! A switched shunt is read as a fixed shunt at its steady-state susceptance
//! `BINIT` (the same reduction PowerModels makes); the block/step control detail
//! is not modeled. Impedances are written on the system base with per-unit turns
//! ratios (`CZ = 1`, `CW = 1`); the reader assumes the same and does not convert
//! other impedance/turns bases — a non-unit `CZ`/`CW` is read verbatim (so
//! misread). 3-winding transformers, two-terminal DC, and the other advanced
//! sections are not modeled: on write they're emitted as empty sections, on read
//! they're skipped, and HVDC/storage carried on the `Network` are reported as
//! dropped. Same-format round-trip is byte-exact via the retained source (see
//! [`crate::write_as`]); this serializer is the cross-format path.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use super::{Conversion, sanitize_quoted};
use crate::network::{
    Branch, Bus, BusId, BusType, Extras, Generator, Load, Network, Shunt, SourceFormat,
};
use crate::{Error, Result};

const FMT: &str = "PSS/E .raw";
const REV: u32 = 33;

/// Characters that would corrupt a single-quoted PSS/E name field. The quote
/// toggles the reader's quoted state early, and `/` truncates the record at the
/// inline-comment delimiter (a PSS/E record splits on `/` before tokenizing).
const NAME_FORBIDDEN: &[char] = &['\'', '/'];

// ---- Writer -----------------------------------------------------------------

#[must_use]
// A flat serializer: one stanza per PSS/E record type; splitting it would add
// indirection without clarity.
#[expect(clippy::too_many_lines)]
pub fn write_psse(net: &Network) -> Conversion {
    let mut warnings = Vec::new();
    let mut nonfinite = false;
    let mut sanitized_names = 0usize;
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
            let sentinel = if x > 0.0 {
                1.0e10
            } else if x < 0.0 {
                -1.0e10
            } else {
                0.0
            };
            format!("{sentinel}.0")
        }
    };

    let _ = writeln!(
        s,
        "0, {}, {REV}, 0, 0, {}   / powerio export: {}",
        net.base_mva,
        num(net.base_frequency),
        net.name
    );
    let _ = writeln!(s, "{}", net.name);
    let _ = writeln!(s);

    // Bus, with area/zone kept for the load records that reference them.
    let mut bus_area: BTreeMap<BusId, (usize, usize)> = BTreeMap::new();
    for b in &net.buses {
        bus_area.insert(b.id, (b.area, b.zone));
        let raw_name = b.name.as_deref().unwrap_or("");
        let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
        if matches!(name, std::borrow::Cow::Owned(_)) {
            sanitized_names += 1;
        }
        let _ = writeln!(
            s,
            "{}, '{:<12}', {}, {}, {}, {}, 1, {}, {}, {}, {}, {}, {}",
            b.id,
            name,
            num(b.base_kv),
            ide(b.kind),
            b.area,
            b.zone,
            num(b.vm),
            num(b.va),
            num(b.vmax),
            num(b.vmin),
            num(b.vmax),
            num(b.vmin)
        );
    }
    let _ = writeln!(s, "0 / END OF BUS DATA, BEGIN LOAD DATA");

    for l in &net.loads {
        let (area, zone) = bus_area.get(&l.bus).copied().unwrap_or((1, 1));
        let _ = writeln!(
            s,
            "{}, '1', {}, {}, {}, {}, {}, 0, 0, 0, 0, 1, 1, 0",
            l.bus,
            i32::from(l.in_service),
            area,
            zone,
            num(l.p),
            num(l.q)
        );
    }
    let _ = writeln!(s, "0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA");

    for sh in &net.shunts {
        let _ = writeln!(
            s,
            "{}, '1', {}, {}, {}",
            sh.bus,
            i32::from(sh.in_service),
            num(sh.g),
            num(sh.b)
        );
    }
    let _ = writeln!(s, "0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA");

    for g in &net.generators {
        let _ = writeln!(
            s,
            "{}, '1', {}, {}, {}, {}, {}, 0, {}, 0, 1, 0, 0, 1, {}, 100, {}, {}, 1, 1",
            g.bus,
            num(g.pg),
            num(g.qg),
            num(g.qmax),
            num(g.qmin),
            num(g.vg),
            num(g.mbase),
            i32::from(g.in_service),
            num(g.pmax),
            num(g.pmin)
        );
    }
    let _ = writeln!(s, "0 / END OF GENERATOR DATA, BEGIN BRANCH DATA");

    // Non-transformer branches here; transformers go in their own section.
    for br in net.branches.iter().filter(|b| !b.is_transformer()) {
        let _ = writeln!(
            s,
            "{}, {}, '1', {}, {}, {}, {}, {}, {}, 0, 0, 0, 0, {}, 1, 0, 1, 1",
            br.from,
            br.to,
            num(br.r),
            num(br.x),
            num(br.b),
            num(br.rate_a),
            num(br.rate_b),
            num(br.rate_c),
            i32::from(br.in_service)
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
            br.from,
            br.to,
            i32::from(br.in_service)
        );
        let _ = writeln!(s, "{}, {}, {}", num(br.r), num(br.x), net.base_mva);
        let _ = writeln!(
            s,
            "{}, 0, {}, {}, {}, {}, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0",
            num(br.effective_tap()),
            num(br.shift),
            num(br.rate_a),
            num(br.rate_b),
            num(br.rate_c)
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
        warnings.push(format!(
            "{} dcline(s) dropped: PSS/E HVDC not modeled",
            net.hvdc.len()
        ));
    }
    if !net.storage.is_empty() {
        warnings.push(format!(
            "{} storage unit(s) dropped: PSS/E has no storage record",
            net.storage.len()
        ));
    }
    if net.generators.iter().any(|g| g.cost.is_some()) {
        warnings.push("generator cost curves dropped: PSS/E .raw has no cost data".into());
    }
    if net.branches.iter().any(Branch::has_angle_limits) {
        warnings.push(
            "branch angle limits (angmin/angmax) dropped: PSS/E branch records carry none".into(),
        );
    }
    if net.generators.iter().any(Generator::has_caps) {
        warnings.push(
            "generator ramp/capability columns dropped: PSS/E .raw has no equivalent fields".into(),
        );
    }
    if nonfinite {
        warnings.push("non-finite values written as ±1e10 sentinels (PSS/E has no Inf/NaN)".into());
    }
    if sanitized_names > 0 {
        warnings.push(format!(
            "{sanitized_names} bus name(s) contained a quote or '/' that would corrupt a PSS/E \
             record; replaced with spaces"
        ));
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
    parse_psse_source(Arc::new(content.to_owned()), None)
}

/// Owned-source entry used by the format hub: parse by borrowing `source`, then
/// move the buffer into the retained source (no copy). `name_hint` (e.g. a file
/// stem) names the network when the title line is blank.
// A flat reader: header parse plus one match arm per section. Splitting it would
// add indirection without clarity.
#[expect(clippy::too_many_lines)]
pub(crate) fn parse_psse_source(source: Arc<String>, name_hint: Option<&str>) -> Result<Network> {
    let content: &str = &source;
    let mut lines = content.lines();

    // Header line 1: IC, SBASE, REV, ...
    let header = lines
        .by_ref()
        .find(|line| {
            let line = line.trim();
            !line.is_empty() && !is_comment(line)
        })
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "empty file".into(),
        })?;
    let header_fields = fields(header);
    let base_mva = header_fields
        .get(1)
        .and_then(|f| f.parse::<f64>().ok())
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "missing SBASE in header".into(),
        })?;
    let raw_rev = header_fields
        .get(2)
        .and_then(|f| f.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map_or(33, |v| v as u32);
    // BASFRQ is the sixth header field (IC, SBASE, REV, XFRRAT, NXFRAT, BASFRQ);
    // older revisions that carry only `SBASE, title` lack it, so default it.
    let base_frequency = header_fields
        .get(5)
        .and_then(|f| f.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(crate::network::DEFAULT_BASE_FREQUENCY);
    // Line 2 is the case title; we write the network name there, so read it back.
    let title = lines.next().unwrap_or("").trim();
    let name = if title.is_empty() {
        name_hint.unwrap_or("case").to_string()
    } else {
        title.to_string()
    };
    lines.next(); // line 3: second comment

    let mut buses = Vec::new();
    let mut loads = Vec::new();
    let mut shunts = Vec::new();
    let mut generators = Vec::new();
    let mut branches = Vec::new();

    // Sections appear in fixed order, each ended by a record whose first field is
    // `0`. We read the ones we model and treat the rest as skipped.
    let mut section = Section::Bus;
    let mut saw_bus_marker = false;
    let mut lines = lines.peekable();
    while let Some(raw) = lines.next() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if is_comment(line) {
            continue;
        }
        if line == "Q" {
            break;
        }
        if is_terminator(line) {
            // The terminator names the section that begins next ("…, BEGIN
            // SWITCHED SHUNT DATA"); read that rather than counting, so the many
            // unmodeled sections between transformers and switched shunts don't
            // throw off the position.
            section = section_after_marker(line);
            saw_bus_marker |= matches!(section, Section::Bus);
            continue;
        }
        let f = fields(line);
        match section {
            Section::Bus if !saw_bus_marker && buses.is_empty() && is_system_wide_record(&f) => {
                section = Section::Skip;
            }
            Section::Bus => buses.push(read_bus(&f)?),
            Section::Load => loads.push(read_load(&f)?),
            Section::FixedShunt => shunts.push(read_shunt(&f)?),
            Section::SwitchedShunt => shunts.push(read_switched_shunt(&f)?),
            Section::Generator => generators.push(read_gen(&f)?),
            Section::Branch => branches.push(read_branch(&f, raw_rev)?),
            Section::Transformer => {
                // 2-winding = 4 lines (K field == 0); 3-winding = 5 lines (skip).
                let two_winding = f.get(2).and_then(|x| x.parse::<i64>().ok()) == Some(0);
                let l2 = lines.next().map_or("", str::trim);
                let l3 = lines.next().map_or("", str::trim);
                let l4 = lines.next().map_or("", str::trim);
                if two_winding {
                    branches.push(read_transformer(&f, &fields(l2), &fields(l3), &fields(l4))?);
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
        base_frequency,
        buses,
        loads,
        shunts,
        branches,
        generators,
        storage: Vec::new(),
        hvdc: Vec::new(),
        source_format: SourceFormat::Psse,
        source: Some(source),
    };
    net.check_references(FMT)?;
    Ok(net)
}

#[derive(Clone, Copy)]
enum Section {
    Bus,
    Load,
    FixedShunt,
    SwitchedShunt,
    Generator,
    Branch,
    Transformer,
    Skip,
}

/// The section a `BEGIN <name> DATA` terminator introduces. Sections we don't
/// model map to [`Section::Skip`]. Case-insensitive on the marker text, so the
/// number of skipped sections between the modeled ones doesn't matter.
fn section_after_marker(line: &str) -> Section {
    let u = line.to_ascii_uppercase();
    if u.contains("BEGIN BUS DATA") {
        Section::Bus
    } else if u.contains("BEGIN LOAD DATA") {
        Section::Load
    } else if u.contains("BEGIN FIXED SHUNT DATA") {
        Section::FixedShunt
    } else if u.contains("BEGIN SWITCHED SHUNT DATA") {
        Section::SwitchedShunt
    } else if u.contains("BEGIN GENERATOR DATA") {
        Section::Generator
    } else if u.contains("BEGIN BRANCH DATA") {
        Section::Branch
    } else if u.contains("BEGIN TRANSFORMER DATA") {
        Section::Transformer
    } else {
        Section::Skip
    }
}

/// A record line's first field is `0` (the section terminator).
fn is_terminator(line: &str) -> bool {
    fields(line).first().map(String::as_str) == Some("0")
}

fn is_comment(line: &str) -> bool {
    line.starts_with("@!") || line.starts_with('@')
}

fn is_system_wide_record(f: &[String]) -> bool {
    matches!(
        f.first().map(|s| s.to_ascii_uppercase()),
        Some(first) if matches!(first.as_str(), "GENERAL" | "RATING")
    )
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
            ',' if !quoted && comma_delimited => {
                out.push(std::mem::take(&mut cur).trim().to_string());
            }
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

fn bad_field(i: usize, tok: &str) -> Error {
    Error::FormatRead {
        format: FMT,
        message: format!("field {i} {tok:?} is not a number"),
    }
}

/// Field `i` as f64. Absent or empty → `default` (a genuinely optional column).
/// Present but unparseable → a hard error: a malformed number must not silently
/// become a plausible default (e.g. a garbled reactance collapsing to 0.0, which
/// would drop the branch from every matrix) and corrupt the result.
fn num_at(f: &[String], i: usize, default: f64) -> Result<f64> {
    match f.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(s) => s.parse().map_err(|_| bad_field(i, s)),
    }
}
/// Field `i` as a bus id (parsed as f64 then truncated, the PSS/E convention).
fn id_at(f: &[String], i: usize, default: usize) -> Result<usize> {
    match f.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(s) => s
            .parse::<f64>()
            .map(|v| v as usize)
            .map_err(|_| bad_field(i, s)),
    }
}
/// Field `i` as a status flag (nonzero = in service).
fn on_at(f: &[String], i: usize, default: bool) -> Result<bool> {
    match f.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(s) => s
            .parse::<f64>()
            .map(|v| v != 0.0)
            .map_err(|_| bad_field(i, s)),
    }
}
/// Field `i` as an integer code (bus type, etc.).
fn int_at(f: &[String], i: usize, default: i64) -> Result<i64> {
    match f.get(i).map(String::as_str) {
        None | Some("") => Ok(default),
        Some(s) => s.parse().map_err(|_| bad_field(i, s)),
    }
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
    let id = f
        .first()
        .and_then(|x| x.parse::<f64>().ok())
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: "bus record missing numeric id (field I)".into(),
        })? as usize;
    let name = f
        .get(1)
        .filter(|n| !n.is_empty())
        .map(|n| n.trim().to_string());
    Ok(Bus {
        id: BusId(id),
        kind: bustype(int_at(f, 3, 1)?),
        vm: num_at(f, 7, 1.0)?,
        va: num_at(f, 8, 0.0)?,
        base_kv: num_at(f, 2, 0.0)?,
        vmax: num_at(f, 9, 1.1)?,
        vmin: num_at(f, 10, 0.9)?,
        area: id_at(f, 4, 0)?,
        zone: id_at(f, 5, 0)?,
        name,
        extras: Extras::new(),
    })
}

fn read_load(f: &[String]) -> Result<Load> {
    // I, ID, STATUS, AREA, ZONE, PL, QL, ...
    Ok(Load {
        bus: BusId(id_at(f, 0, 0)?),
        p: num_at(f, 5, 0.0)?,
        q: num_at(f, 6, 0.0)?,
        in_service: on_at(f, 2, true)?,
        extras: Extras::new(),
    })
}

fn read_shunt(f: &[String]) -> Result<Shunt> {
    // I, ID, STATUS, GL, BL
    Ok(Shunt {
        bus: BusId(id_at(f, 0, 0)?),
        g: num_at(f, 3, 0.0)?,
        b: num_at(f, 4, 0.0)?,
        in_service: on_at(f, 2, true)?,
        extras: Extras::new(),
    })
}

fn read_switched_shunt(f: &[String]) -> Result<Shunt> {
    // I, MODSW, ADJM, STAT, VSWHI, VSWLO, SWREM, RMPCT, RMIDNT, BINIT(9), N1, B1, ...
    // Model the steady-state susceptance BINIT as a fixed shunt (gs = 0), the same
    // reduction PowerModels makes; the block/step control detail isn't modeled.
    Ok(Shunt {
        bus: BusId(id_at(f, 0, 0)?),
        g: 0.0,
        b: num_at(f, 9, 0.0)?,
        in_service: on_at(f, 3, true)?,
        extras: Extras::new(),
    })
}

fn read_gen(f: &[String]) -> Result<Generator> {
    // I, ID, PG, QG, QT, QB, VS, IREG, MBASE, ..., STAT(14), ..., PT(16), PB(17)
    Ok(Generator {
        bus: BusId(id_at(f, 0, 0)?),
        pg: num_at(f, 2, 0.0)?,
        qg: num_at(f, 3, 0.0)?,
        qmax: num_at(f, 4, 0.0)?,
        qmin: num_at(f, 5, 0.0)?,
        vg: num_at(f, 6, 1.0)?,
        mbase: num_at(f, 8, 100.0)?,
        in_service: on_at(f, 14, true)?,
        pmax: num_at(f, 16, 0.0)?,
        pmin: num_at(f, 17, 0.0)?,
        cost: None,
        caps: Default::default(),
    })
}

fn read_branch(f: &[String], raw_rev: u32) -> Result<Branch> {
    // v33: I, J, CKT, R, X, B, RATEA, RATEB, RATEC, GI,BI,GJ,BJ, ST(13)
    // v34 exports insert NAME before twelve rating columns, putting STAT after
    // GI/BI/GJ/BJ. v33 can still have a long owner/fraction tail, so the RAW
    // revision, not RATEA parseability, decides the long named layout.
    let named_record = raw_rev >= 34 && f.len() >= 24;
    let rating = if named_record { 7 } else { 6 };
    let status = if named_record { 23 } else { 13 };
    Ok(Branch {
        from: BusId(id_at(f, 0, 0)?),
        to: BusId(id_at(f, 1, 0)?),
        r: num_at(f, 3, 0.0)?,
        x: num_at(f, 4, 0.0)?,
        b: num_at(f, 5, 0.0)?,
        rate_a: num_at(f, rating, 0.0)?,
        rate_b: num_at(f, rating + 1, 0.0)?,
        rate_c: num_at(f, rating + 2, 0.0)?,
        tap: 0.0,
        shift: 0.0,
        in_service: on_at(f, status, true)?,
        angmin: -360.0,
        angmax: 360.0,
        extras: Extras::new(),
    })
}

fn read_transformer(l1: &[String], l2: &[String], l3: &[String], _l4: &[String]) -> Result<Branch> {
    // l1: I, J, K, CKT, CW, CZ, CM, MAG1, MAG2, NMETR, NAME, STAT(11)
    // l2: R1-2, X1-2, SBASE1-2
    // l3: WINDV1, NOMV1, ANG1, RATA1, RATB1, RATC1, ...
    Ok(Branch {
        from: BusId(id_at(l1, 0, 0)?),
        to: BusId(id_at(l1, 1, 0)?),
        r: num_at(l2, 0, 0.0)?,
        x: num_at(l2, 1, 0.0)?,
        b: 0.0,
        rate_a: num_at(l3, 3, 0.0)?,
        rate_b: num_at(l3, 4, 0.0)?,
        rate_c: num_at(l3, 5, 0.0)?,
        tap: num_at(l3, 0, 1.0)?,
        shift: num_at(l3, 2, 0.0)?,
        in_service: on_at(l1, 11, true)?,
        angmin: -360.0,
        angmax: 360.0,
        extras: Extras::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn reads_comment_headers_system_wide_block_and_named_branch_records() {
        let raw = r#"@!IC, SBASE,REV,XFRRAT,NXFRAT,BASFRQ
0, 100.00, 34, 0, 0, 60.00 / synthetic v34 export


GENERAL, THRSHZ=0.0002
RATING, 1, "      ", "                                "
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
@!   I,'NAME        ', BASKV, IDE,AREA,ZONE,OWNER, VM,        VA,    NVHI,   NVLO,   EVHI,   EVLO
1,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
2,'BUS2        ', 230.0000,1,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / END OF BUS DATA, BEGIN LOAD DATA
@!   I,'ID',STAT,AREA,ZONE,      PL,        QL
2,'1 ',1,1,1,10.0,5.0
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
@!   I,'ID',      PG,        QG,        QT,        QB,     VS,    IREG,     MBASE,     ZR,         ZX,         RT,         XT,     GTAP,STAT, RMPCT,      PT,        PB
1,'1 ',50.0,5.0,20.0,-10.0,1.0,0,100.0,0.0,1.0,0.0,0.0,1.0,1,100.0,80.0,10.0
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
@!   I,     J,'CKT',     R,          X,         B,                    'N A M E'                 ,   RATE1,   RATE2,   RATE3,   RATE4,   RATE5,   RATE6,   RATE7,   RATE8,   RATE9,  RATE10,  RATE11,  RATE12,    GI,       BI,       GJ,       BJ,STAT,MET,  LEN
1,2,'1 ',0.01,0.05,0.001,'named branch',100.0,90.0,80.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,1,1,0.0
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
"#;

        let net = parse_psse(raw).unwrap();

        close(net.base_mva, 100.0);
        assert_eq!(net.buses.len(), 2);
        assert_eq!(net.loads.len(), 1);
        assert_eq!(net.generators.len(), 1);
        assert_eq!(net.branches.len(), 1);
        close(net.branches[0].rate_a, 100.0);
        assert!(net.branches[0].in_service);
    }

    #[test]
    fn v33_long_branch_with_blank_ratea_keeps_v33_columns() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic v33 export
CASE
COMMENT
1,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
2,'BUS2        ', 230.0000,1,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
1,2,'1 ',0.01,0.05,0.001,,90.0,80.0,0.0,0.0,0.0,0.0,1,1,0.0,1,1.0,2,0.0,3,0.0,4,0.0
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";

        let net = parse_psse(raw).unwrap();

        assert_eq!(net.branches.len(), 1);
        close(net.branches[0].rate_a, 0.0);
        close(net.branches[0].rate_b, 90.0);
        close(net.branches[0].rate_c, 80.0);
        assert!(net.branches[0].in_service);
    }

    #[test]
    fn writer_sanitizes_bus_names_that_would_corrupt_a_record() {
        // A name with an apostrophe closes the single-quoted field early; a name
        // with '/' truncates the record at the inline-comment delimiter. Either
        // shifts every later column. The writer replaces both and warns, so the
        // second bus's base kV survives the round trip.
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
2,'BUS2        ', 138.0000,1,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let mut net = parse_psse(raw).unwrap();
        net.buses[0].name = Some("O'Brien/X".to_string());

        let conv = write_psse(&net);
        let reparsed = parse_psse(&conv.text).unwrap();

        assert_eq!(reparsed.buses.len(), 2);
        close(reparsed.buses[0].base_kv, 230.0);
        close(reparsed.buses[1].base_kv, 138.0);
        let name = reparsed.buses[0].name.as_deref().unwrap();
        assert!(!name.contains('\'') && !name.contains('/'), "got {name:?}");
        assert!(
            conv.warnings.iter().any(|w| w.contains("bus name")),
            "expected a sanitization warning, got {:?}",
            conv.warnings
        );
    }

    #[test]
    fn malformed_first_bus_id_is_not_treated_as_system_wide_data() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / synthetic malformed export
CASE
COMMENT
BAD,'BUS1        ', 230.0000,3,1,1,1,1.00000,0.0000,1.1000,0.9000,1.1000,0.9000
0 / END OF BUS DATA, BEGIN LOAD DATA
Q
";

        let err = parse_psse(raw).unwrap_err();

        assert!(
            err.to_string().contains("bus record missing numeric id"),
            "malformed bus id should be reported directly: {err}"
        );
    }
}
